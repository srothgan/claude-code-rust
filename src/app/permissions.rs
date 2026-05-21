// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::inline_interactions::{
    clear_inline_interaction_focus, focus_next_inline_interaction, focused_interaction,
    focused_interaction_dirty_idx, focused_interaction_is_active, get_focused_interaction_tc,
    handle_interaction_focus_cycle, invalidate_if_changed, normalize_pending_interaction_queue,
    pop_next_valid_interaction_id,
};
use super::{App, InvalidationLevel, MessageBlock};
use crate::agent::model;
use crate::agent::model::PermissionOptionKind;
use crate::app::keymap::InteractionAction;
use crate::app::keys::KeyOutcome;
use crossterm::event::{KeyCode, KeyEvent};

fn focused_permission(app: &App) -> Option<&crate::app::InlinePermission> {
    focused_interaction(app)?.pending_permission.as_ref()
}

fn focused_option_index_by_kind(app: &App, kind: PermissionOptionKind) -> Option<usize> {
    focused_option_index_where(app, |opt| opt.kind == kind)
}

fn focused_option_index_where<F>(app: &App, mut predicate: F) -> Option<usize>
where
    F: FnMut(&model::PermissionOption) -> bool,
{
    focused_permission(app)?.options.iter().position(&mut predicate)
}

fn normalized_option_tokens(option: &model::PermissionOption) -> String {
    let mut out = String::new();
    for ch in option.name.chars().chain(option.option_id.chars()) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        }
    }
    out
}

fn option_tokens(option: &model::PermissionOption) -> (bool, bool, bool, bool) {
    let tokens = normalized_option_tokens(option);
    let allow_like =
        tokens.contains("allow") || tokens.contains("accept") || tokens.contains("approve");
    let reject_like =
        tokens.contains("reject") || tokens.contains("deny") || tokens.contains("disallow");
    let persistent_like = tokens.contains("always")
        || tokens.contains("dontask")
        || tokens.contains("remember")
        || tokens.contains("persist")
        || tokens.contains("bypasspermissions");
    let session_like = tokens.contains("session") || tokens.contains("onesession");
    (allow_like, reject_like, persistent_like, session_like)
}

fn option_is_reject_fallback(option: &model::PermissionOption) -> bool {
    let (allow_like, reject_like, _, _) = option_tokens(option);
    reject_like && !allow_like
}

pub(super) fn focused_permission_is_plan_approval(app: &App) -> bool {
    focused_permission(app).is_some_and(|pending| {
        pending.options.iter().any(|opt| {
            matches!(opt.kind, PermissionOptionKind::PlanApprove | PermissionOptionKind::PlanReject)
        })
    })
}

fn focused_permission_option_count(app: &App) -> usize {
    focused_permission(app).map_or(0, |permission| permission.options.len())
}

pub(super) fn execute_permission_action(
    app: &mut App,
    action: InteractionAction,
    key: KeyEvent,
) -> KeyOutcome {
    normalize_pending_interaction_queue(app);
    if !focused_interaction_is_active(app) || focused_permission(app).is_none() {
        return KeyOutcome::Ignored;
    }

    let option_count = focused_permission_option_count(app);
    match action {
        InteractionAction::MovePrevious => {
            if let Some(outcome) = handle_permission_vertical_focus_cycle(app, key) {
                return outcome;
            }
            if option_count == 0 {
                return KeyOutcome::Handled(false);
            }
            move_permission_option_left(app);
            KeyOutcome::Handled(true)
        }
        InteractionAction::MoveNext => {
            if let Some(outcome) = handle_permission_vertical_focus_cycle(app, key) {
                return outcome;
            }
            if option_count == 0 {
                return KeyOutcome::Handled(false);
            }
            move_permission_option_right(app, option_count);
            KeyOutcome::Handled(true)
        }
        InteractionAction::Confirm => {
            if option_count == 0 {
                return KeyOutcome::Ignored;
            }
            respond_permission(app, None);
            KeyOutcome::Handled(true)
        }
        InteractionAction::Cancel => {
            if !respond_permission_reject_or_cancel(app, option_count) {
                return KeyOutcome::Ignored;
            }
            KeyOutcome::Handled(true)
        }
        InteractionAction::FocusNext => {
            clear_inline_interaction_focus(app);
            KeyOutcome::Handled(true)
        }
        InteractionAction::MoveStart
        | InteractionAction::MoveEnd
        | InteractionAction::ToggleSelection
        | InteractionAction::ToggleNotes => KeyOutcome::Ignored,
    }
}

fn handle_permission_vertical_focus_cycle(app: &mut App, key: KeyEvent) -> Option<KeyOutcome> {
    if !matches!(key.code, KeyCode::Up | KeyCode::Down) || focused_permission_is_plan_approval(app)
    {
        return None;
    }

    handle_interaction_focus_cycle(app, key, true, false).map(|_consumed| KeyOutcome::Handled(true))
}

fn move_permission_option_left(app: &mut App) {
    let dirty_idx = focused_interaction_dirty_idx(app);
    let mut changed = false;
    if let Some(tc) = get_focused_interaction_tc(app)
        && let Some(ref mut permission) = tc.pending_permission
    {
        let next = permission.selected_index.saturating_sub(1);
        if next != permission.selected_index {
            permission.selected_index = next;
            tc.invalidate_render_cache();
            changed = true;
        }
    }
    invalidate_if_changed(app, dirty_idx, changed);
}

fn move_permission_option_right(app: &mut App, option_count: usize) {
    let dirty_idx = focused_interaction_dirty_idx(app);
    let mut changed = false;
    if let Some(tc) = get_focused_interaction_tc(app)
        && let Some(ref mut permission) = tc.pending_permission
        && permission.selected_index + 1 < option_count
    {
        permission.selected_index += 1;
        tc.invalidate_render_cache();
        changed = true;
    }
    invalidate_if_changed(app, dirty_idx, changed);
}

fn respond_permission_reject_or_cancel(app: &mut App, option_count: usize) -> bool {
    if let Some(idx) = focused_option_index_by_kind(app, PermissionOptionKind::RejectOnce)
        .or_else(|| focused_option_index_by_kind(app, PermissionOptionKind::RejectAlways))
        .or_else(|| focused_option_index_where(app, option_is_reject_fallback))
    {
        respond_permission(app, Some(idx));
        true
    } else if option_count > 0 {
        respond_permission(app, Some(option_count - 1));
        true
    } else {
        respond_permission_cancel(app);
        true
    }
}

fn respond_permission(app: &mut App, override_index: Option<usize>) {
    let Some(tool_id) = pop_next_valid_interaction_id(app) else {
        return;
    };

    let Some((mi, bi)) = app.tool_call_index.get(&tool_id).copied() else {
        return;
    };
    let Some(MessageBlock::ToolCall(tc)) =
        app.messages.get_mut(mi).and_then(|m| m.blocks.get_mut(bi))
    else {
        return;
    };
    let tc = tc.as_mut();
    let mut invalidated = false;
    if let Some(pending) = tc.pending_permission.take() {
        let idx = override_index.unwrap_or(pending.selected_index);
        if let Some(opt) = pending.options.get(idx) {
            tracing::debug!(
                target: crate::logging::targets::APP_PERMISSION,
                event_name = "permission_response_applied",
                message = "permission response applied",
                outcome = "success",
                tool_call_id = %tool_id,
                selected_index = idx,
                option_id = %opt.option_id,
                option_name = %opt.name,
                option_kind = ?opt.kind,
            );
            let _ = pending.response_tx.send(model::RequestPermissionResponse::new(
                model::RequestPermissionOutcome::Selected(model::SelectedPermissionOutcome::new(
                    opt.option_id.clone(),
                )),
            ));
        } else {
            tracing::warn!(
                target: crate::logging::targets::APP_PERMISSION,
                event_name = "permission_response_rejected",
                message = "permission response index was out of bounds",
                outcome = "failure",
                tool_call_id = %tool_id,
                selected_index = idx,
                option_count = pending.options.len(),
            );
        }
        tc.invalidate_render_cache();
        invalidated = true;
    }
    if invalidated {
        app.sync_render_cache_slot(mi, bi);
        app.recompute_message_retained_bytes(mi);
        app.invalidate_layout(InvalidationLevel::MessageChanged(mi));
        app.request_chat_mutable_rebuild();
    }

    focus_next_inline_interaction(app);
}

fn respond_permission_cancel(app: &mut App) {
    let Some(tool_id) = pop_next_valid_interaction_id(app) else {
        return;
    };

    let Some((mi, bi)) = app.tool_call_index.get(&tool_id).copied() else {
        return;
    };
    let Some(MessageBlock::ToolCall(tc)) =
        app.messages.get_mut(mi).and_then(|m| m.blocks.get_mut(bi))
    else {
        return;
    };
    let tc = tc.as_mut();
    if let Some(pending) = tc.pending_permission.take() {
        let _ = pending.response_tx.send(model::RequestPermissionResponse::new(
            model::RequestPermissionOutcome::Cancelled,
        ));
        tc.invalidate_render_cache();
        app.sync_render_cache_slot(mi, bi);
        app.recompute_message_retained_bytes(mi);
        app.invalidate_layout(InvalidationLevel::MessageChanged(mi));
        app.request_chat_mutable_rebuild();
    }

    focus_next_inline_interaction(app);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::keymap::KeyContext;
    use crate::app::{
        App, AppStatus, BlockCache, ChatMessage, IncrementalMarkdown, InlinePermission,
        MessageBlock, MessageRole, ToolCallInfo,
    };
    use crossterm::event::KeyModifiers;
    use pretty_assertions::assert_eq;
    use tokio::sync::oneshot;

    fn test_tool_call(id: &str) -> ToolCallInfo {
        ToolCallInfo {
            id: id.to_owned(),
            title: format!("Tool {id}"),
            sdk_tool_name: "Read".to_owned(),
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
            cache: BlockCache::default(),
            pending_permission: None,
            pending_question: None,
        }
    }

    fn assistant_tool_msg(tc: ToolCallInfo) -> ChatMessage {
        ChatMessage::new(MessageRole::Assistant, vec![MessageBlock::ToolCall(Box::new(tc))], None)
    }

    fn allow_options() -> Vec<model::PermissionOption> {
        vec![
            model::PermissionOption::new(
                "allow-once",
                "Allow once",
                PermissionOptionKind::AllowOnce,
            ),
            model::PermissionOption::new(
                "allow-always",
                "Allow always",
                PermissionOptionKind::AllowAlways,
            ),
            model::PermissionOption::new("reject-once", "Reject", PermissionOptionKind::RejectOnce),
        ]
    }

    fn add_permission(
        app: &mut App,
        tool_id: &str,
        options: Vec<model::PermissionOption>,
        focused: bool,
    ) -> oneshot::Receiver<model::RequestPermissionResponse> {
        let msg_idx = app.messages.len();
        app.messages.push(assistant_tool_msg(test_tool_call(tool_id)));
        app.index_tool_call(tool_id.to_owned(), msg_idx, 0);

        let (tx, rx) = oneshot::channel();
        if let Some(MessageBlock::ToolCall(tc)) =
            app.messages.get_mut(msg_idx).and_then(|m| m.blocks.get_mut(0))
        {
            tc.pending_permission = Some(InlinePermission {
                options,
                display: None,
                response_tx: tx,
                selected_index: 0,
                focused,
            });
        }
        app.pending_interaction_ids.push(tool_id.to_owned());
        rx
    }

    fn permission_focused(app: &App, tool_id: &str) -> bool {
        let Some((mi, bi)) = app.lookup_tool_call(tool_id) else {
            return false;
        };
        let Some(MessageBlock::ToolCall(tc)) = app.messages.get(mi).and_then(|m| m.blocks.get(bi))
        else {
            return false;
        };
        tc.pending_permission.as_ref().is_some_and(|permission| permission.focused)
    }

    #[test]
    fn step2_up_down_rotates_permission_focus_and_enter_targets_focused_prompt() {
        let mut app = App::test_default();
        app.status = AppStatus::Ready;
        let mut rx1 = add_permission(&mut app, "perm-1", allow_options(), true);
        let mut rx2 = add_permission(&mut app, "perm-2", allow_options(), false);

        assert_eq!(app.pending_interaction_ids, vec!["perm-1", "perm-2"]);
        assert!(permission_focused(&app, "perm-1"));
        assert!(!permission_focused(&app, "perm-2"));

        let consumed = execute_permission_action(
            &mut app,
            InteractionAction::MoveNext,
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
        );
        assert_eq!(consumed, KeyOutcome::Handled(true));
        assert_eq!(app.pending_interaction_ids, vec!["perm-2", "perm-1"]);
        assert!(permission_focused(&app, "perm-2"));
        assert!(!permission_focused(&app, "perm-1"));

        let consumed = execute_permission_action(
            &mut app,
            InteractionAction::Confirm,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        assert_eq!(consumed, KeyOutcome::Handled(true));

        let resp2 = rx2.try_recv().expect("focused permission should receive response");
        let model::RequestPermissionOutcome::Selected(sel2) = resp2.outcome else {
            panic!("expected selected permission response");
        };
        assert_eq!(sel2.option_id.clone(), "allow-once");
        assert!(matches!(rx1.try_recv(), Err(tokio::sync::oneshot::error::TryRecvError::Empty)));
        assert_eq!(app.pending_interaction_ids, vec!["perm-1"]);
    }

    #[test]
    fn default_permission_keymap_has_no_letter_shortcuts() {
        let app = App::test_default();

        for key in [
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Char('\u{19}'), KeyModifiers::NONE),
        ] {
            assert_eq!(app.keymap.action_for_event(KeyContext::InlinePermission, key), None);
        }
    }

    #[test]
    fn permission_actions_are_ignored_when_prompt_is_not_focused() {
        let mut app = App::test_default();
        let mut rx = add_permission(&mut app, "perm-1", allow_options(), false);

        assert_eq!(
            execute_permission_action(
                &mut app,
                InteractionAction::MoveNext,
                KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
            ),
            KeyOutcome::Ignored
        );
        assert_eq!(
            execute_permission_action(
                &mut app,
                InteractionAction::Confirm,
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            ),
            KeyOutcome::Ignored
        );

        assert_eq!(app.pending_interaction_ids, vec!["perm-1"]);
        assert!(matches!(rx.try_recv(), Err(tokio::sync::oneshot::error::TryRecvError::Empty)));
    }

    #[test]
    fn keeps_non_tool_blocks_untouched() {
        let app = App::test_default();
        let _ = IncrementalMarkdown::default();
        assert!(app.messages.is_empty());
    }

    #[test]
    fn esc_cancels_permission_when_no_reject_option_exists() {
        let mut app = App::test_default();
        let mut rx = add_permission(
            &mut app,
            "perm-1",
            vec![model::PermissionOption::new(
                "allow-once",
                "Allow once",
                PermissionOptionKind::AllowOnce,
            )],
            true,
        );

        respond_permission_cancel(&mut app);

        let resp = rx.try_recv().expect("permission should be cancelled");
        assert!(matches!(resp.outcome, model::RequestPermissionOutcome::Cancelled));
    }
}
