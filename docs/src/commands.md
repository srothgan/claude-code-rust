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
| `/resume` | `/resume [session_id]` | Open the session picker, or resume a supplied session id. |
| `/rewind` | `/rewind <user_message_uuid> <both\|conversation\|code>` | Restore conversation, code, or both to a previous user message. |

Enter completes the selected slash command or argument and submits it when ready. Tab completes without submitting. Exact command names appear ahead of substring matches; arrow keys can select a different match. App commands that still need required arguments stay in the composer; commands with argument choices show those choices, while `/btw` continues as ordinary question text. An empty suggestion menu allows Enter to submit the typed input for normal validation.

Editing the draft cancels a pending submission. Editing, completion, and submission keys cannot change or send the draft while pasted text is awaiting insertion, including when suggestions are open or keys are remapped. Suggestions refresh after the pasted text is inserted.

Completing the name of a command with no arguments or optional arguments closes the popup and returns focus to the draft. Optional arguments remain supported: `/resume` opens the session picker, and `/resume <session_id>` resumes directly. Pressing Tab again requests argument completion explicitly. When the SDK provides only an argument hint, Tab displays that hint without changing or sending the draft; Enter sends the typed command for SDK validation. SDK hints are descriptive text, so their brackets never determine whether the app allows submission. Empty hints and `none` or `[none]` do not produce argument help.

Space inserts a literal separator without accepting the highlighted suggestion. It ends command-name completion and continues argument assistance only when the app requires arguments, or when you explicitly requested argument help. There is no special Space dismissal state. Escape dismisses the current menu; argument edits, cursor movement within arguments, and metadata refreshes keep it closed. Tab requests completion again, and editing the command name starts a new completion interaction. A command is recognized only at the start of the draft, allowing leading whitespace; slashes on later lines remain literal prompt or argument text.

The command menu includes the full merged inventory and scrolls as you move through it. SDK commands remain available after `/clear` changes the session identifier. In SDK sessions that advertise `/clear [name]`, `/new` and `/reset` are aliases; the optional name labels the conversation you are leaving. `/new-session` is the separate app command that starts a fresh bridge session. App command definitions take precedence when the SDK advertises the same name.

## Side Questions

`/btw` treats the complete non-empty text after the command as one question, including internal spaces and line breaks. Side questions are shown in a separate status field while pending and produce a bordered `Claude · BTW` transcript card only when answered. During an active turn, the card is inserted at the current point in the agent's output, with subsequent output below it; when idle, it is a standalone transcript entry with the same design. You can continue using the normal composer while up to ten side questions are unresolved; Rust queues and dispatches them one at a time in submission order. Each question sees main-conversation context at SDK dispatch time, not a snapshot from submission. Failures free capacity immediately and remain visible briefly as error rows.

## SDK-Advertised Commands

The active SDK session can advertise additional slash commands. These are not documented as a fixed table here because they can change with SDK behavior, session capabilities, account state, and future upstream changes.

Use:

```text
/docs commands
```

to inspect the current session's full command list. The output includes app-owned commands and SDK-advertised commands, with descriptions when the SDK provides them.

## Session Commands

`/ultracode on` enables session-scoped dynamic-workflow orchestration while retaining the current thinking effort; `/ultracode off` disables it. Both require an idle turn. `/ultracode status` can be used during a turn and reports the latest verified SDK snapshot: on, available and off, requested but unavailable, unavailable and off, or unknown. Availability depends on SDK session capabilities and model support. The footer shows `Ultracode` only when it is verified as effective. Changing effort preserves active Ultracode, and changing models refreshes its state. This command does not persist a preference or control the one-turn `ultracode` keyword trigger.

If the SDK accepts `/ultracode on` but reports Ultracode unavailable, the command shows an error explaining that the request was saved but Ultracode remains inactive. The verified requested-but-unavailable state remains visible through `/ultracode status`, which reports it as information.

`/model`, `/mode`, `/effort`, `/thinking`, `/agent`, `/fast`, and `/ultracode` change the current session; `/config` edits saved defaults for new sessions. Choices are checked against SDK capabilities and acknowledged only after the SDK accepts them. Model, effort, fast mode, and Ultracode show reported runtime results, including caps or fallbacks, rather than echoing requests. The starting permission mode comes from SDK initialization, before the first prompt. Saved permission defaults remain inherited by the native session. Both `/mode` and the mode-cycle shortcut keep the confirmed mode visible while awaiting SDK acceptance; a rejected change leaves it unchanged.

`/effort reset` uses the current model's native session default; it does not reload the saved per-model effort. `max` is session-only. `/thinking reset` removes the temporary override and restores the inherited thinking preference. Thinking is a preference, and the model may still require or restrict thinking. `/agent reset` clears the active main-thread agent rather than restoring its saved default.

`/fast` changes fast mode only for the active session. It does not rewrite the persisted Fast mode setting. Use the settings surface to choose the fast-mode preference applied when future sessions start. If an accepted change cannot be verified, the footer shows `FAST:?`; retry explicitly with `/fast on` or `/fast off`.
