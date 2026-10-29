// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::agent::events::ClientEvent;
use crate::agent::settings::{
    SettingsApplication, SettingsPersistence, SettingsResult, SettingsScope, SettingsSnapshot,
};
use crate::app::footer::{FooterFieldId, FooterSide, SETTING_ID};
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer, style::Color};
use serde_json::{Value, json};

struct Fixture {
    directory: tempfile::TempDir,
    commands: crate::agent::client::CommandReceiver,
}
impl Fixture {
    fn path(&self) -> &std::path::Path {
        self.directory.path()
    }
}

fn fixture(value: Option<Value>) -> (App, Fixture) {
    let directory = tempfile::tempdir().expect("fixture directory");
    let path = directory.path().join("settings.json");
    let mut document =
        json!({"presentation":{"copyFullResponse":true,"future":17},"updates":{"future":"keep"}});
    if let Some(value) = value {
        document["presentation"]["footer"] = value;
    }
    std::fs::write(&path, document.to_string()).expect("settings fixture");
    let mut app = App::test_default();
    app.global_settings_path = Some(path);
    let (connection, commands) = crate::agent::client::AgentConnection::test_channel();
    app.session_runtime.conn = Some(std::rc::Rc::new(connection));
    app.session_runtime.session_id = Some("session-1".into());
    app.config.load_footer(app.global_settings_path.as_deref());
    app.surface_mode = crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
    app.config.settings.select(SETTING_ID.into());
    (app, Fixture { directory, commands })
}
fn document(app: &App) -> Value {
    serde_json::from_slice(
        &std::fs::read(app.global_settings_path.as_ref().expect("path")).expect("read"),
    )
    .expect("JSON")
}
fn key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
    crate::app::handle_terminal_event(
        app,
        crossterm::event::Event::Key(KeyEvent::new(code, modifiers)),
    );
}
fn plain(app: &mut App, code: KeyCode) {
    key(app, code, KeyModifiers::NONE);
}
fn editor(app: &App) -> &FooterOverlayState {
    let Some(ConfigOverlayState::Footer(editor)) = &app.config.overlay else {
        panic!("footer editor");
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
async fn next_save(app: &mut App) -> ClientEvent {
    let event = tokio::time::timeout(std::time::Duration::from_secs(5), app.event_rx.recv())
        .await
        .expect("save deadline")
        .expect("save event");
    assert!(matches!(event, ClientEvent::FooterSaved { .. }));
    event
}
async fn finish_saves(app: &mut App) {
    while app.config.footer.pending.is_some() {
        let event = next_save(app).await;
        crate::app::handle_client_event(app, event);
    }
}
fn render(app: &mut App, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal.draw(|frame| crate::ui::render_fullscreen_surface(frame, app)).expect("draw");
    terminal.backend().buffer().clone()
}
fn row(buffer: &Buffer, y: u16) -> String {
    (0..buffer.area.width).map(|x| buffer[(x, y)].symbol()).collect()
}
fn find_row(buffer: &Buffer, text: &str) -> u16 {
    (0..buffer.area.height).find(|&y| row(buffer, y).contains(text)).unwrap_or_else(|| {
        panic!(
            "missing {text} in {:?}",
            (0..buffer.area.height).map(|y| row(buffer, y)).collect::<Vec<_>>()
        )
    })
}
fn field(app: &App, id: FooterFieldId) -> crate::app::footer::FooterField {
    *editor(app).layout().fields().iter().find(|field| field.id == id).expect("field")
}

#[tokio::test(flavor = "current_thread")]
async fn a_locked_file_keeps_the_active_value_and_draft_until_an_explicit_retry_or_discard() {
    tokio::task::LocalSet::new()
        .run_until(async {
            for retry in [false, true] {
                let (mut app, _directory) = fixture(None);
                let original = document(&app);
                let active = footer(&app);
                plain(&mut app, KeyCode::Enter);
                let lock = crate::json_file::lock(app.global_settings_path.as_ref().expect("path"))
                    .expect("other writer");
                plain(&mut app, KeyCode::Down);
                plain(&mut app, KeyCode::Char(' '));
                finish_saves(&mut app).await;
                assert_eq!(document(&app), original);
                assert_eq!(footer(&app), active);
                assert!(!field(&app, FooterFieldId::Model).visible);
                assert!(editor(&app).can_retry());
                assert!(app.config.overlay_message.is_some());
                assert!(app.config.footer.pending.is_none());
                drop(lock);
                if retry {
                    plain(&mut app, KeyCode::Enter);
                    finish_saves(&mut app).await;
                    assert_eq!(document(&app)["presentation"]["footer"][1]["visible"], false);
                    assert!(!footer(&app).contains("test-model"));
                } else {
                    plain(&mut app, KeyCode::Esc);
                    assert!(app.config.overlay.is_none());
                    assert!(
                        app.config
                            .last_error
                            .as_ref()
                            .expect("unsaved warning")
                            .contains("not saved")
                    );
                    assert_eq!(document(&app), original);
                }
            }
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn reset_queued_during_a_save_removes_only_the_footer_after_both_writes_finish() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (mut app, _directory) = fixture(None);
            let original = document(&app);
            plain(&mut app, KeyCode::Enter);
            plain(&mut app, KeyCode::Down);
            plain(&mut app, KeyCode::Char(' '));
            key(&mut app, KeyCode::Char('r'), KeyModifiers::CONTROL);
            plain(&mut app, KeyCode::Esc);
            let first = next_save(&mut app).await;
            assert!(document(&app)["presentation"].get("footer").is_some());
            crate::app::handle_client_event(&mut app, first);
            finish_saves(&mut app).await;
            assert_eq!(document(&app), original);
            assert!(app.config.footer.saved().is_none());
            assert!(app.config.overlay.is_none());
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_row_reset_conflict_keeps_the_file_and_active_layout_until_delete_is_retried() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (mut app, _fixture) = fixture(Some(json!([
                {"id":"model","visible":true,"side":"right","color":"cyan"}
            ])));
            let active = footer(&app);
            let external = json!({"presentation":{"footer":[],"future":"outside"},"outside":true});
            std::fs::write(app.global_settings_path.as_ref().expect("path"), external.to_string())
                .expect("external edit");
            plain(&mut app, KeyCode::Delete);
            finish_saves(&mut app).await;
            assert_eq!(document(&app), external);
            assert_eq!(footer(&app), active);
            assert!(app.config.overlay.is_none());
            assert!(app.config.last_error.is_some());
            assert!(app.config.footer.pending.is_none(), "no automatic conflict retry");
            plain(&mut app, KeyCode::Delete);
            finish_saves(&mut app).await;
            assert_eq!(document(&app), json!({"presentation":{"future":"outside"},"outside":true}));
            assert!(app.config.footer.saved().is_none());
            assert!(app.config.last_error.is_none());
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn native_saves_survive_authentication_and_sdk_inspection_without_bridge_mutations() {
    use crate::agent::client::AgentConnection;
    use std::rc::Rc;
    tokio::task::LocalSet::new()
        .run_until(async {
            let (mut app, _directory) = fixture(None);
            let (connection, mut commands) = AgentConnection::test_channel();
            app.session_runtime.conn = Some(Rc::new(connection));
            app.session_runtime.session_id = Some("old-session".into());
            app.config.pending_settings_request =
                Some(super::super::PendingSettingsRequest::Inspection("sdk-inspection".into()));
            plain(&mut app, KeyCode::Enter);
            plain(&mut app, KeyCode::Down);
            plain(&mut app, KeyCode::Char(' '));
            let request =
                app.config.footer.pending.as_ref().expect("local write").request_id.clone();
            let mut snapshot = SettingsSnapshot::test_value("language", json!("German"));
            snapshot.cwd.clone_from(&app.cwd_raw);
            crate::app::handle_client_event(
                &mut app,
                ClientEvent::SettingsResultReceived {
                    session_id: "old-session".into(),
                    request_id: Some("sdk-inspection".into()),
                    result: SettingsResult {
                        persistence: SettingsPersistence::NotRequested,
                        application: SettingsApplication::Host,
                        snapshot: Some(snapshot),
                        error: None,
                    },
                },
            );
            assert_eq!(
                app.config.footer.pending.as_ref().expect("unaffected local write").request_id,
                request
            );
            crate::app::handle_client_event(
                &mut app,
                ClientEvent::AuthRequired {
                    method_name: "login".into(),
                    method_description: "Authenticate".into(),
                },
            );
            assert_eq!(
                app.config.footer.pending.as_ref().expect("survives auth").request_id,
                request
            );
            assert!(app.session_runtime.session_id.is_none());
            assert!(
                app.config
                    .snapshot
                    .as_ref()
                    .expect("offline catalog")
                    .catalog
                    .iter()
                    .any(|entry| entry.id == SETTING_ID)
            );
            finish_saves(&mut app).await;
            assert_eq!(document(&app)["presentation"]["footer"][1]["visible"], false);
            assert!(!field(&app, FooterFieldId::Model).visible);
            assert!(
                commands.try_recv().is_err(),
                "local preferences must not produce bridge commands"
            );
        })
        .await;
}

#[test]
fn malformed_saved_layouts_report_errors_without_replacing_the_file_on_inspection() {
    for value in [
        json!([{"id":"model","visible":true,"side":"middle"}]),
        json!([{"id":"fastMode","visible":true,"side":"left","color":"cyan"}]),
    ] {
        let (mut app, _directory) = fixture(Some(value));
        let before = document(&app);
        let buffer = render(&mut app, 120, 30);
        let footer_row = find_row(&buffer, "Footer");
        assert!(row(&buffer, footer_row).contains("Invalid layout"));
        plain(&mut app, KeyCode::Enter);
        assert!(app.config.overlay_message.is_some());
        plain(&mut app, KeyCode::Esc);
        assert_eq!(document(&app), before);
        assert!(app.config.footer.pending.is_none());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn config_can_edit_local_footer_preferences_after_auth_required_without_an_active_session() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (mut app, mut fixture) = fixture(None);
            app.surface_mode = crate::app::SurfaceMode::Chat;
            crate::app::handle_client_event(
                &mut app,
                ClientEvent::AuthRequired {
                    method_name: "login".into(),
                    method_description: "Authenticate".into(),
                },
            );
            assert_eq!(app.status, crate::app::AppStatus::Ready);
            assert!(app.session_runtime.conn.is_some(), "the bridge exists");
            assert!(
                app.session_runtime.session_id.is_none(),
                "authentication has no active agent session"
            );
            app.input.set_text("/config");
            crate::app::input_submit::submit_input(&mut app);
            assert_eq!(
                app.surface_mode,
                crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config)
            );
            assert!(app.input.is_empty());
            let buffer = render(&mut app, 100, 30);
            assert!(row(&buffer, find_row(&buffer, "Footer")).contains("Default layout"));
            plain(&mut app, KeyCode::Enter);
            plain(&mut app, KeyCode::Down);
            plain(&mut app, KeyCode::Char(' '));
            finish_saves(&mut app).await;
            assert_eq!(document(&app)["presentation"]["footer"][1]["visible"], false);
            assert!(
                fixture.commands.try_recv().is_err(),
                "no agent settings inspection or mutation is needed"
            );
        })
        .await;
}

#[path = "audit_tests.rs"]
mod audit;
