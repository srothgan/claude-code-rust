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

`FAKE_BRIDGE_JOURNAL` records commands, events, and the `reply-held` coordination point as JSONL. `FAKE_BRIDGE_RELEASE_FILE` lets a test release a held turn after exercising the UI. Reply length and interval remain configurable with `FAKE_BRIDGE_LINES`, `FAKE_BRIDGE_FOLLOW_UP_LINES`, and `FAKE_BRIDGE_INTERVAL_MS`.

The fixture serializes overlapping prompts and correlates queued/start events by message UUID. Completion includes the remaining queue count. Cancellation emits an interrupt receipt and an `aborted_streaming` completion, stops the active timer, and keeps queued work. Permission and question requests follow an existing tool-call event; responses are correlated by session and tool-call ID. A new-session command emits `session_replaced` with a different session ID. These sequences follow the checked-in Rust wire types and TypeScript bridge handlers, but the fixture is not an independent SDK conformance oracle.

The tests cover queued/start ordering, cancellation followed by another turn, permission acceptance and denial, question selection with notes and cancellation, simultaneous resizing and input, and fatal EOF/malformed-output shutdown. Journals verify exact prompts and exactly one interaction response. EOF and malformed NDJSON are deliberately injected protocol faults, not claims that the SDK normally emits them. Protocol failures currently terminate the real app; these tests do not claim draft persistence after that exit.

`fake-claude.rs` is a native stand-in for `claude auth login`, placed first on the child app's `PATH`. It reads one line from inherited stdin, records it inside the isolated profile, and waits for a release file. It can succeed using a clearly invalid fixture token or fail with exit code 7. Another case uses an invalid executable to exercise spawn failure. The tests verify that the real TUI stays silent during child ownership, sends all input to the child, restores usable typing and paste after resizing, and stops an owned child during fatal shutdown. They do not exercise browser OAuth, real credential renewal, or the installed Claude CLI.

Each test owns its temporary project, configuration directory, fake CLI, journal, and diagnostics log. No model, subscription, or user credential file is used. Add protocol behavior alongside a real workflow test; session resume/history reconstruction and SDK-specific cancellation races remain outside this fixture's coverage.
