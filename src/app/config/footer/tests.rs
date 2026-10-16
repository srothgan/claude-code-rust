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
async fn edits_persist_in_order_and_escape_waits_for_the_last_write() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (mut app, _directory) = fixture(None);
            let original = footer(&app);
            plain(&mut app, KeyCode::Enter);
            plain(&mut app, KeyCode::Down);
            plain(&mut app, KeyCode::Char(' '));
            plain(&mut app, KeyCode::Right);
            key(&mut app, KeyCode::Up, KeyModifiers::CONTROL);
            assert_eq!(
                editor(&app).layout().fields()[editor(&app).selected].id,
                FooterFieldId::Model
            );
            plain(&mut app, KeyCode::Esc);
            assert_eq!(
                footer(&app),
                original,
                "an unconfirmed write must not change the active footer"
            );
            let first = next_save(&mut app).await;
            let saved = document(&app);
            assert_eq!(
                saved["presentation"]["footer"][1],
                json!({"id":"model","visible":false,"side":"left"})
            );
            assert!(app.config.overlay.is_some());
            crate::app::handle_client_event(&mut app, first);
            assert!(app.config.footer.pending.is_some(), "the newer draft must be saved next");
            finish_saves(&mut app).await;
            assert!(app.config.overlay.is_none());
            let saved = document(&app);
            let entries = saved["presentation"]["footer"].as_array().expect("footer");
            let ids: Vec<_> =
                entries.iter().map(|entry| entry["id"].as_str().expect("id")).collect();
            assert_eq!(
                ids,
                [
                    "permissionMode",
                    "effort",
                    "ultracode",
                    "fastMode",
                    "directory",
                    "gitBranch",
                    "model",
                    "contextRemaining"
                ]
            );
            assert_eq!(entries[6], json!({"id":"model","visible":false,"side":"right"}));
            assert_eq!(saved["presentation"]["future"], 17);
            assert_eq!(saved["presentation"]["copyFullResponse"], true);
            assert_eq!(saved["updates"]["future"], "keep");
            assert!(!footer(&app).contains("test-model"));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn color_browsing_cancels_without_writes_and_confirming_or_clearing_saves_only_the_choice() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (mut app, _directory) = fixture(None);
            let original = document(&app);
            plain(&mut app, KeyCode::Enter);
            plain(&mut app, KeyCode::Char('c'));
            assert!(editor(&app).color_selection.is_none(), "permission mode is fixed");
            plain(&mut app, KeyCode::Down);
            plain(&mut app, KeyCode::Char('c'));
            for _ in 0..9 {
                plain(&mut app, KeyCode::Right);
            }
            assert_eq!(editor(&app).selected_color(), Some(FooterColor::Cyan));
            assert_eq!(
                field(&app, FooterFieldId::Model).color,
                None,
                "browsing is not a draft edit"
            );
            assert!(app.config.footer.pending.is_none());
            plain(&mut app, KeyCode::Esc);
            assert!(app.config.overlay.is_some(), "Esc collapses the inline picker");
            assert_eq!(document(&app), original);
            plain(&mut app, KeyCode::Char('c'));
            for _ in 0..9 {
                plain(&mut app, KeyCode::Right);
            }
            plain(&mut app, KeyCode::Enter);
            finish_saves(&mut app).await;
            assert_eq!(
                document(&app)["presentation"]["footer"][1],
                json!({"id":"model","visible":true,"side":"left","color":"cyan"})
            );
            let spans =
                crate::ui::footer_rows::serialize_footer_rows(&app, 160).rows[0].spans.clone();
            assert_eq!(
                spans
                    .iter()
                    .find(|span| span.content.contains("test-model"))
                    .expect("model")
                    .style
                    .fg,
                Some(Color::Rgb(34, 211, 238))
            );
            plain(&mut app, KeyCode::Char('c'));
            plain(&mut app, KeyCode::Home);
            plain(&mut app, KeyCode::Enter);
            finish_saves(&mut app).await;
            assert_eq!(
                document(&app)["presentation"]["footer"][1],
                json!({"id":"model","visible":true,"side":"left"})
            );
            let footer = crate::ui::footer_rows::serialize_footer_rows(&app, 160);
            assert_eq!(
                footer.rows[0]
                    .spans
                    .iter()
                    .find(|span| span.content.contains("test-model"))
                    .expect("model")
                    .style
                    .fg,
                Some(Color::Gray)
            );
            assert_eq!(document(&app)["presentation"]["future"], 17);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn effort_color_persists_across_levels_and_current_restores_its_original_color() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (mut app, _directory) = fixture(None);
            app.session_runtime.current_model = Some(
                crate::agent::model::CurrentModel::new("opus", "Opus", "Opus")
                    .supports_effort(true),
            );
            plain(&mut app, KeyCode::Enter);
            for _ in 0..editor(&app).layout().fields().len() {
                if editor(&app).layout().fields()[editor(&app).selected].id == FooterFieldId::Effort
                {
                    break;
                }
                plain(&mut app, KeyCode::Down);
            }
            assert_eq!(
                editor(&app).layout().fields()[editor(&app).selected].id,
                FooterFieldId::Effort
            );
            plain(&mut app, KeyCode::Char('c'));
            assert!(editor(&app).color_selection.is_some(), "Effort offers the color picker");
            plain(&mut app, KeyCode::Home);
            plain(&mut app, KeyCode::Right);
            plain(&mut app, KeyCode::Enter);
            finish_saves(&mut app).await;
            let saved = document(&app);
            let effort = saved["presentation"]["footer"]
                .as_array()
                .expect("footer")
                .iter()
                .find(|field| field["id"] == "effort")
                .expect("saved Effort");
            assert_eq!(effort["color"], "red");
            app.config.load_footer(app.global_settings_path.as_deref());
            for (level, label) in [("low", "[Low]"), ("high", "[High]")] {
                app.session_runtime.config_options.insert("effortLevel".into(), json!(level));
                let footer = crate::ui::footer_rows::serialize_footer_rows(&app, 160);
                let span = footer.rows[0]
                    .spans
                    .iter()
                    .find(|span| span.content == label)
                    .expect("current effort level");
                assert_eq!(span.style.fg, Some(Color::Rgb(248, 113, 113)));
            }
            plain(&mut app, KeyCode::Char('c'));
            plain(&mut app, KeyCode::Home);
            plain(&mut app, KeyCode::Enter);
            finish_saves(&mut app).await;
            let saved = document(&app);
            assert!(
                saved["presentation"]["footer"]
                    .as_array()
                    .expect("footer")
                    .iter()
                    .find(|field| field["id"] == "effort")
                    .expect("saved Effort")
                    .get("color")
                    .is_none()
            );
            let footer = crate::ui::footer_rows::serialize_footer_rows(&app, 160);
            assert_eq!(
                footer.rows[0]
                    .spans
                    .iter()
                    .find(|span| span.content == "[High]")
                    .expect("effort")
                    .style
                    .fg,
                Some(Color::Gray)
            );
            assert_eq!(field(&app, FooterFieldId::Effort).color, None);
        })
        .await;
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
async fn a_conflicting_external_footer_is_preserved_until_an_explicit_retry() {
    tokio::task::LocalSet::new().run_until(async {
        let (mut app, _directory) = fixture(None);
        let active = footer(&app);
        plain(&mut app, KeyCode::Enter);
        plain(&mut app, KeyCode::Down);
        plain(&mut app, KeyCode::Char(' '));
        let external = json!({"presentation":{"footer":[],"future":"external change"},"updates":{"other":42}});
        std::fs::write(app.global_settings_path.as_ref().expect("path"), external.to_string()).expect("external edit");
        finish_saves(&mut app).await;
        assert_eq!(document(&app), external);
        assert_eq!(footer(&app), active, "a failed save cannot install another value");
        assert!(!field(&app, FooterFieldId::Model).visible);
        assert!(app.config.overlay_message.is_some());
        assert!(app.config.footer.pending.is_none(), "conflicts never retry automatically");
        plain(&mut app, KeyCode::Enter);
        finish_saves(&mut app).await;
        assert_eq!(document(&app)["presentation"]["future"], "external change");
        assert_eq!(document(&app)["updates"]["other"], 42);
        assert_eq!(document(&app)["presentation"]["footer"][1]["visible"], false);
        assert!(!footer(&app).contains("test-model"));
    }).await;
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
async fn opening_the_editor_during_a_row_reset_applies_new_edits_to_the_requested_defaults() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (mut app, _fixture) = fixture(Some(json!([
                {"id":"model","visible":true,"side":"right","color":"cyan"}
            ])));
            let active = footer(&app);
            plain(&mut app, KeyCode::Delete);
            assert!(app.config.footer.pending.is_some(), "the row reset is saving");
            plain(&mut app, KeyCode::Enter);
            assert_eq!(editor(&app).layout(), FooterLayout::default());
            plain(&mut app, KeyCode::Char(' '));
            plain(&mut app, KeyCode::Esc);
            assert_eq!(footer(&app), active, "the reset is not yet acknowledged");
            finish_saves(&mut app).await;
            assert!(app.config.overlay.is_none());
            let saved = document(&app);
            let entries = saved["presentation"]["footer"].as_array().expect("saved draft");
            for id in [
                "permissionMode",
                "model",
                "effort",
                "ultracode",
                "fastMode",
                "directory",
                "gitBranch",
                "contextRemaining",
            ] {
                let entry = entries.iter().find(|entry| entry["id"] == id).expect("restored field");
                assert_eq!(entry["visible"], id != "permissionMode");
                assert!(entry.get("color").is_none(), "the reset cleared the old override");
            }
            let model = entries.iter().find(|entry| entry["id"] == "model").expect("model");
            assert_eq!(model["side"], "left", "new edits must not restore the pre-reset layout");
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
fn browsing_read_only_scopes_and_fixed_badges_never_write() {
    for scope in [SettingsScope::User, SettingsScope::Project, SettingsScope::Local] {
        let (mut app, _directory) = fixture(None);
        app.config.selected_scope = scope;
        let before = document(&app);
        plain(&mut app, KeyCode::Enter);
        for code in [KeyCode::Down, KeyCode::Home, KeyCode::End] {
            plain(&mut app, code);
        }
        if scope != SettingsScope::User {
            for code in [KeyCode::Char(' '), KeyCode::Right, KeyCode::Char('c')] {
                plain(&mut app, code);
            }
            key(&mut app, KeyCode::Up, KeyModifiers::CONTROL);
            key(&mut app, KeyCode::Char('r'), KeyModifiers::CONTROL);
            assert_eq!(editor(&app).layout(), FooterLayout::default());
            assert!(editor(&app).color_selection.is_none());
        }
        plain(&mut app, KeyCode::Esc);
        assert_eq!(document(&app), before);
        assert!(app.config.footer.pending.is_none());
    }
}

#[test]
fn fixed_labels_are_readable_and_palette_rows_have_spacing() {
    let (mut app, _fixture) = fixture(None);
    plain(&mut app, KeyCode::Enter);
    let buffer = render(&mut app, 100, 30);
    let permission = find_row(&buffer, "Permission mode");
    assert!(row(&buffer, permission).contains("Fixed"));
    let fixed_x = (0..96)
        .find(|&x| {
            (x..x + 5)
                .zip(["F", "i", "x", "e", "d"])
                .all(|(column, symbol)| buffer[(column, permission)].symbol() == symbol)
        })
        .expect("fixed color label");
    for x in fixed_x..fixed_x + 5 {
        assert_eq!(buffer[(x, permission)].fg, Color::White);
        assert!(buffer[(x, permission)].modifier.contains(ratatui::style::Modifier::BOLD));
    }
    assert!(row(&buffer, permission).contains('›'));
    plain(&mut app, KeyCode::Down);
    let buffer = render(&mut app, 100, 30);
    let model = find_row(&buffer, "Model");
    assert!(row(&buffer, model).contains("Current"));
    assert!(row(&buffer, model).contains('›'));
    assert!(find_row(&buffer, "C color") > model);
    plain(&mut app, KeyCode::Char('c'));
    let buffer = render(&mut app, 100, 30);
    let position = |color| {
        (0..30)
            .flat_map(|y| (0..100).map(move |x| (x, y)))
            .find(|&position| buffer[position].bg == color)
            .expect("color box")
    };
    let red = position(Color::Rgb(248, 113, 113));
    let orange = position(Color::Rgb(251, 146, 60));
    let blue = position(Color::Rgb(96, 165, 250));
    assert_eq!(orange.0 - red.0 - 2, 2, "two cells separate color boxes horizontally");
    assert_eq!(blue.1 - red.1 - 1, 1, "a blank row separates palette rows");
    assert!((red.0..orange.0 + 2).all(|x| buffer[(x, red.1 + 1)].symbol() == " "));
}

#[test]
fn inline_palette_keeps_the_field_focused_and_uses_background_boxes_through_resize_and_errors() {
    for (width, height, error) in
        [(100, 30, false), (60, 20, false), (30, 12, false), (30, 12, true), (20, 8, false)]
    {
        let (mut app, _directory) = fixture(None);
        plain(&mut app, KeyCode::Enter);
        plain(&mut app, KeyCode::Down);
        plain(&mut app, KeyCode::Char('c'));
        for _ in 0..9 {
            plain(&mut app, KeyCode::Right);
        }
        if error {
            app.config.set_overlay_error("The file is locked. Retry your save.");
        }
        let buffer = render(&mut app, width, height);
        let selected = find_row(&buffer, "Model");
        assert!(row(&buffer, selected).contains('›'));
        let cyan: Vec<_> = (0..height)
            .flat_map(|y| (0..width).map(move |x| (x, y)))
            .filter(|&(x, y)| buffer[(x, y)].bg == Color::Rgb(34, 211, 238))
            .collect();
        assert_eq!(cyan.len(), 2, "two filled cells form one cyan box at {width}x{height}");
        assert_eq!(cyan[0].1, cyan[1].1);
        assert!(cyan[0].1 > selected, "picker belongs below the selected field");
        assert_eq!(buffer[cyan[0]].symbol(), " ");
        assert_eq!(buffer[cyan[1]].symbol(), " ");
        assert_eq!(buffer[(cyan[0].0 - 1, cyan[0].1)].symbol(), "[");
        assert_eq!(buffer[(cyan[1].0 + 1, cyan[1].1)].symbol(), "]");
        if width >= 60 {
            let next = find_row(&buffer, "Effort");
            assert!(cyan[0].1 < next, "palette is inserted before the next field");
        }
        let label = find_row(&buffer, "Cyan");
        assert!(label > cyan[0].1);
        assert!(row(&buffer, find_row(&buffer, "Enter")).contains("Enter"));
        assert!(row(&buffer, find_row(&buffer, "Esc")).contains("Esc"));
        assert_eq!(field(&app, FooterFieldId::Model).side, FooterSide::Left);
        assert!(app.config.footer.pending.is_none());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn arrow_navigation_uses_the_visible_grid_and_confirms_the_same_field_after_resizing() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (mut app, _directory) = fixture(None);
            plain(&mut app, KeyCode::Enter);
            plain(&mut app, KeyCode::Down);
            plain(&mut app, KeyCode::Char('c'));
            render(&mut app, 30, 12);
            plain(&mut app, KeyCode::Right);
            plain(&mut app, KeyCode::Down);
            assert_eq!(
                editor(&app).selected_color(),
                Some(FooterColor::Green),
                "five-column grid moves from red to green"
            );
            render(&mut app, 100, 30);
            plain(&mut app, KeyCode::Down);
            assert_eq!(
                editor(&app).selected_color(),
                Some(FooterColor::Pink),
                "ten-column grid moves to pink"
            );
            plain(&mut app, KeyCode::Enter);
            finish_saves(&mut app).await;
            assert_eq!(
                document(&app)["presentation"]["footer"][1],
                json!({"id":"model","visible":true,"side":"left","color":"pink"})
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
async fn the_first_startup_frame_and_real_session_transitions_keep_saved_alignment_and_colors() {
    use crate::agent::model;
    use clap::Parser;
    tokio::task::LocalSet::new()
        .run_until(async {
            let (_fixture_app, directory) = fixture(Some(json!([
                {"id":"directory","visible":true,"side":"left","color":"orange"},
                {"id":"model","visible":true,"side":"right","color":"cyan"}
            ])));
            let mut cli =
                crate::Cli::try_parse_from(["claude-rs", "--no-update-check"]).expect("CLI");
            cli.dir = Some(directory.path().to_path_buf());
            let path = directory.path().join("settings.json");
            let mut app = crate::app::connect::create_app_with_settings(
                &cli,
                crate::app::settings::LoadedAppSettings {
                    settings: crate::app::settings::load_from_path(&path).expect("load"),
                    path: Some(path),
                },
            );
            assert_eq!(app.status, crate::app::AppStatus::Connecting);
            assert_location_color_and_alignment(&app);
            let (connection, _commands) = crate::agent::client::AgentConnection::test_channel();
            app.session_runtime.conn = Some(std::rc::Rc::new(connection));
            crate::app::handle_client_event(
                &mut app,
                ClientEvent::AuthRequired {
                    method_name: "login".into(),
                    method_description: "Login".into(),
                },
            );
            assert_location_color_and_alignment(&app);
            crate::app::handle_client_event(
                &mut app,
                ClientEvent::Connected {
                    session_id: "session-1".into(),
                    cwd: directory.path().display().to_string(),
                    current_model: model::CurrentModel::new("opus", "Opus", "Opus"),
                    available_models: vec![],
                    mode: None,
                    fast_mode_state: model::FastModeState::Off,
                    fast_mode_disabled_reason: None,
                    ultracode: None,
                    history_updates: vec![],
                },
            );
            assert_location_color_and_alignment(&app);
            let line = crate::ui::footer_rows::serialize_footer_rows(&app, 160);
            assert_eq!(
                line.rows[0]
                    .spans
                    .iter()
                    .find(|span| span.content.contains("[Opus]"))
                    .expect("model")
                    .style
                    .fg,
                Some(Color::Rgb(34, 211, 238))
            );
            crate::app::handle_client_event(
                &mut app,
                ClientEvent::SessionReplaced {
                    session_id: "session-2".into(),
                    cwd: directory.path().display().to_string(),
                    current_model: model::CurrentModel::new("sonnet", "Sonnet", "Sonnet"),
                    available_models: vec![],
                    mode: None,
                    fast_mode_state: model::FastModeState::Off,
                    fast_mode_disabled_reason: None,
                    ultracode: None,
                    history_updates: vec![],
                    restored_input: None,
                },
            );
            assert_location_color_and_alignment(&app);
            assert!(app.config.footer.saved().is_some());
        })
        .await;
}
fn assert_location_color_and_alignment(app: &App) {
    let line = crate::ui::footer_rows::serialize_footer_rows(app, 160);
    let first = line.rows[0].spans.first().expect("first footer field");
    assert!(first.content.starts_with("Loc: "));
    assert_eq!(first.style.fg, Some(Color::Rgb(251, 146, 60)));
    assert!(!line.rows[0].spans.iter().any(|span| span.content.contains("FAST:")));
}

#[tokio::test(flavor = "current_thread")]
async fn reopening_after_discarding_a_conflict_reads_the_external_baseline_before_the_next_edit() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (mut app, _directory) = fixture(None);
            plain(&mut app, KeyCode::Enter);
            plain(&mut app, KeyCode::Down);
            plain(&mut app, KeyCode::Char(' '));
            std::fs::write(
                app.global_settings_path.as_ref().expect("path"),
                r#"{"presentation":{"footer":[]},"external":true}"#,
            )
            .expect("external edit");
            finish_saves(&mut app).await;
            plain(&mut app, KeyCode::Esc);
            plain(&mut app, KeyCode::Enter);
            assert!(editor(&app).layout().fields().iter().all(|field| !field.visible));
            plain(&mut app, KeyCode::Down);
            plain(&mut app, KeyCode::Char(' '));
            finish_saves(&mut app).await;
            let saved = document(&app);
            let fields = saved["presentation"]["footer"].as_array().expect("footer");
            let visible: Vec<_> = fields
                .iter()
                .filter(|field| field["visible"] == true)
                .map(|field| field["id"].as_str().expect("id"))
                .collect();
            assert_eq!(visible, ["model"]);
            assert_eq!(saved["external"], true);
        })
        .await;
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

#[test]
fn the_initial_settings_catalog_selects_and_highlights_the_top_setting_instead_of_footer() {
    let (mut app, _fixture) = fixture(None);
    app.config.settings = Box::default();
    assert_eq!(app.config.selected_setting().expect("initial local row").id, SETTING_ID);
    let catalog: Value =
        serde_json::from_str(include_str!("../../../../tests/fixtures/settings-ui-catalog.json"))
            .expect("catalog");
    let descriptor = catalog["catalog"]
        .as_array()
        .expect("catalog")
        .iter()
        .find(|entry| entry["id"] == "model")
        .expect("SDK descriptor");
    let mut incoming = SettingsSnapshot::test_value("model", json!("sonnet"));
    incoming.catalog.push(serde_json::from_value(descriptor.clone()).expect("descriptor"));
    incoming.cwd.clone_from(&app.cwd_raw);
    app.config.pending_settings_request =
        Some(super::super::PendingSettingsRequest::Inspection("inspection".into()));
    crate::app::handle_client_event(
        &mut app,
        ClientEvent::SettingsResultReceived {
            session_id: "session-1".into(),
            request_id: Some("inspection".into()),
            result: SettingsResult {
                persistence: SettingsPersistence::NotRequested,
                application: SettingsApplication::Host,
                snapshot: Some(incoming),
                error: None,
            },
        },
    );
    assert_eq!(app.config.selected_setting().expect("top setting").id, "model");
    let buffer = render(&mut app, 100, 30);
    assert!(row(&buffer, find_row(&buffer, "Default model")).contains('›'));
    assert!(!row(&buffer, find_row(&buffer, "Footer")).contains('›'));
}

#[test]
fn reopening_config_discards_selection_filter_and_scroll_in_current_and_new_sessions() {
    for new_session in [false, true] {
        let (mut app, _fixture) = fixture(None);
        let catalog: Value = serde_json::from_str(include_str!(
            "../../../../tests/fixtures/settings-ui-catalog.json"
        ))
        .expect("catalog");
        app.config.snapshot.as_mut().expect("snapshot").catalog.push(
            serde_json::from_value(
                catalog["catalog"]
                    .as_array()
                    .expect("catalog")
                    .iter()
                    .find(|entry| entry["id"] == "model")
                    .expect("Default model control")
                    .clone(),
            )
            .expect("descriptor"),
        );
        app.config.project_footer(app.global_settings_path.as_deref());
        let mut incoming = app.config.snapshot.clone().expect("loaded settings");
        incoming.cwd.clone_from(&app.cwd_raw);
        app.config.settings.select(SETTING_ID.into());
        app.config.settings.position_mut().scroll = 20;
        app.config.settings.query = "Footer".into();
        app.config.settings.cursor = 6;
        super::super::controller::close(&mut app);
        if new_session {
            app.bump_session_scope_epoch();
            app.session_runtime.session_id = Some("session-2".into());
        }
        app.input.set_text("/config");
        crate::app::input_submit::submit_input(&mut app);
        let request_id = app
            .config
            .pending_settings_request
            .as_ref()
            .expect("inspection")
            .request_id()
            .to_owned();
        let session_id = app.session_runtime.session_id.as_ref().expect("session").to_string();
        crate::app::handle_client_event(
            &mut app,
            ClientEvent::SettingsResultReceived {
                session_id,
                request_id: Some(request_id),
                result: SettingsResult {
                    persistence: SettingsPersistence::NotRequested,
                    application: SettingsApplication::Host,
                    snapshot: Some(incoming),
                    error: None,
                },
            },
        );
        assert!(app.config.settings.query.is_empty());
        assert_eq!(app.config.settings.focus, super::super::SettingsFocus::Content);
        assert_eq!(app.config.selected_setting().expect("top control").id, "model");
        let buffer = render(&mut app, 100, 30);
        let top = find_row(&buffer, "Default model");
        let text = row(&buffer, top);
        assert!(text.contains('›'));
        let x = u16::try_from(text.chars().position(|ch| ch == 'D').expect("control label"))
            .expect("column");
        assert_eq!(buffer[(x, top)].bg, crate::ui::theme::USER_MSG_BG);
        assert_eq!(app.config.settings.position().map_or(0, |position| position.scroll), 0);
    }
}
