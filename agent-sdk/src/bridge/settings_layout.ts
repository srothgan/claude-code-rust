import { HOOK_EVENTS } from "@anthropic-ai/claude-agent-sdk";
import type { SettingDescriptor, SettingsCategory, SettingsEditorSchema as Schema } from "../types.js";

export const SETTINGS_CATEGORIES: SettingsCategory[] = [
  { id: "general", label: "General", short_label: "General" },
  { id: "memory", label: "Memory", short_label: "Memory" },
  { id: "permissions", label: "Permissions", short_label: "Permissions" },
  { id: "sandbox", label: "Sandbox", short_label: "Sandbox" },
  { id: "hooks", label: "Hooks", short_label: "Hooks" },
  { id: "workflows", label: "Workflows & Worktrees", short_label: "Workflows" },
];

// Explicit assignments live beside the catalog, never in the renderer.
const CATEGORY_SETTINGS: Record<string, string[]> = {
  "general": [
    "model",
    "defaultEffort",
    "agent",
    "alwaysThinkingEnabled",
    "fastMode",
    "switchModelsOnFlag",
    "outputStyle",
    "language",
    "promptSuggestionEnabled",
    "askUserQuestionTimeout",
    "dialogExpiry",
    "crossSessionInbound",
    "autoContinueAtUsageLimit",
    "autoScrollEnabled",
    "showTurnDuration",
    "showMessageTimestamps",
    "timeFormat",
    "spinnerTipsEnabled",
    "prefersReducedMotion",
    "presentation.showStatusInTerminalTab",
    "presentation.copyFullResponse",
    "respectGitignore",
    "preferredNotifChannel",
    "notifications.actionsRequired",
    "notifications.turnComplete",
    "notifications.modelDirected",
    "updates.autoInstall"
  ],
  "memory": [
    "autoCompactEnabled",
    "fileCheckpointingEnabled",
    "autoMemoryEnabled",
    "autoMemoryDirectory",
    "claudeMdExcludes",
    "plansDirectory"
  ],
  "permissions": [
    "permissions.defaultMode",
    "useAutoModeDuringPlan",
    "permissions.disableBypassPermissionsMode",
    "permissions.blockReadsOutsideWorkingDirectories",
    "permissions.allow",
    "permissions.ask",
    "permissions.deny",
    "permissions.additionalDirectories"
  ],
  "sandbox": [
    "sandbox.enabled",
    "sandbox.failIfUnavailable",
    "sandbox.autoAllowBashIfSandboxed",
    "sandbox.allowUnsandboxedCommands",
    "sandbox.excludedCommands",
    "sandbox.filesystem.allowRead",
    "sandbox.filesystem.allowWrite",
    "sandbox.filesystem.denyRead",
    "sandbox.filesystem.denyWrite",
    "sandbox.filesystem.disabled",
    "sandbox.network.allowedDomains",
    "sandbox.network.deniedDomains",
    "sandbox.network.strictAllowlist",
    "sandbox.network.allowUnixSockets",
    "sandbox.network.allowAllUnixSockets",
    "sandbox.network.allowLocalBinding",
    "sandbox.network.allowMachLookup",
    "sandbox.network.httpProxyPort",
    "sandbox.network.socksProxyPort",
    "sandbox.credentials",
    "sandbox.network.tlsTerminate",
    "sandbox.ignoreViolations",
    "sandbox.ripgrep",
    "sandbox.enableWeakerNestedSandbox",
    "sandbox.enableWeakerNetworkIsolation",
    "sandbox.allowAppleEvents"
  ],
  "hooks": [
    "disableAllHooks",
    "allowedHttpHookUrls",
    "httpHookAllowedEnvVars",
    "hooks"
  ],
  "workflows": [
    "enableWorkflows",
    "workflowKeywordTriggerEnabled",
    "workflowSizeGuideline",
    "worktree.baseRef",
    "worktree.symlinkDirectories",
    "worktree.sparsePaths",
    "worktree.bgIsolation"
  ]
};

const text: Schema = { type: "string" };
const boolean: Schema = { type: "boolean" };
const number: Schema = { type: "number" };
const list = (item: Schema): Schema => ({ type: "array", item });
const map = (item: Schema, keys?: string[]): Schema => ({ type: "map", item, ...(keys ? { keys } : {}) });
const choice = (...options: string[]): Schema => ({ type: "string", options });
const help = (schema: Schema, description: string): Schema => ({ ...schema, description });
const path = help(text, "Enter a filesystem path. Paths keep their native settings-file meaning; the editor does not open the file.");
const envName = help(text, "Enter an environment variable name, for example API_TOKEN. Enter the name, not its secret value.");
const rule = help(text, "Enter one tool permission rule, for example Read(./src/**) or Bash(npm test *). Use a separate entry for each rule.");
const argument = help(text, "Enter one literal command argument. Add a separate entry for each argument; do not enter a shell-quoted argument list.");
const FIELD_LABELS: Record<string, string> = {
  args: "Arguments", if: "Tool rule filter", statusMessage: "Status message", async: "Run in background", asyncRewake: "Wake model on blocking exit", once: "Run once", continueOnBlock: "Continue after a block", timeout: "Timeout (seconds)", mode: "Protection mode", files: "Credential files", path: "File path", name: "Variable name", streaming: "Streaming requests", presigned: "Presigned requests",
  hooks: "Handlers", caCertPath: "CA certificate path", caKeyPath: "CA private key path", envVars: "Environment variables", awsPairs: "AWS credential pairs", sigv4: "AWS request policies",
  extract: "Extraction pattern", onExtractNoMatch: "If extraction finds nothing", decode: "Encoded credential format", maskClaims: "JWT claims to mask", maskDuplicates: "Mask repeated values", injectHosts: "Injection hosts", allowPlaintextInject: "Allow plain HTTP injection",
  accessKeyIdVar: "Access key variable", secretAccessKeyVar: "Secret key variable", sessionTokenVar: "Session token variable", allowedEnvVars: "Allowed environment variables", input: "Tool arguments",
};
const fields = (entries: Record<string, Schema>, required: string[] = []): Schema => ({ type: "object", fields: Object.entries(entries).map(([key, schema]) => ({ key, label: FIELD_LABELS[key] ?? key.replace(/^./, letter => letter.toUpperCase()), schema, ...(required.includes(key) ? { required: true } : {}) })) });

// Affordances for the pinned public Settings contract. SDK validation stays authoritative.
const hookCommon = { if: rule, timeout: help(number, "Enter a timeout in seconds."), statusMessage: text, once: boolean };
const hook: Schema = { type: "variant", options: ["command", "prompt", "agent", "http", "mcp_tool"], variants: {
  command: fields({ command: help(text, "Enter the command to run, for example npm run lint. Editing does not execute it."), args: list(argument), shell: choice("bash", "powershell"), ...hookCommon, async: boolean, asyncRewake: boolean }, ["command"]),
  prompt: fields({ prompt: help(text, "Enter the hook's evaluation prompt. Ctrl+J inserts a new line."), ...hookCommon, model: text, continueOnBlock: boolean }, ["prompt"]),
  agent: fields({ prompt: help(text, "Enter the task for the hook agent. Ctrl+J inserts a new line."), ...hookCommon, model: text }, ["prompt"]),
  http: fields({ url: help(text, "Enter the hook endpoint URL, for example https://example.com/hook."), ...hookCommon, headers: help(map(text), "Add a header name, then its value. Values appear only while editing that header."), allowedEnvVars: list(envName) }, ["url"]),
  mcp_tool: fields({ server: help(text, "Enter the exact name of an already configured MCP server."), tool: help(text, "Enter the exact tool name exposed by that server."), input: help(map({ type: "json" }), "Add an argument name, then enter its JSON value."), ...hookCommon }, ["server", "tool"]),
} };
const mask = { mode: help(choice("deny", "mask"), "deny blocks access to the credential; mask substitutes a sentinel inside the sandbox."), extract: help(text, "Optional regular expression for mask mode: capture group 1 contains the credential."), onExtractNoMatch: help(choice("warn", "deny", "error"), "When extraction finds nothing: warn leaves it readable, deny blocks access, error stops sandbox setup. Native restrictions apply for JWT decoding."), decode: help(choice("jwt"), "Mask a JWT with a structurally valid substitute. Reset this field to use ordinary masking."), maskClaims: list(text), injectHosts: help(list(text), "Add one allowed injection host per entry, for example api.example.com." ) };
const OBJECT_EDITORS: Record<string, Schema> = {
  hooks: map(list(fields({ matcher: help(text, "Optional event matcher. For tool events use a tool name or pattern, for example Bash or Write|Edit. Leave blank for all matching events. Editing a group's matcher affects every hook in that group."), hooks: list(hook) })), [...HOOK_EVENTS]),
  "sandbox.network.tlsTerminate": help(fields({ caCertPath: path, caKeyPath: path }), "Leave both paths unset for native certificate setup, or supply both certificate and private key paths."),
  "sandbox.ripgrep": help(fields({ command: help(text, "Enter the ripgrep executable name or path, for example rg."), args: list(argument) }, ["command"]), "Set the required command; arguments are optional and edited one at a time."),
  "sandbox.ignoreViolations": help(map(help(list(path), "Add one filesystem path per entry for this command pattern.")), "Add a command pattern, for example git * or *, then add the paths whose violations should not be reported."),
  "sandbox.credentials": help(fields({
    files: help(list(fields({ path, ...mask, maskDuplicates: boolean }, ["path", "mode"])), "Add a credential file, then set its required path and protection mode. Optional fields control extraction and injection."),
    envVars: help(list(fields({ name: envName, ...mask }, ["name", "mode"])), "Add an environment variable, then set its required name and protection mode. No secret values are read while editing."),
    awsPairs: help(list(fields({ accessKeyIdVar: envName, secretAccessKeyVar: envName, sessionTokenVar: envName }, ["accessKeyIdVar", "secretAccessKeyVar"])), "Add a pair and enter the required access-key and secret-key variable names. The session-token variable is optional."),
    allowPlaintextInject: boolean,
    sigv4: fields({ streaming: choice("deny", "passthrough"), presigned: choice("deny", "passthrough"), sigv4a: choice("deny", "passthrough") }),
  }), "Choose a collection to add credential protection, or edit the optional request policies. Required fields are marked; save after completing them."),
};

export function settingsLayout(id: string, kind: SettingDescriptor["kind"], description?: string): Pick<SettingDescriptor, "category" | "editor"> {
  const category = Object.entries(CATEGORY_SETTINGS).find(([, ids]) => ids.includes(id))?.[0];
  if (!category) throw new Error(`Setting has no category: ${id}`);
  return {
    category,
    ...(kind === "string_list" ? { editor: list(id === "permissions.allow" || id === "permissions.ask" || id === "permissions.deny" ? rule : id === "httpHookAllowedEnvVars" ? envName : { ...text, description }) } : kind === "json" ? { editor: OBJECT_EDITORS[id] } : {}),
  };
}
