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
    fn startup_loading_blocks_actions_until_connection_and_list_are_ready() {
        use crate::app::test_support::{drain_session_refreshes, session_connection};
        for connected in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let mut app = picker_app();
            app.startup = startup_choice();
            let (connection, mut rx) = AgentConnection::test_channel();
            let connection = Rc::new(connection);
            app.session_runtime.conn = connected.then(|| connection.clone());
            app.input.set_text("Unsent draft");
            app.session_picker.query = "First".into();
            let draft = app.input.snapshot();
            for code in [
                KeyCode::Down,
                KeyCode::End,
                KeyCode::Enter,
                KeyCode::Right,
                KeyCode::Esc,
                KeyCode::Char('x'),
            ] {
                key(&mut app, code);
            }
            assert!(!super::paste_search(&mut app, "ignored paste"));
            assert_eq!(app.session_picker.query, "First");
            assert_eq!(app.input.snapshot(), draft);
            assert!(app.session_picker.turn_session_id.is_none());
            assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::SessionPicker));
            assert!(rx.try_recv().is_err());
            app.session_runtime.conn = Some(connection);
            crate::app::events::handle_client_event(
                &mut app,
                session_connection("session-1", dir.path()),
            );
            drain_session_refreshes(&mut rx, "session-1");
            deliver_list(&mut app, &[("session-1", "First"), ("session-2", "Second")]);
            key(&mut app, KeyCode::End);
            let target = super::selected_session(&app).unwrap().session_id.clone();
            assert_eq!(target, "session-2");
            key(&mut app, KeyCode::Enter);
            expect_resume(&mut rx, "session-2");
        }
        let mut app = picker_app();
        app.startup = startup_choice();
        handle_key(&mut app, KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL));
        assert!(app.shutdown_requested());
    }

    #[test]
    fn navigation_resumes_only_the_selected_session_on_confirmation() {
        let mut app = picker_app();
        let mut rx = attach(&mut app);
        let mut third = app.recent_sessions[0].clone();
        third.session_id = "session-3".into();
        third.custom_title = Some("Third".into());
        app.recent_sessions.push(third);
        for (code, id) in [
            (KeyCode::Up, "session-1"),
            (KeyCode::Down, "session-2"),
            (KeyCode::End, "session-3"),
            (KeyCode::Down, "session-3"),
            (KeyCode::Home, "session-1"),
            (KeyCode::Up, "session-1"),
            (KeyCode::Down, "session-2"),
        ] {
            key(&mut app, code);
            assert_eq!(super::selected_session(&app).unwrap().session_id, id);
            assert!(rx.try_recv().is_err());
        }
        key(&mut app, KeyCode::Enter);
        expect_resume(&mut rx, "session-2");
        assert_eq!(app.pending_session_resume_id(), Some("session-2"));
        assert_eq!(app.surface_mode, SurfaceMode::Chat);
        assert!(matches!(app.status, AppStatus::CommandPending));
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
        let mut rx = attach(&mut app);
        app.recent_sessions[0].custom_title = Some("Other".into());
        app.recent_sessions[1].custom_title = Some("\u{dc}berblick".into());
        key(&mut app, KeyCode::Down);
        for ch in "\u{dc}BER".chars() {
            key(&mut app, KeyCode::Char(ch));
            assert_eq!(super::selected_session(&app).unwrap().session_id, "session-2");
        }
        assert_eq!(
            super::picker_sessions(&app).map(|s| s.session_id.as_str()).collect::<Vec<_>>(),
            ["session-2"]
        );
        for _ in 0..4 {
            key(&mut app, KeyCode::Backspace);
            assert_eq!(super::selected_session(&app).unwrap().session_id, "session-2");
        }
        assert!(super::paste_search(&mut app, "SESSION-2"));
        assert_eq!(super::selected_session(&app).unwrap().session_id, "session-2");
        assert!(rx.try_recv().is_err());
        key(&mut app, KeyCode::Enter);
        expect_resume(&mut rx, "session-2");
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
    fn filtered_message_navigation_abandons_old_results_and_allows_reentry() {
        for back in [KeyCode::Left, KeyCode::Esc] {
            let mut app = picker_app();
            let mut rx = attach(&mut app);
            assert!(super::paste_search(&mut app, "Second"));
            request_history(&mut app, &mut rx, "session-2");
            assert!(!super::paste_search(&mut app, "ignored"));
            key(&mut app, back);
            deliver_targets(&mut app, "session-2", "STALE target");
            assert!(app.session_picker.turn_session_id.is_none());
            assert_eq!(app.session_picker.query, "Second");
            assert_eq!(super::selected_session(&app).unwrap().session_id, "session-2");
            assert!(!picker_rows(&mut app).iter().any(|r| r.contains("STALE")));
            assert!(rx.try_recv().is_err());
            request_history(&mut app, &mut rx, "session-2");
            deliver_targets(&mut app, "session-2", "Fresh target");
            assert!(picker_rows(&mut app).iter().any(|r| r.contains("Fresh target")));
            key(&mut app, KeyCode::Right);
            assert!(picker_rows(&mut app).iter().any(|r| r.contains("Fresh target")));
            key(&mut app, back);
            assert!(app.session_picker.turn_session_id.is_some());
            key(&mut app, back);
            assert_eq!(super::selected_session(&app).unwrap().session_id, "session-2");
        }
    }

    #[test]
    fn search_obeys_visible_title_precedence_and_independent_id_and_branch_matches() {
        for (custom, prompt, summary, hit, hidden) in [
            (Some("Named"), Some("Prompt"), "Summary", "named", Some("prompt")),
            (Some(" "), Some("Prompt"), "Summary", "prompt", Some("summary")),
            (None, Some(" "), "Summary", "summary", None),
            (None, None, " ", "session-1", None),
        ] {
            let mut app = picker_app();
            app.recent_sessions[0].custom_title = custom.map(str::to_owned);
            app.recent_sessions[0].first_prompt = prompt.map(str::to_owned);
            app.recent_sessions[0].summary = summary.into();
            app.recent_sessions[0].git_branch = Some("unique-branch".into());
            for query in
                [Some(hit), Some("SESSION-1"), Some("UNIQUE-BRANCH"), hidden, Some("no-match")]
                    .into_iter()
                    .flatten()
            {
                super::open(&mut app);
                assert!(super::paste_search(&mut app, query));
                let expected = if Some(query) == hidden || query == "no-match" {
                    vec![]
                } else {
                    vec!["session-1"]
                };
                assert_eq!(
                    super::picker_sessions(&app).map(|s| s.session_id.as_str()).collect::<Vec<_>>(),
                    expected,
                    "query={query}"
                );
            }
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
    fn cancellation_releases_startup_choice_and_preserves_the_active_draft() {
        for startup in [false, true] {
            let mut app = picker_app();
            let mut rx = attach(&mut app);
            seed_chat(&mut app, &mut rx);
            if startup {
                app.startup = startup_choice();
                app.startup.mark_recent_sessions_loaded();
                assert!(!app.startup.launch_completed());
                assert!(!app.startup.session_picker_resolved());
            }
            super::open(&mut app);
            let draft = app.input.snapshot();
            key(&mut app, KeyCode::Esc);
            assert_eq!(app.surface_mode, SurfaceMode::Chat);
            assert_eq!(app.session_runtime.session_id.as_ref().unwrap().as_str(), "session-1");
            assert_eq!(app.input.snapshot(), draft);
            assert!(app.startup.launch_completed());
            assert!(app.startup.session_picker_resolved());
            assert!(rx.try_recv().is_err());
        }
    }

    #[test]
    fn failed_resume_preserves_chat_and_allows_a_new_admission() {
        let mut app = picker_app();
        let mut rx = attach(&mut app);
        seed_chat(&mut app, &mut rx);
        let draft = app.input.snapshot();
        drop(rx);
        super::open(&mut app);
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Enter);
        assert_admission_failure(&app, &draft, "resume");
        let mut rx = attach(&mut app);
        super::open(&mut app);
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Enter);
        expect_resume(&mut rx, "session-2");
    }
    fn key(app: &mut App, code: KeyCode) {
        handle_key(app, KeyEvent::new(code, KeyModifiers::NONE));
    }
    fn attach(app: &mut App) -> crate::agent::client::CommandReceiver {
        let (conn, rx) = AgentConnection::test_channel();
        app.session_runtime.conn = Some(Rc::new(conn));
        rx
    }
    fn expect_resume(rx: &mut crate::agent::client::CommandReceiver, id: &str) {
        assert!(
            matches!(rx.try_recv().expect("selected resume").command,BridgeCommand::ResumeSession {session_id,..} if session_id==id)
        );
        assert!(rx.try_recv().is_err());
    }
    fn startup_choice() -> crate::app::state::StartupState {
        use clap::Parser;
        let cli = crate::Cli::try_parse_from(["claude-rs", "--resume", "--model", "fixture-model"])
            .unwrap();
        let mut state = crate::app::state::StartupState::from_cli(&cli);
        state.request_connection();
        assert!(state.mark_connection_started());
        state
    }
    fn request_history(app: &mut App, rx: &mut crate::agent::client::CommandReceiver, id: &str) {
        key(app, KeyCode::Right);
        assert!(
            matches!(rx.try_recv().expect("history request").command,BridgeCommand::GetRewindTargets {session_id} if session_id==id)
        );
        assert!(rx.try_recv().is_err());
    }
    fn deliver_targets(app: &mut App, id: &str, text: &str) {
        crate::app::events::handle_client_event(
            app,
            crate::agent::events::ClientEvent::RewindTargetsReceived {
                session_id: id.into(),
                targets: vec![crate::agent::model::RewindTarget {
                    uuid: "chosen-user".into(),
                    first_text: text.into(),
                    input_text: text.into(),
                    index: 4,
                    previous_assistant_uuid: Some("different-assistant".into()),
                    resume_anchor_uuid: Some("different-anchor".into()),
                }],
                error: None,
            },
        );
    }
    fn deliver_list(app: &mut App, entries: &[(&str, &str)]) {
        crate::app::events::handle_client_event(
            app,
            crate::agent::events::ClientEvent::SessionsListed {
                sessions: entries
                    .iter()
                    .map(|(id, title)| crate::agent::types::SessionListEntry {
                        session_id: (*id).into(),
                        summary: (*title).into(),
                        last_modified_ms: 1,
                        file_size_bytes: 1,
                        cwd: None,
                        git_branch: None,
                        custom_title: Some((*title).into()),
                        first_prompt: None,
                    })
                    .collect(),
            },
        );
    }
    fn picker_rows(app: &mut App) -> Vec<String> {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 25)).unwrap();
        terminal.draw(|f| crate::ui::render_fullscreen_surface(f, app)).unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .chunks(100)
            .map(|r| r.iter().map(ratatui::buffer::Cell::symbol).collect())
            .collect()
    }
    fn seed_chat(app: &mut App, rx: &mut crate::agent::client::CommandReceiver) {
        use crate::app::test_support::{deliver_update, drain_session_refreshes, source_chunk};
        app.session_runtime.session_id = Some("session-1".into());
        deliver_update(app, source_chunk("Saved source", "saved-user", true));
        deliver_update(app, source_chunk("Saved answer", "saved-assistant", false));
        crate::app::events::handle_client_event(
            app,
            crate::agent::events::ClientEvent::TurnComplete {
                session_id: "session-1".into(),
                queued_turn_count: None,
                terminal_reason: None,
            },
        );
        drain_session_refreshes(rx, "session-1");
        app.input.set_text("Unsent source |tail");
        app.input.set_cursor(0, 7);
    }
    fn assert_admission_failure(
        app: &App,
        draft: &crate::app::input::InputSnapshot,
        operation: &str,
    ) {
        assert_eq!(app.surface_mode, SurfaceMode::Chat);
        assert_eq!(app.status, AppStatus::Ready);
        assert_eq!(app.session_runtime.session_id.as_ref().unwrap().as_str(), "session-1");
        assert_eq!(&app.input.snapshot(), draft);
        assert!(app.pending_session_resume.is_none());
        assert!(app.turn.pending_command_label.is_none());
        assert!(app.transcript.messages.iter().any(|m| {
            m.blocks
                .iter()
                .any(|b| matches!(b,crate::app::MessageBlock::Text(t) if t.text=="Saved answer"))
        }));
        assert_error(app, operation);
    }
    fn assert_error(app: &App, operation: &str) {
        let last = app.transcript.messages.last().unwrap();
        assert!(matches!(
            last.role,
            crate::app::MessageRole::System(Some(crate::app::SystemSeverity::Error))
        ));
        let text = last
            .blocks
            .iter()
            .filter_map(|b| {
                if let crate::app::MessageBlock::Text(t) = b { Some(t.text.as_str()) } else { None }
            })
            .collect::<String>();
        assert!(text.to_lowercase().contains(operation) && text.contains("closed"), "{text}");
    }
    fn expect_fork(app: &App, rx: &mut crate::agent::client::CommandReceiver, session: &str) {
        let e = rx.try_recv().expect("new fork command");
        assert!(
            matches!(e.command,BridgeCommand::ResumeSessionAt {session_id,target_user_message_id,..} if session_id==session && target_user_message_id=="chosen-user")
        );
        assert_eq!(e.request_id.as_deref(), app.pending_resume_at_operation_id());
        assert!(e.request_id.is_some());
        assert!(rx.try_recv().is_err());
    }
    #[test]
    fn history_admission_failure_retains_search_and_retries_without_stale_ownership() {
        let mut app = picker_app();
        let rx = attach(&mut app);
        drop(rx);
        super::paste_search(&mut app, "Second");
        key(&mut app, KeyCode::Right);
        assert_eq!(super::selected_session(&app).unwrap().session_id, "session-2");
        assert_eq!(app.session_picker.query, "Second");
        assert!(app.session_picker.turn_session_id.is_none());
        assert!(!app.sdk_inventory.rewind_targets_in_flight);
        assert!(app.sdk_inventory.rewind_targets_request_session_id.is_none());
        assert_error(&app, "load");
        let mut rx = attach(&mut app);
        request_history(&mut app, &mut rx, "session-2");
        deliver_targets(&mut app, "session-2", "Recovered target");
        key(&mut app, KeyCode::Enter);
        expect_fork(&app, &mut rx, "session-2");
    }
    #[test]
    fn fork_admission_failure_preserves_chat_and_retries_from_list_and_reader() {
        for reader in [false, true] {
            let mut app = picker_app();
            let mut rx = attach(&mut app);
            seed_chat(&mut app, &mut rx);
            super::open(&mut app);
            key(&mut app, KeyCode::Down);
            request_history(&mut app, &mut rx, "session-2");
            deliver_targets(&mut app, "session-2", "Chosen source");
            if reader {
                key(&mut app, KeyCode::Right);
            }
            let draft = app.input.snapshot();
            drop(rx);
            key(&mut app, KeyCode::Enter);
            assert_admission_failure(&app, &draft, "fork");
            let mut rx = attach(&mut app);
            super::open(&mut app);
            key(&mut app, KeyCode::Down);
            request_history(&mut app, &mut rx, "session-2");
            deliver_targets(&mut app, "session-2", "Chosen source");
            if reader {
                key(&mut app, KeyCode::Right);
            }
            key(&mut app, KeyCode::Enter);
            expect_fork(&app, &mut rx, "session-2");
        }
    }
    #[test]
    fn removing_the_inspected_session_invalidates_reader_and_late_results() {
        for reading in [false, true] {
            let mut app = picker_app();
            let mut rx = attach(&mut app);
            seed_chat(&mut app, &mut rx);
            deliver_list(
                &mut app,
                &[("session-1", "Active"), ("session-2", "Inspect"), ("session-3", "Remaining")],
            );
            super::open(&mut app);
            super::paste_search(&mut app, "Inspect");
            let draft = app.input.snapshot();
            request_history(&mut app, &mut rx, "session-2");
            if reading {
                deliver_targets(&mut app, "session-2", "Inspected body");
                key(&mut app, KeyCode::Right);
                assert!(picker_rows(&mut app).iter().any(|r| r.contains("Inspected body")));
            }
            deliver_list(&mut app, &[("session-2", "Inspect"), ("session-3", "Remaining")]);
            assert_eq!(app.session_picker.turn_session_id.as_deref(), Some("session-2"));
            if reading {
                assert!(picker_rows(&mut app).iter().any(|r| r.contains("Inspected body")));
            }
            deliver_list(&mut app, &[("session-3", "Remaining")]);
            assert!(app.session_picker.turn_session_id.is_none());
            assert_eq!(app.session_picker.query, "Inspect");
            assert_eq!(app.input.snapshot(), draft);
            assert!(super::selected_session(&app).is_none());
            deliver_targets(&mut app, "session-2", "STALE body");
            crate::app::events::handle_client_event(
                &mut app,
                crate::agent::events::ClientEvent::RewindTargetsReceived {
                    session_id: "session-2".into(),
                    targets: vec![],
                    error: Some("STALE failure".into()),
                },
            );
            assert!(
                !picker_rows(&mut app)
                    .iter()
                    .any(|r| r.contains("Inspected body") || r.contains("STALE"))
            );
            assert!(!(0..app.transcript.messages.len()).any(|i| {
                crate::app::test_support::transcript_region(&mut app, i).contains("STALE")
            }));
            key(&mut app, KeyCode::Enter);
            assert!(rx.try_recv().is_err());
            for _ in "Inspect".chars() {
                key(&mut app, KeyCode::Backspace);
            }
            assert_eq!(super::selected_session(&app).unwrap().session_id, "session-3");
            request_history(&mut app, &mut rx, "session-3");
            deliver_targets(&mut app, "session-3", "Surviving target");
            key(&mut app, KeyCode::Enter);
            expect_fork(&app, &mut rx, "session-3");
        }
    }
}
