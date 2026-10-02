# Terminal regression fixture

`fake-bridge.js` is a local NDJSON peer for the real `claude-rs` binary. The PTY tests exercise terminal rendering, resizing, input delivery, and shutdown without calling a model or using credentials. They do not validate the real SDK or terminal font rendering.

Run the suite with `cargo test --all-features --test terminal_resize -- --test-threads=1`. Bun or Node must be available on `PATH`.

The PR workflow runs this suite on Ubuntu, Windows, and macOS, and requires all three platform jobs to pass. Windows uses ConPTY; Linux and macOS use native Unix PTYs. Windows receives complete paste actions; Unix additionally exercises fragmented control sequences and UTF-8 scalars. Exact Unicode payloads, including combining accents, are checked in the journal on every platform. Submission waits out the application's existing paste Enter suppression window.

| Scenario | First turn |
| --- | --- |
| `stream` | Streams text and completes normally. |
| `hold-success` | Streams text, waits for the release file, then completes. |
| `hold-error` | Streams text, waits for the release file, then emits a service error. |

`FAKE_BRIDGE_JOURNAL` records commands, events, and the `reply-held` coordination point as JSONL. `FAKE_BRIDGE_RELEASE_FILE` lets a test release a held turn after exercising the UI. Reply length and interval remain configurable with `FAKE_BRIDGE_LINES`, `FAKE_BRIDGE_FOLLOW_UP_LINES`, and `FAKE_BRIDGE_INTERVAL_MS`.

The fixture serializes overlapping prompts and correlates their queued/start events by message UUID. Add protocol behavior alongside a test that exercises it; cancellation, permission decisions, questions, and session replacement are not implemented here yet.
