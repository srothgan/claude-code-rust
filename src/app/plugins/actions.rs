// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use super::overlays::{
    open_installed_plugin_uninstall_confirmation, open_marketplace_remove_confirmation,
};
use super::prelude::*;
use super::selection::clamp_selection;
use super::text::display_label;

pub(super) fn execute_selected_installed_overlay_action(app: &mut App) {
    let Some(overlay) = app.config.installed_plugin_actions_overlay().cloned() else {
        return;
    };
    let Some(action) = overlay.actions.get(overlay.selected_index).copied() else {
        return;
    };

    if action == InstalledPluginActionKind::Uninstall {
        open_installed_plugin_uninstall_confirmation(app, overlay);
        return;
    }

    execute_installed_plugin_action(app, overlay, action);
}

pub(crate) fn execute_confirmed_installed_plugin_action(
    app: &mut App,
    action: InstalledPluginActionKind,
) {
    let Some(overlay) = app.config.installed_plugin_actions_overlay().cloned() else {
        return;
    };
    execute_installed_plugin_action(app, overlay, action);
}

pub(super) fn execute_installed_plugin_action(
    app: &mut App,
    overlay: InstalledPluginActionOverlayState,
    action: InstalledPluginActionKind,
) {
    let (cwd_raw, args, status_message) = installed_action_command(app, &overlay, action);

    if tokio::runtime::Handle::try_current().is_err() {
        app.config.clear_overlay();
        app.config.status_message = None;
        app.config.last_error = Some("No runtime available for plugin action".to_owned());
        return;
    }

    app.config.clear_overlay();
    app.config.last_error = None;
    app.config.status_message = Some(status_message);
    app.plugins.loading = true;
    app.plugins.last_inventory_refresh_at = None;
    app.request_active_surface_repaint();
    let event_tx = app.event_tx.clone();
    let cwd_context = app.cwd_raw.clone();
    let cached_claude_path = app.plugins.claude_path.clone();
    tokio::task::spawn_local(async move {
        match cli::run_cli_command_and_refresh(cwd_raw, cached_claude_path, args).await {
            Ok((snapshot, claude_path)) => {
                let message =
                    installed_action_success_message(action, &overlay.title, &overlay.scope);
                let _ = event_tx.send(ClientEvent::PluginsCliActionSucceeded {
                    cwd_raw: cwd_context,
                    result: PluginsCliActionSuccess { snapshot, message, claude_path },
                });
            }
            Err(message) => {
                let _ = event_tx
                    .send(ClientEvent::PluginsCliActionFailed { cwd_raw: cwd_context, message });
            }
        }
    });
}

pub(super) fn execute_selected_plugin_install_action(app: &mut App) {
    let Some(overlay) = app.config.plugin_install_overlay().cloned() else {
        return;
    };
    let Some(action) = overlay.actions.get(overlay.selected_index).copied() else {
        return;
    };

    if tokio::runtime::Handle::try_current().is_err() {
        app.config.clear_overlay();
        app.config.status_message = None;
        app.config.last_error = Some("No runtime available for plugin action".to_owned());
        return;
    }

    let scope = action.scope();
    let args = vec![
        "plugin".to_owned(),
        "install".to_owned(),
        overlay.plugin_id.clone(),
        "--scope".to_owned(),
        scope.to_owned(),
    ];
    let status_message = match action {
        PluginInstallActionKind::User => format!("Installing {} for user scope...", overlay.title),
        PluginInstallActionKind::Project => {
            format!("Installing {} for project scope...", overlay.title)
        }
        PluginInstallActionKind::Local => {
            format!("Installing {} locally...", overlay.title)
        }
    };

    app.config.clear_overlay();
    app.config.last_error = None;
    app.config.status_message = Some(status_message);
    app.plugins.loading = true;
    app.plugins.last_inventory_refresh_at = None;
    app.request_active_surface_repaint();
    let event_tx = app.event_tx.clone();
    let cwd_raw = app.cwd_raw.clone();
    let cwd_context = app.cwd_raw.clone();
    let cached_claude_path = app.plugins.claude_path.clone();
    tokio::task::spawn_local(async move {
        match cli::run_cli_command_and_refresh(cwd_raw, cached_claude_path, args).await {
            Ok((snapshot, claude_path)) => {
                let message = plugin_install_success_message(action, &overlay.title);
                let _ = event_tx.send(ClientEvent::PluginsCliActionSucceeded {
                    cwd_raw: cwd_context,
                    result: PluginsCliActionSuccess { snapshot, message, claude_path },
                });
            }
            Err(message) => {
                let _ = event_tx
                    .send(ClientEvent::PluginsCliActionFailed { cwd_raw: cwd_context, message });
            }
        }
    });
}

pub(super) fn execute_selected_marketplace_action(app: &mut App) {
    let Some(overlay) = app.config.marketplace_actions_overlay().cloned() else {
        return;
    };
    let Some(action) = overlay.actions.get(overlay.selected_index).copied() else {
        return;
    };

    if action == MarketplaceActionKind::Remove {
        open_marketplace_remove_confirmation(app, overlay);
        return;
    }

    execute_marketplace_action(app, overlay, action);
}

pub(crate) fn execute_confirmed_marketplace_action(app: &mut App, action: MarketplaceActionKind) {
    let Some(overlay) = app.config.marketplace_actions_overlay().cloned() else {
        return;
    };
    execute_marketplace_action(app, overlay, action);
}

pub(super) fn execute_marketplace_action(
    app: &mut App,
    overlay: MarketplaceActionsOverlayState,
    action: MarketplaceActionKind,
) {
    if tokio::runtime::Handle::try_current().is_err() {
        app.config.clear_overlay();
        app.config.status_message = None;
        app.config.last_error = Some("No runtime available for marketplace action".to_owned());
        return;
    }

    let args = marketplace_action_command(&overlay, action);
    let status_message = marketplace_action_status_message(&overlay.title, action);

    app.config.clear_overlay();
    app.config.last_error = None;
    app.config.status_message = Some(status_message);
    app.plugins.loading = true;
    app.plugins.last_inventory_refresh_at = None;
    app.request_active_surface_repaint();
    let event_tx = app.event_tx.clone();
    let cwd_raw = app.cwd_raw.clone();
    let cwd_context = app.cwd_raw.clone();
    let cached_claude_path = app.plugins.claude_path.clone();
    tokio::task::spawn_local(async move {
        match cli::run_cli_command_and_refresh(cwd_raw, cached_claude_path, args).await {
            Ok((snapshot, claude_path)) => {
                let message = marketplace_action_success_message(&overlay.title, action);
                let _ = event_tx.send(ClientEvent::PluginsCliActionSucceeded {
                    cwd_raw: cwd_context,
                    result: PluginsCliActionSuccess { snapshot, message, claude_path },
                });
            }
            Err(message) => {
                let _ = event_tx
                    .send(ClientEvent::PluginsCliActionFailed { cwd_raw: cwd_context, message });
            }
        }
    });
}

pub(crate) fn apply_cli_action_success(app: &mut App, result: PluginsCliActionSuccess) {
    app.plugins.installed = result.snapshot.installed;
    app.plugins.marketplace = result.snapshot.marketplace;
    app.plugins.marketplaces = result.snapshot.marketplaces;
    app.plugins.last_error = None;
    app.plugins.last_inventory_refresh_at = Some(Instant::now());
    app.plugins.claude_path = Some(result.claude_path);
    clamp_selection(app);
    crate::app::config::reconcile_stale_plugin_mcp_servers(app);
    start_runtime_reload(app, result.message);
}

pub(crate) fn apply_cli_action_failure(app: &mut App, message: String) {
    app.plugins.loading = false;
    app.plugins.pending_runtime_reload_success_message = None;
    app.config.status_message = None;
    app.config.last_error = Some(message);
}

pub(crate) fn apply_runtime_reload_success(app: &mut App) {
    app.plugins.loading = false;
    app.plugins.last_error = None;
    if let Some(message) = app.plugins.pending_runtime_reload_success_message.take() {
        app.plugins.status_message = Some(message.clone());
        app.config.last_error = None;
        app.config.status_message = Some(message);
    }
}

pub(crate) fn apply_runtime_reload_failure(app: &mut App, message: &str) {
    app.plugins.loading = false;
    app.plugins.status_message = None;
    app.plugins.last_error = Some(message.to_owned());
    app.plugins.pending_runtime_reload_success_message = None;
    app.config.status_message = None;
    app.config.last_error = Some(format!("Failed to reload session plugins: {message}"));
}

pub(super) fn start_runtime_reload(app: &mut App, success_message: String) {
    app.plugins.loading = true;
    app.plugins.status_message = Some("Reloading session plugins...".to_owned());
    app.plugins.last_error = None;
    app.plugins.pending_runtime_reload_success_message = Some(success_message);
    app.config.last_error = None;
    app.config.status_message = Some("Reloading session plugins...".to_owned());
    match crate::app::session_runtime::request_runtime_reload(app) {
        crate::app::session_runtime::RuntimeReloadRequestOutcome::Requested => {}
        crate::app::session_runtime::RuntimeReloadRequestOutcome::Unavailable => {
            apply_runtime_reload_success(app);
        }
        crate::app::session_runtime::RuntimeReloadRequestOutcome::Failed => {
            apply_runtime_reload_failure(app, "failed to request session runtime plugin reload");
        }
    }
}

pub(super) fn installed_action_command(
    app: &App,
    overlay: &InstalledPluginActionOverlayState,
    action: InstalledPluginActionKind,
) -> (String, Vec<String>, String) {
    let cwd_raw = action_cwd(app, overlay);
    let plugin_id = overlay.plugin_id.clone();
    let scope = overlay.scope.clone();
    let action_label = display_label(&plugin_id);
    match action {
        InstalledPluginActionKind::Enable => (
            cwd_raw.clone(),
            vec![
                "plugin".to_owned(),
                "enable".to_owned(),
                plugin_id.clone(),
                "--scope".to_owned(),
                scope.clone(),
            ],
            format!("Enabling {action_label}..."),
        ),
        InstalledPluginActionKind::Disable => (
            cwd_raw.clone(),
            vec![
                "plugin".to_owned(),
                "disable".to_owned(),
                plugin_id.clone(),
                "--scope".to_owned(),
                scope.clone(),
            ],
            format!("Disabling {action_label}..."),
        ),
        InstalledPluginActionKind::Update => (
            cwd_raw.clone(),
            vec![
                "plugin".to_owned(),
                "update".to_owned(),
                plugin_id.clone(),
                "--scope".to_owned(),
                scope.clone(),
            ],
            format!("Updating {action_label}..."),
        ),
        InstalledPluginActionKind::Uninstall => (
            cwd_raw,
            vec![
                "plugin".to_owned(),
                "uninstall".to_owned(),
                plugin_id,
                "--scope".to_owned(),
                scope,
            ],
            format!("Uninstalling {action_label}..."),
        ),
    }
}

pub(super) fn installed_action_success_message(
    action: InstalledPluginActionKind,
    title: &str,
    scope: &str,
) -> String {
    let message = match action {
        InstalledPluginActionKind::Enable => format!("Enabled {title} in {scope} scope"),
        InstalledPluginActionKind::Disable => format!("Disabled {title} in {scope} scope"),
        InstalledPluginActionKind::Update => format!("Updated {title} in {scope} scope"),
        InstalledPluginActionKind::Uninstall => format!("Uninstalled {title} from {scope} scope"),
    };
    with_new_session_hint(message)
}

pub(super) fn plugin_install_success_message(
    action: PluginInstallActionKind,
    title: &str,
) -> String {
    let message = match action {
        PluginInstallActionKind::User => format!("Installed {title} for user scope"),
        PluginInstallActionKind::Project => format!("Installed {title} for project scope"),
        PluginInstallActionKind::Local => format!("Installed {title} locally"),
    };
    with_new_session_hint(message)
}

pub(super) fn with_new_session_hint(mut message: String) -> String {
    message.push_str(". You might need to run /new-session to apply plugin changes.");
    message
}

pub(super) fn marketplace_action_command(
    overlay: &MarketplaceActionsOverlayState,
    action: MarketplaceActionKind,
) -> Vec<String> {
    match action {
        MarketplaceActionKind::Update => vec![
            "plugin".to_owned(),
            "marketplace".to_owned(),
            "update".to_owned(),
            overlay.name.clone(),
        ],
        MarketplaceActionKind::Remove => vec![
            "plugin".to_owned(),
            "marketplace".to_owned(),
            "remove".to_owned(),
            overlay.name.clone(),
        ],
    }
}

pub(super) fn marketplace_action_status_message(
    title: &str,
    action: MarketplaceActionKind,
) -> String {
    match action {
        MarketplaceActionKind::Update => format!("Updating {title} marketplace..."),
        MarketplaceActionKind::Remove => format!("Removing {title} marketplace..."),
    }
}

pub(super) fn marketplace_action_success_message(
    title: &str,
    action: MarketplaceActionKind,
) -> String {
    match action {
        MarketplaceActionKind::Update => format!("Updated {title} marketplace"),
        MarketplaceActionKind::Remove => format!("Removed {title} marketplace"),
    }
}

pub(super) fn action_cwd(app: &App, overlay: &InstalledPluginActionOverlayState) -> String {
    match overlay.scope.as_str() {
        "local" | "project" => overlay.project_path.clone().unwrap_or_else(|| app.cwd_raw.clone()),
        _ => app.cwd_raw.clone(),
    }
}

pub(super) fn installed_overlay_actions(
    entry: &InstalledPluginEntry,
) -> Vec<InstalledPluginActionKind> {
    vec![
        if entry.enabled {
            InstalledPluginActionKind::Disable
        } else {
            InstalledPluginActionKind::Enable
        },
        InstalledPluginActionKind::Update,
        InstalledPluginActionKind::Uninstall,
    ]
}

pub(super) fn installed_overlay_description(app: &App, entry: &InstalledPluginEntry) -> String {
    if let Some(description) = app
        .plugins
        .marketplace
        .iter()
        .find(|candidate| candidate.plugin_id == entry.id)
        .and_then(|candidate| candidate.description.as_deref())
    {
        return description.to_owned();
    }

    match entry.project_path.as_deref() {
        Some(project_path) => format!("Installed in {} scope for {}.", entry.scope, project_path),
        None => format!("Installed in {} scope.", entry.scope),
    }
}
