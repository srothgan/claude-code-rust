import type { PermissionMode } from "@anthropic-ai/claude-agent-sdk";
import { isEffortLevel } from "./effort.js";
import type {
  BridgeCommand,
  BridgeCommandEnvelope,
  ElicitationAction,
  EffortLevel,
  Json,
  ModeInfo,
  ModeState,
  PermissionOutcome,
  QuestionOutcome,
  RefusalFallbackPromptChoice,
  RewindRestoreMode,
  SessionLaunchSettings,
  SettingsMutation,
  UserDialogOutcome,
} from "../types.js";
import { parseMcpServersRecord } from "./mcp_metadata.js";
import type { SessionState } from "./session_lifecycle.js";
import { resolveCurrentModel } from "./session_lifecycle.js";
import { SessionObservations } from "./session_observations.js";

const modeObservations = new SessionObservations();

export function beginSessionModeRead(session: SessionState): () => boolean {
  return modeObservations.begin(session);
}

export function observeSessionMode(session: SessionState, value: unknown): boolean {
  const mode = typeof value === "string" ? toPermissionMode(value) : null;
  if (!mode) return false;
  modeObservations.begin(session);
  session.mode = mode;
  return true;
}

const MODE_NAMES: Record<PermissionMode, string> = {
  default: "Default",
  auto: "Auto",
  acceptEdits: "Accept Edits",
  bypassPermissions: "Bypass Permissions",
  plan: "Plan",
  dontAsk: "Don't Ask",
};

const MODE_OPTIONS: ModeInfo[] = [
  { id: "default", name: "Default", description: "Standard permission flow" },
  {
    id: "auto",
    name: "Auto",
    description: "Model-classified permission approvals",
  },
  {
    id: "acceptEdits",
    name: "Accept Edits",
    description: "Auto-approve edit operations",
  },
  { id: "plan", name: "Plan", description: "No tool execution" },
  {
    id: "dontAsk",
    name: "Don't Ask",
    description: "Reject non-approved tools",
  },
  {
    id: "bypassPermissions",
    name: "Bypass Permissions",
    description: "Auto-approve all tools",
  },
];

const BASE_SUPPORTED_MODE_IDS: PermissionMode[] = [
  "default",
  "acceptEdits",
  "plan",
  "dontAsk",
];

function currentModelSupportsAutoMode(session: SessionState): boolean {
  const currentModel = session.currentModel ?? resolveCurrentModel(session);
  return currentModel.supports_auto_mode === true;
}

function modeInfoForId(mode: PermissionMode): ModeInfo {
  return (
    MODE_OPTIONS.find((entry) => entry.id === mode) ?? {
      id: mode,
      name: MODE_NAMES[mode],
    }
  );
}

function uniqueModeIds(modeIds: PermissionMode[]): PermissionMode[] {
  const unique = new Set(modeIds);
  const ordered = MODE_OPTIONS.map(
    (entry) => entry.id as PermissionMode,
  ).filter((mode) => unique.has(mode));
  const extras = Array.from(unique).filter((mode) => !ordered.includes(mode));
  return [...ordered, ...extras];
}

function computedSupportedModeIds(session: SessionState): PermissionMode[] {
  const supported = [...BASE_SUPPORTED_MODE_IDS];
  if (currentModelSupportsAutoMode(session)) {
    supported.push("auto");
  }
  if (session.supportsBypassPermissionsMode) {
    supported.push("bypassPermissions");
  }
  if (session.mode) {
    supported.push(session.mode);
  }
  return uniqueModeIds(supported);
}

export function refreshSupportedModesForSession(session: SessionState): void {
  const computed = computedSupportedModeIds(session);
  session.supportedModeIds = computed.filter(
    (mode) =>
      mode === session.mode ||
      !session.runtimeUnavailableModeIds.includes(mode),
  );
}

export function markModeUnavailableForSession(
  session: SessionState,
  mode: PermissionMode,
): boolean {
  if (session.runtimeUnavailableModeIds.includes(mode)) {
    return false;
  }
  session.runtimeUnavailableModeIds = [
    ...session.runtimeUnavailableModeIds,
    mode,
  ];
  refreshSupportedModesForSession(session);
  return true;
}

export function permissionModeFailureLooksUnsupported(
  mode: PermissionMode,
  message: string,
): boolean {
  const normalizedMessage = message.toLowerCase();
  return (
    normalizedMessage.includes("cannot set permission mode to") &&
    normalizedMessage.includes(mode.toLowerCase())
  );
}

export function availableModesForSession(session: SessionState): ModeInfo[] {
  return session.supportedModeIds.map(modeInfoForId);
}

function asRecord(value: unknown, context: string): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${context} must be an object`);
  }
  return value as Record<string, unknown>;
}

function expectString(
  record: Record<string, unknown>,
  key: string,
  context: string,
): string {
  const value = record[key];
  if (typeof value !== "string") {
    throw new Error(`${context}.${key} must be a string`);
  }
  return value;
}

function expectEffortLevel(
  record: Record<string, unknown>,
  key: string,
  context: string,
): EffortLevel {
  const value = expectString(record, key, context);
  if (!isEffortLevel(value)) {
    throw new Error(
      `${context}.${key} must be one of low, medium, high, xhigh, max`,
    );
  }
  return value;
}

function expectRewindRestoreMode(
  record: Record<string, unknown>,
  key: string,
  context: string,
): RewindRestoreMode {
  const value = expectString(record, key, context);
  if (value === "both" || value === "conversation" || value === "code") {
    return value;
  }
  throw new Error(`${context}.${key} must be one of both, conversation, code`);
}

function expectNonEmptyStringOrNull(
  record: Record<string, unknown>,
  key: string,
  context: string,
): string | null {
  const value = record[key];
  if (value === null) {
    return null;
  }
  if (typeof value !== "string" || value.trim().length === 0) {
    throw new Error(`${context}.${key} must be a non-empty string or null`);
  }
  return value;
}

function optionalString(
  record: Record<string, unknown>,
  key: string,
  context: string,
): string | undefined {
  const value = record[key];
  if (value === undefined || value === null) {
    return undefined;
  }
  if (typeof value !== "string") {
    throw new Error(`${context}.${key} must be a string when provided`);
  }
  return value;
}

function optionalMetadata(
  record: Record<string, unknown>,
  key: string,
): Record<string, Json> {
  const value = record[key];
  if (value === undefined || value === null) {
    return {};
  }
  return asRecord(value, `${key} metadata`) as Record<string, Json>;
}

function optionalLaunchSettings(
  record: Record<string, unknown>,
  key: string,
  context: string,
): SessionLaunchSettings {
  const value = record[key];
  if (value === undefined || value === null) {
    return {};
  }
  const parsed = asRecord(value, `${context}.${key}`);
  const model = optionalString(parsed, "model", `${context}.${key}`);
  const permissionMode = optionalString(parsed, "permission_mode", `${context}.${key}`);
  if (permissionMode !== undefined && !Object.hasOwn(MODE_NAMES, permissionMode)) {
    throw new Error(`${context}.${key}.permission_mode is unsupported: ${permissionMode}`);
  }
  const agent = optionalString(parsed, "agent", `${context}.${key}`);
  const effort = parsed.effort == null ? undefined : expectEffortLevel(parsed, "effort", `${context}.${key}`);
  return {
    ...(model ? { model } : {}),
    ...(permissionMode ? { permission_mode: permissionMode as SessionLaunchSettings["permission_mode"] } : {}),
    ...(agent ? { agent } : {}),
    ...(effort !== undefined ? { effort } : {}),
  };
}

function optionalBoolean(
  record: Record<string, unknown>,
  key: string,
  context: string,
): boolean | undefined {
  const value = record[key];
  if (value === undefined || value === null) {
    return undefined;
  }
  if (typeof value !== "boolean") {
    throw new Error(`${context}.${key} must be a boolean when provided`);
  }
  return value;
}

function optionalJsonObject(
  record: Record<string, unknown>,
  key: string,
  context: string,
): { [key: string]: Json } | undefined {
  const value = record[key];
  if (value === undefined || value === null) {
    return undefined;
  }
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${context}.${key} must be an object when provided`);
  }
  return value as { [key: string]: Json };
}

function expectElicitationAction(
  record: Record<string, unknown>,
  key: string,
  context: string,
): ElicitationAction {
  const value = expectString(record, key, context);
  if (value === "accept" || value === "decline" || value === "cancel") {
    return value;
  }
  throw new Error(`${context}.${key} must be one of accept, decline, cancel`);
}

function parsePromptChunks(
  record: Record<string, unknown>,
  context: string,
): Array<{ kind: string; value: Json }> {
  const rawChunks = record.chunks;
  if (!Array.isArray(rawChunks)) {
    throw new Error(`${context}.chunks must be an array`);
  }
  return rawChunks.map((chunk, index) => {
    const parsed = asRecord(chunk, `${context}.chunks[${index}]`);
    const kind = expectString(parsed, "kind", `${context}.chunks[${index}]`);
    return { kind, value: (parsed.value ?? null) as Json };
  });
}

function expectBoolean(
  record: Record<string, unknown>,
  key: string,
  context: string,
): boolean {
  const value = record[key];
  if (typeof value !== "boolean") {
    throw new Error(`${context}.${key} must be a boolean`);
  }
  return value;
}

function expectRefusalFallbackPromptChoice(
  record: Record<string, unknown>,
  key: string,
  context: string,
): RefusalFallbackPromptChoice {
  const value = expectString(record, key, context);
  if (value !== "retry_fallback" && value !== "edit_prompt") {
    throw new Error(
      `${context}.${key} must be 'retry_fallback' or 'edit_prompt'`,
    );
  }
  return value;
}

export function parseCommandEnvelope(line: string): {
  requestId?: string;
  command: BridgeCommand;
} {
  const raw = asRecord(
    JSON.parse(line) as BridgeCommandEnvelope,
    "command envelope",
  );
  const requestId =
    typeof raw.request_id === "string" ? raw.request_id : undefined;
  const commandName = expectString(raw, "command", "command envelope");

  const command: BridgeCommand = (() => {
    switch (commandName) {
      case "inspect_settings":
        return { command: "inspect_settings", session_id: expectString(raw, "session_id", commandName) };
      case "mutate_setting": {
        const input = asRecord(raw.mutation, "mutate_setting.mutation");
        const scope = expectString(input, "scope", commandName);
        const operation = expectString(input, "operation", commandName);
        if (!["user", "project", "local"].includes(scope) || !["set", "remove"].includes(operation)) throw new Error("Invalid settings scope or operation.");
        const mutation: SettingsMutation = {
          context: expectString(input, "context", commandName), id: expectString(input, "id", commandName),
          scope: scope as SettingsMutation["scope"], operation: operation as SettingsMutation["operation"],
          expected_revision: expectString(input, "expected_revision", commandName),
          ...(operation === "set" ? { value: input.value as Json } : {}),
        };
        return { command: "mutate_setting", session_id: expectString(raw, "session_id", commandName), mutation };
      }
      case "initialize":
        return {
          command: "initialize",
          cwd: expectString(raw, "cwd", "initialize"),
          metadata: optionalMetadata(raw, "metadata"),
        };
      case "create_session":
        return {
          command: "create_session",
          cwd: expectString(raw, "cwd", "create_session"),
          resume: optionalString(raw, "resume", "create_session"),
          continue_session: optionalBoolean(raw, "continue_session", "create_session"),
          launch_settings: optionalLaunchSettings(
            raw,
            "launch_settings",
            "create_session",
          ),
          metadata: optionalMetadata(raw, "metadata"),
        };
      case "resume_session":
        return {
          command: "resume_session",
          session_id: expectString(raw, "session_id", "resume_session"),
          launch_settings: optionalLaunchSettings(
            raw,
            "launch_settings",
            "resume_session",
          ),
          metadata: optionalMetadata(raw, "metadata"),
        };
      case "resume_session_at":
        return {
          command: "resume_session_at",
          session_id: expectString(raw, "session_id", "resume_session_at"),
          target_user_message_id: expectString(
            raw,
            "target_user_message_id",
            "resume_session_at",
          ),
          launch_settings: optionalLaunchSettings(
            raw,
            "launch_settings",
            "resume_session_at",
          ),
        };
      case "new_session":
        return {
          command: "new_session",
          cwd: expectString(raw, "cwd", "new_session"),
          launch_settings: optionalLaunchSettings(
            raw,
            "launch_settings",
            "new_session",
          ),
        };
      case "prompt":
        if (
          raw.inline_pastes !== undefined &&
          (!Array.isArray(raw.inline_pastes) ||
            !raw.inline_pastes.every((entry) => typeof entry === "string"))
        ) {
          throw new Error("prompt.inline_pastes must be an array of strings");
        }
        return {
          command: "prompt",
          session_id: expectString(raw, "session_id", "prompt"),
          message_uuid: expectString(raw, "message_uuid", "prompt"),
          chunks: parsePromptChunks(raw, "prompt"),
          ...(Array.isArray(raw.inline_pastes)
            ? { inline_pastes: raw.inline_pastes as string[] }
            : {}),
        };
      case "side_question": {
        const question = expectString(raw, "question", "side_question");
        if (question.trim().length === 0) {
          throw new Error("side_question.question must not be empty");
        }
        return {
          command: "side_question",
          session_id: expectString(raw, "session_id", "side_question"),
          btw_id: expectString(raw, "btw_id", "side_question"),
          question,
        };
      }
      case "cancel_turn":
        return {
          command: "cancel_turn",
          session_id: expectString(raw, "session_id", "cancel_turn"),
        };
      case "set_model":
        return {
          command: "set_model",
          session_id: expectString(raw, "session_id", "set_model"),
          model: expectString(raw, "model", "set_model"),
        };
      case "set_mode":
        return {
          command: "set_mode",
          session_id: expectString(raw, "session_id", "set_mode"),
          mode: expectString(raw, "mode", "set_mode"),
        };
      case "set_effort":
        return {
          command: "set_effort",
          session_id: expectString(raw, "session_id", "set_effort"),
          effort: raw.effort === null ? null : expectEffortLevel(raw, "effort", "set_effort"),
        };
      case "set_thinking":
        return {
          command: "set_thinking",
          session_id: expectString(raw, "session_id", "set_thinking"),
          enabled: raw.enabled === null ? null : expectBoolean(raw, "enabled", "set_thinking"),
        };
      case "set_agent":
        return {
          command: "set_agent",
          session_id: expectString(raw, "session_id", "set_agent"),
          agent: expectNonEmptyStringOrNull(raw, "agent", "set_agent"),
        };
      case "set_ultracode":
        return {
          command: "set_ultracode",
          session_id: expectString(raw, "session_id", "set_ultracode"),
          enabled: expectBoolean(raw, "enabled", "set_ultracode"),
        };
      case "refresh_ultracode":
        return {
          command: "refresh_ultracode",
          session_id: expectString(raw, "session_id", "refresh_ultracode"),
        };
      case "set_fast_mode":
        return {
          command: "set_fast_mode",
          session_id: expectString(raw, "session_id", "set_fast_mode"),
          enabled: expectBoolean(raw, "enabled", "set_fast_mode"),
        };
      case "generate_session_title":
        return {
          command: "generate_session_title",
          session_id: expectString(raw, "session_id", "generate_session_title"),
          description: expectString(
            raw,
            "description",
            "generate_session_title",
          ),
        };
      case "rename_session":
        return {
          command: "rename_session",
          session_id: expectString(raw, "session_id", "rename_session"),
          title: expectString(raw, "title", "rename_session"),
        };
      case "get_status_snapshot":
        return {
          command: "get_status_snapshot",
          session_id: expectString(raw, "session_id", "get_status_snapshot"),
        };
      case "get_context_usage":
        return {
          command: "get_context_usage",
          session_id: expectString(raw, "session_id", "get_context_usage"),
        };
      case "get_usage":
        return {
          command: "get_usage",
          session_id: expectString(raw, "session_id", "get_usage"),
        };
      case "get_rewind_targets":
        return {
          command: "get_rewind_targets",
          session_id: expectString(raw, "session_id", "get_rewind_targets"),
        };
      case "rewind":
        return {
          command: "rewind",
          session_id: expectString(raw, "session_id", "rewind"),
          target_user_message_id: expectString(
            raw,
            "target_user_message_id",
            "rewind",
          ),
          restore_mode: expectRewindRestoreMode(raw, "restore_mode", "rewind"),
          launch_settings: optionalLaunchSettings(
            raw,
            "launch_settings",
            "rewind",
          ),
        };
      case "reload_plugins":
        return {
          command: "reload_plugins",
          session_id: expectString(raw, "session_id", "reload_plugins"),
          ...(optionalBoolean(raw, "force", "reload_plugins") !== undefined
            ? { force: optionalBoolean(raw, "force", "reload_plugins") }
            : {}),
        };
      case "mcp_status":
      // Rust historically sends `get_mcp_snapshot`; normalize it to the bridge-internal command.
      case "get_mcp_snapshot":
        return {
          command: "mcp_status",
          session_id: expectString(raw, "session_id", commandName),
        };
      case "mcp_reconnect":
        return {
          command: "mcp_reconnect",
          session_id: expectString(raw, "session_id", "mcp_reconnect"),
          server_name: expectString(raw, "server_name", "mcp_reconnect"),
        };
      case "mcp_toggle":
        return {
          command: "mcp_toggle",
          session_id: expectString(raw, "session_id", "mcp_toggle"),
          server_name: expectString(raw, "server_name", "mcp_toggle"),
          enabled: expectBoolean(raw, "enabled", "mcp_toggle"),
        };
      case "mcp_set_servers":
        return {
          command: "mcp_set_servers",
          session_id: expectString(raw, "session_id", "mcp_set_servers"),
          servers: parseMcpServersRecord(
            raw.servers ?? {},
            "mcp_set_servers.servers",
          ),
        };
      case "permission_response": {
        const outcome = asRecord(raw.outcome, "permission_response.outcome");
        const outcomeType = expectString(
          outcome,
          "outcome",
          "permission_response.outcome",
        );
        if (outcomeType !== "selected" && outcomeType !== "cancelled") {
          throw new Error(
            "permission_response.outcome.outcome must be 'selected' or 'cancelled'",
          );
        }
        const parsedOutcome: PermissionOutcome =
          outcomeType === "selected"
            ? {
                outcome: "selected",
                option_id: expectString(
                  outcome,
                  "option_id",
                  "permission_response.outcome",
                ),
              }
            : { outcome: "cancelled" };
        return {
          command: "permission_response",
          session_id: expectString(raw, "session_id", "permission_response"),
          tool_call_id: expectString(
            raw,
            "tool_call_id",
            "permission_response",
          ),
          outcome: parsedOutcome,
        };
      }
      case "question_response": {
        const outcome = asRecord(raw.outcome, "question_response.outcome");
        const outcomeType = expectString(
          outcome,
          "outcome",
          "question_response.outcome",
        );
        if (outcomeType !== "answered" && outcomeType !== "cancelled") {
          throw new Error(
            "question_response.outcome.outcome must be 'answered' or 'cancelled'",
          );
        }
        const parsedOutcome: QuestionOutcome =
          outcomeType === "answered"
            ? {
                outcome: "answered",
                selected_option_ids: expectStringArray(
                  outcome,
                  "selected_option_ids",
                  "question_response.outcome",
                ),
                ...(outcome.annotation === undefined ||
                outcome.annotation === null
                  ? {}
                  : {
                      annotation: parseQuestionAnnotation(outcome.annotation),
                    }),
              }
            : { outcome: "cancelled" };
        return {
          command: "question_response",
          session_id: expectString(raw, "session_id", "question_response"),
          tool_call_id: expectString(raw, "tool_call_id", "question_response"),
          outcome: parsedOutcome,
        };
      }
      case "user_dialog_response": {
        const outcome = asRecord(raw.outcome, "user_dialog_response.outcome");
        const outcomeType = expectString(
          outcome,
          "outcome",
          "user_dialog_response.outcome",
        );
        if (outcomeType !== "selected" && outcomeType !== "cancelled") {
          throw new Error(
            "user_dialog_response.outcome.outcome must be 'selected' or 'cancelled'",
          );
        }
        const parsedOutcome: UserDialogOutcome =
          outcomeType === "selected"
            ? {
                outcome: "selected",
                option_id: expectRefusalFallbackPromptChoice(
                  outcome,
                  "option_id",
                  "user_dialog_response.outcome",
                ),
              }
            : { outcome: "cancelled" };
        return {
          command: "user_dialog_response",
          session_id: expectString(raw, "session_id", "user_dialog_response"),
          request_id: expectString(raw, "request_id", "user_dialog_response"),
          outcome: parsedOutcome,
        };
      }
      case "elicitation_response":
        return {
          command: "elicitation_response",
          session_id: expectString(raw, "session_id", "elicitation_response"),
          elicitation_request_id: expectString(
            raw,
            "elicitation_request_id",
            "elicitation_response",
          ),
          action: expectElicitationAction(
            raw,
            "action",
            "elicitation_response",
          ),
          ...(optionalJsonObject(raw, "content", "elicitation_response")
            ? {
                content: optionalJsonObject(
                  raw,
                  "content",
                  "elicitation_response",
                ),
              }
            : {}),
        };
      case "mcp_authenticate":
        return {
          command: "mcp_authenticate",
          session_id: expectString(raw, "session_id", "mcp_authenticate"),
          server_name: expectString(raw, "server_name", "mcp_authenticate"),
        };
      case "mcp_clear_auth":
        return {
          command: "mcp_clear_auth",
          session_id: expectString(raw, "session_id", "mcp_clear_auth"),
          server_name: expectString(raw, "server_name", "mcp_clear_auth"),
        };
      case "mcp_oauth_callback_url":
        return {
          command: "mcp_oauth_callback_url",
          session_id: expectString(raw, "session_id", "mcp_oauth_callback_url"),
          server_name: expectString(
            raw,
            "server_name",
            "mcp_oauth_callback_url",
          ),
          callback_url: expectString(
            raw,
            "callback_url",
            "mcp_oauth_callback_url",
          ),
        };
      case "shutdown":
        return { command: "shutdown" };
      default:
        throw new Error(`unsupported command: ${commandName}`);
    }
  })();

  return { requestId, command };
}

function expectStringArray(
  record: Record<string, unknown>,
  key: string,
  context: string,
): string[] {
  const value = record[key];
  if (!Array.isArray(value)) {
    throw new Error(`${context}.${key} must be an array`);
  }
  return value.map((entry, index) => {
    if (typeof entry !== "string") {
      throw new Error(`${context}.${key}[${index}] must be a string`);
    }
    return entry;
  });
}

function parseQuestionAnnotation(value: unknown): {
  preview?: string;
  notes?: string;
} {
  const record = asRecord(value, "question_response.outcome.annotation");
  const preview = optionalString(
    record,
    "preview",
    "question_response.outcome.annotation",
  );
  const notes = optionalString(
    record,
    "notes",
    "question_response.outcome.annotation",
  );
  return {
    ...(preview !== undefined ? { preview } : {}),
    ...(notes !== undefined ? { notes } : {}),
  };
}

export function toPermissionMode(mode: string): PermissionMode | null {
  if (
    mode === "default" ||
    mode === "auto" ||
    mode === "acceptEdits" ||
    mode === "bypassPermissions" ||
    mode === "plan" ||
    mode === "dontAsk"
  ) {
    return mode;
  }
  return null;
}

export function buildModeState(
  session: SessionState,
  mode: PermissionMode | null,
): ModeState {
  return {
    current_mode_id: mode ?? "unknown",
    current_mode_name: mode ? MODE_NAMES[mode] : "Unknown",
    available_modes: availableModesForSession(session),
  };
}
