// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::actions::start_runtime_reload;
use super::prelude::*;
use super::selection::clamp_selection;

const INVENTORY_REFRESH_TTL: Duration = Duration::from_secs(5);

pub(crate) fn request_inventory_refresh_if_needed(app: &mut App) {
    if app.plugins.loading {
        return;
    }
    if app
        .plugins
        .last_inventory_refresh_at
        .is_some_and(|refreshed_at| refreshed_at.elapsed() < INVENTORY_REFRESH_TTL)
    {
        clamp_selection(app);
        return;
    }
    request_inventory_refresh(app);
}

pub(crate) fn request_inventory_refresh_manual(app: &mut App) {
    app.plugins.runtime_reload_after_refresh = true;
    request_inventory_refresh(app);
}

pub(crate) fn request_inventory_refresh(app: &mut App) {
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    app.plugins.loading = true;
    app.plugins.clear_feedback();
    app.plugins.status_message = Some("Refreshing plugin inventory...".to_owned());
    app.request_active_surface_repaint();
    let event_tx = app.event_tx.clone();
    let cwd_context = app.cwd_raw.clone();
    let cwd_raw = app.cwd_raw.clone();
    let cached_claude_path = app.plugins.claude_path.clone();
    tokio::task::spawn_local(async move {
        match cli::refresh_inventory(cwd_raw, cached_claude_path).await {
            Ok((snapshot, claude_path)) => {
                let _ = event_tx.send(crate::agent::events::ClientEvent::PluginsInventoryUpdated {
                    cwd_raw: cwd_context,
                    snapshot,
                    claude_path,
                });
            }
            Err(message) => {
                let _ = event_tx.send(
                    crate::agent::events::ClientEvent::PluginsInventoryRefreshFailed {
                        cwd_raw: cwd_context,
                        message,
                    },
                );
            }
        }
    });
}

pub(crate) fn apply_inventory_refresh_success(
    app: &mut App,
    snapshot: PluginsInventorySnapshot,
    claude_path: PathBuf,
) {
    let should_reload_runtime = std::mem::take(&mut app.plugins.runtime_reload_after_refresh);
    app.plugins.installed = snapshot.installed;
    app.plugins.marketplace = snapshot.marketplace;
    app.plugins.marketplaces = snapshot.marketplaces;
    app.plugins.loading = false;
    app.plugins.last_error = None;
    app.plugins.last_inventory_refresh_at = Some(Instant::now());
    app.plugins.claude_path = Some(claude_path);
    clamp_selection(app);
    crate::app::config::reconcile_stale_plugin_mcp_servers(app);
    if should_reload_runtime {
        start_runtime_reload(app, "Plugin inventory refreshed".to_owned());
    } else {
        app.plugins.status_message = Some("Plugin inventory refreshed".to_owned());
        app.config.last_error = None;
        app.config.status_message = Some("Plugin inventory refreshed".to_owned());
    }
}

pub(crate) fn apply_inventory_refresh_failure(app: &mut App, message: String) {
    app.plugins.loading = false;
    app.plugins.runtime_reload_after_refresh = false;
    app.plugins.pending_runtime_reload_success_message = None;
    app.plugins.status_message = None;
    app.plugins.last_error = Some(message);
}

pub(crate) fn reset_for_session_change(app: &mut App) {
    app.plugins.loading = false;
    app.plugins.status_message = None;
    app.plugins.last_error = None;
    app.plugins.last_inventory_refresh_at = None;
    app.plugins.installed.clear();
    app.plugins.marketplace.clear();
    app.plugins.marketplaces.clear();
    app.plugins.claude_path = None;
    app.plugins.runtime_reload_after_refresh = false;
    app.plugins.pending_runtime_reload_success_message = None;
    clamp_selection(app);
}
