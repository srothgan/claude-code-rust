// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

//! Bridge event dispatch: routes incoming `BridgeEvent` envelopes to appropriate
//! `ClientEvent` messages, and handles permission request/response forwarding.

use crate::agent::client::AgentConnection;
use crate::agent::error_handling::parse_turn_error_class;
use crate::agent::events::ClientEvent;
use crate::agent::model;
use crate::agent::types;
use crate::agent::wire::EventEnvelope;
use crate::error::AppError;
use tokio::sync::mpsc;

use super::bridge_lifecycle::emit_connection_failed;
use super::type_converters::{
    convert_account_info, convert_current_model, convert_fast_mode_state, convert_mode_state,
    map_available_models, map_mcp_auth_capabilities, map_mcp_server_status, map_permission_request,
    map_question_request, map_rewind_result, map_rewind_targets, map_session_update,
    map_user_dialog_request,
};

struct ConnectedEventData {
    session_id: String,
    cwd: String,
    current_model: types::CurrentModel,
    available_models: Vec<types::AvailableModel>,
    mode: Option<types::ModeState>,
    fast_mode_state: types::FastModeState,
    fast_mode_disabled_reason: Option<String>,
    history_updates: Option<Vec<types::SessionUpdate>>,
}

#[allow(clippy::too_many_lines)]
pub(super) async fn handle_bridge_event(
    event_tx: &mpsc::Sender<ClientEvent>,
    connection: &AgentConnection,
    connected_once: &mut bool,
    resume_requested: bool,
    envelope: EventEnvelope,
) {
    let EventEnvelope { request_id, event } = envelope;
    match event {
        crate::agent::wire::BridgeEvent::Connected {
            session_id,
            cwd,
            current_model,
            available_models,
            mode,
            fast_mode_state,
            fast_mode_disabled_reason,
            history_updates,
        } => {
            handle_connected_event(
                event_tx,
                connected_once,
                ConnectedEventData {
                    session_id,
                    cwd,
                    current_model,
                    available_models,
                    mode,
                    fast_mode_state,
                    fast_mode_disabled_reason,
                    history_updates,
                },
            )
            .await;
        }
        crate::agent::wire::BridgeEvent::AuthRequired { method_name, method_description } => {
            let _ =
                event_tx.send(ClientEvent::AuthRequired { method_name, method_description }).await;
        }
        crate::agent::wire::BridgeEvent::ConnectionFailed { message } => {
            emit_connection_failed(event_tx, message, AppError::BridgeSdkFailure).await;
        }
        crate::agent::wire::BridgeEvent::SessionUpdate { session_id, update } => {
            if let Some(update) = map_session_update(update) {
                let _ = event_tx.send(ClientEvent::SessionUpdate { session_id, update }).await;
            }
        }
        crate::agent::wire::BridgeEvent::PermissionRequest { session_id, request } => {
            handle_permission_request_event(event_tx, connection, session_id, request).await;
        }
        crate::agent::wire::BridgeEvent::QuestionRequest { session_id, request } => {
            handle_question_request_event(event_tx, connection, session_id, request).await;
        }
        crate::agent::wire::BridgeEvent::UserDialogRequest { session_id, request } => {
            handle_user_dialog_request_event(event_tx, connection, session_id, request).await;
        }
        crate::agent::wire::BridgeEvent::ElicitationRequest { session_id, request } => {
            handle_elicitation_request_event(event_tx, &session_id, request).await;
        }
        crate::agent::wire::BridgeEvent::ElicitationComplete {
            session_id,
            elicitation_id,
            server_name,
        } => {
            let _ = event_tx
                .send(ClientEvent::McpElicitationCompleted {
                    session_id,
                    elicitation_id,
                    server_name,
                })
                .await;
        }
        crate::agent::wire::BridgeEvent::McpAuthRedirect { session_id, redirect } => {
            let _ = event_tx.send(ClientEvent::McpAuthRedirect { session_id, redirect }).await;
        }
        crate::agent::wire::BridgeEvent::McpOperationError { session_id, error } => {
            let _ = event_tx.send(ClientEvent::McpOperationError { session_id, error }).await;
        }
        crate::agent::wire::BridgeEvent::McpSetServersResult { session_id, result } => {
            let _ = event_tx.send(ClientEvent::McpSetServersResult { session_id, result }).await;
        }
        crate::agent::wire::BridgeEvent::UserMessageQueued { session_id, message_uuid } => {
            let _ =
                event_tx.send(ClientEvent::UserMessageQueued { session_id, message_uuid }).await;
        }
        crate::agent::wire::BridgeEvent::UserMessageStarted {
            session_id,
            message_uuid,
            source,
        } => {
            let _ = event_tx
                .send(ClientEvent::UserMessageStarted { session_id, message_uuid, source })
                .await;
        }
        crate::agent::wire::BridgeEvent::UserMessageRejected {
            session_id,
            message_uuid,
            reason,
        } => {
            let _ = event_tx
                .send(ClientEvent::UserMessageRejected { session_id, message_uuid, reason })
                .await;
        }
        crate::agent::wire::BridgeEvent::TurnInterruptReceipt { session_id, still_queued } => {
            let _ =
                event_tx.send(ClientEvent::TurnInterruptReceipt { session_id, still_queued }).await;
        }
        crate::agent::wire::BridgeEvent::TurnComplete {
            session_id,
            queued_turn_count,
            terminal_reason,
        } => {
            let _ = event_tx
                .send(ClientEvent::TurnComplete { session_id, queued_turn_count, terminal_reason })
                .await;
        }
        crate::agent::wire::BridgeEvent::TurnError {
            session_id,
            message,
            queued_turn_count,
            error_kind,
            api_error_status,
            terminal_reason,
            ..
        } => {
            if let Some(class) = error_kind.as_deref().and_then(parse_turn_error_class) {
                let _ = event_tx
                    .send(ClientEvent::TurnErrorClassified {
                        session_id,
                        message,
                        class,
                        queued_turn_count,
                        api_error_status,
                        terminal_reason,
                    })
                    .await;
            } else {
                let _ = event_tx
                    .send(ClientEvent::TurnError {
                        session_id,
                        message,
                        queued_turn_count,
                        api_error_status,
                        terminal_reason,
                    })
                    .await;
            }
        }
        crate::agent::wire::BridgeEvent::SlashError { session_id, message } => {
            if resume_requested
                && !*connected_once
                && message.to_ascii_lowercase().contains("unknown session")
            {
                let _ = event_tx.send(ClientEvent::FatalError(AppError::SessionNotFound)).await;
                return;
            }
            let _ = event_tx
                .send(ClientEvent::SlashCommandError { session_id: Some(session_id), message })
                .await;
        }
        crate::agent::wire::BridgeEvent::SessionResumeFailed { session_id, message } => {
            if let Some(operation_id) = request_id {
                let _ = event_tx
                    .send(ClientEvent::SessionResumeFailed { session_id, operation_id, message })
                    .await;
            } else {
                tracing::error!(
                    target: crate::logging::targets::BRIDGE_PROTOCOL,
                    event_name = "session_resume_failure_missing_request_id",
                    message = "session resume failure omitted its required request id",
                    outcome = "failure",
                    session_id,
                );
            }
        }
        crate::agent::wire::BridgeEvent::RuntimeReloadCompleted { session_id } => {
            let _ = event_tx.send(ClientEvent::RuntimeReloadCompleted { session_id }).await;
        }
        crate::agent::wire::BridgeEvent::RuntimeReloadFailed { session_id, message } => {
            let _ = event_tx.send(ClientEvent::RuntimeReloadFailed { session_id, message }).await;
        }
        crate::agent::wire::BridgeEvent::SessionReplaced {
            session_id,
            cwd,
            current_model,
            available_models,
            mode,
            fast_mode_state,
            fast_mode_disabled_reason,
            history_updates,
            restored_input,
        } => {
            let history_updates = history_updates
                .unwrap_or_default()
                .into_iter()
                .filter_map(map_session_update)
                .collect();
            let _ = event_tx
                .send(ClientEvent::SessionReplaced {
                    session_id: model::SessionId::new(session_id),
                    cwd,
                    current_model: convert_current_model(current_model),
                    available_models: map_available_models(available_models),
                    mode: mode.map(convert_mode_state),
                    fast_mode_state: convert_fast_mode_state(fast_mode_state),
                    fast_mode_disabled_reason,
                    history_updates,
                    restored_input,
                })
                .await;
        }
        crate::agent::wire::BridgeEvent::SessionsListed { sessions } => {
            let _ = event_tx.send(ClientEvent::SessionsListed { sessions }).await;
        }
        crate::agent::wire::BridgeEvent::Initialized { .. } => {}
        crate::agent::wire::BridgeEvent::StatusSnapshot { session_id, account } => {
            let _ = event_tx
                .send(ClientEvent::StatusSnapshotReceived {
                    session_id,
                    account: convert_account_info(account),
                })
                .await;
        }
        crate::agent::wire::BridgeEvent::ContextUsage { session_id, percentage } => {
            let _ =
                event_tx.send(ClientEvent::ContextUsageReceived { session_id, percentage }).await;
        }
        crate::agent::wire::BridgeEvent::UsageSnapshot { session_id, snapshot, error } => {
            let _ = event_tx
                .send(ClientEvent::StructuredUsageReceived { session_id, snapshot, error })
                .await;
        }
        crate::agent::wire::BridgeEvent::RewindTargets { session_id, targets, error } => {
            let _ = event_tx
                .send(ClientEvent::RewindTargetsReceived {
                    session_id,
                    targets: map_rewind_targets(targets),
                    error,
                })
                .await;
        }
        crate::agent::wire::BridgeEvent::RewindResult {
            session_id,
            restore_mode,
            status,
            file_result,
            message,
        } => {
            let _ = event_tx
                .send(ClientEvent::RewindResultReceived {
                    result: map_rewind_result(
                        session_id,
                        restore_mode,
                        status,
                        file_result,
                        message,
                    ),
                })
                .await;
        }
        crate::agent::wire::BridgeEvent::McpSnapshot {
            session_id,
            servers,
            auth_capabilities,
            source,
            error,
        } => {
            let _ = event_tx
                .send(ClientEvent::McpSnapshotReceived {
                    session_id,
                    servers: servers.into_iter().map(map_mcp_server_status).collect(),
                    auth_capabilities: map_mcp_auth_capabilities(auth_capabilities),
                    source,
                    error,
                })
                .await;
        }
    }
}

async fn handle_connected_event(
    event_tx: &mpsc::Sender<ClientEvent>,
    connected_once: &mut bool,
    event: ConnectedEventData,
) {
    let mode = event.mode.map(convert_mode_state);
    let history_updates = event
        .history_updates
        .unwrap_or_default()
        .into_iter()
        .filter_map(map_session_update)
        .collect();
    if *connected_once {
        let _ = event_tx
            .send(ClientEvent::SessionReplaced {
                session_id: model::SessionId::new(event.session_id),
                cwd: event.cwd,
                current_model: convert_current_model(event.current_model),
                available_models: map_available_models(event.available_models),
                mode,
                fast_mode_state: convert_fast_mode_state(event.fast_mode_state),
                fast_mode_disabled_reason: event.fast_mode_disabled_reason,
                history_updates,
                restored_input: None,
            })
            .await;
    } else {
        *connected_once = true;
        let _ = event_tx
            .send(ClientEvent::Connected {
                session_id: model::SessionId::new(event.session_id),
                cwd: event.cwd,
                current_model: convert_current_model(event.current_model),
                available_models: map_available_models(event.available_models),
                mode,
                fast_mode_state: convert_fast_mode_state(event.fast_mode_state),
                fast_mode_disabled_reason: event.fast_mode_disabled_reason,
                history_updates,
            })
            .await;
    }
}

async fn handle_permission_request_event(
    event_tx: &mpsc::Sender<ClientEvent>,
    connection: &AgentConnection,
    session_id: String,
    request: types::PermissionRequest,
) {
    let (request, tool_call_id) = map_permission_request(&session_id, request);
    let (response_tx, response_rx) = tokio::sync::oneshot::channel();
    if event_tx
        .send(ClientEvent::PermissionRequest {
            session_id: session_id.clone(),
            request,
            response_tx,
        })
        .await
        .is_ok()
    {
        spawn_permission_response_forwarder(
            connection.clone(),
            response_rx,
            session_id,
            tool_call_id,
        );
    } else {
        tracing::error!(
            target: crate::logging::targets::APP_PERMISSION,
            event_name = "permission_request_dispatch_failed",
            message = "failed to dispatch permission request to app event loop",
            outcome = "failure",
            session_id = %session_id,
            tool_call_id = %tool_call_id,
        );
    }
}

async fn handle_question_request_event(
    event_tx: &mpsc::Sender<ClientEvent>,
    connection: &AgentConnection,
    session_id: String,
    request: types::QuestionRequest,
) {
    let (request, tool_call_id) = map_question_request(&session_id, request);
    let (response_tx, response_rx) = tokio::sync::oneshot::channel();
    if event_tx
        .send(ClientEvent::QuestionRequest { session_id: session_id.clone(), request, response_tx })
        .await
        .is_ok()
    {
        spawn_question_response_forwarder(
            connection.clone(),
            response_rx,
            session_id,
            tool_call_id,
        );
    } else {
        tracing::error!(
            target: crate::logging::targets::APP_PERMISSION,
            event_name = "question_request_dispatch_failed",
            message = "failed to dispatch question request to app event loop",
            outcome = "failure",
            session_id = %session_id,
            tool_call_id = %tool_call_id,
        );
    }
}

async fn handle_user_dialog_request_event(
    event_tx: &mpsc::Sender<ClientEvent>,
    connection: &AgentConnection,
    session_id: String,
    request: types::UserDialogRequest,
) {
    let (request, request_id) = map_user_dialog_request(&session_id, request);
    let (response_tx, response_rx) = tokio::sync::oneshot::channel();
    if event_tx
        .send(ClientEvent::UserDialogRequest {
            session_id: session_id.clone(),
            request,
            response_tx,
        })
        .await
        .is_ok()
    {
        spawn_user_dialog_response_forwarder(
            connection.clone(),
            response_rx,
            session_id,
            request_id,
        );
    } else {
        tracing::error!(
            target: crate::logging::targets::APP_PERMISSION,
            event_name = "user_dialog_request_dispatch_failed",
            message = "failed to dispatch user dialog request to app event loop",
            outcome = "failure",
            session_id = %session_id,
            request_id = %request_id,
        );
    }
}

async fn handle_elicitation_request_event(
    event_tx: &mpsc::Sender<ClientEvent>,
    session_id: &str,
    request: types::ElicitationRequest,
) {
    if event_tx
        .send(ClientEvent::McpElicitationRequest { session_id: session_id.to_owned(), request })
        .await
        .is_err()
    {
        tracing::error!(
            target: crate::logging::targets::APP_PERMISSION,
            event_name = "elicitation_request_dispatch_failed",
            message = "failed to dispatch elicitation request to app event loop",
            outcome = "failure",
            session_id = %session_id,
        );
    }
}

fn spawn_permission_response_forwarder(
    connection: AgentConnection,
    response_rx: tokio::sync::oneshot::Receiver<model::RequestPermissionResponse>,
    session_id: String,
    tool_call_id: String,
) {
    tokio::task::spawn_local(async move {
        let Ok(response) = response_rx.await else {
            tracing::warn!(
                target: crate::logging::targets::APP_PERMISSION,
                event_name = "permission_response_abandoned",
                message = "permission response channel closed before bridge forwarding",
                outcome = "dropped",
                session_id = %session_id,
                tool_call_id = %tool_call_id,
            );
            return;
        };
        let outcome = match response.outcome {
            model::RequestPermissionOutcome::Selected(selected) => {
                types::PermissionOutcome::Selected { option_id: selected.option_id.clone() }
            }
            model::RequestPermissionOutcome::Cancelled => types::PermissionOutcome::Cancelled,
        };
        let selected_option = match &outcome {
            types::PermissionOutcome::Selected { option_id } => option_id.clone(),
            types::PermissionOutcome::Cancelled => "cancelled".to_owned(),
        };
        let session_id_for_log = session_id.clone();
        let tool_call_id_for_log = tool_call_id.clone();
        match connection.respond_to_permission(session_id, tool_call_id, outcome).await {
            Ok(()) => {
                tracing::info!(
                    target: crate::logging::targets::APP_PERMISSION,
                    event_name = "permission_response_forwarded",
                    message = "permission response forwarded to bridge",
                    outcome = "success",
                    session_id = %session_id_for_log,
                    tool_call_id = %tool_call_id_for_log,
                    selected_option = %selected_option,
                );
            }
            Err(err) => {
                tracing::error!(
                    target: crate::logging::targets::APP_PERMISSION,
                    event_name = "permission_response_forward_failed",
                    message = "failed to forward permission response to bridge",
                    outcome = "failure",
                    session_id = %session_id_for_log,
                    tool_call_id = %tool_call_id_for_log,
                    selected_option = %selected_option,
                    error = %err,
                );
            }
        }
    });
}

fn spawn_question_response_forwarder(
    connection: AgentConnection,
    response_rx: tokio::sync::oneshot::Receiver<model::RequestQuestionResponse>,
    session_id: String,
    tool_call_id: String,
) {
    tokio::task::spawn_local(async move {
        let Ok(response) = response_rx.await else {
            tracing::warn!(
                target: crate::logging::targets::APP_PERMISSION,
                event_name = "question_response_abandoned",
                message = "question response channel closed before bridge forwarding",
                outcome = "dropped",
                session_id = %session_id,
                tool_call_id = %tool_call_id,
            );
            return;
        };
        let outcome = match response.outcome {
            model::RequestQuestionOutcome::Answered(answered) => types::QuestionOutcome::Answered {
                selected_option_ids: answered.selected_option_ids,
                annotation: answered.annotation.map(|annotation| types::QuestionAnnotation {
                    preview: annotation.preview,
                    notes: annotation.notes,
                }),
            },
            model::RequestQuestionOutcome::Cancelled => types::QuestionOutcome::Cancelled,
        };
        let selected_option_count = match &outcome {
            types::QuestionOutcome::Answered { selected_option_ids, .. } => {
                selected_option_ids.len()
            }
            types::QuestionOutcome::Cancelled => 0,
        };
        let session_id_for_log = session_id.clone();
        let tool_call_id_for_log = tool_call_id.clone();
        match connection.respond_to_question(session_id, tool_call_id, outcome).await {
            Ok(()) => {
                tracing::info!(
                    target: crate::logging::targets::APP_PERMISSION,
                    event_name = "question_response_forwarded",
                    message = "question response forwarded to bridge",
                    outcome = "success",
                    session_id = %session_id_for_log,
                    tool_call_id = %tool_call_id_for_log,
                    selected_option_count,
                );
            }
            Err(err) => {
                tracing::error!(
                    target: crate::logging::targets::APP_PERMISSION,
                    event_name = "question_response_forward_failed",
                    message = "failed to forward question response to bridge",
                    outcome = "failure",
                    session_id = %session_id_for_log,
                    tool_call_id = %tool_call_id_for_log,
                    selected_option_count,
                    error = %err,
                );
            }
        }
    });
}

fn spawn_user_dialog_response_forwarder(
    connection: AgentConnection,
    response_rx: tokio::sync::oneshot::Receiver<model::RequestUserDialogResponse>,
    session_id: String,
    request_id: String,
) {
    tokio::task::spawn_local(async move {
        let Ok(response) = response_rx.await else {
            tracing::warn!(
                target: crate::logging::targets::APP_PERMISSION,
                event_name = "user_dialog_response_abandoned",
                message = "user dialog response channel closed before bridge forwarding",
                outcome = "dropped",
                session_id = %session_id,
                request_id = %request_id,
            );
            return;
        };
        let outcome = match response.outcome {
            model::RequestUserDialogOutcome::Selected(selected) => {
                types::UserDialogOutcome::Selected { option_id: selected.option_id }
            }
            model::RequestUserDialogOutcome::Cancelled => types::UserDialogOutcome::Cancelled,
        };
        let selected_option = match &outcome {
            types::UserDialogOutcome::Selected { option_id } => option_id.clone(),
            types::UserDialogOutcome::Cancelled => "cancelled".to_owned(),
        };
        let session_id_for_log = session_id.clone();
        let request_id_for_log = request_id.clone();
        match connection.respond_to_user_dialog(session_id, request_id, outcome).await {
            Ok(()) => {
                tracing::info!(
                    target: crate::logging::targets::APP_PERMISSION,
                    event_name = "user_dialog_response_forwarded",
                    message = "user dialog response forwarded to bridge",
                    outcome = "success",
                    session_id = %session_id_for_log,
                    request_id = %request_id_for_log,
                    selected_option = %selected_option,
                );
            }
            Err(err) => {
                tracing::error!(
                    target: crate::logging::targets::APP_PERMISSION,
                    event_name = "user_dialog_response_forward_failed",
                    message = "failed to forward user dialog response to bridge",
                    outcome = "failure",
                    session_id = %session_id_for_log,
                    request_id = %request_id_for_log,
                    selected_option = %selected_option,
                    error = %err,
                );
            }
        }
    });
}
