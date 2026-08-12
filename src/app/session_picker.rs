// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use super::connect::{begin_resume_session, begin_resume_session_at};
use super::events::push_system_message_with_severity;
use super::view;
use super::{App, AppStatus, SystemSeverity};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub(crate) const MAX_PICKER_SESSIONS: usize = 10;

pub(crate) fn picker_session_count(app: &App) -> usize {
    app.recent_sessions.len().min(MAX_PICKER_SESSIONS)
}

pub(crate) fn startup_picker_is_loading(app: &App) -> bool {
    app.startup.startup_picker_is_loading(app.session_runtime.conn.is_some())
}

pub(crate) fn picker_turn_count(app: &App) -> usize {
    app.sdk_inventory.rewind_targets.len()
}

pub fn handle_key(app: &mut App, key: KeyEvent) {
    if is_ctrl(key, 'q') || is_ctrl(key, 'c') {
        app.should_quit = true;
        return;
    }

    if startup_picker_is_loading(app) {
        return;
    }

    if app.session_picker.turn_session_id.is_some() {
        handle_turn_key(app, key);
        return;
    }

    let session_count = picker_session_count(app);
    if session_count == 0 {
        if matches!(key.code, KeyCode::Esc | KeyCode::Enter) {
            app.startup.resolve_session_picker();
            view::set_chat_surface(app);
        }
        return;
    }

    match (key.code, key.modifiers) {
        (KeyCode::Up | KeyCode::Char('k'), KeyModifiers::NONE) => {
            app.session_picker.selected = app.session_picker.selected.saturating_sub(1);
        }
        (KeyCode::Down | KeyCode::Char('j'), KeyModifiers::NONE) => {
            app.session_picker.selected =
                (app.session_picker.selected + 1).min(session_count.saturating_sub(1));
        }
        (KeyCode::Home, _) => app.session_picker.selected = 0,
        (KeyCode::End, _) => app.session_picker.selected = session_count.saturating_sub(1),
        (KeyCode::Enter, KeyModifiers::NONE) => activate_selection(app),
        (KeyCode::Right, KeyModifiers::NONE) => open_turn_selection(app),
        (KeyCode::Esc, KeyModifiers::NONE) => {
            app.startup.resolve_session_picker();
            view::set_chat_surface(app);
        }
        _ => {}
    }
}

fn handle_turn_key(app: &mut App, key: KeyEvent) {
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
        _ => {}
    }
}

fn open_turn_selection(app: &mut App) {
    let Some(session_id) = app
        .recent_sessions
        .iter()
        .take(MAX_PICKER_SESSIONS)
        .nth(app.session_picker.selected)
        .map(|session| session.session_id.clone())
    else {
        return;
    };
    let Some(conn) = app.session_runtime.conn.clone() else {
        return;
    };

    app.session_picker.turn_session_id = Some(session_id.clone());
    app.session_picker.turn_selected = 0;
    app.session_picker.turn_scroll_offset = 0;
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
    app.session_picker.turn_selected = 0;
    app.session_picker.turn_scroll_offset = 0;
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
    let Some(session) =
        app.recent_sessions.iter().take(MAX_PICKER_SESSIONS).nth(app.session_picker.selected)
    else {
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

fn is_ctrl(key: KeyEvent, ch: char) -> bool {
    matches!(key.code, KeyCode::Char(c) if c == ch) && key.modifiers == KeyModifiers::CONTROL
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
        app.startup = crate::app::state::StartupState::new(None, None, true);
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
        app.startup = crate::app::state::StartupState::new(None, None, true);
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
    fn space_is_ignored_to_avoid_accidental_destructive_navigation() {
        let mut app = picker_app();
        let (connection, mut rx) = AgentConnection::test_channel();
        app.session_runtime.conn = Some(Rc::new(connection));

        handle_key(&mut app, KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));

        assert!(app.session_picker.turn_session_id.is_none());
        assert!(!app.sdk_inventory.rewind_targets_in_flight);
        assert!(rx.try_recv().is_err());
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
    fn enter_on_turn_sends_resume_at_without_changing_plain_resume_command() {
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
