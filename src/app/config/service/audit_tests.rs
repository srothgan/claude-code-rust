// SPDX-License-Identifier: Apache-2.0
use crate::app::config::settings_audit_support::*;

#[tokio::test(flavor = "current_thread")]
async fn scoped_rule_browsing_keeps_the_captured_source_and_never_sends_a_mutation() {
    let mut h = fixture();
    let snapshot = h.app.config.snapshot.as_mut().unwrap();
    put(
        snapshot,
        SettingsScope::User,
        "permissions.deny",
        Some(json!(["Read(./.env)"])),
        "user-r1",
    );
    put(
        snapshot,
        SettingsScope::Project,
        "permissions.deny",
        Some(json!(["Bash(git push *)", "Bash(rm *)"])),
        "project-r1",
    );
    put(snapshot, SettingsScope::Local, "permissions.deny", Some(json!([])), "local-r1");
    effective(
        snapshot,
        "permissions.deny",
        Some(json!(["Read(./.env)", "Bash(git push *)", "Bash(rm *)"])),
        &["user", "project"],
    );
    select(&mut h.app, "permissions.deny", SettingsScope::User);
    for (scope, value, count, label) in [
        (SettingsScope::User, json!(["Read(./.env)"]), "1 item", "User"),
        (SettingsScope::Project, json!(["Bash(git push *)", "Bash(rm *)"]), "2 items", "Project"),
        (SettingsScope::Local, json!([]), "0 items", "Local"),
    ] {
        let rows = screen(&mut h.app, 100, 32);
        assert!(row(&rows, "Deny rules").contains(count));
        assert!(row(&rows, "Save in:").contains(label));
        press(&mut h.app, KeyCode::Enter);
        assert_eq!(draft(&h.app), value);
        press(&mut h.app, KeyCode::Char('s'));
        assert_eq!(h.app.config.selected_scope, scope);
        assert_eq!(h.app.config.setting_overlay().unwrap().scope, scope);
        assert_eq!(draft(&h.app), value);
        assert!(h.commands.try_recv().is_err());
        press(&mut h.app, KeyCode::Esc);
        press(&mut h.app, KeyCode::Char('s'));
    }
}

fn rules_snapshot(app: &App, allow: Value, revision: &str) -> SettingsSnapshot {
    let mut snapshot = app.config.snapshot.clone().unwrap();
    put(&mut snapshot, SettingsScope::Project, "permissions.allow", Some(allow.clone()), revision);
    effective(&mut snapshot, "permissions.allow", Some(allow), &["project"]);
    snapshot
}

#[tokio::test(flavor = "current_thread")]
async fn rule_save_conflict_preserves_the_project_draft_and_retries_the_complete_target() {
    let mut h = fixture();
    seed(&mut h.app, SettingsScope::User, "permissions.allow", json!(["Read(./user-only)"]));
    seed(&mut h.app, SettingsScope::Project, "permissions.allow", json!(["Read(./src/**)"]));
    seed(&mut h.app, SettingsScope::Project, "permissions.deny", json!(["Read(./.env)"]));
    select(&mut h.app, "permissions.allow", SettingsScope::Project);
    press(&mut h.app, KeyCode::Enter);
    for rule in ["Read(./docs/**)", "Bash(npm test *)"] {
        press(&mut h.app, KeyCode::Char('a'));
        paste(&mut h.app, rule);
        press(&mut h.app, KeyCode::Enter);
    }
    for (w, height) in [(100, 35), (42, 20), (87, 25)] {
        let rows = screen(&mut h.app, w, height);
        let body = editor_body(&rows);
        assert!(body.contains("Bash(npm test *)"));
        assert!(body.contains("Ctrl+S") && body.contains("Esc"), "{body}");
    }
    press(&mut h.app, KeyCode::Tab);
    assert_eq!(h.app.config.active_tab, crate::app::config::ConfigTab::Settings);
    assert!(h.commands.try_recv().is_err());
    control(&mut h.app, 's');
    let first = mutation(
        &mut h,
        "permissions.allow",
        SettingsScope::Project,
        "project-r1",
        SettingsOperation::Set,
        Some(json!(["Read(./src/**)", "Read(./docs/**)", "Bash(npm test *)"])),
    )
    .await;
    let conflicting = rules_snapshot(&h.app, json!(["Read(./external)"]), "project-r2");
    deliver(
        &mut h.app,
        "settings-session",
        Some(&first),
        SettingsPersistence::Conflict,
        Some(conflicting),
        Some("Review external rules before retrying"),
    );
    assert_eq!(draft(&h.app), json!(["Read(./src/**)", "Read(./docs/**)", "Bash(npm test *)"]));
    assert_eq!(h.app.config.saved_value("permissions.allow"), Some(&json!(["Read(./external)"])));
    assert!(h.app.config.overlay_message.as_ref().unwrap().text.contains("external rules"));
    assert!(h.commands.try_recv().is_err());
    control(&mut h.app, 's');
    let retry = mutation(
        &mut h,
        "permissions.allow",
        SettingsScope::Project,
        "project-r2",
        SettingsOperation::Set,
        Some(json!(["Read(./src/**)", "Read(./docs/**)", "Bash(npm test *)"])),
    )
    .await;
    assert_ne!(first, retry);
    let saved = rules_snapshot(
        &h.app,
        json!(["Read(./src/**)", "Read(./docs/**)", "Bash(npm test *)"]),
        "project-r3",
    );
    ack(&mut h.app, &retry, saved);
    assert!(h.app.config.overlay.is_none());
    assert_eq!(
        h.app.config.saved_value("permissions.allow"),
        Some(&json!(["Read(./src/**)", "Read(./docs/**)", "Bash(npm test *)"]))
    );
    assert_eq!(h.app.config.saved_value("permissions.deny"), Some(&json!(["Read(./.env)"])));
    assert_eq!(
        h.app
            .config
            .snapshot
            .as_ref()
            .unwrap()
            .scoped("permissions.allow", SettingsScope::User)
            .unwrap()
            .value,
        Some(json!(["Read(./user-only)"]))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn hooks_json_parse_failure_retains_text_and_the_corrected_nested_value_is_acknowledged() {
    let mut h = fixture();
    seed(
        &mut h.app,
        SettingsScope::Local,
        "hooks",
        json!({"Stop":[{"future":"keep","hooks":[{"type":"command","command":"original"}]}]}),
    );
    select(&mut h.app, "hooks", SettingsScope::Local);
    press(&mut h.app, KeyCode::Enter);
    control(&mut h.app, 'j');
    replace_input(&mut h.app, "{invalid-json");
    control(&mut h.app, 's');
    assert!(h.commands.try_recv().is_err());
    assert!(h.app.config.pending_settings_request.is_none());
    assert_eq!(h.app.config.setting_overlay().unwrap().draft, "{invalid-json");
    assert!(!h.app.config.overlay_message.as_ref().unwrap().text.is_empty());
    control(&mut h.app, 'f');
    assert!(h.app.config.setting_overlay().unwrap().structured.as_ref().unwrap().advanced);
    let corrected = json!({"Stop":[{"future":"keep","hooks":[{"type":"command","command":"corrected","args":["--all"]},{"type":"http","url":"https://hooks.example/notify"}]}]});
    replace_input(&mut h.app, &corrected.to_string());
    control(&mut h.app, 's');
    let request = mutation(
        &mut h,
        "hooks",
        SettingsScope::Local,
        "local-r1",
        SettingsOperation::Set,
        Some(corrected.clone()),
    )
    .await;
    let mut saved = h.app.config.snapshot.clone().unwrap();
    put(&mut saved, SettingsScope::Local, "hooks", Some(corrected.clone()), "local-r2");
    effective(&mut saved, "hooks", Some(corrected.clone()), &["local"]);
    ack(&mut h.app, &request, saved);
    assert!(h.app.config.overlay.is_none());
    assert_eq!(h.app.config.saved_value("hooks"), Some(&corrected));
}

fn language_snapshot(app: &App, value: &str, revision: &str) -> SettingsSnapshot {
    let mut snapshot = app.config.snapshot.clone().unwrap();
    put(&mut snapshot, SettingsScope::Local, "language", Some(json!(value)), revision);
    effective(&mut snapshot, "language", Some(json!(value)), &["local"]);
    snapshot
}

#[tokio::test(flavor = "current_thread")]
async fn stale_settings_results_preserve_pending_work_and_matching_results_can_retry() {
    tokio::task::LocalSet::new()
        .run_until(Box::pin(async {
            let mut h = fixture();
            seed(&mut h.app, SettingsScope::Local, "language", json!("German"));
            select(&mut h.app, "language", SettingsScope::Local);
            press(&mut h.app, KeyCode::Char(' '));
            replace_input(&mut h.app, "Greek");
            press(&mut h.app, KeyCode::Enter);
            let request = mutation(
                &mut h,
                "language",
                SettingsScope::Local,
                "local-r1",
                SettingsOperation::Set,
                Some(json!("Greek")),
            )
            .await;
            let accepted = language_snapshot(&h.app, "Greek", "local-r2");
            for (session, id) in [
                ("old-session", Some(request.as_str())),
                ("settings-session", None),
                ("settings-session", Some("wrong-request")),
            ] {
                deliver(
                    &mut h.app,
                    session,
                    id,
                    SettingsPersistence::Saved,
                    Some(accepted.clone()),
                    None,
                );
                assert_eq!(
                    h.app.config.pending_settings_request.as_ref().unwrap().request_id(),
                    request
                );
                assert_eq!(h.app.config.setting_overlay().unwrap().draft, "Greek");
                assert_eq!(h.app.config.saved_value("language"), Some(&json!("German")));
            }
            deliver(
                &mut h.app,
                "settings-session",
                Some(&request),
                SettingsPersistence::Failure,
                None,
                Some("Could not persist the selected profile"),
            );
            assert!(h.app.config.pending_settings_request.is_none());
            assert_eq!(h.app.config.setting_overlay().unwrap().draft, "Greek");
            assert!(h.app.config.overlay_message.as_ref().unwrap().text.contains("persist"));
            press(&mut h.app, KeyCode::Enter);
            let retry = mutation(
                &mut h,
                "language",
                SettingsScope::Local,
                "local-r1",
                SettingsOperation::Set,
                Some(json!("Greek")),
            )
            .await;
            ack(&mut h.app, &retry, accepted);
            assert!(h.app.config.overlay.is_none());
            press(&mut h.app, KeyCode::Char('r'));
            let inspect = receive(&mut h).await;
            assert!(matches!(inspect.command, BridgeCommand::InspectSettings { .. }));
            let snapshot = h.app.config.snapshot.clone().unwrap();
            deliver(
                &mut h.app,
                "settings-session",
                Some("stale-inspection"),
                SettingsPersistence::NotRequested,
                Some(snapshot.clone()),
                None,
            );
            assert_eq!(
                h.app.config.pending_settings_request.as_ref().unwrap().request_id(),
                inspect.request_id.as_deref().unwrap()
            );
            deliver(
                &mut h.app,
                "settings-session",
                inspect.request_id.as_deref(),
                SettingsPersistence::NotRequested,
                Some(snapshot),
                None,
            );
        }))
        .await;
}

// The bounded table keeps independently specified initial/next values beside each control.
#[allow(clippy::too_many_lines)]
#[tokio::test(flavor = "current_thread")]
async fn fixed_choices_wait_for_acknowledgement_and_target_boolean_model_and_effort_controls() {
    for (id, label, initial, options, key, next, display) in [
        (
            "updates.autoInstall",
            "Automatic updates",
            None,
            vec![json!(true), json!(false)],
            KeyCode::Char(' '),
            json!(true),
            "On",
        ),
        (
            "updates.autoInstall",
            "Automatic updates",
            Some(json!(true)),
            vec![json!(true), json!(false)],
            KeyCode::Right,
            json!(false),
            "Off",
        ),
        (
            "model",
            "Default model",
            Some(json!("sonnet")),
            vec![json!("sonnet"), json!("opus"), json!("haiku")],
            KeyCode::Left,
            json!("haiku"),
            "haiku",
        ),
        (
            "model",
            "Default model",
            Some(json!("external-model")),
            vec![json!("sonnet"), json!("opus")],
            KeyCode::Right,
            json!("sonnet"),
            "sonnet",
        ),
        (
            "defaultEffort",
            "Default effort",
            Some(json!("medium")),
            vec![json!("low"), json!("medium"), json!("high")],
            KeyCode::Char(' '),
            json!("high"),
            "high",
        ),
    ] {
        let mut h = fixture();
        let snapshot = h.app.config.snapshot.as_mut().unwrap();
        let setting = snapshot.catalog.iter_mut().find(|s| s.id == id).unwrap();
        setting.options = options;
        setting.allows_custom = false;
        put(snapshot, SettingsScope::User, id, initial.clone(), "user-r1");
        effective(snapshot, id, initial.clone(), &["user"]);
        select(&mut h.app, id, SettingsScope::User);
        press(&mut h.app, key);
        let request = mutation(
            &mut h,
            id,
            SettingsScope::User,
            "user-r1",
            SettingsOperation::Set,
            Some(next.clone()),
        )
        .await;
        assert_eq!(h.app.config.saved_value(id), initial.as_ref());
        press(&mut h.app, KeyCode::Char(' '));
        assert!(h.commands.try_recv().is_err());
        let mut acknowledged = h.app.config.snapshot.clone().unwrap();
        put(&mut acknowledged, SettingsScope::User, id, Some(next.clone()), "user-r2");
        effective(&mut acknowledged, id, Some(next.clone()), &["user"]);
        ack(&mut h.app, &request, acknowledged);
        assert!(row(&screen(&mut h.app, 100, 32), label).contains(display));
        assert_eq!(h.app.config.selected_setting().unwrap().id, id);
        assert!(!h.app.shutdown_requested());
        if id == "updates.autoInstall" {
            press(&mut h.app, KeyCode::Delete);
            let reset = mutation(
                &mut h,
                id,
                SettingsScope::User,
                "user-r2",
                SettingsOperation::Remove,
                None,
            )
            .await;
            let mut saved = h.app.config.snapshot.clone().unwrap();
            put(&mut saved, SettingsScope::User, id, None, "user-r3");
            effective(&mut saved, id, None, &[]);
            ack(&mut h.app, &reset, saved);
            assert!(row(&screen(&mut h.app, 100, 32), label).contains("Default"));
            assert!(!h.app.shutdown_requested());
        }
        h.app
            .config
            .snapshot
            .as_mut()
            .unwrap()
            .catalog
            .iter_mut()
            .find(|s| s.id == id)
            .unwrap()
            .writable_scopes
            .clear();
        let saved = h.app.config.saved_value(id).cloned();
        for code in [KeyCode::Left, KeyCode::Right, KeyCode::Char(' '), KeyCode::Delete] {
            press(&mut h.app, code);
        }
        assert!(h.commands.try_recv().is_err());
        assert_eq!(h.app.config.saved_value(id), saved.as_ref());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn authentication_loads_profiled_settings_beside_the_usable_local_footer() {
    tokio::task::LocalSet::new()
        .run_until(Box::pin(async {
            let mut h = fixture();
            h.app.config.snapshot = None;
            h.app.session_runtime.session_id = None;
            h.app.status = crate::app::AppStatus::Connecting;
            crate::app::handle_client_event(
                &mut h.app,
                ClientEvent::AuthRequired {
                    method_name: "login".into(),
                    method_description: "Authenticate".into(),
                },
            );
            h.app.input.set_text("/config");
            crate::app::input_submit::submit_input(&mut h.app);
            let rows = screen(&mut h.app, 100, 32);
            assert!(row(&rows, "Footer").contains("Default layout"));
            assert_eq!(h.app.config.selected_setting().unwrap().id, crate::app::footer::SETTING_ID);
            press(&mut h.app, KeyCode::Enter);
            assert!(matches!(
                h.app.config.overlay,
                Some(crate::app::config::ConfigOverlayState::Footer(_))
            ));
            press(&mut h.app, KeyCode::Esc);
            crate::app::handle_client_event(
                &mut h.app,
                ClientEvent::Connected {
                    session_id: "settings-session".into(),
                    cwd: h.scratch.path().to_string_lossy().into_owned(),
                    current_model: crate::agent::model::CurrentModel::new("opus", "Opus", "Opus"),
                    available_models: vec![],
                    mode: None,
                    fast_mode_state: crate::agent::model::FastModeState::Off,
                    fast_mode_disabled_reason: None,
                    ultracode: None,
                    history_updates: vec![],
                },
            );
            let id = inspection(&mut h, "settings-session").await;
            let mut loaded = fixture().app.config.snapshot.unwrap();
            loaded.cwd = h.app.cwd_raw.clone();
            loaded.catalog.retain(|s| ["model", "language"].contains(&s.id.as_str()));
            put(&mut loaded, SettingsScope::User, "language", Some(json!("German")), "user-r1");
            effective(&mut loaded, "language", Some(json!("German")), &["user"]);
            deliver(
                &mut h.app,
                "settings-session",
                Some(&id),
                SettingsPersistence::NotRequested,
                Some(loaded),
                None,
            );
            let rows = screen(&mut h.app, 100, 32);
            assert!(row(&rows, "Language").contains("German"));
            assert_eq!(h.app.config.selected_setting().unwrap().id, "model");
            assert!(row(&rows, "Default model").contains('›'));
            assert!(
                row(&rows, "Footer").contains("Default layout")
                    && !row(&rows, "Footer").contains('›')
            );
        }))
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn local_scalar_save_and_both_reset_routes_show_inherited_or_default_values() {
    for inherited in [Some("English"), None] {
        for editor_reset in [false, true] {
            let mut h = fixture();
            seed(&mut h.app, SettingsScope::Local, "language", json!("German"));
            put(
                h.app.config.snapshot.as_mut().unwrap(),
                SettingsScope::User,
                "language",
                inherited.map(|v| json!(v)),
                "user-r1",
            );
            let initial = h.app.config.snapshot.clone().unwrap();
            crate::app::config::open(&mut h.app).unwrap();
            let request = inspection(&mut h, "settings-session").await;
            deliver(
                &mut h.app,
                "settings-session",
                Some(&request),
                SettingsPersistence::NotRequested,
                Some(initial),
                None,
            );
            select(&mut h.app, "language", SettingsScope::Local);
            press(&mut h.app, KeyCode::Char(' '));
            replace_input(&mut h.app, "Greek");
            press(&mut h.app, KeyCode::Enter);
            let save = mutation(
                &mut h,
                "language",
                SettingsScope::Local,
                "local-r1",
                SettingsOperation::Set,
                Some(json!("Greek")),
            )
            .await;
            assert_eq!(h.app.config.saved_value("language"), Some(&json!("German")));
            let saved = language_snapshot(&h.app, "Greek", "local-r2");
            ack(&mut h.app, &save, saved);
            press(&mut h.app, KeyCode::Char(' '));
            assert_eq!(h.app.config.setting_overlay().unwrap().draft, "Greek");
            if editor_reset {
                control(&mut h.app, 'r');
            } else {
                press(&mut h.app, KeyCode::Esc);
                press(&mut h.app, KeyCode::Delete);
            }
            let reset = mutation(
                &mut h,
                "language",
                SettingsScope::Local,
                "local-r2",
                SettingsOperation::Remove,
                None,
            )
            .await;
            let mut saved = h.app.config.snapshot.clone().unwrap();
            put(&mut saved, SettingsScope::Local, "language", None, "local-r3");
            effective(
                &mut saved,
                "language",
                inherited.map(|s| json!(s)),
                if inherited.is_some() { &["user"] } else { &[] },
            );
            ack(&mut h.app, &reset, saved);
            assert!(
                row(&screen(&mut h.app, 100, 32), "Language")
                    .contains(inherited.unwrap_or("Default"))
            );
            assert_eq!(
                h.app
                    .config
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .scoped("language", SettingsScope::User)
                    .unwrap()
                    .value,
                inherited.map(|s| json!(s))
            );
            press(&mut h.app, KeyCode::Char(' '));
            assert_eq!(h.app.config.setting_overlay().unwrap().draft, inherited.unwrap_or(""));
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn scalar_conflict_keeps_the_edited_greek_draft_while_acknowledging_external_japanese() {
    let mut h = fixture();
    seed(&mut h.app, SettingsScope::Local, "language", json!("German"));
    select(&mut h.app, "language", SettingsScope::Local);
    press(&mut h.app, KeyCode::Char(' '));
    replace_input(&mut h.app, "Greek");
    press(&mut h.app, KeyCode::Enter);
    let first = mutation(
        &mut h,
        "language",
        SettingsScope::Local,
        "local-r1",
        SettingsOperation::Set,
        Some(json!("Greek")),
    )
    .await;
    paste(&mut h.app, "pending-input");
    press(&mut h.app, KeyCode::Backspace);
    press(&mut h.app, KeyCode::Enter);
    assert_eq!(h.app.config.setting_overlay().unwrap().draft, "Greek");
    assert!(h.commands.try_recv().is_err());
    let external = language_snapshot(&h.app, "Japanese", "external-r2");
    deliver(
        &mut h.app,
        "settings-session",
        Some(&first),
        SettingsPersistence::Conflict,
        Some(external),
        Some("External language changed"),
    );
    assert_eq!(h.app.config.saved_value("language"), Some(&json!("Japanese")));
    assert_eq!(h.app.config.setting_overlay().unwrap().draft, "Greek");
    assert!(h.commands.try_recv().is_err());
    assert!(h.app.config.overlay_message.as_ref().unwrap().text.contains("External language"));
    press(&mut h.app, KeyCode::Enter);
    let retry = mutation(
        &mut h,
        "language",
        SettingsScope::Local,
        "external-r2",
        SettingsOperation::Set,
        Some(json!("Greek")),
    )
    .await;
    let saved = language_snapshot(&h.app, "Greek", "local-r3");
    ack(&mut h.app, &retry, saved);
    assert!(h.app.config.overlay.is_none());
    assert_eq!(h.app.config.saved_value("language"), Some(&json!("Greek")));
}

#[tokio::test(flavor = "current_thread")]
async fn saved_default_effort_never_changes_the_current_session_effort() {
    let mut h = fixture();
    seed(&mut h.app, SettingsScope::Project, "defaultEffort", json!("medium"));
    h.app.session_runtime.config_options.insert("effortLevel".into(), json!("max"));
    select(&mut h.app, "defaultEffort", SettingsScope::Project);
    for (code, operation, expected, revision, next_revision) in [
        (KeyCode::Right, SettingsOperation::Set, Some(json!("high")), "project-r1", "project-r2"),
        (KeyCode::Delete, SettingsOperation::Remove, None, "project-r2", "project-r3"),
    ] {
        let before = h.app.config.saved_value("defaultEffort").cloned();
        press(&mut h.app, code);
        let id = mutation(
            &mut h,
            "defaultEffort",
            SettingsScope::Project,
            revision,
            operation,
            expected.clone(),
        )
        .await;
        assert_eq!(h.app.config.saved_value("defaultEffort"), before.as_ref());
        assert_eq!(h.app.session_runtime.config_options.get("effortLevel"), Some(&json!("max")));
        let mut saved = h.app.config.snapshot.clone().unwrap();
        put(&mut saved, SettingsScope::Project, "defaultEffort", expected.clone(), next_revision);
        effective(&mut saved, "defaultEffort", expected.clone(), &["project"]);
        ack(&mut h.app, &id, saved);
        assert_eq!(h.app.config.saved_value("defaultEffort"), expected.as_ref());
        assert_eq!(h.app.session_runtime.config_options.get("effortLevel"), Some(&json!("max")));
        assert!(h.commands.try_recv().is_err(), "no live effort command");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn failed_inspection_admission_preserves_saved_settings_and_allows_explicit_refresh() {
    let mut h = fixture();
    seed(&mut h.app, SettingsScope::User, "language", json!("German"));
    crate::app::config::service::request_settings(&mut h.app);
    let first = inspection(&mut h, "settings-session").await;
    let saved = h.app.config.snapshot.clone().unwrap();
    deliver(
        &mut h.app,
        "settings-session",
        Some(&first),
        SettingsPersistence::NotRequested,
        Some(saved),
        None,
    );
    let (conn, closed) = crate::agent::client::AgentConnection::test_channel();
    drop(closed);
    h.app.session_runtime.conn = Some(std::rc::Rc::new(conn));
    press(&mut h.app, KeyCode::Char('r'));
    assert!(h.app.config.pending_settings_request.is_none());
    assert!(!h.app.config.last_error.as_ref().unwrap().is_empty());
    assert_eq!(h.app.config.saved_value("language"), Some(&json!("German")));
    assert!(row(&screen(&mut h.app, 100, 32), "Footer").contains("Default layout"));
    reconnect(&mut h);
    assert!(h.commands.try_recv().is_err());
    press(&mut h.app, KeyCode::Char('r'));
    let id = inspection(&mut h, "settings-session").await;
    assert!(h.commands.try_recv().is_err());
    let mut refreshed = h.app.config.snapshot.clone().unwrap();
    put(&mut refreshed, SettingsScope::User, "language", Some(json!("French")), "user-r2");
    effective(&mut refreshed, "language", Some(json!("French")), &["user"]);
    deliver(
        &mut h.app,
        "settings-session",
        Some(&id),
        SettingsPersistence::NotRequested,
        Some(refreshed),
        None,
    );
    assert!(h.app.config.last_error.is_none());
    assert!(row(&screen(&mut h.app, 100, 32), "Language").contains("French"));
    press(&mut h.app, KeyCode::Char(' '));
    assert_eq!(h.app.config.setting_overlay().unwrap().draft, "French");
}

#[tokio::test(flavor = "current_thread")]
async fn failed_mutation_admission_retains_the_draft_and_can_retry_on_a_live_connection() {
    let mut h = fixture();
    seed(&mut h.app, SettingsScope::Local, "language", json!("German"));
    select(&mut h.app, "language", SettingsScope::Local);
    press(&mut h.app, KeyCode::Char(' '));
    replace_input(&mut h.app, "Greek");
    let (conn, closed) = crate::agent::client::AgentConnection::test_channel();
    drop(closed);
    h.app.session_runtime.conn = Some(std::rc::Rc::new(conn));
    press(&mut h.app, KeyCode::Enter);
    assert!(h.app.config.pending_settings_request.is_none());
    assert_eq!(h.app.config.setting_overlay().unwrap().draft, "Greek");
    assert!(!h.app.config.overlay_message.as_ref().unwrap().text.is_empty());
    assert_eq!(h.app.config.saved_value("language"), Some(&json!("German")));
    reconnect(&mut h);
    assert!(h.commands.try_recv().is_err());
    press(&mut h.app, KeyCode::Enter);
    let id = mutation(
        &mut h,
        "language",
        SettingsScope::Local,
        "local-r1",
        SettingsOperation::Set,
        Some(json!("Greek")),
    )
    .await;
    let saved = language_snapshot(&h.app, "Greek", "local-r2");
    ack(&mut h.app, &id, saved);
    assert!(h.app.config.overlay.is_none());
    assert!(row(&screen(&mut h.app, 100, 32), "Language").contains("Greek"));
    press(&mut h.app, KeyCode::Char(' '));
    replace_input(&mut h.app, "editable after recovery");
    assert_eq!(h.app.config.setting_overlay().unwrap().draft, "editable after recovery");
}

#[tokio::test(flavor = "current_thread")]
async fn session_replacement_keeps_an_open_scalar_draft_until_the_user_cancels() {
    tokio::task::LocalSet::new()
        .run_until(Box::pin(async {
            let mut h = fixture();
            seed(&mut h.app, SettingsScope::Local, "language", json!("German"));
            select(&mut h.app, "language", SettingsScope::Local);
            press(&mut h.app, KeyCode::Char(' '));
            replace_input(&mut h.app, "inspectable after replacement");
            let next = tempfile::tempdir().unwrap();
            replace_session(&mut h.app, next.path());
            assert_eq!(
                h.app.config.setting_overlay().unwrap().draft,
                "inspectable after replacement"
            );
            assert!(
                editor_body(&screen(&mut h.app, 100, 35)).contains("Settings location changed")
            );
            let request = inspection(&mut h, "replacement-session").await;
            let mut fresh = fixture().app.config.snapshot.unwrap();
            fresh.cwd = h.app.cwd_raw.clone();
            fresh.context = "replacement-context".into();
            deliver(
                &mut h.app,
                "replacement-session",
                Some(&request),
                SettingsPersistence::NotRequested,
                Some(fresh),
                None,
            );
            assert_eq!(
                h.app.config.setting_overlay().unwrap().draft,
                "inspectable after replacement"
            );
            press(&mut h.app, KeyCode::Esc);
            assert!(h.app.config.overlay.is_none());
        }))
        .await;
}
