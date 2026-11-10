// SPDX-License-Identifier: Apache-2.0
use crate::agent::model;
use crate::app::config::{ConfigHelpSection, ConfigTab, SettingsFocus, settings_audit_support::*};
use crate::app::{AppStatus, SurfaceMode, test_support};

async fn inspection_only(h: &mut Harness, session: &str) -> String {
    let envelope = receive(h).await;
    assert!(
        matches!(envelope.command, BridgeCommand::InspectSettings { session_id, app_settings_path }
        if session_id == session && app_settings_path.as_deref() == Some(h.profile.as_str()))
    );
    assert!(h.commands.try_recv().is_err(), "opening settings must not send another command");
    envelope.request_id.unwrap()
}

async fn open_settings(h: &mut Harness) {
    let incoming = h.app.config.snapshot.clone().unwrap();
    h.app.surface_mode = SurfaceMode::Chat;
    crate::app::config::open(&mut h.app).unwrap();
    let id = inspection_only(h, "settings-session").await;
    deliver(
        &mut h.app,
        "settings-session",
        Some(&id),
        SettingsPersistence::NotRequested,
        Some(incoming),
        None,
    );
    select(&mut h.app, "language", SettingsScope::User);
    assert!(h.commands.try_recv().is_err());
}

fn update(app: &mut App, update: model::SessionUpdate) {
    crate::app::handle_client_event(
        app,
        ClientEvent::SessionUpdate { session_id: "settings-session".into(), update },
    );
}

#[tokio::test(flavor = "current_thread")]
async fn opening_config_during_a_real_turn_keeps_streaming_and_completion_intact() {
    let mut h = fixture();
    h.app.status = AppStatus::Ready;
    h.app.surface_mode = SurfaceMode::Chat;
    update(&mut h.app, model::SessionUpdate::AgentResponseStarted);
    update(&mut h.app, test_support::source_chunk("Before config. ", "reply", false));
    let assistant = h.app.active_turn_assistant_idx().unwrap();
    open_settings(&mut h).await;
    assert!(h.commands.try_recv().is_err(), "opening must not interrupt the turn");
    update(&mut h.app, test_support::source_chunk("While config is open.", "reply", false));
    crate::app::handle_client_event(
        &mut h.app,
        ClientEvent::TurnComplete {
            session_id: "settings-session".into(),
            queued_turn_count: None,
            terminal_reason: None,
        },
    );
    assert_eq!(h.app.status, AppStatus::Ready);
    assert_eq!(h.app.surface_mode, SurfaceMode::Fullscreen(crate::app::FullscreenView::Config));
    let transcript = test_support::transcript_region(&mut h.app, assistant);
    assert!(transcript.contains("Before config. While config is open."), "{transcript}");
    test_support::drain_session_refreshes(&mut h.commands, "settings-session");
    press(&mut h.app, KeyCode::Esc);
    assert_eq!(h.app.surface_mode, SurfaceMode::Chat);
}

#[tokio::test(flavor = "current_thread")]
async fn config_close_keys_preserve_the_chat_draft_and_only_explicit_submission_sends_it() {
    for close in [KeyCode::Enter, KeyCode::Esc] {
        let clock = test_support::EventClock::new();
        let mut h = fixture();
        h.app.status = AppStatus::Ready;
        h.app.input.set_text("unfinished chat draft");
        open_settings(&mut h).await;
        press(&mut h.app, close);
        assert_eq!(h.app.surface_mode, SurfaceMode::Chat);
        assert_eq!(h.app.input.text(), "unfinished chat draft");
        assert!(h.commands.try_recv().is_err());
        clock.at(1);
        press(&mut h.app, KeyCode::Enter);
        crate::app::finalize_deferred_submit(&mut h.app);
        test_support::expect_session_prompt(
            &mut h.commands,
            "settings-session",
            "unfinished chat draft",
            &[],
            &[],
        );
        assert!(h.commands.try_recv().is_err());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn config_routes_typing_and_paste_to_search_then_resumes_exact_chat_delivery() {
    let clock = test_support::EventClock::new();
    let mut h = fixture();
    h.app.status = AppStatus::Ready;
    h.app.input.set_text("chat seed");
    open_settings(&mut h).await;
    paste(&mut h.app, "ignored in neutral config");
    crate::app::finalize_pending_paste_event(&mut h.app);
    assert_eq!(h.app.input.text(), "chat seed");
    assert!(h.commands.try_recv().is_err());
    press(&mut h.app, KeyCode::Char('/'));
    press(&mut h.app, KeyCode::Char('l'));
    paste(&mut h.app, "anguage");
    assert_eq!(h.app.config.settings.query, "language");
    assert_eq!(h.app.input.text(), "chat seed");
    assert!(h.app.slash.visible().is_none());
    let rows = screen(&mut h.app, 100, 30);
    let search = rows.iter().position(|r| r.contains("Search")).unwrap();
    assert!(rows[search + 1].contains("language"));
    assert!(row(&rows[search + 2..], "Language").contains("Default"));
    assert!(h.commands.try_recv().is_err());
    press(&mut h.app, KeyCode::Esc);
    assert_eq!(h.app.surface_mode, SurfaceMode::Fullscreen(crate::app::FullscreenView::Config));
    press(&mut h.app, KeyCode::Esc);
    assert_eq!(h.app.surface_mode, SurfaceMode::Chat);
    paste(&mut h.app, " + pasted after close");
    crate::app::finalize_pending_paste_event(&mut h.app);
    clock.at(1);
    assert_eq!(h.app.input.text(), "chat seed + pasted after close");
    assert!(h.commands.try_recv().is_err());
    press(&mut h.app, KeyCode::Enter);
    crate::app::finalize_deferred_submit(&mut h.app);
    test_support::expect_session_prompt(
        &mut h.commands,
        "settings-session",
        "chat seed + pasted after close",
        &[],
        &[],
    );
}

#[tokio::test(flavor = "current_thread")]
async fn reopening_config_refreshes_values_and_discards_stale_feedback_and_browse_state() {
    tokio::task::LocalSet::new()
        .run_until(Box::pin(async {
            for replacement in [false, true] {
                let mut h = fixture();
                h.app.trust.preferences_path = Some(h.scratch.path().join("trust.json"));
                crate::app::trust::accept(&mut h.app).unwrap();
                h.app.status = AppStatus::Ready;
                seed(&mut h.app, SettingsScope::User, "language", json!("German"));
                open_settings(&mut h).await;
                // Preserve the earlier Footer selection/filter/scroll regression through actual keys.
                press(&mut h.app, KeyCode::End);
                assert!(row(&screen(&mut h.app, 100, 18), "Show tips").contains('›'));
                press(&mut h.app, KeyCode::Char('/'));
                paste(&mut h.app, "Footer");
                press(&mut h.app, KeyCode::Down);
                let rows = screen(&mut h.app, 100, 20);
                let selected =
                    rows.iter().find(|r| r.contains("Footer") && r.contains('›')).unwrap();
                assert!(selected.contains("Default layout"), "{selected}");
                press(&mut h.app, KeyCode::Esc);
                press(&mut h.app, KeyCode::Esc);
                let mut fresh = h.app.config.snapshot.clone().unwrap();
                let session = if replacement {
                    test_support::drain_session_refreshes(&mut h.commands, "settings-session");
                    let cwd = h.scratch.path().to_path_buf();
                    replace_session(&mut h.app, &cwd);
                    // Complete the replacement's automatic inspection before testing a fresh open.
                    let request = inspection(&mut h, "replacement-session").await;
                    deliver(
                        &mut h.app,
                        "replacement-session",
                        Some(&request),
                        SettingsPersistence::NotRequested,
                        Some(fresh.clone()),
                        None,
                    );
                    test_support::drain_session_refreshes(&mut h.commands, "replacement-session");
                    "replacement-session"
                } else {
                    "settings-session"
                };
                h.app.config.last_error = Some("STALE_CONFIG_ERROR".into());
                h.app.config.status_message = Some("STALE_CONFIG_STATUS".into());
                h.app.input.set_text("/config");
                crate::app::input_submit::submit_input(&mut h.app);
                assert_eq!(
                    h.app.surface_mode,
                    SurfaceMode::Fullscreen(crate::app::FullscreenView::Config),
                    "reopen in replacement={replacement}"
                );
                let request = inspection_only(&mut h, session).await;
                fresh.catalog.retain(|s| matches!(s.id.as_str(), "model" | "language"));
                fresh.context = "fresh-open-context".into();
                fresh.cwd = h.app.cwd_raw.clone();
                put(
                    &mut fresh,
                    SettingsScope::User,
                    "language",
                    Some(json!("Japanese")),
                    "reopened-r2",
                );
                effective(&mut fresh, "language", Some(json!("Japanese")), &["user"]);
                deliver(
                    &mut h.app,
                    session,
                    Some(&request),
                    SettingsPersistence::NotRequested,
                    Some(fresh),
                    None,
                );
                assert!(h.app.config.settings.query.is_empty());
                assert_eq!(h.app.config.settings.focus, SettingsFocus::Content);
                let rows = screen(&mut h.app, 100, 24);
                assert!(row(&rows, "Default model").contains('›'));
                assert!(row(&rows, "Language").contains("Japanese"));
                assert!(!row(&rows, "Footer").contains('›'));
                assert!(!rows.iter().any(|r| r.contains("STALE_CONFIG_")));
                press(&mut h.app, KeyCode::End);
                press(&mut h.app, KeyCode::Char(' '));
                assert_eq!(h.app.config.setting_overlay().unwrap().draft, "Japanese");
                assert!(h.commands.try_recv().is_err());
            }
        }))
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn tab_wrap_help_sections_and_plugin_lists_keep_their_content_and_remembered_targets() {
    let mut h = fixture();
    h.app.status = AppStatus::Ready;
    open_settings(&mut h).await;
    h.app.sdk_inventory.available_commands =
        vec![model::AvailableCommand::new("aaa-audit", "COMMAND_DESCRIPTION")];
    h.app.sdk_inventory.available_agents =
        vec![model::AvailableAgent::new("audit-agent", "AGENT_DESCRIPTION")];
    h.app.config.last_error = Some("STALE_TAB_ERROR".into());
    h.app.config.status_message = Some("STALE_TAB_STATUS".into());
    press(&mut h.app, KeyCode::BackTab);
    assert_eq!(h.app.config.active_tab, ConfigTab::Help);
    assert!(!screen(&mut h.app, 100, 30).iter().any(|r| r.contains("STALE_TAB_")));
    press(&mut h.app, KeyCode::Right);
    let rows = screen(&mut h.app, 100, 30);
    assert!(row(&rows, "/aaa-audit").contains("COMMAND_DESCRIPTION"));
    press(&mut h.app, KeyCode::Right);
    let rows = screen(&mut h.app, 100, 30);
    assert!(row(&rows, "&audit-agent").contains("AGENT_DESCRIPTION"));
    press(&mut h.app, KeyCode::Left);
    assert_eq!(h.app.config.help_section, ConfigHelpSection::Commands);
    assert!(row(&screen(&mut h.app, 100, 30), "/aaa-audit").contains("COMMAND_DESCRIPTION"));
    press(&mut h.app, KeyCode::Tab);
    let request = inspection_only(&mut h, "settings-session").await;
    let incoming = h.app.config.snapshot.clone().unwrap();
    deliver(
        &mut h.app,
        "settings-session",
        Some(&request),
        SettingsPersistence::NotRequested,
        Some(incoming),
        None,
    );
    assert_eq!(h.app.config.selected_setting().unwrap().id, "language");
    h.app.plugins.installed = vec![
        super::installed_plugin_entry("alpha@market"),
        super::installed_plugin_entry("zeta@market"),
    ];
    h.app.plugins.marketplace = [("alpha", "ALPHA_DESCRIPTION"), ("zeta", "ZETA_DESCRIPTION")]
        .into_iter()
        .map(|(name, description)| crate::app::plugins::MarketplaceEntry {
            plugin_id: format!("{name}@market"),
            name: name.into(),
            description: Some(description.into()),
            marketplace_name: Some("market".into()),
            version: None,
            install_count: None,
            source: None,
        })
        .collect();
    h.app.plugins.session_plugins = vec![crate::agent::types::SessionPlugin {
        name: "local-tools".into(),
        path: "session-folder".into(),
        source: None,
        version: None,
    }];
    h.app.plugins.last_inventory_refresh_at = Some(std::time::Instant::now());
    press(&mut h.app, KeyCode::Tab);
    press(&mut h.app, KeyCode::Down);
    press(&mut h.app, KeyCode::Enter);
    assert_eq!(h.app.config.installed_plugin_actions_overlay().unwrap().plugin_id, "zeta@market");
    assert!(editor_body(&screen(&mut h.app, 100, 30)).contains("ZETA_DESCRIPTION"));
    press(&mut h.app, KeyCode::Esc);
    press(&mut h.app, KeyCode::Right);
    press(&mut h.app, KeyCode::Enter);
    assert!(editor_body(&screen(&mut h.app, 100, 30)).contains("ALPHA_DESCRIPTION"));
    press(&mut h.app, KeyCode::Esc);
    press(&mut h.app, KeyCode::Left);
    press(&mut h.app, KeyCode::Left);
    press(&mut h.app, KeyCode::Enter);
    assert_eq!(h.app.config.plugin_source_input_overlay().unwrap().draft, "session-folder");
    press(&mut h.app, KeyCode::Esc);
    press(&mut h.app, KeyCode::Right);
    press(&mut h.app, KeyCode::Enter);
    assert_eq!(h.app.config.installed_plugin_actions_overlay().unwrap().plugin_id, "zeta@market");
    press(&mut h.app, KeyCode::Esc);
    press(&mut h.app, KeyCode::BackTab);
    let request = inspection_only(&mut h, "settings-session").await;
    let incoming = h.app.config.snapshot.clone().unwrap();
    deliver(
        &mut h.app,
        "settings-session",
        Some(&request),
        SettingsPersistence::NotRequested,
        Some(incoming),
        None,
    );
    assert!(row(&screen(&mut h.app, 100, 30), "Language").contains('›'));
    press(&mut h.app, KeyCode::Char(' '));
    assert_eq!(h.app.config.setting_overlay().unwrap().setting.id, "language");
    assert!(h.commands.try_recv().is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn navigating_to_mcp_requests_its_snapshot_but_non_mcp_activation_does_not() {
    let mut h = fixture();
    open_settings(&mut h).await;
    press(&mut h.app, KeyCode::BackTab);
    assert_eq!(h.app.config.active_tab, ConfigTab::Help);
    assert!(h.commands.try_recv().is_err());
    press(&mut h.app, KeyCode::BackTab);
    assert_eq!(h.app.config.active_tab, ConfigTab::Mcp);
    let command = receive(&mut h).await;
    assert!(
        matches!(command.command,BridgeCommand::GetMcpSnapshot{session_id} if session_id=="settings-session")
    );
    assert!(h.commands.try_recv().is_err());
    press(&mut h.app, KeyCode::Tab);
    assert_eq!(h.app.config.active_tab, ConfigTab::Help);
    assert!(h.commands.try_recv().is_err());
}
