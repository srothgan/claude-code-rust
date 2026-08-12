// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

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

pub(super) async fn run_connection_task(
    params: StartConnectionParams,
    conn_slot_writer: Rc<std::cell::RefCell<Option<ConnectionSlot>>>,
    mut shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) {
    let request_kind = if params.resume_id.is_some() { "resume" } else { "create" };
    let session_id = params.resume_id.clone().unwrap_or_default();
    let connection_span = info_span!(
        target: crate::logging::targets::BRIDGE_LIFECYCLE,
        "bridge_connection",
        request_kind,
        resume_requested = params.resume_requested,
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
            resume_requested = params.resume_requested,
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
    if let Err(app_error) = wait_for_bridge_initialized(
        bridge,
        &params.event_tx,
        &connection,
        &mut connected_once,
        params.resume_requested,
    )
    .await
    {
        emit_connection_failed(
            &params.event_tx,
            "Bridge did not complete initialization".to_owned(),
            app_error,
        )
        .await;
        return;
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
    if let Some(resume) = &params.resume_id {
        CommandEnvelope {
            request_id: None,
            command: BridgeCommand::ResumeSession {
                session_id: resume.clone(),
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
                launch_settings: params.session_launch_settings.clone(),
                metadata: std::collections::BTreeMap::new(),
            },
        }
    }
}

fn log_session_connect_command_sent(params: &StartConnectionParams, command: &BridgeCommand) {
    let has_language = params.session_launch_settings.language.is_some();
    let has_settings = params.session_launch_settings.settings.is_some();
    let agent_progress_summaries_enabled =
        params.session_launch_settings.agent_progress_summaries.unwrap_or(false);
    match command {
        BridgeCommand::ResumeSession { session_id, .. } => tracing::info!(
            target: crate::logging::targets::APP_SESSION,
            event_name = "session_connect_command_sent",
            message = "session connect command sent to bridge",
            outcome = "success",
            request_kind = "resume",
            resume_requested = true,
            session_id = %session_id,
            has_language,
            has_settings,
            agent_progress_summaries_enabled,
        ),
        BridgeCommand::CreateSession { .. } => tracing::info!(
            target: crate::logging::targets::APP_SESSION,
            event_name = "session_connect_command_sent",
            message = "session connect command sent to bridge",
            outcome = "success",
            request_kind = "create",
            resume_requested = false,
            cwd = %params.cwd_raw,
            has_language,
            has_settings,
            agent_progress_summaries_enabled,
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
                handle_bridge_event(
                    &params.event_tx,
                    connection,
                    connected_once,
                    params.resume_requested,
                    envelope,
                )
                .await;
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
    message: String,
    app_error: AppError,
) {
    tracing::error!(
        target: crate::logging::targets::BRIDGE_LIFECYCLE,
        event_name = "bridge_failure_reported",
        message = "bridge failure reported to app",
        outcome = "failure",
        error_category = app_error.category_tag(),
        exit_code = app_error.exit_code(),
        user_message = %message,
    );
    let _ = event_tx.send(ClientEvent::ConnectionFailed(message)).await;
    let _ = event_tx.send(ClientEvent::FatalError(app_error)).await;
}

pub(super) async fn wait_for_bridge_initialized(
    bridge: &mut BridgeClient,
    event_tx: &mpsc::Sender<ClientEvent>,
    connection: &AgentConnection,
    connected_once: &mut bool,
    resume_requested: bool,
) -> Result<(), AppError> {
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
) -> Result<(), AppError> {
    let timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
    let initialize_span = info_span!(
        target: crate::logging::targets::BRIDGE_LIFECYCLE,
        "bridge_initialize",
        resume_requested,
        timeout_ms,
    );

    async {
        let started = tokio::time::Instant::now();
        loop {
            let elapsed = tokio::time::Instant::now().saturating_duration_since(started);
            let remaining = timeout.saturating_sub(elapsed);
            if remaining.is_zero() {
                tracing::error!(
                    target: crate::logging::targets::BRIDGE_LIFECYCLE,
                    event_name = "bridge_initialize_timed_out",
                    message = "bridge initialization timed out",
                    outcome = "timeout",
                    timeout_ms,
                );
                return Err(AppError::BridgeTimeout);
            }

            let event = tokio::time::timeout(remaining, bridge.recv()).await;
            match event {
                Ok(Ok(Some(envelope))) => {
                    if matches!(envelope.event, BridgeEvent::Initialized { .. }) {
                        return Ok(());
                    }
                    if matches!(envelope.event, BridgeEvent::ConnectionFailed { .. }) {
                        handle_bridge_event(
                            event_tx,
                            connection,
                            connected_once,
                            resume_requested,
                            envelope,
                        )
                        .await;
                        return Err(AppError::BridgeInitializationFailed);
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
                Ok(Ok(None)) => return Err(AppError::BridgeStdoutClosed),
                Ok(Err(_)) => return Err(AppError::BridgeSdkFailure),
                Err(_) => return Err(AppError::BridgeTimeout),
            }
        }
    }
    .instrument(initialize_span)
    .await
}

#[cfg(test)]
mod tests {
    use super::{
        ConnectionSlot, StartConnectionParams, handle_bridge_event, run_connection_task,
        wait_for_bridge_initialized_with_timeout,
    };
    use crate::agent::bridge::BridgeLauncher;
    use crate::agent::client::{BridgeClient, BridgeShutdownOutcome};
    use crate::agent::events::ClientEvent;
    use crate::agent::wire::{BridgeEvent, EventEnvelope, SessionLaunchSettings};
    use crate::error::AppError;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;
    use std::time::Duration;
    use tempfile::TempDir;

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
    async fn bridge_shutdown_exits_and_reaps_without_force_kill() {
        let fixture = RuntimeFixture::new(exit_on_shutdown_script()).expect("runtime fixture");
        let bridge = BridgeClient::spawn(&fixture.launcher()).expect("spawn bridge");

        let outcome =
            bridge.shutdown_and_wait(Duration::from_secs(1)).await.expect("graceful shutdown");

        let BridgeShutdownOutcome::Graceful(status) = outcome else {
            panic!("bridge should exit without force-kill");
        };
        assert!(status.success());
    }

    #[tokio::test]
    async fn bridge_shutdown_force_kills_and_reaps_unresponsive_process() {
        let fixture = RuntimeFixture::new(ignore_shutdown_script()).expect("runtime fixture");
        let bridge = BridgeClient::spawn(&fixture.launcher()).expect("spawn bridge");

        let outcome =
            bridge.shutdown_and_wait(Duration::from_millis(50)).await.expect("forced shutdown");

        assert!(matches!(outcome, BridgeShutdownOutcome::Forced(_)));
    }

    #[tokio::test]
    async fn bridge_shutdown_deadline_covers_a_blocked_stdin_writer() {
        let fixture = RuntimeFixture::new(delayed_no_output_script()).expect("runtime fixture");
        let bridge = BridgeClient::spawn(&fixture.launcher()).expect("spawn bridge");
        let connection = bridge.connection();
        connection
            .prompt_text("session-1".to_owned(), "x".repeat(8 * 1024 * 1024))
            .expect("queue oversized prompt");
        let started = tokio::time::Instant::now();

        let outcome =
            bridge.shutdown_and_wait(Duration::from_millis(50)).await.expect("forced shutdown");

        assert!(matches!(outcome, BridgeShutdownOutcome::Forced(_)));
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "shutdown must not wait for the blocked stdin write"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn shutdown_interrupts_error_reporting_to_a_full_event_queue() {
        tokio::task::LocalSet::new()
            .run_until(async {
                let fixture_dir = tempfile::tempdir().expect("fixture directory");
                let missing_bridge = fixture_dir.path().join("missing-bridge.js");
                let (event_tx, _event_rx) = tokio::sync::mpsc::channel(1);
                event_tx.send(ClientEvent::LogoutCompleted).await.expect("prefill event queue");
                let params = StartConnectionParams {
                    event_tx,
                    cwd_raw: fixture_dir.path().display().to_string(),
                    bridge_script: Some(missing_bridge),
                    resume_id: None,
                    resume_requested: false,
                    session_launch_settings: SessionLaunchSettings::default(),
                };
                let connection_slot: Rc<std::cell::RefCell<Option<ConnectionSlot>>> =
                    Rc::new(std::cell::RefCell::new(None));
                let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
                let task = tokio::task::spawn_local(run_connection_task(
                    params,
                    Rc::clone(&connection_slot),
                    shutdown_rx,
                ));

                tokio::time::sleep(Duration::from_millis(20)).await;
                shutdown_tx.send(()).expect("signal shutdown");
                tokio::time::timeout(Duration::from_secs(1), task)
                    .await
                    .expect(
                        "connection task must observe shutdown while event reporting is blocked",
                    )
                    .expect("connection task should not panic");

                assert!(connection_slot.borrow().is_none());
            })
            .await;
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

    #[tokio::test]
    async fn bridge_initialization_fails_when_process_exits_before_protocol_ready() {
        let fixture = RuntimeFixture::new(exit_failure_script()).expect("runtime fixture");
        let mut bridge = BridgeClient::spawn(&fixture.launcher()).expect("spawn bridge");
        let (event_tx, _event_rx) = tokio::sync::mpsc::channel::<ClientEvent>(1);
        let connection = bridge.connection();
        let mut connected_once = false;

        let err = wait_for_bridge_initialized_with_timeout(
            &mut bridge,
            &event_tx,
            &connection,
            &mut connected_once,
            false,
            Duration::from_secs(1),
        )
        .await
        .expect_err("closed stdout should fail initialization");

        assert_eq!(err, AppError::BridgeStdoutClosed);
        let status = bridge.wait().await.expect("wait for bridge");
        assert!(!status.success());
    }

    #[tokio::test]
    async fn bridge_initialization_timeout_is_deterministic() {
        let fixture = RuntimeFixture::new(delayed_no_output_script()).expect("runtime fixture");
        let mut bridge = BridgeClient::spawn(&fixture.launcher()).expect("spawn bridge");
        let (event_tx, _event_rx) = tokio::sync::mpsc::channel::<ClientEvent>(1);
        let connection = bridge.connection();
        let mut connected_once = false;

        let err = wait_for_bridge_initialized_with_timeout(
            &mut bridge,
            &event_tx,
            &connection,
            &mut connected_once,
            false,
            Duration::from_millis(50),
        )
        .await
        .expect_err("missing initialized event should time out");

        assert_eq!(err, AppError::BridgeTimeout);
        let _ = bridge.wait().await;
    }

    #[tokio::test]
    async fn bridge_initialization_succeeds_after_initialized_event() {
        let fixture = RuntimeFixture::new(initialized_stdout_script()).expect("runtime fixture");
        let mut bridge = BridgeClient::spawn(&fixture.launcher()).expect("spawn bridge");
        let (event_tx, _event_rx) = tokio::sync::mpsc::channel::<ClientEvent>(1);
        let connection = bridge.connection();
        let mut connected_once = false;

        wait_for_bridge_initialized_with_timeout(
            &mut bridge,
            &event_tx,
            &connection,
            &mut connected_once,
            false,
            Duration::from_secs(1),
        )
        .await
        .expect("initialized event should complete handshake");

        let status = bridge.wait().await.expect("wait for bridge");
        assert!(status.success());
    }

    #[tokio::test]
    async fn control_commands_are_forwarded_while_event_dispatch_waits_for_capacity() {
        let marker_dir = tempfile::tempdir().expect("marker directory");
        let marker_path = marker_dir.path().join("command.json");
        let fixture =
            RuntimeFixture::new(capture_one_command_script(&marker_path)).expect("runtime fixture");
        let bridge = BridgeClient::spawn(&fixture.launcher()).expect("spawn bridge");
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<ClientEvent>(1);
        event_tx.send(ClientEvent::LogoutCompleted).await.expect("prefill event queue");
        let connection = bridge.connection();
        connection.cancel("session-1".to_owned()).expect("queue cancel command");
        let mut connected_once = false;
        let envelope = EventEnvelope {
            request_id: None,
            event: BridgeEvent::AuthRequired {
                method_name: "test".to_owned(),
                method_description: "test".to_owned(),
            },
        };

        {
            let dispatch =
                handle_bridge_event(&event_tx, &connection, &mut connected_once, false, envelope);
            tokio::pin!(dispatch);
            let marker = wait_for_nonempty_file(&marker_path);
            tokio::pin!(marker);

            tokio::select! {
                result = &mut dispatch => panic!("event dispatch should remain backpressured: {result:?}"),
                contents = &mut marker => {
                    let contents = contents.expect("child should capture command");
                    assert!(contents.contains(r#""command":"cancel_turn""#), "{contents}");
                    assert!(contents.contains(r#""session_id":"session-1""#), "{contents}");
                }
            }

            assert!(matches!(event_rx.recv().await, Some(ClientEvent::LogoutCompleted)));
            dispatch.await;
            assert!(matches!(event_rx.recv().await, Some(ClientEvent::AuthRequired { .. })));
        }
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
            let runtime_path = dir.path().join(runtime_name());
            let script_path = dir.path().join("bridge.js");
            fs::write(&runtime_path, runtime_contents.as_ref())?;
            fs::write(&script_path, "// fake bridge script\n")?;
            make_executable(&runtime_path)?;
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

    async fn wait_for_nonempty_file(path: &Path) -> std::io::Result<String> {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match fs::read_to_string(path) {
                    Ok(contents) if !contents.is_empty() => return Ok(contents),
                    Ok(_) => {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    Err(err) => return Err(err),
                }
            }
        })
        .await
        .map_err(std::io::Error::other)?
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
    fn exit_failure_script() -> &'static str {
        "@echo off\r\nexit /b 7\r\n"
    }

    #[cfg(not(windows))]
    fn exit_failure_script() -> &'static str {
        "#!/bin/sh\nexit 7\n"
    }

    #[cfg(windows)]
    fn exit_on_shutdown_script() -> &'static str {
        "@echo off\r\nset /p line=\r\nexit /b 0\r\n"
    }

    #[cfg(not(windows))]
    fn exit_on_shutdown_script() -> &'static str {
        "#!/bin/sh\nIFS= read -r line\nexit 0\n"
    }

    #[cfg(windows)]
    fn capture_one_command_script(marker_path: &Path) -> String {
        format!(
            "@echo off\r\nset /p line=\r\n> \"{}\" echo %line%\r\nexit /b 0\r\n",
            marker_path.display()
        )
    }

    #[cfg(not(windows))]
    fn capture_one_command_script(marker_path: &Path) -> String {
        let marker_path = marker_path.display().to_string().replace('\'', "'\\''");
        format!(
            "#!/bin/sh\nIFS= read -r line\nprintf '%s\\n' \"$line\" > '{marker_path}'\nexit 0\n"
        )
    }

    #[cfg(windows)]
    fn ignore_shutdown_script() -> &'static str {
        "@echo off\r\n:read\r\nset \"line=\"\r\nset /p line=\r\ngoto read\r\n"
    }

    #[cfg(not(windows))]
    fn ignore_shutdown_script() -> &'static str {
        "#!/bin/sh\nwhile IFS= read -r line; do :; done\n"
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
    fn initialized_stdout_script() -> String {
        format!("@echo off\r\necho {}\r\n", initialized_event_json())
    }

    #[cfg(not(windows))]
    fn initialized_stdout_script() -> String {
        format!("#!/bin/sh\nprintf '%s\\n' '{}'\n", initialized_event_json())
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

    #[cfg(windows)]
    fn delayed_no_output_script() -> &'static str {
        "@echo off\r\npowershell -NoProfile -Command \"Start-Sleep -Milliseconds 500\"\r\n"
    }

    #[cfg(not(windows))]
    fn delayed_no_output_script() -> &'static str {
        "#!/bin/sh\nsleep 0.5\n"
    }

    #[cfg(unix)]
    fn make_executable(path: &Path) -> std::io::Result<()> {
        use std::os::unix::fs::PermissionsExt as _;

        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions)
    }

    #[cfg(not(unix))]
    #[allow(clippy::unnecessary_wraps)]
    fn make_executable(_path: &Path) -> std::io::Result<()> {
        Ok(())
    }
}
