use super::*;
use crate::agent::model;
use crate::agent::wire::BridgeCommand;

fn app_with_connection()
-> (crate::app::App, tokio::sync::mpsc::UnboundedReceiver<crate::agent::wire::CommandEnvelope>) {
    let mut app = crate::app::App::test_default();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    app.conn = Some(std::rc::Rc::new(crate::agent::client::AgentConnection::new(tx)));
    app.session_id = Some(model::SessionId::new("session-1"));
    (app, rx)
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
    assert_eq!(PluginsViewTab::Installed.prev(), PluginsViewTab::Marketplace);
    assert_eq!(PluginsViewTab::Marketplace.next(), PluginsViewTab::Installed);
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
        BridgeCommand::ReloadPlugins { session_id } if session_id == "session-1"
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
        BridgeCommand::ReloadPlugins { session_id } if session_id == "session-1"
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
            tools: Vec::new(),
        },
        model::McpServerStatus {
            name: "fff".to_owned(),
            status: model::McpServerConnectionStatus::Connected,
            server_info: None,
            error: None,
            config: None,
            scope: Some("user".to_owned()),
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
        BridgeCommand::ReloadPlugins { session_id } if session_id == "session-1"
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
