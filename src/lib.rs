// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

pub mod agent;
pub mod app;
mod claude_paths;
pub mod cli;
pub mod error;
pub mod failure;
pub mod install_method;
mod json_file;
pub mod logging;
pub mod permission_mode;
pub mod ui;

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Clone, Debug, ValueEnum, PartialEq, Eq)]
pub enum DiagnosticsPreset {
    Runtime,
    Session,
    Render,
    Bridge,
    Full,
}

impl DiagnosticsPreset {
    #[must_use]
    pub fn filter_directives(&self) -> &'static str {
        match self {
            Self::Runtime => {
                "info,bridge.lifecycle=debug,bridge.protocol=debug,app.session=debug,app.tool=debug,app.command=debug,app.permission=debug,app.network=debug,app.update=debug,app.file_index=debug"
            }
            Self::Session => {
                "info,bridge.lifecycle=debug,bridge.protocol=debug,app.session=debug,app.permission=debug,app.command=debug"
            }
            Self::Render => "info,app.render=trace,app.cache=debug,app.input=debug,app.paste=debug",
            Self::Bridge => {
                "info,bridge.lifecycle=debug,bridge.protocol=debug,bridge.sdk=debug,bridge.permission=debug,bridge.mcp=debug"
            }
            Self::Full => {
                "info,app.render=trace,bridge.lifecycle=debug,bridge.protocol=debug,bridge.sdk=debug,bridge.permission=debug,bridge.mcp=debug,app.session=debug,app.tool=debug,app.command=debug,app.permission=debug,app.network=debug,app.update=debug,app.cache=debug,app.input=debug,app.paste=debug,app.config=debug,app.auth=debug,app.file_index=debug"
            }
        }
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "claude-rs",
    version = env!("CARGO_PKG_VERSION"),
    about = "Native Rust terminal for Claude Code"
)]
#[command(
    after_help = "Examples:\n  claude-rs --enable-logs --diagnostics-preset session\n  claude-rs --enable-logs --diagnostics-preset render"
)]
#[allow(clippy::struct_excessive_bools)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Initial prompt to send after the interactive session is ready.
    #[arg(value_name = "PROMPT")]
    pub prompt: Option<String>,

    /// Resume a session by ID, or open the session picker when no ID is given.
    // Clap uses the nested option for absent flag, picker, and explicit ID.
    #[allow(clippy::option_option)]
    #[arg(long, short = 'r', num_args = 0..=1, value_name = "ID", conflicts_with = "continue_session")]
    pub resume: Option<Option<String>>,

    /// Continue the most recent session in the working directory (including git worktrees).
    #[arg(long = "continue", short = 'c')]
    pub continue_session: bool,

    #[command(flatten)]
    pub session_options: SessionOptions,

    /// Disable startup update checks.
    #[arg(long)]
    pub no_update_check: bool,

    /// Working directory (defaults to cwd)
    #[arg(long, short = 'C')]
    pub dir: Option<std::path::PathBuf>,

    /// Path to the agent bridge script (defaults to agent-sdk/dist/bridge.js).
    #[arg(long)]
    pub bridge_script: Option<std::path::PathBuf>,

    /// Enable detailed info-level diagnostics in addition to the always-on warning/error baseline.
    #[arg(long)]
    pub enable_logs: bool,

    /// Named diagnostics preset for common logging workflows.
    /// Ignored when `--log-filter` is provided explicitly.
    #[arg(long, value_enum)]
    pub diagnostics_preset: Option<DiagnosticsPreset>,

    /// Write tracing diagnostics to a file.
    ///
    /// When omitted, interactive runs use a timestamped default log path.
    #[arg(long, value_name = "PATH")]
    pub log_file: Option<std::path::PathBuf>,

    /// Tracing filter directives (example: `info,app.render=trace`).
    /// Overrides `--diagnostics-preset` and falls back to `RUST_LOG` when omitted.
    #[arg(long, value_name = "FILTER")]
    pub log_filter: Option<String>,

    /// Append to an explicit `--log-file`.
    ///
    /// Without `--log-file`, appends to the legacy shared default log for compatibility.
    #[arg(long)]
    pub log_append: bool,
}

/// Per-launch options, separate from persisted application preferences.
#[derive(Args, Debug, Clone, Default)]
pub struct SessionOptions {
    /// Starting model alias or full model ID; does not change saved preferences.
    #[arg(long, global = true, value_name = "MODEL", value_parser = clap::builder::NonEmptyStringValueParser::new())]
    pub model: Option<String>,

    /// Starting reasoning effort; availability depends on the selected model.
    #[arg(long, global = true, value_enum)]
    pub effort: Option<agent::model::EffortLevel>,

    /// Starting permission mode; does not change saved preferences.
    #[arg(long, global = true, value_enum)]
    pub permission_mode: Option<permission_mode::DefaultPermissionMode>,

    /// Main-thread agent to use for this session.
    #[arg(long, global = true, value_name = "NAME", value_parser = clap::builder::NonEmptyStringValueParser::new())]
    pub agent: Option<String>,
}

impl Cli {
    pub fn validate(&self) -> Result<(), clap::Error> {
        use clap::CommandFactory;
        let has_session_options = self.session_options.model.is_some()
            || self.session_options.effort.is_some()
            || self.session_options.permission_mode.is_some()
            || self.session_options.agent.is_some();
        let conflict = match &self.command {
            Some(Command::Resume { .. }) => {
                self.resume.is_some() || self.continue_session || self.prompt.is_some()
            }
            Some(_) => {
                self.resume.is_some()
                    || self.continue_session
                    || self.prompt.is_some()
                    || has_session_options
            }
            None => false,
        };
        if conflict {
            return Err(Self::command().error(
                clap::error::ErrorKind::ArgumentConflict,
                "interactive startup arguments cannot be combined with this subcommand",
            ));
        }
        Ok(())
    }
    /// Normalize both resume spellings into one startup intent.
    #[must_use]
    pub fn startup_launch(&self) -> StartupLaunch {
        let resume = match &self.command {
            Some(Command::Resume { session_id, .. }) => Some(session_id.as_ref()),
            _ => self.resume.as_ref().map(Option::as_ref),
        };
        match resume {
            Some(Some(id)) => StartupLaunch::ResumeSession { session_id: id.clone() },
            Some(None) => StartupLaunch::SessionPicker,
            None if self.continue_session => StartupLaunch::ContinueSession,
            None => StartupLaunch::NewSession,
        }
    }

    #[must_use]
    pub fn initial_prompt(&self) -> Option<&str> {
        match &self.command {
            Some(Command::Resume { prompt, .. }) => prompt.as_deref(),
            _ => self.prompt.as_deref(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum StartupLaunch {
    #[default]
    NewSession,
    ResumeSession {
        session_id: String,
    },
    SessionPicker,
    ContinueSession,
}

impl StartupLaunch {
    #[must_use]
    pub fn resume_id(&self) -> Option<&str> {
        match self {
            Self::ResumeSession { session_id } => Some(session_id),
            _ => None,
        }
    }

    #[must_use]
    pub fn resume_requested(&self) -> bool {
        matches!(self, Self::ResumeSession { .. } | Self::ContinueSession)
    }
}

#[derive(Subcommand, Debug, PartialEq, Eq)]
pub enum Command {
    /// Resume a previous session by ID, or pick from recent sessions
    Resume {
        /// Session ID to resume directly. Omit to show a session picker.
        session_id: Option<String>,
        /// Initial prompt to send after resuming.
        prompt: Option<String>,
    },
    /// Print a shell completion script.
    Completions {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
    /// Generate manual pages for the CLI and its subcommands.
    #[command(hide = true)]
    Man { out_dir: std::path::PathBuf },
    /// Run deterministic installation and runtime diagnostics
    Doctor(DoctorArgs),
    /// Find runtime logs or create a redacted debug bundle
    Logs(LogsArgs),
    /// Inspect and export redacted configuration
    Config(ConfigArgs),
}

#[derive(Args, Clone, Debug, PartialEq, Eq)]
pub struct DoctorArgs {
    /// Emit a machine-readable JSON report.
    #[arg(long)]
    pub json: bool,

    /// Exit non-zero when hard runtime prerequisites fail.
    #[arg(long)]
    pub strict: bool,
}

#[derive(Args, Clone, Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct LogsArgs {
    /// Print only the runtime log directory path.
    #[arg(long, conflicts_with_all = ["latest", "tail", "bundle"])]
    pub path: bool,

    /// Print only the latest discovered log path.
    #[arg(long, conflicts_with_all = ["path", "tail", "bundle"])]
    pub latest: bool,

    /// Print the last N redacted lines from the latest discovered log.
    #[arg(long, value_name = "LINES", conflicts_with_all = ["path", "latest", "bundle"])]
    pub tail: Option<usize>,

    /// Create a redacted ZIP bundle for support.
    #[arg(long, conflicts_with_all = ["path", "latest", "tail"])]
    pub bundle: bool,

    /// Write the bundle ZIP to this path.
    #[arg(long, value_name = "PATH", requires = "bundle")]
    pub output: Option<std::path::PathBuf>,

    /// Skip interactive confirmation for bundle creation.
    #[arg(long)]
    pub yes: bool,
}

#[derive(Args, Clone, Debug, PartialEq, Eq)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: Option<ConfigCommand>,
}

#[derive(Subcommand, Clone, Debug, PartialEq, Eq)]
pub enum ConfigCommand {
    /// Print resolved config file paths
    Path(ConfigPathArgs),
    /// Show a concise redacted config summary
    Show(ConfigShowArgs),
    /// Export a redacted support-safe config snapshot
    Export(ConfigExportArgs),
}

#[derive(Args, Clone, Debug, PartialEq, Eq)]
pub struct ConfigPathArgs {
    /// Emit machine-readable JSON path metadata.
    #[arg(long)]
    pub json: bool,

    /// Print only one config file path for scripting.
    #[arg(long, value_enum)]
    pub which: Option<ConfigFileSelector>,
}

#[derive(Args, Clone, Debug, PartialEq, Eq)]
pub struct ConfigShowArgs {
    /// Emit machine-readable redacted JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Clone, Debug, PartialEq, Eq)]
pub struct ConfigExportArgs {
    /// Write the redacted export JSON to this new file.
    #[arg(long, value_name = "PATH")]
    pub output: Option<std::path::PathBuf>,
}

#[derive(Clone, Copy, Debug, ValueEnum, PartialEq, Eq)]
pub enum ConfigFileSelector {
    Settings,
    ProjectSettings,
    LocalSettings,
    Preferences,
}

#[cfg(test)]
mod tests {
    use super::{
        Cli, Command, ConfigArgs, ConfigCommand, ConfigExportArgs, ConfigFileSelector,
        ConfigPathArgs, ConfigShowArgs, DoctorArgs, LogsArgs,
    };
    use clap::{CommandFactory, Parser};

    #[test]
    fn cli_without_subcommand_starts_new_session() {
        let cli = Cli::try_parse_from(["claude-rs"]).expect("parse");
        assert!(cli.command.is_none());
    }

    #[test]
    fn cli_resume_without_id_requests_picker() {
        let cli = Cli::try_parse_from(["claude-rs", "resume"]).expect("parse");
        assert_eq!(cli.command, Some(Command::Resume { session_id: None, prompt: None }));
    }

    #[test]
    fn cli_resume_with_id_resumes_directly() {
        let cli = Cli::try_parse_from(["claude-rs", "resume", "abc-123"]).expect("parse");
        assert_eq!(
            cli.command,
            Some(Command::Resume { session_id: Some("abc-123".to_owned()), prompt: None })
        );
    }

    #[test]
    fn cli_resume_flag_selects_the_same_session_as_the_subcommand() {
        let flag = Cli::try_parse_from(["claude-rs", "--resume", "abc-123"]).expect("flag");
        let command = Cli::try_parse_from(["claude-rs", "resume", "abc-123"]).expect("command");
        assert_eq!(flag.startup_launch(), command.startup_launch());
    }

    #[test]
    fn cli_rejects_removed_perf_flags() {
        let cases: &[&[&str]] = &[
            &["claude-rs", "--enable-perf"],
            &["claude-rs", "--perf-log", "perf.jsonl"],
            &["claude-rs", "--perf-append"],
        ];
        for args in cases {
            let error = Cli::try_parse_from(args.iter().copied()).expect_err("removed flag");
            assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
        }
    }

    #[test]
    fn cli_help_omits_perf_flags_and_retains_runtime_logging() {
        let help = Cli::command().render_long_help().to_string();
        for flag in ["--enable-perf", "--perf-log", "--perf-append"] {
            assert!(!help.contains(flag), "help still advertises {flag}");
        }
        for flag in ["--enable-logs", "--log-file", "--log-append", "--diagnostics-preset"] {
            assert!(help.contains(flag), "runtime logging flag {flag} missing");
        }
    }

    #[test]
    fn cli_doctor_defaults_to_human_output() {
        let cli = Cli::try_parse_from(["claude-rs", "doctor"]).expect("parse");
        assert_eq!(cli.command, Some(Command::Doctor(DoctorArgs { json: false, strict: false })));
    }

    #[test]
    fn cli_doctor_accepts_json_and_strict() {
        let cli =
            Cli::try_parse_from(["claude-rs", "doctor", "--json", "--strict"]).expect("parse");
        assert_eq!(cli.command, Some(Command::Doctor(DoctorArgs { json: true, strict: true })));
    }

    #[test]
    fn cli_logs_defaults_to_summary() {
        let cli = Cli::try_parse_from(["claude-rs", "logs"]).expect("parse");
        assert_eq!(
            cli.command,
            Some(Command::Logs(LogsArgs {
                path: false,
                latest: false,
                tail: None,
                bundle: false,
                output: None,
                yes: false,
            }))
        );
    }

    #[test]
    fn cli_logs_accepts_modes() {
        let cli = Cli::try_parse_from(["claude-rs", "logs", "--tail", "200"]).expect("parse");
        assert_eq!(
            cli.command,
            Some(Command::Logs(LogsArgs {
                path: false,
                latest: false,
                tail: Some(200),
                bundle: false,
                output: None,
                yes: false,
            }))
        );

        let cli =
            Cli::try_parse_from(["claude-rs", "logs", "--bundle", "--yes", "--output", "out.zip"])
                .expect("parse");
        assert_eq!(
            cli.command,
            Some(Command::Logs(LogsArgs {
                path: false,
                latest: false,
                tail: None,
                bundle: true,
                output: Some(std::path::PathBuf::from("out.zip")),
                yes: true,
            }))
        );
    }

    #[test]
    fn cli_logs_rejects_conflicting_modes() {
        assert!(Cli::try_parse_from(["claude-rs", "logs", "--path", "--latest"]).is_err());
        assert!(Cli::try_parse_from(["claude-rs", "logs", "--output", "out.zip"]).is_err());
    }

    #[test]
    fn cli_config_accepts_path_modes() {
        let cli = Cli::try_parse_from(["claude-rs", "config", "path"]).expect("parse");
        assert_eq!(
            cli.command,
            Some(Command::Config(ConfigArgs {
                command: Some(ConfigCommand::Path(ConfigPathArgs { json: false, which: None })),
            }))
        );

        let cli = Cli::try_parse_from([
            "claude-rs",
            "config",
            "path",
            "--json",
            "--which",
            "local-settings",
        ])
        .expect("parse");
        assert_eq!(
            cli.command,
            Some(Command::Config(ConfigArgs {
                command: Some(ConfigCommand::Path(ConfigPathArgs {
                    json: true,
                    which: Some(ConfigFileSelector::LocalSettings),
                })),
            }))
        );
    }

    #[test]
    fn cli_config_defaults_to_summary() {
        let cli = Cli::try_parse_from(["claude-rs", "config"]).expect("parse");
        assert_eq!(cli.command, Some(Command::Config(ConfigArgs { command: None })));
    }

    #[test]
    fn cli_config_accepts_show_and_export() {
        let cli = Cli::try_parse_from(["claude-rs", "config", "show", "--json"]).expect("parse");
        assert_eq!(
            cli.command,
            Some(Command::Config(ConfigArgs {
                command: Some(ConfigCommand::Show(ConfigShowArgs { json: true })),
            }))
        );

        let cli = Cli::try_parse_from(["claude-rs", "config", "export", "--output", "config.json"])
            .expect("parse");
        assert_eq!(
            cli.command,
            Some(Command::Config(ConfigArgs {
                command: Some(ConfigCommand::Export(ConfigExportArgs {
                    output: Some(std::path::PathBuf::from("config.json")),
                })),
            }))
        );
    }

    #[test]
    fn cli_exposes_package_version() {
        assert_eq!(Cli::command().get_version(), Some(env!("CARGO_PKG_VERSION")));
    }
}
