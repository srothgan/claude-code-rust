// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use clap::Parser;
use claude_code_rust::Cli;
use claude_code_rust::app::PostExitAction;
use claude_code_rust::error::AppError;
use claude_code_rust::install_method::InstallMethod;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::Instant;
use tracing::info_span;

#[cfg(not(target_os = "windows"))]
const UNIX_INSTALLER_URL: &str =
    "https://raw.githubusercontent.com/srothgan/claude-code-rust/main/scripts/install/install.sh";
#[cfg(target_os = "windows")]
const WINDOWS_INSTALLER_URL: &str =
    "https://raw.githubusercontent.com/srothgan/claude-code-rust/main/scripts/install/install.ps1";

#[allow(clippy::exit)]
fn main() {
    claude_code_rust::failure::install_panic_hook();
    match run() {
        Ok(0) => {}
        Ok(code) => std::process::exit(code),
        Err(err) => {
            if let Some(app_error) = extract_app_error(&err) {
                let mut stderr = std::io::stderr().lock();
                let detail = format!("{err:#}");
                if let Err(report_error) =
                    claude_code_rust::failure::write_app_error_report_with_detail(
                        &mut stderr,
                        &app_error,
                        Some(&detail),
                    )
                {
                    eprintln!("{}", app_error.user_message());
                    eprintln!("failed to write failure report: {report_error}");
                }
                std::process::exit(app_error.exit_code());
            }
            eprintln!("{err:#}");
            std::process::exit(1);
        }
    }
}

fn run() -> anyhow::Result<i32> {
    let cli = Cli::parse();
    if let Some(exit_code) = claude_code_rust::cli::run_support_command(
        &cli,
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
    )? {
        return Ok(exit_code);
    }

    let _logging = claude_code_rust::logging::LoggingRuntime::init(&cli)?;
    let perf_path = claude_code_rust::logging::resolve_perf_path(&cli)?;

    #[cfg(not(feature = "perf"))]
    if perf_path.is_some() {
        return Err(anyhow::anyhow!(
            "perf telemetry requires a binary built with `--features perf`"
        ));
    }

    {
        let startup_bootstrap_span = info_span!(
            target: claude_code_rust::logging::targets::APP_LIFECYCLE,
            "startup_bootstrap",
            resume_requested = matches!(
                cli.command,
                Some(claude_code_rust::Command::Resume { .. })
            ),
            perf_telemetry_requested = perf_path.is_some(),
            explicit_bridge_script = cli.bridge_script.is_some(),
        );
        let _entered = startup_bootstrap_span.enter();
        let resolve_started = Instant::now();
        let bridge_launcher =
            claude_code_rust::agent::bridge::resolve_bridge_launcher(cli.bridge_script.as_deref())?;
        let duration_ms = u64::try_from(resolve_started.elapsed().as_millis()).unwrap_or(u64::MAX);
        tracing::info!(
            target: claude_code_rust::logging::targets::BRIDGE_LIFECYCLE,
            event_name = "bridge_launcher_resolved",
            message = "resolved agent bridge launcher",
            duration_ms,
            launcher = %bridge_launcher.describe(),
        );
    }

    let rt = tokio::runtime::Runtime::new()?;
    let local_set = tokio::task::LocalSet::new();

    let exit_code = rt.block_on(local_set.run_until(async move {
        // Phase 1: create app in Connecting state (instant, no I/O)
        let mut app = claude_code_rust::app::create_app(&cli);

        // Phase 2: start non-session startup work + TUI.
        // The bridge itself is started from the TUI loop only after trust is accepted.
        claude_code_rust::app::start_update_check(&app, &cli);
        let result = claude_code_rust::app::run_tui(&mut app).await;
        let post_exit_action = app.post_exit_action.take();
        maybe_print_resume_hint(&app, result.is_ok() && post_exit_action.is_none());

        // Kill any spawned terminal child processes before exiting

        if let Some(app_error) = app.exit_error.take() {
            return Err(anyhow::Error::new(app_error));
        }

        result?;

        if let Some(action) = post_exit_action {
            return Ok(run_post_exit_action(&mut app, action));
        }

        Ok(0)
    }))?;

    Ok(exit_code)
}

fn run_post_exit_action(app: &mut claude_code_rust::app::App, action: PostExitAction) -> i32 {
    match action {
        PostExitAction::InstallUpdate { latest_version, method } => {
            run_update_install(app, &latest_version, method)
        }
    }
}

fn run_update_install(
    app: &mut claude_code_rust::app::App,
    latest_version: &str,
    method: InstallMethod,
) -> i32 {
    let method_label = method.label();
    let result = match method {
        InstallMethod::Npm => run_npm_update(),
        InstallMethod::Script { install_dir } => {
            run_script_update(latest_version, install_dir.as_deref())
        }
        InstallMethod::Unknown => Err("no update install method was selected".to_owned()),
    };

    match result {
        Ok(status) if status.success() => {
            clear_install_failure(app);
            0
        }
        Ok(status) => {
            let code = status.code().unwrap_or(1);
            let message = format!(
                "{method_label} update install for v{latest_version} exited with status {status}."
            );
            eprintln!("{message}");
            record_install_failure(app, message);
            code
        }
        Err(error) => {
            let message = format!(
                "Failed to run {method_label} update install for v{latest_version}: {error}"
            );
            eprintln!("{message}");
            record_install_failure(app, message);
            1
        }
    }
}

fn run_npm_update() -> Result<ExitStatus, String> {
    let npm = resolve_npm().map_err(|error| format!("failed to resolve npm: {error}"))?;
    Command::new(&npm)
        .args(["install", "-g", "claude-code-rust"])
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|error| format!("failed to start {}: {error}", npm.display()))
}

#[cfg(not(target_os = "windows"))]
fn run_script_update(
    latest_version: &str,
    install_dir: Option<&Path>,
) -> Result<ExitStatus, String> {
    let installer = download_unix_installer()?;
    let mut command = Command::new("sh");
    command
        .arg(&installer.script)
        .args(["--release", latest_version, "--yes", "--keep-npm"])
        .env_remove("CLAUDE_RS_RELEASE")
        .env_remove("CLAUDE_RS_INSTALL_DIR")
        .env_remove("CLAUDE_RS_BIN_DIR")
        .env_remove("CLAUDE_RS_NO_MODIFY_PATH")
        .env_remove("CLAUDE_RS_REMOVE_NPM")
        .env_remove("CLAUDE_RS_RUN")
        .env_remove("CLAUDE_RS_UNINSTALL")
        .env_remove("CLAUDE_RS_UPDATE")
        .env_remove("CLAUDE_RS_VERIFY")
        .env("CLAUDE_RS_NON_INTERACTIVE", "1")
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    if let Some(install_dir) = install_dir {
        command.arg("--update").arg("--install-dir").arg(install_dir);
    }
    command.status().map_err(|error| format!("failed to start install script: {error}"))
}

#[cfg(target_os = "windows")]
fn run_script_update(
    latest_version: &str,
    install_dir: Option<&Path>,
) -> Result<ExitStatus, String> {
    let powershell = resolve_powershell()?;
    let mut command = Command::new(&powershell);
    command
        .args([
            "-NoLogo",
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &format!(
                "$ProgressPreference='SilentlyContinue'; Invoke-Expression (Invoke-RestMethod -Uri '{WINDOWS_INSTALLER_URL}')"
            ),
        ])
        .env_remove("CLAUDE_RS_INSTALL_DIR")
        .env_remove("CLAUDE_RS_NO_MODIFY_PATH")
        .env_remove("CLAUDE_RS_REMOVE_NPM")
        .env_remove("CLAUDE_RS_RUN")
        .env_remove("CLAUDE_RS_UNINSTALL")
        .env_remove("CLAUDE_RS_UPDATE")
        .env_remove("CLAUDE_RS_UPDATE_PARENT_PID")
        .env_remove("CLAUDE_RS_VERIFY")
        .env("CLAUDE_RS_RELEASE", latest_version)
        .env("CLAUDE_RS_NON_INTERACTIVE", "1")
        .env("CLAUDE_RS_KEEP_NPM", "1")
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    if let Some(install_dir) = install_dir {
        command
            .env("CLAUDE_RS_INSTALL_DIR", install_dir)
            .env("CLAUDE_RS_UPDATE", "1")
            .env("CLAUDE_RS_UPDATE_PARENT_PID", std::process::id().to_string());
    }
    command.status().map_err(|error| format!("failed to start {}: {error}", powershell.display()))
}

#[cfg(not(target_os = "windows"))]
struct DownloadedInstaller {
    root: PathBuf,
    script: PathBuf,
}

#[cfg(not(target_os = "windows"))]
impl Drop for DownloadedInstaller {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[cfg(not(target_os = "windows"))]
fn download_unix_installer() -> Result<DownloadedInstaller, String> {
    let root = create_update_temp_dir()?;
    let script = root.join("install.sh");
    let installer = DownloadedInstaller { root, script };
    let (downloader, args): (PathBuf, Vec<std::ffi::OsString>) =
        if let Ok(curl) = which::which("curl") {
            (
                curl,
                vec![
                    "-fsSL".into(),
                    UNIX_INSTALLER_URL.into(),
                    "-o".into(),
                    installer.script.as_os_str().to_owned(),
                ],
            )
        } else if let Ok(wget) = which::which("wget") {
            (
                wget,
                vec![
                    "-q".into(),
                    "-O".into(),
                    installer.script.as_os_str().to_owned(),
                    UNIX_INSTALLER_URL.into(),
                ],
            )
        } else {
            return Err("neither curl nor wget was found in PATH".to_owned());
        };

    let status = Command::new(&downloader)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|error| format!("failed to start {}: {error}", downloader.display()))?;
    if !status.success() {
        return Err(format!("installer download exited with status {status}"));
    }
    if !installer.script.is_file() {
        return Err("installer download did not create install.sh".to_owned());
    }
    Ok(installer)
}

#[cfg(not(target_os = "windows"))]
fn create_update_temp_dir() -> Result<PathBuf, String> {
    let base = std::env::temp_dir();
    for attempt in 0..100_u32 {
        let candidate = base.join(format!("claude-rs-update-{}-{attempt}", std::process::id()));
        match std::fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(format!("failed to create {}: {error}", candidate.display()));
            }
        }
    }
    Err("could not allocate a temporary update directory".to_owned())
}

fn resolve_npm() -> Result<std::path::PathBuf, String> {
    #[cfg(target_os = "windows")]
    let candidates = ["npm.cmd", "npm"];
    #[cfg(not(target_os = "windows"))]
    let candidates = ["npm"];

    candidates
        .iter()
        .find_map(|candidate| which::which(candidate).ok())
        .ok_or_else(|| format!("none of {} were found in PATH", candidates.join(", ")))
}

#[cfg(target_os = "windows")]
fn resolve_powershell() -> Result<PathBuf, String> {
    ["powershell.exe", "pwsh.exe"]
        .iter()
        .find_map(|candidate| which::which(candidate).ok())
        .ok_or_else(|| "neither powershell.exe nor pwsh.exe was found in PATH".to_owned())
}

fn record_install_failure(app: &mut claude_code_rust::app::App, message: String) {
    claude_code_rust::app::record_update_install_failure(app, message);
}

fn clear_install_failure(app: &mut claude_code_rust::app::App) {
    claude_code_rust::app::clear_update_install_failure(app);
}

fn extract_app_error(err: &anyhow::Error) -> Option<AppError> {
    err.chain().find_map(|cause| cause.downcast_ref::<AppError>().cloned())
}

fn maybe_print_resume_hint(app: &claude_code_rust::app::App, success: bool) {
    if !success {
        return;
    }
    let Some(session_id) = app.session_runtime.session_id.as_ref() else {
        return;
    };
    let mut stderr = std::io::stderr().lock();
    if let Err(err) = write_resume_hint(&mut stderr, session_id) {
        tracing::warn!(
            target: claude_code_rust::logging::targets::APP_LIFECYCLE,
            event_name = "resume_hint_write_failed",
            message = "failed to write resume hint",
            outcome = "failure",
            error_message = %err,
        );
    }
}

fn write_resume_hint(
    mut writer: impl std::io::Write,
    session_id: impl std::fmt::Display,
) -> std::io::Result<()> {
    writeln!(writer, "\r\nResume this session: claude-rs resume {session_id}")
}

#[cfg(test)]
mod tests {
    use super::write_resume_hint;

    #[test]
    fn resume_hint_starts_on_fresh_line_and_ends_with_newline() {
        let mut output = Vec::new();

        assert!(write_resume_hint(&mut output, "abc-123").is_ok());

        assert_eq!(output, b"\r\nResume this session: claude-rs resume abc-123\n");
    }
}
