// SPDX-License-Identifier: Apache-2.0
use crate::agent::{
    client::{AgentConnection, CommandReceiver},
    wire::CommandEnvelope,
};
pub(super) use crate::agent::{events::ClientEvent, settings::*, wire::BridgeCommand};
pub(super) use crate::app::App;
pub(super) use crossterm::event::KeyCode;
use crossterm::event::{Event, KeyEvent, KeyModifiers};
pub(super) use serde_json::{Value, json};
use std::{rc::Rc, time::Duration};

pub(super) struct Harness {
    pub app: App,
    pub commands: CommandReceiver,
    pub scratch: tempfile::TempDir,
    pub profile: String,
}

pub(super) fn fixture() -> Harness {
    let scratch = tempfile::tempdir().unwrap();
    let profile = scratch.path().join("profile.json").to_string_lossy().into_owned();
    let mut app = App::test_default();
    app.cwd_raw = scratch.path().to_string_lossy().into_owned();
    app.global_settings_path = Some(profile.clone().into());
    app.session_runtime.session_id = Some("settings-session".into());
    app.surface_mode = crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
    let mut catalog: Value =
        serde_json::from_str(include_str!("../../../tests/fixtures/settings-ui-catalog.json"))
            .unwrap();
    catalog["catalog"].as_array_mut().unwrap().extend([
        json!({"id":"updates.autoInstall","label":"Automatic updates","description":"Install automatically after exit. Default: Off.","key_path":["updates","autoInstall"],"category":"general","kind":"boolean","options":[true,false],"allows_custom":false,"writable_scopes":["user"],"reset":"Remove the saved preference","application":"host"}),
        json!({"id":"defaultEffort","label":"Default effort","description":"Effort for new sessions","key_path":["modelSettings","claude-sonnet-4-6","effortLevel"],"category":"general","kind":"string","options":["low","medium","high"],"allows_custom":false,"writable_scopes":["user","project","local"],"reset":"Remove the saved preference","application":"next_session"})
    ]);
    catalog["cwd"] = json!(app.cwd_raw);
    catalog["context"] = json!("settings-context");
    catalog["diagnostics"] = json!([]);
    catalog["resolution_sources"] = json!([]);
    catalog["provenance"] = json!({});
    catalog["values"] = json!([]);
    catalog["sources"] = json!(["user","project","local"].map(|scope| json!({
        "scope":scope,"path":format!("{scope}.json"),"status":"valid",
        "values":catalog["catalog"].as_array().unwrap().iter().map(|setting| json!({"id":setting["id"],"revision":format!("{scope}-r1")})).collect::<Vec<_>>()
    })));
    app.config.snapshot = Some(serde_json::from_value(catalog).unwrap());
    app.config.settings.select("language".into());
    let (connection, commands) = AgentConnection::test_channel();
    app.session_runtime.conn = Some(Rc::new(connection));
    Harness { app, commands, scratch, profile }
}

pub(super) fn press(app: &mut App, code: KeyCode) {
    crate::app::handle_terminal_event(app, Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
}
pub(super) fn control(app: &mut App, ch: char) {
    crate::app::handle_terminal_event(
        app,
        Event::Key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::CONTROL)),
    );
}
pub(super) fn paste(app: &mut App, text: &str) {
    crate::app::handle_terminal_event(app, Event::Paste(text.into()));
}
pub(super) fn replace_input(app: &mut App, text: &str) {
    if app.config.setting_overlay().is_some_and(|editor| editor.structured.is_none()) {
        press(app, KeyCode::End);
        for _ in 0..app.config.setting_overlay().unwrap().draft.chars().count() {
            press(app, KeyCode::Backspace);
        }
    } else {
        control(app, 'u');
    }
    paste(app, text);
}
pub(super) fn select(app: &mut App, id: &str, scope: SettingsScope) {
    let category = app
        .config
        .snapshot
        .as_ref()
        .unwrap()
        .catalog
        .iter()
        .find(|s| s.id == id)
        .unwrap()
        .category
        .clone();
    app.config.settings.open_category(&category);
    app.config.settings.select(id.into());
    app.config.selected_scope = scope;
}
pub(super) fn put(
    snapshot: &mut SettingsSnapshot,
    scope: SettingsScope,
    id: &str,
    value: Option<Value>,
    revision: &str,
) {
    let entry = snapshot
        .sources
        .iter_mut()
        .find(|source| source.scope == scope)
        .unwrap()
        .values
        .iter_mut()
        .find(|v| v.id == id)
        .unwrap();
    entry.value = value;
    entry.revision = revision.into();
}
pub(super) fn effective(
    snapshot: &mut SettingsSnapshot,
    id: &str,
    value: Option<Value>,
    contributors: &[&str],
) {
    snapshot.values.retain(|v| v.id != id);
    snapshot.values.push(SavedSetting {
        id: id.into(),
        value,
        contributors: contributors.iter().map(|s| (*s).into()).collect(),
        policy_restricted: false,
    });
}
pub(super) fn seed(app: &mut App, scope: SettingsScope, id: &str, value: Value) {
    let snapshot = app.config.snapshot.as_mut().unwrap();
    let (name, revision) = match scope {
        SettingsScope::User => ("user", "user-r1"),
        SettingsScope::Project => ("project", "project-r1"),
        SettingsScope::Local => ("local", "local-r1"),
    };
    put(snapshot, scope, id, Some(value.clone()), revision);
    effective(snapshot, id, Some(value), &[name]);
}
pub(super) fn draft(app: &App) -> Value {
    serde_json::from_str(&app.config.setting_overlay().unwrap().draft).unwrap()
}
pub(super) async fn receive(h: &mut Harness) -> CommandEnvelope {
    tokio::time::timeout(Duration::from_secs(5), h.commands.recv_envelope())
        .await
        .expect("settings command deadline")
        .expect("settings command")
}
pub(super) fn assert_target(
    envelope: &CommandEnvelope,
    session: &str,
    profile: &str,
    expected: &SettingsMutation,
) {
    assert!(envelope.request_id.as_ref().is_some_and(|id| !id.is_empty()));
    match &envelope.command {
        BridgeCommand::MutateSetting { session_id, app_settings_path, mutation } => {
            assert_eq!(session_id, session);
            assert_eq!(app_settings_path.as_deref(), Some(profile));
            assert_eq!(mutation, expected);
        }
        other => panic!("expected mutation, got {other:?}"),
    }
}
pub(super) async fn mutation(
    h: &mut Harness,
    id: &str,
    scope: SettingsScope,
    revision: &str,
    operation: SettingsOperation,
    value: Option<Value>,
) -> String {
    let envelope = receive(h).await;
    assert_target(
        &envelope,
        "settings-session",
        &h.profile,
        &SettingsMutation {
            context: "settings-context".into(),
            id: id.into(),
            scope,
            expected_revision: revision.into(),
            operation,
            value,
        },
    );
    assert!(h.commands.try_recv().is_err(), "exactly one settings mutation");
    envelope.request_id.unwrap()
}
pub(super) fn deliver(
    app: &mut App,
    session: &str,
    request_id: Option<&str>,
    persistence: SettingsPersistence,
    snapshot: Option<SettingsSnapshot>,
    error: Option<&str>,
) {
    crate::app::handle_client_event(
        app,
        ClientEvent::SettingsResultReceived {
            session_id: session.into(),
            request_id: request_id.map(str::to_owned),
            result: SettingsResult {
                persistence,
                application: SettingsApplication::NextSession,
                snapshot,
                error: error.map(str::to_owned),
            },
        },
    );
}
pub(super) fn ack(app: &mut App, request_id: &str, snapshot: SettingsSnapshot) {
    deliver(
        app,
        "settings-session",
        Some(request_id),
        SettingsPersistence::Saved,
        Some(snapshot),
        None,
    );
}
pub(super) fn screen(app: &mut App, width: u16, height: u16) -> Vec<String> {
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| crate::ui::render_fullscreen_surface(frame, app)).unwrap();
    terminal
        .backend()
        .buffer()
        .content
        .chunks(usize::from(width))
        .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect())
        .collect()
}
pub(super) fn row<'a>(rows: &'a [String], label: &str) -> &'a str {
    rows.iter()
        .find(|r| r.contains(label))
        .unwrap_or_else(|| panic!("missing {label}:\n{}", rows.join("\n")))
}
pub(super) fn details(rows: &[String]) -> String {
    let start = rows.iter().position(|r| r.contains("Description:")).unwrap();
    rows[start..]
        .iter()
        .map(|r| r.trim_matches(|c: char| c.is_whitespace() || c == '│'))
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
pub(super) fn editor_body(rows: &[String]) -> String {
    let top = rows.iter().position(|r| r.contains('\u{250c}') && !r.contains("Search")).unwrap();
    let left = rows[top].chars().position(|c| c == '\u{250c}').unwrap();
    let right = rows[top].chars().position(|c| c == '\u{2510}').unwrap();
    let end = rows
        .iter()
        .enumerate()
        .skip(top + 1)
        .find(|(_, r)| r.chars().nth(left) == Some('\u{2514}'))
        .map(|(i, _)| i)
        .unwrap();
    rows[top + 1..end]
        .iter()
        .map(|r| r.chars().skip(left + 1).take(right - left - 1).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}
pub(super) fn reconnect(h: &mut Harness) {
    let (conn, commands) = AgentConnection::test_channel();
    h.app.session_runtime.conn = Some(Rc::new(conn));
    h.commands = commands;
}
pub(super) fn replace_session(app: &mut App, cwd: &std::path::Path) {
    crate::app::handle_client_event(
        app,
        ClientEvent::SessionReplaced {
            session_id: "replacement-session".into(),
            cwd: cwd.to_string_lossy().into_owned(),
            current_model: crate::agent::model::CurrentModel::new("model", "Model", "Model"),
            available_models: vec![],
            mode: None,
            fast_mode_state: crate::agent::model::FastModeState::Off,
            fast_mode_disabled_reason: None,
            ultracode: None,
            history_updates: vec![],
            restored_input: None,
        },
    );
}

pub(super) async fn inspection(h: &mut Harness, session: &str) -> String {
    let envelope = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let envelope = h.commands.recv_envelope().await.unwrap();
            assert!(
                !matches!(envelope.command, BridgeCommand::MutateSetting { .. }),
                "unexpected mutation during refresh"
            );
            if matches!(envelope.command, BridgeCommand::InspectSettings { .. }) {
                break envelope;
            }
        }
    })
    .await
    .expect("inspection deadline");
    assert!(
        matches!(&envelope.command,BridgeCommand::InspectSettings { session_id,app_settings_path } if session_id==session && app_settings_path.as_deref()==Some(h.profile.as_str()))
    );
    envelope.request_id.unwrap()
}
