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
use super::{App, MessageBlock};
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
        app.sync_after_message_blocks_changed(mi);
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
        dialog.resolve(outcome.clone());
        let _ = response_tx.send(model::RequestUserDialogResponse::new(outcome));
        app.sync_after_message_blocks_changed(mi);
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
    let last_user_text = app.transcript.latest_user_text().map(str::to_owned);
    if let Some(text) = last_user_text {
        app.input.set_text(&text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, AppStatus, ChatMessage, MessageRole, SystemSeverity, TextBlock};
    use crossterm::event::KeyCode;
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

    fn assert_only_context_refreshes(commands: &mut crate::agent::client::CommandReceiver) {
        while let Ok(envelope) = commands.try_recv() {
            assert!(
                matches!(envelope.command, crate::agent::wire::BridgeCommand::GetContextUsage { session_id } if session_id == "session-1"),
                "only context refreshes may accompany turn completion"
            );
        }
    }

    fn admit_dialog(
        app: &mut App,
        request: model::RequestUserDialogRequest,
    ) -> oneshot::Receiver<model::RequestUserDialogResponse> {
        let (response_tx, receiver) = oneshot::channel();
        crate::app::handle_client_event(
            app,
            crate::agent::events::ClientEvent::UserDialogRequest {
                session_id: request.session_id.to_string(),
                request,
                response_tx,
            },
        );
        receiver
    }

    fn assert_dialog_answer(
        receiver: &mut oneshot::Receiver<model::RequestUserDialogResponse>,
        id: &str,
    ) {
        assert_eq!(
            receiver.try_recv().expect("dialog response").outcome,
            model::RequestUserDialogOutcome::Selected(model::SelectedUserDialogOutcome::new(id))
        );
    }

    #[test]
    fn native_expiry_preserves_peer_callback_and_draft_without_fabricating_a_response() {
        use crate::agent::events::ClientEvent;
        use crate::app::test_support::{app_with_connection, assert_pending, press};
        let (mut app, mut commands) = app_with_connection();
        app.status = AppStatus::Ready;
        app.input.set_text("keep this draft");
        let mut expired = admit_dialog(&mut app, dialog_request("expired"));
        let mut peer_request = dialog_request("peer");
        peer_request.options[0] = model::UserDialogOption::new("peer-retry", "Retry peer");
        let mut peer = admit_dialog(&mut app, peer_request);
        press(&mut app, KeyCode::Tab);
        for (session, closed) in [("old-session", false), ("session-1", true)] {
            crate::app::handle_client_event(
                &mut app,
                ClientEvent::InteractionCancelled {
                    session_id: session.into(),
                    interaction_id: "expired".into(),
                },
            );
            if closed {
                assert!(matches!(expired.try_recv(), Err(oneshot::error::TryRecvError::Closed)));
            } else {
                assert_pending(&mut expired);
            }
            assert_pending(&mut peer);
        }
        let (mi, bi) = app.lookup_tool_call("expired").expect("expired dialog");
        let MessageBlock::UserDialog(dialog) = &app.transcript.messages[mi].blocks[bi] else {
            panic!("dialog");
        };
        assert_eq!(dialog.outcome, Some(model::RequestUserDialogOutcome::Cancelled));
        crate::app::handle_client_event(
            &mut app,
            ClientEvent::InteractionCancelled {
                session_id: "session-1".into(),
                interaction_id: "expired".into(),
            },
        );
        assert_pending(&mut peer);
        press(&mut app, KeyCode::Enter);
        assert_dialog_answer(&mut peer, "peer-retry");
        assert_eq!(app.input.text(), "keep this draft");
        assert!(commands.try_recv().is_err());
    }

    #[test]
    fn dialog_keys_deliver_default_boundary_choices_and_cancellation() {
        use crate::app::test_support::{app_with_connection, press};
        for (keys, selected) in [
            (vec![KeyCode::Enter], Some("retry_fallback")),
            (vec![KeyCode::Up, KeyCode::Up, KeyCode::Enter], Some("retry_fallback")),
            (vec![KeyCode::Down, KeyCode::Down, KeyCode::Enter], Some("edit_prompt")),
            (vec![KeyCode::Esc], None),
        ] {
            let (mut app, mut commands) = app_with_connection();
            app.status = AppStatus::Ready;
            app.input.set_text("existing draft");
            let mut receiver = admit_dialog(&mut app, dialog_request("choice"));
            press(&mut app, KeyCode::Tab);
            for key in keys {
                press(&mut app, key);
            }
            if let Some(id) = selected {
                assert_dialog_answer(&mut receiver, id);
            } else {
                assert_eq!(
                    receiver.try_recv().expect("cancelled").outcome,
                    model::RequestUserDialogOutcome::Cancelled
                );
            }
            assert_eq!(app.input.text(), "existing draft");
            assert!(commands.try_recv().is_err(), "choice must not submit or cancel a prompt");
            press(&mut app, KeyCode::Backspace);
            assert_eq!(app.input.text(), "existing draf", "composer is usable after settlement");
        }
    }

    #[test]
    fn edit_prompt_recovers_latest_submitted_unicode_text_and_submits_only_the_revision() {
        use crate::agent::wire::BridgeCommand;
        use crate::app::test_support::{app_with_connection, deliver_update, press};
        let (mut app, mut commands) = app_with_connection();
        app.status = AppStatus::Ready;
        for text in ["older prompt", "latest\nGr\u{fc}\u{df}e \u{1f680}"] {
            app.input.set_text(text);
            press(&mut app, KeyCode::Enter);
            let BridgeCommand::Prompt { session_id, chunks, .. } =
                commands.try_recv().expect("submitted prompt").command
            else {
                panic!("prompt");
            };
            assert_eq!(session_id, "session-1");
            assert_eq!(chunks[0].value.as_str(), Some(text));
            assert_only_context_refreshes(&mut commands);
            deliver_update(
                &mut app,
                model::SessionUpdate::AgentMessageChunk(model::ContentChunk::new(
                    model::ContentBlock::Text(model::TextContent::new("intervening answer")),
                )),
            );
            crate::app::handle_client_event(
                &mut app,
                crate::agent::events::ClientEvent::TurnComplete {
                    session_id: "session-1".into(),
                    queued_turn_count: None,
                    terminal_reason: None,
                },
            );
        }
        assert_only_context_refreshes(&mut commands);
        app.push_message_tracked(ChatMessage::new(
            MessageRole::System(Some(SystemSeverity::Info)),
            vec![MessageBlock::Text(TextBlock::from_complete("intervening notice"))],
            None,
        ));
        let mut receiver = admit_dialog(&mut app, dialog_request("revise"));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert_dialog_answer(&mut receiver, "edit_prompt");
        assert_eq!(app.input.text(), "latest\nGr\u{fc}\u{df}e \u{1f680}");
        assert!(commands.try_recv().is_err(), "selecting edit must not submit or switch models");
        press(&mut app, KeyCode::Backspace);
        press(&mut app, KeyCode::Backspace);
        assert_eq!(app.input.text(), "latest\nGr\u{fc}\u{df}e");
        press(&mut app, KeyCode::Enter);
        let BridgeCommand::Prompt { session_id, chunks, .. } =
            commands.try_recv().expect("revised prompt").command
        else {
            panic!("prompt");
        };
        assert_eq!(session_id, "session-1");
        assert_eq!(chunks[0].value.as_str(), Some("latest\nGr\u{fc}\u{df}e"));
        assert!(commands.try_recv().is_err(), "only one revised prompt");
    }

    #[test]
    fn duplicate_dialog_rejects_only_its_own_callback_and_keeps_both_original_choices_usable() {
        use crate::app::test_support::{app_with_connection, assert_pending, press};
        let (mut app, mut commands) = app_with_connection();
        app.status = AppStatus::Ready;
        let mut original_request = dialog_request("original");
        original_request.options[1] =
            model::UserDialogOption::new("original-second", "Original alternate");
        let mut original = admit_dialog(&mut app, original_request);
        let mut peer_request = dialog_request("peer");
        peer_request.options[0] = model::UserDialogOption::new("peer-first", "Peer default");
        let mut peer = admit_dialog(&mut app, peer_request);
        let mut duplicate_request = dialog_request("original");
        duplicate_request.options =
            vec![model::UserDialogOption::new("duplicate-only", "Duplicate choice")];
        duplicate_request.payload.original_model = "duplicate-model".into();
        let mut duplicate = admit_dialog(&mut app, duplicate_request);
        assert_eq!(
            duplicate.try_recv().expect("duplicate refused").outcome,
            model::RequestUserDialogOutcome::Cancelled
        );
        assert_pending(&mut original);
        assert_pending(&mut peer);
        let (mi, bi) = app.lookup_tool_call("original").expect("original retained");
        let MessageBlock::UserDialog(dialog) = &app.transcript.messages[mi].blocks[bi] else {
            panic!("dialog");
        };
        assert_eq!(dialog.payload.original_model, "claude-opus-4-8");
        assert_eq!(dialog.options[1].label, "Original alternate");
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert_dialog_answer(&mut original, "original-second");
        assert_pending(&mut peer);
        press(&mut app, KeyCode::Enter);
        assert_dialog_answer(&mut peer, "peer-first");
        assert!(commands.try_recv().is_err());
    }

    fn dialog_rows(app: &mut App, width: u16, pending: bool) -> Vec<ratatui::text::Line<'static>> {
        let (mi, _) = app.lookup_tool_call("visible-dialog").expect("dialog");
        let live = crate::ui::inline_chat_rows::serialize_live_rows_with_boundaries_excluding(
            app,
            width,
            &std::collections::BTreeSet::default(),
            None,
        );
        let segments: Vec<_> =
            live.segments().iter().filter(|segment| segment.msg_idx == mi).collect();
        assert!(!segments.is_empty(), "dialog has its own live region");
        if pending {
            assert!(
                segments.iter().all(|segment| !segment.commit_ready),
                "unresolved choices stay mutable"
            );
        }
        segments
            .into_iter()
            .flat_map(|segment| live.segment_rows(segment).expect("own rows").to_vec())
            .collect()
    }

    fn assert_dialog_selection(rows: &[ratatui::text::Line<'_>], selected: Option<&str>) {
        use ratatui::style::Modifier;
        for marker in ["FIRST", "SECOND"] {
            let span = rows
                .iter()
                .flat_map(|line| &line.spans)
                .find(|span| span.content.contains(marker))
                .expect("distinct option row");
            assert_eq!(
                span.style.add_modifier.contains(Modifier::BOLD),
                selected == Some(marker),
                "highlight belongs to {marker}"
            );
        }
    }

    #[test]
    fn pending_dialog_repaints_its_own_choices_across_focus_and_width_changes_until_resolution() {
        use crate::app::test_support::{app_with_connection, assert_pending, press};
        for cancel in [false, true] {
            let (mut app, mut commands) = app_with_connection();
            app.status = AppStatus::Ready;
            app.input.set_text("draft protects input focus");
            let mut request = dialog_request("visible-dialog");
            request.options = vec![
                model::UserDialogOption::new("first", "FIRST retry with the fallback model"),
                model::UserDialogOption::new("second", "SECOND revise the original prompt"),
            ];
            let mut receiver = admit_dialog(&mut app, request);
            assert_eq!(app.focus_owner(), crate::app::FocusOwner::Input);
            assert_dialog_selection(&dialog_rows(&mut app, 80, true), None);
            press(&mut app, KeyCode::Tab);
            assert_dialog_selection(&dialog_rows(&mut app, 80, true), Some("FIRST"));
            press(&mut app, KeyCode::Down);
            for width in [80, 24, 80] {
                let rows = dialog_rows(&mut app, width, true);
                assert_dialog_selection(&rows, Some("SECOND"));
                let text: String = rows.iter().map(ToString::to_string).collect();
                assert!(text.contains("confirm"));
                assert!(rows.iter().all(|line| line.width() <= usize::from(width)));
            }
            press(&mut app, KeyCode::Tab);
            assert_dialog_selection(&dialog_rows(&mut app, 24, true), None);
            assert_pending(&mut receiver);
            press(&mut app, KeyCode::Tab);
            assert_dialog_selection(&dialog_rows(&mut app, 24, true), Some("SECOND"));
            assert_pending(&mut receiver);
            press(&mut app, if cancel { KeyCode::Esc } else { KeyCode::Enter });
            if cancel {
                assert_eq!(
                    receiver.try_recv().expect("cancel").outcome,
                    model::RequestUserDialogOutcome::Cancelled
                );
            } else {
                assert_dialog_answer(&mut receiver, "second");
            }
            let rows = dialog_rows(&mut app, 24, false);
            let text: String = rows.iter().map(ToString::to_string).collect();
            assert!(text.contains(if cancel { "Cancelled" } else { "SECOND" }));
            assert!(!text.contains("confirm") && !text.contains("Tab to focus"));
            assert!(!text.contains("FIRST"));
            assert!(
                !rows
                    .iter()
                    .flat_map(|line| &line.spans)
                    .any(|span| span.content.contains("SECOND")
                        && span.style.add_modifier.contains(ratatui::style::Modifier::BOLD))
            );
            assert_eq!(app.input.text(), "draft protects input focus");
            assert!(commands.try_recv().is_err());
        }
    }
}
