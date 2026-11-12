// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::agent::types::McpSnapshotSource;
use pretty_assertions::assert_eq;
use std::collections::BTreeSet;

fn server(name: &str, scope: &str) -> model::McpServerStatus {
    model::McpServerStatus {
        name: name.into(),
        status: model::McpServerConnectionStatus::Connected,
        server_info: None,
        error: None,
        config: None,
        scope: Some(scope.into()),
        source: None,
        tools: vec![],
    }
}

fn snapshot(
    session: &str,
    source: McpSnapshotSource,
    include_removed: bool,
    error: Option<&str>,
) -> ClientEvent {
    let mut servers = vec![server("unrelated", "user"), server("chosen", "project")];
    if include_removed {
        servers.push(server("chosen", "user"));
    }
    ClientEvent::McpSnapshotReceived {
        session_id: session.into(),
        servers,
        auth_capabilities: model::McpAuthCapabilities::default(),
        source: Some(source),
        error: error.map(str::to_owned),
    }
}

fn identities(app: &App) -> BTreeSet<(&str, &str)> {
    app.mcp
        .servers
        .iter()
        .map(|server| (server.name.as_str(), server.scope.as_deref().unwrap()))
        .collect()
}

#[test]
fn removed_mcp_target_stays_suppressed_until_its_source_confirms_absence_then_can_reappear() {
    let mut app = make_test_app();
    app.mcp.removed_config_servers.insert(
        crate::app::state::types::RemovedMcpServerKey::new("user".into(), "chosen".into()),
        crate::app::state::types::RemovedMcpServerGuard {
            expected_source: McpSnapshotSource::ReloadPlugins,
        },
    );
    for event in [
        snapshot("stale-session", McpSnapshotSource::ReloadPlugins, false, None),
        snapshot("test-session", McpSnapshotSource::McpStatus, false, None),
        snapshot(
            "test-session",
            McpSnapshotSource::ReloadPlugins,
            false,
            Some("reload query failed"),
        ),
    ] {
        handle_client_event(&mut app, event);
        handle_client_event(
            &mut app,
            snapshot("test-session", McpSnapshotSource::McpStatus, true, None),
        );
        assert_eq!(
            identities(&app),
            BTreeSet::from([("unrelated", "user"), ("chosen", "project")]),
            "unconfirmed removal must suppress only its own scope"
        );
    }
    handle_client_event(
        &mut app,
        snapshot("test-session", McpSnapshotSource::ReloadPlugins, false, None),
    );
    handle_client_event(
        &mut app,
        snapshot("test-session", McpSnapshotSource::McpStatus, true, None),
    );
    assert_eq!(
        identities(&app),
        BTreeSet::from([("chosen", "user"), ("chosen", "project"), ("unrelated", "user")]),
        "confirmed absence permits a later valid addition"
    );
}

fn inventory(directory: &str, id: &str, version: &str) -> ClientEvent {
    let mut entry = installed_plugin_entry(id);
    entry.version = Some(version.into());
    ClientEvent::PluginsInventoryUpdated {
        cwd_raw: directory.into(),
        snapshot: crate::app::plugins::PluginsInventorySnapshot {
            installed: vec![entry],
            marketplace: vec![],
            marketplaces: vec![],
        },
        claude_path: std::path::PathBuf::from("fixture-cli"),
    }
}

#[test]
fn plugin_inventory_rejects_old_directory_data_and_accepts_the_current_directorys_new_result() {
    let mut app = make_test_app();
    app.cwd_raw = "/current-project".into();
    handle_client_event(&mut app, inventory("/current-project", "current-plugin", "1.2.3"));
    handle_client_event(&mut app, inventory("/old-project", "foreign-plugin", "9.9.9"));
    assert!(
        matches!(app.plugins.installed.as_slice(), [plugin] if plugin.id == "current-plugin" && plugin.version.as_deref() == Some("1.2.3"))
    );
    handle_client_event(&mut app, inventory("/current-project", "replacement-plugin", "4.5.6"));
    assert!(
        matches!(app.plugins.installed.as_slice(), [plugin] if plugin.id == "replacement-plugin" && plugin.version.as_deref() == Some("4.5.6"))
    );
}

#[test]
fn mcp_operation_error_keeps_target_and_cause_on_its_surface_without_changing_chat() {
    let mut app = make_test_app();
    app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::Config);
    app.config.active_tab = crate::app::ConfigTab::Mcp;
    app.config.status_message = Some("auth request in progress".into());
    app.mcp.in_flight = true;
    app.mcp.servers = vec![server("calendar", "user")];
    app.push_message_tracked(user_msg("existing chat"));
    let original = app.transcript.messages.iter().map(|message| message.id).collect::<Vec<_>>();
    handle_client_event(
        &mut app,
        ClientEvent::McpOperationError {
            session_id: "test-session".into(),
            error: crate::agent::types::McpOperationError {
                server_name: Some("calendar".into()),
                operation: "authenticate".into(),
                message: "fixture OAuth unavailable".into(),
            },
        },
    );
    for feedback in [app.mcp.last_error.as_deref(), app.config.last_error.as_deref()] {
        let feedback = feedback.expect("owning surface feedback");
        for part in ["calendar", "authenticate", "fixture OAuth unavailable"] {
            assert!(feedback.contains(part), "{feedback}");
        }
    }
    assert!(app.config.status_message.is_none());
    assert!(!app.mcp.in_flight);
    assert_eq!(
        app.transcript.messages.iter().map(|message| message.id).collect::<Vec<_>>(),
        original
    );
    assert!(
        matches!(app.transcript.messages[0].blocks.as_slice(), [MessageBlock::Text(text)] if text.text == "existing chat")
    );
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(180, 36)).unwrap();
    terminal.draw(|frame| crate::ui::render_fullscreen_surface(frame, &mut app)).unwrap();
    let feedback_rows = (29..35)
        .map(|y| (2..178).map(|x| terminal.backend().buffer()[(x, y)].symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        feedback_rows.contains("calendar") && feedback_rows.contains("fixture OAuth unavailable"),
        "{feedback_rows}"
    );
}
