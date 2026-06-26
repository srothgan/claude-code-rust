// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum McpServerActionKind {
    RefreshSnapshot,
    Authenticate,
    ClearAuth,
    Reconnect,
    Enable,
    Disable,
    ManagePlugin,
    RemoveUserConfig,
    RemoveLocalConfig,
    RemoveProjectConfig,
    RemoveDynamicConfig,
}

impl McpServerActionKind {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::RefreshSnapshot => "Refresh",
            Self::Authenticate => "Authenticate",
            Self::ClearAuth => "Clear auth",
            Self::Reconnect => "Reconnect server",
            Self::Enable => "Enable server",
            Self::Disable => "Disable server",
            Self::ManagePlugin => "Manage plugin",
            Self::RemoveUserConfig
            | Self::RemoveLocalConfig
            | Self::RemoveProjectConfig
            | Self::RemoveDynamicConfig => "Remove",
        }
    }

    #[must_use]
    pub const fn mcp_config_scope(self) -> Option<McpConfigScope> {
        match self {
            Self::RemoveUserConfig => Some(McpConfigScope::User),
            Self::RemoveLocalConfig => Some(McpConfigScope::Local),
            Self::RemoveProjectConfig => Some(McpConfigScope::Project),
            Self::RemoveDynamicConfig => Some(McpConfigScope::Dynamic),
            Self::RefreshSnapshot
            | Self::Authenticate
            | Self::ClearAuth
            | Self::Reconnect
            | Self::Enable
            | Self::Disable
            | Self::ManagePlugin => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum McpConfigScope {
    Local,
    User,
    Project,
    Dynamic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum McpServerOwnership<'a> {
    Persisted(McpConfigScope),
    SdkDynamic,
    PluginOwned(&'a InstalledPluginEntry),
    PluginOwnedUnknown,
    RuntimeOnly,
}

impl McpConfigScope {
    #[must_use]
    pub const fn cli_arg(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::User => "user",
            Self::Project => "project",
            Self::Dynamic => "dynamic",
        }
    }

    #[must_use]
    pub fn from_status_scope(scope: &str) -> Option<Self> {
        match scope.trim() {
            scope if scope.eq_ignore_ascii_case("user") => Some(Self::User),
            scope if scope.eq_ignore_ascii_case("local") => Some(Self::Local),
            scope if scope.eq_ignore_ascii_case("project") => Some(Self::Project),
            scope if scope.eq_ignore_ascii_case("dynamic") => Some(Self::Dynamic),
            _ => None,
        }
    }
}

#[must_use]
pub(crate) const fn remove_action_for_mcp_config_scope(
    scope: McpConfigScope,
) -> McpServerActionKind {
    match scope {
        McpConfigScope::User => McpServerActionKind::RemoveUserConfig,
        McpConfigScope::Local => McpServerActionKind::RemoveLocalConfig,
        McpConfigScope::Project => McpServerActionKind::RemoveProjectConfig,
        McpConfigScope::Dynamic => McpServerActionKind::RemoveDynamicConfig,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PendingDynamicMcpRemovalConfirmation {
    Confirmed { server_name: String },
    Failed { server_name: String, message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct McpConfigRemoveConfirmationFailure {
    pub(super) server_name: String,
    pub(super) scope: String,
    pub(super) message: String,
}
