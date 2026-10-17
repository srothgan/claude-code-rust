# Architecture

Claude Code Rust is split into a native Rust terminal app and a TypeScript Agent SDK bridge.

## Runtime Shape

The Rust binary owns the terminal UI and process lifecycle. It parses CLI options with Clap, starts a Tokio runtime, and runs the app inside a `LocalSet` because parts of the terminal and child-process runtime are not `Send`.

The app then starts or resumes a bridge session and renders the chat view directly in the terminal.

Interactive startup arguments resolve into one launch intent: a new session, a specified resume ID, the session picker, or the latest session in the working directory. Session overrides are applied through the existing launch-settings boundary and released after the initial session is selected. Launch configuration (`--add-dir`, `--settings`, `--mcp-config`, `--strict-mcp-config` and the system prompt flags) travels through the same boundary but is kept for every session the process starts. Rust resolves paths against the working directory, reads files and checks that JSON parses before logging, the bridge or the terminal start, so a bad flag stops the launch with nothing renamed or written. The bridge is the only place that turns the result into SDK options: it lays host restrictions over the `--settings` flag layer, merges MCP configs in command-line order, names a new or forked session through the SDK `title` option and a resumed one through `renameSession()`, and keeps its runtime guard prompt in every system prompt. Initial prompts use normal prompt queue admission once startup dialogs are complete.

Support commands run before bridge resolution. Shell completions and manual pages are generated from the same Clap command definitions as CLI help, so neither requires the bridge or authentication.

## Rust Terminal App

Important Rust areas:

| Area | Responsibility |
| --- | --- |
| `src/main.rs` | Process entrypoint, runtime setup, logging setup, and exit behavior. |
| `src/lib.rs` | CLI arguments, subcommands, and diagnostics presets. |
| `src/agent/` | Bridge process resolution, NDJSON client, wire types, and bridge error handling. |
| `src/app/` | App state, lifecycle, sessions, config, permissions, input, slash commands, plugins, MCP, usage, and trust. |
| `src/ui/` | Ratatui rendering for messages, markdown, diffs, tool calls, config tabs, help, autocomplete, and input. |

The current runtime uses inline terminal-owned rendering rather than an older fullscreen-only model. Fullscreen views are still used for config, help, status, usage, MCP, plugin and subagent surfaces. The reasons for that change are described in [I rebuilt Claude Code's terminal UI in Rust. Then I deleted 12,000 lines of it.](https://medium.com/@simonrothgang/i-rebuilt-claude-codes-terminal-ui-in-rust-then-i-deleted-12-000-lines-of-it-e8593a200452)

## Agent SDK Bridge

In packaged npm installs, the Rust process resolves a private Bun runtime named `claude-rs-bridge-bun` or `claude-rs-bridge-bun.exe` from the installed package layout. That runtime runs:

```text
agent-sdk/dist/bridge.js
```

The TypeScript bridge wraps `@anthropic-ai/claude-agent-sdk`. Rust and TypeScript communicate over stdin/stdout using newline-delimited JSON command and event envelopes.

Rust sends commands such as session creation, session resume, prompt submission, permission responses, MCP actions, and runtime refresh requests. The bridge sends events such as assistant messages, tool updates, permission requests, question requests, available commands, modes, models, usage, and errors.

## Subsystems

Each subsection names the owner of a concern and the invariants that keep the Rust side and the bridge from holding two versions of the same state.

### Settings and trust

| Concern | Owner | Location |
| --- | --- | --- |
| Setting catalog: IDs, labels, kinds, choices, writable scopes, panes, editor schemas | Bridge | `agent-sdk/src/bridge/settings_catalog.ts`, `settings_layout.ts` |
| Saved-value resolution across scopes | Agent SDK | public `resolveSettings()` |
| Mutation validation and targeted JSON writes | Bridge | `agent-sdk/src/bridge/settings_service.ts` |
| Config window, edit drafts, acknowledgement tracking | Rust | `src/app/config/`, `src/ui/config/` |
| Workspace trust state and store | Rust | `src/app/trust/` |
| App preferences file and updater state | Rust | `src/app/settings.rs` |

Rust receives catalog descriptors and sanitized snapshots over NDJSON and renders them. It does not merge settings files, mirror scope rules, or supply defaults for unset values. Saved defaults stay separate from active session choices: a settings save never becomes a session flag override. Snapshots and pending drafts are invalidated when the session scope changes. Offline `claude-rs config` inspection is a separate read-only view of the physical files and does not resolve the cascade.

The snapshot is the SDK's raw file cascade. The running session additionally applies trust, managed policy, capability restrictions and explicit session choices, so an observed session value can differ from the saved one. SDK provenance is reported per top-level key, which does not prove that a particular leaf is enforced policy. `resolveSettings()` does not execute `policyHelper`.

The write path has one implementation for every setting, including the app-owned `presentation.*`, `notifications.*` and `updates.*` leaves:

1. The bridge validates the value and the target scope against the catalog immediately before saving. Only the edited leaf is validated through the SDK's resolver, so unknown keys, newer settings and unrelated invalid values survive.
2. The current file is read and only the target key is patched. Reset removes the key and empty ancestor objects.
3. A revision of the displayed target detects conflicting edits. Unrelated external edits are incorporated into the fresh document; a conflict refreshes the snapshot and keeps the editor draft.
4. The write takes a cooperative lock, rechecks the file bytes, and replaces the file atomically. An external editor that ignores the lock can still race between the final recheck and the replacement; this is not a filesystem compare-and-swap.
5. The acknowledgement is correlated with its originating mutation, so a delayed result cannot close a different editor. A pending save keeps the last acknowledged value visible.

Structured editors and their advanced JSON view share this one mutation and persistence path. Inspection and validation never execute hooks or credential helpers and never resolve secret files or environment variables.

Default effort targets `modelSettings.<canonical model>.effortLevel`. SDK model metadata selects the target: aliases and dated, context and provider spellings are normalized to the canonical key, and unknown model identities stay read-only. Mutation context checks stop an outdated edit from targeting a different model after Default model changes.

Workspace trust never uses the config snapshot as authority. Trust acceptance reads the current preferences document and preserves unrelated authentication and MCP data. Shared atomic file replacement is an I/O mechanism, not a domain resolver.

Automatic updates uses the same catalog and writer for the User-only `updates.autoInstall` leaf, and the Rust updater owns its application. Startup prompt selection skips the manual window when the preference is enabled, background check results return through the app event queue, and a normal post-TUI exit reads fresh saved consent and cached release metadata before selecting the installer. An install chosen in the update window runs in the terminal. An automatic install is handed to a detached run of the same executable through the hidden `update-worker` subcommand, which holds an OS file lock, sends installer output to its own log, stops the installer process tree at a time limit, and saves the outcome against the settings file because it has no session. The next start reads that outcome and shows a failure once the launch transcript is settled. Updater metadata saves omit the editable preference and preserve it, along with the presentation and notification namespaces, under the shared document lock. Checking is independent of installation.

### Presentation and timing

Tool titles share a one-line display projection and a source-keyed inline cache in the existing block cache. Shell command titles render literally; other tool titles retain Markdown styling. Full titles and command inputs remain canonical data. Spinner/status chrome, metadata badges and width clipping stay live, while unchanged title content survives body updates and resize. Inline cache bytes participate in the existing render budget and eviction workflow.

| Concern | Owner | Location |
| --- | --- | --- |
| Normalizing SDK wall timestamps, result elapsed and API timing, tool and task elapsed metadata | Bridge | `agent-sdk/src/bridge/presentation_metadata.ts` |
| Each message's clock and timing provenance, stored on the canonical transcript message | Rust | `src/app/presentation.rs` |
| One formatter for locale, clock presets, custom patterns and timezone | Rust | `src/app/presentation.rs` |

Presentation preferences are projections of the acknowledged config snapshot; Rust keeps no second presentation store. History replay removes locally observed times and preserves native timestamps without reordering messages. Completed duration rows go through the same scrollback boundary and commit protocol as other transcript rows.

### Transcript rendering

The canonical transcript owns message and tool content. The chat session traverses it through a resumable row producer, with cursors for assistant sections, Markdown lists and code lines, and file-preview syntax state. Text rendering avoids generating an extra physical-row copy solely to measure height. Capped tool bodies lay out the visible prefix and tail without wrapping hidden middle lines when the source-line count already proves overflow.

### Resize replay

Replay inserts ready rows before preparing more source. Tools can emit their title before their body is ready, and completed sections can be inserted before the rest of an assistant turn is laid out. Text production and terminal insertion use the same viewport-derived row target, while preparation also yields on a cooperative time budget. One replay stays open until both source traversal and insertion finish; an empty output queue alone does not confirm completion. No historical width layouts are retained.

Pending replay work is independent of the repaint flag. Preparation can continue without redrawing unchanged content, while input and other visible changes still request a repaint. The terminal reserves insertion space only when history rows are queued; an empty preparation slice leaves the viewport geometry intact. When replay stops at mutable content, completion schedules a render of the full live suffix from canonical state. Shutdown waits for pending preparation and insertion even when no repaint is requested.

Layout cancellation checkpoints observe terminal size independently of queued input events, including during Markdown parsing, source grouping, highlighting and physical-row conversion. A changed size discards the attempt and restarts through the resize owner. Transcript topology changes, changes to a tool being prepared, and reading-policy changes also discard an active replay; ordinary appended messages can be consumed by the existing traversal. Cancellation is cooperative: an individual parser or syntax-library call can exceed the scheduling budget before the next checkpoint.

`inline_chat_layout_step` records preparation duration and row count; `inline_chat_draw_interrupted_by_resize` uses phase `layout` when cancellation happens before terminal insertion.

Shutdown redraws the live viewport without resetting committed history. Reading returns to live output so its viewport copy does not remain beside native scrollback. The terminal owner finishes pending resize recovery and transcript replay before restoration; an explicit force interrupt can stop that recovery.

### Transcript reading

Chat reading owns a stable transcript-segment anchor and a rendered-text position; the saved Auto-scroll setting controls only the following policy. Reading chooses the visible source window independently of scrollback restoration, survives reflow of retained content, and returns to following new output through an explicit keymap action.

While reading, the current replay accumulates the rows needed to resolve the saved text anchor and preserves that anchor until traversal finishes. These temporary rows belong to the active replay and are released with it.

### `/copy`

`/copy` derives its material from the canonical response text and the text-joining rule shared with rendering, then writes through the clipboard boundary shared with MCP authorization. Its picker owns only the current selection and retry state.

### Turn activity and thinking

| Concern | Owner | Location |
| --- | --- | --- |
| Interpreting the live main-agent activity phase from SDK stream events | Bridge | `agent-sdk/src/bridge/activity.ts` |
| Presentation decision from the typed `agent_activity_update` phase, turn liveness, pending interactions, compaction and cancellation | Rust | `src/app/activity.rs` |

Only top-level `message_start` and indexed `thinking` or `redacted_thinking` block boundaries select the thinking phase, including blocks with empty or omitted text. A matching block stop, response stop or completion, retry or fallback, result or error, conversation replacement and session close reset it. SDK stream wrapper UUIDs identify individual frames; the API response ID scopes the ordered block stream and correlates completed assistant messages. A per-block assistant frame with a null stop reason keeps that scope open, and only a terminal, aborted or error frame closes its response. Stops carry no response ID and follow the SDK's ordered response boundaries. The same correlation decides where reply text comes from: a streamed response is shown from its deltas, and a completed top-level message whose response never streamed, such as a local command's reply, is shown from that message once; error messages report through the turn result. Subagent frames and completed history never start main-agent thinking. `system/thinking_tokens` estimates stay diagnostics, because they carry neither a response or block identity nor a matching stop.

Rust consumes the typed phases without interpreting SDK content and never infers thinking from text, tools or silence. One derived decision assigns the activity heading to the active assistant, or to the composer for standalone work without an assistant message. SDK thinking appears separately in the active assistant output as a temporary live row that is never committed to history. Waiting or completing without output removes the temporary heading and activity; real assistant output keeps its heading. Composer-only headings and the composer activity block never enter transcript or history output.

A turn keeps one verb and one shuffled traversal of the shared welcome tips, advanced by monotonic deadlines on the TUI wake loop, including under reduced motion. `spinnerTipsEnabled` from the acknowledged snapshot controls only the tip text; activity, its heading and SDK thinking are independent of it.

### Background tool execution

The bridge normalizes confirmed launch results and SDK task lifecycle events into the existing `Detached` tool status. Requested background flags and host elapsed time do not confirm a handoff. Task-to-tool correlation survives foreground turn completion and resume; stale progress and repeated launch acknowledgements cannot reopen terminal execution. Live and replay use the same task adapters and transition rules.

Rust keeps execution state on the original `ToolCallInfo`. Its history projection freezes the launch at its creation position with the fixed `↗` icon, allowing the transcript prefix to enter terminal history while the task runs independently. Actual completion, failure or stopping appends one separately identified result linked to that tool. The session owns pending interactions; foreground turn reset preserves background controls, which remain mutable at the end of the live region during transcript replay after resize. Cache and retention accounting include the frozen projections without adding them to the execution index or foreground activity set.

A task row's lifecycle state is separate from its tool call. A notification that follows a finished tool call still settles the row, and only a new start or another outcome changes a settled row. Every lifecycle edge marks its row as lifecycle-owned. `background_tasks_changed` removes and resets only rows it alone created, so a subagent that leaves the snapshot before or after its outcome keeps its row. A listed row that is still running when the snapshot stops listing it, and a row still running at the end of saved history, settle as `completed` with `terminal_status: "interrupted"`: not running, with no outcome claimed, a real outcome that arrives later replaces it, and a later `task_progress` or a snapshot that lists the row again (the SDK defines the snapshot as every live task) returns it to `in_progress`. Only that inferred state is revived: a row with a recorded outcome is never reopened by a listing. A `TaskList` snapshot replaces plan tasks while retaining lifecycle rows. On resume, a saved task notification takes the row's type, subagent type and title from the Agent call it links to, because background agents are saved without a `task_started` record.

### Subagents view

| Concern | Owner | Location |
| --- | --- | --- |
| Live subagent status, from the SDK task lifecycle | Bridge | `agent-sdk/src/bridge/tasks.ts` |
| Saved transcript ids and messages, stop requests | Agent SDK, called by the bridge | `listSubagents()`, `getSubagentMessages()`, `Query.stopTask()` in `agent-sdk/src/bridge/subagents.ts` |
| `/agents` selection, transcript view, sent stop requests, footer count | Rust | `src/app/agents.rs`, `src/ui/agents.rs` |

The view lists the active session's `local_agent` lifecycle rows from the task state Rust already holds, one per task id; there is no second task store and no polling. The selected subagent's details come from the same row, the Agent call that launched it (prompt and requested model), the agent definitions the session advertises (default model) and the subagent's own tool calls Rust already scopes to that Agent call (current and recent activity). The footer's running subagents field counts the rows the lifecycle reports as running. A row has a transcript only when `listSubagents()` returned its task id, which is read when the view opens and on `r`. A transcript read never changes a row's status. The bridge reads a saved `SubagentHandback` call as a reply carrying the full report, because a subagent can answer only through it. The shared transcript classification (`transcript_presentation.ts`) takes a saved user record the SDK marks as metadata (`is_meta`) whose whole text is one `<system-reminder>` element as a system reminder: the transcript gets its inner text under the `system_reminder` role, and the chat's history projection leaves it out, as the SDK's session history does. Any other text keeps its tags. A completed row's response is the last reply of a read made after it completed. The list reads it when the row is selected or completes while selected, and again on `r`; a read made while the row ran is never taken as its response, a row that runs again is read again once it completes, and a failed read is retried only by `r`. Moving along the list keeps at most one such read in the session command lane, and opening a completed row's transcript shows the read its response came from. Each read is its own request: `r` in an open transcript reads it again and keeps the reading position, and an answer to an earlier read is ignored. A stop request is offered only for a running row. Once it is sent the row shows stopping, its acknowledgement does not change that, and only the lifecycle reports it stopped. A rejected request is shown as an error and leaves the row as it was. The bridge refuses to stop a task it does not hold as a running subagent, and runs the request outside the session command lane because `stopTask()` can stay pending until its query ends.

The bridge announces each live top-level SDK response through the typed `agent_response_started` update before sending its text. A response triggered by a background completion receives a fresh mutable assistant owner even when no user prompt is pending; history replay does not activate live responses. The renderer groups consecutive visible assistant messages under one speaker label, carrying speaker context across content already inserted into terminal history. Streaming ownership, message identities and tool insertion order remain separate from that visual grouping.

### Session title

| Concern | Owner | Location |
| --- | --- | --- |
| Reporting and changing a session's title | Agent SDK | public `getSessionInfo()`, `renameSession()` |
| Reading the active session's title and sending `session_title_update` | Bridge | `agent-sdk/src/bridge/session_title.ts` |
| Holding the active session's title for the Status tab, its rename draft and title generation | Rust | `src/app/state/session_runtime.rs` |
| Listing snapshot of all sessions, including each `custom_title` | Bridge, held by Rust | `agent-sdk/src/bridge/events.ts`, `recent_sessions` |
| Whether the active session is saved, which `renameSession()` requires | Rust | `src/claude_paths.rs`, `src/app/config/status.rs` |

The bridge reads `getSessionInfo().customTitle` after a connect, a replacement, a conversation reset, each top-level turn and each rename or title generation it performs, and sends every title it reads; a read that a later one overtakes is dropped. Rust keeps the active title as one value, strips control characters once in the converter, stores a repeated title idempotently, and clears it when the session id changes or the conversation resets. Neither side derives a title of its own. The session listing is a separate snapshot of all sessions, refreshed by `listSessions()` on connect, replacement and the Status-tab rename and generate actions; the Status tab falls back to it only while the active session has no title. `customTitle` falls back to a generated title, so an unnamed session shows that title after its first turn.

`renameSession()` appends to the session's saved transcript and fails before Claude Code has written one, so the Status rename dialog confirms only once `<session id>.jsonl` exists as a non-empty regular file (the SDK skips an empty one). Rust looks where the SDK's lookup does: under the configuration directory's `projects/`, in the folder named from the resolved working directory with every non-alphanumeric UTF-16 unit replaced by `-` (or a valid `CLAUDE_CODE_PROJECT_DIR_NAME` with `CLAUDE_CONFIG_DIR`), and also in the folder of the directory the session is listed under when that differs, as for a session resumed from a sibling worktree. For a name longer than 200 characters Rust accepts any folder with the shortened prefix, a superset of the folders the SDK accepts; a transcript in a folder the SDK rejects surfaces as the SDK's rename error. The check runs only at its refresh points, never per frame. `ClaudePaths` owns the folder name; the Status tab's memory row builds its folder with the same encoder. A missing path is "not saved"; any other I/O error is reported as a failed check, unless another candidate folder has the transcript.

### Notifications

| Concern | Owner | Location |
| --- | --- | --- |
| Focus suppression, category enablement, terminal capability detection, transport selection, SDK delivery identities, external delivery boundary | Rust | `src/app/notify.rs` |
| Projecting native notices and verified proactive tool results into notification origins | Bridge | `agent-sdk/src/bridge/notifications.ts` |
| Shared wire and runtime payload | Rust | `src/agent/notifications.rs` |
| Session provenance validation and presentation of native notices | Rust | `src/app/events/notifications.rs` |

Delivery waits for the first acknowledged settings snapshot, so startup cannot send using guessed preferences. Saved categories are read from that snapshot; there is no mirrored notification configuration struct. Local interaction handlers notify only after accepting a real waiting request, and pending-interaction ownership rejects duplicates. Turn completion sends only for an active, non-cancelled turn and skips SDK abortion reasons.

In the bridge, native key, UUID, session, original priority, color and timeout stay structured metadata. Proactive intent requires the actual `PushNotification` tool with `status: proactive` input, followed by a successful result without non-execution metadata. Delivery-report discovery is shared with tool rendering in `tooling.ts`; strings displayed in the transcript are never parsed to recover delivery decisions. Public `isReplay` provenance is carried through tool-result mapping.

General native notices are transcript-only because the SDK supplies no universal delivery category. Proactive fallback requires an explicit `localSent: false` and no native suppression other than `no_transport`; `pushSent` describes mobile transport and is kept separately. Unknown or missing delivery reports do not authorize another local send.

Replay never delivers. `load_resume_history` observes notifications as replay and primes terminal tool identities, including older histories without delivery reports. The manager keeps a bounded ledger of session/UUID and session/tool-ID identities across reconnects; native replacement keys are not event identities. Live suppression consumes an event instead of queuing it for a later focus change.

## Packaging

The npm install is split across a root command package and platform payload packages.

The root package is `claude-code-rust`. It exposes the `claude-rs` command through the npm launcher and includes the built Agent SDK bridge under:

```text
bin/claude-rs.js
agent-sdk/dist/bridge.js
```

The platform packages are selected through root package optional dependencies. They include the native Rust binary and private Bun runtime. The exact package mapping lives in `scripts/shared/npm-package-config.mjs`; supported npm payloads currently cover Linux x64/arm64 glibc, Windows x64/arm64, and macOS x64/arm64.

At runtime, npm's generated shim starts `bin/claude-rs.js`. The launcher selects the matching platform package, sets `CLAUDE_RS_AGENT_BRIDGE` to the root package bridge script, and spawns the native binary. The native binary resolves the bundled Bun runtime from the platform package `bin/` directory. No npm `postinstall` script, install-time binary download, or global Bun is required.

## Release Model

Release packaging is designed around immutable artifacts:

- Native binaries are built on GitHub-hosted runners for Linux x64 glibc, Linux arm64 glibc, Windows x64, Windows arm64, macOS x64, and macOS arm64.
- Private Bun runtime files are staged into each platform package as third-party runtime artifacts.
- Generated npm package directories are verified against allowlisted package contents before packing.
- Unix install archives include man pages generated by the packaging host's staged native binary. The Unix installer links those pages beside its command directory and removes only its own links on uninstall.
- Packed npm tarballs are smoke-tested before publication.
- GitHub Releases include native binaries, npm tarballs, package-content manifests, build metadata, and `SHA256SUMS`.
- The release workflow generates and verifies build provenance attestations for native binaries before npm publication.
- npm publication uses Trusted Publishing rather than a long-lived npm token.
- The root package remains the user-facing npm install package and depends optionally on platform payload packages.

Source builds are different: `cargo build` or `cargo install --path .` produce only the Rust binary. They do not build or install the JavaScript bridge or private Bun runtime. Build the bridge with Node.js 24/npm and provide it through the checkout fallback, `--bridge-script`, or `CLAUDE_RS_AGENT_BRIDGE`. Debug builds can use `CLAUDE_RS_AGENT_BRIDGE_RUNTIME` to point at a local Bun runtime; release npm installs use only the bundled runtime. See [Development](development.md) for the exact steps.

## Boundaries

Claude Code Rust owns the terminal UI, local settings surface, bridge process management, and event rendering. Anthropic owns the Agent SDK, authentication, service behavior, billing, models, and upstream Claude Code semantics.

The project does not depend on Agent SDK package subpath exports such as `/browser`, `/bridge`, or `/assistant` as the runtime path. The runtime path is the local TypeScript bridge in this repository.
