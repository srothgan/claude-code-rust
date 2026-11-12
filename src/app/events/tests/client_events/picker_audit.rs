// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::agent::{client::CommandReceiver, wire::BridgeCommand};
use crate::app::test_support::transcript_region;
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;

fn key(app: &mut App, code: KeyCode) {
    handle_terminal_event(app, Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
}

fn no_command(commands: &mut CommandReceiver) {
    assert!(matches!(commands.try_recv(), Err(tokio::sync::mpsc::error::TryRecvError::Empty)));
}

fn listed(app: &mut App, entries: &[(&str, &str)]) {
    handle_client_event(
        app,
        ClientEvent::SessionsListed {
            sessions: entries.iter().map(|(id, title)| listed_session(id, title)).collect(),
        },
    );
}

fn selected(app: &App) -> Option<&str> {
    crate::app::session_picker::selected_session(app).map(|session| session.session_id.as_str())
}

fn screen(app: &mut App) -> Vec<String> {
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(140, 36)).unwrap();
    terminal.draw(|frame| crate::ui::render_fullscreen_surface(frame, app)).unwrap();
    (1..35)
        .map(|y| (2..138).map(|x| terminal.backend().buffer()[(x, y)].symbol()).collect())
        .collect()
}

fn assert_resume(commands: &mut CommandReceiver, expected: &str) {
    assert!(
        matches!(commands.try_recv().expect("selected resume command").command, BridgeCommand::ResumeSession { session_id, .. } if session_id == expected)
    );
    no_command(commands);
}

#[test]
fn title_results_associate_requested_and_unrelated_sessions_and_never_report_missing_targets_as_success()
 {
    use crate::app::config::{PendingSessionTitleChangeKind, PendingSessionTitleChangeState};
    for generate in [false, true] {
        for target_present in [false, true] {
            let mut app = make_test_app();
            app.config.pending_session_title_change = Some(PendingSessionTitleChangeState {
                session_id: "requested".into(),
                kind: if generate {
                    PendingSessionTitleChangeKind::Generate
                } else {
                    PendingSessionTitleChangeKind::Rename {
                        requested_title: Some("Requested title".into()),
                    }
                },
            });
            app.config.last_error = Some("previous failure".into());
            let mut entries = vec![("unrelated", "Independent title")];
            if target_present {
                entries.push((
                    "requested",
                    if generate { "Generated target title" } else { "Requested title" },
                ));
            }
            listed(&mut app, &entries);
            let actual = app
                .recent_sessions
                .iter()
                .map(|session| (session.session_id.as_str(), session.custom_title.as_deref()))
                .collect::<BTreeMap<_, _>>();
            assert_eq!(actual.get("unrelated"), Some(&Some("Independent title")));
            if target_present {
                assert_eq!(
                    actual.get("requested"),
                    Some(&Some(if generate {
                        "Generated target title"
                    } else {
                        "Requested title"
                    }))
                );
                assert!(app.config.pending_session_title_change.is_none());
                assert!(app.config.last_error.is_none());
                let feedback =
                    app.config.status_message.as_deref().expect("owning surface success");
                if generate {
                    assert!(feedback.to_lowercase().contains("generat"));
                } else {
                    assert!(feedback.contains("Requested title"));
                }
            } else {
                assert!(
                    app.config.status_message.is_none(),
                    "absent target must not report success"
                );
            }
            assert!(app.transcript.messages.is_empty(), "title completion stays out of chat");
        }
    }
}

#[test]
fn startup_picker_waits_for_both_events_and_handles_nonempty_and_empty_lists_in_either_order() {
    for list_first in [false, true] {
        for has_sessions in [false, true] {
            let home = tempfile::tempdir().unwrap();
            let (mut app, mut commands) = super::connection_audit::fixture(home.path());
            app.session_runtime.session_id = None;
            app.startup =
                crate::app::state::StartupState::new(None, crate::StartupLaunch::SessionPicker);
            app.startup.request_connection();
            assert!(app.startup.mark_connection_started());
            let entries =
                if has_sessions { vec![("resume-target", "Visible target title")] } else { vec![] };
            if list_first {
                let connection = app.session_runtime.conn.take();
                listed(&mut app, &entries);
                assert_eq!(app.surface_mode, SurfaceMode::Chat);
                app.session_runtime.conn = connection;
                handle_client_event(
                    &mut app,
                    crate::app::test_support::session_connection("connected-session", home.path()),
                );
            } else {
                handle_client_event(
                    &mut app,
                    crate::app::test_support::session_connection("connected-session", home.path()),
                );
                assert_eq!(app.surface_mode, SurfaceMode::Chat);
                listed(&mut app, &entries);
            }
            crate::app::test_support::drain_session_refreshes(&mut commands, "connected-session");
            if has_sessions {
                assert_eq!(
                    app.surface_mode,
                    SurfaceMode::Fullscreen(FullscreenView::SessionPicker)
                );
                assert_eq!(selected(&app), Some("resume-target"));
                let rows = screen(&mut app);
                let title = rows
                    .iter()
                    .position(|row| row.contains("Visible target title"))
                    .expect("visible session title");
                assert!(
                    rows[title + 1].contains("resume-target"),
                    "title must label its own ID: {rows:?}"
                );
                key(&mut app, KeyCode::Enter);
                assert_resume(&mut commands, "resume-target");
            } else {
                assert_eq!(app.surface_mode, SurfaceMode::Chat);
                let info = app.transcript.messages.iter().find(|message| message.role == MessageRole::System(Some(SystemSeverity::Info)) && message.blocks.iter().any(|block| matches!(block, MessageBlock::Text(text) if text.text.to_lowercase().contains("no recent sessions")))).expect("empty-list explanation");
                let index = app
                    .transcript
                    .messages
                    .iter()
                    .position(|message| message.id == info.id)
                    .unwrap();
                assert!(
                    transcript_region(&mut app, index)
                        .to_lowercase()
                        .contains("no recent sessions")
                );
                no_command(&mut commands);
            }
        }
    }
}

fn open_beta(app: &mut App, filtered: bool) {
    listed(
        app,
        &[
            ("a", "Feature alpha"),
            ("b", "Feature beta"),
            ("c", "Feature gamma"),
            ("hidden", "Other"),
        ],
    );
    crate::app::session_picker::open(app);
    if filtered {
        handle_terminal_event(app, Event::Paste("feature".into()));
    }
    key(app, KeyCode::Down);
    assert_eq!(selected(app), Some("b"));
}

#[test]
fn list_refresh_preserves_selected_identity_and_reuses_the_picker_after_removal_and_empty_results()
{
    for filtered in [false, true] {
        for activate_preserved in [false, true] {
            let (mut app, mut commands) = app_with_bridge_connection();
            open_beta(&mut app, filtered);
            listed(
                &mut app,
                &[
                    ("c", "Feature gamma"),
                    ("hidden", "Other"),
                    ("a", "Feature alpha"),
                    ("b", "Feature beta renamed"),
                ],
            );
            assert_eq!(selected(&app), Some("b"));
            assert_eq!(app.session_picker.query, if filtered { "feature" } else { "" });
            if activate_preserved {
                key(&mut app, KeyCode::Enter);
                assert_resume(&mut commands, "b");
                continue;
            }
            listed(
                &mut app,
                &[("hidden", "Other"), ("a", "Feature alpha"), ("c", "Feature gamma")],
            );
            assert_eq!(selected(&app), Some("c"));
            if filtered {
                listed(&mut app, &[("hidden", "Other")]);
            } else {
                listed(&mut app, &[]);
            }
            assert!(selected(&app).is_none());
            key(&mut app, KeyCode::Enter);
            assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::SessionPicker));
            no_command(&mut commands);
            let rows = screen(&mut app);
            assert!(
                rows.iter().any(|row| row.to_lowercase().contains("no")
                    && row.to_lowercase().contains("sessions")),
                "empty picker feedback: {rows:?}"
            );
            listed(&mut app, &[("new", "Feature new")]);
            assert_eq!(selected(&app), Some("new"));
            key(&mut app, KeyCode::Enter);
            assert_resume(&mut commands, "new");
        }
    }
}

fn target() -> model::RewindTarget {
    model::RewindTarget {
        uuid: "loaded-target".into(),
        first_text: "Loaded source prompt".into(),
        input_text: "Loaded source prompt".into(),
        index: 0,
        previous_assistant_uuid: None,
        resume_anchor_uuid: None,
    }
}

#[test]
fn removing_an_inspected_session_discards_loaded_targets_and_rejects_its_late_response() {
    let (mut app, mut commands) = app_with_bridge_connection();
    listed(&mut app, &[("inspected", "Inspected session")]);
    crate::app::session_picker::open(&mut app);
    key(&mut app, KeyCode::Right);
    assert_eq!(
        commands.try_recv().unwrap().command,
        BridgeCommand::GetRewindTargets { session_id: "inspected".into() }
    );
    handle_client_event(
        &mut app,
        ClientEvent::RewindTargetsReceived {
            session_id: "inspected".into(),
            targets: vec![target()],
            error: None,
        },
    );
    assert!(
        screen(&mut app).iter().any(|row| row.contains("Loaded source prompt")),
        "seed a genuinely loaded target"
    );
    listed(&mut app, &[]);
    assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::SessionPicker));
    assert!(app.session_picker.turn_session_id.is_none());
    key(&mut app, KeyCode::Enter);
    no_command(&mut commands);
    handle_client_event(
        &mut app,
        ClientEvent::RewindTargetsReceived {
            session_id: "inspected".into(),
            targets: vec![target()],
            error: None,
        },
    );
    key(&mut app, KeyCode::Enter);
    no_command(&mut commands);
    assert!(!screen(&mut app).iter().any(|row| row.contains("Loaded source prompt")));
    listed(&mut app, &[("fresh", "Fresh session")]);
    key(&mut app, KeyCode::Enter);
    assert_resume(&mut commands, "fresh");
}

#[test]
fn live_title_changes_retain_the_selected_session_when_an_earlier_filtered_result_disappears() {
    let (mut app, mut commands) = app_with_bridge_connection();
    app.session_runtime.session_id = Some("active".into());
    listed(&mut app, &[("active", "Feature active"), ("chosen", "Feature chosen")]);
    crate::app::session_picker::open(&mut app);
    handle_terminal_event(&mut app, Event::Paste("feature".into()));
    key(&mut app, KeyCode::Down);
    assert_eq!(selected(&app), Some("chosen"));
    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate {
            session_id: "active".into(),
            update: model::SessionUpdate::SessionTitleUpdate(Some("Unrelated title".into())),
        },
    );
    assert_eq!(selected(&app), Some("chosen"));
    assert_eq!(app.session_picker.query, "feature");
    key(&mut app, KeyCode::Enter);
    assert_resume(&mut commands, "chosen");
}

fn history() -> Vec<model::SessionUpdate> {
    vec![
        model::SessionUpdate::UserMessageChunk(model::ContentChunk::new(
            model::ContentBlock::Text(model::TextContent::new("Restored user question")),
        )),
        model::SessionUpdate::AgentMessageChunk(model::ContentChunk::new(
            model::ContentBlock::Text(model::TextContent::new("Restored assistant answer")),
        )),
    ]
}

fn assert_restored_history(app: &mut App) {
    for (role, text) in [
        (MessageRole::User, "Restored user question"),
        (MessageRole::Assistant, "Restored assistant answer"),
    ] {
        let index = app
            .transcript
            .messages
            .iter()
            .position(|message| {
                message.role == role
                    && message.blocks.iter().any(
                        |block| matches!(block,MessageBlock::Text(content) if content.text==text),
                    )
            })
            .expect("independently supplied history and role");
        assert!(transcript_region(app, index).contains(text));
    }
    assert!(
        !app.transcript
            .messages
            .iter()
            .any(|message| matches!(message.role, MessageRole::System(_))),
        "resume must not add a confirmation notice"
    );
    assert_no_session_overview_rows(app);
}

#[test]
fn resumed_history_is_rendered_at_its_roles_without_confirmation_or_a_returning_overview() {
    for startup in [false, true] {
        let home = tempfile::tempdir().unwrap();
        let (mut app, mut commands) = super::connection_audit::fixture(home.path());
        app.set_pending_session_resume("requested".into(), None);
        let event = if startup {
            let mut event = crate::app::test_support::session_connection("restored", home.path());
            let ClientEvent::Connected { history_updates, .. } = &mut event else { unreachable!() };
            *history_updates = history();
            event
        } else {
            crate::app::test_support::session_replacement("restored", home.path(), history(), None)
        };
        handle_client_event(&mut app, event);
        assert_eq!(app.status, AppStatus::Ready);
        assert!(app.pending_session_resume.is_none());
        crate::app::test_support::drain_session_refreshes(&mut commands, "restored");
        assert_restored_history(&mut app);
        handle_client_event(
            &mut app,
            ClientEvent::StatusSnapshotReceived {
                session_id: "restored".into(),
                account: model::AccountInfo {
                    email: None,
                    organization: None,
                    subscription_type: Some("Fixture Max".into()),
                    token_source: None,
                    api_key_source: None,
                    api_provider: None,
                },
            },
        );
        assert_restored_history(&mut app);
    }
}
