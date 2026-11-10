// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::agent::model;
use crate::agent::wire::BridgeCommand;
use crate::app::{MessageBlock, MessageRole};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

#[test]
fn shutdown_preparation_normalizes_ui_and_preserves_resume_identity() {
    let mut app = App::test_default();
    app.status = AppStatus::Running;
    app.session_runtime.activate_session(model::SessionId::new("session-123"));
    app.input.set_text("draft");
    app.pending_submit = Some(app.input.snapshot());
    app.input.image_payloads.push(crate::app::clipboard_image::ImageAttachment {
        data: "image".to_owned(),
        mime_type: "image/png".to_owned(),
    });
    view::set_surface_mode(&mut app, SurfaceMode::Fullscreen(FullscreenView::Config));

    prepare_app_shutdown(&mut app);

    assert!(app.shutdown_requested());
    assert_eq!(app.surface_mode, SurfaceMode::Chat);
    assert!(app.input.is_empty());
    assert!(app.pending_submit.is_none());
    assert!(app.input.image_payloads.is_empty());
    assert_eq!(
        app.session_runtime.resumable_session_id().map(ToString::to_string),
        Some("session-123".to_owned())
    );
}

#[tokio::test(start_paused = true)]
async fn event_loop_interval_waits_between_idle_ticks() {
    let mut interval = event_loop_interval();

    assert!(interval.tick().now_or_never().is_none());

    for _ in 0..3 {
        tokio::time::advance(EVENT_LOOP_TICK_INTERVAL).await;
        assert!(interval.tick().now_or_never().is_some());
        assert!(interval.tick().now_or_never().is_none());
    }
}

#[tokio::test(start_paused = true)]
async fn event_loop_interval_skips_missed_ticks() {
    let mut interval = event_loop_interval();

    tokio::time::advance(EVENT_LOOP_TICK_INTERVAL).await;
    interval.tick().await;
    tokio::time::advance(EVENT_LOOP_TICK_INTERVAL * 5).await;

    assert!(interval.tick().now_or_never().is_some());
    assert!(interval.tick().now_or_never().is_none());

    tokio::time::advance(EVENT_LOOP_TICK_INTERVAL).await;
    assert!(interval.tick().now_or_never().is_some());
}

#[tokio::test(start_paused = true)]
async fn ready_events_are_not_delayed_until_the_next_tick() {
    let mut interval = event_loop_interval();
    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
    event_tx.send(()).unwrap();

    let event_received = tokio::select! {
        _ = interval.tick() => false,
        event = event_rx.recv() => event.is_some(),
    };

    assert!(event_received);
}

fn app_with_connection() -> (App, crate::agent::client::CommandReceiver) {
    let mut app = App::test_default();
    let (connection, rx) = crate::agent::client::AgentConnection::test_channel();
    app.session_runtime.conn = Some(std::rc::Rc::new(connection));
    app.session_runtime.session_id = Some(model::SessionId::new("session-1"));
    (app, rx)
}

#[test]
fn timer_flushed_typing_repaints_and_submits_the_complete_draft() {
    let mut app = App::test_default();
    let (connection, mut commands) = crate::agent::client::AgentConnection::test_channel();
    app.session_runtime.conn = Some(std::rc::Rc::new(connection));
    app.session_runtime.activate_session(model::SessionId::new("session-1"));
    app.status = AppStatus::Ready;
    let started = Instant::now();
    assert_eq!(app.paste.burst.on_char('g', started), paste_burst::CharAction::Passthrough('g'));
    app.input.set_text("g");
    assert_eq!(
        app.paste.burst.on_char('o', started + Duration::from_millis(1)),
        paste_burst::CharAction::Consumed
    );
    app.surface_dirty.chat.repaint = false;
    let action =
        app.paste.burst.tick(started + Duration::from_millis(500)).expect("held character flush");

    apply_paste_burst_flush(&mut app, action);

    assert_eq!(app.input.text(), "go");
    assert!(app.surface_dirty.chat.repaint);
    input_submit::submit_input(&mut app);
    let BridgeCommand::Prompt { chunks, .. } =
        commands.try_recv().expect("prompt admitted").command
    else {
        panic!("expected prompt");
    };
    assert_eq!(chunks[0].value.as_str(), Some("go"));
    assert!(app.input.is_empty());
}

#[test]
fn enter_preserves_the_draft_until_one_exact_correlated_submission() {
    use self::route_submit_key as press;
    for (source, cursor) in [("hello world", (0, 5)), ("alpha beta\ngamma", (0, 5)), ("?", (0, 1))]
    {
        let (mut app, mut rx) = app_with_connection();
        if source == "?" {
            press(&mut app, KeyCode::Char('?'));
        } else {
            app.input.set_text(source);
            app.input.set_cursor(cursor.0, cursor.1);
        }
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.input.text(), source);
        assert_eq!(app.input.cursor(), cursor);
        assert!(rx.try_recv().is_err());
        finalize_deferred_submit(&mut app);
        assert!(app.input.is_empty());
        assert_eq!(app.surface_mode, SurfaceMode::Chat);
        let message_uuid = assert_complete_prompt(&mut rx, source);
        assert!(app.transcript.messages.iter().any(|m| m.role == MessageRole::User
            && matches!(m.blocks.as_slice(), [MessageBlock::Text(t)] if t.text == source && t.source_message_uuids == [message_uuid.clone()])));
        finalize_deferred_submit(&mut app);
        assert!(rx.try_recv().is_err());
    }
}

#[test]
fn compaction_allows_drafting_but_blocks_submit() {
    use self::route_submit_key as press;
    let (mut app, mut rx) = app_with_connection();
    app.push_message_tracked(ChatMessage::new(
        MessageRole::User,
        vec![MessageBlock::Text(TextBlock::from_complete("prior question"))],
        None,
    ));
    let update = |update| crate::agent::events::ClientEvent::SessionUpdate {
        session_id: "session-1".into(),
        update,
    };
    handle_client_event(
        &mut app,
        update(model::SessionUpdate::CompactionUpdate(model::CompactionUpdate::Started)),
    );
    app.input.set_text("draft");
    press(&mut app, KeyCode::Char('!'));
    events::handle_terminal_event(&mut app, Event::Paste(" pasted\u{754c}".into()));
    finalize_pending_paste_event(&mut app);
    press(&mut app, KeyCode::Left);
    press(&mut app, KeyCode::Char('?'));
    press(&mut app, KeyCode::Enter);
    finalize_deferred_submit(&mut app);
    assert_eq!(app.input.text(), "draft! pasted?\u{754c}");
    assert!(rx.try_recv().is_err());
    assert!(app.transcript.messages.iter().any(|m| m.blocks.iter().any(
        |b| matches!(b, MessageBlock::Notice(n) if n.text.text.contains("between agent turns"))
    )));
    handle_client_event(
        &mut app,
        update(model::SessionUpdate::CompactionUpdate(model::CompactionUpdate::Finished {
            result: model::CompactionResult::Success,
            error_code: None,
            error: None,
        })),
    );
    while let Ok(envelope) = rx.try_recv() {
        assert!(
            matches!(envelope.command, BridgeCommand::GetContextUsage { session_id } if session_id == "session-1"),
            "completion cannot submit without explicit Enter"
        );
    }
    press(&mut app, KeyCode::Enter);
    finalize_deferred_submit(&mut app);
    assert_complete_prompt(&mut rx, "draft! pasted?\u{754c}");
    assert!(app.transcript.messages.iter().any(
        |m| matches!(m.blocks.as_slice(), [MessageBlock::Text(t)] if t.text == "prior question")
    ));
}

#[test]
fn docs_name_waits_for_a_topic_then_topic_enter_submits() {
    use self::route_submit_key as press;
    for initial in ["/do", "/docs co"] {
        let (mut app, mut rx) = app_with_connection();
        app.input.set_text(initial);
        app.input.set_cursor(0, 0);
        press(&mut app, KeyCode::End);
        if initial == "/do" {
            press(&mut app, KeyCode::Enter);
            finalize_deferred_submit(&mut app);
            assert_eq!(app.input.text(), "/docs ");
            assert!(app.transcript.messages.is_empty());
            assert!(rx.try_recv().is_err());
        }
        let limit = app.slash.visible().expect("topics").candidates.len();
        for _ in 0..limit {
            let menu = app.slash.visible().expect("topics");
            if menu.candidates[menu.dialog.selected].insert_value == "commands" {
                break;
            }
            press(&mut app, KeyCode::Down);
        }
        let menu = app.slash.visible().expect("topics");
        assert_eq!(menu.candidates[menu.dialog.selected].insert_value, "commands");
        press(&mut app, KeyCode::Enter);
        finalize_deferred_submit(&mut app);
        assert!(rx.try_recv().is_err());
        assert!(app.transcript.messages.iter().any(|m| matches!(m.blocks.as_slice(), [MessageBlock::Text(t)] if t.text.contains("/model") && t.text.contains("/new-session") && t.text.contains("/docs"))));
        assert!(app.input.is_empty());
    }
}

#[test]
fn paste_event_cancels_deferred_submit_snapshot() {
    use self::route_submit_key as press;
    let (mut app, mut rx) = app_with_connection();
    app.input.set_text("draft|tail");
    app.input.set_cursor(0, 5);
    press(&mut app, KeyCode::Enter);
    assert!(app.pending_submit.is_some());
    events::handle_terminal_event(&mut app, Event::Paste("pasted\u{754c}".into()));
    finalize_pending_paste_event(&mut app);
    finalize_deferred_submit(&mut app);
    assert!(rx.try_recv().is_err());
    assert!(app.transcript.messages.is_empty());
    assert_eq!(app.input.text(), "draftpasted\u{754c}|tail");
    assert_eq!(app.input.cursor(), (0, 12));
    press(&mut app, KeyCode::Enter);
    finalize_deferred_submit(&mut app);
    assert_complete_prompt(&mut rx, "draftpasted\u{754c}|tail");
}

#[test]
fn esc_cancels_deferred_submit_snapshot_before_finalize() {
    let (mut app, mut rx) = app_with_connection();
    app.input.set_text("draft");

    events::handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );
    assert!(app.pending_submit.is_some());

    events::handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
    );

    assert!(app.pending_submit.is_none());
    finalize_deferred_submit(&mut app);
    assert_eq!(app.input.text(), "draft");
    assert!(app.transcript.messages.is_empty());
    assert!(rx.try_recv().is_err(), "Esc should prevent deferred submit dispatch");
}

#[test]
fn reduced_motion_stops_animation_and_reset_resumes_it() {
    let mut app = App::test_default();
    let base = Instant::now();
    advance_spinner_frame(&mut app, base);
    let animated = crate::ui::SpinnerState::for_app(&app).icon();
    advance_spinner_frame(&mut app, base + Duration::from_millis(40));
    assert_ne!(crate::ui::SpinnerState::for_app(&app).icon(), animated);
    app.config.snapshot = Some(crate::agent::settings::SettingsSnapshot::test_value(
        "prefersReducedMotion",
        serde_json::json!(true),
    ));
    app.status = AppStatus::CommandPending;
    app.turn.pending_command_label = Some("Apply workspace setting".into());
    let rendered = |app: &App| {
        crate::ui::input_rows::blocked_input_lines(
            app,
            crate::app::ComposerBlockReason::CommandPending,
            80,
        )
        .unwrap()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
    };
    let stable = rendered(&app);
    assert!(stable.iter().any(|r| r.contains("Apply workspace setting")));
    let frame = app.spinner_frame;
    for elapsed in [95, 121, 5000] {
        advance_spinner_frame(&mut app, base + Duration::from_millis(elapsed));
        assert_eq!(app.spinner_frame, frame);
        assert_eq!(rendered(&app), stable);
        assert_eq!(crate::ui::SpinnerState::for_app(&app).icon(), "\u{25C6}");
    }
    app.config.snapshot = None;
    advance_spinner_frame(&mut app, base + Duration::from_millis(5001));
    assert_ne!(app.spinner_frame, frame);
    assert_ne!(rendered(&app), stable);
    assert!(rendered(&app).iter().any(|r| r.contains("Apply workspace setting")));
    assert_ne!(crate::ui::SpinnerState::for_app(&app).icon(), "\u{25C6}");
}

/// A ready app whose first connect was at `connected`, with the welcome as
/// its whole transcript and the settings snapshot acknowledged.
fn welcomed_app(connected: Instant, reduced_motion: bool) -> App {
    let mut app = App::test_default();
    app.status = AppStatus::Ready;
    app.startup.mark_connected(connected);
    app.config.snapshot = Some(crate::agent::settings::SettingsSnapshot::test_value(
        "prefersReducedMotion",
        serde_json::json!(reduced_motion),
    ));
    app.transcript.messages.push(ChatMessage::welcome("1.2.3", "Pro", "/demo", "session-1"));
    app.chat_render.observe_terminal_size(110, 34);
    app
}

/// Runs one frame-loop scheduling step at `at` and reports whether it asked
/// for a draw, clearing the request for the next step.
fn scheduled_draw(app: &mut App, at: Instant) -> bool {
    schedule_animation_frame(app, at);
    std::mem::take(&mut app.surface_dirty.chat.repaint)
}

#[test]
fn welcome_hello_redraws_through_its_window_and_ends_with_exactly_one_more_draw() {
    let connected = Instant::now();
    let mut app = welcomed_app(connected, false);
    app.surface_dirty.chat.repaint = false;

    assert!(scheduled_draw(&mut app, connected + Duration::from_millis(100)));
    assert!(scheduled_draw(&mut app, connected + Duration::from_millis(1_199)));
    assert!(
        scheduled_draw(&mut app, connected + Duration::from_millis(1_200)),
        "the draw that commits the rested welcome"
    );
    assert!(!scheduled_draw(&mut app, connected + Duration::from_millis(1_216)));
    assert!(!scheduled_draw(&mut app, connected + Duration::from_secs(60)), "idle stays idle");
}

#[test]
fn a_welcome_held_back_without_a_wave_still_gets_the_draw_that_commits_it() {
    let connected = Instant::now();
    let mut app = welcomed_app(connected, false);
    app.config.snapshot = None;
    app.surface_dirty.chat.repaint = false;

    assert!(scheduled_draw(&mut app, connected + Duration::from_millis(100)));
    assert!(
        scheduled_draw(&mut app, connected + Duration::from_millis(1_200)),
        "the window closed with no wave drawn; this draw commits the welcome"
    );
    assert!(!scheduled_draw(&mut app, connected + Duration::from_millis(1_216)));
}

#[test]
fn a_narrow_terminal_neither_holds_the_welcome_nor_schedules_draws() {
    let connected = Instant::now();
    // 36 columns is the width at which the overview drops the crab column.
    let mut app = welcomed_app(connected, false);
    app.chat_render.observe_terminal_size(36, 34);
    app.surface_dirty.chat.repaint = false;

    assert!(!scheduled_draw(&mut app, connected + Duration::from_millis(100)));
    assert!(!scheduled_draw(&mut app, connected + Duration::from_millis(1_200)));
}

#[test]
fn the_hello_does_not_redraw_a_fullscreen_view_over_the_welcome() {
    let connected = Instant::now();
    let mut app = welcomed_app(connected, false);
    app.terminal_lifecycle =
        TerminalLifecycleState::Running(SurfaceMode::Fullscreen(FullscreenView::SessionPicker));

    assert!(!frame_is_animating(&app, connected + Duration::from_millis(100)));
}

#[test]
fn reduced_motion_schedules_no_welcome_redraws() {
    let connected = Instant::now();
    let mut app = welcomed_app(connected, true);
    assert!(!frame_is_animating(&app, connected + Duration::from_millis(100)));

    app.status = AppStatus::Connecting;
    assert!(!frame_is_animating(&app, connected + Duration::from_millis(100)));
}

#[test]
fn a_stopped_animation_requests_no_extra_draw_once_the_transcript_has_more_than_the_welcome() {
    let mut app = App::test_default();
    app.status = AppStatus::Connecting;
    app.transcript.messages.push(ChatMessage::welcome("1.2.3", "Pro", "/demo", "session-1"));
    app.transcript.messages.push(ChatMessage::new(
        MessageRole::User,
        vec![MessageBlock::Text(crate::app::TextBlock::from_complete("hello"))],
        None,
    ));
    let start = Instant::now();
    app.surface_dirty.chat.repaint = false;
    assert!(scheduled_draw(&mut app, start), "Connecting animates");

    app.status = AppStatus::Ready;

    assert!(!scheduled_draw(&mut app, start + Duration::from_millis(16)));
}

/// The welcome's rows as text, from the real inline row serializer.
fn rendered_welcome_text(app: &mut App) -> String {
    let rows = crate::ui::inline_chat_rows::serialize_live_rows_with_boundaries_excluding(
        app,
        120,
        &std::collections::BTreeSet::new(),
        None,
    );
    rows.rows()
        .iter()
        .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_connecting_crab_changes_pose_every_192_ms_on_the_real_loop_tick() {
    let start = Instant::now();
    let mut app = App::test_default();
    app.status = AppStatus::Connecting;
    app.transcript.messages.push(ChatMessage::welcome("1.2.3", "-", "/demo", "-"));
    let mut shown = rendered_welcome_text(&mut app);
    let mut changes_ms = Vec::new();

    // The loop ticks every 16 ms; the spinner that paces the crab advances at
    // most once per 30 ms, so once per two ticks.
    for tick in 0..=70_u32 {
        let elapsed = EVENT_LOOP_TICK_INTERVAL * tick;
        schedule_animation_frame(&mut app, start + elapsed);
        let now_shown = rendered_welcome_text(&mut app);
        if now_shown != shown {
            changes_ms.push(elapsed.as_millis());
            shown = now_shown;
        }
    }

    // The first advance is on the first tick, so the first pose is shorter;
    // after that a pose lasts 6 spinner frames of 32 ms.
    assert_eq!(changes_ms, [160, 352, 544, 736, 928, 1120]);
}

fn drain_typing(app: &mut App, now: Instant) {
    if let Some(action) = app.paste.burst.tick(now) {
        apply_paste_burst_flush(app, action);
    }
    finalize_pending_paste_event(app);
    finalize_deferred_submit(app);
}

fn assert_paste_prompt(commands: &mut crate::agent::client::CommandReceiver, expected: &str) {
    let BridgeCommand::Prompt { session_id, chunks, .. } =
        commands.try_recv().expect("Prompt").command
    else {
        panic!("expected Prompt");
    };
    assert_eq!(session_id, "session-1");
    assert_eq!(
        chunks.iter().filter(|c| c.kind == "text").map(|c| c.value.clone()).collect::<Vec<_>>(),
        [serde_json::json!(expected)]
    );
    assert!(!chunks.iter().any(|c| c.kind == "image"));
    while let Ok(envelope) = commands.try_recv() {
        assert!(
            !matches!(envelope.command, BridgeCommand::Prompt { .. }),
            "only one intentional Prompt"
        );
    }
}

#[test]
fn slow_followup_typing_preserves_distinct_and_equal_held_characters() {
    use super::test_support::{EventClock, press};
    for (last, expected) in [('c', "\u{e9} abc|\u{5c3e}"), ('b', "\u{e9} abb|\u{5c3e}")] {
        let clock = EventClock::new();
        let (mut app, mut commands) = app_with_connection();
        app.status = AppStatus::Ready;
        app.input.set_text("\u{e9} |\u{5c3e}");
        app.input.set_cursor(0, 2);
        press(&mut app, KeyCode::Char('a'));
        clock.at_millis(2);
        press(&mut app, KeyCode::Char('b'));
        clock.at_millis(300);
        press(&mut app, KeyCode::Char(last));
        assert_eq!(app.input.text(), expected);
        drain_typing(&mut app, clock.at(1));
        assert_eq!(app.input.text(), expected);
        assert!(commands.try_recv().is_err());
        press(&mut app, KeyCode::Enter);
        assert_paste_prompt(&mut commands, expected);
    }
}

#[test]
fn moving_the_cursor_places_held_typing_before_the_next_edit() {
    use super::test_support::{EventClock, press};
    for typed in ["ab", "abc"] {
        let clock = EventClock::new();
        let (mut app, mut commands) = app_with_connection();
        app.status = AppStatus::Ready;
        app.input.set_text("\u{e9} |\u{5c3e}");
        app.input.set_cursor(0, 2);
        for ch in typed.chars() {
            press(&mut app, KeyCode::Char(ch));
        }
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Char('X'));
        let expected = format!("\u{e9} {typed}|X\u{5c3e}");
        drain_typing(&mut app, clock.at(1));
        assert_eq!(app.input.text(), expected);
        drain_typing(&mut app, clock.at(2));
        assert_eq!(app.input.text(), expected);
        assert!(commands.try_recv().is_err());
        press(&mut app, KeyCode::Enter);
        assert_paste_prompt(&mut commands, &expected);
    }
}

#[tokio::test]
async fn a_form_transition_preserves_held_chat_source_without_typing_into_the_form() {
    use super::test_support::{EventClock, press};
    use crate::agent::{
        events::ClientEvent,
        types::{ElicitationMode, ElicitationRequest},
    };
    for typed in ["ab", "abc"] {
        let clock = EventClock::new();
        let (mut app, mut commands) = app_with_connection();
        app.status = AppStatus::Ready;
        app.input.set_text("\u{e9} |\u{5c3e}");
        app.input.set_cursor(0, 2);
        for ch in typed.chars() {
            press(&mut app, KeyCode::Char(ch));
        }
        handle_client_event(
            &mut app,
            ClientEvent::McpElicitationRequest {
                session_id: "session-1".into(),
                request: ElicitationRequest {
                    request_id: "held-form".into(),
                    server_name: "forms".into(),
                    message: "Independent field".into(),
                    mode: ElicitationMode::Form,
                    url: None,
                    elicitation_id: None,
                    field_order: vec!["answer".into()],
                    requested_schema: Some(
                        serde_json::json!({"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"]}),
                    ),
                },
            },
        );
        assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::Config));
        press(&mut app, KeyCode::Char('Z'));
        let active = app.mcp.elicitations.active().expect("usable form");
        assert_eq!(
            active.form.as_ref().unwrap().content().unwrap()["answer"],
            serde_json::json!("Z")
        );
        handle_client_event(
            &mut app,
            ClientEvent::McpElicitationSettled {
                session_id: "session-1".into(),
                request_id: "held-form".into(),
                session_closing: false,
            },
        );
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.surface_mode, SurfaceMode::Chat);
        let expected = format!("\u{e9} {typed}|\u{5c3e}");
        drain_typing(&mut app, clock.at(1));
        assert_eq!(app.input.text(), expected);
        assert_eq!(app.input.cursor(), (0, 2 + typed.len()));
        drain_typing(&mut app, clock.at(2));
        assert_eq!(app.input.text(), expected);
        while let Ok(envelope) = commands.try_recv() {
            assert!(!matches!(envelope.command, BridgeCommand::Prompt { .. }));
        }
        press(&mut app, KeyCode::Enter);
        assert_paste_prompt(&mut commands, &expected);
    }
}

#[test]
fn paste_chunks_cross_the_character_threshold_without_losing_source_or_surroundings() {
    use super::test_support::press;
    let thousand = format!("START{}END", "\u{754c}".repeat(992));
    let thousand_one = format!("START{}END!", "\u{754c}".repeat(992));
    let first = format!("FIRST{}\n", "\u{e9}".repeat(594)); // 600 characters, not UTF-8 bytes.
    let second = format!("SECOND{}END", "Z".repeat(492)); // 501 characters.
    for (chunks, source, collapsed) in [
        (vec![thousand.clone()], thousand, false),
        (vec![thousand_one.clone()], thousand_one, true),
        (vec![first.clone(), second.clone()], format!("{first}{second}"), true),
    ] {
        let (mut app, mut commands) = app_with_connection();
        app.status = AppStatus::Ready;
        app.input.set_text("\u{e9} |\u{5c3e}");
        app.input.set_cursor(0, 2);
        for chunk in chunks {
            events::handle_terminal_event(&mut app, Event::Paste(chunk));
        }
        assert_eq!(app.input.text(), "\u{e9} |\u{5c3e}", "paste is applied after the drain");
        app.surface_dirty.chat.repaint = false;
        finalize_pending_paste_event(&mut app);
        let expected = format!("\u{e9} {source}|\u{5c3e}");
        assert_eq!(app.input.text(), expected);
        assert_eq!(app.input.inline_pastes(), if collapsed { vec![source] } else { vec![] });
        assert!(app.surface_dirty.chat.repaint);
        if collapsed {
            assert!(app.input.lines()[0].starts_with("\u{e9} [Pasted Text "));
            assert!(app.input.lines()[0].ends_with("|\u{5c3e}"));
        } else {
            assert_eq!(app.input.lines().join("\n"), expected);
        }
        assert!(commands.try_recv().is_err());
        press(&mut app, KeyCode::Enter);
        assert_paste_prompt(&mut commands, &expected);
    }
}

#[test]
fn timing_detected_unicode_pastes_cross_the_placeholder_boundary_without_losing_source() {
    use super::test_support::{EventClock, press};
    for (char_count, collapsed) in [(999, false), (1000, false), (1001, true)] {
        let clock = EventClock::new();
        let (mut app, mut commands) = app_with_connection();
        app.status = AppStatus::Ready;
        let suffix = format!(" EDGE_{char_count}");
        let prefix = "Unicode 界 🦀 ";
        let fixed_chars = prefix.chars().count() + suffix.chars().count();
        let source = format!("{prefix}{}{suffix}", "x".repeat(char_count - fixed_chars));

        for ch in source.chars() {
            clock.at_millis(0);
            press(&mut app, KeyCode::Char(ch));
        }
        drain_typing(&mut app, clock.at_millis(1000));

        if collapsed {
            let pastes = app.input.inline_pastes();
            assert_eq!(pastes.len(), 1);
            assert_eq!(pastes[0], source);
            assert!(
                app.input.lines().join("\n").contains("[Pasted Text 1 - 1001 chars]"),
                "draft lines after threshold paste: {:?}",
                app.input.lines()
            );
        } else {
            assert!(app.input.inline_pastes().is_empty());
            assert_eq!(app.input.text(), source);
        }
        assert!(commands.try_recv().is_err());
        clock.at_millis(2000);
        press(&mut app, KeyCode::Enter);
        assert_paste_prompt(&mut commands, &source);
    }
}

#[test]
fn paste_continuation_and_deleted_atoms_cannot_capture_an_independent_paste() {
    use super::test_support::press;
    let first = format!("FIRST{}END", "a".repeat(1000));
    let second = format!("SECOND{}FIN", "\u{754c}".repeat(1000));
    let replacement = format!("NEW{}LAST", "b".repeat(1000));
    for continuation in [true, false] {
        let (mut app, mut commands) = app_with_connection();
        app.status = AppStatus::Ready;
        app.input.set_text("\u{e9} |\u{5c3e}");
        app.input.set_cursor(0, 2);
        events::handle_terminal_event(&mut app, Event::Paste(first.clone()));
        finalize_pending_paste_event(&mut app);
        if !continuation {
            press(&mut app, KeyCode::Left);
            press(&mut app, KeyCode::Right);
        }
        events::handle_terminal_event(&mut app, Event::Paste(second.clone()));
        finalize_pending_paste_event(&mut app);
        let payload = if continuation {
            assert_eq!(app.input.inline_pastes(), [format!("{first}{second}")]);
            format!("{first}{second}")
        } else {
            assert_eq!(app.input.inline_pastes(), [first.clone(), second.clone()]);
            press(&mut app, KeyCode::Left);
            press(&mut app, KeyCode::Delete);
            assert_eq!(app.input.text(), format!("\u{e9} {first}|\u{5c3e}"));
            events::handle_terminal_event(&mut app, Event::Paste(replacement.clone()));
            finalize_pending_paste_event(&mut app);
            assert_eq!(app.input.inline_pastes(), [first.clone(), replacement.clone()]);
            format!("{first}{replacement}")
        };
        let expected = format!("\u{e9} {payload}|\u{5c3e}");
        assert_eq!(app.input.text(), expected);
        assert!(commands.try_recv().is_err());
        press(&mut app, KeyCode::Enter);
        assert_paste_prompt(&mut commands, &expected);
    }
}

#[test]
fn suppressed_enter_and_followup_keys_reach_the_eventual_prompt_once() {
    use super::test_support::{EventClock, press};
    let clock = EventClock::new();
    let (mut app, mut commands) = app_with_connection();
    app.status = AppStatus::Ready;
    app.input.set_text("prefix |suffix");
    app.input.set_cursor(0, 7);
    for (i, ch) in "abcd".chars().enumerate() {
        clock.at_millis(u64::try_from(i).unwrap() * 2);
        press(&mut app, KeyCode::Char(ch));
    }
    drain_typing(&mut app, clock.at(1));
    assert_eq!(app.input.text(), "prefix abcd|suffix");
    assert!(commands.try_recv().is_err());
    clock.at_millis(1001);
    press(&mut app, KeyCode::Enter);
    for ch in ['\u{e9}', 'f'] {
        press(&mut app, KeyCode::Char(ch));
    }
    assert!(commands.try_recv().is_err(), "suppressed Enter must not submit");
    drain_typing(&mut app, clock.at(2));
    assert_eq!(app.input.text(), "prefix abcd\n\u{e9}f|suffix");
    assert!(commands.try_recv().is_err());
    drain_typing(&mut app, clock.at(3));
    press(&mut app, KeyCode::Enter);
    assert_paste_prompt(&mut commands, "prefix abcd\n\u{e9}f|suffix");
}

#[test]
fn held_typing_counts_as_a_draft_when_left_arrow_is_routed() {
    use super::test_support::{EventClock, press};
    for (typed, expected) in [("ab", "aXb"), ("abc", "abXc")] {
        let clock = EventClock::new();
        let (mut app, mut commands) = app_with_connection();
        app.status = AppStatus::Ready;
        for ch in typed.chars() {
            press(&mut app, KeyCode::Char(ch));
        }
        press(&mut app, KeyCode::Left);
        assert_eq!(app.surface_mode, SurfaceMode::Chat, "held draft must not open Agents");
        press(&mut app, KeyCode::Char('X'));
        drain_typing(&mut app, clock.at(1));
        assert_eq!(app.input.text(), expected);
        assert!(commands.try_recv().is_err());
        press(&mut app, KeyCode::Enter);
        assert_paste_prompt(&mut commands, expected);
    }
}

#[test]
fn clearing_a_completed_burst_allows_immediate_submission_without_old_enter_suppression() {
    use super::test_support::{EventClock, press};
    let clock = EventClock::new();
    let (mut app, mut commands) = app_with_connection();
    app.status = AppStatus::Ready;
    for ch in "abc".chars() {
        press(&mut app, KeyCode::Char(ch));
    }
    drain_typing(&mut app, clock.at(1));
    assert_eq!(app.input.text(), "abc");
    clock.at_millis(1001);
    events::handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
    );
    assert!(app.input.is_empty());
    assert!(!app.shutdown_requested());
    events::handle_terminal_event(&mut app, Event::Paste("fresh immediately".into()));
    finalize_pending_paste_event(&mut app);
    press(&mut app, KeyCode::Enter);
    assert_paste_prompt(&mut commands, "fresh immediately");
    drain_typing(&mut app, clock.at(2));
    assert!(app.input.is_empty());
    while let Ok(envelope) = commands.try_recv() {
        assert!(!matches!(envelope.command, BridgeCommand::Prompt { .. }));
    }
}

fn assert_complete_prompt(
    rx: &mut crate::agent::client::CommandReceiver,
    expected: &str,
) -> String {
    let BridgeCommand::Prompt { session_id, message_uuid, chunks, .. } =
        rx.try_recv().expect("Prompt").command
    else {
        panic!("expected Prompt")
    };
    assert_eq!(session_id, "session-1");
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].kind, "text");
    assert_eq!(chunks[0].value, serde_json::json!(expected));
    while let Ok(envelope) = rx.try_recv() {
        assert!(matches!(envelope.command, BridgeCommand::GetContextUsage { .. }));
    }
    message_uuid
}

#[test]
fn bracketed_paste_delivers_held_source_before_its_payload_and_original_suffix_once() {
    use super::test_support::{EventClock, press};
    for (typed, expected) in
        [("a\u{e9}", "start a\u{e9}PASTE|tail"), ("a\u{e9}b", "start a\u{e9}bPASTE|tail")]
    {
        let clock = EventClock::new();
        let (mut app, mut rx) = app_with_connection();
        app.input.set_text("start |tail");
        app.input.set_cursor(0, 6);
        for ch in typed.chars() {
            press(&mut app, KeyCode::Char(ch));
        }
        assert!(app.paste.burst.has_held_input());
        events::handle_terminal_event(&mut app, Event::Paste("PASTE".into()));
        finalize_pending_paste_event(&mut app);
        finalize_deferred_submit(&mut app);
        assert_eq!(app.input.text(), expected);
        assert!(rx.try_recv().is_err());
        if let Some(action) = app.paste.burst.tick(clock.at(1)) {
            apply_paste_burst_flush(&mut app, action);
        }
        finalize_pending_paste_event(&mut app);
        assert_eq!(app.input.text(), expected);
        press(&mut app, KeyCode::Enter);
        finalize_deferred_submit(&mut app);
        assert_complete_prompt(&mut rx, expected);
        if let Some(action) = app.paste.burst.tick(clock.at(2)) {
            apply_paste_burst_flush(&mut app, action);
        }
        finalize_pending_paste_event(&mut app);
        finalize_deferred_submit(&mut app);
        assert!(app.input.is_empty());
        assert!(rx.try_recv().is_err());
    }
}

fn route_submit_key(app: &mut App, code: KeyCode) {
    events::handle_terminal_event(app, Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
}
