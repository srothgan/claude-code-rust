# Usage

Start a new session in the current directory:

```bash
claude-rs
```

Start with an initial prompt:

```bash
claude-rs "Explain this project"
```

The prompt is sent automatically after project trust, startup dialogs, and session selection are complete. The TUI stays open for follow-up messages.

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

The Claude-style flags use the same resume workflow:

```bash
claude-rs --resume
claude-rs -r <session_id> "Continue reviewing the changes"
claude-rs --continue
claude-rs -c "Finish the review"
```

`--resume` without an ID opens the session picker. `--continue` selects the most recently modified SDK-visible session for the working directory, including its git worktrees; this includes sessions created by `claude-rs`. If no session exists, it starts a new one. Resume accepts session IDs; name and transcript-path lookup are not supported. Use `--resume=<session_id>` to make the ID boundary explicit, or `--resume -- "prompt"` to open the picker with an initial prompt. `--resume`, `--continue`, and the `resume` subcommand cannot be combined.

Choose how the initial session starts:

```bash
claude-rs --model opus --effort high --permission-mode plan "Review this project"
claude-rs --agent reviewer "Review the current changes"
claude-rs resume <session_id> --model sonnet --effort medium
```

`--model`, `--effort`, `--permission-mode`, and `--agent` apply to the initial new or resumed session before its first prompt. They do not write saved preferences. Later sessions started inside the TUI use saved settings. Model aliases, full model IDs, agent names, and model-specific effort availability are resolved by the SDK. Effort values are `low`, `medium`, `high`, `xhigh`, and `max`; `max` is a session-only override. Permission modes are `default` (also `manual`), `auto`, `acceptEdits`, `plan`, `dontAsk`, and `bypassPermissions`.

An initial prompt whose text matches a subcommand name can be passed after `--`, for example `claude-rs -- "doctor"`. Startup arguments cannot be combined with support commands such as `doctor`, `logs`, `config`, or `completions`.

After the TUI exits, the app prints a resume hint whenever a session was established, using the active or most recently established session id.

## Support And Diagnostics

Use support commands when you need a repeatable snapshot of the local environment or diagnostic logs.

| Command | Purpose |
| --- | --- |
| `claude-rs doctor` | Run installation, runtime, config path, log path, npm metadata, and credential checks. |
| `claude-rs doctor --json` | Emit the diagnostics report as JSON. |
| `claude-rs logs` | Show diagnostics log locations and useful follow-up commands. |
| `claude-rs logs --tail <LINES>` | Print redacted lines from the latest discovered log. |
| `claude-rs logs --bundle --yes` | Write a redacted ZIP debug bundle. |
| `claude-rs config` | Show resolved config paths and file states. |
| `claude-rs config show` | Show a concise redacted config summary. |
| `claude-rs config export --output <PATH>` | Write a redacted config export without overwriting existing files. |

See [Diagnostics](diagnostics.md) for full options, logging presets, bundle contents, and sharing guidance.

## CLI Options

The installed `claude-rs --help` command exposes these options:

| Option | Purpose |
| --- | --- |
| `-r, --resume [ID]` | Resume by session ID, or open the picker without an ID. |
| `-c, --continue` | Continue the latest session for the working directory and its git worktrees; start new if none exists. |
| `--model <MODEL>` | Set the initial session model without changing saved preferences. |
| `--effort <LEVEL>` | Set the initial session effort; availability depends on the model. |
| `--permission-mode <MODE>` | Set the initial session permission mode without changing saved preferences. |
| `--agent <NAME>` | Select the main-thread agent for the initial session. |
| `--no-update-check` | Disable startup update checks. |
| `-C, --dir <DIR>` | Run in a specific working directory. |
| `--bridge-script <PATH>` | Use a specific Agent SDK bridge script. |
| `--enable-logs` | Expand the always-on baseline to detailed info-level diagnostics. |
| `--diagnostics-preset <runtime|session|render|bridge|full>` | Use a named diagnostics filter. |
| `--log-file <PATH>` | Write tracing diagnostics to a specific file. |
| `--log-filter <FILTER>` | Use explicit tracing filter directives. |
| `--log-append` | Append to the active log file instead of resetting it on startup. |

See [Diagnostics](diagnostics.md) before enabling verbose logs.

## Shell Completions

`claude-rs completions <shell>` prints a completion script for `bash`, `zsh`, `fish`, `powershell`, or `elvish`. It works with script, npm, archive, and source installations and does not start the bridge or require credentials. All five scripts suggest CLI commands and flags. Bash, Zsh, and Fish also suggest fixed choices such as effort levels; the generated PowerShell and Elvish scripts currently complete command and option names. They do not query live session IDs, model catalogs, or agents. The TUI's slash-command completion is separate.

For PowerShell, enable completions in the current session:

```powershell
claude-rs completions powershell | Out-String | Invoke-Expression
```

Put that same line in your PowerShell `$PROFILE` to enable completions in future sessions. For Bash, run the following and add it to `~/.bashrc` for future shells:

```bash
source <(claude-rs completions bash)
```

For Zsh, generate the script:

```zsh
mkdir -p ~/.zfunc
claude-rs completions zsh > ~/.zfunc/_claude-rs
```

Add `fpath=(~/.zfunc $fpath)` to `~/.zshrc` before its completion initialization. If completion is not already initialized, add `autoload -Uz compinit` followed by `compinit` after the `fpath` line.

For Fish:

```fish
mkdir -p ~/.config/fish/completions
claude-rs completions fish > ~/.config/fish/completions/claude-rs.fish
```

Regenerate scripts saved to disk after upgrading `claude-rs`. The PowerShell and Bash startup lines above generate completions from the currently installed binary each time the shell starts.

## Man Pages

A man page is a local command reference opened in a terminal with the Unix `man` viewer.

Unix release archives include generated manuals under `share/man/man1/`, including a root page and pages for visible subcommands. The Unix installer links them into `share/man/man1/` beside the installation's `bin` directory, normally `~/.local/share/man/man1/`. Open the manual with:

```bash
man claude-rs
man claude-rs-config-show
```

If your man viewer does not search that directory, use `MANPATH="$HOME/.local/share/man:" man claude-rs`; the trailing colon preserves its default search paths. Custom `--bin-dir` installations use the corresponding sibling `share/man` directory. Uninstall removes only manual links pointing into the script installation.

For npm, source, or manually extracted installations, generate the pages locally with the packaging command:

```bash
claude-rs man ./man-pages
man ./man-pages/claude-rs.1
```

The `man` generator is hidden from normal command help because it is primarily a packaging utility. It works on Windows too, although viewing the output requires a Unix-compatible man viewer. Help, completion scripts, and manual pages all derive from the same CLI definitions.

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
