// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::connect::begin_resume_session;
use super::events::push_system_message_with_severity;
use super::view::{self, ActiveView};
use super::{App, AppStatus, SystemSeverity};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub(crate) const MAX_PICKER_SESSIONS: usize = 10;

pub(crate) fn picker_session_count(app: &App) -> usize {
    app.recent_sessions.len().min(MAX_PICKER_SESSIONS)
}

pub(crate) fn startup_picker_is_loading(app: &App) -> bool {
    app.startup_session_picker_requested
        && !app.startup_session_picker_resolved
        && (app.conn.is_none() || !app.startup_recent_sessions_loaded)
}

pub fn handle_key(app: &mut App, key: KeyEvent) {
    if is_ctrl(key, 'q') || is_ctrl(key, 'c') {
        app.should_quit = true;
        return;
    }

    if startup_picker_is_loading(app) {
        return;
    }

    let session_count = picker_session_count(app);
    if session_count == 0 {
        if matches!(key.code, KeyCode::Esc | KeyCode::Enter) {
            app.startup_session_picker_resolved = true;
            view::set_active_view(app, ActiveView::Chat);
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
        (KeyCode::Esc, KeyModifiers::NONE) => {
            app.startup_session_picker_resolved = true;
            view::set_active_view(app, ActiveView::Chat);
        }
        _ => {}
    }
}

fn activate_selection(app: &mut App) {
    let Some(session) =
        app.recent_sessions.iter().take(MAX_PICKER_SESSIONS).nth(app.session_picker.selected)
    else {
        return;
    };
    let session_id = session.session_id.clone();
    let Some(conn) = app.conn.clone() else {
        app.startup_session_picker_resolved = true;
        view::set_active_view(app, ActiveView::Chat);
        return;
    };

    app.startup_session_picker_resolved = true;
    app.status = AppStatus::CommandPending;
    app.pending_command_label = Some(format!("Resuming session {session_id}..."));
    app.pending_command_ack = None;
    if let Err(e) = begin_resume_session(app, &conn, session_id) {
        app.pending_command_label = None;
        app.pending_command_ack = None;
        app.status = AppStatus::Ready;
        app.resuming_session_id = None;
        push_system_message_with_severity(
            app,
            Some(SystemSeverity::Error),
            &format!("Failed to resume session: {e}"),
        );
    }

    view::set_active_view(app, ActiveView::Chat);
}

fn is_ctrl(key: KeyEvent, ch: char) -> bool {
    matches!(key.code, KeyCode::Char(c) if c == ch) && key.modifiers == KeyModifiers::CONTROL
}

#[cfg(test)]
mod tests {
    use super::handle_key;
    use crate::agent::client::AgentConnection;
    use crate::agent::wire::{BridgeCommand, CommandEnvelope};
    use crate::app::{ActiveView, App, AppStatus, RecentSessionInfo};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::rc::Rc;

    fn picker_app() -> App {
        let mut app = App::test_default();
        app.active_view = ActiveView::SessionPicker;
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
        app.startup_session_picker_requested = true;
        app.startup_recent_sessions_loaded = false;
        app.conn = None;

        handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));

        assert_eq!(app.session_picker.selected, 0);
        assert_eq!(app.active_view, ActiveView::SessionPicker);
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
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<CommandEnvelope>();
        app.conn = Some(Rc::new(AgentConnection::new(tx)));

        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(app.active_view, ActiveView::Chat);
        assert!(matches!(app.status, AppStatus::CommandPending));
        assert_eq!(app.resuming_session_id.as_deref(), Some("session-1"));
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
    fn esc_switches_back_to_chat() {
        let mut app = picker_app();

        handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        assert_eq!(app.active_view, ActiveView::Chat);
        assert!(app.startup_session_picker_resolved);
    }

    #[test]
    fn failed_resume_restores_ready_state_and_surfaces_error() {
        let mut app = picker_app();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<CommandEnvelope>();
        drop(rx);
        app.conn = Some(Rc::new(AgentConnection::new(tx)));

        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(app.active_view, ActiveView::Chat);
        assert!(matches!(app.status, AppStatus::Ready));
        assert!(app.resuming_session_id.is_none());
        assert!(app.pending_command_label.is_none());
        let last = app.messages.last().expect("error message");
        let text = match last.blocks.first().expect("text block") {
            crate::app::MessageBlock::Text(block) => block.text.as_str(),
            _ => panic!("expected text block"),
        };
        assert!(text.contains("Failed to resume session:"));
    }
}
