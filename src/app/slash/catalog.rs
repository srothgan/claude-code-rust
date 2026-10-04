// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

//! Static metadata for app-owned slash commands.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AppSlashCommand {
    Btw,
    Cancel,
    Compact,
    Config,
    Memory,
    Permissions,
    Sandbox,
    Hooks,

    Copy,
    Docs,
    Agent,
    Effort,
    Thinking,
    Ultracode,
    Fast,
    Help,
    Mcp,
    Plugins,
    Status,
    Usage,
    Login,
    Logout,
    Mode,
    Model,
    NewSession,
    Resume,
    Rewind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SubmissionClass {
    Invalid,
    TurnControl,
    Fullscreen,
    Informational,
    TurnExclusive,
}

impl SubmissionClass {
    pub(crate) const fn requires_idle_turn(self) -> bool {
        match self {
            Self::TurnExclusive => true,
            Self::Invalid | Self::TurnControl | Self::Fullscreen | Self::Informational => false,
        }
    }
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

pub(crate) const APP_SLASH_COMMANDS: &[AppSlashCommandSpec] = &[
    AppSlashCommandSpec {
        command: AppSlashCommand::Btw,
        name: "/btw",
        usage: "Usage: /btw <question>",
        short_description: "Ask a contextual side question",
        long_description: "Ask one contextual question without adding it to the main conversation.",
        args: NO_ARGS,
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
        command: AppSlashCommand::Memory,
        name: "/memory",
        usage: "Usage: /memory",
        short_description: "Open memory settings",
        long_description: "Open the memory pane of Settings without changing session choices.",
        args: NO_ARGS,
    },
    AppSlashCommandSpec {
        command: AppSlashCommand::Permissions,
        name: "/permissions",
        usage: "Usage: /permissions",
        short_description: "Open permissions settings",
        long_description: "Open the permissions pane of Settings without changing session choices.",
        args: NO_ARGS,
    },
    AppSlashCommandSpec {
        command: AppSlashCommand::Sandbox,
        name: "/sandbox",
        usage: "Usage: /sandbox",
        short_description: "Open sandbox settings",
        long_description: "Open the sandbox pane of Settings without changing session choices.",
        args: NO_ARGS,
    },
    AppSlashCommandSpec {
        command: AppSlashCommand::Hooks,
        name: "/hooks",
        usage: "Usage: /hooks",
        short_description: "Open hooks settings",
        long_description: "Open the hooks pane of Settings without changing session choices.",
        args: NO_ARGS,
    },
    AppSlashCommandSpec {
        command: AppSlashCommand::Copy,
        name: "/copy",
        usage: "Usage: /copy",
        short_description: "Copy last response",
        long_description: "Copy the last finished assistant response or choose a code block. Configure Skip the /copy picker to copy the full response directly.",
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
        command: AppSlashCommand::Agent,
        name: "/agent",
        usage: "Usage: /agent <name|reset>",
        short_description: "Set session agent",
        long_description: "Change the main-thread agent for the active session. Applies on the next turn.",
        args: NO_ARGS,
    },
    AppSlashCommandSpec {
        command: AppSlashCommand::Effort,
        name: "/effort",
        usage: "Usage: /effort <low|medium|high|xhigh|max|reset>",
        short_description: "Set session effort",
        long_description: "Change effort for the current session; reset uses the model's default. Use /config for saved defaults. Max is session-only.",
        args: NO_ARGS,
    },
    AppSlashCommandSpec {
        command: AppSlashCommand::Thinking,
        name: "/thinking",
        usage: "Usage: /thinking <on|off|reset>",
        short_description: "Set session thinking preference",
        long_description: "Change the thinking preference for this session. Model restrictions still apply; reset uses saved defaults.",
        args: &[
            SlashArgSpec { value: "on", description: "Prefer thinking for this session" },
            SlashArgSpec { value: "off", description: "Disable thinking where supported" },
            SlashArgSpec { value: "reset", description: "Use the saved thinking preference" },
        ],
    },
    AppSlashCommandSpec {
        command: AppSlashCommand::Ultracode,
        name: "/ultracode",
        usage: "Usage: /ultracode <on|off|status>",
        short_description: "Control session Ultracode",
        long_description: "Enable, disable, or inspect verified Ultracode state for the active session. Retains thinking effort.",
        args: &[
            SlashArgSpec { value: "on", description: "Enable session Ultracode" },
            SlashArgSpec { value: "off", description: "Disable session Ultracode" },
            SlashArgSpec { value: "status", description: "Show verified session Ultracode state" },
        ],
    },
    AppSlashCommandSpec {
        command: AppSlashCommand::Fast,
        name: "/fast",
        usage: "Usage: /fast [on|off]",
        short_description: "Toggle session fast mode",
        long_description: "Enable or disable fast mode for the active session.",
        args: &[
            SlashArgSpec { value: "on", description: "Enable fast mode for this session" },
            SlashArgSpec { value: "off", description: "Disable fast mode for this session" },
        ],
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
        usage: "Usage: /resume [session_id]",
        short_description: "Resume a session by ID",
        long_description: "Open the session picker, or resume a manually supplied session ID.",
        args: NO_ARGS,
    },
    AppSlashCommandSpec {
        command: AppSlashCommand::Rewind,
        name: "/rewind",
        usage: "Usage: /rewind <user_message_uuid> <both|conversation|code>",
        short_description: "Restore conversation or code",
        long_description: "Restore conversation, code, or both to a previous user message.",
        args: NO_ARGS,
    },
];

impl AppSlashCommand {
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        command_spec(name).map(|spec| spec.command)
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Btw => "/btw",
            Self::Cancel => "/cancel",
            Self::Compact => "/compact",
            Self::Config => "/config",
            Self::Memory => "/memory",
            Self::Permissions => "/permissions",
            Self::Sandbox => "/sandbox",
            Self::Hooks => "/hooks",

            Self::Copy => "/copy",
            Self::Docs => "/docs",
            Self::Agent => "/agent",
            Self::Effort => "/effort",
            Self::Thinking => "/thinking",
            Self::Ultracode => "/ultracode",
            Self::Fast => "/fast",
            Self::Help => "/help",
            Self::Mcp => "/mcp",
            Self::Plugins => "/plugins",
            Self::Status => "/status",
            Self::Usage => "/usage",
            Self::Login => "/login",
            Self::Logout => "/logout",
            Self::Mode => "/mode",
            Self::Model => "/model",
            Self::NewSession => "/new-session",
            Self::Resume => "/resume",
            Self::Rewind => "/rewind",
        }
    }

    pub(crate) fn usage(self) -> &'static str {
        command_spec(self.name()).map_or(self.name(), |spec| spec.usage)
    }

    pub(crate) fn submission_class(self, args: &[&str]) -> SubmissionClass {
        match self {
            Self::Btw => {
                if args.is_empty() {
                    SubmissionClass::Invalid
                } else {
                    SubmissionClass::Informational
                }
            }
            Self::Cancel => {
                if args.is_empty() {
                    SubmissionClass::TurnControl
                } else {
                    SubmissionClass::Invalid
                }
            }
            Self::Config
            | Self::Memory
            | Self::Permissions
            | Self::Sandbox
            | Self::Hooks
            | Self::Help
            | Self::Mcp
            | Self::Plugins
            | Self::Status
            | Self::Usage => {
                if args.is_empty() {
                    SubmissionClass::Fullscreen
                } else {
                    SubmissionClass::Invalid
                }
            }
            Self::Copy => {
                if args.is_empty() {
                    SubmissionClass::Informational
                } else {
                    SubmissionClass::Invalid
                }
            }
            Self::Docs => {
                if matches!(args, ["mode" | "models" | "shortcuts" | "commands" | "agents"]) {
                    SubmissionClass::Informational
                } else {
                    SubmissionClass::Invalid
                }
            }
            Self::Compact | Self::Login | Self::Logout | Self::NewSession => {
                if args.is_empty() {
                    SubmissionClass::TurnExclusive
                } else {
                    SubmissionClass::Invalid
                }
            }
            Self::Fast => {
                if args.is_empty() || matches!(args, ["on" | "off"]) {
                    SubmissionClass::TurnExclusive
                } else {
                    SubmissionClass::Invalid
                }
            }
            Self::Thinking => {
                if matches!(args, ["on" | "off" | "reset"]) {
                    SubmissionClass::TurnExclusive
                } else {
                    SubmissionClass::Invalid
                }
            }
            Self::Agent | Self::Mode | Self::Model => {
                if matches!(args, [_]) {
                    SubmissionClass::TurnExclusive
                } else {
                    SubmissionClass::Invalid
                }
            }
            Self::Ultracode => match args {
                ["status"] => SubmissionClass::Informational,
                ["on" | "off"] => SubmissionClass::TurnExclusive,
                _ => SubmissionClass::Invalid,
            },
            Self::Effort => {
                if matches!(args, ["low" | "medium" | "high" | "xhigh" | "max" | "reset"]) {
                    SubmissionClass::TurnExclusive
                } else {
                    SubmissionClass::Invalid
                }
            }
            Self::Resume => {
                if args.len() <= 1 {
                    SubmissionClass::TurnExclusive
                } else {
                    SubmissionClass::Invalid
                }
            }
            Self::Rewind => {
                if matches!(args, [_, "both" | "conversation" | "code"]) {
                    SubmissionClass::TurnExclusive
                } else {
                    SubmissionClass::Invalid
                }
            }
        }
    }
}

pub(crate) fn command_spec(name: &str) -> Option<&'static AppSlashCommandSpec> {
    APP_SLASH_COMMANDS.iter().find(|spec| spec.name == name)
}
