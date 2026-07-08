// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::prelude::*;
use super::text::normalize_project_path;

pub(crate) fn clamp_selection(app: &mut App) {
    let installed_len = ordered_installed(&app.plugins, &app.cwd_raw).len();
    let plugin_len = filtered_marketplace_plugins(&app.plugins).len();
    let marketplace_len = marketplace_row_count(&app.plugins);
    app.plugins.installed_selected_index =
        clamp_index(app.plugins.installed_selected_index, installed_len);
    app.plugins.plugins_selected_index =
        clamp_index(app.plugins.plugins_selected_index, plugin_len);
    app.plugins.marketplace_selected_index =
        clamp_index(app.plugins.marketplace_selected_index, marketplace_len);
}

#[must_use]
pub(crate) fn filtered_installed(state: &PluginsState) -> Vec<&InstalledPluginEntry> {
    state
        .installed
        .iter()
        .filter(|entry| {
            installed_entry_matches(entry, state.search_query_for(PluginsViewTab::Installed))
        })
        .collect()
}

#[must_use]
pub(crate) fn ordered_installed<'a>(
    state: &'a PluginsState,
    current_project_raw: &str,
) -> Vec<&'a InstalledPluginEntry> {
    let current_project = normalize_project_path(current_project_raw);
    filtered_installed(state)
        .into_iter()
        .filter(|entry| is_visible_installed_entry(entry, &current_project))
        .collect()
}

#[must_use]
pub(crate) fn filtered_marketplace_plugins(state: &PluginsState) -> Vec<&MarketplaceEntry> {
    state
        .marketplace
        .iter()
        .filter(|entry| {
            marketplace_plugin_matches(entry, state.search_query_for(PluginsViewTab::Plugins))
        })
        .collect()
}

#[must_use]
pub(crate) fn visible_marketplaces(state: &PluginsState) -> Vec<&MarketplaceSourceEntry> {
    state.marketplaces.iter().collect()
}

pub(super) fn selected_marketplace_plugin(app: &App) -> Option<&MarketplaceEntry> {
    filtered_marketplace_plugins(&app.plugins).get(app.plugins.plugins_selected_index).copied()
}

pub(super) fn selected_marketplace_source(app: &App) -> Option<&MarketplaceSourceEntry> {
    visible_marketplaces(&app.plugins).get(app.plugins.marketplace_selected_index).copied()
}

pub(super) fn selected_add_marketplace_row(app: &App) -> bool {
    app.plugins.marketplace_selected_index >= visible_marketplaces(&app.plugins).len()
}

pub(super) fn marketplace_row_count(state: &PluginsState) -> usize {
    state.marketplaces.len().saturating_add(1)
}

pub(super) fn reset_selection_for_active_tab(app: &mut App) {
    app.plugins.set_selected_index_for(app.plugins.active_tab, 0);
    clamp_selection(app);
}

pub(super) fn move_selection(app: &mut App, delta: isize) {
    let tab = app.plugins.active_tab;
    let len = match tab {
        PluginsViewTab::Installed => ordered_installed(&app.plugins, &app.cwd_raw).len(),
        PluginsViewTab::Plugins => filtered_marketplace_plugins(&app.plugins).len(),
        PluginsViewTab::Marketplace => marketplace_row_count(&app.plugins),
    };
    if len == 0 {
        app.plugins.set_selected_index_for(tab, 0);
        return;
    }
    let current = app.plugins.selected_index_for(tab);
    let next = if delta.is_negative() {
        current.saturating_sub(delta.unsigned_abs())
    } else {
        current.saturating_add(delta.cast_unsigned()).min(len.saturating_sub(1))
    };
    app.plugins.set_selected_index_for(tab, next);
}

pub(super) fn clamp_index(current: usize, len: usize) -> usize {
    if len == 0 { 0 } else { current.min(len.saturating_sub(1)) }
}

pub(super) fn installed_entry_matches(entry: &InstalledPluginEntry, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let query = query.to_ascii_lowercase();
    entry.id.to_ascii_lowercase().contains(&query)
        || entry.scope.to_ascii_lowercase().contains(&query)
        || entry
            .version
            .as_deref()
            .is_some_and(|version| version.to_ascii_lowercase().contains(&query))
}

pub(super) fn is_visible_installed_entry(
    entry: &InstalledPluginEntry,
    current_project: &str,
) -> bool {
    match entry.scope.as_str() {
        "user" => true,
        "local" | "project" => entry
            .project_path
            .as_deref()
            .map(normalize_project_path)
            .is_some_and(|project| project == current_project),
        _ => false,
    }
}

pub(super) fn installed_entry_applies_to_project(
    entry: &InstalledPluginEntry,
    current_project_raw: &str,
) -> bool {
    let current_project = normalize_project_path(current_project_raw);
    is_visible_installed_entry(entry, &current_project)
}

pub(super) fn marketplace_plugin_matches(entry: &MarketplaceEntry, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let query = query.to_ascii_lowercase();
    entry.plugin_id.to_ascii_lowercase().contains(&query)
        || entry.name.to_ascii_lowercase().contains(&query)
        || entry
            .description
            .as_deref()
            .is_some_and(|description| description.to_ascii_lowercase().contains(&query))
        || entry
            .marketplace_name
            .as_deref()
            .is_some_and(|marketplace| marketplace.to_ascii_lowercase().contains(&query))
        || entry
            .version
            .as_deref()
            .is_some_and(|version| version.to_ascii_lowercase().contains(&query))
}

#[must_use]
pub(crate) const fn search_enabled(tab: PluginsViewTab) -> bool {
    !matches!(tab, PluginsViewTab::Marketplace)
}
