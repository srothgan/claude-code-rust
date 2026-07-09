// SPDX-License-Identifier: Apache-2.0
use super::resolve::{language_input_validation_message, normalized_language_value};
use super::{
    AddMarketplaceOverlayState, ConfigOverlayState, ConfirmationAction, DEFAULT_MODEL_ALIAS_ID,
    DefaultPermissionMode, LanguageOverlayState, ModelOverlayState, OutputStyle,
    OutputStyleOverlayState, PendingSessionTitleChangeKind, PendingSessionTitleChangeState,
    PreferredNotifChannel, ResolvedChoice, ResolvedSettingValue, SessionRenameOverlayState,
    SettingFile, SettingId, SettingOptions, SettingSpec, ThinkingEffortOverlayState,
    resolved_setting, setting_display_value, setting_spec, store,
};
use crate::agent::model::EffortLevel;
use crate::app::App;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::Value;

pub(super) fn activate_setting(app: &mut App, spec: &SettingSpec) {
    match spec.id {
        SettingId::AlwaysThinking => {
            let next = !store::always_thinking_enabled(&app.config.committed_settings_document)
                .unwrap_or(false);
            persist_setting_change(app, spec, |document| {
                store::set_always_thinking_enabled(document, next);
            });
        }
        SettingId::ShowTips => {
            let next = !store::spinner_tips_enabled(&app.config.committed_local_settings_document)
                .unwrap_or(true);
            persist_setting_change(app, spec, |document| {
                store::set_spinner_tips_enabled(document, next);
            });
        }
        SettingId::TerminalProgressBar => {
            let next =
                !store::terminal_progress_bar_enabled(&app.config.committed_preferences_document)
                    .unwrap_or(true);
            persist_setting_change(app, spec, |document| {
                store::set_terminal_progress_bar_enabled(document, next);
            });
        }
        SettingId::ReduceMotion => {
            let next =
                !store::prefers_reduced_motion(&app.config.committed_local_settings_document)
                    .unwrap_or(false);
            persist_setting_change(app, spec, |document| {
                store::set_prefers_reduced_motion(document, next);
            });
        }
        SettingId::FastMode => {
            let next = !store::fast_mode(&app.config.committed_settings_document).unwrap_or(false);
            persist_setting_change(app, spec, |document| {
                store::set_fast_mode(document, next);
            });
        }
        SettingId::RespectGitignore => {
            let next = !store::respect_gitignore(&app.config.committed_preferences_document)
                .unwrap_or(true);
            persist_setting_change(app, spec, |document| {
                store::set_respect_gitignore(document, next);
            });
        }
        SettingId::DefaultPermissionMode => {
            let current = match super::resolve::resolve_setting_document(
                &app.config.committed_settings_document,
                SettingId::DefaultPermissionMode,
                &[],
            )
            .value
            {
                ResolvedSettingValue::Choice(ResolvedChoice::Stored(value)) => {
                    DefaultPermissionMode::from_stored(&value).unwrap_or_default()
                }
                ResolvedSettingValue::Bool(_)
                | ResolvedSettingValue::Choice(ResolvedChoice::Automatic)
                | ResolvedSettingValue::Text(_) => DefaultPermissionMode::Default,
            };
            let next = current.next();
            persist_setting_change(app, spec, |document| {
                store::set_default_permission_mode(document, next);
            });
        }
        SettingId::Language => open_language_overlay(app),
        SettingId::Model => open_model_overlay(app),
        SettingId::OutputStyle => open_output_style_overlay(app),
        SettingId::ThinkingEffort => open_thinking_effort_overlay(app),
        SettingId::Theme | SettingId::Notifications | SettingId::EditorMode => {
            cycle_static_enum(app, spec, 1);
        }
    }
}

pub(super) fn step_setting(app: &mut App, spec: &SettingSpec, delta: isize) {
    match spec.id {
        SettingId::AlwaysThinking
        | SettingId::ShowTips
        | SettingId::TerminalProgressBar
        | SettingId::ReduceMotion
        | SettingId::FastMode
        | SettingId::RespectGitignore => activate_setting(app, spec),
        SettingId::DefaultPermissionMode => {
            let current = match super::resolve::resolve_setting_document(
                &app.config.committed_settings_document,
                SettingId::DefaultPermissionMode,
                &[],
            )
            .value
            {
                ResolvedSettingValue::Choice(ResolvedChoice::Stored(value)) => {
                    DefaultPermissionMode::from_stored(&value).unwrap_or_default()
                }
                ResolvedSettingValue::Bool(_)
                | ResolvedSettingValue::Choice(ResolvedChoice::Automatic)
                | ResolvedSettingValue::Text(_) => DefaultPermissionMode::Default,
            };
            let next = if delta.is_negative() { current.prev() } else { current.next() };
            persist_setting_change(app, spec, |document| {
                store::set_default_permission_mode(document, next);
            });
        }
        SettingId::Theme | SettingId::Notifications | SettingId::EditorMode => {
            cycle_static_enum(app, spec, delta);
        }
        SettingId::Language
        | SettingId::Model
        | SettingId::OutputStyle
        | SettingId::ThinkingEffort => {
            activate_setting(app, spec);
        }
    }
}

pub(super) fn handle_overlay_key(app: &mut App, key: KeyEvent) {
    if super::mcp_edit::handle_overlay_key(app, key) {
        return;
    }
    match app.config.overlay.clone() {
        Some(ConfigOverlayState::Model(_)) => match (key.code, key.modifiers) {
            (KeyCode::Enter, KeyModifiers::NONE) => confirm_model_overlay(app),
            (KeyCode::Esc, KeyModifiers::NONE) => app.config.clear_overlay(),
            (KeyCode::Up, KeyModifiers::NONE) => move_model_overlay_selection(app, -1),
            (KeyCode::Down, KeyModifiers::NONE) => move_model_overlay_selection(app, 1),
            _ => {}
        },
        Some(ConfigOverlayState::ThinkingEffort(_)) => match (key.code, key.modifiers) {
            (KeyCode::Enter, KeyModifiers::NONE) => confirm_thinking_effort_overlay(app),
            (KeyCode::Esc, KeyModifiers::NONE) => app.config.clear_overlay(),
            (KeyCode::Up, KeyModifiers::NONE) => move_effort_overlay_selection(app, -1),
            (KeyCode::Down, KeyModifiers::NONE) => move_effort_overlay_selection(app, 1),
            _ => {}
        },
        Some(ConfigOverlayState::OutputStyle(_)) => match (key.code, key.modifiers) {
            (KeyCode::Enter, KeyModifiers::NONE) => confirm_output_style_overlay(app),
            (KeyCode::Esc, KeyModifiers::NONE) => app.config.clear_overlay(),
            (KeyCode::Up, KeyModifiers::NONE) => move_output_style_overlay_selection(app, -1),
            (KeyCode::Down, KeyModifiers::NONE) => move_output_style_overlay_selection(app, 1),
            _ => {}
        },
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
        Some(
            ConfigOverlayState::McpDetails(_)
            | ConfigOverlayState::McpCallbackUrl(_)
            | ConfigOverlayState::McpAuthRedirect(_)
            | ConfigOverlayState::McpElicitation(_),
        )
        | None => {}
        Some(ConfigOverlayState::Language(_)) => handle_language_overlay_key(app, key),
        Some(ConfigOverlayState::SessionRename(_)) => handle_session_rename_overlay_key(app, key),
    }
}

pub(super) fn handle_overlay_paste(app: &mut App, text: &str) -> bool {
    if super::mcp_edit::handle_overlay_paste(app, text) {
        return true;
    }
    match app.config.overlay {
        Some(ConfigOverlayState::Language(_)) => {
            insert_text_str(app.config.language_overlay_mut(), text);
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
        Some(
            ConfigOverlayState::Model(_)
            | ConfigOverlayState::ThinkingEffort(_)
            | ConfigOverlayState::OutputStyle(_)
            | ConfigOverlayState::InstalledPluginActions(_)
            | ConfigOverlayState::PluginInstallActions(_)
            | ConfigOverlayState::MarketplaceActions(_)
            | ConfigOverlayState::McpDetails(_)
            | ConfigOverlayState::McpCallbackUrl(_)
            | ConfigOverlayState::McpAuthRedirect(_)
            | ConfigOverlayState::McpElicitation(_)
            | ConfigOverlayState::Confirmation(_),
        )
        | None => false,
    }
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
    app.config.replace_overlay(*overlay.previous);
}

fn confirm_confirmation_overlay(app: &mut App) {
    let Some(overlay) = app.config.confirmation_overlay().cloned() else {
        return;
    };
    if overlay.selected_index == 0 {
        app.config.replace_overlay(*overlay.previous);
        return;
    }

    let action = overlay.action;
    app.config.replace_overlay(*overlay.previous);
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
    }
}

pub(crate) fn supported_effort_levels_for_model(app: &App, model_id: &str) -> Vec<EffortLevel> {
    model_overlay_options(app)
        .into_iter()
        .find(|option| option.matches_model_id(model_id))
        .map_or_else(Vec::new, |option| {
            if option.supports_effort {
                option
                    .supported_effort_levels
                    .into_iter()
                    .filter(|level| level.is_persistable_setting())
                    .collect()
            } else {
                Vec::new()
            }
        })
}

#[derive(Debug, Clone)]
pub(crate) struct OverlayModelOption {
    pub id: String,
    pub resolved_model: Option<String>,
    pub display_name: String,
    pub description: Option<String>,
    pub supports_effort: bool,
    pub supported_effort_levels: Vec<EffortLevel>,
    pub supports_adaptive_thinking: Option<bool>,
    pub supports_fast_mode: Option<bool>,
    pub supports_auto_mode: Option<bool>,
}

impl OverlayModelOption {
    #[must_use]
    pub fn matches_model_id(&self, model_id: &str) -> bool {
        self.id == model_id || self.resolved_model.as_deref() == Some(model_id)
    }
}

pub(crate) fn model_overlay_options(app: &App) -> Vec<OverlayModelOption> {
    let mut options = app
        .sdk_inventory
        .available_models
        .iter()
        .map(|model| OverlayModelOption {
            id: model.id.clone(),
            resolved_model: model.resolved_model.clone(),
            display_name: model.display_name.clone(),
            description: model.description.clone(),
            supports_effort: model.supports_effort,
            supported_effort_levels: if model.supported_effort_levels.is_empty()
                && model.supports_effort
            {
                EffortLevel::PERSISTABLE_SETTINGS.to_vec()
            } else {
                model.supported_effort_levels.clone()
            },
            supports_adaptive_thinking: model.supports_adaptive_thinking,
            supports_fast_mode: model.supports_fast_mode,
            supports_auto_mode: model.supports_auto_mode,
        })
        .collect::<Vec<_>>();
    options.sort_by(|left, right| {
        let left_key = left.display_name.to_ascii_lowercase();
        let right_key = right.display_name.to_ascii_lowercase();
        left_key.cmp(&right_key).then_with(|| left.id.cmp(&right.id))
    });
    options
}

fn persist_setting_change<F>(app: &mut App, spec: &SettingSpec, edit: F) -> bool
where
    F: FnOnce(&mut Value),
{
    let Some(path) = app.config.path_for(spec.file).cloned() else {
        let message = "Settings paths are not available".to_owned();
        if app.config.overlay.is_some() {
            app.config.set_overlay_error(message);
        } else {
            app.config.last_error = Some(message.clone());
            app.config.status_message = None;
        }
        return false;
    };

    let previous_respect_gitignore = matches!(spec.id, SettingId::RespectGitignore)
        .then(|| app.config.respect_gitignore_effective());
    let mut next_document = app.config.document_for(spec.file).clone();
    edit(&mut next_document);

    match store::save(&path, &next_document) {
        Ok(()) => {
            *app.config.committed_document_for_mut(spec.file) = next_document;
            if previous_respect_gitignore
                .is_some_and(|previous| previous != app.config.respect_gitignore_effective())
            {
                if app.mention.is_some() {
                    crate::app::file_index::restart(app);
                    crate::app::mention::refresh_from_file_index(app);
                } else {
                    crate::app::file_index::reset(app);
                }
            }
            app.reconcile_runtime_from_persisted_settings_change();
            app.config.last_error = None;
            app.config.status_message = Some(format!(
                "Saved {}: {}",
                spec.label,
                setting_display_value(app, spec, &resolved_setting(app, spec))
            ));
            true
        }
        Err(err) => {
            if app.config.overlay.is_some() {
                app.config.set_overlay_error(err);
            } else {
                app.config.last_error = Some(err);
                app.config.status_message = None;
            }
            false
        }
    }
}

fn cycle_static_enum(app: &mut App, spec: &SettingSpec, delta: isize) {
    let current = {
        let document = app.config.document_for(spec.file);
        match store::read_persisted_setting(document, spec) {
            Ok(store::PersistedSettingValue::String(value)) => value,
            _ => default_static_value(spec.id).to_owned(),
        }
    };

    let SettingOptions::Static(options) = spec.options else {
        return;
    };
    let current_index =
        options.iter().position(|option| option.stored == current).unwrap_or_default();
    let next = options[step_index_wrapped(current_index, delta, options.len())].stored;

    persist_setting_change(app, spec, |document| {
        if spec.id == SettingId::Notifications {
            if let Some(channel) = PreferredNotifChannel::from_stored(next) {
                store::set_preferred_notification_channel(document, channel);
            }
        } else {
            store::write_persisted_setting(
                document,
                spec,
                store::PersistedSettingValue::String(next.to_owned()),
            );
        }
    });
}

const fn default_static_value(setting_id: SettingId) -> &'static str {
    match setting_id {
        SettingId::Theme => "dark",
        SettingId::OutputStyle => OutputStyle::Default.as_stored(),
        SettingId::ThinkingEffort => "medium",
        SettingId::Notifications => "iterm2",
        SettingId::EditorMode => "default",
        SettingId::AlwaysThinking
        | SettingId::ReduceMotion
        | SettingId::ShowTips
        | SettingId::TerminalProgressBar
        | SettingId::FastMode
        | SettingId::DefaultPermissionMode
        | SettingId::Language
        | SettingId::RespectGitignore
        | SettingId::Model => "",
    }
}

fn open_model_overlay(app: &mut App) {
    let options = model_overlay_options(app);
    let current_model = app
        .config
        .model_effective()
        .and_then(|value| {
            options
                .iter()
                .find(|option| option.matches_model_id(&value))
                .map(|option| option.id.clone())
        })
        .unwrap_or_else(|| DEFAULT_MODEL_ALIAS_ID.to_owned());
    app.config.replace_overlay(ConfigOverlayState::Model(ModelOverlayState {
        selected_model: current_model,
    }));
    app.config.last_error = None;
}

fn open_thinking_effort_overlay(app: &mut App) {
    let current_model = selected_model_for_effort(app);
    let current_effort = app.config.thinking_effort_effective();
    let selected_effort = overlay_effort_for_model(app, &current_model, current_effort);
    app.config.replace_overlay(ConfigOverlayState::ThinkingEffort(ThinkingEffortOverlayState {
        selected_effort,
    }));
    app.config.last_error = None;
}

fn open_output_style_overlay(app: &mut App) {
    app.config.replace_overlay(ConfigOverlayState::OutputStyle(OutputStyleOverlayState {
        selected: app.config.output_style_effective(),
    }));
    app.config.last_error = None;
}

fn open_language_overlay(app: &mut App) {
    let draft = store::language(&app.config.committed_settings_document)
        .ok()
        .flatten()
        .and_then(|value| normalized_language_value(&value))
        .unwrap_or_default();
    app.config.replace_overlay(ConfigOverlayState::Language(text_input_overlay_state(
        draft,
        LanguageOverlayState::from_text_input,
    )));
    app.config.last_error = None;
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

fn move_model_overlay_selection(app: &mut App, delta: isize) {
    let Some(overlay) = app.config.model_overlay().cloned() else {
        return;
    };
    let options = model_overlay_options(app);
    if options.is_empty() {
        return;
    }
    let current_index =
        options.iter().position(|option| option.id == overlay.selected_model).unwrap_or(0);
    let next_index = step_index_clamped(current_index, delta, options.len());
    let next_model = &options[next_index];
    if let Some(state) = app.config.model_overlay_mut() {
        state.selected_model.clone_from(&next_model.id);
    }
}

fn move_effort_overlay_selection(app: &mut App, delta: isize) {
    let Some(overlay) = app.config.thinking_effort_overlay().copied() else {
        return;
    };
    let current_model = selected_model_for_effort(app);
    let levels = supported_effort_levels_for_model(app, &current_model);
    if levels.is_empty() {
        return;
    }
    let current_index =
        levels.iter().position(|level| *level == overlay.selected_effort).unwrap_or(0);
    let next_index = step_index_clamped(current_index, delta, levels.len());
    if let Some(state) = app.config.thinking_effort_overlay_mut() {
        state.selected_effort = levels[next_index];
    }
}

fn confirm_model_overlay(app: &mut App) {
    let Some(overlay) = app.config.model_overlay().cloned() else {
        return;
    };
    if persist_model_change(app, &overlay.selected_model) {
        app.config.clear_overlay();
    }
}

fn confirm_thinking_effort_overlay(app: &mut App) {
    let Some(overlay) = app.config.thinking_effort_overlay().copied() else {
        return;
    };
    let current_model = selected_model_for_effort(app);
    if supported_effort_levels_for_model(app, &current_model).is_empty() {
        app.config.clear_overlay();
        return;
    }
    if persist_thinking_effort_change(app, overlay.selected_effort) {
        app.config.clear_overlay();
    }
}

fn move_output_style_overlay_selection(app: &mut App, delta: isize) {
    let Some(overlay) = app.config.output_style_overlay().copied() else {
        return;
    };
    let current_index =
        OutputStyle::ALL.iter().position(|style| *style == overlay.selected).unwrap_or_default();
    let next_index = step_index_clamped(current_index, delta, OutputStyle::ALL.len());
    if let Some(state) = app.config.output_style_overlay_mut() {
        state.selected = OutputStyle::ALL[next_index];
    }
}

fn confirm_output_style_overlay(app: &mut App) {
    let Some(overlay) = app.config.output_style_overlay().copied() else {
        return;
    };
    let spec = setting_spec(SettingId::OutputStyle);
    if persist_setting_change(app, spec, |document| {
        store::set_output_style(document, overlay.selected);
    }) {
        app.config.clear_overlay();
    }
}

fn handle_language_overlay_key(app: &mut App, key: KeyEvent) {
    match (key.code, key.modifiers) {
        (KeyCode::Enter, KeyModifiers::NONE) => confirm_language_overlay(app),
        (KeyCode::Esc, KeyModifiers::NONE) => app.config.clear_overlay(),
        (KeyCode::Left, KeyModifiers::NONE) => {
            move_text_cursor_left(app.config.language_overlay_mut());
        }
        (KeyCode::Right, KeyModifiers::NONE) => {
            move_text_cursor_right(app.config.language_overlay_mut());
        }
        (KeyCode::Home, KeyModifiers::NONE) => {
            set_text_cursor(app.config.language_overlay_mut(), 0);
        }
        (KeyCode::End, KeyModifiers::NONE) => {
            move_text_cursor_to_end(app.config.language_overlay_mut());
        }
        (KeyCode::Backspace, KeyModifiers::NONE) => {
            delete_text_before_cursor(app.config.language_overlay_mut());
        }
        (KeyCode::Delete, KeyModifiers::NONE) => {
            delete_text_at_cursor(app.config.language_overlay_mut());
        }
        (KeyCode::Char(ch), modifiers) if accepts_text_input(modifiers) => {
            insert_text_char(app.config.language_overlay_mut(), ch);
        }
        _ => {}
    }
}

pub(super) fn accepts_text_input(modifiers: KeyModifiers) -> bool {
    modifiers.is_empty() || modifiers == KeyModifiers::SHIFT
}

fn confirm_language_overlay(app: &mut App) {
    let Some(overlay) = app.config.language_overlay().cloned() else {
        return;
    };
    let normalized = normalized_language_value(&overlay.draft);
    if let Some(message) = normalized.as_deref().and_then(language_input_validation_message) {
        app.config.set_overlay_error(message);
        return;
    }

    let spec = setting_spec(SettingId::Language);
    if persist_setting_change(app, spec, |document| {
        store::set_language(document, normalized.as_deref());
    }) {
        app.config.clear_overlay();
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

fn persist_model_change(app: &mut App, model: &str) -> bool {
    let Some(path) = app.config.path_for(SettingFile::Settings).cloned() else {
        app.config.set_overlay_error("Settings paths are not available");
        return false;
    };
    let mut next_document = app.config.committed_settings_document.clone();
    store::set_model(&mut next_document, Some(model));
    match store::save(&path, &next_document) {
        Ok(()) => {
            app.config.committed_settings_document = next_document;
            app.reconcile_runtime_from_persisted_settings_change();
            app.config.last_error = None;
            app.config.status_message = None;
            true
        }
        Err(err) => {
            app.config.set_overlay_error(err);
            false
        }
    }
}

fn persist_thinking_effort_change(app: &mut App, effort: EffortLevel) -> bool {
    let Some(path) = app.config.path_for(SettingFile::Settings).cloned() else {
        app.config.set_overlay_error("Settings paths are not available");
        return false;
    };
    let mut next_document = app.config.committed_settings_document.clone();
    if store::set_thinking_effort_level(&mut next_document, effort).is_err() {
        app.config.set_overlay_error(format!(
            "{} cannot be saved as a default thinking effort. Use /effort {} for the active session.",
            effort.label(),
            effort.as_stored()
        ));
        return false;
    }
    match store::save(&path, &next_document) {
        Ok(()) => {
            app.config.committed_settings_document = next_document;
            app.reconcile_runtime_from_persisted_settings_change();
            app.config.last_error = None;
            app.config.status_message = Some(format!("Saved Thinking effort: {}", effort.label()));
            true
        }
        Err(err) => {
            app.config.set_overlay_error(err);
            false
        }
    }
}

fn selected_model_for_effort(app: &App) -> String {
    app.config.model_effective().unwrap_or_else(|| DEFAULT_MODEL_ALIAS_ID.to_owned())
}

fn overlay_effort_for_model(app: &App, model_id: &str, current: EffortLevel) -> EffortLevel {
    let supported = supported_effort_levels_for_model(app, model_id);
    if supported.is_empty() || supported.contains(&current) {
        return current;
    }
    supported.iter().copied().find(|level| *level == EffortLevel::Medium).unwrap_or(supported[0])
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

fn step_index_wrapped(current: usize, delta: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    if delta.is_negative() {
        (current + len - (delta.unsigned_abs() % len)) % len
    } else {
        (current + delta.cast_unsigned()) % len
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

impl TextInputOverlay for LanguageOverlayState {
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

impl LanguageOverlayState {
    fn from_text_input(draft: String, cursor: usize) -> Self {
        Self { draft, cursor }
    }
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
