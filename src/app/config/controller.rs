// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use super::prelude::*;

pub fn open(app: &mut App) -> Result<(), String> {
    open_category(app, "general")
}

pub(crate) fn open_category(app: &mut App, category: &str) -> Result<(), String> {
    open_tab(app, ConfigTab::Settings)?;
    app.config.settings.open_category(category);
    Ok(())
}

pub(crate) fn open_tab(app: &mut App, tab: ConfigTab) -> Result<(), String> {
    if !app.is_project_trusted() {
        return Err("Project trust must be accepted before opening settings".to_owned());
    }

    app.config.clear_overlay();
    app.config.status_message = None;
    app.config.last_error = None;
    app.config.active_tab = tab;
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
    if is_ctrl_shortcut(key, 'q') {
        app.request_shutdown();
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
    if handle_settings_key(app, key) {
        return;
    }

    match (key.code, key.modifiers) {
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
        (KeyCode::BackTab, _) | (KeyCode::Tab, KeyModifiers::SHIFT) => {
            activate_tab(app, app.config.active_tab.prev());
        }
        (KeyCode::Tab, KeyModifiers::NONE) => {
            activate_tab(app, app.config.active_tab.next());
        }
        _ => {}
    }
}

fn handle_settings_key(app: &mut App, key: KeyEvent) -> bool {
    use super::SettingsFocus;
    if app.config.active_tab != ConfigTab::Settings
        || matches!(key.code, KeyCode::Tab | KeyCode::BackTab)
    {
        return false;
    }
    if app.config.settings.focus == SettingsFocus::Search {
        handle_settings_search(app, key);
        return true;
    }
    if !key.modifiers.is_empty() {
        return false;
    }
    if key.code == KeyCode::Char('/') {
        app.config.settings.focus = SettingsFocus::Search;
        return true;
    }
    if key.code == KeyCode::Esc && !app.config.settings.query.is_empty() {
        app.config.settings.clear_search();
        app.config.settings.focus = SettingsFocus::Content;
        return true;
    }
    if app.config.settings.focus == SettingsFocus::CategoryTabs {
        handle_category_key(app, key);
        return true;
    }
    match key.code {
        KeyCode::Up
        | KeyCode::Down
        | KeyCode::Home
        | KeyCode::End
        | KeyCode::PageUp
        | KeyCode::PageDown => {
            if let Some(snapshot) = &app.config.snapshot {
                let items = app.config.settings.items(snapshot);
                let index = app.config.settings.selected_index(snapshot);
                if key.code == KeyCode::Up && index == 0 {
                    app.config.settings.focus = SettingsFocus::Search;
                } else if !items.is_empty() {
                    let page = app.config.settings.visible_count.max(1);
                    let next = match key.code {
                        KeyCode::Up => index.saturating_sub(1),
                        KeyCode::Down => (index + 1).min(items.len() - 1),
                        KeyCode::Home => 0,
                        KeyCode::End => items.len() - 1,
                        KeyCode::PageUp => index.saturating_sub(page),
                        _ => index.saturating_add(page).min(items.len() - 1),
                    };
                    let id = items[next].id.clone();
                    app.config.settings.select(id);
                } else {
                    app.config.settings.focus = SettingsFocus::Search;
                }
            } else {
                app.config.settings.focus = SettingsFocus::Search;
            }
        }
        KeyCode::Char(' ') => {
            if let Some(setting) = app.config.selected_setting().cloned() {
                edit::activate_setting(app, &setting);
            }
        }
        KeyCode::Enter if !app.config.settings.query.is_empty() => {
            let id = app.config.selected_setting().map(|setting| setting.id.clone());
            app.config.settings.clear_search();
            if let Some(id) = id {
                app.config.settings.select(id);
            }
        }
        KeyCode::Enter
            if app
                .config
                .selected_setting()
                .is_some_and(|setting| setting.kind.is_structured()) =>
        {
            if let Some(setting) = app.config.selected_setting().cloned() {
                edit::activate_setting(app, &setting);
            }
        }
        KeyCode::Left | KeyCode::Right => {
            edit::step_setting(app, if key.code == KeyCode::Left { -1 } else { 1 });
        }
        KeyCode::Char('s') => app.config.selected_scope = app.config.selected_scope.next(),
        KeyCode::Char('r') => super::service::request_settings(app),
        KeyCode::Delete => super::service::reset_selected(app),
        _ => return false,
    }
    true
}

fn handle_category_key(app: &mut App, key: KeyEvent) {
    use super::SettingsFocus;
    match key.code {
        KeyCode::Left | KeyCode::Right => {
            if let Some(snapshot) = &app.config.snapshot
                && !snapshot.categories.is_empty()
            {
                let index = snapshot
                    .categories
                    .iter()
                    .position(|category| category.id == app.config.settings.category)
                    .unwrap_or(0);
                let len = snapshot.categories.len();
                let next = if key.code == KeyCode::Left {
                    (index + len - 1) % len
                } else {
                    (index + 1) % len
                };
                let category = snapshot.categories[next].id.clone();
                app.config.settings.open_category(&category);
                app.config.settings.focus = SettingsFocus::CategoryTabs;
            }
        }
        KeyCode::Down => app.config.settings.focus = SettingsFocus::Search,
        KeyCode::Enter => {
            app.config.settings.clear_search();
            app.config.settings.focus = if app.config.selected_setting().is_some() {
                SettingsFocus::Content
            } else {
                SettingsFocus::Search
            };
        }
        KeyCode::Esc => close(app),
        _ => {}
    }
}

fn handle_settings_search(app: &mut App, key: KeyEvent) {
    use super::SettingsFocus;
    match (key.code, key.modifiers) {
        (KeyCode::Up, KeyModifiers::NONE) => {
            app.config.settings.focus = SettingsFocus::CategoryTabs;
        }
        (KeyCode::Down | KeyCode::Enter, KeyModifiers::NONE) => {
            if app.config.selected_setting().is_some() {
                app.config.settings.focus = SettingsFocus::Content;
            }
        }
        (KeyCode::Esc, KeyModifiers::NONE) => {
            app.config.settings.clear_search();
            app.config.settings.focus = if app.config.selected_setting().is_some() {
                SettingsFocus::Content
            } else {
                SettingsFocus::CategoryTabs
            };
        }
        (KeyCode::Left, KeyModifiers::NONE) => {
            edit::move_text_cursor_left(Some(app.config.settings.as_mut()));
        }
        (KeyCode::Right, KeyModifiers::NONE) => {
            edit::move_text_cursor_right(Some(app.config.settings.as_mut()));
        }
        (KeyCode::Home, KeyModifiers::NONE) => {
            edit::set_text_cursor(Some(app.config.settings.as_mut()), 0);
        }
        (KeyCode::End, KeyModifiers::NONE) => {
            edit::move_text_cursor_to_end(Some(app.config.settings.as_mut()));
        }
        (KeyCode::Backspace, KeyModifiers::NONE) => {
            edit::delete_text_before_cursor(Some(app.config.settings.as_mut()));
        }
        (KeyCode::Delete, KeyModifiers::NONE) => {
            edit::delete_text_at_cursor(Some(app.config.settings.as_mut()));
        }
        (KeyCode::Char(ch), modifiers) if edit::accepts_text_input(modifiers) => {
            edit::insert_text_char(Some(app.config.settings.as_mut()), ch);
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
    if app.config.active_tab == ConfigTab::Settings
        && app.config.settings.focus == super::SettingsFocus::Search
    {
        edit::insert_text_str(Some(app.config.settings.as_mut()), text);
        return true;
    }
    false
}

fn request_active_tab_side_effects(app: &mut App) {
    if app.config.active_tab == ConfigTab::Settings {
        super::service::request_settings(app);
    }
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
