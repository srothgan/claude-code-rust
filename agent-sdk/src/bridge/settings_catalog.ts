import type { AvailableAgent, AvailableModel, Json, SettingDescriptor, SettingsScope } from "../types.js";
import { canonicalModelName } from "./model_metadata.js";

// This is the only catalog. Rust receives descriptors, never a second set of rules.
const ALL_SCOPES: SettingsScope[] = ["user", "project", "local"];
type Definition = [string, string, string, "boolean" | "string", string[]?, SettingsScope[]?, string?];
const DEFINITIONS: Definition[] = [
  ["autoCompactEnabled", "Auto compact", "Compact context automatically.", "boolean"],
  ["autoContinueAtUsageLimit", "Continue at usage limit", "Resume after a usage-limit pause.", "boolean", undefined, ["user"], "Automatic continuation after usage limits is not available yet."],
  ["switchModelsOnFlag", "Switch models on flagged messages", "Allow automatic alternate-model handling.", "boolean"],
  ["alwaysThinkingEnabled", "Thinking", "Thinking preference for new sessions; model restrictions still apply. Use /thinking for the current session.", "boolean"],
  ["fastMode", "Fast mode", "Saved fast-mode preference; account and model eligibility still apply.", "boolean"],
  ["promptSuggestionEnabled", "Prompt suggestions", "Generate suggested follow-up prompts.", "boolean"],
  ["fileCheckpointingEnabled", "File checkpoints", "Record file snapshots for code rewind in future sessions.", "boolean"],
  ["enableWorkflows", "Dynamic workflows", "Allow workflow orchestration when available for your account.", "boolean"],
  ["workflowKeywordTriggerEnabled", "Workflow keyword trigger", "Allow keyword-triggered workflows.", "boolean"],
  ["workflowSizeGuideline", "Workflow size", "Suggested agent count, rather than an enforced limit.", "string", ["unrestricted", "small", "medium", "large"], ["user"]],
  ["permissions.defaultMode", "Default permission mode", "Starting permission mode for new sessions, subject to workspace trust and account restrictions.", "string", ["default", "plan", "acceptEdits", "auto", "dontAsk", "bypassPermissions"], ["user"]],
  ["worktree.baseRef", "Worktree base ref", "Base used when creating a worktree.", "string", ["fresh", "head"]],
  ["useAutoModeDuringPlan", "Auto mode during planning", "Use auto mode while planning.", "boolean", undefined, ["user", "local"], "Auto mode during planning is not available yet."],
  ["respectGitignore", "Respect .gitignore", "Hide ignored files from the file picker.", "boolean"],
  ["preferredNotifChannel", "Notification method", "Choose how notifications are delivered.", "string", ["auto", "iterm2", "terminal_bell", "iterm2_with_bell", "kitty", "ghostty", "notifications_disabled"], ["user"], "Changing the notification method is not available yet."],
  ["outputStyle", "Output style", "Exact built-in or custom response-style name.", "string"],
  ["language", "Language", "Preferred response language or ISO code.", "string"],
  ["askUserQuestionTimeout", "Question timeout", "How long to wait for an answer before continuing.", "string", ["never", "60s", "5m", "10m"], ["user"], "Question timeouts are not available yet."],
  ["dialogExpiry", "Dialog expiry", "How long an approval dialog remains open.", "string", ["60s", "5m", "10m", "never"], ["user"], "Automatic approval-dialog expiry is not available yet."],
  ["crossSessionInbound", "Messages from other sessions", "Choose whether to accept, review, or refuse messages from other sessions.", "string", ["accept", "hold", "refuse"], ["user"], "Message-review settings are not available yet."],
  ["model", "Default model", "Choose a model for new sessions. Use /model to change the current session.", "string", []],
  ["agent", "Default agent", "Main-thread agent for new sessions. Use /agent to change the current session.", "string", []],
  ["prefersReducedMotion", "Reduce motion", "Show a static activity icon instead of animated spinners.", "boolean"],
];

export function settingsCatalog(models: AvailableModel[] = [], agents: AvailableAgent[] = [], defaultModel?: Json): SettingDescriptor[] {
  const catalog = DEFINITIONS.map<SettingDescriptor>(([id, label, description, kind, choices, scopes, unavailable]) => ({
    id, label, description, key_path: id.split("."), kind,
    options: kind === "boolean" ? [true, false] : choices ?? [],
    writable_scopes: unavailable ? [] : scopes ?? ALL_SCOPES,
    allows_custom: kind === "string" && choices === undefined,
    reset: "Reset clears this scope's value and uses the other scopes or Default.",
    application: id === "respectGitignore" || id === "prefersReducedMotion" ? "host" : "next_session",
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
