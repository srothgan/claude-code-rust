// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use super::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingSessionTitleChangeKind {
    Rename { requested_title: Option<String> },
    Generate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingSessionTitleChangeState {
    pub session_id: String,
    pub kind: PendingSessionTitleChangeKind,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConfigState {
    pub active_tab: ConfigTab,
    pub selected_setting_index: usize,
    pub settings_scroll_offset: usize,
    pub mcp_selected_server_index: usize,
    pub help_section: ConfigHelpSection,
    pub help_dialog: DialogState,
    pub help_visible_count: usize,
    pub overlay: Option<ConfigOverlayState>,
    pub committed_settings_document: Value,
    pub committed_local_settings_document: Value,
    pub committed_preferences_document: Value,
    pub settings_path: Option<PathBuf>,
    pub local_settings_path: Option<PathBuf>,
    pub preferences_path: Option<PathBuf>,
    pub status_message: Option<String>,
    pub last_error: Option<String>,
    pub overlay_message: Option<OverlayMessage>,
    pub pending_session_title_change: Option<PendingSessionTitleChangeState>,
}

impl Default for ConfigState {
    fn default() -> Self {
        Self {
            active_tab: ConfigTab::Settings,
            selected_setting_index: 0,
            settings_scroll_offset: 0,
            mcp_selected_server_index: 0,
            help_section: ConfigHelpSection::default(),
            help_dialog: DialogState::default(),
            help_visible_count: 0,
            overlay: None,
            committed_settings_document: Value::Object(serde_json::Map::new()),
            committed_local_settings_document: Value::Object(serde_json::Map::new()),
            committed_preferences_document: Value::Object(serde_json::Map::new()),
            settings_path: None,
            local_settings_path: None,
            preferences_path: None,
            status_message: None,
            last_error: None,
            overlay_message: None,
            pending_session_title_change: None,
        }
    }
}

impl ConfigState {
    #[must_use]
    pub fn fast_mode_effective(&self) -> bool {
        match resolve_setting_document(&self.committed_settings_document, SettingId::FastMode, &[])
            .value
        {
            ResolvedSettingValue::Bool(value) => value,
            ResolvedSettingValue::Choice(_) | ResolvedSettingValue::Text(_) => false,
        }
    }

    #[must_use]
    pub fn always_thinking_effective(&self) -> bool {
        match resolve_setting_document(
            &self.committed_settings_document,
            SettingId::AlwaysThinking,
            &[],
        )
        .value
        {
            ResolvedSettingValue::Bool(value) => value,
            ResolvedSettingValue::Choice(_) | ResolvedSettingValue::Text(_) => false,
        }
    }

    #[must_use]
    pub fn model_effective(&self) -> Option<String> {
        match resolve_setting_document(&self.committed_settings_document, SettingId::Model, &[])
            .value
        {
            ResolvedSettingValue::Choice(ResolvedChoice::Automatic) => {
                Some(DEFAULT_MODEL_ALIAS_ID.to_owned())
            }
            ResolvedSettingValue::Choice(ResolvedChoice::Stored(value)) => Some(value),
            ResolvedSettingValue::Bool(_) | ResolvedSettingValue::Text(_) => None,
        }
    }

    #[must_use]
    pub fn thinking_effort_effective(&self) -> EffortLevel {
        store::thinking_effort_level(&self.committed_settings_document)
            .unwrap_or(EffortLevel::Medium)
    }

    #[must_use]
    pub fn default_permission_mode_effective(&self) -> DefaultPermissionMode {
        match resolve_setting_document(
            &self.committed_settings_document,
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
    pub fn output_style_effective(&self) -> OutputStyle {
        store::output_style(&self.committed_local_settings_document).unwrap_or_default()
    }

    #[must_use]
    pub fn selected_setting_spec(&self) -> Option<&'static SettingSpec> {
        setting_specs().get(self.selected_setting_index)
    }

    pub fn replace_overlay(&mut self, overlay: ConfigOverlayState) {
        self.overlay = Some(overlay);
        self.overlay_message = None;
    }

    pub fn clear_overlay(&mut self) {
        self.overlay = None;
        self.overlay_message = None;
    }

    pub fn set_overlay_info(&mut self, message: impl Into<String>) {
        self.overlay_message = Some(OverlayMessage::info(message));
        self.last_error = None;
        self.status_message = None;
    }

    pub fn set_overlay_error(&mut self, message: impl Into<String>) {
        self.overlay_message = Some(OverlayMessage::error(message));
        self.last_error = None;
        self.status_message = None;
    }

    #[must_use]
    pub fn model_overlay(&self) -> Option<&ModelOverlayState> {
        if let Some(ConfigOverlayState::Model(overlay)) = &self.overlay {
            Some(overlay)
        } else {
            None
        }
    }

    pub fn model_overlay_mut(&mut self) -> Option<&mut ModelOverlayState> {
        if let Some(ConfigOverlayState::Model(overlay)) = &mut self.overlay {
            Some(overlay)
        } else {
            None
        }
    }

    #[must_use]
    pub fn thinking_effort_overlay(&self) -> Option<&ThinkingEffortOverlayState> {
        if let Some(ConfigOverlayState::ThinkingEffort(overlay)) = &self.overlay {
            Some(overlay)
        } else {
            None
        }
    }

    pub fn thinking_effort_overlay_mut(&mut self) -> Option<&mut ThinkingEffortOverlayState> {
        if let Some(ConfigOverlayState::ThinkingEffort(overlay)) = &mut self.overlay {
            Some(overlay)
        } else {
            None
        }
    }

    #[must_use]
    pub fn output_style_overlay(&self) -> Option<&OutputStyleOverlayState> {
        if let Some(ConfigOverlayState::OutputStyle(overlay)) = &self.overlay {
            Some(overlay)
        } else {
            None
        }
    }

    pub fn output_style_overlay_mut(&mut self) -> Option<&mut OutputStyleOverlayState> {
        if let Some(ConfigOverlayState::OutputStyle(overlay)) = &mut self.overlay {
            Some(overlay)
        } else {
            None
        }
    }

    #[must_use]
    pub fn language_overlay(&self) -> Option<&LanguageOverlayState> {
        if let Some(ConfigOverlayState::Language(overlay)) = &self.overlay {
            Some(overlay)
        } else {
            None
        }
    }

    pub fn language_overlay_mut(&mut self) -> Option<&mut LanguageOverlayState> {
        if let Some(ConfigOverlayState::Language(overlay)) = &mut self.overlay {
            Some(overlay)
        } else {
            None
        }
    }

    #[must_use]
    pub fn session_rename_overlay(&self) -> Option<&SessionRenameOverlayState> {
        if let Some(ConfigOverlayState::SessionRename(overlay)) = &self.overlay {
            Some(overlay)
        } else {
            None
        }
    }

    pub fn session_rename_overlay_mut(&mut self) -> Option<&mut SessionRenameOverlayState> {
        if let Some(ConfigOverlayState::SessionRename(overlay)) = &mut self.overlay {
            Some(overlay)
        } else {
            None
        }
    }

    #[must_use]
    pub fn installed_plugin_actions_overlay(&self) -> Option<&InstalledPluginActionOverlayState> {
        if let Some(ConfigOverlayState::InstalledPluginActions(overlay)) = &self.overlay {
            Some(overlay)
        } else {
            None
        }
    }

    pub fn installed_plugin_actions_overlay_mut(
        &mut self,
    ) -> Option<&mut InstalledPluginActionOverlayState> {
        if let Some(ConfigOverlayState::InstalledPluginActions(overlay)) = &mut self.overlay {
            Some(overlay)
        } else {
            None
        }
    }

    #[must_use]
    pub fn plugin_install_overlay(&self) -> Option<&PluginInstallOverlayState> {
        if let Some(ConfigOverlayState::PluginInstallActions(overlay)) = &self.overlay {
            Some(overlay)
        } else {
            None
        }
    }

    pub fn plugin_install_overlay_mut(&mut self) -> Option<&mut PluginInstallOverlayState> {
        if let Some(ConfigOverlayState::PluginInstallActions(overlay)) = &mut self.overlay {
            Some(overlay)
        } else {
            None
        }
    }

    #[must_use]
    pub fn marketplace_actions_overlay(&self) -> Option<&MarketplaceActionsOverlayState> {
        if let Some(ConfigOverlayState::MarketplaceActions(overlay)) = &self.overlay {
            Some(overlay)
        } else {
            None
        }
    }

    pub fn marketplace_actions_overlay_mut(
        &mut self,
    ) -> Option<&mut MarketplaceActionsOverlayState> {
        if let Some(ConfigOverlayState::MarketplaceActions(overlay)) = &mut self.overlay {
            Some(overlay)
        } else {
            None
        }
    }

    #[must_use]
    pub fn add_marketplace_overlay(&self) -> Option<&AddMarketplaceOverlayState> {
        if let Some(ConfigOverlayState::AddMarketplace(overlay)) = &self.overlay {
            Some(overlay)
        } else {
            None
        }
    }

    pub fn add_marketplace_overlay_mut(&mut self) -> Option<&mut AddMarketplaceOverlayState> {
        if let Some(ConfigOverlayState::AddMarketplace(overlay)) = &mut self.overlay {
            Some(overlay)
        } else {
            None
        }
    }

    #[must_use]
    pub fn confirmation_overlay(&self) -> Option<&ConfirmationOverlayState> {
        if let Some(ConfigOverlayState::Confirmation(overlay)) = &self.overlay {
            Some(overlay)
        } else {
            None
        }
    }

    pub fn confirmation_overlay_mut(&mut self) -> Option<&mut ConfirmationOverlayState> {
        if let Some(ConfigOverlayState::Confirmation(overlay)) = &mut self.overlay {
            Some(overlay)
        } else {
            None
        }
    }

    #[must_use]
    pub fn path_for(&self, file: SettingFile) -> Option<&PathBuf> {
        match file {
            SettingFile::Settings => self.settings_path.as_ref(),
            SettingFile::LocalSettings => self.local_settings_path.as_ref(),
            SettingFile::Preferences => self.preferences_path.as_ref(),
        }
    }

    #[must_use]
    pub fn document_for(&self, file: SettingFile) -> &Value {
        match file {
            SettingFile::Settings => &self.committed_settings_document,
            SettingFile::LocalSettings => &self.committed_local_settings_document,
            SettingFile::Preferences => &self.committed_preferences_document,
        }
    }

    pub fn committed_document_for_mut(&mut self, file: SettingFile) -> &mut Value {
        match file {
            SettingFile::Settings => &mut self.committed_settings_document,
            SettingFile::LocalSettings => &mut self.committed_local_settings_document,
            SettingFile::Preferences => &mut self.committed_preferences_document,
        }
    }

    pub(super) fn apply_loaded(
        &mut self,
        loaded: store::LoadedSettingsDocuments,
        notice: Option<String>,
        preserve_status: bool,
    ) {
        self.settings_path = Some(loaded.paths.settings);
        self.local_settings_path = Some(loaded.paths.local_settings);
        self.preferences_path = Some(loaded.paths.preferences);
        self.committed_settings_document = loaded.settings_document;
        self.committed_local_settings_document = loaded.local_settings_document;
        self.committed_preferences_document = loaded.preferences_document;
        self.overlay = None;
        self.overlay_message = None;
        self.selected_setting_index =
            self.selected_setting_index.min(setting_specs().len().saturating_sub(1));
        self.settings_scroll_offset = self.settings_scroll_offset.min(self.selected_setting_index);
        self.mcp_selected_server_index = 0;
        if !preserve_status {
            self.status_message = notice;
            self.last_error = None;
        } else if let Some(notice) = notice {
            self.status_message = Some(notice);
        }
    }
}
