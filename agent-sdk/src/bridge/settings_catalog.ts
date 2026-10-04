import type { AvailableAgent, AvailableModel, Json, SettingDescriptor, SettingsScope } from "../types.js";
import { canonicalModelName } from "./model_metadata.js";

// This is the only catalog. Rust receives descriptors, never a second set of rules.
const ALL_SCOPES: SettingsScope[] = ["user", "project", "local"];
type Definition = [string, string, string, SettingDescriptor["kind"], string[]?, SettingsScope[]?, string?];
const DEFINITIONS: Definition[] = [
  ["autoScrollEnabled", "Auto-scroll", "Follow new output. Page Up pauses following; Ctrl+End returns to live output.", "boolean"],
  ["showTurnDuration", "Show turn duration", "Show total elapsed time for completed responses.", "boolean"],
  ["showMessageTimestamps", "Show message timestamps", "Show message times. Locally observed times are marked when Claude supplies no timestamp.", "boolean"],
  ["timeFormat", "Time format", "Clock format for message times. Custom strftime patterns in settings files are preserved.", "string", ["auto", "12-hour", "24-hour", "24-hour-utc"]],
  ["presentation.showStatusInTerminalTab", "Show activity in tab title", "Show a busy/idle icon beside the folder name in the terminal tab title. Off keeps just the folder name.", "boolean", undefined, ["user"]],
  ["presentation.copyFullResponse", "Skip the /copy picker", "Copy the entire last response directly instead of choosing the response or a code block.", "boolean", undefined, ["user"]],
  ["notifications.actionsRequired", "Notify when input is needed", "Alert when a permission, question or dialog is waiting. Only while this terminal is unfocused. Default: On.", "boolean", undefined, ["user"]],
  ["notifications.modelDirected", "Notify when Claude requests it", "Allow local alerts from proactive PushNotification tool results when available. Only while this terminal is unfocused. Default: On. Mobile push is configured separately.", "boolean", undefined, ["user"]],
  ["notifications.turnComplete", "Notify when a turn finishes", "Alert after an active turn finishes successfully. Only while this terminal is unfocused. Default: On.", "boolean", undefined, ["user"]],
  ["autoCompactEnabled", "Auto compact", "Compact context automatically.", "boolean"],
  ["autoContinueAtUsageLimit", "Continue at usage limit", "Wait for a subscription usage limit to reset and continue automatically when available for your account.", "boolean", undefined, ["user"], "Automatic continuation at usage limits is not available yet."],
  ["switchModelsOnFlag", "Switch models on flagged messages", "Allow automatic alternate-model handling.", "boolean"],
  ["alwaysThinkingEnabled", "Thinking", "Thinking preference for new sessions; model restrictions still apply. Use /thinking for the current session.", "boolean"],
  ["fastMode", "Fast mode", "Saved fast-mode preference; account and model eligibility still apply.", "boolean"],
  ["promptSuggestionEnabled", "Prompt suggestions", "Generate suggested follow-up prompts.", "boolean"],
  ["fileCheckpointingEnabled", "File checkpoints", "Record file snapshots for code rewind in future sessions.", "boolean"],
  ["enableWorkflows", "Dynamic workflows", "Allow workflow orchestration when available for your account.", "boolean"],
  ["workflowKeywordTriggerEnabled", "Workflow keyword trigger", "Allow keyword-triggered workflows.", "boolean"],
  ["workflowSizeGuideline", "Workflow size", "Agent-count guideline: small aims below 5, medium below 10, large below 50; unrestricted gives no guideline. Account restrictions still apply.", "string", ["unrestricted", "small", "medium", "large"]],
  ["permissions.defaultMode", "Default permission mode", "Starting permission mode for new sessions, subject to workspace trust and account restrictions.", "string", ["default", "plan", "acceptEdits", "auto", "dontAsk", "bypassPermissions"], ["user"]],
  ["worktree.baseRef", "Worktree base ref", "Base used when creating a worktree.", "string", ["fresh", "head"]],
  ["useAutoModeDuringPlan", "Auto mode during planning", "Allow native auto mode while planning when the model, account and permission policy support it.", "boolean", undefined, ["user", "local"]],
  ["respectGitignore", "Respect .gitignore", "Hide ignored files from the file picker.", "boolean"],
  ["preferredNotifChannel", "Notification method", "Auto chooses this terminal's notification protocol, otherwise desktop alerts with a bell. A terminal-specific method falls back to desktop alerts when unsupported. Disabled silences every category.", "string", ["auto", "iterm2", "terminal_bell", "iterm2_with_bell", "kitty", "ghostty", "notifications_disabled"], ["user"]],
  ["outputStyle", "Output style", "Exact built-in or custom response-style name.", "string"],
  ["language", "Language", "Preferred response language or ISO code.", "string"],
  ["askUserQuestionTimeout", "Question timeout", "Continue with selected answers after this much idle time. The Never option waits for confirmation. Unanswered questions are skipped on timeout.", "string", ["never", "60s", "5m", "10m"], ["user"]],
  ["dialogExpiry", "Dialog expiry", "Expire forwarded remote dialogs and held messages with a safe cancellation. Local permission prompts are unaffected. The dialog timeout environment variable takes precedence.", "string", ["60s", "5m", "10m", "never"], ["user"]],
  ["crossSessionInbound", "Messages from other sessions", "Choose whether to accept, review, or refuse messages from other sessions.", "string", ["accept", "hold", "refuse"], ["user"], "Message-review settings are not available yet."],
  ["model", "Default model", "Choose a model for new sessions. Use /model to change the current session.", "string", []],
  ["agent", "Default agent", "Main-thread agent for new sessions. Use /agent to change the current session.", "string", []],
  ["prefersReducedMotion", "Reduce motion", "Show a static activity icon instead of animated spinners.", "boolean"],
  ["permissions.allow", "Permissions: allow rules", "One rule per line, e.g. Bash(npm run test *) or Read(./docs/**). Lists merge across scopes; deny rules take precedence.", "string_list"],
  ["permissions.ask", "Permissions: ask rules", "One tool permission rule per line. Matching operations require confirmation unless denied.", "string_list"],
  ["permissions.deny", "Permissions: deny rules", "One tool permission rule per line. Matching operations are denied, even if another scope allows them.", "string_list"],
  ["permissions.additionalDirectories", "Permissions: additional directories", "One directory per line to extend the working-directory permission scope. Paths retain their native settings-file meaning.", "string_list"],
  ["permissions.blockReadsOutsideWorkingDirectories", "Permissions: block outside reads", "Block Read, Grep, Glob and LSP outside working directories in every mode. On in any scope wins.", "boolean"],
  ["permissions.disableBypassPermissionsMode", "Permissions: disable bypass", "Remove the bypass permission mode. Reset removes this scope's restriction.", "string", ["disable"]],
  ["autoMemoryEnabled", "Memory: automatic memory", "Read and write automatic memory for this project.", "boolean"],
  ["autoMemoryDirectory", "Memory: directory", "Automatic-memory directory; supports ~/ expansion. Shared project settings cannot set this path.", "string", undefined, ["user", "local"]],
  ["claudeMdExcludes", "Memory: excluded instructions", "One glob or absolute path per line for CLAUDE.md or rules to omit. Managed instructions cannot be excluded.", "string_list"],
  ["plansDirectory", "Memory: plans directory", "Directory for plan files; relative paths are relative to the project root.", "string"],
  ["worktree.symlinkDirectories", "Worktree: symlink directories", "One repository directory per line to share with new worktrees, e.g. node_modules. No directories are linked by default.", "string_list"],
  ["worktree.sparsePaths", "Worktree: sparse paths", "One directory per line to include in a new worktree using Git sparse-checkout cone mode.", "string_list"],
  ["worktree.bgIsolation", "Worktree: background isolation", "Worktree requires background jobs to enter a worktree before editing. None allows edits in the main checkout.", "string", ["worktree", "none"]],
  ["disableAllHooks", "Hooks: disable all", "Disable settings and plugin hooks, including status-line execution. Built-in features keep their own controls.", "boolean"],
  ["hooks", "Hooks: definitions", "JSON object of event names and matcher groups, each with a hooks array. Actions: command, prompt, agent, http or mcp_tool. Command actions require command; prompt/agent require prompt; http requires url; mcp_tool requires server and tool. Browsing and validation do not run hooks.", "json"],
  ["allowedHttpHookUrls", "Hooks: allowed HTTP URLs", "One URL pattern per line; * is a wildcard. An empty list blocks all HTTP hooks. Reset removes this scope's allowlist; lists merge across scopes.", "string_list"],
  ["httpHookAllowedEnvVars", "Hooks: allowed environment variables", "One variable name per line. HTTP hook header interpolation is limited to these names and the hook's own allowedEnvVars. No values are read in the editor.", "string_list"],
  ["sandbox.enabled", "Sandbox: enabled", "Run shell commands in the native sandbox when supported and installed. Platform and organization restrictions still apply.", "boolean"],
  ["sandbox.failIfUnavailable", "Sandbox: require availability", "Fail startup if sandboxing is enabled but cannot start. Off permits the native warning and unsandboxed fallback.", "boolean"],
  ["sandbox.autoAllowBashIfSandboxed", "Sandbox: allow sandboxed shell", "Allow sandboxed shell operations without another permission prompt. Other permission restrictions still apply.", "boolean"],
  ["sandbox.allowUnsandboxedCommands", "Sandbox: allow unsandboxed commands", "Allow the unsandboxed-command escape parameter. Off in user or managed settings cannot be relaxed by project settings.", "boolean"],
  ["sandbox.excludedCommands", "Sandbox: excluded commands", "One shell command pattern per line to run outside the sandbox. Exclusions still require permission and cannot override enforced sandboxing.", "string_list"],
  ["sandbox.filesystem.allowRead", "Sandbox: readable paths", "One path per line to permit reading within denied regions. Native trust and organization restrictions apply.", "string_list"],
  ["sandbox.filesystem.allowWrite", "Sandbox: writable paths", "One path per line to grant additional write access. Paths retain their native settings-file meaning.", "string_list"],
  ["sandbox.filesystem.denyRead", "Sandbox: denied read paths", "One path per line to deny reading in the sandbox. Lists merge across scopes.", "string_list"],
  ["sandbox.filesystem.denyWrite", "Sandbox: denied write paths", "One path per line to deny writing in the sandbox. Lists merge across scopes.", "string_list"],
  ["sandbox.filesystem.disabled", "Sandbox: disable filesystem isolation", "Disable filesystem isolation on macOS/Linux/WSL, retaining network isolation. Ignored on native Windows; managed filesystem restrictions can prevent this.", "boolean", undefined, ["user"]],
  ["sandbox.network.allowedDomains", "Sandbox: allowed domains", "One domain or wildcard per line, e.g. *.example.com. Permissions and organization restrictions also apply.", "string_list"],
  ["sandbox.network.deniedDomains", "Sandbox: denied domains", "One domain or wildcard per line to block, even if allowed elsewhere. Lists merge across scopes.", "string_list"],
  ["sandbox.network.strictAllowlist", "Sandbox: strict domain allowlist", "Deny unlisted domains instead of prompting. Applies to sandboxed commands, not in-process WebFetch. Shared and local project settings cannot enable it.", "boolean", undefined, ["user"]],
  ["sandbox.network.allowUnixSockets", "Sandbox: Unix socket paths", "One Unix socket path per line. Path filtering is macOS-only; Linux cannot filter sockets by path.", "string_list"],
  ["sandbox.network.allowAllUnixSockets", "Sandbox: allow all Unix sockets", "Allow all Unix sockets instead of blocking them. Native platform and policy restrictions apply.", "boolean"],
  ["sandbox.network.allowLocalBinding", "Sandbox: allow local binding", "Allow sandboxed commands to bind localhost ports on macOS.", "boolean"],
  ["sandbox.network.allowMachLookup", "Sandbox: Mach services", "One macOS XPC/Mach service per line; trailing * matches a prefix. Other platforms ignore this setting.", "string_list"],
  ["sandbox.network.httpProxyPort", "Sandbox: HTTP proxy port", "Local TCP port for your HTTP proxy, instead of the native sandbox proxy. Enter a port from 1 to 65535.", "number"],
  ["sandbox.network.socksProxyPort", "Sandbox: SOCKS proxy port", "Local TCP port for your SOCKS5 proxy, instead of the native sandbox proxy. Enter a port from 1 to 65535.", "number"],
  ["sandbox.network.tlsTerminate", "Sandbox: TLS termination", "Experimental JSON object with caCertPath and caKeyPath, or {} for native certificate setup. Both configured paths must be supplied together; native Windows requires a trusted persistent CA.", "json", undefined, ["user"]],
  ["sandbox.ignoreViolations", "Sandbox: ignored violations", "JSON object mapping command patterns to arrays of filesystem paths whose violations are not reported, e.g. {\"*\":[\"/tmp\"]}.", "json"],
  ["sandbox.credentials", "Sandbox: credential protection", "Advanced JSON object: files, envVars and awsPairs. Entries configure deny or mask, extraction and host injection; native platform restrictions apply. No credentials are read or injected while editing.", "json", undefined, ["user"]],
  ["sandbox.enableWeakerNestedSandbox", "Sandbox: weaker nested isolation", "Linux-only: omit a fresh /proc mount for restricted containers. This weakens filesystem isolation.", "boolean"],
  ["sandbox.enableWeakerNetworkIsolation", "Sandbox: weaker network isolation", "macOS-only: allow trustd access for custom-CA command-line clients. This weakens network isolation.", "boolean"],
  ["sandbox.allowAppleEvents", "Sandbox: allow Apple Events", "macOS-only: permit launching or scripting applications from sandboxed commands, weakening process isolation. Project settings cannot enable it.", "boolean", undefined, ["user"]],
  ["sandbox.ripgrep", "Sandbox: ripgrep command", "JSON object with command and optional args for the native sandbox's ripgrep helper. Project settings cannot configure it.", "json", undefined, ["user"]],
];

export function settingsCatalog(models: AvailableModel[] = [], agents: AvailableAgent[] = [], defaultModel?: Json): SettingDescriptor[] {
  const catalog = DEFINITIONS.map<SettingDescriptor>(([id, label, description, kind, choices, scopes, unavailable]) => ({
    id, label, description, key_path: id.split("."), kind,
    options: kind === "boolean" ? [true, false] : choices ?? [],
    writable_scopes: unavailable ? [] : scopes ?? ALL_SCOPES,
    allows_custom: kind !== "boolean" && choices === undefined,
    reset: "Reset clears this scope's value and uses the other scopes or Default.",
    application: ["respectGitignore", "prefersReducedMotion", "autoScrollEnabled", "showTurnDuration", "showMessageTimestamps", "timeFormat", "preferredNotifChannel"].includes(id) || isAppSetting({ id }) ? "host" : "next_session",
    ...(unavailable ? { unavailable } : {}),
  }));
  for (const setting of catalog) {
    if (setting.id === "model") setting.options = models.map(model => model.id);
    if (setting.id === "agent") setting.options = agents.map(agent => agent.name);
  }
  const model = models.find(model => model.id === (defaultModel ?? "default"))
    ?? models.find(model => model.resolved_model === defaultModel);
  const canonical = canonicalModelName(model?.resolved_model ?? model?.id);
  const options = model?.supports_effort ? model.supported_effort_levels.filter(level => level !== "max") : [];
  const unavailable = !canonical ? "Choose an available default model to edit its effort."
    : options.length === 0 ? "The default model does not offer saved effort choices." : undefined;
  catalog.push({
    id: "defaultEffort", label: "Default effort",
    description: canonical ? `Saved effort for ${model?.display_name}. Use /effort for the current session; max is session-only.` : "Saved effort for the default model. Use /effort for the current session.",
    key_path: canonical ? ["modelSettings", canonical, "effortLevel"] : [], kind: "string",
    options, writable_scopes: unavailable ? [] : ALL_SCOPES, allows_custom: false,
    reset: "Reset clears this model's effort at this scope and uses the other scopes or Default.", application: "next_session",
    ...(unavailable ? { unavailable } : {}),
  });
  return catalog.sort((left, right) => left.label.localeCompare(right.label, "en", { sensitivity: "base" }));
}

/** Personal host preferences occupy these namespaces in the app document. */
export function isAppSetting(setting: Pick<SettingDescriptor, "id">): boolean {
  return setting.id.startsWith("presentation.") || setting.id.startsWith("notifications.");
}
