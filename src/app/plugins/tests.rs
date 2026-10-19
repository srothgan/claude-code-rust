// SPDX-License-Identifier: Apache-2.0
use super::actions::{
    apply_runtime_reload_failure, apply_runtime_reload_held, apply_runtime_reload_success,
    installed_action_success_message, plugin_install_success_message,
};
use super::*;
use crate::agent::model;
use crate::agent::wire::BridgeCommand;
use crate::app::App;
use crate::app::config::{InstalledPluginActionKind, PluginInstallActionKind};
use std::time::Instant;

use crate::app::test_support::app_with_connection;

fn begin_local_folder(app: &mut App, folder: &str) {
    app.surface_mode = crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
    app.config.active_tab = crate::app::ConfigTab::Plugins;
    assert!(handle_key(
        app,
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('l'),
            crossterm::event::KeyModifiers::NONE,
        )
    ));
    crate::app::handle_terminal_event(app, crossterm::event::Event::Paste(folder.to_owned()));
    crate::app::handle_terminal_event(
        app,
        crossterm::event::Event::Key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        )),
    );
}

fn held_local_reload(app: &mut App, request_id: String) {
    crate::app::handle_client_event(
        app,
        crate::agent::events::ClientEvent::RuntimeReloadHeld {
            session_id: "session-1".to_owned(),
            request_id: Some(request_id),
            error_count: 0,
            cache_impact: crate::agent::types::RuntimeReloadCacheImpact {
                mcp_servers_added: vec!["plugin:fixture:tools".to_owned()],
                mcp_servers_removed: vec![],
                lsp_tool_change: None,
                invalid_server_name_count: 0,
            },
        },
    );
}

#[test]
fn session_reset_discards_attachment_inventory_and_old_reload_acknowledgements() {
    let (mut app, mut commands) = app_with_connection();
    begin_local_folder(&mut app, "folder");
    let request_id = commands.try_recv().expect("reload").request_id;
    app.plugins.session_plugins.push(crate::agent::types::SessionPlugin {
        name: "old".to_owned(),
        path: "old-folder".to_owned(),
        source: None,
        version: None,
    });
    super::inventory::reset_for_session_change(&mut app);
    app.session_runtime.session_id = Some(model::SessionId::new("new-session"));
    crate::app::handle_client_event(
        &mut app,
        crate::agent::events::ClientEvent::RuntimeReloadCompleted {
            session_id: "session-1".to_owned(),
            request_id,
            error_count: 0,
            reconciliation_error: None,
        },
    );
    crate::app::handle_client_event(
        &mut app,
        crate::agent::events::ClientEvent::PluginInventory {
            session_id: "session-1".to_owned(),
            plugins: vec![crate::agent::types::SessionPlugin {
                name: "late".to_owned(),
                path: "late-folder".to_owned(),
                source: None,
                version: None,
            }],
        },
    );
    assert!(app.plugins.session_plugins.is_empty());
    assert!(app.plugins.pending_reload.is_none());
    assert!(app.plugins.status_message.is_none());
}

#[test]
fn local_folder_hold_apply_and_partial_result_keep_request_identity_and_confirmed_inventory() {
    let (mut app, mut commands) = app_with_connection();
    begin_local_folder(&mut app, "relative-folder");
    let first = commands.try_recv().expect("folder reload");
    assert!(
        matches!(first.command, BridgeCommand::ReloadPlugins { force: false, ref add_plugins, .. } if add_plugins == &["relative-folder"])
    );
    let request = first.request_id.expect("correlated operation");
    held_local_reload(&mut app, request.clone());
    assert!(app.plugins.session_plugins.is_empty());
    assert!(app.config.confirmation_overlay().is_some());
    let key = |app: &mut App, code| {
        crate::app::handle_terminal_event(
            app,
            crossterm::event::Event::Key(crossterm::event::KeyEvent::new(
                code,
                crossterm::event::KeyModifiers::NONE,
            )),
        )
    };
    key(&mut app, crossterm::event::KeyCode::Down);
    key(&mut app, crossterm::event::KeyCode::Enter);
    let apply = commands.try_recv().expect("apply folder reload");
    assert!(
        matches!(apply.command, BridgeCommand::ReloadPlugins { force: true, ref add_plugins, .. } if add_plugins == &["relative-folder"])
    );
    let current = apply.request_id.expect("new request identity");
    assert_ne!(request, current);
    crate::app::handle_client_event(
        &mut app,
        crate::agent::events::ClientEvent::RuntimeReloadFailed {
            session_id: "session-1".to_owned(),
            request_id: Some(request),
            message: "stale error".to_owned(),
        },
    );
    assert!(app.plugins.loading);
    let plugins = vec![crate::agent::types::SessionPlugin {
        name: "fixture".to_owned(),
        path: app.cwd_raw.clone(),
        source: Some("fixture@inline".to_owned()),
        version: Some("1.2.3".to_owned()),
    }];
    crate::app::handle_client_event(
        &mut app,
        crate::agent::events::ClientEvent::PluginInventory {
            session_id: "session-1".to_owned(),
            plugins: plugins.clone(),
        },
    );
    crate::app::handle_client_event(
        &mut app,
        crate::agent::events::ClientEvent::RuntimeReloadCompleted {
            session_id: "session-1".to_owned(),
            request_id: Some(current),
            error_count: 1,
            reconciliation_error: None,
        },
    );
    assert_eq!(app.plugins.session_plugins, plugins);
    assert!(
        app.plugins.installed.is_empty(),
        "session loading must not fabricate installed entries"
    );
    assert_eq!(app.plugins.active_tab, PluginsViewTab::Session);
    assert!(app.config.overlay.is_none());
    assert!(!app.plugins.loading);
    assert!(
        app.plugins
            .status_message
            .as_ref()
            .is_some_and(|text| text.contains("applied") && text.contains("1 component"))
    );
}

#[test]
fn declining_a_local_folder_reload_preserves_the_draft_and_does_not_dispatch_an_apply() {
    let (mut app, mut commands) = app_with_connection();
    begin_local_folder(&mut app, "folder with spaces");
    let request = commands.try_recv().expect("reload").request_id.expect("request");
    held_local_reload(&mut app, request);
    crate::app::handle_terminal_event(
        &mut app,
        crossterm::event::Event::Key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        )),
    );
    assert!(commands.try_recv().is_err());
    assert!(app.plugins.session_plugins.is_empty());
    assert!(app.plugins.pending_reload.is_none());
    assert_eq!(
        app.config.plugin_source_input_overlay().expect("retained input").draft,
        "folder with spaces"
    );
}

#[test]
fn a_refused_local_folder_keeps_its_input_and_confirmed_inventory_available_for_retry() {
    let (mut app, mut commands) = app_with_connection();
    let confirmed = vec![crate::agent::types::SessionPlugin {
        name: "existing".to_owned(),
        path: app.cwd_raw.clone(),
        source: None,
        version: None,
    }];
    app.plugins.session_plugins = confirmed.clone();
    begin_local_folder(&mut app, "new-folder");
    let request_id = commands.try_recv().expect("reload").request_id;
    crate::app::handle_client_event(
        &mut app,
        crate::agent::events::ClientEvent::RuntimeReloadFailed {
            session_id: "session-1".to_owned(),
            request_id,
            message: "Managed settings refuse local plugin folders".to_owned(),
        },
    );
    assert!(!app.plugins.loading);
    assert_eq!(app.plugins.session_plugins, confirmed);
    assert_eq!(app.config.plugin_source_input_overlay().expect("retry input").draft, "new-folder");
    assert!(
        app.config
            .overlay_message
            .as_ref()
            .is_some_and(|message| message.text.contains("Managed settings"))
    );
    assert!(commands.try_recv().is_err());
}

fn installed_plugin_entry(
    id: &str,
    scope: &str,
    project_path: Option<&str>,
    mcp_server_names: &[&str],
) -> InstalledPluginEntry {
    InstalledPluginEntry {
        id: id.to_owned(),
        version: Some("1.0.0".to_owned()),
        scope: scope.to_owned(),
        enabled: true,
        installed_at: None,
        last_updated: None,
        project_path: project_path.map(ToOwned::to_owned),
        mcp_server_names: mcp_server_names.iter().map(|name| (*name).to_owned()).collect(),
    }
}

fn sample_snapshot() -> PluginsInventorySnapshot {
    PluginsInventorySnapshot {
        installed: vec![installed_plugin_entry(
            "frontend-design@claude-plugins-official",
            "user",
            None,
            &[],
        )],
        marketplace: vec![],
        marketplaces: vec![],
    }
}

#[test]
fn plugins_tabs_wrap_in_both_directions() {
    assert_eq!(PluginsViewTab::Installed.prev(), PluginsViewTab::Session);
    assert_eq!(PluginsViewTab::Session.next(), PluginsViewTab::Installed);
}

#[test]
fn recent_inventory_snapshot_skips_refresh() {
    let mut app = crate::app::App::test_default();
    app.plugins.active_tab = PluginsViewTab::Installed;
    app.plugins.last_inventory_refresh_at = Some(Instant::now());

    request_inventory_refresh_if_needed(&mut app);

    assert!(!app.plugins.loading);
}

#[test]
fn display_label_normalizes_plugin_and_marketplace_names() {
    assert_eq!(
        display_label("frontend-design@claude-plugins-official"),
        "Frontend Design From Claude Plugins Official"
    );
    assert_eq!(display_label("claude-plugins-official"), "Claude Plugins Official");
}

#[test]
fn plugin_success_messages_hint_new_session() {
    assert_eq!(
        plugin_install_success_message(PluginInstallActionKind::User, "Notion"),
        "Installed Notion for user scope. You might need to run /new-session to apply plugin changes."
    );
    assert_eq!(
        installed_action_success_message(InstalledPluginActionKind::Uninstall, "Notion", "user",),
        "Uninstalled Notion from user scope. You might need to run /new-session to apply plugin changes."
    );
}

#[test]
fn filtered_marketplace_plugins_match_on_name_description_and_marketplace() {
    let state = PluginsState {
        plugins_search_query: "official".to_owned(),
        marketplace: vec![MarketplaceEntry {
            plugin_id: "frontend-design@claude-plugins-official".to_owned(),
            name: "frontend-design".to_owned(),
            description: Some("Create distinctive interfaces".to_owned()),
            marketplace_name: Some("claude-plugins-official".to_owned()),
            version: Some("1.0.0".to_owned()),
            install_count: Some(42),
            source: None,
        }],
        ..PluginsState::default()
    };

    assert_eq!(filtered_marketplace_plugins(&state).len(), 1);
}

#[test]
fn installed_and_plugins_search_queries_are_independent() {
    let state = PluginsState {
        installed_search_query: "installed".to_owned(),
        plugins_search_query: "plugins".to_owned(),
        ..PluginsState::default()
    };

    assert_eq!(state.search_query_for(PluginsViewTab::Installed), "installed");
    assert_eq!(state.search_query_for(PluginsViewTab::Plugins), "plugins");
}

#[test]
fn ordered_installed_keeps_only_user_and_current_project_entries() {
    let state = PluginsState {
        installed: vec![
            installed_plugin_entry(
                "other-local@claude-plugins-official",
                "local",
                Some("C:\\work\\project-a"),
                &[],
            ),
            installed_plugin_entry("user-plugin@claude-plugins-official", "user", None, &[]),
            installed_plugin_entry(
                "current-local@claude-plugins-official",
                "local",
                Some("C:\\work\\project-b"),
                &[],
            ),
            installed_plugin_entry(
                "managed-plugin@claude-plugins-official",
                "managed",
                None,
                &["managed"],
            ),
        ],
        ..PluginsState::default()
    };

    let ordered = ordered_installed(&state, "C:\\work\\project-b");
    let ordered_ids = ordered.iter().map(|entry| entry.id.as_str()).collect::<Vec<_>>();

    assert_eq!(
        ordered_ids,
        vec!["user-plugin@claude-plugins-official", "current-local@claude-plugins-official",]
    );
}

#[test]
fn inventory_refresh_success_triggers_runtime_reload_when_requested() {
    let (mut app, mut rx) = app_with_connection();
    app.plugins.runtime_reload_after_refresh = true;

    apply_inventory_refresh_success(
        &mut app,
        sample_snapshot(),
        std::path::PathBuf::from("C:\\tools\\claude.exe"),
    );

    let envelope = rx.try_recv().expect("reload command");
    assert!(matches!(
        envelope.command,
        BridgeCommand::ReloadPlugins { session_id, force: false, .. } if session_id == "session-1"
    ));
    assert!(!app.plugins.runtime_reload_after_refresh);
    assert_eq!(app.config.status_message.as_deref(), Some("Reloading session plugins..."));
    assert_eq!(
        app.plugins.pending_runtime_reload_success_message.as_deref(),
        Some("Plugin inventory refreshed")
    );
}

#[test]
fn cli_action_success_triggers_runtime_reload() {
    let (mut app, mut rx) = app_with_connection();

    apply_cli_action_success(
        &mut app,
        PluginsCliActionSuccess {
            snapshot: sample_snapshot(),
            message: "Updated plugin".to_owned(),
            claude_path: std::path::PathBuf::from("C:\\tools\\claude.exe"),
        },
    );

    let envelope = rx.try_recv().expect("reload command");
    assert!(matches!(
        envelope.command,
        BridgeCommand::ReloadPlugins { session_id, force: false, .. } if session_id == "session-1"
    ));
    assert_eq!(
        app.plugins.pending_runtime_reload_success_message.as_deref(),
        Some("Updated plugin")
    );
}

#[test]
fn cli_action_success_reconciles_stale_plugin_mcp_servers() {
    let (mut app, mut rx) = app_with_connection();
    app.mcp.servers = vec![
        model::McpServerStatus {
            name: "plugin:Notion:notion".to_owned(),
            status: model::McpServerConnectionStatus::Connected,
            server_info: None,
            error: None,
            config: None,
            scope: Some("dynamic".to_owned()),
            source: None,
            tools: Vec::new(),
        },
        model::McpServerStatus {
            name: "fff".to_owned(),
            status: model::McpServerConnectionStatus::Connected,
            server_info: None,
            error: None,
            config: None,
            scope: Some("user".to_owned()),
            source: None,
            tools: Vec::new(),
        },
    ];
    app.config.overlay = Some(crate::app::config::ConfigOverlayState::McpDetails(
        crate::app::config::McpDetailsOverlayState {
            server_name: "plugin:Notion:notion".to_owned(),
            selected_index: 0,
        },
    ));

    apply_cli_action_success(
        &mut app,
        PluginsCliActionSuccess {
            snapshot: sample_snapshot(),
            message: "Uninstalled plugin".to_owned(),
            claude_path: std::path::PathBuf::from("C:\\tools\\claude.exe"),
        },
    );

    let envelope = rx.try_recv().expect("reload command");
    assert!(matches!(
        envelope.command,
        BridgeCommand::ReloadPlugins { session_id, force: false, .. } if session_id == "session-1"
    ));
    assert_eq!(
        app.mcp.servers.iter().map(|server| server.name.as_str()).collect::<Vec<_>>(),
        vec!["fff"]
    );
    assert!(app.config.overlay.is_none());
}

#[test]
fn runtime_reload_success_applies_pending_success_message() {
    let mut app = App::test_default();
    app.plugins.loading = true;
    app.plugins.pending_runtime_reload_success_message = Some("Updated plugin".to_owned());

    apply_runtime_reload_success(&mut app);

    assert!(!app.plugins.loading);
    assert_eq!(app.config.status_message.as_deref(), Some("Updated plugin"));
    assert!(app.config.last_error.is_none());
    assert!(app.plugins.pending_runtime_reload_success_message.is_none());
}

#[test]
fn runtime_reload_failure_surfaces_visible_error() {
    let mut app = App::test_default();
    app.plugins.loading = true;
    app.plugins.pending_runtime_reload_success_message = Some("Updated plugin".to_owned());

    apply_runtime_reload_failure(&mut app, "boom");

    assert!(!app.plugins.loading);
    assert_eq!(app.config.last_error.as_deref(), Some("Failed to reload session plugins: boom"));
    assert!(app.config.status_message.is_none());
    assert!(app.plugins.pending_runtime_reload_success_message.is_none());
}

#[test]
fn held_runtime_reload_requires_explicit_force_and_preserves_pending_success() {
    let (mut app, mut rx) = app_with_connection();
    app.plugins.loading = true;
    app.plugins.pending_runtime_reload_success_message = Some("Updated plugin".to_owned());

    apply_runtime_reload_held(
        &mut app,
        &crate::agent::types::RuntimeReloadCacheImpact {
            mcp_servers_added: vec!["plugin:docs:search".to_owned()],
            mcp_servers_removed: Vec::new(),
            lsp_tool_change: Some("adds".to_owned()),
            invalid_server_name_count: 1,
        },
    );

    assert!(!app.plugins.loading);
    assert!(matches!(
        app.config.overlay,
        Some(crate::app::config::ConfigOverlayState::Confirmation(ref overlay))
            if overlay.action == crate::app::config::ConfirmationAction::ForceRuntimePluginReload
                && overlay.selected_index == 0
                && overlay.body.contains("plugin:docs:search")
                && overlay.body.contains("unsafe to display")
    ));
    assert_eq!(
        app.plugins.pending_runtime_reload_success_message.as_deref(),
        Some("Updated plugin")
    );

    force_held_runtime_reload(&mut app);
    let envelope = rx.try_recv().expect("forced reload command");
    assert!(matches!(
        envelope.command,
        BridgeCommand::ReloadPlugins { session_id, force: true, .. } if session_id == "session-1"
    ));
}

#[test]
fn cancelling_held_runtime_reload_does_not_report_runtime_success() {
    let mut app = App::test_default();
    app.plugins.pending_runtime_reload_success_message = Some("Updated plugin".to_owned());

    cancel_held_runtime_reload(&mut app);

    assert!(app.plugins.pending_runtime_reload_success_message.is_none());
    assert!(
        app.config
            .status_message
            .as_deref()
            .is_some_and(|message| message.contains("kept its existing tools"))
    );
}

#[test]
fn cli_action_success_without_active_session_keeps_success_message() {
    let mut app = App::test_default();

    apply_cli_action_success(
        &mut app,
        PluginsCliActionSuccess {
            snapshot: sample_snapshot(),
            message: "Updated plugin".to_owned(),
            claude_path: std::path::PathBuf::from("C:\\tools\\claude.exe"),
        },
    );

    assert!(!app.plugins.loading);
    assert_eq!(app.config.status_message.as_deref(), Some("Updated plugin"));
    assert!(app.config.last_error.is_none());
    assert!(app.plugins.pending_runtime_reload_success_message.is_none());
}
