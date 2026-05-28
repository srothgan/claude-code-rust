# Slash Commands

Claude Code Rust has app-owned slash commands and can also show slash commands advertised by the active Agent SDK session. App-owned commands are available from the Rust TUI itself; SDK-advertised commands depend on the current session and what the bridge reports.

Use `/docs commands` in the app to render the live merged command list into chat. That is the source to use when you want to know exactly which app-owned and SDK-advertised commands are available in the current session.

## App-Owned Commands

| Command | Usage | Purpose |
| --- | --- | --- |
| `/1m-context` | `/1m-context <enable|disable|status>` | Enable, disable, or inspect project-local 1M context settings for future sessions. |
| `/cancel` | `/cancel` | Cancel the active assistant turn. |
| `/compact` | `/compact` | Ask the active session to compact conversation context. |
| `/config` | `/config` | Open fullscreen settings. |
| `/docs` | `/docs <mode|models|shortcuts|commands|agents>` | Render command, shortcut, model, mode, or subagent help into chat. |
| `/help` | `/help` | Open the fullscreen Help tab. |
| `/mcp` | `/mcp` | Open MCP status and authorization. |
| `/plugins` | `/plugins` | Open plugin management. |
| `/opus-version` | `/opus-version <4.5|4.6|4.7|default|status>` | Set, clear, or inspect the project-local Opus alias pin for future sessions. |
| `/status` | `/status` | Open session and account status. |
| `/usage` | `/usage` | Open quota and usage information. |
| `/login` | `/login` | Run Claude CLI authentication and reconnect the session. |
| `/logout` | `/logout` | Run Claude CLI logout and clear the active authenticated session. |
| `/mode` | `/mode <id>` | Switch to a mode advertised by the active session. |
| `/model` | `/model <id>` | Switch to a model advertised by the active session. |
| `/new-session` | `/new-session` | Start a fresh bridge session in the current folder. |
| `/resume` | `/resume <session_id>` | Resume a recent or manually supplied session id. |

## SDK-Advertised Commands

The active SDK session can advertise additional slash commands. These are not documented as a fixed table here because they can change with SDK behavior, session capabilities, account state, and future upstream changes.

Use:

```text
/docs commands
```

to inspect the current session's full command list. The output includes app-owned commands and SDK-advertised commands, with descriptions when the SDK provides them.

## Project-Local Commands

`/1m-context` and `/opus-version` persist folder-local settings under `./.claude/settings.local.json`. The current session is not restarted automatically, so run `/new-session` after changing either setting.

`/1m-context disable` writes the environment setting used to disable the 1M context window for future sessions in that folder. `enable` clears that override.

`/opus-version <version>` pins the folder-local Opus alias. `default` clears the pin.
