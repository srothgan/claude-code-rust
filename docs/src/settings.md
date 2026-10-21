# Settings reference

Use this page to look up saved scopes, setting keys, persistence and runtime behavior. For editing steps, start with [Configure claude-rs](configure.md). A saved value and the value running in the current session can differ.

## Where settings are saved

Each save goes to one scope. Press `s` in the list to cycle the save scope through User, Project and Local.

| Scope | File | Use |
| --- | --- | --- |
| User | `~/.claude/settings.json` | Personal defaults for every project. |
| Project | `./.claude/settings.json` | Defaults shared with the repository. |
| Local | `./.claude/settings.local.json` | Private defaults for this project. |
| Managed | Organization policy | Shown for information. Settings restricted by policy are read-only. |
| App preferences | `claude-code-rust/settings.json` in the OS config directory, or `<CLAUDE_RS_CONFIG_DIR>/settings.json` when overridden | This app's own preferences: footer layout, tab-title activity, the `/copy` picker, notification categories and automatic updates. Always saved for the user. |

Not every setting can be saved at every scope. A setting that the selected scope cannot hold is read-only until you switch scope; [Settings by pane](#settings-by-pane) lists the restrictions.

Workspace trust is recorded separately in `~/.claude.json`. It is not a settings scope and is never used as a fallback for a setting.

Set `CLAUDE_CONFIG_DIR` before startup to use an isolated Claude profile. User settings then live in `<directory>/settings.json`, workspace trust in `<directory>/.claude.json`, and file credentials in `<directory>/.credentials.json`. Sessions, plugins and authentication use the same directory. Project and Local settings stay in the project, and app preferences and diagnostics keep their normal OS locations.

Set `CLAUDE_RS_CONFIG_DIR` to select a separate directory for this app's preferences. They are read from and saved to `<directory>/settings.json`, including the footer layout, presentation preferences, notification categories and update preferences. An empty or unset value uses the normal OS config location. This override is independent of `CLAUDE_CONFIG_DIR`; diagnostics retain their normal location.

The Settings tab shows saved values. A running session can differ, because workspace trust, managed policy, model capabilities, startup arguments and session commands also apply. The Status tab and the chat footer show what the session is running with.

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
| Reduce motion | `prefersReducedMotion` | On / Off | Shows a static `◆` in place of animated spinners and keeps the welcome crab still. Applies immediately. |

General also holds the settings described under [Presentation and scrolling](#presentation-and-scrolling), [Notifications](#notifications) and [Automatic updates](#automatic-updates).

**Default effort** belongs to the saved Default model and is saved per model, so changing Default model switches the effort row to the new model. `max` is available only through `/effort` in a session. The row is read-only when the default model is unknown or offers no effort levels. Environment settings, workspace trust and organization caps can still change the effort a session runs with; the chat footer shows the applied level.

**Question timeout** submits a waiting question after the chosen idle time. Only answers you explicitly selected are sent, together with any notes; untouched questions are skipped. Keyboard, paste and mouse activity restart the interval, and the timer pauses while another screen has focus. Never waits for confirmation.

**Dialog expiry** cancels forwarded remote dialogs and held cross-session messages after the chosen time. Local permission prompts get no deadline. Reset restores 5 minutes. The `CLAUDE_CODE_USER_DIALOG_TIMEOUT_MS` environment variable takes precedence.

### Memory

| Setting | Key | Notes |
| --- | --- | --- |
| Auto compact | `autoCompactEnabled` | Compacts context automatically. |
| Idle compaction | `idleCompaction` | Off stops compaction while idle. On or Default does not enable the feature. User, Project and Local scopes; saved for the next session. |
| File checkpoints | `fileCheckpointingEnabled` | Records file snapshots for code rewind. |
| Automatic memory | `autoMemoryEnabled` | Reads and writes automatic memory for the project. |
| Directory | `autoMemoryDirectory` | User and Local only; shared Project settings cannot redirect memory. Supports `~/`. |
| Excluded instructions | `claudeMdExcludes` | One glob or absolute path per entry. Managed instructions cannot be excluded. |
| Plans directory | `plansDirectory` | Relative paths are relative to the project root. |

Idle compaction follows the SDK's settings cascade. Reset removes the preference at the selected scope and exposes the remaining scope or Default. Its saved On/Off value is a preference, not evidence that idle compaction is available or running; availability remains under SDK control.

### Permissions

| Setting | Key | Notes |
| --- | --- | --- |
| Default permission mode | `permissions.defaultMode` | User only. `default`, `plan`, `acceptEdits`, `auto`, `dontAsk` or `bypassPermissions`, subject to workspace trust and account restrictions. |
| Auto mode during planning | `useAutoModeDuringPlan` | User and Local only. |
| Disable bypass | `permissions.disableBypassPermissionsMode` | Removes the bypass permission mode. Reset removes this scope's restriction. |
| Block outside reads | `permissions.blockReadsOutsideWorkingDirectories` | Blocks Read, Grep, Glob and LSP outside working directories. On in any scope wins. |
| Allow rules, Ask rules, Deny rules | `permissions.allow`, `permissions.ask`, `permissions.deny` | See [Editing permission rules](configure.md#editing-permission-rules). |
| Additional directories | `permissions.additionalDirectories` | One directory per entry. |

### Sandbox

The Sandbox pane covers sandbox execution, filesystem paths, network domains, proxy ports and credential protection under the `sandbox.` key. Descriptions name the settings that apply to one platform only. Workspace trust, platform support and organization restrictions decide which saved values take effect, and can make rules and allowlists read-only.

These settings are User only: Disable filesystem isolation, Strict domain allowlist, TLS termination, Credential protection, Allow Apple Events and Ripgrep command.

### Hooks

| Setting | Key | Notes |
| --- | --- | --- |
| Definitions | `hooks` | See [Editing hooks](configure.md#editing-hooks). |
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
| Skip the /copy picker | `presentation.copyFullResponse` | On / Off | Copy the complete response directly. See [Copying responses](conversations.md#copying-responses). |

The first five are Claude settings and can be saved at User, Project or Local scope. The last two are app preferences and are always saved for the user.

**Timestamps and durations.** When Claude supplies no timestamp, a live message shows the locally observed time marked `(observed)`; resumed messages without a timestamp stay unstamped. With both settings On, the duration row also shows the completion time (`Done`). When the SDK reports no turn timing, a live response shows an `Observed` duration. Tool progress time is a lower bound, shown with `≥`, and task time is labelled separately; neither is added to the turn total. Resumed responses show a duration only when the session history contains one.

**Time format.** Auto follows the system locale. 12-hour shows AM/PM and 24-hour UTC adds a `Z` suffix. A custom strftime pattern written into a settings file is used and preserved; the picker offers only the four presets. A `timeZone` entry in a settings file, such as `Europe/Berlin`, sets the zone for local presets and custom patterns. An unknown zone name falls back to the system zone.

**Scrolling.** `Page Up` pauses following and reads earlier output; `Page Down` reads later output. `Ctrl+End` jumps to the latest output and resumes following when Auto-scroll is On. With Auto-scroll Off it jumps once and then holds that position. The reading position survives new output and window resizing. The mouse wheel belongs to the terminal's own scrollback unless the terminal forwards mouse events to the app.

## Footer

For the editing procedure and keys, see [Customize the footer](configure.md#customize-the-footer).

Permission mode, model, effort, Ultracode, fast mode, running subagents, working directory, Git branch and context remaining are independent fields. Hiding the model does not hide effort or Ultracode; hiding the directory does not hide the branch. Fields appear only when their session data applies: effort requires a model that supports it, Ultracode requires verified effective state, running subagents requires at least one non-ambient subagent of the current session still running (one count per task across resumes, including stopping requests; see [`/agents`](commands.md#app-owned-commands)), and branch requires a Git branch.

Permission and startup MCP-authentication notices remain at the far right and take priority over configured fields. On narrow terminals, paths and branches shorten first, then later fields are omitted, with the left group preceding the right group for overflow priority. The footer never wraps.

The layout is a user-level app preference saved as the `presentation.footer` array in the app-preferences file described under [Where settings are saved](#where-settings-are-saved). Each entry has an `id`, a `visible` boolean, a `side` of `left` or `right`, and an optional `color`; array order determines reading order within each group. The field IDs are `permissionMode`, `model`, `effort`, `ultracode`, `fastMode`, `runningSubagents`, `directory`, `gitBranch` and `contextRemaining`. Omitted fields remain available in the dialog but are hidden. An empty array hides all optional fields. Removing the setting restores the built-in layout.

For example, `{ "id": "model", "visible": true, "side": "left", "color": "cyan" }` sets the model color. Omitting `color` keeps its existing appearance. The palette IDs are `red`, `orange`, `amber`, `yellow`, `lime`, `green`, `emerald`, `teal`, `cyan`, `sky`, `blue`, `indigo`, `violet`, `purple`, `fuchsia`, `pink`, `rose`, `white`, `gray`, and `slate`.

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
- Only the edited setting is validated. An unrelated invalid value elsewhere in the file does not block the save, but it can still prevent Claude from loading the file; see `Not loaded` under [Reading the list](configure.md#reading-the-list).
- A change made to the same setting by another program is detected before writing. The app reloads and keeps your draft. An editor that writes at the same instant without honoring the app's lock file can still overwrite a save.

## Offline inspection

These read-only commands inspect redacted settings files without starting the TUI:

```bash
claude-rs config
claude-rs config show --which project-settings
claude-rs config export --output claude-rs-config.json
```

The output covers User, Project and Local settings and the preferences file. Malformed files are reported, not rewritten. The commands read the files as they are on disk: they do not combine scopes and do not include managed policy. See [Config Inspection](diagnostics.md#config-inspection).
