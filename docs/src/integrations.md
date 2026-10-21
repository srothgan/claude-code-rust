# Use MCP servers and plugins

MCP (Model Context Protocol) servers provide tools to a session. Plugins can supply skills, agents and other capabilities. Open or resume a conversation before inspecting live runtime state; persistent plugin management also requires the [Claude CLI](about.md#claude-cli-dependencies).

## Inspect and authorize an MCP server

1. Enter `/mcp` in the composer.
2. Select the server you want to inspect. Use the actions offered for that server to authorize or reconnect it.
3. Complete any browser authorization or input request, then refresh status to check the result. Available actions depend on what the session reports.

If the list is empty, check which servers the session was configured to load. Launch-time configuration is described under [MCP flags](cli.md#process-wide-launch-configuration).

## Answer an MCP input request

The MCP tab shows live session-backed MCP state. Use `/mcp` to inspect servers, refresh status, complete authorization, reconnect servers, and handle SDK-provided MCP prompts when available.

If no session is active, the tab asks you to open or resume a session first. If the active session reports no MCP servers, the tab shows an empty state rather than editing raw config files.

MCP input requests open a dialog over this tab. Supported forms edit strings, numbers, whole numbers, booleans, single choices, and arrays of string choices, including titled choices and defaults. Required fields and supported length, numeric range, choice-count and date/email/URI/date-time constraints are validated before Accept. `false` and `0` are answers. Optional fields start omitted unless the server supplies a default; `Ctrl+O` includes or omits the focused optional field, so an included empty string differs from omission. Unsupported schemas show a reason and allow Decline or Cancel without offering Accept.

Use `Tab`/`Shift+Tab` or `Up`/`Down` to move between fields and actions. Type or paste into text and numeric fields, use `Left`/`Right` for choices, and `Space` to toggle a boolean or the highlighted multi-choice option. `Enter` advances from a field or activates the selected action. `Ctrl+Enter` accepts when the terminal distinguishes it; navigating to Accept works in every terminal. `Esc` cancels. A validation or send error retains the draft for correction or retry.

Overlapping requests wait in arrival order; the dialog shows the waiting count. A new request does not replace the form being edited. Only the active URL request opens a browser. SDK cancellation removes its matching request, including one still queued. A submitted dialog stays pending until its SDK callback settles, and prevents duplicate submission. Drafts remain in memory for the active session and are discarded on settlement or session teardown; form answers are not written as settings or diagnostic fields.

## Plugins

The Plugins tab is available through `/plugins`. It shows installed plugins, marketplace plugins, configured marketplaces, and Session attachments.

Supported actions include enabling, disabling, updating, uninstalling, and installing plugins into user, project, or local scopes when those actions are available for the selected plugin.

After plugin changes, the app requests a session runtime plugin reload when an active session is available.

## Load a plugin folder for this session

1. Open `/plugins` and switch to the Session list.
2. Choose Load local folder, or press l outside search.
3. Enter an absolute plugin directory or a path relative to the session's working directory.
4. If the app asks whether to invalidate the prompt cache, review the choice and select Apply to continue.
5. Check for the confirmed folder labeled Source: Session. Its SDK source and version appear when supplied.

This does not install the plugin into a persistent scope or change marketplaces. After editing a loaded plugin, select its folder and press Enter to load it again. Press r to refresh the inventory.

A reload that would invalidate the prompt cache opens an Apply/Decline confirmation. Until Apply, no requested folder is added. Apply submits the same folders; declining keeps the draft and the session's existing plugins. A rejected request keeps the draft for retry.

A reload can apply with component errors: the app shows the SDK's error count and reconciles the confirmed plugins, commands, agents and MCP servers. A host reconciliation failure is reported separately and does not imply that the attachment was rolled back.

Confirmed attachments are retained when the bridge replaces or resumes the same active session, including rewind. They are not persisted by the host: after restarting the app or switching away and returning, load the folders again. A fresh session does not inherit them. Before the first prompt after a replacement, press `r` in Session to obtain a new SDK inventory if it is not yet listed. Managed policy, remote environments or an already-running sandbox can reject local folder loading; the SDK's refusal is shown without a persistent-install fallback.
