# Settings

Claude Code Rust has a fullscreen settings surface with multiple tabs. These slash commands open that surface directly:

| Command | Tab | Purpose |
| --- | --- | --- |
| `/config` | Settings → General | Edit saved defaults and everyday preferences. |
| `/memory` | Settings → Memory | Configure context, checkpoints, automatic memory and plans. |
| `/permissions` | Settings → Permissions | Configure saved permission defaults, rules and working directories. |
| `/sandbox` | Settings → Sandbox | Configure sandbox execution, filesystem, network and credentials. |
| `/hooks` | Settings → Hooks | Inspect and edit saved hook handlers without executing them. |
| `/mcp` | MCP | Inspect live MCP server status and complete MCP authorization flows. |
| `/plugins` | Plugins | Manage installed plugins, marketplace plugins, and marketplaces. |
| `/status` | Status | Inspect session, account, authentication, and runtime status. |
| `/usage` | Usage | Inspect quota and usage information reported by the active session. |
| `/help` | Help | Open fullscreen in-app help. |

The settings surface is session-aware. Settings load when the app's session starts, and some tabs need an active session before they can show live data.

## Opening and navigating

Every command in the table above opens the same fullscreen surface on a different starting tab. The tab order is:

```text
Settings -> Plugins -> Status -> Usage -> MCP -> Help
```

`Tab` moves to the next tab and `Shift+Tab` to the previous one. `Esc` closes the surface. `Enter` closes it as well wherever the selected item has no action of its own. The hint line at the bottom of the surface shows what `Enter` and `Esc` do at the current position. Each tab also has its own keys: the Plugins tab navigates plugin lists, the MCP tab opens server actions and authorization flows, the Status tab renames the session with `r` or generates a title with `g`, and the Usage tab refreshes with `r`.

### Settings panes

The Settings tab has a second row with six panes: General, Memory, Permissions, Sandbox, Hooks, and Workflows & Worktrees. Each pane is one alphabetical list. Focus moves between three areas stacked from top to bottom: the pane row, the search field, and the list.

| Focus | Key | Action |
| --- | --- | --- |
| List | `Up` on the first item | Move to the search field. |
| Search field | `Up` | Move to the pane row. |
| Search field | `Down`, `Enter` | Move to the list. |
| Pane row | `Left`, `Right` | Switch pane. |
| Pane row | `Down` | Move to the search field. |
| Pane row | `Enter` | Move to the list. |

Each pane remembers its own selection. Switching to another top-level tab and back keeps the pane, selection and search.

Below 30 columns or 12 rows the surface asks you to resize the window instead of drawing an unusable layout.

### Search

Press `/` to search the current pane. A setting matches when its label, description or setting ID contains every word you typed, ignoring case. Search covers only the current pane and never looks inside saved values such as hook commands, credential entries or environment values.

| Key | Action |
| --- | --- |
| `Enter` in the search field | Move to the results. |
| `Enter` on a result | Clear the query and keep that setting selected in the full list. |
| `Esc` | Clear the query and return to the previous position. Without a query, `Esc` closes the surface. |

Switching panes clears the search.

## Help

Use `/help` when you want fullscreen help inside the same tabbed surface. The Help tab has three sections:

| Section | Shows |
| --- | --- |
| Shortcuts | Keyboard shortcuts for the current app state and focused UI context. |
| Commands | App-owned slash commands plus slash commands advertised by the active SDK session. |
| Subagents | Subagents advertised by the active SDK session, including model labels when provided. |

Use `Left` and `Right` inside the Help tab to switch sections. Use `Up` and `Down` to move through rows in the active section.

The Help tab is live UI, not a static manual page. Its Shortcuts section changes with focus and state, and its Commands and Subagents sections depend on what the active SDK session advertises.

## Where settings are saved

Each save goes to one scope. Press `s` in the list to cycle the save scope through User, Project and Local.

| Scope | File | Use |
| --- | --- | --- |
| User | `~/.claude/settings.json` | Personal defaults for every project. |
| Project | `./.claude/settings.json` | Defaults shared with the repository. |
| Local | `./.claude/settings.local.json` | Private defaults for this project. |
| Managed | Organization policy | Shown for information. Settings restricted by policy are read-only. |
| App preferences | `claude-code-rust/settings.json` in the OS config directory | This app's own preferences: tab-title activity, the `/copy` picker, notification categories and automatic updates. Always saved for the user. |

Not every setting can be saved at every scope. A setting that the selected scope cannot hold is read-only until you switch scope; [Settings by pane](#settings-by-pane) lists the restrictions.

Workspace trust is recorded separately in `~/.claude.json`. It is not a settings scope and is never used as a fallback for a setting.

Set `CLAUDE_CONFIG_DIR` before startup to use an isolated Claude profile. User settings then live in `<directory>/settings.json`, workspace trust in `<directory>/.claude.json`, and file credentials in `<directory>/.credentials.json`. Sessions, plugins and authentication use the same directory. Project and Local settings stay in the project, and app preferences and diagnostics keep their normal OS locations.

The Settings tab shows saved values. A running session can differ, because workspace trust, managed policy, model capabilities, startup arguments and session commands also apply. The Status tab and the chat footer show what the session is running with.

## Reading the list

Each row shows a setting and its value, and a counter such as `1/27` shows the position in the pane. Below the list, `Description:` explains the selected setting and a context line states where its value comes from at the selected scope:

| Context line | Meaning |
| --- | --- |
| `Saved in User` | The value is saved at the selected scope. |
| `User value: … · Overridden by Project` | A value is saved at the selected scope, but another scope takes precedence. |
| `From Project · Not set in User` | The value comes from another scope. |
| `Using Default` | No scope sets a value. |
| `Not set in User` | A list or structured setting has no entries at the selected scope. |
| `Saved in User · Not loaded` | The file contains the value, but Claude did not load it. |
| `· Also supplied by Project` | Another scope contributes as well, as with merged lists. |
| `· Applies immediately`, `· Applies to new sessions` | When a saved change takes effect. |

`Default` means that no scope sets a value. It does not say whether the built-in behavior is On or Off; where the built-in behavior is known, the description states it, for example `Default: On`.

Read-only restrictions and settings-file errors appear as warnings below the description.

Claude decides which settings files it loads: some invalid entries are ignored individually, while other schema errors exclude the whole file. `Not loaded` reports that outcome without naming the cause. Saving and resetting still work on any readable file, and correcting the offending value restores loading.

## Editing a value

| Key | Action |
| --- | --- |
| `Up`, `Down` | Select the previous or next setting. |
| `Home`, `End` | Select the first or last setting. |
| `Page Up`, `Page Down` | Move one page. |
| `Left`, `Right` | Step through the choices of an On/Off or fixed-choice setting. The change is saved immediately. |
| `Space` | Same as `Right` for fixed choices. Opens the editor for text, number, list and structured settings. |
| `Enter` | On a list or structured setting, opens the editor, or opens it for inspection when the scope is read-only. On any other setting, closes the surface. |
| `Delete` | Reset the setting at the selected scope. |
| `s` | Cycle the save scope. |
| `r` | Reload settings from disk. |

Changes made with `Left`, `Right` or `Space` are saved as you make them, so closing with `Enter` or `Esc` discards nothing.

The text editor used for settings such as Language takes `Enter` to save, `Ctrl+R` to reset and `Esc` to cancel.

Reset removes the value saved at the selected scope. The row then shows the value from another scope, or `Default`. Reset does not write a replacement value and does not restore an earlier edit.

While a save is in progress the row keeps showing the last confirmed value. A successful save shows `Saved.`

Saving a default never changes the running session. Use `/model`, `/mode`, `/effort`, `/thinking`, `/agent`, `/fast` or `/ultracode` for the current session; see [Session Commands](commands.md#session-commands).

## Lists and structured settings

Permission rules, directory and domain lists, hook definitions and the structured sandbox settings open an editor instead of changing in place. The row in the main list shows how many entries are saved at the selected scope.

The editor loads only the entries saved at the selected scope, not a merged view of all scopes, and keeps that scope until it closes.

| Key | Action |
| --- | --- |
| `Up`, `Down`, `Home`, `End` | Move through entries or fields. |
| `a` | Add an entry. |
| `Enter`, `Space` | Edit the selected entry, open a nested group, or toggle an On/Off field. |
| `Delete` | Remove the selected entry or optional field. |
| `Ctrl+S` | Save the draft. |
| `Ctrl+R` | Reset the whole setting at this scope. |
| `Esc` | Go up one level. At the top level, discard the draft and close the editor. |
| `Ctrl+J` | Switch to a JSON view of the same draft. |
| `Ctrl+F` | Return from JSON to the form. The JSON must be valid. |

While typing in a field:

| Key | Action |
| --- | --- |
| `Enter` | Accept the field into the draft. |
| `Esc` | Cancel the field. |
| `Ctrl+U` | Clear the field. |
| `Ctrl+J` | Insert a line break. |
| `Up`, `Down`, `Left`, `Right` | Choose a value when the field offers fixed choices. |

Rules that apply to every editor:

- Edits stay in a draft until `Ctrl+S`. Accepting a field does not write the file.
- Many lists merge across scopes. Removing the last entry saves an explicit empty list, and entries from other scopes still apply. Reset removes the setting from this scope instead.
- Invalid JSON or a value that Claude rejects keeps the draft open for correction. Nothing is written.
- If the file changed on disk while the editor was open, the editor loads the latest version and keeps your draft. Save again to confirm.
- If the session moves to another project while an editor is open, saving is blocked. Close the editor and open it again.
- A scope that cannot be edited can still be opened for inspection.
- Opening, browsing and validating never run hooks or credential helpers, and never read secret files or environment values. Header values and command arguments are redacted in summaries.
- Fields that the form does not know are preserved when you save.

### Editing permission rules

In the Permissions pane, open Allow rules, Ask rules, Deny rules or Additional directories. Add one tool rule per entry, for example `Bash(npm test *)` or `Read(./src/**)`. A deny rule wins over an allow rule from any scope.

### Editing hooks

Hooks → Definitions shows a flat list with each hook's event, matcher and action.

- `a` starts guided creation: choose an event, enter an optional matcher, choose an action, fill its required fields, then review. `Enter` on the review step adds the hook to the draft and opens its optional fields. `Esc` goes back one step and cancels at the first step.
- `Enter` edits the selected hook.
- `m` edits the matcher of the hook's group. Every hook in that group shares the matcher.
- `Delete` removes only the selected hook.

Hook actions are command, prompt, agent, HTTP and MCP tool. Event names come from the installed SDK. The JSON view shows the same definitions in the settings-file format:

```json
{
  "PreToolUse": [
    {
      "matcher": "Bash",
      "hooks": [
        { "type": "command", "command": "echo Check shell operation", "timeout": 10 }
      ]
    }
  ]
}
```

Saving replaces the `hooks` object at the selected scope. Hooks from other scopes keep merging in. Disable all is a separate On/Off setting.

### Editing sandbox settings

Path, domain and command lists use the list editor. Credential protection, TLS termination, Ignored violations and Ripgrep command open forms:

- Required fields are marked with `*`.
- Fixed choices open a picker.
- Credential entries take file paths and variable names, never secret values.
- TLS termination takes both a CA certificate path and a CA key path, or neither.
- Proxy ports accept whole numbers from 1 to 65535.

## Settings by pane

Every row has a description in the app. This section covers behavior that needs more explanation. Unless noted, a setting can be saved at User, Project or Local scope and applies to new sessions.

### General

| Setting | Key | Choices | Notes |
| --- | --- | --- | --- |
| Default model | `model` | Models offered by the session | A model ID cannot be typed. A value written by another tool is kept until you change or reset it. |
| Default effort | `modelSettings.<model>.effortLevel` | Effort levels of the default model, without `max` | See below. |
| Default agent | `agent` | Agents offered by the session | |
| Thinking | `alwaysThinkingEnabled` | On / Off | A preference. The model may still require or restrict thinking. |
| Fast mode | `fastMode` | On / Off | Account and model eligibility still apply. |
| Output style | `outputStyle` | Text | A built-in or custom style name. |
| Language | `language` | Text | Preferred response language. Does not translate the app. |
| Question timeout | `askUserQuestionTimeout` | Never / 60 seconds / 5 minutes / 10 minutes | User only. See below. |
| Dialog expiry | `dialogExpiry` | 60 seconds / 5 minutes / 10 minutes / Never | User only. See below. |
| Respect .gitignore | `respectGitignore` | On / Off | Hides ignored files from the file picker. Applies immediately. |
| Reduce motion | `prefersReducedMotion` | On / Off | Shows a static `◆` in place of animated spinners. Applies immediately. |

General also holds the settings described under [Presentation and scrolling](#presentation-and-scrolling), [Notifications](#notifications) and [Automatic updates](#automatic-updates).

**Default effort** belongs to the saved Default model and is saved per model, so changing Default model switches the effort row to the new model. `max` is available only through `/effort` in a session. The row is read-only when the default model is unknown or offers no effort levels. Environment settings, workspace trust and organization caps can still change the effort a session runs with; the chat footer shows the applied level.

**Question timeout** submits a waiting question after the chosen idle time. Only answers you explicitly selected are sent, together with any notes; untouched questions are skipped. Keyboard, paste and mouse activity restart the interval, and the timer pauses while another screen has focus. Never waits for confirmation.

**Dialog expiry** cancels forwarded remote dialogs and held cross-session messages after the chosen time. Local permission prompts get no deadline. Reset restores 5 minutes. The `CLAUDE_CODE_USER_DIALOG_TIMEOUT_MS` environment variable takes precedence.

### Memory

| Setting | Key | Notes |
| --- | --- | --- |
| Auto compact | `autoCompactEnabled` | Compacts context automatically. |
| File checkpoints | `fileCheckpointingEnabled` | Records file snapshots for code rewind. |
| Automatic memory | `autoMemoryEnabled` | Reads and writes automatic memory for the project. |
| Directory | `autoMemoryDirectory` | User and Local only; shared Project settings cannot redirect memory. Supports `~/`. |
| Excluded instructions | `claudeMdExcludes` | One glob or absolute path per entry. Managed instructions cannot be excluded. |
| Plans directory | `plansDirectory` | Relative paths are relative to the project root. |

### Permissions

| Setting | Key | Notes |
| --- | --- | --- |
| Default permission mode | `permissions.defaultMode` | User only. `default`, `plan`, `acceptEdits`, `auto`, `dontAsk` or `bypassPermissions`, subject to workspace trust and account restrictions. |
| Auto mode during planning | `useAutoModeDuringPlan` | User and Local only. |
| Disable bypass | `permissions.disableBypassPermissionsMode` | Removes the bypass permission mode. Reset removes this scope's restriction. |
| Block outside reads | `permissions.blockReadsOutsideWorkingDirectories` | Blocks Read, Grep, Glob and LSP outside working directories. On in any scope wins. |
| Allow rules, Ask rules, Deny rules | `permissions.allow`, `permissions.ask`, `permissions.deny` | See [Editing permission rules](#editing-permission-rules). |
| Additional directories | `permissions.additionalDirectories` | One directory per entry. |

### Sandbox

The Sandbox pane covers sandbox execution, filesystem paths, network domains, proxy ports and credential protection under the `sandbox.` key. Descriptions name the settings that apply to one platform only. Workspace trust, platform support and organization restrictions decide which saved values take effect, and can make rules and allowlists read-only.

These settings are User only: Disable filesystem isolation, Strict domain allowlist, TLS termination, Credential protection, Allow Apple Events and Ripgrep command.

### Hooks

| Setting | Key | Notes |
| --- | --- | --- |
| Definitions | `hooks` | See [Editing hooks](#editing-hooks). |
| Disable all | `disableAllHooks` | Disables settings and plugin hooks, including status-line execution. |
| Allowed HTTP URLs | `allowedHttpHookUrls` | One URL pattern per entry; `*` is a wildcard. An empty list blocks all HTTP hooks. |
| Allowed environment variables | `httpHookAllowedEnvVars` | Variable names that HTTP hook headers may interpolate. |

### Workflows & Worktrees

| Setting | Key | Notes |
| --- | --- | --- |
| Dynamic workflows | `enableWorkflows` | Availability depends on the account. |
| Workflow keyword trigger | `workflowKeywordTriggerEnabled` | Allows keyword-triggered workflows. |
| Workflow size | `workflowSizeGuideline` | An advisory agent count: Small aims below 5, Medium below 10, Large below 50. Unrestricted gives no guideline. |
| Worktree base ref | `worktree.baseRef` | `fresh` or `head`. |
| Symlink directories | `worktree.symlinkDirectories` | Repository directories shared with new worktrees, for example `node_modules`. |
| Sparse paths | `worktree.sparsePaths` | Directories included through Git sparse-checkout. |
| Background isolation | `worktree.bgIsolation` | `worktree` makes background jobs enter a worktree before editing; `none` allows edits in the main checkout. |

### Unavailable settings

Continue at usage limit and Messages from other sessions are listed in General but are read-only in this version. Themes and Vim editor mode are not offered.

## Presentation and scrolling

These General settings apply immediately, including to the conversation already on screen.

| Setting | Key | Choices | Behavior |
| --- | --- | --- | --- |
| Auto-scroll | `autoScrollEnabled` | On / Off | Follow new output when On; hold the reading position when Off. |
| Show tips | `spinnerTipsEnabled` | On / Off | Show tips during active turns. Off hides only the tip text. Default: On. |
| Show message timestamps | `showMessageTimestamps` | On / Off | Show the message time beside its role label. Default: Off. |
| Show turn duration | `showTurnDuration` | On / Off | Show one total elapsed time per completed response. Default: Off. |
| Time format | `timeFormat` | Auto / 12-hour / 24-hour / 24-hour UTC | Format clock times. Elapsed durations are unaffected. |
| Show activity in tab title | `presentation.showStatusInTerminalTab` | On / Off | Show a busy/idle icon beside the folder name in the terminal tab title. |
| Skip the /copy picker | `presentation.copyFullResponse` | On / Off | Copy the complete response directly. See [Copying responses](commands.md#copying-responses). |

The first five are Claude settings and can be saved at User, Project or Local scope. The last two are app preferences and are always saved for the user.

**Timestamps and durations.** When Claude supplies no timestamp, a live message shows the locally observed time marked `(observed)`; resumed messages without a timestamp stay unstamped. With both settings On, the duration row also shows the completion time (`Done`). When the SDK reports no turn timing, a live response shows an `Observed` duration. Tool progress time is a lower bound, shown with `≥`, and task time is labelled separately; neither is added to the turn total. Resumed responses show a duration only when the session history contains one.

**Time format.** Auto follows the system locale. 12-hour shows AM/PM and 24-hour UTC adds a `Z` suffix. A custom strftime pattern written into a settings file is used and preserved; the picker offers only the four presets. A `timeZone` entry in a settings file, such as `Europe/Berlin`, sets the zone for local presets and custom patterns. An unknown zone name falls back to the system zone.

**Scrolling.** `Page Up` pauses following and reads earlier output; `Page Down` reads later output. `Ctrl+End` jumps to the latest output and resumes following when Auto-scroll is On. With Auto-scroll Off it jumps once and then holds that position. The reading position survives new output and window resizing. The mouse wheel belongs to the terminal's own scrollback unless the terminal forwards mouse events to the app.

## Notifications

Notifications alert only while the terminal reports that it is unfocused. A terminal without focus reporting stays silent. These General settings apply immediately:

| Setting | Key | Choices | Behavior |
| --- | --- | --- | --- |
| Notification method | `preferredNotifChannel` | Auto, iTerm2, Terminal bell, iTerm2 with bell, Kitty, Ghostty, Disabled. Default: Auto | How alerts are delivered. Disabled silences every category. |
| Notify when input is needed | `notifications.actionsRequired` | On / Off. Default: On | Alert for a waiting permission, question or dialog. A request that is already waiting does not alert twice. |
| Notify when Claude requests it | `notifications.modelDirected` | On / Off. Default: On | Allow alerts that Claude sends through the `PushNotification` tool, when the account offers that tool. |
| Notify when a turn finishes | `notifications.turnComplete` | On / Off. Default: On | Alert when an active turn completes. Cancelled turns, errors and resumed history do not alert. |

Notification method is a Claude setting saved for the user. The three category toggles are app preferences. They do not change the mobile-push settings `inputNeededNotifEnabled` and `agentPushNotifEnabled`.

Auto uses the terminal's own notification protocol on iTerm2, Ghostty and Kitty, and otherwise sends a desktop notification with a terminal bell. A terminal-specific method uses its protocol on that terminal and falls back to a desktop notification elsewhere. iTerm2 with bell adds the bell, and Terminal bell sends only the bell. Desktop notifications can be unavailable over SSH. Protocol references: [iTerm2](https://iterm2.com/documentation-escape-codes.html), [Ghostty](https://ghostty.org/docs/vt/osc/9), and [Kitty](https://sw.kovidgoyal.net/kitty/desktop-notifications/).

Two limits apply to every method:

- Ordinary notices from Claude, including high-priority ones, appear in the transcript and do not raise an alert.
- An alert is sent when its event happens or not at all. Resuming a session, regaining focus or changing a setting never delivers earlier alerts.

If Claude already delivered a requested alert on this machine, the app does not send a second one. A mobile push alone does not suppress the local alert.

To investigate a missing alert, see [Notification diagnostics](diagnostics.md#notification-diagnostics).

## Automatic updates

Automatic updates is an app preference saved as `updates.autoInstall`. It defaults to Off.

| Value | Behavior |
| --- | --- |
| Off | A newer version opens the update window at startup, with manual installation choices. |
| On | The window is skipped. A newer version is installed in the background after a normal exit, using the installation method of the running app (script or npm). |

Update checks run in both modes and are cached for 24 hours. `--no-update-check` and `CLAUDE_RUST_NO_UPDATE_CHECK` disable checking and therefore automatic installation.

Automatic installation is skipped for source builds and unrecognized installations, after a forced shutdown, and after an application error. The setting is read again at exit, so switching it Off during a session cancels the pending installation.

The installation runs detached from the terminal and asks no questions. On exit the app prints one line naming the version and returns to the shell; closing the terminal does not stop the installation. Only one background installation runs at a time, and one that takes longer than 10 minutes is stopped.

Installer output is written to `claude-rs-update.log` in the parent of the [runtime log directory](diagnostics.md#logging), replacing the log of the previous installation. A failed installation is shown as a warning at the next start and retried after the next normal exit.

The setting updates only `claude-rs`. It does not update a separately installed Claude CLI.

## How files are changed

- A save changes only the key of the edited setting. Other keys, unknown keys and values the app does not understand are left as they are.
- Reset removes the key and any parent object that becomes empty. It never writes a substitute default.
- A missing settings file is created on the first save.
- A file that contains invalid JSON, is not a JSON object, cannot be read, or is a symlink is blocked from editing. The app does not repair, replace or back up such a file.
- Saving at Local scope inside a Git repository adds `.claude/settings.local.json` to the repository's private `.git/info/exclude` when needed. A Local settings file that Git already tracks is rejected.
- Only the edited setting is validated. An unrelated invalid value elsewhere in the file does not block the save, but it can still prevent Claude from loading the file; see `Not loaded` under [Reading the list](#reading-the-list).
- A change made to the same setting by another program is detected before writing. The app reloads and keeps your draft. An editor that writes at the same instant without honoring the app's lock file can still overwrite a save.

## Offline inspection

These read-only commands inspect redacted settings files without starting the TUI:

```bash
claude-rs config
claude-rs config show --which project-settings
claude-rs config export --output claude-rs-config.json
```

The output covers User, Project and Local settings and the preferences file. Malformed files are reported, not rewritten. The commands read the files as they are on disk: they do not combine scopes and do not include managed policy. See [Config Inspection](diagnostics.md#config-inspection).

## MCP

The MCP tab shows live session-backed MCP state. Use `/mcp` to inspect servers, refresh status, complete authorization, reconnect servers, and handle SDK-provided MCP prompts when available.

If no session is active, the tab asks you to open or resume a session first. If the active session reports no MCP servers, the tab shows an empty state rather than editing raw config files.

## Plugins

The Plugins tab is available through `/plugins`. It shows installed plugins, marketplace plugins, and configured marketplaces.

Supported actions include enabling, disabling, updating, uninstalling, and installing plugins into user, project, or local scopes when those actions are available for the selected plugin.

After plugin changes, the app requests a session runtime plugin reload when an active session is available.
