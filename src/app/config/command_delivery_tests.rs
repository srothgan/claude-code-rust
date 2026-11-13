// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::app::config::mcp::McpServerActionKind;

fn key(app: &mut App, code: KeyCode) {
    handle_key(app, KeyEvent::new(code, KeyModifiers::NONE));
}

fn open_target(app: &mut App) {
    let target = app.mcp.servers.iter().position(|s| s.name == "chosen-server").unwrap();
    while app.config.mcp_selected_server_index != target {
        key(
            app,
            if app.config.mcp_selected_server_index < target { KeyCode::Down } else { KeyCode::Up },
        );
    }
    key(app, KeyCode::Enter);
    assert_eq!(app.config.mcp_details_overlay().unwrap().server_name, "chosen-server");
}

fn assert_visible_status(app: &mut App, expected: &str) {
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 32)).unwrap();
    terminal.draw(|frame| crate::ui::render_fullscreen_surface(frame, app)).unwrap();
    let buffer = terminal.backend().buffer();
    let rows: Vec<String> =
        (0..32).map(|y| (0..100).map(|x| buffer[(x, y)].symbol()).collect()).collect();
    let title = rows
        .iter()
        .position(|r| r.contains("chosen-server") && r.contains('─'))
        .expect("selected details title");
    let chars: Vec<_> = rows[title].chars().collect();
    let left = chars
        .iter()
        .position(|c| matches!(c, '\u{256d}' | '\u{250c}'))
        .expect("details left border");
    let right = chars
        .iter()
        .rposition(|c| matches!(c, '\u{256e}' | '\u{2510}'))
        .expect("details right border");
    let region: Vec<String> = rows
        .iter()
        .skip(title + 1)
        .take_while(|row| {
            !row.chars().nth(left).is_some_and(|c| matches!(c, '\u{2570}' | '\u{2514}'))
        })
        .map(|row| row.chars().skip(left + 1).take(right - left - 1).collect())
        .collect();
    let status = region
        .iter()
        .find(|row| row.contains("Status") && row.contains(expected))
        .expect("status in selected details region");
    assert!(!status.contains("sibling-server"));
}

#[test]
fn selected_mcp_actions_target_the_named_server_and_wait_for_authoritative_snapshots() {
    let (_directory, mut app) = open_settings_test_app();
    let mut commands = attach_test_connection(&mut app);
    app.session_runtime.activate_session("mcp-session".into());
    app.config.active_tab = ConfigTab::Mcp;
    let sibling = connected_mcp_server_status("sibling-server", "project", None);
    let target = connected_mcp_server_status("chosen-server", "project", None);
    app.mcp.servers = vec![sibling.clone(), target];
    for (action, expected, reported, status_text) in [
        (
            McpServerActionKind::Reconnect,
            BridgeCommand::McpReconnect {
                session_id: "mcp-session".into(),
                server_name: "chosen-server".into(),
            },
            McpServerConnectionStatus::Failed,
            "failed",
        ),
        (
            McpServerActionKind::Disable,
            BridgeCommand::McpToggle {
                session_id: "mcp-session".into(),
                server_name: "chosen-server".into(),
                enabled: false,
            },
            McpServerConnectionStatus::Disabled,
            "disabled",
        ),
        (
            McpServerActionKind::Enable,
            BridgeCommand::McpToggle {
                session_id: "mcp-session".into(),
                server_name: "chosen-server".into(),
                enabled: true,
            },
            McpServerConnectionStatus::Connected,
            "connected",
        ),
    ] {
        open_target(&mut app);
        let server = app.mcp.servers.iter().find(|s| s.name == "chosen-server").unwrap();
        let actions = available_mcp_actions(&app, server);
        if server.status == McpServerConnectionStatus::Disabled {
            assert!(actions.contains(&McpServerActionKind::Enable));
            assert!(!actions.contains(&McpServerActionKind::Disable));
            assert!(!actions.contains(&McpServerActionKind::Reconnect));
        }
        let target_action = actions.iter().position(|a| *a == action).unwrap();
        for _ in 0..target_action {
            key(&mut app, KeyCode::Down);
        }
        key(&mut app, KeyCode::Enter);
        assert_eq!(commands.try_recv().unwrap().command, expected);
        assert_eq!(
            commands.try_recv().unwrap().command,
            BridgeCommand::GetMcpSnapshot { session_id: "mcp-session".into() }
        );
        assert!(commands.try_recv().is_err());
        assert!(app.mcp.in_flight);
        assert!(app.mcp.servers.is_empty(), "refresh must not invent an optimistic server state");
        let mut updated = connected_mcp_server_status("chosen-server", "project", None);
        updated.status = reported;
        crate::app::events::handle_client_event(
            &mut app,
            crate::agent::events::ClientEvent::McpSnapshotReceived {
                session_id: "mcp-session".into(),
                servers: vec![updated, sibling.clone()],
                auth_capabilities: crate::agent::model::McpAuthCapabilities::default(),
                source: Some(crate::agent::types::McpSnapshotSource::McpStatus),
                error: None,
            },
        );
        assert_eq!(app.mcp.servers.iter().find(|s| s.name == "sibling-server").unwrap(), &sibling);
        open_target(&mut app);
        assert_visible_status(&mut app, status_text);
        key(&mut app, KeyCode::Esc);
    }
}
