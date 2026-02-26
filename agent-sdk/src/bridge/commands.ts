import type { PermissionMode } from "@anthropic-ai/claude-agent-sdk";
import type {
  BridgeCommand,
  BridgeCommandEnvelope,
  Json,
  ModeInfo,
  ModeState,
  PermissionOutcome,
} from "../types.js";

const MODE_NAMES: Record<PermissionMode, string> = {
  default: "Default",
  acceptEdits: "Accept Edits",
  bypassPermissions: "Bypass Permissions",
  plan: "Plan",
  dontAsk: "Don't Ask",
};

const MODE_OPTIONS: ModeInfo[] = [
  { id: "default", name: "Default", description: "Standard permission flow" },
  { id: "acceptEdits", name: "Accept Edits", description: "Auto-approve edit operations" },
  { id: "plan", name: "Plan", description: "No tool execution" },
  { id: "dontAsk", name: "Don't Ask", description: "Reject non-approved tools" },
  { id: "bypassPermissions", name: "Bypass Permissions", description: "Auto-approve all tools" },
];

function asRecord(value: unknown, context: string): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${context} must be an object`);
  }
  return value as Record<string, unknown>;
}

function expectString(record: Record<string, unknown>, key: string, context: string): string {
  const value = record[key];
  if (typeof value !== "string") {
    throw new Error(`${context}.${key} must be a string`);
  }
  return value;
}

function expectBoolean(record: Record<string, unknown>, key: string, context: string): boolean {
  const value = record[key];
  if (typeof value !== "boolean") {
    throw new Error(`${context}.${key} must be a boolean`);
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

function optionalMetadata(record: Record<string, unknown>, key: string): Record<string, Json> {
  const value = record[key];
  if (value === undefined || value === null) {
    return {};
  }
  return asRecord(value, `${key} metadata`) as Record<string, Json>;
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

export function parseCommandEnvelope(line: string): { requestId?: string; command: BridgeCommand } {
  const raw = asRecord(JSON.parse(line) as BridgeCommandEnvelope, "command envelope");
  const requestId = typeof raw.request_id === "string" ? raw.request_id : undefined;
  const commandName = expectString(raw, "command", "command envelope");

  const command: BridgeCommand = (() => {
    switch (commandName) {
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
          yolo: expectBoolean(raw, "yolo", "create_session"),
          model: optionalString(raw, "model", "create_session"),
          resume: optionalString(raw, "resume", "create_session"),
          metadata: optionalMetadata(raw, "metadata"),
        };
      case "load_session":
        return {
          command: "load_session",
          session_id: expectString(raw, "session_id", "load_session"),
          metadata: optionalMetadata(raw, "metadata"),
        };
      case "new_session":
        return {
          command: "new_session",
          cwd: expectString(raw, "cwd", "new_session"),
          yolo: expectBoolean(raw, "yolo", "new_session"),
          model: optionalString(raw, "model", "new_session"),
        };
      case "prompt":
        return {
          command: "prompt",
          session_id: expectString(raw, "session_id", "prompt"),
          chunks: parsePromptChunks(raw, "prompt"),
        };
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
      case "permission_response": {
        const outcome = asRecord(raw.outcome, "permission_response.outcome");
        const outcomeType = expectString(outcome, "outcome", "permission_response.outcome");
        if (outcomeType !== "selected" && outcomeType !== "cancelled") {
          throw new Error("permission_response.outcome.outcome must be 'selected' or 'cancelled'");
        }
        const parsedOutcome: PermissionOutcome =
          outcomeType === "selected"
            ? {
                outcome: "selected",
                option_id: expectString(outcome, "option_id", "permission_response.outcome"),
              }
            : { outcome: "cancelled" };
        return {
          command: "permission_response",
          session_id: expectString(raw, "session_id", "permission_response"),
          tool_call_id: expectString(raw, "tool_call_id", "permission_response"),
          outcome: parsedOutcome,
        };
      }
      case "shutdown":
        return { command: "shutdown" };
      default:
        throw new Error(`unsupported command: ${commandName}`);
    }
  })();

  return { requestId, command };
}

export function toPermissionMode(mode: string): PermissionMode | null {
  if (
    mode === "default" ||
    mode === "acceptEdits" ||
    mode === "bypassPermissions" ||
    mode === "plan" ||
    mode === "dontAsk"
  ) {
    return mode;
  }
  return null;
}

export function buildModeState(mode: PermissionMode): ModeState {
  return {
    current_mode_id: mode,
    current_mode_name: MODE_NAMES[mode],
    available_modes: MODE_OPTIONS,
  };
}

