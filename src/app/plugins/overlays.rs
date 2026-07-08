// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::actions::{
    execute_selected_installed_overlay_action, execute_selected_marketplace_action,
    execute_selected_plugin_install_action, installed_overlay_actions,
    installed_overlay_description,
};
use super::prelude::*;
use super::runtime_mcp::selected_installed_entry;
use super::selection::{
    selected_add_marketplace_row, selected_marketplace_plugin, selected_marketplace_source,
};
use super::text::{display_label, marketplace_overlay_description};

pub(crate) fn handle_installed_overlay_key(app: &mut App, key: KeyEvent) {
    match (key.code, key.modifiers) {
        (KeyCode::Esc, KeyModifiers::NONE) => app.config.clear_overlay(),
        (KeyCode::Up, KeyModifiers::NONE) => move_installed_overlay_selection(app, -1),
        (KeyCode::Down, KeyModifiers::NONE) => move_installed_overlay_selection(app, 1),
        (KeyCode::Enter, KeyModifiers::NONE) => execute_selected_installed_overlay_action(app),
        _ => {}
    }
}

pub(crate) fn handle_plugin_install_overlay_key(app: &mut App, key: KeyEvent) {
    match (key.code, key.modifiers) {
        (KeyCode::Esc, KeyModifiers::NONE) => app.config.clear_overlay(),
        (KeyCode::Up, KeyModifiers::NONE) => move_plugin_install_overlay_selection(app, -1),
        (KeyCode::Down, KeyModifiers::NONE) => move_plugin_install_overlay_selection(app, 1),
        (KeyCode::Enter, KeyModifiers::NONE) => execute_selected_plugin_install_action(app),
        _ => {}
    }
}

pub(crate) fn handle_marketplace_overlay_key(app: &mut App, key: KeyEvent) {
    match (key.code, key.modifiers) {
        (KeyCode::Esc, KeyModifiers::NONE) => app.config.clear_overlay(),
        (KeyCode::Up, KeyModifiers::NONE) => move_marketplace_overlay_selection(app, -1),
        (KeyCode::Down, KeyModifiers::NONE) => move_marketplace_overlay_selection(app, 1),
        (KeyCode::Enter, KeyModifiers::NONE) => execute_selected_marketplace_action(app),
        _ => {}
    }
}

pub(crate) fn handle_add_marketplace_overlay_key(app: &mut App, key: KeyEvent) {
    match (key.code, key.modifiers) {
        (KeyCode::Enter, KeyModifiers::NONE) => confirm_add_marketplace_overlay(app),
        (KeyCode::Esc, KeyModifiers::NONE) => app.config.clear_overlay(),
        (KeyCode::Left, KeyModifiers::NONE) => {
            move_add_marketplace_cursor_left(app);
        }
        (KeyCode::Right, KeyModifiers::NONE) => {
            move_add_marketplace_cursor_right(app);
        }
        (KeyCode::Home, KeyModifiers::NONE) => set_add_marketplace_cursor(app, 0),
        (KeyCode::End, KeyModifiers::NONE) => move_add_marketplace_cursor_to_end(app),
        (KeyCode::Backspace, KeyModifiers::NONE) => delete_add_marketplace_before_cursor(app),
        (KeyCode::Delete, KeyModifiers::NONE) => delete_add_marketplace_at_cursor(app),
        (KeyCode::Char(ch), modifiers)
            if modifiers.is_empty() || modifiers == KeyModifiers::SHIFT =>
        {
            insert_add_marketplace_char(app, ch);
        }
        _ => {}
    }
}

pub(super) fn open_marketplace_overlay(app: &mut App) -> bool {
    if selected_add_marketplace_row(app) {
        open_add_marketplace_overlay(app)
    } else {
        open_marketplace_actions_overlay(app)
    }
}

pub(super) fn open_installed_actions_overlay(app: &mut App) -> bool {
    let Some(entry) = selected_installed_entry(app).cloned() else {
        return false;
    };
    open_installed_actions_overlay_for_entry(app, entry);
    true
}

pub(super) fn open_installed_actions_overlay_for_entry(app: &mut App, entry: InstalledPluginEntry) {
    let title = display_label(&entry.id);
    let description = installed_overlay_description(app, &entry);
    let actions = installed_overlay_actions(&entry);
    app.config.replace_overlay(ConfigOverlayState::InstalledPluginActions(
        InstalledPluginActionOverlayState {
            plugin_id: entry.id,
            title,
            description,
            scope: entry.scope,
            project_path: entry.project_path,
            selected_index: 0,
            actions,
        },
    ));
}

pub(super) fn open_plugin_install_overlay(app: &mut App) -> bool {
    let selected = selected_marketplace_plugin(app).cloned();
    let Some(entry) = selected else {
        return false;
    };

    app.config.replace_overlay(ConfigOverlayState::PluginInstallActions(
        PluginInstallOverlayState {
            plugin_id: entry.plugin_id,
            title: display_label(&entry.name),
            description: entry
                .description
                .unwrap_or_else(|| "Install this plugin into Claude Code.".to_owned()),
            selected_index: 0,
            actions: vec![
                PluginInstallActionKind::User,
                PluginInstallActionKind::Project,
                PluginInstallActionKind::Local,
            ],
        },
    ));
    true
}

pub(super) fn open_marketplace_actions_overlay(app: &mut App) -> bool {
    let selected = selected_marketplace_source(app).cloned();
    let Some(entry) = selected else {
        return false;
    };

    app.config.replace_overlay(ConfigOverlayState::MarketplaceActions(
        MarketplaceActionsOverlayState {
            name: entry.name.clone(),
            title: display_label(&entry.name),
            description: marketplace_overlay_description(&entry),
            selected_index: 0,
            actions: vec![MarketplaceActionKind::Update, MarketplaceActionKind::Remove],
        },
    ));
    true
}

pub(super) fn open_add_marketplace_overlay(app: &mut App) -> bool {
    app.config.replace_overlay(ConfigOverlayState::AddMarketplace(
        AddMarketplaceOverlayState::from_text_input(String::new(), 0),
    ));
    true
}

pub(super) fn move_installed_overlay_selection(app: &mut App, delta: isize) {
    let Some(overlay) = app.config.installed_plugin_actions_overlay_mut() else {
        return;
    };
    let len = overlay.actions.len();
    if len == 0 {
        overlay.selected_index = 0;
        return;
    }
    let current = overlay.selected_index;
    overlay.selected_index = if delta.is_negative() {
        current.saturating_sub(delta.unsigned_abs())
    } else {
        current.saturating_add(delta.cast_unsigned()).min(len.saturating_sub(1))
    };
}

pub(super) fn move_plugin_install_overlay_selection(app: &mut App, delta: isize) {
    let Some(overlay) = app.config.plugin_install_overlay_mut() else {
        return;
    };
    let len = overlay.actions.len();
    if len == 0 {
        overlay.selected_index = 0;
        return;
    }
    let current = overlay.selected_index;
    overlay.selected_index = if delta.is_negative() {
        current.saturating_sub(delta.unsigned_abs())
    } else {
        current.saturating_add(delta.cast_unsigned()).min(len.saturating_sub(1))
    };
}

pub(super) fn move_marketplace_overlay_selection(app: &mut App, delta: isize) {
    let Some(overlay) = app.config.marketplace_actions_overlay_mut() else {
        return;
    };
    let len = overlay.actions.len();
    if len == 0 {
        overlay.selected_index = 0;
        return;
    }
    let current = overlay.selected_index;
    overlay.selected_index = if delta.is_negative() {
        current.saturating_sub(delta.unsigned_abs())
    } else {
        current.saturating_add(delta.cast_unsigned()).min(len.saturating_sub(1))
    };
}

pub(super) fn open_installed_plugin_uninstall_confirmation(
    app: &mut App,
    overlay: InstalledPluginActionOverlayState,
) {
    let title = format!("Uninstall {}", overlay.title);
    let scope = overlay.scope.clone();
    let plugin_id = overlay.plugin_id.clone();
    let body = match overlay.project_path.as_deref() {
        Some(project_path) => format!(
            "Uninstall plugin {plugin_id} from {scope} scope for {project_path}? This removes the plugin registration from that scope."
        ),
        None => format!(
            "Uninstall plugin {plugin_id} from {scope} scope? This removes the plugin registration from that scope."
        ),
    };
    open_confirmation_overlay(
        app,
        ConfigOverlayState::InstalledPluginActions(overlay),
        ConfirmationAction::InstalledPluginUninstall,
        title,
        body,
        "Uninstall",
    );
}

pub(super) fn open_marketplace_remove_confirmation(
    app: &mut App,
    overlay: MarketplaceActionsOverlayState,
) {
    let title = format!("Remove {}", overlay.title);
    let marketplace_name = overlay.name.clone();
    let body = format!(
        "Remove marketplace {marketplace_name} from user configuration? Plugins already installed from it remain installed."
    );
    open_confirmation_overlay(
        app,
        ConfigOverlayState::MarketplaceActions(overlay),
        ConfirmationAction::MarketplaceRemove,
        title,
        body,
        "Remove",
    );
}

pub(super) fn open_confirmation_overlay(
    app: &mut App,
    previous: ConfigOverlayState,
    action: ConfirmationAction,
    title: impl Into<String>,
    body: impl Into<String>,
    confirm_label: impl Into<String>,
) {
    app.config.replace_overlay(ConfigOverlayState::Confirmation(ConfirmationOverlayState {
        title: title.into(),
        body: body.into(),
        confirm_label: confirm_label.into(),
        cancel_label: "Cancel".to_owned(),
        selected_index: 0,
        action,
        previous: Box::new(previous),
    }));
}

pub(super) fn confirm_add_marketplace_overlay(app: &mut App) {
    let Some(overlay) = app.config.add_marketplace_overlay().cloned() else {
        return;
    };
    let source = overlay.draft.trim().to_owned();
    if source.is_empty() {
        app.config.set_overlay_error("Marketplace source cannot be empty");
        return;
    }
    if tokio::runtime::Handle::try_current().is_err() {
        app.config.clear_overlay();
        app.config.status_message = None;
        app.config.last_error = Some("No runtime available for marketplace action".to_owned());
        return;
    }

    let args = vec![
        "plugin".to_owned(),
        "marketplace".to_owned(),
        "add".to_owned(),
        source.clone(),
        "--scope".to_owned(),
        "user".to_owned(),
    ];

    app.config.clear_overlay();
    app.config.last_error = None;
    app.config.status_message = Some(format!("Adding marketplace {source}..."));
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
                let _ = event_tx.send(ClientEvent::PluginsCliActionSucceeded {
                    cwd_raw: cwd_context,
                    result: PluginsCliActionSuccess {
                        snapshot,
                        message: format!("Added marketplace {source}"),
                        claude_path,
                    },
                });
            }
            Err(message) => {
                let _ = event_tx
                    .send(ClientEvent::PluginsCliActionFailed { cwd_raw: cwd_context, message });
            }
        }
    });
}

pub(super) fn move_add_marketplace_cursor_left(app: &mut App) {
    let Some(overlay) = app.config.add_marketplace_overlay_mut() else {
        return;
    };
    overlay.cursor = overlay.cursor.saturating_sub(1);
}

pub(super) fn move_add_marketplace_cursor_right(app: &mut App) {
    let Some(overlay) = app.config.add_marketplace_overlay_mut() else {
        return;
    };
    overlay.cursor = overlay.cursor.saturating_add(1).min(overlay.draft.chars().count());
}

pub(super) fn move_add_marketplace_cursor_to_end(app: &mut App) {
    let Some(overlay) = app.config.add_marketplace_overlay_mut() else {
        return;
    };
    overlay.cursor = overlay.draft.chars().count();
}

pub(super) fn set_add_marketplace_cursor(app: &mut App, cursor: usize) {
    let Some(overlay) = app.config.add_marketplace_overlay_mut() else {
        return;
    };
    overlay.cursor = cursor.min(overlay.draft.chars().count());
}

pub(super) fn insert_add_marketplace_char(app: &mut App, ch: char) {
    let Some(overlay) = app.config.add_marketplace_overlay_mut() else {
        return;
    };
    let byte_index = char_to_byte_index(&overlay.draft, overlay.cursor);
    overlay.draft.insert(byte_index, ch);
    overlay.cursor += 1;
}

pub(super) fn delete_add_marketplace_before_cursor(app: &mut App) {
    let Some(overlay) = app.config.add_marketplace_overlay_mut() else {
        return;
    };
    if overlay.cursor == 0 {
        return;
    }
    let end = char_to_byte_index(&overlay.draft, overlay.cursor);
    let start = char_to_byte_index(&overlay.draft, overlay.cursor - 1);
    overlay.draft.replace_range(start..end, "");
    overlay.cursor -= 1;
}

pub(super) fn delete_add_marketplace_at_cursor(app: &mut App) {
    let Some(overlay) = app.config.add_marketplace_overlay_mut() else {
        return;
    };
    let char_count = overlay.draft.chars().count();
    if overlay.cursor >= char_count {
        return;
    }
    let start = char_to_byte_index(&overlay.draft, overlay.cursor);
    let end = char_to_byte_index(&overlay.draft, overlay.cursor + 1);
    overlay.draft.replace_range(start..end, "");
}

pub(super) fn char_to_byte_index(text: &str, char_index: usize) -> usize {
    text.char_indices().nth(char_index).map_or(text.len(), |(idx, _)| idx)
}
