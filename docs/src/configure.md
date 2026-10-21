# Configure claude-rs

Use `/config` to change saved defaults and interface preferences. To change the model or permission mode only for the current conversation, use the [session commands](commands.md#session-commands). See the [settings reference](settings.md) for file locations, keys, scopes and effective behavior. Most settings need an active session to load; if the list is still connecting, wait for the session before editing.

## Save a preference

1. Open `/config`. In General, press `/`, type `Reduce motion`, then press `Enter` to move to the result.
2. Press `Enter` on the result to leave search with that setting selected.
3. Press `s` until User is the selected scope.
4. Use `Left` or `Right` to choose On. This kind of setting saves immediately; closing the window does not undo it.
5. Check for `Saved.` and the new value. Reduce motion applies immediately. Press `Esc` to return to chat.

To remove your override, select the same setting and scope and press `Delete`. The inherited value or Default returns; reset does not restore a previous edit.

Claude Code Rust has a fullscreen settings surface with multiple tabs. These slash commands open that surface directly:

| Command | Tab | Purpose |
| --- | --- | --- |
| `/config` | Settings → General | Edit saved defaults and everyday preferences. |
| `/memory` | Settings → Memory | Configure context, checkpoints, automatic memory and plans. |
| `/permissions` | Settings → Permissions | Configure saved permission defaults, rules and working directories. |
| `/sandbox` | Settings → Sandbox | Configure sandbox execution, filesystem, network and credentials. |
| `/hooks` | Settings → Hooks | Inspect and edit saved hook handlers without executing them. |
| `/mcp` | MCP | Inspect live MCP server status and complete MCP authorization flows. |
| `/plugins` | Plugins | Manage installed plugins, marketplace plugins, and marketplaces. |
| `/status` | Status | Inspect session, account, authentication, and runtime status. |
| `/usage` | Usage | Inspect quota and usage information reported by the active session. |
| `/help` | Help | Open fullscreen in-app help. |

The settings surface is session-aware. Settings load when the app's session starts, and some tabs need an active session before they can show live data.

## Opening and navigating

Every command in the table above opens the same fullscreen surface on a different starting tab. The tab order is:

```text
Settings -> Plugins -> Status -> Usage -> MCP -> Help
```

`Tab` moves to the next tab and `Shift+Tab` to the previous one. `Esc` closes the surface. `Enter` closes it as well wherever the selected item has no action of its own. The hint line at the bottom of the surface shows what `Enter` and `Esc` do at the current position. Each tab also has its own keys: the Plugins tab navigates plugin lists, the MCP tab opens server actions and authorization flows, the Status tab renames the session with `r` or generates a title with `g`, and the Usage tab refreshes with `r`.

### Settings panes

The Settings tab has a second row with six panes: General, Memory, Permissions, Sandbox, Hooks, and Workflows & Worktrees. Each pane is one alphabetical list. Focus moves between three areas stacked from top to bottom: the pane row, the search field, and the list.

| Focus | Key | Action |
| --- | --- | --- |
| List | `Up` on the first item | Move to the search field. |
| Search field | `Up` | Move to the pane row. |
| Search field | `Down`, `Enter` | Move to the list. |
| Pane row | `Left`, `Right` | Switch pane. |
| Pane row | `Down` | Move to the search field. |
| Pane row | `Enter` | Move to the list. |

Each pane remembers its own selection. Switching to another top-level tab and back keeps the pane, selection and search.

Below 30 columns or 12 rows the surface asks you to resize the window instead of drawing an unusable layout.

### Search

Press `/` to search the current pane. A setting matches when its label, description or setting ID contains every word you typed, ignoring case. Search covers only the current pane and never looks inside saved values such as hook commands, credential entries or environment values.

| Key | Action |
| --- | --- |
| `Enter` in the search field | Move to the results. |
| `Enter` on a result | Clear the query and keep that setting selected in the full list. |
| `Esc` | Clear the query and return to the previous position. Without a query, `Esc` closes the surface. |

Switching panes clears the search.

## Reading the list

Each row shows a setting and its value, and a counter such as `1/27` shows the position in the pane. Below the list, `Description:` explains the selected setting and a context line states where its value comes from at the selected scope:

| Context line | Meaning |
| --- | --- |
| `Saved in User` | The value is saved at the selected scope. |
| `User value: … · Overridden by Project` | A value is saved at the selected scope, but another scope takes precedence. |
| `From Project · Not set in User` | The value comes from another scope. |
| `Using Default` | No scope sets a value. |
| `Not set in User` | A list or structured setting has no entries at the selected scope. |
| `Saved in User · Not loaded` | The file contains the value, but Claude did not load it. |
| `· Also supplied by Project` | Another scope contributes as well, as with merged lists. |
| `· Applies immediately`, `· Applies to new sessions` | When a saved change takes effect. |

`Default` means that no scope sets a value. It does not say whether the built-in behavior is On or Off; where the built-in behavior is known, the description states it, for example `Default: On`.

Read-only restrictions and settings-file errors appear as warnings below the description.

Claude decides which settings files it loads: some invalid entries are ignored individually, while other schema errors exclude the whole file. `Not loaded` reports that outcome without naming the cause. Saving and resetting still work on any readable file, and correcting the offending value restores loading.

## Editing a value

| Key | Action |
| --- | --- |
| `Up`, `Down` | Select the previous or next setting. |
| `Home`, `End` | Select the first or last setting. |
| `Page Up`, `Page Down` | Move one page. |
| `Left`, `Right` | Step through the choices of an On/Off or fixed-choice setting. The change is saved immediately. |
| `Space` | Same as `Right` for fixed choices. Opens the editor for text, number, list and structured settings. |
| `Enter` | On a list or structured setting, opens the editor, or opens it for inspection when the scope is read-only. On any other setting, closes the surface. |
| `Delete` | Reset the setting at the selected scope. |
| `s` | Cycle the save scope. |
| `r` | Reload settings from disk. |

Changes made with `Left`, `Right` or `Space` are saved as you make them, so closing with `Enter` or `Esc` discards nothing.

The text editor used for settings such as Language takes `Enter` to save, `Ctrl+R` to reset and `Esc` to cancel.

Reset removes the value saved at the selected scope. The row then shows the value from another scope, or `Default`. Reset does not write a replacement value and does not restore an earlier edit.

While a save is in progress the row keeps showing the last confirmed value. A successful save shows `Saved.`

Saving a default never changes the running session. Use `/model`, `/mode`, `/effort`, `/thinking`, `/agent`, `/fast` or `/ultracode` for the current session; see [Session Commands](commands.md#session-commands).

## Lists and structured settings

Permission rules, directory and domain lists, hook definitions and the structured sandbox settings open an editor instead of changing in place. The row in the main list shows how many entries are saved at the selected scope.

The editor loads only the entries saved at the selected scope, not a merged view of all scopes, and keeps that scope until it closes.

| Key | Action |
| --- | --- |
| `Up`, `Down`, `Home`, `End` | Move through entries or fields. |
| `a` | Add an entry. |
| `Enter`, `Space` | Edit the selected entry, open a nested group, or toggle an On/Off field. |
| `Delete` | Remove the selected entry or optional field. |
| `Ctrl+S` | Save the draft. |
| `Ctrl+R` | Reset the whole setting at this scope. |
| `Esc` | Go up one level. At the top level, discard the draft and close the editor. |
| `Ctrl+J` | Switch to a JSON view of the same draft. |
| `Ctrl+F` | Return from JSON to the form. The JSON must be valid. |

While typing in a field:

| Key | Action |
| --- | --- |
| `Enter` | Accept the field into the draft. |
| `Esc` | Cancel the field. |
| `Ctrl+U` | Clear the field. |
| `Ctrl+J` | Insert a line break. |
| `Up`, `Down`, `Left`, `Right` | Choose a value when the field offers fixed choices. |

Rules for these structured editors:

- Edits stay in a draft until `Ctrl+S`. Accepting a field does not write the file.
- Many lists merge across scopes. Removing the last entry saves an explicit empty list, and entries from other scopes still apply. Reset removes the setting from this scope instead.
- Invalid JSON or a value that Claude rejects keeps the draft open for correction. Nothing is written.
- If the file changed on disk while the editor was open, the editor loads the latest version and keeps your draft. Save again to confirm.
- If the session moves to another project while an editor is open, saving is blocked. Close the editor and open it again.
- A scope that cannot be edited can still be opened for inspection.
- Opening, browsing and validating never run hooks or credential helpers, and never read secret files or environment values. Header values and command arguments are redacted in summaries.
- Fields that the form does not know are preserved when you save.

### Editing permission rules

In the Permissions pane, open Allow rules, Ask rules, Deny rules or Additional directories. Add one tool rule per entry, for example `Bash(npm test *)` or `Read(./src/**)`. A deny rule wins over an allow rule from any scope.

### Editing hooks

Hooks → Definitions shows a flat list with each hook's event, matcher and action.

- `a` starts guided creation: choose an event, enter an optional matcher, choose an action, fill its required fields, then review. `Enter` on the review step adds the hook to the draft and opens its optional fields. `Esc` goes back one step and cancels at the first step.
- `Enter` edits the selected hook.
- `m` edits the matcher of the hook's group. Every hook in that group shares the matcher.
- `Delete` removes only the selected hook.

Hook actions are command, prompt, agent, HTTP and MCP tool. Event names come from the installed SDK. The JSON view shows the same definitions in the settings-file format:

```json
{
  "PreToolUse": [
    {
      "matcher": "Bash",
      "hooks": [
        { "type": "command", "command": "echo Check shell operation", "timeout": 10 }
      ]
    }
  ]
}
```

Saving replaces the `hooks` object at the selected scope. Hooks from other scopes keep merging in. Disable all is a separate On/Off setting.

Command and HTTP actions offer **Failure policy** (`onFailure`): `continue` reports a failed hook and lets the guarded action proceed; `block` treats the failure as exit code 2. Failures include startup errors, timeouts, exit codes other than 0 or 2, and invalid JSON or JSON that does not match the hook schema. Unset uses `continue`. Select the field and press `Delete` to remove only that handler's override, then `Ctrl+S` to save.

The SDK ignores this policy for asynchronous hooks and the Stop, SubagentStop, TaskCompleted and TeammateIdle events. It decides whether an event can block an action. Editing or validating the policy never executes a hook.

### Editing sandbox settings

Path, domain and command lists use the list editor. Credential protection, TLS termination, Ignored violations and Ripgrep command open forms:

- Required fields are marked with `*`.
- Fixed choices open a picker.
- Credential entries take file paths and variable names, never secret values.
- TLS termination takes both a CA certificate path and a CA key path, or neither.
- Proxy ports accept whole numbers from 1 to 65535.

## Customize the footer

Open `/config` → General → Footer with `Enter` or `Space`. The chat footer uses one row with independently ordered left- and right-aligned groups. Changes save automatically, and the running footer changes after each successful save. Layout and colors load before the first frame and remain available while connecting or authenticating. You can edit the footer when authentication is required, before an agent session exists. The field-position counter appears in the dialog's top-right corner. The settings row shows `Default layout` until a layout is saved, then `Custom layout`.

| Key | Action |
| --- | --- |
| `Up`, `Down` | Select a field. |
| `Space` | Toggle visibility, preserving the field's side and position. |
| `Left`, `Right` | Switch between Left and Right; append the field to the destination group. |
| `Ctrl+Up`, `Ctrl+Down` | Move the field within its group; selection follows it. |
| `C` | Expand or collapse the inline color palette for an eligible field. |
| `Ctrl+R` | Remove the saved layout and restore defaults. |
| `Esc` | Close after any pending changes finish saving. |

You can keep editing while a save is in progress. Later edits are saved next, using the confirmed file revision. A failed save or conflict keeps your edits in the dialog and shows an error; `Enter` retries the save, and `Esc` discards the unsaved edits and closes. Session changes leave the active footer, editor draft, and local saves intact.

Model, effort, working directory, Git branch, and context remaining support optional colors. Press `C` to expand a palette directly below the selected field. Arrow keys move between color boxes; the selected color name appears underneath. `Enter` applies and autosaves the choice, while `Esc` or `C` collapses the palette without applying it. Choose `Current` (`Home`) to restore the field's existing appearance. Permission mode, Fast mode, and Ultracode retain their fixed colors. Narrow windows show fewer boxes per row and keep the focused box visible.
