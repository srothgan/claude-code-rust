// Claude Code Rust - A native Rust terminal interface for Claude Code
// Copyright (C) 2025  Simon Peter Rothgang
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as
// published by the Free Software Foundation, either version 3 of the
// License, or (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

pub mod store;

use super::view::{self, ActiveView};
use crate::app::App;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::Value;
use std::path::PathBuf;

const CONFIG_ITEM_COUNT: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    Config,
    Status,
    Usage,
    Mcp,
}

impl SettingsTab {
    pub const ALL: [Self; 4] = [Self::Config, Self::Status, Self::Usage, Self::Mcp];

    pub const fn title(self) -> &'static str {
        match self {
            Self::Config => "Settings",
            Self::Status => "Status",
            Self::Usage => "Usage",
            Self::Mcp => "MCP",
        }
    }

    const fn next(self) -> Self {
        match self {
            Self::Config => Self::Status,
            Self::Status => Self::Usage,
            Self::Usage => Self::Mcp,
            Self::Mcp => Self::Config,
        }
    }

    const fn prev(self) -> Self {
        match self {
            Self::Config => Self::Mcp,
            Self::Status => Self::Config,
            Self::Usage => Self::Status,
            Self::Mcp => Self::Usage,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DefaultPermissionMode {
    #[default]
    Default,
    AcceptEdits,
    Plan,
    DontAsk,
    BypassPermissions,
}

impl DefaultPermissionMode {
    pub const ALL: [Self; 5] =
        [Self::Default, Self::AcceptEdits, Self::Plan, Self::DontAsk, Self::BypassPermissions];

    #[must_use]
    pub const fn as_stored(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::AcceptEdits => "acceptEdits",
            Self::Plan => "plan",
            Self::DontAsk => "dontAsk",
            Self::BypassPermissions => "bypassPermissions",
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::AcceptEdits => "Accept Edits",
            Self::Plan => "Plan",
            Self::DontAsk => "Don't Ask",
            Self::BypassPermissions => "Bypass Permissions",
        }
    }

    #[must_use]
    pub fn from_stored(value: &str) -> Option<Self> {
        match value {
            "default" => Some(Self::Default),
            "acceptEdits" => Some(Self::AcceptEdits),
            "plan" => Some(Self::Plan),
            "dontAsk" => Some(Self::DontAsk),
            "bypassPermissions" => Some(Self::BypassPermissions),
            _ => None,
        }
    }

    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Default => Self::AcceptEdits,
            Self::AcceptEdits => Self::Plan,
            Self::Plan => Self::DontAsk,
            Self::DontAsk => Self::BypassPermissions,
            Self::BypassPermissions => Self::Default,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SettingsState {
    pub active_tab: SettingsTab,
    pub selected_config_index: usize,
    pub committed_document: Value,
    pub draft_document: Value,
    pub dirty: bool,
    pub path: Option<PathBuf>,
    pub status_message: Option<String>,
    pub last_error: Option<String>,
}

impl Default for SettingsState {
    fn default() -> Self {
        Self {
            active_tab: SettingsTab::Config,
            selected_config_index: 0,
            committed_document: Value::Object(serde_json::Map::new()),
            draft_document: Value::Object(serde_json::Map::new()),
            dirty: false,
            path: None,
            status_message: None,
            last_error: None,
        }
    }
}

impl SettingsState {
    #[must_use]
    pub fn fast_mode_effective(&self) -> bool {
        store::fast_mode(&self.draft_document).unwrap_or(false)
    }

    #[must_use]
    pub fn fast_mode_invalid(&self) -> bool {
        store::fast_mode(&self.draft_document).is_err()
    }

    #[must_use]
    pub fn committed_fast_mode(&self) -> bool {
        store::fast_mode(&self.committed_document).unwrap_or(false)
    }

    #[must_use]
    pub fn model_effective(&self) -> Option<String> {
        store::model(&self.draft_document).ok().flatten()
    }

    #[must_use]
    pub fn model_invalid(&self) -> bool {
        store::model(&self.draft_document).is_err()
    }

    #[must_use]
    pub fn committed_model(&self) -> Option<String> {
        store::model(&self.committed_document).ok().flatten()
    }

    #[must_use]
    pub fn default_permission_mode_effective(&self) -> DefaultPermissionMode {
        store::default_permission_mode(&self.draft_document).unwrap_or_default()
    }

    #[must_use]
    pub fn default_permission_mode_invalid(&self) -> bool {
        store::default_permission_mode(&self.draft_document).is_err()
    }

    #[must_use]
    pub fn committed_default_permission_mode(&self) -> DefaultPermissionMode {
        store::default_permission_mode(&self.committed_document).unwrap_or_default()
    }

    fn apply_loaded(
        &mut self,
        path: PathBuf,
        document: Value,
        notice: Option<String>,
        preserve_status: bool,
    ) {
        self.path = Some(path);
        self.committed_document = document.clone();
        self.draft_document = document;
        self.dirty = false;
        if !preserve_status {
            self.status_message = notice;
            self.last_error = None;
        } else if let Some(notice) = notice {
            self.status_message = Some(notice);
        }
    }

    fn reset_editor(&mut self) {
        self.draft_document = self.committed_document.clone();
        self.dirty = false;
        self.last_error = None;
    }
}

pub fn initialize_shared_state(app: &mut App) -> Result<(), String> {
    let loaded = store::load(app.settings_path_override.as_deref())?;
    app.settings.apply_loaded(loaded.path, loaded.document, loaded.notice, false);
    Ok(())
}

pub fn open(app: &mut App) -> Result<(), String> {
    ensure_loaded(app)?;
    app.settings.reset_editor();
    view::set_active_view(app, ActiveView::Settings);
    Ok(())
}

fn ensure_loaded(app: &mut App) -> Result<(), String> {
    if settings_source_matches_override(app) && app.settings.path.is_some() {
        return Ok(());
    }
    let loaded = store::load(app.settings_path_override.as_deref())?;
    app.settings.apply_loaded(loaded.path, loaded.document, loaded.notice, true);
    Ok(())
}

pub fn save(app: &mut App) -> Result<(), String> {
    let permission_mode = app.settings.default_permission_mode_effective();
    let Some(path) = app.settings.path.clone() else {
        let message = "Settings path is not available".to_owned();
        app.settings.last_error = Some(message.clone());
        return Err(message);
    };

    let mut normalized_document = app.settings.draft_document.clone();
    store::set_default_permission_mode(&mut normalized_document, permission_mode);
    let model = app.settings.model_effective();
    store::set_model(&mut normalized_document, model.as_deref());

    store::save(&path, &normalized_document)?;
    app.settings.committed_document = normalized_document.clone();
    app.settings.draft_document = normalized_document;
    app.settings.dirty = false;
    app.settings.last_error = None;
    app.settings.status_message = Some(format!(
        "Saved settings to {}. New sessions will use the updated config",
        path.display()
    ));
    Ok(())
}

pub fn close(app: &mut App) -> Result<(), String> {
    if app.settings.dirty {
        save(app)?;
    }
    view::set_active_view(app, ActiveView::Chat);
    Ok(())
}

pub fn handle_key(app: &mut App, key: KeyEvent) {
    if is_ctrl_shortcut(key, 'q') || is_ctrl_shortcut(key, 'c') {
        app.should_quit = true;
        return;
    }

    match (key.code, key.modifiers) {
        (KeyCode::Char('s'), m) if m == KeyModifiers::CONTROL => {
            if let Err(err) = save(app) {
                app.settings.last_error = Some(err);
            }
        }
        (KeyCode::Esc, KeyModifiers::NONE) => {
            if let Err(err) = close(app) {
                app.settings.last_error = Some(err);
            }
        }
        (KeyCode::Left, KeyModifiers::NONE) | (KeyCode::BackTab, _) => {
            app.settings.active_tab = app.settings.active_tab.prev();
            app.settings.status_message = None;
        }
        (KeyCode::Right | KeyCode::Tab, KeyModifiers::NONE) => {
            app.settings.active_tab = app.settings.active_tab.next();
            app.settings.status_message = None;
        }
        (KeyCode::Up, KeyModifiers::NONE) => {
            if app.settings.active_tab == SettingsTab::Config {
                app.settings.selected_config_index =
                    app.settings.selected_config_index.saturating_sub(1);
            }
        }
        (KeyCode::Down, KeyModifiers::NONE) => {
            if app.settings.active_tab == SettingsTab::Config {
                app.settings.selected_config_index =
                    (app.settings.selected_config_index + 1).min(CONFIG_ITEM_COUNT - 1);
            }
        }
        (KeyCode::Enter, KeyModifiers::NONE) if app.settings.active_tab == SettingsTab::Config => {
            match app.settings.selected_config_index {
                0 => {
                    let next = !app.settings.fast_mode_effective();
                    store::set_fast_mode(&mut app.settings.draft_document, next);
                    app.settings.dirty = true;
                    app.settings.last_error = None;
                    app.settings.status_message =
                        Some(format!("Fast mode set to {}", on_off(next)));
                }
                1 => {
                    let next = app.settings.default_permission_mode_effective().next();
                    store::set_default_permission_mode(&mut app.settings.draft_document, next);
                    app.settings.dirty = true;
                    app.settings.last_error = None;
                    app.settings.status_message =
                        Some(format!("Default permission mode set to {}", next.label()));
                }
                2 => {
                    if let Some(next_model) = next_model_selection(app) {
                        let next_model_id = match &next_model {
                            NextModelSelection::Automatic => None,
                            NextModelSelection::Named(model) => Some(model.as_str()),
                        };
                        store::set_model(&mut app.settings.draft_document, next_model_id);
                        app.settings.dirty = true;
                        app.settings.last_error = None;
                        app.settings.status_message = Some(format!(
                            "Default model set to {}",
                            model_status_label(next_model_id, app)
                        ));
                    } else {
                        app.settings.status_message =
                            Some("Connect to load available models".to_owned());
                    }
                }
                _ => {}
            }
        }
        _ => {}
    }
}

fn is_ctrl_shortcut(key: KeyEvent, ch: char) -> bool {
    matches!(key.code, KeyCode::Char(candidate) if candidate == ch)
        && key.modifiers == KeyModifiers::CONTROL
}

fn on_off(value: bool) -> &'static str {
    if value { "On" } else { "Off" }
}

enum NextModelSelection {
    Automatic,
    Named(String),
}

fn next_model_selection(app: &App) -> Option<NextModelSelection> {
    let choices = model_cycle_values(app);
    if choices.is_empty() {
        return None;
    }

    let current = app.settings.model_effective();
    let current_index = choices
        .iter()
        .position(|candidate| candidate.as_deref() == current.as_deref())
        .unwrap_or(0);
    match &choices[(current_index + 1) % choices.len()] {
        Some(model) => Some(NextModelSelection::Named(model.clone())),
        None => Some(NextModelSelection::Automatic),
    }
}

fn model_cycle_values(app: &App) -> Vec<Option<String>> {
    if app.available_models.is_empty() {
        return Vec::new();
    }

    let mut values = Vec::with_capacity(app.available_models.len() + 1);
    values.push(None);
    values.extend(app.available_models.iter().map(|model| Some(model.id.clone())));
    values
}

pub(crate) fn model_status_label(model: Option<&str>, app: &App) -> String {
    match model {
        None => "Automatic".to_owned(),
        Some(model_id) => app
            .available_models
            .iter()
            .find(|candidate| candidate.id == model_id)
            .map_or_else(|| model_id.to_owned(), |candidate| candidate.display_name.clone()),
    }
}

pub(crate) fn model_is_unavailable(app: &App, model: Option<&str>) -> bool {
    match model {
        Some(model_id) if !app.available_models.is_empty() => {
            !app.available_models.iter().any(|candidate| candidate.id == model_id)
        }
        _ => false,
    }
}

fn settings_source_matches_override(app: &App) -> bool {
    match (&app.settings.path, app.settings_path_override.as_ref()) {
        (None, None | Some(_)) => false,
        (Some(_), None) => true,
        (Some(path), Some(override_path)) => path == override_path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn open_loads_document_and_switches_view() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{"fastMode":true}"#).expect("write");
        let mut app = App::test_default();
        app.settings_path_override = Some(path);

        open(&mut app).expect("open");

        assert_eq!(app.active_view, ActiveView::Settings);
        assert!(app.settings.fast_mode_effective());
        assert!(app.settings.path.is_some());
    }

    #[test]
    fn save_persists_toggled_fast_mode() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        let mut app = App::test_default();
        app.settings_path_override = Some(path.clone());

        open(&mut app).expect("open");
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        save(&mut app).expect("save");

        let raw = std::fs::read_to_string(path).expect("read");
        assert!(raw.contains("\"fastMode\": true"));
        assert!(!app.settings.dirty);
    }

    #[test]
    fn close_saves_dirty_document_and_returns_to_chat() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        let mut app = App::test_default();
        app.settings_path_override = Some(path.clone());

        open(&mut app).expect("open");
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        close(&mut app).expect("close");

        assert_eq!(app.active_view, ActiveView::Chat);
        let raw = std::fs::read_to_string(path).expect("read");
        assert!(raw.contains("\"fastMode\": true"));
    }

    #[test]
    fn handle_key_toggles_fast_mode_in_config_tab() {
        let mut app = App::test_default();
        app.active_view = ActiveView::Settings;

        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert!(app.settings.fast_mode_effective());
        assert!(app.settings.dirty);
    }

    #[test]
    fn save_updates_committed_document_from_draft() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        let mut app = App::test_default();
        app.settings_path_override = Some(path);

        open(&mut app).expect("open");
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        save(&mut app).expect("save");

        assert!(app.settings.committed_fast_mode());
        assert_eq!(app.settings.committed_document, app.settings.draft_document);
    }

    #[test]
    fn handle_key_moves_between_config_rows() {
        let mut app = App::test_default();
        app.active_view = ActiveView::Settings;

        handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.settings.selected_config_index, 1);

        handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.settings.selected_config_index, 2);

        handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.settings.selected_config_index, 2);

        handle_key(&mut app, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.settings.selected_config_index, 1);
    }

    #[test]
    fn handle_key_cycles_default_permission_mode_in_config_tab() {
        let mut app = App::test_default();
        app.active_view = ActiveView::Settings;
        app.settings.selected_config_index = 1;

        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(
            app.settings.default_permission_mode_effective(),
            DefaultPermissionMode::AcceptEdits
        );
        assert!(app.settings.dirty);
    }

    #[test]
    fn committed_default_permission_mode_reads_loaded_settings_document() {
        let mut app = App::test_default();
        app.settings.path = Some(PathBuf::from("settings.json"));
        store::set_default_permission_mode(
            &mut app.settings.committed_document,
            DefaultPermissionMode::Plan,
        );
        app.settings.draft_document = app.settings.committed_document.clone();

        assert_eq!(app.settings.committed_default_permission_mode(), DefaultPermissionMode::Plan);
    }

    #[test]
    fn handle_key_cycles_model_when_available_models_are_loaded() {
        let mut app = App::test_default();
        app.active_view = ActiveView::Settings;
        app.settings.selected_config_index = 2;
        app.available_models = vec![
            crate::agent::model::AvailableModel::new("sonnet", "Claude Sonnet"),
            crate::agent::model::AvailableModel::new("opus", "Claude Opus"),
        ];

        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.settings.model_effective().as_deref(), Some("sonnet"));

        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.settings.model_effective().as_deref(), Some("opus"));

        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.settings.model_effective(), None);
    }

    #[test]
    fn handle_key_does_not_cycle_model_without_loaded_catalog() {
        let mut app = App::test_default();
        app.active_view = ActiveView::Settings;
        app.settings.selected_config_index = 2;

        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(app.settings.model_effective(), None);
        assert_eq!(
            app.settings.status_message.as_deref(),
            Some("Connect to load available models")
        );
    }
}
