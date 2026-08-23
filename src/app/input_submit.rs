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
    app.slash = None;
    app.subagent = None;

    // No connection yet - can't submit
    let text = app.input.text();
    if text.trim().is_empty() {
        return;
    }
    app.session_runtime.prompt_suggestion = None;

    let submission = slash::ResolvedSubmission::resolve(text);
    if app.is_agent_turn_active() && submission.class().requires_idle_turn() {
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

    app.input.clear();
    dispatch_submission(app, submission);
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

fn dispatch_submission(app: &mut App, submission: slash::ResolvedSubmission) {
    if slash::try_handle_submission(app, &submission) {
        return;
    }
    dispatch_prompt_turn(app, submission.into_text());
}

fn dispatch_prompt_turn(app: &mut App, text: String) {
    // New turn started by user input: force-stop stale tool calls from older turns
    // so their spinners don't continue during this turn.
    let _ = app.finalize_in_progress_tool_calls(model::ToolCallStatus::Failed);

    let Some(conn) = app.session_runtime.conn.clone() else { return };
    let Some(sid) = app.session_runtime.session_id.clone() else {
        return;
    };
    let input_chars = text.chars().count();
    let session_id = sid.to_string();

    // Take pending images for this turn.
    let images = std::mem::take(&mut app.pending_images);

    let user_blocks = vec![MessageBlock::Text(TextBlock::from_complete(&text))];

    app.push_message_tracked(ChatMessage::new(MessageRole::User, user_blocks, None));
    // Create empty assistant message immediately -- message.rs shows thinking indicator
    app.push_message_tracked(ChatMessage::new(MessageRole::Assistant, Vec::new(), None));
    app.bind_active_turn_assistant_to_tail();
    app.enforce_history_retention_tracked();
    app.status = AppStatus::Thinking;

    // The text already contains [Image #N] badges from the textarea,
    // so the model can correlate user references with image attachments.
    match conn.prompt_with_images(sid.to_string(), text, images) {
        Ok(resp) => {
            crate::app::session_runtime::request_context_usage_refresh(app);
            tracing::info!(
                target: crate::logging::targets::APP_INPUT,
                event_name = "prompt_dispatched",
                message = "prompt dispatched to the bridge",
                outcome = "success",
                session_id = %session_id,
                input_chars,
                stop_reason = ?resp.stop_reason,
            );
        }
        Err(e) => {
            crate::app::events::handle_local_prompt_dispatch_error(app, &e.to_string());
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
    fn connecting_submission_preserves_the_draft() {
        let mut app = App::test_default();
        app.status = AppStatus::Connecting;
        app.input.set_text("draft while connecting");

        submit_input(&mut app);

        assert_eq!(app.input.text(), "draft while connecting");
    }

    #[test]
    fn submit_input_while_running_preserves_prompt_and_does_not_cancel() {
        let (mut app, mut rx) = app_with_connection();
        app.status = AppStatus::Running;
        app.transcript.messages.push(ChatMessage::new(MessageRole::Assistant, Vec::new(), None));
        app.bind_active_turn_assistant(0);
        app.input.set_text("next prompt");
        app.pending_images.push(crate::app::clipboard_image::ImageAttachment {
            data: "image-data".to_owned(),
            mime_type: "image/png".to_owned(),
        });
        let before = app.input.snapshot();

        submit_input(&mut app);

        assert_eq!(app.input.snapshot(), before);
        assert_eq!(app.pending_images.len(), 1);
        assert_eq!(app.pending_images[0].data, "image-data");
        assert_eq!(app.pending_images[0].mime_type, "image/png");
        assert!(!app.turn.cancel_requested);
        assert!(matches!(app.status, AppStatus::Running));
        assert!(rx.try_recv().is_err(), "rejected prompt must not dispatch or cancel");
        let [MessageBlock::Notice(notice)] = app.transcript.messages[0].blocks.as_slice() else {
            panic!("expected inline active-turn notice");
        };
        assert!(notice.text.text.contains("New prompts"));
        assert_eq!(notice.severity, super::super::SystemSeverity::Info);
    }

    #[test]
    fn repeated_active_turn_rejections_update_one_inline_notice() {
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
        assert!(rx.try_recv().is_err());
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
        app.sdk_inventory.available_commands =
            vec![model::AvailableCommand::new("/remote-command", "Remote command")];
        app.input.set_text("/remote-command");

        submit_input(&mut app);

        assert!(app.input.text().is_empty());
        assert!(matches!(app.status, AppStatus::Thinking));
        assert_eq!(app.transcript.messages.len(), 2);
        assert!(matches!(app.transcript.messages[0].role, MessageRole::User));
        assert!(matches!(app.transcript.messages[1].role, MessageRole::Assistant));
        let envelope = rx.try_recv().expect("advertised slash command should be sent");
        match envelope.command {
            BridgeCommand::Prompt { session_id, chunks } => {
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
    fn config_slash_submit_opens_config_without_prompt_turn() {
        let (mut app, mut rx) = app_with_connection();
        let dir = tempfile::tempdir().expect("tempdir");
        app.settings_home_override = Some(dir.path().to_path_buf());
        app.cwd_raw = dir.path().to_string_lossy().to_string();
        app.input.set_text("/config");

        submit_input(&mut app);

        assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::Config));
        assert!(app.input.text().is_empty());
        assert!(matches!(app.status, AppStatus::Ready));
        assert!(rx.try_recv().is_err(), "config open should not dispatch a prompt turn");
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
    fn rejected_prompt_is_not_submitted_when_the_active_turn_completes() {
        let (mut app, mut rx) = app_with_connection();
        app.status = AppStatus::Running;
        app.input.set_text("submit manually later");

        submit_input(&mut app);
        crate::app::events::handle_client_event(
            &mut app,
            ClientEvent::TurnComplete { session_id: "session-1".to_owned(), terminal_reason: None },
        );

        assert_eq!(app.input.text(), "submit manually later");
        assert!(matches!(app.status, AppStatus::Ready));
        while let Ok(envelope) = rx.try_recv() {
            assert!(!matches!(envelope.command, BridgeCommand::Prompt { .. }));
        }
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
        app.status = AppStatus::Ready;

        dispatch_prompt_turn(&mut app, "hello".into());

        assert!(app.transcript.messages.is_empty());
        assert!(matches!(app.status, AppStatus::Ready));
    }
}
