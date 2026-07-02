// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::edit::model_overlay_options;
use super::prelude::*;
use super::status::model_status_label;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingId {
    AlwaysThinking,
    Model,
    DefaultPermissionMode,
    EditorMode,
    FastMode,
    Language,
    Notifications,
    OutputStyle,
    ReduceMotion,
    RespectGitignore,
    ShowTips,
    TerminalProgressBar,
    Theme,
    ThinkingEffort,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingKind {
    Bool,
    Enum,
    DynamicEnum,
    Text,
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
    Settings,
    LocalSettings,
    Preferences,
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
    Unset,
}

impl FallbackPolicy {
    #[must_use]
    pub const fn short_label(self) -> &'static str {
        match self {
            Self::None => "current value",
            Self::AppDefault => "default",
            Self::English => "English",
            Self::RuntimeDefault => "runtime default",
            Self::Unset => "unset",
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
    Auto,
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
            Self::Auto => "auto",
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
            Self::Auto => "Auto",
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
            "auto" => Some(Self::Auto),
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
            Self::Default => Self::Auto,
            Self::Auto => Self::AcceptEdits,
            Self::AcceptEdits => Self::Plan,
            Self::Plan => Self::DontAsk,
            Self::DontAsk => Self::BypassPermissions,
            Self::BypassPermissions => Self::Default,
        }
    }

    #[must_use]
    pub const fn prev(self) -> Self {
        match self {
            Self::Default => Self::BypassPermissions,
            Self::Auto => Self::Default,
            Self::AcceptEdits => Self::Auto,
            Self::Plan => Self::AcceptEdits,
            Self::DontAsk => Self::Plan,
            Self::BypassPermissions => Self::DontAsk,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputStyle {
    #[default]
    Default,
    Explanatory,
    Learning,
}

impl OutputStyle {
    pub const ALL: [Self; 3] = [Self::Default, Self::Explanatory, Self::Learning];

    #[must_use]
    pub const fn as_stored(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::Explanatory => "Explanatory",
            Self::Learning => "Learning",
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        self.as_stored()
    }

    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::Default => {
                "Claude completes coding tasks efficiently and provides concise responses"
            }
            Self::Explanatory => "Claude explains its implementation choices and codebase patterns",
            Self::Learning => {
                "Claude pauses and asks you to write small pieces of code for hands-on practice"
            }
        }
    }

    #[must_use]
    pub fn from_stored(value: &str) -> Option<Self> {
        match value {
            "Default" => Some(Self::Default),
            "Explanatory" => Some(Self::Explanatory),
            "Learning" => Some(Self::Learning),
            _ => None,
        }
    }
}

pub(crate) const DEFAULT_PERMISSION_OPTIONS: &[SettingOption] = &[
    SettingOption { stored: "default", label: "Default" },
    SettingOption { stored: "auto", label: "Auto" },
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

const OUTPUT_STYLE_OPTIONS: &[SettingOption] = &[
    SettingOption { stored: "Default", label: "Default" },
    SettingOption { stored: "Explanatory", label: "Explanatory" },
    SettingOption { stored: "Learning", label: "Learning" },
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
pub(crate) const DEFAULT_MODEL_ALIAS_ID: &str = "fable";
pub(crate) const DEFAULT_MODEL_ALIAS_LABEL: &str = "Fable 5";
pub(crate) const LANGUAGE_MIN_CHARS: usize = 2;
pub(crate) const LANGUAGE_MAX_CHARS: usize = 30;

const CONFIG_SETTINGS: [SettingSpec; 14] = [
    SettingSpec {
        id: SettingId::AlwaysThinking,
        entry_id: "A04",
        label: "Always Thinking",
        description: "Enable adaptive thinking for new sessions. When off, new sessions start with thinking disabled.",
        file: SettingFile::Settings,
        json_path: &["alwaysThinkingEnabled"],
        kind: SettingKind::Bool,
        editor: EditorKind::Toggle,
        source: ValueSource::PersistedOnly,
        options: SettingOptions::None,
        fallback: FallbackPolicy::AppDefault,
        supported: true,
    },
    SettingSpec {
        id: SettingId::Model,
        entry_id: "A19",
        label: "Model",
        description: "Sets the model for new sessions.",
        file: SettingFile::Settings,
        json_path: &["model"],
        kind: SettingKind::DynamicEnum,
        editor: EditorKind::Overlay,
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
        file: SettingFile::Settings,
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
        description: "Controls how text editing keys behave in the TUI.",
        file: SettingFile::Preferences,
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
        file: SettingFile::Settings,
        json_path: &["fastMode"],
        kind: SettingKind::Bool,
        editor: EditorKind::Toggle,
        source: ValueSource::PersistedOnly,
        options: SettingOptions::None,
        fallback: FallbackPolicy::AppDefault,
        supported: true,
    },
    SettingSpec {
        id: SettingId::Language,
        entry_id: "A16",
        label: "Language",
        description: "Controls the free-text language instruction Claude uses in sessions. Accepts 2 to 30 characters and does not localize the UI.",
        file: SettingFile::Settings,
        json_path: &["language"],
        kind: SettingKind::Text,
        editor: EditorKind::Overlay,
        source: ValueSource::PersistedOnly,
        options: SettingOptions::None,
        fallback: FallbackPolicy::Unset,
        supported: true,
    },
    SettingSpec {
        id: SettingId::Notifications,
        entry_id: "A14",
        label: "Notifications",
        description: "Controls how Claude Code notifies you when attention is needed.",
        file: SettingFile::Preferences,
        json_path: &["preferredNotifChannel"],
        kind: SettingKind::Enum,
        editor: EditorKind::Cycle,
        source: ValueSource::PersistedOnly,
        options: SettingOptions::Static(NOTIFICATION_OPTIONS),
        fallback: FallbackPolicy::AppDefault,
        supported: true,
    },
    SettingSpec {
        id: SettingId::OutputStyle,
        entry_id: "A15",
        label: "Output style",
        description: "Changes how Claude communicates with you in sessions.",
        file: SettingFile::LocalSettings,
        json_path: &["outputStyle"],
        kind: SettingKind::Enum,
        editor: EditorKind::Overlay,
        source: ValueSource::PersistedOnly,
        options: SettingOptions::Static(OUTPUT_STYLE_OPTIONS),
        fallback: FallbackPolicy::AppDefault,
        supported: true,
    },
    SettingSpec {
        id: SettingId::ReduceMotion,
        entry_id: "A03",
        label: "Reduce motion",
        description: "Reduce UI motion by slowing spinners and disabling smooth chat scrolling.",
        file: SettingFile::LocalSettings,
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
        file: SettingFile::Preferences,
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
        description: "Controls whether Claude shows spinner tips in supported clients.",
        file: SettingFile::LocalSettings,
        json_path: &["spinnerTipsEnabled"],
        kind: SettingKind::Bool,
        editor: EditorKind::Toggle,
        source: ValueSource::PersistedOnly,
        options: SettingOptions::None,
        fallback: FallbackPolicy::AppDefault,
        supported: false,
    },
    SettingSpec {
        id: SettingId::TerminalProgressBar,
        entry_id: "A08",
        label: "Terminal progress bar",
        description: "Controls whether Claude should show its terminal progress bar in supported clients.",
        file: SettingFile::Preferences,
        json_path: &["terminalProgressBarEnabled"],
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
        description: "Controls the TUI color theme.",
        file: SettingFile::Preferences,
        json_path: &["theme"],
        kind: SettingKind::Enum,
        editor: EditorKind::Cycle,
        source: ValueSource::PersistedOnly,
        options: SettingOptions::Static(THEME_OPTIONS),
        fallback: FallbackPolicy::AppDefault,
        supported: false,
    },
    SettingSpec {
        id: SettingId::ThinkingEffort,
        entry_id: "A20",
        label: "Thinking effort",
        description: "Controls how much effort Claude uses when thinking for new sessions: Low, Medium, High, or XHigh. Only applies when Always Thinking is on and the selected model supports effort.",
        file: SettingFile::Settings,
        json_path: &["effortLevel"],
        kind: SettingKind::Enum,
        editor: EditorKind::Overlay,
        source: ValueSource::PersistedOnly,
        options: SettingOptions::None,
        fallback: FallbackPolicy::AppDefault,
        supported: true,
    },
];

#[must_use]
pub const fn setting_specs() -> &'static [SettingSpec] {
    &CONFIG_SETTINGS
}

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
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSetting {
    pub value: ResolvedSettingValue,
    pub validation: SettingValidation,
}

pub fn setting_spec(id: SettingId) -> &'static SettingSpec {
    &CONFIG_SETTINGS[id as usize]
}

#[must_use]
pub fn resolved_setting(app: &App, spec: &SettingSpec) -> ResolvedSetting {
    let document = app.config.document_for(spec.file);
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
        (ResolvedSettingValue::Text(value), _) => {
            if value.is_empty() {
                "Not set".to_owned()
            } else {
                value.clone()
            }
        }
        (ResolvedSettingValue::Choice(ResolvedChoice::Automatic), SettingId::Model) => {
            DEFAULT_MODEL_ALIAS_LABEL.to_owned()
        }
        (ResolvedSettingValue::Choice(ResolvedChoice::Stored(value)), SettingId::Model) => {
            model_status_label(Some(value), app)
        }
        (
            ResolvedSettingValue::Choice(ResolvedChoice::Stored(value)),
            SettingId::ThinkingEffort,
        ) => effort_level_label(value).unwrap_or_else(|| value.clone()),
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
        SettingKind::Text => Vec::new(),
        SettingKind::Enum | SettingKind::DynamicEnum if spec.id == SettingId::ThinkingEffort => {
            EffortLevel::PERSISTABLE_SETTINGS.iter().map(|level| level.label().to_owned()).collect()
        }
        SettingKind::Enum | SettingKind::DynamicEnum => match spec.options {
            SettingOptions::None => Vec::new(),
            SettingOptions::Static(options) => {
                options.iter().map(|option| option.label.to_owned()).collect()
            }
            SettingOptions::RuntimeCatalog(RuntimeCatalogKind::Models) => {
                if app.available_models.is_empty() {
                    vec![
                        DEFAULT_MODEL_ALIAS_LABEL.to_owned(),
                        "Connect to load available models".to_owned(),
                    ]
                } else {
                    model_overlay_options(app)
                        .into_iter()
                        .map(|option| option.display_name)
                        .collect()
                }
            }
            SettingOptions::RuntimeCatalog(RuntimeCatalogKind::PermissionModes) => {
                DEFAULT_PERMISSION_OPTIONS.iter().map(|option| option.label.to_owned()).collect()
            }
        },
    }
}

pub(super) fn effort_level_label(value: &str) -> Option<String> {
    EffortLevel::from_persisted_setting(value).map(|level| level.label().to_owned())
}

pub(super) fn project_root(app: &App) -> &std::path::Path {
    std::path::Path::new(&app.cwd_raw)
}

pub(super) fn option_label(spec: &SettingSpec, value: &str) -> Option<String> {
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
