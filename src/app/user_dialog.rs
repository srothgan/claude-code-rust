// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

//! Key handling for the inline turn-level user dialog (`refusal_fallback_prompt`).
//!
//! Mirrors [`super::permissions::execute_permission_action`] but drives a
//! standalone [`MessageBlock::UserDialog`] block rather than an interaction
//! anchored on a tool call. Vertical navigation moves the selection within the
//! dialog's options; `Confirm` sends the chosen option, `Cancel` declines.

use super::inline_interactions::{
    clear_inline_interaction_focus, focus_next_inline_interaction, focused_interaction_id,
    has_focused_user_dialog, pop_next_valid_interaction_id,
};
use super::{App, InvalidationLevel, MessageBlock, MessageRole};
use crate::agent::model;
use crate::app::keymap::InteractionAction;
use crate::app::keys::KeyOutcome;
use crossterm::event::KeyEvent;

const EDIT_PROMPT_OPTION_ID: &str = "edit_prompt";

pub(super) fn execute_user_dialog_action(
    app: &mut App,
    action: InteractionAction,
    _key: KeyEvent,
) -> KeyOutcome {
    if !has_focused_user_dialog(app) {
        return KeyOutcome::Ignored;
    }

    match action {
        InteractionAction::MovePrevious => {
            move_dialog_selection(app, -1);
            KeyOutcome::Handled(true)
        }
        InteractionAction::MoveNext => {
            move_dialog_selection(app, 1);
            KeyOutcome::Handled(true)
        }
        InteractionAction::Confirm => {
            respond_dialog(app, DialogResolution::Selected);
            KeyOutcome::Handled(true)
        }
        InteractionAction::Cancel => {
            respond_dialog(app, DialogResolution::Cancelled);
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

fn focused_dialog_slot(app: &App) -> Option<(usize, usize)> {
    let tool_id = focused_interaction_id(app)?;
    app.lookup_tool_call(tool_id)
}

fn move_dialog_selection(app: &mut App, delta: i32) {
    let Some((mi, bi)) = focused_dialog_slot(app) else {
        return;
    };
    let mut changed = false;
    if let Some(MessageBlock::UserDialog(dialog)) =
        app.transcript.messages.get_mut(mi).and_then(|m| m.blocks.get_mut(bi))
    {
        if dialog.options.is_empty() {
            return;
        }
        let max = dialog.options.len() - 1;
        let next = if delta < 0 {
            dialog.selected_index.saturating_sub(1)
        } else {
            (dialog.selected_index + 1).min(max)
        };
        if next != dialog.selected_index {
            dialog.selected_index = next;
            dialog.cache.invalidate();
            changed = true;
        }
    }
    if changed {
        app.sync_render_cache_slot(mi, bi);
        app.recompute_message_retained_bytes(mi);
        app.invalidate_layout(InvalidationLevel::MessageChanged(mi));
        app.request_chat_mutable_rebuild();
    }
}

#[derive(Clone, Copy)]
enum DialogResolution {
    Selected,
    Cancelled,
}

fn respond_dialog(app: &mut App, resolution: DialogResolution) {
    let Some(tool_id) = pop_next_valid_interaction_id(app) else {
        return;
    };
    let Some((mi, bi)) = app.lookup_tool_call(&tool_id) else {
        return;
    };
    let Some(MessageBlock::UserDialog(dialog)) =
        app.transcript.messages.get_mut(mi).and_then(|m| m.blocks.get_mut(bi))
    else {
        return;
    };

    let mut repopulate_composer = false;
    if let Some(response_tx) = dialog.response_tx.take() {
        let selected_option_id = match resolution {
            DialogResolution::Selected => {
                dialog.options.get(dialog.selected_index).map(|option| option.option_id.clone())
            }
            DialogResolution::Cancelled => None,
        };
        let outcome = if let Some(option_id) = selected_option_id {
            repopulate_composer = option_id == EDIT_PROMPT_OPTION_ID;
            tracing::info!(
                target: crate::logging::targets::APP_PERMISSION,
                event_name = "user_dialog_response_applied",
                message = "user dialog response applied",
                outcome = "success",
                request_id = %dialog.request_id,
                option_id = %option_id,
            );
            model::RequestUserDialogOutcome::Selected(model::SelectedUserDialogOutcome::new(
                option_id,
            ))
        } else {
            tracing::info!(
                target: crate::logging::targets::APP_PERMISSION,
                event_name = "user_dialog_response_applied",
                message = "user dialog declined",
                outcome = "success",
                request_id = %dialog.request_id,
                option_id = "cancelled",
            );
            model::RequestUserDialogOutcome::Cancelled
        };
        let _ = response_tx.send(model::RequestUserDialogResponse::new(outcome));
        dialog.answered = true;
        dialog.focused = false;
        dialog.cache.invalidate();
        app.sync_render_cache_slot(mi, bi);
        app.recompute_message_retained_bytes(mi);
        app.invalidate_layout(InvalidationLevel::MessageChanged(mi));
        app.request_chat_mutable_rebuild();
    }

    // `edit_prompt` keeps the original model but asks the user to revise their
    // prompt. The original text is not in the payload, so reconstruct it from
    // the local message store (best-effort: leave the composer empty if absent).
    if repopulate_composer {
        repopulate_composer_from_last_user_message(app);
    }

    // Releases the Permission focus target once the queue drains, returning
    // focus to the composer.
    focus_next_inline_interaction(app);
}

fn repopulate_composer_from_last_user_message(app: &mut App) {
    let last_user_text = app.transcript.messages.iter().rev().find_map(|message| {
        if !matches!(message.role, MessageRole::User) {
            return None;
        }
        message.blocks.iter().rev().find_map(|block| {
            if let MessageBlock::Text(text) = block { Some(text.text.clone()) } else { None }
        })
    });
    if let Some(text) = last_user_text {
        app.input.set_text(&text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, AppStatus, ChatMessage, SystemSeverity, TextBlock, UserDialogBlock};
    use crossterm::event::{KeyCode, KeyModifiers};
    use tokio::sync::oneshot;

    fn dialog_request(request_id: &str) -> model::RequestUserDialogRequest {
        model::RequestUserDialogRequest::new(
            model::SessionId::new("session-1"),
            request_id,
            "refusal_fallback_prompt",
            model::RefusalFallbackPayload {
                original_model: "claude-opus-4-8".to_owned(),
                fallback_model: "claude-sonnet-4-6".to_owned(),
                ..Default::default()
            },
            vec![
                model::UserDialogOption::new("retry_fallback", "Switch to claude-sonnet-4-6"),
                model::UserDialogOption::new(
                    "edit_prompt",
                    "Edit prompt and retry with claude-opus-4-8",
                ),
            ],
        )
    }

    fn add_user_dialog(
        app: &mut App,
        request_id: &str,
        focused: bool,
    ) -> oneshot::Receiver<model::RequestUserDialogResponse> {
        let msg_idx = app.transcript.messages.len();
        let (tx, rx) = oneshot::channel();
        let mut block = UserDialogBlock::new(dialog_request(request_id), tx);
        block.focused = focused;
        app.transcript.messages.push(ChatMessage::new(
            MessageRole::System(Some(SystemSeverity::Warning)),
            vec![MessageBlock::UserDialog(block)],
            None,
        ));
        app.index_tool_call(request_id.to_owned(), msg_idx, 0);
        app.turn.pending_interaction_ids.push(request_id.to_owned());
        rx
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn confirm_sends_retry_fallback_by_default() {
        let mut app = App::test_default();
        app.status = AppStatus::Ready;
        let mut rx = add_user_dialog(&mut app, "dialog-1", true);

        let outcome =
            execute_user_dialog_action(&mut app, InteractionAction::Confirm, key(KeyCode::Enter));
        assert_eq!(outcome, KeyOutcome::Handled(true));

        let response = rx.try_recv().expect("dialog should receive a response");
        let model::RequestUserDialogOutcome::Selected(selected) = response.outcome else {
            panic!("expected a selected outcome");
        };
        assert_eq!(selected.option_id, "retry_fallback");
        assert!(!app.turn.pending_interaction_ids.iter().any(|id| id == "dialog-1"));
    }

    #[test]
    fn move_next_then_confirm_sends_edit_prompt_and_repopulates_composer() {
        let mut app = App::test_default();
        app.status = AppStatus::Ready;
        app.transcript.messages.push(ChatMessage::new(
            MessageRole::User,
            vec![MessageBlock::Text(TextBlock::from_complete("original prompt text"))],
            None,
        ));
        let mut rx = add_user_dialog(&mut app, "dialog-1", true);

        let outcome =
            execute_user_dialog_action(&mut app, InteractionAction::MoveNext, key(KeyCode::Down));
        assert_eq!(outcome, KeyOutcome::Handled(true));

        let outcome =
            execute_user_dialog_action(&mut app, InteractionAction::Confirm, key(KeyCode::Enter));
        assert_eq!(outcome, KeyOutcome::Handled(true));

        let response = rx.try_recv().expect("dialog should receive a response");
        let model::RequestUserDialogOutcome::Selected(selected) = response.outcome else {
            panic!("expected a selected outcome");
        };
        assert_eq!(selected.option_id, "edit_prompt");
        assert_eq!(app.input.text(), "original prompt text");
    }

    #[test]
    fn cancel_sends_cancelled_outcome() {
        let mut app = App::test_default();
        app.status = AppStatus::Ready;
        let mut rx = add_user_dialog(&mut app, "dialog-1", true);

        let outcome =
            execute_user_dialog_action(&mut app, InteractionAction::Cancel, key(KeyCode::Esc));
        assert_eq!(outcome, KeyOutcome::Handled(true));

        let response = rx.try_recv().expect("dialog should receive a response");
        assert!(matches!(response.outcome, model::RequestUserDialogOutcome::Cancelled));
        assert!(app.input.text().is_empty());
        assert!(!app.turn.pending_interaction_ids.iter().any(|id| id == "dialog-1"));
    }

    #[test]
    fn move_previous_clamps_at_first_option() {
        let mut app = App::test_default();
        app.status = AppStatus::Ready;
        let _rx = add_user_dialog(&mut app, "dialog-1", true);

        // Already at index 0; MovePrevious must not underflow.
        let outcome =
            execute_user_dialog_action(&mut app, InteractionAction::MovePrevious, key(KeyCode::Up));
        assert_eq!(outcome, KeyOutcome::Handled(true));

        let (mi, bi) = app.lookup_tool_call("dialog-1").expect("indexed dialog");
        let Some(MessageBlock::UserDialog(dialog)) =
            app.transcript.messages.get(mi).and_then(|m| m.blocks.get(bi))
        else {
            panic!("expected user dialog block");
        };
        assert_eq!(dialog.selected_index, 0);
    }
}
