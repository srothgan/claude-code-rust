# About

Claude Code Rust (`claude-rs`) is a keyboard-driven terminal interface for Claude Code. It uses Anthropic's Agent SDK for conversations, tools and session history, and provides its own interface for reading output, reviewing changes and managing sessions.

Start with [Installation](installation.md) and [Your first session](usage.md). If you already use the app, go to [conversation tasks](conversations.md), [configuration](configure.md) or [troubleshooting](troubleshooting.md). Contributors should start with [Development](development.md).

## How It Renders

The normal conversation renders inline: completed output enters your terminal's scrollback, while the streaming reply, composer and footer stay in the live area. Terminal search, selection and copy behavior remain available according to your terminal's capabilities.

Fullscreen views such as `/diff`, `/agents`, `/config` and `/help` use the alternate screen while open and return to the inline conversation when closed. App controls are keyboard-driven; see [Keyboard shortcuts](shortcuts.md).

There are no speed or memory benchmarks against the stock Claude Code interface. The [architecture overview](architecture.md) explains the rendering boundary for contributors.

## Project Status

The project is pre-1.0 and under active development. Run `claude-rs --version` to identify your installed version and consult the [Changelog](changelog.md) for release changes. The website follows the repository's main branch, so it can describe changes newer than your installation.

The project is useful today, but the runtime still depends on the upstream Claude Agent SDK bridge. Startup readiness, authentication behavior, billing, model availability, and service limits are controlled by Anthropic.

## Claude CLI Dependencies

Core conversations, tools, file edits, permissions, and session history run through the bundled Agent SDK. The separately installed `claude` command is used for the following operations and must be available on `PATH` when you use them:

| Operation | CLI command | When it is needed |
| --- | --- | --- |
| `/login` and `/logout` | `claude auth login` and `claude auth logout` | Account sign-in and sign-out from the Rust TUI. An already authenticated SDK session does not invoke these commands for each turn. |
| Plugin inventory and management | `claude plugin ...` | Listing installed and available plugins; installing, enabling, disabling, updating, and uninstalling plugins; and managing marketplaces. Installed plugins execute through the SDK, and runtime reload uses the SDK. |
| Remove a persisted MCP server | `claude mcp remove --scope <scope> <server>` | Removing user-, project-, or local-scope server configuration. Dynamic session servers are removed through the SDK. |
| Usage data from the CLI | `claude /usage --allowed-tools ""` | When the CLI usage source is selected, or an eligible fallback reaches it. The default source first tries the connected SDK session, then OAuth; only eligible OAuth failures fall back to the CLI. |

This list was audited against Agent SDK `0.3.286` on October 2, 2026, by checking the application's CLI process launches and usage-source selection. Installing `claude` provides these operations; it does not add the stock terminal interface's other screens or controls to the Rust TUI.

## Feature Overview

The Rust TUI implements its own interface and uses the SDK's session capabilities. It does not reproduce every stock Claude Code interface feature.

| Feature | Availability | Details |
| --- | --- | --- |
| Conversations, tools, file edits, and permissions | Rust TUI and SDK | Core interactive workflow. |
| Model, effort, permission mode, and agent selection | Rust TUI and SDK | Available through [startup flags](cli.md) and session controls. |
| Resume, continue, compaction, and rewind | Rust TUI and SDK | Session startup and history controls; see [conversation tasks](conversations.md) and [Slash Commands](commands.md). |
| Subagents and background tasks within a session | Rust TUI and SDK | SDK task activity and results are displayed in the conversation. This is distinct from detaching the entire session. |
| Skills and SDK-advertised slash commands | SDK, when advertised | Use `/docs commands` for the current session's merged list. |
| MCP status, authorization, and dynamic servers | Rust TUI and SDK | Persisted server removal additionally needs the CLI, as listed above. |
| Inline edit diffs | Rust TUI | Tool output displays file changes. |
| Git change review | Rust TUI | `/diff` opens a fullscreen, read-only viewer; see [Reviewing Git changes](conversations.md#reviewing-git-changes). It does not open beside the conversation like the stock fullscreen diff panel. |
| Shell completions and man pages | Native CLI | Generated from the CLI definitions; see [CLI reference](cli.md#shell-completions). |
| Account login and logout | Installed Claude CLI | Invoked from `/login` and `/logout`. |
| Plugin inventory and persistent management | Installed Claude CLI | Invoked from the plugin management interface. |
| Session-only local plugin folders | Rust TUI and SDK | Load local folder in `/plugins`; no persistent installation. |
| Usage information | SDK, OAuth, or installed Claude CLI | The CLI is conditional; see the dependency table above. |
| Whole-session background detach/attach and stock agent view | Unavailable in the Rust TUI | SDK background tasks are supported, but the stock `/background` and `claude agents` interface is not provided. |
| Remote Control and stock IDE integrations | Unavailable in the Rust TUI | No equivalent to the stock `/remote-control` or `/ide` integration. Running in an IDE terminal still works. |
| Vim input mode | Unavailable in the Rust TUI | The settings surface does not offer an editor-mode preference; see [unavailable settings](settings.md#unavailable-settings). |
| Stock `/tui` renderer switch | Unavailable in the Rust TUI | Rendering and fullscreen settings screens are owned by the Rust application. |
| Mouse interaction with app controls | Unavailable in the Rust TUI | Use keyboard controls. Terminal text selection and native scrollback depend on your terminal. |

The comparison was checked against the [official Claude Code command reference](https://code.claude.com/docs/en/commands), the app-owned command definitions, and a metadata-only `supportedCommands()` query using SDK `0.3.286`. The SDK list varies with account, platform, configuration, and installed skills; it is not the stock CLI's complete `/help` menu. A missing SDK command alone is not evidence that a capability is unavailable. The interface gaps above were also checked against the Rust and bridge implementations. For the upstream integrations, see [Remote Control](https://code.claude.com/docs/en/remote-control) and [VS Code](https://code.claude.com/docs/en/vs-code).

## Relationship To Anthropic

This project is not affiliated with, endorsed by, or supported by Anthropic. It is a third-party terminal UI that talks to the official Agent SDK through a local TypeScript bridge. It is not a fork, copy, or port of Anthropic's Claude Code source.

For official Claude documentation, use the Claude documentation:

- [Claude Docs](https://claude.ai/docs)
- [Claude Code Agent SDK overview](https://code.claude.com/docs/en/agent-sdk/overview)

## Billing Note

Usage is handled through Anthropic's Agent SDK. Billing, subscription allowances, API credits and service limits depend on Anthropic's current policy and your account. Check [Use the Claude Agent SDK with your Claude plan](https://support.claude.com/en/articles/15036540-use-the-claude-agent-sdk-with-your-claude-plan) before relying on a particular allowance. This project does not set those terms.
