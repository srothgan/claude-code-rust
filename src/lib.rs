// SPDX-License-Identifier: Apache-2.0

// The test harness hands every test to its runner as one generated local array of references.
// Past 2048 tests that array exceeds Clippy's 16 KiB `large_stack_arrays` limit, which it reports
// without a span. Test builds only; production code keeps the lint.
#![cfg_attr(test, allow(clippy::large_stack_arrays))]

pub mod agent;
pub mod app;
mod claude_paths;
pub mod cli;
pub mod error;
pub mod failure;
pub mod install_method;
mod json_file;
pub mod logging;
mod markdown;
pub mod permission_mode;
pub mod session_config;
pub mod ui;

use clap::{Args, Parser, Subcommand, ValueEnum};
pub use session_config::SessionConfigArgs;

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
                "info,bridge.lifecycle=debug,bridge.protocol=debug,app.session=debug,app.tool=debug,app.command=debug,app.permission=debug,app.notify=debug,app.network=debug,app.update=debug,app.file_index=debug"
            }
            Self::Session => {
                "info,bridge.lifecycle=debug,bridge.protocol=debug,app.session=debug,app.permission=debug,app.notify=debug,app.command=debug"
            }
            Self::Render => "info,app.render=trace,app.cache=debug,app.input=debug,app.paste=debug",
            Self::Bridge => {
                "info,bridge.lifecycle=debug,bridge.protocol=debug,bridge.sdk=debug,bridge.permission=debug,bridge.mcp=debug"
            }
            Self::Full => {
                "info,app.render=trace,bridge.lifecycle=debug,bridge.protocol=debug,bridge.sdk=debug,bridge.permission=debug,bridge.mcp=debug,app.session=debug,app.tool=debug,app.command=debug,app.permission=debug,app.notify=debug,app.network=debug,app.update=debug,app.cache=debug,app.input=debug,app.paste=debug,app.config=debug,app.auth=debug,app.file_index=debug"
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

    #[command(flatten)]
    pub session_config: SessionConfigArgs,

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
    ///
    /// The baseline log already records warnings and errors from the app, the bridge and the SDK.
    /// Run `claude-rs logs` to find it.
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

    /// Name a new session, or rename the one being resumed.
    #[arg(long, short = 'n', global = true, value_name = "TEXT", value_parser = parse_session_name)]
    pub name: Option<String>,

    /// Resume under a new session ID, leaving the original unchanged.
    #[arg(long, global = true)]
    pub fork_session: bool,
}

/// Session names are shown in the terminal, so they go through the one title sanitiser.
fn parse_session_name(value: &str) -> Result<String, String> {
    session_config::sanitize_session_title(value)
        .ok_or_else(|| "the session name must contain visible characters".to_owned())
}

impl Cli {
    pub fn validate(&self) -> Result<(), clap::Error> {
        use clap::CommandFactory;
        let has_session_options = self.session_options.model.is_some()
            || self.session_options.effort.is_some()
            || self.session_options.permission_mode.is_some()
            || self.session_options.agent.is_some()
            || self.session_options.name.is_some()
            || self.session_options.fork_session
            || self.session_config.is_set();
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
        if self.session_options.fork_session && !self.startup_launch().resume_or_pick() {
            return Err(Self::command().error(
                clap::error::ErrorKind::MissingRequiredArgument,
                "--fork-session requires --resume, --continue or the resume subcommand",
            ));
        }
        if matches!(self.command, None | Some(Command::Resume { .. })) {
            self.validate_launch_paths()?;
        }
        Ok(())
    }

    /// The session working directory: [`Cli::working_dir`], or `.` when the process
    /// directory cannot be read.
    #[must_use]
    pub fn startup_cwd(&self) -> std::path::PathBuf {
        self.working_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
    }

    /// Resolve the session configuration flags against the session working directory.
    pub fn resolve_session_config(&self) -> Result<agent::wire::SessionConfig, clap::Error> {
        use clap::CommandFactory;
        self.session_config.resolve(&self.startup_cwd()).map_err(|message| {
            Self::command().error(clap::error::ErrorKind::ValueValidation, message)
        })
    }

    /// Checks `--dir` and `--bridge-script` before the terminal, trust prompt or bridge
    /// exist, so a bad path is a usage error instead of a failure further in. Only
    /// interactive launches use them: `doctor` reports on a bad bridge script rather
    /// than refusing it, and the other support commands never start the bridge.
    fn validate_launch_paths(&self) -> Result<(), clap::Error> {
        use clap::CommandFactory;
        let usage_error = |id: &str, reason: String| {
            let mut command = Self::command();
            command.build();
            // clap renders the label the way its own value errors do ("--dir <DIR>").
            let flag = command
                .get_arguments()
                .find(|arg| arg.get_id() == id)
                .map_or_else(|| id.to_owned(), ToString::to_string);
            command.error(
                clap::error::ErrorKind::ValueValidation,
                format!("invalid value for '{flag}': {reason}"),
            )
        };
        if let Some(reason) = self.dir.as_deref().and_then(working_dir_problem) {
            return Err(usage_error("dir", reason));
        }
        // The same inspection `doctor` reports, so the two judge a script alike.
        if let Some(reason) = self
            .bridge_script
            .as_deref()
            .and_then(|script| agent::bridge::inspect_bridge_script(Some(script)).error)
        {
            return Err(usage_error("bridge_script", reason));
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

    /// The directory `--dir` names, or the process directory without it. The one answer
    /// for the session, `config` and `doctor`, so they never disagree about `--dir .`.
    ///
    /// # Errors
    ///
    /// Fails when the process directory is needed and cannot be read.
    pub fn working_dir(&self) -> std::io::Result<std::path::PathBuf> {
        resolve_working_dir(self.dir.as_deref(), std::env::current_dir)
    }
}

/// An absolute `--dir` is used as given. A relative one is joined to the process directory
/// and resolved the way the platform resolves its own working directory, so `--dir .`
/// names the same directory as no `--dir` at all: physically on Unix, where the process
/// directory is already physical, and lexically on Windows, which keeps a mapped drive
/// letter and adds no `\\?\` prefix. A relative `--dir` that cannot be resolved (support
/// commands do not validate it, and a launch's may vanish after validation) is kept
/// joined, so the failure surfaces where the directory is used.
fn resolve_working_dir(
    dir: Option<&std::path::Path>,
    process_cwd: impl FnOnce() -> std::io::Result<std::path::PathBuf>,
) -> std::io::Result<std::path::PathBuf> {
    let Some(dir) = dir else {
        return process_cwd();
    };
    if dir.is_absolute() {
        return Ok(dir.to_path_buf());
    }
    let joined = process_cwd()?.join(dir);
    Ok(resolve_relative_dir(&joined).unwrap_or(joined))
}

#[cfg(windows)]
fn resolve_relative_dir(joined: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    std::path::absolute(joined)
}

#[cfg(not(windows))]
fn resolve_relative_dir(joined: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    joined.canonicalize()
}

/// Why `dir` cannot be the session's working directory, judged the way the session will
/// use it: relative to the current directory, with no `~` expansion of its own. The
/// session lists the directory (read permission) and enters it and opens files inside it
/// (search permission), so both are required.
fn working_dir_problem(dir: &std::path::Path) -> Option<String> {
    let shown = dir.display();
    match std::fs::metadata(dir) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Some(format!("directory does not exist: {shown}"));
        }
        Err(err) => return Some(format!("directory cannot be accessed: {shown}: {err}")),
        Ok(metadata) if !metadata.is_dir() => return Some(format!("not a directory: {shown}")),
        Ok(_) => {}
    }
    if let Err(err) = std::fs::read_dir(dir) {
        return Some(format!("directory cannot be read: {shown}: {err}"));
    }
    // Looking up `.` inside the directory is a path lookup through it, which needs search
    // permission and nothing else: a directory with read but no search permission lists
    // but fails here, as entering it would. Windows has no search bit and passes.
    std::fs::metadata(dir.join("."))
        .err()
        .map(|err| format!("directory cannot be entered: {shown}: {err}"))
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

    /// Whether the launch resumes an existing session, directly or through the picker.
    #[must_use]
    pub fn resume_or_pick(&self) -> bool {
        self.resume_requested() || matches!(self, Self::SessionPicker)
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
    /// Install an update after exit, detached from the terminal.
    #[command(hide = true)]
    UpdateWorker(UpdateWorkerArgs),
    /// Run deterministic installation and runtime diagnostics
    Doctor(DoctorArgs),
    /// Find runtime logs or create a redacted debug bundle
    Logs(LogsArgs),
    /// Inspect and export redacted configuration
    Config(ConfigArgs),
}

#[derive(Args, Clone, Debug, PartialEq, Eq)]
pub struct UpdateWorkerArgs {
    /// Version to install.
    pub version: String,

    /// Installation to update.
    #[arg(long, value_enum)]
    pub method: UpdateWorkerMethod,

    /// Script installation directory to update in place.
    #[arg(long)]
    pub install_dir: Option<std::path::PathBuf>,
}

#[derive(Clone, Copy, Debug, ValueEnum, PartialEq, Eq)]
pub enum UpdateWorkerMethod {
    Npm,
    Script,
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

    /// The fixture root as the platform spells its own working directory: physical on Unix
    /// (macOS temp dirs sit behind a symlink), as given on Windows.
    fn platform_root(path: &std::path::Path) -> std::path::PathBuf {
        if cfg!(windows) {
            path.to_path_buf()
        } else {
            path.canonicalize().expect("canonical tempdir")
        }
    }

    #[test]
    fn a_relative_dir_resolves_to_the_absolute_directory_it_names() {
        let base = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(base.path().join("project").join("sub")).expect("project");
        // Every expected child is spelled out by name under the fixture root.
        let root = platform_root(base.path());
        let cases = [
            (".", root.clone()),
            ("project", root.join("project")),
            ("./project/./sub", root.join("project").join("sub")),
            ("project/sub/..", root.join("project")),
        ];
        for (dir, expected) in cases {
            let resolved = super::resolve_working_dir(Some(std::path::Path::new(dir)), || {
                Ok(base.path().into())
            })
            .expect("resolve");
            assert_eq!(resolved, expected, "--dir {dir}");
        }
    }

    #[test]
    fn an_absolute_dir_is_used_as_given_even_without_a_process_dir() {
        let base = tempfile::tempdir().expect("tempdir");
        let project = base.path().join("project");
        std::fs::create_dir_all(&project).expect("project");

        let resolved = super::resolve_working_dir(Some(&project), || {
            Err(std::io::Error::other("process directory unavailable"))
        })
        .expect("absolute dir needs no process dir");

        assert_eq!(resolved, project);
    }

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
    fn system_prompt_text_and_file_are_mutually_exclusive() {
        for args in [
            ["claude-rs", "--system-prompt", "a", "--system-prompt-file", "p.md"],
            ["claude-rs", "--system-prompt-file", "p.md", "--system-prompt", "a"],
        ] {
            let error = Cli::try_parse_from(args).expect_err("conflicting prompt spellings");
            assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict, "{args:?}");
        }
    }

    #[test]
    fn append_system_prompt_text_and_file_are_mutually_exclusive() {
        for args in [
            ["claude-rs", "--append-system-prompt", "a", "--append-system-prompt-file", "a.md"],
            ["claude-rs", "--append-system-prompt-file", "a.md", "--append-system-prompt", "a"],
        ] {
            let error = Cli::try_parse_from(args).expect_err("conflicting append spellings");
            assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict, "{args:?}");
        }
    }

    #[test]
    fn replacing_and_appending_the_system_prompt_can_be_combined() {
        let cli = Cli::try_parse_from([
            "claude-rs",
            "--system-prompt",
            "a",
            "--append-system-prompt-file",
            "a.md",
        ])
        .expect("one of each pair");
        cli.validate().expect("valid launch");
    }

    #[test]
    fn fork_session_is_accepted_with_every_resume_spelling() {
        let cases: &[&[&str]] = &[
            &["claude-rs", "--resume", "--fork-session"],
            &["claude-rs", "-r", "session-1", "--fork-session"],
            &["claude-rs", "--continue", "--fork-session"],
            &["claude-rs", "resume", "--fork-session"],
            &["claude-rs", "resume", "session-1", "--fork-session"],
        ];
        for args in cases {
            let cli = Cli::try_parse_from(args.iter().copied()).expect("parse");
            cli.validate().expect("fork with a resume");
            assert!(cli.session_options.fork_session, "{args:?}");
        }
    }

    #[test]
    fn fork_session_without_a_resume_is_rejected() {
        for args in [
            &["claude-rs", "--fork-session"][..],
            &["claude-rs", "--fork-session", "explain this project"][..],
            &["claude-rs", "--fork-session", "--name", "Experiment"][..],
        ] {
            let cli = Cli::try_parse_from(args.iter().copied()).expect("parse");
            let error = cli.validate().expect_err("fork needs a resume");
            assert!(error.to_string().contains("--fork-session requires"), "{error}");
        }
    }

    #[test]
    fn launch_flags_belong_to_interactive_launches_only() {
        let cases: &[&[&str]] = &[
            &["claude-rs", "doctor", "--name", "Experiment"],
            &["claude-rs", "doctor", "--fork-session"],
            &["claude-rs", "logs", "--add-dir", "src"],
            &["claude-rs", "config", "--settings", "{}"],
            &["claude-rs", "doctor", "--mcp-config", "{\"mcpServers\":{}}"],
            &["claude-rs", "doctor", "--strict-mcp-config"],
            &["claude-rs", "doctor", "--append-system-prompt", "a"],
            &["claude-rs", "doctor", "--system-prompt-file", "p.md"],
        ];
        for args in cases {
            let cli = Cli::try_parse_from(args.iter().copied()).expect("parse");
            let error = cli.validate().expect_err("launch flag with a support command");
            assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict, "{args:?}");
        }
    }

    #[test]
    fn launch_flags_before_and_after_resume_parse_to_the_same_launch() {
        let before = Cli::try_parse_from([
            "claude-rs",
            "--add-dir",
            "src",
            "-n",
            "Experiment",
            "resume",
            "session-1",
        ])
        .expect("before");
        let after = Cli::try_parse_from([
            "claude-rs",
            "resume",
            "session-1",
            "--add-dir",
            "src",
            "-n",
            "Experiment",
        ])
        .expect("after");
        for cli in [&before, &after] {
            cli.validate().expect("valid launch");
            assert_eq!(cli.session_config.add_dirs, [std::path::PathBuf::from("src")]);
            assert_eq!(cli.session_options.name.as_deref(), Some("Experiment"));
        }
    }

    #[test]
    fn session_names_lose_control_characters_and_must_keep_visible_text() {
        let cli = Cli::try_parse_from(["claude-rs", "-n", "Release\u{7} prep "]).expect("name");
        assert_eq!(cli.session_options.name.as_deref(), Some("Release prep"));
        for name in ["", "\u{1b}\t "] {
            assert!(Cli::try_parse_from(["claude-rs", "--name", name]).is_err(), "{name:?}");
        }
    }

    #[test]
    fn session_configuration_resolves_against_the_working_directory_option() {
        let project = tempfile::tempdir().expect("project");
        std::fs::create_dir(project.path().join("docs")).expect("docs");
        let dir = project.path().to_str().expect("UTF-8 path");
        let cli =
            Cli::try_parse_from(["claude-rs", "-C", dir, "--add-dir", "docs"]).expect("parse");
        let config = cli.resolve_session_config().expect("resolved");
        assert_eq!(
            config.additional_directories,
            [project.path().join("docs").to_str().expect("UTF-8 path")]
        );
        let cli =
            Cli::try_parse_from(["claude-rs", "-C", dir, "--add-dir", "absent"]).expect("parse");
        let error = cli.resolve_session_config().expect_err("missing directory");
        assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
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
