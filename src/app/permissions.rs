// SPDX-License-Identifier: Apache-2.0
use super::App;
use super::inline_interactions::{
    clear_inline_interaction_focus, focus_next_inline_interaction, focused_interaction,
    focused_interaction_dirty_idx, focused_interaction_is_active, get_focused_interaction_tc,
    handle_interaction_focus_cycle, invalidate_if_changed, normalize_pending_interaction_queue,
    pop_next_valid_interaction_id,
};
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

    if let Some(pending) = app.mutate_tool_call(&tool_id, |tool| tool.pending_permission.take()) {
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

        app.request_chat_mutable_rebuild();
    }

    focus_next_inline_interaction(app);
}

fn respond_permission_cancel(app: &mut App) {
    let Some(tool_id) = pop_next_valid_interaction_id(app) else {
        return;
    };

    if let Some(pending) = app.mutate_tool_call(&tool_id, |tool| tool.pending_permission.take()) {
        let _ = pending.response_tx.send(model::RequestPermissionResponse::new(
            model::RequestPermissionOutcome::Cancelled,
        ));
        app.request_chat_mutable_rebuild();
    }

    focus_next_inline_interaction(app);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{
        App, AppStatus, ChatMessage, InlinePermission, MessageBlock, MessageRole, ToolCallInfo,
    };
    use crossterm::event::KeyModifiers;
    use pretty_assertions::assert_eq;
    use tokio::sync::oneshot;

    fn test_tool_call(id: &str) -> ToolCallInfo {
        ToolCallInfo {
            title: format!("Tool {id}"),
            ..crate::app::test_support::named_tool_call_info(
                id,
                "Read",
                model::ToolCallStatus::InProgress,
            )
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
        let msg_idx = app.transcript.messages.len();
        app.transcript.messages.push(assistant_tool_msg(test_tool_call(tool_id)));
        app.index_tool_call(tool_id.to_owned(), msg_idx, 0);

        let (tx, rx) = oneshot::channel();
        if let Some(MessageBlock::ToolCall(tc)) =
            app.transcript.messages.get_mut(msg_idx).and_then(|m| m.blocks.get_mut(0))
        {
            tc.pending_permission = Some(InlinePermission {
                options,
                display: None,
                subagent_context: None,
                response_tx: tx,
                selected_index: 0,
                focused,
            });
        }
        app.pending_interaction_ids.push(tool_id.to_owned());
        rx
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

    fn admitted_permission(
        app: &mut App,
        id: &str,
        plan: bool,
    ) -> oneshot::Receiver<model::RequestPermissionResponse> {
        use crate::app::test_support::{admit_tool, permission_options, request_permission};
        admit_tool(
            app,
            model::ToolCall::new(id, format!("Command {id}"))
                .status(model::ToolCallStatus::InProgress)
                .meta(serde_json::json!({"claudeCode":{"toolName":"Bash"}})),
        );
        let options = if plan {
            vec![
                model::PermissionOption::new(
                    format!("{id}-approve"),
                    "Approve plan",
                    PermissionOptionKind::PlanApprove,
                ),
                model::PermissionOption::new(
                    format!("{id}-reject"),
                    "Reject plan",
                    PermissionOptionKind::PlanReject,
                ),
            ]
        } else {
            permission_options(id)
        };
        request_permission(
            app,
            model::RequestPermissionRequest::new(
                "session-1",
                model::ToolCallUpdate::new(id, model::ToolCallUpdateFields::new()),
                options,
                None,
            ),
        )
    }

    fn permission_card_rows(app: &mut App, id: &str) -> Vec<ratatui::text::Line<'static>> {
        let (mi, bi) = app.lookup_tool_call(id).expect("requested tool");
        let live = crate::ui::inline_chat_rows::serialize_live_rows_with_boundaries_excluding(
            app,
            100,
            &std::collections::BTreeSet::default(),
            None,
        );
        let rows: Vec<_> = live
            .segments()
            .iter()
            .filter(|segment| segment.msg_idx == mi && segment.block_idx == Some(bi))
            .flat_map(|segment| live.segment_rows(segment).expect("own tool region").to_vec())
            .collect();
        rows
    }

    fn card_option_styles(app: &mut App, id: &str, labels: &[&str]) -> Vec<ratatui::style::Style> {
        let rows = permission_card_rows(app, id);
        labels
            .iter()
            .map(|label| {
                let row = rows
                    .iter()
                    .find(|row| row.to_string().contains(label))
                    .expect("own option rendered");
                let position = row.to_string().find(label).expect("option label");
                let mut offset = 0;
                row.spans
                    .iter()
                    .find(|span| {
                        offset += span.content.len();
                        position < offset
                    })
                    .expect("label style")
                    .style
            })
            .collect()
    }

    #[test]
    fn ordinary_permission_keys_target_the_focused_request_and_preserve_the_draft() {
        use crate::app::test_support::{
            app_with_connection, assert_pending, assert_permission_response, press,
        };
        let (mut app, mut commands) = app_with_connection();
        app.status = AppStatus::Running;
        app.push_message_tracked(ChatMessage::new(
            MessageRole::User,
            vec![MessageBlock::Text(crate::app::TextBlock::from_complete("preserved transcript"))],
            None,
        ));
        let mut first = admitted_permission(&mut app, "first", false);
        let mut second = admitted_permission(&mut app, "second", false);
        app.input.set_text("preserved draft");
        let labels = ["first once", "first always", "first deny"];
        let styles = card_option_styles(&mut app, "first", &labels);
        assert_ne!(styles[0], styles[1]);
        assert_eq!(styles[1], styles[2]);
        press(&mut app, KeyCode::Down);
        let styles = card_option_styles(
            &mut app,
            "second",
            &["second once", "second always", "second deny"],
        );
        assert_ne!(styles[0], styles[1]);
        let rows = permission_card_rows(&mut app, "first");
        assert!(rows.iter().any(|row| row.to_string().contains("Tab to focus")));
        assert!(!rows.iter().any(|row| row.to_string().contains("first once")));
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Right);
        let styles = card_option_styles(&mut app, "first", &labels);
        assert_ne!(styles[1], styles[0]);
        assert_eq!(styles[0], styles[2]);
        press(&mut app, KeyCode::Enter);
        assert_permission_response(&mut first, "first-always");
        assert_pending(&mut second);
        press(&mut app, KeyCode::Enter);
        assert_permission_response(&mut second, "second-once");
        assert_eq!(app.input.text(), "preserved draft");
        assert!(commands.try_recv().is_err(), "answering must not submit or cancel the turn");
        let MessageBlock::Text(text) = &app.transcript.messages[0].blocks[0] else {
            panic!("user transcript");
        };
        assert_eq!(text.text, "preserved transcript");
    }

    #[test]
    fn plan_vertical_keys_select_options_and_escape_rejects_only_the_current_request() {
        use crate::app::test_support::{
            app_with_connection, assert_pending, assert_permission_response, press,
        };
        for (plan, keys, expected) in [
            (true, vec![KeyCode::Down, KeyCode::Enter], "first-reject"),
            (true, vec![KeyCode::Down, KeyCode::Up, KeyCode::Enter], "first-approve"),
            (true, vec![KeyCode::Esc], "first-reject"),
            (false, vec![KeyCode::Esc], "first-deny"),
        ] {
            let (mut app, mut commands) = app_with_connection();
            app.status = AppStatus::Running;
            let mut first = admitted_permission(&mut app, "first", plan);
            let mut second = admitted_permission(&mut app, "second", false);
            app.input.set_text("keep draft");
            for key in keys {
                press(&mut app, key);
                if plan && matches!(key, KeyCode::Down | KeyCode::Up) {
                    let styles =
                        card_option_styles(&mut app, "first", &["Approve plan", "Reject plan"]);
                    let selected = usize::from(key == KeyCode::Down);
                    assert!(styles[selected].add_modifier.contains(ratatui::style::Modifier::BOLD));
                    assert!(
                        !styles[1 - selected].add_modifier.contains(ratatui::style::Modifier::BOLD)
                    );
                }
            }
            assert_permission_response(&mut first, expected);
            assert_pending(&mut second);
            assert!(commands.try_recv().is_err(), "first Escape must not cancel the turn");
            press(&mut app, KeyCode::Enter);
            assert_permission_response(&mut second, "second-once");
            assert_eq!(app.input.text(), "keep draft");
        }
    }
}
