// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::prelude::*;

pub(crate) fn refresh_mcp_snapshot_if_needed(app: &mut App) {
    if app.config.active_tab == ConfigTab::Mcp {
        refresh_mcp_snapshot(app);
    }
}

pub(crate) fn refresh_mcp_snapshot(app: &mut App) {
    app.mcp.servers.clear();
    app.mcp.last_error = None;
    request_mcp_snapshot(app);
}

pub(crate) fn request_mcp_snapshot(app: &mut App) {
    let Some(conn) = app.session_runtime.conn.as_ref() else {
        app.mcp.in_flight = false;
        return;
    };
    let Some(ref sid) = app.session_runtime.session_id else {
        app.mcp.in_flight = false;
        return;
    };
    let session_id = sid.to_string();
    app.mcp.in_flight = true;
    app.mcp.last_error = None;
    match conn.get_mcp_snapshot(session_id.clone()) {
        Ok(()) => tracing::debug!(
            target: crate::logging::targets::APP_CONFIG,
            event_name = "mcp_snapshot_requested",
            message = "MCP snapshot requested",
            outcome = "start",
            session_id = %session_id,
        ),
        Err(err) => {
            app.mcp.in_flight = false;
            app.mcp.last_error = Some(err.to_string());
            tracing::warn!(
                target: crate::logging::targets::APP_CONFIG,
                event_name = "mcp_snapshot_request_failed",
                message = "failed to request MCP snapshot",
                outcome = "failure",
                session_id = %session_id,
                error_message = %err,
            );
        }
    }
}

pub(crate) fn filter_removed_config_mcp_servers(
    app: &App,
    servers: &mut Vec<crate::agent::model::McpServerStatus>,
) {
    if app.mcp.removed_config_servers.is_empty() {
        return;
    }
    servers.retain(|server| !is_removed_config_mcp_server_suppressed(app, server));
}

pub(crate) fn filter_stale_plugin_mcp_servers(
    app: &App,
    source: Option<types::McpSnapshotSource>,
    servers: &mut Vec<crate::agent::model::McpServerStatus>,
) {
    let stale_server_names = stale_plugin_mcp_server_names(app, servers);
    suppress_stale_plugin_mcp_servers(source, servers, stale_server_names);
}

pub(crate) fn reconcile_stale_plugin_mcp_servers(app: &mut App) {
    let stale_server_names = stale_plugin_mcp_server_names(app, &app.mcp.servers);
    if stale_server_names.is_empty() {
        return;
    }

    suppress_stale_plugin_mcp_servers(None, &mut app.mcp.servers, stale_server_names);
    app.config.mcp_selected_server_index =
        app.config.mcp_selected_server_index.min(app.mcp.servers.len().saturating_sub(1));
    clear_missing_mcp_server_overlays(app);
}

fn stale_plugin_mcp_server_names(
    app: &App,
    servers: &[crate::agent::model::McpServerStatus],
) -> Vec<String> {
    servers
        .iter()
        .filter(|server| crate::app::plugins::is_stale_plugin_mcp_runtime_server(app, &server.name))
        .map(|server| server.name.clone())
        .collect()
}

fn suppress_stale_plugin_mcp_servers(
    source: Option<types::McpSnapshotSource>,
    servers: &mut Vec<crate::agent::model::McpServerStatus>,
    stale_server_names: Vec<String>,
) {
    if stale_server_names.is_empty() {
        return;
    }

    servers.retain(|server| {
        !stale_server_names.iter().any(|stale_name| mcp_server_name_eq(stale_name, &server.name))
    });
    for server_name in stale_server_names {
        tracing::info!(
            target: crate::logging::targets::APP_CONFIG,
            event_name = "mcp_stale_plugin_runtime_server_suppressed",
            message = "stale plugin-owned MCP server suppressed from MCP list",
            outcome = "suppressed",
            source = ?source,
            server_name = %server_name,
        );
    }
}

fn clear_missing_mcp_server_overlays(app: &mut App) {
    let missing_server_name = match app.config.overlay.as_ref() {
        Some(ConfigOverlayState::McpDetails(overlay)) => Some(overlay.server_name.as_str()),
        Some(ConfigOverlayState::McpCallbackUrl(overlay)) => Some(overlay.server_name.as_str()),
        Some(ConfigOverlayState::McpAuthRedirect(overlay)) => {
            Some(overlay.redirect.server_name.as_str())
        }
        Some(ConfigOverlayState::McpElicitation(overlay)) => {
            Some(overlay.request.server_name.as_str())
        }
        Some(
            ConfigOverlayState::Model(_)
            | ConfigOverlayState::ThinkingEffort(_)
            | ConfigOverlayState::OutputStyle(_)
            | ConfigOverlayState::Language(_)
            | ConfigOverlayState::SessionRename(_)
            | ConfigOverlayState::Confirmation(_)
            | ConfigOverlayState::InstalledPluginActions(_)
            | ConfigOverlayState::PluginInstallActions(_)
            | ConfigOverlayState::MarketplaceActions(_)
            | ConfigOverlayState::AddMarketplace(_),
        )
        | None => None,
    }
    .is_some_and(|server_name| {
        !app.mcp.servers.iter().any(|server| mcp_server_name_eq(&server.name, server_name))
    });

    if missing_server_name {
        app.config.clear_overlay();
    }
}

pub(crate) fn reconcile_removed_config_mcp_server_guards(
    app: &mut App,
    source: Option<types::McpSnapshotSource>,
    error: Option<&str>,
    servers: &[crate::agent::model::McpServerStatus],
) -> Vec<McpConfigRemoveConfirmationFailure> {
    let mut failures = Vec::new();
    if app.mcp.removed_config_servers.is_empty() || error.is_some() {
        return failures;
    }
    let Some(source) = source else {
        return failures;
    };
    app.mcp.removed_config_servers.retain(|removed_key, guard| {
        if guard.expected_source != source {
            return true;
        }

        if let Some(server) =
            servers.iter().find(|server| mcp_server_matches_removed_key(server, removed_key))
        {
            failures.push(McpConfigRemoveConfirmationFailure {
                server_name: server.name.clone(),
                scope: removed_key.scope.clone(),
                message: format!(
                    "Removal was reported as successful, but the confirming {} snapshot still contains the server.",
                    mcp_snapshot_source_label(source),
                ),
            });
        }

        false
    });
    failures
}

pub(crate) fn apply_removed_config_mcp_server_confirmation_failures(
    app: &mut App,
    failures: Vec<McpConfigRemoveConfirmationFailure>,
) {
    for failure in failures {
        apply_mcp_config_remove_failure(
            app,
            &failure.server_name,
            &failure.scope,
            &failure.message,
        );
    }
}
