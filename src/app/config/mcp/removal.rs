// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::prelude::*;

pub(crate) fn remove_mcp_server_from_config(
    app: &mut App,
    server_name: &str,
    scope: McpConfigScope,
) {
    if !is_mcp_config_removal_available(app, server_name, scope) {
        let message = format!(
            "MCP server {server_name} is not removable from {} config. Plugin-owned MCP servers must be managed from the plugin action menu.",
            scope.cli_arg()
        );
        apply_mcp_config_remove_failure(app, server_name, scope.cli_arg(), &message);
        return;
    }
    if scope == McpConfigScope::Dynamic {
        remove_dynamic_mcp_server_from_config(app, server_name);
        return;
    }
    remove_persisted_mcp_server_from_config(app, server_name, scope);
}

fn remove_persisted_mcp_server_from_config(
    app: &mut App,
    server_name: &str,
    scope: McpConfigScope,
) {
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    let cwd_raw = app.cwd_raw.clone();
    let cached_claude_path = app.mcp.claude_path.clone();
    let server_name = server_name.to_owned();
    let scope_name = scope.cli_arg().to_owned();
    let args = vec![
        "mcp".to_owned(),
        "remove".to_owned(),
        "--scope".to_owned(),
        scope_name.clone(),
        server_name.clone(),
    ];
    let event_tx = app.event_tx.clone();

    app.mcp.in_flight = true;
    app.mcp.last_error = None;
    app.config.last_error = None;
    app.config.status_message =
        Some(format!("Removing MCP server {server_name} from {scope_name} config..."));

    tokio::task::spawn_local(async move {
        match crate::app::claude_cli::run_command_task(cwd_raw.clone(), cached_claude_path, args)
            .await
        {
            Ok(claude_path) => {
                let _ = event_tx.send(ClientEvent::McpConfigRemoveSucceeded {
                    cwd_raw,
                    server_name,
                    scope: scope_name,
                    claude_path,
                });
            }
            Err(message) => {
                let _ = event_tx.send(ClientEvent::McpConfigRemoveFailed {
                    cwd_raw,
                    server_name,
                    scope: scope_name,
                    message,
                });
            }
        }
    });
}

fn remove_dynamic_mcp_server_from_config(app: &mut App, server_name: &str) {
    if app.mcp.pending_dynamic_config_removal.is_some() {
        let formatted = format!(
            "Failed to remove MCP server {server_name} from dynamic config: Another dynamic MCP removal is still in progress."
        );
        app.mcp.last_error = Some(formatted.clone());
        app.config.last_error = Some(formatted);
        app.config.status_message = None;
        return;
    }
    let Some(conn) = app.session_runtime.conn.clone() else {
        apply_mcp_config_remove_failure(
            app,
            server_name,
            McpConfigScope::Dynamic.cli_arg(),
            "No active bridge connection.",
        );
        return;
    };
    let Some(session_id) = app.session_runtime.session_id.as_ref().map(ToString::to_string) else {
        apply_mcp_config_remove_failure(
            app,
            server_name,
            McpConfigScope::Dynamic.cli_arg(),
            "No active session.",
        );
        return;
    };
    let remaining_servers = match dynamic_mcp_servers_without(app, server_name) {
        Ok(servers) => servers,
        Err(message) => {
            apply_mcp_config_remove_failure(
                app,
                server_name,
                McpConfigScope::Dynamic.cli_arg(),
                &message,
            );
            return;
        }
    };

    app.mcp.in_flight = true;
    app.mcp.last_error = None;
    app.config.last_error = None;
    app.config.status_message =
        Some(format!("Removing MCP server {server_name} from dynamic config..."));
    app.mcp.pending_dynamic_config_removal = Some(server_name.to_owned());

    match conn.set_mcp_servers(session_id.clone(), remaining_servers) {
        Ok(()) => {
            tracing::info!(
                target: crate::logging::targets::APP_CONFIG,
                event_name = "mcp_dynamic_remove_requested",
                message = "dynamic MCP removal requested",
                outcome = "start",
                session_id = %session_id,
                server_name = %server_name,
            );
        }
        Err(error) => {
            app.mcp.pending_dynamic_config_removal = None;
            tracing::warn!(
                target: crate::logging::targets::APP_CONFIG,
                event_name = "mcp_dynamic_remove_request_failed",
                message = "failed to request dynamic MCP removal",
                outcome = "failure",
                session_id = %session_id,
                server_name = %server_name,
                error_message = %error,
            );
            apply_mcp_config_remove_failure(
                app,
                server_name,
                McpConfigScope::Dynamic.cli_arg(),
                &error.to_string(),
            );
        }
    }
}

fn dynamic_mcp_servers_without(
    app: &App,
    server_name: &str,
) -> Result<BTreeMap<String, types::McpServerConfig>, String> {
    let removed_key = normalized_removed_config_key(McpConfigScope::Dynamic.cli_arg(), server_name);
    let mut found_removed_server = false;
    let mut servers = BTreeMap::new();

    for server in app
        .mcp
        .servers
        .iter()
        .filter(|server| mcp_config_removal_scope(app, server) == Some(McpConfigScope::Dynamic))
    {
        if mcp_server_matches_removed_key(server, &removed_key) {
            found_removed_server = true;
            continue;
        }
        let config = dynamic_mcp_server_config_for_set_servers(server)?;
        servers.insert(server.name.clone(), config);
    }

    if !found_removed_server {
        return Err(format!("MCP server {server_name} is no longer present in dynamic config."));
    }

    Ok(servers)
}

fn dynamic_mcp_server_config_for_set_servers(
    server: &model::McpServerStatus,
) -> Result<types::McpServerConfig, String> {
    let Some(config) = server.config.as_ref() else {
        return Err(format!(
            "Cannot safely preserve dynamic MCP server {} because the SDK snapshot did not include its config.",
            server.name
        ));
    };

    match config {
        model::McpServerStatusConfig::Stdio {
            command,
            args,
            env,
            timeout,
            request_timeout_ms,
            always_load,
        } => Ok(types::McpServerConfig::Stdio {
            command: command.clone(),
            args: args.clone(),
            env: env.clone(),
            timeout: *timeout,
            request_timeout_ms: *request_timeout_ms,
            always_load: *always_load,
        }),
        model::McpServerStatusConfig::Sse {
            url,
            headers,
            tools,
            timeout,
            request_timeout_ms,
            always_load,
        } => Ok(types::McpServerConfig::Sse {
            url: url.clone(),
            headers: headers.clone(),
            tools: tools.iter().map(dynamic_mcp_tool_policy_for_set_servers).collect(),
            timeout: *timeout,
            request_timeout_ms: *request_timeout_ms,
            always_load: *always_load,
        }),
        model::McpServerStatusConfig::Http {
            url,
            headers,
            tools,
            timeout,
            request_timeout_ms,
            always_load,
        } => Ok(types::McpServerConfig::Http {
            url: url.clone(),
            headers: headers.clone(),
            tools: tools.iter().map(dynamic_mcp_tool_policy_for_set_servers).collect(),
            timeout: *timeout,
            request_timeout_ms: *request_timeout_ms,
            always_load: *always_load,
        }),
        model::McpServerStatusConfig::Sdk { .. } => Err(format!(
            "Cannot safely preserve dynamic MCP server {} because SDK-server instances cannot be represented by the Rust bridge.",
            server.name
        )),
        model::McpServerStatusConfig::ClaudeaiProxy { .. } => Err(format!(
            "Cannot safely preserve dynamic MCP server {} because Claude.ai proxy servers are not dynamic SDK configs.",
            server.name
        )),
        model::McpServerStatusConfig::Unknown { raw_type } => Err(format!(
            "Cannot safely preserve dynamic MCP server {} because its config type {raw_type} is unknown.",
            server.name
        )),
    }
}

fn dynamic_mcp_tool_policy_for_set_servers(
    policy: &model::McpServerToolPolicy,
) -> types::McpServerToolPolicy {
    types::McpServerToolPolicy {
        name: policy.name.clone(),
        permission_policy: policy
            .permission_policy
            .map(dynamic_mcp_tool_permission_policy_for_set_servers),
        org_max_permission: policy
            .org_max_permission
            .map(dynamic_mcp_org_permission_for_set_servers),
    }
}

const fn dynamic_mcp_tool_permission_policy_for_set_servers(
    policy: model::McpServerToolPermissionPolicy,
) -> types::McpServerToolPermissionPolicy {
    match policy {
        model::McpServerToolPermissionPolicy::Allow => types::McpServerToolPermissionPolicy::Allow,
        model::McpServerToolPermissionPolicy::Ask => types::McpServerToolPermissionPolicy::Ask,
        model::McpServerToolPermissionPolicy::Deny => types::McpServerToolPermissionPolicy::Deny,
    }
}

const fn dynamic_mcp_org_permission_for_set_servers(
    permission: model::McpServerOrgMaxPermission,
) -> types::McpServerOrgMaxPermission {
    match permission {
        model::McpServerOrgMaxPermission::Allow => types::McpServerOrgMaxPermission::Allow,
        model::McpServerOrgMaxPermission::Ask => types::McpServerOrgMaxPermission::Ask,
        model::McpServerOrgMaxPermission::Blocked => types::McpServerOrgMaxPermission::Blocked,
    }
}

pub(crate) fn apply_mcp_config_remove_success(
    app: &mut App,
    server_name: &str,
    scope: &str,
    claude_path: std::path::PathBuf,
) {
    let scope_name = McpConfigScope::from_status_scope(scope)
        .map_or_else(|| normalize_mcp_config_scope_key(scope), |scope| scope.cli_arg().to_owned());
    app.mcp.claude_path = Some(claude_path);
    apply_mcp_config_remove_success_state(
        app,
        server_name,
        &scope_name,
        Some(types::McpSnapshotSource::ReloadPlugins),
    );
    match crate::app::session_runtime::request_runtime_reload(app) {
        crate::app::session_runtime::RuntimeReloadRequestOutcome::Requested => {
            app.mcp.in_flight = true;
            app.mcp.last_error = None;
        }
        crate::app::session_runtime::RuntimeReloadRequestOutcome::Unavailable => {
            app.mcp.in_flight = false;
        }
        crate::app::session_runtime::RuntimeReloadRequestOutcome::Failed => {
            app.mcp.in_flight = false;
            app.mcp.last_error = Some("failed to request session runtime plugin reload".to_owned());
        }
    }
}

fn apply_mcp_dynamic_config_remove_success(app: &mut App, server_name: &str) {
    apply_mcp_config_remove_success_state(
        app,
        server_name,
        McpConfigScope::Dynamic.cli_arg(),
        Some(types::McpSnapshotSource::McpSetServers),
    );
}

pub(crate) fn handle_mcp_set_servers_result(app: &mut App, result: &types::McpSetServersResult) {
    let Some(server_name) = app.mcp.pending_dynamic_config_removal.clone() else {
        return;
    };
    if let Some((_, message)) =
        result.errors.iter().find(|(name, _)| name.eq_ignore_ascii_case(&server_name))
    {
        app.mcp.pending_dynamic_config_removal = None;
        apply_mcp_config_remove_failure(
            app,
            &server_name,
            McpConfigScope::Dynamic.cli_arg(),
            message,
        );
        return;
    }

    if !result.removed.iter().any(|removed| mcp_server_name_eq(removed, &server_name)) {
        tracing::info!(
            target: crate::logging::targets::APP_CONFIG,
            event_name = "mcp_dynamic_remove_pending_confirmation",
            message = "dynamic MCP removal waiting for confirming snapshot",
            outcome = "pending",
            server_name = %server_name,
            added_count = result.added.len(),
            removed_count = result.removed.len(),
            error_count = result.errors.len(),
        );
        app.mcp.in_flight = true;
        app.config.status_message = Some(format!(
            "Removing MCP server {server_name} from dynamic config... Waiting for SDK confirmation."
        ));
        return;
    }

    app.mcp.pending_dynamic_config_removal = None;
    tracing::info!(
        target: crate::logging::targets::APP_CONFIG,
        event_name = "mcp_dynamic_remove_completed",
        message = "dynamic MCP removal completed",
        outcome = "success",
        server_name = %server_name,
        added_count = result.added.len(),
        removed_count = result.removed.len(),
        error_count = result.errors.len(),
    );
    apply_mcp_dynamic_config_remove_success(app, &server_name);
}

pub(crate) fn pending_dynamic_mcp_removal_confirmation_from_snapshot(
    app: &App,
    source: Option<types::McpSnapshotSource>,
    error: Option<&str>,
    servers: &[crate::agent::model::McpServerStatus],
) -> Option<PendingDynamicMcpRemovalConfirmation> {
    if source != Some(types::McpSnapshotSource::McpSetServers) {
        return None;
    }

    let server_name = app.mcp.pending_dynamic_config_removal.as_ref()?.clone();
    if let Some(message) = error {
        return Some(PendingDynamicMcpRemovalConfirmation::Failed {
            server_name,
            message: format!("SDK snapshot failed after setMcpServers: {message}"),
        });
    }

    if servers.iter().any(|server| mcp_server_name_eq(&server.name, &server_name)) {
        return Some(PendingDynamicMcpRemovalConfirmation::Failed {
            server_name,
            message: "SDK setMcpServers completed without reporting an error, but the confirming snapshot still contains the server.".to_owned(),
        });
    }

    Some(PendingDynamicMcpRemovalConfirmation::Confirmed { server_name })
}

pub(crate) fn apply_pending_dynamic_mcp_removal_confirmation(
    app: &mut App,
    confirmation: Option<PendingDynamicMcpRemovalConfirmation>,
) {
    let Some(confirmation) = confirmation else {
        return;
    };

    match confirmation {
        PendingDynamicMcpRemovalConfirmation::Confirmed { server_name } => {
            if app
                .mcp
                .pending_dynamic_config_removal
                .as_deref()
                .is_some_and(|pending| mcp_server_name_eq(pending, &server_name))
            {
                app.mcp.pending_dynamic_config_removal = None;
            }
            tracing::info!(
                target: crate::logging::targets::APP_CONFIG,
                event_name = "mcp_dynamic_remove_confirmed",
                message = "dynamic MCP removal confirmed by snapshot",
                outcome = "success",
                server_name = %server_name,
            );
            apply_mcp_config_remove_success_state(
                app,
                &server_name,
                McpConfigScope::Dynamic.cli_arg(),
                None,
            );
        }
        PendingDynamicMcpRemovalConfirmation::Failed { server_name, message } => {
            if app
                .mcp
                .pending_dynamic_config_removal
                .as_deref()
                .is_some_and(|pending| mcp_server_name_eq(pending, &server_name))
            {
                app.mcp.pending_dynamic_config_removal = None;
            }
            tracing::warn!(
                target: crate::logging::targets::APP_CONFIG,
                event_name = "mcp_dynamic_remove_confirmation_failed",
                message = "dynamic MCP removal was not confirmed",
                outcome = "failure",
                server_name = %server_name,
                error_message = %message,
            );
            apply_mcp_config_remove_failure(
                app,
                &server_name,
                McpConfigScope::Dynamic.cli_arg(),
                &message,
            );
        }
    }
}

pub(super) fn apply_mcp_config_remove_success_state(
    app: &mut App,
    server_name: &str,
    scope_name: &str,
    expected_source: Option<types::McpSnapshotSource>,
) {
    if let Some(expected_source) = expected_source {
        remember_removed_config_mcp_server(app, scope_name, server_name, expected_source);
    }
    remove_matching_mcp_server_from_snapshot(app, scope_name, server_name);
    app.config.mcp_selected_server_index =
        app.config.mcp_selected_server_index.min(app.mcp.servers.len().saturating_sub(1));
    app.mcp.last_error = None;
    app.config.last_error = None;
    app.config.status_message = Some(format!(
        "Removed MCP server {server_name} from {scope_name} config. You might need to run /new-session to apply MCP changes."
    ));
}

fn remember_removed_config_mcp_server(
    app: &mut App,
    scope: &str,
    server_name: &str,
    expected_source: types::McpSnapshotSource,
) {
    app.mcp.removed_config_servers.insert(
        normalized_removed_config_key(scope, server_name),
        RemovedMcpServerGuard { expected_source },
    );
}

fn remove_matching_mcp_server_from_snapshot(app: &mut App, scope: &str, server_name: &str) {
    let removed_key = normalized_removed_config_key(scope, server_name);
    app.mcp.servers.retain(|server| !mcp_server_matches_removed_key(server, &removed_key));
}

pub(super) fn is_removed_config_mcp_server_suppressed(
    app: &App,
    server: &crate::agent::model::McpServerStatus,
) -> bool {
    app.mcp
        .removed_config_servers
        .keys()
        .any(|removed_key| mcp_server_matches_removed_key(server, removed_key))
}

pub(super) fn mcp_server_matches_removed_key(
    server: &crate::agent::model::McpServerStatus,
    removed_key: &RemovedMcpServerKey,
) -> bool {
    match server.scope.as_deref() {
        Some(scope) => normalized_removed_config_key(scope, &server.name) == *removed_key,
        None => mcp_server_name_eq(&server.name, &removed_key.server_name),
    }
}

pub(super) fn mcp_server_name_eq(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

pub(super) const fn mcp_snapshot_source_label(source: types::McpSnapshotSource) -> &'static str {
    match source {
        types::McpSnapshotSource::ReloadPlugins => "reload_plugins",
        types::McpSnapshotSource::McpStatus => "mcp_status",
        types::McpSnapshotSource::McpSetServers => "mcp_set_servers",
        types::McpSnapshotSource::Init => "init",
    }
}

pub(super) fn normalized_removed_config_key(scope: &str, server_name: &str) -> RemovedMcpServerKey {
    RemovedMcpServerKey::new(
        normalize_mcp_config_scope_key(scope),
        server_name.to_ascii_lowercase(),
    )
}

pub(super) fn normalize_mcp_config_scope_key(scope: &str) -> String {
    scope.trim().to_ascii_lowercase()
}

pub(crate) fn apply_mcp_config_remove_failure(
    app: &mut App,
    server_name: &str,
    scope: &str,
    message: &str,
) {
    app.mcp.in_flight = false;
    let formatted =
        format!("Failed to remove MCP server {server_name} from {scope} config: {message}");
    app.mcp.last_error = Some(formatted.clone());
    app.config.last_error = Some(formatted.clone());
    app.config.status_message = None;
    if app.config.overlay.is_some() {
        app.config.set_overlay_error(formatted);
    } else {
        app.config.last_error = Some(formatted);
    }
}
