// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::agent::model::{McpServerConnectionStatus, McpServerStatus, McpServerStatusConfig};
use crate::agent::wire::BridgeCommand;
use crate::app::{App, SurfaceMode};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;
use tempfile::TempDir;

#[path = "shell_audit_tests.rs"]
mod shell_audit;

#[path = "mcp_audit_tests.rs"]
mod mcp_audit;

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

/// A Status-tab session whose Claude home and project live in `TempDir`.
fn app_with_status_connection() -> (TempDir, App, crate::agent::client::CommandReceiver) {
    let dir = tempfile::tempdir().expect("tempdir");
    let project = dir.path().join("project");
    std::fs::create_dir(&project).expect("project");
    let mut app = App::test_default();
    app.settings_home_override = Some(dir.path().join("home"));
    app.cwd_raw = project.to_string_lossy().into_owned();
    let rx = attach_test_connection(&mut app);
    app.session_runtime.session_id = Some(crate::agent::model::SessionId::new("session-1"));
    // The live title the bridge reported; the listed one may lag behind it.
    app.session_runtime.session_title = Some("Current custom title".to_owned());
    app.config.active_tab = ConfigTab::Status;
    app.recent_sessions = vec![crate::app::RecentSessionInfo {
        session_id: "session-1".to_owned(),
        summary: "Existing session summary".to_owned(),
        last_modified_ms: 0,
        file_size_bytes: 0,
        cwd: Some("/test".to_owned()),
        git_branch: None,
        custom_title: Some("Stale listed title".to_owned()),
        first_prompt: Some("First prompt".to_owned()),
    }];
    (dir, app, rx)
}

/// Writes the transcript Claude Code saves for `session_id`, at the path the
/// Agent SDK reads: every non-alphanumeric character of the resolved project
/// path becomes `-` (the fixture path is ASCII).
fn save_transcript(dir: &TempDir, session_id: &str) -> PathBuf {
    let project = std::fs::canonicalize(dir.path().join("project")).expect("project path");
    let name: String = project
        .to_string_lossy()
        // Node's realpath omits Rust's Windows verbatim prefix.
        .trim_start_matches(r"\\?\")
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect();
    let folder = dir.path().join("home/.claude/projects").join(name);
    std::fs::create_dir_all(&folder).expect("project folder");
    let transcript = folder.join(format!("{session_id}.jsonl"));
    std::fs::write(&transcript, "{}\n").expect("transcript");
    transcript
}

fn rename_commands(rx: &mut crate::agent::client::CommandReceiver) -> Vec<BridgeCommand> {
    std::iter::from_fn(|| rx.try_recv().ok())
        .map(|envelope| envelope.command)
        .filter(|command| matches!(command, BridgeCommand::RenameSession { .. }))
        .collect()
}

#[tokio::test(flavor = "current_thread")]
async fn leaving_config_discards_unsaved_rename_and_reopens_the_current_title() {
    tokio::task::LocalSet::new().run_until(async {
        let (dir, mut app, mut commands) = app_with_status_connection();
        let transcript = save_transcript(&dir, "session-1");
        let before = std::fs::read(&transcript).unwrap();
        app.input.set_text("unsent rename-neighbor 界");
        open_tab(&mut app, ConfigTab::Status).unwrap();
        press(&mut app, KeyCode::Char('r'));
        assert_eq!(app.config.session_rename_overlay().unwrap().draft, "Current custom title");
        assert!(handle_paste(&mut app, " UNSAVED_ORCHID"));
        assert_eq!(app.config.session_rename_overlay().unwrap().draft, "Current custom title UNSAVED_ORCHID");
        crate::app::handle_terminal_event(&mut app, Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)));
        assert_eq!(app.surface_mode, SurfaceMode::Chat);
        assert!(rename_commands(&mut commands).is_empty());
        assert_eq!(std::fs::read(&transcript).unwrap(), before);
        assert_eq!(app.active_session_custom_title(), Some("Current custom title"));
        assert_eq!(app.input.text(), "unsent rename-neighbor 界");
        open_tab(&mut app, ConfigTab::Status).unwrap();
        press(&mut app, KeyCode::Char('r'));
        assert_eq!(app.config.session_rename_overlay().unwrap().draft, "Current custom title");
        assert!(app.config.last_error.is_none());
        assert!(app.config.overlay_message.is_none());
        assert!(handle_paste(&mut app, "!"));
        press(&mut app, KeyCode::Enter);
        let sent = rename_commands(&mut commands);
        assert!(matches!(&sent[..], [BridgeCommand::RenameSession { session_id, title }] if session_id == "session-1" && title == "Current custom title!"), "{sent:?}");
        assert_eq!(app.input.text(), "unsent rename-neighbor 界");
    }).await;
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

fn press(app: &mut App, code: KeyCode) {
    handle_key(app, KeyEvent::new(code, KeyModifiers::NONE));
}

fn turn_complete(session_id: &str) -> crate::agent::events::ClientEvent {
    crate::agent::events::ClientEvent::TurnComplete {
        session_id: session_id.to_owned(),
        queued_turn_count: None,
        terminal_reason: None,
    }
}

#[tokio::test(flavor = "current_thread")]
async fn clear_rechecks_the_session_that_replaces_a_saved_one() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (dir, mut app, mut rx) = app_with_status_connection();
            save_transcript(&dir, "session-1");
            open_tab(&mut app, ConfigTab::Status).expect("status tab");
            assert_eq!(app.config.session_transcript, Some(SessionTranscriptState::Saved));

            // A session change, as `/clear` makes, to a session with no transcript yet.
            let cwd = app.cwd_raw.clone();
            crate::app::handle_client_event(
                &mut app,
                crate::agent::events::ClientEvent::SessionReplaced {
                    session_id: crate::agent::model::SessionId::new("session-2"),
                    cwd,
                    current_model: crate::agent::model::CurrentModel::new(
                        "sonnet", "Sonnet", "Sonnet",
                    ),
                    available_models: vec![],
                    mode: None,
                    fast_mode_state: crate::agent::model::FastModeState::Off,
                    fast_mode_disabled_reason: None,
                    ultracode: None,
                    history_updates: vec![],
                    restored_input: None,
                },
            );
            assert_eq!(app.config.session_transcript, Some(SessionTranscriptState::NotSaved));

            press(&mut app, KeyCode::Char('r'));
            press(&mut app, KeyCode::Enter);
            assert!(app.config.session_rename_overlay().is_some());
            assert!(rename_commands(&mut rx).is_empty());

            save_transcript(&dir, "session-2");
            crate::app::handle_client_event(&mut app, turn_complete("session-2"));
            press(&mut app, KeyCode::Enter);
            assert!(matches!(
                rename_commands(&mut rx).as_slice(),
                [BridgeCommand::RenameSession { session_id, .. }] if session_id == "session-2"
            ));
        })
        .await;
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

#[path = "command_delivery_tests.rs"]
mod command_delivery_tests;
