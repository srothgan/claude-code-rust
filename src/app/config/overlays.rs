// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::mcp::{
    McpAuthRedirectOverlayState, McpCallbackUrlOverlayState, McpDetailsOverlayState,
    McpElicitationOverlayState,
};
use super::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelOverlayState {
    pub selected_model: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThinkingEffortOverlayState {
    pub selected_effort: EffortLevel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputStyleOverlayState {
    pub selected: OutputStyle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageOverlayState {
    pub draft: String,
    pub cursor: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRenameOverlayState {
    pub draft: String,
    pub cursor: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayMessageKind {
    Info,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayMessage {
    pub kind: OverlayMessageKind,
    pub text: String,
}

impl OverlayMessage {
    #[must_use]
    pub fn info(text: impl Into<String>) -> Self {
        Self { kind: OverlayMessageKind::Info, text: text.into() }
    }

    #[must_use]
    pub fn error(text: impl Into<String>) -> Self {
        Self { kind: OverlayMessageKind::Error, text: text.into() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarketplaceActionKind {
    Update,
    Remove,
}

impl MarketplaceActionKind {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Update => "Update",
            Self::Remove => "Remove",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstalledPluginActionKind {
    Enable,
    Disable,
    Update,
    Uninstall,
}

impl InstalledPluginActionKind {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Enable => "Enable",
            Self::Disable => "Disable",
            Self::Update => "Update",
            Self::Uninstall => "Uninstall",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginInstallActionKind {
    User,
    Project,
    Local,
}

impl PluginInstallActionKind {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::User => "Install for user",
            Self::Project => "Install for project",
            Self::Local => "Install locally",
        }
    }

    #[must_use]
    pub const fn scope(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
            Self::Local => "local",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledPluginActionOverlayState {
    pub plugin_id: String,
    pub title: String,
    pub description: String,
    pub scope: String,
    pub project_path: Option<String>,
    pub selected_index: usize,
    pub actions: Vec<InstalledPluginActionKind>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginInstallOverlayState {
    pub plugin_id: String,
    pub title: String,
    pub description: String,
    pub selected_index: usize,
    pub actions: Vec<PluginInstallActionKind>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketplaceActionsOverlayState {
    pub name: String,
    pub title: String,
    pub description: String,
    pub selected_index: usize,
    pub actions: Vec<MarketplaceActionKind>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddMarketplaceOverlayState {
    pub draft: String,
    pub cursor: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmationAction {
    InstalledPluginUninstall,
    MarketplaceRemove,
    McpClearAuth,
    McpRemoveConfig,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConfirmationOverlayState {
    pub title: String,
    pub body: String,
    pub confirm_label: String,
    pub cancel_label: String,
    pub selected_index: usize,
    pub action: ConfirmationAction,
    pub previous: Box<ConfigOverlayState>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConfigOverlayState {
    Model(ModelOverlayState),
    ThinkingEffort(ThinkingEffortOverlayState),
    OutputStyle(OutputStyleOverlayState),
    Language(LanguageOverlayState),
    SessionRename(SessionRenameOverlayState),
    InstalledPluginActions(InstalledPluginActionOverlayState),
    PluginInstallActions(PluginInstallOverlayState),
    MarketplaceActions(MarketplaceActionsOverlayState),
    AddMarketplace(AddMarketplaceOverlayState),
    McpDetails(McpDetailsOverlayState),
    McpCallbackUrl(McpCallbackUrlOverlayState),
    McpElicitation(McpElicitationOverlayState),
    McpAuthRedirect(McpAuthRedirectOverlayState),
    Confirmation(ConfirmationOverlayState),
}
