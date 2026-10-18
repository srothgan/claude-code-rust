# Keyboard Shortcuts

Keyboard shortcuts are context-sensitive. Use `/docs shortcuts` in the app to show the live shortcuts for the current state.

## Global

| Shortcut | Action |
| --- | --- |
| `Ctrl+Q` | Quit. |
| `Ctrl+L` | Redraw. |
| `Ctrl+Z` on Unix | Suspend the process. |

In a fullscreen view, `Ctrl+C` closes the view and returns to chat. After shutdown begins, pressing `Ctrl+C` again forces any remaining cleanup to stop.

## Chat Reading

| Shortcut | Action |
| --- | --- |
| `Page Up` | Pause following and read earlier output. |
| `Page Down` | Read later output. |
| `Ctrl+End` | Jump to the latest output. Following resumes when Auto-scroll is On. |

See [Presentation and scrolling](settings.md#presentation-and-scrolling) for the Auto-scroll setting.

## Settings Surface

The fullscreen settings surface has its own keys for tabs, panes, search and editors. See [Opening and navigating](settings.md#opening-and-navigating), [Editing a value](settings.md#editing-a-value) and [Lists and structured settings](settings.md#lists-and-structured-settings).

## Plugins

Open `/plugins`. `Left` and `Right` switch between Installed, Plugins, Marketplace and Session; `Up` and `Down` select an entry. Outside search, `l` opens Load local folder. In Session, Enter opens the selected folder for reloading, or the new-folder input; `r` reloads the runtime inventory. In the folder input, Enter requests loading and Esc returns. A held reload defaults to declining; select Apply explicitly to add the same folders and invalidate the prompt cache. Esc declines and keeps the folder draft.

## Subagents View

`/agents`, or `Left` in an empty input, opens the current session's subagents.

| Shortcut | Action |
| --- | --- |
| `Up`, `Down` | Select a subagent. |
| `Enter` | Open the selected subagent's saved transcript. |
| `s` | Ask the selected running subagent to stop. |
| `r` | Check again which subagents have a saved transcript, and read the selected subagent's response again. |
| `Page Up`, `Page Down` | Scroll the selected subagent's details. |
| `Esc` | Return to chat with the draft unchanged. |

In a transcript, `Up` and `Down` scroll, `Page Up` and `Page Down` scroll by a page, `r` reads the whole saved conversation again at the same position (also after a failed read), and `Esc` or `Left` returns to the list. If the agent starts another run, the open transcript stays labeled as a saved snapshot until `r`; returning to the list refreshes the current run's saved-response preview.

## Chat Input

| Shortcut | Action |
| --- | --- |
| `Enter` | Submit. |
| `Shift+Enter`, `Ctrl+Enter` | Insert newline. |
| `Esc` | Cancel the active assistant turn. |
| `Ctrl+C` | Clear the local draft, or quit when the draft is empty. |
| `Tab` | Focus prompts or accept suggestions. |
| `Shift+Tab`, `Shift+Right` | Switch to the next permission mode, wrapping after the last. |
| `Shift+Left` | Switch to the previous permission mode, wrapping before the first. |
| `Shift+Up`, `Shift+Down` | Raise or lower effort to the next level the current model supports, stopping at the highest and lowest level. |
| `Up` | Move up through text, or recall the latest user message when the input is empty. |
| `Left` | Move left through text, or open `/agents` when the input is empty. |
| `Down`, `Right` | Move through text. |
| `Home`, `End` | Move to line start or line end. |
| `Ctrl+Left`, `Ctrl+Right` | Move by word. |
| `Alt+Left`, `Alt+Right` | Move by word. |
| `Ctrl+Backspace`, `Ctrl+Delete` | Delete by word. |
| `Alt+Backspace`, `Alt+Delete` | Delete by word. |

`Shift` with an arrow key changes the permission mode or effort; it does not select text. Mode and effort shortcuts act only while the composer has focus and no turn or command is running. They change the current session only and keep the draft. The footer updates once Claude confirms the change.

Readline-style bindings are also supported:

| Shortcut | Action |
| --- | --- |
| `Ctrl+A`, `Ctrl+E` | Move to line start or line end. |
| `Ctrl+B`, `Ctrl+F` | Move one character. |
| `Ctrl+D` | Delete after cursor. |
| `Ctrl+H` | Delete before cursor. |
| `Ctrl+K` | Kill to line end. |
| `Ctrl+U` | Kill to line start. |
| `Ctrl+W` | Delete previous word. |
| `Ctrl+Y` | Yank. |
| `Alt+B`, `Alt+F` | Move by word. |
| `Alt+D` | Delete next word. |

## Undo And Redo

| Platform | Undo | Redo |
| --- | --- | --- |
| macOS | `Cmd+Z` | `Cmd+Shift+Z`, `Cmd+Y` |
| Windows | `Ctrl+Z` | `Ctrl+Shift+Z` |
| Unix except macOS | `Ctrl+_`, `Ctrl+/` | `Ctrl+Shift+Z` |

On Unix except macOS, `Ctrl+Z` is reserved for process suspend.

## Autocomplete

| Shortcut | Action |
| --- | --- |
| `Up`, `Down` | Move through candidates. |
| `Enter`, `Tab` | Accept the selected candidate. |
| `Esc` | Cancel autocomplete. |

## Inline Permissions

| Shortcut | Action |
| --- | --- |
| `Left`, `Up` | Move to the previous option. |
| `Right`, `Down` | Move to the next option. |
| `Enter` | Confirm the focused option. |
| `Esc` | Cancel. |
| `Tab` | Move focus. |

Letter shortcuts such as `Ctrl+A`, `Ctrl+Y`, or `Ctrl+N` are not permission shortcuts.

## Inline Questions

| Shortcut | Action |
| --- | --- |
| `Left`, `Up` | Move to the previous option. |
| `Right`, `Down` | Move to the next option. |
| `Home`, `End` | Move to first or last option. |
| `Space` | Toggle/select where applicable. |
| `Enter` | Submit. |
| `Esc` | Cancel. |
| `Tab` | Toggle notes or move focus. |
| `Shift+Tab` | Move focus backward. |

## MCP input forms

| Shortcut | Action |
| --- | --- |
| `Tab`, `Down` | Next field or action. |
| `Shift+Tab`, `Up` | Previous field or action. |
| `Left`, `Right` | Move the text cursor or change the highlighted choice. |
| `Space` | Toggle a boolean or multi-choice option; insert a space in text. |
| `Ctrl+O` | Include or omit the focused optional field. |
| `Enter` | Advance from a field or activate the selected action. |
| `Ctrl+Enter` | Validate and accept, if distinguished by the terminal. |
| `Esc` | Cancel the active request. |

Type or paste into text and numeric fields. `Home`, `End`, `Backspace` and `Delete` edit the focused text. While a response awaits SDK confirmation, editing and repeated submission are disabled. Queued requests retain their own drafts and activate in arrival order.
