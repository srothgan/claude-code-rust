// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use super::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsEditTarget {
    id: String,
    scope: crate::agent::settings::SettingsScope,
    context: String,
    revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingSettingsRequest {
    Inspection(String),
    Mutation { request_id: String, target: Box<SettingsEditTarget> },
}
impl PendingSettingsRequest {
    pub fn mutation(
        request_id: String,
        mutation: &crate::agent::settings::SettingsMutation,
    ) -> Self {
        Self::Mutation {
            request_id,
            target: Box::new(SettingsEditTarget {
                id: mutation.id.clone(),
                scope: mutation.scope,
                context: mutation.context.clone(),
                revision: mutation.expected_revision.clone(),
            }),
        }
    }
    pub fn request_id(&self) -> &str {
        match self {
            Self::Inspection(id) | Self::Mutation { request_id: id, .. } => id,
        }
    }
    pub fn owns_editor(&self, editor: &SettingOverlayState) -> bool {
        matches!(self, Self::Mutation { target, .. } if target.id == editor.setting.id && target.scope == editor.scope && target.context == editor.context && target.revision == editor.revision)
    }
}

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
    pub settings: Box<super::SettingsBrowse>,
    pub mcp_selected_server_index: usize,
    pub help_section: ConfigHelpSection,
    pub help_dialog: DialogState,
    pub help_visible_count: usize,
    pub overlay: Option<ConfigOverlayState>,
    pub snapshot: Option<crate::agent::settings::SettingsSnapshot>,
    pub selected_scope: crate::agent::settings::SettingsScope,
    pub pending_settings_request: Option<PendingSettingsRequest>,
    pub status_message: Option<String>,
    pub last_error: Option<String>,
    pub overlay_message: Option<OverlayMessage>,
    pub pending_session_title_change: Option<PendingSessionTitleChangeState>,
}

impl Default for ConfigState {
    fn default() -> Self {
        Self {
            active_tab: ConfigTab::Settings,
            settings: Box::default(),
            mcp_selected_server_index: 0,
            help_section: ConfigHelpSection::default(),
            help_dialog: DialogState::default(),
            help_visible_count: 0,
            overlay: None,
            snapshot: None,
            selected_scope: crate::agent::settings::SettingsScope::User,
            pending_settings_request: None,
            status_message: None,
            last_error: None,
            overlay_message: None,
            pending_session_title_change: None,
        }
    }
}

impl ConfigState {
    pub fn saved_value(&self, id: &str) -> Option<&Value> {
        self.snapshot.as_ref()?.value(id)
    }
    fn saved_bool(&self, id: &str, default: bool) -> bool {
        self.saved_value(id).and_then(Value::as_bool).unwrap_or(default)
    }
    pub fn auto_scroll_effective(&self) -> bool {
        self.saved_bool("autoScrollEnabled", true)
    }
    pub fn show_turn_duration_effective(&self) -> bool {
        self.saved_bool("showTurnDuration", false)
    }
    pub fn show_message_timestamps_effective(&self) -> bool {
        self.saved_bool("showMessageTimestamps", false)
    }
    pub fn copy_full_response_effective(&self) -> bool {
        self.saved_bool("presentation.copyFullResponse", false)
    }
    pub fn status_in_terminal_tab_effective(&self) -> bool {
        self.saved_bool("presentation.showStatusInTerminalTab", true)
    }
    pub(crate) fn notification_enabled(&self, event: crate::app::notify::NotifyEvent) -> bool {
        use crate::app::notify::NotifyEvent;
        let (id, default) = match event {
            NotifyEvent::PermissionRequired | NotifyEvent::QuestionRequired => {
                ("notifications.actionsRequired", true)
            }
            NotifyEvent::TurnComplete => ("notifications.turnComplete", true),
            NotifyEvent::ModelDirected => ("notifications.modelDirected", true),
        };
        self.saved_bool(id, default)
    }
    pub fn time_format(&self) -> &str {
        self.saved_value("timeFormat").and_then(Value::as_str).unwrap_or("auto")
    }
    pub fn time_zone(&self) -> Option<&str> {
        self.snapshot.as_ref()?.time_zone.as_deref()
    }
    pub fn fast_mode_effective(&self) -> bool {
        self.saved_value("fastMode").and_then(Value::as_bool).unwrap_or(false)
    }
    pub fn respect_gitignore_effective(&self) -> bool {
        self.saved_value("respectGitignore").and_then(Value::as_bool).unwrap_or(true)
    }
    pub fn spinner_tips_enabled_effective(&self) -> bool {
        self.saved_bool("spinnerTipsEnabled", true)
    }
    pub fn prefers_reduced_motion_effective(&self) -> bool {
        self.saved_value("prefersReducedMotion").and_then(Value::as_bool).unwrap_or(false)
    }
    pub fn preferred_notification_channel_effective(
        &self,
    ) -> crate::app::notify::PreferredNotifChannel {
        self.saved_value("preferredNotifChannel")
            .and_then(Value::as_str)
            .and_then(crate::app::notify::PreferredNotifChannel::from_stored)
            .unwrap_or_default()
    }
    pub fn selected_setting(&self) -> Option<&crate::agent::settings::SettingDescriptor> {
        self.settings.selected(self.snapshot.as_ref()?)
    }
    pub fn setting_overlay(&self) -> Option<&SettingOverlayState> {
        if let Some(ConfigOverlayState::Setting(overlay)) = &self.overlay {
            Some(overlay)
        } else {
            None
        }
    }
    pub fn setting_overlay_mut(&mut self) -> Option<&mut SettingOverlayState> {
        if let Some(ConfigOverlayState::Setting(overlay)) = &mut self.overlay {
            Some(overlay)
        } else {
            None
        }
    }

    pub fn replace_overlay(&mut self, overlay: ConfigOverlayState) {
        self.overlay = Some(overlay);
        self.overlay_message = None;
    }

    pub fn clear_overlay(&mut self) {
        self.overlay = None;
        self.overlay_message = None;
    }

    pub fn invalidate_session(&mut self) {
        self.snapshot = None;
        self.pending_settings_request = None;
        self.pending_session_title_change = None;
        self.status_message = None;
        self.last_error = None;
        if self.setting_overlay().is_none() {
            self.clear_overlay();
        }
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
}
