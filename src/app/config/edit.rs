use super::resolve::{language_input_validation_message, normalized_language_value};
use super::{
    DEFAULT_EFFORT_LEVELS, DEFAULT_MODEL_ID, DEFAULT_MODEL_LABEL, DefaultPermissionMode,
    LanguageOverlayState, ModelAndEffortOverlayState, OutputStyle, OutputStyleOverlayState,
    OverlayFocus, PreferredNotifChannel, ResolvedChoice, ResolvedSettingValue, SettingFile,
    SettingId, SettingOptions, SettingSpec, SettingsOverlayState, resolved_setting,
    setting_display_value, setting_spec, store,
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
        SettingId::Model => open_model_and_effort_overlay(app, OverlayFocus::Model),
        SettingId::OutputStyle => open_output_style_overlay(app),
        SettingId::ThinkingEffort => {
            open_model_and_effort_overlay(app, OverlayFocus::Effort);
        }
        SettingId::Theme | SettingId::Notifications | SettingId::EditorMode => {
            cycle_static_enum(app, spec);
        }
    }
}

pub(super) fn handle_overlay_key(app: &mut App, key: KeyEvent) {
    match app.config.overlay.clone() {
        Some(SettingsOverlayState::ModelAndEffort(_)) => match (key.code, key.modifiers) {
            (KeyCode::Enter, KeyModifiers::NONE) => confirm_model_and_effort_overlay(app),
            (KeyCode::Esc, KeyModifiers::NONE) => app.config.overlay = None,
            (KeyCode::Tab | KeyCode::Right | KeyCode::Left, KeyModifiers::NONE)
            | (KeyCode::BackTab, _) => toggle_model_and_effort_focus(app),
            (KeyCode::Up, KeyModifiers::NONE) => move_overlay_selection(app, -1),
            (KeyCode::Down, KeyModifiers::NONE) => move_overlay_selection(app, 1),
            _ => {}
        },
        Some(SettingsOverlayState::OutputStyle(_)) => match (key.code, key.modifiers) {
            (KeyCode::Enter, KeyModifiers::NONE) => confirm_output_style_overlay(app),
            (KeyCode::Esc, KeyModifiers::NONE) => app.config.overlay = None,
            (KeyCode::Up, KeyModifiers::NONE) => move_output_style_overlay_selection(app, -1),
            (KeyCode::Down, KeyModifiers::NONE) => move_output_style_overlay_selection(app, 1),
            _ => {}
        },
        Some(SettingsOverlayState::Language(_)) => handle_language_overlay_key(app, key),
        None => {}
    }
}

pub(crate) fn model_supports_effort(app: &App, model_id: &str) -> bool {
    if model_id == DEFAULT_MODEL_ID {
        return true;
    }

    model_overlay_options(app)
        .into_iter()
        .find(|option| option.id == model_id)
        .is_none_or(|option| option.supports_effort)
}

pub(crate) fn supported_effort_levels_for_model(app: &App, model_id: &str) -> Vec<EffortLevel> {
    model_overlay_options(app).into_iter().find(|option| option.id == model_id).map_or_else(
        Vec::new,
        |option| {
            if option.supports_effort { option.supported_effort_levels } else { Vec::new() }
        },
    )
}

#[derive(Debug, Clone)]
pub(crate) struct OverlayModelOption {
    pub id: String,
    pub display_name: String,
    pub description: Option<String>,
    pub supports_effort: bool,
    pub supported_effort_levels: Vec<EffortLevel>,
    pub supports_adaptive_thinking: Option<bool>,
    pub supports_fast_mode: Option<bool>,
    pub supports_auto_mode: Option<bool>,
}

pub(crate) fn model_overlay_options(app: &App) -> Vec<OverlayModelOption> {
    let mut options = app
        .available_models
        .iter()
        .map(|model| OverlayModelOption {
            id: model.id.clone(),
            display_name: model.display_name.clone(),
            description: model.description.clone(),
            supports_effort: model.supports_effort,
            supported_effort_levels: if model.supported_effort_levels.is_empty()
                && model.supports_effort
            {
                DEFAULT_EFFORT_LEVELS.to_vec()
            } else {
                model.supported_effort_levels.clone()
            },
            supports_adaptive_thinking: model.supports_adaptive_thinking,
            supports_fast_mode: model.supports_fast_mode,
            supports_auto_mode: model.supports_auto_mode,
        })
        .collect::<Vec<_>>();
    if !options.iter().any(|option| option.id == DEFAULT_MODEL_ID) {
        options.push(OverlayModelOption {
            id: DEFAULT_MODEL_ID.to_owned(),
            display_name: DEFAULT_MODEL_LABEL.to_owned(),
            description: Some("Uses Claude's default model selection.".to_owned()),
            supports_effort: true,
            supported_effort_levels: DEFAULT_EFFORT_LEVELS.to_vec(),
            supports_adaptive_thinking: None,
            supports_fast_mode: None,
            supports_auto_mode: None,
        });
    }
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
        app.config.last_error = Some(message.clone());
        app.config.status_message = None;
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
                crate::app::mention::invalidate_session_cache(app);
            }
            app.config.last_error = None;
            app.config.status_message = Some(format!(
                "Saved {}: {}",
                spec.label,
                setting_display_value(app, spec, &resolved_setting(app, spec))
            ));
            true
        }
        Err(err) => {
            app.config.last_error = Some(err);
            app.config.status_message = None;
            false
        }
    }
}

fn cycle_static_enum(app: &mut App, spec: &SettingSpec) {
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
    let next = options[(current_index + 1) % options.len()].stored;

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

fn open_model_and_effort_overlay(app: &mut App, focus: OverlayFocus) {
    let options = model_overlay_options(app);
    let current_model = app
        .config
        .model_effective()
        .filter(|value| options.iter().any(|option| option.id == *value))
        .unwrap_or_else(|| DEFAULT_MODEL_ID.to_owned());
    let current_effort = app.config.thinking_effort_effective();
    let selected_effort = overlay_effort_for_model(app, &current_model, current_effort);
    app.config.overlay = Some(SettingsOverlayState::ModelAndEffort(ModelAndEffortOverlayState {
        focus,
        selected_model: current_model,
        selected_effort,
    }));
    app.config.last_error = None;
}

fn open_output_style_overlay(app: &mut App) {
    app.config.overlay = Some(SettingsOverlayState::OutputStyle(OutputStyleOverlayState {
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
    let cursor = draft.chars().count();
    app.config.overlay =
        Some(SettingsOverlayState::Language(LanguageOverlayState { draft, cursor }));
    app.config.last_error = None;
}

fn toggle_model_and_effort_focus(app: &mut App) {
    let Some(overlay) = app.config.model_and_effort_overlay_mut() else {
        return;
    };
    overlay.focus = match overlay.focus {
        OverlayFocus::Model => OverlayFocus::Effort,
        OverlayFocus::Effort => OverlayFocus::Model,
    };
}

fn move_overlay_selection(app: &mut App, delta: isize) {
    let Some(overlay) = app.config.model_and_effort_overlay().cloned() else {
        return;
    };
    match overlay.focus {
        OverlayFocus::Model => move_overlay_model_selection(app, &overlay, delta),
        OverlayFocus::Effort => move_overlay_effort_selection(app, &overlay, delta),
    }
}

fn move_overlay_model_selection(app: &mut App, overlay: &ModelAndEffortOverlayState, delta: isize) {
    let options = model_overlay_options(app);
    if options.is_empty() {
        return;
    }
    let current_index =
        options.iter().position(|option| option.id == overlay.selected_model).unwrap_or(0);
    let next_index = step_index_clamped(current_index, delta, options.len());
    let next_model = &options[next_index];
    let next_effort = overlay_effort_for_model(app, &next_model.id, overlay.selected_effort);
    if let Some(state) = app.config.model_and_effort_overlay_mut() {
        state.selected_model.clone_from(&next_model.id);
        state.selected_effort = next_effort;
    }
}

fn move_overlay_effort_selection(
    app: &mut App,
    overlay: &ModelAndEffortOverlayState,
    delta: isize,
) {
    let levels = supported_effort_levels_for_model(app, &overlay.selected_model);
    if levels.is_empty() {
        return;
    }
    let current_index =
        levels.iter().position(|level| *level == overlay.selected_effort).unwrap_or(0);
    let next_index = step_index_clamped(current_index, delta, levels.len());
    if let Some(state) = app.config.model_and_effort_overlay_mut() {
        state.selected_effort = levels[next_index];
    }
}

fn confirm_model_and_effort_overlay(app: &mut App) {
    let Some(overlay) = app.config.model_and_effort_overlay().cloned() else {
        return;
    };
    if persist_model_and_effort_change(app, &overlay.selected_model, overlay.selected_effort) {
        app.config.overlay = None;
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
        app.config.overlay = None;
    }
}

fn handle_language_overlay_key(app: &mut App, key: KeyEvent) {
    match (key.code, key.modifiers) {
        (KeyCode::Enter, KeyModifiers::NONE) => confirm_language_overlay(app),
        (KeyCode::Esc, KeyModifiers::NONE) => app.config.overlay = None,
        (KeyCode::Left, KeyModifiers::NONE) => move_language_cursor_left(app),
        (KeyCode::Right, KeyModifiers::NONE) => move_language_cursor_right(app),
        (KeyCode::Home, KeyModifiers::NONE) => set_language_cursor(app, 0),
        (KeyCode::End, KeyModifiers::NONE) => move_language_cursor_to_end(app),
        (KeyCode::Backspace, KeyModifiers::NONE) => delete_language_before_cursor(app),
        (KeyCode::Delete, KeyModifiers::NONE) => delete_language_at_cursor(app),
        (KeyCode::Char(ch), modifiers) if accepts_text_input(modifiers) => {
            insert_language_char(app, ch);
        }
        _ => {}
    }
}

fn accepts_text_input(modifiers: KeyModifiers) -> bool {
    modifiers.is_empty() || modifiers == KeyModifiers::SHIFT
}

fn confirm_language_overlay(app: &mut App) {
    let Some(overlay) = app.config.language_overlay().cloned() else {
        return;
    };
    let normalized = normalized_language_value(&overlay.draft);
    if normalized.as_deref().is_some_and(|value| language_input_validation_message(value).is_some())
    {
        return;
    }

    let spec = setting_spec(SettingId::Language);
    if persist_setting_change(app, spec, |document| {
        store::set_language(document, normalized.as_deref());
    }) {
        app.config.overlay = None;
    }
}

fn move_language_cursor_left(app: &mut App) {
    let Some(overlay) = app.config.language_overlay_mut() else {
        return;
    };
    overlay.cursor = overlay.cursor.saturating_sub(1);
}

fn move_language_cursor_right(app: &mut App) {
    let Some(overlay) = app.config.language_overlay_mut() else {
        return;
    };
    overlay.cursor = (overlay.cursor + 1).min(overlay.draft.chars().count());
}

fn move_language_cursor_to_end(app: &mut App) {
    let Some(overlay) = app.config.language_overlay_mut() else {
        return;
    };
    overlay.cursor = overlay.draft.chars().count();
}

fn set_language_cursor(app: &mut App, cursor: usize) {
    let Some(overlay) = app.config.language_overlay_mut() else {
        return;
    };
    overlay.cursor = cursor.min(overlay.draft.chars().count());
}

fn insert_language_char(app: &mut App, ch: char) {
    let Some(overlay) = app.config.language_overlay_mut() else {
        return;
    };
    let byte_index = char_to_byte_index(&overlay.draft, overlay.cursor);
    overlay.draft.insert(byte_index, ch);
    overlay.cursor += 1;
}

fn delete_language_before_cursor(app: &mut App) {
    let Some(overlay) = app.config.language_overlay_mut() else {
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

fn delete_language_at_cursor(app: &mut App) {
    let Some(overlay) = app.config.language_overlay_mut() else {
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

fn persist_model_and_effort_change(app: &mut App, model: &str, effort: EffortLevel) -> bool {
    let Some(path) = app.config.path_for(SettingFile::Settings).cloned() else {
        app.config.last_error = Some("Settings paths are not available".to_owned());
        app.config.status_message = None;
        return false;
    };
    let mut next_document = app.config.committed_settings_document.clone();
    store::set_model(&mut next_document, Some(model));
    if model_supports_effort(app, model) {
        store::set_thinking_effort_level(&mut next_document, effort);
    }
    match store::save(&path, &next_document) {
        Ok(()) => {
            app.config.committed_settings_document = next_document;
            app.config.last_error = None;
            app.config.status_message = None;
            true
        }
        Err(err) => {
            app.config.last_error = Some(err);
            app.config.status_message = None;
            false
        }
    }
}

fn overlay_effort_for_model(app: &App, model_id: &str, current: EffortLevel) -> EffortLevel {
    let supported = supported_effort_levels_for_model(app, model_id);
    if supported.is_empty() || supported.contains(&current) {
        return current;
    }
    supported.iter().copied().find(|level| *level == EffortLevel::Medium).unwrap_or(supported[0])
}

fn step_index_clamped(current: usize, delta: isize, len: usize) -> usize {
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
