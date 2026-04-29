// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

//! Static metadata for app-owned slash commands.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AppSlashCommand {
    OneMContext,
    Cancel,
    Compact,
    Config,
    Docs,
    Help,
    Mcp,
    Plugins,
    OpusVersion,
    Status,
    Usage,
    Login,
    Logout,
    Mode,
    Model,
    NewSession,
    Resume,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SlashArgSpec {
    pub value: &'static str,
    pub description: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AppSlashCommandSpec {
    pub command: AppSlashCommand,
    pub name: &'static str,
    pub usage: &'static str,
    pub short_description: &'static str,
    pub long_description: &'static str,
    pub args: &'static [SlashArgSpec],
}

const NO_ARGS: &[SlashArgSpec] = &[];

pub(crate) const DOCS_TOPICS: &[SlashArgSpec] = &[
    SlashArgSpec { value: "mode", description: "Show current and available session modes" },
    SlashArgSpec { value: "models", description: "Show advertised models and capabilities" },
    SlashArgSpec {
        value: "shortcuts",
        description: "Show live keyboard shortcuts for the current app state",
    },
    SlashArgSpec { value: "commands", description: "Show app and SDK slash commands" },
    SlashArgSpec { value: "agents", description: "Show advertised subagents" },
];

pub(crate) const ONE_M_CONTEXT_ARGS: &[SlashArgSpec] = &[
    SlashArgSpec {
        value: "disable",
        description: "Disable 1M context for future sessions in this folder",
    },
    SlashArgSpec {
        value: "enable",
        description: "Enable 1M context for future sessions in this folder",
    },
    SlashArgSpec {
        value: "status",
        description: "Show the current 1M context setting for this folder",
    },
];

pub(crate) const OPUS_VERSION_ARGS: &[SlashArgSpec] = &[
    SlashArgSpec { value: "4.5", description: "Claude Opus 4.5" },
    SlashArgSpec { value: "4.6", description: "Claude Opus 4.6" },
    SlashArgSpec { value: "4.7", description: "Claude Opus 4.7" },
    SlashArgSpec { value: "default", description: "Use Claude default Opus alias" },
    SlashArgSpec { value: "status", description: "Show current project-local Opus pin" },
];

pub(crate) const APP_SLASH_COMMANDS: &[AppSlashCommandSpec] = &[
    AppSlashCommandSpec {
        command: AppSlashCommand::OneMContext,
        name: "/1m-context",
        usage: "Usage: /1m-context <enable|disable|status>",
        short_description: "Manage 1M context for this folder",
        long_description: "Enable, disable, or inspect project-local 1M context settings for future sessions.",
        args: ONE_M_CONTEXT_ARGS,
    },
    AppSlashCommandSpec {
        command: AppSlashCommand::Cancel,
        name: "/cancel",
        usage: "Usage: /cancel",
        short_description: "Cancel active turn",
        long_description: "Cancel the currently thinking or running assistant turn.",
        args: NO_ARGS,
    },
    AppSlashCommandSpec {
        command: AppSlashCommand::Compact,
        name: "/compact",
        usage: "Usage: /compact",
        short_description: "Compact session context",
        long_description: "Ask the active session to compact its conversation context.",
        args: NO_ARGS,
    },
    AppSlashCommandSpec {
        command: AppSlashCommand::Config,
        name: "/config",
        usage: "Usage: /config",
        short_description: "Open settings",
        long_description: "Open the fullscreen settings tab.",
        args: NO_ARGS,
    },
    AppSlashCommandSpec {
        command: AppSlashCommand::Docs,
        name: "/docs",
        usage: "Usage: /docs <mode|models|shortcuts|commands|agents>",
        short_description: "Show in-chat help topics",
        long_description: "Render command, shortcut, model, mode, or subagent documentation into the chat.",
        args: DOCS_TOPICS,
    },
    AppSlashCommandSpec {
        command: AppSlashCommand::Help,
        name: "/help",
        usage: "Usage: /help",
        short_description: "Open help",
        long_description: "Open the fullscreen Help tab.",
        args: NO_ARGS,
    },
    AppSlashCommandSpec {
        command: AppSlashCommand::Mcp,
        name: "/mcp",
        usage: "Usage: /mcp",
        short_description: "Open MCP",
        long_description: "Open the fullscreen MCP status and authorization tab.",
        args: NO_ARGS,
    },
    AppSlashCommandSpec {
        command: AppSlashCommand::Plugins,
        name: "/plugins",
        usage: "Usage: /plugins",
        short_description: "Open plugins",
        long_description: "Open the fullscreen plugins tab.",
        args: NO_ARGS,
    },
    AppSlashCommandSpec {
        command: AppSlashCommand::OpusVersion,
        name: "/opus-version",
        usage: "Usage: /opus-version <4.5|4.6|4.7|default|status>",
        short_description: "Pin the Opus alias version for this folder",
        long_description: "Set, clear, or inspect the project-local Opus alias pin for future sessions.",
        args: OPUS_VERSION_ARGS,
    },
    AppSlashCommandSpec {
        command: AppSlashCommand::Status,
        name: "/status",
        usage: "Usage: /status",
        short_description: "Show session status",
        long_description: "Open the fullscreen status tab.",
        args: NO_ARGS,
    },
    AppSlashCommandSpec {
        command: AppSlashCommand::Usage,
        name: "/usage",
        usage: "Usage: /usage",
        short_description: "Open usage",
        long_description: "Open the fullscreen usage tab.",
        args: NO_ARGS,
    },
    AppSlashCommandSpec {
        command: AppSlashCommand::Login,
        name: "/login",
        usage: "Usage: /login",
        short_description: "Authenticate with Claude",
        long_description: "Run Claude CLI authentication and reconnect the session after login.",
        args: NO_ARGS,
    },
    AppSlashCommandSpec {
        command: AppSlashCommand::Logout,
        name: "/logout",
        usage: "Usage: /logout",
        short_description: "Sign out of Claude",
        long_description: "Run Claude CLI logout and clear the active authenticated session.",
        args: NO_ARGS,
    },
    AppSlashCommandSpec {
        command: AppSlashCommand::Mode,
        name: "/mode",
        usage: "Usage: /mode <id>",
        short_description: "Set session mode",
        long_description: "Switch to one of the modes advertised by the active session.",
        args: NO_ARGS,
    },
    AppSlashCommandSpec {
        command: AppSlashCommand::Model,
        name: "/model",
        usage: "Usage: /model <id>",
        short_description: "Set session model",
        long_description: "Switch to one of the models advertised by the active session.",
        args: NO_ARGS,
    },
    AppSlashCommandSpec {
        command: AppSlashCommand::NewSession,
        name: "/new-session",
        usage: "Usage: /new-session",
        short_description: "Start a fresh session",
        long_description: "Start a new bridge session in the current folder.",
        args: NO_ARGS,
    },
    AppSlashCommandSpec {
        command: AppSlashCommand::Resume,
        name: "/resume",
        usage: "Usage: /resume <session_id>",
        short_description: "Resume a session by ID",
        long_description: "Resume a recent or manually supplied session ID.",
        args: NO_ARGS,
    },
];

impl AppSlashCommand {
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        command_spec(name).map(|spec| spec.command)
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::OneMContext => "/1m-context",
            Self::Cancel => "/cancel",
            Self::Compact => "/compact",
            Self::Config => "/config",
            Self::Docs => "/docs",
            Self::Help => "/help",
            Self::Mcp => "/mcp",
            Self::Plugins => "/plugins",
            Self::OpusVersion => "/opus-version",
            Self::Status => "/status",
            Self::Usage => "/usage",
            Self::Login => "/login",
            Self::Logout => "/logout",
            Self::Mode => "/mode",
            Self::Model => "/model",
            Self::NewSession => "/new-session",
            Self::Resume => "/resume",
        }
    }

    pub(crate) fn usage(self) -> &'static str {
        command_spec(self.name()).map_or(self.name(), |spec| spec.usage)
    }
}

pub(crate) fn command_spec(name: &str) -> Option<&'static AppSlashCommandSpec> {
    APP_SLASH_COMMANDS.iter().find(|spec| spec.name == name)
}
