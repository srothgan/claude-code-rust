// SPDX-License-Identifier: Apache-2.0

use super::ConfigOverlayState;
use crate::agent::settings::{SettingDescriptor, SettingsScope};
use crate::app::{
    App,
    footer::{FooterColor, FooterLayout, PendingSave},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FooterOverlayState {
    setting: Box<SettingDescriptor>,
    scope: SettingsScope,
    // None removes the preference; a draft never replaces the active saved value.
    draft: Option<FooterLayout>,
    save_queued: bool,
    close_when_saved: bool,
    pub(crate) selected: usize,
    pub(crate) color_selection: Option<usize>,
    // Geometry of the last rendered inline palette, used by arrow navigation.
    pub(crate) color_columns: usize,
}

impl FooterOverlayState {
    pub(crate) fn read_only(&self) -> bool {
        !self.setting.writable_at(self.scope)
    }
    pub(crate) fn layout(&self) -> FooterLayout {
        self.draft.clone().unwrap_or_default()
    }
    pub(crate) fn can_retry(&self) -> bool {
        self.save_queued
    }
    pub(crate) fn selected_color(&self) -> Option<FooterColor> {
        self.color_selection
            .and_then(|index| index.checked_sub(1))
            .and_then(|index| FooterColor::ALL.get(index))
            .copied()
    }
    fn queue_save(&mut self) {
        self.save_queued = true;
        self.close_when_saved = false;
    }
}

pub(super) fn open(app: &mut App, setting: &SettingDescriptor) {
    if app.config.footer.pending.is_none() {
        app.config.load_footer(app.global_settings_path.as_deref());
    }
    let error = app.config.footer.error.clone();
    let draft = app
        .config
        .footer
        .pending
        .as_ref()
        .map_or_else(|| app.config.footer.saved().cloned(), |pending| pending.layout.clone());
    app.config.replace_overlay(ConfigOverlayState::Footer(FooterOverlayState {
        setting: Box::new(setting.clone()),
        scope: app.config.selected_scope,
        draft,
        save_queued: false,
        close_when_saved: false,
        selected: 0,
        color_selection: None,
        color_columns: 10,
    }));
    if let Some(error) = error {
        app.config.set_overlay_error(format!(
            "{error}. Showing defaults; edit to replace it or Esc to leave it unchanged."
        ));
    }
}

pub(super) fn handle_key(app: &mut App, key: KeyEvent) {
    if matches!(&app.config.overlay, Some(ConfigOverlayState::Footer(overlay)) if overlay.read_only())
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
    if handle_color_key(app, key) {
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
            if app.config.footer.pending.is_none() {
                if overlay.save_queued
                    && let Some(error) = failed
                {
                    app.config.clear_overlay();
                    app.config.last_error = Some(format!("Footer changes were not saved: {error}"));
                    return;
                }
                if !overlay.save_queued {
                    app.config.clear_overlay();
                    return;
                }
            }
            overlay.close_when_saved = true;
            save_queued(app);
        }
        (KeyCode::Enter, KeyModifiers::NONE) => save_queued(app),
        (KeyCode::Char('r'), KeyModifiers::CONTROL) => reset(app),
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
                (KeyCode::Home, KeyModifiers::NONE) => overlay.selected = 0,
                (KeyCode::End, KeyModifiers::NONE) => overlay.selected = layout.fields().len() - 1,
                (KeyCode::Char(' '), KeyModifiers::NONE) => layout.toggle(overlay.selected),
                (KeyCode::Left | KeyCode::Right, KeyModifiers::NONE) => {
                    overlay.selected = layout.switch_side(overlay.selected);
                }
                (KeyCode::Up | KeyCode::Down, KeyModifiers::CONTROL) => {
                    overlay.selected =
                        layout.move_field(overlay.selected, key.code == KeyCode::Down);
                }
                (KeyCode::Char('c'), KeyModifiers::NONE)
                    if layout.fields()[overlay.selected].id.supports_color() =>
                {
                    overlay.color_selection = Some(
                        layout.fields()[overlay.selected]
                            .color
                            .and_then(|color| {
                                FooterColor::ALL.iter().position(|entry| *entry == color)
                            })
                            .map_or(0, |index| index + 1),
                    );
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

fn handle_color_key(app: &mut App, key: KeyEvent) -> bool {
    let Some(ConfigOverlayState::Footer(overlay)) = &mut app.config.overlay else {
        return false;
    };
    let Some(index) = &mut overlay.color_selection else {
        return false;
    };
    if !key.modifiers.is_empty() {
        return true;
    }
    match key.code {
        KeyCode::Left => *index = index.saturating_sub(1),
        KeyCode::Right => *index = (*index + 1).min(FooterColor::ALL.len()),
        KeyCode::Up => *index = index.saturating_sub(overlay.color_columns),
        KeyCode::Down => {
            *index = if *index == 0 {
                1
            } else {
                (*index + overlay.color_columns).min(FooterColor::ALL.len())
            }
        }
        KeyCode::Home => *index = 0,
        KeyCode::End => *index = FooterColor::ALL.len(),
        KeyCode::Esc | KeyCode::Char('c') => overlay.color_selection = None,
        KeyCode::Enter => {
            let color = overlay.selected_color();
            overlay.color_selection = None;
            let mut layout = overlay.layout();
            let before = layout.clone();
            layout.set_color(overlay.selected, color);
            if layout != before {
                overlay.draft = Some(layout);
                overlay.queue_save();
                save_queued(app);
            }
        }
        _ => {}
    }
    true
}

pub(super) fn reset(app: &mut App) {
    if let Some(ConfigOverlayState::Footer(overlay)) = &mut app.config.overlay {
        overlay.draft = None;
        overlay.selected = 0;
        overlay.queue_save();
        save_queued(app);
    } else if app.config.footer.pending.is_none() {
        start_save(app, None, app.config.footer.revision().to_owned());
    }
}

fn save_queued(app: &mut App) {
    if app.config.footer.pending.is_some() {
        return;
    }
    let Some(ConfigOverlayState::Footer(overlay)) = &app.config.overlay else {
        return;
    };
    if !overlay.save_queued {
        return;
    }
    let (draft, revision) = (overlay.draft.clone(), app.config.footer.revision().to_owned());
    start_save(app, draft, revision);
    if app.config.footer.pending.is_some()
        && let Some(ConfigOverlayState::Footer(overlay)) = &mut app.config.overlay
    {
        overlay.save_queued = false;
        app.config.overlay_message = None;
    }
}

fn start_save(app: &mut App, layout: Option<FooterLayout>, revision: String) {
    let Some(path) = app.global_settings_path.clone() else {
        app.config.set_overlay_error("App settings are unavailable. Your changes were not saved.");
        return;
    };
    let request_id = uuid::Uuid::new_v4().to_string();
    app.config.footer.pending =
        Some(PendingSave { request_id: request_id.clone(), layout: layout.clone() });
    let event_tx = app.event_tx.clone();
    tokio::task::spawn_local(async move {
        let (revision, error) = match tokio::task::spawn_blocking(move || {
            crate::app::footer::save(&path, &revision, layout.as_ref())
        })
        .await
        {
            Ok(outcome) => outcome,
            Err(error) => (None, Some(format!("Footer save could not finish: {error}"))),
        };
        let _ = event_tx
            .send(crate::agent::events::ClientEvent::FooterSaved { request_id, revision, error })
            .await;
    });
}

pub(crate) fn complete_save(
    app: &mut App,
    request_id: &str,
    revision: Option<String>,
    error: Option<String>,
) {
    if !app.config.footer.complete(request_id, revision, error.is_none()) {
        return;
    }
    app.config.project_footer(app.global_settings_path.as_deref());
    let Some(ConfigOverlayState::Footer(overlay)) = &mut app.config.overlay else {
        app.config.last_error = error;
        return;
    };
    if let Some(error) = error {
        overlay.queue_save();
        app.config.set_overlay_error(error);
    } else if overlay.save_queued {
        save_queued(app);
    } else if overlay.close_when_saved {
        app.config.clear_overlay();
    }
}

#[cfg(test)]
mod tests;
