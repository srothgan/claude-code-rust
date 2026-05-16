// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::super::{
    App, AppStatus, CancelOrigin, ChatMessage, FocusTarget, InlinePermission, InlineQuestion,
    InvalidationLevel, MessageBlock, MessageRole, NoticeStage, SystemSeverity, TextBlock,
};
use super::clear_compaction_state;
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
    cancelled_requested: Option<CancelOrigin>,
    show_interrupted_hint: bool,
}

pub(super) fn handle_permission_request_event(
    app: &mut App,
    request: model::RequestPermissionRequest,
    response_tx: tokio::sync::oneshot::Sender<model::RequestPermissionResponse>,
) {
    let session_id = request.session_id.to_string();
    let tool_id = request.tool_call.tool_call_id.clone();
    let options = request.options.clone();

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
    if let Some(MessageBlock::ToolCall(tc)) =
        app.messages.get_mut(mi).and_then(|m| m.blocks.get_mut(bi))
    {
        let tc = tc.as_mut();
        tc.pending_permission = Some(InlinePermission {
            options: request.options,
            display: request.display,
            response_tx,
            selected_index: 0,
            focused: auto_focus,
        });
        tc.invalidate_render_cache();
        layout_dirty = true;
        app.pending_interaction_ids.push(tool_id.clone());
        if auto_focus {
            app.claim_focus_target(FocusTarget::Permission);
        }
        app.notifications.notify(
            app.config.preferred_notification_channel_effective(),
            super::super::notify::NotifyEvent::PermissionRequired,
        );
        tracing::info!(
            target: crate::logging::targets::APP_PERMISSION,
            event_name = "permission_request_applied",
            message = "permission request applied to inline tool call",
            outcome = "success",
            session_id = %session_id,
            tool_call_id = %tool_id,
            option_count = options.len(),
            focused = auto_focus,
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
    if let Some(MessageBlock::ToolCall(tc)) =
        app.messages.get_mut(mi).and_then(|m| m.blocks.get_mut(bi))
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
        app.pending_interaction_ids.push(tool_id.clone());
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
    if app.pending_cancel_origin.is_none() {
        app.pending_cancel_origin = Some(CancelOrigin::Manual);
    }
    app.cancelled_turn_pending_hint =
        matches!(app.pending_cancel_origin, Some(CancelOrigin::Manual));
    let _ = app.finalize_in_progress_tool_calls(model::ToolCallStatus::Failed);
}

fn begin_turn_exit(app: &mut App, emit_manual_compaction_success: bool) -> TurnExitState {
    let state = TurnExitState {
        tail_assistant_idx: app
            .messages
            .iter()
            .rposition(|m| matches!(m.role, MessageRole::Assistant)),
        turn_was_active: matches!(app.status, AppStatus::Thinking | AppStatus::Running),
        cancelled_requested: app.pending_cancel_origin,
        show_interrupted_hint: matches!(app.pending_cancel_origin, Some(CancelOrigin::Manual)),
    };
    clear_compaction_state(app, emit_manual_compaction_success);
    app.pending_cancel_origin = None;
    app.cancelled_turn_pending_hint = false;
    state
}

fn finish_ready_turn_exit(app: &mut App, exit: TurnExitState, tool_status: model::ToolCallStatus) {
    app.finalize_turn_runtime_artifacts(tool_status);
    app.status = AppStatus::Ready;
    app.files_accessed = 0;
    app.sync_git_context();

    let removed_tail_assistant = remove_empty_tail_assistant(app, exit.tail_assistant_idx);
    if exit.show_interrupted_hint {
        push_interrupted_hint(app);
    }
    if removed_tail_assistant.is_none()
        && (exit.turn_was_active || exit.cancelled_requested.is_some())
    {
        mark_turn_exit_assistant_layout_dirty(app, exit.tail_assistant_idx);
    }
    app.clear_active_turn_assistant();
    super::notices::clear_turn_notice_tracking(app);
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
    let tool_status = if exit.cancelled_requested.is_some() {
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
    if app.surface_mode == super::super::SurfaceMode::Chat {
        super::super::input_submit::maybe_auto_submit_after_cancel(app);
    }
}

pub(super) fn handle_turn_error_event(
    app: &mut App,
    msg: &str,
    classified: Option<TurnErrorClass>,
    terminal_reason: Option<crate::agent::types::TerminalReason>,
) {
    let exit = begin_turn_exit(app, true);

    if exit.cancelled_requested.is_some() {
        let summary = summarize_internal_error(msg);
        tracing::warn!(
            target: crate::logging::targets::APP_SESSION,
            event_name = "turn_error_suppressed",
            message = "turn error suppressed after cancellation request",
            outcome = "cancelled",
            error_preview = %summary,
            terminal_reason = terminal_reason.map_or("", crate::agent::types::TerminalReason::as_stored),
        );
        app.pending_submit = None;
        finish_ready_turn_exit(app, exit, model::ToolCallStatus::Failed);
        request_post_turn_resize_purge_replay_if_needed(app);
        crate::app::session_runtime::request_context_usage_refresh(app);
        if app.surface_mode == super::super::SurfaceMode::Chat {
            super::super::input_submit::maybe_auto_submit_after_cancel(app);
        }
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
        terminal_reason = terminal_reason.map_or("", crate::agent::types::TerminalReason::as_stored),
    );
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
            app.should_quit = true;
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
    app.finalize_turn_runtime_artifacts(model::ToolCallStatus::Failed);
    app.pending_auto_submit_after_cancel = false;
    app.input.clear();
    app.pending_submit = None;
    app.status = AppStatus::Error;
    let rate_limit_context = if matches!(error_class, TurnErrorClass::PlanLimit) {
        app.last_rate_limit_update
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
    app.clear_active_turn_assistant();
    super::notices::clear_turn_notice_tracking(app);
    request_post_turn_resize_purge_replay_if_needed(app);
    crate::app::session_runtime::request_context_usage_refresh(app);
}

fn request_post_turn_resize_purge_replay_if_needed(app: &mut App) {
    if !app.chat_render.take_resize_purge_replay_after_turn() {
        return;
    }
    if matches!(
        app.terminal_lifecycle,
        super::super::TerminalLifecycleState::Running(super::super::SurfaceMode::Chat)
    ) {
        app.request_chat_resize_purge_replay_rebuild();
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
    if app.messages.get(idx).is_some_and(|msg| matches!(msg.role, MessageRole::Assistant)) {
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
        TurnErrorClass::Internal | TurnErrorClass::Other => {
            let message = format!("Turn failed: {error}\n\n{TURN_ERROR_INPUT_LOCK_HINT}");
            super::push_system_message_with_severity(app, None, &message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, ChatRebuildKind, SurfaceMode, TerminalLifecycleState};

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

    #[test]
    fn turn_complete_removes_empty_tail_assistant() {
        let mut app = App::test_default();
        app.status = AppStatus::Thinking;
        app.messages.push(user_message("hello"));
        app.messages.push(empty_assistant_message());

        handle_turn_complete_event(&mut app, None);

        assert_eq!(app.messages.len(), 1);
        assert!(matches!(app.messages[0].role, MessageRole::User));
    }

    #[test]
    fn cancelled_turn_error_removes_empty_tail_assistant_before_hint() {
        let mut app = App::test_default();
        app.status = AppStatus::Thinking;
        app.pending_cancel_origin = Some(CancelOrigin::Manual);
        app.messages.push(user_message("hello"));
        app.messages.push(empty_assistant_message());

        handle_turn_error_event(&mut app, "cancelled", None, None);

        assert_eq!(app.messages.len(), 2);
        assert!(matches!(app.messages[0].role, MessageRole::User));
        assert!(matches!(app.messages[1].role, MessageRole::System(Some(SystemSeverity::Info))));
    }

    #[test]
    fn turn_error_removes_empty_tail_assistant_before_error_message() {
        let mut app = App::test_default();
        app.status = AppStatus::Thinking;
        app.messages.push(user_message("hello"));
        app.messages.push(empty_assistant_message());

        handle_turn_error_event(&mut app, "boom", None, None);

        assert_eq!(app.messages.len(), 2);
        assert!(matches!(app.messages[0].role, MessageRole::User));
        assert!(matches!(app.messages[1].role, MessageRole::System(None)));
    }

    #[test]
    fn turn_complete_keeps_canonical_assistant_and_clears_active_owner() {
        let mut app = App::test_default();
        app.status = AppStatus::Thinking;
        app.messages.push(user_message("hello"));
        app.messages.push(ChatMessage::new(
            MessageRole::Assistant,
            vec![MessageBlock::Text(TextBlock::from_complete("done"))],
            None,
        ));
        app.bind_active_turn_assistant(1);

        handle_turn_complete_event(&mut app, None);

        assert_eq!(app.status, AppStatus::Ready);
        assert_eq!(app.active_turn_assistant_idx(), None);
        assert_eq!(app.messages.len(), 2);
        let Some(MessageBlock::Text(text)) = app.messages[1].blocks.first() else {
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
        app.messages.push(user_message("hello"));
        app.messages.push(ChatMessage::new(
            MessageRole::Assistant,
            vec![MessageBlock::Text(TextBlock::from_complete("streamed"))],
            None,
        ));
        app.bind_active_turn_assistant(1);
        app.chat_render.mark_resize_purge_replay_during_turn();

        handle_turn_complete_event(&mut app, None);

        assert_eq!(app.surface_dirty.chat.rebuild, ChatRebuildKind::ResizePurgeReplay);
        assert!(app.surface_dirty.chat.repaint);
        assert!(!app.chat_render.resize_purge_replay_after_turn);
    }

    #[test]
    fn permission_request_marks_canonical_tool_pending_permission() {
        let mut app = App::test_default();
        let tool_id = "bash-1";
        app.messages.push(ChatMessage::new(
            MessageRole::Assistant,
            vec![MessageBlock::ToolCall(Box::new(crate::app::ToolCallInfo {
                id: tool_id.to_owned(),
                title: "tool".to_owned(),
                sdk_tool_name: "Bash".to_owned(),
                raw_input: None,
                raw_input_bytes: 0,
                output_metadata: None,
                task_metadata: None,
                status: model::ToolCallStatus::InProgress,
                content: Vec::new(),
                hidden: false,
                terminal_id: None,
                terminal_command: None,
                terminal_output: None,
                terminal_output_len: 0,
                terminal_bytes_seen: 0,
                terminal_snapshot_mode: crate::app::TerminalSnapshotMode::AppendOnly,
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

        let Some(MessageBlock::ToolCall(tool)) = app.messages[0].blocks.first() else {
            panic!("expected tool call block");
        };
        assert!(tool.pending_permission.is_some());
    }
}
