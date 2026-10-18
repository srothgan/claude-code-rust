// SPDX-License-Identifier: Apache-2.0

use super::prelude::*;

pub(super) fn open_folder_input(app: &mut App) {
    if app.plugins.loading {
        return;
    }
    if app.session_runtime.session_id.is_none() {
        app.config.last_error =
            Some("Connect to a session before loading a plugin folder.".to_owned());
        return;
    }
    let draft = if app.plugins.active_tab == PluginsViewTab::Session {
        app.plugins
            .session_plugins
            .get(app.plugins.session_selected_index)
            .map(|plugin| plugin.path.clone())
    } else {
        None
    }
    .unwrap_or_default();
    app.config.replace_overlay(ConfigOverlayState::LocalPluginFolder(
        PluginSourceInputOverlayState { cursor: draft.chars().count(), draft },
    ));
}

pub(super) fn load_folder(app: &mut App) {
    let Some(input) = app.config.plugin_source_input_overlay() else { return };
    let folder = input.draft.trim().to_owned();
    if folder.is_empty() || folder.chars().any(char::is_control) {
        app.config.set_overlay_error("Enter a local plugin directory.");
        return;
    }
    match crate::app::session_runtime::request_plugin_folders(app, vec![folder], false) {
        crate::app::session_runtime::RuntimeReloadRequestOutcome::Requested => {
            app.plugins.loading = true;
            app.plugins.clear_feedback();
            app.config.set_overlay_info("Loading local folder...");
        }
        _ => app.config.set_overlay_error(
            "Could not request the session plugin reload. Your folder is kept for retry.",
        ),
    }
}

pub(crate) fn apply_plugin_inventory(
    app: &mut App,
    plugins: Vec<crate::agent::types::SessionPlugin>,
) {
    app.plugins.session_plugins = plugins;
    super::selection::clamp_selection(app);
}

pub(crate) fn reload_matches(app: &App, request_id: Option<&str>) -> bool {
    match &app.plugins.pending_reload {
        Some(operation) => {
            Some(operation.request_id.as_str()) == request_id
                && app
                    .session_runtime
                    .session_id
                    .as_ref()
                    .is_some_and(|id| id.to_string() == operation.session_id)
        }
        None => false,
    }
}

pub(crate) fn hold_reload(
    app: &mut App,
    request_id: Option<&str>,
    cache_impact: &crate::agent::types::RuntimeReloadCacheImpact,
    error_count: u32,
) {
    if !reload_matches(app, request_id) {
        return;
    }
    if let Some(operation) = app.plugins.pending_reload.as_mut() {
        operation.held = true;
    }
    super::actions::apply_runtime_reload_held(app, cache_impact);
    if error_count > 0 {
        app.config.set_overlay_error(format!(
            "Held reload reported {error_count} component error(s). No attachment was applied."
        ));
    }
    app.mcp.in_flight = false;
}

pub(crate) fn fail_reload(app: &mut App, request_id: Option<&str>, message: &str) {
    if !reload_matches(app, request_id) {
        return;
    }
    super::actions::apply_runtime_reload_failure(app, message);
    if matches!(app.config.overlay, Some(ConfigOverlayState::LocalPluginFolder(_))) {
        app.config.set_overlay_error(message);
    }
    if app.mcp.in_flight {
        app.mcp.in_flight = false;
        app.mcp.last_error = Some(format!("Failed to reload MCP server snapshot: {message}"));
    }
}

pub(crate) fn complete_reload(
    app: &mut App,
    request_id: Option<&str>,
    error_count: u32,
    reconciliation_error: Option<String>,
) {
    if !reload_matches(app, request_id) {
        return;
    }
    let local =
        app.plugins.pending_reload.take().is_some_and(|operation| !operation.folders.is_empty());
    super::actions::apply_runtime_reload_success(app);
    if local {
        app.plugins.active_tab = PluginsViewTab::Session;
        app.plugins.search_focused = false;
        if matches!(app.config.overlay, Some(ConfigOverlayState::LocalPluginFolder(_))) {
            app.config.clear_overlay();
        }
        app.plugins.status_message =
            Some("Plugin reload applied. The Session list shows confirmed folders.".to_owned());
        app.config.status_message.clone_from(&app.plugins.status_message);
    }
    if error_count > 0 || reconciliation_error.is_some() {
        let message = if let Some(error) = reconciliation_error {
            format!(
                "Plugin reload applied with {error_count} component error(s), but host reconciliation failed: {error}"
            )
        } else {
            format!(
                "Plugin reload applied with {error_count} component error(s). Review the confirmed Session inventory and SDK diagnostics."
            )
        };
        app.plugins.status_message = Some(message.clone());
        app.config.status_message = Some(message);
    }
}
