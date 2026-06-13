// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) fn resolve_claude_path(cached_claude_path: Option<PathBuf>) -> Result<PathBuf, String> {
    if let Some(path) = cached_claude_path
        && path.is_file()
    {
        return Ok(path);
    }
    which::which("claude").map_err(|_| "claude CLI not found in PATH".to_owned())
}

pub(crate) fn parse_json_command<T>(
    claude_path: &Path,
    cwd_raw: &str,
    args: &[&str],
) -> Result<T, String>
where
    T: for<'de> Deserialize<'de>,
{
    let output = Command::new(claude_path)
        .args(args)
        .current_dir(cwd_raw)
        .output()
        .map_err(|error| format!("Failed to run `claude {}`: {error}", args.join(" ")))?;

    if !output.status.success() {
        return Err(command_failure_message(args, output.status.code(), &output.stderr));
    }

    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("Failed to parse JSON from `claude {}`: {error}", args.join(" ")))
}

pub(crate) fn run_command(
    claude_path: &Path,
    cwd_raw: &str,
    args: &[String],
) -> Result<(), String> {
    let output = Command::new(claude_path)
        .args(args)
        .current_dir(cwd_raw)
        .output()
        .map_err(|error| format!("Failed to run `claude {}`: {error}", args.join(" ")))?;

    if output.status.success() {
        return Ok(());
    }

    Err(command_failure_message(args, output.status.code(), &output.stderr))
}

pub(crate) async fn run_command_task(
    cwd_raw: String,
    cached_claude_path: Option<PathBuf>,
    args: Vec<String>,
) -> Result<PathBuf, String> {
    tokio::task::spawn_blocking(move || {
        let claude_path = resolve_claude_path(cached_claude_path)?;
        run_command(&claude_path, &cwd_raw, &args)?;
        Ok(claude_path)
    })
    .await
    .map_err(|error| format!("Claude CLI task failed: {error}"))?
}

fn command_failure_message<S>(args: &[S], exit_code: Option<i32>, stderr: &[u8]) -> String
where
    S: AsRef<str>,
{
    let stderr = String::from_utf8_lossy(stderr).trim().to_owned();
    let exit_code = exit_code.map_or_else(|| "unknown".to_owned(), |code| code.to_string());
    let detail = if stderr.is_empty() {
        format!("exit code {exit_code}")
    } else {
        format!("exit code {exit_code}: {stderr}")
    };
    let args = args.iter().map(AsRef::as_ref).collect::<Vec<_>>().join(" ");
    format!("`claude {args}` failed: {detail}")
}
