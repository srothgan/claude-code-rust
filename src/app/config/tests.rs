// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::agent::model::{McpServerConnectionStatus, McpServerStatus, McpServerStatusConfig};
use crate::agent::wire::BridgeCommand;
use crate::app::{App, AppStatus, FullscreenView, SurfaceMode};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;
use tempfile::TempDir;

fn attach_test_connection(app: &mut App) -> crate::agent::client::CommandReceiver {
    let (connection, receiver) = crate::agent::client::AgentConnection::test_channel();
    app.session_runtime.conn = Some(Rc::new(connection));
    receiver
}

fn open_settings_app_in_dir(dir: &TempDir) -> App {
    let mut app = App::test_default();
    app.settings_home_override = Some(dir.path().to_path_buf());
    app.cwd_raw = dir.path().to_string_lossy().into_owned();
    open(&mut app).expect("open");
    app
}

fn open_settings_test_app() -> (TempDir, App) {
    let dir = tempfile::tempdir().expect("tempdir");
    let app = open_settings_app_in_dir(&dir);
    (dir, app)
}

fn app_with_status_connection() -> (App, crate::agent::client::CommandReceiver) {
    let mut app = App::test_default();
    let rx = attach_test_connection(&mut app);
    app.session_runtime.session_id = Some(crate::agent::model::SessionId::new("session-1"));
    app.config.active_tab = ConfigTab::Status;
    app.recent_sessions = vec![crate::app::RecentSessionInfo {
        session_id: "session-1".to_owned(),
        summary: "Existing session summary".to_owned(),
        last_modified_ms: 0,
        file_size_bytes: 0,
        cwd: Some("/test".to_owned()),
        git_branch: None,
        custom_title: Some("Current custom title".to_owned()),
        first_prompt: Some("First prompt".to_owned()),
    }];
    (app, rx)
}

fn connected_mcp_server_status(
    name: &str,
    scope: &str,
    config: Option<McpServerStatusConfig>,
) -> McpServerStatus {
    McpServerStatus {
        name: name.to_owned(),
        status: McpServerConnectionStatus::Connected,
        server_info: None,
        error: None,
        config,
        scope: Some(scope.to_owned()),
        source: None,
        tools: Vec::new(),
    }
}

fn installed_plugin_entry(id: &str) -> crate::app::plugins::InstalledPluginEntry {
    installed_plugin_entry_with_scope(id, "user", None)
}

fn disabled_installed_plugin_entry(id: &str) -> crate::app::plugins::InstalledPluginEntry {
    let mut entry = installed_plugin_entry(id);
    entry.enabled = false;
    entry
}

fn installed_plugin_entry_with_scope(
    id: &str,
    scope: &str,
    project_path: Option<&str>,
) -> crate::app::plugins::InstalledPluginEntry {
    crate::app::plugins::InstalledPluginEntry {
        id: id.to_owned(),
        version: Some("1.0.0".to_owned()),
        scope: scope.to_owned(),
        enabled: true,
        installed_at: None,
        last_updated: None,
        project_path: project_path.map(ToOwned::to_owned),
        mcp_server_names: Vec::new(),
    }
}

fn installed_mcp_plugin_entry(
    id: &str,
    mcp_server_names: &[&str],
) -> crate::app::plugins::InstalledPluginEntry {
    let mut entry = installed_plugin_entry(id);
    entry.mcp_server_names = mcp_server_names.iter().map(|name| (*name).to_owned()).collect();
    entry
}

fn notion_plugin_entry() -> crate::app::plugins::InstalledPluginEntry {
    let mut entry = installed_mcp_plugin_entry("notion@claude-plugins-official", &["notion"]);
    entry.version = Some("0.1.0".to_owned());
    entry
}

fn dynamic_http_mcp_server_status(name: &str, url: &str) -> McpServerStatus {
    connected_mcp_server_status(
        name,
        "dynamic",
        Some(McpServerStatusConfig::Http {
            url: url.to_owned(),
            headers: BTreeMap::new(),
            timeout: None,
            request_timeout_ms: None,
            tools: Vec::new(),
            always_load: None,
        }),
    )
}

fn notion_mcp_server_status() -> McpServerStatus {
    dynamic_http_mcp_server_status("plugin:Notion:notion", "https://mcp.notion.com/mcp")
}

#[test]
fn open_does_not_force_stop_active_turn() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = App::test_default();
    app.settings_home_override = Some(dir.path().to_path_buf());
    app.cwd_raw = dir.path().to_string_lossy().to_string();
    app.status = AppStatus::Running;

    open(&mut app).expect("open");

    assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::Config));
    assert!(matches!(app.status, AppStatus::Running));
    assert!(!app.turn.cancel_requested);
}

#[test]
fn activate_tab_clears_status_and_error_feedback() {
    let (_dir, mut app) = open_settings_test_app();
    app.config.status_message = Some("saved".into());
    app.config.last_error = Some("failed".into());

    controller::activate_tab(&mut app, ConfigTab::Plugins);

    assert!(app.config.status_message.is_none());
    assert!(app.config.last_error.is_none());
}

#[test]
fn reopen_clears_stale_transient_feedback() {
    let (_dir, mut app) = open_settings_test_app();
    app.config.status_message = Some("stale status".to_owned());
    app.config.last_error = Some("stale error".to_owned());

    super::controller::close(&mut app);
    open(&mut app).expect("reopen");

    assert!(app.config.status_message.is_none());
    assert!(
        app.config
            .last_error
            .as_deref()
            .is_some_and(|error| error.contains("Settings are currently unavailable"))
    );
}

#[test]
fn open_rejects_untrusted_projects() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = App::test_default();
    app.settings_home_override = Some(dir.path().to_path_buf());
    app.cwd_raw = dir.path().to_string_lossy().to_string();
    app.trust.status = crate::app::trust::TrustStatus::Untrusted;

    let err = open(&mut app).expect_err("open should be blocked");

    assert!(err.contains("Project trust"));
    assert_eq!(app.surface_mode, SurfaceMode::Chat);
}

#[test]
fn tab_navigation_wraps_and_clears_status_message() {
    let (_dir, mut app) = open_settings_test_app();
    app.config.status_message = Some("saved".to_owned());

    handle_key(&mut app, KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));

    assert_eq!(app.config.active_tab, ConfigTab::Help);
    assert!(app.config.status_message.is_none());

    handle_key(&mut app, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));

    assert_eq!(app.config.active_tab, ConfigTab::Mcp);
}

#[test]
fn help_tab_left_right_switches_help_sections() {
    let (_dir, mut app) = open_settings_test_app();
    app.config.active_tab = ConfigTab::Help;
    app.config.help_section = ConfigHelpSection::Shortcuts;

    handle_key(&mut app, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    assert_eq!(app.config.help_section, ConfigHelpSection::Commands);

    handle_key(&mut app, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    assert_eq!(app.config.help_section, ConfigHelpSection::Subagents);

    handle_key(&mut app, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    assert_eq!(app.config.help_section, ConfigHelpSection::Commands);
}

#[test]
fn plugins_tab_uses_arrow_keys_for_inner_navigation() {
    let (_dir, mut app) = open_settings_test_app();
    app.config.active_tab = ConfigTab::Plugins;
    app.config.settings.select("remembered-setting".into());
    app.plugins.installed = vec![
        installed_plugin_entry("frontend-design@claude-plugins-official"),
        installed_plugin_entry("rust-analyzer-lsp@claude-plugins-official"),
    ];

    handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));

    assert_eq!(
        app.config.settings.position().expect("position").selected.as_deref(),
        Some("remembered-setting")
    );
    assert_eq!(app.config.active_tab, ConfigTab::Plugins);
    assert_eq!(app.plugins.installed_selected_index, 1);
    assert_eq!(app.plugins.active_tab, crate::app::plugins::PluginsViewTab::Plugins);
    assert_eq!(app.plugins.installed_search_query, "");
    assert_eq!(app.plugins.plugins_search_query, "");
    assert!(app.config.overlay.is_none());
}

#[test]
fn plugins_inner_tab_switch_does_not_trigger_refresh() {
    let (_dir, mut app) = open_settings_test_app();
    app.config.active_tab = ConfigTab::Plugins;
    app.plugins.loading = false;
    app.plugins.last_inventory_refresh_at = None;

    handle_key(&mut app, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));

    assert_eq!(app.plugins.active_tab, crate::app::plugins::PluginsViewTab::Plugins);
    assert!(!app.plugins.loading);
}

#[test]
fn installed_plugin_enter_opens_actions_overlay() {
    let (_dir, mut app) = open_settings_test_app();
    app.config.active_tab = ConfigTab::Plugins;
    app.cwd_raw = "C:\\work\\project-a".to_owned();
    app.plugins.installed = vec![installed_plugin_entry_with_scope(
        "frontend-design@claude-plugins-official",
        "local",
        Some("C:\\work\\project-a"),
    )];
    app.plugins.marketplace = vec![crate::app::plugins::MarketplaceEntry {
        plugin_id: "frontend-design@claude-plugins-official".to_owned(),
        name: "frontend-design".to_owned(),
        description: Some("Create distinctive interfaces".to_owned()),
        marketplace_name: Some("claude-plugins-official".to_owned()),
        version: Some("1.0.0".to_owned()),
        install_count: Some(42),
        source: None,
    }];

    handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let overlay = app.config.installed_plugin_actions_overlay().expect("installed actions overlay");
    assert_eq!(overlay.title, "Frontend Design From Claude Plugins Official");
    assert_eq!(overlay.description, "Create distinctive interfaces");
    assert_eq!(
        overlay.actions,
        vec![
            InstalledPluginActionKind::Disable,
            InstalledPluginActionKind::Update,
            InstalledPluginActionKind::Uninstall,
        ]
    );
}

#[test]
fn installed_plugin_overlay_uses_up_down_and_escape() {
    let (_dir, mut app) = open_settings_test_app();
    app.config.active_tab = ConfigTab::Plugins;
    app.plugins.installed =
        vec![disabled_installed_plugin_entry("frontend-design@claude-plugins-official")];

    handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));

    assert_eq!(
        app.config.installed_plugin_actions_overlay().map(|overlay| overlay.selected_index),
        Some(1)
    );

    handle_key(&mut app, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));

    assert_eq!(
        app.config.installed_plugin_actions_overlay().map(|overlay| overlay.selected_index),
        Some(0)
    );

    handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    assert!(app.config.overlay.is_none());
}

#[test]
fn plugin_enter_opens_install_overlay() {
    let (_dir, mut app) = open_settings_test_app();
    app.config.active_tab = ConfigTab::Plugins;
    app.plugins.active_tab = crate::app::plugins::PluginsViewTab::Plugins;
    app.plugins.marketplace = vec![crate::app::plugins::MarketplaceEntry {
        plugin_id: "frontend-design@claude-plugins-official".to_owned(),
        name: "frontend-design".to_owned(),
        description: Some("Create distinctive interfaces".to_owned()),
        marketplace_name: Some("claude-plugins-official".to_owned()),
        version: Some("1.0.0".to_owned()),
        install_count: Some(42),
        source: None,
    }];

    handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let overlay = app.config.plugin_install_overlay().expect("Plugin install overlay");
    assert_eq!(overlay.title, "Frontend Design");
    assert_eq!(overlay.description, "Create distinctive interfaces");
    assert_eq!(
        overlay.actions,
        vec![
            PluginInstallActionKind::User,
            PluginInstallActionKind::Project,
            PluginInstallActionKind::Local,
        ]
    );
}

#[test]
fn plugin_install_overlay_uses_up_down_and_escape() {
    let (_dir, mut app) = open_settings_test_app();
    app.config.active_tab = ConfigTab::Plugins;
    app.plugins.active_tab = crate::app::plugins::PluginsViewTab::Plugins;
    app.plugins.marketplace = vec![crate::app::plugins::MarketplaceEntry {
        plugin_id: "frontend-design@claude-plugins-official".to_owned(),
        name: "frontend-design".to_owned(),
        description: Some("Create distinctive interfaces".to_owned()),
        marketplace_name: Some("claude-plugins-official".to_owned()),
        version: Some("1.0.0".to_owned()),
        install_count: Some(42),
        source: None,
    }];

    handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));

    assert_eq!(app.config.plugin_install_overlay().map(|overlay| overlay.selected_index), Some(1));

    handle_key(&mut app, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));

    assert_eq!(app.config.plugin_install_overlay().map(|overlay| overlay.selected_index), Some(0));

    handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    assert!(app.config.overlay.is_none());
}

#[test]
fn marketplace_enter_opens_actions_overlay_for_configured_marketplace() {
    let (_dir, mut app) = open_settings_test_app();
    app.config.active_tab = ConfigTab::Plugins;
    app.plugins.active_tab = crate::app::plugins::PluginsViewTab::Marketplace;
    app.plugins.marketplaces = vec![crate::app::plugins::MarketplaceSourceEntry {
        name: "claude-plugins-official".to_owned(),
        source: Some("github".to_owned()),
        repo: Some("anthropics/claude-plugins-official".to_owned()),
    }];

    handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let overlay = app.config.marketplace_actions_overlay().expect("marketplace actions overlay");
    assert_eq!(overlay.title, "Claude Plugins Official");
    assert!(overlay.description.contains("Source: github"));
    assert!(overlay.description.contains("Repo: anthropics/claude-plugins-official"));
    assert_eq!(
        overlay.actions,
        vec![
            crate::app::config::MarketplaceActionKind::Update,
            crate::app::config::MarketplaceActionKind::Remove,
        ]
    );
}

#[test]
fn marketplace_add_row_opens_text_input_overlay() {
    let (_dir, mut app) = open_settings_test_app();
    app.config.active_tab = ConfigTab::Plugins;
    app.plugins.active_tab = crate::app::plugins::PluginsViewTab::Marketplace;
    app.plugins.marketplaces = vec![crate::app::plugins::MarketplaceSourceEntry {
        name: "claude-plugins-official".to_owned(),
        source: Some("github".to_owned()),
        repo: Some("anthropics/claude-plugins-official".to_owned()),
    }];

    handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let overlay = app.config.add_marketplace_overlay().expect("add marketplace overlay");
    assert_eq!(overlay.draft, "");
    assert_eq!(overlay.cursor, 0);
}

#[test]
fn add_marketplace_overlay_supports_editing_and_escape() {
    let (_dir, mut app) = open_settings_test_app();
    app.config.active_tab = ConfigTab::Plugins;
    app.plugins.active_tab = crate::app::plugins::PluginsViewTab::Marketplace;

    handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));

    let overlay = app.config.add_marketplace_overlay().expect("add marketplace overlay");
    assert_eq!(overlay.draft, "on");
    assert_eq!(overlay.cursor, 1);

    handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    assert!(app.config.overlay.is_none());
}

#[test]
fn add_marketplace_overlay_accepts_paste() {
    let (_dir, mut app) = open_settings_test_app();
    app.config.active_tab = ConfigTab::Plugins;
    app.plugins.active_tab = crate::app::plugins::PluginsViewTab::Marketplace;

    handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    crate::app::events::handle_terminal_event(
        &mut app,
        Event::Paste("anthropics/claude-plugins-official".into()),
    );

    let overlay = app.config.add_marketplace_overlay().expect("add marketplace overlay");
    assert_eq!(overlay.draft, "anthropics/claude-plugins-official");
}

#[test]
fn empty_marketplace_source_sets_overlay_error_and_keeps_overlay_open() {
    let (_dir, mut app) = open_settings_test_app();
    app.config.active_tab = ConfigTab::Plugins;
    app.plugins.active_tab = crate::app::plugins::PluginsViewTab::Marketplace;

    handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert!(app.config.add_marketplace_overlay().is_some());
    let message = app.config.overlay_message.as_ref().expect("overlay message");
    assert_eq!(message.kind, OverlayMessageKind::Error);
    assert_eq!(message.text, "Marketplace source cannot be empty");
    assert!(app.config.last_error.is_none());
}

#[test]
fn installed_plugin_uninstall_requires_confirmation_and_restores_previous_overlay_on_cancel() {
    let (_dir, mut app) = open_settings_test_app();
    app.config.active_tab = ConfigTab::Plugins;
    app.plugins.installed =
        vec![disabled_installed_plugin_entry("frontend-design@claude-plugins-official")];

    handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let confirmation = app.config.confirmation_overlay().expect("confirmation overlay");
    assert_eq!(confirmation.action, ConfirmationAction::InstalledPluginUninstall);
    assert_eq!(confirmation.selected_index, 0);
    assert!(confirmation.body.contains("frontend-design@claude-plugins-official"));
    assert!(confirmation.body.contains("user scope"));

    handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let restored =
        app.config.installed_plugin_actions_overlay().expect("restored installed actions overlay");
    assert_eq!(restored.selected_index, 2);
}

#[test]
fn marketplace_remove_requires_confirmation_and_escape_restores_previous_overlay() {
    let (_dir, mut app) = open_settings_test_app();
    app.config.active_tab = ConfigTab::Plugins;
    app.plugins.active_tab = crate::app::plugins::PluginsViewTab::Marketplace;
    app.plugins.marketplaces = vec![crate::app::plugins::MarketplaceSourceEntry {
        name: "claude-plugins-official".to_owned(),
        source: Some("github".to_owned()),
        repo: Some("anthropics/claude-plugins-official".to_owned()),
    }];

    handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let confirmation = app.config.confirmation_overlay().expect("confirmation overlay");
    assert_eq!(confirmation.action, ConfirmationAction::MarketplaceRemove);
    assert_eq!(confirmation.selected_index, 0);
    assert!(confirmation.body.contains("claude-plugins-official"));

    handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    let restored = app.config.marketplace_actions_overlay().expect("restored marketplace overlay");
    assert_eq!(restored.selected_index, 1);
}

#[test]
fn plugins_search_accepts_paste_when_focused() {
    let (_dir, mut app) = open_settings_test_app();
    app.config.active_tab = ConfigTab::Plugins;
    app.plugins.active_tab = crate::app::plugins::PluginsViewTab::Plugins;
    app.plugins.search_focused = true;

    crate::app::events::handle_terminal_event(
        &mut app,
        Event::Paste("frontend-design\nsupabase".into()),
    );

    assert_eq!(app.plugins.plugins_search_query, "frontend-design supabase");
}

#[test]
fn status_tab_r_opens_session_rename_overlay() {
    let (mut app, _rx) = app_with_status_connection();

    handle_key(&mut app, KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));

    assert_eq!(
        app.config.session_rename_overlay().map(|overlay| overlay.draft.as_str()),
        Some("Current custom title")
    );
    assert_eq!(app.config.session_rename_overlay().map(|overlay| overlay.cursor), Some(20));
}

#[tokio::test(flavor = "current_thread")]
async fn activating_usage_tab_starts_refresh_lifecycle() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (_dir, mut app) = open_settings_test_app();

            controller::activate_tab(&mut app, ConfigTab::Usage);

            assert_eq!(app.config.active_tab, ConfigTab::Usage);
            assert!(app.usage.in_flight);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn usage_tab_r_triggers_manual_refresh() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (_dir, mut app) = open_settings_test_app();
            app.config.active_tab = ConfigTab::Usage;

            handle_key(&mut app, KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));

            assert!(app.usage.in_flight);
        })
        .await;
}

#[test]
fn status_tab_rename_confirm_sends_bridge_command() {
    let (mut app, mut rx) = app_with_status_connection();

    handle_key(&mut app, KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
    for _ in 0.."Current custom title".chars().count() {
        handle_key(&mut app, KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
    }
    for ch in "Renamed session".chars() {
        handle_key(&mut app, KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let envelope = rx.try_recv().expect("rename command");
    assert_eq!(
        envelope.command,
        BridgeCommand::RenameSession {
            session_id: "session-1".to_owned(),
            title: "Renamed session".to_owned(),
        }
    );
    assert!(app.config.overlay.is_none());
    assert_eq!(app.config.status_message.as_deref(), Some("Renaming session..."));
    assert!(app.config.last_error.is_none());
    assert!(matches!(
        app.config.pending_session_title_change.as_ref(),
        Some(pending)
            if pending.session_id == "session-1"
                && matches!(
                    pending.kind,
                    PendingSessionTitleChangeKind::Rename {
                        requested_title: Some(ref requested_title)
                    } if requested_title == "Renamed session"
                )
    ));
}

#[test]
fn status_tab_rename_empty_confirm_clears_custom_title() {
    let (mut app, mut rx) = app_with_status_connection();

    handle_key(&mut app, KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
    for _ in 0.."Current custom title".chars().count() {
        handle_key(&mut app, KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
    }
    handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let envelope = rx.try_recv().expect("rename command");
    assert_eq!(
        envelope.command,
        BridgeCommand::RenameSession { session_id: "session-1".to_owned(), title: String::new() }
    );
    assert_eq!(app.config.status_message.as_deref(), Some("Clearing session name..."));
    assert!(matches!(
        app.config.pending_session_title_change.as_ref(),
        Some(pending)
            if matches!(
                pending.kind,
                PendingSessionTitleChangeKind::Rename { requested_title: None }
            )
    ));
}

#[test]
fn status_tab_rename_escape_cancels_without_command() {
    let (mut app, mut rx) = app_with_status_connection();

    handle_key(&mut app, KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Char('X'), KeyModifiers::SHIFT));
    handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    assert!(app.config.overlay.is_none());
    assert!(rx.try_recv().is_err());
    assert!(app.config.pending_session_title_change.is_none());
}

#[test]
fn status_tab_g_generates_session_title_from_current_title_fallback() {
    let (mut app, mut rx) = app_with_status_connection();

    handle_key(&mut app, KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));

    let envelope = rx.try_recv().expect("generate command");
    assert_eq!(
        envelope.command,
        BridgeCommand::GenerateSessionTitle {
            session_id: "session-1".to_owned(),
            description: "Current custom title".to_owned(),
        }
    );
    assert_eq!(app.config.status_message.as_deref(), Some("Generating session title..."));
    assert!(matches!(
        app.config.pending_session_title_change.as_ref(),
        Some(pending)
            if pending.session_id == "session-1"
                && matches!(pending.kind, PendingSessionTitleChangeKind::Generate)
    ));
}

#[test]
fn status_tab_g_requires_existing_session_metadata() {
    let (mut app, mut rx) = app_with_status_connection();
    app.recent_sessions[0].custom_title = None;
    app.recent_sessions[0].summary.clear();
    app.recent_sessions[0].first_prompt = None;

    handle_key(&mut app, KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));

    assert!(rx.try_recv().is_err());
    assert_eq!(
        app.config.last_error.as_deref(),
        Some("No session summary is available to generate a title")
    );
    assert!(app.config.pending_session_title_change.is_none());
}

#[test]
fn mcp_enter_opens_details_overlay_instead_of_closing_config() {
    let (_dir, mut app) = open_settings_test_app();
    app.config.active_tab = ConfigTab::Mcp;
    app.session_runtime.session_id = Some(crate::agent::model::SessionId::new("session-1"));
    app.mcp.servers = vec![crate::agent::model::McpServerStatus {
        name: "filesystem".to_owned(),
        status: crate::agent::model::McpServerConnectionStatus::Connected,
        server_info: None,
        error: None,
        config: Some(crate::agent::model::McpServerStatusConfig::Stdio {
            command: "npx".to_owned(),
            args: vec!["@modelcontextprotocol/server-filesystem".to_owned()],
            env: BTreeMap::new(),
            timeout: None,
            request_timeout_ms: None,
            always_load: None,
        }),
        scope: Some("project".to_owned()),
        source: None,
        tools: vec![],
    }];

    handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::Config));
    assert_eq!(
        app.config.mcp_details_overlay().map(|overlay| overlay.server_name.as_str()),
        Some("filesystem")
    );
}

#[test]
fn mcp_details_overlay_enter_closes_overlay() {
    let (_dir, mut app) = open_settings_test_app();
    app.config.active_tab = ConfigTab::Mcp;
    app.config.overlay = Some(ConfigOverlayState::McpDetails(McpDetailsOverlayState {
        server_name: "filesystem".to_owned(),
        selected_index: 0,
    }));

    handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert!(app.config.overlay.is_none());
    assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::Config));
}

#[test]
fn mcp_clear_auth_requires_confirmation_and_cancel_restores_details_overlay() {
    let (_dir, mut app) = open_settings_test_app();
    app.config.active_tab = ConfigTab::Mcp;
    app.mcp.auth_capabilities.clear_auth = true;
    app.mcp.servers = vec![crate::agent::model::McpServerStatus {
        name: "filesystem".to_owned(),
        status: crate::agent::model::McpServerConnectionStatus::Connected,
        server_info: None,
        error: None,
        config: None,
        scope: Some("project".to_owned()),
        source: None,
        tools: Vec::new(),
    }];
    app.config.overlay = Some(ConfigOverlayState::McpDetails(McpDetailsOverlayState {
        server_name: "filesystem".to_owned(),
        selected_index: 1,
    }));

    handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let confirmation = app.config.confirmation_overlay().expect("confirmation overlay");
    assert_eq!(confirmation.action, ConfirmationAction::McpClearAuth);
    assert_eq!(confirmation.selected_index, 0);
    assert!(confirmation.body.contains("filesystem"));

    handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let restored = app.config.mcp_details_overlay().expect("restored mcp details overlay");
    assert_eq!(restored.server_name, "filesystem");
    assert_eq!(restored.selected_index, 1);
}

#[test]
fn user_mcp_server_offers_matching_config_remove_action() {
    let (_dir, app) = open_settings_test_app();
    let server = crate::agent::model::McpServerStatus {
        name: "filesystem".to_owned(),
        status: crate::agent::model::McpServerConnectionStatus::Connected,
        server_info: None,
        error: None,
        config: None,
        scope: Some("user".to_owned()),
        source: None,
        tools: Vec::new(),
    };

    let actions = available_mcp_actions(&app, &server);

    assert!(actions.contains(&super::mcp::McpServerActionKind::RemoveUserConfig));
    assert_eq!(super::mcp::McpServerActionKind::RemoveUserConfig.label(), "Remove");
    assert!(!actions.contains(&super::mcp::McpServerActionKind::RemoveLocalConfig));
    assert!(!actions.contains(&super::mcp::McpServerActionKind::RemoveProjectConfig));
    assert!(!actions.contains(&super::mcp::McpServerActionKind::RemoveDynamicConfig));
    assert!(super::mcp::is_mcp_action_available(
        &app,
        &server,
        super::mcp::McpServerActionKind::RemoveUserConfig
    ));
}

#[test]
fn mcp_source_is_authoritative_for_config_removal_scope() {
    let (_dir, app) = open_settings_test_app();
    let mut server = connected_mcp_server_status("session-search", "user", None);
    server.source = Some("dynamic".to_owned());

    let actions = available_mcp_actions(&app, &server);

    assert!(actions.contains(&super::mcp::McpServerActionKind::RemoveDynamicConfig));
    assert!(!actions.contains(&super::mcp::McpServerActionKind::RemoveUserConfig));
}

#[test]
fn mcp_config_source_works_without_legacy_scope() {
    let (_dir, app) = open_settings_test_app();
    let mut server = connected_mcp_server_status("filesystem", "dynamic", None);
    server.scope = None;
    server.source = Some("user".to_owned());

    let actions = available_mcp_actions(&app, &server);

    assert!(actions.contains(&super::mcp::McpServerActionKind::RemoveUserConfig));
    assert!(!actions.contains(&super::mcp::McpServerActionKind::RemoveDynamicConfig));
}

#[test]
fn sdk_and_unknown_mcp_sources_never_inherit_removal_from_scope_or_name() {
    let (_dir, mut app) = open_settings_test_app();
    app.plugins.installed = vec![notion_plugin_entry()];
    for source in ["sdk", "future-source"] {
        let mut server = notion_mcp_server_status();
        server.source = Some(source.to_owned());

        let actions = available_mcp_actions(&app, &server);

        assert!(!actions.contains(&super::mcp::McpServerActionKind::ManagePlugin));
        assert!(!actions.contains(&super::mcp::McpServerActionKind::RemoveDynamicConfig));
    }
}

#[test]
fn true_dynamic_mcp_server_offers_matching_config_remove_action() {
    let (_dir, app) = open_settings_test_app();
    let server = dynamic_http_mcp_server_status("session-search", "https://example.test/mcp");

    let actions = available_mcp_actions(&app, &server);

    assert!(actions.contains(&super::mcp::McpServerActionKind::RemoveDynamicConfig));
    assert_eq!(super::mcp::McpServerActionKind::RemoveDynamicConfig.label(), "Remove");
    assert!(!actions.contains(&super::mcp::McpServerActionKind::RemoveUserConfig));
    assert!(!actions.contains(&super::mcp::McpServerActionKind::RemoveLocalConfig));
    assert!(!actions.contains(&super::mcp::McpServerActionKind::RemoveProjectConfig));
    assert!(super::mcp::is_mcp_action_available(
        &app,
        &server,
        super::mcp::McpServerActionKind::RemoveDynamicConfig
    ));
}

#[test]
fn plugin_owned_mcp_server_offers_manage_plugin_instead_of_dynamic_remove() {
    let (_dir, mut app) = open_settings_test_app();
    app.plugins.installed = vec![notion_plugin_entry()];
    let server = notion_mcp_server_status();

    let actions = available_mcp_actions(&app, &server);

    assert!(actions.contains(&super::mcp::McpServerActionKind::ManagePlugin));
    assert!(!actions.contains(&super::mcp::McpServerActionKind::RemoveDynamicConfig));
    assert!(super::mcp::is_mcp_action_available(
        &app,
        &server,
        super::mcp::McpServerActionKind::ManagePlugin
    ));
    assert!(!super::mcp::is_mcp_action_available(
        &app,
        &server,
        super::mcp::McpServerActionKind::RemoveDynamicConfig
    ));
}

#[test]
fn plugin_owned_mcp_server_maps_to_installed_plugin_entry() {
    let (_dir, mut app) = open_settings_test_app();
    app.plugins.installed = vec![notion_plugin_entry()];

    let owner =
        crate::app::plugins::installed_mcp_plugin_for_runtime_server(&app, "plugin:Notion:notion")
            .expect("plugin owner");

    assert_eq!(owner.id, "notion@claude-plugins-official");
    assert_eq!(owner.mcp_server_names, vec!["notion"]);
}

#[test]
fn mcp_manage_plugin_action_opens_installed_plugin_actions_overlay() {
    let (_dir, mut app) = open_settings_test_app();
    app.config.active_tab = ConfigTab::Mcp;
    app.plugins.installed = vec![notion_plugin_entry()];
    app.mcp.servers = vec![notion_mcp_server_status()];
    let manage_index = available_mcp_actions(&app, &app.mcp.servers[0])
        .iter()
        .position(|action| *action == super::mcp::McpServerActionKind::ManagePlugin)
        .expect("manage plugin action");
    app.config.overlay = Some(ConfigOverlayState::McpDetails(McpDetailsOverlayState {
        server_name: "plugin:Notion:notion".to_owned(),
        selected_index: manage_index,
    }));

    handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let overlay = app.config.installed_plugin_actions_overlay().expect("plugin actions overlay");
    assert_eq!(overlay.plugin_id, "notion@claude-plugins-official");
    assert_eq!(overlay.scope, "user");
    assert_eq!(
        overlay.actions,
        vec![
            InstalledPluginActionKind::Disable,
            InstalledPluginActionKind::Update,
            InstalledPluginActionKind::Uninstall,
        ]
    );
}

#[test]
fn unknown_plugin_owned_mcp_server_does_not_call_set_servers_for_dynamic_remove() {
    let (_dir, mut app) = open_settings_test_app();
    let mut rx = attach_test_connection(&mut app);
    app.session_runtime.session_id = Some(crate::agent::model::SessionId::new("session-1"));
    app.mcp.servers = vec![dynamic_http_mcp_server_status(
        "plugin:Unknown:unknown",
        "https://unknown.example.test/mcp",
    )];

    let actions = available_mcp_actions(&app, &app.mcp.servers[0]);
    assert!(!actions.contains(&super::mcp::McpServerActionKind::ManagePlugin));
    assert!(!actions.contains(&super::mcp::McpServerActionKind::RemoveDynamicConfig));

    super::mcp::remove_mcp_server_from_config(
        &mut app,
        "plugin:Unknown:unknown",
        super::mcp::McpConfigScope::Dynamic,
    );

    assert!(rx.try_recv().is_err());
    assert!(app.mcp.pending_dynamic_config_removal.is_none());
    assert!(!app.mcp.in_flight);
    assert_eq!(
        app.mcp.servers.iter().map(|server| server.name.as_str()).collect::<Vec<_>>(),
        vec!["plugin:Unknown:unknown"]
    );
    let message = app.mcp.last_error.as_deref().expect("last MCP error");
    assert!(message.contains("not removable from dynamic config"));
}

#[test]
fn stale_plugin_owned_mcp_server_is_filtered_after_plugin_inventory_refresh() {
    let (_dir, mut app) = open_settings_test_app();
    app.plugins.last_inventory_refresh_at = Some(std::time::Instant::now());
    let mut servers =
        vec![notion_mcp_server_status(), connected_mcp_server_status("fff", "user", None)];

    super::mcp::filter_stale_plugin_mcp_servers(
        &app,
        Some(crate::agent::types::McpSnapshotSource::ReloadPlugins),
        &mut servers,
    );

    assert_eq!(servers.iter().map(|server| server.name.as_str()).collect::<Vec<_>>(), vec!["fff"]);
}

#[test]
fn unknown_plugin_owned_mcp_server_stays_visible_before_plugin_inventory_refresh() {
    let (_dir, app) = open_settings_test_app();
    let mut servers = vec![notion_mcp_server_status()];

    super::mcp::filter_stale_plugin_mcp_servers(
        &app,
        Some(crate::agent::types::McpSnapshotSource::McpStatus),
        &mut servers,
    );

    assert_eq!(
        servers.iter().map(|server| server.name.as_str()).collect::<Vec<_>>(),
        vec!["plugin:Notion:notion"]
    );
}

#[test]
fn installed_plugin_owned_mcp_server_survives_stale_filter() {
    let (_dir, mut app) = open_settings_test_app();
    app.plugins.last_inventory_refresh_at = Some(std::time::Instant::now());
    app.plugins.installed = vec![notion_plugin_entry()];
    let mut servers = vec![notion_mcp_server_status()];

    super::mcp::filter_stale_plugin_mcp_servers(
        &app,
        Some(crate::agent::types::McpSnapshotSource::ReloadPlugins),
        &mut servers,
    );

    assert_eq!(
        servers.iter().map(|server| server.name.as_str()).collect::<Vec<_>>(),
        vec!["plugin:Notion:notion"]
    );
}

#[test]
fn mcp_config_remove_requires_confirmation_with_exact_scope() {
    let (_dir, mut app) = open_settings_test_app();
    app.config.active_tab = ConfigTab::Mcp;
    app.mcp.servers = vec![crate::agent::model::McpServerStatus {
        name: "filesystem".to_owned(),
        status: crate::agent::model::McpServerConnectionStatus::Connected,
        server_info: None,
        error: None,
        config: None,
        scope: Some("user".to_owned()),
        source: None,
        tools: Vec::new(),
    }];
    let remove_index = available_mcp_actions(&app, &app.mcp.servers[0])
        .iter()
        .position(|action| *action == super::mcp::McpServerActionKind::RemoveUserConfig)
        .expect("remove action");
    app.config.overlay = Some(ConfigOverlayState::McpDetails(McpDetailsOverlayState {
        server_name: "filesystem".to_owned(),
        selected_index: remove_index,
    }));

    handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let confirmation = app.config.confirmation_overlay().expect("confirmation overlay");
    assert_eq!(confirmation.action, ConfirmationAction::McpRemoveConfig);
    assert_eq!(confirmation.confirm_label, "Remove");
    assert_eq!(confirmation.body, "Remove MCP server filesystem? This cannot be reversed.");
}

#[test]
fn non_config_mcp_server_does_not_offer_remove_action() {
    let (_dir, app) = open_settings_test_app();
    let server = crate::agent::model::McpServerStatus {
        name: "claude.ai Google Drive".to_owned(),
        status: crate::agent::model::McpServerConnectionStatus::Disabled,
        server_info: None,
        error: None,
        config: Some(crate::agent::model::McpServerStatusConfig::ClaudeaiProxy {
            url: "https://mcp-proxy.anthropic.com/v1/mcp/server".to_owned(),
            id: "mcpsrv_test".to_owned(),
            timeout: None,
        }),
        scope: Some("claudeai".to_owned()),
        source: None,
        tools: Vec::new(),
    };

    let actions = available_mcp_actions(&app, &server);

    assert!(!actions.contains(&super::mcp::McpServerActionKind::RemoveUserConfig));
    assert!(!actions.contains(&super::mcp::McpServerActionKind::RemoveLocalConfig));
    assert!(!actions.contains(&super::mcp::McpServerActionKind::RemoveProjectConfig));
    assert!(!actions.contains(&super::mcp::McpServerActionKind::RemoveDynamicConfig));
}

#[test]
fn mcp_config_remove_success_reloads_runtime_without_extra_snapshot() {
    let (_dir, mut app) = open_settings_test_app();
    let mut rx = attach_test_connection(&mut app);
    app.session_runtime.session_id = Some(crate::agent::model::SessionId::new("session-1"));
    app.mcp.servers = vec![
        crate::agent::model::McpServerStatus {
            name: "filesystem".to_owned(),
            status: crate::agent::model::McpServerConnectionStatus::Connected,
            server_info: None,
            error: None,
            config: None,
            scope: Some("user".to_owned()),
            source: None,
            tools: Vec::new(),
        },
        crate::agent::model::McpServerStatus {
            name: "other".to_owned(),
            status: crate::agent::model::McpServerConnectionStatus::Connected,
            server_info: None,
            error: None,
            config: None,
            scope: Some("user".to_owned()),
            source: None,
            tools: Vec::new(),
        },
    ];

    super::mcp::apply_mcp_config_remove_success(
        &mut app,
        "filesystem",
        "user",
        PathBuf::from("C:\\tools\\claude.exe"),
    );

    assert_eq!(app.mcp.claude_path, Some(PathBuf::from("C:\\tools\\claude.exe")));
    let guard = app
        .mcp
        .removed_config_servers
        .get(&crate::app::state::types::RemovedMcpServerKey::new(
            "user".to_owned(),
            "filesystem".to_owned(),
        ))
        .expect("removed MCP guard");
    assert_eq!(guard.expected_source, crate::agent::types::McpSnapshotSource::ReloadPlugins);
    assert_eq!(
        app.mcp.servers.iter().map(|server| server.name.as_str()).collect::<Vec<_>>(),
        vec!["other"]
    );
    assert_eq!(
        app.config.status_message.as_deref(),
        Some(
            "Removed MCP server filesystem from user config. You might need to run /new-session to apply MCP changes."
        )
    );
    let envelope = rx.try_recv().expect("runtime reload command");
    assert_eq!(
        envelope.command,
        BridgeCommand::ReloadPlugins { session_id: "session-1".to_owned(), force: false }
    );
    assert!(app.mcp.in_flight);
    assert!(rx.try_recv().is_err());
}

#[test]
fn dynamic_mcp_config_remove_uses_sdk_set_servers_and_preserves_other_dynamic_servers() {
    let (_dir, mut app) = open_settings_test_app();
    let mut rx = attach_test_connection(&mut app);
    app.session_runtime.session_id = Some(crate::agent::model::SessionId::new("session-1"));
    let mut keep_headers = BTreeMap::new();
    keep_headers.insert("Authorization".to_owned(), "Bearer token".to_owned());
    app.mcp.servers = vec![
        connected_mcp_server_status(
            "session-search",
            "dynamic",
            Some(crate::agent::model::McpServerStatusConfig::Http {
                url: "https://search.example.test/mcp".to_owned(),
                headers: std::collections::BTreeMap::new(),
                timeout: None,
                request_timeout_ms: None,
                tools: Vec::new(),
                always_load: None,
            }),
        ),
        connected_mcp_server_status(
            "keep-dynamic",
            "dynamic",
            Some(crate::agent::model::McpServerStatusConfig::Http {
                url: "https://example.test/mcp".to_owned(),
                headers: keep_headers.clone(),
                timeout: Some(5_000),
                request_timeout_ms: Some(30_000),
                tools: vec![crate::agent::model::McpServerToolPolicy {
                    name: "search".to_owned(),
                    permission_policy: Some(
                        crate::agent::model::McpServerToolPermissionPolicy::Ask,
                    ),
                    org_max_permission: Some(crate::agent::model::McpServerOrgMaxPermission::Ask),
                }],
                always_load: Some(true),
            }),
        ),
        connected_mcp_server_status("fff", "user", None),
    ];

    super::mcp::remove_mcp_server_from_config(
        &mut app,
        "session-search",
        super::mcp::McpConfigScope::Dynamic,
    );

    let envelope = rx.try_recv().expect("mcp set servers command");
    let BridgeCommand::McpSetServers { session_id, servers } = envelope.command else {
        panic!("expected mcp_set_servers");
    };
    assert_eq!(session_id, "session-1");
    assert_eq!(servers.len(), 1);
    assert_eq!(
        servers.get("keep-dynamic"),
        Some(&crate::agent::types::McpServerConfig::Http {
            url: "https://example.test/mcp".to_owned(),
            headers: keep_headers,
            tools: vec![crate::agent::types::McpServerToolPolicy {
                name: "search".to_owned(),
                permission_policy: Some(crate::agent::types::McpServerToolPermissionPolicy::Ask),
                org_max_permission: Some(crate::agent::types::McpServerOrgMaxPermission::Ask),
            }],
            timeout: Some(5_000),
            request_timeout_ms: Some(30_000),
            always_load: Some(true),
        })
    );
    assert_eq!(app.mcp.pending_dynamic_config_removal.as_deref(), Some("session-search"));
    assert_eq!(
        app.mcp.servers.iter().map(|server| server.name.as_str()).collect::<Vec<_>>(),
        vec!["session-search", "keep-dynamic", "fff"]
    );
    assert!(rx.try_recv().is_err());

    super::mcp::handle_mcp_set_servers_result(
        &mut app,
        &crate::agent::types::McpSetServersResult {
            removed: vec!["session-search".to_owned()],
            ..Default::default()
        },
    );

    let guard = app
        .mcp
        .removed_config_servers
        .get(&crate::app::state::types::RemovedMcpServerKey::new(
            "dynamic".to_owned(),
            "session-search".to_owned(),
        ))
        .expect("removed MCP guard");
    assert_eq!(guard.expected_source, crate::agent::types::McpSnapshotSource::McpSetServers);
    assert_eq!(
        app.mcp.servers.iter().map(|server| server.name.as_str()).collect::<Vec<_>>(),
        vec!["keep-dynamic", "fff"]
    );
    assert_eq!(
        app.config.status_message.as_deref(),
        Some(
            "Removed MCP server session-search from dynamic config. You might need to run /new-session to apply MCP changes."
        )
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn dynamic_mcp_config_remove_waits_for_snapshot_when_sdk_result_does_not_name_server() {
    let (_dir, mut app) = open_settings_test_app();
    let mut rx = attach_test_connection(&mut app);
    app.session_runtime.session_id = Some(crate::agent::model::SessionId::new("session-1"));
    app.mcp.servers = vec![connected_mcp_server_status(
        "session-search",
        "dynamic",
        Some(crate::agent::model::McpServerStatusConfig::Http {
            url: "https://search.example.test/mcp".to_owned(),
            headers: BTreeMap::new(),
            timeout: None,
            request_timeout_ms: None,
            tools: Vec::new(),
            always_load: None,
        }),
    )];

    super::mcp::remove_mcp_server_from_config(
        &mut app,
        "session-search",
        super::mcp::McpConfigScope::Dynamic,
    );
    assert!(matches!(
        rx.try_recv().expect("mcp set servers command").command,
        BridgeCommand::McpSetServers { .. }
    ));

    super::mcp::handle_mcp_set_servers_result(
        &mut app,
        &crate::agent::types::McpSetServersResult::default(),
    );

    assert_eq!(app.mcp.pending_dynamic_config_removal.as_deref(), Some("session-search"));
    assert!(app.mcp.removed_config_servers.is_empty());
    assert_eq!(
        app.mcp.servers.iter().map(|server| server.name.as_str()).collect::<Vec<_>>(),
        vec!["session-search"]
    );
    assert!(app.mcp.in_flight);
    assert_eq!(
        app.config.status_message.as_deref(),
        Some(
            "Removing MCP server session-search from dynamic config... Waiting for SDK confirmation."
        )
    );
}

#[test]
fn dynamic_mcp_config_remove_fails_when_confirming_snapshot_still_contains_server() {
    let (_dir, mut app) = open_settings_test_app();
    app.mcp.pending_dynamic_config_removal = Some("session-search".to_owned());
    app.mcp.in_flight = true;
    app.mcp.servers = vec![connected_mcp_server_status(
        "session-search",
        "dynamic",
        Some(crate::agent::model::McpServerStatusConfig::Http {
            url: "https://search.example.test/mcp".to_owned(),
            headers: BTreeMap::new(),
            timeout: None,
            request_timeout_ms: None,
            tools: Vec::new(),
            always_load: None,
        }),
    )];

    let confirmation = super::mcp::pending_dynamic_mcp_removal_confirmation_from_snapshot(
        &app,
        Some(crate::agent::types::McpSnapshotSource::McpSetServers),
        None,
        &app.mcp.servers,
    );
    super::mcp::apply_pending_dynamic_mcp_removal_confirmation(&mut app, confirmation);

    assert!(app.mcp.pending_dynamic_config_removal.is_none());
    assert!(app.mcp.removed_config_servers.is_empty());
    assert!(!app.mcp.in_flight);
    assert_eq!(
        app.mcp.servers.iter().map(|server| server.name.as_str()).collect::<Vec<_>>(),
        vec!["session-search"]
    );
    let message = app.mcp.last_error.as_deref().expect("last MCP error");
    assert!(message.contains("Failed to remove MCP server session-search from dynamic config"));
    assert!(message.contains("confirming snapshot still contains the server"));
    assert!(app.config.status_message.is_none());
}

#[test]
fn dynamic_mcp_config_remove_succeeds_when_confirming_snapshot_proves_absence() {
    let (_dir, mut app) = open_settings_test_app();
    app.mcp.pending_dynamic_config_removal = Some("session-search".to_owned());
    app.mcp.in_flight = true;
    app.mcp.servers = vec![
        connected_mcp_server_status(
            "session-search",
            "dynamic",
            Some(crate::agent::model::McpServerStatusConfig::Http {
                url: "https://search.example.test/mcp".to_owned(),
                headers: BTreeMap::new(),
                timeout: None,
                request_timeout_ms: None,
                tools: Vec::new(),
                always_load: None,
            }),
        ),
        connected_mcp_server_status(
            "keep-dynamic",
            "dynamic",
            Some(crate::agent::model::McpServerStatusConfig::Http {
                url: "https://example.test/mcp".to_owned(),
                headers: BTreeMap::new(),
                timeout: None,
                request_timeout_ms: None,
                tools: Vec::new(),
                always_load: None,
            }),
        ),
    ];
    let snapshot_servers = vec![connected_mcp_server_status(
        "keep-dynamic",
        "dynamic",
        Some(crate::agent::model::McpServerStatusConfig::Http {
            url: "https://example.test/mcp".to_owned(),
            headers: BTreeMap::new(),
            timeout: None,
            request_timeout_ms: None,
            tools: Vec::new(),
            always_load: None,
        }),
    )];

    let confirmation = super::mcp::pending_dynamic_mcp_removal_confirmation_from_snapshot(
        &app,
        Some(crate::agent::types::McpSnapshotSource::McpSetServers),
        None,
        &snapshot_servers,
    );
    app.mcp.servers = snapshot_servers;
    app.mcp.in_flight = false;
    super::mcp::apply_pending_dynamic_mcp_removal_confirmation(&mut app, confirmation);

    assert!(app.mcp.pending_dynamic_config_removal.is_none());
    assert!(app.mcp.removed_config_servers.is_empty());
    assert_eq!(
        app.mcp.servers.iter().map(|server| server.name.as_str()).collect::<Vec<_>>(),
        vec!["keep-dynamic"]
    );
    assert_eq!(
        app.config.status_message.as_deref(),
        Some(
            "Removed MCP server session-search from dynamic config. You might need to run /new-session to apply MCP changes."
        )
    );
}

#[test]
fn removed_config_guard_stops_suppressing_when_confirming_snapshot_still_contains_server() {
    let (_dir, mut app) = open_settings_test_app();
    app.mcp.removed_config_servers.insert(
        crate::app::state::types::RemovedMcpServerKey::new(
            "dynamic".to_owned(),
            "session-search".to_owned(),
        ),
        crate::app::state::types::RemovedMcpServerGuard {
            expected_source: crate::agent::types::McpSnapshotSource::McpSetServers,
        },
    );
    let mut servers = vec![connected_mcp_server_status(
        "session-search",
        "dynamic",
        Some(crate::agent::model::McpServerStatusConfig::Http {
            url: "https://search.example.test/mcp".to_owned(),
            headers: BTreeMap::new(),
            timeout: None,
            request_timeout_ms: None,
            tools: Vec::new(),
            always_load: None,
        }),
    )];

    let failures = super::mcp::reconcile_removed_config_mcp_server_guards(
        &mut app,
        Some(crate::agent::types::McpSnapshotSource::McpSetServers),
        None,
        &servers,
    );
    super::mcp::filter_removed_config_mcp_servers(&app, &mut servers);
    app.mcp.servers = servers;
    super::mcp::apply_removed_config_mcp_server_confirmation_failures(&mut app, failures);

    assert!(app.mcp.removed_config_servers.is_empty());
    assert_eq!(
        app.mcp.servers.iter().map(|server| server.name.as_str()).collect::<Vec<_>>(),
        vec!["session-search"]
    );
    let message = app.mcp.last_error.as_deref().expect("last MCP error");
    assert!(message.contains("Failed to remove MCP server session-search from dynamic config"));
    assert!(message.contains("confirming mcp_set_servers snapshot still contains the server"));
}

#[test]
fn dynamic_mcp_config_remove_failure_from_sdk_keeps_server_visible() {
    let (_dir, mut app) = open_settings_test_app();
    let mut rx = attach_test_connection(&mut app);
    app.session_runtime.session_id = Some(crate::agent::model::SessionId::new("session-1"));
    app.mcp.servers = vec![connected_mcp_server_status(
        "session-search",
        "dynamic",
        Some(crate::agent::model::McpServerStatusConfig::Http {
            url: "https://search.example.test/mcp".to_owned(),
            headers: BTreeMap::new(),
            timeout: None,
            request_timeout_ms: None,
            tools: Vec::new(),
            always_load: None,
        }),
    )];

    super::mcp::remove_mcp_server_from_config(
        &mut app,
        "session-search",
        super::mcp::McpConfigScope::Dynamic,
    );
    assert!(matches!(
        rx.try_recv().expect("mcp set servers command").command,
        BridgeCommand::McpSetServers { .. }
    ));

    super::mcp::handle_mcp_operation_error(
        &mut app,
        &crate::agent::types::McpOperationError {
            server_name: None,
            operation: "set-servers".to_owned(),
            message: "dynamic update failed".to_owned(),
        },
    );

    assert!(app.mcp.pending_dynamic_config_removal.is_none());
    assert!(app.mcp.removed_config_servers.is_empty());
    assert!(!app.mcp.in_flight);
    assert_eq!(
        app.mcp.servers.iter().map(|server| server.name.as_str()).collect::<Vec<_>>(),
        vec!["session-search"]
    );
    let message = app.mcp.last_error.as_deref().expect("last MCP error");
    assert!(message.contains("Failed to remove MCP server session-search from dynamic config"));
    assert!(message.contains("dynamic update failed"));
    assert!(rx.try_recv().is_err());
}

#[test]
fn dynamic_mcp_config_remove_refuses_to_drop_unrepresentable_dynamic_servers() {
    let (_dir, mut app) = open_settings_test_app();
    let mut rx = attach_test_connection(&mut app);
    app.session_runtime.session_id = Some(crate::agent::model::SessionId::new("session-1"));
    app.mcp.servers = vec![
        connected_mcp_server_status(
            "session-search",
            "dynamic",
            Some(crate::agent::model::McpServerStatusConfig::Http {
                url: "https://search.example.test/mcp".to_owned(),
                headers: BTreeMap::new(),
                timeout: None,
                request_timeout_ms: None,
                tools: Vec::new(),
                always_load: None,
            }),
        ),
        connected_mcp_server_status(
            "in-process",
            "dynamic",
            Some(crate::agent::model::McpServerStatusConfig::Sdk { name: "sdk-server".to_owned() }),
        ),
    ];

    super::mcp::remove_mcp_server_from_config(
        &mut app,
        "session-search",
        super::mcp::McpConfigScope::Dynamic,
    );

    assert!(rx.try_recv().is_err());
    assert!(!app.mcp.in_flight);
    assert!(app.mcp.removed_config_servers.is_empty());
    assert_eq!(
        app.mcp.servers.iter().map(|server| server.name.as_str()).collect::<Vec<_>>(),
        vec!["session-search", "in-process"]
    );
    let message = app.mcp.last_error.as_deref().expect("last MCP error");
    assert!(message.contains("Failed to remove MCP server session-search from dynamic config"));
    assert!(
        message.contains(
            "Cannot safely preserve dynamic MCP server in-process because SDK-server instances cannot be represented by the Rust bridge"
        )
    );
}

#[test]
fn mcp_config_remove_failure_surfaces_overlay_error() {
    let (_dir, mut app) = open_settings_test_app();
    app.mcp.in_flight = true;
    app.config.overlay = Some(ConfigOverlayState::McpDetails(McpDetailsOverlayState {
        server_name: "filesystem".to_owned(),
        selected_index: 0,
    }));

    super::mcp::apply_mcp_config_remove_failure(&mut app, "filesystem", "user", "boom");

    assert!(!app.mcp.in_flight);
    let message = app.config.overlay_message.as_ref().expect("overlay message");
    assert_eq!(message.kind, OverlayMessageKind::Error);
    assert!(message.text.contains("Failed to remove MCP server filesystem from user config"));
}

#[test]
fn empty_mcp_callback_url_sets_overlay_error_and_keeps_overlay_open() {
    let (_dir, mut app) = open_settings_test_app();
    app.config.active_tab = ConfigTab::Mcp;
    app.config.overlay = Some(ConfigOverlayState::McpCallbackUrl(McpCallbackUrlOverlayState {
        server_name: "filesystem".to_owned(),
        draft: String::new(),
        cursor: 0,
    }));

    handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert!(app.config.mcp_callback_url_overlay().is_some());
    let message = app.config.overlay_message.as_ref().expect("overlay message");
    assert_eq!(message.kind, OverlayMessageKind::Error);
    assert_eq!(message.text, "Callback URL cannot be empty");
    assert!(app.config.last_error.is_none());
}

#[test]
fn mcp_tab_refresh_key_requests_snapshot() {
    let (_dir, mut app) = open_settings_test_app();
    let mut rx = attach_test_connection(&mut app);
    app.session_runtime.session_id = Some(crate::agent::model::SessionId::new("session-1"));
    app.config.active_tab = ConfigTab::Mcp;
    app.mcp.servers.push(crate::agent::model::McpServerStatus {
        name: "stale".to_owned(),
        status: crate::agent::model::McpServerConnectionStatus::NeedsAuth,
        server_info: None,
        error: None,
        config: None,
        scope: None,
        source: None,
        tools: Vec::new(),
    });

    handle_key(&mut app, KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));

    let envelope = rx.try_recv().expect("runtime reload command");
    assert_eq!(
        envelope.command,
        BridgeCommand::ReloadPlugins { session_id: "session-1".to_owned(), force: false }
    );
    let envelope = rx.try_recv().expect("mcp snapshot command");
    assert_eq!(
        envelope.command,
        BridgeCommand::GetMcpSnapshot { session_id: "session-1".to_owned() }
    );
    assert!(app.mcp.in_flight);
    assert!(app.mcp.servers.is_empty());
}

#[test]
fn request_mcp_snapshot_sends_outside_mcp_tab() {
    let (_dir, mut app) = open_settings_test_app();
    let mut rx = attach_test_connection(&mut app);
    app.session_runtime.session_id = Some(crate::agent::model::SessionId::new("session-1"));
    app.config.active_tab = ConfigTab::Status;

    super::mcp::request_mcp_snapshot(&mut app);

    let envelope = rx.try_recv().expect("mcp snapshot command");
    assert_eq!(
        envelope.command,
        BridgeCommand::GetMcpSnapshot { session_id: "session-1".to_owned() }
    );
    assert!(app.mcp.in_flight);
}

#[test]
fn refresh_mcp_snapshot_clears_existing_servers_before_request() {
    let (_dir, mut app) = open_settings_test_app();
    let mut rx = attach_test_connection(&mut app);
    app.session_runtime.session_id = Some(crate::agent::model::SessionId::new("session-1"));
    app.mcp.servers.push(crate::agent::model::McpServerStatus {
        name: "stale".to_owned(),
        status: crate::agent::model::McpServerConnectionStatus::Connected,
        server_info: None,
        error: None,
        config: None,
        scope: None,
        source: None,
        tools: Vec::new(),
    });

    refresh_mcp_snapshot(&mut app);

    let envelope = rx.try_recv().expect("mcp snapshot command");
    assert_eq!(
        envelope.command,
        BridgeCommand::GetMcpSnapshot { session_id: "session-1".to_owned() }
    );
    assert!(app.mcp.servers.is_empty());
    assert!(app.mcp.in_flight);
}

#[test]
fn refresh_mcp_snapshot_if_needed_skips_outside_mcp_tab() {
    let (_dir, mut app) = open_settings_test_app();
    let mut rx = attach_test_connection(&mut app);
    app.session_runtime.session_id = Some(crate::agent::model::SessionId::new("session-1"));
    app.config.active_tab = ConfigTab::Status;

    super::mcp::refresh_mcp_snapshot_if_needed(&mut app);

    assert!(rx.try_recv().is_err());
    assert!(!app.mcp.in_flight);
}

#[test]
fn claudeai_proxy_server_shows_disabled_authenticate_action() {
    let (_dir, mut app) = open_settings_test_app();
    app.mcp.auth_capabilities = crate::agent::model::McpAuthCapabilities {
        authenticate: true,
        clear_auth: true,
        submit_oauth_callback_url: true,
    };
    let server = crate::agent::model::McpServerStatus {
        name: "claude.ai Google Calendar".to_owned(),
        status: crate::agent::model::McpServerConnectionStatus::NeedsAuth,
        server_info: None,
        error: Some(
            "MCP server requires authentication but no OAuth token is configured.".to_owned(),
        ),
        config: Some(crate::agent::model::McpServerStatusConfig::ClaudeaiProxy {
            url: "https://mcp-proxy.anthropic.com/v1/mcp/server".to_owned(),
            id: "mcpsrv_test".to_owned(),
            timeout: None,
        }),
        scope: Some("session".to_owned()),
        source: None,
        tools: Vec::new(),
    };

    let actions = available_mcp_actions(&app, &server);

    assert!(actions.contains(&super::mcp::McpServerActionKind::Authenticate));
    assert!(!super::mcp::is_mcp_action_available(
        &app,
        &server,
        super::mcp::McpServerActionKind::Authenticate
    ));
    assert!(actions.contains(&super::mcp::McpServerActionKind::Reconnect));
}

#[test]
fn mcp_auth_actions_follow_bridge_capabilities() {
    let (_dir, mut app) = open_settings_test_app();
    let server = crate::agent::model::McpServerStatus {
        name: "docs".to_owned(),
        status: crate::agent::model::McpServerConnectionStatus::NeedsAuth,
        server_info: None,
        error: None,
        config: Some(crate::agent::model::McpServerStatusConfig::Http {
            url: "https://mcp.example.test".to_owned(),
            headers: BTreeMap::new(),
            tools: Vec::new(),
            timeout: None,
            request_timeout_ms: None,
            always_load: None,
        }),
        scope: Some("user".to_owned()),
        source: None,
        tools: Vec::new(),
    };

    assert!(!super::mcp::is_mcp_action_available(
        &app,
        &server,
        super::mcp::McpServerActionKind::Authenticate
    ));
    assert!(!super::mcp::is_mcp_action_available(
        &app,
        &server,
        super::mcp::McpServerActionKind::ClearAuth
    ));

    app.mcp.auth_capabilities.authenticate = true;
    assert!(!super::mcp::is_mcp_action_available(
        &app,
        &server,
        super::mcp::McpServerActionKind::Authenticate
    ));

    app.mcp.auth_capabilities.submit_oauth_callback_url = true;
    app.mcp.auth_capabilities.clear_auth = true;
    assert!(super::mcp::is_mcp_action_available(
        &app,
        &server,
        super::mcp::McpServerActionKind::Authenticate
    ));
    assert!(super::mcp::is_mcp_action_available(
        &app,
        &server,
        super::mcp::McpServerActionKind::ClearAuth
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn elicitation_response_precedes_the_follow_up_snapshot_request() {
    tokio::task::LocalSet::new()
        .run_until(Box::pin(async {
            let mut app = App::test_default();
            let mut commands = attach_test_connection(&mut app);
            app.session_runtime.session_id = Some(crate::agent::model::SessionId::new("session-1"));
            app.mcp.pending_elicitation = Some(crate::agent::types::ElicitationRequest {
                request_id: "request-1".to_owned(),
                server_name: "server-1".to_owned(),
                message: "Continue?".to_owned(),
                mode: crate::agent::types::ElicitationMode::Form,
                url: None,
                elicitation_id: None,
                requested_schema: None,
            });

            super::mcp::send_mcp_elicitation_response(
                &mut app,
                "request-1",
                crate::agent::types::ElicitationAction::Decline,
                None,
            );

            assert!(app.mcp.pending_elicitation.is_some());
            let response = commands.recv_envelope().await.expect("elicitation response");
            assert!(matches!(
                response.command,
                BridgeCommand::ElicitationResponse {
                    elicitation_request_id,
                    action: crate::agent::types::ElicitationAction::Decline,
                    ..
                } if elicitation_request_id == "request-1"
            ));

            let queued = app.event_rx.recv().await.expect("response admission event");
            assert!(matches!(
                queued,
                crate::agent::events::ClientEvent::McpElicitationResponseQueued {
                    ref request_id,
                    ..
                } if request_id == "request-1"
            ));
            crate::app::events::handle_client_event(&mut app, queued);

            assert!(app.mcp.pending_elicitation.is_none());
            let snapshot = commands.recv_envelope().await.expect("follow-up snapshot");
            assert!(matches!(snapshot.command, BridgeCommand::GetMcpSnapshot { .. }));
        }))
        .await;
}
