use super::*;
use crate::agent::model;
use crate::agent::wire::BridgeCommand;
use crate::app::{MessageBlock, MessageRole};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

fn app_with_connection()
-> (App, tokio::sync::mpsc::UnboundedReceiver<crate::agent::wire::CommandEnvelope>) {
    let mut app = App::test_default();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    app.session_runtime.conn =
        Some(std::rc::Rc::new(crate::agent::client::AgentConnection::new(tx)));
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
    app.turn.is_compacting = true;
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
    assert!(app.transcript.messages.is_empty());
    assert!(rx.try_recv().is_err());
}

#[test]
fn compaction_allows_paste_drafting() {
    let (mut app, _rx) = app_with_connection();
    app.turn.is_compacting = true;

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
fn docs_topic_selected_with_enter_then_second_enter_submits() {
    let mut app = App::test_default();
    app.input.set_text("/docs co");
    let _ = app.input.set_cursor(0, "/docs co".chars().count());
    crate::app::slash::sync_with_cursor(&mut app);

    assert!(app.slash.is_some(), "topic autocomplete should be active before selection");
    assert_eq!(app.focus_owner(), FocusOwner::Mention);

    events::handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    assert_eq!(app.input.text(), "/docs commands ");
    assert!(app.slash.is_none(), "topic selection should leave slash mode");
    assert_eq!(app.focus_owner(), FocusOwner::Input);

    events::handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    assert!(app.pending_submit.is_some(), "second Enter should arm submit");

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
fn docs_command_selection_then_topic_selection_then_submit_works_with_enter_only() {
    let mut app = App::test_default();
    app.input.set_text("/do");
    let _ = app.input.set_cursor(0, "/do".chars().count());
    crate::app::slash::sync_with_cursor(&mut app);

    assert!(app.slash.is_some(), "command autocomplete should be active before selection");
    assert_eq!(app.focus_owner(), FocusOwner::Mention);

    events::handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    assert_eq!(app.input.text(), "/docs ");
    let slash = app.slash.as_ref().expect("topic autocomplete should activate");
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
    assert!(app.slash.is_none(), "topic selection should leave slash mode");
    assert_eq!(app.focus_owner(), FocusOwner::Input);

    events::handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    assert!(app.pending_submit.is_some(), "submit should arm after topic selection");

    finalize_deferred_submit(&mut app);

    let last = app.transcript.messages.last().expect("expected docs system message");
    assert!(matches!(
        last.blocks.as_slice(),
        [MessageBlock::Text(block)] if block.text.contains("| Command | Description |")
    ));
}

#[test]
fn mode_selection_then_second_enter_arms_submit() {
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
    assert!(app.slash.is_none());
    assert_eq!(app.focus_owner(), FocusOwner::Input);

    events::handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    assert!(app.pending_submit.is_some());
}

#[test]
fn model_selection_then_second_enter_arms_submit() {
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
    assert!(app.slash.is_none());
    assert_eq!(app.focus_owner(), FocusOwner::Input);

    events::handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    assert!(app.pending_submit.is_some());
}

#[test]
fn resume_selection_then_second_enter_arms_submit() {
    let mut app = App::test_default();
    app.recent_sessions = vec![RecentSessionInfo {
        session_id: "session-1".to_owned(),
        summary: "Session one".to_owned(),
        last_modified_ms: 1,
        file_size_bytes: 1,
        cwd: None,
        git_branch: None,
        custom_title: None,
        first_prompt: None,
    }];
    app.input.set_text("/resume se");
    let _ = app.input.set_cursor(0, "/resume se".chars().count());
    crate::app::slash::sync_with_cursor(&mut app);

    events::handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

    assert_eq!(app.input.text(), "/resume session-1 ");
    assert!(app.slash.is_none());
    assert_eq!(app.focus_owner(), FocusOwner::Input);

    events::handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );

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
fn spinner_advances_less_frequently_when_reduced_motion_enabled() {
    let mut app = App::test_default();
    let base = Instant::now();

    advance_spinner_frame(&mut app, base);
    assert_eq!(app.spinner_frame, 1);
    advance_spinner_frame(&mut app, base + Duration::from_millis(40));
    assert_eq!(app.spinner_frame, 2);

    crate::app::config::store::set_prefers_reduced_motion(
        &mut app.config.committed_local_settings_document,
        true,
    );
    app.spinner_last_advance_at = None;
    app.spinner_frame = 0;

    advance_spinner_frame(&mut app, base);
    assert_eq!(app.spinner_frame, 1);
    advance_spinner_frame(&mut app, base + Duration::from_millis(95));
    assert_eq!(app.spinner_frame, 1);
    advance_spinner_frame(&mut app, base + Duration::from_millis(121));
    assert_eq!(app.spinner_frame, 2);
}
