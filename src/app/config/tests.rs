use super::resolve::resolve_setting_document;
use super::*;
use crate::agent::model::{
    AvailableModel, EffortLevel, McpServerConnectionStatus, McpServerStatus, McpServerStatusConfig,
};
use crate::agent::wire::BridgeCommand;
use crate::app::{App, AppStatus, FullscreenView, SurfaceMode};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;
use tempfile::TempDir;

fn open_settings_app_in_dir(dir: &TempDir) -> App {
    std::fs::write(
        dir.path().join(".claude.json"),
        format!(
            "{{\n  \"projects\": {{\n    \"{}\": {{ \"hasTrustDialogAccepted\": true }}\n  }}\n}}\n",
            crate::app::trust::store::normalize_project_key(dir.path())
        ),
    )
    .expect("write trust prefs");
    let mut app = App::test_default();
    app.settings_home_override = Some(dir.path().to_path_buf());
    app.cwd_raw = dir.path().to_string_lossy().to_string();
    open(&mut app).expect("open");
    app
}

fn open_settings_test_app() -> (TempDir, App) {
    let dir = tempfile::tempdir().expect("tempdir");
    let app = open_settings_app_in_dir(&dir);
    (dir, app)
}

fn select_setting(app: &mut App, setting_id: SettingId) {
    app.config.selected_setting_index =
        setting_specs().iter().position(|spec| spec.id == setting_id).expect("setting row");
}

fn app_with_status_connection()
-> (App, tokio::sync::mpsc::UnboundedReceiver<crate::agent::wire::CommandEnvelope>) {
    let mut app = App::test_default();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    app.conn = Some(Rc::new(crate::agent::client::AgentConnection::new(tx)));
    app.session_id = Some(crate::agent::model::SessionId::new("session-1"));
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

fn read_json_file(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).expect("read")).expect("json")
}

fn json_at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut current = value;
    for segment in path {
        current = current.as_object()?.get(*segment)?;
    }
    Some(current)
}

#[test]
fn open_loads_document_and_switches_view() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(".claude").join("settings.json");
    std::fs::create_dir_all(path.parent().expect("settings parent")).expect("create dir");
    std::fs::write(&path, r#"{"fastMode":true}"#).expect("write");
    let mut app = App::test_default();
    app.settings_home_override = Some(dir.path().to_path_buf());
    app.cwd_raw = dir.path().to_string_lossy().to_string();

    open(&mut app).expect("open");

    assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::Config));
    assert!(matches!(
        resolve_setting_document(&app.config.committed_settings_document, SettingId::FastMode, &[])
            .value,
        ResolvedSettingValue::Bool(true)
    ));
    assert!(app.config.settings_path.is_some());
    assert!(app.config.local_settings_path.is_some());
    assert!(app.config.preferences_path.is_some());
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
    assert!(app.pending_cancel_origin.is_none());
}

#[test]
fn initialize_shared_state_reconciles_trust_from_preferences() {
    let dir = tempfile::tempdir().expect("tempdir");
    let prefs_path = dir.path().join(".claude.json");
    std::fs::write(&prefs_path, r#"{"projects":{}}"#).expect("write prefs");

    let mut app = App::test_default();
    app.settings_home_override = Some(dir.path().to_path_buf());
    app.cwd_raw = dir.path().join("project").to_string_lossy().to_string();
    app.trust.status = crate::app::trust::TrustStatus::Trusted;

    initialize_shared_state(&mut app).expect("initialize");

    assert_eq!(app.trust.status, crate::app::trust::TrustStatus::Untrusted);
    assert_eq!(
        app.trust.project_key,
        crate::app::trust::store::normalize_project_key(std::path::Path::new(&app.cwd_raw))
    );
}

#[test]
fn activate_tab_clears_status_and_error_feedback() {
    let (_dir, mut app) = open_settings_test_app();
    app.config.status_message = Some("saved".into());
    app.config.last_error = Some("failed".into());

    activate_tab(&mut app, ConfigTab::Plugins);

    assert!(app.config.status_message.is_none());
    assert!(app.config.last_error.is_none());
}

#[test]
fn reopen_reload_picks_up_external_settings_changes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(".claude").join("settings.json");
    std::fs::create_dir_all(path.parent().expect("settings parent")).expect("create dir");
    std::fs::write(&path, r#"{"fastMode":false}"#).expect("write");
    std::fs::write(
        dir.path().join(".claude.json"),
        format!(
            "{{\n  \"projects\": {{\n    \"{}\": {{ \"hasTrustDialogAccepted\": true }}\n  }}\n}}\n",
            crate::app::trust::store::normalize_project_key(dir.path())
        ),
    )
    .expect("write trust prefs");
    let mut app = App::test_default();
    app.settings_home_override = Some(dir.path().to_path_buf());
    app.cwd_raw = dir.path().to_string_lossy().to_string();

    open(&mut app).expect("open");
    assert!(!app.config.fast_mode_effective());

    close(&mut app);
    std::fs::write(&path, r#"{"fastMode":true}"#).expect("rewrite");

    open(&mut app).expect("reopen");

    assert!(app.config.fast_mode_effective());
}

#[test]
fn reopen_clears_stale_transient_feedback() {
    let (_dir, mut app) = open_settings_test_app();
    app.config.status_message = Some("stale status".to_owned());
    app.config.last_error = Some("stale error".to_owned());

    close(&mut app);
    open(&mut app).expect("reopen");

    assert!(app.config.status_message.is_none());
    assert!(app.config.last_error.is_none());
}

#[test]
fn space_persists_setting_toggles_to_the_expected_document() {
    let cases = [
        (SettingId::FastMode, ".claude/settings.json", vec!["fastMode"], Value::Bool(true)),
        (
            SettingId::AlwaysThinking,
            ".claude/settings.json",
            vec!["alwaysThinkingEnabled"],
            Value::Bool(true),
        ),
        (
            SettingId::TerminalProgressBar,
            ".claude.json",
            vec!["terminalProgressBarEnabled"],
            Value::Bool(false),
        ),
    ];

    for (setting_id, relative_path, json_path, expected) in cases {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(relative_path);
        let mut app = open_settings_app_in_dir(&dir);

        select_setting(&mut app, setting_id);
        handle_key(&mut app, KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));

        let saved = read_json_file(&path);
        assert_eq!(json_at(&saved, &json_path), Some(&expected));
        assert!(app.config.last_error.is_none());
    }
}

#[test]
fn handle_key_moves_between_config_rows() {
    let mut app = App::test_default();
    app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::Config);
    let last_index = setting_specs().len().saturating_sub(1);

    handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.config.selected_setting_index, 1);

    for _ in 0..setting_specs().len().saturating_add(4) {
        handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    }

    assert_eq!(app.config.selected_setting_index, last_index);
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
    app.config.selected_setting_index = 3;
    app.plugins.installed = vec![
        installed_plugin_entry("frontend-design@claude-plugins-official"),
        installed_plugin_entry("rust-analyzer-lsp@claude-plugins-official"),
    ];

    handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));

    assert_eq!(app.config.selected_setting_index, 3);
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
fn left_and_right_adjust_selected_setting_without_switching_tabs() {
    let (_dir, mut app) = open_settings_test_app();
    select_setting(&mut app, SettingId::FastMode);

    handle_key(&mut app, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));

    assert_eq!(app.config.active_tab, ConfigTab::Settings);
    assert!(app.config.fast_mode_effective());

    handle_key(&mut app, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));

    assert_eq!(app.config.active_tab, ConfigTab::Settings);
    assert!(!app.config.fast_mode_effective());
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

            activate_tab(&mut app, ConfigTab::Usage);

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
fn overlay_enter_confirms_without_closing_config_screen() {
    let (_dir, mut app) = open_settings_test_app();
    select_setting(&mut app, SettingId::OutputStyle);

    handle_key(&mut app, KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::Config));
    assert!(app.config.overlay.is_none());
    assert_eq!(
        store::output_style(&app.config.committed_local_settings_document),
        Ok(OutputStyle::Explanatory)
    );
}

#[test]
fn always_thinking_toggles_in_settings_document() {
    let (_dir, mut app) = open_settings_test_app();
    select_setting(&mut app, SettingId::AlwaysThinking);

    handle_key(&mut app, KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));

    assert_eq!(store::always_thinking_enabled(&app.config.committed_settings_document), Ok(true));
}

#[test]
fn reduce_motion_toggles_in_local_settings_document() {
    let (_dir, mut app) = open_settings_test_app();
    select_setting(&mut app, SettingId::ReduceMotion);

    handle_key(&mut app, KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));

    assert_eq!(
        store::prefers_reduced_motion(&app.config.committed_local_settings_document),
        Ok(true)
    );
}

#[test]
fn show_tips_toggles_in_local_settings_document() {
    let (_dir, mut app) = open_settings_test_app();
    select_setting(&mut app, SettingId::ShowTips);

    handle_key(&mut app, KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));

    assert_eq!(
        store::spinner_tips_enabled(&app.config.committed_local_settings_document),
        Ok(false)
    );
}

#[test]
fn handle_key_cycles_default_permission_mode() {
    let (_dir, mut app) = open_settings_test_app();
    select_setting(&mut app, SettingId::DefaultPermissionMode);

    handle_key(&mut app, KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));

    assert_eq!(
        store::default_permission_mode(&app.config.committed_settings_document),
        Ok(DefaultPermissionMode::Auto)
    );
}

#[test]
fn thinking_effort_options_come_from_persistable_effort_levels() {
    let (_dir, app) = open_settings_test_app();
    let spec = setting_spec(SettingId::ThinkingEffort);

    assert_eq!(
        setting_detail_options(&app, spec),
        EffortLevel::PERSISTABLE_SETTINGS
            .iter()
            .map(|level| level.label().to_owned())
            .collect::<Vec<_>>()
    );
    for level in EffortLevel::PERSISTABLE_SETTINGS {
        let mut document = serde_json::json!({ "effortLevel": level.as_stored() });
        assert!(matches!(
            resolve_setting_document(&document, SettingId::ThinkingEffort, &[]).validation,
            SettingValidation::Valid
        ));
        store::set_thinking_effort_level(&mut document, level).expect("level is persistable");
        assert_eq!(store::thinking_effort_level(&document), Ok(level));
    }
}

#[test]
fn thinking_effort_rejects_max_as_persisted_setting() {
    let mut document = serde_json::json!({ "effortLevel": "max" });

    assert!(matches!(
        resolve_setting_document(&document, SettingId::ThinkingEffort, &[]).validation,
        SettingValidation::InvalidValue
    ));
    assert_eq!(store::thinking_effort_level(&document), Err(()));
    assert_eq!(store::set_thinking_effort_level(&mut document, EffortLevel::Max), Err(()));
    assert_eq!(document, serde_json::json!({ "effortLevel": "max" }));
}

#[test]
fn model_effort_overlay_uses_persistable_effort_levels_when_runtime_omits_level_list() {
    let (_dir, mut app) = open_settings_test_app();
    app.available_models = vec![AvailableModel::new("opus", "Opus").supports_effort(true)];
    store::set_model(&mut app.config.committed_settings_document, Some("opus"));
    select_setting(&mut app, SettingId::ThinkingEffort);

    handle_key(&mut app, KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    for _ in 0..4 {
        handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    }

    let overlay = app.config.thinking_effort_overlay().expect("thinking effort overlay");
    assert_eq!(overlay.selected_effort, EffortLevel::XHigh);
}

#[test]
fn model_effort_overlay_filters_session_only_max_from_runtime_levels() {
    let (_dir, mut app) = open_settings_test_app();
    app.available_models = vec![
        AvailableModel::new("opus", "Opus")
            .supports_effort(true)
            .supported_effort_levels(EffortLevel::ALL.to_vec()),
    ];
    store::set_model(&mut app.config.committed_settings_document, Some("opus"));
    select_setting(&mut app, SettingId::ThinkingEffort);

    handle_key(&mut app, KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    for _ in 0..4 {
        handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    }

    let overlay = app.config.thinking_effort_overlay().expect("thinking effort overlay");
    assert_eq!(overlay.selected_effort, EffortLevel::XHigh);
}

#[test]
fn model_overlay_confirm_persists_model_without_rewriting_effort() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(".claude").join("settings.json");
    let mut app = open_settings_app_in_dir(&dir);
    app.available_models = vec![
        AvailableModel::new("fable", "Fable 5").supports_effort(true),
        AvailableModel::new("opus", "Opus").supports_effort(true),
    ];
    store::set_model(&mut app.config.committed_settings_document, Some("fable"));
    store::set_thinking_effort_level(
        &mut app.config.committed_settings_document,
        EffortLevel::High,
    )
    .expect("high is persistable");

    select_setting(&mut app, SettingId::Model);
    handle_key(&mut app, KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    assert!(app.config.model_overlay().is_some());
    handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let saved = read_json_file(&path);
    assert_eq!(json_at(&saved, &["model"]), Some(&Value::String("opus".to_owned())));
    assert_eq!(json_at(&saved, &["effortLevel"]), Some(&Value::String("high".to_owned())));
    assert_eq!(app.config.thinking_effort_effective(), EffortLevel::High);
    assert!(app.config.overlay.is_none());
}

#[test]
fn thinking_effort_overlay_confirm_persists_effort_without_rewriting_model() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(".claude").join("settings.json");
    let mut app = open_settings_app_in_dir(&dir);
    app.available_models = vec![
        AvailableModel::new("fable", "Fable 5")
            .supports_effort(true)
            .supported_effort_levels(EffortLevel::PERSISTABLE_SETTINGS.to_vec()),
    ];
    store::set_model(&mut app.config.committed_settings_document, Some("fable"));

    select_setting(&mut app, SettingId::ThinkingEffort);
    handle_key(&mut app, KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    assert!(app.config.thinking_effort_overlay().is_some());
    handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let saved = read_json_file(&path);
    assert_eq!(json_at(&saved, &["model"]), Some(&Value::String("fable".to_owned())));
    assert_eq!(json_at(&saved, &["effortLevel"]), Some(&Value::String("high".to_owned())));
    assert!(app.config.overlay.is_none());
}

#[test]
fn respect_gitignore_toggles_in_preferences_document() {
    let (_dir, mut app) = open_settings_test_app();
    select_setting(&mut app, SettingId::RespectGitignore);

    handle_key(&mut app, KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));

    assert_eq!(store::respect_gitignore(&app.config.committed_preferences_document), Ok(false));
}

#[test]
fn terminal_progress_bar_toggles_in_preferences_document() {
    let (_dir, mut app) = open_settings_test_app();
    select_setting(&mut app, SettingId::TerminalProgressBar);

    handle_key(&mut app, KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));

    assert_eq!(
        store::terminal_progress_bar_enabled(&app.config.committed_preferences_document),
        Ok(false)
    );
}

#[test]
fn immediate_save_respect_gitignore_invalidates_active_mention_session_cache() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = App::test_default();
    app.settings_home_override = Some(dir.path().to_path_buf());
    app.cwd_raw = dir.path().to_string_lossy().to_string();

    open(&mut app).expect("open");
    app.mention = Some(crate::app::mention::MentionState::new(0, 0, "rs".to_owned(), vec![]));
    select_setting(&mut app, SettingId::RespectGitignore);
    handle_key(&mut app, KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));

    let mention = app.mention.as_ref().expect("mention should stay active");
    assert!(mention.candidates.is_empty());
    assert_eq!(mention.placeholder_message().as_deref(), Some("Searching files..."));
    assert!(!app.config.respect_gitignore_effective());
}

#[test]
fn save_preserves_invalid_unedited_values() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(".claude").join("settings.json");
    std::fs::create_dir_all(path.parent().expect("settings parent")).expect("create dir");
    std::fs::write(&path, r#"{"permissions":{"defaultMode":"broken"},"fastMode":false}"#)
        .expect("write");
    let mut app = App::test_default();
    app.settings_home_override = Some(dir.path().to_path_buf());
    app.cwd_raw = dir.path().to_string_lossy().to_string();

    open(&mut app).expect("open");
    select_setting(&mut app, SettingId::FastMode);
    handle_key(&mut app, KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));

    let raw = std::fs::read_to_string(path).expect("read");
    assert!(raw.contains("\"defaultMode\": \"broken\""));
    assert!(raw.contains("\"fastMode\": true"));
}

#[test]
fn resolved_model_uses_runtime_fallback_when_catalog_rejects_value() {
    let mut app = App::test_default();
    app.available_models = vec![AvailableModel::new("sonnet", "Claude Sonnet")];
    store::set_model(&mut app.config.committed_settings_document, Some("unknown"));

    let resolved = resolved_setting(&app, setting_spec(SettingId::Model));

    assert_eq!(resolved.validation, SettingValidation::UnavailableOption);
    assert_eq!(setting_display_value(&app, setting_spec(SettingId::Model), &resolved), "Fable 5");
}

#[test]
fn resolved_model_accepts_runtime_alias_resolved_model_id() {
    let mut app = App::test_default();
    app.available_models =
        vec![AvailableModel::new("fable", "Fable 5").resolved_model("claude-fable-5")];
    store::set_model(&mut app.config.committed_settings_document, Some("claude-fable-5"));

    let resolved = resolved_setting(&app, setting_spec(SettingId::Model));

    assert_eq!(resolved.validation, SettingValidation::Valid);
    assert_eq!(setting_display_value(&app, setting_spec(SettingId::Model), &resolved), "Fable 5");
}

#[test]
fn model_overlay_options_are_sorted_alphabetically() {
    let mut app = App::test_default();
    app.available_models = vec![
        AvailableModel::new("sonnet", "Sonnet"),
        AvailableModel::new("haiku", "Haiku"),
        AvailableModel::new("opus", "Opus"),
    ];

    let labels = model_overlay_options(&app)
        .into_iter()
        .map(|option| option.display_name)
        .collect::<Vec<_>>();

    assert_eq!(labels, vec!["Haiku", "Opus", "Sonnet"]);
}

#[test]
fn notifications_cycle_in_preferences_document() {
    let (_dir, mut app) = open_settings_test_app();
    select_setting(&mut app, SettingId::Notifications);

    handle_key(&mut app, KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));

    assert_eq!(
        store::preferred_notification_channel(&app.config.committed_preferences_document),
        Ok(PreferredNotifChannel::Iterm2WithBell)
    );
}

#[test]
fn theme_cycles_in_preferences_document() {
    let (_dir, mut app) = open_settings_test_app();
    select_setting(&mut app, SettingId::Theme);

    handle_key(&mut app, KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));

    let stored = store::read_persisted_setting(
        &app.config.committed_preferences_document,
        setting_spec(SettingId::Theme),
    );
    assert_eq!(stored, Ok(store::PersistedSettingValue::String("light".to_owned())));
}

#[test]
fn editor_mode_cycles_in_preferences_document() {
    let (_dir, mut app) = open_settings_test_app();
    select_setting(&mut app, SettingId::EditorMode);

    handle_key(&mut app, KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));

    let stored = store::read_persisted_setting(
        &app.config.committed_preferences_document,
        setting_spec(SettingId::EditorMode),
    );
    assert_eq!(stored, Ok(store::PersistedSettingValue::String("vim".to_owned())));
}

#[test]
fn output_style_resolves_existing_project_value() {
    let mut app = App::test_default();
    store::set_output_style(
        &mut app.config.committed_local_settings_document,
        OutputStyle::Explanatory,
    );

    let resolved = resolved_setting(&app, setting_spec(SettingId::OutputStyle));

    assert_eq!(resolved.validation, SettingValidation::Valid);
    assert_eq!(
        setting_display_value(&app, setting_spec(SettingId::OutputStyle), &resolved),
        "Explanatory"
    );
}

#[test]
fn output_style_missing_value_falls_back_to_default() {
    let app = App::test_default();

    let resolved = resolved_setting(&app, setting_spec(SettingId::OutputStyle));

    assert_eq!(resolved.validation, SettingValidation::Valid);
    assert_eq!(
        setting_display_value(&app, setting_spec(SettingId::OutputStyle), &resolved),
        "Default"
    );
}

#[test]
fn output_style_invalid_value_uses_default_with_invalid_state() {
    let mut app = App::test_default();
    app.config.committed_local_settings_document = serde_json::json!({ "outputStyle": "Verbose" });

    let resolved = resolved_setting(&app, setting_spec(SettingId::OutputStyle));

    assert_eq!(resolved.validation, SettingValidation::InvalidValue);
    assert_eq!(
        setting_display_value(&app, setting_spec(SettingId::OutputStyle), &resolved),
        "Default"
    );
}

#[test]
fn language_resolves_existing_project_value() {
    let mut app = App::test_default();
    store::set_language(&mut app.config.committed_settings_document, Some("German"));

    let resolved = resolved_setting(&app, setting_spec(SettingId::Language));

    assert_eq!(resolved.validation, SettingValidation::Valid);
    assert_eq!(setting_display_value(&app, setting_spec(SettingId::Language), &resolved), "German");
}

#[test]
fn language_missing_value_displays_not_set() {
    let app = App::test_default();

    let resolved = resolved_setting(&app, setting_spec(SettingId::Language));

    assert_eq!(resolved.validation, SettingValidation::Valid);
    assert_eq!(
        setting_display_value(&app, setting_spec(SettingId::Language), &resolved),
        "Not set"
    );
}

#[test]
fn language_invalid_persisted_length_uses_not_set_with_invalid_state() {
    let mut app = App::test_default();
    app.config.committed_settings_document = serde_json::json!({ "language": "E" });

    let resolved = resolved_setting(&app, setting_spec(SettingId::Language));

    assert_eq!(resolved.validation, SettingValidation::InvalidValue);
    assert_eq!(
        setting_display_value(&app, setting_spec(SettingId::Language), &resolved),
        "Not set"
    );
}

#[test]
fn language_whitespace_only_persisted_value_uses_not_set_with_invalid_state() {
    let mut app = App::test_default();
    app.config.committed_settings_document = serde_json::json!({ "language": "   " });

    let resolved = resolved_setting(&app, setting_spec(SettingId::Language));

    assert_eq!(resolved.validation, SettingValidation::InvalidValue);
    assert_eq!(
        setting_display_value(&app, setting_spec(SettingId::Language), &resolved),
        "Not set"
    );
}

#[test]
fn space_opens_output_style_overlay() {
    let (_dir, mut app) = open_settings_test_app();
    select_setting(&mut app, SettingId::OutputStyle);

    handle_key(&mut app, KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));

    assert_eq!(
        app.config.output_style_overlay().map(|overlay| overlay.selected),
        Some(OutputStyle::Default)
    );
}

#[test]
fn space_opens_language_overlay_with_existing_value() {
    let (_dir, mut app) = open_settings_test_app();
    store::set_language(&mut app.config.committed_settings_document, Some("German"));
    select_setting(&mut app, SettingId::Language);

    handle_key(&mut app, KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));

    assert_eq!(app.config.language_overlay().map(|overlay| overlay.draft.as_str()), Some("German"));
    assert_eq!(app.config.language_overlay().map(|overlay| overlay.cursor), Some(6));
}

#[test]
fn space_persists_local_project_settings_immediately() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(".claude").join("settings.local.json");
    let mut app = App::test_default();
    app.settings_home_override = Some(dir.path().to_path_buf());
    app.cwd_raw = dir.path().to_string_lossy().to_string();

    open(&mut app).expect("open");
    select_setting(&mut app, SettingId::ReduceMotion);
    handle_key(&mut app, KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    select_setting(&mut app, SettingId::ShowTips);
    handle_key(&mut app, KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));

    let saved = read_json_file(&path);
    assert_eq!(json_at(&saved, &["prefersReducedMotion"]), Some(&Value::Bool(true)));
    assert_eq!(json_at(&saved, &["spinnerTipsEnabled"]), Some(&Value::Bool(false)));
}

#[test]
fn output_style_overlay_confirm_persists_local_setting() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(".claude").join("settings.local.json");
    let mut app = App::test_default();
    app.settings_home_override = Some(dir.path().to_path_buf());
    app.cwd_raw = dir.path().to_string_lossy().to_string();

    open(&mut app).expect("open");
    select_setting(&mut app, SettingId::OutputStyle);
    handle_key(&mut app, KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let raw = std::fs::read_to_string(path).expect("read");
    assert!(raw.contains("\"outputStyle\": \"Learning\""));
    assert_eq!(
        store::output_style(&app.config.committed_local_settings_document),
        Ok(OutputStyle::Learning)
    );
    assert!(app.config.overlay.is_none());
}

#[test]
fn output_style_overlay_escape_cancels_without_persisting() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(".claude").join("settings.local.json");
    let mut app = App::test_default();
    app.settings_home_override = Some(dir.path().to_path_buf());
    app.cwd_raw = dir.path().to_string_lossy().to_string();

    open(&mut app).expect("open");
    select_setting(&mut app, SettingId::OutputStyle);
    handle_key(&mut app, KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    assert!(!path.exists());
    assert!(app.config.overlay.is_none());
    assert_eq!(
        store::output_style(&app.config.committed_local_settings_document),
        Ok(OutputStyle::Default)
    );
}

#[test]
fn language_overlay_confirm_persists_trimmed_project_setting() {
    for input in ["German", "  German  "] {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".claude").join("settings.json");
        let mut app = open_settings_app_in_dir(&dir);

        select_setting(&mut app, SettingId::Language);
        handle_key(&mut app, KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        for ch in input.chars() {
            handle_key(&mut app, KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        let saved = read_json_file(&path);
        assert_eq!(json_at(&saved, &["language"]), Some(&Value::String("German".to_owned())));
        assert_eq!(
            store::language(&app.config.committed_settings_document),
            Ok(Some("German".to_owned()))
        );
        assert!(app.config.overlay.is_none());
    }
}

#[test]
fn language_overlay_empty_confirm_clears_existing_setting() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(".claude").join("settings.json");
    let mut app = App::test_default();
    app.settings_home_override = Some(dir.path().to_path_buf());
    app.cwd_raw = dir.path().to_string_lossy().to_string();
    std::fs::create_dir_all(path.parent().expect("settings parent")).expect("create dir");
    std::fs::write(&path, r#"{"language":"German"}"#).expect("write");

    open(&mut app).expect("open");
    select_setting(&mut app, SettingId::Language);
    handle_key(&mut app, KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    for _ in 0..6 {
        handle_key(&mut app, KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
    }
    handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let raw = std::fs::read_to_string(path).expect("read");
    assert!(!raw.contains("\"language\""));
    assert_eq!(store::language(&app.config.committed_settings_document), Ok(None));
    assert!(app.config.overlay.is_none());
}

#[test]
fn language_overlay_escape_cancels_without_persisting() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(".claude").join("settings.json");
    let mut app = App::test_default();
    app.settings_home_override = Some(dir.path().to_path_buf());
    app.cwd_raw = dir.path().to_string_lossy().to_string();

    open(&mut app).expect("open");
    select_setting(&mut app, SettingId::Language);
    handle_key(&mut app, KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Char('G'), KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    assert!(!path.exists());
    assert!(app.config.overlay.is_none());
    assert_eq!(store::language(&app.config.committed_settings_document), Ok(None));
}

#[test]
fn language_overlay_blocks_too_short_input() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(".claude").join("settings.json");
    let mut app = App::test_default();
    app.settings_home_override = Some(dir.path().to_path_buf());
    app.cwd_raw = dir.path().to_string_lossy().to_string();

    open(&mut app).expect("open");
    select_setting(&mut app, SettingId::Language);
    handle_key(&mut app, KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Char('E'), KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert!(app.config.language_overlay().is_some());
    assert!(!path.exists());
    assert_eq!(store::language(&app.config.committed_settings_document), Ok(None));
}

#[test]
fn language_overlay_blocks_too_long_input() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(".claude").join("settings.json");
    let mut app = App::test_default();
    app.settings_home_override = Some(dir.path().to_path_buf());
    app.cwd_raw = dir.path().to_string_lossy().to_string();

    open(&mut app).expect("open");
    select_setting(&mut app, SettingId::Language);
    handle_key(&mut app, KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    for ch in "abcdefghijklmnopqrstuvwxyzabcde".chars() {
        handle_key(&mut app, KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert!(app.config.language_overlay().is_some());
    assert!(!path.exists());
    assert_eq!(store::language(&app.config.committed_settings_document), Ok(None));
}

#[test]
fn language_overlay_supports_cursor_aware_editing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(".claude").join("settings.json");
    let mut app = App::test_default();
    app.settings_home_override = Some(dir.path().to_path_buf());
    app.cwd_raw = dir.path().to_string_lossy().to_string();
    std::fs::create_dir_all(path.parent().expect("settings parent")).expect("create dir");
    std::fs::write(&path, r#"{"language":"German"}"#).expect("write");

    open(&mut app).expect("open");
    select_setting(&mut app, SettingId::Language);
    handle_key(&mut app, KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(
        store::language(&app.config.committed_settings_document),
        Ok(Some("Gerian".to_owned()))
    );
}

#[test]
fn invalid_language_confirm_sets_overlay_error_and_keeps_overlay_open() {
    let (_dir, mut app) = open_settings_test_app();
    select_setting(&mut app, SettingId::Language);

    handle_key(&mut app, KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Char('E'), KeyModifiers::NONE));
    handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert!(app.config.language_overlay().is_some());
    let message = app.config.overlay_message.as_ref().expect("overlay message");
    assert_eq!(message.kind, OverlayMessageKind::Error);
    assert_eq!(message.text, "Language must be at least 2 characters.");
    assert!(app.config.last_error.is_none());
}

#[test]
fn escape_cancels_language_overlay_without_writing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(".claude").join("settings.json");
    let mut app = App::test_default();
    app.settings_home_override = Some(dir.path().to_path_buf());
    app.cwd_raw = dir.path().to_string_lossy().to_string();
    std::fs::create_dir_all(path.parent().expect("settings parent")).expect("create dir");
    std::fs::write(&path, r#"{"language":"German"}"#).expect("write");
    open(&mut app).expect("open");
    select_setting(&mut app, SettingId::Language);

    handle_key(&mut app, KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    for _ in 0.."German".chars().count() {
        handle_key(&mut app, KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
    }
    for ch in "English".chars() {
        handle_key(&mut app, KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    assert!(app.config.overlay.is_none());
    assert_eq!(store::language(&read_json_file(&path)), Ok(Some("German".to_owned())));
}

#[test]
fn enter_closes_settings_without_editing_selected_row() {
    let (_dir, mut app) = open_settings_test_app();
    select_setting(&mut app, SettingId::FastMode);

    handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(app.surface_mode, SurfaceMode::Chat);
    assert!(!app.config.fast_mode_effective());
}

#[test]
fn mcp_enter_opens_details_overlay_instead_of_closing_config() {
    let (_dir, mut app) = open_settings_test_app();
    app.config.active_tab = ConfigTab::Mcp;
    app.session_id = Some(crate::agent::model::SessionId::new("session-1"));
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
    app.mcp.servers = vec![crate::agent::model::McpServerStatus {
        name: "filesystem".to_owned(),
        status: crate::agent::model::McpServerConnectionStatus::Connected,
        server_info: None,
        error: None,
        config: None,
        scope: Some("project".to_owned()),
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
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    app.conn = Some(Rc::new(crate::agent::client::AgentConnection::new(tx)));
    app.session_id = Some(crate::agent::model::SessionId::new("session-1"));
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
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    app.conn = Some(Rc::new(crate::agent::client::AgentConnection::new(tx)));
    app.session_id = Some(crate::agent::model::SessionId::new("session-1"));
    app.mcp.servers = vec![
        crate::agent::model::McpServerStatus {
            name: "filesystem".to_owned(),
            status: crate::agent::model::McpServerConnectionStatus::Connected,
            server_info: None,
            error: None,
            config: None,
            scope: Some("user".to_owned()),
            tools: Vec::new(),
        },
        crate::agent::model::McpServerStatus {
            name: "other".to_owned(),
            status: crate::agent::model::McpServerConnectionStatus::Connected,
            server_info: None,
            error: None,
            config: None,
            scope: Some("user".to_owned()),
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
        BridgeCommand::ReloadPlugins { session_id: "session-1".to_owned() }
    );
    assert!(app.mcp.in_flight);
    assert!(rx.try_recv().is_err());
}

#[test]
fn dynamic_mcp_config_remove_uses_sdk_set_servers_and_preserves_other_dynamic_servers() {
    let (_dir, mut app) = open_settings_test_app();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    app.conn = Some(Rc::new(crate::agent::client::AgentConnection::new(tx)));
    app.session_id = Some(crate::agent::model::SessionId::new("session-1"));
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
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    app.conn = Some(Rc::new(crate::agent::client::AgentConnection::new(tx)));
    app.session_id = Some(crate::agent::model::SessionId::new("session-1"));
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
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    app.conn = Some(Rc::new(crate::agent::client::AgentConnection::new(tx)));
    app.session_id = Some(crate::agent::model::SessionId::new("session-1"));
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
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    app.conn = Some(Rc::new(crate::agent::client::AgentConnection::new(tx)));
    app.session_id = Some(crate::agent::model::SessionId::new("session-1"));
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
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    app.conn = Some(Rc::new(crate::agent::client::AgentConnection::new(tx)));
    app.session_id = Some(crate::agent::model::SessionId::new("session-1"));
    app.config.active_tab = ConfigTab::Mcp;
    app.mcp.servers.push(crate::agent::model::McpServerStatus {
        name: "stale".to_owned(),
        status: crate::agent::model::McpServerConnectionStatus::NeedsAuth,
        server_info: None,
        error: None,
        config: None,
        scope: None,
        tools: Vec::new(),
    });

    handle_key(&mut app, KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));

    let envelope = rx.try_recv().expect("runtime reload command");
    assert_eq!(
        envelope.command,
        BridgeCommand::ReloadPlugins { session_id: "session-1".to_owned() }
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
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    app.conn = Some(Rc::new(crate::agent::client::AgentConnection::new(tx)));
    app.session_id = Some(crate::agent::model::SessionId::new("session-1"));
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
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    app.conn = Some(Rc::new(crate::agent::client::AgentConnection::new(tx)));
    app.session_id = Some(crate::agent::model::SessionId::new("session-1"));
    app.mcp.servers.push(crate::agent::model::McpServerStatus {
        name: "stale".to_owned(),
        status: crate::agent::model::McpServerConnectionStatus::Connected,
        server_info: None,
        error: None,
        config: None,
        scope: None,
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
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    app.conn = Some(Rc::new(crate::agent::client::AgentConnection::new(tx)));
    app.session_id = Some(crate::agent::model::SessionId::new("session-1"));
    app.config.active_tab = ConfigTab::Status;

    super::mcp::refresh_mcp_snapshot_if_needed(&mut app);

    assert!(rx.try_recv().is_err());
    assert!(!app.mcp.in_flight);
}

#[test]
fn claudeai_proxy_server_shows_disabled_authenticate_action() {
    let (_dir, app) = open_settings_test_app();
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
fn esc_closes_settings_without_editing_selected_row() {
    let (_dir, mut app) = open_settings_test_app();
    select_setting(&mut app, SettingId::FastMode);

    handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    assert_eq!(app.surface_mode, SurfaceMode::Chat);
    assert!(!app.config.fast_mode_effective());
}

#[test]
fn save_failure_keeps_previous_value_and_surfaces_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = App::test_default();
    app.settings_home_override = Some(dir.path().to_path_buf());
    app.cwd_raw = dir.path().to_string_lossy().to_string();

    open(&mut app).expect("open");
    app.config.settings_path = Some(PathBuf::new());
    select_setting(&mut app, SettingId::FastMode);

    handle_key(&mut app, KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));

    assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::Config));
    assert!(!app.config.fast_mode_effective());
    assert!(app.config.last_error.is_some());
    assert!(app.config.status_message.is_none());
}
