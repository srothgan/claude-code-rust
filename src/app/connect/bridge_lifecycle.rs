// SPDX-License-Identifier: Apache-2.0

//! Bridge process lifecycle: spawning, initialization handshake, event loop,
//! and connection slot management.

use crate::agent::bridge::BridgeLauncher;
use crate::agent::client::{AgentConnection, BridgeClient};
use crate::agent::events::ClientEvent;
use crate::agent::wire::{BridgeCommand, BridgeEvent, CommandEnvelope};
use crate::error::AppError;
use std::rc::Rc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{Instrument as _, info_span};

use super::event_dispatch::handle_bridge_event;
use super::{ConnectionSlot, StartConnectionParams, extract_app_error};

const BRIDGE_GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

#[cfg(test)]
#[path = "bridge_lifecycle_audit_tests.rs"]
mod audit_tests;

pub(super) async fn run_connection_task(
    params: StartConnectionParams,
    conn_slot_writer: Rc<std::cell::RefCell<Option<ConnectionSlot>>>,
    mut shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) {
    let request_kind = if params.launch.resume_requested() { "resume" } else { "create" };
    let session_id = params.launch.resume_id().unwrap_or_default().to_owned();
    let connection_span = info_span!(
        target: crate::logging::targets::BRIDGE_LIFECYCLE,
        "bridge_connection",
        request_kind,
        resume_requested = params.launch.resume_requested(),
        session_id = %session_id,
        cwd = %params.cwd_raw,
    );

    async move {
        tracing::debug!(
            target: crate::logging::targets::BRIDGE_LIFECYCLE,
            event_name = "bridge_connection_task_started",
            message = "bridge connection task started",
            outcome = "start",
            request_kind,
            resume_requested = params.launch.resume_requested(),
            session_id = %session_id,
        );

        let mut bridge = None;
        {
            let connection = establish_and_drive_bridge(&params, &conn_slot_writer, &mut bridge);
            tokio::pin!(connection);
            tokio::select! {
                () = &mut connection => {}
                _ = &mut shutdown_rx => {
                    tracing::info!(
                        target: crate::logging::targets::BRIDGE_LIFECYCLE,
                        event_name = "bridge_connection_shutdown_signalled",
                        message = "bridge connection task received its shutdown signal",
                        outcome = "success",
                    );
                }
            }
        }
        conn_slot_writer.borrow_mut().take();
        if let Some(bridge) = bridge
            && let Err(err) = bridge.shutdown_and_wait(BRIDGE_GRACEFUL_SHUTDOWN_TIMEOUT).await
        {
            tracing::error!(
                target: crate::logging::targets::BRIDGE_LIFECYCLE,
                event_name = "bridge_cleanup_failed",
                message = "bridge process cleanup did not complete cleanly",
                outcome = "failure",
                error = %err,
            );
        }
    }
    .instrument(connection_span)
    .await;
}

async fn establish_and_drive_bridge(
    params: &StartConnectionParams,
    conn_slot_writer: &Rc<std::cell::RefCell<Option<ConnectionSlot>>>,
    bridge_owner: &mut Option<BridgeClient>,
) {
    let Some(launcher) = resolve_launcher(params).await else {
        return;
    };
    let Some(bridge) = spawn_bridge_client(&params.event_tx, &launcher).await else {
        return;
    };
    let bridge = bridge_owner.insert(bridge);
    drive_bridge_connection(params, conn_slot_writer, bridge).await;
}

async fn drive_bridge_connection(
    params: &StartConnectionParams,
    conn_slot_writer: &Rc<std::cell::RefCell<Option<ConnectionSlot>>>,
    bridge: &mut BridgeClient,
) {
    let mut connected_once = false;
    let connection = bridge.connection();

    if !send_initialize_command(params, bridge).await {
        return;
    }
    match wait_for_bridge_initialized(
        bridge,
        &params.event_tx,
        &connection,
        &mut connected_once,
        params.launch.resume_requested(),
    )
    .await
    {
        Ok(()) => {}
        Err(InitializationFailure::Rejected(envelope)) => {
            // A terminal SDK rejection belongs to the existing event reporter. Do not
            // follow it with a second, generic initialization failure.
            handle_bridge_event(
                &params.event_tx,
                &connection,
                &mut connected_once,
                params.launch.resume_requested(),
                *envelope,
            )
            .await;
            return;
        }
        Err(InitializationFailure::Transport(app_error)) => {
            emit_connection_failed(
                &params.event_tx,
                "Bridge did not complete initialization".to_owned(),
                app_error,
            )
            .await;
            return;
        }
    }
    if !send_session_command(params, bridge).await {
        return;
    }
    publish_connection_slot(conn_slot_writer, &connection);

    bridge_event_loop(params, bridge, &connection, &mut connected_once).await;
}

async fn resolve_launcher(params: &StartConnectionParams) -> Option<BridgeLauncher> {
    match crate::agent::bridge::resolve_bridge_launcher(params.bridge_script.as_deref()) {
        Ok(launcher) => Some(launcher),
        Err(err) => {
            tracing::error!(
                target: crate::logging::targets::BRIDGE_LIFECYCLE,
                event_name = "bridge_launcher_resolution_failed",
                message = "failed to resolve bridge launcher",
                outcome = "failure",
                error = %err,
            );
            let app_error = extract_app_error(&err).unwrap_or(AppError::BridgeSpawnFailed);
            emit_connection_failed(
                &params.event_tx,
                format!(
                    "{} Detail: {}",
                    app_error.user_message(),
                    crate::cli::redaction::redact_line(&err.to_string())
                ),
                app_error,
            )
            .await;
            None
        }
    }
}

async fn spawn_bridge_client(
    event_tx: &mpsc::Sender<ClientEvent>,
    launcher: &BridgeLauncher,
) -> Option<BridgeClient> {
    match BridgeClient::spawn(launcher) {
        Ok(client) => Some(client),
        Err(err) => {
            tracing::error!(
                target: crate::logging::targets::BRIDGE_LIFECYCLE,
                event_name = "bridge_spawn_failed",
                message = "failed to spawn bridge process",
                outcome = "failure",
                error = %err,
            );
            let app_error = extract_app_error(&err).unwrap_or(AppError::BridgeSpawnFailed);
            emit_connection_failed(
                event_tx,
                format!(
                    "{} Detail: {}",
                    app_error.user_message(),
                    crate::cli::redaction::redact_line(&err.to_string())
                ),
                app_error,
            )
            .await;
            None
        }
    }
}

fn publish_connection_slot(
    conn_slot_writer: &Rc<std::cell::RefCell<Option<ConnectionSlot>>>,
    connection: &AgentConnection,
) {
    *conn_slot_writer.borrow_mut() = Some(ConnectionSlot { conn: Rc::new(connection.clone()) });
}

async fn send_initialize_command(
    params: &StartConnectionParams,
    bridge: &mut BridgeClient,
) -> bool {
    let init_cmd = CommandEnvelope {
        request_id: None,
        command: BridgeCommand::Initialize {
            cwd: params.cwd_raw.clone(),
            metadata: std::collections::BTreeMap::new(),
        },
    };
    if let Err(err) = bridge.send(init_cmd).await {
        emit_connection_failed(
            &params.event_tx,
            format!(
                "{} Detail: {}",
                AppError::BridgeInitializationFailed.user_message(),
                crate::cli::redaction::redact_line(&err.to_string())
            ),
            AppError::BridgeInitializationFailed,
        )
        .await;
        return false;
    }
    true
}

fn build_session_command(params: &StartConnectionParams) -> CommandEnvelope {
    if let Some(resume) = params.launch.resume_id() {
        CommandEnvelope {
            request_id: None,
            command: BridgeCommand::ResumeSession {
                session_id: resume.to_owned(),
                launch_settings: params.session_launch_settings.clone(),
                metadata: std::collections::BTreeMap::new(),
            },
        }
    } else {
        CommandEnvelope {
            request_id: None,
            command: BridgeCommand::CreateSession {
                cwd: params.cwd_raw.clone(),
                resume: None,
                continue_session: matches!(params.launch, crate::StartupLaunch::ContinueSession),
                launch_settings: params.session_launch_settings.clone(),
                metadata: std::collections::BTreeMap::new(),
            },
        }
    }
}

fn log_session_connect_command_sent(params: &StartConnectionParams, command: &BridgeCommand) {
    let has_launch_overrides = !params.session_launch_settings.is_empty();
    match command {
        BridgeCommand::ResumeSession { session_id, .. } => tracing::info!(
            target: crate::logging::targets::APP_SESSION,
            event_name = "session_connect_command_sent",
            message = "session connect command sent to bridge",
            outcome = "success",
            request_kind = "resume",
            resume_requested = true,
            session_id = %session_id,
            has_launch_overrides,
        ),
        BridgeCommand::CreateSession { .. } => tracing::info!(
            target: crate::logging::targets::APP_SESSION,
            event_name = "session_connect_command_sent",
            message = "session connect command sent to bridge",
            outcome = "success",
            request_kind = "create",
            resume_requested = false,
            cwd = %params.cwd_raw,
            has_launch_overrides,
        ),
        _ => {}
    }
}

async fn send_session_command(params: &StartConnectionParams, bridge: &mut BridgeClient) -> bool {
    let command = build_session_command(params);
    if let Err(err) = bridge.send(command.clone()).await {
        emit_connection_failed(
            &params.event_tx,
            format!(
                "{} Detail: {}",
                AppError::BridgeSdkFailure.user_message(),
                crate::cli::redaction::redact_line(&err.to_string())
            ),
            AppError::BridgeSdkFailure,
        )
        .await;
        return false;
    }
    log_session_connect_command_sent(params, &command.command);
    true
}

async fn bridge_event_loop(
    params: &StartConnectionParams,
    bridge: &mut BridgeClient,
    connection: &AgentConnection,
    connected_once: &mut bool,
) {
    loop {
        match bridge.recv().await {
            Ok(Some(envelope)) => {
                let startup_failed = matches!(
                    &envelope.event,
                    BridgeEvent::ConnectionFailed { startup_failure: Some(_), .. }
                );
                handle_bridge_event(
                    &params.event_tx,
                    connection,
                    connected_once,
                    params.launch.resume_requested(),
                    envelope,
                )
                .await;
                if startup_failed {
                    break;
                }
            }
            Ok(None) => {
                tracing::error!(
                    target: crate::logging::targets::BRIDGE_LIFECYCLE,
                    event_name = "bridge_stdout_closed",
                    message = "bridge stdout closed unexpectedly",
                    outcome = "failure",
                );
                emit_connection_failed(
                    &params.event_tx,
                    AppError::BridgeStdoutClosed.user_message().to_owned(),
                    AppError::BridgeStdoutClosed,
                )
                .await;
                break;
            }
            Err(err) => {
                emit_connection_failed(
                    &params.event_tx,
                    format!(
                        "{} Detail: {}",
                        AppError::BridgeSdkFailure.user_message(),
                        crate::cli::redaction::redact_line(&err.to_string())
                    ),
                    AppError::BridgeSdkFailure,
                )
                .await;
                break;
            }
        }
    }
}

pub(super) async fn emit_connection_failed(
    event_tx: &mpsc::Sender<ClientEvent>,
    failure: impl Into<crate::agent::events::ConnectionFailure>,
    app_error: AppError,
) {
    let failure = failure.into();
    tracing::error!(
        target: crate::logging::targets::BRIDGE_LIFECYCLE,
        event_name = "bridge_failure_reported",
        message = "bridge failure reported to app",
        outcome = "failure",
        error_category = app_error.category_tag(),
        exit_code = app_error.exit_code(),
        user_message = %failure.message,
    );
    let _ = event_tx.send(ClientEvent::ConnectionFailed(failure)).await;
    let _ = event_tx.send(ClientEvent::FatalError(app_error)).await;
}

#[derive(Debug)]
enum InitializationFailure {
    Transport(AppError),
    Rejected(Box<crate::agent::wire::EventEnvelope>),
}

async fn wait_for_bridge_initialized(
    bridge: &mut BridgeClient,
    event_tx: &mpsc::Sender<ClientEvent>,
    connection: &AgentConnection,
    connected_once: &mut bool,
    resume_requested: bool,
) -> Result<(), InitializationFailure> {
    wait_for_bridge_initialized_with_timeout(
        bridge,
        event_tx,
        connection,
        connected_once,
        resume_requested,
        Duration::from_secs(10),
    )
    .await
}

async fn wait_for_bridge_initialized_with_timeout(
    bridge: &mut BridgeClient,
    event_tx: &mpsc::Sender<ClientEvent>,
    connection: &AgentConnection,
    connected_once: &mut bool,
    resume_requested: bool,
    timeout: Duration,
) -> Result<(), InitializationFailure> {
    let timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
    let initialize_span = info_span!(
        target: crate::logging::targets::BRIDGE_LIFECYCLE,
        "bridge_initialize",
        resume_requested,
        timeout_ms,
    );

    // The budget covers both protocol reads and forwarding pre-ready events. A
    // full application queue must not turn a finite handshake into an infinite wait.
    let result = tokio::time::timeout(timeout, async {
        loop {
            match bridge.recv().await {
                Ok(Some(envelope)) => {
                    if matches!(envelope.event, BridgeEvent::Initialized { .. }) {
                        return Ok(());
                    }
                    if matches!(envelope.event, BridgeEvent::ConnectionFailed { .. }) {
                        return Err(InitializationFailure::Rejected(Box::new(envelope)));
                    }
                    handle_bridge_event(
                        event_tx,
                        connection,
                        connected_once,
                        resume_requested,
                        envelope,
                    )
                    .await;
                }
                Ok(None) => {
                    return Err(InitializationFailure::Transport(AppError::BridgeStdoutClosed));
                }
                Err(_) => return Err(InitializationFailure::Transport(AppError::BridgeSdkFailure)),
            }
        }
    })
    .instrument(initialize_span)
    .await;
    result.unwrap_or_else(|_| {
        tracing::error!(
            target: crate::logging::targets::BRIDGE_LIFECYCLE,
            event_name = "bridge_initialize_timed_out",
            message = "bridge initialization timed out",
            outcome = "timeout",
            timeout_ms,
        );
        Err(InitializationFailure::Transport(AppError::BridgeTimeout))
    })
}

#[cfg(test)]
mod tests {
    use super::{StartConnectionParams, bridge_event_loop};
    use crate::agent::bridge::BridgeLauncher;
    use crate::agent::client::BridgeClient;
    use crate::agent::events::ClientEvent;
    use crate::agent::wire::{BridgeEvent, SessionLaunchSettings};
    use crate::error::AppError;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::Duration;
    use tempfile::TempDir;

    #[tokio::test]
    async fn startup_failure_stops_event_loop_without_reporting_stdout_closure() {
        let json = r#"{"event":"connection_failed","request_id":"connect-1","message":"Claude Code startup failed.","startup_failure":{"reason":"future_reason","errors":["SDK guidance"]}}"#;
        let script = if cfg!(windows) {
            format!("@echo off\r\necho {json}\r\nexit /b 0\r\n")
        } else {
            format!("#!/bin/sh\nprintf '%s\\n' '{json}'\nexit 0\n")
        };
        let fixture = RuntimeFixture::new(script).expect("runtime fixture");
        let mut bridge = BridgeClient::spawn(&fixture.launcher()).expect("spawn bridge");
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<ClientEvent>(8);
        let params = StartConnectionParams {
            event_tx,
            cwd_raw: fixture.script_path.parent().expect("fixture directory").display().to_string(),
            bridge_script: None,
            launch: crate::StartupLaunch::NewSession,
            session_launch_settings: SessionLaunchSettings::default(),
        };
        let connection = bridge.connection();
        let mut connected_once = false;
        tokio::time::timeout(
            Duration::from_secs(2),
            bridge_event_loop(&params, &mut bridge, &connection, &mut connected_once),
        )
        .await
        .expect("event loop completes");
        let ClientEvent::ConnectionFailed(failure) =
            event_rx.try_recv().expect("connection failure")
        else {
            panic!("connection failure")
        };
        assert_eq!(failure.request_id.as_deref(), Some("connect-1"));
        let startup = failure.startup_failure.expect("structured startup failure");
        assert_eq!(startup.reason, "future_reason");
        assert_eq!(startup.errors, ["SDK guidance"]);
        assert!(matches!(
            event_rx.try_recv(),
            Ok(ClientEvent::FatalError(AppError::BridgeSdkFailure))
        ));
        assert!(event_rx.try_recv().is_err(), "no second connection failure");
        bridge.wait().await.expect("reap fixture");
    }

    #[tokio::test]
    async fn bridge_client_recv_returns_none_when_stdout_closes() {
        let fixture = RuntimeFixture::new(exit_success_script()).expect("runtime fixture");
        let mut bridge = BridgeClient::spawn(&fixture.launcher()).expect("spawn bridge");

        let event = bridge.recv().await.expect("recv should not fail");

        assert_eq!(event, None);
        let status = bridge.wait().await.expect("wait for bridge");
        assert!(status.success());
    }

    #[tokio::test]
    async fn bridge_client_recv_reports_malformed_event_json() {
        let fixture = RuntimeFixture::new(malformed_stdout_script()).expect("runtime fixture");
        let mut bridge = BridgeClient::spawn(&fixture.launcher()).expect("spawn bridge");

        let err = bridge.recv().await.expect_err("malformed event should fail");

        assert!(
            err.to_string().contains("failed to decode bridge event json"),
            "unexpected error: {err:#}"
        );
        let _ = bridge.wait().await;
    }

    #[tokio::test]
    async fn bridge_client_reads_protocol_after_child_stderr_output() {
        let fixture =
            RuntimeFixture::new(stderr_then_initialized_script()).expect("runtime fixture");
        let mut bridge = BridgeClient::spawn(&fixture.launcher()).expect("spawn bridge");

        let event = bridge.recv().await.expect("recv should parse initialized event");

        assert!(matches!(
            event.map(|envelope| envelope.event),
            Some(BridgeEvent::Initialized { .. })
        ));
        let status = bridge.wait().await.expect("wait for bridge");
        assert!(status.success());
    }

    struct RuntimeFixture {
        _dir: TempDir,
        runtime_path: PathBuf,
        script_path: PathBuf,
    }

    impl RuntimeFixture {
        fn new(runtime_contents: impl AsRef<str>) -> std::io::Result<Self> {
            let dir = tempfile::tempdir()?;
            let (runtime_path, script_path) = stage_runtime(dir.path(), runtime_contents.as_ref())?;
            Ok(Self { _dir: dir, runtime_path, script_path })
        }

        fn launcher(&self) -> BridgeLauncher {
            BridgeLauncher {
                runtime_path: self.runtime_path.clone(),
                script_path: self.script_path.clone(),
            }
        }
    }

    fn initialized_event_json() -> &'static str {
        r#"{"event":"initialized","result":{"agent_name":"test","agent_version":"0","auth_methods":[],"capabilities":{"prompt_image":false,"prompt_embedded_context":false,"supports_session_listing":false,"supports_resume_session":false}}}"#
    }

    #[cfg(windows)]
    fn runtime_name() -> &'static str {
        "fake_bridge.cmd"
    }

    #[cfg(not(windows))]
    fn runtime_name() -> &'static str {
        "fake_bridge.sh"
    }

    #[cfg(windows)]
    fn exit_success_script() -> &'static str {
        "@echo off\r\nexit /b 0\r\n"
    }

    #[cfg(not(windows))]
    fn exit_success_script() -> &'static str {
        "#!/bin/sh\nexit 0\n"
    }

    #[cfg(windows)]
    fn malformed_stdout_script() -> String {
        "@echo off\r\necho not-json\r\n".to_owned()
    }

    #[cfg(not(windows))]
    fn malformed_stdout_script() -> String {
        "#!/bin/sh\nprintf 'not-json\\n'\n".to_owned()
    }

    #[cfg(windows)]
    fn stderr_then_initialized_script() -> String {
        format!("@echo off\r\necho bridge diagnostic 1>&2\r\necho {}\r\n", initialized_event_json())
    }

    #[cfg(not(windows))]
    fn stderr_then_initialized_script() -> String {
        format!(
            "#!/bin/sh\nprintf 'bridge diagnostic\\n' >&2\nprintf '%s\\n' '{}'\n",
            initialized_event_json()
        )
    }

    /// Stage the fake bridge and return the `(runtime, script)` pair to launch it with.
    ///
    /// On Unix the launcher runs the script through `/bin/sh` rather than marking it
    /// executable and running it directly. The suite executes roughly eighteen hundred tests
    /// across parallel threads and several of them spawn child processes. A `fork` in one
    /// thread transiently inherits the writable descriptor another thread still holds on its
    /// freshly written script, and an `execve` of that path then fails with `ETXTBSY`. Keeping
    /// the exec target on an interpreter that nothing in the suite writes removes that race by
    /// construction instead of narrowing the window.
    #[cfg(not(windows))]
    fn stage_runtime(dir: &Path, runtime_contents: &str) -> std::io::Result<(PathBuf, PathBuf)> {
        let script_path = dir.join(runtime_name());
        fs::write(&script_path, runtime_contents)?;
        Ok((PathBuf::from("/bin/sh"), script_path))
    }

    /// Windows has no `ETXTBSY`, so the batch file stays the runtime and keeps the launcher
    /// shape identical to production, where a real runtime is handed a separate script path.
    #[cfg(windows)]
    fn stage_runtime(dir: &Path, runtime_contents: &str) -> std::io::Result<(PathBuf, PathBuf)> {
        let runtime_path = dir.join(runtime_name());
        let script_path = dir.join("bridge.js");
        fs::write(&runtime_path, runtime_contents)?;
        fs::write(&script_path, "// fake bridge script\n")?;
        Ok((runtime_path, script_path))
    }
}

/// The session command the app sends the bridge for a command line, as the bridge reads it.
#[cfg(test)]
mod launch_command_tests {
    use super::{StartConnectionParams, build_session_command};
    use crate::app::App;
    use crate::app::connect::session_start::{
        SessionStartReason, session_launch_settings_for_reason,
    };
    use clap::Parser;
    use serde_json::json;

    fn startup_command(args: &[&str]) -> serde_json::Value {
        let cli = crate::Cli::try_parse_from(args).expect("parse CLI");
        cli.validate().expect("valid launch");
        let mut app = App::test_default();
        app.startup = crate::app::state::StartupState::from_cli(&cli)
            .with_session_config(cli.resolve_session_config().expect("session config"));
        let (event_tx, _event_rx) = tokio::sync::mpsc::channel(1);
        let params = StartConnectionParams {
            event_tx,
            cwd_raw: "/work/project".to_owned(),
            bridge_script: None,
            launch: cli.startup_launch(),
            session_launch_settings: session_launch_settings_for_reason(
                &app,
                SessionStartReason::Startup,
            ),
        };
        serde_json::to_value(build_session_command(&params)).expect("serialize command")
    }

    #[test]
    fn name_reaches_new_continued_and_resumed_sessions() {
        let created = startup_command(&["claude-rs", "-n", "Release prep"]);
        assert_eq!(created["command"], "create_session");
        assert_eq!(created["launch_settings"], json!({ "name": "Release prep" }));

        let continued = startup_command(&["claude-rs", "--continue", "--name", "Follow-up"]);
        assert_eq!(continued["continue_session"], true);
        assert_eq!(continued["launch_settings"], json!({ "name": "Follow-up" }));

        let resumed = startup_command(&["claude-rs", "resume", "session-1", "-n", "Renamed"]);
        assert_eq!(resumed["command"], "resume_session");
        assert_eq!(resumed["session_id"], "session-1");
        assert_eq!(resumed["launch_settings"], json!({ "name": "Renamed" }));
    }

    #[test]
    fn fork_session_reaches_resumed_and_continued_sessions() {
        let resumed = startup_command(&["claude-rs", "-r", "session-1", "--fork-session"]);
        assert_eq!(resumed["command"], "resume_session");
        assert_eq!(resumed["launch_settings"], json!({ "fork_session": true }));

        let continued = startup_command(&["claude-rs", "-c", "--fork-session", "-n", "Branch"]);
        assert_eq!(continued["continue_session"], true);
        assert_eq!(continued["launch_settings"], json!({ "fork_session": true, "name": "Branch" }));
    }

    #[test]
    fn settings_and_mcp_configs_reach_the_bridge_as_parsed_objects() {
        let project = tempfile::tempdir().expect("project");
        std::fs::write(
            project.path().join("flag-settings.json"),
            "{ \"spinnerTipsEnabled\": false }\n",
        )
        .expect("settings");
        std::fs::write(
            project.path().join("mcp.json"),
            r#"{"mcpServers":{"docs":{"command":"docs-mcp"}}}"#,
        )
        .expect("MCP config");
        let dir = project.path().to_str().expect("UTF-8 path");
        let inline = r#"{"mcpServers":{"docs":{"command":"override"}}}"#;

        let created = startup_command(&[
            "claude-rs",
            "-C",
            dir,
            "--settings",
            "flag-settings.json",
            "--mcp-config",
            "mcp.json",
            "--mcp-config",
            inline,
            "--strict-mcp-config",
        ]);
        assert_eq!(
            created["launch_settings"],
            json!({
                "settings": { "spinnerTipsEnabled": false },
                "mcp_configs": [
                    { "docs": { "command": "docs-mcp" } },
                    { "docs": { "command": "override" } },
                ],
                "strict_mcp_config": true,
            })
        );
        let resumed = startup_command(&[
            "claude-rs",
            "-r",
            "session-1",
            "--settings",
            r#"{"model":"haiku"}"#,
        ]);
        assert_eq!(resumed["command"], "resume_session");
        assert_eq!(resumed["launch_settings"], json!({ "settings": { "model": "haiku" } }));
    }

    #[test]
    fn system_prompt_text_and_files_reach_the_bridge() {
        let project = tempfile::tempdir().expect("project");
        std::fs::write(project.path().join("prompt.md"), "You are a release bot.\n")
            .expect("prompt");
        std::fs::write(project.path().join("append.md"), "End every reply with DONE.")
            .expect("append");
        let dir = project.path().to_str().expect("UTF-8 path");
        let from_files = startup_command(&[
            "claude-rs",
            "-C",
            dir,
            "--system-prompt-file",
            "prompt.md",
            "--append-system-prompt-file",
            "append.md",
        ]);
        assert_eq!(
            from_files["launch_settings"],
            json!({
                "system_prompt": "You are a release bot.\n",
                "append_system_prompt": "End every reply with DONE."
            })
        );
        let inline = startup_command(&["claude-rs", "--append-system-prompt", "Answer in German."]);
        assert_eq!(
            inline["launch_settings"],
            json!({ "append_system_prompt": "Answer in German." })
        );
    }

    #[test]
    fn added_directories_reach_the_bridge_as_absolute_paths() {
        let project = tempfile::tempdir().expect("project");
        let shared = tempfile::tempdir().expect("shared directory");
        std::fs::create_dir(project.path().join("docs")).expect("docs directory");
        let dir = project.path().to_str().expect("UTF-8 path");
        let shared_dir = shared.path().to_str().expect("UTF-8 path");
        let created = startup_command(&[
            "claude-rs",
            "-C",
            dir,
            "--add-dir",
            "docs",
            "--add-dir",
            shared_dir,
        ]);
        assert_eq!(
            created["launch_settings"],
            json!({
                "additional_directories": [
                    project.path().join("docs").to_str().expect("UTF-8 path"),
                    shared_dir,
                ]
            })
        );
    }
}
