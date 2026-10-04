// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::agent::client::AgentConnection;
use crate::agent::events::ClientEvent;
use crate::agent::settings::{SettingsOperation, SettingsScope, SettingsSnapshot};
use crate::agent::wire::BridgeCommand;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::json;
use std::rc::Rc;

fn catalog_editor(id: &str) -> Option<crate::agent::settings::EditorSchema> {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../../../../tests/fixtures/settings-ui-catalog.json"))
            .expect("catalog fixture");
    let setting = fixture["catalog"]
        .as_array()
        .expect("catalog")
        .iter()
        .find(|setting| setting["id"] == id)
        .expect("setting");
    serde_json::from_value(setting["editor"].clone()).expect("editor schema")
}

fn snapshot(cwd: &str, value: &str) -> SettingsSnapshot {
    serde_json::from_value(json!({
        "cwd": cwd, "context": "context-1", "diagnostics": [], "resolution_sources": [], "provenance": {},
        "categories": [{"id":"general", "label":"General", "short_label":"General"}], "catalog": [{ "id": "language", "label": "Language", "description": "Response language", "key_path": ["language"], "category":"general", "kind": "string", "options": [], "allows_custom": true, "writable_scopes": ["user", "project", "local"], "reset": "Reset removes the saved value here", "application": "next_session" }],
        "sources": [{ "scope": "user", "path": "settings.json", "status": "valid", "values": [{ "id": "language", "revision": "revision-1", "value": value }] }],
        "values": [{ "id": "language", "value": value, "contributors": ["user"], "policy_restricted": false }]
    })).expect("snapshot contract")
}

#[tokio::test]
async fn acknowledged_spinner_tip_setting_only_controls_tips() {
    let mut app = App::test_default();
    app.session_runtime.session_id = Some("session-1".into());
    app.transcript.messages.push(crate::app::ChatMessage::new(
        crate::app::MessageRole::Assistant,
        Vec::new(),
        None,
    ));
    app.bind_active_turn_assistant(0);
    app.status = crate::app::AppStatus::Running;
    let start = std::time::Instant::now();
    app.begin_turn_activity(start);
    let now = start;
    assert!(app.activity_presentation(now).expect("active").tip.is_some());
    for enabled in [false, true] {
        let mut acknowledged = SettingsSnapshot::test_value("spinnerTipsEnabled", json!(enabled));
        acknowledged.cwd.clone_from(&app.cwd_raw);
        app.config.pending_settings_request =
            Some(crate::app::config::PendingSettingsRequest::Inspection("tips".into()));
        crate::app::events::handle_client_event(
            &mut app,
            ClientEvent::SettingsResultReceived {
                session_id: "session-1".into(),
                request_id: Some("tips".into()),
                result: SettingsResult {
                    persistence: SettingsPersistence::NotRequested,
                    application: SettingsApplication::Host,
                    snapshot: Some(acknowledged),
                    error: None,
                },
            },
        );
        let presentation = app.activity_presentation(now).expect("active turn");
        assert_eq!(presentation.tip.is_some(), enabled);
        assert!(!presentation.thinking, "settings cannot invent SDK thinking");
        let rows = crate::ui::activity_rows::build_activity_rows(&app, 80, now);
        assert!(!rows.is_empty());
        assert_eq!(rows.iter().any(|row| row.to_string().contains("Tip:")), enabled);
        assert!(rows.iter().all(|row| row.to_string().starts_with('│')));
        let live = crate::ui::inline_chat_rows::serialize_live_rows_with_boundaries_excluding(
            &mut app,
            80,
            &std::collections::BTreeSet::new(),
        );
        assert!(live.rows().iter().any(|row| row.to_string().trim_end() == "Claude"));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn automatic_update_editor_cycles_acknowledged_choices_and_resets_the_personal_value() {
    use ratatui::{Terminal, backend::TestBackend};
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut app = App::test_default();
            let (connection, mut commands) = AgentConnection::test_channel();
            app.session_runtime.conn = Some(Rc::new(connection));
            app.session_runtime.session_id = Some("session-1".into());
            app.surface_mode =
                crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
            let mut initial = snapshot(&app.cwd_raw, "");
            let descriptor = &mut initial.catalog[0];
            descriptor.id = "updates.autoInstall".into();
            descriptor.label = "Automatic updates".into();
            descriptor.description = "Install automatically after exit. Default: Off.".into();
            descriptor.key_path = vec!["updates".into(), "autoInstall".into()];
            descriptor.kind = crate::agent::settings::SettingKind::Boolean;
            descriptor.options = vec![json!(true), json!(false)];
            descriptor.allows_custom = false;
            descriptor.application = SettingsApplication::Host;
            descriptor.writable_scopes = vec![SettingsScope::User];
            initial.sources[0].values[0].id = "updates.autoInstall".into();
            initial.sources[0].values[0].value = None;
            initial.values[0].id = "updates.autoInstall".into();
            initial.values[0].value = None;
            app.config.snapshot = Some(initial);
            let mut terminal = Terminal::new(TestBackend::new(100, 30)).expect("terminal");
            for expected in [Some(true), Some(false), None] {
                let before = app.config.saved_value("updates.autoInstall").cloned();
                let key = if expected.is_some() { KeyCode::Char(' ') } else { KeyCode::Delete };
                crate::app::config::handle_key(&mut app, KeyEvent::new(key, KeyModifiers::NONE));
                let request = commands.recv_envelope().await.expect("mutation");
                let BridgeCommand::MutateSetting { mutation, .. } = request.command else {
                    panic!("setting mutation");
                };
                assert_eq!(mutation.id, "updates.autoInstall");
                assert_eq!(mutation.scope, SettingsScope::User);
                assert_eq!(mutation.value, expected.map(serde_json::Value::Bool));
                assert_eq!(app.config.saved_value("updates.autoInstall"), before.as_ref());
                let mut saved = app.config.snapshot.clone().expect("snapshot");
                saved.sources[0].values[0].value = mutation.value.clone();
                saved.values[0].value = mutation.value;
                crate::app::events::handle_client_event(
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
                terminal
                    .draw(|frame| crate::ui::render_fullscreen_surface(frame, &mut app))
                    .expect("render choice");
                let rendered: String = terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(ratatui::buffer::Cell::symbol)
                    .collect();
                assert!(rendered.contains("Automatic updates"));
                assert!(rendered.contains(match expected {
                    Some(true) => "On",
                    Some(false) => "Off",
                    None => "Default",
                }));
                assert_eq!(
                    app.surface_mode,
                    crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config)
                );
                assert!(!app.shutdown_requested());
            }
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn permission_scope_selection_loads_only_its_rules_and_keeps_dialog_scope_fixed() {
    use crate::agent::settings::SettingKind;
    use ratatui::{Terminal, backend::TestBackend};
    let mut app = App::test_default();
    app.surface_mode = crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
    let mut saved = snapshot(&app.cwd_raw, "");
    saved.catalog[0].id = "permissions.deny".to_owned();
    saved.catalog[0].label = "Permissions: deny rules".to_owned();
    saved.catalog[0].kind = SettingKind::StringList;
    saved.catalog[0].editor = catalog_editor("permissions.allow");
    saved.catalog[0].key_path = vec!["permissions".to_owned(), "deny".to_owned()];
    saved.values[0].id = "permissions.deny".to_owned();
    saved.values[0].value = Some(json!(["Read(./.env)", "Bash(git push *)", "Bash(rm *)"]));
    let template = saved.sources[0].clone();
    let cases = [
        (SettingsScope::User, vec!["Read(./.env)"]),
        (SettingsScope::Project, vec!["Bash(git push *)", "Bash(rm *)"]),
        (SettingsScope::Local, vec![]),
    ];
    saved.sources = cases
        .iter()
        .map(|(scope, rules)| {
            let mut source = template.clone();
            source.scope = *scope;
            source.values[0].id = "permissions.deny".to_owned();
            source.values[0].value = Some(json!(rules));
            source
        })
        .collect();
    app.config.snapshot = Some(saved);
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).expect("terminal");
    for (scope, rules) in cases {
        assert_eq!(app.config.selected_scope, scope);
        terminal
            .draw(|frame| crate::ui::render_fullscreen_surface(frame, &mut app))
            .expect("scope list");
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        let count =
            if rules.len() == 1 { "1 item".to_owned() } else { format!("{} items", rules.len()) };
        assert!(rendered.contains(&count), "{rendered}");
        crate::app::config::handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
        );
        let editor = app.config.setting_overlay().expect("scoped list");
        assert_eq!(editor.scope, scope);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&editor.draft).expect("draft JSON"),
            json!(rules)
        );
        crate::app::config::handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE),
        );
        assert_eq!(app.config.selected_scope, scope);
        assert_eq!(app.config.setting_overlay().expect("fixed scope").scope, scope);
        crate::app::config::handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        crate::app::config::handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE),
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn rule_item_editor_preserves_draft_across_resize_conflict_and_acknowledged_save() {
    use ratatui::{Terminal, backend::TestBackend};
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut app = App::test_default();
            app.surface_mode =
                crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
            let (connection, mut commands) = AgentConnection::test_channel();
            app.session_runtime.conn = Some(Rc::new(connection));
            app.session_runtime.session_id = Some("session-1".into());
            let mut saved = snapshot(&app.cwd_raw, "");
            saved.catalog[0].id = "permissions.allow".to_owned();
            saved.catalog[0].label = "Permissions: allow rules".to_owned();
            saved.catalog[0].kind = crate::agent::settings::SettingKind::StringList;
            saved.catalog[0].editor = catalog_editor("permissions.allow");
            saved.catalog[0].key_path = vec!["permissions".to_owned(), "allow".to_owned()];
            saved.values[0].id = "permissions.allow".to_owned();
            saved.values[0].value = Some(json!(["Read(./src/**)"]));
            saved.sources[0].values[0].id = "permissions.allow".to_owned();
            saved.sources[0].values[0].value = Some(json!(["Read(./src/**)"]));
            app.config.snapshot = Some(saved.clone());
            crate::app::config::handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
            );
            for rule in ["Read(./docs/**)", "Bash(npm test *)"] {
                crate::app::config::handle_key(
                    &mut app,
                    KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
                );
                assert!(crate::app::config::handle_paste(&mut app, rule));
                crate::app::config::handle_key(
                    &mut app,
                    KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                );
            }
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(
                    &app.config.setting_overlay().expect("editor").draft
                )
                .expect("JSON"),
                json!(["Read(./src/**)", "Read(./docs/**)", "Bash(npm test *)"])
            );
            for (width, height) in [(100, 35), (42, 16), (87, 25)] {
                let mut terminal =
                    Terminal::new(TestBackend::new(width, height)).expect("terminal");
                terminal
                    .draw(|frame| crate::ui::render_fullscreen_surface(frame, &mut app))
                    .expect("render");
                let rendered = terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(ratatui::buffer::Cell::symbol)
                    .collect::<String>();
                assert!(rendered.contains("Bash(npm test *)"), "{rendered}");
            }
            crate::app::config::handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
            );
            let save = commands.recv_envelope().await.expect("save");
            let BridgeCommand::MutateSetting { mutation, .. } = save.command else {
                panic!("mutation");
            };
            assert_eq!(
                mutation.value,
                Some(json!(["Read(./src/**)", "Read(./docs/**)", "Bash(npm test *)"]))
            );
            assert_eq!(
                app.config.saved_value("permissions.allow"),
                Some(&json!(["Read(./src/**)"]))
            );
            saved.sources[0].values[0].revision = "external-revision".to_owned();
            saved.sources[0].values[0].value = Some(json!(["Read(./external)"]));
            saved.values[0].value = Some(json!(["Read(./external)"]));
            apply_settings_result(
                &mut app,
                save.request_id.as_deref(),
                SettingsResult {
                    persistence: SettingsPersistence::Conflict,
                    application: SettingsApplication::Blocked,
                    snapshot: Some(saved.clone()),
                    error: Some("Review the external edit".to_owned()),
                },
            );
            assert!(
                app.config.setting_overlay().expect("draft").draft.contains("Bash(npm test *)")
            );
            crate::app::config::handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
            );
            let retry = commands.recv_envelope().await.expect("retry");
            let BridgeCommand::MutateSetting { mutation, .. } = retry.command else {
                panic!("mutation");
            };
            assert_eq!(mutation.expected_revision, "external-revision");
            saved.values[0].value.clone_from(&mutation.value);
            saved.sources[0].values[0].value = mutation.value;
            apply_settings_result(
                &mut app,
                retry.request_id.as_deref(),
                SettingsResult {
                    persistence: SettingsPersistence::Saved,
                    application: SettingsApplication::NextSession,
                    snapshot: Some(saved),
                    error: None,
                },
            );
            assert_eq!(
                app.config.saved_value("permissions.allow"),
                Some(&json!(["Read(./src/**)", "Read(./docs/**)", "Bash(npm test *)"]))
            );
            assert!(app.config.setting_overlay().is_none());
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn structured_editor_reports_parse_error_and_saves_corrected_json_without_flattening() {
    tokio::task::LocalSet::new().run_until(async {
        let mut app = App::test_default();
        let (connection, mut commands) = AgentConnection::test_channel();
        app.session_runtime.conn = Some(Rc::new(connection));
        app.session_runtime.session_id = Some("session-1".into());
        let mut saved = snapshot(&app.cwd_raw, "{}");
        saved.catalog[0].kind = crate::agent::settings::SettingKind::Json;
        saved.catalog[0].editor = catalog_editor("hooks");
        saved.values[0].value = Some(json!({}));
        saved.sources[0].values[0].value = Some(json!({}));
        app.config.snapshot = Some(saved);
        crate::app::config::handle_key(&mut app, KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        crate::app::config::handle_key(&mut app, KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL));
        crate::app::config::handle_key(&mut app, KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        crate::app::config::handle_key(&mut app, KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        assert!(app.config.overlay_message.as_ref().expect("error").text.contains("Check the value"));
        assert_eq!(app.config.setting_overlay().expect("draft").draft, "{");
        assert!(crate::app::config::handle_paste(&mut app, "\n\"PreToolUse\": [{\"matcher\": \"Bash\", \"hooks\": [{\"type\": \"command\", \"command\": \"echo checked\"}]}]\n}"));
        crate::app::config::handle_key(&mut app, KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        let envelope = commands.recv_envelope().await.expect("save");
        let BridgeCommand::MutateSetting { mutation, .. } = envelope.command else { panic!("mutation"); };
        assert_eq!(mutation.value, Some(json!({"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"echo checked"}]}]})));
    }).await;
}

#[tokio::test(flavor = "current_thread")]
async fn config_edits_and_resets_use_correlated_snapshot_mutations() {
    tokio::task::LocalSet::new().run_until(async {
        let mut app = App::test_default();
        let (connection, mut commands) = AgentConnection::test_channel();
        app.session_runtime.conn = Some(Rc::new(connection));
        app.session_runtime.session_id = Some(crate::agent::model::SessionId::new("session-1"));
        crate::app::config::open(&mut app).expect("open");
        let inspect = commands.recv_envelope().await.expect("inspect");
        assert!(matches!(inspect.command, BridgeCommand::InspectSettings { .. }));
        let initial = snapshot(&app.cwd_raw, "German");
        crate::app::events::handle_client_event(&mut app, ClientEvent::SettingsResultReceived {
            session_id: "session-1".to_owned(), request_id: inspect.request_id,
            result: SettingsResult { persistence: SettingsPersistence::NotRequested, application: SettingsApplication::Blocked, snapshot: Some(initial), error: None },
        });
        crate::app::config::handle_key(&mut app, KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        assert_eq!(app.config.setting_overlay().expect("editor").draft, "German");
        for _ in 0..6 { crate::app::config::handle_key(&mut app, KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)); }
        assert!(crate::app::config::handle_paste(&mut app, "Greek"));
        crate::app::config::handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let save = commands.recv_envelope().await.expect("save");
        let BridgeCommand::MutateSetting { mutation, .. } = save.command else { panic!("mutation") };
        assert_eq!(mutation.id, "language");
        assert_eq!(mutation.scope, SettingsScope::User);
        assert_eq!(mutation.expected_revision, "revision-1");
        assert_eq!(mutation.operation, SettingsOperation::Set);
        assert_eq!(mutation.value, Some(json!("Greek")));
        assert_eq!(app.config.saved_value("language"), Some(&json!("German")));
        let updated = snapshot(&app.cwd_raw, "Greek");
        apply_settings_result(&mut app, save.request_id.as_deref(), SettingsResult { persistence: SettingsPersistence::Saved, application: SettingsApplication::NextSession, snapshot: Some(updated), error: None });
        assert_eq!(app.config.saved_value("language"), Some(&json!("Greek")));
        assert!(app.config.status_message.as_deref().is_some_and(|message| message.contains("next session")));
        crate::app::config::handle_key(&mut app, KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE));
        let reset = commands.recv_envelope().await.expect("reset");
        assert!(matches!(reset.command, BridgeCommand::MutateSetting { mutation, .. } if mutation.operation == SettingsOperation::Remove && mutation.id == "language"));
    }).await;
}

#[test]
fn request_correlation_and_session_epoch_protect_editor_state() {
    let mut app = App::test_default();
    app.config.pending_settings_request =
        Some(crate::app::config::PendingSettingsRequest::Inspection("current".to_owned()));
    let saved = snapshot(&app.cwd_raw, "German");
    app.config.snapshot = Some(saved.clone());
    let incoming = snapshot(&app.cwd_raw, "Greek");
    apply_settings_result(
        &mut app,
        Some("stale"),
        SettingsResult {
            persistence: SettingsPersistence::Saved,
            application: SettingsApplication::NextSession,
            snapshot: Some(incoming),
            error: None,
        },
    );
    assert_eq!(app.config.snapshot, Some(saved));
    crate::app::config::handle_key(&mut app, KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    app.bump_session_scope_epoch();
    assert!(app.config.snapshot.is_none());
    assert!(app.config.pending_settings_request.is_none());
    assert!(app.config.setting_overlay().is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn inline_arrows_and_space_cycle_choices_and_render_acknowledged_values() {
    use ratatui::{Terminal, backend::TestBackend};
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut app = App::test_default();
            app.surface_mode =
                crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
            let (connection, mut commands) = AgentConnection::test_channel();
            app.session_runtime.conn = Some(Rc::new(connection));
            app.session_runtime.session_id = Some(crate::agent::model::SessionId::new("session-1"));
            let mut resolved = snapshot(&app.cwd_raw, "small");
            resolved.catalog[0].options = vec![json!("small"), json!("medium"), json!("large")];
            resolved.catalog[0].allows_custom = false;
            app.config.snapshot = Some(resolved);
            let mut terminal = Terminal::new(TestBackend::new(100, 30)).expect("terminal");
            for (key, expected) in [
                (KeyCode::Left, "large"),
                (KeyCode::Right, "small"),
                (KeyCode::Char(' '), "medium"),
            ] {
                crate::app::config::handle_key(&mut app, KeyEvent::new(key, KeyModifiers::NONE));
                let save = commands.recv_envelope().await.expect("choice save");
                let BridgeCommand::MutateSetting { mutation, .. } = save.command else {
                    panic!("mutation")
                };
                assert_eq!(mutation.operation, SettingsOperation::Set);
                assert_eq!(mutation.value, Some(json!(expected)));
                let mut updated = app.config.snapshot.clone().expect("previous snapshot");
                updated.sources[0].values[0].value = Some(json!(expected));
                updated.values[0].value = Some(json!(expected));
                crate::app::events::handle_client_event(
                    &mut app,
                    ClientEvent::SettingsResultReceived {
                        session_id: "session-1".to_owned(),
                        request_id: save.request_id,
                        result: SettingsResult {
                            persistence: SettingsPersistence::Saved,
                            application: SettingsApplication::NextSession,
                            snapshot: Some(updated),
                            error: None,
                        },
                    },
                );
                assert_eq!(app.config.saved_value("language"), Some(&json!(expected)));
                assert!(app.config.pending_settings_request.is_none());
                terminal
                    .draw(|frame| crate::ui::render_fullscreen_surface(frame, &mut app))
                    .expect("saved choice");
                let text: String = terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(ratatui::buffer::Cell::symbol)
                    .collect();
                assert!(text.contains(&format!("Saved in user: {expected}")));
            }
            app.config.snapshot.as_mut().expect("snapshot").catalog[0].writable_scopes.clear();
            crate::app::config::handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE),
            );
            crate::app::config::handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
            );
            assert!(
                commands.try_recv().is_err(),
                "read-only controls must preserve the saved value"
            );
            assert_eq!(app.config.saved_value("language"), Some(&json!("medium")));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn settings_load_automatically_after_startup_without_a_manual_refresh() {
    use crate::agent::model;
    use ratatui::{Terminal, backend::TestBackend};
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut app = App::test_default();
            app.status = crate::app::AppStatus::Connecting;
            let (connection, mut commands) = AgentConnection::test_channel();
            app.session_runtime.conn = Some(Rc::new(connection));
            crate::app::config::open(&mut app).expect("open during startup");
            assert!(app.config.last_error.is_none(), "startup is not a settings error");
            let mut terminal = Terminal::new(TestBackend::new(100, 30)).expect("terminal");
            terminal
                .draw(|frame| crate::ui::render_fullscreen_surface(frame, &mut app))
                .expect("startup render");
            let text: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(ratatui::buffer::Cell::symbol)
                .collect();
            assert!(text.contains("Loading settings"));
            let cwd = app.cwd_raw.clone();
            crate::app::events::handle_client_event(
                &mut app,
                ClientEvent::Connected {
                    session_id: model::SessionId::new("session-1"),
                    cwd,
                    current_model: model::CurrentModel::new("opus", "Opus", "Opus"),
                    available_models: Vec::new(),
                    mode: None,
                    fast_mode_state: model::FastModeState::Off,
                    fast_mode_disabled_reason: None,
                    ultracode: None,
                    history_updates: Vec::new(),
                },
            );
            assert!(
                app.config.pending_settings_request.is_some(),
                "startup must request settings automatically"
            );
            let inspect = loop {
                let command = commands.recv_envelope().await.expect("startup command");
                if matches!(command.command, BridgeCommand::InspectSettings { .. }) {
                    break command;
                }
            };
            let resolved = snapshot(&app.cwd_raw, "German");
            crate::app::events::handle_client_event(
                &mut app,
                ClientEvent::SettingsResultReceived {
                    session_id: "session-1".to_owned(),
                    request_id: inspect.request_id,
                    result: SettingsResult {
                        persistence: SettingsPersistence::NotRequested,
                        application: SettingsApplication::Blocked,
                        snapshot: Some(resolved),
                        error: None,
                    },
                },
            );
            terminal
                .draw(|frame| crate::ui::render_fullscreen_surface(frame, &mut app))
                .expect("loaded settings");
            let text: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(ratatui::buffer::Cell::symbol)
                .collect();
            assert!(text.contains("Saved in user: German"));
            assert!(app.config.last_error.is_none());
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn language_save_and_editor_reset_show_the_remaining_value_or_default() {
    use crate::agent::settings::{ScopedSetting, SettingsSource};
    use ratatui::{Terminal, backend::TestBackend};
    tokio::task::LocalSet::new().run_until(async {
        for inherited in [Some("English"), None] {
            let mut app = App::test_default();
            let (connection, mut commands) = AgentConnection::test_channel();
            app.session_runtime.conn = Some(Rc::new(connection));
            app.session_runtime.session_id = Some(crate::agent::model::SessionId::new("session-1"));
            app.surface_mode = crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
            let mut initial = snapshot(&app.cwd_raw, "German");
            initial.sources[0].scope = SettingsScope::Local;
            initial.sources.push(SettingsSource {
                scope: SettingsScope::User, path: "user/settings.json".to_owned(), status: "valid".to_owned(), error: None,
                values: vec![ScopedSetting { id: "language".to_owned(), revision: "user-revision".to_owned(), value: inherited.map(|value| json!(value)) }],
            });
            initial.values[0].contributors = vec!["local".to_owned()];
            app.config.snapshot = Some(initial);
            app.config.selected_scope = SettingsScope::Local;
            crate::app::config::handle_key(&mut app, KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
            for _ in 0..6 { crate::app::config::handle_key(&mut app, KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)); }
            crate::app::config::handle_paste(&mut app, "Greek");
            crate::app::config::handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
            let save = commands.recv_envelope().await.expect("save");
            assert!(matches!(save.command, BridgeCommand::MutateSetting { mutation, .. } if mutation.scope == SettingsScope::Local && mutation.value == Some(json!("Greek"))));
            let mut saved = app.config.snapshot.clone().expect("snapshot");
            saved.sources[0].values[0].value = Some(json!("Greek"));
            saved.values[0].value = Some(json!("Greek"));
            crate::app::events::handle_client_event(&mut app, ClientEvent::SettingsResultReceived {
                session_id: "session-1".to_owned(), request_id: save.request_id,
                result: SettingsResult { persistence: SettingsPersistence::Saved, application: SettingsApplication::NextSession, snapshot: Some(saved), error: None },
            });
            crate::app::config::handle_key(&mut app, KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
            assert_eq!(app.config.setting_overlay().expect("reopened").draft, "Greek");
            crate::app::config::handle_key(&mut app, KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
            let reset = commands.recv_envelope().await.expect("reset");
            assert!(matches!(reset.command, BridgeCommand::MutateSetting { mutation, .. } if mutation.scope == SettingsScope::Local && mutation.operation == SettingsOperation::Remove));
            let mut resolved = app.config.snapshot.clone().expect("saved snapshot");
            resolved.sources[0].values[0].value = None;
            resolved.values[0].value = inherited.map(|value| json!(value));
            resolved.values[0].contributors = inherited.map_or_else(Vec::new, |_| vec!["user".to_owned()]);
            crate::app::events::handle_client_event(&mut app, ClientEvent::SettingsResultReceived {
                session_id: "session-1".to_owned(), request_id: reset.request_id,
                result: SettingsResult { persistence: SettingsPersistence::Saved, application: SettingsApplication::NextSession, snapshot: Some(resolved), error: None },
            });
            let mut terminal = Terminal::new(TestBackend::new(100, 30)).expect("terminal");
                terminal.draw(|frame| crate::ui::render_fullscreen_surface(frame, &mut app)).expect("reset render");
            let lines: Vec<String> = terminal.backend().buffer().content.chunks(100).map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect()).collect();
            assert!(lines.iter().any(|line| line.contains("Language") && line.contains(inherited.unwrap_or("Default"))));
            assert!(lines.iter().any(|line| line.contains("Saved in local: not set")));
            crate::app::config::handle_key(&mut app, KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
            assert_eq!(app.config.setting_overlay().expect("reset editor").draft, inherited.unwrap_or(""));
        }
    }).await;
}

#[tokio::test(flavor = "current_thread")]
async fn conflict_refresh_preserves_draft_and_deliberate_retry_uses_the_refreshed_value() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut app = App::test_default();
            let (connection, mut commands) = AgentConnection::test_channel();
            app.session_runtime.conn = Some(Rc::new(connection));
            app.session_runtime.session_id = Some(crate::agent::model::SessionId::new("session-1"));
            app.config.snapshot = Some(snapshot(&app.cwd_raw, "German"));
            crate::app::config::handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
            );
            crate::app::config::handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            );
            let first = commands.recv_envelope().await.expect("initial save");
            assert!(matches!(first.command, BridgeCommand::MutateSetting { .. }));
            // Keep the submitted draft frozen while its acknowledgement is pending.
            assert!(crate::app::config::handle_paste(&mut app, "pending"));
            assert_eq!(app.config.setting_overlay().expect("draft").draft, "German");
            let mut refreshed = snapshot(&app.cwd_raw, "Japanese");
            refreshed.sources[0].values[0].revision = "revision-2".to_owned();
            apply_settings_result(
                &mut app,
                first.request_id.as_deref(),
                SettingsResult {
                    persistence: SettingsPersistence::Conflict,
                    application: SettingsApplication::Blocked,
                    snapshot: Some(refreshed),
                    error: Some("Review the refreshed saved value".to_owned()),
                },
            );
            assert_eq!(app.config.setting_overlay().expect("draft").draft, "German");
            assert_eq!(app.config.saved_value("language"), Some(&json!("Japanese")));
            assert_eq!(
                app.config.overlay_message.as_ref().expect("message").text,
                "Review the refreshed saved value"
            );
            crate::app::config::handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            );
            let retry = commands.recv_envelope().await.expect("deliberate retry");
            let BridgeCommand::MutateSetting { mutation, .. } = retry.command else {
                panic!("mutation")
            };
            assert_eq!(mutation.expected_revision, "revision-2");
            assert_eq!(mutation.value, Some(json!("German")));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn default_effort_cycles_and_resets_without_changing_session_effort() {
    tokio::task::LocalSet::new().run_until(async {
        let mut app = App::test_default();
        let (connection, mut commands) = AgentConnection::test_channel();
        app.session_runtime.conn = Some(Rc::new(connection));
        app.session_runtime.session_id = Some("session-1".into());
        app.session_runtime.config_options.insert("effortLevel".to_owned(), json!("max"));
        let mut shown = snapshot(&app.cwd_raw, "medium");
        let setting = &mut shown.catalog[0];
        setting.id = "defaultEffort".to_owned();
        setting.label = "Default effort".to_owned();
        setting.key_path = vec!["modelSettings".to_owned(), "claude-sonnet-4-6".to_owned(), "effortLevel".to_owned()];
        setting.options = vec![json!("low"), json!("medium"), json!("high")];
        setting.allows_custom = false;
        shown.sources[0].values[0].id = setting.id.clone();
        shown.values[0].id = setting.id.clone();
        app.config.snapshot = Some(shown);
        app.surface_mode = crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
        crate::app::config::handle_key(&mut app, KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        let save = commands.recv_envelope().await.expect("save effort");
        assert!(matches!(save.command, BridgeCommand::MutateSetting { mutation, .. } if mutation.id == "defaultEffort" && mutation.value == Some(json!("high"))));
        assert_eq!(app.config.saved_value("defaultEffort"), Some(&json!("medium")), "await acknowledgement");
        let mut saved = app.config.snapshot.clone().expect("snapshot");
        saved.sources[0].values[0].value = Some(json!("high"));
        saved.values[0].value = Some(json!("high"));
        apply_settings_result(&mut app, save.request_id.as_deref(), SettingsResult { persistence: SettingsPersistence::Saved, application: SettingsApplication::NextSession, snapshot: Some(saved), error: None });
        assert_eq!(app.config.saved_value("defaultEffort"), Some(&json!("high")));
        crate::app::config::handle_key(&mut app, KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE));
        let reset = commands.recv_envelope().await.expect("reset effort");
        assert!(matches!(reset.command, BridgeCommand::MutateSetting { mutation, .. } if mutation.id == "defaultEffort" && mutation.operation == SettingsOperation::Remove));
        let mut inherited = app.config.snapshot.clone().expect("snapshot");
        inherited.sources[0].values[0].value = None;
        inherited.values[0].value = None;
        apply_settings_result(&mut app, reset.request_id.as_deref(), SettingsResult { persistence: SettingsPersistence::Saved, application: SettingsApplication::NextSession, snapshot: Some(inherited), error: None });
        assert_eq!(app.config.saved_value("defaultEffort"), None);
        assert_eq!(app.session_runtime.config_options.get("effortLevel"), Some(&json!("max")));
    }).await;
}

#[tokio::test(flavor = "current_thread")]
async fn fixed_model_and_effort_choices_cycle_in_place_through_acknowledged_saves() {
    use ratatui::{Terminal, backend::TestBackend};
    tokio::task::LocalSet::new().run_until(async {
        for (id, label, choices) in [
            ("model", "Default model", (1..=12).map(|n| format!("model-{n}")).collect::<Vec<_>>()),
            ("defaultEffort", "Default effort", vec!["low".to_owned(), "medium".to_owned(), "high".to_owned()]),
        ] {
            let mut app = App::test_default();
            let (connection, mut commands) = AgentConnection::test_channel();
            app.session_runtime.conn = Some(Rc::new(connection));
            app.session_runtime.session_id = Some("session-1".into());
            let mut shown = snapshot(&app.cwd_raw, &choices[0]);
            shown.catalog[0].id = id.to_owned(); shown.catalog[0].label = label.to_owned();
            shown.catalog[0].allows_custom = false;
            shown.catalog[0].options = choices.iter().map(|value| json!(value)).collect();
            shown.sources[0].values[0].id = id.to_owned(); shown.values[0].id = id.to_owned();
            app.config.snapshot = Some(shown);
            app.surface_mode = crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
            let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("terminal");
            for step in 1..=choices.len() {
                let previous = app.config.saved_value(id).cloned();
                let key = if step % 2 == 0 { KeyCode::Right } else { KeyCode::Char(' ') };
                crate::app::config::handle_key(&mut app, KeyEvent::new(key, KeyModifiers::NONE));
                let save = commands.recv_envelope().await.expect("save selected option");
                let expected = json!(choices[step % choices.len()]);
                assert!(matches!(save.command, BridgeCommand::MutateSetting { mutation, .. } if mutation.id == id && mutation.value == Some(expected.clone())));
                assert_eq!(app.config.saved_value(id), previous.as_ref());
                crate::app::config::handle_key(&mut app, KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
                assert!(commands.try_recv().is_err(), "one save in flight");
                let mut acknowledged = app.config.snapshot.clone().expect("snapshot");
                acknowledged.sources[0].values[0].value = Some(expected.clone());
                acknowledged.values[0].value = Some(expected.clone());
                apply_settings_result(&mut app, save.request_id.as_deref(), SettingsResult { persistence: SettingsPersistence::Saved, application: SettingsApplication::NextSession, snapshot: Some(acknowledged), error: None });
                terminal.draw(|frame| crate::ui::render_fullscreen_surface(frame, &mut app)).expect("saved choice");
                let text: String = terminal.backend().buffer().content.iter().map(ratatui::buffer::Cell::symbol).collect();
                assert!(text.contains(label));
                assert!(text.contains(expected.as_str().expect("string choice")));
                assert!(text.contains("1/1"));
                assert!(text.contains("Space change"));
                assert!(text.contains("Left/Right change"));
                assert_eq!(app.config.selected_setting().expect("selection").id, id);
            }
        }
    }).await;
}
