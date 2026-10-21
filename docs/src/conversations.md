# Work with conversations

Use these tasks after [your first session](usage.md). Shell commands start or resume the app; slash commands work in the conversation composer.

## Continue from the shell

Run `claude-rs --continue` in the project directory to continue its most recent session, including sessions from its Git worktrees. If none exists, it starts a new session. To choose a conversation instead, run `claude-rs --resume`. See [resume syntax](cli.md#resume-or-continue) for IDs and initial prompts.

## Resuming sessions

1. Enter `/resume` in the composer, or run `claude-rs resume` from your shell.
2. Type part of a session name, ID or branch to filter the list. Search ignores case; pasted text also searches, and Backspace edits the query.
3. Use Up/Down to select a session. Check its title, last activity, branch and ID before opening it; the current session has a marker.
4. Press Enter to resume, or Escape to return without switching. The resumed conversation displays its saved history.

All available sessions for the project directory are searchable. If no sessions match, edit the query or close the picker. A metadata refresh keeps the selected session by ID when possible; if it disappears, selection moves to the next result or the previous result at the end.

`/resume` takes no chat arguments or inline session suggestions. Command-name completion still accepts `/resu`. For a known ID, use the [shell resume syntax](cli.md#resume-or-continue).

## Fork from a message

1. Open the session picker and select the conversation to branch from.
2. Press Right to open its fork action and message list.
3. Select a message with Up/Down. Press Right to read it in full, then Left or Escape to return to the message list.
4. Press Enter to fork before the selected message. Left or Escape in the message list returns to the filtered session list.

Previews preserve paragraph breaks and show up to four wrapped lines with a more indicator. Saved commands appear as readable command records and their output as notices. Internal command context, compaction summaries and background-task notifications do not appear as fork targets. Ordinary conversation XML remains literal, and the source session's saved content is unchanged.

## Exporting the conversation

1. Enter `/export` to use an automatic filename, or `/export ./session notes.md` to choose a path.
2. Wait for the completion notice and check the absolute path it reports.
3. Open the Markdown file and review its contents before sharing it. Conversation text and tool results can contain private project information.

`/export [path]` writes the session's saved history to a Markdown file. The history comes from the Agent SDK's saved transcript, translated by the same code a resume uses, not from the chat on screen, so it includes messages the app no longer keeps in memory and history from before a compaction.

### Destination and filenames

Without a path the file is `export-<session-id>-<YYYYMMDDTHHMMSSZ>.md` in the session directory, using the full session ID and the export time in UTC, for example `export-11d785e2-d277-406c-9f99-31d840a3705b-20261013T123045Z.md`.

With a path, a relative path is relative to the session directory, a leading `~/` is your home directory, and everything after the command is the path, so spaces need no quoting: `/export ./session notes.md`.

On Windows, drive-letter and UNC paths (`C:\Users\me\notes.md`, `\\server\share\notes.md`) are used as given and `~\` also means your home directory. A drive-relative path such as `C:notes.md` is refused, because it points at that drive's current directory and not at the session directory.

### Export contents and limits

The file starts with the session title, ID, directory, model, export time and a `Not included` line naming what is never exported: thinking blocks, images inside tool results, background task state, and timing and other session metadata.

Your messages and the assistant's text are written as they were, except that a code fence or HTML comment a message leaves open is closed at the end of that message, and a line that would read as one of the export's own `User`, `Assistant` or `Tool:` headings gets its first `#` escaped, so message text cannot change the file's structure.

Saved command output, notices and notifications appear as quoted notes.

Each tool call is a heading with the tool name and a short summary of its input, followed by its result in a fenced block and, for edits and new files, a `diff` block.

Results over 60 lines or 4,000 characters and diffs over 200 lines are cut with a note saying how much is shown, and terminal colour codes are removed. A session too large to send at once keeps its beginning, and the file ends with a note saying how many later entries were left out.

History entries the app cannot show, such as an image of an unsupported type, are counted in a closing note, and an image in a tool result is named where it occurred.

Rendering and writing happen on the app's event loop, so the terminal pauses until a very large export is written.

`/export` never overwrites a file and creates no directories. If the default name is taken, it uses `-2`, `-3` and so on before `.md`. If an explicit destination exists (a directory is reported as a directory), or its directory does not, the command reports the error and writes nothing. When it finishes it shows the absolute path it wrote, or the failure.

## Reviewing Git changes

1. Enter `/diff` from a directory inside the repository.
2. Press s and select HEAD, Staged or Unstaged with Up/Down and Enter.
3. Select a file, then press Enter or Right to inspect its contents. Press v to switch between the diff and full file view.
4. Return to the file tree with Left or Escape, then press Escape to return to the conversation.

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

Use `/btw` for a contextual side question, including while the main turn is running. For example, enter `/btw What does this error code mean?` The answer appears in its own card without adding the question to the main conversation.

`/btw` treats the complete non-empty text after the command as one question, including internal spaces and line breaks. Side questions are shown in a separate status field while pending and produce a bordered `Claude · BTW` transcript card only when answered. During an active turn, the card is inserted at the current point in the agent's output, with subsequent output below it; when idle, it is a standalone transcript entry with the same design. You can continue using the normal composer while up to ten side questions are unresolved; Rust queues and dispatches them one at a time in submission order. Each question sees main-conversation context at SDK dispatch time, not a snapshot from submission. Failures free capacity immediately and remain visible briefly as error rows.

## Session Name

Open `/status` to see the current session name. Press r to edit it or g to generate a title. A generated title uses the existing name as its description when one is available.

Renaming requires a saved transcript. Before Claude has saved a command or message, the dialog remains editable but cannot confirm the rename. A failed rename keeps the draft and shows the error in Status. After a successful rename, check the name shown in that tab.

The Agent SDK owns the title. An unnamed session can acquire a generated title after its first turn; until the SDK reports one, Status shows `(unnamed session)`. A `/rename` command entered in chat is reflected after its turn finishes. Naming a session at launch is covered in the [CLI reference](cli.md#name-or-fork-a-session).

SDK-local commands such as `/rename`, `/color` and `/context` show their replies in chat when advertised. `/color` does not change the Rust interface's session color. Internal title lookup and persistence rules are documented under [Session title](subsystems.md#session-title).

## Read and stop subagents

1. Enter `/agents` and select a subagent.
2. Press Enter to read its saved transcript, or r to check again if the transcript is not yet available.
3. To stop a running subagent, press s and wait for its lifecycle result. Acknowledgement of the request alone does not mean it has stopped.
4. Press Escape to return to chat.

`/agents` lists the active session's subagents in lifecycle order and keeps selection by task identity across execution runs.

Details show the subagent type, known parent, current run ID, model source, transcript availability, current activity or lifecycle result, recent tool calls and prompt.

Observed models take precedence over requested or agent-default models. Forks inherit their parent's model; ignored model and effort overrides are not shown as applied. Unknown or finished parents do not prevent reading or stopping a child. Ambient housekeeping tasks are excluded. The configurable footer counts running subagents once per task; see [Footer](settings.md#footer).

A completed row can show a **Saved response** preview. This is the last reply in the SDK's saved conversation, including reports sent through `SubagentHandback`; persistence may still contain an older run's report. The lifecycle result is the authoritative outcome of the current run. Enter opens the whole saved conversation, with Markdown replies, tool calls and labeled SDK reminders. User-authored tags remain literal.

A run change invalidates the preview; an open older transcript remains a labeled saved snapshot at its current scroll position until `r` refreshes it. The SDK provides no run-specific transcript selector.

A transcript becomes available only when the SDK lists it; `r` checks again and retries failed reads. `s` requests a stop for the selected running execution. Stopping state and acknowledgements are correlated to that request and run; only the lifecycle establishes the stopped outcome.

A resumed session retains recorded outcomes and labels unfinished historical work as interrupted, without counting it as running or offering a stop. Keys are listed in [Keyboard Shortcuts](shortcuts.md#subagents-view).

Opening or refreshing `/agents` also restores saved agents at every nesting depth, including children launched inside another agent. They join the same inventory as live agents, with their saved ancestry and supported outcomes. Missing completion evidence is shown as interrupted. This discovery does not add nested conversations to the main chat transcript.

## Scheduled jobs and notification reads

`CronCreate` and `CronList` tool cards show **Next run** beside the corresponding schedule ID when the SDK supplies it. This is the scheduler's timestamp, including its scheduling offset; the app does not calculate another time from the cron expression. Dates and clock times use your [time format and time zone](settings.md#presentation-and-scrolling). Preset formats include the calendar date; custom strftime formats are used as written. Older saved results without a next-run timestamp leave it unknown.

`ReadNotifications` cards show the queue's **Remaining** count and optional **Read at** timestamp for that drain. Each item keeps its notification ID, origin and literal content, with **Queued at** for the backend queue timestamp and optional **Arrived at** for arrival in the local queue. These times describe separate stages; they do not indicate PushNotification delivery. Missing or invalid timestamps are omitted.

Saved conversations preserve these fields when the SDK saved them. Replaying a queue result displays its saved data without sending another desktop notification. These cards retain the normal tool preview limit and hidden-line indicator, keeping the leading IDs and timestamps visible when results are clipped. Full saved content remains available in transcript exports.
