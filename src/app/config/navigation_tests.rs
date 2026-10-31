// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::{
    agent::{client::AgentConnection, settings::*, wire::BridgeCommand},
    app::{App, FullscreenView, SurfaceMode, handle_terminal_event},
};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use serde_json::{Value, json};
use std::rc::Rc;

#[path = "navigation_audit_tests.rs"]
mod audit;

fn app() -> App {
    let mut app = App::test_default();
    let mut fixture: Value =
        serde_json::from_str(include_str!("../../../tests/fixtures/settings-ui-catalog.json"))
            .expect("catalog fixture");
    fixture["cwd"] = json!(app.cwd_raw);
    fixture["context"] = json!("test-context");
    fixture["diagnostics"] = json!([]);
    fixture["resolution_sources"] = json!([]);
    fixture["provenance"] = json!({});
    let values = fixture["catalog"]
        .as_array()
        .expect("catalog")
        .iter()
        .map(|setting| json!({"id":setting["id"], "revision":"r1"}))
        .collect::<Vec<_>>();
    fixture["sources"] = json!([{"scope":"user", "path":"personal-settings.json", "status":"valid", "values":values}, {"scope":"project", "path":"project-settings.json", "status":"valid", "values":values}, {"scope":"local", "path":"local-settings.json", "status":"valid", "values":values}]);
    fixture["values"] = json!([]);
    app.config.snapshot = Some(serde_json::from_value(fixture).expect("snapshot"));
    app.config.settings.select("language".into());
    app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::Config);
    app
}
fn key(app: &mut App, code: KeyCode) {
    handle_terminal_event(app, Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
}
fn ctrl(app: &mut App, ch: char) {
    handle_terminal_event(app, Event::Key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::CONTROL)));
}
fn paste(app: &mut App, text: &str) {
    handle_terminal_event(app, Event::Paste(text.into()));
}
fn set_saved(app: &mut App, id: &str, value: Value) {
    let snapshot = app.config.snapshot.as_mut().expect("snapshot");
    snapshot.sources[0]
        .values
        .iter_mut()
        .find(|entry| entry.id == id)
        .expect("scoped value")
        .value = Some(value.clone());
    snapshot.values.push(SavedSetting {
        id: id.into(),
        value: Some(value),
        contributors: vec!["user".into()],
        policy_restricted: false,
    });
}
fn render_buffer(app: &mut App, width: u16, height: u16) -> ratatui::buffer::Buffer {
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height))
        .expect("terminal");
    terminal.draw(|frame| crate::ui::render_fullscreen_surface(frame, app)).expect("draw");
    terminal.backend().buffer().clone()
}
fn render(app: &mut App, width: u16, height: u16) -> String {
    render_buffer(app, width, height)
        .content
        .chunks(usize::from(width))
        .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}
fn field(app: &mut App, label: &str) {
    let overlay = app.config.setting_overlay().expect("editor");
    let form = overlay.structured.as_ref().expect("form");
    let root = serde_json::from_str(&overlay.draft).expect("draft");
    let index = form.rows(&root).iter().position(|row| row.label == label).expect("field");
    key(app, KeyCode::Home);
    for _ in 0..index {
        key(app, KeyCode::Down);
    }
    key(app, KeyCode::Enter);
}

#[tokio::test(flavor = "current_thread")]
async fn delayed_save_updates_saved_values_without_closing_another_editor() {
    let mut app = app();
    let (connection, mut commands) = AgentConnection::test_channel();
    app.session_runtime.conn = Some(Rc::new(connection));
    app.session_runtime.session_id = Some("session".into());
    key(&mut app, KeyCode::Char(' '));
    paste(&mut app, "German");
    key(&mut app, KeyCode::Enter);
    let save = commands.recv_envelope().await.expect("save");
    app.config.replace_overlay(ConfigOverlayState::SessionRename(SessionRenameOverlayState {
        draft: "another draft".into(),
        cursor: 13,
    }));
    let mut saved = app.config.snapshot.clone().expect("snapshot");
    saved.values.push(SavedSetting {
        id: "language".into(),
        value: Some(json!("German")),
        contributors: vec!["user".into()],
        policy_restricted: false,
    });
    apply_settings_result(
        &mut app,
        save.request_id.as_deref(),
        SettingsResult {
            persistence: SettingsPersistence::Saved,
            application: SettingsApplication::NextSession,
            snapshot: Some(saved),
            error: None,
        },
    );
    assert_eq!(app.config.saved_value("language"), Some(&json!("German")));
    assert_eq!(app.config.session_rename_overlay().expect("other editor").draft, "another draft");
    assert_eq!(app.config.selected_setting().expect("selection").id, "language");
}

fn choose(app: &mut App, value: &str) {
    let form = app.config.setting_overlay().expect("editor").structured.as_ref().expect("form");
    let input = form
        .hook_creation
        .as_ref()
        .and_then(|wizard| wizard.input.as_ref())
        .or(form.input.as_ref())
        .expect("choice input");
    let options = input.options();
    let index =
        options.iter().position(|option| option.as_str() == Some(value)).expect("offered choice");
    key(app, KeyCode::Home);
    for _ in 0..index {
        key(app, KeyCode::Down);
    }
    key(app, KeyCode::Enter);
}

#[tokio::test(flavor = "current_thread")]
async fn hook_failure_policy_choices_and_reset_save_only_the_selected_handler() {
    for kind in ["command", "http"] {
        for policy in [Some("block"), Some("continue"), None] {
            let mut app = app();
            let (connection, mut commands) = AgentConnection::test_channel();
            app.session_runtime.conn = Some(Rc::new(connection));
            app.session_runtime.session_id = Some("session".into());
            let handler = if kind == "command" {
                json!({"type":"command","command":"never-execute","onFailure":"block","future":42})
            } else {
                json!({"type":"http","url":"https://example.com/hook","onFailure":"block","future":42})
            };
            let mut expected = json!({"PreToolUse":[{"matcher":"Write","hooks":[handler,{"type":"prompt","prompt":"Keep this"}]}]});
            set_saved(&mut app, "hooks", expected.clone());
            app.config.settings.open_category("hooks");
            app.config.settings.select("hooks".into());
            key(&mut app, KeyCode::Enter);
            key(&mut app, KeyCode::Enter);
            field(&mut app, "Failure policy");
            if let Some(policy) = policy {
                choose(&mut app, policy);
                expected["PreToolUse"][0]["hooks"][0]["onFailure"] = json!(policy);
            } else {
                key(&mut app, KeyCode::Esc);
                key(&mut app, KeyCode::Delete);
                expected["PreToolUse"][0]["hooks"][0]
                    .as_object_mut()
                    .expect("handler")
                    .remove("onFailure");
            }
            ctrl(&mut app, 's');
            let save = commands.recv_envelope().await.expect("save");
            let BridgeCommand::MutateSetting { mutation, .. } = save.command else {
                panic!("mutation");
            };
            assert_eq!(mutation.id, "hooks");
            assert_eq!(mutation.scope, SettingsScope::User);
            assert_eq!(mutation.value, Some(expected));
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn guided_hook_creation_offers_every_action_and_saves_complete_hooks_without_changing_siblings()
 {
    let mut app = app();
    let (connection, mut commands) = AgentConnection::test_channel();
    app.session_runtime.conn = Some(Rc::new(connection));
    app.session_runtime.session_id = Some("session".into());
    let original =
        json!({"Stop":[{"hooks":[{"type":"command","command":"keep-original","future":"keep"}]}]});
    set_saved(&mut app, "hooks", original.clone());
    app.config.settings.open_category("hooks");
    app.config.settings.select("hooks".into());
    key(&mut app, KeyCode::Enter);
    let mut expected = original;
    for (kind, values) in [
        ("command", vec![("command", "npm run lint")]),
        ("prompt", vec![("prompt", "Check completion")]),
        ("agent", vec![("prompt", "Verify the files")]),
        ("http", vec![("url", "https://example.com/hook")]),
        ("mcp_tool", vec![("server", "configured-server"), ("tool", "inspect")]),
    ] {
        key(&mut app, KeyCode::Char('a'));
        let text = render(&mut app, 100, 35);
        assert!(text.contains("Choose event"), "{text}");
        assert!(text.contains("PreToolUse"), "{text}");
        choose(&mut app, "PreToolUse");
        assert!(render(&mut app, 100, 35).contains("Matcher (optional)"));
        paste(&mut app, "Bash");
        handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(
                KeyCode::Char('|'),
                KeyModifiers::CONTROL | KeyModifiers::ALT,
            )),
        );
        paste(&mut app, "Write");
        key(&mut app, KeyCode::Enter);
        let text = render(&mut app, 100, 35);
        for option in ["command", "prompt", "agent", "http", "mcp_tool"] {
            assert!(text.contains(option), "{text}");
        }
        choose(&mut app, kind);
        let mut handler = json!({"type":kind});
        for (name, value) in values {
            // Ctrl+S cannot persist an unfinished creation step.
            ctrl(&mut app, 's');
            assert!(commands.try_recv().is_err());
            paste(&mut app, value);
            key(&mut app, KeyCode::Enter);
            handler[name] = json!(value);
        }
        assert!(render(&mut app, 100, 35).contains("Review"));
        assert_eq!(
            serde_json::from_str::<Value>(&app.config.setting_overlay().expect("draft").draft)
                .expect("JSON"),
            expected
        );
        key(&mut app, KeyCode::Enter);
        if expected.get("PreToolUse").is_none() {
            expected["PreToolUse"] = json!([]);
        }
        expected["PreToolUse"]
            .as_array_mut()
            .expect("groups")
            .push(json!({"matcher":"Bash|Write","hooks":[handler]}));
        assert_eq!(
            serde_json::from_str::<Value>(&app.config.setting_overlay().expect("draft").draft)
                .expect("JSON"),
            expected
        );
        key(&mut app, KeyCode::Esc);
        assert!(
            app.config
                .setting_overlay()
                .expect("list")
                .structured
                .as_ref()
                .expect("form")
                .path
                .is_empty()
        );
    }
    ctrl(&mut app, 's');
    let save = commands.recv_envelope().await.expect("save");
    let BridgeCommand::MutateSetting { mutation, .. } = save.command else {
        panic!("mutation");
    };
    assert_eq!(mutation.id, "hooks");
    assert_eq!(mutation.scope, SettingsScope::User);
    assert_eq!(mutation.value, Some(expected));
}

#[test]
fn canceling_hook_creation_keeps_the_draft_and_nested_add_edits_arguments() {
    let mut app = app();
    let original = json!({"PreToolUse":[{"matcher":"Bash", "hooks":[{"type":"command","command":"rg","args":["--hidden"]}]}]});
    set_saved(&mut app, "hooks", original.clone());
    app.config.settings.open_category("hooks");
    app.config.settings.select("hooks".into());
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Char('a'));
    choose(&mut app, "PreToolUse");
    paste(&mut app, "Write");
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Esc);
    assert_eq!(
        serde_json::from_str::<Value>(&app.config.setting_overlay().expect("editor").draft)
            .expect("JSON"),
        original
    );
    key(&mut app, KeyCode::Enter);
    field(&mut app, "Arguments");
    key(&mut app, KeyCode::Char('a'));
    assert!(
        app.config
            .setting_overlay()
            .expect("editor")
            .structured
            .as_ref()
            .expect("form")
            .hook_creation
            .is_none()
    );
    paste(&mut app, "--files");
    key(&mut app, KeyCode::Enter);
    let mut expected = original;
    expected["PreToolUse"][0]["hooks"][0]["args"] = json!(["--hidden", "--files"]);
    assert_eq!(
        serde_json::from_str::<Value>(&app.config.setting_overlay().expect("editor").draft)
            .expect("JSON"),
        expected
    );
}
