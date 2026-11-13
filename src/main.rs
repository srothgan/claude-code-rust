// SPDX-License-Identifier: Apache-2.0

use clap::Parser;
use claude_code_rust::Cli;
use claude_code_rust::app::PostExitAction;
use claude_code_rust::error::AppError;
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
    if let Err(error) = cli.validate() {
        error.print()?;
        return Ok(error.exit_code());
    }
    if let Some(exit_code) = claude_code_rust::cli::run_support_command(
        &cli,
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
    )? {
        return Ok(exit_code);
    }
    // Before logging, the bridge and the terminal: a bad flag must stop the launch with nothing
    // started or written.
    let session_config = match cli.resolve_session_config() {
        Ok(session_config) => session_config,
        Err(error) => {
            error.print()?;
            return Ok(error.exit_code());
        }
    };

    let _logging = claude_code_rust::logging::LoggingRuntime::init(&cli)?;
    {
        let startup_bootstrap_span = info_span!(
            target: claude_code_rust::logging::targets::APP_LIFECYCLE,
            "startup_bootstrap",
            resume_requested = cli.startup_launch().resume_requested(),
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
        let mut app = claude_code_rust::app::create_app(&cli, session_config);

        // Phase 2: start non-session startup work + TUI.
        // The bridge itself is started from the TUI loop only after trust is accepted.
        claude_code_rust::app::start_update_check(&app, &cli);
        let result = claude_code_rust::app::run_tui(&mut app).await;
        let post_exit_action = app.post_exit_action.take();
        maybe_print_resume_hint(&app);

        // Kill any spawned terminal child processes before exiting

        if let Some(app_error) = app.exit_error.take() {
            return Err(anyhow::Error::new(app_error));
        }

        result?;

        if let Some(action) = post_exit_action {
            return Ok(run_post_exit_action(&app, action));
        }
        if let Some(action) = claude_code_rust::app::automatic_update_action(&app, &cli) {
            start_automatic_update(&app, action);
        }

        Ok(0)
    }))?;

    Ok(exit_code)
}

fn run_post_exit_action(app: &claude_code_rust::app::App, action: PostExitAction) -> i32 {
    match action {
        PostExitAction::InstallUpdate { latest_version, method } => {
            claude_code_rust::app::run_update_install(app, &latest_version, &method)
        }
    }
}

/// Automatic installs run in a detached worker so the shell prompt returns at once.
fn start_automatic_update(app: &claude_code_rust::app::App, action: PostExitAction) {
    match action {
        PostExitAction::InstallUpdate { latest_version, method } => {
            claude_code_rust::app::start_background_update_install(app, &latest_version, &method);
        }
    }
}

fn extract_app_error(err: &anyhow::Error) -> Option<AppError> {
    err.chain().find_map(|cause| cause.downcast_ref::<AppError>().cloned())
}

fn maybe_print_resume_hint(app: &claude_code_rust::app::App) {
    let mut stderr = std::io::stderr().lock();
    if let Err(err) = write_resume_hint_for_app(&mut stderr, app) {
        tracing::warn!(
            target: claude_code_rust::logging::targets::APP_LIFECYCLE,
            event_name = "resume_hint_write_failed",
            message = "failed to write resume hint",
            outcome = "failure",
            error_message = %err,
        );
    }
}

fn write_resume_hint_for_app(
    mut writer: impl std::io::Write,
    app: &claude_code_rust::app::App,
) -> std::io::Result<()> {
    let Some(session_id) = app.session_runtime.resumable_session_id() else {
        return Ok(());
    };
    write_resume_hint(&mut writer, session_id)
}

fn write_resume_hint(
    mut writer: impl std::io::Write,
    session_id: impl std::fmt::Display,
) -> std::io::Result<()> {
    writeln!(writer, "\r\nResume this session: claude-rs resume {session_id}")
}

#[cfg(test)]
mod tests {
    use super::write_resume_hint_for_app;
    use claude_code_rust::app::App;

    #[tokio::test]
    async fn resume_hint_consumes_established_replaced_and_retained_session_identity() {
        use claude_code_rust::agent::{events::ClientEvent, model};
        use claude_code_rust::app::handle_client_event;
        const CHILD: &str = "CLAUDE_RS_RESUME_HINT_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let isolated = tempfile::tempdir().unwrap();
            let output = tokio::time::timeout(
                std::time::Duration::from_secs(30),
                tokio::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "tests::resume_hint_consumes_established_replaced_and_retained_session_identity", "--nocapture"])
                    .env(CHILD, "1")
                    .env("CLAUDE_CONFIG_DIR", isolated.path().join("profile"))
                    .env("HOME", isolated.path())
                    .env("USERPROFILE", isolated.path())
                    .kill_on_drop(true)
                    .output(),
            ).await.unwrap().unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            return;
        }
        let project = tempfile::tempdir().unwrap();
        let mut app = App::test_default();

        let mut empty = Vec::new();
        write_resume_hint_for_app(&mut empty, &app).unwrap();
        assert!(empty.is_empty());
        for (id, replacement) in [("first-session", false), ("replacement-session", true)] {
            let session_id = model::SessionId::new(id);
            let cwd = project.path().to_string_lossy().into_owned();
            let event = if replacement {
                ClientEvent::SessionReplaced {
                    session_id,
                    cwd,
                    current_model: model::CurrentModel::new("model", "Model", "Model"),
                    available_models: vec![],
                    mode: None,
                    fast_mode_state: model::FastModeState::Off,
                    fast_mode_disabled_reason: None,
                    ultracode: None,
                    history_updates: vec![],
                    restored_input: None,
                }
            } else {
                ClientEvent::Connected {
                    session_id,
                    cwd,
                    current_model: model::CurrentModel::new("model", "Model", "Model"),
                    available_models: vec![],
                    mode: None,
                    fast_mode_state: model::FastModeState::Off,
                    fast_mode_disabled_reason: None,
                    ultracode: None,
                    history_updates: vec![],
                }
            };
            handle_client_event(&mut app, event);
            for clear in [false, true] {
                if clear {
                    app.session_runtime.clear_identity();
                }
                let mut bytes = Vec::new();
                write_resume_hint_for_app(&mut bytes, &app).unwrap();
                let text = String::from_utf8(bytes).unwrap();
                assert!(text.starts_with("\r\n") && text.ends_with('\n'));
                let line = text.lines().find(|line| line.contains("claude-rs resume ")).unwrap();
                assert_eq!(line.split_once("claude-rs resume ").unwrap().1, id);
                if replacement {
                    assert!(!text.contains("first-session"));
                }
            }
        }
    }
}
