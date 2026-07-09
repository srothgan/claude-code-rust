// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use super::prelude::*;

pub(crate) fn submit_mcp_oauth_callback_url(
    app: &mut App,
    server_name: &str,
    callback_url: String,
) {
    let Some(conn) = app.session_runtime.conn.as_ref() else {
        return;
    };
    let Some(ref sid) = app.session_runtime.session_id else {
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

pub(super) fn open_url_in_browser(url: &str) -> Result<(), String> {
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
