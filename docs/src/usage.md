# Usage

Start a new session in the current directory:

```bash
claude-rs
```

While the session connects, the crab beside the welcome overview scuttles and looks around; on the first connect of the process it waves once, then stays still. The `prefersReducedMotion` setting keeps it still throughout, once claude-rs has read that setting from the connected session.

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

`--model`, `--effort`, `--permission-mode`, and `--agent` apply to the initial new or resumed session before its first prompt. They do not write saved preferences. Later sessions started inside the TUI use saved settings. Model aliases, full model IDs, agent names, and model-specific effort availability are resolved by the SDK. Effort values are `low`, `medium`, `high`, `xhigh`, and `max`; `max` is a session-only override. Permission modes are `default`, `auto`, `acceptEdits`, `plan`, `dontAsk`, and `bypassPermissions`.

Name the session as you start it, or branch from an existing one:

```bash
claude-rs -n "Release prep"
claude-rs --continue --name "Release prep, day two"
claude-rs --continue --fork-session --name "Experiment"
```

`-n, --name` names a new session or renames the session being resumed or continued. A new or forked session takes the name as its title, and the SDK reports a new session's title after its first turn. A resumed session is renamed once it has connected, through the same SDK rename as the Status tab, so a resume that fails to start, or that asks you to sign in, leaves its saved title unchanged; a rename that fails is shown as an error and the resumed session stays open. Control characters are removed and a name with no visible characters is rejected. `--fork-session` resumes under a new session ID and leaves the original conversation and its title unchanged; it requires `--resume`, `--continue` or the `resume` subcommand. With `--continue` and no earlier session in the directory, `--fork-session` has nothing to fork and a plain new session starts. The resume hint after exit shows the new session ID. Both apply to the initial session only.

Add directories, settings, MCP servers and system prompt text for a run:

```bash
claude-rs --add-dir ../shared --settings ./settings.json --mcp-config ./mcp.json
claude-rs --settings '{"cleanupPeriodDays": 7}' --strict-mcp-config --mcp-config ./mcp.json
claude-rs --system-prompt-file ./release-bot.md --append-system-prompt "Answer in German."
```

- `--add-dir <PATH>` gives Claude access to a directory beyond the working directory; repeat it for more.
- `--settings <JSON_OR_FILE>` takes a JSON object inline (a value starting with `{`) or the path of a JSON file. It becomes the SDK's flag-settings layer, which outranks the user, project and local settings files; managed policy still outranks it. The layer reaches SDK behaviour only: `claude-rs`'s own interface preferences (tips, notifications, time format, auto-scroll) are read from the saved settings files, so putting them in `--settings` does nothing. Explicit launch options such as `--model` are applied as SDK options of their own. `claude-rs` lays two host restrictions over this layer: `feedbackDrafts` is always `off`, and `crossSessionInbound` is `refuse` unless the value in effect, from this layer or else the saved settings, is `accept` or `refuse`. Draft review and held peer-message review need surfaces `claude-rs` does not have.
- `--mcp-config <JSON_OR_FILE>` takes the same inline-or-file form, with an object that contains `mcpServers`, and can be repeated. Servers are merged in command-line order, and a later server replaces an earlier one of the same name. The SDK validates each server definition. `--strict-mcp-config` restricts the session to the servers supplied this way, ignoring project `.mcp.json`, user settings, plugins and agent definitions; without any `--mcp-config` it starts the session with no MCP servers.
- `--system-prompt <TEXT>` or `--system-prompt-file <PATH>` replaces the default system prompt, and `--append-system-prompt <TEXT>` or `--append-system-prompt-file <PATH>` appends instructions to it. The text and file spellings of each option cannot be combined, and an empty prompt, inline or in a file, is rejected. `claude-rs` keeps one instruction of its own in every system prompt, which tells Claude not to stop the bridge runtime process: appended text follows it, and a replacement prompt is followed by it.

Relative paths resolve against `--dir`, or the current directory without it. Everything is read and checked before logging, the bridge or the terminal starts: a missing directory, an unreadable or empty file, invalid JSON or an MCP config without `mcpServers` stops the launch with an error that names the flag, and nothing is renamed, written or started. Files are read once at startup, so later edits do not reach a running session. None of these flags write saved preferences, and the Config window shows the saved settings files, not the `--settings` layer.

Lifetime: `--name` and `--fork-session` apply to the initial session. The other flags stay in effect for every session the process starts, including `/new-session`, sessions opened with `/resume` and a new session after login. The SDK records a conversation's system prompt on its first request where recording is enabled; a resumed or continued session then keeps the recorded prompt and ignores a different one, until compaction or a new session. Launch flags apply to interactive launches and cannot be combined with support commands.

An initial prompt whose text matches a subcommand name can be passed after `--`, for example `claude-rs -- "doctor"`. Startup arguments cannot be combined with support commands such as `doctor`, `logs`, `config`, or `completions`.

`--dir` and `--bridge-script` are checked before the terminal, trust prompt, or bridge start. A missing path, a file where a directory is expected, a directory where the script is expected, a path behind a directory that cannot be searched, a directory without read or search (execute) permission, or a script that cannot be read prints an error naming the flag and exits with status 2. Nothing is saved. Support commands do not refuse them: `claude-rs --bridge-script <PATH> doctor` reports the same bridge script problem as a failed check instead.

A relative `--dir` is resolved against the current directory to its absolute path, with `.` and `..` resolved (following symlinks on Unix, lexically on Windows), before it is used for the session, shown as `Loc:`, or read by `config` and `doctor`. An absolute `--dir` is used as given. `claude-rs --dir .` shows and uses the current directory's path, not `.`.

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

Every run writes a default log of warnings and errors, including failures reported by the bridge and the SDK, without any flag. `claude-rs logs --latest` prints its path, and `--enable-logs` adds the detailed records.

A hook that fails shows one warning in the transcript with its exit code and the start of its output, and the failure is also in the default log. Exit code 2 depends on the hook event, as in the [hook reference](https://code.claude.com/docs/en/hooks#exit-code-2-behavior-per-event). Where it is the hook's decision, such as blocking a tool call or a prompt, keeping Claude working, or handing the output to Claude after `PostToolUse`, it is not a failure and is recorded only in the detailed log; when nothing else shows that decision, as after `PostToolUse`, `PostToolUseFailure` or `PostToolBatch`, the transcript shows one line saying the hook returned a blocking error. Where Claude Code only shows the output to the user, ignores the exit code, or fails the operation on any non-zero exit (as `WorktreeCreate` does), and on events the reference does not list, it is reported as a failure. A hook cancelled by an interrupted turn is not reported at all.

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
| `-n, --name <TEXT>` | Name a new session, or rename the session being resumed. |
| `--fork-session` | Resume under a new session ID, leaving the original unchanged. |
| `--add-dir <PATH>` | Add a directory Claude may access; repeatable. |
| `--settings <JSON_OR_FILE>` | Apply temporary settings as inline JSON or a JSON file. |
| `--mcp-config <JSON_OR_FILE>` | Add MCP servers from inline JSON or a file with an `mcpServers` object; repeatable. |
| `--strict-mcp-config` | Use only the MCP servers from `--mcp-config`. |
| `--system-prompt <TEXT>` | Replace the default system prompt for new sessions. |
| `--system-prompt-file <PATH>` | Read the replacement system prompt from a file. |
| `--append-system-prompt <TEXT>` | Append instructions to the system prompt for new sessions. |
| `--append-system-prompt-file <PATH>` | Read the appended system prompt instructions from a file. |
| `--no-update-check` | Disable startup update checks. |
| `-C, --dir <DIR>` | Run in a specific working directory, which must exist and be readable and searchable. |
| `--bridge-script <PATH>` | Use a specific Agent SDK bridge script, which must be a readable file. |
| `--enable-logs` | Add detailed info-level diagnostics to the always-on warning and error log. |
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
