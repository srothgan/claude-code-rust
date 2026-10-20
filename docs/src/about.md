# About

Claude Code Rust (`claude-rs`) is an alternative terminal frontend for Claude Code, built in Rust on Ratatui. It runs sessions through Anthropic's official Agent SDK via a local bridge, with the same account, models, tools, file edits, permissions, MCP servers, skills and session history as Claude Code. Only the interface is different.

Its main difference is how it renders: the conversation stays in your terminal's own scrollback, without flicker.

## How It Renders

Stock Claude Code has two renderers. The classic renderer keeps the conversation in terminal scrollback but redraws it as output streams, which can flicker or jump the scroll position. The [fullscreen renderer](https://code.claude.com/docs/en/fullscreen) avoids that by drawing on the alternate screen, like `vim`; terminal search, tmux copy mode and native text selection then no longer see the conversation unless you export it from transcript mode.

Claude Code Rust renders inline instead. Finished messages and tool output are inserted into the terminal's scrollback once, and only the live area at the bottom of the screen, which holds the streaming reply, the composer and the footer, is redrawn. Your terminal's scrollbar, search, copy mode and click-and-drag selection work on the whole conversation.

| | Stock classic | Stock fullscreen | Claude Code Rust |
| --- | --- | --- | --- |
| Conversation lives in | Terminal scrollback | Alternate screen | Terminal scrollback |
| Flicker while streaming | Can flicker | No | No |
| Terminal search and tmux copy mode | Yes | Only after exporting from transcript mode | Yes |
| Native text selection | Yes | Mouse is captured by default | Yes |
| Mouse controls in the app | No | Yes | No |

Fullscreen views such as `/diff`, `/agents`, `/config` and `/help` use the alternate screen while they are open and return to the inline chat when closed.

The comparison describes how each renderer works. There are no benchmarks of speed or memory against the stock interface yet.

## Project Status

The project is pre-1.0. The crate version in the root `Cargo.toml` and the [Changelog](changelog.md) are the source of truth for the current release.

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
| Model, effort, permission mode, and agent selection | Rust TUI and SDK | Available through [startup flags](usage.md) and session controls. |
| Resume, continue, compaction, and rewind | Rust TUI and SDK | Session startup and history controls; see [Usage](usage.md) and [Slash Commands](commands.md). |
| Subagents and background tasks within a session | Rust TUI and SDK | SDK task activity and results are displayed in the conversation. This is distinct from detaching the entire session. |
| Skills and SDK-advertised slash commands | SDK, when advertised | Use `/docs commands` for the current session's merged list. |
| MCP status, authorization, and dynamic servers | Rust TUI and SDK | Persisted server removal additionally needs the CLI, as listed above. |
| Inline edit diffs | Rust TUI | Tool output displays file changes. |
| Git change review | Rust TUI | `/diff` opens a fullscreen, read-only viewer; see [Reviewing Git changes](commands.md#reviewing-git-changes). It does not open beside the conversation like the stock fullscreen diff panel. |
| Shell completions and man pages | Native CLI | Generated from the CLI definitions; see [Usage](usage.md#shell-completions). |
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

Because Claude Code Rust uses the Agent SDK, usage should be treated as Agent SDK usage. Anthropic has paused the previously announced Agent SDK credit change. For now, Agent SDK usage, including `claude -p` and third-party apps like this one, still draws from normal Claude subscription limits.

Check Anthropic's current support article before relying on billing assumptions:

- [Use the Claude Agent SDK with your Claude plan](https://support.claude.com/en/articles/15036540-use-the-claude-agent-sdk-with-your-claude-plan)
