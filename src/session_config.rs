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
        std::fs::write(project.path().join("notes.txt"), "").expect("file");
        let shared_path = shared.path().to_str().expect("UTF-8 path");

        let config = resolve(project.path(), &["--add-dir", "docs", "--add-dir", shared_path])
            .expect("existing directories");
        assert_eq!(
            config.additional_directories,
            [project.path().join("docs").to_str().expect("UTF-8 path"), shared_path]
        );

        for missing in ["absent", "notes.txt"] {
            let error = resolve(project.path(), &["--add-dir", missing]).expect_err(missing);
            assert_eq!(
                error,
                format!(
                    "--add-dir: {} is not an existing directory",
                    project.path().join(missing).display()
                )
            );
        }
    }

    #[test]
    fn settings_come_from_inline_json_or_a_file_and_must_be_an_object() {
        let project = tempfile::tempdir().expect("project");
        std::fs::write(project.path().join("ci.json"), "{ \"cleanupPeriodDays\": 7 }\n")
            .expect("settings file");
        std::fs::write(project.path().join("list.json"), "[1]").expect("array file");
        std::fs::write(project.path().join("broken.json"), "{ \"a\": ").expect("broken file");

        let from_file = resolve(project.path(), &["--settings", "ci.json"]).expect("file");
        assert_eq!(from_file.settings, json!({"cleanupPeriodDays": 7}).as_object().cloned());
        let inline =
            resolve(project.path(), &["--settings", "  {\"model\": \"haiku\"}"]).expect("inline");
        assert_eq!(inline.settings, json!({"model": "haiku"}).as_object().cloned());

        let missing = resolve(project.path(), &["--settings", "absent.json"]).expect_err("missing");
        assert!(
            missing.starts_with(&format!(
                "--settings: cannot read {}: ",
                project.path().join("absent.json").display()
            )),
            "{missing}"
        );
        let array = resolve(project.path(), &["--settings", "list.json"]).expect_err("array");
        assert_eq!(
            array,
            format!(
                "--settings: {} must be a JSON object",
                project.path().join("list.json").display()
            )
        );
        let broken = resolve(project.path(), &["--settings", "broken.json"]).expect_err("broken");
        assert!(broken.contains("is not valid JSON"), "{broken}");
        let inline_broken =
            resolve(project.path(), &["--settings", "{\"a\":"]).expect_err("inline broken");
        assert!(
            inline_broken.starts_with("--settings: the inline value is not valid JSON"),
            "{inline_broken}"
        );
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

        let without = resolve(project.path(), &["--mcp-config", "{\"servers\":{}}"])
            .expect_err("no mcpServers");
        assert_eq!(without, "--mcp-config: the inline value must contain an \"mcpServers\" object");
        let scalar = resolve(project.path(), &["--mcp-config", r#"{"mcpServers":{"docs":"x"}}"#])
            .expect_err("server is not an object");
        assert_eq!(scalar, "--mcp-config: the inline value: MCP server \"docs\" must be an object");
    }

    #[test]
    fn prompt_files_are_read_whole_and_an_empty_one_is_rejected() {
        let project = tempfile::tempdir().expect("project");
        std::fs::write(project.path().join("prompt.md"), "You are a release bot.\n").expect("file");
        std::fs::write(project.path().join("empty.md"), "").expect("empty file");
        std::fs::write(project.path().join("blank.md"), " \n\t\n").expect("blank file");

        let config = resolve(
            project.path(),
            &["--system-prompt-file", "prompt.md", "--append-system-prompt", "Answer in German."],
        )
        .expect("prompts");
        assert_eq!(config.system_prompt.as_deref(), Some("You are a release bot.\n"));
        assert_eq!(config.append_system_prompt.as_deref(), Some("Answer in German."));

        for (flag, file) in [
            ("--system-prompt-file", "empty.md"),
            ("--system-prompt-file", "blank.md"),
            ("--append-system-prompt-file", "empty.md"),
        ] {
            let error = resolve(project.path(), &[flag, file]).expect_err(file);
            assert_eq!(error, format!("{flag}: {} is empty", project.path().join(file).display()));
        }
        let missing = resolve(project.path(), &["--append-system-prompt-file", "absent.md"])
            .expect_err("missing prompt file");
        assert!(missing.starts_with("--append-system-prompt-file: cannot read "), "{missing}");
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
        for args in [
            ["claude-rs", "--system-prompt", ""],
            ["claude-rs", "--system-prompt", "  "],
            ["claude-rs", "--append-system-prompt", ""],
        ] {
            assert!(Cli::try_parse_from(args).is_err(), "{args:?}");
        }
    }
}
