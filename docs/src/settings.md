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

The settings surface is session-aware. Some tabs need an active bridge session before they can show live SDK-backed state.

## Usage

Open the settings surface with any command in the table above. Each command opens the same fullscreen surface but targets a different starting tab.

The tab order is:

```text
Settings -> Plugins -> Status -> Usage -> MCP -> Help
```

Use `Tab` to move to the next tab and `Shift+Tab` to move to the previous tab. The active tab can also have its own navigation and action keys. For example, the Settings tab edits persisted settings, the Plugins tab navigates plugin lists and overlays, the MCP tab opens server actions and authorization flows, and the Usage and Status tabs refresh live session-backed data.

Inside Settings, a second tab row contains General, Memory, Permissions, Sandbox, Hooks, and Workflows & Worktrees. Models and everyday preferences stay in General. Up from the first item focuses search; Up again focuses the pane tabs. Down returns through search to the list. Left/Right changes panes only while the pane row has focus; it changes supported setting choices while the list has focus. Home/End and Page Up/Down navigate the current list. Tab/Shift+Tab still changes top-level tabs. An open editor owns its keys and keeps its captured save scope.

Press `/` to search the current pane. Search matches labels, descriptions and canonical setting IDs, using case-insensitive matching for all entered words. It never searches another pane or saved hook commands, credential content or environment values. Enter in search focuses results; Enter on a result clears the query and reveals its setting without editing. Esc clears search and restores the browsing position; Esc without a query closes the window. Choosing another pane clears search and restores that pane's remembered selection. Leaving Settings for another top-level tab preserves browsing and search state.

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
| App preferences | OS config directory, `claude-code-rust/settings.json` | Personal presentation and notification preferences plus updater state, separate from Claude settings. |

Set `CLAUDE_CONFIG_DIR` before startup to isolate the Claude profile. User settings use `<directory>/settings.json`, trust uses `<directory>/.claude.json`, and file credentials use `<directory>/.credentials.json`. Sessions, plugins, and authentication inherit that directory. App preferences and diagnostics retain their normal OS locations. Existing settings sources use paths reported by the SDK; missing writable sources use the corresponding profile or working-directory path.

Settings load automatically when the app's session starts; the user does not connect a session manually. The Settings tab shows aligned setting/value columns, highlights the selected row, and displays the selected row and total, such as `1/24`. Saved values and `Default` use distinct styles. The selected row's description and value saved at the chosen scope appear below the list, including feedback when another scope overrides that value. Read-only rows show their restriction and appropriate controls. Active session information belongs in Status and the chat footer. Source metadata remains available in the bridge contract; SDK provenance is provided at top-level key granularity and is not proof that a particular leaf is an enforced policy. `resolveSettings()` does not execute `policyHelper`.

## Editing saved settings

Use Up/Down to select and scroll, Home/End to jump to the first/last setting, `s` to cycle user/project/local scope, and `r` to refresh. Left/Right cycles fixed choices and saves the change; Space is equivalent to Right. These settings have no separate dialog. Booleans display as On/Off. Space opens a text editor for free-text fields such as Language; Enter saves, Ctrl+R resets, and Escape cancels. Delete on a row resets its saved value at the selected scope. Reset displays the remaining scope's value or `Default`; it does not restore an earlier edit or save a replacement value. `Default` is a display label for an unset resolved preference, not a literal SDK value. The bridge validates values and writable scopes against its single catalog immediately before saving. A pending save keeps the last acknowledged value visible until the result arrives.

Compact windows keep the selected row, value, and essential controls visible. Below 30 columns or 12 rows, the settings surface asks the user to resize instead of showing an unusable layout; Escape cancels an editor or closes the list.

Each edit reads the current file and patches only its target key. Unrelated keys and siblings survive. Reset removes the key and empty ancestor objects; it does not write a substitute default. Missing files may be created; invalid JSON, non-object documents, unreadable files, and symlinks are blocked for editing without backup, repair, or replacement. Local saves in Git repositories add a literal exclusion to Git's private `info/exclude` when needed and reject an already tracked local settings file.

Validation applies to the setting being edited, not to unrelated fields or the whole document's schema. Unknown keys, newer settings, and unrelated invalid values are preserved. Claude decides which settings it loads: some invalid entries are ignored while other schema errors exclude the entire file. A nonempty file absent from the resolved sources is labeled `not loaded`, without claiming the cause. Targeted saves and resets remain available for readable JSON objects. If a set is saved but the resulting file still does not load, the editor reports the successful save separately from the loading problem; correcting the offending value can restore loading. Reset remains a successful deletion even when only unrelated ignored or invalid fields remain. Local temporary files containing settings data are also protected by the private Git exclusion before they are written.

A revision for the displayed target detects conflicting edits. Unrelated edits are incorporated into the fresh document; a conflict refreshes the snapshot while preserving the editor draft. Writes use a cooperative lock, byte rechecks, and atomic replacement. An external editor that does not honor the lock can still race between the final recheck and replacement; this is not filesystem compare-and-swap.

The window states once that saved changes apply to new sessions and marks immediate exceptions. The app does not turn a settings save into a session flag override. Use `/model`, `/mode`, `/effort`, `/thinking`, `/agent`, `/fast`, or `/ultracode` for their existing session actions. Host presentation preferences, `respectGitignore`, and `prefersReducedMotion` apply immediately after acknowledgement. New sessions and resumed sessions retain native settings inheritance. The footer's effort badge reflects the SDK's applied level at initialization and after session changes, including model restrictions and organization caps. Per-turn init observations supersede earlier reads. If the SDK reports no discrete level, the footer shows the model without an effort suffix.

## Structured editors

Lists, including permission Allow/Ask/Deny rules, open an item editor. Press `a` to add, Enter/Space to edit an item, and Delete to remove it. Hooks show individual event/matcher/action entries, with guided creation and direct field editing for command, prompt, agent, HTTP and MCP-tool actions. Sandbox forms cover TLS certificate paths, ripgrep command/arguments, violation mappings, credential files/environment variables, AWS pairs and request policies. Forms preserve unknown fields and sibling entries; native validation remains authoritative.

An editor loads entries saved at the selected scope, rather than reconstructing a merged list. Other sources contribute separately. Ctrl+S submits the draft, Ctrl+R resets the whole selected setting at that scope, and Esc returns to the parent or cancels the draft at its root. Removing the last item saves an explicit empty collection; reset removes the setting key. Field input uses Enter to accept, Ctrl+U to clear, Ctrl+J to insert a newline, and Esc to cancel the field. Optional fields can be removed with Delete. Configured values may be inspected in read-only scopes; saving remains unavailable.

Ctrl+J outside a field opens advanced JSON for the same draft; Ctrl+F returns to the form after valid JSON. There is one mutation and persistence path. Invalid JSON or a native validation failure retains the draft. A conflict loads the latest revision and requires deliberate retry. A changed project/settings context blocks saving until the editor is canceled and reopened. Delayed acknowledgements belong to their originating mutation and cannot close another editor. Inspection and validation never execute hooks or credential helpers or resolve secret files and environment variables; ordinary header and arbitrary argument summaries are redacted.

## Current catalog

The catalog exposes 77 settings, organized into one alphabetical list per pane. The bridge supplies category assignments, alphabetical ordering and structured-editor metadata; Rust uses that one catalog for rendering, navigation, search and mutation targets. It enables validated boolean and string editors for auto-compaction, flagged-message model switching, thinking, fast mode, prompt suggestions, checkpoints, workflows, keyword triggers, workflow size, default permission mode, worktree base reference, file-picker Gitignore behavior, output style, language, default model, and reduced motion. The catalog describes per-row writable scopes; not every setting can be saved at every scope. Default model cycles only through the session's SDK-advertised choices; it does not accept typed model IDs. Existing externally configured values remain preserved on disk until explicitly changed or reset. Output style still accepts custom style names. Default agent uses the SDK-advertised agent inventory.

Default effort is one inline choice control for the currently saved Default model, using the same Left/Right/Space interaction as Default model. Its choices come from that model's SDK capabilities, excluding session-only `max`. Saving writes `modelSettings.<canonical model>.effortLevel` at the chosen scope and preserves other models' preferences and sibling fields. Changing Default model retargets the effort control; mutation context checks prevent an outdated edit from silently targeting a different model. An unsupported or unresolved model leaves the effort editor read-only. Reset removes only that model's effort at the selected scope. SDK model metadata determines the target; aliases and dated/context/provider spellings are normalized to Claude's canonical key, while unknown model identities remain read-only. The saved snapshot is the SDK's file cascade; environment settings, top-level effort fallbacks, trust, and policy caps can still change the running effort, which is shown in the footer.

`Default` means no configured value was returned for this preference. It does not promise On or Off. The installed SDK's file resolver does not fill in built-in defaults; its applied-settings response exposes selected session values rather than effective defaults for the whole catalog. Keep this label until the fallback for an individual setting can be determined reliably, without treating a current-session override as its default.

Continuation at usage limits and cross-session inbound policy remain read-only until their host workflows are completed. The installed runtime gates native automatic continuation on interactive mode; the SDK session cannot activate that coordinator by loading the preference. Auto mode during planning is editable as a saved preference, subject to native capabilities. Themes and Vim/editor modes are omitted. Notification and automatic-update controls remain in General; their runtime behavior is described below.

When Reduce motion is On, active thinking, tool execution, compaction, cancellation, and pending commands use a static diamond (`◆`) instead of an animated spinner. Animation-only redraws stop; incoming state changes still update normally. This setting is ready and applies immediately after saving, with reset following the remaining scopes or Default.

## Presentation and scrolling

These settings apply after saving, including to the retained conversation:

| Setting | Choices | Behavior |
| --- | --- | --- |
| Auto-scroll | On / Off | Follow new output when On; hold the reading position when Off. |
| Show tips | On / Off | Show tips during active turns. Off hides tip text; activity and the Claude heading remain visible. Default: On. |
| Show message timestamps | On / Off | Show the first available message time beside its role label. |
| Show turn duration | On / Off | Show one total elapsed duration per completed response, and tool/task elapsed observations when available. |
| Time format | Auto / 12-hour / 24-hour / 24-hour UTC | Format clocks without changing elapsed durations. |
| Show activity in tab title | On / Off | Show a busy/idle icon beside the folder name in the terminal tab title. Reduced motion keeps activity static. |
| Skip the /copy picker | On / Off | Copy the complete response directly, or choose the response or a code block. |

Turn durations and message timestamps default to Off when no saved value is present. Saved On/Off choices take precedence. A completed response shows one total elapsed duration when enabled; completion time (`Done`) is included only when message timestamps are enabled. API timing is retained internally.

SDK timestamps retain their original times during session resume. When no native timestamp is supplied, live messages use a locally observed time labeled `(observed)`; resumed messages without timestamps remain unstamped. Turn elapsed time and API time are separate SDK result measurements. Without a result measurement, a completed live response shows an `Observed` duration from a monotonic clock. Tool progress elapsed time is a lower bound (`≥`), and task usage time is labeled separately; neither is added to turn elapsed time. Public session history may omit result timing, so resumed responses do not invent old durations.

Auto follows the system locale. Explicit 12-hour clocks show AM/PM; 24-hour UTC shows a `Z` suffix. Valid custom strftime patterns already present in settings files are consumed and preserved, while the picker offers the four presets. An externally saved IANA `timeZone`, such as `Europe/Berlin`, controls local presets and custom patterns; an unknown name falls back to the system zone. The UTC preset always uses UTC.

Page Up pauses following and reads earlier output; Page Down reads later output. Ctrl+End jumps to the latest output and resumes following when Auto-scroll is On. With Auto-scroll Off, it jumps once and continues holding that position. Reading remains anchored through new output and resize/reflow within retained history. These actions use the existing keybinding catalog and appear in shortcut help. Ordinary mouse-wheel behavior belongs to terminal scrollback unless the terminal delivers mouse events to the application; use the page keys for application-controlled reading.

`/copy` uses the last completed assistant response's original Markdown, including responses loaded from history; it excludes thinking and tool output. With code blocks and the picker enabled, Up/Down selects the full response or an individual fenced/indented code block, Enter copies, and Escape cancels. Without code blocks, it copies the response directly. Clipboard failures keep the picker open for retry, and an empty conversation displays a short explanation.

The four Claude-compatible presentation preferences use the resolved user/project/local cascade. Terminal-tab status and copy-picker preference are personal User-only app preferences under `presentation.showStatusInTerminalTab` and `presentation.copyFullResponse`. They appear in the same editor and use the same targeted save/reset acknowledgements. Native global preferences are not a fallback. The updater writes only its own fields and honors the shared document lock so its saves preserve presentation edits.

## Behavioral and structured settings

Question timeout offers Never, 60 seconds, 5 minutes, or 10 minutes. After the configured idle time, the current question submits only answers explicitly selected by navigation or toggling, plus any notes; untouched questions are skipped. Keyboard, paste, and mouse activity restart the idle interval. The timer pauses while another screen or input owns focus. It starts afresh when the question regains focus. Timeout comes from trusted user or managed settings, rather than a checked-in repository preference. The default waits for confirmation.

Dialog expiry offers 60 seconds, 5 minutes, 10 minutes, or Never. Reset restores the native fallback of 5 minutes. It expires forwarded remote dialogs and held cross-session messages with safe cancellation; ordinary local-only permission prompts have no added deadline. `CLAUDE_CODE_USER_DIALOG_TIMEOUT_MS` takes precedence. Native cancellation removes the affected question, permission, or user dialog from the interaction queue, preserving other pending interactions and the composer draft.

Permission rules and additional directories, memory exclusions, worktree directories, sandbox paths/domains, and hook allowlists use the item editor described above. Many native lists merge across scopes, so an empty list does not necessarily remove rules contributed elsewhere. List rows show the selected scope's item count. Change scope on the settings list with `s`; an open editor keeps its scope fixed until it closes.

Hooks, sandbox credentials, ignored violations, TLS termination, and the ripgrep helper open structured forms with advanced JSON access. Object rows show their entry count. Syntax errors and native schema errors retain the draft for correction. Validation checks only the edited leaf through the installed SDK's public resolver; it does not run hooks, read credential contents, or change unrelated settings. Organization restrictions can make permission rules, hooks, and sandbox allowlists read-only. Numeric proxy ports accept whole numbers from 1 through 65535.

For example, a Hooks definitions value can be:

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

Supported hook actions include command, prompt, agent, HTTP, and MCP tool actions. Event names come from the installed SDK. Saving replaces the `hooks` object at the chosen scope; hooks from other sources retain their native merging behavior. Disable all hooks is a separate boolean. Opening or validating hook definitions does not execute them.

Workflow size is an advisory agent-count guideline: Small aims below 5, Medium below 10, Large below 50, and Unrestricted supplies no guideline. Workflow availability still depends on the account and runtime. Worktree settings configure fresh/head base reference, symlink directories, sparse-checkout paths, and background isolation with worktree/none choices. Memory controls configure automatic memory, its directory, excluded instruction paths/globs, and the plans directory. Project settings cannot redirect automatic memory. Sandbox descriptions identify platform restrictions; native enforcement and workspace trust still determine which saved values take effect.

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


## Notifications

Notifications apply immediately after an acknowledged save. They alert only when the terminal reports that it is unfocused; terminals without focus reporting remain silent. These controls appear in General under Files, notifications and updates:

| Setting | Choices / default | Behavior |
| --- | --- | --- |
| Notification method | Auto, iTerm2, Terminal bell, iTerm2 with bell, Kitty, Ghostty, Disabled; default Auto | Choose local delivery. Disabled silences every category. |
| Notify when input is needed | On / Off; default On | Alert for a waiting permission, question or user dialog. Duplicate pending requests do not alert again. |
| Notify when Claude requests it | On / Off; default On | Allow local alerts from successful proactive `PushNotification` tool results, when the native account/runtime exposes that tool. |
| Notify when a turn finishes | On / Off; default On | Alert after an active turn completes; cancellation, errors, idle command results and history do not trigger completion alerts. |

Auto detects iTerm2/Ghostty OSC 9 or Kitty OSC 99 support; otherwise it preserves desktop notification plus terminal bell delivery. A terminal-specific method uses its protocol only on that terminal and falls back to an OS desktop notification elsewhere. iTerm2 with bell adds the bell; Terminal bell sends only BEL. The OS backend runs outside the TUI loop and may be unavailable over SSH. Terminal alert text strips control characters before constructing escape sequences. Protocol references: [iTerm2](https://iterm2.com/documentation-escape-codes.html), [Ghostty](https://ghostty.org/docs/vt/osc/9), and [Kitty](https://sw.kovidgoyal.net/kitty/desktop-notifications/).

The three category toggles are personal app preferences under `notifications.actionsRequired`, `notifications.modelDirected`, and `notifications.turnComplete`. Notification method saves `preferredNotifChannel` in User Claude settings and consumes its resolved value. The category toggles do not change the native mobile-push settings `inputNeededNotifEnabled` or `agentPushNotifEnabled`. Reset removes the selected saved value and resumes inheritance or the documented default.

Ordinary native CLI notices remain visible in the transcript, including high-priority notices; their SDK payload does not identify them as requests for a local alert. Proactive tool reports distinguish mobile delivery from local delivery: a mobile push alone does not suppress our local alert, but an upstream local delivery does. Native configuration/presence suppression remains respected; `no_transport` allows local fallback. Missing local-delivery reports are not guessed. Repeated tool results and replayed history never queue alerts for later delivery after focus or settings change. Reused native notice keys can produce a new visible notice when the UUID changes.

For notification diagnostics, use `--diagnostics-preset full`, `runtime` or `session`, or include `app.notify=debug` in a custom log filter. The `app.notify` records show observed terminal focus, SDK origin and delivery reports, duplicate/replay handling, category and method decisions, and terminal/desktop transport outcomes. Session and interaction identities follow delivery through the background desktop thread. Question and notification text are omitted. A successful terminal write or accepted OS request confirms submission to the transport; it does not prove that the terminal or operating system displayed or sounded an alert.

## Automatic updates

Automatic updates is a personal app setting, saved as `updates.autoInstall` in the app settings file. It defaults to Off. Off keeps the existing startup update window and manual installation choices. On skips that window and installs an available newer claude-rs version after a normal exit, using the running installation's detected script or npm method. Source builds and unrecognized installations are not automatically replaced. The setting does not update an external Claude CLI installation or independently change the bundled SDK.

Update checks run in both modes, retaining the existing 24-hour cache. Explicit `--no-update-check` or `CLAUDE_RUST_NO_UPDATE_CHECK` disablement still takes precedence and also prevents automatic installation. The updater reads the current saved setting again at exit, so saving Off or resetting cancels automatic installation. Forced shutdown and application errors skip automatic installation. Installation errors are reported and saved without opening another window; the session remains resumable.

## Structured editor interactions

Each pane has one alphabetical list without subsection headings or column headers. Settings and Plugins share the same bordered search field. Escape appears with the other keyboard hints and wraps only when needed.

For Permissions, open Allow rules, Ask rules, Deny rules or Additional directories at the desired save scope. Press `a` to add one entry, Enter to edit an entry, and Delete to remove it. Rule inputs include tool-rule examples. Enter accepts the field into the draft; Ctrl+S saves the collection. Removing its final entry saves an explicit empty list; resetting the setting removes that scope's key. Other scopes can still supply rules.

For Hooks, Definitions shows a flat list with each hook's event, matcher and action. Press `a` to add a hook: choose an event from a visible list, enter an optional matcher, choose an action, enter its required fields and review. Enter on Review adds the hook to the draft and opens its optional fields; Ctrl+S persists it. Esc goes back within creation and cancels adding at the first step. Enter edits an existing hook; `m` edits its group's matcher, with a warning that sibling hooks share it; Delete removes only that hook.

For Sandbox, select a field or collection. Lists and maps offer Add/Edit/Remove; objects offer field editing and field reset. Required fields have `*` and a `* Required` legend; optional fields have no marker. Fixed choices show a visible picker navigated with Up/Down and accepted with Enter. Inputs explain expected paths, variable names, command arguments or patterns. Credential entries take names and paths, never secret values. Edits remain in the draft until Ctrl+S and receive native validation on save.

Config text fields accept AltGr characters, including `|` in matchers and `\` in paths. Native validation checks supported structured fields without discarding additional saved object fields; invalid supported values keep the draft open instead of changing the file.
