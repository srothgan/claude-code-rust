// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::agent::{model, wire::BridgeCommand};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

struct ShutdownFixture {
    app: App,
    commands: crate::agent::client::CommandReceiver,
    permission: tokio::sync::oneshot::Receiver<model::RequestPermissionResponse>,
    question: tokio::sync::oneshot::Receiver<model::RequestQuestionResponse>,
    received: ChatMessageId,
}

fn shutdown_fixture(kind: &str) -> ShutdownFixture {
    let (mut app, commands) = test_support::app_with_connection();
    app.status = AppStatus::Running;
    app.session_runtime.activate_session(model::SessionId::new("session-1"));
    let tools = [("shutdown-permission", "Write"), ("shutdown-question", "AskUserQuestion")].map(
        |(id, name)| {
            MessageBlock::ToolCall(Box::new(test_support::named_tool_call_info(
                id,
                name,
                model::ToolCallStatus::InProgress,
            )))
        },
    );
    app.push_message_tracked(ChatMessage::new(MessageRole::Assistant, tools.into(), None));
    for (index, id) in ["shutdown-permission", "shutdown-question"].into_iter().enumerate() {
        app.index_tool_call(id.into(), 0, index);
    }
    app.bind_active_turn_assistant(0);
    app.terminal_lifecycle = TerminalLifecycleState::Running(SurfaceMode::Chat);
    let received = ChatMessage::new(
        MessageRole::Assistant,
        vec![MessageBlock::Text(TextBlock::from_complete("RECEIVED_BEFORE_SHUTDOWN"))],
        None,
    );
    let received_id = received.id;
    app.push_message_tracked(received);
    app.input.set_text("UNSENT_DRAFT");
    let image = clipboard_image::encode_clipboard_image(arboard::ImageData {
        width: 1,
        height: 1,
        bytes: std::borrow::Cow::Owned(vec![12, 34, 56, 255]),
    })
    .unwrap();
    app.input.insert_parts([input::InputYankPart::Image(image)]);
    if kind == "deferred" {
        handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        );
        assert!(app.pending_submit.is_some());
    } else {
        handle_terminal_event(&mut app, Event::Paste("QUEUED_PASTE".into()));
        assert!(app.paste.has_pending_text());
        if kind == "fullscreen" {
            view::set_fullscreen_view(&mut app, FullscreenView::Config);
            assert!(app.input.text().contains("QUEUED_PASTE"));
        }
    }
    let permission = test_support::request_permission(
        &mut app,
        model::RequestPermissionRequest::new(
            "session-1",
            model::ToolCallUpdate::new("shutdown-permission", model::ToolCallUpdateFields::new()),
            test_support::permission_options("shutdown"),
            None,
        ),
    );
    let (response_tx, question) = tokio::sync::oneshot::channel();
    handle_client_event(
        &mut app,
        ClientEvent::QuestionRequest {
            session_id: "session-1".into(),
            request: model::RequestQuestionRequest::new(
                "session-1",
                model::ToolCallUpdate::new("shutdown-question", model::ToolCallUpdateFields::new()),
                model::QuestionPrompt::new(
                    "Choose a shutdown destination",
                    "Destination",
                    false,
                    vec![
                        model::QuestionOption::new("oak", "Oak"),
                        model::QuestionOption::new("elm", "Elm"),
                    ],
                ),
                0,
                1,
            ),
            response_tx,
        },
    );
    ShutdownFixture { app, commands, permission, question, received: received_id }
}

#[test]
fn lifecycle_shutdown_discards_unsubmitted_work_and_rejects_interactions() {
    for kind in ["deferred", "queued-paste", "fullscreen"] {
        let ShutdownFixture { mut app, mut commands, mut permission, mut question, received } =
            shutdown_fixture(kind);
        test_support::assert_pending(&mut permission);
        test_support::assert_pending(&mut question);
        if kind == "deferred" {
            assert!(app.pending_submit.is_some());
        }
        if kind == "queued-paste" {
            assert!(app.paste.has_pending_text());
        }
        while let Ok(command) = commands.try_recv() {
            assert!(!matches!(command.command, BridgeCommand::Prompt { .. }));
        }
        prepare_app_shutdown(&mut app);
        assert!(app.shutdown_requested());
        assert_eq!(app.surface_mode, SurfaceMode::Chat);
        assert_eq!(
            app.session_runtime.resumable_session_id().map(ToString::to_string),
            Some("session-1".into())
        );
        assert_eq!(
            permission.try_recv().unwrap().outcome,
            model::RequestPermissionOutcome::Selected(model::SelectedPermissionOutcome::new(
                "shutdown-deny"
            ))
        );
        assert_eq!(question.try_recv().unwrap().outcome, model::RequestQuestionOutcome::Cancelled);
        assert!(matches!(commands.try_recv().expect("cancel active session").command,
            BridgeCommand::CancelTurn { session_id } if session_id == "session-1"));
        assert!(commands.try_recv().is_err());
        for event in [
            Event::Paste("AFTER_SHUTDOWN".into()),
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        ] {
            handle_terminal_event(&mut app, event);
        }
        assert!(app.input.is_empty());
        assert!(app.input.image_payloads.is_empty());
        assert!(app.pending_submit.is_none());
        assert!(!app.paste.has_pending_text());
        assert!(commands.try_recv().is_err(), "shutdown input must not dispatch a Prompt");
        let retained = app
            .transcript
            .messages
            .iter()
            .find(|message| message.id == received)
            .expect("received history remains addressable");
        assert!(retained.blocks.iter().any(|block| matches!(block,
            MessageBlock::Text(text) if text.text == "RECEIVED_BEFORE_SHUTDOWN")));
    }
}

fn refresh_motion_policy(
    app: &mut App,
    commands: &mut crate::agent::client::CommandReceiver,
    reduced: Option<bool>,
) {
    use crate::agent::settings::{
        SettingsApplication, SettingsPersistence, SettingsResult, SettingsSnapshot,
    };
    config::request_settings(app);
    let request = commands.try_recv().expect("inspect effective preferences");
    assert!(
        matches!(request.command, BridgeCommand::InspectSettings { ref session_id, .. } if session_id == "sid-47")
    );
    let mut snapshot = reduced.map_or_else(SettingsSnapshot::default, |value| {
        SettingsSnapshot::test_value("prefersReducedMotion", serde_json::json!(value))
    });
    snapshot.cwd.clone_from(&app.cwd_raw);
    handle_client_event(
        app,
        ClientEvent::SettingsResultReceived {
            session_id: "sid-47".into(),
            request_id: request.request_id,
            result: SettingsResult {
                persistence: SettingsPersistence::Saved,
                application: SettingsApplication::Host,
                snapshot: Some(snapshot),
                error: None,
            },
        },
    );
}

fn motion_rows(app: &mut App, id: &HistoryOutputId, now: Instant) -> Vec<String> {
    match app.status {
        AppStatus::Running => crate::ui::activity_rows::build_activity_rows(app, 120, now)
            .iter()
            .map(ToString::to_string)
            .collect(),
        AppStatus::CommandPending => crate::ui::input_rows::blocked_input_lines(
            app,
            ComposerBlockReason::CommandPending,
            120,
        )
        .unwrap()
        .iter()
        .map(ToString::to_string)
        .collect(),
        _ => {
            let rows = crate::ui::inline_chat_rows::serialize_live_rows_at(
                app,
                120,
                &std::collections::BTreeSet::new(),
                now,
                &crate::ui::layout_control::LayoutControl::uninterrupted(),
            );
            test_support::output_region(&rows, id).1
        }
    }
}

#[test]
fn lifecycle_reduced_motion_preserves_status_and_reset_restores_animation() {
    let base = Instant::now();
    for status in
        [AppStatus::Connecting, AppStatus::Ready, AppStatus::Running, AppStatus::CommandPending]
    {
        let (mut app, id) =
            test_support::welcome_lifecycle_app(base, status != AppStatus::Connecting, Some(false));
        app.status = status;
        let (connection, mut commands) = crate::agent::client::AgentConnection::test_channel();
        app.session_runtime.conn = Some(std::rc::Rc::new(connection));
        app.session_runtime.activate_session(model::SessionId::new("sid-47"));
        let expected = match status {
            AppStatus::Running => {
                app.begin_turn_activity(base);
                activity::testing::choices(&mut app)
            }
            AppStatus::CommandPending => {
                app.turn.pending_command_label = Some("Apply garden preference".into());
                "Apply garden preference"
            }
            _ => "8.6.4",
        };
        let mut phases = Vec::new();
        for (start, preference) in [(0, Some(false)), (400, Some(true)), (800, None)] {
            refresh_motion_policy(&mut app, &mut commands, preference);
            let mut frames = Vec::new();
            for millis in (start..=start + 350).step_by(35) {
                let now = base + Duration::from_millis(millis);
                schedule_animation_frame(&mut app, now);
                let rows = motion_rows(&mut app, &id, now);
                assert!(rows.iter().any(|row| row.contains(expected)), "{status:?}: {rows:?}");
                frames.push(rows);
            }
            phases.push(frames);
        }
        assert!(
            phases[0].windows(2).any(|frames| frames[0] != frames[1]),
            "{status:?} must animate"
        );
        assert!(
            phases[1].iter().all(|frame| frame == &phases[1][0]),
            "{status:?} reduced motion must be stable"
        );
        assert!(
            phases[2].windows(2).any(|frames| frames[0] != frames[1]),
            "{status:?} reset must permit motion"
        );
    }
}

fn scheduled_welcome_rows(
    app: &mut App,
    now: Instant,
) -> Option<crate::ui::inline_chat_rows::SerializedLiveRows> {
    schedule_animation_frame(app, now);
    std::mem::take(&mut app.surface_dirty.chat.repaint).then(|| {
        let width = app.chat_render.terminal_size().width;
        crate::ui::inline_chat_rows::serialize_live_rows_at(
            app,
            width,
            &std::collections::BTreeSet::new(),
            now,
            &crate::ui::layout_control::LayoutControl::uninterrupted(),
        )
    })
}

#[test]
fn lifecycle_scheduled_welcome_releases_final_rows_and_then_quiesces() {
    let base = Instant::now();
    for (motion, width, fullscreen) in [
        (Some(false), 120, false),
        (None, 120, false),
        (Some(false), 36, false),
        (Some(false), 120, true),
    ] {
        let (mut app, id) = test_support::welcome_lifecycle_app(base, true, motion);
        app.chat_render.observe_terminal_size(width, 34);
        app.surface_dirty.chat.repaint = false;
        let initial = crate::ui::inline_chat_rows::serialize_live_rows_at(
            &mut app,
            width,
            &std::collections::BTreeSet::new(),
            base,
            &crate::ui::layout_control::LayoutControl::uninterrupted(),
        );
        let narrow = width == 36;
        assert_eq!(test_support::output_region(&initial, &id).0.commit_ready, narrow);
        assert_eq!(
            scheduled_welcome_rows(&mut app, base + Duration::from_millis(100)).is_some(),
            !narrow
        );
        if fullscreen {
            view::set_fullscreen_view(&mut app, FullscreenView::Config);
            assert_eq!(
                app.terminal_lifecycle,
                TerminalLifecycleState::Running(SurfaceMode::Fullscreen(FullscreenView::Config))
            );
            // Consume the actual transition draw, including a coalesced final request.
            schedule_animation_frame(&mut app, base + Duration::from_millis(200));
            app.surface_dirty.fullscreen.redraw = false;
            for millis in [500, 2_000] {
                assert!(!schedule_animation_frame(&mut app, base + Duration::from_millis(millis)));
                assert!(
                    !app.surface_dirty.fullscreen.redraw,
                    "welcome must not animate the fullscreen view"
                );
            }
            view::set_chat_surface(&mut app);
        }
        let final_frame = scheduled_welcome_rows(&mut app, base + Duration::from_secs(2));
        if narrow {
            assert!(final_frame.is_none());
        }
        let final_frame = final_frame.as_ref().unwrap_or(&initial);
        let (segment, text) = test_support::output_region(final_frame, &id);
        assert!(segment.commit_ready);
        assert!(final_frame.stable_row_count() >= segment.end_row);
        for value in ["8.6.4", "Orchid", "/garden", "sid-47"] {
            assert!(text.iter().any(|row| row.contains(value)), "{text:?}");
        }
        for seconds in [3, 60] {
            assert!(
                scheduled_welcome_rows(&mut app, base + Duration::from_secs(seconds)).is_none()
            );
        }
    }
}
