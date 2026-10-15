# Slash Commands

Claude Code Rust has app-owned slash commands and can also show slash commands advertised by the active Agent SDK session. App-owned commands are available from the Rust TUI itself; SDK-advertised commands depend on the current session and what the bridge reports.

Use `/docs commands` in the app to render the live merged command list into chat. That is the source to use when you want to know exactly which app-owned and SDK-advertised commands are available in the current session.

## App-Owned Commands

| Command | Usage | Purpose |
| --- | --- | --- |
| `/btw` | `/btw <question>` | Ask a contextual side question without adding it to the main conversation. |
| `/cancel` | `/cancel` | Cancel the active assistant turn. |
| `/compact` | `/compact` | Ask the active session to compact conversation context. |
| `/config` | `/config` | Open fullscreen settings. |
| `/memory` | `/memory` | Open the Memory settings pane. |
| `/permissions` | `/permissions` | Open saved permission defaults, rules and directories. |
| `/sandbox` | `/sandbox` | Open sandbox settings without toggling sandboxing. |
| `/hooks` | `/hooks` | Inspect and edit saved hook configuration without executing hooks. |
| `/copy` | `/copy` | Copy the last completed response, or choose a code block from it. |
| `/diff` | `/diff` | Review Git changes and full file contents without changing files or the index. |
| `/docs` | `/docs <mode\|models\|shortcuts\|commands\|agents>` | Render command, shortcut, model, mode, or subagent help into chat. |
| `/agent` | `/agent <name\|reset>` | Change the main-thread agent for the active session. Applies on the next turn. |
| `/effort` | `/effort <low\|medium\|high\|xhigh\|max\|reset>` | Change thinking effort for the active session. |
| `/thinking` | `/thinking <on\|off\|reset>` | Set the thinking preference for the active session or restore its inherited preference. |
| `/ultracode` | `/ultracode <on\|off\|status>` | Enable, disable, or inspect Ultracode for the active session. |
| `/fast` | `/fast [on\|off]` | Toggle fast mode or explicitly enable/disable it for the active session. |
| `/help` | `/help` | Open the fullscreen Help tab. |
| `/mcp` | `/mcp` | Open MCP status and authorization. |
| `/plugins` | `/plugins` | Open plugin management. |
| `/status` | `/status` | Open session and account status. |
| `/usage` | `/usage` | Open quota and usage information. |
| `/login` | `/login` | Run Claude CLI authentication and reconnect the session. |
| `/logout` | `/logout` | Run Claude CLI logout and clear the active authenticated session. |
| `/mode` | `/mode <id>` | Switch to a mode advertised by the active session. |
| `/model` | `/model <id>` | Switch to a model advertised by the active session. |
| `/new-session` | `/new-session` | Start a fresh bridge session in the current folder. |
| `/resume` | `/resume` | Search and select a session in the fullscreen picker. |
| `/rewind` | `/rewind <user_message_uuid> <both\|conversation\|code>` | Restore conversation, code, or both to a previous user message. |

Enter completes the selected slash command or argument and submits it when ready. Tab completes without submitting. Exact command names appear ahead of substring matches; arrow keys can select a different match. App commands that still need required arguments stay in the composer; commands with argument choices show those choices, while `/btw` continues as ordinary question text. An empty suggestion menu allows Enter to submit the typed input for normal validation.

Editing the draft cancels a pending submission. Editing, completion, and submission keys cannot change or send the draft while pasted text is awaiting insertion, including when suggestions are open or keys are remapped. Suggestions refresh after the pasted text is inserted.

Completing the name of a command with no arguments or optional arguments closes the popup and returns focus to the draft. For commands accepting arguments, pressing Tab again requests argument completion explicitly. When the SDK provides only an argument hint, Tab displays that hint without changing or sending the draft; Enter sends the typed command for SDK validation. SDK hints are descriptive text, so their brackets never determine whether the app allows submission. Empty hints and `none` or `[none]` do not produce argument help.

Space inserts a literal separator without accepting the highlighted suggestion. It ends command-name completion and continues argument assistance only when the app requires arguments, or when you explicitly requested argument help. There is no special Space dismissal state. Escape dismisses the current menu; argument edits, cursor movement within arguments, and metadata refreshes keep it closed. Tab requests completion again, and editing the command name starts a new completion interaction. A command is recognized only at the start of the draft, allowing leading whitespace; slashes on later lines remain literal prompt or argument text.

The command menu includes the full merged inventory and scrolls as you move through it. SDK commands remain available after `/clear` changes the session identifier. In SDK sessions that advertise `/clear [name]`, `/new` and `/reset` are aliases; the optional name labels the conversation you are leaving. `/new-session` is the separate app command that starts a fresh bridge session. App command definitions take precedence when the SDK advertises the same name.

## Resuming sessions

`/resume` opens the same fullscreen session picker used by `claude-rs resume` at startup. Type to filter by session name, session ID, or branch name; search matches partial text without case sensitivity, and pasted text also searches. Up/Down selects a matching session, Enter resumes it, and Escape closes the picker without switching sessions. Backspace edits the search. The scrollable list shows the SDK-provided title, last activity, branch when available, session ID, and a marker for the current session. All available sessions for the project directory are searchable.

Refreshing session metadata keeps the selected session by ID when it still matches. If it disappears or stops matching, the next result at that position is selected, or the previous result at the end. An empty result list stays open and cannot resume an unrelated session; edit the search or press Escape. Right activates the `fork` action and opens the message picker; Left/Escape returns to the filtered session list. Message previews wrap to the terminal width, preserve paragraph breaks, and show up to four text lines with a `more` indicator for longer messages. Up/Down selects a message, and Enter forks before it. Right opens the selected message for full-text reading with Up/Down scrolling; Left/Escape returns to the message list.

Session selection happens inside the picker; `/resume` has no inline session suggestions or chat arguments. Ordinary command-name completion still completes `/resu` to `/resume`. For direct resume by ID from the terminal, use `claude-rs resume <session_id>`.

## Reviewing Git changes

`/diff` opens a fullscreen, read-only Git viewer for the repository containing the current directory. The left pane shows changed files in an expanded tree; folders are not selectable. Each file shows its status and line counts, with additions in green and removals in red; zero counts are hidden. Totals appear above the tree. The right pane previews the selected file. Binary files, submodules, unavailable files, clean repositories, and Git read failures are shown explicitly. Repositories without a first commit are supported.

Press `s` to choose a comparison with Up/Down and Enter; Escape cancels. HEAD compares the last commit with current files, Staged compares the last commit with the Git index, and Unstaged compares the index with current files. HEAD and Unstaged also include untracked files that Git does not ignore. This view shows repository changes, not session or turn history.

Refresh keeps the current pane focused and retains the selected change when it remains available. If that change disappears, the next remaining file takes its place, or the previous file when the selection was at the end. If no changes remain, the viewer clears the old preview and stays open with an empty-state message.

| Key | Action |
| --- | --- |
| Up/Down | Select files in the left pane or scroll content in the right pane. |
| Enter or Right | Focus the content pane. |
| Left or Escape in the content pane | Return focus to the file tree. |
| Tab / Shift+Tab | Select the next/previous file without changing the focused pane. |
| `v` | Toggle the diff and full file contents. Staged shows the indexed version; deleted files show their contents before deletion. |
| `[` | Jump to the previous diff hunk (changed block). |
| `]` | Jump to the next diff hunk (changed block). |
| `r` | Refresh, retaining the selected file when it remains in the comparison. |
| Escape in the file tree | Return to the conversation. |
| Ctrl+C | Close the viewer and return to the conversation. |
| Ctrl+Q | Quit the app. |

## Copying responses

`/copy` copies the last completed assistant response as its original Markdown, without thinking or tool output. Responses loaded from session history can be copied too. If the response contains code blocks, choose Full response or a code block with Up/Down, then Enter; Escape cancels. Enable Skip the /copy picker in `/config` to copy the complete Markdown response directly. Responses without code blocks copy directly. If the clipboard is unavailable, the picker stays open so you can retry. The command can copy an earlier completed response while a new response is still running; it does not submit a model request.

## Side Questions

`/btw` treats the complete non-empty text after the command as one question, including internal spaces and line breaks. Side questions are shown in a separate status field while pending and produce a bordered `Claude · BTW` transcript card only when answered. During an active turn, the card is inserted at the current point in the agent's output, with subsequent output below it; when idle, it is a standalone transcript entry with the same design. You can continue using the normal composer while up to ten side questions are unresolved; Rust queues and dispatches them one at a time in submission order. Each question sees main-conversation context at SDK dispatch time, not a snapshot from submission. Failures free capacity immediately and remain visible briefly as error rows.

## SDK-Advertised Commands

The active SDK session can advertise additional slash commands. These are not documented as a fixed table here because they can change with SDK behavior, session capabilities, account state, and future upstream changes.

Use:

```text
/docs commands
```

to inspect the current session's full command list. The output includes app-owned commands and SDK-advertised commands, with descriptions when the SDK provides them.

## Session Name

The Status tab shows the session name. Its rename draft (`r`) starts from that name, and title generation (`g`) uses it as the description when there is one.

The name is whatever the Agent SDK's session API reports as the session's title; claude-rs keeps no title of its own. It is read when a session connects or resumes, when the session is replaced, when the conversation resets, after each turn, and after a Status-tab rename or title generation. A `/rename` typed in the composer therefore shows once its turn completes. For a session nobody named, the API reports a generated title instead, so such a session shows that title after its first turn.

Commands that Claude Code runs locally, such as `/rename`, `/color` or `/context`, reply in the chat under the command as they do in Claude Code. `/color` runs in Claude Code and its reply shows, but Claude Rust does not draw a session colour.

## Session Commands

`/ultracode on` enables session-scoped dynamic-workflow orchestration while retaining the current thinking effort; `/ultracode off` disables it. Both require an idle turn. `/ultracode status` can be used during a turn and reports the latest verified SDK snapshot: on, available and off, requested but unavailable, unavailable and off, or unknown. Availability depends on SDK session capabilities and model support. The footer shows `Ultracode` only when it is verified as effective. Changing effort preserves active Ultracode, and changing models refreshes its state. This command does not persist a preference or control the one-turn `ultracode` keyword trigger.

If the SDK accepts `/ultracode on` but reports Ultracode unavailable, the command shows an error explaining that the request was saved but Ultracode remains inactive. The verified requested-but-unavailable state remains visible through `/ultracode status`, which reports it as information.

`/model`, `/mode`, `/effort`, `/thinking`, `/agent`, `/fast`, and `/ultracode` change the current session; `/config` edits saved defaults for new sessions. Choices are checked against SDK capabilities and acknowledged only after the SDK accepts them. Model, effort, fast mode, and Ultracode show reported runtime results, including caps or fallbacks, rather than echoing requests. The starting permission mode comes from SDK initialization, before the first prompt. Saved permission defaults remain inherited by the native session. Both `/mode` and the mode-cycle shortcut keep the confirmed mode visible while awaiting SDK acceptance; a rejected change leaves it unchanged.

`/effort reset` uses the current model's native session default; it does not reload the saved per-model effort. `max` is session-only. `/thinking reset` removes the temporary override and restores the inherited thinking preference. Thinking is a preference, and the model may still require or restrict thinking. `/agent reset` clears the active main-thread agent rather than restoring its saved default.

`/fast` changes fast mode only for the active session. It does not rewrite the persisted Fast mode setting. Use the settings surface to choose the fast-mode preference applied when future sessions start. If an accepted change cannot be verified, the footer shows `FAST:?`; retry explicitly with `/fast on` or `/fast off`.
