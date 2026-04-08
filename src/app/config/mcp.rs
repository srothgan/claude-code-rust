use super::{ConfigOverlayState, ConfigState, ConfigTab};
use crate::app::App;
use crate::app::view::{self, ActiveView};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum McpServerActionKind {
    RefreshSnapshot,
    Authenticate,
    ClearAuth,
    Reconnect,
    Enable,
    Disable,
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
        }
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
                    available_mcp_actions(server).iter().position(|candidate| *candidate == action)
                })
                .unwrap_or(0)
        });
    app.config.overlay = Some(ConfigOverlayState::McpDetails(McpDetailsOverlayState {
        server_name,
        selected_index,
    }));
    app.config.last_error = None;
}

#[must_use]
pub(crate) fn available_mcp_actions(
    server: &crate::agent::types::McpServerStatus,
) -> Vec<McpServerActionKind> {
    let mut actions = vec![McpServerActionKind::RefreshSnapshot];
    if matches!(server.status, crate::agent::types::McpServerConnectionStatus::Disabled) {
        actions.push(McpServerActionKind::Enable);
    } else {
        if matches!(
            server.status,
            crate::agent::types::McpServerConnectionStatus::NeedsAuth
                | crate::agent::types::McpServerConnectionStatus::Failed
                | crate::agent::types::McpServerConnectionStatus::Pending
        ) {
            actions.push(McpServerActionKind::Authenticate);
        }
        actions.push(McpServerActionKind::ClearAuth);
        actions.push(McpServerActionKind::Reconnect);
        actions.push(McpServerActionKind::Disable);
    }
    actions
}

#[must_use]
pub(crate) fn is_mcp_action_available(
    server: &crate::agent::types::McpServerStatus,
    action: McpServerActionKind,
) -> bool {
    !matches!(
        (action, server.config.as_ref()),
        (
            McpServerActionKind::Authenticate,
            Some(crate::agent::types::McpServerStatusConfig::ClaudeaiProxy { .. })
        )
    )
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
    view::set_active_view(app, ActiveView::Config);
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
    app.config.overlay = Some(ConfigOverlayState::McpElicitation(McpElicitationOverlayState {
        request,
        selected_index: 0,
        browser_opened,
        browser_open_error,
    }));
    app.config.last_error = None;
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
    view::set_active_view(app, ActiveView::Config);
    app.config.active_tab = ConfigTab::Mcp;
    refresh_mcp_snapshot(app);
    let (browser_opened, browser_open_error) = match open_url_in_browser(&redirect.auth_url) {
        Ok(()) => (true, None),
        Err(error) => (false, Some(error)),
    };
    app.config.overlay = Some(ConfigOverlayState::McpAuthRedirect(McpAuthRedirectOverlayState {
        redirect,
        selected_index: 0,
        browser_opened,
        browser_open_error,
    }));
    app.config.last_error = None;
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
            app.config.overlay = None;
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
    app.mcp.in_flight = false;
    let formatted = format_mcp_operation_error(error);
    app.mcp.last_error = Some(formatted.clone());
    app.config.last_error = Some(formatted);
    app.config.status_message = None;
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
