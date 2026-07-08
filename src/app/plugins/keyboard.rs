// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::inventory::request_inventory_refresh_manual;
use super::overlays::{
    open_installed_actions_overlay, open_marketplace_overlay, open_plugin_install_overlay,
};
use super::prelude::*;
use super::selection::{
    clamp_selection, move_selection, reset_selection_for_active_tab, search_enabled,
};
use super::text::normalize_single_line_input;

pub(crate) fn handle_paste(app: &mut App, text: &str) -> bool {
    if !search_enabled(app.plugins.active_tab) || !app.plugins.search_focused {
        return false;
    }
    let normalized = normalize_single_line_input(text);
    if normalized.is_empty() {
        return false;
    }
    if let Some(query) = app.plugins.active_search_query_mut() {
        query.push_str(&normalized);
        reset_selection_for_active_tab(app);
        return true;
    }
    false
}

pub(crate) fn handle_key(app: &mut App, key: KeyEvent) -> bool {
    match (key.code, key.modifiers) {
        (KeyCode::Left, KeyModifiers::NONE) => {
            app.plugins.active_tab = app.plugins.active_tab.prev();
            app.plugins.search_focused = false;
            clamp_selection(app);
            true
        }
        (KeyCode::Right, KeyModifiers::NONE) => {
            app.plugins.active_tab = app.plugins.active_tab.next();
            app.plugins.search_focused = false;
            clamp_selection(app);
            true
        }
        (KeyCode::Up, KeyModifiers::NONE) => {
            if search_enabled(app.plugins.active_tab)
                && !app.plugins.search_focused
                && app.plugins.selected_index_for(app.plugins.active_tab) == 0
            {
                app.plugins.search_focused = true;
            } else if !app.plugins.search_focused {
                move_selection(app, -1);
            }
            true
        }
        (KeyCode::Down, KeyModifiers::NONE) => {
            if app.plugins.search_focused {
                app.plugins.search_focused = false;
            } else {
                move_selection(app, 1);
            }
            true
        }
        (KeyCode::Enter, KeyModifiers::NONE) => {
            if app.plugins.search_focused {
                false
            } else {
                match app.plugins.active_tab {
                    PluginsViewTab::Installed => open_installed_actions_overlay(app),
                    PluginsViewTab::Plugins => open_plugin_install_overlay(app),
                    PluginsViewTab::Marketplace => open_marketplace_overlay(app),
                }
            }
        }
        (KeyCode::Backspace, KeyModifiers::NONE) => {
            if search_enabled(app.plugins.active_tab)
                && app.plugins.search_focused
                && let Some(query) = app.plugins.active_search_query_mut()
                && query.pop().is_some()
            {
                reset_selection_for_active_tab(app);
            }
            true
        }
        (KeyCode::Delete, KeyModifiers::NONE) => {
            if search_enabled(app.plugins.active_tab)
                && app.plugins.search_focused
                && let Some(query) = app.plugins.active_search_query_mut()
                && !query.is_empty()
            {
                query.clear();
                reset_selection_for_active_tab(app);
            }
            true
        }
        (KeyCode::Char(ch), modifiers)
            if matches!(ch, 'r' | 'R')
                && (modifiers.is_empty() || modifiers == KeyModifiers::SHIFT)
                && !app.plugins.search_focused =>
        {
            request_inventory_refresh_manual(app);
            true
        }
        (KeyCode::Char(ch), modifiers)
            if modifiers.is_empty() || modifiers == KeyModifiers::SHIFT =>
        {
            if search_enabled(app.plugins.active_tab)
                && app.plugins.search_focused
                && let Some(query) = app.plugins.active_search_query_mut()
            {
                query.push(ch);
                reset_selection_for_active_tab(app);
            }
            true
        }
        _ => false,
    }
}
