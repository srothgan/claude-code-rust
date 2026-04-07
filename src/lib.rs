// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

pub mod agent;
pub mod app;
pub mod error;
pub mod perf;
pub mod ui;

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "claude-rs", about = "Native Rust terminal for Claude Code")]
#[allow(clippy::struct_excessive_bools)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Disable startup update checks.
    #[arg(long)]
    pub no_update_check: bool,

    /// Working directory (defaults to cwd)
    #[arg(long, short = 'C')]
    pub dir: Option<std::path::PathBuf>,

    /// Path to the agent bridge script (defaults to agent-sdk/dist/bridge.js).
    #[arg(long)]
    pub bridge_script: Option<std::path::PathBuf>,

    /// Write tracing diagnostics to a file (disabled unless explicitly set).
    #[arg(long, value_name = "PATH")]
    pub log_file: Option<std::path::PathBuf>,

    /// Tracing filter directives (example: `info,claude_code_rust::ui=trace`).
    /// Falls back to `RUST_LOG` when omitted.
    #[arg(long, value_name = "FILTER")]
    pub log_filter: Option<String>,

    /// Append to `--log-file` instead of truncating on startup.
    #[arg(long)]
    pub log_append: bool,

    /// Write frame performance events to a file (requires `--features perf` build).
    #[arg(long, value_name = "PATH")]
    pub perf_log: Option<std::path::PathBuf>,

    /// Append to `--perf-log` instead of truncating on startup.
    #[arg(long)]
    pub perf_append: bool,
}

#[derive(Subcommand, Debug, PartialEq, Eq)]
pub enum Command {
    /// Resume a previous session by ID, or pick from recent sessions
    Resume {
        /// Session ID to resume directly. Omit to show a session picker.
        session_id: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::{Cli, Command};
    use clap::Parser;

    #[test]
    fn cli_without_subcommand_starts_new_session() {
        let cli = Cli::try_parse_from(["claude-rs"]).expect("parse");
        assert!(cli.command.is_none());
    }

    #[test]
    fn cli_resume_without_id_requests_picker() {
        let cli = Cli::try_parse_from(["claude-rs", "resume"]).expect("parse");
        assert_eq!(cli.command, Some(Command::Resume { session_id: None }));
    }

    #[test]
    fn cli_resume_with_id_resumes_directly() {
        let cli = Cli::try_parse_from(["claude-rs", "resume", "abc-123"]).expect("parse");
        assert_eq!(cli.command, Some(Command::Resume { session_id: Some("abc-123".to_owned()) }));
    }

    #[test]
    fn cli_rejects_legacy_resume_flag() {
        assert!(Cli::try_parse_from(["claude-rs", "--resume", "abc-123"]).is_err());
    }
}
