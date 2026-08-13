// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use super::super::{
    App, AppStatus, ChatMessage, FocusTarget, InlinePermission, InlineQuestion, InvalidationLevel,
    MessageBlock, MessageRole, NoticeStage, SubagentPermissionContext, SystemSeverity, TextBlock,
    ToolCallInfo, ToolCallScope, UserDialogBlock,
};
use super::compaction;
use super::rate_limit::{format_rate_limit_summary, rate_limit_notice_key};
use crate::agent::error_handling::{TurnErrorClass, classify_turn_error, summarize_internal_error};
use crate::agent::model;
use std::collections::BTreeSet;

const CONVERSATION_INTERRUPTED_HINT: &str =
    "Conversation interrupted. Tell the model how to proceed.";
const TURN_ERROR_INPUT_LOCK_HINT: &str =
    "Input disabled after an error. Press Ctrl+Q to quit and try again.";
const PLAN_LIMIT_NEXT_STEPS_HINT: &str = "Next steps:\n\
1. Wait a few minutes and retry.\n\
2. Reduce request size or request frequency.\n\
3. Check quota/billing for your account or switch plans.";
const AUTH_REQUIRED_NEXT_STEPS_HINT: &str = "Authentication required. Type /login to authenticate, or run `claude auth login` in a terminal.";

#[derive(Clone, Copy)]
struct TurnExitState {
    tail_assistant_idx: Option<usize>,
    turn_was_active: bool,
    cancel_requested: bool,
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

    if app.turn.pending_interaction_ids.iter().any(|id| id == &tool_id) {
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
    let auto_focus =
        app.turn.pending_interaction_ids.is_empty() && !app.has_draft_input_for_focus();
    if let Some(MessageBlock::ToolCall(tc)) =
        app.transcript.messages.get_mut(mi).and_then(|m| m.blocks.get_mut(bi))
    {
        let tc = tc.as_mut();
        tc.pending_permission = Some(InlinePermission {
            options: request.options,
            display: request.display,
            subagent_context: permission_context.clone(),
            response_tx,
            selected_index: 0,
            focused: auto_focus,
        });
        tc.invalidate_render_cache();
        layout_dirty = true;
        app.turn.pending_interaction_ids.push(tool_id.clone());
        if auto_focus {
            app.claim_focus_target(FocusTarget::Permission);
        }
        app.notifications.notify(
            app.config.preferred_notification_channel_effective(),
            super::super::notify::NotifyEvent::PermissionRequired,
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
        app.sync_render_cache_slot(mi, bi);
        app.recompute_message_retained_bytes(mi);
        app.invalidate_layout(InvalidationLevel::MessageChanged(mi));
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

    if app.turn.pending_interaction_ids.iter().any(|id| id == &tool_id) {
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
    let auto_focus =
        app.turn.pending_interaction_ids.is_empty() && !app.has_draft_input_for_focus();
    if let Some(MessageBlock::ToolCall(tc)) =
        app.transcript.messages.get_mut(mi).and_then(|m| m.blocks.get_mut(bi))
    {
        let tc = tc.as_mut();
        tc.pending_question = Some(InlineQuestion {
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
        app.turn.pending_interaction_ids.push(tool_id.clone());
        if auto_focus {
            app.claim_focus_target(FocusTarget::Permission);
        }
        app.notifications.notify(
            app.config.preferred_notification_channel_effective(),
            super::super::notify::NotifyEvent::QuestionRequired,
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
        app.sync_render_cache_slot(mi, bi);
        app.recompute_message_retained_bytes(mi);
        app.invalidate_layout(InvalidationLevel::MessageChanged(mi));
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

    if app.turn.pending_interaction_ids.iter().any(|id| id == &request_id) {
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

    let auto_focus =
        app.turn.pending_interaction_ids.is_empty() && !app.has_draft_input_for_focus();
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
    app.turn.pending_interaction_ids.push(request_id.clone());
    if auto_focus {
        app.claim_focus_target(FocusTarget::Permission);
    }
    app.notifications.notify(
        app.config.preferred_notification_channel_effective(),
        super::super::notify::NotifyEvent::QuestionRequired,
    );
    app.sync_render_cache_slot(mi, bi);
    app.recompute_message_retained_bytes(mi);
    app.invalidate_layout(InvalidationLevel::MessageChanged(mi));
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
    compaction::finish_inferred(app, emit_manual_compaction_success);
    app.turn.cancel_requested = false;
    state
}

fn finish_ready_turn_exit(app: &mut App, exit: TurnExitState, tool_status: model::ToolCallStatus) {
    app.finalize_turn_runtime_artifacts(tool_status);
    app.status = AppStatus::Ready;
    app.files_accessed = 0;
    app.sdk_inventory.clear_rewind_targets();
    app.sync_git_context();

    let removed_tail_assistant = remove_empty_tail_assistant(app, exit.tail_assistant_idx);
    if exit.cancel_requested {
        push_interrupted_hint(app);
    }
    if removed_tail_assistant.is_none() && (exit.turn_was_active || exit.cancel_requested) {
        mark_turn_exit_assistant_layout_dirty(app, exit.tail_assistant_idx);
    }
    app.turn.reset_for_turn_exit();
}

pub(super) fn handle_turn_complete_event(
    app: &mut App,
    terminal_reason: Option<crate::agent::types::TerminalReason>,
) {
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
    if turn_was_active {
        app.notifications.notify(
            app.config.preferred_notification_channel_effective(),
            super::super::notify::NotifyEvent::TurnComplete,
        );
    }
}

pub(super) fn handle_turn_error_event(
    app: &mut App,
    msg: &str,
    classified: Option<TurnErrorClass>,
    api_error_status: Option<u16>,
    terminal_reason: Option<crate::agent::types::TerminalReason>,
) {
    let exit = begin_turn_exit(app, false);

    if exit.cancel_requested {
        let summary = summarize_internal_error(msg);
        tracing::warn!(
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
    app.input.clear();
    app.pending_submit = None;
    app.status = AppStatus::Error;
    let rate_limit_context = if matches!(error_class, TurnErrorClass::PlanLimit) {
        app.session_runtime
            .last_rate_limit_update
            .clone()
            .filter(|update| !matches!(update.status, model::RateLimitStatus::Allowed))
    } else {
        None
    };
    let removed_tail_assistant = remove_empty_tail_assistant(app, exit.tail_assistant_idx);
    push_turn_error_message(app, msg, error_class, rate_limit_context.as_ref());
    if removed_tail_assistant.is_none() && exit.turn_was_active {
        mark_turn_exit_assistant_layout_dirty(app, exit.tail_assistant_idx);
    }
    app.turn.reset_for_turn_exit();
    request_post_turn_resize_purge_replay_if_needed(app);
    crate::app::session_runtime::request_context_usage_refresh(app);
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

fn push_turn_error_message(
    app: &mut App,
    error: &str,
    class: TurnErrorClass,
    rate_limit_context: Option<&model::RateLimitUpdate>,
) {
    match class {
        TurnErrorClass::PlanLimit => {
            let base_message = {
                let summary = summarize_internal_error(error);
                format!(
                    "Turn blocked by account or plan limits: {summary}\n\n{PLAN_LIMIT_NEXT_STEPS_HINT}\n\n{TURN_ERROR_INPUT_LOCK_HINT}"
                )
            };
            let (severity, message, dedup_key) = if let Some(update) = rate_limit_context {
                let prefix = format_rate_limit_summary(update);
                let severity = match update.status {
                    model::RateLimitStatus::AllowedWarning => SystemSeverity::Warning,
                    model::RateLimitStatus::Rejected | model::RateLimitStatus::Allowed => {
                        SystemSeverity::Error
                    }
                };
                (severity, format!("{prefix}\n\n{base_message}"), rate_limit_notice_key(update))
            } else {
                (
                    SystemSeverity::Error,
                    base_message,
                    super::super::NoticeDedupKey::RateLimit(super::super::RateLimitIncidentKey {
                        rate_limit_type: None,
                        resets_at_bucket: None,
                    }),
                )
            };
            super::notices::upsert_turn_notice(
                app,
                dedup_key,
                NoticeStage::PlanLimitTurnError,
                severity,
                &message,
            );
        }
        TurnErrorClass::AuthRequired => {
            let message =
                format!("{AUTH_REQUIRED_NEXT_STEPS_HINT}\n\n{TURN_ERROR_INPUT_LOCK_HINT}");
            super::push_system_message_with_severity(app, None, &message);
        }
        TurnErrorClass::AccountAccess => {
            let summary = summarize_internal_error(error);
            let message = format!(
                "The current account or organization cannot use the requested resource: {summary}\n\n{TURN_ERROR_INPUT_LOCK_HINT}"
            );
            super::push_system_message_with_severity(app, None, &message);
        }
        TurnErrorClass::ModelUnavailable => {
            let summary = summarize_internal_error(error);
            let message = format!(
                "The selected model is unavailable: {summary}\n\nUse /model to choose another available model.\n\n{TURN_ERROR_INPUT_LOCK_HINT}"
            );
            super::push_system_message_with_severity(app, None, &message);
        }
        TurnErrorClass::TransientService => {
            let summary = summarize_internal_error(error);
            let message = format!(
                "The service is temporarily overloaded or unavailable: {summary}\n\nWait a moment and retry.\n\n{TURN_ERROR_INPUT_LOCK_HINT}"
            );
            super::push_system_message_with_severity(app, None, &message);
        }
        TurnErrorClass::Internal | TurnErrorClass::Other => {
            let message = format!("Turn failed: {error}\n\n{TURN_ERROR_INPUT_LOCK_HINT}");
            super::push_system_message_with_severity(app, None, &message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{
        App, ChatPurgeReplayOptions, ChatRebuildKind, SurfaceMode, TerminalLifecycleState,
    };

    fn empty_assistant_message() -> ChatMessage {
        ChatMessage::new(MessageRole::Assistant, Vec::new(), None)
    }

    fn user_message(text: &str) -> ChatMessage {
        ChatMessage::new(
            MessageRole::User,
            vec![MessageBlock::Text(TextBlock::from_complete(text))],
            None,
        )
    }

    fn tool_call_info(
        id: &str,
        title: &str,
        sdk_tool_name: &str,
        raw_input: Option<serde_json::Value>,
        hidden: bool,
    ) -> crate::app::ToolCallInfo {
        crate::app::ToolCallInfo {
            id: id.to_owned(),
            source_message_uuids: Vec::new(),
            title: title.to_owned(),
            sdk_tool_name: sdk_tool_name.to_owned(),
            raw_input,
            raw_input_bytes: 0,
            locations: Vec::new(),
            output_metadata: None,
            task_metadata: None,
            status: model::ToolCallStatus::InProgress,
            content: Vec::new(),
            hidden,
            terminal_id: None,
            terminal_command: None,
            terminal_output: None,
            terminal_output_len: 0,
            cache: crate::app::BlockCache::default(),
            pending_permission: None,
            pending_question: None,
        }
    }

    fn push_tool(app: &mut App, tool: crate::app::ToolCallInfo) {
        let msg_idx = app.transcript.messages.len();
        let tool_id = tool.id.clone();
        app.transcript.messages.push(ChatMessage::new(
            MessageRole::Assistant,
            vec![MessageBlock::ToolCall(Box::new(tool))],
            None,
        ));
        app.index_tool_call(tool_id, msg_idx, 0);
    }

    fn permission_request(tool_id: &str) -> model::RequestPermissionRequest {
        model::RequestPermissionRequest::new(
            "session-1",
            model::ToolCallUpdate::new(tool_id, model::ToolCallUpdateFields::new()),
            vec![model::PermissionOption::new(
                "allow",
                "Allow",
                model::PermissionOptionKind::AllowOnce,
            )],
            None,
        )
    }

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

    #[derive(Clone)]
    struct SharedLogWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    struct LogWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for SharedLogWriter {
        type Writer = LogWriter;

        fn make_writer(&'a self) -> Self::Writer {
            LogWriter(std::sync::Arc::clone(&self.0))
        }
    }

    impl std::io::Write for LogWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("log buffer lock").extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn turn_complete_removes_empty_tail_assistant() {
        let mut app = App::test_default();
        app.status = AppStatus::Thinking;
        app.transcript.messages.push(user_message("hello"));
        app.transcript.messages.push(empty_assistant_message());

        handle_turn_complete_event(&mut app, None);

        assert_eq!(app.transcript.messages.len(), 1);
        assert!(matches!(app.transcript.messages[0].role, MessageRole::User));
    }

    #[test]
    fn cancelled_turn_error_removes_empty_tail_assistant_before_hint() {
        let mut app = App::test_default();
        app.status = AppStatus::Thinking;
        app.turn.cancel_requested = true;
        app.transcript.messages.push(user_message("hello"));
        app.transcript.messages.push(empty_assistant_message());

        handle_turn_error_event(&mut app, "cancelled", None, None, None);

        assert_eq!(app.transcript.messages.len(), 2);
        assert!(matches!(app.transcript.messages[0].role, MessageRole::User));
        assert!(matches!(
            app.transcript.messages[1].role,
            MessageRole::System(Some(SystemSeverity::Info))
        ));
    }

    #[test]
    fn turn_error_removes_empty_tail_assistant_before_error_message() {
        let mut app = App::test_default();
        app.status = AppStatus::Thinking;
        app.transcript.messages.push(user_message("hello"));
        app.transcript.messages.push(empty_assistant_message());

        handle_turn_error_event(&mut app, "boom", None, None, None);

        assert_eq!(app.transcript.messages.len(), 2);
        assert!(matches!(app.transcript.messages[0].role, MessageRole::User));
        assert!(matches!(app.transcript.messages[1].role, MessageRole::System(None)));
    }

    #[test]
    fn turn_complete_keeps_canonical_assistant_and_clears_active_owner() {
        let mut app = App::test_default();
        app.status = AppStatus::Thinking;
        app.transcript.messages.push(user_message("hello"));
        app.transcript.messages.push(ChatMessage::new(
            MessageRole::Assistant,
            vec![MessageBlock::Text(TextBlock::from_complete("done"))],
            None,
        ));
        app.bind_active_turn_assistant(1);

        handle_turn_complete_event(&mut app, None);

        assert_eq!(app.status, AppStatus::Ready);
        assert_eq!(app.active_turn_assistant_idx(), None);
        assert_eq!(app.transcript.messages.len(), 2);
        let Some(MessageBlock::Text(text)) = app.transcript.messages[1].blocks.first() else {
            panic!("expected assistant text block");
        };
        assert_eq!(text.text, "done");
    }

    #[test]
    fn turn_complete_runs_final_resize_purge_replay_after_stream_time_resize() {
        let mut app = App::test_default();
        app.surface_dirty = crate::app::SurfaceDirtyState::default();
        app.terminal_lifecycle = TerminalLifecycleState::Running(SurfaceMode::Chat);
        app.status = AppStatus::Running;
        app.transcript.messages.push(user_message("hello"));
        app.transcript.messages.push(ChatMessage::new(
            MessageRole::Assistant,
            vec![MessageBlock::Text(TextBlock::from_complete("streamed"))],
            None,
        ));
        app.bind_active_turn_assistant(1);
        app.chat_render.mark_resize_purge_replay_during_turn();

        handle_turn_complete_event(&mut app, None);

        assert_eq!(
            app.surface_dirty.chat.rebuild,
            ChatRebuildKind::PurgeReplay(ChatPurgeReplayOptions::post_turn_resize())
        );
        assert!(app.surface_dirty.chat.repaint);
        assert!(!app.chat_render.resize_purge_replay_after_turn);
    }

    #[test]
    fn permission_request_marks_canonical_tool_pending_permission() {
        let mut app = App::test_default();
        let tool_id = "bash-1";
        app.transcript.messages.push(ChatMessage::new(
            MessageRole::Assistant,
            vec![MessageBlock::ToolCall(Box::new(crate::app::ToolCallInfo {
                id: tool_id.to_owned(),
                source_message_uuids: Vec::new(),
                title: "tool".to_owned(),
                sdk_tool_name: "Bash".to_owned(),
                raw_input: None,
                raw_input_bytes: 0,
                locations: Vec::new(),
                output_metadata: None,
                task_metadata: None,
                status: model::ToolCallStatus::InProgress,
                content: Vec::new(),
                hidden: false,
                terminal_id: None,
                terminal_command: None,
                terminal_output: None,
                terminal_output_len: 0,
                cache: crate::app::BlockCache::default(),
                pending_permission: None,
                pending_question: None,
            }))],
            None,
        ));
        app.bind_active_turn_assistant(0);
        app.index_tool_call(tool_id.to_owned(), 0, 0);
        let (tx, _rx) = tokio::sync::oneshot::channel();
        let request = model::RequestPermissionRequest::new(
            "session-1",
            model::ToolCallUpdate::new(tool_id, model::ToolCallUpdateFields::new()),
            vec![model::PermissionOption::new(
                "allow",
                "Allow",
                model::PermissionOptionKind::AllowOnce,
            )],
            None,
        );

        handle_permission_request_event(&mut app, request, tx);

        let Some(MessageBlock::ToolCall(tool)) = app.transcript.messages[0].blocks.first() else {
            panic!("expected tool call block");
        };
        assert!(tool.pending_permission.is_some());
    }

    #[test]
    fn permission_request_applied_log_includes_subagent_and_display_fields() {
        let buffer = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::fmt()
            .json()
            .with_writer(SharedLogWriter(std::sync::Arc::clone(&buffer)))
            .finish();
        let context = SubagentPermissionContext {
            subagent_label: "reviewer".to_owned(),
            child_tool_name: "Bash".to_owned(),
            child_tool_title: "Bash".to_owned(),
            parent_tool_call_id: "agent-1".to_owned(),
            parent_tool_title: Some("Agent: reviewer".to_owned()),
            parent_model: Some("claude-opus-4-8".to_owned()),
            parent_raw_input: None,
        };

        tracing::subscriber::with_default(subscriber, || {
            log_permission_request_applied(
                "session-1",
                "bash-1",
                2,
                true,
                Some(&context),
                PermissionDisplayLogFields {
                    title: "Allow command?",
                    name: "Bash",
                    description: "Runs a command",
                },
            );
        });

        let output =
            String::from_utf8(buffer.lock().expect("log buffer lock").clone()).expect("utf8 log");
        assert!(output.contains(r#""subagent_label":"reviewer""#));
        assert!(output.contains(r#""subagent_parent_tool_call_id":"agent-1""#));
        assert!(output.contains(r#""subagent_parent_model":"claude-opus-4-8""#));
        assert!(output.contains(r#""subagent_child_tool_name":"Bash""#));
        assert!(output.contains(r#""permission_display_title":"Allow command?""#));
        assert!(output.contains(r#""permission_display_name":"Bash""#));
        assert!(output.contains(r#""permission_display_description":"Runs a command""#));
    }

    #[test]
    fn main_agent_permission_gets_no_subagent_context() {
        let mut app = App::test_default();
        push_tool(&mut app, tool_call_info("bash-1", "Bash", "Bash", None, false));
        app.register_tool_call_scope("bash-1".to_owned(), ToolCallScope::MainAgent);
        let (tx, _rx) = tokio::sync::oneshot::channel();

        handle_permission_request_event(&mut app, permission_request("bash-1"), tx);

        assert!(pending_permission_context(&app, "bash-1").is_none());
    }

    #[test]
    fn subagent_child_permission_resolves_parent_and_child_context() {
        let mut app = App::test_default();
        push_tool(
            &mut app,
            tool_call_info(
                "agent-1",
                "Agent: reviewer",
                "Agent",
                Some(serde_json::json!({
                    "name": "reviewer",
                    "subagent_type": "general-purpose",
                    "model": "claude-opus-4-8"
                })),
                false,
            ),
        );
        push_tool(&mut app, tool_call_info("bash-1", "Bash", "Bash", None, true));
        app.register_tool_call_scope("agent-1".to_owned(), ToolCallScope::SubagentRoot);
        app.register_tool_call_scope(
            "bash-1".to_owned(),
            ToolCallScope::SubagentChild { parent_tool_use_id: "agent-1".to_owned() },
        );
        let (tx, _rx) = tokio::sync::oneshot::channel();

        handle_permission_request_event(&mut app, permission_request("bash-1"), tx);

        let context = pending_permission_context(&app, "bash-1").expect("subagent context");
        assert_eq!(context.subagent_label, "reviewer");
        assert_eq!(context.child_tool_name, "Bash");
        assert_eq!(context.child_tool_title, "Bash");
        assert_eq!(context.parent_tool_call_id, "agent-1");
        assert_eq!(context.parent_tool_title.as_deref(), Some("Agent: reviewer"));
        assert_eq!(context.parent_model.as_deref(), Some("claude-opus-4-8"));
        assert!(context.parent_raw_input.is_some());
    }

    #[test]
    fn subagent_child_permission_missing_parent_falls_back_cleanly() {
        let mut app = App::test_default();
        push_tool(&mut app, tool_call_info("bash-1", "Bash", "Bash", None, true));
        app.register_tool_call_scope(
            "bash-1".to_owned(),
            ToolCallScope::SubagentChild { parent_tool_use_id: "missing-parent".to_owned() },
        );
        let (tx, _rx) = tokio::sync::oneshot::channel();

        handle_permission_request_event(&mut app, permission_request("bash-1"), tx);

        let context = pending_permission_context(&app, "bash-1").expect("subagent context");
        assert_eq!(context.subagent_label, "Subagent");
        assert_eq!(context.parent_tool_call_id, "missing-parent");
        assert_eq!(context.parent_tool_title, None);
        assert_eq!(context.parent_model, None);
        assert_eq!(context.parent_raw_input, None);
    }

    #[test]
    fn subagent_label_prefers_name_over_subagent_type() {
        let mut app = App::test_default();
        push_tool(
            &mut app,
            tool_call_info(
                "agent-1",
                "Agent: reviewer",
                "Agent",
                Some(serde_json::json!({
                    "name": "reviewer",
                    "subagent_type": "tester"
                })),
                false,
            ),
        );
        push_tool(&mut app, tool_call_info("bash-1", "Bash", "Bash", None, true));
        app.register_tool_call_scope(
            "bash-1".to_owned(),
            ToolCallScope::SubagentChild { parent_tool_use_id: "agent-1".to_owned() },
        );
        let (tx, _rx) = tokio::sync::oneshot::channel();

        handle_permission_request_event(&mut app, permission_request("bash-1"), tx);

        let context = pending_permission_context(&app, "bash-1").expect("subagent context");
        assert_eq!(context.subagent_label, "reviewer");
    }
}
