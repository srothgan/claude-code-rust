# Architecture

Claude Code Rust is a terminal application with two local components: a Rust process owns the interface and application state; a TypeScript bridge translates between the app and Anthropic's Agent SDK. Read [Development](development.md) to run a checkout and [Testing](testing.md) to validate a change.

## Runtime Shape

```text
Keyboard / terminal events
           |
           v
Rust app state and event loop <----> Rust terminal rendering
           |
     NDJSON commands / events
           |
           v
TypeScript bridge <----> Claude Agent SDK <----> Claude services and tools
```

Rust starts a Tokio runtime and runs the app inside a `LocalSet`, which supports the non-`Send` parts of terminal and child-process handling. The bridge is a child process running under Bun. Newline-delimited JSON (NDJSON) carries one command or event envelope per line over its standard input and output.

Support commands such as `doctor`, CLI help, completions and manual generation take a separate path before an interactive session starts. Detailed launch resolution belongs in [Startup and launch settings](subsystems.md#startup-and-launch-settings).

## Follow one prompt

1. Startup resolves the working directory and launch arguments. The app handles trust, authentication and session selection before admitting an initial prompt.
2. The composer submits a prompt through the application's existing input and command path. The bridge client sends a command envelope over stdin.
3. The TypeScript bridge maps the command to the active SDK session. The SDK supplies model output, tool activity and interaction requests.
4. The bridge converts those results into typed events. Rust applies them to the relevant session state; permission and question requests become interactive UI.
5. Rendering projects the canonical transcript into terminal rows. Finished output enters terminal scrollback; the live reply, composer and footer remain in the active area.
6. A permission answer or other interaction travels back through the command path. Completion, cancellation and failure are handled by their lifecycle owners, rather than inferred from displayed text.

The invariant reference explains the identity checks, saved-history behavior and failure paths behind this flow. In particular, saved history must not cause another desktop notification, and a transport acknowledgement does not by itself prove that an operation completed.

## Rust Terminal App

| Area | Responsibility |
| --- | --- |
| `src/main.rs` | Process entrypoint, runtime, logging and exit behavior. |
| `src/lib.rs` | CLI arguments, subcommands and diagnostics presets. |
| `src/agent/` | Bridge resolution, process client, wire types and bridge errors. |
| `src/app/` | Application state, lifecycle, sessions, input, permissions, settings and trust. |
| `src/ui/` | Ratatui and Crossterm rendering of chat, tools, dialogs and fullscreen views. |

The normal conversation renders inline. Fullscreen views temporarily use the alternate screen and return to chat when closed. Transcript layout, insertion into terminal scrollback, and resize replay have separate responsibilities; see [Transcript rendering](subsystems.md#transcript-rendering) and [Resize replay](subsystems.md#resize-replay) before changing that path.

## Agent SDK Bridge

`agent-sdk/src/bridge.ts` and the modules under `agent-sdk/src/bridge/` adapt SDK commands, callbacks and history into the application's protocol. The runtime executes the compiled `agent-sdk/dist/bridge.js`; editing TypeScript requires rebuilding it.

The paired wire definitions are `src/agent/wire.rs` and `agent-sdk/src/types.ts`. Update both in one change when the transport contract changes. These definitions describe the internal Rust/bridge boundary, not a supported third-party SDK.

The SDK owns subprocess option serialization. The bridge forwards its argument array without reconstructing it. Assistant text and Markdown links are preserved across streaming, completed replies and saved history; structured citation objects are not transported to Rust as source references.

## Where a change belongs

| Change | Start here | Relevant coverage |
| --- | --- | --- |
| Launch flag or startup behavior | `src/lib.rs`, launch resolution, bridge session lifecycle | `tests/cli_startup.rs`, `tests/docs_cli_contract.rs` |
| Saved setting or structured editor | Bridge settings catalog/service; Rust config state and UI | Bridge settings tests, `tests/config_flow.rs` |
| Slash command or composer interaction | App command and input handling, bridge dispatch where needed | `tests/slash_input_flow.rs`, bridge command tests |
| Transcript layout, resizing or shutdown | UI layout and application terminal lifecycle | `tests/terminal_resize.rs` and rendering unit tests |
| SDK event or wire change | Bridge event projection and Rust conversion/consumer | Bridge contract tests plus the affected Rust workflow |
| Installer or package contents | `scripts/`, npm launcher and platform mapping | The `npm-package-layout` CI job; [Packaging](packaging.md) |

The [testing guide](testing.md) maps these areas to executable checks. For a cross-boundary change, follow the actual input through its consumer and observable outcome before choosing tests.

## Subsystems

Each link below owns the detailed rules for that concern.

| Concern | Detailed reference |
| --- | --- |
| MCP input requests | [Queue ownership, validation, settlement and teardown](subsystems.md#mcp-elicitation-ownership) |
| Settings and trust | [Catalog, scope resolution, writes, trust and updates](subsystems.md#settings-and-trust) |
| Presentation and timing | [Metadata provenance and formatting](subsystems.md#presentation-and-timing) |
| Transcript rendering | [Canonical content and resumable row production](subsystems.md#transcript-rendering) |
| Resize replay | [Preparation, insertion, cancellation and shutdown](subsystems.md#resize-replay) |
| Transcript reading | [Stable anchors and following policy](subsystems.md#transcript-reading) |
| Response copying | [Canonical text and clipboard boundary](subsystems.md#copy) |
| Turn activity | [SDK activity and presentation decisions](subsystems.md#turn-activity-and-thinking) |
| Background tools | [Run identity and lifecycle outcomes](subsystems.md#background-tool-execution) |
| Subagents | [Discovery, saved transcripts and stop requests](subsystems.md#subagents-view) |
| Session titles | [SDK title ownership and rename persistence](subsystems.md#session-title) |
| Notifications | [Delivery eligibility, deduplication and replay](subsystems.md#notifications) |
| Plugin attachments | [Reload, confirmation and session lifetime](subsystems.md#session-plugin-attachments) |

## Boundaries

Claude Code Rust owns the terminal UI, local settings surface, bridge process management and event rendering. Anthropic owns the SDK, authentication and service behavior, billing, models and upstream Claude Code semantics.

The application uses its local TypeScript bridge as the SDK integration point. SDK package subpath exports such as `/browser`, `/bridge` and `/assistant` are not the application's runtime entry point.

## Related maintainer guides

- [Packaging and release invariants](packaging.md): artifact layout, generation and verification.
- [Governance](governance.md): repository protection and publication authority.
- [Test and validate changes](testing.md): checks for each affected area.
