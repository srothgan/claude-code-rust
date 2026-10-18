// SPDX-License-Identifier: Apache-2.0
use super::prelude::*;

pub(crate) fn send_mcp_elicitation_response(
    app: &mut App,
    request_id: &str,
    action: types::ElicitationAction,
) {
    let Some(entry) =
        app.mcp.elicitations.active_mut().filter(|entry| entry.request.request_id == request_id)
    else {
        return;
    };
    let Some(response) = entry.response(action) else {
        return;
    };
    let (Some(conn), Some(sid)) =
        (app.session_runtime.conn.clone(), app.session_runtime.session_id.as_ref())
    else {
        entry.error = Some("No active connection. Your answers are retained.".to_owned());
        return;
    };
    entry.sending = true;
    entry.error = None;
    let session_id = sid.to_string();
    let request_id = request_id.to_owned();
    let event_tx = app.event_tx.clone();
    tokio::task::spawn_local(async move {
        let result = conn
            .respond_to_elicitation(
                session_id.clone(),
                request_id.clone(),
                response.action,
                response.content,
            )
            .await;
        // Neither answers nor possibly answer-bearing errors belong in diagnostics.
        tracing::info!(target: crate::logging::targets::APP_PERMISSION,
            event_name = "elicitation_response_transport", session_id = %session_id,
            request_id = %request_id, success = result.is_ok());
        let event = if result.is_ok() {
            ClientEvent::McpElicitationResponseQueued { session_id, request_id }
        } else {
            ClientEvent::McpElicitationResponseFailed { session_id, request_id }
        };
        let _ = event_tx.send(event).await;
    });
}

fn activate(app: &mut App) {
    let Some(entry) = app.mcp.elicitations.active_mut() else {
        if matches!(app.config.overlay, Some(ConfigOverlayState::McpElicitation(_))) {
            app.config.clear_overlay();
        }
        return;
    };
    if entry.request.mode == types::ElicitationMode::Url {
        let result = entry
            .request
            .url
            .as_deref()
            .ok_or_else(|| "SDK did not provide a URL.".to_owned())
            .and_then(open_url_in_browser);
        entry.browser_opened = result.is_ok();
        entry.browser_open_error = result.err();
    }
    view::set_fullscreen_view(app, FullscreenView::Config);
    app.config.active_tab = ConfigTab::Mcp;
    app.config.replace_overlay(ConfigOverlayState::McpElicitation(McpElicitationOverlayState));
}

pub(crate) fn present_mcp_elicitation_request(app: &mut App, request: types::ElicitationRequest) {
    let was_empty = app.mcp.elicitations.is_empty();
    if app.mcp.elicitations.insert(request) && was_empty {
        activate(app);
    }
}

pub(crate) fn handle_mcp_elicitation_settled(
    app: &mut App,
    request_id: &str,
    session_closing: bool,
) {
    let active =
        app.mcp.elicitations.active().is_some_and(|entry| entry.request.request_id == request_id);
    if !app.mcp.elicitations.remove(request_id) {
        return;
    }
    if session_closing {
        // The bridge is cancelling every callback. Never promote a queued URL
        // between individual settlements during teardown.
        app.mcp.elicitations = super::form::ElicitationQueue::default();
        if matches!(app.config.overlay, Some(ConfigOverlayState::McpElicitation(_))) {
            app.config.clear_overlay();
        }
        return;
    }
    if active {
        activate(app);
        refresh_mcp_snapshot(app);
    }
}

#[cfg(test)]
mod tests;
