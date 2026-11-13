// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

//! Launch flags that configure every session the process starts.
//!
//! The flags are read here once, before logging, the runtime or the bridge start, so a typo
//! stops the launch with a command-line error instead of surfacing inside a running session.
//! Rust reads files and checks that JSON parses; the bridge is the only place that turns the
//! result into Agent SDK options.

use crate::agent::wire::SessionConfig;
use clap::Args;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

// A doc comment on this struct would replace the command's about text in `--help`.
//
// Unlike `--name` and `--fork-session`, which are for the initial session, these are kept for
// every session the process starts, including `/new` and `/resume`. Relative paths resolve
// against the session working directory.
#[derive(Args, Debug, Clone, Default)]
pub struct SessionConfigArgs {
    /// Add a directory Claude may access; repeatable.
    #[arg(long = "add-dir", global = true, value_name = "PATH")]
    pub add_dirs: Vec<PathBuf>,

    /// Temporary settings as inline JSON or a JSON file.
    #[arg(long, global = true, value_name = "JSON_OR_FILE")]
    pub settings: Option<String>,

    /// MCP servers as inline JSON or a JSON file with an `mcpServers` object; repeatable.
    #[arg(long = "mcp-config", global = true, value_name = "JSON_OR_FILE")]
    pub mcp_configs: Vec<String>,

    /// Use only the MCP servers from `--mcp-config`.
    #[arg(long, global = true)]
    pub strict_mcp_config: bool,

    /// Replace the default system prompt for new sessions.
    #[arg(long, global = true, value_name = "TEXT", conflicts_with = "system_prompt_file", value_parser = parse_prompt_text)]
    pub system_prompt: Option<String>,

    /// Read the replacement system prompt from a file.
    #[arg(long, global = true, value_name = "PATH")]
    pub system_prompt_file: Option<PathBuf>,

    /// Append instructions to the system prompt for new sessions.
    #[arg(long, global = true, value_name = "TEXT", conflicts_with = "append_system_prompt_file", value_parser = parse_prompt_text)]
    pub append_system_prompt: Option<String>,

    /// Read the appended system prompt instructions from a file.
    #[arg(long, global = true, value_name = "PATH")]
    pub append_system_prompt_file: Option<PathBuf>,
}

impl SessionConfigArgs {
    /// Whether any of the flags was given.
    #[must_use]
    pub fn is_set(&self) -> bool {
        !self.add_dirs.is_empty()
            || self.settings.is_some()
            || !self.mcp_configs.is_empty()
            || self.strict_mcp_config
            || self.system_prompt.is_some()
            || self.system_prompt_file.is_some()
            || self.append_system_prompt.is_some()
            || self.append_system_prompt_file.is_some()
    }

    /// Resolve paths against `base`, read files and parse JSON, failing on the first flag
    /// whose input cannot be used. Nothing is written anywhere.
    pub fn resolve(&self, base: &Path) -> Result<SessionConfig, String> {
        Ok(SessionConfig {
            additional_directories: self
                .add_dirs
                .iter()
                .map(|dir| existing_directory(base, dir))
                .collect::<Result<_, _>>()?,
            settings: self
                .settings
                .as_deref()
                .map(|value| json_object(base, value, "--settings").map(|(object, _)| object))
                .transpose()?,
            mcp_configs: self
                .mcp_configs
                .iter()
                .map(|value| mcp_servers(base, value))
                .collect::<Result<_, _>>()?,
            strict_mcp_config: self.strict_mcp_config,
            system_prompt: prompt(
                base,
                self.system_prompt.as_deref(),
                self.system_prompt_file.as_deref(),
                "--system-prompt-file",
            )?,
            append_system_prompt: prompt(
                base,
                self.append_system_prompt.as_deref(),
                self.append_system_prompt_file.as_deref(),
                "--append-system-prompt-file",
            )?,
        })
    }
}

/// A prompt with no visible text would silently do nothing, so text and files share one rule.
/// Session titles and names are user text that the Status tab writes to the terminal, so
/// control characters are stripped and the result trimmed, here, once. `None` means nothing
/// visible is left.
pub(crate) fn sanitize_session_title(text: &str) -> Option<String> {
    let text: String = text.chars().filter(|ch| !ch.is_control()).collect();
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn is_blank(text: &str) -> bool {
    text.trim().is_empty()
}

fn parse_prompt_text(value: &str) -> Result<String, String> {
    if is_blank(value) {
        return Err("the prompt text must not be empty".to_owned());
    }
    Ok(value.to_owned())
}

fn absolute(base: &Path, path: &Path, flag: &str) -> Result<PathBuf, String> {
    std::path::absolute(base.join(path))
        .map_err(|error| format!("{flag}: cannot resolve {}: {error}", path.display()))
}

fn existing_directory(base: &Path, dir: &Path) -> Result<String, String> {
    let path = absolute(base, dir, "--add-dir")?;
    if !path.is_dir() {
        return Err(format!("--add-dir: {} is not an existing directory", path.display()));
    }
    path.into_os_string()
        .into_string()
        .map_err(|path| format!("--add-dir: {} is not valid UTF-8", path.to_string_lossy()))
}

fn read_text(path: &Path, flag: &str) -> Result<String, String> {
    std::fs::read_to_string(path)
        .map_err(|error| format!("{flag}: cannot read {}: {error}", path.display()))
}

/// The text of whichever spelling was given; clap keeps the two mutually exclusive.
fn prompt(
    base: &Path,
    text: Option<&str>,
    file: Option<&Path>,
    file_flag: &str,
) -> Result<Option<String>, String> {
    if let Some(text) = text {
        return Ok(Some(text.to_owned()));
    }
    let Some(file) = file else {
        return Ok(None);
    };
    let path = absolute(base, file, file_flag)?;
    let text = read_text(&path, file_flag)?;
    if is_blank(&text) {
        return Err(format!("{file_flag}: {} is empty", path.display()));
    }
    Ok(Some(text))
}

/// A JSON object given inline (a value starting with `{`) or as a file, with the source's
/// name for error messages.
fn json_object(
    base: &Path,
    value: &str,
    flag: &str,
) -> Result<(Map<String, Value>, String), String> {
    let (text, source) = if value.trim_start().starts_with('{') {
        (value.to_owned(), "the inline value".to_owned())
    } else {
        let path = absolute(base, Path::new(value), flag)?;
        (read_text(&path, flag)?, path.display().to_string())
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(object)) => Ok((object, source)),
        Ok(_) => Err(format!("{flag}: {source} must be a JSON object")),
        Err(error) => Err(format!("{flag}: {source} is not valid JSON: {error}")),
    }
}

/// The `mcpServers` object of one `--mcp-config`. The bridge merges the objects of all
/// configs in command-line order; the SDK validates each server definition.
fn mcp_servers(base: &Path, value: &str) -> Result<Map<String, Value>, String> {
    let (mut config, source) = json_object(base, value, "--mcp-config")?;
    let Some(Value::Object(servers)) = config.remove("mcpServers") else {
        return Err(format!("--mcp-config: {source} must contain an \"mcpServers\" object"));
    };
    if let Some(name) =
        servers.iter().find_map(|(name, server)| (!server.is_object()).then_some(name))
    {
        return Err(format!("--mcp-config: {source}: MCP server \"{name}\" must be an object"));
    }
    Ok(servers)
}

#[cfg(test)]
mod tests {
    use super::sanitize_session_title;
    use crate::Cli;
    use clap::Parser;
    use serde_json::json;

    fn resolve(
        base: &std::path::Path,
        flags: &[&str],
    ) -> Result<crate::agent::wire::SessionConfig, String> {
        let command_line = std::iter::once("claude-rs").chain(flags.iter().copied());
        Cli::try_parse_from(command_line).expect("parse").session_config.resolve(base)
    }

    #[test]
    fn added_directories_resolve_against_the_base_and_must_exist() {
        let project = tempfile::tempdir().expect("project");
        let shared = tempfile::tempdir().expect("shared");
        std::fs::create_dir(project.path().join("docs")).expect("docs");
        std::fs::write(project.path().join("notes.txt"), "notes").expect("file");
        let shared_path = shared.path().to_str().expect("UTF-8 path");
        let config = resolve(project.path(), &["--add-dir", "docs", "--add-dir", shared_path])
            .expect("existing directories");
        assert_eq!(
            config.additional_directories,
            [project.path().join("docs").to_str().expect("path"), shared_path]
        );
        for target in ["absent", "notes.txt"] {
            let error = resolve(project.path(), &["--add-dir", "docs", "--add-dir", target])
                .expect_err(target);
            for detail in ["--add-dir", target, "directory"] {
                assert!(error.contains(detail), "{error}");
            }
        }
    }

    #[test]
    fn settings_come_from_inline_json_or_a_file_and_must_be_an_object() {
        let project = tempfile::tempdir().expect("project");
        for (file, contents, category) in [
            ("absent.json", None, "read"),
            ("list.json", Some("[1]"), "object"),
            ("scalar.json", Some("42"), "object"),
            ("broken.json", Some("{\"a\":"), "JSON"),
        ] {
            if let Some(contents) = contents {
                std::fs::write(project.path().join(file), contents).expect("invalid settings");
            }
            let error = resolve(project.path(), &["--settings", file]).expect_err(file);
            for detail in ["--settings", file, category] {
                assert!(error.contains(detail), "{error}");
            }
            std::fs::write(
                project.path().join(file),
                r#"{"permissions":{"allow":["Read"]},"cleanupPeriodDays":7}"#,
            )
            .expect("correct settings");
            let corrected =
                resolve(project.path(), &["--settings", file]).expect("corrected settings");
            assert_eq!(
                corrected.settings,
                json!({"permissions":{"allow":["Read"]},"cleanupPeriodDays":7})
                    .as_object()
                    .cloned()
            );
        }
        let error = resolve(project.path(), &["--settings", " {\"a\":"]).expect_err("inline JSON");
        for detail in ["--settings", "inline", "JSON"] {
            assert!(error.contains(detail), "{error}");
        }
        for (text, expected) in
            [("  {\"model\":\"haiku\"}", json!({"model":"haiku"})), (" {}", json!({}))]
        {
            assert_eq!(
                resolve(project.path(), &["--settings", text]).expect("inline object").settings,
                expected.as_object().cloned()
            );
        }
    }

    #[test]
    fn mcp_configs_keep_command_line_order_and_need_an_mcp_servers_object() {
        let project = tempfile::tempdir().expect("project");
        std::fs::write(
            project.path().join("mcp.json"),
            r#"{"mcpServers":{"docs":{"command":"docs-mcp"}}}"#,
        )
        .expect("mcp file");
        let inline = r#"{"mcpServers":{"docs":{"command":"override"}}}"#;

        let config = resolve(
            project.path(),
            &["--mcp-config", "mcp.json", "--mcp-config", inline, "--strict-mcp-config"],
        )
        .expect("two configs");
        assert!(config.strict_mcp_config);
        assert_eq!(
            config.mcp_configs,
            [
                json!({"docs": {"command": "docs-mcp"}}).as_object().cloned().expect("object"),
                json!({"docs": {"command": "override"}}).as_object().cloned().expect("object"),
            ]
        );

        for (text, detail) in [
            (r#"{"servers":{}}"#, "mcpServers"),
            (r#"{"mcpServers":[]}"#, "mcpServers"),
            (r#"{"mcpServers":{"docs":"x"}}"#, "docs"),
        ] {
            for source in [text, "invalid-mcp.json"] {
                std::fs::write(project.path().join("invalid-mcp.json"), text)
                    .expect("invalid MCP file");
                let error = resolve(project.path(), &["--mcp-config", source]).expect_err("shape");
                for part in [
                    "--mcp-config",
                    detail,
                    "object",
                    if source == text { "inline" } else { "invalid-mcp.json" },
                ] {
                    assert!(error.contains(part), "{error}");
                }
            }
            std::fs::write(
                project.path().join("invalid-mcp.json"),
                r#"{"mcpServers":{"docs":{"command":"repaired"}}}"#,
            )
            .expect("repair");
            assert_eq!(
                resolve(project.path(), &["--mcp-config", "invalid-mcp.json"])
                    .expect("corrected MCP")
                    .mcp_configs,
                [json!({"docs":{"command":"repaired"}}).as_object().cloned().expect("object")]
            );
        }
        assert_eq!(
            resolve(project.path(), &["--mcp-config", r#"{"mcpServers":{}}"#])
                .expect("empty valid config")
                .mcp_configs,
            [serde_json::Map::new()]
        );
    }

    #[test]
    fn prompt_files_are_read_whole_and_an_empty_one_is_rejected() {
        let project = tempfile::tempdir().expect("project");
        std::fs::write(
            project.path().join("replace.md"),
            "You are a release bot.\nGr\u{fc}\u{df}e.\n",
        )
        .expect("replacement file");
        std::fs::write(
            project.path().join("append.md"),
            "Append these notes:\n\u{65e5}\u{672c}\u{8a9e}\n",
        )
        .expect("append file");
        let first = resolve(
            project.path(),
            &[
                "--system-prompt-file",
                "replace.md",
                "--append-system-prompt",
                " Answer in German. ",
            ],
        )
        .expect("file/text");
        assert_eq!(
            first.system_prompt.as_deref(),
            Some("You are a release bot.\nGr\u{fc}\u{df}e.\n")
        );
        assert_eq!(first.append_system_prompt.as_deref(), Some(" Answer in German. "));
        let second = resolve(
            project.path(),
            &[
                "--system-prompt",
                "Text replacement.\nKeep this line.\n",
                "--append-system-prompt-file",
                "append.md",
            ],
        )
        .expect("text/file");
        assert_eq!(second.system_prompt.as_deref(), Some("Text replacement.\nKeep this line.\n"));
        assert_eq!(
            second.append_system_prompt.as_deref(),
            Some("Append these notes:\n\u{65e5}\u{672c}\u{8a9e}\n")
        );
        for flag in ["--system-prompt-file", "--append-system-prompt-file"] {
            for (file, content, category) in [
                ("empty.md", Some(""), "empty"),
                ("blank.md", Some(" \n\t\n"), "empty"),
                ("absent.md", None, "read"),
            ] {
                if let Some(content) = content {
                    std::fs::write(project.path().join(file), content).expect("invalid file");
                }
                let error = resolve(project.path(), &[flag, file]).expect_err("invalid prompt");
                for part in [flag, file, category] {
                    assert!(error.contains(part), "{error}");
                }
            }
        }
    }

    #[test]
    fn session_titles_lose_control_characters_and_surrounding_space() {
        assert_eq!(
            sanitize_session_title("  Re\x1b[31mlease\n prep \t").as_deref(),
            Some("Re[31mlease prep")
        );
        assert_eq!(sanitize_session_title(" \x07\n "), None);
    }

    #[test]
    fn blank_inline_prompts_are_rejected_by_the_parser() {
        let project = tempfile::tempdir().expect("project");
        for flag in ["--system-prompt", "--append-system-prompt"] {
            for blank in ["", "  ", "\n\t\u{2003}"] {
                let error =
                    Cli::try_parse_from(["claude-rs", flag, blank]).expect_err("blank prompt");
                assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
                assert!(error.to_string().contains(flag), "{error}");
            }
            let text = " \nGr\u{fc}\u{df}e \u{65e5}\u{672c}\u{8a9e}\t ";
            let config = resolve(project.path(), &[flag, text]).expect("visible prompt");
            if flag == "--system-prompt" {
                assert_eq!(config.system_prompt.as_deref(), Some(text));
                assert!(config.append_system_prompt.is_none());
            } else {
                assert_eq!(config.append_system_prompt.as_deref(), Some(text));
                assert!(config.system_prompt.is_none());
            }
        }
    }
}
