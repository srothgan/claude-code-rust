// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::{doctor, redaction};
use crate::{Cli, LogsArgs};
use anyhow::Context as _;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions, create_dir_all, rename};
use std::io::{IsTerminal as _, Write};
use std::path::{Path, PathBuf};
use time::{OffsetDateTime, macros::format_description};
use zip::CompressionMethod;
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

const BUNDLE_SCHEMA: &str = "claude-rs-debug-bundle/v1";
const BUNDLE_RUNTIME_LOG_LIMIT: usize = 5;

pub fn run(
    cli: &Cli,
    args: &LogsArgs,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> anyhow::Result<i32> {
    let paths = crate::logging::default_diagnostics_paths()?;

    if args.path {
        writeln!(stdout, "{}", paths.runtime_dir.display())?;
        return Ok(0);
    }

    if args.latest {
        return write_latest_path(stdout, stderr, &paths.runtime_dir, &paths.legacy_log_path);
    }

    if let Some(lines) = args.tail {
        return write_tail(stdout, stderr, &paths.runtime_dir, &paths.legacy_log_path, lines);
    }

    if args.bundle {
        return write_bundle(cli, args, stdout, stderr, &paths);
    }

    write_summary(stdout, &paths)
}

fn write_summary(
    stdout: &mut impl Write,
    paths: &crate::logging::DiagnosticsPaths,
) -> anyhow::Result<i32> {
    let runtime_logs = crate::logging::list_managed_runtime_logs_in(&paths.runtime_dir)?;
    let latest = crate::logging::latest_log_path_in(&paths.runtime_dir, &paths.legacy_log_path)?;
    let style = HumanStyle::detect();

    writeln!(stdout, "{}", style.title("claude-rs logs"))?;
    writeln!(
        stdout,
        "{} {}, {}",
        style.detail_label("Summary:"),
        style.count(runtime_logs.len(), "managed runtime logs"),
        if latest.is_some() { "latest log found" } else { "no logs found" }
    )?;
    writeln!(stdout)?;

    writeln!(stdout, "{}", style.heading("Locations"))?;
    write_path_row(stdout, style, "Runtime logs", &paths.runtime_dir)?;
    write_path_row(stdout, style, "Legacy log", &paths.legacy_log_path)?;
    write_path_row(stdout, style, "Perf telemetry", &paths.perf_dir)?;
    writeln!(stdout)?;

    writeln!(stdout, "{}", style.heading("Latest"))?;
    if let Some(path) = latest {
        write_status_block(stdout, style, "FOUND", "Latest log", &path.display().to_string())?;
    } else {
        write_status_block(stdout, style, "MISS", "Latest log", "none found")?;
    }
    writeln!(stdout)?;

    writeln!(stdout, "{}", style.heading("Commands"))?;
    write_command_block(
        stdout,
        style,
        "path",
        "claude-rs logs --path",
        "Print runtime log directory",
    )?;
    write_command_block(
        stdout,
        style,
        "latest",
        "claude-rs logs --latest",
        "Print latest log path",
    )?;
    write_command_block(
        stdout,
        style,
        "tail",
        "claude-rs logs --tail 200",
        "Print redacted latest log tail",
    )?;
    write_command_block(
        stdout,
        style,
        "bundle",
        "claude-rs logs --bundle --yes",
        "Create redacted support ZIP",
    )?;
    Ok(0)
}

fn write_command_block(
    stdout: &mut impl Write,
    style: HumanStyle,
    mode: &str,
    command: &str,
    purpose: &str,
) -> std::io::Result<()> {
    writeln!(stdout, "{}", style.command_block(mode, command, purpose))
}

fn write_status_block(
    stdout: &mut impl Write,
    style: HumanStyle,
    status: &str,
    label: &str,
    value: &str,
) -> std::io::Result<()> {
    writeln!(stdout, "  {} {}", style.status(status), label)?;
    writeln!(stdout, "      {value}")
}

fn write_path_row(
    stdout: &mut impl Write,
    style: HumanStyle,
    label: &str,
    path: &Path,
) -> std::io::Result<()> {
    write_status_block(stdout, style, path_status(path), label, &path.display().to_string())
}

fn path_status(path: &Path) -> &'static str {
    if path.is_dir() {
        "DIR"
    } else if path.is_file() {
        "FILE"
    } else if path.exists() {
        "EXISTS"
    } else {
        "MISS"
    }
}

#[derive(Clone, Copy, Debug)]
struct HumanStyle {
    color: bool,
}

impl HumanStyle {
    fn detect() -> Self {
        Self { color: std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none() }
    }

    fn title(self, text: &str) -> String {
        if self.color { format!("\x1b[1;36m{text}\x1b[0m") } else { text.to_owned() }
    }

    fn heading(self, text: &str) -> String {
        if self.color { format!("\x1b[1;36m{text}\x1b[0m") } else { text.to_owned() }
    }

    fn detail_label(self, text: &str) -> String {
        if self.color { format!("\x1b[2m{text}\x1b[0m") } else { text.to_owned() }
    }

    fn status(self, status: &str) -> String {
        let label = format!("[{status}]");
        if !self.color {
            return label;
        }

        match status {
            "DIR" | "FILE" | "FOUND" | "EXISTS" => format!("\x1b[32m{label}\x1b[0m"),
            "MISS" => format!("\x1b[2m{label}\x1b[0m"),
            _ => label,
        }
    }

    fn count(self, count: usize, label: &str) -> String {
        let text = format!("{count} {label}");
        if self.color { format!("\x1b[32m{text}\x1b[0m") } else { text }
    }

    fn mode(self, mode: &str) -> String {
        if self.color { format!("\x1b[2m{mode}\x1b[0m") } else { mode.to_owned() }
    }

    fn command_block(self, mode: &str, command: &str, purpose: &str) -> String {
        format!("  {} {}\n      {}", self.mode(mode), command, purpose)
    }
}

fn write_latest_path(
    stdout: &mut impl Write,
    stderr: &mut impl Write,
    runtime_dir: &Path,
    legacy_log_path: &Path,
) -> anyhow::Result<i32> {
    let Some(path) = crate::logging::latest_log_path_in(runtime_dir, legacy_log_path)? else {
        writeln!(stderr, "No claude-rs log file was found.")?;
        return Ok(1);
    };

    writeln!(stdout, "{}", path.display())?;
    Ok(0)
}
fn write_tail(
    stdout: &mut impl Write,
    stderr: &mut impl Write,
    runtime_dir: &Path,
    legacy_log_path: &Path,
    line_count: usize,
) -> anyhow::Result<i32> {
    let Some(path) = crate::logging::latest_log_path_in(runtime_dir, legacy_log_path)? else {
        writeln!(stderr, "No claude-rs log file was found.")?;
        return Ok(1);
    };

    let text =
        read_text_lossy(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let lines = text.lines().collect::<Vec<_>>();
    let start = lines.len().saturating_sub(line_count);
    for line in &lines[start..] {
        writeln!(stdout, "{}", redaction::redact_line(line))?;
    }
    Ok(0)
}

fn write_bundle(
    cli: &Cli,
    args: &LogsArgs,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
    paths: &crate::logging::DiagnosticsPaths,
) -> anyhow::Result<i32> {
    let plan = BundlePlan::build(paths, args.output.clone())?;
    if !args.yes && !confirm_bundle(stderr, &plan)? {
        writeln!(stderr, "Bundle creation cancelled.")?;
        return Ok(1);
    }

    let report = create_bundle(cli, paths, &plan)?;
    writeln!(stdout, "Created debug bundle: {}", report.output_path.display())?;
    Ok(0)
}

fn confirm_bundle(stderr: &mut impl Write, plan: &BundlePlan) -> anyhow::Result<bool> {
    if !std::io::stdin().is_terminal() {
        writeln!(stderr, "Refusing to create a debug bundle non-interactively without --yes.")?;
        return Ok(false);
    }

    writeln!(stderr, "claude-rs will create a redacted debug bundle at:")?;
    writeln!(stderr, "  {}", plan.output_path.display())?;
    writeln!(stderr)?;
    writeln!(stderr, "Included files:")?;
    writeln!(stderr, "  manifest.json")?;
    writeln!(stderr, "  doctor.json")?;
    writeln!(stderr, "  paths.json")?;
    for path in &plan.runtime_logs {
        writeln!(stderr, "  {}", path.display())?;
    }
    if let Some(path) = &plan.legacy_log {
        writeln!(stderr, "  {}", path.display())?;
    }
    writeln!(stderr)?;
    write!(stderr, "Continue? [y/N] ")?;
    stderr.flush()?;

    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    Ok(matches!(input.trim(), "y" | "Y" | "yes" | "YES"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BundlePlan {
    output_path: PathBuf,
    runtime_logs: Vec<PathBuf>,
    legacy_log: Option<PathBuf>,
}

impl BundlePlan {
    fn build(
        paths: &crate::logging::DiagnosticsPaths,
        output_path: Option<PathBuf>,
    ) -> anyhow::Result<Self> {
        let runtime_logs = select_bundle_runtime_logs(&paths.runtime_dir)?;
        let legacy_log = paths.legacy_log_path.is_file().then(|| paths.legacy_log_path.clone());
        Ok(Self {
            output_path: output_path.unwrap_or_else(|| default_bundle_path(&paths.root_dir)),
            runtime_logs,
            legacy_log,
        })
    }
}

#[derive(Debug, Serialize)]
struct BundleManifest {
    schema: &'static str,
    created_at: String,
    version: &'static str,
    platform: PlatformManifest,
    output_path: String,
    diagnostics_root: String,
    runtime_log_dir: String,
    legacy_log_path: String,
    perf_log_dir: String,
    included_files: Vec<String>,
    skipped: Vec<&'static str>,
    redaction: &'static str,
    last_crash_metadata: &'static str,
}

#[derive(Debug, Serialize)]
struct PlatformManifest {
    os: &'static str,
    arch: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BundleReport {
    output_path: PathBuf,
}

fn create_bundle(
    cli: &Cli,
    paths: &crate::logging::DiagnosticsPaths,
    plan: &BundlePlan,
) -> anyhow::Result<BundleReport> {
    if plan.output_path.exists() {
        anyhow::bail!("bundle output already exists: {}", plan.output_path.display());
    }
    if let Some(parent) = plan.output_path.parent() {
        create_dir_all(parent).with_context(|| {
            format!("failed to create bundle output directory {}", parent.display())
        })?;
    }

    let temp_path = temporary_bundle_path(&plan.output_path);
    let temp_file =
        OpenOptions::new().write(true).create_new(true).open(&temp_path).with_context(|| {
            format!("failed to create temporary bundle {}", temp_path.display())
        })?;

    let result = write_bundle_zip(cli, paths, plan, temp_file);
    if let Err(error) = result {
        let _ = fs::remove_file(&temp_path);
        return Err(error);
    }

    rename(&temp_path, &plan.output_path).with_context(|| {
        format!(
            "failed to move bundle from {} to {}",
            temp_path.display(),
            plan.output_path.display()
        )
    })?;
    Ok(BundleReport { output_path: plan.output_path.clone() })
}

fn write_bundle_zip(
    cli: &Cli,
    paths: &crate::logging::DiagnosticsPaths,
    plan: &BundlePlan,
    file: File,
) -> anyhow::Result<()> {
    let mut zip = ZipWriter::new(file);
    let mut included_files =
        vec!["manifest.json".to_owned(), "doctor.json".to_owned(), "paths.json".to_owned()];

    let mut runtime_log_entries = Vec::new();
    for path in &plan.runtime_logs {
        let entry_name = format!("logs/runtime/{}", safe_file_name(path));
        let text =
            read_text_lossy(path).with_context(|| format!("failed to read {}", path.display()))?;
        runtime_log_entries.push((entry_name, redaction::redact_text(&text)));
    }

    let legacy_log_entry = if let Some(path) = &plan.legacy_log {
        let text =
            read_text_lossy(path).with_context(|| format!("failed to read {}", path.display()))?;
        Some((format!("logs/legacy/{}", safe_file_name(path)), redaction::redact_text(&text)))
    } else {
        None
    };

    included_files.extend(runtime_log_entries.iter().map(|(name, _)| name.clone()));
    if let Some((name, _)) = &legacy_log_entry {
        included_files.push(name.clone());
    }
    included_files.push("logs/bridge-diagnostics.jsonl".to_owned());

    let manifest = BundleManifest {
        schema: BUNDLE_SCHEMA,
        created_at: timestamp_now(),
        version: env!("CARGO_PKG_VERSION"),
        platform: PlatformManifest { os: std::env::consts::OS, arch: std::env::consts::ARCH },
        output_path: plan.output_path.display().to_string(),
        diagnostics_root: paths.root_dir.display().to_string(),
        runtime_log_dir: paths.runtime_dir.display().to_string(),
        legacy_log_path: paths.legacy_log_path.display().to_string(),
        perf_log_dir: paths.perf_dir.display().to_string(),
        included_files,
        skipped: vec![
            "full config files",
            "Claude credentials",
            "environment variable dumps",
            "arbitrary project files",
        ],
        redaction: "credential-like keys and bearer/API token values are replaced with [redacted]",
        last_crash_metadata: "not_available",
    };

    add_json_file(&mut zip, "manifest.json", &manifest)?;

    let mut doctor_json = serde_json::to_value(doctor::build_report(cli))?;
    redaction::redact_json_value(&mut doctor_json);
    add_json_value_file(&mut zip, "doctor.json", &doctor_json)?;

    add_json_file(
        &mut zip,
        "paths.json",
        &serde_json::json!({
            "diagnostics_root": paths.root_dir.display().to_string(),
            "runtime_log_dir": paths.runtime_dir.display().to_string(),
            "legacy_log_path": paths.legacy_log_path.display().to_string(),
            "perf_log_dir": paths.perf_dir.display().to_string(),
            "config_files": "excluded",
        }),
    )?;

    for (entry_name, text) in &runtime_log_entries {
        add_text_file(&mut zip, entry_name, text)?;
    }
    if let Some((entry_name, text)) = &legacy_log_entry {
        add_text_file(&mut zip, entry_name, text)?;
    }

    let bridge_diagnostics = extract_bridge_diagnostics(
        runtime_log_entries
            .iter()
            .map(|(_, text)| text.as_str())
            .chain(legacy_log_entry.iter().map(|(_, text)| text.as_str())),
    );
    add_text_file(&mut zip, "logs/bridge-diagnostics.jsonl", &bridge_diagnostics)?;

    zip.finish()?;
    Ok(())
}

fn add_json_file<T: Serialize>(
    zip: &mut ZipWriter<File>,
    name: &str,
    value: &T,
) -> anyhow::Result<()> {
    let text = serde_json::to_string_pretty(value)?;
    add_text_file(zip, name, &format!("{text}\n"))
}

fn add_json_value_file(zip: &mut ZipWriter<File>, name: &str, value: &Value) -> anyhow::Result<()> {
    let text = serde_json::to_string_pretty(value)?;
    add_text_file(zip, name, &format!("{text}\n"))
}

fn add_text_file(zip: &mut ZipWriter<File>, name: &str, text: &str) -> anyhow::Result<()> {
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o644);
    zip.start_file(name, options)?;
    zip.write_all(text.as_bytes())?;
    Ok(())
}

fn select_bundle_runtime_logs(runtime_dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let logs = crate::logging::list_managed_runtime_logs_in(runtime_dir)?;
    let selected_bases = logs
        .iter()
        .filter(|log| !is_rotated_log(&log.path))
        .take(BUNDLE_RUNTIME_LOG_LIMIT)
        .map(|log| log.path.clone())
        .collect::<Vec<_>>();

    if selected_bases.is_empty() {
        return Ok(logs.into_iter().take(BUNDLE_RUNTIME_LOG_LIMIT).map(|log| log.path).collect());
    }

    let selected_base_set = selected_bases.iter().collect::<BTreeSet<_>>();
    let selected = logs
        .into_iter()
        .filter(|log| {
            base_log_path(&log.path).as_ref().is_some_and(|base| selected_base_set.contains(base))
        })
        .map(|log| log.path)
        .collect();
    Ok(selected)
}

fn extract_bridge_diagnostics<'a>(logs: impl Iterator<Item = &'a str>) -> String {
    let mut output = String::new();
    for log in logs {
        for line in log.lines() {
            let Ok(value) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            let target = value.get("target").and_then(Value::as_str).unwrap_or_default();
            let event_name = value.get("event_name").and_then(Value::as_str).unwrap_or_default();
            if target.starts_with("bridge.") || event_name.contains("bridge_stderr") {
                output.push_str(&redaction::redact_line(line));
                output.push('\n');
            }
        }
    }
    output
}

fn read_text_lossy(path: &Path) -> std::io::Result<String> {
    let bytes = fs::read(path)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn default_bundle_path(root_dir: &Path) -> PathBuf {
    root_dir.join(format!("claude-rs-debug-bundle-{}.zip", timestamp_for_file()))
}

fn temporary_bundle_path(output_path: &Path) -> PathBuf {
    let file_name = output_path.file_name().and_then(|name| name.to_str()).unwrap_or("bundle.zip");
    output_path.with_file_name(format!("{file_name}.{}.tmp", uuid::Uuid::new_v4().simple()))
}

fn timestamp_now() -> String {
    OffsetDateTime::now_utc()
        .format(format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]Z"))
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
}

fn timestamp_for_file() -> String {
    OffsetDateTime::now_utc()
        .format(format_description!("[year][month][day]T[hour][minute][second]Z"))
        .unwrap_or_else(|_| "19700101T000000Z".to_owned())
}

fn safe_file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map_or_else(|| "log-file".to_owned(), ToOwned::to_owned)
}

fn is_rotated_log(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.rsplit_once('.'))
        .is_some_and(|(_, suffix)| suffix.chars().all(|ch| ch.is_ascii_digit()))
}

fn base_log_path(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_str()?;
    let base_name = name
        .rsplit_once('.')
        .filter(|(_, suffix)| suffix.chars().all(|ch| ch.is_ascii_digit()))
        .map_or(name, |(base, _)| base);
    Some(path.with_file_name(base_name))
}

#[cfg(test)]
mod tests {
    use super::{
        BundlePlan, create_bundle, extract_bridge_diagnostics, select_bundle_runtime_logs,
        write_latest_path, write_summary, write_tail,
    };
    use crate::{Cli, LogsArgs};
    use std::fs;
    use std::io::Read as _;
    use std::path::PathBuf;
    use tempfile::tempdir;
    use zip::ZipArchive;

    #[test]
    fn latest_path_falls_back_to_legacy_log() {
        let dir = tempdir().expect("tempdir");
        let runtime_dir = dir.path().join("runtime");
        let legacy = dir.path().join("claude-rs.log");
        fs::write(&legacy, "legacy").expect("write legacy");
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let code =
            write_latest_path(&mut stdout, &mut stderr, &runtime_dir, &legacy).expect("latest");

        assert_eq!(code, 0);
        assert_eq!(stderr, b"");
        assert!(String::from_utf8(stdout).expect("utf8").contains("claude-rs.log"));
    }

    #[test]
    fn summary_output_uses_sections_and_status_rows() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path().join("logs");
        let runtime_dir = root.join("runtime");
        let perf_dir = root.join("perf");
        fs::create_dir_all(&runtime_dir).expect("runtime dir");
        fs::create_dir_all(&perf_dir).expect("perf dir");
        fs::write(runtime_dir.join("claude-rs-20260614T075924Z-p1-rabc.log"), "runtime")
            .expect("runtime log");
        let paths = crate::logging::DiagnosticsPaths {
            root_dir: root.clone(),
            runtime_dir,
            legacy_log_path: root.join("claude-rs.log"),
            perf_dir,
        };
        let mut stdout = Vec::new();

        let code = write_summary(&mut stdout, &paths).expect("summary");

        assert_eq!(code, 0);
        let output = String::from_utf8(stdout).expect("utf8");
        assert!(output.contains("claude-rs logs"));
        assert!(output.contains("Summary: 1 managed runtime logs, latest log found"));
        assert!(output.contains("Locations"));
        assert!(output.contains("[DIR]"));
        assert!(output.contains("[MISS] Legacy log"));
        assert!(output.contains("Latest"));
        assert!(output.contains("[FOUND] Latest log"));
        assert!(output.contains("Commands"));
        assert!(output.contains("Print redacted latest log tail"));
    }

    #[test]
    fn latest_path_reports_missing_logs() {
        let dir = tempdir().expect("tempdir");
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let code = write_latest_path(
            &mut stdout,
            &mut stderr,
            &dir.path().join("runtime"),
            &dir.path().join("claude-rs.log"),
        )
        .expect("latest");

        assert_eq!(code, 1);
        assert!(stdout.is_empty());
        assert!(String::from_utf8(stderr).expect("utf8").contains("No claude-rs log"));
    }

    #[test]
    fn tail_redacts_latest_log_lines() {
        let dir = tempdir().expect("tempdir");
        let runtime_dir = dir.path().join("runtime");
        fs::create_dir_all(&runtime_dir).expect("runtime dir");
        let log = runtime_dir.join("claude-rs-20260614T075924Z-p1-rabc.log");
        fs::write(&log, "first\nANTHROPIC_API_KEY=sk-ant-secret\nlast\n").expect("write log");
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let code = write_tail(
            &mut stdout,
            &mut stderr,
            &runtime_dir,
            &dir.path().join("claude-rs.log"),
            2,
        )
        .expect("tail");

        assert_eq!(code, 0);
        assert!(stderr.is_empty());
        let output = String::from_utf8(stdout).expect("utf8");
        assert!(!output.contains("sk-ant-secret"));
        assert!(output.contains("ANTHROPIC_API_KEY=[redacted]"));
        assert!(output.contains("last"));
    }

    #[test]
    fn bundle_runtime_selection_includes_rotated_siblings() {
        let dir = tempdir().expect("tempdir");
        let runtime_dir = dir.path();
        let base = runtime_dir.join("claude-rs-20260614T075924Z-p1-rabc.log");
        let rotated = runtime_dir.join("claude-rs-20260614T075924Z-p1-rabc.log.1");
        fs::write(&base, "base").expect("write base");
        fs::write(&rotated, "rotated").expect("write rotated");
        fs::write(runtime_dir.join("other.log"), "ignored").expect("write ignored");

        let selected = select_bundle_runtime_logs(runtime_dir).expect("select");

        assert!(selected.contains(&base));
        assert!(selected.contains(&rotated));
        assert_eq!(selected.len(), 2);
    }

    #[test]
    fn bridge_diagnostics_extracts_bridge_targets_only() {
        let logs = r#"{"target":"app.session","event_name":"x","message":"skip"}"#.to_owned()
            + "\n"
            + r#"{"target":"bridge.sdk","event_name":"sdk_stderr_line","fields":{"Authorization":"Bearer secret"}}"#;

        let extracted = extract_bridge_diagnostics([logs.as_str()].into_iter());

        assert!(extracted.contains("bridge.sdk"));
        assert!(!extracted.contains("app.session"));
        assert!(!extracted.contains("secret"));
        assert!(extracted.contains("[redacted]"));
    }

    #[test]
    fn bundle_contains_manifest_doctor_paths_and_redacted_logs() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path().join("logs");
        let runtime_dir = root.join("runtime");
        let perf_dir = root.join("perf");
        fs::create_dir_all(&runtime_dir).expect("runtime dir");
        let runtime_log = runtime_dir.join("claude-rs-20260614T075924Z-p1-rabc.log");
        fs::write(
            &runtime_log,
            r#"{"target":"bridge.sdk","event_name":"sdk_stderr_line","fields":{"Authorization":"Bearer secret"}}"#,
        )
        .expect("write runtime log");
        let legacy_log_path = root.join("claude-rs.log");
        fs::write(&legacy_log_path, "accessToken=secret-token").expect("write legacy log");
        let output_path = dir.path().join("bundle.zip");
        let paths = crate::logging::DiagnosticsPaths {
            root_dir: root,
            runtime_dir,
            legacy_log_path,
            perf_dir,
        };
        let plan = BundlePlan {
            output_path: output_path.clone(),
            runtime_logs: vec![runtime_log],
            legacy_log: Some(paths.legacy_log_path.clone()),
        };

        create_bundle(&test_cli(), &paths, &plan).expect("bundle");

        let file = fs::File::open(output_path).expect("open bundle");
        let mut archive = ZipArchive::new(file).expect("zip archive");
        let names = (0..archive.len())
            .map(|index| archive.by_index(index).expect("entry").name().to_owned())
            .collect::<Vec<_>>();
        assert!(names.contains(&"manifest.json".to_owned()));
        assert!(names.contains(&"doctor.json".to_owned()));
        assert!(names.contains(&"paths.json".to_owned()));
        assert!(names.contains(&"logs/bridge-diagnostics.jsonl".to_owned()));

        let mut bridge = String::new();
        archive
            .by_name("logs/bridge-diagnostics.jsonl")
            .expect("bridge entry")
            .read_to_string(&mut bridge)
            .expect("read bridge");
        assert!(!bridge.contains("secret"));
        assert!(bridge.contains("[redacted]"));
    }

    fn test_cli() -> Cli {
        Cli {
            command: Some(crate::Command::Logs(LogsArgs {
                path: false,
                latest: false,
                tail: None,
                bundle: true,
                output: Some(PathBuf::from("bundle.zip")),
                yes: true,
            })),
            no_update_check: false,
            dir: None,
            bridge_script: None,
            enable_logs: false,
            diagnostics_preset: None,
            log_file: None,
            log_filter: None,
            log_append: false,
            enable_perf: false,
            perf_log: None,
            perf_append: false,
        }
    }
}
