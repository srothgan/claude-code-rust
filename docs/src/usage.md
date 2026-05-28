# Usage

Start a new session in the current directory:

```bash
claude-rs
```

Start in a specific working directory:

```bash
claude-rs -C path/to/project
```

Resume a previous session:

```bash
claude-rs resume
```

Resume by session id:

```bash
claude-rs resume <session_id>
```

The app prints a resume hint on clean exit when the active session has an id.

## CLI Options

The installed `claude-rs --help` command exposes these options:

| Option | Purpose |
| --- | --- |
| `--no-update-check` | Disable startup update checks. |
| `-C, --dir <DIR>` | Run in a specific working directory. |
| `--bridge-script <PATH>` | Use a specific Agent SDK bridge script. |
| `--enable-logs` | Enable diagnostics using the default log path when no `--log-file` is set. |
| `--diagnostics-preset <runtime|session|render|bridge|full>` | Use a named diagnostics filter. |
| `--log-file <PATH>` | Write tracing diagnostics to a specific file. |
| `--log-filter <FILTER>` | Use explicit tracing filter directives. |
| `--log-append` | Append to the active log file instead of resetting it on startup. |
| `--enable-perf` | Enable perf telemetry when the binary was built with the `perf` feature. |
| `--perf-log <PATH>` | Write high-frequency perf telemetry to a specific JSON-lines file. |
| `--perf-append` | Append to the perf log instead of truncating it. |

See [Diagnostics](diagnostics.md) before enabling verbose logs or perf telemetry.

## Core UI

The main screen is a terminal-owned chat view. The app renders messages, tool calls, diffs, permissions, questions, autocomplete, and status directly through Crossterm and Ratatui.

Common surfaces:

- Chat input for prompts and multiline text.
- File, slash-command, and subagent autocomplete.
- Inline permission prompts for tool decisions.
- Inline questions for agent-requested choices or text.
- Fullscreen settings, status, usage, MCP, plugins, and help tabs.
- Session picker when running `claude-rs resume` without a session id.

Use `/help` for the fullscreen help tab, `/config` for settings, and `/docs <topic>` for live in-chat help generated from the running app state.

## More Usage Topics

- [Slash Commands](commands.md)
- [Keyboard Shortcuts](shortcuts.md)
- [Settings](settings.md)
- [Diagnostics](diagnostics.md)
- [Help](help.md)
