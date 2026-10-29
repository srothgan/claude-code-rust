// SPDX-License-Identifier: Apache-2.0

use super::*;
fn select(app: &mut App, id: FooterFieldId) {
    plain(app, KeyCode::Home);
    for _ in 0..editor(app).layout().fields().len() {
        if editor(app).layout().fields()[editor(app).selected].id == id {
            return;
        }
        plain(app, KeyCode::Down);
    }
    panic!("missing field {id:?}");
}
fn color(app: &mut App, choice: Option<FooterColor>) {
    plain(app, KeyCode::Char('c'));
    plain(app, KeyCode::Home);
    for _ in 0..=FooterColor::ALL.len() {
        if editor(app).selected_color() == choice {
            return;
        }
        plain(app, KeyCode::Right);
    }
    panic!("missing color");
}
fn saved_field(value: &Value, id: &str) -> Value {
    value["presentation"]["footer"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == id)
        .unwrap()
        .clone()
}
fn active_color(app: &App, needle: &str) -> Color {
    crate::ui::footer_rows::serialize_footer_rows(app, 180).rows[0]
        .spans
        .iter()
        .find(|s| s.content.contains(needle))
        .unwrap()
        .style
        .fg
        .unwrap()
}
fn footer_buffer(app: &App) -> Buffer {
    use ratatui::widgets::Widget;
    let mut buffer = Buffer::empty(ratatui::layout::Rect::new(0, 0, 180, 1));
    ratatui::widgets::Paragraph::new(
        crate::ui::footer_rows::serialize_footer_rows(app, 180).rows[0].clone(),
    )
    .render(buffer.area, &mut buffer);
    buffer
}
fn assert_footer_alignment(app: &App) {
    let buffer = footer_buffer(app);
    let text = row(&buffer, 0);
    assert!(text.starts_with("Loc: "));
    assert!(text.contains(&app.cwd));
    assert_eq!(buffer[(0, 0)].fg, FooterColor::Orange.color());
    assert!(!text.contains("FAST:"));
    if let Some(model) = &app.session_runtime.current_model {
        let label = format!("[{}]", model.display_name_short);
        let start = text.find(&label).unwrap();
        let x = u16::try_from(unicode_width::UnicodeWidthStr::width(&text[..start])).unwrap();
        assert!(x > 90, "model must remain in the right group: {text}");
        assert!(!text.ends_with(' '), "right group must reach the edge");
        assert_eq!(buffer[(x, 0)].fg, FooterColor::Cyan.color());
    }
}
fn swatch(buffer: &Buffer, color: FooterColor) -> Option<(u16, u16)> {
    (0..buffer.area.height)
        .flat_map(|y| (0..buffer.area.width).map(move |x| (x, y)))
        .find(|&p| buffer[p].bg == color.color())
}
fn selected_swatch(buffer: &Buffer, color: FooterColor) -> (u16, u16) {
    let start = swatch(buffer, color).unwrap();
    let end = (start.0..buffer.area.width)
        .take_while(|&x| buffer[(x, start.1)].bg == color.color())
        .last()
        .unwrap();
    assert!(!buffer[(start.0 - 1, start.1)].symbol().trim().is_empty());
    assert!(!buffer[(end + 1, start.1)].symbol().trim().is_empty());
    start
}

#[test]
fn palette_resize_preserves_field_focus_color_identity_and_separate_controls() {
    let (mut app, _fixture) = fixture(None);
    let before = document(&app);
    plain(&mut app, KeyCode::Enter);
    select(&mut app, FooterFieldId::Model);
    color(&mut app, Some(FooterColor::Cyan));
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    for (width, height, error) in
        [(100, 30, false), (60, 20, false), (30, 12, false), (30, 12, true), (20, 8, false)]
    {
        if error {
            app.config.set_overlay_error("Save locked");
        } else {
            app.config.overlay_message = None;
        }
        terminal.backend_mut().resize(width, height);
        terminal.resize(ratatui::layout::Rect::new(0, 0, width, height)).unwrap();
        terminal.draw(|frame| crate::ui::render_fullscreen_surface(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        let field = find_row(buffer, "Model");
        assert!(row(buffer, field).contains('›'));
        let selected = selected_swatch(buffer, FooterColor::Cyan);
        assert!(selected.1 > field);
        let label = find_row(buffer, "Cyan");
        assert!(label >= selected.1);
        let navigation = find_row(buffer, "Arrow");
        assert!(navigation > label);
        let confirm = find_row(buffer, "Enter");
        let cancel = find_row(buffer, "Esc");
        assert!(confirm > label && cancel >= confirm);
        if error {
            let cause = find_row(buffer, "Save locked");
            assert!(cause > label && cause != confirm && cause != cancel);
        }
        assert_eq!(document(&app), before);
        assert!(app.config.footer.pending.is_none());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn down_navigation_follows_rendered_swatch_neighbors_after_resize() {
    let work = async {
        let (mut app, _fixture) = fixture(None);
        plain(&mut app, KeyCode::Enter);
        select(&mut app, FooterFieldId::Model);
        color(&mut app, Some(FooterColor::Red));
        let mut expected = FooterColor::Red;
        for (width, height) in [(30, 30), (100, 30)] {
            let buffer = render(&mut app, width, height);
            let current = swatch(&buffer, expected).unwrap();
            expected = FooterColor::ALL
                .iter()
                .filter_map(|&c| swatch(&buffer, c).map(|p| (c, p)))
                .filter(|(_, p)| p.0 == current.0 && p.1 > current.1)
                .min_by_key(|(_, p)| p.1)
                .unwrap()
                .0;
            plain(&mut app, KeyCode::Down);
            let buffer = render(&mut app, width, height);
            find_row(&buffer, expected.label());
            selected_swatch(&buffer, expected);
            assert_eq!(editor(&app).selected_color(), Some(expected));
        }
        plain(&mut app, KeyCode::Enter);
        finish_saves(&mut app).await;
        let saved = document(&app);
        assert_eq!(
            saved_field(&saved, "model"),
            json!({"id":"model","visible":true,"side":"left","color":expected.label().to_lowercase()})
        );
        assert!(
            saved["presentation"]["footer"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|f| f["id"] != "model")
                .all(|f| f.get("color").is_none())
        );
        assert_eq!(active_color(&app, "test-model"), expected.color());
    };
    tokio::task::LocalSet::new().run_until(work).await;
}

#[tokio::test(flavor = "current_thread")]
async fn queued_edits_acknowledge_in_order_and_close_only_after_the_final_layout() {
    let work = async {
        let (mut app, _fixture) = fixture(Some(json!([
            {"id":"directory","visible":true,"side":"left"},
            {"id":"model","visible":true,"side":"left"},
            {"id":"gitBranch","visible":false,"side":"right"},
            {"id":"contextRemaining","visible":true,"side":"right"}
        ])));
        let original = document(&app);
        let active = footer(&app);
        plain(&mut app, KeyCode::Enter);
        select(&mut app, FooterFieldId::Model);
        plain(&mut app, KeyCode::Char(' '));
        plain(&mut app, KeyCode::Right);
        key(&mut app, KeyCode::Up, KeyModifiers::CONTROL);
        plain(&mut app, KeyCode::Esc);
        let first = next_save(&mut app).await;
        assert_eq!(footer(&app), active);
        assert!(app.config.overlay.is_some());
        let first_doc = document(&app);
        assert_eq!(
            saved_field(&first_doc, "model"),
            json!({"id":"model","visible":false,"side":"left"})
        );
        crate::app::handle_client_event(&mut app, first);
        assert!(app.config.footer.pending.is_some());
        assert!(!footer(&app).contains("test-model"));
        let final_save = next_save(&mut app).await;
        assert!(app.config.overlay.is_some(), "close waits for final acknowledgement");
        assert_eq!(
            app.config
                .footer
                .layout()
                .fields()
                .iter()
                .find(|f| f.id == FooterFieldId::Model)
                .unwrap()
                .side,
            FooterSide::Left
        );
        assert_eq!(editor(&app).layout().fields()[editor(&app).selected].id, FooterFieldId::Model);
        crate::app::handle_client_event(&mut app, final_save);
        assert!(app.config.footer.pending.is_none());
        assert!(app.config.overlay.is_none());
        let final_doc = document(&app);
        assert_eq!(
            saved_field(&final_doc, "model"),
            json!({"id":"model","visible":false,"side":"right"})
        );
        let entries = final_doc["presentation"]["footer"].as_array().unwrap();
        let position = |id| entries.iter().position(|e| e["id"] == id).unwrap();
        assert!(
            position("directory") < position("model")
                && position("model") < position("contextRemaining")
        );
        let mut siblings = final_doc;
        siblings["presentation"]["footer"] = original["presentation"]["footer"].clone();
        assert_eq!(siblings, original);
    };
    tokio::task::LocalSet::new().run_until(work).await;
}

#[tokio::test(flavor = "current_thread")]
async fn named_color_edits_cancel_save_reload_and_clear_for_model_and_dynamic_effort() {
    let work = async {
        for (id, name, choice) in [
            (FooterFieldId::Model, "model", FooterColor::Cyan),
            (FooterFieldId::Effort, "effort", FooterColor::Red),
        ] {
            let (mut app, mut fixture) = fixture(None);
            app.session_runtime.current_model = Some(
                crate::agent::model::CurrentModel::new("opus", "Opus", "Opus")
                    .supports_effort(true),
            );
            app.session_runtime.config_options.insert("effortLevel".into(), json!("low"));
            let original = document(&app);
            let normal_model = active_color(&app, "Opus");
            let normal_effort = active_color(&app, "[Low]");
            plain(&mut app, KeyCode::Enter);
            select(&mut app, FooterFieldId::PermissionMode);
            plain(&mut app, KeyCode::Char('c'));
            assert!(editor(&app).color_selection.is_none());
            select(&mut app, id);
            color(&mut app, Some(choice));
            let screen = render(&mut app, 100, 30);
            find_row(&screen, choice.label());
            assert!(app.config.footer.pending.is_none());
            assert_eq!(field(&app, id).color, None);
            plain(&mut app, KeyCode::Esc);
            assert_eq!(document(&app), original);
            color(&mut app, Some(choice));
            plain(&mut app, KeyCode::Enter);
            finish_saves(&mut app).await;
            let saved = document(&app);
            assert_eq!(saved_field(&saved, name)["color"], json!(choice.label().to_lowercase()));
            assert!(
                saved_field(&saved, if name == "model" { "effort" } else { "model" })
                    .get("color")
                    .is_none()
            );
            app.config.load_footer(app.global_settings_path.as_deref());
            for (level, label) in [("low", "[Low]"), ("high", "[High]")] {
                app.session_runtime.config_options.insert("effortLevel".into(), json!(level));
                assert_eq!(
                    active_color(&app, if name == "model" { "Opus" } else { label }),
                    choice.color()
                );
                assert_eq!(
                    active_color(&app, if name == "model" { label } else { "Opus" }),
                    if name == "model" { normal_effort } else { normal_model }
                );
            }
            color(&mut app, None);
            plain(&mut app, KeyCode::Enter);
            finish_saves(&mut app).await;
            assert!(saved_field(&document(&app), name).get("color").is_none());
            for (level, label) in [("low", "[Low]"), ("high", "[High]")] {
                app.session_runtime.config_options.insert("effortLevel".into(), json!(level));
                assert_eq!(active_color(&app, label), normal_effort);
                assert_eq!(active_color(&app, "Opus"), normal_model);
            }
            let mut siblings = document(&app);
            siblings["presentation"].as_object_mut().unwrap().remove("footer");
            assert_eq!(siblings, original);
            assert!(fixture.commands.try_recv().is_err());
        }
    };
    tokio::task::LocalSet::new().run_until(work).await;
}

#[tokio::test(flavor = "current_thread")]
async fn conflicts_require_explicit_retry_or_reopen_from_the_external_baseline() {
    let work = async {
        for retry in [false, true] {
            let (mut app, _fixture) =
                fixture(Some(json!([{"id":"model","visible":true,"side":"right","color":"cyan"}])));
            let active = footer(&app);
            plain(&mut app, KeyCode::Enter);
            select(&mut app, FooterFieldId::Model);
            plain(&mut app, KeyCode::Char(' '));
            let external =
                json!({"presentation":{"footer":[],"future":"external"},"updates":{"sentinel":42}});
            std::fs::write(app.global_settings_path.as_ref().unwrap(), external.to_string())
                .unwrap();
            finish_saves(&mut app).await;
            assert_eq!(document(&app), external);
            assert_eq!(footer(&app), active);
            assert!(app.config.overlay_message.is_some());
            assert!(!field(&app, FooterFieldId::Model).visible);
            assert!(app.config.footer.pending.is_none());
            if retry {
                plain(&mut app, KeyCode::Enter);
            } else {
                plain(&mut app, KeyCode::Esc);
                plain(&mut app, KeyCode::Enter);
                assert!(editor(&app).layout().fields().iter().all(|f| !f.visible));
                select(&mut app, FooterFieldId::Model);
                plain(&mut app, KeyCode::Char(' '));
            }
            finish_saves(&mut app).await;
            let saved = document(&app);
            let model = saved_field(&saved, "model");
            assert_eq!(model["visible"], !retry);
            if retry {
                assert_eq!(model["side"], "right");
                assert_eq!(model["color"], "cyan");
            } else {
                let visible: Vec<_> = saved["presentation"]["footer"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|f| f["visible"] == true)
                    .map(|f| f["id"].clone())
                    .collect();
                assert_eq!(visible, vec![json!("model")]);
            }
            let mut siblings = saved;
            siblings["presentation"]["footer"] = json!([]);
            assert_eq!(siblings, external);
            assert!(app.config.overlay_message.is_none());
        }
    };
    tokio::task::LocalSet::new().run_until(work).await;
}

#[tokio::test(flavor = "current_thread")]
async fn editing_during_row_reset_uses_requested_defaults_and_preserves_siblings() {
    let work = async {
        let (mut app, _fixture) =
            fixture(Some(json!([{"id":"model","visible":true,"side":"right","color":"cyan"}])));
        let original = document(&app);
        let active = footer(&app);
        plain(&mut app, KeyCode::Delete);
        plain(&mut app, KeyCode::Enter);
        assert_eq!(field(&app, FooterFieldId::Model).side, FooterSide::Left);
        assert_eq!(field(&app, FooterFieldId::Model).color, None);
        select(&mut app, FooterFieldId::PermissionMode);
        plain(&mut app, KeyCode::Char(' '));
        plain(&mut app, KeyCode::Esc);
        let reset = next_save(&mut app).await;
        assert!(document(&app)["presentation"].get("footer").is_none());
        assert_eq!(footer(&app), active);
        crate::app::handle_client_event(&mut app, reset);
        finish_saves(&mut app).await;
        let saved = document(&app);
        assert_eq!(
            saved_field(&saved, "model"),
            json!({"id":"model","visible":true,"side":"left"})
        );
        assert_eq!(saved_field(&saved, "permissionMode")["visible"], false);
        let mut siblings = saved;
        siblings["presentation"]["footer"] = original["presentation"]["footer"].clone();
        assert_eq!(siblings, original);
        assert!(app.config.overlay.is_none());
    };
    tokio::task::LocalSet::new().run_until(work).await;
}

#[test]
fn read_only_scopes_ignore_row_reset_and_all_editor_mutations() {
    for scope in [SettingsScope::Project, SettingsScope::Local] {
        let (mut app, mut fixture) = fixture(Some(
            json!([{"id":"model","visible":true,"side":"right","color":"cyan"},{"id":"directory","visible":false,"side":"left"}]),
        ));
        app.config.selected_scope = scope;
        let before = document(&app);
        let active = footer(&app);
        plain(&mut app, KeyCode::Delete);
        assert!(app.config.footer.pending.is_none());
        plain(&mut app, KeyCode::Enter);
        let draft = editor(&app).layout();
        select(&mut app, FooterFieldId::Model);
        for code in
            [KeyCode::Char(' '), KeyCode::Left, KeyCode::Right, KeyCode::Char('c'), KeyCode::Enter]
        {
            plain(&mut app, code);
        }
        for code in [KeyCode::Up, KeyCode::Down, KeyCode::Char('r')] {
            key(&mut app, code, KeyModifiers::CONTROL);
        }
        assert_eq!(editor(&app).layout(), draft);
        assert!(editor(&app).color_selection.is_none());
        assert!(app.config.footer.pending.is_none());
        plain(&mut app, KeyCode::Esc);
        assert!(app.config.overlay.is_none());
        assert_eq!(document(&app), before);
        assert_eq!(footer(&app), active);
        assert!(fixture.commands.try_recv().is_err());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn invalid_footer_can_be_repaired_by_reset_or_valid_edit_without_sdk_mutation() {
    let work = async {
        for reset in [false, true] {
            let (mut app, mut fixture) =
                fixture(Some(json!([{"id":"model","visible":true,"side":"invalid"}])));
            let before = document(&app);
            let buffer = render(&mut app, 120, 30);
            assert!(row(&buffer, find_row(&buffer, "Footer")).contains("Invalid layout"));
            plain(&mut app, KeyCode::Enter);
            assert!(app.config.overlay_message.is_some());
            if reset {
                key(&mut app, KeyCode::Char('r'), KeyModifiers::CONTROL);
            } else {
                select(&mut app, FooterFieldId::Model);
                color(&mut app, Some(FooterColor::Cyan));
                plain(&mut app, KeyCode::Enter);
            }
            finish_saves(&mut app).await;
            assert!(app.config.overlay_message.is_none());
            assert!(app.config.footer.error.is_none());
            let saved = document(&app);
            if reset {
                assert!(saved["presentation"].get("footer").is_none());
                let shown = footer_buffer(&app);
                assert!(row(&shown, 0).contains("test-model"));
                assert!(row(&shown, 0).contains("Loc:"));
            } else {
                assert_eq!(
                    saved_field(&saved, "model"),
                    json!({"id":"model","visible":true,"side":"left","color":"cyan"})
                );
                assert_eq!(active_color(&app, "test-model"), FooterColor::Cyan.color());
            }
            let mut siblings = saved;
            siblings["presentation"]["footer"] = before["presentation"]["footer"].clone();
            assert_eq!(siblings, before);
            plain(&mut app, KeyCode::Esc);
            app.config.load_footer(app.global_settings_path.as_deref());
            let buffer = render(&mut app, 120, 30);
            assert!(!row(&buffer, find_row(&buffer, "Footer")).contains("Invalid"));
            plain(&mut app, KeyCode::Enter);
            assert!(app.config.overlay_message.is_none());
            assert_eq!(
                field(&app, FooterFieldId::Model).color,
                if reset { None } else { Some(FooterColor::Cyan) }
            );
            assert!(fixture.commands.try_recv().is_err());
        }
    };
    tokio::task::LocalSet::new().run_until(work).await;
}

#[tokio::test(flavor = "current_thread")]
async fn startup_and_session_replacement_preserve_actual_footer_anchors_and_overrides() {
    use crate::agent::model;
    use clap::Parser;
    let work = async {
        let (_fixture_app, directory) = fixture(Some(json!([
            {"id":"directory","visible":true,"side":"left","color":"orange"},
            {"id":"model","visible":true,"side":"right","color":"cyan"}
        ])));
        let mut cli = crate::Cli::try_parse_from(["claude-rs", "--no-update-check"]).expect("CLI");
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
        assert_footer_alignment(&app);
        let (connection, _commands) = crate::agent::client::AgentConnection::test_channel();
        app.session_runtime.conn = Some(std::rc::Rc::new(connection));
        crate::app::handle_client_event(
            &mut app,
            ClientEvent::AuthRequired {
                method_name: "login".into(),
                method_description: "Login".into(),
            },
        );
        assert_footer_alignment(&app);
        crate::app::handle_client_event(
            &mut app,
            ClientEvent::Connected {
                session_id: "session-1".into(),
                cwd: directory.path().display().to_string(),
                current_model: model::CurrentModel::new("opus", "Opus", "Opus"),
                available_models: vec![],
                mode: None,
                fast_mode_state: model::FastModeState::On,
                fast_mode_disabled_reason: None,
                ultracode: None,
                history_updates: vec![],
            },
        );
        assert_footer_alignment(&app);
        crate::app::handle_client_event(
            &mut app,
            ClientEvent::SessionReplaced {
                session_id: "session-2".into(),
                cwd: directory.path().display().to_string(),
                current_model: model::CurrentModel::new("sonnet", "Sonnet", "Sonnet"),
                available_models: vec![],
                mode: None,
                fast_mode_state: model::FastModeState::On,
                fast_mode_disabled_reason: None,
                ultracode: None,
                history_updates: vec![],
                restored_input: None,
            },
        );
        assert_footer_alignment(&app);
        let screen = footer_buffer(&app);
        assert!(row(&screen, 0).contains("Sonnet"));
        assert!(!row(&screen, 0).contains("Opus"));
    };
    tokio::task::LocalSet::new().run_until(work).await;
}
