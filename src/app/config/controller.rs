// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::prelude::*;

pub fn initialize_shared_state(app: &mut App) -> Result<(), String> {
    let loaded = store::load(app.settings_home_override.as_deref(), Some(project_root(app)))?;
    let notice = loaded.notice.clone();
    app.config.apply_loaded(loaded, notice, false);
    app.reconcile_runtime_from_persisted_settings_change();
    Ok(())
}

pub fn open(app: &mut App) -> Result<(), String> {
    if !app.is_project_trusted() {
        return Err("Project trust must be accepted before opening settings".to_owned());
    }

    let loaded = store::load(app.settings_home_override.as_deref(), Some(project_root(app)))?;
    let notice = loaded.notice.clone();
    app.config.apply_loaded(loaded, notice, false);
    app.reconcile_runtime_from_persisted_settings_change();
    view::set_fullscreen_view(app, FullscreenView::Config);
    request_active_tab_side_effects(app);
    Ok(())
}

pub(crate) fn refresh_runtime_tabs_for_session_change(app: &mut App) {
    if app.surface_mode != SurfaceMode::Fullscreen(FullscreenView::Config) {
        return;
    }
    request_status_snapshot_if_needed(app);
    if app.config.active_tab == ConfigTab::Usage {
        crate::app::usage::request_refresh_if_needed(app);
    }
    if app.config.active_tab == ConfigTab::Plugins {
        crate::app::plugins::request_inventory_refresh_if_needed(app);
    }
}

pub fn close(app: &mut App) {
    view::set_chat_surface(app);
}

pub(crate) fn activate_tab(app: &mut App, tab: ConfigTab) {
    app.config.active_tab = tab;
    app.config.status_message = None;
    app.config.last_error = None;
    request_active_tab_side_effects(app);
}

pub fn handle_key(app: &mut App, key: KeyEvent) {
    if is_ctrl_shortcut(key, 'q') || is_ctrl_shortcut(key, 'c') {
        app.should_quit = true;
        return;
    }

    if app.config.overlay.is_some() {
        edit::handle_overlay_key(app, key);
        return;
    }

    if app.config.active_tab == ConfigTab::Help && help::handle_key(app, key) {
        return;
    }
    if app.config.active_tab == ConfigTab::Plugins && crate::app::plugins::handle_key(app, key) {
        return;
    }
    if mcp::handle_mcp_key(app, key) {
        return;
    }

    match (key.code, key.modifiers) {
        (KeyCode::Char(' '), KeyModifiers::NONE)
            if app.config.active_tab == ConfigTab::Settings =>
        {
            if let Some(spec) = app.config.selected_setting_spec() {
                edit::activate_setting(app, spec);
            }
        }
        (KeyCode::Left, KeyModifiers::NONE) if app.config.active_tab == ConfigTab::Settings => {
            if let Some(spec) = app.config.selected_setting_spec() {
                edit::step_setting(app, spec, -1);
            }
        }
        (KeyCode::Right, KeyModifiers::NONE) if app.config.active_tab == ConfigTab::Settings => {
            if let Some(spec) = app.config.selected_setting_spec() {
                edit::step_setting(app, spec, 1);
            }
        }
        (KeyCode::Char(ch), modifiers)
            if app.config.active_tab == ConfigTab::Status
                && matches!(ch, 'r' | 'R')
                && (modifiers.is_empty() || modifiers == KeyModifiers::SHIFT) =>
        {
            edit::open_session_rename_overlay(app);
        }
        (KeyCode::Char(ch), modifiers)
            if app.config.active_tab == ConfigTab::Status
                && matches!(ch, 'g' | 'G')
                && (modifiers.is_empty() || modifiers == KeyModifiers::SHIFT) =>
        {
            edit::generate_session_title(app);
        }
        (KeyCode::Char(ch), modifiers)
            if app.config.active_tab == ConfigTab::Usage
                && matches!(ch, 'r' | 'R')
                && (modifiers.is_empty() || modifiers == KeyModifiers::SHIFT) =>
        {
            crate::app::usage::request_refresh(app);
        }
        (KeyCode::Enter | KeyCode::Esc, KeyModifiers::NONE) => {
            close(app);
        }
        (KeyCode::BackTab, _) => {
            activate_tab(app, app.config.active_tab.prev());
        }
        (KeyCode::Tab, KeyModifiers::NONE) => {
            activate_tab(app, app.config.active_tab.next());
        }
        (KeyCode::Up, KeyModifiers::NONE) => {
            if app.config.active_tab == ConfigTab::Settings {
                app.config.selected_setting_index =
                    app.config.selected_setting_index.saturating_sub(1);
            }
        }
        (KeyCode::Down, KeyModifiers::NONE) => {
            if app.config.active_tab == ConfigTab::Settings {
                let last_index = setting_specs().len().saturating_sub(1);
                app.config.selected_setting_index =
                    (app.config.selected_setting_index + 1).min(last_index);
            }
        }
        _ => {}
    }
}

pub fn handle_paste(app: &mut App, text: &str) -> bool {
    if app.config.overlay.is_some() {
        return edit::handle_overlay_paste(app, text);
    }
    if app.config.active_tab == ConfigTab::Plugins {
        return crate::app::plugins::handle_paste(app, text);
    }
    false
}

fn request_active_tab_side_effects(app: &mut App) {
    request_status_snapshot_if_needed(app);
    mcp::refresh_mcp_snapshot_if_needed(app);
    if app.config.active_tab == ConfigTab::Usage {
        crate::app::usage::request_refresh_if_needed(app);
    }
    if app.config.active_tab == ConfigTab::Plugins {
        crate::app::plugins::request_inventory_refresh_if_needed(app);
    }
}

fn is_ctrl_shortcut(key: KeyEvent, ch: char) -> bool {
    matches!(key.code, KeyCode::Char(candidate) if candidate == ch)
        && key.modifiers == KeyModifiers::CONTROL
}
