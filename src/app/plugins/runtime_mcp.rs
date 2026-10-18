// SPDX-License-Identifier: Apache-2.0

use super::overlays::open_installed_actions_overlay_for_entry;
use super::prelude::*;
use super::selection::{installed_entry_applies_to_project, ordered_installed};
use super::text::display_label;

pub(super) fn selected_installed_entry(app: &App) -> Option<&InstalledPluginEntry> {
    ordered_installed(&app.plugins, &app.cwd_raw).get(app.plugins.installed_selected_index).copied()
}

pub(crate) fn open_plugin_for_mcp_server(app: &mut App, server_name: &str) -> bool {
    if let Some(plugin) = session_mcp_plugin_for_runtime_server(app, server_name) {
        app.plugins.session_selected_index =
            app.plugins.session_plugins.iter().position(|entry| entry == plugin).unwrap_or(0);
        app.plugins.active_tab = PluginsViewTab::Session;
        app.plugins.search_focused = false;
        app.config.active_tab = crate::app::ConfigTab::Plugins;
        app.config.clear_overlay();
        return true;
    }
    let Some(entry) = installed_mcp_plugin_for_runtime_server(app, server_name).cloned() else {
        return false;
    };
    open_installed_actions_overlay_for_entry(app, entry);
    true
}

pub(crate) fn installed_mcp_plugin_for_runtime_server<'a>(
    app: &'a App,
    server_name: &str,
) -> Option<&'a InstalledPluginEntry> {
    let runtime_name = parse_plugin_mcp_runtime_server_name(server_name)?;
    app.plugins
        .installed
        .iter()
        .filter(|entry| installed_entry_applies_to_project(entry, &app.cwd_raw))
        .filter(|entry| {
            entry
                .mcp_server_names
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(runtime_name.mcp_server_name))
        })
        .find(|entry| plugin_entry_matches_runtime_name(entry, runtime_name.plugin_name))
}

pub(crate) fn is_stale_plugin_mcp_runtime_server(app: &App, server_name: &str) -> bool {
    is_plugin_mcp_runtime_server_name(server_name)
        && app.plugins.last_inventory_refresh_at.is_some()
        && installed_mcp_plugin_for_runtime_server(app, server_name).is_none()
        && session_mcp_plugin_for_runtime_server(app, server_name).is_none()
}

pub(crate) fn session_mcp_plugin_for_runtime_server<'a>(
    app: &'a App,
    server_name: &str,
) -> Option<&'a crate::agent::types::SessionPlugin> {
    let runtime = parse_plugin_mcp_runtime_server_name(server_name)?;
    app.plugins
        .session_plugins
        .iter()
        .find(|plugin| plugin_id_matches_runtime_name(&plugin.name, runtime.plugin_name))
}

pub(crate) fn is_plugin_mcp_runtime_server_name(server_name: &str) -> bool {
    parse_plugin_mcp_runtime_server_name(server_name).is_some()
}

struct PluginMcpRuntimeServerName<'a> {
    plugin_name: &'a str,
    mcp_server_name: &'a str,
}

fn parse_plugin_mcp_runtime_server_name(
    server_name: &str,
) -> Option<PluginMcpRuntimeServerName<'_>> {
    let (prefix, rest) = server_name.split_once(':')?;
    if !prefix.eq_ignore_ascii_case("plugin") {
        return None;
    }
    let (plugin_name, mcp_server_name) = rest.split_once(':')?;
    if plugin_name.trim().is_empty() || mcp_server_name.trim().is_empty() {
        return None;
    }
    Some(PluginMcpRuntimeServerName { plugin_name, mcp_server_name })
}

pub(super) fn plugin_entry_matches_runtime_name(
    entry: &InstalledPluginEntry,
    runtime_plugin_name: &str,
) -> bool {
    plugin_id_matches_runtime_name(&entry.id, runtime_plugin_name)
}

fn plugin_id_matches_runtime_name(id: &str, runtime_plugin_name: &str) -> bool {
    let runtime = normalize_plugin_runtime_identifier(runtime_plugin_name);
    let id_base = id.split('@').next().unwrap_or(id);
    [
        normalize_plugin_runtime_identifier(id_base),
        normalize_plugin_runtime_identifier(&display_label(id_base)),
        normalize_plugin_runtime_identifier(id),
    ]
    .into_iter()
    .any(|candidate| !candidate.is_empty() && candidate == runtime)
}

pub(super) fn normalize_plugin_runtime_identifier(value: &str) -> String {
    value.chars().filter(char::is_ascii_alphanumeric).map(|ch| ch.to_ascii_lowercase()).collect()
}
