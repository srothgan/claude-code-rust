// SPDX-License-Identifier: Apache-2.0

use super::super::{
    App, AppStatus, ChatMessage, FocusTarget, InlinePermission, InlineQuestion, InvalidationLevel,
    MessageBlock, MessageRole, SubagentPermissionContext, SystemSeverity, TextBlock, ToolCallInfo,
    ToolCallScope, UserDialogBlock,
};
use super::compaction;
use crate::agent::error_handling::{TurnErrorClass, classify_turn_error, summarize_internal_error};
use crate::agent::model;
use std::collections::BTreeSet;
use std::time::Instant;

const CONVERSATION_INTERRUPTED_HINT: &str =
    "Conversation interrupted. Tell the model how to proceed.";
const TURN_ERROR_INPUT_LOCK_HINT: &str =
    "Input disabled after an error. Press Ctrl+Q to quit and try again.";
const PLAN_LIMIT_NEXT_STEPS_HINT: &str =
    "You can keep editing and retry here when your allowance is available.";
const AUTH_REQUIRED_NEXT_STEPS_HINT: &str = "Authentication required. Type /login to authenticate, or run `claude auth login` in a terminal.";

pub(super) fn handle_btw_result_event(
    app: &mut App,
    btw_id: &str,
    echoed_question: &str,
    answer: String,
    metadata: &crate::agent::wire::SideQuestionMetadata,
) {
    let Some(local_question) = app.btw.get_active(btw_id).map(|item| item.question.clone()) else {
        log_unknown_btw_event(app, btw_id, "result");
        return;
    };
    if local_question != echoed_question {
        let reason = "bridge protocol error: echoed question did not match".to_owned();
        let _ = app.btw.fail(btw_id, reason, Instant::now());
        crate::app::btw::dispatch_next(app);
        tracing::error!(
            target: crate::logging::targets::BRIDGE_PROTOCOL,
            event_name = "side_question_question_mismatch",
            message = "side-question result echoed a different question",
            outcome = "failure",
            btw_id,
        );
        return;
    }

    let Some(completed) = app.btw.complete(btw_id) else {
        log_unknown_btw_event(app, btw_id, "result");
        return;
    };
    super::notices::insert_turn_presentation_block(
        app,
        MessageBlock::BtwExchange(super::super::BtwExchangeBlock::new(completed.question, answer)),
        MessageRole::System(None),
    );
    app.enforce_history_retention_tracked();
    app.request_chat_mutable_rebuild();
    crate::app::btw::dispatch_next(app);
    tracing::info!(
        target: crate::logging::targets::APP_SESSION,
        event_name = "side_question_completed",
        message = "side question completed",
        outcome = "success",
        btw_id,
        synthetic = metadata.synthetic,
        refusal_fallback = metadata.refusal_fallback.is_some(),
    );
}

pub(super) fn handle_btw_failed_event(
    app: &mut App,
    btw_id: &str,
    echoed_question: &str,
    error: String,
) {
    let Some(local_question) = app.btw.get_active(btw_id).map(|item| item.question.clone()) else {
        log_unknown_btw_event(app, btw_id, "failure");
        return;
    };
    let reason = if local_question == echoed_question {
        error
    } else {
        tracing::error!(
            target: crate::logging::targets::BRIDGE_PROTOCOL,
            event_name = "side_question_question_mismatch",
            message = "side-question failure echoed a different question",
            outcome = "failure",
            btw_id,
        );
        "bridge protocol error: echoed question did not match".to_owned()
    };
    let _ = app.btw.fail(btw_id, reason, Instant::now());
    crate::app::btw::dispatch_next(app);
}

fn log_unknown_btw_event(app: &App, btw_id: &str, event_kind: &str) {
    tracing::warn!(
        target: crate::logging::targets::BRIDGE_PROTOCOL,
        event_name = "stale_side_question_event_dropped",
        message = "side-question event did not match local outstanding state",
        outcome = "dropped",
        session_id = app
            .session_runtime
            .session_id
            .as_ref()
            .map_or("<none>", model::SessionId::as_str),
        btw_id,
        event_kind,
    );
}

fn permission_initial_selected_index(request: &model::RequestPermissionRequest) -> usize {
    if !request.display.as_ref().is_some_and(|display| display.default_to_no) {
        return 0;
    }
    request
        .options
        .iter()
        .position(|option| {
            matches!(
                option.kind,
                model::PermissionOptionKind::RejectOnce
                    | model::PermissionOptionKind::RejectAlways
                    | model::PermissionOptionKind::PlanReject
            )
        })
        .unwrap_or_else(|| request.options.len().saturating_sub(1))
}

#[derive(Clone, Copy)]
struct TurnExitState {
    tail_assistant_idx: Option<usize>,
    turn_was_active: bool,
    cancel_requested: bool,
}

fn log_permission_mcp_provenance(
    session_id: &str,
    tool_id: &str,
    server: Option<&model::McpServerProvenance>,
) {
    let Some(server) = server else { return };
    tracing::debug!(
        target: crate::logging::targets::APP_PERMISSION,
        event_name = "permission_request_mcp_provenance",
        message = "MCP permission request provenance received",
        outcome = "observed",
        session_id,
        tool_call_id = tool_id,
        mcp_server_name = %server.name,
        mcp_server_source = %server.source,
        sdk_registered = server.source == "sdk",
    );
}

pub(super) fn handle_permission_request_event(
    app: &mut App,
    request: model::RequestPermissionRequest,
    response_tx: tokio::sync::oneshot::Sender<model::RequestPermissionResponse>,
) {
    let session_id = request.session_id.to_string();
    let tool_id = request.tool_call.tool_call_id.clone();
    let options = request.options.clone();
    let permission_context = resolve_permission_context(app, &tool_id);
    let display_title =
        permission_display_field(request.display.as_ref(), |display| display.title.as_deref());
    let display_name = permission_display_field(request.display.as_ref(), |display| {
        display.display_name.as_deref()
    });
    let display_description = permission_display_field(request.display.as_ref(), |display| {
        display.description.as_deref()
    });
    let selected_index = permission_initial_selected_index(&request);
    log_permission_mcp_provenance(&session_id, &tool_id, request.mcp_server.as_ref());

    let Some((mi, bi)) = app.lookup_tool_call(&tool_id) else {
        tracing::warn!(
            target: crate::logging::targets::APP_PERMISSION,
            event_name = "permission_request_rejected",
            message = "permission request rejected for unknown tool call",
            outcome = "dropped",
            session_id = %session_id,
            tool_call_id = %tool_id,
            reason = "unknown_tool_call",
        );
        reject_permission_request(response_tx, &options);
        return;
    };

    if app.pending_interaction_ids.iter().any(|id| id == &tool_id) {
        tracing::warn!(
            target: crate::logging::targets::APP_PERMISSION,
            event_name = "permission_request_rejected",
            message = "duplicate permission request rejected",
            outcome = "dropped",
            session_id = %session_id,
            tool_call_id = %tool_id,
            reason = "duplicate_pending_interaction",
        );
        reject_permission_request(response_tx, &options);
        return;
    }

    let mut layout_dirty = false;
    let auto_focus = app.pending_interaction_ids.is_empty() && !app.has_draft_input_for_focus();
    if let Some(tc) = app.tool_call_at_mut(mi, bi) {
        tc.pending_permission = Some(InlinePermission {
            options: request.options,
            display: request.display,
            subagent_context: permission_context.clone(),
            response_tx,
            selected_index,
            focused: auto_focus,
        });
        tc.invalidate_render_cache();
        layout_dirty = true;
        app.pending_interaction_ids.push(tool_id.clone());
        if auto_focus {
            app.claim_focus_target(FocusTarget::Permission);
        }
        app.notifications.notify(
            &app.config,
            super::super::notify::NotifyEvent::PermissionRequired,
            &session_id,
            Some(&tool_id),
        );
        log_permission_request_applied(
            &session_id,
            &tool_id,
            options.len(),
            auto_focus,
            permission_context.as_ref(),
            PermissionDisplayLogFields {
                title: &display_title,
                name: &display_name,
                description: &display_description,
            },
        );
    } else {
        tracing::warn!(
            target: crate::logging::targets::APP_PERMISSION,
            event_name = "permission_request_rejected",
            message = "permission request rejected because target block was not a tool call",
            outcome = "dropped",
            session_id = %session_id,
            tool_call_id = %tool_id,
            reason = "non_tool_block",
        );
        reject_permission_request(response_tx, &options);
    }

    if layout_dirty {
        app.sync_after_message_blocks_changed(mi);
        app.request_chat_mutable_rebuild();
    }
}

#[derive(Clone, Copy)]
struct PermissionDisplayLogFields<'a> {
    title: &'a str,
    name: &'a str,
    description: &'a str,
}

fn log_permission_request_applied(
    session_id: &str,
    tool_id: &str,
    option_count: usize,
    focused: bool,
    permission_context: Option<&SubagentPermissionContext>,
    display_fields: PermissionDisplayLogFields<'_>,
) {
    tracing::info!(
        target: crate::logging::targets::APP_PERMISSION,
        event_name = "permission_request_applied",
        message = "permission request applied to inline tool call",
        outcome = "success",
        session_id = %session_id,
        tool_call_id = %tool_id,
        option_count,
        focused,
        subagent_label = %permission_context.map_or("", |context| context.subagent_label.as_str()),
        subagent_parent_tool_call_id = %permission_context
            .map_or("", |context| context.parent_tool_call_id.as_str()),
        subagent_parent_tool_title = %permission_context
            .and_then(|context| context.parent_tool_title.as_deref())
            .unwrap_or(""),
        subagent_parent_model = %permission_context
            .and_then(|context| context.parent_model.as_deref())
            .unwrap_or(""),
        subagent_child_tool_name = %permission_context
            .map_or("", |context| context.child_tool_name.as_str()),
        subagent_child_tool_title = %permission_context
            .map_or("", |context| context.child_tool_title.as_str()),
        permission_display_title = %display_fields.title,
        permission_display_name = %display_fields.name,
        permission_display_description = %display_fields.description,
    );
}

fn permission_display_field(
    display: Option<&model::PermissionDisplay>,
    field: impl FnOnce(&model::PermissionDisplay) -> Option<&str>,
) -> String {
    display
        .and_then(field)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("")
        .to_owned()
}

fn resolve_permission_context(app: &App, tool_call_id: &str) -> Option<SubagentPermissionContext> {
    let ToolCallScope::SubagentChild { parent_tool_use_id } = app.tool_call_scope(tool_call_id)?
    else {
        return None;
    };
    let child = indexed_tool_call(app, tool_call_id)?;
    let parent = indexed_tool_call(app, &parent_tool_use_id);
    let parent_raw_input = parent.and_then(|tool| tool.raw_input.clone());
    let parent_model = parent_raw_input
        .as_ref()
        .and_then(|raw_input| raw_input_string(raw_input, "model"))
        .map(str::to_owned);
    let parent_tool_title =
        parent.map(|tool| tool.title.trim()).filter(|title| !title.is_empty()).map(str::to_owned);
    let subagent_label = parent_raw_input
        .as_ref()
        .and_then(|raw_input| raw_input_string(raw_input, "name"))
        .or_else(|| {
            parent_raw_input
                .as_ref()
                .and_then(|raw_input| raw_input_string(raw_input, "subagent_type"))
        })
        .or(parent_tool_title.as_deref())
        .unwrap_or("Subagent")
        .to_owned();

    Some(SubagentPermissionContext {
        subagent_label,
        child_tool_name: child.sdk_tool_name.clone(),
        child_tool_title: child.title.clone(),
        parent_tool_call_id: parent_tool_use_id,
        parent_tool_title,
        parent_model,
        parent_raw_input,
    })
}

fn indexed_tool_call<'a>(app: &'a App, tool_call_id: &str) -> Option<&'a ToolCallInfo> {
    let (mi, bi) = app.lookup_tool_call(tool_call_id)?;
    let MessageBlock::ToolCall(tc) = app.transcript.messages.get(mi)?.blocks.get(bi)? else {
        return None;
    };
    Some(tc.as_ref())
}

fn raw_input_string<'a>(raw_input: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    raw_input
        .as_object()
        .and_then(|input| input.get(key))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

pub(super) fn handle_question_request_event(
    app: &mut App,
    request: model::RequestQuestionRequest,
    response_tx: tokio::sync::oneshot::Sender<model::RequestQuestionResponse>,
) {
    let session_id = request.session_id.to_string();
    let tool_id = request.tool_call.tool_call_id.clone();
    let option_count = request.prompt.options.len();
    let question_index = request.question_index;
    let total_questions = request.total_questions;

    let Some((mi, bi)) = app.lookup_tool_call(&tool_id) else {
        tracing::warn!(
            target: crate::logging::targets::APP_PERMISSION,
            event_name = "question_request_rejected",
            message = "question request rejected for unknown tool call",
            outcome = "dropped",
            session_id = %session_id,
            tool_call_id = %tool_id,
            reason = "unknown_tool_call",
        );
        let _ = response_tx
            .send(model::RequestQuestionResponse::new(model::RequestQuestionOutcome::Cancelled));
        return;
    };

    if app.pending_interaction_ids.iter().any(|id| id == &tool_id) {
        tracing::warn!(
            target: crate::logging::targets::APP_PERMISSION,
            event_name = "question_request_rejected",
            message = "duplicate question request rejected",
            outcome = "dropped",
            session_id = %session_id,
            tool_call_id = %tool_id,
            reason = "duplicate_pending_interaction",
        );
        let _ = response_tx
            .send(model::RequestQuestionResponse::new(model::RequestQuestionOutcome::Cancelled));
        return;
    }

    let mut layout_dirty = false;
    let auto_focus = app.pending_interaction_ids.is_empty() && !app.has_draft_input_for_focus();
    if let Some(tc) = app.tool_call_at_mut(mi, bi) {
        tc.pending_question = Some(InlineQuestion {
            idle_timeout: request.idle_timeout_ms.map(std::time::Duration::from_millis),
            last_activity: crate::app::input_event_now(),
            prompt: request.prompt,
            response_tx,
            focused_option_index: 0,
            selected_option_indices: BTreeSet::new(),
            notes: String::new(),
            notes_cursor: 0,
            editing_notes: false,
            focused: auto_focus,
            question_index: request.question_index,
            total_questions: request.total_questions,
        });
        tc.invalidate_render_cache();
        layout_dirty = true;
        app.pending_interaction_ids.push(tool_id.clone());
        if auto_focus {
            app.claim_focus_target(FocusTarget::Permission);
        }
        app.notifications.notify(
            &app.config,
            super::super::notify::NotifyEvent::QuestionRequired,
            &session_id,
            Some(&tool_id),
        );
        tracing::info!(
            target: crate::logging::targets::APP_PERMISSION,
            event_name = "question_request_applied",
            message = "question request applied to inline tool call",
            outcome = "success",
            session_id = %session_id,
            tool_call_id = %tool_id,
            question_index,
            total_questions,
            option_count,
            focused = auto_focus,
        );
    } else {
        tracing::warn!(
            target: crate::logging::targets::APP_PERMISSION,
            event_name = "question_request_rejected",
            message = "question request rejected because target block was not a tool call",
            outcome = "dropped",
            session_id = %session_id,
            tool_call_id = %tool_id,
            reason = "non_tool_block",
        );
        let _ = response_tx
            .send(model::RequestQuestionResponse::new(model::RequestQuestionOutcome::Cancelled));
    }

    if layout_dirty {
        app.sync_after_message_blocks_changed(mi);
        app.request_chat_mutable_rebuild();
    }
}

pub(super) fn handle_user_dialog_request_event(
    app: &mut App,
    request: model::RequestUserDialogRequest,
    response_tx: tokio::sync::oneshot::Sender<model::RequestUserDialogResponse>,
) {
    let session_id = request.session_id.to_string();
    let request_id = request.request_id.clone();
    let dialog_kind = request.dialog_kind.clone();
    let option_count = request.options.len();

    if app.pending_interaction_ids.iter().any(|id| id == &request_id) {
        tracing::warn!(
            target: crate::logging::targets::APP_PERMISSION,
            event_name = "user_dialog_request_rejected",
            message = "duplicate user dialog request rejected",
            outcome = "dropped",
            session_id = %session_id,
            request_id = %request_id,
            reason = "duplicate_pending_interaction",
        );
        let _ = response_tx.send(model::RequestUserDialogResponse::new(
            model::RequestUserDialogOutcome::Cancelled,
        ));
        return;
    }

    let auto_focus = app.pending_interaction_ids.is_empty() && !app.has_draft_input_for_focus();
    let mi = app.transcript.messages.len();
    let bi = 0;
    let mut block = UserDialogBlock::new(request, response_tx);
    block.focused = auto_focus;
    app.push_message_tracked(ChatMessage::new(
        MessageRole::System(Some(SystemSeverity::Warning)),
        vec![MessageBlock::UserDialog(block)],
        None,
    ));
    app.index_tool_call(request_id.clone(), mi, bi);
    app.pending_interaction_ids.push(request_id.clone());
    if auto_focus {
        app.claim_focus_target(FocusTarget::Permission);
    }
    app.notifications.notify(
        &app.config,
        super::super::notify::NotifyEvent::QuestionRequired,
        &session_id,
        Some(&request_id),
    );
    app.sync_after_message_blocks_changed(mi);
    app.request_chat_mutable_rebuild();
    tracing::info!(
        target: crate::logging::targets::APP_PERMISSION,
        event_name = "user_dialog_request_applied",
        message = "user dialog request applied as inline system chooser",
        outcome = "success",
        session_id = %session_id,
        request_id = %request_id,
        dialog_kind = %dialog_kind,
        option_count,
        focused = auto_focus,
    );
}

fn reject_permission_request(
    response_tx: tokio::sync::oneshot::Sender<model::RequestPermissionResponse>,
    options: &[model::PermissionOption],
) {
    if let Some(last_opt) = options.last() {
        let _ = response_tx.send(model::RequestPermissionResponse::new(
            model::RequestPermissionOutcome::Selected(model::SelectedPermissionOutcome::new(
                last_opt.option_id.clone(),
            )),
        ));
    }
}

pub(super) fn handle_turn_cancelled_event(app: &mut App) {
    app.turn.cancel_requested = true;
    let _ = app.finalize_in_progress_tool_calls(model::ToolCallStatus::Failed);
}

fn begin_turn_exit(app: &mut App, emit_manual_compaction_success: bool) -> TurnExitState {
    let state = TurnExitState {
        tail_assistant_idx: app
            .transcript
            .messages
            .iter()
            .rposition(|m| matches!(m.role, MessageRole::Assistant)),
        turn_was_active: matches!(app.status, AppStatus::Thinking | AppStatus::Running),
        cancel_requested: app.turn.cancel_requested,
    };
    if (state.turn_was_active || state.cancel_requested)
        && let Some(index) = app.active_turn_assistant_idx()
    {
        app.transcript.messages[index].timing.finish();
    }
    compaction::finish_inferred(app, emit_manual_compaction_success);
    app.turn.cancel_requested = false;
    state
}

fn finish_ready_turn_exit(app: &mut App, exit: TurnExitState, tool_status: model::ToolCallStatus) {
    app.finalize_turn_runtime_artifacts(tool_status);
    app.status = AppStatus::Ready;
    app.sdk_inventory.clear_rewind_targets();
    app.sync_git_context();

    let removed_tail_assistant = remove_empty_tail_assistant(app, exit.tail_assistant_idx);
    if exit.cancel_requested && app.pending_user_messages.is_empty() {
        push_interrupted_hint(app);
    }
    if removed_tail_assistant.is_none() && (exit.turn_was_active || exit.cancel_requested) {
        mark_turn_exit_assistant_layout_dirty(app, exit.tail_assistant_idx);
    }
    app.turn.reset_for_turn_exit();
}

pub(super) fn handle_turn_complete_event(
    app: &mut App,
    queued_turn_count: Option<usize>,
    terminal_reason: Option<crate::agent::types::TerminalReason>,
) {
    log_queued_turn_count(app, queued_turn_count, "success");
    let exit = begin_turn_exit(app, true);
    let turn_was_active = exit.turn_was_active;
    if let Some(reason) = terminal_reason {
        tracing::debug!(
            target: crate::logging::targets::APP_SESSION,
            event_name = "turn_complete_terminal_reason",
            message = "turn completed with SDK terminal reason",
            outcome = "success",
            terminal_reason = reason.as_stored(),
        );
    }
    let tool_status = if exit.cancel_requested {
        model::ToolCallStatus::Failed
    } else {
        model::ToolCallStatus::Completed
    };
    finish_ready_turn_exit(app, exit, tool_status);
    request_post_turn_resize_purge_replay_if_needed(app);
    crate::app::session_runtime::request_context_usage_refresh(app);
    crate::app::config::refresh_session_transcript_if_shown(app);
    if turn_was_active
        && !exit.cancel_requested
        && !matches!(
            terminal_reason,
            Some(
                crate::agent::types::TerminalReason::AbortedStreaming
                    | crate::agent::types::TerminalReason::AbortedTools
            )
        )
    {
        app.notifications.notify(
            &app.config,
            super::super::notify::NotifyEvent::TurnComplete,
            app.session_runtime.session_id.as_ref().map_or("", model::SessionId::as_str),
            None,
        );
    }
}

pub(super) fn handle_turn_error_event(
    app: &mut App,
    msg: &str,
    classified: Option<TurnErrorClass>,
    queued_turn_count: Option<usize>,
    api_error_status: Option<u16>,
    terminal_reason: Option<crate::agent::types::TerminalReason>,
) {
    log_queued_turn_count(app, queued_turn_count, "error");
    let exit = begin_turn_exit(app, false);

    if exit.cancel_requested {
        let summary = summarize_internal_error(msg);
        // The user asked for the cancellation, so this is routine and stays out of the
        // default log; a genuine turn error below is still logged as an error.
        tracing::debug!(
            target: crate::logging::targets::APP_SESSION,
            event_name = "turn_error_suppressed",
            message = "turn error suppressed after cancellation request",
            outcome = "cancelled",
            error_preview = %summary,
            api_error_status = api_error_status.unwrap_or_default(),
            terminal_reason = terminal_reason.map_or("", crate::agent::types::TerminalReason::as_stored),
        );
        app.pending_submit = None;
        finish_ready_turn_exit(app, exit, model::ToolCallStatus::Failed);
        request_post_turn_resize_purge_replay_if_needed(app);
        crate::app::session_runtime::request_context_usage_refresh(app);
        crate::app::config::refresh_session_transcript_if_shown(app);
        return;
    }

    let error_class = classified.unwrap_or_else(|| classify_turn_error(msg));
    let summary = summarize_internal_error(msg);
    tracing::error!(
        target: crate::logging::targets::APP_SESSION,
        event_name = "turn_error_received",
        message = "turn error received",
        outcome = "failure",
        error_class = ?error_class,
        error_preview = %summary,
        api_error_status = api_error_status.unwrap_or_default(),
        terminal_reason = terminal_reason.map_or("", crate::agent::types::TerminalReason::as_stored),
    );
    apply_turn_error_class_side_effects(app, error_class, &summary, api_error_status);
    app.finalize_turn_runtime_artifacts(model::ToolCallStatus::Failed);
    // The composer may already contain the next unsent message. An error in
    // the active turn does not consume that draft or its paste/image atoms.
    app.pending_submit = None;
    app.status = match error_class {
        TurnErrorClass::PlanLimit => AppStatus::Ready,
        _ => AppStatus::Error,
    };
    // A failed reply owns its notice even when no assistant text arrived. Keep
    // that owner alive until the notice is attached, then release turn tracking.
    push_turn_error_message(app, msg, error_class);
    app.turn.reset_for_turn_exit();
    request_post_turn_resize_purge_replay_if_needed(app);
    crate::app::session_runtime::request_context_usage_refresh(app);
    crate::app::config::refresh_session_transcript_if_shown(app);
}

pub(super) fn handle_user_message_queued_event(app: &mut App, message_uuid: &str) {
    let transitioned = app.pending_user_messages.mark_queued(message_uuid);
    tracing::debug!(
        target: crate::logging::targets::APP_INPUT,
        event_name = "user_message_locally_accepted",
        message = "bridge accepted user message into the SDK input path",
        outcome = if transitioned { "queued" } else { "ignored" },
        message_uuid,
        pending_message_count = app.pending_user_messages.len(),
    );
}

pub(super) fn handle_user_message_started_event(
    app: &mut App,
    message_uuid: &str,
    source: crate::agent::types::UserMessageStartSource,
) {
    let started = app.pending_user_messages.take_started_prefix(message_uuid);
    if started.is_empty() {
        tracing::debug!(
            target: crate::logging::targets::APP_INPUT,
            event_name = "user_message_start_ignored",
            message = "user message start acknowledgement did not match a pending host message",
            outcome = "ignored",
            message_uuid,
            source = ?source,
        );
        return;
    }

    let coalesced_message_count = started.len();
    for message in started {
        let mut user = ChatMessage::new(
            MessageRole::User,
            vec![MessageBlock::Text(
                TextBlock::from_complete(&message.text)
                    .with_source_message_uuid(Some(&message.uuid)),
            )],
            None,
        );
        user.timing.timestamp = message.timestamp;
        app.push_message_tracked(user);
        app.start_assistant_response();
    }
    tracing::info!(
        target: crate::logging::targets::APP_INPUT,
        event_name = "user_message_started",
        message = "SDK began a correlated pending user turn",
        outcome = "started",
        message_uuid,
        source = ?source,
        coalesced_message_count,
        pending_message_count = app.pending_user_messages.len(),
    );
}

pub(super) fn handle_user_message_rejected_event(app: &mut App, message_uuid: &str, reason: &str) {
    let Some(message) = app.pending_user_messages.remove(message_uuid) else {
        tracing::debug!(
            target: crate::logging::targets::APP_INPUT,
            event_name = "user_message_rejection_ignored",
            message = "user message rejection did not match a pending host message",
            outcome = "ignored",
            message_uuid,
            reason,
        );
        return;
    };
    restore_pending_user_messages(app, vec![message]);
    super::notices::emit_system_notice(
        app,
        SystemSeverity::Error,
        &format!("Queued message was not accepted and has been restored: {reason}"),
    );
}

pub(super) fn handle_turn_interrupt_receipt_event(app: &mut App, still_queued: &[String]) {
    let matched = app.pending_user_messages.reconcile_interrupt_survivors(still_queued);
    tracing::info!(
        target: crate::logging::targets::APP_INPUT,
        event_name = "turn_interrupt_queue_reconciled",
        message = "interrupt receipt reconciled queued user messages",
        outcome = "success",
        receipt_message_count = still_queued.len(),
        matched_message_count = matched,
        pending_message_count = app.pending_user_messages.len(),
    );
}

pub(super) fn recover_all_pending_user_messages(app: &mut App) {
    let messages = app.pending_user_messages.drain();
    restore_pending_user_messages(app, messages);
}

fn restore_pending_user_messages(app: &mut App, messages: Vec<super::super::PendingUserMessage>) {
    if messages.is_empty() {
        return;
    }
    let current_text = app.input.text();
    let mut text_parts: Vec<String> = messages.iter().map(|message| message.text.clone()).collect();
    if !current_text.trim().is_empty() {
        text_parts.push(current_text);
    }
    let mut images = messages.into_iter().flat_map(|message| message.images).collect::<Vec<_>>();
    images.extend(app.input.images());
    app.input.set_text(&text_parts.join("\n"));
    app.input.image_payloads = images;
    app.input.renumber_image_badges();
    app.request_active_surface_repaint();
}

fn log_queued_turn_count(app: &App, queued_turn_count: Option<usize>, outcome: &str) {
    let Some(queued_turn_count) = queued_turn_count else {
        return;
    };
    tracing::debug!(
        target: crate::logging::targets::APP_INPUT,
        event_name = "sdk_queued_turn_count_observed",
        message = "SDK result reported its queued user-send count",
        outcome,
        queued_turn_count,
        host_pending_message_count = app.pending_user_messages.len(),
    );
}

fn apply_turn_error_class_side_effects(
    app: &mut App,
    error_class: TurnErrorClass,
    summary: &str,
    api_error_status: Option<u16>,
) {
    match error_class {
        TurnErrorClass::PlanLimit => {
            tracing::warn!(
                target: crate::logging::targets::APP_SESSION,
                event_name = "turn_error_classified",
                message = "turn error classified as plan limit",
                outcome = "degraded",
                error_class = "plan_limit",
                error_preview = %summary,
            );
        }
        TurnErrorClass::AuthRequired => {
            tracing::warn!(
                target: crate::logging::targets::APP_AUTH,
                event_name = "turn_error_classified",
                message = "turn error indicates authentication is required",
                outcome = "degraded",
                error_class = "auth_required",
                error_preview = %summary,
            );
            app.exit_error = Some(crate::error::AppError::AuthRequired);
            app.request_shutdown();
        }
        TurnErrorClass::AccountAccess => {
            tracing::warn!(
                target: crate::logging::targets::APP_AUTH,
                event_name = "turn_error_classified",
                message = "turn error indicates account access is not allowed",
                outcome = "degraded",
                error_class = "account_access",
                error_preview = %summary,
                api_error_status = api_error_status.unwrap_or_default(),
            );
        }
        TurnErrorClass::ModelUnavailable => {
            tracing::warn!(
                target: crate::logging::targets::APP_SESSION,
                event_name = "turn_error_classified",
                message = "turn error indicates model is unavailable",
                outcome = "degraded",
                error_class = "model_unavailable",
                error_preview = %summary,
                api_error_status = api_error_status.unwrap_or_default(),
            );
        }
        TurnErrorClass::TransientService => {
            tracing::warn!(
                target: crate::logging::targets::APP_SESSION,
                event_name = "turn_error_classified",
                message = "turn error indicates transient service failure",
                outcome = "degraded",
                error_class = "transient_service",
                error_preview = %summary,
                api_error_status = api_error_status.unwrap_or_default(),
            );
        }
        TurnErrorClass::Internal => {
            tracing::debug!(
                target: crate::logging::targets::APP_SESSION,
                event_name = "turn_error_classified",
                message = "turn error classified as internal SDK error",
                outcome = "degraded",
                error_class = "internal",
                error_preview = %summary,
            );
        }
        TurnErrorClass::Other => {}
    }
}

fn request_post_turn_resize_purge_replay_if_needed(app: &mut App) {
    if !app.chat_render.take_resize_purge_replay_after_turn() {
        return;
    }
    if matches!(
        app.terminal_lifecycle,
        super::super::TerminalLifecycleState::Running(super::super::SurfaceMode::Chat)
    ) {
        app.request_chat_purge_replay_rebuild(
            super::super::ChatPurgeReplayOptions::post_turn_resize(),
        );
    }
}

fn push_interrupted_hint(app: &mut App) {
    app.push_message_tracked(ChatMessage::new(
        MessageRole::System(Some(SystemSeverity::Info)),
        vec![MessageBlock::Text(TextBlock::from_complete(CONVERSATION_INTERRUPTED_HINT))],
        None,
    ));
    app.enforce_history_retention_tracked();
}

fn remove_empty_tail_assistant(app: &mut App, idx: Option<usize>) -> Option<usize> {
    let idx = idx?;
    let should_remove = app
        .transcript
        .messages
        .get(idx)
        .is_some_and(|msg| matches!(msg.role, MessageRole::Assistant) && msg.blocks.is_empty());
    if !should_remove {
        return None;
    }
    app.remove_message_tracked(idx)?;
    Some(idx)
}

fn mark_turn_exit_assistant_layout_dirty(app: &mut App, idx: Option<usize>) {
    let Some(idx) = idx else {
        return;
    };
    if app
        .transcript
        .messages
        .get(idx)
        .is_some_and(|msg| matches!(msg.role, MessageRole::Assistant))
    {
        app.invalidate_layout(InvalidationLevel::MessageChanged(idx));
    }
}

fn push_turn_error_message(app: &mut App, error: &str, class: TurnErrorClass) {
    let message = match class {
        TurnErrorClass::PlanLimit => {
            let summary = summarize_internal_error(error);
            format!(
                "Turn blocked by account or plan limits: {summary}\n\n{PLAN_LIMIT_NEXT_STEPS_HINT}"
            )
        }
        TurnErrorClass::AuthRequired => {
            format!("{AUTH_REQUIRED_NEXT_STEPS_HINT}\n\n{TURN_ERROR_INPUT_LOCK_HINT}")
        }
        TurnErrorClass::AccountAccess => {
            let summary = summarize_internal_error(error);
            format!(
                "The current account or organization cannot use the requested resource: {summary}\n\n{TURN_ERROR_INPUT_LOCK_HINT}"
            )
        }
        TurnErrorClass::ModelUnavailable => {
            let summary = summarize_internal_error(error);
            format!(
                "The selected model is unavailable: {summary}\n\nUse /model to choose another available model.\n\n{TURN_ERROR_INPUT_LOCK_HINT}"
            )
        }
        TurnErrorClass::TransientService => {
            let summary = summarize_internal_error(error);
            format!(
                "The service is temporarily overloaded or unavailable: {summary}\n\nWait a moment and retry.\n\n{TURN_ERROR_INPUT_LOCK_HINT}"
            )
        }
        TurnErrorClass::Internal | TurnErrorClass::Other => {
            format!("Turn failed: {error}\n\n{TURN_ERROR_INPUT_LOCK_HINT}")
        }
    };
    super::notices::emit_system_notice(app, SystemSeverity::Error, &message);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;

    fn pending_permission_context(
        app: &App,
        tool_id: &str,
    ) -> Option<crate::app::SubagentPermissionContext> {
        let (mi, bi) = app.lookup_tool_call(tool_id)?;
        let Some(MessageBlock::ToolCall(tool)) =
            app.transcript.messages.get(mi).and_then(|msg| msg.blocks.get(bi))
        else {
            return None;
        };
        tool.pending_permission.as_ref().and_then(|permission| permission.subagent_context.clone())
    }

    fn deliver(app: &mut App, event: crate::agent::events::ClientEvent) {
        crate::app::events::handle_client_event(app, event);
    }

    fn live_text(app: &mut App, text: &str) {
        crate::app::test_support::deliver_update(
            app,
            model::SessionUpdate::AgentMessageChunk(model::ContentChunk::new(
                model::ContentBlock::Text(model::TextContent::new(text)),
            )),
        );
    }

    fn started(app: &mut App, uuid: &str) {
        deliver(
            app,
            crate::agent::events::ClientEvent::UserMessageStarted {
                session_id: "session-1".into(),
                message_uuid: uuid.into(),
                source: crate::agent::types::UserMessageStartSource::StreamEvent,
            },
        );
    }

    fn assert_prompt(
        rx: &mut crate::agent::client::CommandReceiver,
        expected: &str,
        images: &[crate::app::clipboard_image::ImageAttachment],
    ) -> String {
        use crate::agent::wire::BridgeCommand;
        let mut prompt = None;
        while let Ok(envelope) = rx.try_recv() {
            match envelope.command {
                BridgeCommand::Prompt { session_id, message_uuid, chunks, inline_pastes } => {
                    assert!(prompt.is_none(), "one user action sends one prompt");
                    assert_eq!(session_id, "session-1");
                    assert!(inline_pastes.is_empty());
                    assert_eq!(
                        chunks
                            .iter()
                            .filter(|c| c.kind == "text")
                            .map(|c| &c.value)
                            .collect::<Vec<_>>(),
                        [&serde_json::json!(expected)]
                    );
                    let actual_images: Vec<_> =
                        chunks.iter().filter(|c| c.kind == "image").collect();
                    assert_eq!(actual_images.len(), images.len());
                    for (actual, expected) in actual_images.iter().zip(images) {
                        assert_eq!(actual.value["data"], expected.data);
                        assert_eq!(actual.value["mime_type"], expected.mime_type);
                    }
                    assert_eq!(chunks.len(), images.len() + 1);
                    prompt = Some(message_uuid);
                }
                BridgeCommand::GetContextUsage { session_id } => {
                    assert_eq!(session_id, "session-1");
                }
                other => panic!("unexpected command {other:?}"),
            }
        }
        prompt.expect("prompt delivered")
    }

    fn admit(
        app: &mut App,
        rx: &mut crate::agent::client::CommandReceiver,
        text: &str,
        images: &[crate::app::clipboard_image::ImageAttachment],
    ) -> String {
        app.input.set_text(text);
        app.input.image_payloads = images.to_vec();
        crate::app::input_submit::submit_input(app);
        assert_prompt(rx, text, images)
    }

    fn source_pairs(app: &App) -> Vec<(String, Vec<String>)> {
        app.transcript
            .messages
            .iter()
            .filter(|m| matches!(m.role, MessageRole::User))
            .flat_map(|m| &m.blocks)
            .filter_map(|b| match b {
                MessageBlock::Text(t) => Some((t.text.clone(), t.source_message_uuids.clone())),
                _ => None,
            })
            .collect()
    }

    fn response_source(app: &App, owner: usize) -> String {
        app.transcript.messages[owner]
            .blocks
            .iter()
            .filter_map(|b| match b {
                MessageBlock::Text(t) => Some(t.text.as_str()),
                _ => None,
            })
            .collect()
    }

    fn colored_image(red: u8) -> crate::app::clipboard_image::ImageAttachment {
        use base64::Engine as _;
        let pixels = image::RgbaImage::from_pixel(1, 1, image::Rgba([red, 30, 90, 255]));
        let mut png = std::io::Cursor::new(Vec::new());
        pixels.write_to(&mut png, image::ImageFormat::Png).unwrap();
        crate::app::clipboard_image::ImageAttachment {
            data: base64::engine::general_purpose::STANDARD.encode(png.into_inner()),
            mime_type: "image/png".into(),
        }
    }

    fn reject(app: &mut App, uuid: &str) {
        deliver(
            app,
            crate::agent::events::ClientEvent::UserMessageRejected {
                session_id: "session-1".into(),
                message_uuid: uuid.into(),
                reason: "fixture admission refused".into(),
            },
        );
    }

    #[test]
    fn correlated_start_commits_each_message_in_the_coalesced_prefix_once() {
        use crate::agent::events::ClientEvent;
        let (mut app, mut rx) = crate::app::test_support::app_with_connection();
        let first = admit(&mut app, &mut rx, "original question", &[]);
        live_text(&mut app, "Original reply");
        let original = app.active_turn_assistant_idx().unwrap();
        let sources =
            ["first queued line\ncontinued", "second queued source", "third unstarted source"];
        let ids: Vec<_> =
            sources.iter().map(|source| admit(&mut app, &mut rx, source, &[])).collect();
        let pending: Vec<_> = app
            .pending_user_messages
            .iter()
            .map(|p| (p.uuid.clone(), p.text.clone(), p.images.clone()))
            .collect();
        for uuid in [&ids[1], &ids[1], "unknown"] {
            deliver(
                &mut app,
                ClientEvent::UserMessageQueued {
                    session_id: "session-1".into(),
                    message_uuid: uuid.into(),
                },
            );
        }
        deliver(
            &mut app,
            ClientEvent::TurnInterruptReceipt {
                session_id: "session-1".into(),
                still_queued: vec![ids[1].clone(), "unknown".into()],
            },
        );
        started(&mut app, "unknown");
        assert_eq!(source_pairs(&app), [("original question".into(), vec![first.clone()])]);
        assert_eq!(
            app.pending_user_messages
                .iter()
                .map(|p| (p.uuid.clone(), p.text.clone(), p.images.clone()))
                .collect::<Vec<_>>(),
            pending
        );
        live_text(&mut app, " continues before queued start");
        assert_eq!(app.active_turn_assistant_idx(), Some(original));
        started(&mut app, &ids[1]);
        let owner = app.active_turn_assistant_idx().unwrap();
        assert_ne!(owner, original);
        let expected = vec![
            ("original question".into(), vec![first]),
            (sources[0].into(), vec![ids[0].clone()]),
            (sources[1].into(), vec![ids[1].clone()]),
        ];
        assert_eq!(source_pairs(&app), expected);
        assert_eq!(
            app.pending_user_messages
                .iter()
                .map(|p| (p.uuid.as_str(), p.text.as_str()))
                .collect::<Vec<_>>(),
            [(ids[2].as_str(), sources[2])]
        );
        for uuid in [&ids[0], &ids[1], "unknown"] {
            started(&mut app, uuid);
            assert_eq!(app.active_turn_assistant_idx(), Some(owner));
            assert_eq!(source_pairs(&app), expected);
        }
        live_text(&mut app, "Coalesced response belongs to second");
        assert_eq!(response_source(&app, owner), "Coalesced response belongs to second");
        assert_eq!(response_source(&app, original), "Original reply continues before queued start");
        started(&mut app, &ids[2]);
        let final_owner = app.active_turn_assistant_idx().unwrap();
        live_text(&mut app, "Third independent response");
        assert_ne!(final_owner, owner);
        assert_eq!(response_source(&app, final_owner), "Third independent response");
        assert_eq!(
            source_pairs(&app),
            expected
                .into_iter()
                .chain([(sources[2].into(), vec![ids[2].clone()])])
                .collect::<Vec<_>>()
        );
        assert!(app.pending_user_messages.is_empty());
        assert!(rx.try_recv().is_err());
    }

    fn reconnect_recovered_draft(
        app: &mut App,
        rx: &mut crate::agent::client::CommandReceiver,
        directory: &std::path::Path,
    ) {
        let (connection, receiver) = crate::agent::client::AgentConnection::test_channel();
        app.session_runtime.conn = Some(std::rc::Rc::new(connection));
        *rx = receiver;
        deliver(
            app,
            crate::agent::events::ClientEvent::Connected {
                session_id: "session-1".into(),
                cwd: directory.to_string_lossy().into_owned(),
                current_model: model::CurrentModel::new("model", "model", "model")
                    .authoritative(true),
                available_models: vec![],
                mode: None,
                fast_mode_state: model::FastModeState::Off,
                fast_mode_disabled_reason: None,
                ultracode: None,
                history_updates: vec![],
            },
        );
        while let Ok(envelope) = rx.try_recv() {
            assert!(matches!(envelope.command,
                crate::agent::wire::BridgeCommand::GetMcpSnapshot { session_id }
                | crate::agent::wire::BridgeCommand::GetStatusSnapshot { session_id }
                | crate::agent::wire::BridgeCommand::GetContextUsage { session_id }
                | crate::agent::wire::BridgeCommand::InspectSettings { session_id, .. }
                if session_id == "session-1"));
        }
    }

    #[test]
    fn rejected_message_restores_ordered_payload_before_newer_draft() {
        use crate::agent::events::ClientEvent;
        for cause in ["rejection", "authentication", "connection"] {
            let directory = tempfile::tempdir().unwrap();
            let (mut app, mut rx) = crate::app::test_support::app_with_connection();
            admit(&mut app, &mut rx, "main question", &[]);
            live_text(&mut app, "Main response");
            let original = app.active_turn_assistant_idx().unwrap();
            let old = colored_image(10);
            let other = colored_image(20);
            let draft = colored_image(30);
            let one = admit(&mut app, &mut rx, "old [Image #1]", std::slice::from_ref(&old));
            let two = admit(&mut app, &mut rx, "neighbor [Image #1]", std::slice::from_ref(&other));
            app.input.set_text("new \u{754c} [Image #1] trailing");
            app.input.image_payloads.push(draft.clone());
            let reset = || {
                if cause == "authentication" {
                    ClientEvent::AuthRequired {
                        method_name: "login".into(),
                        method_description: "fixture login needed".into(),
                    }
                } else {
                    ClientEvent::ConnectionFailed("fixture bridge exit".into())
                }
            };
            if cause == "rejection" {
                reject(&mut app, &one);
            } else {
                deliver(&mut app, reset());
                if cause == "connection" {
                    assert!(app.transcript.messages.iter().any(|message| {
                        matches!(message.role, MessageRole::System(_))
                            && message.blocks.iter().any(|block| matches!(block,
                                MessageBlock::Text(text) if text.text.contains("fixture bridge exit")))
                    }));
                }
            }
            let (expected, images) = if cause == "rejection" {
                ("old [Image #1]\nnew \u{754c} [Image #2] trailing", vec![old, draft])
            } else {
                (
                    "old [Image #1]\nneighbor [Image #2]\nnew \u{754c} [Image #3] trailing",
                    vec![old, other, draft],
                )
            };
            assert_eq!(app.input.text(), expected);
            assert_eq!(app.input.images(), images);
            if cause == "rejection" {
                assert_eq!(
                    app.pending_user_messages
                        .iter()
                        .map(|p| (p.uuid.as_str(), p.text.as_str()))
                        .collect::<Vec<_>>(),
                    [(two.as_str(), "neighbor [Image #1]")]
                );
                reject(&mut app, "unknown");
                reject(&mut app, &one);
                assert!(app.transcript.messages[original].blocks.iter().any(|b|matches!(b,MessageBlock::Notice(n) if n.text.text.contains("fixture admission refused"))));
                started(&mut app, &two);
                live_text(&mut app, "Neighbor still answers");
                assert_eq!(
                    response_source(&app, app.active_turn_assistant_idx().unwrap()),
                    "Neighbor still answers"
                );
            } else {
                assert!(app.pending_user_messages.is_empty());
                deliver(&mut app, reset());
                crate::app::input_submit::submit_input(&mut app);
                assert!(rx.try_recv().is_err(), "reset is not a usable connection");
                // A new successful connection is required before the recovered composer can be sent.
                reconnect_recovered_draft(&mut app, &mut rx, directory.path());
            }
            assert_eq!(app.input.text(), expected);
            assert_eq!(app.input.images(), images);
            crate::app::test_support::press(&mut app, crossterm::event::KeyCode::Char('!'));
            assert_eq!(app.input.text(), format!("{expected}!"));
            crate::app::input_submit::submit_input(&mut app);
            assert_prompt(&mut rx, &format!("{expected}!"), &images);
            assert!(app.input.is_empty());
        }
    }

    fn event_tool(id: &str, title: &str, name: &str, parent: Option<&str>) -> model::ToolCall {
        model::ToolCall::new(id, title)
            .status(model::ToolCallStatus::InProgress)
            .meta(serde_json::json!({"claudeCode":{"toolName":name,"parentToolUseId":parent}}))
    }

    fn owned_permission(
        app: &mut App,
        id: &str,
    ) -> tokio::sync::oneshot::Receiver<model::RequestPermissionResponse> {
        crate::app::test_support::request_permission(
            app,
            model::RequestPermissionRequest::new(
                "session-1",
                model::ToolCallUpdate::new(id, model::ToolCallUpdateFields::new()),
                crate::app::test_support::permission_options(id),
                Some(
                    model::PermissionDisplay::new()
                        .title(Some("Command approval".into()))
                        .display_name(Some("Build command".into()))
                        .description(Some("Writes build output".into())),
                ),
            ),
        )
    }

    #[test]
    fn admitted_child_permissions_preserve_context_diagnostics_and_callback_identity() {
        use crate::app::test_support::{
            admit_tool, app_with_connection, assert_pending, assert_permission_response, press,
        };
        use crossterm::event::KeyCode;
        for (parent_input, parent_title, present, child, expected_label) in [
            (
                serde_json::json!({"name":" reviewer ","subagent_type":"tester","model":"parent-model"}),
                "Parent title",
                true,
                true,
                "reviewer",
            ),
            (
                serde_json::json!({"name":" ","subagent_type":"tester","model":"parent-model"}),
                "Parent title",
                true,
                true,
                "tester",
            ),
            (serde_json::json!({}), "Parent title", true, true, "Parent title"),
            (serde_json::json!({}), "  ", true, true, "Subagent"),
            (serde_json::json!({}), "Absent parent", false, true, "Subagent"),
            (serde_json::json!({"name":"unrelated"}), "Parent title", true, false, ""),
        ] {
            let (mut app, mut commands) = app_with_connection();
            app.status = AppStatus::Running;
            if present {
                admit_tool(
                    &mut app,
                    event_tool("parent", parent_title, "Agent", None)
                        .raw_input(parent_input.clone()),
                );
            }
            admit_tool(
                &mut app,
                event_tool("target", "Child build", "Bash", child.then_some("parent")).raw_input(
                    serde_json::json!({"command":"child-build","model":"not-parent-model"}),
                ),
            );
            admit_tool(&mut app, event_tool("neighbor", "Neighbor read", "Read", None));
            let mut receiver = None;
            let records = crate::logging::test_capture::capture(|| {
                receiver = Some(owned_permission(&mut app, "target"));
            });
            let mut receiver = receiver.expect("request admitted");
            let mut neighbor = owned_permission(&mut app, "neighbor");
            let context = pending_permission_context(&app, "target");
            if child {
                let context = context.expect("child context");
                assert_eq!(context.subagent_label, expected_label);
                assert_eq!(context.child_tool_name, "Bash");
                assert_eq!(context.child_tool_title, "Child build");
                assert_eq!(context.parent_tool_call_id, "parent");
                assert_eq!(
                    context.parent_tool_title.as_deref(),
                    (present && !parent_title.trim().is_empty()).then_some(parent_title)
                );
                assert_eq!(
                    context.parent_model.as_deref(),
                    parent_input.get("model").and_then(serde_json::Value::as_str)
                );
                assert_eq!(context.parent_raw_input, present.then_some(parent_input));
            } else {
                assert!(context.is_none());
            }
            let record = records
                .iter()
                .find(|record| record["event_name"] == "permission_request_applied")
                .expect("real admission diagnostic");
            assert_eq!(record["session_id"], "session-1");
            assert_eq!(record["tool_call_id"], "target");
            assert_eq!(record["subagent_label"], expected_label);
            assert_eq!(
                record["subagent_parent_model"],
                if child && matches!(expected_label, "reviewer" | "tester") {
                    "parent-model"
                } else {
                    ""
                }
            );
            assert_eq!(record["subagent_child_tool_name"], if child { "Bash" } else { "" });
            assert_eq!(record["subagent_child_tool_title"], if child { "Child build" } else { "" });
            assert_eq!(record["subagent_parent_tool_call_id"], if child { "parent" } else { "" });
            assert_eq!(record["permission_display_title"], "Command approval");
            assert_eq!(record["permission_display_name"], "Build command");
            assert_eq!(record["permission_display_description"], "Writes build output");
            press(&mut app, KeyCode::Right);
            press(&mut app, KeyCode::Enter);
            assert_permission_response(&mut receiver, "target-always");
            assert_pending(&mut neighbor);
            if present {
                let tool = indexed_tool_call(&app, "parent").expect("parent retained");
                assert_eq!(tool.status, model::ToolCallStatus::InProgress);
                assert!(tool.pending_permission.is_none());
            }
            press(&mut app, KeyCode::Enter);
            assert_permission_response(&mut neighbor, "neighbor-once");
            assert!(commands.try_recv().is_err());
        }
    }

    #[test]
    fn nested_background_permission_remains_answerable_after_real_foreground_completion() {
        use crate::app::test_support::{
            admit_tool, app_with_connection, assert_permission_response, deliver_update, press,
        };
        let (mut app, mut commands) = app_with_connection();
        app.status = AppStatus::Running;
        deliver_update(
            &mut app,
            model::SessionUpdate::AgentMessageChunk(model::ContentChunk::new(
                model::ContentBlock::Text(model::TextContent::new("foreground answer")),
            )),
        );
        admit_tool(&mut app, event_tool("foreground", "Foreground read", "Read", None));
        admit_tool(
            &mut app,
            event_tool("outer", "Outer agent", "Agent", None)
                .status(model::ToolCallStatus::Detached)
                .raw_input(serde_json::json!({"name":"outer-name"})),
        );
        admit_tool(
            &mut app,
            event_tool("nested", "Nested agent", "Agent", Some("outer"))
                .status(model::ToolCallStatus::Detached)
                .raw_input(serde_json::json!({"name":"nearest-name","model":"nested-model"})),
        );
        admit_tool(&mut app, event_tool("nested-command", "Nested build", "Bash", Some("nested")));
        crate::app::handle_client_event(
            &mut app,
            crate::agent::events::ClientEvent::TurnComplete {
                session_id: "session-1".into(),
                queued_turn_count: None,
                terminal_reason: None,
            },
        );
        assert_eq!(app.status, AppStatus::Ready);
        assert_eq!(
            indexed_tool_call(&app, "foreground").expect("foreground").status,
            model::ToolCallStatus::Completed
        );
        for id in ["outer", "nested"] {
            assert_eq!(
                indexed_tool_call(&app, id).expect("background agent").status,
                model::ToolCallStatus::Detached
            );
        }
        assert_eq!(
            indexed_tool_call(&app, "nested-command").expect("child").status,
            model::ToolCallStatus::InProgress
        );
        assert!(app.transcript.messages.iter().flat_map(|message| &message.blocks).any(
            |block| matches!(block, MessageBlock::Text(text) if text.text == "foreground answer")
        ));
        while let Ok(envelope) = commands.try_recv() {
            assert!(
                matches!(envelope.command, crate::agent::wire::BridgeCommand::GetContextUsage { session_id } if session_id == "session-1")
            );
        }
        let mut receiver = owned_permission(&mut app, "nested-command");
        let context = pending_permission_context(&app, "nested-command").expect("nearest context");
        assert_eq!(context.parent_tool_call_id, "nested");
        assert_eq!(context.subagent_label, "nearest-name");
        assert_eq!(context.parent_model.as_deref(), Some("nested-model"));
        assert_eq!(context.child_tool_name, "Bash");
        press(&mut app, crossterm::event::KeyCode::Enter);
        assert_permission_response(&mut receiver, "nested-command-once");
        assert!(commands.try_recv().is_err());
    }

    #[test]
    fn recovered_tenth_image_preserves_cursor_editing_and_exact_attachment_order() {
        let (mut app, mut rx) = crate::app::test_support::app_with_connection();
        admit(&mut app, &mut rx, "main question", &[]);
        live_text(&mut app, "Working reply");
        let mut images: Vec<_> = (1..=9).map(colored_image).collect();
        let source = "queued [Image #1] [Image #2] [Image #3] [Image #4] [Image #5] [Image #6] [Image #7] [Image #8] [Image #9]";
        let uuid = admit(&mut app, &mut rx, source, &images);
        let tenth = colored_image(10);
        images.push(tenth.clone());
        app.input.set_text("new \u{754c}\u{1f980} [Image #1] trailing");
        app.input.image_payloads.push(tenth);
        reject(&mut app, &uuid);
        let restored = format!("{source}\nnew \u{754c}\u{1f980} [Image #10] trailing");
        assert_eq!(app.input.text(), restored);
        assert_eq!(app.input.images(), images);
        crate::app::test_support::press(&mut app, crossterm::event::KeyCode::Char('!'));
        assert_eq!(app.input.text(), format!("{restored}!"));
        crate::app::input_submit::submit_input(&mut app);
        assert_prompt(&mut rx, &format!("{restored}!"), &images);
        assert!(app.input.is_empty());
    }
}
