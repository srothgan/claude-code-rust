// SPDX-License-Identifier: Apache-2.0
use crate::app::{UsageSnapshot, UsageSourceKind, UsageWindow};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

const CLI_USAGE_TIMEOUT: Duration = Duration::from_secs(15);

#[cfg(test)]
#[path = "cli_audit_tests.rs"]
mod audit_tests;

#[derive(Clone)]
pub(super) struct Probe {
    pub(super) executable: Option<PathBuf>,
    pub(super) timeout: Duration,
}

impl Probe {
    pub(super) fn system() -> Self {
        Self { executable: None, timeout: CLI_USAGE_TIMEOUT }
    }

    pub(super) async fn fetch_snapshot(&self, cwd_raw: String) -> Result<UsageSnapshot, String> {
        let claude_path = self.executable.clone().map_or_else(resolve_claude_path, Ok)?;
        let mut child = tokio::process::Command::new(&claude_path)
            .args(["/usage", "--allowed-tools", ""])
            .current_dir(cwd_raw)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| {
                format!("Failed to run `claude /usage` ({}): {error}", claude_path.display())
            })?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let result = tokio::time::timeout(self.timeout, async {
            tokio::try_join!(child.wait(), read_pipe(stdout), read_pipe(stderr))
        })
        .await
        .map_err(|_| "Claude CLI usage probe timed out.".to_owned())
        .and_then(|result| {
            result.map_err(|error| format!("Failed to read Claude CLI usage output: {error}"))
        });
        let (status, stdout, stderr) = match result {
            Ok(output) => output,
            Err(message) => {
                // Keep ownership through cancellation: kill() also waits for the child.
                child
                    .kill()
                    .await
                    .map_err(|error| format!("Failed to reap Claude CLI usage probe: {error}"))?;
                return Err(message);
            }
        };
        let combined = combine_output(&stdout, &stderr);
        if !status.success() {
            let code = status.code().map_or_else(|| "unknown".into(), |code| code.to_string());
            return Err(format!(
                "`claude /usage` failed with exit code {code}: {}",
                combined.trim().replace('\n', " ")
            ));
        }
        parse_usage_output(&combined)
    }
}

async fn read_pipe(pipe: Option<impl tokio::io::AsyncRead + Unpin>) -> std::io::Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;
    let mut bytes = Vec::new();
    if let Some(mut pipe) = pipe {
        pipe.read_to_end(&mut bytes).await?;
    }
    Ok(bytes)
}

fn resolve_claude_path() -> Result<PathBuf, String> {
    which::which("claude").map_err(|_| "claude CLI not found in PATH.".to_owned())
}

fn combine_output(stdout: &[u8], stderr: &[u8]) -> String {
    let mut text = String::from_utf8_lossy(stdout).into_owned();
    let stderr_text = String::from_utf8_lossy(stderr);
    if !stderr_text.trim().is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&stderr_text);
    }
    text
}

fn parse_usage_output(text: &str) -> Result<UsageSnapshot, String> {
    let clean = strip_ansi(text);
    let trimmed = clean.trim();
    if trimmed.is_empty() {
        return Err("Claude CLI usage output was empty.".to_owned());
    }
    let panel = trim_to_latest_usage_panel(trimmed).unwrap_or(trimmed);
    if let Some(error) = extract_usage_error(panel) {
        return Err(error);
    }

    let five_hour = extract_window(panel, &["Current session"], "5-hour")
        .ok_or_else(|| "Could not parse Claude usage: missing Current session.".to_owned())?;
    let seven_day = extract_window(panel, &["Current week (all models)"], "7-day");
    let seven_day_sonnet = extract_window(
        panel,
        &["Current week (Sonnet only)", "Current week (Sonnet)"],
        "7-day Sonnet",
    );
    let seven_day_opus = extract_window(panel, &["Current week (Opus)"], "7-day Opus");

    Ok(UsageSnapshot {
        source: UsageSourceKind::Cli,
        fetched_at: SystemTime::now(),
        subscription_type: None,
        five_hour: Some(five_hour),
        seven_day,
        seven_day_oauth_apps: None,
        seven_day_opus,
        seven_day_sonnet,
        model_scoped: Vec::new(),
        extra_usage: None,
        session: None,
        activity: None,
    })
}

fn extract_window(text: &str, labels: &[&str], window_label: &str) -> Option<UsageWindow> {
    let lines = text.lines().collect::<Vec<_>>();
    let normalized_labels =
        labels.iter().map(|label| normalized_for_label_search(label)).collect::<Vec<_>>();

    for (index, line) in lines.iter().enumerate() {
        let normalized_line = normalized_for_label_search(line);
        if !normalized_labels.contains(&normalized_line) {
            continue;
        }

        let window = lines
            .iter()
            .skip(index + 1)
            .copied()
            .take_while(|line| !is_window_heading(line))
            .collect::<Vec<_>>();
        let utilization = window.iter().find_map(|candidate| percent_used_from_line(candidate))?;
        let reset_description = window.iter().find_map(|candidate| {
            let normalized = normalized_for_label_search(candidate);
            if normalized.contains("resets") {
                let trimmed = candidate.trim();
                if trimmed.is_empty() { None } else { Some(trimmed.to_owned()) }
            } else {
                None
            }
        });

        return Some(UsageWindow {
            label: window_label.to_owned(),
            utilization,
            resets_at: None,
            reset_description,
        });
    }

    None
}

fn is_window_heading(line: &str) -> bool {
    matches!(
        normalized_for_label_search(line).as_str(),
        "currentsession"
            | "currentweekallmodels"
            | "currentweeksonnetonly"
            | "currentweeksonnet"
            | "currentweekopus"
    )
}

fn percent_used_from_line(line: &str) -> Option<f64> {
    if is_likely_status_context_line(line) {
        return None;
    }

    let percent_index = line.find('%')?;
    let number = parse_number_before_percent(line, percent_index)?;
    let lower = line.to_ascii_lowercase();
    let used_keywords = ["used", "spent", "consumed"];
    let remaining_keywords = ["left", "remaining", "available"];

    let normalized = if used_keywords.iter().any(|keyword| lower.contains(keyword)) {
        number
    } else if remaining_keywords.iter().any(|keyword| lower.contains(keyword)) {
        100.0 - number
    } else {
        return None;
    };

    Some(normalized.clamp(0.0, 100.0))
}

fn parse_number_before_percent(line: &str, percent_index: usize) -> Option<f64> {
    let prefix = &line[..percent_index];
    let mut collected = String::new();

    for ch in prefix.chars().rev() {
        if ch.is_ascii_digit() || ch == '.' {
            collected.push(ch);
        } else if !collected.is_empty() {
            break;
        }
    }

    if collected.is_empty() {
        return None;
    }

    let number = collected.chars().rev().collect::<String>();
    number.parse::<f64>().ok()
}

fn is_likely_status_context_line(line: &str) -> bool {
    if !line.contains('|') {
        return false;
    }
    let lower = line.to_ascii_lowercase();
    ["opus", "sonnet", "haiku"].iter().any(|token| lower.contains(token))
}

fn extract_usage_error(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let compact = normalized_for_label_search(text);
    if lower.contains("failed to load usage data") || compact.contains("failedtoloadusagedata") {
        return Some(
            "Claude CLI could not load usage data. Open Claude directly and retry `/usage`."
                .to_owned(),
        );
    }
    if lower.contains("rate limited") {
        return Some(
            "Claude CLI usage endpoint is rate limited right now. Please try again later."
                .to_owned(),
        );
    }
    None
}

fn trim_to_latest_usage_panel(text: &str) -> Option<&str> {
    text.rfind("Current session").map(|index| &text[index..])
}

fn normalized_for_label_search(text: &str) -> String {
    text.chars().filter(char::is_ascii_alphanumeric).flat_map(char::to_lowercase).collect()
}

fn strip_ansi(text: &str) -> String {
    enum State {
        Normal,
        Escape,
        Csi,
        Osc,
        OscEscape,
    }

    let mut out = String::with_capacity(text.len());
    let mut state = State::Normal;

    for ch in text.chars() {
        state = match state {
            State::Normal => {
                if ch == '\u{1b}' {
                    State::Escape
                } else {
                    out.push(ch);
                    State::Normal
                }
            }
            State::Escape => match ch {
                '[' => State::Csi,
                ']' => State::Osc,
                _ => State::Normal,
            },
            State::Csi => {
                if ('\u{40}'..='\u{7e}').contains(&ch) {
                    State::Normal
                } else {
                    State::Csi
                }
            }
            State::Osc => match ch {
                '\u{07}' => State::Normal,
                '\u{1b}' => State::OscEscape,
                _ => State::Osc,
            },
            State::OscEscape => {
                if ch == '\\' {
                    State::Normal
                } else {
                    State::Osc
                }
            }
        };
    }

    out
}
