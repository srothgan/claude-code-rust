// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use super::prelude::*;

pub(crate) fn send_mcp_elicitation_response(
    app: &mut App,
    request_id: &str,
    action: crate::agent::types::ElicitationAction,
    content: Option<serde_json::Value>,
) {
    let Some(conn) = app.session_runtime.conn.as_ref() else {
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
    let Some(ref sid) = app.session_runtime.session_id else {
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
