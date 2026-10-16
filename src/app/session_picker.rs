// SPDX-License-Identifier: Apache-2.0

use super::connect::{begin_resume_session, begin_resume_session_at};
use super::events::push_system_message_with_severity;
use super::view;
use super::{App, AppStatus, RecentSessionInfo, SessionPickerState, SystemSeverity};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub(crate) fn open(app: &mut App) {
    app.session_picker = SessionPickerState::default();
    let current = app.session_runtime.session_id.as_ref().map(ToString::to_string);
    reconcile_selection(app, current.as_deref());
    view::set_fullscreen_view(app, super::FullscreenView::SessionPicker);
}

pub(crate) fn session_title(session: &RecentSessionInfo) -> &str {
    session
        .custom_title
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| session.first_prompt.as_deref().filter(|value| !value.trim().is_empty()))
        .or_else(|| (!session.summary.trim().is_empty()).then_some(session.summary.as_str()))
        .unwrap_or(&session.session_id)
}

pub(crate) fn picker_sessions(app: &App) -> impl Iterator<Item = &RecentSessionInfo> {
    let query = app.session_picker.query.trim().to_lowercase();
    app.recent_sessions.iter().filter(move |session| {
        query.is_empty()
            || session_title(session).to_lowercase().contains(&query)
            || session.session_id.to_lowercase().contains(&query)
            || session
                .git_branch
                .as_deref()
                .is_some_and(|branch| branch.to_lowercase().contains(&query))
    })
}

pub(crate) fn selected_session(app: &App) -> Option<&RecentSessionInfo> {
    picker_sessions(app).nth(app.session_picker.selected)
}

pub(crate) fn reconcile_selection(app: &mut App, previous_id: Option<&str>) {
    let selected = previous_id
        .and_then(|id| picker_sessions(app).position(|session| session.session_id == id))
        .unwrap_or_else(|| {
            app.session_picker.selected.min(picker_session_count(app).saturating_sub(1))
        });
    app.session_picker.selected = selected;
    app.session_picker.scroll_offset = app.session_picker.scroll_offset.min(selected);

    if app
        .session_picker
        .turn_session_id
        .as_ref()
        .is_some_and(|id| !app.recent_sessions.iter().any(|session| &session.session_id == id))
    {
        close_turn_selection(app);
    }
}

pub(crate) fn picker_session_count(app: &App) -> usize {
    picker_sessions(app).count()
}

pub(crate) fn startup_picker_is_loading(app: &App) -> bool {
    app.startup.startup_picker_is_loading(app.session_runtime.conn.is_some())
}

pub(crate) fn picker_turn_count(app: &App) -> usize {
    app.sdk_inventory.rewind_targets.len()
}

fn edit_query(app: &mut App, edit: impl FnOnce(&mut String)) {
    let previous = selected_session(app).map(|session| session.session_id.clone());
    edit(&mut app.session_picker.query);
    app.session_picker.selected = 0;
    app.session_picker.scroll_offset = 0;
    reconcile_selection(app, previous.as_deref());
}

pub(crate) fn paste_search(app: &mut App, text: &str) -> bool {
    if startup_picker_is_loading(app) || app.session_picker.turn_session_id.is_some() {
        return false;
    }
    edit_query(app, |query| query.extend(text.chars().filter(|ch| !ch.is_control())));
    true
}

pub fn handle_key(app: &mut App, key: KeyEvent) {
    if super::keys::is_ctrl_char_shortcut(key, 'q') {
        app.request_shutdown();
        return;
    }

    if startup_picker_is_loading(app) {
        return;
    }

    if app.session_picker.turn_session_id.is_some() {
        handle_turn_key(app, key);
        return;
    }

    if matches!(key.code, KeyCode::Char(ch) if !ch.is_control())
        && super::keys::is_printable_text_modifiers(key.modifiers)
        || matches!((key.code, key.modifiers), (KeyCode::Backspace, KeyModifiers::NONE))
    {
        edit_query(app, |query| match key.code {
            KeyCode::Char(ch) => query.push(ch),
            KeyCode::Backspace => {
                query.pop();
            }
            _ => {}
        });
        return;
    }

    let session_count = picker_session_count(app);
    match (key.code, key.modifiers) {
        (KeyCode::Up, KeyModifiers::NONE) => {
            app.session_picker.selected = app.session_picker.selected.saturating_sub(1);
        }
        (KeyCode::Down, KeyModifiers::NONE) => {
            app.session_picker.selected =
                (app.session_picker.selected + 1).min(session_count.saturating_sub(1));
        }
        (KeyCode::Home, _) => app.session_picker.selected = 0,
        (KeyCode::End, _) => app.session_picker.selected = session_count.saturating_sub(1),
        (KeyCode::Enter, KeyModifiers::NONE) => activate_selection(app),
        (KeyCode::Right, KeyModifiers::NONE) => open_turn_selection(app),
        (KeyCode::Esc, KeyModifiers::NONE) => {
            app.startup.resolve_session_picker();
            app.startup.complete_launch();
            view::set_chat_surface(app);
        }
        _ => {}
    }
}

fn handle_turn_key(app: &mut App, key: KeyEvent) {
    if app.session_picker.turn_reading {
        match (key.code, key.modifiers) {
            (KeyCode::Esc | KeyCode::Left, KeyModifiers::NONE) => {
                app.session_picker.turn_reading = false;
            }
            (KeyCode::Up, KeyModifiers::NONE) => {
                app.session_picker.turn_read_scroll =
                    app.session_picker.turn_read_scroll.saturating_sub(1);
            }
            (KeyCode::Down, KeyModifiers::NONE) => {
                app.session_picker.turn_read_scroll =
                    app.session_picker.turn_read_scroll.saturating_add(1);
            }
            (KeyCode::Enter, KeyModifiers::NONE) => activate_turn_selection(app),
            _ => {}
        }
        return;
    }
    if matches!((key.code, key.modifiers), (KeyCode::Esc | KeyCode::Left, KeyModifiers::NONE)) {
        close_turn_selection(app);
        return;
    }
    if app.sdk_inventory.rewind_targets_in_flight {
        return;
    }
    let turn_count = picker_turn_count(app);
    if turn_count == 0 {
        return;
    }
    match (key.code, key.modifiers) {
        (KeyCode::Up | KeyCode::Char('k'), KeyModifiers::NONE) => {
            app.session_picker.turn_selected = app.session_picker.turn_selected.saturating_sub(1);
        }
        (KeyCode::Down | KeyCode::Char('j'), KeyModifiers::NONE) => {
            app.session_picker.turn_selected =
                (app.session_picker.turn_selected + 1).min(turn_count.saturating_sub(1));
        }
        (KeyCode::Home, _) => app.session_picker.turn_selected = 0,
        (KeyCode::End, _) => app.session_picker.turn_selected = turn_count.saturating_sub(1),
        (KeyCode::Enter, KeyModifiers::NONE) => activate_turn_selection(app),
        (KeyCode::Right, KeyModifiers::NONE) => {
            app.session_picker.turn_reading = true;
            app.session_picker.turn_read_scroll = 0;
        }
        _ => {}
    }
}

fn open_turn_selection(app: &mut App) {
    let Some(session_id) = selected_session(app).map(|session| session.session_id.clone()) else {
        return;
    };
    let Some(conn) = app.session_runtime.conn.clone() else {
        return;
    };

    app.session_picker.turn_session_id = Some(session_id.clone());
    app.session_picker.reset_turn_navigation();
    app.sdk_inventory.rewind_targets.clear();
    app.sdk_inventory.rewind_targets_session_id = None;
    app.sdk_inventory.rewind_targets_error = None;
    app.sdk_inventory.rewind_targets_request_session_id =
        Some(crate::agent::model::SessionId::new(session_id.clone()));
    app.sdk_inventory.rewind_targets_in_flight = true;
    if let Err(error) = conn.get_rewind_targets(session_id) {
        close_turn_selection(app);
        push_system_message_with_severity(
            app,
            Some(SystemSeverity::Error),
            &format!("Failed to load session turns: {error}"),
        );
    }
}

fn close_turn_selection(app: &mut App) {
    app.session_picker.turn_session_id = None;
    app.session_picker.reset_turn_navigation();
    app.sdk_inventory.clear_rewind_targets();
}

fn activate_turn_selection(app: &mut App) {
    let Some(session_id) = app.session_picker.turn_session_id.clone() else {
        return;
    };
    let Some(target) =
        app.sdk_inventory.rewind_targets.get(app.session_picker.turn_selected).cloned()
    else {
        return;
    };
    let Some(conn) = app.session_runtime.conn.clone() else {
        return;
    };

    app.startup.resolve_session_picker();
    app.status = AppStatus::CommandPending;
    app.turn.pending_command_label = Some("Forking before selected message...".to_owned());
    app.turn.pending_command_ack = None;
    if let Err(error) = begin_resume_session_at(app, &conn, session_id, target.uuid) {
        app.turn.pending_command_label = None;
        app.turn.pending_command_ack = None;
        app.status = AppStatus::Ready;
        app.clear_pending_session_resume();
        push_system_message_with_severity(
            app,
            Some(SystemSeverity::Error),
            &format!("Failed to fork before selected message: {error}"),
        );
    }
    view::set_chat_surface(app);
}

fn activate_selection(app: &mut App) {
    let Some(session) = selected_session(app) else {
        return;
    };
    let session_id = session.session_id.clone();
    let Some(conn) = app.session_runtime.conn.clone() else {
        app.startup.resolve_session_picker();
        view::set_chat_surface(app);
        return;
    };

    app.startup.resolve_session_picker();
    app.status = AppStatus::CommandPending;
    app.turn.pending_command_label = Some(format!("Resuming session {session_id}..."));
    app.turn.pending_command_ack = None;
    if let Err(e) = begin_resume_session(app, &conn, session_id) {
        app.turn.pending_command_label = None;
        app.turn.pending_command_ack = None;
        app.status = AppStatus::Ready;
        app.clear_pending_session_resume();
        push_system_message_with_severity(
            app,
            Some(SystemSeverity::Error),
            &format!("Failed to resume session: {e}"),
        );
    }

    view::set_chat_surface(app);
}

#[cfg(test)]
mod tests {
    use super::handle_key;
    use crate::agent::client::AgentConnection;
    use crate::agent::wire::BridgeCommand;
    use crate::app::{App, AppStatus, FullscreenView, RecentSessionInfo, SurfaceMode};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::rc::Rc;

    fn picker_app() -> App {
        let mut app = App::test_default();
        app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::SessionPicker);
        app.startup =
            crate::app::state::StartupState::new(None, crate::StartupLaunch::SessionPicker);
        app.startup.request_connection();
        assert!(app.startup.mark_connection_started());
        app.startup.mark_recent_sessions_loaded();
        app.startup.resolve_session_picker();
        app.recent_sessions = vec![
            RecentSessionInfo {
                session_id: "session-1".to_owned(),
                summary: "one".to_owned(),
                last_modified_ms: 1,
                file_size_bytes: 1,
                cwd: Some("/test".to_owned()),
                git_branch: Some("main".to_owned()),
                custom_title: Some("First".to_owned()),
                first_prompt: Some("prompt one".to_owned()),
            },
            RecentSessionInfo {
                session_id: "session-2".to_owned(),
                summary: "two".to_owned(),
                last_modified_ms: 2,
                file_size_bytes: 2,
                cwd: Some("/test".to_owned()),
                git_branch: Some("main".to_owned()),
                custom_title: Some("Second".to_owned()),
                first_prompt: Some("prompt two".to_owned()),
            },
        ];
        app
    }

    #[test]
    fn loading_state_ignores_navigation_keys() {
        let mut app = picker_app();
        app.startup =
            crate::app::state::StartupState::new(None, crate::StartupLaunch::SessionPicker);
        app.startup.request_connection();
        assert!(app.startup.mark_connection_started());
        app.session_runtime.conn = None;

        handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));

        assert_eq!(app.session_picker.selected, 0);
        assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::SessionPicker));
    }

    #[test]
    fn up_and_down_move_selection() {
        let mut app = picker_app();

        handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.session_picker.selected, 1);

        handle_key(&mut app, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.session_picker.selected, 0);
    }

    #[test]
    fn enter_triggers_resume() {
        let mut app = picker_app();
        let (connection, mut rx) = AgentConnection::test_channel();
        app.session_runtime.conn = Some(Rc::new(connection));

        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(app.surface_mode, SurfaceMode::Chat);
        assert!(matches!(app.status, AppStatus::CommandPending));
        assert_eq!(app.pending_session_resume_id(), Some("session-1"));
        let envelope = rx.try_recv().expect("resume command");
        assert!(matches!(
            envelope.command,
            BridgeCommand::ResumeSession {
                session_id,
                ..
            } if session_id == "session-1"
        ));
    }

    #[test]
    fn space_searches_without_opening_message_navigation() {
        let mut app = picker_app();
        let (connection, mut rx) = AgentConnection::test_channel();
        app.session_runtime.conn = Some(Rc::new(connection));

        handle_key(&mut app, KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));

        assert!(app.session_picker.turn_session_id.is_none());
        assert!(!app.sdk_inventory.rewind_targets_in_flight);
        assert!(rx.try_recv().is_err());
        assert_eq!(app.session_picker.query, " ");
    }

    #[test]
    fn search_matches_visible_title_and_id_without_losing_the_selected_session() {
        let mut app = picker_app();
        app.recent_sessions[0].custom_title = Some("Other".into());
        app.recent_sessions[1].custom_title = Some("Überblick".into());
        app.session_picker.selected = 1;

        for ch in "ÜBER".chars() {
            handle_key(&mut app, KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        assert_eq!(
            super::picker_sessions(&app)
                .map(|session| session.session_id.as_str())
                .collect::<Vec<_>>(),
            vec!["session-2"]
        );
        assert_eq!(app.session_picker.selected, 0);

        for _ in 0..4 {
            handle_key(&mut app, KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        }
        assert_eq!(app.session_picker.selected, 1);
        assert!(super::paste_search(&mut app, "SESSION-2"));
        assert_eq!(
            super::selected_session(&app).map(|session| session.session_id.as_str()),
            Some("session-2")
        );
    }

    #[test]
    fn no_matches_cannot_resume_or_fork_and_backspace_restores_results() {
        let mut app = picker_app();
        let (connection, mut rx) = AgentConnection::test_channel();
        app.session_runtime.conn = Some(Rc::new(connection));

        handle_key(&mut app, KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        for code in [KeyCode::Down, KeyCode::End, KeyCode::Enter, KeyCode::Right] {
            handle_key(&mut app, KeyEvent::new(code, KeyModifiers::NONE));
        }
        assert!(rx.try_recv().is_err());
        assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::SessionPicker));
        assert!(app.session_picker.turn_session_id.is_none());
        handle_key(&mut app, KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(rx.try_recv().expect("resume after clearing search").command,
            BridgeCommand::ResumeSession { session_id, .. } if session_id == "session-1"));
    }

    #[test]
    fn filtered_message_navigation_uses_matching_session_and_restores_search() {
        let mut app = picker_app();
        let (connection, mut rx) = AgentConnection::test_channel();
        app.session_runtime.conn = Some(Rc::new(connection));
        assert!(super::paste_search(&mut app, "Second"));
        handle_key(&mut app, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert!(matches!(rx.try_recv().expect("history request").command,
            BridgeCommand::GetRewindTargets { session_id } if session_id == "session-2"));
        assert!(!super::paste_search(&mut app, "ignored"));
        handle_key(&mut app, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(app.session_picker.query, "Second");
        assert_eq!(
            super::selected_session(&app).map(|session| session.session_id.as_str()),
            Some("session-2")
        );
    }

    #[test]
    fn search_follows_the_same_title_fallbacks_as_the_display() {
        for (custom, prompt, summary, query) in [
            (Some("Named"), Some("Prompt"), "Summary", "named"),
            (Some(" "), Some("Prompt"), "Summary", "prompt"),
            (None, Some(" "), "Summary", "summary"),
            (None, None, " ", "session-1"),
        ] {
            let mut app = picker_app();
            app.recent_sessions.truncate(1);
            app.recent_sessions[0].custom_title = custom.map(str::to_owned);
            app.recent_sessions[0].first_prompt = prompt.map(str::to_owned);
            app.recent_sessions[0].summary = summary.into();
            assert!(super::paste_search(&mut app, query));
            assert_eq!(
                super::selected_session(&app).map(|session| session.session_id.as_str()),
                Some("session-1")
            );
        }
    }

    #[test]
    fn german_keyboard_text_is_searchable_without_triggering_shortcuts() {
        let mut app = picker_app();
        for (ch, modifiers) in [
            ('j', KeyModifiers::NONE),
            ('k', KeyModifiers::NONE),
            ('@', KeyModifiers::CONTROL | KeyModifiers::ALT),
        ] {
            handle_key(&mut app, KeyEvent::new(KeyCode::Char(ch), modifiers));
        }
        assert_eq!(app.session_picker.query, "jk@");
        assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::SessionPicker));
    }

    #[test]
    fn right_opens_turn_picker_and_left_returns_to_sessions() {
        let mut app = picker_app();
        let (connection, _rx) = AgentConnection::test_channel();
        app.session_runtime.conn = Some(Rc::new(connection));

        handle_key(&mut app, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(app.session_picker.turn_session_id.as_deref(), Some("session-1"));

        handle_key(&mut app, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert!(app.session_picker.turn_session_id.is_none());
        assert!(!app.sdk_inventory.rewind_targets_in_flight);
        assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::SessionPicker));
    }

    #[test]
    fn enter_on_message_or_reader_forks_before_the_selected_message() {
        for reading in [false, true] {
            let mut app = picker_app();
            let (connection, mut rx) = AgentConnection::test_channel();
            app.session_runtime.conn = Some(Rc::new(connection));
            app.session_picker.turn_session_id = Some("session-1".to_owned());
            app.sdk_inventory.rewind_targets = vec![crate::agent::model::RewindTarget {
                uuid: "user-2".to_owned(),
                first_text: "second prompt".to_owned(),
                input_text: "second prompt".to_owned(),
                index: 3,
                previous_assistant_uuid: Some("assistant-1".to_owned()),
                resume_anchor_uuid: Some("assistant-1".to_owned()),
            }];

            app.sdk_inventory.rewind_targets.insert(
                0,
                crate::agent::model::RewindTarget {
                    uuid: "other-message".into(),
                    first_text: "Other prompt".into(),
                    input_text: "Other prompt".into(),
                    index: 5,
                    previous_assistant_uuid: None,
                    resume_anchor_uuid: None,
                },
            );
            handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
            app.session_picker.turn_reading = reading;
            handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

            assert_eq!(app.surface_mode, SurfaceMode::Chat);
            assert_eq!(app.pending_session_resume_id(), Some("session-1"));
            let operation_id =
                app.pending_resume_at_operation_id().expect("pending resume-at operation");
            let envelope = rx.try_recv().expect("resume-at command");
            assert_eq!(envelope.request_id.as_deref(), Some(operation_id));
            assert!(matches!(
                envelope.command,
                BridgeCommand::ResumeSessionAt {
                    session_id,
                    target_user_message_id,
                    ..
                } if session_id == "session-1" && target_user_message_id == "user-2"
            ));
        }
    }

    #[test]
    fn esc_switches_back_to_chat() {
        let mut app = picker_app();

        handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        assert_eq!(app.surface_mode, SurfaceMode::Chat);
        assert!(app.startup.session_picker_resolved());
    }

    #[test]
    fn failed_resume_restores_ready_state_and_surfaces_error() {
        let mut app = picker_app();
        let (connection, rx) = AgentConnection::test_channel();
        drop(rx);
        app.session_runtime.conn = Some(Rc::new(connection));

        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(app.surface_mode, SurfaceMode::Chat);
        assert!(matches!(app.status, AppStatus::Ready));
        assert!(app.pending_session_resume.is_none());
        assert!(app.turn.pending_command_label.is_none());
        let last = app.transcript.messages.last().expect("error message");
        let text = match last.blocks.first().expect("text block") {
            crate::app::MessageBlock::Text(block) => block.text.as_str(),
            _ => panic!("expected text block"),
        };
        assert!(text.contains("Failed to resume session:"));
    }
}
