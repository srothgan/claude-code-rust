# Architecture

Claude Code Rust is split into a native Rust terminal app and a TypeScript Agent SDK bridge.

## Runtime Shape

The Rust binary owns the terminal UI and process lifecycle. It parses CLI options with Clap, starts a Tokio runtime, and runs the app inside a `LocalSet` because parts of the terminal and child-process runtime are not `Send`.

The app then starts or resumes a bridge session and renders the chat view directly in the terminal.

## Rust Terminal App

Important Rust areas:

| Area | Responsibility |
| --- | --- |
| `src/main.rs` | Process entrypoint, runtime setup, logging/perf setup, and exit behavior. |
| `src/lib.rs` | CLI arguments, subcommands, and diagnostics presets. |
| `src/agent/` | Bridge process resolution, NDJSON client, wire types, and bridge error handling. |
| `src/app/` | App state, lifecycle, sessions, config, permissions, input, slash commands, plugins, MCP, usage, and trust. |
| `src/ui/` | Ratatui rendering for messages, markdown, diffs, tool calls, config tabs, help, autocomplete, and input. |

The current runtime uses inline terminal-owned rendering rather than an older fullscreen-only model. Fullscreen views are still used for config, help, status, usage, MCP, and plugin surfaces.

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

The platform packages are selected through root package optional dependencies. They include the native Rust binary and private Bun runtime. The exact package mapping lives in `scripts/npm-package-config.mjs`; supported npm payloads currently cover Linux x64/arm64 glibc, Windows x64/arm64, and macOS x64/arm64.

At runtime, npm's generated shim starts `bin/claude-rs.js`. The launcher selects the matching platform package, sets `CLAUDE_RS_AGENT_BRIDGE` to the root package bridge script, and spawns the native binary. The native binary resolves the bundled Bun runtime from the platform package `bin/` directory. No npm `postinstall` script, install-time binary download, or global Bun is required.

## Release Model

Release packaging is designed around immutable artifacts:

- Native binaries are built on GitHub-hosted runners for Linux x64 glibc, Linux arm64 glibc, Windows x64, Windows arm64, macOS x64, and macOS arm64.
- Private Bun runtime files are staged into each platform package as third-party runtime artifacts.
- Generated npm package directories are verified against allowlisted package contents before packing.
- Packed npm tarballs are smoke-tested before publication.
- GitHub Releases include native binaries, npm tarballs, package-content manifests, build metadata, and `SHA256SUMS`.
- The release workflow generates and verifies build provenance attestations for native binaries before npm publication.
- npm publication uses Trusted Publishing rather than a long-lived npm token.
- The root package remains the user-facing npm install package and depends optionally on platform payload packages.

Source builds are different: `cargo build` or `cargo install --path .` produce only the Rust binary. They do not build or install the JavaScript bridge or private Bun runtime. Build the bridge with Node.js 24/npm and provide it through the checkout fallback, `--bridge-script`, or `CLAUDE_RS_AGENT_BRIDGE`. Debug builds can use `CLAUDE_RS_AGENT_BRIDGE_RUNTIME` to point at a local Bun runtime; release npm installs use only the bundled runtime.

## Boundaries

Claude Code Rust owns the terminal UI, local settings surface, bridge process management, and event rendering. Anthropic owns the Agent SDK, authentication, service behavior, billing, models, and upstream Claude Code semantics.

The project does not depend on Agent SDK package subpath exports such as `/browser`, `/bridge`, or `/assistant` as the runtime path. The runtime path is the local TypeScript bridge in this repository.
