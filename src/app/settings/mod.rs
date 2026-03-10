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
use crate::agent::model::AvailableModel;
use crate::app::App;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::Value;
use std::path::PathBuf;

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

#[repr(usize)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingId {
    Model,
    DefaultPermissionMode,
    EditorMode,
    FastMode,
    Notifications,
    ReduceMotion,
    RespectGitignore,
    ShowTips,
    Theme,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingKind {
    Bool,
    Enum,
    DynamicEnum,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorKind {
    Toggle,
    Cycle,
    Overlay,
    Search,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueSource {
    PersistedOnly,
    RuntimeBacked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingFile {
    SettingsJson,
    LocalSettingsJson,
    PreferencesJson,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeCatalogKind {
    Models,
    PermissionModes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackPolicy {
    None,
    AppDefault,
    English,
    RuntimeDefault,
}

impl FallbackPolicy {
    #[must_use]
    pub const fn short_label(self) -> &'static str {
        match self {
            Self::None => "current value",
            Self::AppDefault => "default",
            Self::English => "English",
            Self::RuntimeDefault => "runtime default",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettingOption {
    pub stored: &'static str,
    pub label: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingOptions {
    None,
    Static(&'static [SettingOption]),
    RuntimeCatalog(RuntimeCatalogKind),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettingSpec {
    pub id: SettingId,
    pub entry_id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub file: SettingFile,
    pub json_path: &'static [&'static str],
    pub kind: SettingKind,
    pub editor: EditorKind,
    pub source: ValueSource,
    pub options: SettingOptions,
    pub fallback: FallbackPolicy,
    pub supported: bool,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PreferredNotifChannel {
    #[default]
    Iterm2,
    Iterm2WithBell,
    TerminalBell,
    NotificationsDisabled,
    Ghostty,
}

impl PreferredNotifChannel {
    #[must_use]
    pub const fn as_stored(self) -> &'static str {
        match self {
            Self::Iterm2 => "iterm2",
            Self::Iterm2WithBell => "iterm2_with_bell",
            Self::TerminalBell => "terminal_bell",
            Self::NotificationsDisabled => "notifications_disabled",
            Self::Ghostty => "ghostty",
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Iterm2 => "Auto / iTerm2",
            Self::Iterm2WithBell => "iTerm2 with Bell",
            Self::TerminalBell => "Terminal Bell",
            Self::NotificationsDisabled => "Disabled",
            Self::Ghostty => "Ghostty",
        }
    }

    #[must_use]
    pub fn from_stored(value: &str) -> Option<Self> {
        match value {
            "iterm2" => Some(Self::Iterm2),
            "iterm2_with_bell" => Some(Self::Iterm2WithBell),
            "terminal_bell" => Some(Self::TerminalBell),
            "notifications_disabled" => Some(Self::NotificationsDisabled),
            "ghostty" => Some(Self::Ghostty),
            _ => None,
        }
    }
}

const DEFAULT_PERMISSION_OPTIONS: &[SettingOption] = &[
    SettingOption { stored: "default", label: "Default" },
    SettingOption { stored: "acceptEdits", label: "Accept Edits" },
    SettingOption { stored: "plan", label: "Plan" },
    SettingOption { stored: "dontAsk", label: "Don't Ask" },
    SettingOption { stored: "bypassPermissions", label: "Bypass Permissions" },
];

const NOTIFICATION_OPTIONS: &[SettingOption] = &[
    SettingOption { stored: "iterm2", label: "Auto / iTerm2" },
    SettingOption { stored: "iterm2_with_bell", label: "iTerm2 with Bell" },
    SettingOption { stored: "terminal_bell", label: "Terminal Bell" },
    SettingOption { stored: "ghostty", label: "Ghostty" },
    SettingOption { stored: "notifications_disabled", label: "Disabled" },
];

const THEME_OPTIONS: &[SettingOption] = &[
    SettingOption { stored: "dark", label: "Dark" },
    SettingOption { stored: "light", label: "Light" },
    SettingOption { stored: "light-daltonized", label: "Light (Daltonized)" },
    SettingOption { stored: "dark-daltonized", label: "Dark (Daltonized)" },
];

const EDITOR_MODE_OPTIONS: &[SettingOption] = &[
    SettingOption { stored: "default", label: "Default" },
    SettingOption { stored: "vim", label: "Vim" },
];

const CONFIG_SETTINGS: [SettingSpec; 9] = [
    SettingSpec {
        id: SettingId::Model,
        entry_id: "A19",
        label: "Default model",
        description: "Sets the model used when starting new sessions.",
        file: SettingFile::SettingsJson,
        json_path: &["model"],
        kind: SettingKind::DynamicEnum,
        editor: EditorKind::Cycle,
        source: ValueSource::RuntimeBacked,
        options: SettingOptions::RuntimeCatalog(RuntimeCatalogKind::Models),
        fallback: FallbackPolicy::RuntimeDefault,
        supported: true,
    },
    SettingSpec {
        id: SettingId::DefaultPermissionMode,
        entry_id: "A09",
        label: "Default permission mode",
        description: "Sets the default approval behavior for future sessions.",
        file: SettingFile::SettingsJson,
        json_path: &["permissions", "defaultMode"],
        kind: SettingKind::DynamicEnum,
        editor: EditorKind::Cycle,
        source: ValueSource::RuntimeBacked,
        options: SettingOptions::RuntimeCatalog(RuntimeCatalogKind::PermissionModes),
        fallback: FallbackPolicy::RuntimeDefault,
        supported: true,
    },
    SettingSpec {
        id: SettingId::EditorMode,
        entry_id: "A17",
        label: "Editor mode",
        description: "Controls how text editing keys behave.",
        file: SettingFile::PreferencesJson,
        json_path: &["editorMode"],
        kind: SettingKind::Enum,
        editor: EditorKind::Cycle,
        source: ValueSource::PersistedOnly,
        options: SettingOptions::Static(EDITOR_MODE_OPTIONS),
        fallback: FallbackPolicy::AppDefault,
        supported: false,
    },
    SettingSpec {
        id: SettingId::FastMode,
        entry_id: "A05",
        label: "Fast mode",
        description: "Controls the persisted fast-mode preference for future sessions.",
        file: SettingFile::SettingsJson,
        json_path: &["fastMode"],
        kind: SettingKind::Bool,
        editor: EditorKind::Toggle,
        source: ValueSource::PersistedOnly,
        options: SettingOptions::None,
        fallback: FallbackPolicy::AppDefault,
        supported: false,
    },
    SettingSpec {
        id: SettingId::Notifications,
        entry_id: "A14",
        label: "Notifications",
        description: "Controls how the app notifies you when attention is needed.",
        file: SettingFile::PreferencesJson,
        json_path: &["preferredNotifChannel"],
        kind: SettingKind::Enum,
        editor: EditorKind::Cycle,
        source: ValueSource::PersistedOnly,
        options: SettingOptions::Static(NOTIFICATION_OPTIONS),
        fallback: FallbackPolicy::AppDefault,
        supported: true,
    },
    SettingSpec {
        id: SettingId::ReduceMotion,
        entry_id: "A03",
        label: "Reduce motion",
        description: "Reduce UI motion by slowing spinners and disabling smooth chat scrolling.",
        file: SettingFile::LocalSettingsJson,
        json_path: &["prefersReducedMotion"],
        kind: SettingKind::Bool,
        editor: EditorKind::Toggle,
        source: ValueSource::PersistedOnly,
        options: SettingOptions::None,
        fallback: FallbackPolicy::AppDefault,
        supported: true,
    },
    SettingSpec {
        id: SettingId::RespectGitignore,
        entry_id: "A10",
        label: "Respect .gitignore",
        description: "Controls whether @ file mentions hide entries ignored by git ignore rules.",
        file: SettingFile::PreferencesJson,
        json_path: &["respectGitignore"],
        kind: SettingKind::Bool,
        editor: EditorKind::Toggle,
        source: ValueSource::PersistedOnly,
        options: SettingOptions::None,
        fallback: FallbackPolicy::AppDefault,
        supported: true,
    },
    SettingSpec {
        id: SettingId::ShowTips,
        entry_id: "A02",
        label: "Show Tips",
        description: "Controls whether Claude should show spinner tips in supported clients for this project.",
        file: SettingFile::LocalSettingsJson,
        json_path: &["spinnerTipsEnabled"],
        kind: SettingKind::Bool,
        editor: EditorKind::Toggle,
        source: ValueSource::PersistedOnly,
        options: SettingOptions::None,
        fallback: FallbackPolicy::AppDefault,
        supported: false,
    },
    SettingSpec {
        id: SettingId::Theme,
        entry_id: "A13",
        label: "Theme",
        description: "Controls the app color theme.",
        file: SettingFile::PreferencesJson,
        json_path: &["theme"],
        kind: SettingKind::Enum,
        editor: EditorKind::Cycle,
        source: ValueSource::PersistedOnly,
        options: SettingOptions::Static(THEME_OPTIONS),
        fallback: FallbackPolicy::AppDefault,
        supported: false,
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingValidation {
    Valid,
    InvalidValue,
    UnavailableOption,
}

impl SettingValidation {
    #[must_use]
    pub const fn is_invalid(self) -> bool {
        !matches!(self, Self::Valid)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedChoice {
    Automatic,
    Stored(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedSettingValue {
    Bool(bool),
    Choice(ResolvedChoice),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSetting {
    pub value: ResolvedSettingValue,
    pub validation: SettingValidation,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SettingsState {
    pub active_tab: SettingsTab,
    pub selected_config_index: usize,
    pub committed_settings_document: Value,
    pub draft_settings_document: Value,
    pub committed_local_settings_document: Value,
    pub draft_local_settings_document: Value,
    pub committed_preferences_document: Value,
    pub draft_preferences_document: Value,
    pub dirty: bool,
    pub settings_path: Option<PathBuf>,
    pub local_settings_path: Option<PathBuf>,
    pub preferences_path: Option<PathBuf>,
    pub status_message: Option<String>,
    pub last_error: Option<String>,
}

impl Default for SettingsState {
    fn default() -> Self {
        Self {
            active_tab: SettingsTab::Config,
            selected_config_index: 0,
            committed_settings_document: Value::Object(serde_json::Map::new()),
            draft_settings_document: Value::Object(serde_json::Map::new()),
            committed_local_settings_document: Value::Object(serde_json::Map::new()),
            draft_local_settings_document: Value::Object(serde_json::Map::new()),
            committed_preferences_document: Value::Object(serde_json::Map::new()),
            draft_preferences_document: Value::Object(serde_json::Map::new()),
            dirty: false,
            settings_path: None,
            local_settings_path: None,
            preferences_path: None,
            status_message: None,
            last_error: None,
        }
    }
}

impl SettingsState {
    #[must_use]
    pub fn fast_mode_effective(&self) -> bool {
        match resolve_setting_document(&self.draft_settings_document, SettingId::FastMode, &[])
            .value
        {
            ResolvedSettingValue::Bool(value) => value,
            ResolvedSettingValue::Choice(_) => false,
        }
    }

    #[must_use]
    pub fn model_effective(&self) -> Option<String> {
        match resolve_setting_document(&self.draft_settings_document, SettingId::Model, &[]).value {
            ResolvedSettingValue::Choice(ResolvedChoice::Stored(value)) => Some(value),
            _ => None,
        }
    }

    #[must_use]
    pub fn default_permission_mode_effective(&self) -> DefaultPermissionMode {
        match resolve_setting_document(
            &self.draft_settings_document,
            SettingId::DefaultPermissionMode,
            &[],
        )
        .value
        {
            ResolvedSettingValue::Choice(ResolvedChoice::Stored(value)) => {
                DefaultPermissionMode::from_stored(&value).unwrap_or_default()
            }
            _ => DefaultPermissionMode::Default,
        }
    }

    #[must_use]
    pub fn respect_gitignore_effective(&self) -> bool {
        store::respect_gitignore(&self.committed_preferences_document).unwrap_or(true)
    }

    #[must_use]
    pub fn preferred_notification_channel_effective(&self) -> PreferredNotifChannel {
        store::preferred_notification_channel(&self.committed_preferences_document)
            .unwrap_or_default()
    }

    #[must_use]
    pub fn prefers_reduced_motion_effective(&self) -> bool {
        store::prefers_reduced_motion(&self.committed_local_settings_document).unwrap_or(false)
    }

    #[must_use]
    pub fn selected_config_spec(&self) -> Option<&'static SettingSpec> {
        config_settings().get(self.selected_config_index)
    }

    #[must_use]
    pub fn path_for(&self, file: SettingFile) -> Option<&PathBuf> {
        match file {
            SettingFile::SettingsJson => self.settings_path.as_ref(),
            SettingFile::LocalSettingsJson => self.local_settings_path.as_ref(),
            SettingFile::PreferencesJson => self.preferences_path.as_ref(),
        }
    }

    #[must_use]
    pub fn draft_document_for(&self, file: SettingFile) -> &Value {
        match file {
            SettingFile::SettingsJson => &self.draft_settings_document,
            SettingFile::LocalSettingsJson => &self.draft_local_settings_document,
            SettingFile::PreferencesJson => &self.draft_preferences_document,
        }
    }

    pub fn draft_document_for_mut(&mut self, file: SettingFile) -> &mut Value {
        match file {
            SettingFile::SettingsJson => &mut self.draft_settings_document,
            SettingFile::LocalSettingsJson => &mut self.draft_local_settings_document,
            SettingFile::PreferencesJson => &mut self.draft_preferences_document,
        }
    }

    fn apply_loaded(
        &mut self,
        loaded: store::LoadedSettingsDocuments,
        notice: Option<String>,
        preserve_status: bool,
    ) {
        self.settings_path = Some(loaded.paths.settings_path);
        self.local_settings_path = Some(loaded.paths.local_settings_path);
        self.preferences_path = Some(loaded.paths.preferences_path);
        self.committed_settings_document = loaded.settings_document.clone();
        self.draft_settings_document = loaded.settings_document;
        self.committed_local_settings_document = loaded.local_settings_document.clone();
        self.draft_local_settings_document = loaded.local_settings_document;
        self.committed_preferences_document = loaded.preferences_document.clone();
        self.draft_preferences_document = loaded.preferences_document;
        self.dirty = false;
        self.selected_config_index =
            self.selected_config_index.min(config_settings().len().saturating_sub(1));
        if !preserve_status {
            self.status_message = notice;
            self.last_error = None;
        } else if let Some(notice) = notice {
            self.status_message = Some(notice);
        }
    }

    fn reset_editor(&mut self) {
        self.draft_settings_document = self.committed_settings_document.clone();
        self.draft_local_settings_document = self.committed_local_settings_document.clone();
        self.draft_preferences_document = self.committed_preferences_document.clone();
        self.dirty = false;
        self.last_error = None;
    }
}

#[must_use]
pub const fn config_settings() -> &'static [SettingSpec] {
    &CONFIG_SETTINGS
}

#[must_use]
pub fn config_setting(id: SettingId) -> &'static SettingSpec {
    &CONFIG_SETTINGS[id as usize]
}

#[must_use]
pub fn resolved_setting(app: &App, spec: &SettingSpec) -> ResolvedSetting {
    let document = app.settings.draft_document_for(spec.file);
    resolve_setting_document(document, spec.id, &app.available_models)
}

#[must_use]
pub fn setting_display_value(app: &App, spec: &SettingSpec, resolved: &ResolvedSetting) -> String {
    match (&resolved.value, spec.id) {
        (ResolvedSettingValue::Bool(value), _) => {
            if *value {
                "On".to_owned()
            } else {
                "Off".to_owned()
            }
        }
        (ResolvedSettingValue::Choice(ResolvedChoice::Automatic), SettingId::Model) => {
            "Automatic".to_owned()
        }
        (ResolvedSettingValue::Choice(ResolvedChoice::Stored(value)), SettingId::Model) => {
            model_status_label(Some(value), app)
        }
        (ResolvedSettingValue::Choice(ResolvedChoice::Stored(value)), _) => {
            option_label(spec, value).unwrap_or_else(|| value.clone())
        }
        _ => String::new(),
    }
}

#[must_use]
pub fn setting_invalid_hint(spec: &SettingSpec, validation: SettingValidation) -> Option<String> {
    match validation {
        SettingValidation::Valid => None,
        SettingValidation::InvalidValue => {
            Some(format!("invalid value, using {}", spec.fallback.short_label()))
        }
        SettingValidation::UnavailableOption if spec.id == SettingId::Model => {
            Some("model not advertised by current SDK session".to_owned())
        }
        SettingValidation::UnavailableOption => {
            Some(format!("value unavailable, using {}", spec.fallback.short_label()))
        }
    }
}

#[must_use]
pub fn setting_detail_options(app: &App, spec: &SettingSpec) -> Vec<String> {
    match spec.kind {
        SettingKind::Bool => vec!["Off".to_owned(), "On".to_owned()],
        SettingKind::Enum | SettingKind::DynamicEnum => match spec.options {
            SettingOptions::None => Vec::new(),
            SettingOptions::Static(options) => {
                options.iter().map(|option| option.label.to_owned()).collect()
            }
            SettingOptions::RuntimeCatalog(RuntimeCatalogKind::Models) => {
                if app.available_models.is_empty() {
                    vec!["Automatic".to_owned(), "Connect to load available models".to_owned()]
                } else {
                    let mut options = Vec::with_capacity(app.available_models.len() + 1);
                    options.push("Automatic".to_owned());
                    options.extend(
                        app.available_models.iter().map(|model| model.display_name.clone()),
                    );
                    options
                }
            }
            SettingOptions::RuntimeCatalog(RuntimeCatalogKind::PermissionModes) => {
                DEFAULT_PERMISSION_OPTIONS.iter().map(|option| option.label.to_owned()).collect()
            }
        },
    }
}

pub fn initialize_shared_state(app: &mut App) -> Result<(), String> {
    let loaded = store::load(app.settings_home_override.as_deref(), Some(project_root(app)))?;
    let notice = loaded.notice.clone();
    app.settings.apply_loaded(loaded, notice, false);
    Ok(())
}

pub fn open(app: &mut App) -> Result<(), String> {
    ensure_loaded(app)?;
    app.settings.reset_editor();
    view::set_active_view(app, ActiveView::Settings);
    Ok(())
}

fn ensure_loaded(app: &mut App) -> Result<(), String> {
    if settings_source_matches_override(app)
        && app.settings.settings_path.is_some()
        && app.settings.local_settings_path.is_some()
        && app.settings.preferences_path.is_some()
    {
        return Ok(());
    }
    let loaded = store::load(app.settings_home_override.as_deref(), Some(project_root(app)))?;
    let notice = loaded.notice.clone();
    app.settings.apply_loaded(loaded, notice, true);
    Ok(())
}

pub fn save(app: &mut App) -> Result<(), String> {
    let (Some(settings_path), Some(local_settings_path), Some(preferences_path)) = (
        app.settings.settings_path.clone(),
        app.settings.local_settings_path.clone(),
        app.settings.preferences_path.clone(),
    ) else {
        let message = "Settings paths are not available".to_owned();
        app.settings.last_error = Some(message.clone());
        return Err(message);
    };

    let previous_respect_gitignore = app.settings.respect_gitignore_effective();
    let mut saved_any = false;
    if app.settings.draft_settings_document != app.settings.committed_settings_document {
        store::save(&settings_path, &app.settings.draft_settings_document)?;
        app.settings.committed_settings_document = app.settings.draft_settings_document.clone();
        saved_any = true;
    }
    if app.settings.draft_local_settings_document != app.settings.committed_local_settings_document
    {
        store::save(&local_settings_path, &app.settings.draft_local_settings_document)?;
        app.settings.committed_local_settings_document =
            app.settings.draft_local_settings_document.clone();
        saved_any = true;
    }
    if app.settings.draft_preferences_document != app.settings.committed_preferences_document {
        store::save(&preferences_path, &app.settings.draft_preferences_document)?;
        app.settings.committed_preferences_document =
            app.settings.draft_preferences_document.clone();
        saved_any = true;
    }
    let current_respect_gitignore = app.settings.respect_gitignore_effective();
    if previous_respect_gitignore != current_respect_gitignore {
        crate::app::mention::invalidate_session_cache(app);
    }
    app.settings.dirty = false;
    app.settings.last_error = None;
    app.settings.status_message = Some(if saved_any {
        "Saved settings changes. New sessions will use the updated config.".to_owned()
    } else {
        "No settings changes to save.".to_owned()
    });
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
                let last_index = config_settings().len().saturating_sub(1);
                app.settings.selected_config_index =
                    (app.settings.selected_config_index + 1).min(last_index);
            }
        }
        (KeyCode::Enter, KeyModifiers::NONE) if app.settings.active_tab == SettingsTab::Config => {
            if let Some(spec) = app.settings.selected_config_spec() {
                activate_setting(app, spec);
            }
        }
        _ => {}
    }
}

fn is_ctrl_shortcut(key: KeyEvent, ch: char) -> bool {
    matches!(key.code, KeyCode::Char(candidate) if candidate == ch)
        && key.modifiers == KeyModifiers::CONTROL
}

fn activate_setting(app: &mut App, spec: &SettingSpec) {
    match spec.id {
        SettingId::ShowTips => {
            let next = !store::spinner_tips_enabled(&app.settings.draft_local_settings_document)
                .unwrap_or(true);
            store::set_spinner_tips_enabled(&mut app.settings.draft_local_settings_document, next);
            mark_setting_edited(app, format!("{} set to {}", spec.label, on_off(next)));
        }
        SettingId::ReduceMotion => {
            let next = !store::prefers_reduced_motion(&app.settings.draft_local_settings_document)
                .unwrap_or(false);
            store::set_prefers_reduced_motion(
                &mut app.settings.draft_local_settings_document,
                next,
            );
            mark_setting_edited(app, format!("{} set to {}", spec.label, on_off(next)));
        }
        SettingId::FastMode => {
            let next = !store::fast_mode(&app.settings.draft_settings_document).unwrap_or(false);
            store::set_fast_mode(&mut app.settings.draft_settings_document, next);
            mark_setting_edited(app, format!("{} set to {}", spec.label, on_off(next)));
        }
        SettingId::RespectGitignore => {
            let next =
                !store::respect_gitignore(&app.settings.draft_preferences_document).unwrap_or(true);
            store::set_respect_gitignore(&mut app.settings.draft_preferences_document, next);
            mark_setting_edited(app, format!("{} set to {}", spec.label, on_off(next)));
        }
        SettingId::DefaultPermissionMode => {
            let current = match resolve_setting_document(
                &app.settings.draft_settings_document,
                SettingId::DefaultPermissionMode,
                &[],
            )
            .value
            {
                ResolvedSettingValue::Choice(ResolvedChoice::Stored(value)) => {
                    DefaultPermissionMode::from_stored(&value).unwrap_or_default()
                }
                _ => DefaultPermissionMode::Default,
            };
            let next = current.next();
            store::set_default_permission_mode(&mut app.settings.draft_settings_document, next);
            mark_setting_edited(app, format!("{} set to {}", spec.label, next.label()));
        }
        SettingId::Model => {
            if let Some(next_model) = next_model_selection(app) {
                let next_model_id = match &next_model {
                    NextModelSelection::Automatic => None,
                    NextModelSelection::Named(model) => Some(model.as_str()),
                };
                store::set_model(&mut app.settings.draft_settings_document, next_model_id);
                mark_setting_edited(
                    app,
                    format!("{} set to {}", spec.label, model_status_label(next_model_id, app)),
                );
            } else {
                app.settings.status_message = Some("Connect to load available models".to_owned());
            }
        }
        SettingId::Theme | SettingId::Notifications | SettingId::EditorMode => {
            cycle_static_enum(app, spec);
        }
    }
}

fn mark_setting_edited(app: &mut App, status_message: String) {
    app.settings.dirty = true;
    app.settings.last_error = None;
    app.settings.status_message = Some(status_message);
}

fn on_off(value: bool) -> &'static str {
    if value { "On" } else { "Off" }
}

fn cycle_static_enum(app: &mut App, spec: &SettingSpec) {
    let current = {
        let document = app.settings.draft_document_for(spec.file);
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

    {
        let document = app.settings.draft_document_for_mut(spec.file);
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
    }
    mark_setting_edited(
        app,
        format!(
            "{} set to {}",
            spec.label,
            option_label(spec, next).unwrap_or_else(|| next.to_owned())
        ),
    );
}

const fn default_static_value(setting_id: SettingId) -> &'static str {
    match setting_id {
        SettingId::Theme => "dark",
        SettingId::Notifications => "iterm2",
        SettingId::EditorMode => "default",
        SettingId::ReduceMotion => "",
        SettingId::ShowTips
        | SettingId::FastMode
        | SettingId::DefaultPermissionMode
        | SettingId::RespectGitignore
        | SettingId::Model => "",
    }
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

    let current = store::model(&app.settings.draft_settings_document).ok().flatten();
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

fn settings_source_matches_override(app: &App) -> bool {
    let (Some(settings_path), Some(local_settings_path), Some(preferences_path)) = (
        &app.settings.settings_path,
        &app.settings.local_settings_path,
        &app.settings.preferences_path,
    ) else {
        return false;
    };
    let expected_local_settings = project_root(app).join(".claude").join("settings.local.json");

    match app.settings_home_override.as_ref() {
        None => local_settings_path == &expected_local_settings,
        Some(home_override) => {
            let expected_settings = home_override.join(".claude").join("settings.json");
            let expected_preferences = home_override.join(".claude.json");
            settings_path == &expected_settings
                && local_settings_path == &expected_local_settings
                && preferences_path == &expected_preferences
        }
    }
}

fn project_root(app: &App) -> &std::path::Path {
    std::path::Path::new(&app.cwd_raw)
}

fn resolve_setting_document(
    document: &Value,
    setting_id: SettingId,
    available_models: &[AvailableModel],
) -> ResolvedSetting {
    let spec = config_setting(setting_id);
    match setting_id {
        SettingId::FastMode => resolve_bool_setting(document, spec, false),
        SettingId::DefaultPermissionMode => {
            resolve_string_setting(document, spec, DefaultPermissionMode::Default.as_stored())
        }
        SettingId::ReduceMotion => resolve_bool_setting(document, spec, false),
        SettingId::ShowTips | SettingId::RespectGitignore => {
            resolve_bool_setting(document, spec, true)
        }
        SettingId::Model => resolve_model_setting(document, spec, available_models),
        SettingId::Theme => resolve_string_setting(document, spec, "dark"),
        SettingId::Notifications => {
            resolve_string_setting(document, spec, PreferredNotifChannel::default().as_stored())
        }
        SettingId::EditorMode => resolve_string_setting(document, spec, "default"),
    }
}

fn resolve_bool_setting(document: &Value, spec: &SettingSpec, fallback: bool) -> ResolvedSetting {
    match store::read_persisted_setting(document, spec) {
        Ok(store::PersistedSettingValue::Bool(value)) => ResolvedSetting {
            value: ResolvedSettingValue::Bool(value),
            validation: SettingValidation::Valid,
        },
        Ok(store::PersistedSettingValue::Missing) => ResolvedSetting {
            value: ResolvedSettingValue::Bool(fallback),
            validation: SettingValidation::Valid,
        },
        Ok(store::PersistedSettingValue::String(_)) | Err(()) => ResolvedSetting {
            value: ResolvedSettingValue::Bool(fallback),
            validation: SettingValidation::InvalidValue,
        },
    }
}

fn resolve_string_setting(
    document: &Value,
    spec: &SettingSpec,
    fallback: &'static str,
) -> ResolvedSetting {
    match store::read_persisted_setting(document, spec) {
        Ok(store::PersistedSettingValue::String(value)) if option_exists(spec, &value) => {
            ResolvedSetting {
                value: ResolvedSettingValue::Choice(ResolvedChoice::Stored(value)),
                validation: SettingValidation::Valid,
            }
        }
        Ok(store::PersistedSettingValue::Missing) => ResolvedSetting {
            value: ResolvedSettingValue::Choice(ResolvedChoice::Stored(fallback.to_owned())),
            validation: SettingValidation::Valid,
        },
        Ok(store::PersistedSettingValue::String(_) | store::PersistedSettingValue::Bool(_))
        | Err(()) => ResolvedSetting {
            value: ResolvedSettingValue::Choice(ResolvedChoice::Stored(fallback.to_owned())),
            validation: SettingValidation::InvalidValue,
        },
    }
}

fn resolve_model_setting(
    document: &Value,
    spec: &SettingSpec,
    available_models: &[AvailableModel],
) -> ResolvedSetting {
    match store::read_persisted_setting(document, spec) {
        Ok(store::PersistedSettingValue::Missing) => ResolvedSetting {
            value: ResolvedSettingValue::Choice(ResolvedChoice::Automatic),
            validation: SettingValidation::Valid,
        },
        Ok(store::PersistedSettingValue::String(value)) if available_models.is_empty() => {
            ResolvedSetting {
                value: ResolvedSettingValue::Choice(ResolvedChoice::Stored(value)),
                validation: SettingValidation::Valid,
            }
        }
        Ok(store::PersistedSettingValue::String(value))
            if available_models.iter().any(|model| model.id == value) =>
        {
            ResolvedSetting {
                value: ResolvedSettingValue::Choice(ResolvedChoice::Stored(value)),
                validation: SettingValidation::Valid,
            }
        }
        Ok(store::PersistedSettingValue::String(_)) => ResolvedSetting {
            value: ResolvedSettingValue::Choice(ResolvedChoice::Automatic),
            validation: SettingValidation::UnavailableOption,
        },
        Ok(store::PersistedSettingValue::Bool(_)) | Err(()) => ResolvedSetting {
            value: ResolvedSettingValue::Choice(ResolvedChoice::Automatic),
            validation: SettingValidation::InvalidValue,
        },
    }
}

fn option_exists(spec: &SettingSpec, value: &str) -> bool {
    match spec.options {
        SettingOptions::Static(options) => options.iter().any(|option| option.stored == value),
        SettingOptions::RuntimeCatalog(RuntimeCatalogKind::PermissionModes) => {
            DEFAULT_PERMISSION_OPTIONS.iter().any(|option| option.stored == value)
        }
        _ => false,
    }
}

fn option_label(spec: &SettingSpec, value: &str) -> Option<String> {
    match spec.options {
        SettingOptions::Static(options) => options
            .iter()
            .find(|option| option.stored == value)
            .map(|option| option.label.to_owned()),
        SettingOptions::RuntimeCatalog(RuntimeCatalogKind::PermissionModes) => {
            DEFAULT_PERMISSION_OPTIONS
                .iter()
                .find(|option| option.stored == value)
                .map(|option| option.label.to_owned())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn open_loads_document_and_switches_view() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".claude").join("settings.json");
        std::fs::create_dir_all(path.parent().expect("settings parent")).expect("create dir");
        std::fs::write(&path, r#"{"fastMode":true}"#).expect("write");
        let mut app = App::test_default();
        app.settings_home_override = Some(dir.path().to_path_buf());
        app.cwd_raw = dir.path().to_string_lossy().to_string();

        open(&mut app).expect("open");

        assert_eq!(app.active_view, ActiveView::Settings);
        assert!(matches!(
            resolve_setting_document(
                &app.settings.draft_settings_document,
                SettingId::FastMode,
                &[]
            )
            .value,
            ResolvedSettingValue::Bool(true)
        ));
        assert!(app.settings.settings_path.is_some());
        assert!(app.settings.local_settings_path.is_some());
        assert!(app.settings.preferences_path.is_some());
    }

    #[test]
    fn save_persists_toggled_fast_mode() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".claude").join("settings.json");
        let mut app = App::test_default();
        app.settings_home_override = Some(dir.path().to_path_buf());
        app.cwd_raw = dir.path().to_string_lossy().to_string();

        open(&mut app).expect("open");
        app.settings.selected_config_index = 3;
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        save(&mut app).expect("save");

        let raw = std::fs::read_to_string(path).expect("read");
        assert!(raw.contains("\"fastMode\": true"));
        assert!(!app.settings.dirty);
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
        assert_eq!(app.settings.selected_config_index, 3);

        handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.settings.selected_config_index, 4);

        handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.settings.selected_config_index, 5);

        handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.settings.selected_config_index, 6);

        handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.settings.selected_config_index, 7);

        handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.settings.selected_config_index, 8);

        handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.settings.selected_config_index, 8);
    }

    #[test]
    fn reduce_motion_toggles_in_local_settings_document() {
        let mut app = App::test_default();
        app.active_view = ActiveView::Settings;
        app.settings.selected_config_index = 5;

        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(
            store::prefers_reduced_motion(&app.settings.draft_local_settings_document),
            Ok(true)
        );
    }

    #[test]
    fn show_tips_toggles_in_local_settings_document() {
        let mut app = App::test_default();
        app.active_view = ActiveView::Settings;
        app.settings.selected_config_index = 7;

        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(
            store::spinner_tips_enabled(&app.settings.draft_local_settings_document),
            Ok(false)
        );
    }

    #[test]
    fn handle_key_cycles_default_permission_mode() {
        let mut app = App::test_default();
        app.active_view = ActiveView::Settings;
        app.settings.selected_config_index = 1;

        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(
            store::default_permission_mode(&app.settings.draft_settings_document),
            Ok(DefaultPermissionMode::AcceptEdits)
        );
    }

    #[test]
    fn respect_gitignore_toggles_in_preferences_document() {
        let mut app = App::test_default();
        app.active_view = ActiveView::Settings;
        app.settings.selected_config_index = 6;

        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(store::respect_gitignore(&app.settings.draft_preferences_document), Ok(false));
    }

    #[test]
    fn save_respect_gitignore_invalidates_active_mention_session_cache() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = App::test_default();
        app.settings_home_override = Some(dir.path().to_path_buf());
        app.cwd_raw = dir.path().to_string_lossy().to_string();

        open(&mut app).expect("open");
        app.mention = Some(crate::app::mention::MentionState::new(0, 0, "rs".to_owned(), vec![]));
        app.settings.selected_config_index = 6;
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        save(&mut app).expect("save");

        let mention = app.mention.as_ref().expect("mention should stay active");
        assert!(mention.candidates.is_empty());
        assert_eq!(mention.placeholder_message().as_deref(), Some("Searching files..."));
        assert!(!app.settings.respect_gitignore_effective());
    }

    #[test]
    fn save_preserves_invalid_unedited_values() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".claude").join("settings.json");
        std::fs::create_dir_all(path.parent().expect("settings parent")).expect("create dir");
        std::fs::write(&path, r#"{"permissions":{"defaultMode":"broken"},"fastMode":false}"#)
            .expect("write");
        let mut app = App::test_default();
        app.settings_home_override = Some(dir.path().to_path_buf());
        app.cwd_raw = dir.path().to_string_lossy().to_string();

        open(&mut app).expect("open");
        app.settings.selected_config_index = 3;
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        save(&mut app).expect("save");

        let raw = std::fs::read_to_string(path).expect("read");
        assert!(raw.contains("\"defaultMode\": \"broken\""));
        assert!(raw.contains("\"fastMode\": true"));
    }

    #[test]
    fn resolved_model_uses_runtime_fallback_when_catalog_rejects_value() {
        let mut app = App::test_default();
        app.available_models = vec![AvailableModel::new("sonnet", "Claude Sonnet")];
        store::set_model(&mut app.settings.draft_settings_document, Some("unknown"));

        let resolved = resolved_setting(&app, config_setting(SettingId::Model));

        assert_eq!(resolved.validation, SettingValidation::UnavailableOption);
        assert_eq!(
            setting_display_value(&app, config_setting(SettingId::Model), &resolved),
            "Automatic"
        );
    }

    #[test]
    fn notifications_cycle_in_preferences_document() {
        let mut app = App::test_default();
        app.active_view = ActiveView::Settings;
        app.settings.selected_config_index = 4;

        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(
            store::preferred_notification_channel(&app.settings.draft_preferences_document),
            Ok(PreferredNotifChannel::Iterm2WithBell)
        );
    }

    #[test]
    fn theme_cycles_in_preferences_document() {
        let mut app = App::test_default();
        app.active_view = ActiveView::Settings;
        app.settings.selected_config_index = 8;

        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        let stored = store::read_persisted_setting(
            &app.settings.draft_preferences_document,
            config_setting(SettingId::Theme),
        );
        assert_eq!(stored, Ok(store::PersistedSettingValue::String("light".to_owned())));
    }

    #[test]
    fn editor_mode_cycles_in_preferences_document() {
        let mut app = App::test_default();
        app.active_view = ActiveView::Settings;
        app.settings.selected_config_index = 2;

        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        let stored = store::read_persisted_setting(
            &app.settings.draft_preferences_document,
            config_setting(SettingId::EditorMode),
        );
        assert_eq!(stored, Ok(store::PersistedSettingValue::String("vim".to_owned())));
    }

    #[test]
    fn save_persists_local_project_settings() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".claude").join("settings.local.json");
        let mut app = App::test_default();
        app.settings_home_override = Some(dir.path().to_path_buf());
        app.cwd_raw = dir.path().to_string_lossy().to_string();

        open(&mut app).expect("open");
        app.settings.selected_config_index = 5;
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        app.settings.selected_config_index = 7;
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        save(&mut app).expect("save");

        let raw = std::fs::read_to_string(path).expect("read");
        assert!(raw.contains("\"prefersReducedMotion\": true"));
        assert!(raw.contains("\"spinnerTipsEnabled\": false"));
    }
}
