// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use super::{App, AppStatus, ChatMessage, MessageBlock, MessageRole, TextBlock};
use crate::agent::model;
use crate::app::slash;

pub(super) fn submit_input(app: &mut App) {
    if !app.composer_access().can_submit() {
        return;
    }

    // Dismiss any open mention dropdown
    app.mention = None;
    app.slash.clear();
    app.subagent = None;

    // No connection yet - can't submit
    let text = app.input.text();
    let inline_pastes = app.input.inline_pastes();
    if text.trim().is_empty() {
        return;
    }

    let submission = slash::ResolvedSubmission::resolve(text);
    let has_active_or_queued_turn = matches!(app.status, AppStatus::Thinking | AppStatus::Running)
        || !app.pending_user_messages.is_empty();
    if has_active_or_queued_turn && submission.is_prompt() {
        dispatch_active_turn_prompt(app, submission.into_text(), inline_pastes);
        return;
    }
    if (app.is_agent_turn_active() || !app.pending_user_messages.is_empty())
        && submission.class().requires_idle_turn()
    {
        let label = submission.blocked_label();
        crate::app::events::push_active_turn_submission_blocked_notice(app, &label);
        tracing::debug!(
            target: crate::logging::targets::APP_INPUT,
            event_name = "submit_blocked_by_active_turn",
            message = "submission rejected while agent turn is active",
            outcome = "blocked",
            submission = %label,
        );
        return;
    }

    if submission.is_btw() && submission.class() != slash::SubmissionClass::Invalid {
        if app.btw.is_full() {
            crate::app::events::push_submission_feedback(
                app,
                super::SystemSeverity::Error,
                &format!(
                    "Too many outstanding BTW questions (maximum {}). Wait for one to finish.",
                    super::state::BtwRequests::CAPACITY
                ),
            );
            return;
        }
        if !inline_pastes.is_empty() || !app.pending_images.is_empty() {
            crate::app::events::push_submission_feedback(
                app,
                super::SystemSeverity::Error,
                "BTW questions are text-only. Remove paste and image attachments before submitting.",
            );
            return;
        }
    }

    dispatch_submission(app, submission, inline_pastes);
}

pub(super) fn request_cancel(app: &mut App) -> Result<(), String> {
    if !matches!(app.status, AppStatus::Thinking | AppStatus::Running) {
        return Ok(());
    }

    if app.turn.cancel_requested {
        return Ok(());
    }

    let Some(ref conn) = app.session_runtime.conn else {
        return Err("not connected yet".to_owned());
    };
    let Some(sid) = app.session_runtime.session_id.clone() else {
        return Err("no active session".to_owned());
    };

    let session_id = sid.to_string();
    conn.cancel(session_id.clone()).map_err(|e| e.to_string())?;
    crate::app::events::handle_local_cancel_enqueued(app);
    tracing::info!(
        target: crate::logging::targets::APP_INPUT,
        event_name = "turn_cancel_requested",
        message = "turn cancel requested",
        outcome = "success",
        session_id = %session_id,
    );
    Ok(())
}

fn dispatch_submission(
    app: &mut App,
    submission: slash::ResolvedSubmission,
    inline_pastes: Vec<String>,
) {
    if slash::try_handle_submission(app, &submission) {
        app.input.clear();
        return;
    }
    dispatch_prompt_turn(app, &submission.into_text(), inline_pastes);
}

fn dispatch_prompt_turn(app: &mut App, text: &str, inline_pastes: Vec<String>) {
    if send_prompt_turn(app, text, app.pending_images.clone(), inline_pastes) {
        app.input.clear();
        app.pending_images.clear();
    }
}

/// Send the launch prompt through normal queue admission while preserving a draft
/// the user may already have typed during connection.
pub(super) fn maybe_submit_initial_prompt(app: &mut App) {
    if app.surface_mode != super::SurfaceMode::Chat
        || app.update_prompt.is_some()
        || !matches!(app.status, AppStatus::Ready)
        || !app.composer_access().can_submit()
    {
        return;
    }
    let Some(prompt) = app.startup.take_initial_prompt() else { return };
    if prompt.trim().is_empty() || send_prompt_turn(app, &prompt, Vec::new(), Vec::new()) {
        return;
    }
    // Keep a failed launch prompt recoverable without overwriting a draft.
    if app.input.text().is_empty() {
        app.input.set_text(&prompt);
    } else {
        crate::app::events::push_submission_feedback(
            app,
            super::SystemSeverity::Error,
            &format!("Initial prompt could not be sent: {prompt}"),
        );
    }
}

fn send_prompt_turn(
    app: &mut App,
    text: &str,
    images: Vec<super::clipboard_image::ImageAttachment>,
    inline_pastes: Vec<String>,
) -> bool {
    let Some(conn) = app.session_runtime.conn.clone() else { return false };
    let Some(sid) = app.session_runtime.session_id.clone() else {
        return false;
    };
    let input_chars = text.chars().count();
    let session_id = sid.to_string();
    let message_uuid = uuid::Uuid::new_v4().to_string();

    // Queue admission is the commit point: retain the complete composer until
    // the bridge accepts it, and do not create a transcript turn on failure.
    let resp = match conn.prompt_with_images_and_pastes(
        session_id.clone(),
        message_uuid.clone(),
        text.to_owned(),
        images,
        inline_pastes,
    ) {
        Ok(resp) => resp,
        Err(error) => {
            crate::app::events::handle_local_prompt_dispatch_error(app, &error.to_string());
            return false;
        }
    };
    let _ = app.finalize_in_progress_tool_calls(model::ToolCallStatus::Failed);

    let user_blocks = vec![MessageBlock::Text(TextBlock::from_complete(text))];

    app.push_message_tracked(ChatMessage::new(MessageRole::User, user_blocks, None));
    // Create empty assistant message immediately -- message.rs shows thinking indicator
    app.push_message_tracked(ChatMessage::new(MessageRole::Assistant, Vec::new(), None));
    app.bind_active_turn_assistant_to_tail();
    app.enforce_history_retention_tracked();
    app.status = AppStatus::Thinking;

    app.session_runtime.prompt_suggestion = None;
    crate::app::session_runtime::request_context_usage_refresh(app);
    tracing::info!(
        target: crate::logging::targets::APP_INPUT,
        event_name = "prompt_dispatched",
        message = "prompt dispatched to the bridge",
        outcome = "success",
        session_id = %session_id,
        message_uuid = %message_uuid,
        input_chars,
        stop_reason = ?resp.stop_reason,
    );
    true
}

fn dispatch_active_turn_prompt(app: &mut App, text: String, inline_pastes: Vec<String>) {
    let Some(conn) = app.session_runtime.conn.clone() else {
        return;
    };
    let Some(session_id) = app.session_runtime.session_id.clone() else {
        return;
    };
    let message_uuid = uuid::Uuid::new_v4().to_string();
    let images = app.pending_images.clone();
    let pending =
        super::PendingUserMessage::sending(message_uuid.clone(), text.clone(), images.clone());
    match app.pending_user_messages.try_push_sending(pending) {
        Ok(()) => {}
        Err(super::PendingUserMessageInsertError::AtCapacity(_)) => return,
        Err(super::PendingUserMessageInsertError::DuplicateUuid(_)) => {
            tracing::warn!(
                target: crate::logging::targets::APP_INPUT,
                event_name = "active_turn_prompt_duplicate_uuid",
                message = "newly generated active-turn prompt UUID already exists",
                outcome = "ignored",
                session_id = %session_id,
                message_uuid = %message_uuid,
            );
            return;
        }
    }

    match conn.prompt_with_images_and_pastes(
        session_id.to_string(),
        message_uuid.clone(),
        text,
        images,
        inline_pastes,
    ) {
        Ok(_) => {
            app.session_runtime.prompt_suggestion = None;
            app.input.clear();
            app.pending_images.clear();
            app.request_active_surface_repaint();
            tracing::info!(
                target: crate::logging::targets::APP_INPUT,
                event_name = "active_turn_prompt_dispatched",
                message = "active-turn prompt dispatched to the bridge",
                outcome = "sending",
                session_id = %session_id,
                message_uuid = %message_uuid,
                pending_message_count = app.pending_user_messages.len(),
            );
        }
        Err(error) => {
            let _ = app.pending_user_messages.remove(&message_uuid);
            crate::app::events::push_submission_feedback(
                app,
                super::SystemSeverity::Error,
                &format!("Queued message could not be sent: {error}"),
            );
            tracing::warn!(
                target: crate::logging::targets::APP_INPUT,
                event_name = "active_turn_prompt_dispatch_failed",
                message = "active-turn prompt could not enter the bridge command queue",
                outcome = "failure",
                session_id = %session_id,
                message_uuid = %message_uuid,
                error = %error,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::events::ClientEvent;
    use crate::agent::wire::BridgeCommand;
    use crate::app::{FullscreenView, SurfaceMode};

    fn app_with_connection() -> (App, crate::agent::client::CommandReceiver) {
        let mut app = App::test_default();
        let (connection, rx) = crate::agent::client::AgentConnection::test_channel();
        app.session_runtime.conn = Some(std::rc::Rc::new(connection));
        app.session_runtime.session_id = Some(model::SessionId::new("session-1"));
        (app, rx)
    }

    #[test]
    fn initial_prompt_waits_for_session_selection_and_preserves_a_typed_draft() {
        use clap::Parser;
        let cli = crate::Cli::try_parse_from(["claude-rs", "--resume", "--", "initial prompt"])
            .expect("CLI");
        let (mut app, mut rx) = app_with_connection();
        app.startup = crate::app::state::StartupState::from_cli(&cli);
        app.input.set_text("draft typed while connecting");
        app.status = AppStatus::Ready;
        maybe_submit_initial_prompt(&mut app);
        assert!(rx.try_recv().is_err(), "picker has not selected a session");
        app.startup.complete_launch();
        app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::Config);
        maybe_submit_initial_prompt(&mut app);
        assert!(rx.try_recv().is_err(), "fullscreen dialog is still open");
        app.surface_mode = SurfaceMode::Chat;
        maybe_submit_initial_prompt(&mut app);
        let envelope = rx.try_recv().expect("initial prompt");
        let BridgeCommand::Prompt { chunks, session_id, .. } = envelope.command else {
            panic!("prompt")
        };
        assert_eq!(session_id, "session-1");
        assert_eq!(chunks[0].value, serde_json::json!("initial prompt"));
        assert_eq!(app.input.text(), "draft typed while connecting");
        app.status = AppStatus::Ready;
        maybe_submit_initial_prompt(&mut app);
        while let Ok(envelope) = rx.try_recv() {
            assert!(
                !matches!(envelope.command, BridgeCommand::Prompt { .. }),
                "prompt is sent only once"
            );
        }
    }

    #[test]
    fn user_paragraph_spacing_survives_submission_queueing_and_paste_expansion() {
        for queued in [false, true] {
            for pasted in [false, true] {
                let (mut app, mut rx) = app_with_connection();
                app.status = if queued { AppStatus::Running } else { AppStatus::Ready };
                if queued {
                    app.push_message_tracked(ChatMessage::new(
                        MessageRole::Assistant,
                        Vec::new(),
                        None,
                    ));
                    app.bind_active_turn_assistant_to_tail();
                }
                let source = if pasted {
                    format!("hello\n\nhow are you\n{}", "long pasted paragraph ".repeat(80))
                } else {
                    "hello\n\nhow are you".to_owned()
                };
                if pasted {
                    app.input.insert_paste_block(&source);
                    assert!(app.input.lines()[0].starts_with("[Pasted Text "));
                } else {
                    app.input.set_text(&source);
                }
                submit_input(&mut app);

                let BridgeCommand::Prompt { message_uuid, chunks, inline_pastes, .. } =
                    rx.try_recv().expect("prompt dispatched").command
                else {
                    panic!("expected prompt command");
                };
                assert_eq!(chunks.len(), 1);
                assert_eq!(chunks[0].value.as_str(), Some(source.as_str()));
                assert_eq!(inline_pastes, if pasted { vec![source.clone()] } else { Vec::new() });
                assert!(app.input.is_empty());
                if queued {
                    assert_eq!(
                        app.pending_user_messages.iter().next().expect("pending prompt").text,
                        source
                    );
                    crate::app::events::handle_client_event(
                        &mut app,
                        ClientEvent::UserMessageQueued {
                            session_id: "session-1".to_owned(),
                            message_uuid: message_uuid.clone(),
                        },
                    );
                    crate::app::events::handle_client_event(
                        &mut app,
                        ClientEvent::UserMessageStarted {
                            session_id: "session-1".to_owned(),
                            message_uuid,
                            source: crate::agent::types::UserMessageStartSource::StreamEvent,
                        },
                    );
                    assert!(app.pending_user_messages.is_empty());
                }
                let user = app
                    .transcript
                    .messages
                    .iter()
                    .find(|msg| matches!(msg.role, MessageRole::User))
                    .expect("user inserted");
                let MessageBlock::Text(block) = &user.blocks[0] else {
                    panic!("expected user text");
                };
                assert_eq!(block.text, source);
                let rows =
                    crate::ui::inline_chat_rows::serialize_live_rows_with_boundaries_excluding(
                        &mut app,
                        80,
                        &std::collections::BTreeSet::new(),
                    );
                let texts: Vec<_> =
                    rows.rows().iter().map(|line| line.to_string().trim_end().to_owned()).collect();
                let start = texts.iter().position(|line| line == "User").expect("user label");
                assert_eq!(&texts[start..start + 4], ["User", "hello", "", "how are you"]);
            }
        }
    }

    #[test]
    fn btw_submission_and_result_follow_the_real_command_event_workflow() {
        let (mut app, mut rx) = app_with_connection();
        app.status = AppStatus::Running;
        app.input.set_text("/btw   Why  does this work?\nInclude this line.  ");

        submit_input(&mut app);

        assert!(app.input.text().is_empty());
        assert!(matches!(app.status, AppStatus::Running));
        assert!(app.pending_user_messages.is_empty());
        assert!(app.transcript.messages.is_empty());
        let BridgeCommand::SideQuestion { session_id, btw_id, question } =
            rx.try_recv().expect("side question should reach the bridge").command
        else {
            panic!("expected side_question command");
        };
        assert_eq!(session_id, "session-1");
        assert_eq!(question, "Why  does this work?\nInclude this line.");
        assert!(matches!(
            app.btw.get(&btw_id).map(|item| &item.state),
            Some(super::super::BtwRequestState::Active)
        ));

        crate::app::events::handle_client_event(
            &mut app,
            ClientEvent::BtwResult {
                session_id: "session-1".to_owned(),
                btw_id: btw_id.clone(),
                question: question.clone(),
                answer: "Because the complete remainder is preserved.".to_owned(),
                metadata: crate::agent::wire::SideQuestionMetadata {
                    synthetic: false,
                    refusal_fallback: None,
                },
            },
        );

        assert!(app.btw.get(&btw_id).is_none());
        assert_eq!(app.transcript.messages.len(), 1);
        let Some(MessageBlock::BtwExchange(exchange)) = app.transcript.messages[0].blocks.first()
        else {
            panic!("expected a completed BTW transcript exchange");
        };
        assert_eq!(exchange.question, question);
        assert_eq!(exchange.answer, "Because the complete remainder is preserved.");
        assert!(rx.try_recv().is_err(), "a BTW result must not enter the main prompt path");
    }

    #[test]
    fn multiple_btw_submissions_stay_independent_and_failure_advances_local_fifo() {
        let (mut app, mut rx) = app_with_connection();
        for input in ["/btw duplicate question", "/btw duplicate question"] {
            app.input.set_text(input);
            submit_input(&mut app);
        }

        let first = rx.try_recv().expect("first command").command;
        let BridgeCommand::SideQuestion { btw_id: first_id, question: first_question, .. } = first
        else {
            panic!("expected first side question");
        };
        assert!(rx.try_recv().is_err(), "waiting work must remain in Rust");
        let second = app
            .btw
            .status_items()
            .into_iter()
            .find(|item| matches!(item.state, super::super::BtwRequestState::Waiting))
            .expect("waiting request");
        let second_id = second.id.clone();
        let second_question = second.question.clone();
        assert_ne!(first_id, second_id);
        assert_eq!(first_question, second_question);
        assert!(matches!(
            app.btw.get(&first_id).map(|item| &item.state),
            Some(super::super::BtwRequestState::Active)
        ));
        assert!(matches!(
            app.btw.get(&second_id).map(|item| &item.state),
            Some(super::super::BtwRequestState::Waiting)
        ));

        crate::app::events::handle_client_event(
            &mut app,
            ClientEvent::BtwFailed {
                session_id: "session-1".to_owned(),
                btw_id: first_id.clone(),
                question: first_question,
                error: "no answer".to_owned(),
            },
        );

        let BridgeCommand::SideQuestion {
            btw_id: dispatched_id,
            question: dispatched_question,
            ..
        } = rx.try_recv().expect("failure dispatches next item").command
        else {
            panic!("expected second side question");
        };
        assert_eq!(dispatched_id, second_id);
        assert_eq!(dispatched_question, second_question);
        assert!(rx.try_recv().is_err());
        assert!(matches!(
            app.btw.get(&first_id).map(|item| &item.state),
            Some(super::super::BtwRequestState::Failed { .. })
        ));
        assert!(matches!(
            app.btw.get(&second_id).map(|item| &item.state),
            Some(super::super::BtwRequestState::Active)
        ));
        assert!(
            app.transcript.messages.is_empty(),
            "a failed side question must not create a card"
        );
    }

    fn next_btw(rx: &mut crate::agent::client::CommandReceiver) -> (String, String) {
        let BridgeCommand::SideQuestion { btw_id, question, .. } =
            rx.try_recv().expect("side question command").command
        else {
            panic!("expected side question");
        };
        (btw_id, question)
    }

    fn deliver_btw_result(app: &mut App, id: &str, question: &str) {
        crate::app::events::handle_client_event(
            app,
            ClientEvent::BtwResult {
                session_id: "session-1".to_owned(),
                btw_id: id.to_owned(),
                question: question.to_owned(),
                answer: "answer".to_owned(),
                metadata: crate::agent::wire::SideQuestionMetadata {
                    synthetic: false,
                    refusal_fallback: None,
                },
            },
        );
    }

    fn start_main_turn(app: &mut App, rx: &mut crate::agent::client::CommandReceiver) {
        app.input.set_text("main task");
        submit_input(app);
        assert!(matches!(
            rx.try_recv().expect("main prompt").command,
            BridgeCommand::Prompt { .. }
        ));
        while let Ok(envelope) = rx.try_recv() {
            assert!(!matches!(
                envelope.command,
                BridgeCommand::Prompt { .. } | BridgeCommand::SideQuestion { .. }
            ));
        }
        assert!(app.active_turn_assistant_idx().is_some());
    }

    fn deliver_main_text(app: &mut App, text: &str) {
        crate::app::events::handle_client_event(
            app,
            ClientEvent::SessionUpdate {
                session_id: "session-1".to_owned(),
                update: model::SessionUpdate::AgentMessageChunk(model::ContentChunk::new(
                    model::ContentBlock::Text(model::TextContent::new(text)),
                )),
            },
        );
    }

    fn finish_main_turn(app: &mut App) {
        crate::app::events::handle_client_event(
            app,
            ClientEvent::TurnComplete {
                session_id: "session-1".to_owned(),
                queued_turn_count: Some(0),
                terminal_reason: None,
            },
        );
        assert!(matches!(app.status, AppStatus::Ready));
        assert_eq!(app.active_turn_assistant_idx(), None);
    }

    fn rendered_btw_card(app: &mut App, width: u16) -> Vec<ratatui::text::Line<'static>> {
        let serialized = crate::ui::inline_chat_rows::serialize_live_rows_with_boundaries_excluding(
            app,
            width,
            &std::collections::BTreeSet::new(),
        );
        let rows = serialized.rows();
        let start = rows
            .iter()
            .position(|line| line.to_string().starts_with('╭'))
            .expect("card top border");
        let end = rows
            .iter()
            .skip(start)
            .position(|line| line.to_string().starts_with('╰'))
            .expect("card bottom border")
            + start;
        rows[start..=end].to_vec()
    }

    #[test]
    fn btw_completion_enters_active_stream_and_history_before_subsequent_agent_output() {
        use crate::ui::inline_chat_rows::{
            LiveRowBoundaryKind, serialize_live_rows_with_boundaries_excluding,
        };
        use std::collections::BTreeSet;

        let (mut app, mut rx) = app_with_connection();
        start_main_turn(&mut app, &mut rx);
        let owner_idx = app.active_turn_assistant_idx().expect("active owner");
        let owner_id = app.transcript.messages[owner_idx].id;
        deliver_main_text(&mut app, "Before the side answer.");
        app.input.set_text("/btw Why is this safe?");
        submit_input(&mut app);
        let (id, question) = next_btw(&mut rx);
        deliver_btw_result(&mut app, &id, &question);

        assert_eq!(
            app.transcript.messages.len(),
            2,
            "BTW must enter the owner, not a separate trailing message"
        );
        assert_eq!(app.active_turn_assistant_idx(), Some(owner_idx));
        assert_eq!(app.transcript.messages[owner_idx].id, owner_id);
        assert!(matches!(app.status, AppStatus::Running));
        assert!(app.pending_user_messages.is_empty());
        assert!(
            rx.try_recv().is_err(),
            "inserting presentation content must not send SDK commands"
        );

        let serialized =
            serialize_live_rows_with_boundaries_excluding(&mut app, 80, &BTreeSet::new());
        let card = serialized
            .segments()
            .iter()
            .find(|segment| segment.kind == LiveRowBoundaryKind::AssistantBtw)
            .expect("inline card segment");
        assert_eq!(card.msg_idx, owner_idx);
        assert_eq!(card.block_idx, Some(1));
        assert!(card.commit_ready);
        assert!(
            card.end_row <= serialized.stable_row_count(),
            "the card must be eligible for normal history insertion while the turn runs"
        );
        let committed_ids: BTreeSet<_> = serialized
            .segments()
            .iter()
            .take_while(|segment| segment.commit_ready)
            .flat_map(|segment| segment.ids.iter().cloned())
            .collect();
        let committed_rows = serialized.rows()[..serialized.stable_row_count()].to_vec();

        deliver_main_text(&mut app, "After the side answer.");
        let blocks = &app.transcript.messages[owner_idx].blocks;
        assert!(
            matches!(&blocks[0], MessageBlock::Text(text) if text.text == "Before the side answer.")
        );
        assert!(
            matches!(&blocks[1], MessageBlock::BtwExchange(exchange) if exchange.question == question)
        );
        assert!(
            matches!(&blocks[2], MessageBlock::Text(text) if text.text == "After the side answer.")
        );
        let all = serialize_live_rows_with_boundaries_excluding(&mut app, 80, &BTreeSet::new());
        let text = all.rows().iter().map(ToString::to_string).collect::<Vec<_>>().join("\n");
        assert!(
            text.find("Before the side answer.").expect("before")
                < text.find("Claude · BTW").expect("card")
        );
        assert!(
            text.find("Claude · BTW").expect("card")
                < text.find("After the side answer.").expect("after")
        );
        let mutable = serialize_live_rows_with_boundaries_excluding(&mut app, 80, &committed_ids);
        assert!(
            mutable.rows().iter().any(|line| line.to_string().contains("After the side answer."))
        );
        assert!(
            !mutable.rows().iter().any(|line| line.to_string().contains("Claude · BTW")),
            "committed cards must not duplicate in the mutable region"
        );
        let recombined: Vec<_> =
            committed_rows.into_iter().chain(mutable.rows().iter().cloned()).collect();
        assert_eq!(
            recombined,
            all.rows(),
            "history plus live rows must preserve the full transcript layout"
        );

        deliver_btw_result(&mut app, &id, &question);
        assert_eq!(
            app.transcript.messages[owner_idx].blocks.len(),
            3,
            "duplicate result must not reinsert the card"
        );
        finish_main_turn(&mut app);
        start_main_turn(&mut app, &mut rx);
        deliver_main_text(&mut app, "A new main turn.");
        let rows = serialize_live_rows_with_boundaries_excluding(&mut app, 80, &BTreeSet::new());
        let text = rows.rows().iter().map(ToString::to_string).collect::<Vec<_>>().join("\n");
        assert_eq!(text.matches("Claude · BTW").count(), 1);
        assert!(
            text.find("Claude · BTW").expect("card")
                < text.find("A new main turn.").expect("new turn")
        );
    }

    #[test]
    fn btw_card_design_is_identical_when_idle_active_or_completed_after_main_turn() {
        let (mut idle, mut idle_rx) = app_with_connection();
        idle.input.set_text("/btw Same **question**?");
        submit_input(&mut idle);
        let (id, question) = next_btw(&mut idle_rx);
        deliver_btw_result(&mut idle, &id, &question);
        assert!(matches!(idle.transcript.messages[0].role, MessageRole::System(None)));

        for complete_main_first in [false, true] {
            let (mut app, mut rx) = app_with_connection();
            start_main_turn(&mut app, &mut rx);
            app.input.set_text("/btw Same **question**?");
            submit_input(&mut app);
            let (id, question) = next_btw(&mut rx);
            if complete_main_first {
                deliver_main_text(&mut app, "Finished main response.");
                finish_main_turn(&mut app);
            }
            deliver_btw_result(&mut app, &id, &question);
            assert_eq!(app.transcript.messages.len(), if complete_main_first { 3 } else { 2 });
            for width in [80, 24, 8, 42, 80] {
                assert_eq!(
                    rendered_btw_card(&mut app, width),
                    rendered_btw_card(&mut idle, width),
                    "same card content and styles at width {width}, main finished: {complete_main_first}"
                );
            }
            if !complete_main_first {
                deliver_main_text(&mut app, "First main chunk after card.");
                assert!(
                    matches!(&app.transcript.messages[1].blocks[..], [MessageBlock::BtwExchange(_), MessageBlock::Text(text)] if text.text == "First main chunk after card.")
                );
                finish_main_turn(&mut app);
                assert_eq!(rendered_btw_card(&mut app, 80), rendered_btw_card(&mut idle, 80));
            }
        }
    }

    #[test]
    fn btw_success_advances_fifo_without_blocking_main_input_and_ignores_duplicate_results() {
        let (mut app, mut rx) = app_with_connection();
        app.status = AppStatus::Running;
        for question in ["first", "second", "third"] {
            app.input.set_text(&format!("/btw {question}"));
            submit_input(&mut app);
        }
        let (first_id, first_question) = next_btw(&mut rx);
        assert!(rx.try_recv().is_err());

        app.input.set_text("normal main prompt");
        submit_input(&mut app);
        assert!(matches!(
            rx.try_recv().expect("normal input remains available").command,
            BridgeCommand::Prompt { .. }
        ));
        assert_eq!(app.pending_user_messages.len(), 1);
        let main_messages = app.transcript.messages.len();

        deliver_btw_result(&mut app, &first_id, &first_question);
        let (second_id, second_question) = next_btw(&mut rx);
        assert_eq!(second_question, "second");
        assert_eq!(app.transcript.messages.len(), main_messages + 1);
        assert!(matches!(app.status, AppStatus::Running));
        assert_eq!(app.pending_user_messages.len(), 1);
        deliver_btw_result(&mut app, &first_id, &first_question);
        assert!(rx.try_recv().is_err(), "duplicate terminal events must not advance the FIFO");
        assert_eq!(app.transcript.messages.len(), main_messages + 1);

        deliver_btw_result(&mut app, &second_id, &second_question);
        let (third_id, third_question) = next_btw(&mut rx);
        assert_eq!(third_question, "third");
        deliver_btw_result(&mut app, &third_id, &third_question);
        assert_eq!(app.btw.len(), 0);
        assert_eq!(app.transcript.messages.len(), main_messages + 3);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn btw_failure_frees_full_queue_capacity_while_error_row_is_still_visible() {
        let (mut app, mut rx) = app_with_connection();
        for index in 0..super::super::BtwRequests::CAPACITY {
            app.input.set_text(&format!("/btw question {index}"));
            submit_input(&mut app);
        }
        let (failed_id, failed_question) = next_btw(&mut rx);
        assert!(app.btw.is_full());
        assert!(rx.try_recv().is_err());
        crate::app::events::handle_client_event(
            &mut app,
            ClientEvent::BtwFailed {
                session_id: "session-1".to_owned(),
                btw_id: failed_id.clone(),
                question: failed_question.clone(),
                error: "no answer".to_owned(),
            },
        );
        assert_eq!(next_btw(&mut rx).1, "question 1");
        assert!(!app.btw.is_full());

        app.input.set_text("/btw accepted while failure is visible");
        submit_input(&mut app);
        assert!(app.input.text().is_empty());
        assert!(app.btw.is_full());
        assert_eq!(app.btw.len(), super::super::BtwRequests::CAPACITY + 1);
        assert!(
            crate::ui::input_rows::build_btw_status_rows(&app, 120)
                .iter()
                .any(|row| row.to_string().contains("no answer"))
        );

        deliver_btw_result(&mut app, &failed_id, &failed_question);
        assert!(app.transcript.messages.is_empty(), "late success cannot revive a failed item");
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn btw_waiting_results_are_ignored_and_active_question_mismatch_advances_fifo() {
        let (mut app, mut rx) = app_with_connection();
        for question in ["first", "second"] {
            app.input.set_text(&format!("/btw {question}"));
            submit_input(&mut app);
        }
        let (first_id, _) = next_btw(&mut rx);
        let waiting_id = app
            .btw
            .status_items()
            .into_iter()
            .find(|item| item.question == "second")
            .expect("waiting")
            .id
            .clone();
        deliver_btw_result(&mut app, &waiting_id, "second");
        assert!(app.transcript.messages.is_empty());
        assert!(rx.try_recv().is_err());

        deliver_btw_result(&mut app, &first_id, "wrong echo");
        assert_eq!(next_btw(&mut rx), (waiting_id, "second".to_owned()));
        assert!(
            matches!(app.btw.get(&first_id).map(|item| &item.state), Some(super::super::BtwRequestState::Failed { reason, .. }) if reason.contains("protocol error"))
        );
        assert!(app.transcript.messages.is_empty());
    }

    #[test]
    fn btw_send_failure_drains_waiting_work_without_creating_cards_or_leaving_active_state() {
        let (mut app, mut rx) = app_with_connection();
        for question in ["first", "second", "third"] {
            app.input.set_text(&format!("/btw {question}"));
            submit_input(&mut app);
        }
        let (first_id, first_question) = next_btw(&mut rx);
        drop(rx);
        deliver_btw_result(&mut app, &first_id, &first_question);
        assert_eq!(
            app.transcript.messages.len(),
            1,
            "only the already completed item creates a card"
        );
        assert!(!app.btw.has_active());
        assert!(!app.btw.is_full());
        assert_eq!(app.btw.len(), 2);
        assert!(app.btw.status_items().iter().all(|item| matches!(&item.state, super::super::BtwRequestState::Failed { reason, .. } if reason.contains("could not send"))));
    }

    #[test]
    fn btw_conversation_reset_discards_waiters_and_drops_late_results_in_the_same_session() {
        let (mut app, mut rx) = app_with_connection();
        for question in ["old active", "old waiting"] {
            app.input.set_text(&format!("/btw {question}"));
            submit_input(&mut app);
        }
        let (old_id, old_question) = next_btw(&mut rx);
        crate::app::events::handle_client_event(
            &mut app,
            ClientEvent::SessionUpdate {
                session_id: "session-1".to_owned(),
                update: model::SessionUpdate::ConversationReset {
                    new_conversation_id: "conversation-2".to_owned(),
                    trigger: None,
                    timestamp: None,
                    user_message_uuid: None,
                },
            },
        );
        assert_eq!(app.btw.len(), 0);
        let reset_message_count = app.transcript.messages.len();
        app.input.set_text("/btw new question");
        submit_input(&mut app);
        let (new_id, new_question) = next_btw(&mut rx);
        assert_eq!(new_question, "new question");

        deliver_btw_result(&mut app, &old_id, &old_question);
        assert_eq!(app.transcript.messages.len(), reset_message_count);
        assert!(app.btw.get_active(&new_id).is_some());
        assert!(rx.try_recv().is_err(), "old waiting question must never dispatch");
        deliver_btw_result(&mut app, &new_id, &new_question);
        assert_eq!(app.transcript.messages.len(), reset_message_count + 1);
    }

    #[test]
    fn btw_bridge_exit_discards_active_and_waiting_work_and_drops_late_results() {
        let (mut app, mut rx) = app_with_connection();
        for question in ["active", "waiting"] {
            app.input.set_text(&format!("/btw {question}"));
            submit_input(&mut app);
        }
        let (id, question) = next_btw(&mut rx);
        crate::app::events::handle_client_event(
            &mut app,
            ClientEvent::ConnectionFailed("bridge exited".to_owned().into()),
        );
        assert_eq!(app.btw.len(), 0);
        assert!(app.session_runtime.session_id.is_none());
        assert!(matches!(app.status, AppStatus::Error));
        let message_count = app.transcript.messages.len();
        deliver_btw_result(&mut app, &id, &question);
        assert_eq!(app.transcript.messages.len(), message_count);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn btw_capacity_rejection_preserves_the_exact_composer_draft() {
        let (mut app, mut rx) = app_with_connection();
        for index in 0..super::super::BtwRequests::CAPACITY {
            app.btw
                .try_push(format!("btw-{index}"), format!("question {index}"))
                .expect("queue slot");
        }
        app.input.set_text("/btw   keep  all spacing  ");
        let before = app.input.snapshot();

        submit_input(&mut app);

        assert_eq!(app.input.snapshot(), before);
        assert_eq!(app.btw.len(), super::super::BtwRequests::CAPACITY);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn empty_btw_reports_usage_even_when_the_queue_is_full() {
        let (mut app, mut rx) = app_with_connection();
        for index in 0..super::super::BtwRequests::CAPACITY {
            app.btw
                .try_push(format!("btw-{index}"), format!("question {index}"))
                .expect("queue slot");
        }
        app.input.set_text("/btw   \n  ");

        submit_input(&mut app);

        assert!(app.input.text().is_empty());
        let Some(MessageBlock::Text(message)) =
            app.transcript.messages.last().and_then(|message| message.blocks.first())
        else {
            panic!("expected usage feedback");
        };
        assert_eq!(message.text, "Usage: /btw <question>");
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn connecting_submission_preserves_the_draft() {
        let mut app = App::test_default();
        app.status = AppStatus::Connecting;
        app.input.set_text("draft while connecting");

        submit_input(&mut app);

        assert_eq!(app.input.text(), "draft while connecting");
    }

    #[test]
    fn submit_input_while_running_queues_full_payload_without_touching_active_turn() {
        let (mut app, mut rx) = app_with_connection();
        app.status = AppStatus::Running;
        app.session_runtime.prompt_suggestion = Some("previous suggestion".to_owned());
        app.transcript.messages.push(ChatMessage::new(MessageRole::Assistant, Vec::new(), None));
        app.bind_active_turn_assistant(0);
        app.input.set_text("next prompt [Image #1]");
        app.pending_images.push(crate::app::clipboard_image::ImageAttachment {
            data: "aGVsbG8=".to_owned(),
            mime_type: "image/png".to_owned(),
        });

        submit_input(&mut app);

        assert!(app.input.text().is_empty());
        assert!(app.pending_images.is_empty());
        assert!(app.session_runtime.prompt_suggestion.is_none());
        assert!(!app.turn.cancel_requested);
        assert!(matches!(app.status, AppStatus::Running));
        assert_eq!(
            app.transcript.messages.len(),
            1,
            "pending input must not enter the transcript before its correlated start"
        );
        assert_eq!(app.pending_user_messages.len(), 1);
        let pending = app.pending_user_messages.iter().next().expect("one pending user message");
        assert_eq!(pending.text, "next prompt [Image #1]");
        assert_eq!(pending.images.len(), 1);
        assert_eq!(pending.images[0].data, "aGVsbG8=");
        let envelope = rx.try_recv().expect("active-turn prompt should be sent");
        let BridgeCommand::Prompt { session_id, message_uuid, chunks, inline_pastes } =
            envelope.command
        else {
            panic!("expected prompt command");
        };
        assert_eq!(session_id, "session-1");
        assert_eq!(message_uuid, pending.uuid);
        assert_eq!(chunks.len(), 2);
        assert!(inline_pastes.is_empty());
        assert!(rx.try_recv().is_err(), "active-turn prompt must not cancel the current turn");
    }

    #[test]
    fn active_turn_prompt_at_capacity_preserves_composer_without_dispatch_or_notice() {
        let (mut app, mut rx) = app_with_connection();
        app.status = AppStatus::Running;
        app.session_runtime.prompt_suggestion = Some("previous suggestion".to_owned());
        for index in 0..crate::app::state::PendingUserMessages::CAPACITY {
            app.pending_user_messages
                .try_push_sending(super::super::PendingUserMessage::sending(
                    format!("queued-{index}"),
                    format!("queued message {index}"),
                    Vec::new(),
                ))
                .expect("queue slot should be available");
        }
        app.input.set_text("keep this draft [Image #1]");
        app.pending_images.push(crate::app::clipboard_image::ImageAttachment {
            data: "aGVsbG8=".to_owned(),
            mime_type: "image/png".to_owned(),
        });
        let input_before = app.input.snapshot();
        let images_before = app.pending_images.clone();

        submit_input(&mut app);

        assert_eq!(app.input.snapshot(), input_before);
        assert_eq!(app.pending_images, images_before);
        assert_eq!(app.session_runtime.prompt_suggestion.as_deref(), Some("previous suggestion"));
        assert_eq!(
            app.pending_user_messages.len(),
            crate::app::state::PendingUserMessages::CAPACITY
        );
        assert!(app.transcript.messages.is_empty());
        assert!(rx.try_recv().is_err(), "capacity rejection must not reach the bridge");
    }

    #[test]
    fn active_turn_prompt_does_not_change_slash_command_blocking() {
        let (mut app, mut rx) = app_with_connection();
        app.status = AppStatus::Running;
        app.transcript.messages.push(ChatMessage::new(MessageRole::Assistant, Vec::new(), None));
        app.bind_active_turn_assistant(0);
        app.input.set_text("first prompt");

        submit_input(&mut app);
        app.input.set_text("/resume");
        submit_input(&mut app);

        assert_eq!(app.input.text(), "/resume");
        assert_eq!(app.transcript.messages[0].blocks.len(), 1);
        let Some(MessageBlock::Notice(notice)) = app.transcript.messages[0].blocks.first() else {
            panic!("expected updated inline notice");
        };
        assert!(notice.text.text.contains("`/resume`"));
        assert_eq!(app.turn.notice_refs.len(), 1);
        assert!(matches!(
            rx.try_recv().expect("plain prompt should be queued").command,
            BridgeCommand::Prompt { .. }
        ));
        assert!(rx.try_recv().is_err(), "blocked slash command must not be dispatched");
    }

    #[test]
    fn idle_prompt_send_failure_preserves_exact_draft_and_attachments() {
        let (mut app, rx) = app_with_connection();
        drop(rx);
        app.status = AppStatus::Ready;
        app.input.set_text("retry [Image #1]\n");
        app.input.insert_paste_block(&"Unicode 界 🦀\n".repeat(120));
        app.pending_images.push(crate::app::clipboard_image::ImageAttachment {
            data: "aGVsbG8=".to_owned(),
            mime_type: "image/png".to_owned(),
        });
        let before = app.input.snapshot();
        let images_before = app.pending_images.clone();

        submit_input(&mut app);

        assert_eq!(app.input.snapshot(), before);
        assert_eq!(app.pending_images, images_before);
        assert!(app.pending_user_messages.is_empty());
        assert!(matches!(app.status, AppStatus::Error));
        assert!(
            app.transcript
                .messages
                .iter()
                .all(|message| matches!(message.role, MessageRole::System(_)))
        );
        assert!(app.active_turn_assistant_idx().is_none());
    }

    #[test]
    fn active_turn_prompt_send_failure_preserves_exact_draft_and_images() {
        let (mut app, rx) = app_with_connection();
        drop(rx);
        app.status = AppStatus::Running;
        app.session_runtime.prompt_suggestion = Some("previous suggestion".to_owned());
        app.input.set_text("retry [Image #1]");
        app.pending_images.push(crate::app::clipboard_image::ImageAttachment {
            data: "aGVsbG8=".to_owned(),
            mime_type: "image/png".to_owned(),
        });
        let before = app.input.snapshot();
        let images_before = app.pending_images.clone();

        submit_input(&mut app);

        assert_eq!(app.input.snapshot(), before);
        assert_eq!(app.pending_images, images_before);
        assert_eq!(app.session_runtime.prompt_suggestion.as_deref(), Some("previous suggestion"));
        assert!(app.pending_user_messages.is_empty());
        assert!(matches!(app.status, AppStatus::Running));
        let message = app.transcript.messages.last().expect("send failure message");
        assert!(matches!(
            message.role,
            MessageRole::System(Some(super::super::SystemSeverity::Error))
        ));
        let Some(MessageBlock::Text(text)) = message.blocks.last() else {
            panic!("expected send failure text");
        };
        assert!(text.text.contains("could not be sent"));
    }

    #[test]
    fn explicit_cancel_request_is_idempotent() {
        let (mut app, mut rx) = app_with_connection();
        app.status = AppStatus::Running;

        request_cancel(&mut app).expect("first cancel request");
        request_cancel(&mut app).expect("duplicate cancel request");

        assert!(app.turn.cancel_requested);
        let envelope = rx.try_recv().expect("cancel command should be sent");
        assert!(matches!(
            envelope.command, BridgeCommand::CancelTurn { session_id } if session_id == "session-1"
        ));
        assert!(rx.try_recv().is_err(), "duplicate request must not send a second cancel");
    }

    #[test]
    fn submit_input_cancel_command_requests_manual_cancel() {
        let (mut app, mut rx) = app_with_connection();
        app.status = AppStatus::Running;
        app.input.set_text("/cancel");

        submit_input(&mut app);

        assert!(app.input.text().is_empty());
        assert!(app.turn.cancel_requested);
        let envelope = rx.try_recv().expect("cancel command should be sent");
        assert!(matches!(
            envelope.command,
            BridgeCommand::CancelTurn { session_id } if session_id == "session-1"
        ));
    }

    #[test]
    fn local_slash_submit_marks_redraw() {
        let (mut app, _rx) = app_with_connection();
        app.input.set_text("/docs commands");
        app.surface_dirty.chat.repaint = false;

        submit_input(&mut app);

        assert!(app.surface_dirty.chat.repaint);
        assert!(app.input.text().is_empty());
        let Some(last) = app.transcript.messages.last() else {
            panic!("expected docs system message");
        };
        assert!(matches!(last.role, MessageRole::System(Some(super::super::SystemSeverity::Info))));
    }

    #[test]
    fn read_only_command_executes_inline_during_active_turn() {
        let (mut app, mut rx) = app_with_connection();
        app.status = AppStatus::Thinking;
        app.transcript.messages.push(ChatMessage::new(MessageRole::Assistant, Vec::new(), None));
        app.bind_active_turn_assistant(0);
        app.input.set_text("/docs commands");

        submit_input(&mut app);

        assert!(app.input.text().is_empty());
        assert!(matches!(app.status, AppStatus::Thinking));
        let [MessageBlock::Notice(notice)] = app.transcript.messages[0].blocks.as_slice() else {
            panic!("expected inline docs notice");
        };
        assert!(notice.text.text.contains("Docs: Commands"));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn invalid_command_syntax_reports_usage_instead_of_active_turn_block() {
        let (mut app, mut rx) = app_with_connection();
        app.status = AppStatus::Running;
        app.transcript.messages.push(ChatMessage::new(MessageRole::Assistant, Vec::new(), None));
        app.bind_active_turn_assistant(0);
        app.input.set_text("/resume one two");

        submit_input(&mut app);

        assert!(app.input.text().is_empty());
        let [MessageBlock::Notice(notice)] = app.transcript.messages[0].blocks.as_slice() else {
            panic!("expected inline usage notice");
        };
        assert_eq!(notice.text.text, "Usage: /resume [session_id]");
        assert!(!notice.text.text.contains("between agent turns"));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn sdk_promoted_command_is_blocked_during_active_turn() {
        let (mut app, mut rx) = app_with_connection();
        app.status = AppStatus::Running;
        app.sdk_inventory.available_commands =
            vec![model::AvailableCommand::new("/remote-command", "Remote command")];
        app.input.set_text("/remote-command");

        submit_input(&mut app);

        assert_eq!(app.input.text(), "/remote-command");
        assert!(matches!(app.status, AppStatus::Running));
        let Some(MessageBlock::Notice(notice)) =
            app.transcript.messages.last().and_then(|message| message.blocks.first())
        else {
            panic!("expected active-turn block notice");
        };
        assert!(notice.text.text.contains("`/remote-command`"));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn mutating_app_command_is_blocked_during_active_turn() {
        let (mut app, mut rx) = app_with_connection();
        app.status = AppStatus::Running;
        app.input.set_text("/model sonnet");

        submit_input(&mut app);

        assert_eq!(app.input.text(), "/model sonnet");
        assert!(matches!(app.status, AppStatus::Running));
        let Some(MessageBlock::Notice(notice)) =
            app.transcript.messages.last().and_then(|message| message.blocks.first())
        else {
            panic!("expected active-turn block notice");
        };
        assert!(notice.text.text.contains("`/model`"));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn prompt_is_blocked_while_compaction_is_active() {
        let (mut app, mut rx) = app_with_connection();
        app.turn.compaction.begin();
        app.input.set_text("wait for compaction");

        submit_input(&mut app);

        assert_eq!(app.input.text(), "wait for compaction");
        assert!(matches!(app.status, AppStatus::Ready));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn supported_advertised_slash_submit_falls_through_to_prompt_turn() {
        let (mut app, mut rx) = app_with_connection();
        app.session_runtime.prompt_suggestion = Some("previous suggestion".to_owned());
        app.sdk_inventory.available_commands =
            vec![model::AvailableCommand::new("/remote-command", "Remote command")];
        app.input.set_text("/remote-command");

        submit_input(&mut app);

        assert!(app.input.text().is_empty());
        assert!(app.session_runtime.prompt_suggestion.is_none());
        assert!(matches!(app.status, AppStatus::Thinking));
        assert_eq!(app.transcript.messages.len(), 2);
        assert!(matches!(app.transcript.messages[0].role, MessageRole::User));
        assert!(matches!(app.transcript.messages[1].role, MessageRole::Assistant));
        let envelope = rx.try_recv().expect("advertised slash command should be sent");
        match envelope.command {
            BridgeCommand::Prompt { session_id, chunks, .. } => {
                assert_eq!(session_id, "session-1");
                assert_eq!(chunks.len(), 1);
                assert_eq!(chunks[0].kind, "text");
                assert_eq!(
                    chunks[0].value,
                    serde_json::Value::String("/remote-command".to_owned())
                );
            }
            other => panic!("expected prompt command, got {other:?}"),
        }
    }

    #[test]
    fn prompt_submission_preserves_expanded_text_and_inline_paste_provenance() {
        let (mut app, mut rx) = app_with_connection();
        let pasted = "p".repeat(1_001);
        app.input.insert_paste_block(&pasted);

        submit_input(&mut app);

        let envelope = rx.try_recv().expect("prompt should be sent");
        let BridgeCommand::Prompt { chunks, inline_pastes, .. } = envelope.command else {
            panic!("expected prompt command");
        };
        assert_eq!(inline_pastes, vec![pasted.clone()]);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].value, serde_json::Value::String(pasted));
    }

    #[test]
    fn prompt_submission_preserves_interleaved_pastes_with_an_image() {
        let (mut app, mut rx) = app_with_connection();
        let first_paste = "a".repeat(1_001);
        let second_paste = "b".repeat(1_002);
        app.input.insert_str("before ");
        app.input.insert_paste_block(&first_paste);
        app.input.insert_str(" [Image #1] between ");
        app.input.insert_paste_block(&second_paste);
        app.input.insert_str(" after");
        app.pending_images.push(crate::app::clipboard_image::ImageAttachment {
            data: "aGVsbG8=".to_owned(),
            mime_type: "image/png".to_owned(),
        });

        submit_input(&mut app);

        let envelope = rx.try_recv().expect("prompt should be sent");
        let BridgeCommand::Prompt { chunks, inline_pastes, .. } = envelope.command else {
            panic!("expected prompt command");
        };
        assert_eq!(inline_pastes, vec![first_paste.clone(), second_paste.clone()]);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].kind, "image");
        assert_eq!(chunks[1].kind, "text");
        assert_eq!(
            chunks[1].value,
            serde_json::Value::String(format!(
                "before {first_paste} [Image #1] between {second_paste} after"
            ))
        );
    }

    #[test]
    fn config_slash_submit_preserves_prompt_suggestion_across_fullscreen_return() {
        let (mut app, mut rx) = app_with_connection();
        let dir = tempfile::tempdir().expect("tempdir");
        app.settings_home_override = Some(dir.path().to_path_buf());
        app.cwd_raw = dir.path().to_string_lossy().to_string();
        app.session_runtime.prompt_suggestion = Some("Write focused tests".to_owned());
        app.input.set_text("/config");

        submit_input(&mut app);

        assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::Config));
        assert!(app.input.text().is_empty());
        assert!(matches!(app.status, AppStatus::Ready));
        assert_eq!(app.session_runtime.prompt_suggestion.as_deref(), Some("Write focused tests"));
        assert!(rx.try_recv().is_err(), "config open should not dispatch a prompt turn");

        crate::app::view::set_chat_surface(&mut app);

        let hint_rows = crate::ui::input_rows::build_composer_hint_rows(&app);
        let hint_text = hint_rows
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(hint_text.contains("Suggestion: Write focused tests"));
    }

    #[test]
    fn local_custom_slash_submit_is_consumed() {
        let (mut app, mut rx) = app_with_connection();
        let dir = tempfile::tempdir().expect("tempdir");
        app.settings_home_override = Some(dir.path().to_path_buf());
        app.cwd_raw = dir.path().to_string_lossy().to_string();
        app.input.set_text("/1m-context status");

        submit_input(&mut app);

        assert!(app.input.text().is_empty());
        assert!(matches!(app.status, AppStatus::Ready));
        let Some(last) = app.transcript.messages.last() else {
            panic!("expected /1m-context status message");
        };
        assert!(matches!(last.role, MessageRole::System(Some(super::super::SystemSeverity::Info))));
        assert!(rx.try_recv().is_err(), "local custom slash command should not dispatch a prompt");
    }

    #[test]
    fn auth_slash_usage_error_is_consumed() {
        let (mut app, mut rx) = app_with_connection();
        app.input.set_text("/login extra");

        submit_input(&mut app);

        assert!(app.input.text().is_empty());
        assert!(matches!(app.status, AppStatus::Ready));
        let Some(last) = app.transcript.messages.last() else {
            panic!("expected /login usage message");
        };
        assert!(matches!(
            last.role,
            MessageRole::System(Some(super::super::SystemSeverity::Error))
        ));
        assert!(rx.try_recv().is_err(), "auth slash usage error should not dispatch a prompt");
    }

    #[test]
    fn queued_prompt_is_not_redispatched_when_the_active_turn_completes() {
        let (mut app, mut rx) = app_with_connection();
        app.status = AppStatus::Running;
        app.input.set_text("submit manually later");

        submit_input(&mut app);
        crate::app::events::handle_client_event(
            &mut app,
            ClientEvent::TurnComplete {
                session_id: "session-1".to_owned(),
                queued_turn_count: None,
                terminal_reason: None,
            },
        );

        assert!(app.input.text().is_empty());
        assert_eq!(app.pending_user_messages.len(), 1);
        assert!(matches!(app.status, AppStatus::Ready));
        let commands = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
        assert_eq!(
            commands
                .iter()
                .filter(|envelope| matches!(envelope.command, BridgeCommand::Prompt { .. }))
                .count(),
            1,
            "turn completion must not redispatch queued input"
        );
    }

    #[test]
    fn prompt_submitted_between_queued_turns_joins_pending_projection() {
        let (mut app, mut rx) = app_with_connection();
        app.status = AppStatus::Running;
        app.input.set_text("second turn");
        submit_input(&mut app);
        crate::app::events::handle_client_event(
            &mut app,
            ClientEvent::TurnComplete {
                session_id: "session-1".to_owned(),
                queued_turn_count: Some(1),
                terminal_reason: None,
            },
        );
        app.input.set_text("third turn");

        submit_input(&mut app);

        assert!(app.input.text().is_empty());
        assert_eq!(
            app.pending_user_messages
                .iter()
                .map(|message| message.text.as_str())
                .collect::<Vec<_>>(),
            ["second turn", "third turn"]
        );
        assert!(matches!(app.status, AppStatus::Ready));
        let commands = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
        assert_eq!(
            commands
                .iter()
                .filter(|envelope| matches!(envelope.command, BridgeCommand::Prompt { .. }))
                .count(),
            2
        );
    }

    #[test]
    fn status_submit_while_running_opens_fullscreen_and_keeps_turn_running() {
        let (mut app, mut rx) = app_with_connection();
        let dir = tempfile::tempdir().expect("tempdir");
        app.settings_home_override = Some(dir.path().to_path_buf());
        app.cwd_raw = dir.path().to_string_lossy().to_string();
        app.status = AppStatus::Running;
        app.input.set_text("/status");

        submit_input(&mut app);

        assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::Config));
        assert_eq!(app.config.active_tab, super::super::ConfigTab::Status);
        assert!(app.input.text().is_empty());
        assert!(matches!(app.status, AppStatus::Running));
        let snapshot = rx.try_recv().expect("status view should request a current snapshot");
        assert!(matches!(
            snapshot.command,
            BridgeCommand::GetStatusSnapshot { session_id } if session_id == "session-1"
        ));
    }

    #[test]
    fn mcp_and_plugins_open_during_active_turn() {
        for (input, expected_tab) in
            [("/mcp", super::super::ConfigTab::Mcp), ("/plugins", super::super::ConfigTab::Plugins)]
        {
            let (mut app, _rx) = app_with_connection();
            let dir = tempfile::tempdir().expect("tempdir");
            app.settings_home_override = Some(dir.path().to_path_buf());
            app.cwd_raw = dir.path().to_string_lossy().to_string();
            app.status = AppStatus::Running;
            app.input.set_text(input);

            submit_input(&mut app);

            assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::Config));
            assert_eq!(app.config.active_tab, expected_tab);
            assert!(app.input.text().is_empty());
            assert!(matches!(app.status, AppStatus::Running));
            assert!(!app.turn.cancel_requested);
        }
    }

    #[test]
    fn dispatch_prompt_turn_without_session_id_leaves_state_unchanged() {
        let mut app = App::test_default();
        let (connection, _rx) = crate::agent::client::AgentConnection::test_channel();
        app.session_runtime.conn = Some(std::rc::Rc::new(connection));
        app.session_runtime.prompt_suggestion = Some("previous suggestion".to_owned());
        app.status = AppStatus::Ready;

        dispatch_prompt_turn(&mut app, "hello", Vec::new());

        assert!(app.transcript.messages.is_empty());
        assert_eq!(app.session_runtime.prompt_suggestion.as_deref(), Some("previous suggestion"));
        assert!(matches!(app.status, AppStatus::Ready));
    }
}
