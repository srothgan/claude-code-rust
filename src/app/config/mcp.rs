use super::{ConfigOverlayState, ConfigState, ConfigTab};
use crate::agent::{events::ClientEvent, model, types};
use crate::app::App;
use crate::app::plugins::InstalledPluginEntry;
use crate::app::state::types::{RemovedMcpServerGuard, RemovedMcpServerKey};
use crate::app::view::{self, FullscreenView};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum McpServerActionKind {
    RefreshSnapshot,
    Authenticate,
    ClearAuth,
    Reconnect,
    Enable,
    Disable,
    ManagePlugin,
    RemoveUserConfig,
    RemoveLocalConfig,
    RemoveProjectConfig,
    RemoveDynamicConfig,
}

impl McpServerActionKind {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::RefreshSnapshot => "Refresh",
            Self::Authenticate => "Authenticate",
            Self::ClearAuth => "Clear auth",
            Self::Reconnect => "Reconnect server",
            Self::Enable => "Enable server",
            Self::Disable => "Disable server",
            Self::ManagePlugin => "Manage plugin",
            Self::RemoveUserConfig
            | Self::RemoveLocalConfig
            | Self::RemoveProjectConfig
            | Self::RemoveDynamicConfig => "Remove",
        }
    }

    #[must_use]
    pub const fn mcp_config_scope(self) -> Option<McpConfigScope> {
        match self {
            Self::RemoveUserConfig => Some(McpConfigScope::User),
            Self::RemoveLocalConfig => Some(McpConfigScope::Local),
            Self::RemoveProjectConfig => Some(McpConfigScope::Project),
            Self::RemoveDynamicConfig => Some(McpConfigScope::Dynamic),
            Self::RefreshSnapshot
            | Self::Authenticate
            | Self::ClearAuth
            | Self::Reconnect
            | Self::Enable
            | Self::Disable
            | Self::ManagePlugin => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum McpConfigScope {
    Local,
    User,
    Project,
    Dynamic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum McpServerOwnership<'a> {
    Persisted(McpConfigScope),
    SdkDynamic,
    PluginOwned(&'a InstalledPluginEntry),
    PluginOwnedUnknown,
    RuntimeOnly,
}

impl McpConfigScope {
    #[must_use]
    pub const fn cli_arg(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::User => "user",
            Self::Project => "project",
            Self::Dynamic => "dynamic",
        }
    }

    #[must_use]
    pub fn from_status_scope(scope: &str) -> Option<Self> {
        match scope.trim() {
            scope if scope.eq_ignore_ascii_case("user") => Some(Self::User),
            scope if scope.eq_ignore_ascii_case("local") => Some(Self::Local),
            scope if scope.eq_ignore_ascii_case("project") => Some(Self::Project),
            scope if scope.eq_ignore_ascii_case("dynamic") => Some(Self::Dynamic),
            _ => None,
        }
    }
}

#[must_use]
pub(crate) const fn remove_action_for_mcp_config_scope(
    scope: McpConfigScope,
) -> McpServerActionKind {
    match scope {
        McpConfigScope::User => McpServerActionKind::RemoveUserConfig,
        McpConfigScope::Local => McpServerActionKind::RemoveLocalConfig,
        McpConfigScope::Project => McpServerActionKind::RemoveProjectConfig,
        McpConfigScope::Dynamic => McpServerActionKind::RemoveDynamicConfig,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpDetailsOverlayState {
    pub server_name: String,
    pub selected_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpCallbackUrlOverlayState {
    pub server_name: String,
    pub draft: String,
    pub cursor: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct McpElicitationOverlayState {
    pub request: crate::agent::types::ElicitationRequest,
    pub selected_index: usize,
    pub browser_opened: bool,
    pub browser_open_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpAuthRedirectOverlayState {
    pub redirect: crate::agent::types::McpAuthRedirect,
    pub selected_index: usize,
    pub browser_opened: bool,
    pub browser_open_error: Option<String>,
}

impl ConfigState {
    #[must_use]
    pub fn mcp_details_overlay(&self) -> Option<&McpDetailsOverlayState> {
        if let Some(ConfigOverlayState::McpDetails(overlay)) = &self.overlay {
            Some(overlay)
        } else {
            None
        }
    }

    pub fn mcp_details_overlay_mut(&mut self) -> Option<&mut McpDetailsOverlayState> {
        if let Some(ConfigOverlayState::McpDetails(overlay)) = &mut self.overlay {
            Some(overlay)
        } else {
            None
        }
    }

    #[must_use]
    pub fn mcp_callback_url_overlay(&self) -> Option<&McpCallbackUrlOverlayState> {
        if let Some(ConfigOverlayState::McpCallbackUrl(overlay)) = &self.overlay {
            Some(overlay)
        } else {
            None
        }
    }

    pub fn mcp_callback_url_overlay_mut(&mut self) -> Option<&mut McpCallbackUrlOverlayState> {
        if let Some(ConfigOverlayState::McpCallbackUrl(overlay)) = &mut self.overlay {
            Some(overlay)
        } else {
            None
        }
    }

    #[must_use]
    pub fn mcp_elicitation_overlay(&self) -> Option<&McpElicitationOverlayState> {
        if let Some(ConfigOverlayState::McpElicitation(overlay)) = &self.overlay {
            Some(overlay)
        } else {
            None
        }
    }

    pub fn mcp_elicitation_overlay_mut(&mut self) -> Option<&mut McpElicitationOverlayState> {
        if let Some(ConfigOverlayState::McpElicitation(overlay)) = &mut self.overlay {
            Some(overlay)
        } else {
            None
        }
    }

    #[must_use]
    pub fn mcp_auth_redirect_overlay(&self) -> Option<&McpAuthRedirectOverlayState> {
        if let Some(ConfigOverlayState::McpAuthRedirect(overlay)) = &self.overlay {
            Some(overlay)
        } else {
            None
        }
    }

    pub fn mcp_auth_redirect_overlay_mut(&mut self) -> Option<&mut McpAuthRedirectOverlayState> {
        if let Some(ConfigOverlayState::McpAuthRedirect(overlay)) = &mut self.overlay {
            Some(overlay)
        } else {
            None
        }
    }
}

pub(super) fn handle_mcp_key(app: &mut App, key: KeyEvent) -> bool {
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
    let Some(conn) = app.conn.as_ref() else {
        app.mcp.in_flight = false;
        return;
    };
    let Some(ref sid) = app.session_id else {
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

pub(crate) fn reconnect_mcp_server(app: &mut App, server_name: &str) {
    let Some(conn) = app.conn.as_ref() else {
        return;
    };
    let Some(ref sid) = app.session_id else {
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
    let Some(conn) = app.conn.as_ref() else {
        return;
    };
    let Some(ref sid) = app.session_id else {
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
    let Some(conn) = app.conn.as_ref() else {
        return;
    };
    let Some(ref sid) = app.session_id else {
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
    let Some(conn) = app.conn.as_ref() else {
        return;
    };
    let Some(ref sid) = app.session_id else {
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
    let Some(conn) = app.conn.clone() else {
        apply_mcp_config_remove_failure(
            app,
            server_name,
            McpConfigScope::Dynamic.cli_arg(),
            "No active bridge connection.",
        );
        return;
    };
    let Some(session_id) = app.session_id.as_ref().map(ToString::to_string) else {
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
        model::McpServerStatusConfig::Stdio { command, args, env, timeout, always_load } => {
            Ok(types::McpServerConfig::Stdio {
                command: command.clone(),
                args: args.clone(),
                env: env.clone(),
                timeout: *timeout,
                always_load: *always_load,
            })
        }
        model::McpServerStatusConfig::Sse { url, headers, tools, timeout, always_load } => {
            Ok(types::McpServerConfig::Sse {
                url: url.clone(),
                headers: headers.clone(),
                tools: tools.iter().map(dynamic_mcp_tool_policy_for_set_servers).collect(),
                timeout: *timeout,
                always_load: *always_load,
            })
        }
        model::McpServerStatusConfig::Http { url, headers, tools, timeout, always_load } => {
            Ok(types::McpServerConfig::Http {
                url: url.clone(),
                headers: headers.clone(),
                tools: tools.iter().map(dynamic_mcp_tool_policy_for_set_servers).collect(),
                timeout: *timeout,
                always_load: *always_load,
            })
        }
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PendingDynamicMcpRemovalConfirmation {
    Confirmed { server_name: String },
    Failed { server_name: String, message: String },
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

fn apply_mcp_config_remove_success_state(
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
            ConfigOverlayState::ModelAndEffort(_)
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct McpConfigRemoveConfirmationFailure {
    server_name: String,
    scope: String,
    message: String,
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

fn is_removed_config_mcp_server_suppressed(
    app: &App,
    server: &crate::agent::model::McpServerStatus,
) -> bool {
    app.mcp
        .removed_config_servers
        .keys()
        .any(|removed_key| mcp_server_matches_removed_key(server, removed_key))
}

fn mcp_server_matches_removed_key(
    server: &crate::agent::model::McpServerStatus,
    removed_key: &RemovedMcpServerKey,
) -> bool {
    match server.scope.as_deref() {
        Some(scope) => normalized_removed_config_key(scope, &server.name) == *removed_key,
        None => mcp_server_name_eq(&server.name, &removed_key.server_name),
    }
}

fn mcp_server_name_eq(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

const fn mcp_snapshot_source_label(source: types::McpSnapshotSource) -> &'static str {
    match source {
        types::McpSnapshotSource::ReloadPlugins => "reload_plugins",
        types::McpSnapshotSource::McpStatus => "mcp_status",
        types::McpSnapshotSource::McpSetServers => "mcp_set_servers",
        types::McpSnapshotSource::Init => "init",
    }
}

fn normalized_removed_config_key(scope: &str, server_name: &str) -> RemovedMcpServerKey {
    RemovedMcpServerKey::new(
        normalize_mcp_config_scope_key(scope),
        server_name.to_ascii_lowercase(),
    )
}

fn normalize_mcp_config_scope_key(scope: &str) -> String {
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

pub(crate) fn submit_mcp_oauth_callback_url(
    app: &mut App,
    server_name: &str,
    callback_url: String,
) {
    let Some(conn) = app.conn.as_ref() else {
        return;
    };
    let Some(ref sid) = app.session_id else {
        return;
    };
    let session_id = sid.to_string();
    let callback_url_chars = callback_url.chars().count();
    match conn.submit_mcp_oauth_callback_url(
        session_id.clone(),
        server_name.to_owned(),
        callback_url,
    ) {
        Ok(()) => {
            tracing::info!(
                target: crate::logging::targets::APP_CONFIG,
                event_name = "mcp_oauth_callback_requested",
                message = "MCP OAuth callback URL submitted",
                outcome = "start",
                session_id = %session_id,
                server_name = %server_name,
                callback_url_chars,
            );
            refresh_mcp_snapshot(app);
        }
        Err(error) => tracing::warn!(
            target: crate::logging::targets::APP_CONFIG,
            event_name = "mcp_oauth_callback_request_failed",
            message = "failed to submit MCP OAuth callback URL",
            outcome = "failure",
            session_id = %session_id,
            server_name = %server_name,
            callback_url_chars,
            error_message = %error,
        ),
    }
}

pub(crate) fn send_mcp_elicitation_response(
    app: &mut App,
    request_id: &str,
    action: crate::agent::types::ElicitationAction,
    content: Option<serde_json::Value>,
) {
    let Some(conn) = app.conn.as_ref() else {
        tracing::warn!(
            target: crate::logging::targets::APP_PERMISSION,
            event_name = "elicitation_response_blocked",
            message = "elicitation response blocked without an active bridge connection",
            outcome = "blocked",
            request_id = %request_id,
            action = ?action,
            reason = "missing_connection",
        );
        return;
    };
    let Some(ref sid) = app.session_id else {
        tracing::warn!(
            target: crate::logging::targets::APP_PERMISSION,
            event_name = "elicitation_response_blocked",
            message = "elicitation response blocked without an active session",
            outcome = "blocked",
            request_id = %request_id,
            action = ?action,
            reason = "missing_session",
        );
        return;
    };
    let session_id_for_log = sid.to_string();
    let has_content = content.is_some();
    if conn.respond_to_elicitation(sid.to_string(), request_id.to_owned(), action, content).is_ok()
    {
        app.mcp.pending_elicitation = None;
        refresh_mcp_snapshot(app);
        tracing::info!(
            target: crate::logging::targets::APP_PERMISSION,
            event_name = "elicitation_response_sent",
            message = "elicitation response sent to bridge",
            outcome = "success",
            session_id = %session_id_for_log,
            request_id = %request_id,
            action = ?action,
            has_content,
        );
    } else {
        tracing::error!(
            target: crate::logging::targets::APP_PERMISSION,
            event_name = "elicitation_response_failed",
            message = "failed to send elicitation response to bridge",
            outcome = "failure",
            session_id = %session_id_for_log,
            request_id = %request_id,
            action = ?action,
            has_content,
        );
    }
}

fn open_selected_mcp_server_details(app: &mut App) {
    let Some(server_name) =
        app.mcp.servers.get(app.config.mcp_selected_server_index).map(|server| server.name.clone())
    else {
        return;
    };
    open_mcp_server_details(app, server_name, None);
}

pub(crate) fn open_mcp_server_details(
    app: &mut App,
    server_name: String,
    preferred_action: Option<McpServerActionKind>,
) {
    let selected_index =
        app.mcp.servers.iter().find(|server| server.name == server_name).map_or(0, |server| {
            preferred_action
                .and_then(|action| {
                    available_mcp_actions(app, server)
                        .iter()
                        .position(|candidate| *candidate == action)
                })
                .unwrap_or(0)
        });
    app.config.replace_overlay(ConfigOverlayState::McpDetails(McpDetailsOverlayState {
        server_name,
        selected_index,
    }));
}

#[must_use]
pub(crate) fn mcp_server_ownership<'a>(
    app: &'a App,
    server: &'a crate::agent::model::McpServerStatus,
) -> McpServerOwnership<'a> {
    if crate::app::plugins::is_plugin_mcp_runtime_server_name(&server.name) {
        return crate::app::plugins::installed_mcp_plugin_for_runtime_server(app, &server.name)
            .map_or(McpServerOwnership::PluginOwnedUnknown, McpServerOwnership::PluginOwned);
    }

    match server.scope.as_deref().and_then(McpConfigScope::from_status_scope) {
        Some(McpConfigScope::User) => McpServerOwnership::Persisted(McpConfigScope::User),
        Some(McpConfigScope::Local) => McpServerOwnership::Persisted(McpConfigScope::Local),
        Some(McpConfigScope::Project) => McpServerOwnership::Persisted(McpConfigScope::Project),
        Some(McpConfigScope::Dynamic) => McpServerOwnership::SdkDynamic,
        None => McpServerOwnership::RuntimeOnly,
    }
}

#[must_use]
pub(crate) fn mcp_config_removal_scope(
    app: &App,
    server: &crate::agent::model::McpServerStatus,
) -> Option<McpConfigScope> {
    match mcp_server_ownership(app, server) {
        McpServerOwnership::Persisted(scope) => Some(scope),
        McpServerOwnership::SdkDynamic => Some(McpConfigScope::Dynamic),
        McpServerOwnership::PluginOwned(_)
        | McpServerOwnership::PluginOwnedUnknown
        | McpServerOwnership::RuntimeOnly => None,
    }
}

#[must_use]
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

#[must_use]
pub(crate) fn mcp_server_owner_summary(
    app: &App,
    server: &crate::agent::model::McpServerStatus,
) -> Option<String> {
    match mcp_server_ownership(app, server) {
        McpServerOwnership::PluginOwned(entry) => {
            Some(format!("Managed by plugin: {}", crate::app::plugins::display_label(&entry.id)))
        }
        McpServerOwnership::PluginOwnedUnknown => Some(
            "Managed by a plugin. Refresh plugin inventory from the Plugins tab to manage it here."
                .to_owned(),
        ),
        McpServerOwnership::Persisted(_)
        | McpServerOwnership::SdkDynamic
        | McpServerOwnership::RuntimeOnly => None,
    }
}

fn is_mcp_config_removal_available(app: &App, server_name: &str, scope: McpConfigScope) -> bool {
    app.mcp
        .servers
        .iter()
        .find(|server| mcp_server_name_eq(&server.name, server_name))
        .is_some_and(|server| mcp_config_removal_scope(app, server) == Some(scope))
}

pub(crate) fn present_mcp_elicitation_request(
    app: &mut App,
    request: crate::agent::types::ElicitationRequest,
) {
    let request_id_for_log = request.request_id.clone();
    let server_name_for_log = request.server_name.clone();
    let mode_for_log = format!("{:?}", request.mode);
    let has_url = request.url.is_some();
    let has_requested_schema = request.requested_schema.is_some();
    app.mcp.pending_elicitation = Some(request.clone());
    view::set_fullscreen_view(app, FullscreenView::Config);
    app.config.active_tab = ConfigTab::Mcp;
    refresh_mcp_snapshot(app);
    let (browser_opened, browser_open_error) =
        if matches!(request.mode, crate::agent::types::ElicitationMode::Url) {
            request.url.as_deref().map_or(
                (false, Some("SDK did not provide an auth URL".to_owned())),
                |url| match open_url_in_browser(url) {
                    Ok(()) => (true, None),
                    Err(error) => (false, Some(error)),
                },
            )
        } else {
            (false, None)
        };
    app.config.replace_overlay(ConfigOverlayState::McpElicitation(McpElicitationOverlayState {
        request,
        selected_index: 0,
        browser_opened,
        browser_open_error,
    }));
    tracing::info!(
        target: crate::logging::targets::APP_PERMISSION,
        event_name = "elicitation_request_presented",
        message = "elicitation request presented in MCP config view",
        outcome = "success",
        request_id = %request_id_for_log,
        server_name = %server_name_for_log,
        mode = %mode_for_log,
        browser_opened,
        has_url,
        has_requested_schema,
    );
}

pub(crate) fn present_mcp_auth_redirect(
    app: &mut App,
    redirect: crate::agent::types::McpAuthRedirect,
) {
    let server_name_for_log = redirect.server_name.clone();
    view::set_fullscreen_view(app, FullscreenView::Config);
    app.config.active_tab = ConfigTab::Mcp;
    refresh_mcp_snapshot(app);
    let (browser_opened, browser_open_error) = match open_url_in_browser(&redirect.auth_url) {
        Ok(()) => (true, None),
        Err(error) => (false, Some(error)),
    };
    app.config.replace_overlay(ConfigOverlayState::McpAuthRedirect(McpAuthRedirectOverlayState {
        redirect,
        selected_index: 0,
        browser_opened,
        browser_open_error,
    }));
    tracing::info!(
        target: crate::logging::targets::APP_CONFIG,
        event_name = "mcp_auth_redirect_presented",
        message = "MCP auth redirect presented",
        outcome = "success",
        server_name = %server_name_for_log,
        browser_opened,
    );
}

pub(crate) fn handle_mcp_elicitation_completed(
    app: &mut App,
    elicitation_id: &str,
    _server_name: Option<String>,
) {
    let should_clear = app
        .mcp
        .pending_elicitation
        .as_ref()
        .and_then(|request| request.elicitation_id.as_deref())
        .is_some_and(|current| current == elicitation_id);
    if should_clear {
        app.mcp.pending_elicitation = None;
        if matches!(app.config.overlay, Some(ConfigOverlayState::McpElicitation(_))) {
            app.config.clear_overlay();
        }
        refresh_mcp_snapshot(app);
        tracing::info!(
            target: crate::logging::targets::APP_PERMISSION,
            event_name = "elicitation_completed_applied",
            message = "elicitation completion applied",
            outcome = "success",
            request_id = %elicitation_id,
        );
    }
}

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

fn open_url_in_browser(url: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut cmd = std::process::Command::new("rundll32.exe");
        cmd.args(["url.dll,FileProtocolHandler", url]);
        cmd
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut cmd = std::process::Command::new("open");
        cmd.arg(url);
        cmd
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut cmd = std::process::Command::new("xdg-open");
        cmd.arg(url);
        cmd
    };

    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Failed to open browser automatically: {error}"))
}

pub(crate) fn copy_text_to_clipboard(text: &str) -> Result<(), String> {
    let mut clipboard = arboard::Clipboard::new()
        .map_err(|error| format!("Failed to access clipboard: {error}"))?;
    clipboard
        .set_text(text.to_owned())
        .map_err(|error| format!("Failed to copy to clipboard: {error}"))
}
