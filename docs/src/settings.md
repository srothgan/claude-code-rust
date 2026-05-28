# Settings

Claude Code Rust has a fullscreen settings surface with multiple tabs. These slash commands open that surface directly:

| Command | Tab | Purpose |
| --- | --- | --- |
| `/config` | Settings | Edit supported Claude-compatible settings. |
| `/mcp` | MCP | Inspect live MCP server status and complete MCP authorization flows. |
| `/plugins` | Plugins | Manage installed plugins, marketplace plugins, and marketplaces. |
| `/status` | Status | Inspect session, account, authentication, and runtime status. |
| `/usage` | Usage | Inspect quota and usage information reported by the active session. |
| `/help` | Help | Open fullscreen in-app help. |

The settings surface is session-aware. Some tabs need an active bridge session before they can show live SDK-backed state.

## Usage

Open the settings surface with any command in the table above. Each command opens the same fullscreen surface but targets a different starting tab.

The tab order is:

```text
Settings -> Plugins -> Status -> Usage -> MCP -> Help
```

Use `Tab` to move to the next tab and `Shift+Tab` to move to the previous tab. The active tab can also have its own navigation and action keys. For example, the Settings tab edits persisted settings, the Plugins tab navigates plugin lists and overlays, the MCP tab opens server actions and authorization flows, and the Usage and Status tabs refresh live session-backed data.

The surface is not only for editing JSON settings. It is the shared fullscreen control area for settings, plugins, MCP, account/session status, usage, and in-app help.

## Help

Use `/help` when you want fullscreen help inside the same tabbed surface. The Help tab has three sections:

| Section | Shows |
| --- | --- |
| Shortcuts | Keyboard shortcuts for the current app state and focused UI context. |
| Commands | App-owned slash commands plus slash commands advertised by the active SDK session. |
| Subagents | Subagents advertised by the active SDK session, including model labels when provided. |

Use `Left` and `Right` inside the Help tab to switch sections. Use `Up` and `Down` to move through rows in the active section.

The Help tab is live UI, not a static manual page. Its Shortcuts section changes with focus and state, and its Commands and Subagents sections depend on what the active SDK session advertises.

## Settings Files

Settings are loaded from Claude-compatible JSON files. The app can edit supported settings and display unsupported settings that exist for compatibility or future work.

| File | Scope |
| --- | --- |
| `~/.claude/settings.json` | User-level Claude settings. |
| `./.claude/settings.local.json` | Project-local settings for the current working directory. |
| `~/.claude.json` | User preferences. |

Malformed JSON files are backed up with a timestamped `.bak` extension and replaced in memory with an empty object so the app can keep running.

## Supported Settings

| Setting | File | JSON path | Notes |
| --- | --- | --- | --- |
| Always Thinking | `~/.claude/settings.json` | `alwaysThinkingEnabled` | Enables adaptive thinking for new sessions. |
| Model | `~/.claude/settings.json` | `model` | Uses the model catalog advertised by the active session. |
| Default permission mode | `~/.claude/settings.json` | `permissions.defaultMode` | Uses permission modes advertised by the active session. |
| Fast mode | `~/.claude/settings.json` | `fastMode` | Persists the fast-mode preference for future sessions. |
| Language | `~/.claude/settings.json` | `language` | Free-text instruction, 2 to 30 characters. Does not localize the UI. |
| Notifications | `~/.claude.json` | `preferredNotifChannel` | Controls how attention-needed notifications are delivered. |
| Output style | `./.claude/settings.local.json` | `outputStyle` | Changes how Claude communicates in sessions. |
| Reduce motion | `./.claude/settings.local.json` | `prefersReducedMotion` | Slows spinners and disables smooth chat scrolling. |
| Respect .gitignore | `~/.claude.json` | `respectGitignore` | Controls whether file mentions hide ignored entries. |
| Thinking effort | `~/.claude/settings.json` | `effortLevel` | Applies when Always Thinking is on and the selected model supports effort. |

## Currently Unsupported Settings

These settings are represented in the settings UI but are not currently supported by the app runtime:

| Setting | File | JSON path |
| --- | --- | --- |
| Editor mode | `~/.claude.json` | `editorMode` |
| Show Tips | `./.claude/settings.local.json` | `spinnerTipsEnabled` |
| Terminal progress bar | `~/.claude.json` | `terminalProgressBarEnabled` |
| Theme | `~/.claude.json` | `theme` |

## MCP

The MCP tab shows live session-backed MCP state. Use `/mcp` to inspect servers, refresh status, complete authorization, reconnect servers, and handle SDK-provided MCP prompts when available.

If no session is active, the tab asks you to open or resume a session first. If the active session reports no MCP servers, the tab shows an empty state rather than editing raw config files.

## Plugins

The Plugins tab is available through `/plugins`. It shows installed plugins, marketplace plugins, and configured marketplaces.

Supported actions include enabling, disabling, updating, uninstalling, and installing plugins into user, project, or local scopes when those actions are available for the selected plugin.

After plugin changes, the app requests a session runtime plugin reload when an active session is available.

## Project-Local Overrides

Two slash commands write project-local environment overrides into `./.claude/settings.local.json`:

```text
/1m-context <enable|disable|status>
/opus-version <4.5|4.6|4.7|default|status>
```

These settings apply to future sessions. Run `/new-session` after changing them.
