// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigTab {
    Settings,
    Plugins,
    Status,
    Usage,
    Mcp,
    Help,
}

impl ConfigTab {
    pub const ALL: [Self; 6] =
        [Self::Settings, Self::Plugins, Self::Status, Self::Usage, Self::Mcp, Self::Help];

    pub const fn title(self) -> &'static str {
        match self {
            Self::Settings => "Settings",
            Self::Plugins => "Plugins",
            Self::Status => "Status",
            Self::Usage => "Usage",
            Self::Mcp => "MCP",
            Self::Help => "Help",
        }
    }

    pub(super) const fn next(self) -> Self {
        match self {
            Self::Settings => Self::Plugins,
            Self::Plugins => Self::Status,
            Self::Status => Self::Usage,
            Self::Usage => Self::Mcp,
            Self::Mcp => Self::Help,
            Self::Help => Self::Settings,
        }
    }

    pub(super) const fn prev(self) -> Self {
        match self {
            Self::Settings => Self::Help,
            Self::Plugins => Self::Settings,
            Self::Status => Self::Plugins,
            Self::Usage => Self::Status,
            Self::Mcp => Self::Usage,
            Self::Help => Self::Mcp,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConfigHelpSection {
    #[default]
    Shortcuts,
    Commands,
    Subagents,
}

impl ConfigHelpSection {
    pub const ALL: [Self; 3] = [Self::Shortcuts, Self::Commands, Self::Subagents];

    pub const fn title(self) -> &'static str {
        match self {
            Self::Shortcuts => "Shortcuts",
            Self::Commands => "Commands",
            Self::Subagents => "Subagents",
        }
    }

    pub(super) const fn next(self) -> Self {
        match self {
            Self::Shortcuts => Self::Commands,
            Self::Commands => Self::Subagents,
            Self::Subagents => Self::Shortcuts,
        }
    }

    pub(super) const fn prev(self) -> Self {
        match self {
            Self::Shortcuts => Self::Subagents,
            Self::Commands => Self::Shortcuts,
            Self::Subagents => Self::Commands,
        }
    }
}
