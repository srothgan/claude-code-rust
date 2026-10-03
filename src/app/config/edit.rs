// SPDX-License-Identifier: Apache-2.0
use super::{
    AddMarketplaceOverlayState, ConfigOverlayState, ConfirmationAction,
    PendingSessionTitleChangeKind, PendingSessionTitleChangeState, SessionRenameOverlayState,
    SettingOverlayState,
};
use crate::agent::settings::{SettingDescriptor, SettingKind, SettingsMutation, SettingsOperation};
use crate::app::App;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub(super) fn activate_setting(app: &mut App, setting: &SettingDescriptor) {
    if app.config.pending_settings_request.is_some() {
        return;
    }
    let Some(snapshot) = &app.config.snapshot else {
        return;
    };
    let scope = app.config.selected_scope;
    if !setting.writable_at(scope) {
        app.config.last_error = Some(
            setting
                .unavailable
                .clone()
                .unwrap_or_else(|| "This scope is read-only for this setting.".to_owned()),
        );
        return;
    }
    let Some(value) = snapshot.scoped(&setting.id, scope) else {
        return;
    };
    let draft = value
        .value
        .as_ref()
        .or_else(|| snapshot.value(&setting.id))
        .map_or_else(String::new, |value| {
            value.as_str().map_or_else(|| value.to_string(), str::to_owned)
        });
    let cursor = draft.chars().count();
    app.config.replace_overlay(ConfigOverlayState::Setting(SettingOverlayState {
        id: setting.id.clone(),
        scope,
        context: snapshot.context.clone(),
        revision: value.revision.clone(),
        draft,
        cursor,
    }));
}

pub(super) fn step_setting(app: &mut App, delta: isize) {
    if app.config.pending_settings_request.is_some() {
        return;
    }
    let Some(snapshot) = &app.config.snapshot else {
        return;
    };
    let Some(setting) = app.config.selected_setting() else {
        return;
    };
    let scope = app.config.selected_scope;
    if !setting.writable_at(scope) {
        return;
    }
    let Some(scoped) = snapshot.scoped(&setting.id, scope) else {
        return;
    };
    let current = scoped
        .value
        .as_ref()
        .or_else(|| snapshot.value(&setting.id))
        .map_or_else(String::new, |value| {
            value.as_str().map_or_else(|| value.to_string(), str::to_owned)
        });
    let Some(next) = next_choice(setting, &current, delta, true) else {
        return;
    };
    let mutation = SettingsMutation {
        context: snapshot.context.clone(),
        id: setting.id.clone(),
        scope,
        expected_revision: scoped.revision.clone(),
        operation: SettingsOperation::Set,
        value: Some(next),
    };
    super::service::send_mutation(app, mutation);
}

fn next_choice(
    setting: &SettingDescriptor,
    current: &str,
    delta: isize,
    wrap: bool,
) -> Option<serde_json::Value> {
    if setting.options.is_empty() {
        return None;
    }
    let next = setting
        .options
        .iter()
        .position(|value| {
            value.as_str().map_or_else(|| value.to_string(), str::to_owned) == current
        })
        .map_or(0, |index| {
            if wrap {
                if delta.is_negative() {
                    if index == 0 { setting.options.len() - 1 } else { index - 1 }
                } else {
                    (index + 1) % setting.options.len()
                }
            } else {
                step_index_clamped(index, delta, setting.options.len())
            }
        });
    setting.options.get(next).cloned()
}

pub(super) fn handle_overlay_key(app: &mut App, key: KeyEvent) {
    if super::mcp_edit::handle_overlay_key(app, key) {
        return;
    }
    match app.config.overlay.clone() {
        Some(ConfigOverlayState::Setting(_)) => handle_setting_key(app, key),
        Some(ConfigOverlayState::SessionRename(_)) => handle_session_rename_overlay_key(app, key),
        Some(ConfigOverlayState::InstalledPluginActions(_)) => {
            crate::app::plugins::handle_installed_overlay_key(app, key);
        }
        Some(ConfigOverlayState::PluginInstallActions(_)) => {
            crate::app::plugins::handle_plugin_install_overlay_key(app, key);
        }
        Some(ConfigOverlayState::MarketplaceActions(_)) => {
            crate::app::plugins::handle_marketplace_overlay_key(app, key);
        }
        Some(ConfigOverlayState::AddMarketplace(_)) => {
            crate::app::plugins::handle_add_marketplace_overlay_key(app, key);
        }
        Some(ConfigOverlayState::Confirmation(_)) => handle_confirmation_overlay_key(app, key),
        _ => {}
    }
}
pub(super) fn handle_overlay_paste(app: &mut App, text: &str) -> bool {
    if super::mcp_edit::handle_overlay_paste(app, text) {
        return true;
    }
    match app.config.overlay {
        Some(ConfigOverlayState::Setting(_)) => {
            if app.config.pending_settings_request.is_none() && setting_accepts_text(app) {
                insert_text_str(app.config.setting_overlay_mut(), text);
            }
            true
        }
        Some(ConfigOverlayState::SessionRename(_)) => {
            insert_text_str(app.config.session_rename_overlay_mut(), text);
            true
        }
        Some(ConfigOverlayState::AddMarketplace(_)) => {
            insert_text_str(app.config.add_marketplace_overlay_mut(), text);
            true
        }
        _ => false,
    }
}
fn handle_setting_key(app: &mut App, key: KeyEvent) {
    if app.config.pending_settings_request.is_some() {
        return;
    }
    if !setting_accepts_text(app)
        && matches!(
            key.code,
            KeyCode::Char(_)
                | KeyCode::Backspace
                | KeyCode::Delete
                | KeyCode::Left
                | KeyCode::Right
        )
        && key.modifiers != KeyModifiers::CONTROL
    {
        return;
    }
    match (key.code, key.modifiers) {
        (KeyCode::Enter, KeyModifiers::NONE) => confirm_setting(app, false),
        (KeyCode::Char('r'), KeyModifiers::CONTROL) => confirm_setting(app, true),
        (KeyCode::Esc, KeyModifiers::NONE) => {
            if app.config.pending_settings_request.is_none() {
                app.config.clear_overlay();
            }
        }
        (KeyCode::Left, KeyModifiers::NONE) => {
            move_text_cursor_left(app.config.setting_overlay_mut());
        }
        (KeyCode::Right, KeyModifiers::NONE) => {
            move_text_cursor_right(app.config.setting_overlay_mut());
        }
        (KeyCode::Home, KeyModifiers::NONE) => set_text_cursor(app.config.setting_overlay_mut(), 0),
        (KeyCode::End, KeyModifiers::NONE) => {
            move_text_cursor_to_end(app.config.setting_overlay_mut());
        }
        (KeyCode::Backspace, KeyModifiers::NONE) => {
            delete_text_before_cursor(app.config.setting_overlay_mut());
        }
        (KeyCode::Delete, KeyModifiers::NONE) => {
            delete_text_at_cursor(app.config.setting_overlay_mut());
        }
        (KeyCode::Up | KeyCode::Down, KeyModifiers::NONE) => {
            let Some(overlay) = app.config.setting_overlay().cloned() else {
                return;
            };
            let Some(setting) = app.config.snapshot.as_ref().and_then(|snapshot| {
                snapshot.catalog.iter().find(|setting| setting.id == overlay.id)
            }) else {
                return;
            };
            let Some(next) = next_choice(
                setting,
                &overlay.draft,
                if key.code == KeyCode::Up { -1 } else { 1 },
                false,
            ) else {
                return;
            };
            if let Some(overlay) = app.config.setting_overlay_mut() {
                overlay.draft = next.as_str().map_or_else(|| next.to_string(), str::to_owned);
                overlay.cursor = overlay.draft.chars().count();
            }
        }
        (KeyCode::Char(ch), modifiers) if accepts_text_input(modifiers) => {
            insert_text_char(app.config.setting_overlay_mut(), ch);
        }
        _ => {}
    }
}
fn setting_accepts_text(app: &App) -> bool {
    app.config.setting_overlay().is_some_and(|overlay| {
        app.config.snapshot.as_ref().is_some_and(|snapshot| {
            snapshot.catalog.iter().any(|setting| setting.id == overlay.id && setting.allows_custom)
        })
    })
}
fn confirm_setting(app: &mut App, remove: bool) {
    let Some(overlay) = app.config.setting_overlay().cloned() else {
        return;
    };
    let Some(setting) = app
        .config
        .snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.catalog.iter().find(|setting| setting.id == overlay.id))
    else {
        return;
    };
    let value = if remove {
        None
    } else if setting.kind == SettingKind::String {
        Some(serde_json::Value::String(overlay.draft))
    } else if let Ok(value) = serde_json::from_str(&overlay.draft) {
        Some(value)
    } else {
        app.config.set_overlay_error("Enter true or false, or select with Up/Down.");
        return;
    };
    super::service::send_mutation(
        app,
        SettingsMutation {
            context: overlay.context,
            id: overlay.id,
            scope: overlay.scope,
            expected_revision: overlay.revision,
            operation: if remove { SettingsOperation::Remove } else { SettingsOperation::Set },
            value,
        },
    );
}

fn handle_confirmation_overlay_key(app: &mut App, key: KeyEvent) {
    match (key.code, key.modifiers) {
        (KeyCode::Esc, KeyModifiers::NONE) => restore_confirmation_previous_overlay(app),
        (KeyCode::Enter, KeyModifiers::NONE) => confirm_confirmation_overlay(app),
        (KeyCode::Up | KeyCode::Down, KeyModifiers::NONE) => toggle_confirmation_selection(app),
        _ => {}
    }
}

fn toggle_confirmation_selection(app: &mut App) {
    let Some(overlay) = app.config.confirmation_overlay_mut() else {
        return;
    };
    overlay.selected_index = usize::from(overlay.selected_index == 0);
}

fn restore_confirmation_previous_overlay(app: &mut App) {
    let Some(overlay) = app.config.confirmation_overlay().cloned() else {
        return;
    };
    restore_previous_overlay(app, overlay.previous);
    if overlay.action == ConfirmationAction::ForceRuntimePluginReload {
        crate::app::plugins::cancel_held_runtime_reload(app);
    }
}

fn restore_previous_overlay(app: &mut App, previous: Option<Box<ConfigOverlayState>>) {
    if let Some(previous) = previous {
        app.config.replace_overlay(*previous);
    } else {
        app.config.clear_overlay();
    }
}

fn confirm_confirmation_overlay(app: &mut App) {
    let Some(overlay) = app.config.confirmation_overlay().cloned() else {
        return;
    };
    if overlay.selected_index == 0 {
        restore_confirmation_previous_overlay(app);
        return;
    }

    let action = overlay.action;
    restore_previous_overlay(app, overlay.previous);
    match action {
        ConfirmationAction::InstalledPluginUninstall => {
            crate::app::plugins::execute_confirmed_installed_plugin_action(
                app,
                super::InstalledPluginActionKind::Uninstall,
            );
        }
        ConfirmationAction::MarketplaceRemove => {
            crate::app::plugins::execute_confirmed_marketplace_action(
                app,
                super::MarketplaceActionKind::Remove,
            );
        }
        ConfirmationAction::McpClearAuth => {
            super::mcp_edit::execute_confirmed_mcp_server_action(
                app,
                super::mcp::McpServerActionKind::ClearAuth,
            );
        }
        ConfirmationAction::McpRemoveConfig => {
            let Some(overlay) = app.config.mcp_details_overlay().cloned() else {
                return;
            };
            let Some(server) =
                app.mcp.servers.iter().find(|server| server.name == overlay.server_name)
            else {
                app.config.clear_overlay();
                return;
            };
            let Some(scope) = super::mcp::mcp_config_removal_scope(app, server) else {
                app.config.set_overlay_error("This MCP server cannot be removed from live config.");
                return;
            };
            let action = match scope {
                super::mcp::McpConfigScope::User => {
                    super::mcp::McpServerActionKind::RemoveUserConfig
                }
                super::mcp::McpConfigScope::Local => {
                    super::mcp::McpServerActionKind::RemoveLocalConfig
                }
                super::mcp::McpConfigScope::Project => {
                    super::mcp::McpServerActionKind::RemoveProjectConfig
                }
                super::mcp::McpConfigScope::Dynamic => {
                    super::mcp::McpServerActionKind::RemoveDynamicConfig
                }
            };
            super::mcp_edit::execute_confirmed_mcp_server_action(app, action);
        }
        ConfirmationAction::ForceRuntimePluginReload => {
            crate::app::plugins::force_held_runtime_reload(app);
        }
    }
}

pub(super) fn open_session_rename_overlay(app: &mut App) {
    let Some(session_id) = app.session_runtime.session_id.as_ref() else {
        return;
    };
    let session_id = session_id.to_string();
    let draft = app
        .recent_sessions
        .iter()
        .find(|session| session.session_id == session_id)
        .and_then(|session| session.custom_title.clone())
        .unwrap_or_default();
    app.config.replace_overlay(ConfigOverlayState::SessionRename(text_input_overlay_state(
        draft,
        SessionRenameOverlayState::from_text_input,
    )));
    app.config.last_error = None;
}

pub(super) fn generate_session_title(app: &mut App) {
    let Some(session_id) =
        app.session_runtime.session_id.as_ref().map(std::string::ToString::to_string)
    else {
        return;
    };
    let Some(conn) = app.session_runtime.conn.clone() else {
        app.config.last_error = Some("No active bridge connection".to_owned());
        app.config.status_message = None;
        return;
    };
    let Some(description) = session_title_generation_description(app, &session_id) else {
        app.config.last_error =
            Some("No session summary is available to generate a title".to_owned());
        app.config.status_message = None;
        return;
    };

    match conn.generate_session_title(session_id.clone(), description) {
        Ok(()) => {
            app.config.pending_session_title_change = Some(PendingSessionTitleChangeState {
                session_id,
                kind: PendingSessionTitleChangeKind::Generate,
            });
            app.config.last_error = None;
            app.config.status_message = Some("Generating session title...".to_owned());
        }
        Err(err) => {
            app.config.last_error = Some(format!("Failed to generate session title: {err}"));
            app.config.status_message = None;
        }
    }
}

fn handle_session_rename_overlay_key(app: &mut App, key: KeyEvent) {
    match (key.code, key.modifiers) {
        (KeyCode::Enter, KeyModifiers::NONE) => confirm_session_rename_overlay(app),
        (KeyCode::Esc, KeyModifiers::NONE) => app.config.clear_overlay(),
        (KeyCode::Left, KeyModifiers::NONE) => {
            move_text_cursor_left(app.config.session_rename_overlay_mut());
        }
        (KeyCode::Right, KeyModifiers::NONE) => {
            move_text_cursor_right(app.config.session_rename_overlay_mut());
        }
        (KeyCode::Home, KeyModifiers::NONE) => {
            set_text_cursor(app.config.session_rename_overlay_mut(), 0);
        }
        (KeyCode::End, KeyModifiers::NONE) => {
            move_text_cursor_to_end(app.config.session_rename_overlay_mut());
        }
        (KeyCode::Backspace, KeyModifiers::NONE) => {
            delete_text_before_cursor(app.config.session_rename_overlay_mut());
        }
        (KeyCode::Delete, KeyModifiers::NONE) => {
            delete_text_at_cursor(app.config.session_rename_overlay_mut());
        }
        (KeyCode::Char(ch), modifiers) if accepts_text_input(modifiers) => {
            insert_text_char(app.config.session_rename_overlay_mut(), ch);
        }
        _ => {}
    }
}

fn confirm_session_rename_overlay(app: &mut App) {
    let Some(session_id) =
        app.session_runtime.session_id.as_ref().map(std::string::ToString::to_string)
    else {
        app.config.clear_overlay();
        return;
    };
    let Some(conn) = app.session_runtime.conn.clone() else {
        app.config.set_overlay_error("No active bridge connection");
        return;
    };
    let Some(overlay) = app.config.session_rename_overlay().cloned() else {
        return;
    };

    let trimmed = overlay.draft.trim().to_owned();
    let requested_title = (!trimmed.is_empty()).then_some(trimmed.clone());
    match conn.rename_session(session_id.clone(), trimmed) {
        Ok(()) => {
            app.config.pending_session_title_change = Some(PendingSessionTitleChangeState {
                session_id,
                kind: PendingSessionTitleChangeKind::Rename {
                    requested_title: requested_title.clone(),
                },
            });
            app.config.clear_overlay();
            app.config.last_error = None;
            app.config.status_message = Some(if requested_title.is_some() {
                "Renaming session...".to_owned()
            } else {
                "Clearing session name...".to_owned()
            });
        }
        Err(err) => {
            app.config.set_overlay_error(format!("Failed to rename session: {err}"));
        }
    }
}

pub(super) fn step_index_clamped(current: usize, delta: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    if delta.is_negative() {
        current.saturating_sub(delta.unsigned_abs()).min(len.saturating_sub(1))
    } else {
        (current + delta.cast_unsigned()).min(len.saturating_sub(1))
    }
}

fn char_to_byte_index(text: &str, char_index: usize) -> usize {
    text.char_indices().nth(char_index).map_or(text.len(), |(idx, _)| idx)
}

fn session_title_generation_description(app: &App, session_id: &str) -> Option<String> {
    let session = app.recent_sessions.iter().find(|session| session.session_id == session_id)?;
    [
        session.custom_title.as_deref(),
        Some(session.summary.as_str()),
        session.first_prompt.as_deref(),
    ]
    .into_iter()
    .flatten()
    .map(str::trim)
    .find(|value| !value.is_empty())
    .map(str::to_owned)
}

pub(super) fn text_input_overlay_state<T>(
    draft: String,
    build: impl FnOnce(String, usize) -> T,
) -> T {
    let cursor = draft.chars().count();
    build(draft, cursor)
}

pub(super) fn move_text_cursor_left<T: TextInputOverlay>(overlay: Option<&mut T>) {
    let Some(overlay) = overlay else {
        return;
    };
    *overlay.cursor_mut() = overlay.cursor().saturating_sub(1);
}

pub(super) fn move_text_cursor_right<T: TextInputOverlay>(overlay: Option<&mut T>) {
    let Some(overlay) = overlay else {
        return;
    };
    let next = overlay.cursor().saturating_add(1).min(overlay.draft().chars().count());
    *overlay.cursor_mut() = next;
}

pub(super) fn move_text_cursor_to_end<T: TextInputOverlay>(overlay: Option<&mut T>) {
    let Some(overlay) = overlay else {
        return;
    };
    *overlay.cursor_mut() = overlay.draft().chars().count();
}

pub(super) fn set_text_cursor<T: TextInputOverlay>(overlay: Option<&mut T>, cursor: usize) {
    let Some(overlay) = overlay else {
        return;
    };
    *overlay.cursor_mut() = cursor.min(overlay.draft().chars().count());
}

pub(super) fn insert_text_char<T: TextInputOverlay>(overlay: Option<&mut T>, ch: char) {
    let Some(overlay) = overlay else {
        return;
    };
    let byte_index = char_to_byte_index(overlay.draft(), overlay.cursor());
    overlay.draft_mut().insert(byte_index, ch);
    *overlay.cursor_mut() += 1;
}

pub(super) fn insert_text_str<T: TextInputOverlay>(overlay: Option<&mut T>, text: &str) {
    let Some(overlay) = overlay else {
        return;
    };
    let byte_index = char_to_byte_index(overlay.draft(), overlay.cursor());
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n").replace('\n', " ");
    overlay.draft_mut().insert_str(byte_index, &normalized);
    *overlay.cursor_mut() += normalized.chars().count();
}

pub(super) fn delete_text_before_cursor<T: TextInputOverlay>(overlay: Option<&mut T>) {
    let Some(overlay) = overlay else {
        return;
    };
    if overlay.cursor() == 0 {
        return;
    }
    let end = char_to_byte_index(overlay.draft(), overlay.cursor());
    let start = char_to_byte_index(overlay.draft(), overlay.cursor() - 1);
    overlay.draft_mut().replace_range(start..end, "");
    *overlay.cursor_mut() -= 1;
}

pub(super) fn delete_text_at_cursor<T: TextInputOverlay>(overlay: Option<&mut T>) {
    let Some(overlay) = overlay else {
        return;
    };
    let char_count = overlay.draft().chars().count();
    if overlay.cursor() >= char_count {
        return;
    }
    let start = char_to_byte_index(overlay.draft(), overlay.cursor());
    let end = char_to_byte_index(overlay.draft(), overlay.cursor() + 1);
    overlay.draft_mut().replace_range(start..end, "");
}

pub(super) trait TextInputOverlay {
    fn draft(&self) -> &str;
    fn draft_mut(&mut self) -> &mut String;
    fn cursor(&self) -> usize;
    fn cursor_mut(&mut self) -> &mut usize;
}

impl TextInputOverlay for SessionRenameOverlayState {
    fn draft(&self) -> &str {
        &self.draft
    }

    fn draft_mut(&mut self) -> &mut String {
        &mut self.draft
    }

    fn cursor(&self) -> usize {
        self.cursor
    }

    fn cursor_mut(&mut self) -> &mut usize {
        &mut self.cursor
    }
}

impl SessionRenameOverlayState {
    fn from_text_input(draft: String, cursor: usize) -> Self {
        Self { draft, cursor }
    }
}

impl TextInputOverlay for AddMarketplaceOverlayState {
    fn draft(&self) -> &str {
        &self.draft
    }

    fn draft_mut(&mut self) -> &mut String {
        &mut self.draft
    }

    fn cursor(&self) -> usize {
        self.cursor
    }

    fn cursor_mut(&mut self) -> &mut usize {
        &mut self.cursor
    }
}

impl AddMarketplaceOverlayState {
    pub(crate) fn from_text_input(draft: String, cursor: usize) -> Self {
        Self { draft, cursor }
    }
}

impl TextInputOverlay for SettingOverlayState {
    fn draft(&self) -> &str {
        &self.draft
    }
    fn draft_mut(&mut self) -> &mut String {
        &mut self.draft
    }
    fn cursor(&self) -> usize {
        self.cursor
    }
    fn cursor_mut(&mut self) -> &mut usize {
        &mut self.cursor
    }
}

pub(super) fn accepts_text_input(modifiers: KeyModifiers) -> bool {
    modifiers.is_empty() || modifiers == KeyModifiers::SHIFT
}
