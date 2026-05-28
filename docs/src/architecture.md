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

The Rust process spawns a local Node runtime that runs:

```text
agent-sdk/dist/bridge.js
```

The TypeScript bridge wraps `@anthropic-ai/claude-agent-sdk`. Rust and TypeScript communicate over stdin/stdout using newline-delimited JSON command and event envelopes.

Rust sends commands such as session creation, session resume, prompt submission, permission responses, MCP actions, and runtime refresh requests. The bridge sends events such as assistant messages, tool updates, permission requests, question requests, available commands, modes, models, usage, and errors.

## Packaging

The npm package exposes a `claude-rs` JavaScript launcher. That launcher finds the platform-specific Rust binary downloaded during npm `postinstall` and forwards CLI arguments to it.

Release packaging includes:

- Prebuilt Rust binaries attached to GitHub Releases.
- `agent-sdk/dist/bridge.js` in the npm package.
- A copied Node runtime next to the Rust binary when postinstall can validate it.
- Fallback to `node` on `PATH` when the copied runtime is unavailable.

Source builds are different: `cargo build` or `cargo install --path .` produce only the Rust binary. They do not build or install the JavaScript bridge. Build the bridge with npm and provide it through the checkout fallback, `--bridge-script`, or `CLAUDE_RS_AGENT_BRIDGE`.

## Boundaries

Claude Code Rust owns the terminal UI, local settings surface, bridge process management, and event rendering. Anthropic owns the Agent SDK, authentication, service behavior, billing, models, and upstream Claude Code semantics.

The project does not depend on Agent SDK package subpath exports such as `/browser`, `/bridge`, or `/assistant` as the runtime path. The runtime path is the local TypeScript bridge in this repository.
