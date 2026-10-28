// SPDX-License-Identifier: Apache-2.0

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
        if !inline_pastes.is_empty() || !app.input.images().is_empty() {
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
    if send_prompt_turn(app, text, app.input.images(), inline_pastes) {
        app.input.clear();
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

    let user_blocks = vec![MessageBlock::Text(
        TextBlock::from_complete(text).with_source_message_uuid(Some(&message_uuid)),
    )];

    app.push_message_tracked(ChatMessage::new(MessageRole::User, user_blocks, None));
    app.start_assistant_response();

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
    let images = app.input.images();
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

    use crate::app::test_support::app_with_connection;

    #[test]
    fn rate_limit_failure_preserves_the_draft_and_allows_editing_and_manual_retry() {
        use crate::agent::error_handling::TurnErrorClass;
        use crate::app::events::handle_client_event;
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        const IMAGE: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aN1sAAAAASUVORK5CYII=";
        let (mut app, mut rx) = app_with_connection();
        app.status = AppStatus::Running;
        app.input.set_text("next draft [Image #1]");
        app.input.image_payloads.push(crate::app::clipboard_image::ImageAttachment {
            data: IMAGE.to_owned(),
            mime_type: "image/png".to_owned(),
        });
        handle_client_event(
            &mut app,
            ClientEvent::SessionUpdate {
                session_id: "session-1".into(),
                update: model::SessionUpdate::RateLimitUpdate(
                    serde_json::from_value(serde_json::json!({
                        "status": "rejected", "rate_limit_type": "five_hour",
                    }))
                    .unwrap(),
                ),
            },
        );
        assert!(app.composer_access().can_edit());
        assert!(app.composer_access().can_submit());
        handle_client_event(
            &mut app,
            ClientEvent::TurnErrorClassified {
                session_id: "session-1".into(),
                message: "Session limit reached; resets in one minute".into(),
                class: TurnErrorClass::PlanLimit,
                queued_turn_count: Some(0),
                api_error_status: Some(429),
                terminal_reason: Some(crate::agent::types::TerminalReason::ApiError),
            },
        );
        assert_eq!(app.input.text(), "next draft [Image #1]");
        assert_eq!(app.input.image_payloads[0].data, IMAGE);
        while let Ok(envelope) = rx.try_recv() {
            assert!(
                !matches!(envelope.command, BridgeCommand::Prompt { .. }),
                "no automatic retry"
            );
        }
        crate::app::keys::dispatch_key_by_focus(
            &mut app,
            KeyEvent::new(KeyCode::End, KeyModifiers::NONE),
        );
        crate::app::keys::dispatch_key_by_focus(
            &mut app,
            KeyEvent::new(KeyCode::Char('!'), KeyModifiers::NONE),
        );
        assert_eq!(app.input.text(), "next draft [Image #1]!");
        submit_input(&mut app);
        let mut prompts = Vec::new();
        while let Ok(envelope) = rx.try_recv() {
            if let BridgeCommand::Prompt { session_id, chunks, .. } = envelope.command {
                prompts.push((session_id, chunks));
            }
        }
        assert_eq!(prompts.len(), 1, "manual retry sends exactly once");
        assert_eq!(prompts[0].0, "session-1");
        let chunks = &prompts[0].1;
        assert_eq!(
            chunks.iter().find(|chunk| chunk.kind == "text").unwrap().value,
            serde_json::json!("next draft [Image #1]!")
        );
        assert_eq!(chunks.iter().find(|chunk| chunk.kind == "image").unwrap().value["data"], IMAGE);
        assert!(app.input.is_empty());
        assert!(app.input.image_payloads.is_empty());
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

    const TEST_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aN1sAAAAASUVORK5CYII=";

    fn complex_draft(app: &mut App) -> (String, Vec<String>) {
        let first = "first pasted paragraph\n".repeat(80);
        let second = "second \u{754c} paragraph\n".repeat(80);
        app.input.set_text("before ");
        app.input.insert_paste_block(&first);
        app.input.insert_str(" [Image #1] between ");
        app.input.insert_paste_block(&second);
        app.input.insert_str(" after");
        app.input.image_payloads.push(crate::app::clipboard_image::ImageAttachment {
            data: TEST_PNG.into(),
            mime_type: "image/png".into(),
        });
        app.input.set_cursor(0, 3);
        (format!("before {first} [Image #1] between {second} after"), vec![first, second])
    }

    fn expect_prompt(
        rx: &mut crate::agent::client::CommandReceiver,
        text: &str,
        pastes: &[String],
        image: bool,
    ) -> String {
        let BridgeCommand::Prompt { session_id, message_uuid, chunks, inline_pastes } =
            rx.try_recv().expect("prompt").command
        else {
            panic!("expected prompt")
        };
        assert_eq!(session_id, "session-1");
        assert_eq!(inline_pastes, pastes);
        let actual_text: Vec<_> =
            chunks.iter().filter(|c| c.kind == "text").map(|c| &c.value).collect();
        assert_eq!(actual_text, [&serde_json::json!(text)]);
        let images: Vec<_> =
            chunks.iter().filter(|c| c.kind == "image").map(|c| &c.value).collect();
        if image {
            assert_eq!(images.len(), 1);
            assert_eq!(images[0]["data"], TEST_PNG);
            assert_eq!(images[0]["mime_type"], "image/png");
        } else {
            assert!(images.is_empty());
        }
        assert_eq!(chunks.len(), 1 + usize::from(image));
        message_uuid
    }

    fn drain_context_only(rx: &mut crate::agent::client::CommandReceiver) {
        while let Ok(envelope) = rx.try_recv() {
            assert!(
                matches!(envelope.command, BridgeCommand::GetContextUsage { .. }),
                "unexpected command: {:?}",
                envelope.command
            );
        }
    }

    fn user_sources(app: &App) -> Vec<(String, Vec<String>)> {
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

    fn assistant_text(app: &App, owner: usize) -> String {
        app.transcript.messages[owner]
            .blocks
            .iter()
            .filter_map(|b| match b {
                MessageBlock::Text(t) => Some(t.text.as_str()),
                _ => None,
            })
            .collect()
    }

    fn feedback(app: &App) -> Vec<&str> {
        app.transcript
            .messages
            .iter()
            .flat_map(|m| &m.blocks)
            .filter_map(|b| match b {
                MessageBlock::Text(t) => Some(t.text.as_str()),
                MessageBlock::Notice(n) => Some(n.text.text.as_str()),
                _ => None,
            })
            .collect()
    }

    fn start_queued(app: &mut App, uuid: &str) {
        crate::app::events::handle_client_event(
            app,
            ClientEvent::UserMessageQueued {
                session_id: "session-1".into(),
                message_uuid: uuid.into(),
            },
        );
        crate::app::events::handle_client_event(
            app,
            ClientEvent::UserMessageStarted {
                session_id: "session-1".into(),
                message_uuid: uuid.into(),
                source: crate::agent::types::UserMessageStartSource::StreamEvent,
            },
        );
    }

    fn answer_btw(app: &mut App, id: &str, question: &str, answer: &str) {
        crate::app::events::handle_client_event(
            app,
            ClientEvent::BtwResult {
                session_id: "session-1".into(),
                btw_id: id.into(),
                question: question.into(),
                answer: answer.into(),
                metadata: crate::agent::wire::SideQuestionMetadata {
                    synthetic: false,
                    refusal_fallback: None,
                },
            },
        );
    }

    fn fail_btw(app: &mut App, id: &str, question: &str, error: &str) {
        crate::app::events::handle_client_event(
            app,
            ClientEvent::BtwFailed {
                session_id: "session-1".into(),
                btw_id: id.into(),
                question: question.into(),
                error: error.into(),
            },
        );
    }

    fn expect_btw(rx: &mut crate::agent::client::CommandReceiver, expected: &str) -> String {
        let BridgeCommand::SideQuestion { session_id, btw_id, question } =
            rx.try_recv().expect("side question").command
        else {
            panic!("expected side question")
        };
        assert_eq!(session_id, "session-1");
        assert_eq!(question, expected);
        assert!(rx.try_recv().is_err(), "only one active side question may be sent");
        btw_id
    }

    fn exchanges(app: &App) -> Vec<(String, String)> {
        app.transcript
            .messages
            .iter()
            .flat_map(|m| &m.blocks)
            .filter_map(|b| match b {
                MessageBlock::BtwExchange(e) => Some((e.question.clone(), e.answer.clone())),
                _ => None,
            })
            .collect()
    }

    fn assert_btw_failure(app: &App, question: &str, cause: &str) {
        assert!(
            crate::ui::input_rows::build_btw_status_rows(app, 160).iter().any(|row| {
                let row = row.to_string();
                row.contains(question) && row.contains(cause)
            }),
            "question and cause must be associated in the visible status row"
        );
    }

    #[test]
    fn submission_queueing_and_paste_expansion_preserve_the_user_source() {
        for queued in [false, true] {
            for variant in 0..3 {
                let (mut app, mut rx) = app_with_connection();
                let owner = if queued {
                    start_main_turn(&mut app, &mut rx);
                    deliver_main_text(&mut app, "original response");
                    app.active_turn_assistant_idx()
                } else {
                    None
                };
                let prior = user_sources(&app);
                let (source, pastes) = match variant {
                    0 => {
                        app.input.set_text("hello\n\nhow are you");
                        ("hello\n\nhow are you".into(), vec![])
                    }
                    1 => {
                        let source = "large \u{754c} pasted paragraph\n".repeat(80);
                        app.input.insert_paste_block(&source);
                        (source.clone(), vec![source])
                    }
                    _ => complex_draft(&mut app),
                };
                let images = app.input.images();
                app.session_runtime.prompt_suggestion = Some("old suggestion".into());
                submit_input(&mut app);
                let uuid = expect_prompt(&mut rx, &source, &pastes, variant == 2);
                drain_context_only(&mut rx);
                assert!(app.input.is_empty());
                assert!(app.input.images().is_empty());
                assert!(app.session_runtime.prompt_suggestion.is_none());
                assert!(!app.turn.cancel_requested);
                if let Some(owner) = owner {
                    let pending = app.pending_user_messages.iter().next().expect("pending payload");
                    assert_eq!(
                        (&pending.uuid, &pending.text, &pending.images),
                        (&uuid, &source, &images)
                    );
                    assert_eq!(user_sources(&app), prior, "no premature transcript admission");
                    deliver_main_text(&mut app, " continues before queued start");
                    assert_eq!(app.active_turn_assistant_idx(), Some(owner));
                    assert_eq!(
                        assistant_text(&app, owner),
                        "original response continues before queued start"
                    );
                    start_queued(&mut app, &uuid);
                    assert!(app.pending_user_messages.is_empty());
                    assert_eq!(
                        assistant_text(&app, owner),
                        "original response continues before queued start"
                    );
                }
                let expected: Vec<_> =
                    prior.into_iter().chain([(source, vec![uuid.clone()])]).collect();
                assert_eq!(user_sources(&app), expected);
                if queued {
                    start_queued(&mut app, &uuid);
                    assert_eq!(
                        user_sources(&app),
                        expected,
                        "duplicate start must not duplicate the user message"
                    );
                }
            }
        }
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
        assert_eq!(
            app.activity_presentation(std::time::Instant::now())
                .expect("dispatch immediately shows ordinary activity")
                .label,
            crate::app::activity::ActivityLabel::Working
        );
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

    #[test]
    #[allow(clippy::too_many_lines)]
    fn btw_completion_enters_active_stream_and_history_before_subsequent_agent_output() {
        use crate::ui::inline_chat_rows::{
            LiveRowBoundaryKind, serialize_live_rows_with_boundaries_excluding,
        };
        use std::collections::BTreeSet;
        for state in ["idle", "live", "completed"] {
            let (mut app, mut rx) = app_with_connection();
            let owner = if state == "idle" {
                None
            } else {
                start_main_turn(&mut app, &mut rx);
                deliver_main_text(&mut app, "Main response before.");
                app.active_turn_assistant_idx()
            };
            app.input.set_text("/btw   Why  this choice?\nExplain the tradeoff.  ");
            submit_input(&mut app);
            let question = "Why  this choice?\nExplain the tradeoff.";
            let answer = "It preserves ownership.\nThe queued request stays separate.";
            let id = expect_btw(&mut rx, question);
            if state == "completed" {
                finish_main_turn(&mut app);
            }
            answer_btw(&mut app, &id, question, answer);
            assert_eq!(exchanges(&app), [(question.into(), answer.into())]);
            let (message_idx, block_idx) = app
                .transcript
                .messages
                .iter()
                .enumerate()
                .find_map(|(mi, m)| {
                    m.blocks
                        .iter()
                        .position(
                            |b| matches!(b, MessageBlock::BtwExchange(e) if e.question == question),
                        )
                        .map(|bi| (mi, bi))
                })
                .unwrap();
            if state == "live" {
                assert_eq!(Some(message_idx), owner);
            }
            if state == "completed" {
                assert!(message_idx > owner.unwrap());
                assert_eq!(assistant_text(&app, owner.unwrap()), "Main response before.");
            }
            let serialized = serialize_live_rows_with_boundaries_excluding(
                &mut app,
                100,
                &BTreeSet::new(),
                None,
            );
            let card = serialized
                .segments()
                .iter()
                .find(|s| {
                    s.msg_idx == message_idx && (state != "live" || s.block_idx == Some(block_idx))
                })
                .expect("owned card segment");
            if state == "live" {
                assert_eq!(card.kind, LiveRowBoundaryKind::AssistantBtw);
            }
            assert!(card.commit_ready);
            assert!(card.end_row <= serialized.stable_row_count());
            let rendered = serialized
                .segment_rows(card)
                .expect("card rows")
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n");
            for expected in [
                "Why  this choice?",
                "Explain the tradeoff.",
                "It preserves ownership.",
                "The queued request stays separate.",
            ] {
                assert!(
                    rendered.contains(expected),
                    "missing {expected:?} in owned segment: {rendered}"
                );
            }
            let committed: BTreeSet<_> = serialized
                .segments()
                .iter()
                .take_while(|s| s.commit_ready)
                .flat_map(|s| s.ids.iter().cloned())
                .collect();
            if state == "live" {
                deliver_main_text(&mut app, "Main response after.");
                assert_eq!(app.active_turn_assistant_idx(), owner);
                let blocks = &app.transcript.messages[message_idx].blocks;
                assert!(
                    matches!(&blocks[block_idx - 1], MessageBlock::Text(t) if t.text == "Main response before.")
                );
                assert!(
                    matches!(&blocks[block_idx + 1], MessageBlock::Text(t) if t.text == "Main response after.")
                );
                let mutable =
                    serialize_live_rows_with_boundaries_excluding(&mut app, 100, &committed, None);
                assert!(
                    !mutable
                        .segments()
                        .iter()
                        .any(|s| s.msg_idx == message_idx && s.block_idx == Some(block_idx))
                );
                assert!(
                    mutable.rows().iter().any(|r| r.to_string().contains("Main response after."))
                );
                finish_main_turn(&mut app);
            }
            answer_btw(&mut app, &id, question, "duplicate must be ignored");
            start_main_turn(&mut app, &mut rx);
            let next_owner = app.active_turn_assistant_idx().unwrap();
            deliver_main_text(&mut app, "A subsequent main response.");
            answer_btw(&mut app, &id, question, "late duplicate must be ignored");
            assert_eq!(exchanges(&app), [(question.into(), answer.into())]);
            assert_eq!(assistant_text(&app, next_owner), "A subsequent main response.");
            assert!(next_owner > message_idx);
            let rows = serialize_live_rows_with_boundaries_excluding(
                &mut app,
                100,
                &BTreeSet::new(),
                None,
            );
            let cards: Vec<_> = rows
                .segments()
                .iter()
                .filter(|s| {
                    s.msg_idx == message_idx && (state != "live" || s.block_idx == Some(block_idx))
                })
                .collect();
            assert_eq!(
                cards.len(),
                1,
                "completed exchange is rendered once after the next turn starts"
            );
            let next = rows.segments().iter().find(|s| s.msg_idx == next_owner).unwrap();
            assert!(cards[0].end_row <= next.start_row);
            assert!(
                rows.segment_rows(next)
                    .unwrap()
                    .iter()
                    .any(|r| r.to_string().contains("A subsequent main response."))
            );
            assert!(rx.try_recv().is_err());
        }
    }

    #[test]
    fn btw_success_advances_fifo_without_blocking_main_input_and_ignores_duplicate_results() {
        for fail_first in [false, true] {
            let (mut app, mut rx) = app_with_connection();
            start_main_turn(&mut app, &mut rx);
            let owner = app.active_turn_assistant_idx().unwrap();
            deliver_main_text(&mut app, "Main work");
            let mut admitted: Vec<(String, String)> = Vec::new();
            for question in ["first", "duplicate question", "duplicate question"] {
                app.input.set_text(&format!("/btw {question}"));
                submit_input(&mut app);
                let new_request = app
                    .btw
                    .status_items()
                    .into_iter()
                    .find(|i| !admitted.iter().any(|(id, _)| id == &i.id))
                    .expect("new independent request identity");
                assert_eq!(new_request.question, question);
                admitted.push((new_request.id.clone(), question.into()));
            }
            let first = expect_btw(&mut rx, "first");
            assert_eq!(first, admitted[0].0);
            let waiting = &admitted[1..];
            assert_ne!(waiting[0].0, waiting[1].0);
            app.input.set_text("next independent main task");
            submit_input(&mut app);
            let main_uuid = expect_prompt(&mut rx, "next independent main task", &[], false);
            assert_eq!(app.pending_user_messages.iter().next().unwrap().uuid, main_uuid);
            assert_eq!(
                app.pending_user_messages.iter().next().unwrap().text,
                "next independent main task"
            );
            let mut expected = vec![];
            if fail_first {
                fail_btw(&mut app, &first, "first", "unavailable answer");
                assert_btw_failure(&app, "first", "unavailable answer");
            } else {
                answer_btw(&mut app, &first, "first", "first answer");
                expected.push(("first".into(), "first answer".into()));
            }
            assert_eq!(expect_btw(&mut rx, "duplicate question"), waiting[0].0);
            answer_btw(&mut app, &first, "first", "duplicate ignored");
            fail_btw(&mut app, &first, "first", "duplicate failure ignored");
            assert!(rx.try_recv().is_err());
            for (index, (id, question)) in waiting.iter().enumerate() {
                let answer = format!("independent answer {index}");
                answer_btw(&mut app, id, question, &answer);
                expected.push((question.clone(), answer));
                if index == 0 {
                    assert_eq!(expect_btw(&mut rx, "duplicate question"), waiting[1].0);
                }
                answer_btw(&mut app, id, question, "duplicate ignored");
                assert!(rx.try_recv().is_err());
            }
            assert_eq!(exchanges(&app), expected);
            assert!(!app.btw.has_active());
            deliver_main_text(&mut app, " continues");
            assert_eq!(app.active_turn_assistant_idx(), Some(owner));
            assert_eq!(assistant_text(&app, owner), "Main work continues");
            assert_eq!(app.pending_user_messages.iter().next().unwrap().uuid, main_uuid);
        }
    }

    #[test]
    fn btw_failure_frees_full_queue_capacity_while_error_row_is_still_visible() {
        let (mut app, mut rx) = app_with_connection();
        for index in 0..super::super::BtwRequests::CAPACITY {
            app.input.set_text(&format!("/btw question {index}"));
            submit_input(&mut app);
        }
        let failed_id = expect_btw(&mut rx, "question 0");
        let waiting: Vec<_> = (1..super::super::BtwRequests::CAPACITY)
            .map(|index| {
                let question = format!("question {index}");
                let id = app
                    .btw
                    .status_items()
                    .into_iter()
                    .find(|i| i.question == question)
                    .unwrap()
                    .id
                    .clone();
                (id, question)
            })
            .collect();
        app.input.set_text("/btw   keep  all spacing  ");
        app.input.set_cursor(0, 8);
        let rejected = app.input.snapshot();
        submit_input(&mut app);
        assert_eq!(app.input.snapshot(), rejected);
        assert!(rx.try_recv().is_err());
        app.input.set_text("/btw   \n  ");
        submit_input(&mut app);
        assert!(feedback(&app).contains(&"Usage: /btw <question>"));
        assert!(app.input.is_empty());
        assert!(rx.try_recv().is_err());
        fail_btw(&mut app, &failed_id, "question 0", "answer unavailable");
        assert_eq!(expect_btw(&mut rx, &waiting[0].1), waiting[0].0);
        app.input.set_text("/btw   keep  all spacing  ");
        submit_input(&mut app);
        let admitted = app
            .btw
            .status_items()
            .into_iter()
            .find(|i| i.question == "keep  all spacing")
            .expect("retry admitted")
            .id
            .clone();
        assert!(rx.try_recv().is_err());
        assert_btw_failure(&app, "question 0", "answer unavailable");
        answer_btw(&mut app, &failed_id, "question 0", "late forbidden answer");
        assert!(exchanges(&app).is_empty());
        for (index, (id, question)) in waiting.iter().enumerate() {
            answer_btw(&mut app, id, question, &format!("answer to {question}"));
            if let Some((next_id, next_question)) = waiting.get(index + 1) {
                assert_eq!(expect_btw(&mut rx, next_question), *next_id);
            } else {
                assert_eq!(expect_btw(&mut rx, "keep  all spacing"), admitted);
            }
        }
        answer_btw(&mut app, &admitted, "keep  all spacing", "retry answer");
        assert!(exchanges(&app).contains(&("keep  all spacing".into(), "retry answer".into())));
        assert!(!exchanges(&app).iter().any(|(q, _)| q == "question 0"));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn btw_waiting_results_are_ignored_and_active_question_mismatch_advances_fifo() {
        let (mut app, mut rx) = app_with_connection();
        for question in ["first", "second"] {
            app.input.set_text(&format!("/btw {question}"));
            submit_input(&mut app);
        }
        let first = expect_btw(&mut rx, "first");
        let second =
            app.btw.status_items().into_iter().find(|i| i.question == "second").unwrap().id.clone();
        answer_btw(&mut app, &second, "second", "premature answer");
        assert!(exchanges(&app).is_empty());
        assert!(rx.try_recv().is_err());
        assert!(app.btw.get_active(&first).is_some());
        answer_btw(&mut app, &first, "wrong echo", "untrusted answer");
        assert_btw_failure(&app, "first", "echoed question did not match");
        assert_eq!(expect_btw(&mut rx, "second"), second);
        assert!(exchanges(&app).is_empty());
        answer_btw(&mut app, &second, "second", "correct second answer");
        assert_eq!(exchanges(&app), [("second".into(), "correct second answer".into())]);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn btw_send_failure_drains_waiting_work_without_creating_cards_or_leaving_active_state() {
        let (mut app, mut rx) = app_with_connection();
        for question in ["first", "second", "third"] {
            app.input.set_text(&format!("/btw {question}"));
            submit_input(&mut app);
        }
        let first = expect_btw(&mut rx, "first");
        drop(rx);
        answer_btw(&mut app, &first, "first", "completed first answer");
        assert_eq!(exchanges(&app), [("first".into(), "completed first answer".into())]);
        for question in ["second", "third"] {
            assert_btw_failure(&app, question, "could not send");
        }
        let mut failed: Vec<_> = app.btw.status_items().into_iter().map(|i| {
            assert!(matches!(&i.state, super::super::BtwRequestState::Failed { reason, .. } if reason.contains("could not send")));
            i.question.as_str()
        }).collect();
        failed.sort_unstable();
        assert_eq!(failed, ["second", "third"]);
        assert!(!app.btw.has_active());
    }

    #[test]
    fn btw_conversation_reset_discards_waiters_and_drops_late_results_in_the_same_session() {
        for disconnected in [false, true] {
            let (mut app, mut rx) = app_with_connection();
            for question in ["old active", "old waiting"] {
                app.input.set_text(&format!("/btw {question}"));
                submit_input(&mut app);
            }
            let old = expect_btw(&mut rx, "old active");
            let waiting = app
                .btw
                .status_items()
                .into_iter()
                .find(|i| i.question == "old waiting")
                .unwrap()
                .id
                .clone();
            let event = if disconnected {
                ClientEvent::ConnectionFailed("bridge exited during BTW".into())
            } else {
                ClientEvent::SessionUpdate {
                    session_id: "session-1".into(),
                    update: model::SessionUpdate::ConversationReset {
                        new_conversation_id: "conversation-2".into(),
                        trigger: None,
                        timestamp: None,
                        user_message_uuid: None,
                    },
                }
            };
            crate::app::events::handle_client_event(&mut app, event);
            assert_eq!(app.btw.len(), 0);
            let new_id = if disconnected {
                None
            } else {
                app.input.set_text("/btw new question");
                submit_input(&mut app);
                Some(expect_btw(&mut rx, "new question"))
            };
            for (id, question) in [(&old, "old active"), (&waiting, "old waiting")] {
                answer_btw(&mut app, id, question, "stale answer");
                fail_btw(&mut app, id, question, "stale failure");
            }
            assert!(exchanges(&app).is_empty());
            assert!(rx.try_recv().is_err());
            if let Some(new_id) = new_id {
                assert!(app.btw.get_active(&new_id).is_some());
                answer_btw(&mut app, &new_id, "new question", "new conversation answer");
                assert_eq!(
                    exchanges(&app),
                    [("new question".into(), "new conversation answer".into())]
                );
            } else {
                assert!(matches!(app.status, AppStatus::Error));
                assert!(app.session_runtime.session_id.is_none());
                assert!(feedback(&app).iter().any(|s| s.contains("bridge exited during BTW")));
            }
        }
    }

    #[test]
    fn active_turn_prompt_at_capacity_preserves_composer_without_dispatch_or_notice() {
        let (mut app, mut rx) = app_with_connection();
        start_main_turn(&mut app, &mut rx);
        deliver_main_text(&mut app, "Main response");
        let owner = app.active_turn_assistant_idx().unwrap();
        for index in 0..crate::app::state::PendingUserMessages::CAPACITY {
            let text = format!("queued message {index}\ncontinued");
            app.input.set_text(&text);
            submit_input(&mut app);
            expect_prompt(&mut rx, &text, &[], false);
        }
        let pending: Vec<_> = app.pending_user_messages.iter().cloned().collect();
        let users = user_sources(&app);
        let notices = feedback(&app).into_iter().map(str::to_owned).collect::<Vec<_>>();
        let (source, pastes) = complex_draft(&mut app);
        let draft = app.input.snapshot();
        let images = app.input.images();
        app.session_runtime.prompt_suggestion = Some("previous suggestion".into());
        submit_input(&mut app);
        assert_eq!(app.input.snapshot(), draft);
        assert_eq!(app.input.images(), images);
        assert_eq!(app.session_runtime.prompt_suggestion.as_deref(), Some("previous suggestion"));
        assert_eq!(app.pending_user_messages.iter().cloned().collect::<Vec<_>>(), pending);
        assert_eq!(feedback(&app), notices);
        assert_eq!(user_sources(&app), users);
        assert_eq!(app.active_turn_assistant_idx(), Some(owner));
        assert!(rx.try_recv().is_err());
        start_queued(&mut app, &pending[0].uuid);
        submit_input(&mut app);
        let uuid = expect_prompt(&mut rx, &source, &pastes, true);
        assert!(app.input.is_empty());
        let remaining: Vec<_> = app.pending_user_messages.iter().cloned().collect();
        assert_eq!(remaining[..remaining.len() - 1], pending[1..]);
        let retry = remaining.last().unwrap();
        assert_eq!((&retry.uuid, &retry.text, &retry.images), (&uuid, &source, &images));
        assert_eq!(
            user_sources(&app),
            users
                .into_iter()
                .chain([("queued message 0\ncontinued".into(), vec![pending[0].uuid.clone()])])
                .collect::<Vec<_>>()
        );
        assert_eq!(assistant_text(&app, owner), "Main response");
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn active_turn_prompt_does_not_change_slash_command_blocking() {
        for between_turns in [false, true] {
            for (command, expected, consumed) in [
                ("/docs commands", "Docs: Commands", true),
                ("/resume one two", "Usage: /resume", true),
                ("/resume", "`/resume`", false),
                ("/model sonnet", "`/model`", false),
                ("/remote-command", "`/remote-command`", false),
            ] {
                let (mut app, mut rx) = app_with_connection();
                start_main_turn(&mut app, &mut rx);
                deliver_main_text(&mut app, "Original answer");
                let owner = app.active_turn_assistant_idx().unwrap();
                app.input.set_text("already admitted next prompt");
                submit_input(&mut app);
                let pending_id = expect_prompt(&mut rx, "already admitted next prompt", &[], false);
                if between_turns {
                    finish_main_turn(&mut app);
                    drain_context_only(&mut rx);
                }
                let pending: Vec<_> = app.pending_user_messages.iter().cloned().collect();
                app.sdk_inventory.available_commands =
                    vec![model::AvailableCommand::new("/remote-command", "Remote command")];
                app.input.set_text(command);
                submit_input(&mut app);
                assert_eq!(app.input.text(), if consumed { "" } else { command });
                assert_eq!(app.pending_user_messages.iter().cloned().collect::<Vec<_>>(), pending);
                let target = if between_turns {
                    app.transcript.messages.last().unwrap()
                } else {
                    &app.transcript.messages[owner]
                };
                let notice = target
                    .blocks
                    .iter()
                    .find_map(|b| match b {
                        MessageBlock::Notice(n) if n.text.text.contains(expected) => {
                            Some(n.text.text.as_str())
                        }
                        MessageBlock::Text(t) if between_turns && t.text.contains(expected) => {
                            Some(t.text.as_str())
                        }
                        _ => None,
                    })
                    .expect("command feedback at the correct owner");
                if command == "/resume one two" {
                    assert_eq!(notice, "Usage: /resume");
                }
                if !consumed {
                    assert!(notice.contains("between agent turns"));
                }
                assert!(rx.try_recv().is_err(), "local or blocked command must not dispatch");
                if between_turns {
                    assert!(matches!(app.status, AppStatus::Ready));
                    assert_eq!(assistant_text(&app, owner), "Original answer");
                    start_queued(&mut app, &pending_id);
                    let next = app.active_turn_assistant_idx().unwrap();
                    assert_ne!(next, owner);
                    deliver_main_text(&mut app, "Queued response");
                    assert_eq!(assistant_text(&app, next), "Queued response");
                } else {
                    deliver_main_text(&mut app, " continues");
                    assert_eq!(app.active_turn_assistant_idx(), Some(owner));
                    assert_eq!(assistant_text(&app, owner), "Original answer continues");
                }
            }
        }
    }

    #[test]
    fn idle_prompt_send_failure_preserves_exact_draft_and_attachments() {
        for active in [false, true] {
            let (mut app, mut rx) = app_with_connection();
            start_main_turn(&mut app, &mut rx);
            deliver_main_text(&mut app, "Preserved response");
            let owner = app.active_turn_assistant_idx().unwrap();
            if active {
                app.input.set_text("previously queued prompt");
                submit_input(&mut app);
                expect_prompt(&mut rx, "previously queued prompt", &[], false);
            } else {
                finish_main_turn(&mut app);
                drain_context_only(&mut rx);
            }
            let users = user_sources(&app);
            let pending: Vec<_> = app.pending_user_messages.iter().cloned().collect();
            let assistant_ids: Vec<_> = app
                .transcript
                .messages
                .iter()
                .filter(|m| matches!(m.role, MessageRole::Assistant))
                .map(|m| m.id)
                .collect();
            drop(rx);
            complex_draft(&mut app);
            app.session_runtime.prompt_suggestion = Some("recoverable suggestion".into());
            let draft = app.input.snapshot();
            let images = app.input.images();
            let pastes = app.input.inline_pastes();
            submit_input(&mut app);
            assert_eq!(app.input.snapshot(), draft);
            assert_eq!(app.input.images(), images);
            assert_eq!(app.input.inline_pastes(), pastes);
            assert_eq!(
                app.session_runtime.prompt_suggestion.as_deref(),
                Some("recoverable suggestion")
            );
            assert_eq!(app.pending_user_messages.iter().cloned().collect::<Vec<_>>(), pending);
            assert_eq!(user_sources(&app), users);
            assert_eq!(
                app.transcript
                    .messages
                    .iter()
                    .filter(|m| matches!(m.role, MessageRole::Assistant))
                    .map(|m| m.id)
                    .collect::<Vec<_>>(),
                assistant_ids
            );
            assert!(
                feedback(&app)
                    .iter()
                    .any(|s| s.contains("could not be sent") || s.contains("channel closed"))
            );
            if active {
                deliver_main_text(&mut app, " still streaming");
                assert_eq!(app.active_turn_assistant_idx(), Some(owner));
                assert_eq!(assistant_text(&app, owner), "Preserved response still streaming");
                assert!(matches!(app.status, AppStatus::Running));
            } else {
                assert_eq!(assistant_text(&app, owner), "Preserved response");
                assert!(app.active_turn_assistant_idx().is_none());
                assert!(matches!(app.status, AppStatus::Error));
                assert!(app.turn.activity.is_none());
            }
        }
    }

    #[test]
    fn submit_input_cancel_command_requests_manual_cancel() {
        let (mut app, mut rx) = app_with_connection();
        start_main_turn(&mut app, &mut rx);
        deliver_main_text(&mut app, "Working response");
        let owner = app.active_turn_assistant_idx().unwrap();
        crate::app::test_support::admit_tool(
            &mut app,
            model::ToolCall::new("running-tool", "Read")
                .status(model::ToolCallStatus::InProgress)
                .content(vec!["partial output".into()]),
        );
        for _ in 0..2 {
            app.input.set_text("/cancel");
            submit_input(&mut app);
            assert!(app.input.is_empty());
        }
        assert!(app.turn.cancel_requested);
        assert!(
            matches!(rx.try_recv().expect("cancel").command, BridgeCommand::CancelTurn {session_id} if session_id == "session-1")
        );
        assert!(
            rx.try_recv().is_err(),
            "duplicate cancel must not dispatch another cancel or prompt"
        );
        let tool = app.tool_call("running-tool").unwrap();
        assert_eq!(tool.status, model::ToolCallStatus::Failed);
        assert_eq!(tool.content, [model::ToolCallContent::from("partial output")]);
        assert_eq!(app.lookup_tool_call("running-tool").unwrap().0, owner);
        assert_eq!(assistant_text(&app, owner), "Working response");
    }

    #[test]
    fn config_slash_submit_preserves_prompt_suggestion_across_fullscreen_return() {
        let (mut app, mut rx) = app_with_connection();
        let dir = tempfile::tempdir().unwrap();
        app.settings_home_override = Some(dir.path().to_path_buf());
        app.cwd_raw = dir.path().to_string_lossy().into_owned();
        app.session_runtime.prompt_suggestion = Some("Write focused tests".into());
        app.input.set_text("/config");
        submit_input(&mut app);
        assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::Config));
        assert!(app.input.is_empty());
        assert!(
            matches!(rx.try_recv().expect("inspect settings").command, BridgeCommand::InspectSettings {session_id, ..} if session_id == "session-1")
        );
        crate::app::test_support::press(&mut app, crossterm::event::KeyCode::Esc);
        assert_eq!(app.surface_mode, SurfaceMode::Chat);
        assert!(
            crate::ui::input_rows::build_composer_hint_rows(&app)
                .iter()
                .any(|r| r.to_string().contains("Suggestion: Write focused tests"))
        );
        crate::app::test_support::press(&mut app, crossterm::event::KeyCode::Tab);
        assert_eq!(app.input.text(), "Write focused tests");
        assert!(matches!(app.status, AppStatus::Ready));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn prompt_submitted_between_queued_turns_joins_pending_projection() {
        for queued_turn_count in [None, Some(1)] {
            let (mut app, mut rx) = app_with_connection();
            start_main_turn(&mut app, &mut rx);
            deliver_main_text(&mut app, "First response");
            let first = app.active_turn_assistant_idx().unwrap();
            let prior = user_sources(&app);
            app.input.set_text("second turn");
            submit_input(&mut app);
            let second = expect_prompt(&mut rx, "second turn", &[], false);
            crate::app::events::handle_client_event(
                &mut app,
                ClientEvent::TurnComplete {
                    session_id: "session-1".into(),
                    queued_turn_count,
                    terminal_reason: None,
                },
            );
            drain_context_only(&mut rx);
            assert!(matches!(app.status, AppStatus::Ready));
            app.input.set_text("third turn");
            submit_input(&mut app);
            let third = expect_prompt(&mut rx, "third turn", &[], false);
            assert_ne!(second, third);
            assert!(rx.try_recv().is_err());
            assert_eq!(user_sources(&app), prior);
            assert_eq!(
                app.pending_user_messages
                    .iter()
                    .map(|m| (m.uuid.as_str(), m.text.as_str()))
                    .collect::<Vec<_>>(),
                [(second.as_str(), "second turn"), (third.as_str(), "third turn")]
            );
            let mut expected = prior;
            for (id, prompt, response) in [
                (&second, "second turn", "Second response"),
                (&third, "third turn", "Third response"),
            ] {
                start_queued(&mut app, id);
                expected.push((prompt.into(), vec![id.clone()]));
                assert_eq!(user_sources(&app), expected);
                let owner = app.active_turn_assistant_idx().unwrap();
                assert!(owner > first);
                deliver_main_text(&mut app, response);
                assert_eq!(assistant_text(&app, owner), response);
                assert_eq!(assistant_text(&app, first), "First response");
                finish_main_turn(&mut app);
                drain_context_only(&mut rx);
            }
            assert!(app.pending_user_messages.is_empty());
            assert_eq!(user_sources(&app), expected);
        }
    }

    #[test]
    fn dispatch_prompt_turn_without_session_id_leaves_state_unchanged() {
        let (mut app, mut rx) = app_with_connection();
        start_main_turn(&mut app, &mut rx);
        deliver_main_text(&mut app, "Earlier conversation");
        finish_main_turn(&mut app);
        drain_context_only(&mut rx);
        let users = user_sources(&app);
        crate::app::events::handle_client_event(
            &mut app,
            ClientEvent::AuthRequired {
                method_name: "login".into(),
                method_description: "Authenticate to continue".into(),
            },
        );
        assert!(matches!(app.status, AppStatus::Ready));
        assert!(app.session_runtime.session_id.is_none());
        complex_draft(&mut app);
        app.session_runtime.prompt_suggestion = Some("previous suggestion".into());
        let draft = app.input.snapshot();
        let images = app.input.images();
        let pastes = app.input.inline_pastes();
        let ids: Vec<_> = app.transcript.messages.iter().map(|m| m.id).collect();
        submit_input(&mut app);
        assert_eq!(app.input.snapshot(), draft);
        assert_eq!(app.input.images(), images);
        assert_eq!(app.input.inline_pastes(), pastes);
        assert_eq!(app.session_runtime.prompt_suggestion.as_deref(), Some("previous suggestion"));
        assert_eq!(user_sources(&app), users);
        assert_eq!(app.transcript.messages.iter().map(|m| m.id).collect::<Vec<_>>(), ids);
        assert!(feedback(&app).contains(&"Earlier conversation"));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn btw_attachment_rejection_preserves_draft_and_allows_text_only_resubmission() {
        for image in [false, true] {
            let (mut app, mut rx) = app_with_connection();
            app.input.set_text("/btw explain this ");
            if image {
                app.input.insert_str("[Image #1]");
                app.input.image_payloads.push(crate::app::clipboard_image::ImageAttachment {
                    data: TEST_PNG.into(),
                    mime_type: "image/png".into(),
                });
            } else {
                app.input.insert_paste_block(&"attached context\n".repeat(80));
            }
            let draft = app.input.snapshot();
            let images = app.input.images();
            let pastes = app.input.inline_pastes();
            submit_input(&mut app);
            assert_eq!(app.input.snapshot(), draft);
            assert_eq!(app.input.images(), images);
            assert_eq!(app.input.inline_pastes(), pastes);
            assert!(
                feedback(&app).iter().any(|s| s.contains("text-only") && s.contains("attachments"))
            );
            assert_eq!(app.btw.len(), 0);
            assert!(exchanges(&app).is_empty());
            assert!(rx.try_recv().is_err());
            crate::app::test_support::press(&mut app, crossterm::event::KeyCode::Backspace);
            assert!(app.input.images().is_empty());
            assert!(app.input.inline_pastes().is_empty());
            submit_input(&mut app);
            let id = expect_btw(&mut rx, "explain this");
            answer_btw(&mut app, &id, "explain this", "text-only answer");
            assert_eq!(exchanges(&app), [("explain this".into(), "text-only answer".into())]);
        }
    }

    #[test]
    fn failed_launch_prompt_remains_recoverable_without_overwriting_a_newer_draft() {
        use clap::Parser;
        for newer_draft in [false, true] {
            let (mut app, mut rx) = app_with_connection();
            start_main_turn(&mut app, &mut rx);
            deliver_main_text(&mut app, "Earlier answer");
            finish_main_turn(&mut app);
            drain_context_only(&mut rx);
            let ids: Vec<_> = app
                .transcript
                .messages
                .iter()
                .filter(|m| !matches!(m.role, MessageRole::System(_)))
                .map(|m| m.id)
                .collect();
            let cli =
                crate::Cli::try_parse_from(["claude-rs", "--", "launch request\nwith details"])
                    .unwrap();
            app.startup = crate::app::state::StartupState::from_cli(&cli);
            app.startup.complete_launch();
            if newer_draft {
                complex_draft(&mut app);
            }
            let draft = app.input.snapshot();
            let images = app.input.images();
            let pastes = app.input.inline_pastes();
            drop(rx);
            maybe_submit_initial_prompt(&mut app);
            if newer_draft {
                assert_eq!(app.input.snapshot(), draft);
                assert_eq!(app.input.images(), images);
                assert_eq!(app.input.inline_pastes(), pastes);
                assert!(feedback(&app).iter().any(|s| {
                    s.contains("Initial prompt could not be sent: launch request\nwith details")
                }));
            } else {
                assert_eq!(app.input.text(), "launch request\nwith details");
            }
            assert_eq!(
                app.transcript
                    .messages
                    .iter()
                    .filter(|m| !matches!(m.role, MessageRole::System(_)))
                    .map(|m| m.id)
                    .collect::<Vec<_>>(),
                ids
            );
            assert!(feedback(&app).contains(&"Earlier answer"));
            assert!(app.active_turn_assistant_idx().is_none());
        }
    }

    #[test]
    fn failed_cancel_admission_preserves_live_tools_and_response_ownership() {
        let (mut app, mut rx) = app_with_connection();
        start_main_turn(&mut app, &mut rx);
        deliver_main_text(&mut app, "Response before cancel");
        let owner = app.active_turn_assistant_idx().unwrap();
        crate::app::test_support::admit_tool(
            &mut app,
            model::ToolCall::new("active-tool", "Read")
                .status(model::ToolCallStatus::InProgress)
                .content(vec!["partial tool output".into()]),
        );
        let content = app.tool_call("active-tool").unwrap().content.clone();
        drop(rx);
        assert!(request_cancel(&mut app).is_err());
        assert!(!app.turn.cancel_requested);
        let tool = app.tool_call("active-tool").unwrap();
        assert_eq!(tool.status, model::ToolCallStatus::InProgress);
        assert_eq!(tool.content, content);
        crate::app::test_support::deliver_update(
            &mut app,
            model::SessionUpdate::ToolCallUpdate(model::ToolCallUpdate::new(
                "active-tool",
                model::ToolCallUpdateFields::new()
                    .status(model::ToolCallStatus::Completed)
                    .content(vec!["completed tool output".into()]),
            )),
        );
        deliver_main_text(&mut app, " and after rejected cancel");
        assert_eq!(app.active_turn_assistant_idx(), Some(owner));
        assert_eq!(assistant_text(&app, owner), "Response before cancel and after rejected cancel");
        assert_eq!(app.lookup_tool_call("active-tool").unwrap().0, owner);
        assert_eq!(app.tool_call("active-tool").unwrap().status, model::ToolCallStatus::Completed);
        assert_eq!(
            app.tool_call("active-tool").unwrap().content,
            [model::ToolCallContent::from("completed tool output")]
        );
    }
    #[test]
    fn btw_session_replacement_discards_waiters_and_isolates_the_new_exchange() {
        use crate::app::test_support::{drain_session_refreshes, session_replacement};
        let directory = tempfile::tempdir().unwrap();
        let (mut app, mut rx) = app_with_connection();
        for question in ["old active", "old waiting"] {
            app.input.set_text(&format!("/btw {question}"));
            submit_input(&mut app);
        }
        let active = expect_btw(&mut rx, "old active");
        let waiting = app
            .btw
            .status_items()
            .into_iter()
            .find(|i| i.question == "old waiting")
            .unwrap()
            .id
            .clone();
        crate::app::events::handle_client_event(
            &mut app,
            session_replacement("new-session", directory.path(), vec![], None),
        );
        drain_session_refreshes(&mut rx, "new-session");
        app.input.set_text("/btw new question");
        submit_input(&mut app);
        let BridgeCommand::SideQuestion { session_id, btw_id, question } =
            rx.try_recv().unwrap().command
        else {
            panic!("new session side question");
        };
        assert_eq!(session_id, "new-session");
        assert_eq!(question, "new question");
        for (id, question) in [(&active, "old active"), (&waiting, "old waiting")] {
            answer_btw(&mut app, id, question, "obsolete answer");
            fail_btw(&mut app, id, question, "obsolete failure");
        }
        assert!(exchanges(&app).is_empty());
        assert!(app.btw.get_active(&btw_id).is_some());
        assert!(rx.try_recv().is_err());
        crate::app::events::handle_client_event(
            &mut app,
            ClientEvent::BtwResult {
                session_id: "new-session".into(),
                btw_id,
                question,
                answer: "New owned answer".into(),
                metadata: crate::agent::wire::SideQuestionMetadata {
                    synthetic: false,
                    refusal_fallback: None,
                },
            },
        );
        assert_eq!(exchanges(&app), [("new question".into(), "New owned answer".into())]);
        assert!(app.btw.status_items().iter().all(|i| !i.question.starts_with("old")));
        assert!(rx.try_recv().is_err());
    }
}
