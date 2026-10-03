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

## Settings ownership and scopes

The bridge uses the installed Agent SDK's public `resolveSettings()` to inspect Claude settings. Rust displays the received catalog and snapshot; it does not merge Claude files or manufacture saved defaults. The SDK snapshot is a raw cascade, while the running Claude session separately applies trust, managed policy, capability restrictions, and explicit session choices.

| Source | Default file | Purpose |
| --- | --- | --- |
| User | `~/.claude/settings.json` | Personal Claude defaults. |
| Project | `./.claude/settings.json` | Shared project defaults. |
| Local | `./.claude/settings.local.json` | Private project defaults. |
| Managed | SDK-reported policy source | Policy contributions are shown and affected controls are read-only. |
| Workspace trust | `~/.claude.json` | A separate trust owner reads and accepts workspace trust; this is not a settings fallback. |
| App preferences | OS config directory, `claude-code-rust/settings.json` | The app settings owner persists updater state separately from Claude settings. |

Set `CLAUDE_CONFIG_DIR` before startup to isolate the Claude profile. User settings use `<directory>/settings.json`, trust uses `<directory>/.claude.json`, and file credentials use `<directory>/.credentials.json`. Sessions, plugins, and authentication inherit that directory. App preferences and diagnostics retain their normal OS locations. Existing settings sources use paths reported by the SDK; missing writable sources use the corresponding profile or working-directory path.

Settings load automatically when the app's session starts; the user does not connect a session manually. The Settings tab shows aligned setting/value columns, highlights the selected row, and displays the selected row and total, such as `1/24`. Saved values and `Default` use distinct styles. The selected row's description and value saved at the chosen scope appear below the list, including feedback when another scope overrides that value. Read-only rows show their restriction and appropriate controls. Active session information belongs in Status and the chat footer. Source metadata remains available in the bridge contract; SDK provenance is provided at top-level key granularity and is not proof that a particular leaf is an enforced policy. `resolveSettings()` does not execute `policyHelper`.

## Editing saved settings

Use Up/Down to select and scroll, Home/End to jump to the first/last setting, `s` to cycle user/project/local scope, and `r` to refresh. Left/Right cycles fixed choices and saves the change; Space is equivalent to Right. These settings have no separate dialog. Booleans display as On/Off. Space opens a text editor for free-text fields such as Language; Enter saves, Ctrl+R resets, and Escape cancels. Delete on a row resets its saved value at the selected scope. Reset displays the remaining scope's value or `Default`; it does not restore an earlier edit or save a replacement value. `Default` is a display label for an unset resolved preference, not a literal SDK value. The bridge validates values and writable scopes against its single catalog immediately before saving. A pending save keeps the last acknowledged value visible until the result arrives.

Compact windows keep the selected row, value, and essential controls visible. Below 30 columns or 12 rows, the settings surface asks the user to resize instead of showing an unusable layout; Escape cancels an editor or closes the list.

Each edit reads the current file and patches only its target key. Unrelated keys and siblings survive. Reset removes the key and empty ancestor objects; it does not write a substitute default. Missing files may be created; invalid JSON, non-object documents, unreadable files, and symlinks are blocked for editing without backup, repair, or replacement. Local saves in Git repositories add a literal exclusion to Git's private `info/exclude` when needed and reject an already tracked local settings file.

Validation applies to the setting being edited, not to unrelated fields or the whole document's schema. Unknown keys, newer settings, and unrelated invalid values are preserved. Claude decides which settings it loads: some invalid entries are ignored while other schema errors exclude the entire file. A nonempty file absent from the resolved sources is labeled `not loaded`, without claiming the cause. Targeted saves and resets remain available for readable JSON objects. If a set is saved but the resulting file still does not load, the editor reports the successful save separately from the loading problem; correcting the offending value can restore loading. Reset remains a successful deletion even when only unrelated ignored or invalid fields remain. Local temporary files containing settings data are also protected by the private Git exclusion before they are written.

A revision for the displayed target detects conflicting edits. Unrelated edits are incorporated into the fresh document; a conflict refreshes the snapshot while preserving the editor draft. Writes use a cooperative lock, byte rechecks, and atomic replacement. An external editor that does not honor the lock can still race between the final recheck and replacement; this is not filesystem compare-and-swap.

The window states once that saved changes apply to new sessions and marks immediate exceptions. The app does not turn a settings save into a session flag override. Use `/model`, `/mode`, `/effort`, `/thinking`, `/agent`, `/fast`, or `/ultracode` for their existing session actions. Host-consumed `respectGitignore` and `prefersReducedMotion` refresh immediately after acknowledgement. New sessions and resumed sessions retain native settings inheritance. The footer's effort badge reflects the SDK's applied level at initialization and after session changes, including model restrictions and organization caps. Per-turn init observations supersede earlier reads. If the SDK reports no discrete level, the footer shows the model without an effort suffix.

## Current catalog

The catalog exposes 24 rows, sorted alphabetically by their displayed names. It enables validated boolean and string editors for auto-compaction, flagged-message model switching, thinking, fast mode, prompt suggestions, checkpoints, workflows, keyword triggers, workflow size, default permission mode, worktree base reference, file-picker Gitignore behavior, output style, language, default model, and reduced motion. The catalog describes per-row writable scopes; not every setting can be saved at every scope. Default model cycles only through the session's SDK-advertised choices; it does not accept typed model IDs. Existing externally configured values remain preserved on disk until explicitly changed or reset. Output style still accepts custom style names. Default agent uses the SDK-advertised agent inventory.

Default effort is one inline choice control for the currently saved Default model, using the same Left/Right/Space interaction as Default model. Its choices come from that model's SDK capabilities, excluding session-only `max`. Saving writes `modelSettings.<canonical model>.effortLevel` at the chosen scope and preserves other models' preferences and sibling fields. Changing Default model retargets the effort control; mutation context checks prevent an outdated edit from silently targeting a different model. An unsupported or unresolved model leaves the effort editor read-only. Reset removes only that model's effort at the selected scope. SDK model metadata determines the target; aliases and dated/context/provider spellings are normalized to Claude's canonical key, while unknown model identities remain read-only. The saved snapshot is the SDK's file cascade; environment settings, top-level effort fallbacks, trust, and policy caps can still change the running effort, which is shown in the footer.

`Default` means no configured value was returned for this preference. It does not promise On or Off. The installed SDK's file resolver does not fill in built-in defaults; its applied-settings response exposes selected session values rather than effective defaults for the whole catalog. Keep this label until the fallback for an individual setting can be determined reliably, without treating a current-session override as its default.

Continuation at usage limits, auto mode during planning, notification transport, question timeout, dialog expiry, and cross-session inbound policy are visible with explicit read-only reasons until their host workflows are completed. Themes and Vim/editor modes are omitted. Advanced structured editors, notification delivery, additional presentation controls, and updater preferences are subsequent implementation groups in `config.md`.

When Reduce motion is On, active thinking, tool execution, compaction, cancellation, and pending commands use a static diamond (`◆`) instead of an animated spinner. Animation-only redraws stop; incoming state changes still update normally. This setting is ready and applies immediately after saving, with reset following the remaining scopes or Default.

## Offline inspection

These read-only commands inspect redacted physical files without a TUI session or mutation:

```bash
claude-rs config
claude-rs config show --which project-settings
claude-rs config export --output claude-rs-config.json
```

Offline output includes user, shared project, local settings, and preferences. It reports malformed files rather than rewriting them. It does not resolve the SDK cascade or virtual managed policy sources.

## MCP

The MCP tab shows live session-backed MCP state. Use `/mcp` to inspect servers, refresh status, complete authorization, reconnect servers, and handle SDK-provided MCP prompts when available.

If no session is active, the tab asks you to open or resume a session first. If the active session reports no MCP servers, the tab shows an empty state rather than editing raw config files.

## Plugins

The Plugins tab is available through `/plugins`. It shows installed plugins, marketplace plugins, and configured marketplaces.

Supported actions include enabling, disabling, updating, uninstalling, and installing plugins into user, project, or local scopes when those actions are available for the selected plugin.

After plugin changes, the app requests a session runtime plugin reload when an active session is available.
