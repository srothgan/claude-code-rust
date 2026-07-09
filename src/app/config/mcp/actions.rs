// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use super::prelude::*;

pub(crate) fn handle_mcp_key(app: &mut App, key: KeyEvent) -> bool {
    if app.config.active_tab != ConfigTab::Mcp {
        return false;
    }

    match (key.code, key.modifiers) {
        (KeyCode::Char(ch), modifiers)
            if matches!(ch, 'r' | 'R')
                && (modifiers.is_empty() || modifiers == KeyModifiers::SHIFT) =>
        {
            crate::app::session_runtime::request_runtime_reload(app);
            refresh_mcp_snapshot(app);
            true
        }
        (KeyCode::Enter, KeyModifiers::NONE) => {
            open_selected_mcp_server_details(app);
            true
        }
        (KeyCode::Up, KeyModifiers::NONE) => {
            app.config.mcp_selected_server_index =
                app.config.mcp_selected_server_index.saturating_sub(1);
            true
        }
        (KeyCode::Down, KeyModifiers::NONE) => {
            let last_index = app.mcp.servers.len().saturating_sub(1);
            app.config.mcp_selected_server_index =
                (app.config.mcp_selected_server_index + 1).min(last_index);
            true
        }
        _ => false,
    }
}

pub(crate) fn reconnect_mcp_server(app: &mut App, server_name: &str) {
    let Some(conn) = app.session_runtime.conn.as_ref() else {
        return;
    };
    let Some(ref sid) = app.session_runtime.session_id else {
        return;
    };
    let session_id = sid.to_string();
    match conn.reconnect_mcp_server(session_id.clone(), server_name.to_owned()) {
        Ok(()) => {
            tracing::info!(
                target: crate::logging::targets::APP_CONFIG,
                event_name = "mcp_reconnect_requested",
                message = "MCP reconnect requested",
                outcome = "start",
                session_id = %session_id,
                server_name = %server_name,
            );
            refresh_mcp_snapshot(app);
        }
        Err(error) => tracing::warn!(
            target: crate::logging::targets::APP_CONFIG,
            event_name = "mcp_reconnect_request_failed",
            message = "failed to request MCP reconnect",
            outcome = "failure",
            session_id = %session_id,
            server_name = %server_name,
            error_message = %error,
        ),
    }
}

pub(crate) fn set_mcp_server_enabled(app: &mut App, server_name: &str, enabled: bool) {
    let Some(conn) = app.session_runtime.conn.as_ref() else {
        return;
    };
    let Some(ref sid) = app.session_runtime.session_id else {
        return;
    };
    let session_id = sid.to_string();
    match conn.toggle_mcp_server(session_id.clone(), server_name.to_owned(), enabled) {
        Ok(()) => {
            tracing::info!(
                target: crate::logging::targets::APP_CONFIG,
                event_name = "mcp_toggle_requested",
                message = "MCP server toggle requested",
                outcome = "start",
                session_id = %session_id,
                server_name = %server_name,
                enabled,
            );
            refresh_mcp_snapshot(app);
        }
        Err(error) => tracing::warn!(
            target: crate::logging::targets::APP_CONFIG,
            event_name = "mcp_toggle_request_failed",
            message = "failed to request MCP server toggle",
            outcome = "failure",
            session_id = %session_id,
            server_name = %server_name,
            enabled,
            error_message = %error,
        ),
    }
}

pub(crate) fn authenticate_mcp_server(app: &mut App, server_name: &str) {
    let Some(conn) = app.session_runtime.conn.as_ref() else {
        return;
    };
    let Some(ref sid) = app.session_runtime.session_id else {
        return;
    };
    let session_id = sid.to_string();
    match conn.authenticate_mcp_server(session_id.clone(), server_name.to_owned()) {
        Ok(()) => {
            tracing::info!(
                target: crate::logging::targets::APP_CONFIG,
                event_name = "mcp_authenticate_requested",
                message = "MCP authentication requested",
                outcome = "start",
                session_id = %session_id,
                server_name = %server_name,
            );
            app.config.status_message = Some(format!("Starting MCP auth for {server_name}..."));
            app.config.last_error = None;
            refresh_mcp_snapshot(app);
        }
        Err(error) => tracing::warn!(
            target: crate::logging::targets::APP_CONFIG,
            event_name = "mcp_authenticate_request_failed",
            message = "failed to request MCP authentication",
            outcome = "failure",
            session_id = %session_id,
            server_name = %server_name,
            error_message = %error,
        ),
    }
}

pub(crate) fn clear_mcp_server_auth(app: &mut App, server_name: &str) {
    let Some(conn) = app.session_runtime.conn.as_ref() else {
        return;
    };
    let Some(ref sid) = app.session_runtime.session_id else {
        return;
    };
    let session_id = sid.to_string();
    match conn.clear_mcp_auth(session_id.clone(), server_name.to_owned()) {
        Ok(()) => {
            tracing::info!(
                target: crate::logging::targets::APP_CONFIG,
                event_name = "mcp_clear_auth_requested",
                message = "MCP auth clear requested",
                outcome = "start",
                session_id = %session_id,
                server_name = %server_name,
            );
            refresh_mcp_snapshot(app);
        }
        Err(error) => tracing::warn!(
            target: crate::logging::targets::APP_CONFIG,
            event_name = "mcp_clear_auth_request_failed",
            message = "failed to request MCP auth clear",
            outcome = "failure",
            session_id = %session_id,
            server_name = %server_name,
            error_message = %error,
        ),
    }
}

pub(crate) fn available_mcp_actions(
    app: &App,
    server: &crate::agent::model::McpServerStatus,
) -> Vec<McpServerActionKind> {
    let mut actions = vec![McpServerActionKind::RefreshSnapshot];
    if matches!(server.status, crate::agent::model::McpServerConnectionStatus::Disabled) {
        actions.push(McpServerActionKind::Enable);
    } else {
        if matches!(
            server.status,
            crate::agent::model::McpServerConnectionStatus::NeedsAuth
                | crate::agent::model::McpServerConnectionStatus::Failed
                | crate::agent::model::McpServerConnectionStatus::Pending
        ) {
            actions.push(McpServerActionKind::Authenticate);
        }
        actions.push(McpServerActionKind::ClearAuth);
        actions.push(McpServerActionKind::Reconnect);
        actions.push(McpServerActionKind::Disable);
    }
    match mcp_server_ownership(app, server) {
        McpServerOwnership::Persisted(scope) => {
            actions.push(remove_action_for_mcp_config_scope(scope));
        }
        McpServerOwnership::SdkDynamic => {
            actions.push(McpServerActionKind::RemoveDynamicConfig);
        }
        McpServerOwnership::PluginOwned(_) => {
            actions.push(McpServerActionKind::ManagePlugin);
        }
        McpServerOwnership::PluginOwnedUnknown | McpServerOwnership::RuntimeOnly => {}
    }
    actions
}

#[must_use]
pub(crate) fn is_mcp_action_available(
    app: &App,
    server: &crate::agent::model::McpServerStatus,
    action: McpServerActionKind,
) -> bool {
    match action {
        McpServerActionKind::Authenticate => !matches!(
            server.config.as_ref(),
            Some(crate::agent::model::McpServerStatusConfig::ClaudeaiProxy { .. })
        ),
        McpServerActionKind::RemoveUserConfig
        | McpServerActionKind::RemoveLocalConfig
        | McpServerActionKind::RemoveProjectConfig
        | McpServerActionKind::RemoveDynamicConfig => {
            action.mcp_config_scope() == mcp_config_removal_scope(app, server)
        }
        McpServerActionKind::ManagePlugin => {
            matches!(mcp_server_ownership(app, server), McpServerOwnership::PluginOwned(_))
        }
        McpServerActionKind::RefreshSnapshot
        | McpServerActionKind::ClearAuth
        | McpServerActionKind::Reconnect
        | McpServerActionKind::Enable
        | McpServerActionKind::Disable => true,
    }
}
