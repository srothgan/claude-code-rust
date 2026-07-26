// SPDX-License-Identifier: Apache-2.0

use super::super::App;
use crate::agent::{
    events::ClientEvent,
    model::{McpServerConnectionStatus, McpServerStatus, McpServerStatusConfig},
};

pub(super) fn handle(app: &mut App, event: ClientEvent) {
    match event {
        ClientEvent::McpElicitationRequest { session_id: _, request } => {
            crate::app::config::present_mcp_elicitation_request(app, request);
        }
        ClientEvent::McpElicitationCompleted { session_id: _, elicitation_id, server_name } => {
            crate::app::config::handle_mcp_elicitation_completed(app, &elicitation_id, server_name);
        }
        ClientEvent::McpElicitationResponseQueued { session_id: _, request_id } => {
            crate::app::config::handle_mcp_elicitation_response_queued(app, &request_id);
        }
        ClientEvent::McpAuthRedirect { session_id: _, redirect } => {
            crate::app::config::present_mcp_auth_redirect(app, redirect);
        }
        ClientEvent::McpOperationError { session_id: _, error } => {
            crate::app::config::handle_mcp_operation_error(app, &error);
        }
        ClientEvent::McpSetServersResult { session_id: _, result } => {
            crate::app::config::handle_mcp_set_servers_result(app, &result);
        }
        ClientEvent::McpConfigRemoveSucceeded { cwd_raw, server_name, scope, claude_path } => {
            if app.cwd_raw == cwd_raw {
                crate::app::config::apply_mcp_config_remove_success(
                    app,
                    &server_name,
                    &scope,
                    claude_path,
                );
            }
        }
        ClientEvent::McpConfigRemoveFailed { cwd_raw, server_name, scope, message } => {
            if app.cwd_raw == cwd_raw {
                crate::app::config::apply_mcp_config_remove_failure(
                    app,
                    &server_name,
                    &scope,
                    &message,
                );
            }
        }
        ClientEvent::McpSnapshotReceived {
            session_id,
            servers,
            auth_capabilities,
            source,
            error,
        } => {
            apply_snapshot(app, &session_id, servers, auth_capabilities, source, error);
        }
        _ => unreachable!("client event family routed a non-MCP event to the MCP handler"),
    }
}

fn apply_snapshot(
    app: &mut App,
    session_id: &str,
    mut servers: Vec<McpServerStatus>,
    auth_capabilities: crate::agent::model::McpAuthCapabilities,
    source: Option<crate::agent::types::McpSnapshotSource>,
    error: Option<String>,
) {
    let pending_dynamic_mcp_removal_confirmation =
        crate::app::config::pending_dynamic_mcp_removal_confirmation_from_snapshot(
            app,
            source,
            error.as_deref(),
            &servers,
        );
    let remove_confirmation_failures =
        crate::app::config::reconcile_removed_config_mcp_server_guards(
            app,
            source,
            error.as_deref(),
            &servers,
        );
    crate::app::config::filter_removed_config_mcp_servers(app, &mut servers);
    crate::app::config::filter_stale_plugin_mcp_servers(app, source, &mut servers);
    let server_count = servers.len();
    let error_present = error.is_some();
    let server_diagnostics = server_diagnostic_summaries(&servers);
    app.mcp.servers = servers;
    app.mcp.auth_capabilities = auth_capabilities;
    app.mcp.in_flight = false;
    app.mcp.last_error = error;
    app.config.mcp_selected_server_index =
        app.config.mcp_selected_server_index.min(app.mcp.servers.len().saturating_sub(1));
    reconcile_auth_overlay(app);
    crate::app::config::apply_pending_dynamic_mcp_removal_confirmation(
        app,
        pending_dynamic_mcp_removal_confirmation,
    );
    crate::app::config::apply_removed_config_mcp_server_confirmation_failures(
        app,
        remove_confirmation_failures,
    );
    tracing::info!(
        target: crate::logging::targets::APP_CONFIG,
        event_name = "mcp_snapshot_applied",
        message = "MCP snapshot applied",
        outcome = "success",
        session_id,
        source = ?source,
        server_count,
        error_present,
        servers = ?server_diagnostics,
    );
}

fn reconcile_auth_overlay(app: &mut App) {
    let Some(overlay) = app.config.mcp_auth_redirect_overlay() else {
        return;
    };
    let server_name = overlay.redirect.server_name.clone();
    let Some(server) = app.mcp.servers.iter().find(|server| server.name == server_name) else {
        return;
    };
    if matches!(
        server.status,
        McpServerConnectionStatus::NeedsAuth | McpServerConnectionStatus::Pending
    ) {
        return;
    }
    if matches!(server.status, McpServerConnectionStatus::Connected) {
        app.config.status_message = Some(format!("{} authenticated successfully.", server.name));
        app.config.last_error = None;
    }
    app.config.clear_overlay();
}

fn server_diagnostic_summaries(servers: &[McpServerStatus]) -> Vec<serde_json::Value> {
    servers
        .iter()
        .map(|server| {
            let (
                config_type,
                timeout_ms,
                request_timeout_ms,
                always_load,
                configured_tool_policy_count,
            ) = config_diagnostics(server.config.as_ref());
            serde_json::json!({
                "name": server.name,
                "status": status_label(server.status),
                "config_type": config_type,
                "scope": server.scope,
                "timeout_ms": timeout_ms,
                "request_timeout_ms": request_timeout_ms,
                "always_load": always_load,
                "tool_count": server.tools.len(),
                "configured_tool_policy_count": configured_tool_policy_count,
                "has_error": server.error.as_deref().is_some_and(|error| !error.is_empty()),
                "has_server_info": server.server_info.is_some(),
            })
        })
        .collect()
}

fn status_label(status: McpServerConnectionStatus) -> &'static str {
    match status {
        McpServerConnectionStatus::Connected => "connected",
        McpServerConnectionStatus::Failed => "failed",
        McpServerConnectionStatus::NeedsAuth => "needs-auth",
        McpServerConnectionStatus::Pending => "pending",
        McpServerConnectionStatus::Disabled => "disabled",
    }
}

fn config_diagnostics(
    config: Option<&McpServerStatusConfig>,
) -> (&'static str, Option<u64>, Option<u64>, Option<bool>, usize) {
    match config {
        Some(McpServerStatusConfig::Stdio { timeout, request_timeout_ms, always_load, .. }) => {
            ("stdio", *timeout, *request_timeout_ms, *always_load, 0)
        }
        Some(McpServerStatusConfig::Sse {
            tools,
            timeout,
            request_timeout_ms,
            always_load,
            ..
        }) => ("sse", *timeout, *request_timeout_ms, *always_load, tools.len()),
        Some(McpServerStatusConfig::Http {
            tools,
            timeout,
            request_timeout_ms,
            always_load,
            ..
        }) => ("http", *timeout, *request_timeout_ms, *always_load, tools.len()),
        Some(McpServerStatusConfig::Sdk { .. }) => ("sdk", None, None, None, 0),
        Some(McpServerStatusConfig::ClaudeaiProxy { timeout, .. }) => {
            ("claudeai-proxy", *timeout, None, None, 0)
        }
        Some(McpServerStatusConfig::Unknown { .. }) => ("unknown", None, None, None, 0),
        None => ("missing", None, None, None, 0),
    }
}
