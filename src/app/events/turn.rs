// Claude Code Rust - A native Rust terminal interface for Claude Code
// Copyright (C) 2025  Simon Peter Rothgang
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as
// published by the Free Software Foundation, either version 3 of the
// License, or (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

use super::super::{
    App, AppStatus, CancelOrigin, ChatMessage, FocusTarget, InlinePermission, InvalidationLevel,
    MessageBlock, MessageRole, SystemSeverity, TextBlock,
};
use super::clear_compaction_state;
use super::rate_limit::format_rate_limit_summary;
use super::session::set_ready_status_unless_startup_blocked;
use crate::agent::error_handling::{TurnErrorClass, classify_turn_error, summarize_internal_error};
use crate::agent::model;

const CONVERSATION_INTERRUPTED_HINT: &str =
    "Conversation interrupted. Tell the model how to proceed.";
const TURN_ERROR_INPUT_LOCK_HINT: &str =
    "Input disabled after an error. Press Ctrl+Q to quit and try again.";
const PLAN_LIMIT_NEXT_STEPS_HINT: &str = "Next steps:\n\
1. Wait a few minutes and retry.\n\
2. Reduce request size or request frequency.\n\
3. Check quota/billing for your account or switch plans.";
const AUTH_REQUIRED_NEXT_STEPS_HINT: &str = "Authentication required. Type /login to authenticate, or run `claude auth login` in a terminal.";

pub(super) fn handle_permission_request_event(
    app: &mut App,
    request: model::RequestPermissionRequest,
    response_tx: tokio::sync::oneshot::Sender<model::RequestPermissionResponse>,
) {
    let tool_id = request.tool_call.tool_call_id.clone();
    let options = request.options.clone();

    let Some((mi, bi)) = app.lookup_tool_call(&tool_id) else {
        tracing::warn!("Permission request for unknown tool call: {tool_id}; auto-rejecting");
        reject_permission_request(response_tx, &options);
        return;
    };

    if app.pending_permission_ids.iter().any(|id| id == &tool_id) {
        tracing::warn!(
            "Duplicate permission request for tool call: {tool_id}; auto-rejecting duplicate"
        );
        reject_permission_request(response_tx, &options);
        return;
    }

    let mut layout_dirty = false;
    if let Some(MessageBlock::ToolCall(tc)) =
        app.messages.get_mut(mi).and_then(|m| m.blocks.get_mut(bi))
    {
        let tc = tc.as_mut();
        let is_first = app.pending_permission_ids.is_empty();
        tc.pending_permission = Some(InlinePermission {
            options: request.options,
            response_tx,
            selected_index: 0,
            focused: is_first,
        });
        tc.mark_tool_call_layout_dirty();
        layout_dirty = true;
        app.pending_permission_ids.push(tool_id);
        app.claim_focus_target(FocusTarget::Permission);
        app.viewport.engage_auto_scroll();
        app.notifications.notify(
            app.config.preferred_notification_channel_effective(),
            super::super::notify::NotifyEvent::PermissionRequired,
        );
    } else {
        tracing::warn!("Permission request for non-tool block index: {tool_id}; auto-rejecting");
        reject_permission_request(response_tx, &options);
    }

    if layout_dirty {
        app.invalidate_layout(InvalidationLevel::Single(mi));
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
    clear_compaction_state(app, false);
    if app.pending_cancel_origin.is_none() {
        app.pending_cancel_origin = Some(CancelOrigin::Manual);
    }
    app.cancelled_turn_pending_hint =
        matches!(app.pending_cancel_origin, Some(CancelOrigin::Manual));
    let _ = app.finalize_in_progress_tool_calls(model::ToolCallStatus::Failed);
}

pub(super) fn handle_turn_complete_event(app: &mut App) {
    let tail_assistant_idx =
        app.messages.iter().rposition(|m| matches!(m.role, MessageRole::Assistant));
    let turn_was_active = matches!(app.status, AppStatus::Thinking | AppStatus::Running);
    clear_compaction_state(app, true);
    let cancelled_requested = app.pending_cancel_origin.is_some();
    let show_interrupted_hint = matches!(app.pending_cancel_origin, Some(CancelOrigin::Manual));
    app.pending_cancel_origin = None;
    app.cancelled_turn_pending_hint = false;

    if cancelled_requested {
        let _ = app.finalize_in_progress_tool_calls(model::ToolCallStatus::Failed);
    } else {
        let _ = app.finalize_in_progress_tool_calls(model::ToolCallStatus::Completed);
    }

    set_ready_status_unless_startup_blocked(app);
    app.files_accessed = 0;
    app.clear_tool_scope_tracking();
    app.refresh_git_branch();
    if show_interrupted_hint {
        push_interrupted_hint(app);
    }
    if turn_was_active || cancelled_requested {
        mark_turn_exit_assistant_layout_dirty(app, tail_assistant_idx);
    }
    if turn_was_active {
        app.notifications.notify(
            app.config.preferred_notification_channel_effective(),
            super::super::notify::NotifyEvent::TurnComplete,
        );
    }
    if app.active_view == super::super::ActiveView::Chat {
        super::super::input_submit::maybe_auto_submit_after_cancel(app);
    }
}

pub(super) fn handle_turn_error_event(
    app: &mut App,
    msg: &str,
    classified: Option<TurnErrorClass>,
) {
    let tail_assistant_idx =
        app.messages.iter().rposition(|m| matches!(m.role, MessageRole::Assistant));
    let turn_was_active = matches!(app.status, AppStatus::Thinking | AppStatus::Running);
    clear_compaction_state(app, false);
    let cancelled_requested = app.pending_cancel_origin;
    let show_interrupted_hint = matches!(cancelled_requested, Some(CancelOrigin::Manual));
    app.pending_cancel_origin = None;
    app.cancelled_turn_pending_hint = false;

    if cancelled_requested.is_some() {
        let summary = summarize_internal_error(msg);
        tracing::warn!(
            error_preview = %summary,
            "Turn error suppressed after cancellation request"
        );
        mark_turn_exit_assistant_layout_dirty(app, tail_assistant_idx);
        let _ = app.finalize_in_progress_tool_calls(model::ToolCallStatus::Failed);
        app.pending_submit = None;
        app.status = AppStatus::Ready;
        app.files_accessed = 0;
        app.clear_tool_scope_tracking();
        app.refresh_git_branch();
        if show_interrupted_hint {
            push_interrupted_hint(app);
        }
        if app.active_view == super::super::ActiveView::Chat {
            super::super::input_submit::maybe_auto_submit_after_cancel(app);
        }
        return;
    }

    let error_class = classified.unwrap_or_else(|| classify_turn_error(msg));
    tracing::error!("Turn error: {msg}");
    let summary = summarize_internal_error(msg);
    match error_class {
        TurnErrorClass::PlanLimit => {
            tracing::warn!(
                error_preview = %summary,
                "Turn error classified as plan/usage limit"
            );
        }
        TurnErrorClass::AuthRequired => {
            tracing::warn!(
                error_preview = %summary,
                "Turn error indicates authentication is required"
            );
            app.exit_error = Some(crate::error::AppError::AuthRequired);
            app.should_quit = true;
        }
        TurnErrorClass::Internal => {
            tracing::debug!(
                error_preview = %summary,
                "Internal Agent SDK turn error payload"
            );
        }
        TurnErrorClass::Other => {}
    }
    let _ = app.finalize_in_progress_tool_calls(model::ToolCallStatus::Failed);
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
    push_turn_error_message(app, msg, error_class, rate_limit_context.as_ref());
    if turn_was_active {
        mark_turn_exit_assistant_layout_dirty(app, tail_assistant_idx);
    }
}

fn push_interrupted_hint(app: &mut App) {
    app.messages.push(ChatMessage {
        role: MessageRole::System(Some(SystemSeverity::Info)),
        blocks: vec![MessageBlock::Text(TextBlock::from_complete(CONVERSATION_INTERRUPTED_HINT))],
        usage: None,
    });
    app.enforce_history_retention_tracked();
    app.viewport.engage_auto_scroll();
}

fn mark_turn_exit_assistant_layout_dirty(app: &mut App, idx: Option<usize>) {
    let Some(idx) = idx else {
        return;
    };
    if app.messages.get(idx).is_some_and(|msg| matches!(msg.role, MessageRole::Assistant)) {
        app.invalidate_layout(InvalidationLevel::Single(idx));
    }
}

fn push_turn_error_message(
    app: &mut App,
    error: &str,
    class: TurnErrorClass,
    rate_limit_context: Option<&model::RateLimitUpdate>,
) {
    let base_message = match class {
        TurnErrorClass::PlanLimit => {
            let summary = summarize_internal_error(error);
            format!(
                "Turn blocked by account or plan limits: {summary}\n\n{PLAN_LIMIT_NEXT_STEPS_HINT}\n\n{TURN_ERROR_INPUT_LOCK_HINT}"
            )
        }
        TurnErrorClass::AuthRequired => {
            format!("{AUTH_REQUIRED_NEXT_STEPS_HINT}\n\n{TURN_ERROR_INPUT_LOCK_HINT}")
        }
        TurnErrorClass::Internal | TurnErrorClass::Other => {
            format!("Turn failed: {error}\n\n{TURN_ERROR_INPUT_LOCK_HINT}")
        }
    };
    let (severity, message) = if matches!(class, TurnErrorClass::PlanLimit)
        && let Some(update) = rate_limit_context
    {
        let prefix = format_rate_limit_summary(update);
        let severity = match update.status {
            model::RateLimitStatus::AllowedWarning => Some(SystemSeverity::Warning),
            model::RateLimitStatus::Rejected | model::RateLimitStatus::Allowed => None,
        };
        (severity, format!("{prefix}\n\n{base_message}"))
    } else {
        (None, base_message)
    };
    super::push_system_message_with_severity(app, severity, &message);
}
