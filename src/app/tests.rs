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
    app.pending_images.push(crate::app::clipboard_image::ImageAttachment {
        data: "image".to_owned(),
        mime_type: "image/png".to_owned(),
    });
    view::set_surface_mode(&mut app, SurfaceMode::Fullscreen(FullscreenView::Config));

    prepare_app_shutdown(&mut app);

    assert!(app.shutdown_requested());
    assert_eq!(app.surface_mode, SurfaceMode::Chat);
    assert!(app.input.is_empty());
    assert!(app.pending_submit.is_none());
    assert!(app.pending_images.is_empty());
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
fn pending_paste_chunks_are_merged_before_threshold_check() {
    let mut app = App::test_default();
    let first = "a".repeat(700);
    let second = "b".repeat(401);
    events::handle_terminal_event(&mut app, Event::Paste(first.clone()));
    events::handle_terminal_event(&mut app, Event::Paste(second.clone()));

    // Not applied until post-drain finalization.
    assert!(app.input.is_empty());
    assert!(!app.paste.pending_text.is_empty());

    finalize_pending_paste_event(&mut app);

    assert_eq!(app.input.lines(), vec!["[Pasted Text 1 - 1101 chars]"]);
    assert_eq!(app.input.text(), format!("{first}{second}"));
}

#[test]
fn pending_paste_chunk_appends_to_same_session_placeholder() {
    let mut app = App::test_default();
    app.input.insert_paste_block("a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk");
    app.paste.active_session = Some(state::PasteSessionState {
        id: 7,
        start: SelectionPoint { row: 0, col: 0 },
        placeholder_index: Some(0),
    });
    app.paste.pending_session = app.paste.active_session;
    app.paste.pending_text = "\nl\nm".to_owned();

    finalize_pending_paste_event(&mut app);

    assert_eq!(app.input.lines(), vec!["[Pasted Text 1 - 25 chars]"]);
    assert_eq!(app.input.text(), "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\nm");
}

#[test]
fn pending_paste_exact_1000_chars_stays_inline() {
    let mut app = App::test_default();
    app.paste.pending_text = "x".repeat(1000);

    finalize_pending_paste_event(&mut app);

    assert_eq!(app.input.lines(), vec!["x".repeat(1000)]);
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
fn pending_paste_finalization_marks_redraw() {
    let mut app = App::test_default();
    app.surface_dirty.chat.repaint = false;
    app.paste.pending_text = "hello\nworld".to_owned();

    finalize_pending_paste_event(&mut app);

    assert!(app.surface_dirty.chat.repaint);
    assert_eq!(app.input.lines(), vec!["hello", "world"]);
}

#[test]
fn suppressed_enter_preserves_multiline_inline_paste() {
    let mut app = App::test_default();
    let t0 = Instant::now();

    assert_eq!(app.paste.burst.on_char('a', t0), paste_burst::CharAction::Passthrough('a'));
    let _ = app.input.textarea_insert_char('a');
    assert_eq!(
        app.paste.burst.on_char('b', t0 + Duration::from_millis(2)),
        paste_burst::CharAction::Consumed
    );
    assert_eq!(
        app.paste.burst.on_char('c', t0 + Duration::from_millis(4)),
        paste_burst::CharAction::RetroCapture(1)
    );
    let _ = app.input.textarea_delete_char_before();

    let t_flush = t0 + Duration::from_millis(200);
    assert_eq!(
        app.paste.burst.tick(t_flush),
        Some(paste_burst::FlushAction::EmitPaste("abc".to_owned()))
    );
    app.queue_paste_text("abc");
    finalize_pending_paste_event(&mut app);
    assert_eq!(app.input.text(), "abc");

    let t_enter = t_flush + Duration::from_millis(10);
    assert!(app.paste.burst.on_enter(t_enter));
    assert_eq!(
        app.paste.burst.on_char('d', t_enter + Duration::from_millis(1)),
        paste_burst::CharAction::Consumed
    );
    assert_eq!(
        app.paste.burst.on_char('e', t_enter + Duration::from_millis(2)),
        paste_burst::CharAction::Consumed
    );
    assert_eq!(
        app.paste.burst.on_char('f', t_enter + Duration::from_millis(3)),
        paste_burst::CharAction::Consumed
    );

    let t_second_flush = t_enter + Duration::from_millis(200);
    assert_eq!(
        app.paste.burst.tick(t_second_flush),
        Some(paste_burst::FlushAction::EmitPaste("\ndef".to_owned()))
    );
    app.queue_paste_text("\ndef");
    finalize_pending_paste_event(&mut app);

    assert_eq!(app.input.lines(), vec!["abc", "def"]);
    assert_eq!(app.input.text(), "abc\ndef");
}

#[test]
fn pending_paste_1001_chars_becomes_placeholder() {
    let mut app = App::test_default();
    app.paste.pending_text = "x".repeat(1001);

    finalize_pending_paste_event(&mut app);

    assert_eq!(app.input.lines(), vec!["[Pasted Text 1 - 1001 chars]"]);
    assert_eq!(app.input.text(), "x".repeat(1001));
}

#[test]
fn pending_paste_session_isolation_prevents_unintended_append() {
    let mut app = App::test_default();
    app.paste.pending_text = "a".repeat(1001);
    finalize_pending_paste_event(&mut app);
    events::handle_terminal_event(
        &mut app,
        Event::Key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('v'),
            crossterm::event::KeyModifiers::CONTROL,
        )),
    );

    app.paste.pending_text = "b".repeat(1001);
    finalize_pending_paste_event(&mut app);

    assert_eq!(app.input.lines(), vec!["[Pasted Text 1 - 1001 chars][Pasted Text 2 - 1001 chars]"]);
    assert_eq!(app.input.text(), format!("{}{}", "a".repeat(1001), "b".repeat(1001)));
}

#[test]
fn plain_enter_preserves_single_line_draft_before_submit() {
    let (mut app, mut rx) = app_with_connection();
    app.input.set_text("hello world");
    let _ = app.input.set_cursor(0, "hello".chars().count());

    events::handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    assert_eq!(app.input.text(), "hello world");
    assert_eq!(app.input.cursor(), (0, "hello".chars().count()));
    assert!(app.pending_submit.is_some());

    finalize_deferred_submit(&mut app);

    assert!(app.pending_submit.is_none());
    assert!(app.input.text().is_empty());
    assert_eq!(app.transcript.messages.len(), 2);
    assert!(matches!(app.transcript.messages[0].role, MessageRole::User));
    assert!(matches!(
        app.transcript.messages[0].blocks.as_slice(),
        [MessageBlock::Text(block)] if block.text == "hello world"
    ));
    let envelope = rx.try_recv().expect("prompt command should be sent");
    assert!(matches!(
        envelope.command,
        BridgeCommand::Prompt { session_id, .. } if session_id == "session-1"
    ));
}

#[test]
fn compaction_allows_drafting_but_blocks_submit() {
    let (mut app, mut rx) = app_with_connection();
    app.turn.compaction.begin();
    app.input.set_text("draft");

    events::handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('!'), KeyModifiers::NONE)),
    );

    assert_eq!(app.input.text(), "draft!");

    events::handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );
    assert!(app.pending_submit.is_some());

    finalize_deferred_submit(&mut app);

    assert!(app.pending_submit.is_none());
    assert_eq!(app.input.text(), "draft!");
    let [message] = app.transcript.messages.as_slice() else {
        panic!("expected one active-turn submission notice");
    };
    assert!(matches!(message.role, MessageRole::System(Some(SystemSeverity::Info))));
    let [MessageBlock::Notice(notice)] = message.blocks.as_slice() else {
        panic!("expected informational notice block");
    };
    assert!(notice.text.text.contains("between agent turns"));
    assert!(rx.try_recv().is_err());
}

#[test]
fn compaction_allows_paste_drafting() {
    let (mut app, _rx) = app_with_connection();
    app.turn.compaction.begin();

    events::handle_terminal_event(&mut app, Event::Paste(" pasted".into()));

    assert_eq!(app.paste.pending_text, " pasted");
}

#[test]
fn plain_enter_preserves_multiline_draft_with_mid_buffer_cursor() {
    let (mut app, mut rx) = app_with_connection();
    app.input.set_text("alpha beta\ngamma");
    let _ = app.input.set_cursor(0, "alpha".chars().count());

    events::handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    assert_eq!(app.input.text(), "alpha beta\ngamma");
    assert_eq!(app.input.cursor(), (0, "alpha".chars().count()));
    assert!(app.pending_submit.is_some());

    finalize_deferred_submit(&mut app);

    assert!(app.pending_submit.is_none());
    assert!(matches!(
        app.transcript.messages[0].blocks.as_slice(),
        [MessageBlock::Text(block)] if block.text == "alpha beta\ngamma"
    ));
    let envelope = rx.try_recv().expect("prompt command should be sent");
    assert!(matches!(
        envelope.command,
        BridgeCommand::Prompt { session_id, .. } if session_id == "session-1"
    ));
}

#[test]
fn sending_lone_question_mark_submits_as_prompt() {
    let (mut app, mut rx) = app_with_connection();

    events::handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE)),
    );

    assert_eq!(app.input.text(), "?");

    events::handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );
    assert!(app.pending_submit.is_some());

    finalize_deferred_submit(&mut app);

    assert!(app.pending_submit.is_none());
    assert!(app.input.text().is_empty());
    assert!(matches!(
        app.transcript.messages[0].blocks.as_slice(),
        [MessageBlock::Text(block)] if block.text == "?"
    ));
    let envelope = rx.try_recv().expect("prompt command should be sent");
    assert!(matches!(
        envelope.command,
        BridgeCommand::Prompt { session_id, .. } if session_id == "session-1"
    ));
}

#[test]
fn docs_topic_selection_submits_on_the_first_enter() {
    let mut app = App::test_default();
    app.input.set_text("/docs co");
    let _ = app.input.set_cursor(0, "/docs co".chars().count());
    crate::app::slash::sync_with_cursor(&mut app);

    assert!(app.slash.is_visible(), "topic autocomplete should be active before selection");
    assert_eq!(app.focus_owner(), FocusOwner::Mention);

    events::handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    assert_eq!(app.input.text(), "/docs commands ");
    assert!(app.slash.visible().is_none(), "topic selection should leave slash mode");
    assert_eq!(app.focus_owner(), FocusOwner::Input);

    assert!(app.pending_submit.is_some(), "first Enter should arm submit");

    finalize_deferred_submit(&mut app);

    assert!(app.pending_submit.is_none());
    let last = app.transcript.messages.last().expect("expected docs system message");
    assert!(matches!(last.role, MessageRole::System(_)));
    assert!(matches!(
        last.blocks.as_slice(),
        [MessageBlock::Text(block)] if block.text.contains("| Command | Description |")
    ));
}

#[test]
fn docs_name_waits_for_a_topic_then_topic_enter_submits() {
    let mut app = App::test_default();
    app.input.set_text("/do");
    let _ = app.input.set_cursor(0, "/do".chars().count());
    crate::app::slash::sync_with_cursor(&mut app);

    assert!(app.slash.is_visible(), "command autocomplete should be active before selection");
    assert_eq!(app.focus_owner(), FocusOwner::Mention);

    events::handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    assert!(app.pending_submit.is_none(), "command name still needs a topic");
    assert_eq!(app.input.text(), "/docs ");
    let slash = app.slash.visible().expect("topic autocomplete should activate");
    assert!(matches!(slash.context, crate::app::slash::SlashContext::Argument { .. }));
    assert_eq!(app.focus_owner(), FocusOwner::Mention);

    for _ in 0..3 {
        events::handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
        );
    }

    events::handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    assert_eq!(app.input.text(), "/docs commands ");
    assert!(app.slash.visible().is_none(), "topic selection should leave slash mode");
    assert_eq!(app.focus_owner(), FocusOwner::Input);

    assert!(app.pending_submit.is_some(), "submit should arm after topic selection");

    finalize_deferred_submit(&mut app);

    let last = app.transcript.messages.last().expect("expected docs system message");
    assert!(matches!(
        last.blocks.as_slice(),
        [MessageBlock::Text(block)] if block.text.contains("| Command | Description |")
    ));
}

#[test]
fn mode_selection_arms_submit_on_the_first_enter() {
    let mut app = App::test_default();
    app.session_runtime.mode = Some(ModeState {
        current_mode_id: "code".to_owned(),
        current_mode_name: "Code".to_owned(),
        available_modes: vec![
            ModeInfo { id: "plan".to_owned(), name: "Plan".to_owned() },
            ModeInfo { id: "code".to_owned(), name: "Code".to_owned() },
        ],
    });
    app.input.set_text("/mode pl");
    let _ = app.input.set_cursor(0, "/mode pl".chars().count());
    crate::app::slash::sync_with_cursor(&mut app);

    events::handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    assert_eq!(app.input.text(), "/mode plan ");
    assert!(app.slash.visible().is_none());
    assert_eq!(app.focus_owner(), FocusOwner::Input);

    assert!(app.pending_submit.is_some());
}

#[test]
fn model_selection_arms_submit_on_the_first_enter() {
    let mut app = App::test_default();
    app.sdk_inventory.available_models = vec![
        model::AvailableModel::new("sonnet", "Claude Sonnet"),
        model::AvailableModel::new("haiku", "Claude Haiku"),
    ];
    app.input.set_text("/model so");
    let _ = app.input.set_cursor(0, "/model so".chars().count());
    crate::app::slash::sync_with_cursor(&mut app);

    events::handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    assert_eq!(app.input.text(), "/model sonnet ");
    assert!(app.slash.visible().is_none());
    assert_eq!(app.focus_owner(), FocusOwner::Input);

    assert!(app.pending_submit.is_some());
}

#[test]
fn paste_event_cancels_deferred_submit_snapshot() {
    let mut app = App::test_default();
    app.input.set_text("draft");

    events::handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );
    assert!(app.pending_submit.is_some());

    events::handle_terminal_event(&mut app, Event::Paste("pasted".into()));

    assert!(app.pending_submit.is_none());
    assert_eq!(app.paste.pending_text, "pasted");
    assert_eq!(app.input.text(), "draft");
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
    let frame = app.spinner_frame;
    for elapsed in [95, 121, 5000] {
        advance_spinner_frame(&mut app, base + Duration::from_millis(elapsed));
        assert_eq!(app.spinner_frame, frame);
        assert_eq!(crate::ui::SpinnerState::for_app(&app).icon(), "\u{25C6}");
    }
    app.config.snapshot = None;
    advance_spinner_frame(&mut app, base + Duration::from_millis(5001));
    assert_ne!(app.spinner_frame, frame);
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
