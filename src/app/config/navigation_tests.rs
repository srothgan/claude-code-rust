// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::{
    agent::{client::AgentConnection, settings::*, wire::BridgeCommand},
    app::{App, FullscreenView, SurfaceMode, handle_terminal_event},
};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use serde_json::{Value, json};
use std::rc::Rc;

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
fn settings_regions(text: &str, label: &str) -> (String, String) {
    let lines = text.lines().collect::<Vec<_>>();
    let description =
        lines.iter().position(|line| line.contains("Description:")).expect("labeled description");
    let row = lines[..description].iter().find(|line| line.contains(label)).expect("setting row");
    ((*row).to_owned(), lines[description..].join("\n"))
}

#[test]
fn settings_metadata_follows_the_selected_scope_without_repeating_the_list_value() {
    let mut app = app();
    set_saved(&mut app, "language", json!("German"));
    let text = render(&mut app, 100, 32);
    let (row, details) = settings_regions(&text, "Language");
    assert!(row.contains("German"), "value must be associated with Language: {row}");
    assert!(!details.contains("German"), "matching saved values must not be repeated: {details}");

    key(&mut app, KeyCode::Char('s'));
    let text = render(&mut app, 100, 32);
    let (row, details) = settings_regions(&text, "Language");
    assert_eq!(app.config.selected_scope, SettingsScope::Project);
    assert!(row.contains("German"));
    assert!(
        details.contains("User") && details.contains("Project"),
        "source and target scope: {details}"
    );
    assert!(!details.contains("German"));

    let snapshot = app.config.snapshot.as_mut().expect("snapshot");
    snapshot.sources[1]
        .values
        .iter_mut()
        .find(|value| value.id == "language")
        .expect("project value")
        .value = Some(json!("English"));
    let resolved = snapshot.values.iter_mut().find(|value| value.id == "language").expect("value");
    resolved.value = Some(json!("English"));
    resolved.contributors.push("project".into());
    key(&mut app, KeyCode::Char('s'));
    key(&mut app, KeyCode::Char('s'));
    let text = render(&mut app, 100, 32);
    let (row, details) = settings_regions(&text, "Language");
    assert!(row.contains("English"));
    assert!(
        details.contains("German") && details.contains("Project"),
        "show overridden saved value and its overriding source: {details}"
    );
    assert!(!details.contains("English"), "effective value belongs in its row: {details}");
}

#[test]
fn settings_metadata_shows_defaults_immediate_timing_and_actual_session_differences() {
    let mut app = app();
    let text = render(&mut app, 100, 32);
    let (row, _) = settings_regions(&text, "Language");
    assert!(row.contains("Default"));
    set_saved(&mut app, "spinnerTipsEnabled", json!(true));
    app.config.settings.select("spinnerTipsEnabled".into());
    let text = render(&mut app, 100, 32);
    let (row, details) = settings_regions(&text, "Show tips");
    assert!(row.contains("On"));
    assert!(details.contains("immediately"), "host preferences apply immediately: {details}");

    set_saved(&mut app, "model", json!("sonnet"));
    app.config.settings.select("model".into());
    app.session_runtime.current_model = Some(
        crate::agent::model::CurrentModel::new("claude-sonnet", "Sonnet", "Claude Sonnet")
            .catalog_id("sonnet")
            .authoritative(true),
    );
    let text = render(&mut app, 100, 32);
    let (_, details) = settings_regions(&text, "Default model");
    assert!(
        !details.contains("Sonnet"),
        "matching active model needs no duplicate value: {details}"
    );
    app.session_runtime.current_model = Some(
        crate::agent::model::CurrentModel::new("claude-opus", "Opus", "Claude Opus")
            .catalog_id("opus")
            .requested_id("sonnet")
            .authoritative(true),
    );
    let text = render(&mut app, 100, 32);
    let (row, details) = settings_regions(&text, "Default model");
    assert!(row.contains("sonnet"), "saved default must remain in its row: {row}");
    assert!(details.contains("Opus"), "show the differing acknowledged model: {details}");
}

#[test]
fn settings_metadata_distinguishes_collection_contributions_and_retains_wrapped_warnings() {
    let mut app = app();
    set_saved(&mut app, "permissions.allow", json!(["Read(./src/**)"]));
    app.config.settings.open_category("permissions");
    app.config.settings.select("permissions.allow".into());
    let snapshot = app.config.snapshot.as_mut().expect("snapshot");
    let value = &mut snapshot.values[0];
    value.value = Some(json!(["Read(./src/**)", "Read(./docs/**)"]));
    value.contributors.push("project".into());
    let text = render(&mut app, 100, 32);
    let (row, details) = settings_regions(&text, "Allow rules");
    assert!(row.contains("1 item"), "the row shows this scope's collection: {row}");
    assert!(
        details.contains("User") && details.contains("Project"),
        "show both contributors: {details}"
    );

    let snapshot = app.config.snapshot.as_mut().expect("snapshot");
    snapshot.values[0].policy_restricted = true;
    snapshot
        .catalog
        .iter_mut()
        .find(|setting| setting.id == "permissions.allow")
        .expect("setting")
        .writable_scopes
        .clear();
    let error =
        "SOURCE_ERROR_START: this scoped file requires correction before loading. SOURCE_ERROR_END";
    snapshot.sources[0].error = Some(error.into());
    let buffer = render_buffer(&mut app, 40, 24);
    let warnings = buffer
        .content
        .chunks(40)
        .map(|row| {
            row.iter()
                .filter(|cell| cell.fg == crate::ui::theme::STATUS_WARNING)
                .map(ratatui::buffer::Cell::symbol)
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join(" ");
    let words = warnings.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        words.contains(error),
        "the complete source error must remain visible as a warning: {warnings}"
    );
    assert!(
        words.contains("organization"),
        "policy restrictions must remain visible alongside source errors: {warnings}"
    );
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

#[test]
fn unsupported_structured_values_remain_available_in_advanced_json() {
    let mut app = app();
    set_saved(&mut app, "hooks", json!("externally-written-invalid-value"));
    app.config.settings.open_category("hooks");
    app.config.settings.select("hooks".into());
    key(&mut app, KeyCode::Enter);
    assert!(render(&mut app, 100, 35).contains("This value needs advanced JSON"));
    ctrl(&mut app, 'j');
    assert_eq!(
        serde_json::from_str::<Value>(&app.config.setting_overlay().expect("editor").draft)
            .expect("JSON"),
        json!("externally-written-invalid-value")
    );
    key(&mut app, KeyCode::Esc);
    assert_eq!(app.config.saved_value("hooks"), Some(&json!("externally-written-invalid-value")));
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

#[tokio::test(flavor = "current_thread")]
async fn pane_search_focus_and_empty_results_never_target_another_pane() {
    let mut app = app();
    let (connection, mut commands) = AgentConnection::test_channel();
    app.session_runtime.conn = Some(Rc::new(connection));
    app.session_runtime.session_id = Some("session".into());
    key(&mut app, KeyCode::Char('/'));
    paste(&mut app, "sandbox");
    assert!(app.config.selected_setting().is_none());
    assert!(render(&mut app, 80, 24).contains("No matching settings in this pane"));
    for code in [KeyCode::Char(' '), KeyCode::Delete, KeyCode::Down, KeyCode::Enter] {
        key(&mut app, code);
    }
    assert_eq!(app.config.settings.focus, SettingsFocus::Search);
    assert!(commands.try_recv().is_err());
    key(&mut app, KeyCode::Esc);
    assert_eq!(app.config.selected_setting().expect("restored").id, "language");
    key(&mut app, KeyCode::Home);
    key(&mut app, KeyCode::Up);
    key(&mut app, KeyCode::Up);
    assert_eq!(app.config.settings.focus, SettingsFocus::CategoryTabs);
    let scope = app.config.selected_scope;
    for _ in 0..3 {
        key(&mut app, KeyCode::Right);
    }
    assert_eq!(app.config.settings.category, "sandbox");
    assert_eq!(app.config.selected_scope, scope);
    key(&mut app, KeyCode::Down);
    paste(&mut app, "RIPGREP command");
    key(&mut app, KeyCode::Down);
    assert_eq!(app.config.selected_setting().expect("match").id, "sandbox.ripgrep");
    key(&mut app, KeyCode::Enter);
    assert!(app.config.settings.query.is_empty());
    assert!(app.config.overlay.is_none());
    assert_eq!(app.config.selected_setting().expect("revealed").id, "sandbox.ripgrep");
    for (width, height) in [(120, 40), (42, 16), (30, 12)] {
        let text = render(&mut app, width, height);
        assert!(text.contains("Sandbox"), "{text}");
    }
    assert!(commands.try_recv().is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn permission_item_draft_survives_conflict_and_saves_only_its_captured_scope() {
    let mut app = app();
    let (connection, mut commands) = AgentConnection::test_channel();
    app.session_runtime.conn = Some(Rc::new(connection));
    app.session_runtime.session_id = Some("session".into());
    set_saved(&mut app, "permissions.allow", json!(["Read(./src/**)"]));
    set_saved(&mut app, "permissions.deny", json!(["Read(./.env)"]));
    app.config.settings.open_category("permissions");
    app.config.settings.select("permissions.allow".into());
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Char('a'));
    paste(&mut app, "Bash(npm test *)");
    key(&mut app, KeyCode::Enter);
    for (width, height) in [(100, 35), (42, 16), (28, 10), (87, 25)] {
        render(&mut app, width, height);
    }
    key(&mut app, KeyCode::Tab);
    assert_eq!(app.config.active_tab, ConfigTab::Settings);
    ctrl(&mut app, 's');
    let first = commands.recv_envelope().await.expect("save");
    let BridgeCommand::MutateSetting { mutation, .. } = first.command else {
        panic!("mutation");
    };
    assert_eq!(mutation.scope, SettingsScope::User);
    assert_eq!(mutation.id, "permissions.allow");
    assert_eq!(mutation.value, Some(json!(["Read(./src/**)", "Bash(npm test *)"])));
    let mut latest = app.config.snapshot.clone().expect("snapshot");
    latest.sources[0]
        .values
        .iter_mut()
        .find(|entry| entry.id == "permissions.allow")
        .expect("entry")
        .revision = "r2".into();
    apply_settings_result(
        &mut app,
        first.request_id.as_deref(),
        SettingsResult {
            persistence: SettingsPersistence::Conflict,
            application: SettingsApplication::Blocked,
            snapshot: Some(latest),
            error: Some("Review external edits before retrying.".into()),
        },
    );
    assert!(app.config.setting_overlay().expect("retained").draft.contains("Bash(npm test *)"));
    ctrl(&mut app, 's');
    let retry = commands.recv_envelope().await.expect("retry");
    let BridgeCommand::MutateSetting { mutation, .. } = retry.command else {
        panic!("mutation");
    };
    assert_eq!(mutation.expected_revision, "r2");
    assert_eq!(app.config.saved_value("permissions.deny"), Some(&json!(["Read(./.env)"])));
}

#[tokio::test(flavor = "current_thread")]
async fn hook_form_and_json_share_one_draft_and_preserve_sibling_handlers_and_unknown_fields() {
    let mut app = app();
    let (connection, mut commands) = AgentConnection::test_channel();
    app.session_runtime.conn = Some(Rc::new(connection));
    app.session_runtime.session_id = Some("session".into());
    let original = json!({"PreToolUse":[{"matcher":"Write|Edit", "future":"preserve", "hooks":[{"type":"command","command":"old-command","args":["--keep"],"future":"keep"},{"type":"http","url":"https://example.com"}]}],"Stop":[{"hooks":[{"type":"prompt","prompt":"Check completion"}]}]});
    set_saved(&mut app, "hooks", original.clone());
    app.config.settings.open_category("hooks");
    app.config.settings.select("hooks".into());
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Enter);
    field(&mut app, "Command");
    ctrl(&mut app, 'u');
    paste(&mut app, "new-command\r\nsecond line ä");
    key(&mut app, KeyCode::Enter);
    ctrl(&mut app, 'j');
    assert!(
        app.config.setting_overlay().expect("editor").structured.as_ref().expect("form").advanced
    );
    ctrl(&mut app, 'f');
    ctrl(&mut app, 's');
    let save = commands.recv_envelope().await.expect("save");
    let BridgeCommand::MutateSetting { mutation, .. } = save.command else {
        panic!("mutation");
    };
    let mut expected = original;
    expected["PreToolUse"][0]["hooks"][0]["command"] = json!("new-command\nsecond line ä");
    assert_eq!(mutation.value, Some(expected));
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

#[tokio::test(flavor = "current_thread")]
async fn permission_rule_edit_cancel_and_remove_save_only_the_selected_collection() {
    let mut app = app();
    let (connection, mut commands) = AgentConnection::test_channel();
    app.session_runtime.conn = Some(Rc::new(connection));
    app.session_runtime.session_id = Some("session".into());
    set_saved(&mut app, "permissions.allow", json!(["Read(./docs/**)", "Bash(npm test *)"]));
    set_saved(&mut app, "permissions.deny", json!(["Read(./.env)"]));
    app.config.settings.open_category("permissions");
    app.config.settings.select("permissions.allow".into());
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Enter);
    let text = render(&mut app, 100, 35);
    assert!(text.contains("Read(./src/**)"), "{text}");
    ctrl(&mut app, 'u');
    paste(&mut app, "Read(./src/**)");
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Enter);
    ctrl(&mut app, 'u');
    paste(&mut app, "Read(./discarded/**)");
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Delete);
    ctrl(&mut app, 's');
    let save = commands.recv_envelope().await.expect("save");
    let BridgeCommand::MutateSetting { mutation, .. } = save.command else {
        panic!("mutation");
    };
    assert_eq!(mutation.id, "permissions.allow");
    assert_eq!(mutation.value, Some(json!(["Read(./src/**)"])));
    assert_eq!(app.config.saved_value("permissions.deny"), Some(&json!(["Read(./.env)"])));
}

#[tokio::test(flavor = "current_thread")]
async fn sandbox_credential_creation_shows_required_fields_and_choices_then_saves_the_selected_mode()
 {
    let mut app = app();
    let (connection, mut commands) = AgentConnection::test_channel();
    app.session_runtime.conn = Some(Rc::new(connection));
    app.session_runtime.session_id = Some("session".into());
    app.config.settings.open_category("sandbox");
    app.config.settings.select("sandbox.credentials".into());
    key(&mut app, KeyCode::Enter);
    field(&mut app, "Environment variables");
    assert!(render(&mut app, 100, 35).contains("Press a to add an entry"));
    key(&mut app, KeyCode::Char('a'));
    let text = render(&mut app, 100, 35);
    assert!(text.contains("Variable name *"), "{text}");
    assert!(text.contains("Protection mode *"), "{text}");
    assert!(text.contains("* Required"), "{text}");
    field(&mut app, "Variable name");
    let text = render(&mut app, 100, 35);
    assert!(text.contains("its secret value."), "{text}");
    paste(&mut app, "API_TOKEN");
    key(&mut app, KeyCode::Enter);
    field(&mut app, "Protection mode");
    let text = render(&mut app, 100, 35);
    assert!(text.contains("deny"));
    assert!(text.contains("mask"));
    choose(&mut app, "mask");
    ctrl(&mut app, 's');
    let save = commands.recv_envelope().await.expect("save");
    let BridgeCommand::MutateSetting { mutation, .. } = save.command else {
        panic!("mutation");
    };
    assert_eq!(mutation.value, Some(json!({"envVars":[{"name":"API_TOKEN","mode":"mask"}]})));
}

#[tokio::test(flavor = "current_thread")]
async fn sandbox_picker_can_replace_an_external_unlisted_value_with_its_first_supported_choice() {
    let mut app = app();
    let (connection, mut commands) = AgentConnection::test_channel();
    app.session_runtime.conn = Some(Rc::new(connection));
    app.session_runtime.session_id = Some("session".into());
    set_saved(
        &mut app,
        "sandbox.credentials",
        json!({"envVars":[{"name":"API_TOKEN","mode":"external-value"}]}),
    );
    app.config.settings.open_category("sandbox");
    app.config.settings.select("sandbox.credentials".into());
    key(&mut app, KeyCode::Enter);
    field(&mut app, "Environment variables");
    key(&mut app, KeyCode::Enter);
    field(&mut app, "Protection mode");
    assert!(render(&mut app, 100, 35).contains("Current: external-value"));
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    ctrl(&mut app, 's');
    let save = commands.recv_envelope().await.expect("save");
    let BridgeCommand::MutateSetting { mutation, .. } = save.command else {
        panic!("mutation");
    };
    assert_eq!(mutation.value, Some(json!({"envVars":[{"name":"API_TOKEN","mode":"deny"}]})));
}

#[tokio::test(flavor = "current_thread")]
async fn sandbox_form_preserves_unknown_fields_and_a_stale_project_cannot_save() {
    let mut app = app();
    let (connection, mut commands) = AgentConnection::test_channel();
    app.session_runtime.conn = Some(Rc::new(connection));
    app.session_runtime.session_id = Some("session".into());
    set_saved(
        &mut app,
        "sandbox.ripgrep",
        json!({"command":"rg","args":["--hidden"],"future":{"keep":true}}),
    );
    app.config.settings.open_category("sandbox");
    app.config.settings.select("sandbox.ripgrep".into());
    key(&mut app, KeyCode::Enter);
    field(&mut app, "Command");
    ctrl(&mut app, 'u');
    paste(&mut app, "custom-rg");
    key(&mut app, KeyCode::Enter);
    let value: Value =
        serde_json::from_str(&app.config.setting_overlay().expect("editor").draft).expect("JSON");
    assert_eq!(value, json!({"command":"custom-rg","args":["--hidden"],"future":{"keep":true}}));
    app.bump_session_scope_epoch();
    app.cwd_raw = "another-project".into();
    assert_eq!(
        serde_json::from_str::<Value>(
            &app.config.setting_overlay().expect("stale draft retained").draft
        )
        .expect("JSON"),
        value
    );
    assert!(render(&mut app, 100, 35).contains("Settings location changed"));
    ctrl(&mut app, 's');
    assert!(commands.try_recv().is_err());
    assert!(
        app.config.overlay_message.as_ref().expect("message").text.contains("location changed")
    );
    assert!(app.config.setting_overlay().expect("retained").draft.contains("custom-rg"));
}

#[test]
fn pane_positions_search_and_refresh_keep_setting_identity_and_scope() {
    let mut app = app();
    app.config.settings.open_category("sandbox");
    app.config.settings.select("sandbox.ripgrep".into());
    key(&mut app, KeyCode::Char('/'));
    paste(&mut app, "ripgrep");
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::BackTab);
    assert_eq!(app.config.active_tab, ConfigTab::Help);
    key(&mut app, KeyCode::Tab);
    assert_eq!(app.config.active_tab, ConfigTab::Settings);
    assert_eq!(app.config.settings.query, "ripgrep");
    assert_eq!(app.config.settings.focus, SettingsFocus::Content);
    app.config.pending_settings_request =
        Some(PendingSettingsRequest::Inspection("refresh".into()));
    let mut refreshed = app.config.snapshot.clone().expect("snapshot");
    refreshed.catalog.reverse();
    apply_settings_result(
        &mut app,
        Some("refresh"),
        SettingsResult {
            persistence: SettingsPersistence::NotRequested,
            application: SettingsApplication::Blocked,
            snapshot: Some(refreshed),
            error: None,
        },
    );
    assert_eq!(app.config.selected_setting().expect("same target").id, "sandbox.ripgrep");
    key(&mut app, KeyCode::Esc);
    assert_eq!(app.config.selected_setting().expect("restored target").id, "sandbox.ripgrep");
    key(&mut app, KeyCode::Home);
    key(&mut app, KeyCode::Up);
    key(&mut app, KeyCode::Up);
    for _ in 0..3 {
        key(&mut app, KeyCode::Left);
    }
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.config.selected_setting().expect("remembered General").id, "language");
    assert_eq!(app.config.selected_scope, SettingsScope::User);
}

#[test]
fn deleting_last_list_entry_keeps_an_explicit_empty_draft_and_read_only_forms_are_inspectable() {
    let mut app = app();
    set_saved(&mut app, "permissions.allow", json!(["Read(./src/**)"]));
    app.config.settings.open_category("permissions");
    app.config.settings.select("permissions.allow".into());
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Delete);
    assert_eq!(
        serde_json::from_str::<Value>(&app.config.setting_overlay().expect("editor").draft)
            .expect("JSON"),
        json!([])
    );
    key(&mut app, KeyCode::Esc);
    assert_eq!(app.config.saved_value("permissions.allow"), Some(&json!(["Read(./src/**)"])));
    let snapshot = app.config.snapshot.as_mut().expect("snapshot");
    let descriptor = snapshot
        .catalog
        .iter_mut()
        .find(|setting| setting.id == "permissions.allow")
        .expect("descriptor");
    descriptor.writable_scopes.clear();
    descriptor.unavailable = Some("Managed policy".into());
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Delete);
    ctrl(&mut app, 's');
    let editor = app.config.setting_overlay().expect("inspection stays open");
    assert_eq!(
        serde_json::from_str::<Value>(&editor.draft).expect("JSON"),
        json!(["Read(./src/**)"])
    );
    assert!(editor.structured.as_ref().expect("form").read_only);
    assert!(render(&mut app, 100, 35).contains("Read-only in this source"));
    ctrl(&mut app, 'j');
    paste(&mut app, "cannot edit a read-only source");
    key(&mut app, KeyCode::Home);
    assert_eq!(
        serde_json::from_str::<Value>(&app.config.setting_overlay().expect("inspection").draft)
            .expect("JSON"),
        json!(["Read(./src/**)"])
    );
    assert!(render(&mut app, 100, 35).contains("Read-only"));
}
