# Architecture

Claude Code Rust is split into a native Rust terminal app and a TypeScript Agent SDK bridge.

## Runtime Shape

The Rust binary owns the terminal UI and process lifecycle. It parses CLI options with Clap, starts a Tokio runtime, and runs the app inside a `LocalSet` because parts of the terminal and child-process runtime are not `Send`.

The app then starts or resumes a bridge session and renders the chat view directly in the terminal.

Interactive startup arguments resolve into one launch intent: a new session, a specified resume ID, the session picker, or the latest session in the working directory. Session overrides are applied through the existing launch-settings boundary and released after the initial session is selected. Initial prompts use normal prompt queue admission once startup dialogs are complete.

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

The current runtime uses inline terminal-owned rendering rather than an older fullscreen-only model. Fullscreen views are still used for config, help, status, usage, MCP, and plugin surfaces. The reasons for that change are described in [I rebuilt Claude Code's terminal UI in Rust. Then I deleted 12,000 lines of it.](https://medium.com/@simonrothgang/i-rebuilt-claude-codes-terminal-ui-in-rust-then-i-deleted-12-000-lines-of-it-e8593a200452)

Presentation preferences remain projections of the acknowledged config snapshot. The bridge's single catalog and targeted writer handle Claude documents and the personal app presentation namespace; Rust owns neither a parallel cascade nor a second presentation store. Updater writes patch their own fields and cooperate through the same document-lock filename.

The bridge normalizes available SDK wall timestamps, result elapsed/API timing, and tool/task elapsed metadata. The canonical transcript owns each message's clock and timing provenance; history replay removes new local observations and preserves native timestamps without reordering messages. One formatter applies locale, clock presets, custom patterns, and timezone. Completed duration rows participate in the existing scrollback boundary/commit protocol.

`/copy` derives its material from the canonical response text and the shared text-joining rule used by rendering, then uses one clipboard boundary shared with MCP authorization. Its picker owns only the current selection and retry state. Chat reading owns a stable transcript-segment anchor and rendered-text position, while saved Auto-scroll controls following policy. Reading suspends scrollback insertion, survives retained-content reflow, and returns to ordinary incremental history commits through an explicit keymap action.

## Agent SDK Bridge

In packaged npm installs, the Rust process resolves a private Bun runtime named `claude-rs-bridge-bun` or `claude-rs-bridge-bun.exe` from the installed package layout. That runtime runs:

```text
agent-sdk/dist/bridge.js
```

The TypeScript bridge wraps `@anthropic-ai/claude-agent-sdk`. Rust and TypeScript communicate over stdin/stdout using newline-delimited JSON command and event envelopes.

Rust sends commands such as session creation, session resume, prompt submission, permission responses, MCP actions, and runtime refresh requests. The bridge sends events such as assistant messages, tool updates, permission requests, question requests, available commands, modes, models, usage, and errors.

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

## Settings and trust ownership

The bridge settings service owns the Claude settings catalog, scoped mutation validation, and targeted JSON persistence. Public SDK `resolveSettings()` owns saved-value resolution. Rust receives catalog descriptors and sanitized snapshots over NDJSON, owns the config window and edit drafts, and tracks correlated acknowledgements without mirroring scope rules or merging files. Saved defaults remain separate from active SDK session choices. Settings snapshots and pending drafts are invalidated when the session scope changes.

Workspace trust has its own state and store under `src/app/trust`; it never uses the config snapshot as authority. Trust acceptance reads the current preferences document and preserves unrelated authentication and MCP data. App preferences retain their separate owner under `src/app/settings.rs`, with no legacy update-cache import. Shared atomic file replacement is an I/O mechanism rather than a domain resolver. Offline CLI config inspection is a separate read-only physical-file view.


## Notification ownership

`src/app/notify.rs` owns focus suppression, category enablement, terminal capability detection, transport selection, SDK delivery identities and the external delivery boundary. Local interaction handlers notify only after accepting a real waiting request; existing pending-interaction ownership rejects duplicate requests. Turn completion sends only for an active, non-cancelled turn and skips SDK abortion reasons. Delivery waits for the first acknowledged settings snapshot, so startup cannot send using guessed preferences. Saved categories are read from that snapshot, without a mirrored notification configuration struct. The single catalog and targeted settings writer handle User-only app notification leaves alongside presentation leaves; updater writes preserve both namespaces through the shared document lock.

`agent-sdk/src/bridge/notifications.ts` projects native loop notices and verified proactive tool results into distinct notification origins. Native key, UUID, session, original priority, color and timeout remain structured metadata. Proactive intent requires the actual `PushNotification` tool and its `status: proactive` input, followed by a successful result without non-execution metadata. Delivery-report record discovery is shared with tool rendering in `tooling.ts`; strings displayed in the transcript are never parsed to recover delivery decisions. Public `isReplay` provenance is carried through tool-result mapping. `src/agent/notifications.rs` is the shared Rust wire/runtime payload, so converters do not rebuild the same semantic object.

General native notices are transcript-only because the SDK supplies no universal delivery category. Proactive fallback requires an explicit `localSent: false` and no native suppression other than `no_transport`; `pushSent` describes mobile transport and is retained separately. Unknown or missing delivery reports do not authorize another local send. `src/app/events/notifications.rs` validates payload session provenance and presents eligible native notices. `load_resume_history` explicitly observes notifications as replay and primes terminal tool identities, including older histories without delivery reports. The manager retains a bounded 512-entry session/UUID or session/tool-ID ledger across reconnects; native replacement keys are not event identities. Live suppression consumes an event instead of queuing it for later focus changes. Replay rebuilds transcript visibility without external delivery. Tests inject the final delivery boundary and exercise actual client events; terminal fixtures verify NDJSON conversion, focus reports, BEL output and acknowledged saves without contacting a model or changing the user's app preferences.
