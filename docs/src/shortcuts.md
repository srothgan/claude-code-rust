# Keyboard Shortcuts

Keyboard shortcuts are context-sensitive. Use `/docs shortcuts` in the app to show the live shortcuts for the current state.

## Global

| Shortcut | Action |
| --- | --- |
| `Ctrl+Q` | Quit. |
| `Ctrl+L` | Redraw. |
| `Ctrl+Z` on Unix | Suspend the process. |

When the app is blocked before a usable session is available, `Ctrl+C` quits.

## Chat Input

| Shortcut | Action |
| --- | --- |
| `Enter` | Submit. |
| `Shift+Enter`, `Ctrl+Enter` | Insert newline. |
| `Esc` | Cancel the active assistant turn. |
| `Ctrl+C` | Clear the local draft, or quit when the draft is empty. |
| `Tab` | Focus prompts or accept suggestions. |
| `Shift+Tab` | Cycle mode. |
| Arrow keys | Move through text. |
| `Home`, `End` | Move to line start or line end. |
| `Ctrl+Left`, `Ctrl+Right` | Move by word. |
| `Alt+Left`, `Alt+Right` | Move by word. |
| `Ctrl+Backspace`, `Ctrl+Delete` | Delete by word. |
| `Alt+Backspace`, `Alt+Delete` | Delete by word. |

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
