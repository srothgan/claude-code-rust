# Terminal regression fixture

`fake-bridge.js` is a local NDJSON peer for the real `claude-rs` binary. The PTY tests exercise terminal rendering, resizing, input delivery, and shutdown without calling a model or using credentials. They do not validate the real SDK or terminal font rendering.

Run the suite with `cargo test --all-features --test terminal_resize -- --test-threads=1`. Bun or Node and `rustc` must be available on `PATH`; the auth tests compile a small native fake CLI into their temporary directory.

The PR workflow runs this suite on Ubuntu, Windows, and macOS, and requires all three platform jobs to pass. Windows uses ConPTY; Linux and macOS use native Unix PTYs. Windows receives complete paste actions; Unix additionally exercises fragmented control sequences and UTF-8 scalars. Exact Unicode payloads, including combining accents, are checked in the journal on every platform. Submission waits out the application's existing paste Enter suppression window.

| Scenario | First turn |
| --- | --- |
| `stream` | Streams text and completes normally. |
| `hold-success` | Streams text, waits for the release file, then completes. |
| `hold-error` | Streams text, waits for the release file, then emits a service error. |
| `hold-eof` | Streams text, waits for the release file, then closes stdout by exiting. |
| `hold-malformed` | Streams text, waits for the release file, then writes invalid NDJSON and remains alive for shutdown. |
| `permission` | Starts a tool, requests an allow/deny decision, then completes after the correlated response. |
| `question` | Starts a tool, requests a destination selection, then completes after the correlated answer or cancellation. |
| `disconnect-during-auth` | Connects normally, then exits when the release file appears while the auth child owns stdin. |
| `resize-ready` | Streams a completed transcript for scrollback checks across zoom and reading-mode resizes. |
| `resize-replay` | Emits a large completed reply in one chunk for interruption and replay-budget checks. |
| `resize-code` | Completes the first reply, then holds a second reply inside an unfinished code block. |
| `agents` | Connects with a running subagent that reports its prompt, a completed subagent and a background shell, as normalized task lifecycle rows. Lists a saved transcript only for the completed subagent, returns its messages, an SDK reminder among them, under the read's request id with one more step on each read, and answers a stop request with an acknowledgement followed by a separate `killed` lifecycle row. Prompts stream normally. |
| `reload` | Answers the first `reload_skills_and_styles` command with two skills and three output styles reloaded, and every later one with the skills reloaded and the output styles failed. |
| `reload-expired` | Answers every `reload_skills_and_styles` command with `skills_and_styles_reload_expired`, as the bridge does when the reload waited too long behind an earlier request. |
| `export` | Answers `export_session_history` with a short conversation (messages, a Bash call with output, an Edit with a diff) in two chunks under the command's request id. |
| `status-rename` | Advertises `/clear`. A message writes the session's transcript where the Agent SDK looks for it (`<CLAUDE_CONFIG_DIR>/projects/<resolved cwd, non-alphanumerics as ->/<session>.jsonl`) and replies `Saved reply`; `/clear` replaces the session with an unsaved one listed with `/clear` as summary and first prompt. Real Claude Code saves the new session at once (its `/clear` records); the fixture leaves it unsaved so the test sees the check rerun for the new session id. `rename_session` fails, as `renameSession()` does, without that transcript or for the title `fail-me`; otherwise it reports the title and lists the session with it. |

`FAKE_BRIDGE_JOURNAL` records commands, events, and the `reply-held` coordination point as JSONL. `FAKE_BRIDGE_RELEASE_FILE` lets a test release a held turn after exercising the UI. Reply length and interval remain configurable with `FAKE_BRIDGE_LINES`, `FAKE_BRIDGE_FOLLOW_UP_LINES`, and `FAKE_BRIDGE_INTERVAL_MS`.

The fixture serializes overlapping prompts and correlates queued/start events by message UUID. Completion includes the remaining queue count. Cancellation emits an interrupt receipt and an `aborted_streaming` completion, stops the active timer, and keeps queued work. Permission and question requests follow an existing tool-call event; responses are correlated by session and tool-call ID. A new-session command emits `session_replaced` with a different session ID, and a resume whose launch settings ask for `fork_session` connects as `<session id>-fork`. These sequences follow the checked-in Rust wire types and TypeScript bridge handlers, but the fixture is not an independent SDK conformance oracle.

The tests cover queued/start ordering, cancellation followed by another turn, permission acceptance and denial, question selection with notes and cancellation, simultaneous resizing and input, and fatal EOF/malformed-output shutdown. Journals verify exact prompts and exactly one interaction response. EOF and malformed NDJSON are deliberately injected protocol faults, not claims that the SDK normally emits them. Protocol failures currently terminate the real app; these tests do not claim draft persistence after that exit.

Resize tests inspect native terminal scrollback as well as the visible screen, including one-row terminals, reading mode, unfinished code blocks, and a new resize interrupting an active transcript replay. They wait for draws at the requested dimensions and require every completed reply row to remain accessible.

`fake-claude.rs` is a native stand-in for `claude auth login`, placed first on the child app's `PATH`. It reads one line from inherited stdin, records it inside the isolated profile, and waits for a release file. It can succeed using a clearly invalid fixture token or fail with exit code 7. The spawn-failure case uses an invalid executable on Windows and an executable script with a missing interpreter on Unix; plain executable text can run through a shell on macOS and does not reliably trigger a spawn error. The tests verify that the real TUI stays silent during child ownership, sends all input to the child, restores usable typing and paste after resizing, and stops an owned child during fatal shutdown. They do not exercise browser OAuth, real credential renewal, or the installed Claude CLI.

Each test owns its temporary project, Claude profile (`CLAUDE_CONFIG_DIR`), app-preferences profile (`CLAUDE_RS_CONFIG_DIR`), fake CLI, journal, and diagnostics log. The shared launcher overrides inherited profile locations on the child process without changing the test runner's environment. Fresh app profiles exercise production defaults; custom configurations under `app-settings/` are immutable fixtures copied into a test's own profile before launch. Tests that exercise preference writes inspect that same isolated file. No model, subscription, or user credential file is used. Add protocol behavior alongside a real workflow test; session resume/history reconstruction and SDK-specific cancellation races remain outside this fixture's coverage.
