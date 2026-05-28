# Diagnostics

Diagnostics are off by default. Enable them only when debugging or preparing a useful issue report because verbose logs can grow quickly.

## Logging

Enable runtime diagnostics with a named preset:

```bash
claude-rs --enable-logs --diagnostics-preset session
claude-rs --enable-logs --diagnostics-preset render
```

Available presets:

| Preset | Use when |
| --- | --- |
| `runtime` | Debugging general app, bridge, session, tool, permission, network, and update flow. |
| `session` | Debugging session startup, permission, and command flow. |
| `render` | Debugging rendering, cache, input, paste, and perf-adjacent UI behavior. |
| `bridge` | Debugging Agent SDK bridge lifecycle, protocol, SDK, permission, and MCP behavior. |
| `full` | Capturing the broadest diagnostic trace. |

Use an explicit log path when you want the file somewhere predictable:

```bash
claude-rs --enable-logs --diagnostics-preset bridge --log-file claude-rs.log
```

Use an explicit tracing filter for targeted debugging:

```bash
claude-rs --log-filter "info,app.render=trace,bridge.protocol=debug"
```

`--log-filter` overrides `--diagnostics-preset`. If `--log-file` is omitted but logging is enabled through `--enable-logs`, `--diagnostics-preset`, `--log-filter`, `--log-append`, or `RUST_LOG`, the app writes to the default diagnostics path.

The default path is under the platform local data directory:

- Windows: `%LOCALAPPDATA%\claude-code-rust\logs\claude-rs.log`
- Linux: usually `$XDG_DATA_HOME/claude-code-rust/logs/claude-rs.log` or `~/.local/share/claude-code-rust/logs/claude-rs.log`
- macOS: the platform data directory reported by the `dirs` crate, under `claude-code-rust/logs/claude-rs.log`

Logs rotate at 10 MB and keep up to five rotated files.

## Bridge Diagnostics

When runtime logging is active, bridge diagnostics are enabled and bridge stderr is captured into the structured log. This is useful for Agent SDK startup, authentication, MCP, permission, and protocol issues.

The bridge script can be overridden with:

```bash
claude-rs --bridge-script /path/to/agent-sdk/dist/bridge.js
```

or:

```bash
CLAUDE_RS_AGENT_BRIDGE=/path/to/agent-sdk/dist/bridge.js
```

The bridge Node runtime can be overridden with:

```bash
CLAUDE_RS_AGENT_BRIDGE_NODE=/path/to/node
```

## Perf Telemetry

Perf telemetry is a separate JSON-lines sidecar intended for high-frequency render and layout samples. It requires a binary built with the `perf` feature.

From source:

```bash
cargo run --features perf -- --enable-perf
cargo run --features perf -- --perf-log claude-rs-perf.log
```

For an already-built perf-enabled binary:

```bash
claude-rs --enable-perf
claude-rs --perf-log claude-rs-perf.log
```

If the binary was not built with `--features perf`, perf flags are rejected at startup.

## Useful Issue Reports

Include:

- `claude-rs --version`
- OS and terminal.
- Install method: npm package, source build, fork build, or manual binary.
- The exact command used to launch the app.
- Whether a custom bridge script or Node runtime was used.
- A short reproduction.
- Relevant log snippets, not full secrets or private conversation content.
