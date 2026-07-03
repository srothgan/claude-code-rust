# Diagnostics

Diagnostics are off by default. Enable them only when debugging or preparing a useful issue report because verbose logs can grow quickly.

## Doctor

Run deterministic environment diagnostics:

```bash
claude-rs doctor
```

For machine-readable output:

```bash
claude-rs doctor --json
```

For CI or support scripts that should fail on hard runtime prerequisites:

```bash
claude-rs doctor --strict
```

Use `-C, --dir` with `doctor` to inspect project-local settings for a specific folder:

```bash
claude-rs -C path/to/project doctor
```

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

`--log-filter` overrides `--diagnostics-preset`. If `--log-file` is omitted but logging is enabled through `--enable-logs`, `--diagnostics-preset`, `--log-filter`, or `RUST_LOG`, the app writes to a timestamped default diagnostics file.

The default diagnostics directory is under the platform local data directory:

- Windows: `%LOCALAPPDATA%\claude-code-rust\logs\runtime\`
- Linux: usually `$XDG_DATA_HOME/claude-code-rust/logs/runtime/` or `~/.local/share/claude-code-rust/logs/runtime/`
- macOS: the platform data directory reported by the `dirs` crate, under `claude-code-rust/logs/runtime/`

Default runtime log files include the UTC start timestamp, process id, and a short run id, for example:

```text
claude-rs-20260614T075924Z-p12345-r8f3a2c1.log
```

Logs rotate at 10 MB and keep up to five rotated files per run. Default runtime logs are retained up to 256 MB or 30 days, while always preserving at least 10 newest files. Retention only applies to app-managed timestamped files in the default runtime log directory; explicit `--log-file` paths are never cleaned up by the app.

`--log-append` appends to an explicit `--log-file`. When used without `--log-file`, it appends to the legacy shared default file `claude-rs.log` for compatibility; prefer the normal timestamped defaults for new diagnostics.

## Finding Logs

Use the logs command to find diagnostics paths without starting the TUI:

```bash
claude-rs logs
claude-rs logs --path
claude-rs logs --latest
```

`claude-rs logs` prints the runtime log directory, legacy log path, perf log directory, latest discovered log, and common follow-up commands. `--path` prints only the default runtime log directory for scripts. `--latest` prints only the latest runtime log path, falling back to the legacy shared log when no timestamped runtime log exists.

To inspect recent log output safely:

```bash
claude-rs logs --tail 200
```

Tail output is redacted for obvious credentials such as API keys, bearer tokens, OAuth tokens, passwords, and authorization headers before it is printed.

## Debug Bundles

Create a redacted support bundle with:

```bash
claude-rs logs --bundle --yes
```

Without `--yes`, an interactive terminal is prompted before the bundle is written. Use `--output <PATH>` to choose the ZIP path.

The bundle includes:

- `manifest.json`
- `doctor.json`, equivalent to `claude-rs doctor --json`
- selected recent runtime logs
- the legacy log if present
- bridge diagnostics extracted from structured log records
- `last-crash.json` when the previous run crashed
- diagnostics paths

The bundle excludes full config files, Claude credentials, environment dumps, and arbitrary project files. Redaction removes obvious credentials, but logs can still contain private conversation text, local file paths, command output, or project-specific context. Review a bundle before sharing it publicly.

## Failure Reports

Top-level failures print a short issue-friendly report to stderr with a category, exit code, version, platform, latest discovered log path, and one next-step command. Bridge failures are categorized as spawn, initialization, stdout close, SDK/protocol failure, or timeout so support output points at the likely failing boundary.

Unexpected Rust panics install a local panic hook. The hook writes a redacted `last-crash.json` file in the diagnostics root and prints the same safe-to-paste metadata to stderr. No crash report is uploaded automatically.

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

When `--perf-log` is omitted, default perf telemetry uses timestamped JSON-lines files under the sibling `logs/perf/` directory.

## Useful Issue Reports

Include:

- `claude-rs --version`
- OS and terminal.
- Install method: npm package, source build, fork build, or manual binary.
- The exact command used to launch the app.
- Whether a custom bridge script or Node runtime was used.
- A short reproduction.
- A `claude-rs logs --bundle --yes` bundle or relevant redacted log snippets, not full secrets or private conversation content.
