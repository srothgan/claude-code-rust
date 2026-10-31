// SPDX-License-Identifier: Apache-2.0
use super::{choose, field};
use crate::app::config::{ConfigTab, SettingsFocus, settings_audit_support::*};

#[test]
fn metadata_associates_source_target_default_and_acknowledged_session_with_the_selected_row() {
    let mut h = fixture();
    let rows = screen(&mut h.app, 100, 32);
    assert!(row(&rows, "Language").contains("Default"));
    assert!(details(&rows).contains("Using Default"));
    seed(&mut h.app, SettingsScope::User, "language", json!("German"));
    let rows = screen(&mut h.app, 100, 32);
    assert!(row(&rows, "Language").contains("German"));
    assert!(details(&rows).contains("Saved in User"));
    assert!(!details(&rows).contains("German"));
    press(&mut h.app, KeyCode::Char('s'));
    let rows = screen(&mut h.app, 100, 32);
    assert!(row(&rows, "Save in:").contains("Project"));
    assert!(row(&rows, "Language").contains("German"));
    assert!(details(&rows).contains("From User"));
    assert!(details(&rows).contains("Not set in Project"));
    seed(&mut h.app, SettingsScope::Project, "language", json!("English"));
    select(&mut h.app, "language", SettingsScope::User);
    let rows = screen(&mut h.app, 100, 32);
    assert!(row(&rows, "Language").contains("English"));
    assert!(details(&rows).contains("User value: German"));
    assert!(details(&rows).contains("Overridden by Project"));
    assert!(!details(&rows).contains("English"));
    seed(&mut h.app, SettingsScope::User, "spinnerTipsEnabled", json!(true));
    select(&mut h.app, "spinnerTipsEnabled", SettingsScope::User);
    let rows = screen(&mut h.app, 100, 32);
    assert!(row(&rows, "Show tips").contains("On"));
    assert!(details(&rows).contains("Applies immediately"));
    seed(&mut h.app, SettingsScope::User, "model", json!("sonnet"));
    select(&mut h.app, "model", SettingsScope::User);
    for (model, label, differs) in [("sonnet", "Sonnet", false), ("opus", "Opus", true)] {
        h.app.session_runtime.current_model = Some(
            crate::agent::model::CurrentModel::new(model, label, label)
                .catalog_id(model)
                .requested_id("sonnet")
                .authoritative(true),
        );
        let rows = screen(&mut h.app, 100, 32);
        assert!(row(&rows, "Default model").contains("sonnet"));
        let info = details(&rows);
        assert_eq!(info.contains("Current session:"), differs, "{info}");
        if differs {
            assert!(info.contains("Current session: Opus"));
        }
    }
}

#[test]
fn collection_details_show_other_contributors_and_the_selected_source_warning_without_color_dependencies()
 {
    let mut h = fixture();
    seed(&mut h.app, SettingsScope::User, "permissions.allow", json!(["Read(./src/**)"]));
    let snapshot = h.app.config.snapshot.as_mut().unwrap();
    put(
        snapshot,
        SettingsScope::Project,
        "permissions.allow",
        Some(json!(["Read(./docs/**)"])),
        "project-r1",
    );
    effective(
        snapshot,
        "permissions.allow",
        Some(json!(["Read(./src/**)", "Read(./docs/**)"])),
        &["user", "project"],
    );
    select(&mut h.app, "permissions.allow", SettingsScope::User);
    let rows = screen(&mut h.app, 100, 32);
    assert!(row(&rows, "Allow rules").contains("1 item"));
    assert!(details(&rows).contains("Saved in User"));
    assert!(details(&rows).contains("Also supplied by Project"));
    let snapshot = h.app.config.snapshot.as_mut().unwrap();
    snapshot.values.iter_mut().find(|v| v.id == "permissions.allow").unwrap().policy_restricted =
        true;
    snapshot
        .catalog
        .iter_mut()
        .find(|s| s.id == "permissions.allow")
        .unwrap()
        .writable_scopes
        .clear();
    let error = "SOURCE_START: correct this scoped file before loading. SOURCE_END";
    snapshot.sources.iter_mut().find(|s| s.scope == SettingsScope::User).unwrap().error =
        Some(error.into());
    snapshot.sources.iter_mut().find(|s| s.scope == SettingsScope::Project).unwrap().error =
        Some("OTHER_SOURCE_ERROR".into());
    let rows = screen(&mut h.app, 40, 24);
    let start = rows.iter().position(|r| r.contains("SOURCE_START:")).unwrap();
    let warning = rows[start..].join(" ").split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(warning.contains(error), "{warning}");
    assert!(warning.contains("Controlled by your organization"), "{warning}");
    assert!(!warning.contains("OTHER_SOURCE_ERROR"));
}

#[tokio::test(flavor = "current_thread")]
async fn unsupported_hooks_can_be_corrected_in_json_or_cancelled_without_changing_the_source() {
    for cancel in [true, false] {
        let mut h = fixture();
        seed(
            &mut h.app,
            SettingsScope::Project,
            "hooks",
            json!("externally authored unsupported shape"),
        );
        select(&mut h.app, "hooks", SettingsScope::Project);
        press(&mut h.app, KeyCode::Enter);
        assert_eq!(draft(&h.app), json!("externally authored unsupported shape"));
        press(&mut h.app, KeyCode::Esc);
        assert!(h.commands.try_recv().is_err());
        press(&mut h.app, KeyCode::Enter);
        control(&mut h.app, 'j');
        assert!(h.app.config.setting_overlay().unwrap().structured.as_ref().unwrap().advanced);
        let corrected = json!({"Stop":[{"hooks":[{"type":"command","command":"verify-correction","future":true}]}]});
        replace_input(&mut h.app, &corrected.to_string());
        assert_eq!(draft(&h.app), corrected);
        if cancel {
            press(&mut h.app, KeyCode::Esc);
            assert!(h.app.config.overlay.is_none());
            assert!(h.commands.try_recv().is_err());
            assert_eq!(
                h.app.config.saved_value("hooks"),
                Some(&json!("externally authored unsupported shape"))
            );
        } else {
            control(&mut h.app, 'f');
            assert!(!h.app.config.setting_overlay().unwrap().structured.as_ref().unwrap().advanced);
            control(&mut h.app, 's');
            mutation(
                &mut h,
                "hooks",
                SettingsScope::Project,
                "project-r1",
                SettingsOperation::Set,
                Some(corrected),
            )
            .await;
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn pane_local_search_refuses_missing_targets_then_recovers_a_visible_editable_match() {
    let mut h = fixture();
    seed(&mut h.app, SettingsScope::User, "sandbox.ripgrep", json!({"command":"fixture-rg"}));
    h.app.config.selected_scope = SettingsScope::User;
    press(&mut h.app, KeyCode::Char('/'));
    paste(&mut h.app, "sandbox");
    assert!(h.app.config.selected_setting().is_none());
    assert!(row(&screen(&mut h.app, 80, 24), "No matching settings").contains("this pane"));
    for code in [KeyCode::Char(' '), KeyCode::Delete, KeyCode::Down, KeyCode::Enter] {
        press(&mut h.app, code);
    }
    assert_eq!(h.app.config.settings.focus, SettingsFocus::Search);
    assert!(h.app.config.selected_setting().is_none());
    assert!(h.commands.try_recv().is_err());
    press(&mut h.app, KeyCode::Esc);
    press(&mut h.app, KeyCode::Home);
    press(&mut h.app, KeyCode::Up);
    press(&mut h.app, KeyCode::Up);
    assert_eq!(h.app.config.settings.focus, SettingsFocus::CategoryTabs);
    for _ in 0..h.app.config.snapshot.as_ref().unwrap().catalog.len() {
        if h.app.config.settings.category == "sandbox" {
            break;
        }
        press(&mut h.app, KeyCode::Right);
    }
    assert_eq!(h.app.config.settings.category, "sandbox");
    press(&mut h.app, KeyCode::Down);
    paste(&mut h.app, "RIPGREP command");
    press(&mut h.app, KeyCode::Down);
    assert_eq!(h.app.config.selected_setting().unwrap().id, "sandbox.ripgrep");
    assert_eq!(h.app.config.settings.focus, SettingsFocus::Content);
    press(&mut h.app, KeyCode::Enter);
    assert!(h.app.config.settings.query.is_empty());
    for (width, height) in [(100, 32), (42, 24)] {
        let rows = screen(&mut h.app, width, height);
        assert!(row(&rows, "Ripgrep command").contains('›'));
        assert!(row(&rows, "Save in:").contains("User"));
        assert!(rows.iter().any(|r| r.contains("Space edit")), "{}", rows.join("\n"));
    }
    press(&mut h.app, KeyCode::Enter);
    field(&mut h.app, "Command");
    replace_input(&mut h.app, "recovered-rg");
    press(&mut h.app, KeyCode::Enter);
    control(&mut h.app, 's');
    mutation(
        &mut h,
        "sandbox.ripgrep",
        SettingsScope::User,
        "user-r1",
        SettingsOperation::Set,
        Some(json!({"command":"recovered-rg"})),
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn hook_form_and_json_edits_share_one_payload_and_preserve_unrelated_fields() {
    let mut h = fixture();
    let original = json!({"PreToolUse":[{"matcher":"Write|Edit","future":"group","hooks":[{"type":"command","command":"old","args":["--keep"],"future":"handler"},{"type":"http","url":"https://example.com/old"}]}],"Stop":[{"hooks":[{"type":"prompt","prompt":"Check completion"}]}]});
    seed(&mut h.app, SettingsScope::Local, "hooks", original);
    select(&mut h.app, "hooks", SettingsScope::Local);
    press(&mut h.app, KeyCode::Enter);
    press(&mut h.app, KeyCode::Enter);
    field(&mut h.app, "Command");
    replace_input(&mut h.app, "new-command\r\nsecond line ä");
    press(&mut h.app, KeyCode::Enter);
    control(&mut h.app, 'j');
    assert_eq!(
        draft(&h.app)["PreToolUse"][0]["hooks"][0]["command"],
        json!("new-command\nsecond line ä")
    );
    // Change only the independent sibling URL in the actual JSON input, preserving the form edit.
    let edited = h
        .app
        .config
        .setting_overlay()
        .unwrap()
        .draft
        .replace("https://example.com/old", "https://example.com/new");
    replace_input(&mut h.app, &edited);
    control(&mut h.app, 'f');
    press(&mut h.app, KeyCode::Enter);
    field(&mut h.app, "Command");
    assert_eq!(
        h.app
            .config
            .setting_overlay()
            .unwrap()
            .structured
            .as_ref()
            .unwrap()
            .input
            .as_ref()
            .unwrap()
            .draft,
        "new-command\nsecond line ä"
    );
    press(&mut h.app, KeyCode::Esc);
    press(&mut h.app, KeyCode::Esc);
    press(&mut h.app, KeyCode::Down);
    press(&mut h.app, KeyCode::Enter);
    field(&mut h.app, "Url");
    assert_eq!(
        h.app
            .config
            .setting_overlay()
            .unwrap()
            .structured
            .as_ref()
            .unwrap()
            .input
            .as_ref()
            .unwrap()
            .draft,
        "https://example.com/new"
    );
    press(&mut h.app, KeyCode::Enter);
    control(&mut h.app, 's');
    mutation(&mut h,"hooks",SettingsScope::Local,"local-r1",SettingsOperation::Set,Some(json!({"PreToolUse":[{"matcher":"Write|Edit","future":"group","hooks":[{"type":"command","command":"new-command\nsecond line ä","args":["--keep"],"future":"handler"},{"type":"http","url":"https://example.com/new"}]}],"Stop":[{"hooks":[{"type":"prompt","prompt":"Check completion"}]}]}))).await;
}

#[tokio::test(flavor = "current_thread")]
async fn idle_compaction_saves_and_resets_only_the_project_preference_without_live_commands() {
    let mut h = fixture();
    seed(&mut h.app, SettingsScope::User, "idleCompaction", json!(false));
    seed(&mut h.app, SettingsScope::Project, "idleCompaction", json!(false));
    select(&mut h.app, "idleCompaction", SettingsScope::Project);
    let rows = screen(&mut h.app, 100, 32);
    assert!(row(&rows, "Idle compaction").contains("Off"));
    let info = details(&rows);
    assert!(info.contains("does not enable") && info.contains("next session"), "{info}");
    for (key, operation, value, revision, next_revision, display) in [
        (
            KeyCode::Right,
            SettingsOperation::Set,
            Some(json!(true)),
            "project-r1",
            "project-r2",
            "On",
        ),
        (KeyCode::Delete, SettingsOperation::Remove, None, "project-r2", "project-r3", "Off"),
    ] {
        press(&mut h.app, key);
        let id = mutation(
            &mut h,
            "idleCompaction",
            SettingsScope::Project,
            revision,
            operation,
            value.clone(),
        )
        .await;
        let mut saved = h.app.config.snapshot.clone().unwrap();
        put(&mut saved, SettingsScope::Project, "idleCompaction", value.clone(), next_revision);
        let contributor = if value.is_some() { "project" } else { "user" };
        effective(
            &mut saved,
            "idleCompaction",
            Some(value.unwrap_or(json!(false))),
            &[contributor],
        );
        ack(&mut h.app, &id, saved);
        assert!(row(&screen(&mut h.app, 100, 32), "Idle compaction").contains(display));
        assert_eq!(
            h.app
                .config
                .snapshot
                .as_ref()
                .unwrap()
                .scoped("idleCompaction", SettingsScope::User)
                .unwrap()
                .value,
            Some(json!(false))
        );
        assert!(h.commands.try_recv().is_err(), "no command enabling live idle compaction");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn rule_item_input_edit_cancel_and_remove_save_only_the_selected_collection() {
    let mut h = fixture();
    seed(&mut h.app, SettingsScope::User, "permissions.allow", json!(["Read(./user-only/**)"]));
    seed(
        &mut h.app,
        SettingsScope::Project,
        "permissions.allow",
        json!(["Read(./docs/**)", "Bash(npm test)"]),
    );
    seed(&mut h.app, SettingsScope::Project, "permissions.deny", json!(["Read(./.env)"]));
    select(&mut h.app, "permissions.allow", SettingsScope::Project);
    press(&mut h.app, KeyCode::Enter);
    press(&mut h.app, KeyCode::Enter);
    assert_eq!(
        h.app
            .config
            .setting_overlay()
            .unwrap()
            .structured
            .as_ref()
            .unwrap()
            .input
            .as_ref()
            .unwrap()
            .draft,
        "Read(./docs/**)"
    );
    let rows = screen(&mut h.app, 100, 35);
    assert!(editor_body(&rows).contains("Read(./docs/**)"));
    replace_input(&mut h.app, "Read(./src/**)");
    press(&mut h.app, KeyCode::Enter);
    press(&mut h.app, KeyCode::Down);
    press(&mut h.app, KeyCode::Enter);
    replace_input(&mut h.app, "discarded-rule");
    press(&mut h.app, KeyCode::Esc);
    press(&mut h.app, KeyCode::Delete);
    assert_eq!(draft(&h.app), json!(["Read(./src/**)"]));
    assert!(h.commands.try_recv().is_err());
    control(&mut h.app, 's');
    mutation(
        &mut h,
        "permissions.allow",
        SettingsScope::Project,
        "project-r1",
        SettingsOperation::Set,
        Some(json!(["Read(./src/**)"])),
    )
    .await;
    let snapshot = h.app.config.snapshot.as_ref().unwrap();
    assert_eq!(
        snapshot.scoped("permissions.deny", SettingsScope::Project).unwrap().value,
        Some(json!(["Read(./.env)"]))
    );
    assert_eq!(
        snapshot.scoped("permissions.allow", SettingsScope::User).unwrap().value,
        Some(json!(["Read(./user-only/**)"]))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn credential_fields_require_name_and_mode_and_preserve_external_choices_until_replaced() {
    let mut h = fixture();
    select(&mut h.app, "sandbox.credentials", SettingsScope::User);
    press(&mut h.app, KeyCode::Enter);
    field(&mut h.app, "Environment variables");
    press(&mut h.app, KeyCode::Char('a'));
    let rows = screen(&mut h.app, 100, 35);
    assert!(row(&rows, "Variable name").contains('*'));
    assert!(row(&rows, "Protection mode").contains('*'));
    field(&mut h.app, "Variable name");
    assert!(editor_body(&screen(&mut h.app, 100, 35)).contains("secret value"));
    paste(&mut h.app, "API_TOKEN");
    press(&mut h.app, KeyCode::Enter);
    field(&mut h.app, "Protection mode");
    let rows = screen(&mut h.app, 100, 35);
    assert!(editor_body(&rows).contains("deny") && editor_body(&rows).contains("mask"));
    choose(&mut h.app, "mask");
    assert!(row(&screen(&mut h.app, 100, 35), "Protection mode").contains("mask"));
    control(&mut h.app, 's');
    mutation(
        &mut h,
        "sandbox.credentials",
        SettingsScope::User,
        "user-r1",
        SettingsOperation::Set,
        Some(json!({"envVars":[{"name":"API_TOKEN","mode":"mask"}]})),
    )
    .await;
    for cancel in [true, false] {
        let mut h = fixture();
        let original = json!({"envVars":[{"name":"API_TOKEN","mode":"external-value","future":true}],"future":"keep"});
        seed(&mut h.app, SettingsScope::User, "sandbox.credentials", original.clone());
        select(&mut h.app, "sandbox.credentials", SettingsScope::User);
        press(&mut h.app, KeyCode::Enter);
        field(&mut h.app, "Environment variables");
        press(&mut h.app, KeyCode::Enter);
        field(&mut h.app, "Protection mode");
        assert!(editor_body(&screen(&mut h.app, 100, 35)).contains("Current: external-value"));
        assert_eq!(draft(&h.app), original);
        if cancel {
            press(&mut h.app, KeyCode::Esc);
            assert_eq!(draft(&h.app), original);
            assert_eq!(h.app.config.saved_value("sandbox.credentials"), Some(&original));
            assert!(h.commands.try_recv().is_err());
        } else {
            press(&mut h.app, KeyCode::Down);
            press(&mut h.app, KeyCode::Enter);
            control(&mut h.app, 's');
            mutation(&mut h,"sandbox.credentials",SettingsScope::User,"user-r1",SettingsOperation::Set,Some(json!({"envVars":[{"name":"API_TOKEN","mode":"deny","future":true}],"future":"keep"}))).await;
        }
    }
}

fn edit_ripgrep(app: &mut App, command: &str) {
    press(app, KeyCode::Enter);
    field(app, "Command");
    replace_input(app, command);
    press(app, KeyCode::Enter);
}

#[tokio::test(flavor = "current_thread")]
async fn sandbox_save_preserves_unknown_fields_and_recovers_after_a_real_session_replacement() {
    tokio::task::LocalSet::new().run_until(Box::pin(async {
        for replace in [false,true] {
            let mut h=fixture();seed(&mut h.app,SettingsScope::User,"sandbox.ripgrep",json!({"command":"rg","args":["--hidden"],"future":{"keep":true}}));select(&mut h.app,"sandbox.ripgrep",SettingsScope::User);
            edit_ripgrep(&mut h.app,"custom-rg");let expected=json!({"command":"custom-rg","args":["--hidden"],"future":{"keep":true}});assert_eq!(draft(&h.app),expected);
            if !replace {control(&mut h.app,'s');mutation(&mut h,"sandbox.ripgrep",SettingsScope::User,"user-r1",SettingsOperation::Set,Some(expected)).await;continue;}
            let mut fresh=h.app.config.snapshot.clone().unwrap();let other=tempfile::tempdir().unwrap();replace_session(&mut h.app,other.path());
            assert_eq!(draft(&h.app),expected);assert!(editor_body(&screen(&mut h.app,100,35)).contains("Settings location changed"));let request=inspection(&mut h,"replacement-session").await;
            while let Ok(command)=h.commands.try_recv() { assert!(!matches!(command.command,BridgeCommand::MutateSetting{..})); }
            fresh.cwd=other.path().to_string_lossy().into_owned();fresh.context="replacement-context".into();put(&mut fresh,SettingsScope::User,"sandbox.ripgrep",Some(json!({"command":"fresh-rg","args":["--fresh"],"future":"new-project"})),"fresh-r4");effective(&mut fresh,"sandbox.ripgrep",Some(json!({"command":"fresh-rg","args":["--fresh"],"future":"new-project"})),&["user"]);
            deliver(&mut h.app,"replacement-session",Some(&request),SettingsPersistence::NotRequested,Some(fresh),None);
            control(&mut h.app,'s');assert!(h.app.config.overlay_message.as_ref().unwrap().text.contains("location changed"));assert!(h.commands.try_recv().is_err());assert_eq!(draft(&h.app),expected);
            press(&mut h.app,KeyCode::Esc);select(&mut h.app,"sandbox.ripgrep",SettingsScope::User);edit_ripgrep(&mut h.app,"recovered-rg");control(&mut h.app,'s');
            let command=receive(&mut h).await;assert_target(&command,"replacement-session",&h.profile,&SettingsMutation {context:"replacement-context".into(),id:"sandbox.ripgrep".into(),scope:SettingsScope::User,expected_revision:"fresh-r4".into(),operation:SettingsOperation::Set,value:Some(json!({"command":"recovered-rg","args":["--fresh"],"future":"new-project"}))});assert!(h.commands.try_recv().is_err());
        }
    })).await;
}

#[tokio::test(flavor = "current_thread")]
async fn refresh_reordering_and_removal_preserve_valid_setting_identity_and_local_scope() {
    let mut h = fixture();
    seed(&mut h.app, SettingsScope::Local, "permissions.allow", json!(["Read(./src/**)"]));
    let catalog = &mut h.app.config.snapshot.as_mut().unwrap().catalog;
    catalog.retain(|s| ["permissions.allow", "permissions.deny"].contains(&s.id.as_str()));
    for setting in catalog.iter_mut() {
        setting.description = "Fixture tool configuration".into();
    }
    catalog.sort_by_key(|s| s.id != "permissions.allow");
    select(&mut h.app, "permissions.allow", SettingsScope::Local);
    press(&mut h.app, KeyCode::Char('/'));
    paste(&mut h.app, "Fixture tool");
    press(&mut h.app, KeyCode::Down);
    assert_eq!(h.app.config.settings.items(h.app.config.snapshot.as_ref().unwrap()).len(), 2);
    press(&mut h.app, KeyCode::Char('r'));
    let request = inspection(&mut h, "settings-session").await;
    let mut refreshed = h.app.config.snapshot.clone().unwrap();
    refreshed.catalog.reverse();
    deliver(
        &mut h.app,
        "settings-session",
        Some(&request),
        SettingsPersistence::NotRequested,
        Some(refreshed),
        None,
    );
    assert_eq!(h.app.config.selected_setting().unwrap().id, "permissions.allow");
    press(&mut h.app, KeyCode::BackTab);
    assert_eq!(h.app.config.active_tab, ConfigTab::Help);
    press(&mut h.app, KeyCode::Tab);
    let tab_request = inspection(&mut h, "settings-session").await;
    let unchanged = h.app.config.snapshot.clone().unwrap();
    deliver(
        &mut h.app,
        "settings-session",
        Some(&tab_request),
        SettingsPersistence::NotRequested,
        Some(unchanged),
        None,
    );
    press(&mut h.app, KeyCode::Esc);
    assert_eq!(h.app.config.selected_setting().unwrap().id, "permissions.allow");
    assert_eq!(h.app.config.selected_scope, SettingsScope::Local);
    press(&mut h.app, KeyCode::Enter);
    press(&mut h.app, KeyCode::Enter);
    replace_input(&mut h.app, "Read(./selected/**)");
    press(&mut h.app, KeyCode::Enter);
    control(&mut h.app, 's');
    let saved = mutation(
        &mut h,
        "permissions.allow",
        SettingsScope::Local,
        "local-r1",
        SettingsOperation::Set,
        Some(json!(["Read(./selected/**)"])),
    )
    .await;
    let mut snapshot = h.app.config.snapshot.clone().unwrap();
    put(
        &mut snapshot,
        SettingsScope::Local,
        "permissions.allow",
        Some(json!(["Read(./selected/**)"])),
        "local-r2",
    );
    effective(&mut snapshot, "permissions.allow", Some(json!(["Read(./selected/**)"])), &["local"]);
    ack(&mut h.app, &saved, snapshot);
    press(&mut h.app, KeyCode::Char('r'));
    let request = inspection(&mut h, "settings-session").await;
    let mut snapshot = h.app.config.snapshot.clone().unwrap();
    snapshot.catalog.retain(|s| s.id != "permissions.allow");
    deliver(
        &mut h.app,
        "settings-session",
        Some(&request),
        SettingsPersistence::NotRequested,
        Some(snapshot),
        None,
    );
    assert_eq!(h.app.config.selected_setting().unwrap().id, "permissions.deny");
    assert_eq!(h.app.config.selected_scope, SettingsScope::Local);
    press(&mut h.app, KeyCode::Enter);
    control(&mut h.app, 'j');
    replace_input(&mut h.app, r#"["Read(./.env)"]"#);
    control(&mut h.app, 's');
    mutation(
        &mut h,
        "permissions.deny",
        SettingsScope::Local,
        "local-r1",
        SettingsOperation::Set,
        Some(json!(["Read(./.env)"])),
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn explicit_empty_list_differs_from_reset_and_read_only_inspection_never_mutates() {
    for action in ["cancel", "empty", "reset", "readonly"] {
        let mut h = fixture();
        let original = json!(["Read(./src/**)"]);
        seed(&mut h.app, SettingsScope::Project, "permissions.allow", original.clone());
        select(&mut h.app, "permissions.allow", SettingsScope::Project);
        if action == "readonly" {
            let setting = h
                .app
                .config
                .snapshot
                .as_mut()
                .unwrap()
                .catalog
                .iter_mut()
                .find(|s| s.id == "permissions.allow")
                .unwrap();
            setting.writable_scopes.clear();
            setting.unavailable = Some("Managed policy".into());
        }
        press(&mut h.app, KeyCode::Enter);
        press(&mut h.app, KeyCode::Delete);
        match action {
            "cancel" => {
                assert_eq!(draft(&h.app), json!([]));
                press(&mut h.app, KeyCode::Esc);
                assert_eq!(h.app.config.saved_value("permissions.allow"), Some(&original));
                assert!(h.commands.try_recv().is_err());
            }
            "empty" => {
                control(&mut h.app, 's');
                mutation(
                    &mut h,
                    "permissions.allow",
                    SettingsScope::Project,
                    "project-r1",
                    SettingsOperation::Set,
                    Some(json!([])),
                )
                .await;
            }
            "reset" => {
                control(&mut h.app, 'r');
                mutation(
                    &mut h,
                    "permissions.allow",
                    SettingsScope::Project,
                    "project-r1",
                    SettingsOperation::Remove,
                    None,
                )
                .await;
            }
            _ => {
                press(&mut h.app, KeyCode::Char('a'));
                press(&mut h.app, KeyCode::Enter);
                paste(&mut h.app, "forbidden");
                control(&mut h.app, 'u');
                control(&mut h.app, 'r');
                control(&mut h.app, 's');
                assert_eq!(draft(&h.app), original);
                assert!(editor_body(&screen(&mut h.app, 100, 35)).contains("Read-only"));
                control(&mut h.app, 'j');
                paste(&mut h.app, "forbidden json");
                press(&mut h.app, KeyCode::Delete);
                control(&mut h.app, 'u');
                control(&mut h.app, 'r');
                control(&mut h.app, 's');
                press(&mut h.app, KeyCode::Home);
                assert_eq!(draft(&h.app), original);
                assert_eq!(h.app.config.saved_value("permissions.allow"), Some(&original));
                assert!(h.commands.try_recv().is_err());
                control(&mut h.app, 'f');
                press(&mut h.app, KeyCode::Esc);
                assert!(h.app.config.overlay.is_none());
            }
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn hook_backtracking_preserves_completed_input_but_type_replacement_discards_abandoned_fields()
 {
    for change_type in [false, true] {
        let mut h = fixture();
        let original = json!({"Stop":[{"future":"group","hooks":[{"type":"command","command":"keep-original","future":"handler"}]}]});
        seed(&mut h.app, SettingsScope::Project, "hooks", original.clone());
        select(&mut h.app, "hooks", SettingsScope::Project);
        press(&mut h.app, KeyCode::Enter);
        press(&mut h.app, KeyCode::Char('a'));
        choose(&mut h.app, "PreToolUse");
        paste(&mut h.app, "Write|Edit");
        press(&mut h.app, KeyCode::Enter);
        choose(&mut h.app, "command");
        paste(&mut h.app, "abandoned-command --distinctive");
        press(&mut h.app, KeyCode::Enter);
        press(&mut h.app, KeyCode::Esc);
        assert_eq!(
            h.app
                .config
                .setting_overlay()
                .unwrap()
                .structured
                .as_ref()
                .unwrap()
                .hook_creation
                .as_ref()
                .unwrap()
                .input
                .as_ref()
                .unwrap()
                .draft,
            "abandoned-command --distinctive"
        );
        replace_input(&mut h.app, "corrected-command");
        press(&mut h.app, KeyCode::Enter);
        if change_type {
            press(&mut h.app, KeyCode::Esc);
            press(&mut h.app, KeyCode::Esc);
            choose(&mut h.app, "http");
            assert!(
                h.app
                    .config
                    .setting_overlay()
                    .unwrap()
                    .structured
                    .as_ref()
                    .unwrap()
                    .hook_creation
                    .as_ref()
                    .unwrap()
                    .input
                    .as_ref()
                    .unwrap()
                    .draft
                    .is_empty()
            );
            paste(&mut h.app, "https://example.com/hook");
            press(&mut h.app, KeyCode::Enter);
        }
        assert_eq!(draft(&h.app), original);
        control(&mut h.app, 's');
        assert!(h.commands.try_recv().is_err());
        assert_eq!(draft(&h.app), original);
        press(&mut h.app, KeyCode::Enter);
        let handler = if change_type {
            json!({"type":"http","url":"https://example.com/hook"})
        } else {
            json!({"type":"command","command":"corrected-command"})
        };
        let expected = json!({"Stop":[{"future":"group","hooks":[{"type":"command","command":"keep-original","future":"handler"}]}],"PreToolUse":[{"matcher":"Write|Edit","hooks":[handler]}]});
        assert_eq!(draft(&h.app), expected);
        control(&mut h.app, 's');
        mutation(
            &mut h,
            "hooks",
            SettingsScope::Project,
            "project-r1",
            SettingsOperation::Set,
            Some(expected),
        )
        .await;
    }
}
