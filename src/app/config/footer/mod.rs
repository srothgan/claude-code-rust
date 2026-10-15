// SPDX-License-Identifier: Apache-2.0

use super::{ConfigOverlayState, SettingOverlayState};
use crate::agent::settings::{SettingsMutation, SettingsOperation};
use crate::app::{App, footer::FooterLayout};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FooterOverlayState {
    pub editor: SettingOverlayState,
    // None requests the built-in layout by removing the saved preference.
    draft: Option<FooterLayout>,
    // Records edits not yet sent, including edits made during an in-flight save.
    save_queued: bool,
    close_when_saved: bool,
    pub(crate) selected: usize,
}

impl FooterOverlayState {
    pub(crate) fn layout(&self) -> FooterLayout {
        self.draft.clone().unwrap_or_default()
    }

    pub(crate) fn can_retry(&self) -> bool {
        self.save_queued
    }

    pub(super) fn queue_save(&mut self) {
        self.save_queued = true;
        self.close_when_saved = false;
    }
}

pub(super) fn open(app: &mut App, editor: SettingOverlayState) {
    let saved = app.config.saved_value(&editor.setting.id);
    let parsed = FooterLayout::from_saved(saved);
    let error = parsed.as_ref().err().map(|error| format!(
        "The saved footer is invalid: {error}. Showing defaults; edit to replace it or Esc to leave it unchanged."
    ));
    let draft = saved.map(|_| parsed.unwrap_or_default());
    app.config.replace_overlay(ConfigOverlayState::Footer(FooterOverlayState {
        editor,
        draft,
        save_queued: false,
        close_when_saved: false,
        selected: 0,
    }));
    if let Some(error) = error {
        app.config.set_overlay_error(error);
    }
}

pub(super) fn handle_key(app: &mut App, key: KeyEvent) {
    if app.config.setting_overlay().is_some_and(|editor| !editor.setting.writable_at(editor.scope))
        && !matches!(
            (key.code, key.modifiers),
            (
                KeyCode::Up | KeyCode::Down | KeyCode::Home | KeyCode::End | KeyCode::Esc,
                KeyModifiers::NONE
            )
        )
    {
        return;
    }
    match (key.code, key.modifiers) {
        (KeyCode::Esc, KeyModifiers::NONE) => {
            let failed = app
                .config
                .overlay_message
                .as_ref()
                .filter(|message| message.kind == super::OverlayMessageKind::Error)
                .map(|message| message.text.clone());
            let Some(ConfigOverlayState::Footer(overlay)) = &mut app.config.overlay else {
                return;
            };
            if overlay.save_queued
                && app.config.pending_settings_request.is_none()
                && let Some(error) = failed
            {
                app.config.clear_overlay();
                app.config.last_error = Some(format!("Footer changes were not saved: {error}"));
                return;
            }
            if !overlay.save_queued && app.config.pending_settings_request.is_none() {
                app.config.clear_overlay();
            } else {
                overlay.close_when_saved = true;
                save_queued(app);
            }
        }
        (KeyCode::Enter, KeyModifiers::NONE) => save_queued(app),
        (KeyCode::Char('r'), KeyModifiers::CONTROL) => {
            if let Some(ConfigOverlayState::Footer(overlay)) = &mut app.config.overlay {
                overlay.draft = None;
                overlay.selected = 0;
                overlay.queue_save();
            }
            save_queued(app);
        }
        _ => {
            let Some(ConfigOverlayState::Footer(overlay)) = &mut app.config.overlay else {
                return;
            };
            let mut layout = overlay.layout();
            let before = layout.clone();
            match (key.code, key.modifiers) {
                (KeyCode::Up, KeyModifiers::NONE) => {
                    overlay.selected = overlay.selected.saturating_sub(1);
                }
                (KeyCode::Down, KeyModifiers::NONE) => {
                    overlay.selected = (overlay.selected + 1).min(layout.fields().len() - 1);
                }
                (KeyCode::Char(' '), KeyModifiers::NONE) => {
                    layout.toggle(overlay.selected);
                }
                (KeyCode::Left | KeyCode::Right, KeyModifiers::NONE) => {
                    overlay.selected = layout.switch_side(overlay.selected);
                }
                (KeyCode::Up | KeyCode::Down, KeyModifiers::CONTROL) => {
                    overlay.selected =
                        layout.move_field(overlay.selected, key.code == KeyCode::Down);
                }
                (KeyCode::Home, KeyModifiers::NONE) => overlay.selected = 0,
                (KeyCode::End, KeyModifiers::NONE) => {
                    overlay.selected = layout.fields().len() - 1;
                }
                _ => {}
            }
            if layout != before {
                overlay.draft = Some(layout);
                overlay.queue_save();
                save_queued(app);
            }
        }
    }
}

fn save_queued(app: &mut App) {
    if app.config.pending_settings_request.is_some() {
        return;
    }
    let Some(ConfigOverlayState::Footer(overlay)) = &app.config.overlay else {
        return;
    };
    if !overlay.save_queued {
        return;
    }
    let value = match overlay.draft.as_ref().map(serde_json::to_value).transpose() {
        Ok(value) => value,
        Err(error) => {
            app.config.set_overlay_error(format!("Cannot save the footer: {error}"));
            return;
        }
    };
    let mutation = SettingsMutation {
        context: overlay.editor.context.clone(),
        id: overlay.editor.setting.id.clone(),
        scope: overlay.editor.scope,
        expected_revision: overlay.editor.revision.clone(),
        operation: if value.is_some() { SettingsOperation::Set } else { SettingsOperation::Remove },
        value,
    };
    super::service::send_mutation(app, mutation);
    if app.config.pending_settings_request.is_some()
        && let Some(ConfigOverlayState::Footer(overlay)) = &mut app.config.overlay
    {
        overlay.save_queued = false;
        app.config.overlay_message = None;
    }
}

pub(super) fn complete_save(app: &mut App, saved: bool) {
    let Some(ConfigOverlayState::Footer(overlay)) = &mut app.config.overlay else {
        return;
    };
    if !saved {
        overlay.queue_save();
        if app.config.overlay_message.is_none() {
            app.config.set_overlay_error("The footer could not be saved. Press Enter to retry.");
        }
    } else if overlay.save_queued {
        save_queued(app);
    } else if overlay.close_when_saved {
        app.config.clear_overlay();
    }
}

#[cfg(test)]
mod tests;
