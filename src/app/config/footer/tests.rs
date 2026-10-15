// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::agent::client::{AgentConnection, CommandReceiver};
use crate::agent::events::ClientEvent;
use crate::agent::settings::{
    SettingsApplication, SettingsPersistence, SettingsResult, SettingsScope, SettingsSnapshot,
};
use crate::agent::wire::BridgeCommand;
use crate::app::footer::{FooterFieldId, FooterSide, SETTING_ID};
use ratatui::{Terminal, backend::TestBackend};
use serde_json::{Value, json};
use std::rc::Rc;

fn snapshot(cwd: &str, value: Option<&Value>, revision: &str) -> SettingsSnapshot {
    let fixture: Value =
        serde_json::from_str(include_str!("../../../../tests/fixtures/settings-ui-catalog.json"))
            .expect("fixture");
    let descriptor = fixture["catalog"]
        .as_array()
        .expect("catalog")
        .iter()
        .find(|entry| entry["id"] == SETTING_ID)
        .expect("footer descriptor");
    serde_json::from_value(json!({
        "cwd":cwd,"context":"footer-context","catalog":[descriptor],
        "categories":[{"id":"general","label":"General","short_label":"General"}],
        "sources":[{"scope":"user","path":"app-settings.json","status":"valid",
            "values":[{"id":SETTING_ID,"revision":revision,"value":value}]},
            {"scope":"project","path":"project-settings.json","status":"missing",
            "values":[{"id":SETTING_ID,"revision":"unset"}]}],
        "values":[{"id":SETTING_ID,"value":value,"contributors":["user"],"policy_restricted":false}],
        "resolution_sources":[],"provenance":{},"diagnostics":[]
    })).expect("snapshot")
}

fn app() -> (App, CommandReceiver) {
    let mut app = App::test_default();
    let (connection, commands) = AgentConnection::test_channel();
    app.session_runtime.conn = Some(Rc::new(connection));
    app.session_runtime.session_id = Some("session-1".into());
    app.surface_mode = crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
    app.config.snapshot = Some(snapshot(&app.cwd_raw, None, "r1"));
    (app, commands)
}

fn key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
    let _ = crate::app::handle_terminal_event(
        app,
        crossterm::event::Event::Key(KeyEvent::new(code, modifiers)),
    );
}

fn editor(app: &App) -> &FooterOverlayState {
    let Some(ConfigOverlayState::Footer(editor)) = &app.config.overlay else {
        panic!("footer editor")
    };
    editor
}

fn footer(app: &App) -> String {
    crate::ui::footer_rows::serialize_footer_rows(app, 160).rows[0]
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

#[test]
fn opening_navigation_and_read_only_scope_do_not_write_settings() {
    let (mut app, mut commands) = app();
    let before = footer(&app);
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    key(&mut app, KeyCode::Down, KeyModifiers::NONE);
    assert_eq!(footer(&app), before);
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(app.config.overlay.is_none());
    assert!(commands.try_recv().is_err());
    assert_eq!(footer(&app), before);
    app.config.selected_scope = SettingsScope::Project;
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(editor(&app).layout(), FooterLayout::default());
    key(&mut app, KeyCode::Char(' '), KeyModifiers::NONE);
    key(&mut app, KeyCode::Right, KeyModifiers::NONE);
    key(&mut app, KeyCode::Down, KeyModifiers::CONTROL);
    key(&mut app, KeyCode::Char('r'), KeyModifiers::CONTROL);
    assert_eq!(editor(&app).layout(), FooterLayout::default());
    assert!(commands.try_recv().is_err());
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(app.config.overlay.is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn rapid_edits_autosave_in_order_and_escape_waits_for_the_last_write() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (mut app, mut commands) = app();
            let original = footer(&app);
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            key(&mut app, KeyCode::Down, KeyModifiers::NONE);
            key(&mut app, KeyCode::Char(' '), KeyModifiers::NONE);
            key(&mut app, KeyCode::Right, KeyModifiers::NONE);
            assert_eq!(editor(&app).selected, 7);
            key(&mut app, KeyCode::Up, KeyModifiers::CONTROL);
            assert_eq!(editor(&app).selected, 6);
            assert_eq!(editor(&app).layout().fields()[6].id, FooterFieldId::Model);
            key(&mut app, KeyCode::Down, KeyModifiers::CONTROL);
            key(&mut app, KeyCode::Left, KeyModifiers::NONE);
            assert_eq!(editor(&app).selected, 4);
            assert_eq!(editor(&app).layout().fields()[4].side, FooterSide::Left);
            key(&mut app, KeyCode::Right, KeyModifiers::NONE);
            let request = commands.recv_envelope().await.expect("save command");
            let BridgeCommand::MutateSetting { mutation, .. } = request.command else {
                panic!("mutation")
            };
            assert_eq!(mutation.id, SETTING_ID);
            assert_eq!(mutation.scope, SettingsScope::User);
            assert_eq!(mutation.expected_revision, "r1");
            let value = mutation.value.expect("saved layout");
            assert_eq!(value[1], json!({"id":"model","visible":false,"side":"left"}));
            assert!(commands.try_recv().is_err(), "one write in flight");
            assert_eq!(footer(&app), original);
            assert!(app.config.overlay.is_some(), "wait for save acknowledgement");
            let saved = snapshot(&app.cwd_raw, Some(&value), "r2");
            crate::app::handle_client_event(
                &mut app,
                ClientEvent::SettingsResultReceived {
                    session_id: "session-1".into(),
                    request_id: request.request_id,
                    result: SettingsResult {
                        persistence: SettingsPersistence::Saved,
                        application: SettingsApplication::Host,
                        snapshot: Some(saved),
                        error: None,
                    },
                },
            );
            assert!(app.config.overlay.is_some(), "autosave keeps the editor open");
            assert!(!footer(&app).contains("test-model"));
            let request = commands.recv_envelope().await.expect("queued save");
            let BridgeCommand::MutateSetting { mutation, .. } = request.command else {
                panic!("mutation")
            };
            assert_eq!(mutation.expected_revision, "r2");
            let value = mutation.value.expect("latest layout");
            assert_eq!(value[7], json!({"id":"model","visible":false,"side":"right"}));
            key(&mut app, KeyCode::Char('r'), KeyModifiers::CONTROL);
            assert_eq!(editor(&app).layout(), FooterLayout::default());
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            assert!(app.config.overlay.is_some(), "wait for the last write before closing");
            let saved = snapshot(&app.cwd_raw, Some(&value), "r3");
            super::super::service::apply_settings_result(
                &mut app,
                request.request_id.as_deref(),
                SettingsResult {
                    persistence: SettingsPersistence::Saved,
                    application: SettingsApplication::Host,
                    snapshot: Some(saved),
                    error: None,
                },
            );
            assert!(app.config.overlay.is_some(), "queued reset must finish too");
            let request = commands.recv_envelope().await.expect("queued reset");
            let BridgeCommand::MutateSetting { mutation, .. } = request.command else {
                panic!("mutation")
            };
            assert_eq!(mutation.operation, crate::agent::settings::SettingsOperation::Remove);
            assert_eq!(mutation.expected_revision, "r3");
            assert!(mutation.value.is_none());
            let reset = snapshot(&app.cwd_raw, None, "r4");
            super::super::service::apply_settings_result(
                &mut app,
                request.request_id.as_deref(),
                SettingsResult {
                    persistence: SettingsPersistence::Saved,
                    application: SettingsApplication::Host,
                    snapshot: Some(reset),
                    error: None,
                },
            );
            assert!(app.config.overlay.is_none());
            assert!(app.config.saved_value(SETTING_ID).is_none());
            assert!(commands.try_recv().is_err());
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn escape_waits_for_autosave_and_reset_removes_the_preference() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (mut app, mut commands) = app();
            let original = footer(&app);
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            key(&mut app, KeyCode::Down, KeyModifiers::NONE);
            key(&mut app, KeyCode::Char(' '), KeyModifiers::NONE);
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            let request = commands.recv_envelope().await.expect("automatic save");
            let BridgeCommand::MutateSetting { mutation, .. } = request.command else {
                panic!("mutation")
            };
            assert!(app.config.overlay.is_some());
            let saved = snapshot(&app.cwd_raw, mutation.value.as_ref(), "r2");
            super::super::service::apply_settings_result(
                &mut app,
                request.request_id.as_deref(),
                SettingsResult {
                    persistence: SettingsPersistence::Saved,
                    application: SettingsApplication::Host,
                    snapshot: Some(saved),
                    error: None,
                },
            );
            assert!(app.config.overlay.is_none());
            assert!(!footer(&app).contains("test-model"));
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            key(&mut app, KeyCode::Char('r'), KeyModifiers::CONTROL);
            let request = commands.recv_envelope().await.expect("reset command");
            let BridgeCommand::MutateSetting { mutation, .. } = request.command else {
                panic!("mutation")
            };
            assert_eq!(mutation.operation, crate::agent::settings::SettingsOperation::Remove);
            assert!(mutation.value.is_none());
            let reset = snapshot(&app.cwd_raw, None, "r3");
            crate::app::handle_client_event(
                &mut app,
                ClientEvent::SettingsResultReceived {
                    session_id: "session-1".into(),
                    request_id: request.request_id,
                    result: SettingsResult {
                        persistence: SettingsPersistence::Saved,
                        application: SettingsApplication::Host,
                        snapshot: Some(reset),
                        error: None,
                    },
                },
            );
            assert!(app.config.overlay.is_some(), "reset keeps the editor open");
            assert_eq!(footer(&app), original);
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            assert!(app.config.overlay.is_none());
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn failed_save_and_conflict_keep_the_draft_and_retry_uses_the_new_revision() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (mut app, mut commands) = app();
            key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            key(&mut app, KeyCode::Char(' '), KeyModifiers::NONE);
            let draft = editor(&app).layout();
            for (persistence, revision) in
                [(SettingsPersistence::Failure, "r1"), (SettingsPersistence::Conflict, "r2")]
            {
                let request = commands.recv_envelope().await.expect("save");
                let refreshed = snapshot(&app.cwd_raw, Some(&json!([])), revision);
                super::super::service::apply_settings_result(
                    &mut app,
                    request.request_id.as_deref(),
                    SettingsResult {
                        persistence,
                        application: SettingsApplication::Blocked,
                        snapshot: Some(refreshed),
                        error: Some("Cannot save yet".into()),
                    },
                );
                assert_eq!(editor(&app).layout(), draft);
                assert!(app.config.overlay_message.is_some());
                assert!(commands.try_recv().is_err(), "failed writes do not retry themselves");
                key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
            }
            let request = commands.recv_envelope().await.expect("retry");
            let BridgeCommand::MutateSetting { mutation, .. } = request.command else {
                panic!("mutation")
            };
            assert_eq!(mutation.expected_revision, "r2");
            assert_eq!(mutation.value, Some(serde_json::to_value(draft).expect("draft")));
        })
        .await;
}

#[test]
fn settings_row_describes_the_active_footer_layout_across_scopes() {
    for scope in [SettingsScope::User, SettingsScope::Project] {
        for (value, expected) in [
            (None, "Default layout"),
            (Some(json!([])), "Custom layout"),
            (Some(json!([{"id":"model","visible":true,"side":"middle"}])), "Invalid layout"),
        ] {
            let (mut app, _) = app();
            app.config.selected_scope = scope;
            app.config.snapshot = Some(snapshot(&app.cwd_raw, value.as_ref(), "r1"));
            let mut terminal = Terminal::new(TestBackend::new(120, 30)).expect("terminal");
            terminal
                .draw(|frame| crate::ui::render_fullscreen_surface(frame, &mut app))
                .expect("draw");
            let buffer = terminal.backend().buffer();
            let lines: Vec<String> = buffer
                .content
                .chunks(120)
                .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect())
                .collect();
            let row = lines.iter().find(|line| line.contains("Footer")).expect("footer row");
            assert!(row.contains(expected), "{scope:?}: {row}");
            if value.is_none() {
                assert!(lines.iter().any(|line| line.contains("Using default layout")));
            }
        }
    }
}

#[test]
fn dialog_keeps_selected_field_controls_and_preview_in_their_regions() {
    for (width, height, error) in
        [(100, 30, false), (60, 20, false), (30, 12, false), (30, 12, true)]
    {
        let (mut app, _commands) = app();
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        key(&mut app, KeyCode::Down, KeyModifiers::NONE);
        key(&mut app, KeyCode::Char(' '), KeyModifiers::NONE);
        key(&mut app, KeyCode::Right, KeyModifiers::NONE);
        if error {
            app.config.set_overlay_error("This value changed since it was displayed. Review the refreshed saved value before retrying.");
        }
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal.draw(|frame| crate::ui::render_fullscreen_surface(frame, &mut app)).expect("draw");
        let buffer = terminal.backend().buffer();
        let lines: Vec<String> = buffer
            .content
            .chunks(usize::from(width))
            .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect())
            .collect();
        let selected = lines
            .iter()
            .position(|line| line.contains("Model") && line.contains("Right"))
            .expect("selected model row");
        assert!(lines[selected].contains("[ ]"));
        assert!(lines[selected].contains('›'));
        assert!(lines.iter().any(|line| line.contains("Space")), "toggle control");
        assert!(!lines.iter().any(|line| line.contains("Ctrl+S")), "autosave needs no save key");
        assert!(lines.iter().any(|line| line.contains("Esc")), "cancel control");
        let title = lines.iter().find(|line| line.contains("Footer")).expect("dialog title");
        let counter_column = title.find("8/8").expect("selected-field counter in title");
        let right_border = title.rfind('┐').expect("dialog top-right corner");
        assert_eq!(
            title[counter_column..right_border].chars().count(),
            4,
            "counter beside the top-right corner"
        );
        if width >= 60 {
            let preview =
                lines.iter().position(|line| line.contains("Preview")).expect("preview label") + 1;
            assert!(lines[preview].contains("FAST:OFF"));
            assert!(!lines[preview].contains("test-model"));
        }
    }
}

#[test]
fn failed_or_interrupted_autosaves_keep_edits_until_explicitly_closed() {
    for interrupted in [false, true] {
        let (mut app, mut commands) = app();
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        key(&mut app, KeyCode::Char(' '), KeyModifiers::NONE);
        let draft = editor(&app).layout();
        let request = commands.try_recv().expect("automatic save");
        if interrupted {
            app.config.invalidate_session();
        } else {
            super::super::service::apply_settings_result(
                &mut app,
                request.request_id.as_deref(),
                SettingsResult {
                    persistence: SettingsPersistence::Failure,
                    application: SettingsApplication::Blocked,
                    snapshot: None,
                    error: Some("Cannot write preferences".into()),
                },
            );
        }
        assert_eq!(editor(&app).layout(), draft);
        assert!(editor(&app).can_retry());
        assert!(app.config.overlay_message.is_some());
        assert!(commands.try_recv().is_err());
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(app.config.overlay.is_none());
        assert!(app.config.last_error.as_ref().expect("unsaved warning").contains("not saved"));
        assert!(commands.try_recv().is_err(), "closing after an error does not retry");
    }
}

#[test]
fn shrinking_below_the_minimum_keeps_edits_and_shows_a_close_hint() {
    let (mut app, _commands) = app();
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    key(&mut app, KeyCode::Down, KeyModifiers::NONE);
    key(&mut app, KeyCode::Char(' '), KeyModifiers::NONE);
    key(&mut app, KeyCode::Right, KeyModifiers::NONE);
    let edited = editor(&app).layout();
    for (width, height) in [(20, 8), (60, 20)] {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal.draw(|frame| crate::ui::render_fullscreen_surface(frame, &mut app)).expect("draw");
        let buffer = terminal.backend().buffer();
        let lines: Vec<String> = buffer
            .content
            .chunks(usize::from(width))
            .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect())
            .collect();
        if width == 20 {
            assert!(lines.iter().any(|line| line.contains("Esc close")));
        } else {
            let selected = lines
                .iter()
                .find(|line| line.contains("Model") && line.contains("Right"))
                .expect("selected field after growing");
            assert!(selected.contains("[ ]") && selected.contains('›'));
        }
        assert_eq!(editor(&app).layout(), edited);
    }
}

#[test]
fn invalid_layout_is_reported_and_location_changes_block_saving() {
    let (mut app, mut commands) = app();
    app.config.snapshot = Some(snapshot(
        &app.cwd_raw,
        Some(&json!([{"id":"model","visible":true,"side":"middle"}])),
        "r1",
    ));
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.config.overlay_message.is_some());
    assert_eq!(editor(&app).layout(), FooterLayout::default());
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(commands.try_recv().is_err(), "inspecting invalid preferences does not replace them");
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    app.cwd_raw = "/another-project".into();
    key(&mut app, KeyCode::Char(' '), KeyModifiers::NONE);
    assert!(commands.try_recv().is_err());
    assert!(app.config.overlay.is_some());
    assert!(app.config.overlay_message.as_ref().expect("error").text.contains("location changed"));
}
