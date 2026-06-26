// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::prelude::*;

pub(crate) fn handle_mcp_operation_error(
    app: &mut App,
    error: &crate::agent::types::McpOperationError,
) {
    if error.operation == "set-servers"
        && let Some(server_name) = app.mcp.pending_dynamic_config_removal.take()
    {
        apply_mcp_config_remove_failure(
            app,
            &server_name,
            McpConfigScope::Dynamic.cli_arg(),
            &error.message,
        );
        return;
    }

    app.mcp.in_flight = false;
    let formatted = format_mcp_operation_error(error);
    app.mcp.last_error = Some(formatted.clone());
    if app.config.overlay.is_some() {
        app.config.set_overlay_error(formatted);
    } else {
        app.config.last_error = Some(formatted);
        app.config.status_message = None;
    }
    tracing::error!(
        target: crate::logging::targets::APP_CONFIG,
        event_name = "mcp_operation_error_applied",
        message = "MCP operation error applied",
        outcome = "failure",
        server_name = %error.server_name.as_deref().unwrap_or(""),
        operation = %error.operation,
        error_message = %error.message,
    );
}

fn format_mcp_operation_error(error: &crate::agent::types::McpOperationError) -> String {
    let action = match error.operation.as_str() {
        "authenticate" => "authenticate",
        "clear-auth" => "clear auth for",
        "reconnect" => "reconnect",
        "set-servers" => "update dynamic config for",
        "toggle" => "update",
        "submit-callback-url" => "submit callback URL for",
        other => other,
    };
    match error.server_name.as_deref() {
        Some(server_name) => {
            format!("Failed to {action} MCP server {server_name}: {}", error.message)
        }
        None => format!("MCP operation failed ({action}): {}", error.message),
    }
}
