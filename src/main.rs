// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use clap::Parser;
use claude_code_rust::Cli;
use claude_code_rust::app::PostExitAction;
use claude_code_rust::error::AppError;
use std::process::Stdio;
use std::time::Instant;
use tracing::info_span;

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
        claude_code_rust::agent::events::kill_all_terminals(&app.terminals);

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
        PostExitAction::InstallUpdate { latest_version } => {
            run_update_install(app, &latest_version)
        }
    }
}

fn run_update_install(app: &mut claude_code_rust::app::App, latest_version: &str) -> i32 {
    let npm = match resolve_npm() {
        Ok(path) => path,
        Err(error) => {
            let message = format!("Failed to resolve npm for update install: {error}");
            eprintln!("{message}");
            record_install_failure(app, message);
            return 1;
        }
    };

    let status = std::process::Command::new(&npm)
        .args(["install", "-g", "claude-code-rust"])
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status();

    match status {
        Ok(status) if status.success() => {
            clear_install_failure(app);
            0
        }
        Ok(status) => {
            let code = status.code().unwrap_or(1);
            let message =
                format!("Update install for v{latest_version} exited with status {status}.");
            eprintln!("{message}");
            record_install_failure(app, message);
            code
        }
        Err(error) => {
            let message = format!("Failed to run update install for v{latest_version}: {error}");
            eprintln!("{message}");
            record_install_failure(app, message);
            1
        }
    }
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
