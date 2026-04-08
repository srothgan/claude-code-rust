// Copyright 2025 Simon Peter Rothgang
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

use super::event_dispatch::handle_bridge_event;
use super::{ConnectionSlot, StartConnectionParams, extract_app_error};

pub(super) async fn run_connection_task(
    params: StartConnectionParams,
    conn_slot_writer: Rc<std::cell::RefCell<Option<ConnectionSlot>>>,
) {
    tracing::debug!(
        target: crate::logging::targets::BRIDGE_LIFECYCLE,
        event_name = "bridge_connection_task_started",
        message = "bridge connection task started",
        outcome = "start",
        resume_requested = params.resume_requested,
    );

    let Some(launcher) = resolve_launcher(&params) else {
        return;
    };
    let Some(mut bridge) = spawn_bridge_client(&params.event_tx, &launcher) else {
        return;
    };

    let mut connected_once = false;
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<CommandEnvelope>();
    publish_connection_slot(&conn_slot_writer, &cmd_tx);

    if !send_initialize_command(&params, &mut bridge).await {
        return;
    }
    if let Err(app_error) = wait_for_bridge_initialized(
        &mut bridge,
        &params.event_tx,
        &cmd_tx,
        &mut connected_once,
        params.resume_requested,
    )
    .await
    {
        emit_connection_failed(
            &params.event_tx,
            "Bridge did not complete initialization".to_owned(),
            app_error,
        );
        return;
    }
    if !send_session_command(&params, &mut bridge).await {
        return;
    }

    bridge_event_loop(&params, &mut bridge, &cmd_tx, &mut cmd_rx, &mut connected_once).await;
}

fn resolve_launcher(params: &StartConnectionParams) -> Option<BridgeLauncher> {
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
            let app_error = extract_app_error(&err).unwrap_or(AppError::ConnectionFailed);
            emit_connection_failed(
                &params.event_tx,
                format!("Failed to resolve bridge launcher: {err}"),
                app_error,
            );
            None
        }
    }
}

fn spawn_bridge_client(
    event_tx: &mpsc::UnboundedSender<ClientEvent>,
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
            let app_error = extract_app_error(&err).unwrap_or(AppError::AdapterCrashed);
            emit_connection_failed(event_tx, format!("Failed to spawn bridge: {err}"), app_error);
            None
        }
    }
}

fn publish_connection_slot(
    conn_slot_writer: &Rc<std::cell::RefCell<Option<ConnectionSlot>>>,
    cmd_tx: &mpsc::UnboundedSender<CommandEnvelope>,
) {
    *conn_slot_writer.borrow_mut() =
        Some(ConnectionSlot { conn: Rc::new(AgentConnection::new(cmd_tx.clone())) });
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
            format!("Failed to initialize bridge: {err}"),
            AppError::ConnectionFailed,
        );
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

async fn send_session_command(params: &StartConnectionParams, bridge: &mut BridgeClient) -> bool {
    let command = build_session_command(params);
    if let Err(err) = bridge.send(command).await {
        emit_connection_failed(
            &params.event_tx,
            format!("Failed to create bridge session: {err}"),
            AppError::ConnectionFailed,
        );
        return false;
    }
    true
}

async fn bridge_event_loop(
    params: &StartConnectionParams,
    bridge: &mut BridgeClient,
    cmd_tx: &mpsc::UnboundedSender<CommandEnvelope>,
    cmd_rx: &mut mpsc::UnboundedReceiver<CommandEnvelope>,
    connected_once: &mut bool,
) {
    loop {
        tokio::select! {
            Some(cmd) = cmd_rx.recv() => {
                if let Err(err) = bridge.send(cmd).await {
                    emit_connection_failed(
                        &params.event_tx,
                        format!("Failed to send bridge command: {err}"),
                        AppError::ConnectionFailed,
                    );
                    break;
                }
            }
            event = bridge.recv() => {
                match event {
                    Ok(Some(envelope)) => {
                        handle_bridge_event(
                            &params.event_tx,
                            cmd_tx,
                            connected_once,
                            params.resume_requested,
                            envelope,
                        );
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
                            "Bridge process exited unexpectedly".to_owned(),
                            AppError::ConnectionFailed,
                        );
                        break;
                    }
                    Err(err) => {
                        emit_connection_failed(
                            &params.event_tx,
                            format!("Bridge communication failure: {err}"),
                            AppError::ConnectionFailed,
                        );
                        break;
                    }
                }
            }
        }
    }
}

pub(super) fn emit_connection_failed(
    event_tx: &mpsc::UnboundedSender<ClientEvent>,
    message: String,
    app_error: AppError,
) {
    let _ = event_tx.send(ClientEvent::ConnectionFailed(message));
    let _ = event_tx.send(ClientEvent::FatalError(app_error));
}

pub(super) async fn wait_for_bridge_initialized(
    bridge: &mut BridgeClient,
    event_tx: &mpsc::UnboundedSender<ClientEvent>,
    cmd_tx: &mpsc::UnboundedSender<CommandEnvelope>,
    connected_once: &mut bool,
    resume_requested: bool,
) -> Result<(), AppError> {
    let timeout = Duration::from_secs(10);
    let started = tokio::time::Instant::now();
    loop {
        let elapsed = tokio::time::Instant::now().saturating_duration_since(started);
        let remaining = timeout.saturating_sub(elapsed);
        if remaining.is_zero() {
            let timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
            tracing::error!(
                target: crate::logging::targets::BRIDGE_LIFECYCLE,
                event_name = "bridge_initialize_timed_out",
                message = "bridge initialization timed out",
                outcome = "timeout",
                timeout_ms,
            );
            return Err(AppError::ConnectionFailed);
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
                        cmd_tx,
                        connected_once,
                        resume_requested,
                        envelope,
                    );
                    return Err(AppError::ConnectionFailed);
                }
                handle_bridge_event(event_tx, cmd_tx, connected_once, resume_requested, envelope);
            }
            Ok(Ok(None) | Err(_)) | Err(_) => return Err(AppError::ConnectionFailed),
        }
    }
}
