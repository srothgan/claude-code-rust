import type { BridgeCommand, BridgeEvent } from "../types.js";
import { redactStartupDetail } from "./startup_failures.js";

const LOG_SCHEMA = "claude-rs-log/v1" as const;
/**
 * `1` is detailed diagnostics (every level, full fields). `0` is the default
 * baseline the host sets when detailed logging is off: warnings and errors
 * only, with each field bounded and redacted. Unset (unit tests) writes nothing.
 */
const DIAGNOSTICS_MODE = process.env.CLAUDE_RS_BRIDGE_DIAGNOSTICS;
const DETAILED_DIAGNOSTICS = DIAGNOSTICS_MODE === "1";
const BASELINE_DIAGNOSTICS = DIAGNOSTICS_MODE === "0";
/** Longest excerpt of one baseline field, in characters. */
const BASELINE_FIELD_LIMIT = 240;

/** Whether the host asked for detailed diagnostics (content-bearing fields are allowed). */
export function detailedDiagnosticsEnabled(): boolean {
  return DETAILED_DIAGNOSTICS;
}

export const LOG_TARGETS = {
  APP_AUTH: "app.auth",
  APP_CACHE: "app.cache",
  APP_CONFIG: "app.config",
  APP_COMMAND: "app.command",
  APP_NETWORK: "app.network",
  APP_INPUT: "app.input",
  APP_PASTE: "app.paste",
  APP_RENDER: "app.render",
  APP_SESSION: "app.session",
  APP_TOOL: "app.tool",
  APP_UPDATE: "app.update",
  BRIDGE_LIFECYCLE: "bridge.lifecycle",
  BRIDGE_MCP: "bridge.mcp",
  BRIDGE_PERMISSION: "bridge.permission",
  BRIDGE_PROTOCOL: "bridge.protocol",
  BRIDGE_SDK: "bridge.sdk",
} as const;

export type LogTarget = (typeof LOG_TARGETS)[keyof typeof LOG_TARGETS];

type LogLevel = "error" | "warn" | "info" | "debug" | "trace";

type DiagnosticFields = Record<string, unknown>;

type DiagnosticEvent = {
  target: LogTarget;
  eventName: string;
  message: string;
  outcome?: string;
  sessionId?: string;
  requestId?: string;
  toolCallId?: string;
  commandId?: string;
  terminalId?: string;
  errorKind?: string;
  errorCode?: string;
  durationMs?: number;
  count?: number;
  sizeBytes?: number;
  fields?: DiagnosticFields;
};

function definedFields(
  fields?: DiagnosticFields,
): DiagnosticFields | undefined {
  if (!fields) {
    return undefined;
  }
  const entries = Object.entries(fields).filter(
    ([, value]) => value !== undefined,
  );
  return entries.length > 0 ? Object.fromEntries(entries) : undefined;
}

/**
 * Baseline fields can reach a log the user never asked to be detailed, so
 * every string is redacted and bounded and nested values are flattened to a
 * bounded excerpt rather than carried whole. Which KEYS may reach the default log
 * is decided once, by the host (`baseline_bridge_field_allowed` in
 * src/logging/mod.rs), which drops the rest before writing; do not keep a
 * second list here.
 */
function baselineFields(fields: DiagnosticFields): DiagnosticFields {
  return Object.fromEntries(
    Object.entries(fields).map(([key, value]) => {
      if (value === null || typeof value === "number" || typeof value === "boolean") {
        return [key, value];
      }
      const text = typeof value === "string" ? value : (JSON.stringify(value) ?? String(value));
      return [key, previewText(redactStartupDetail(text), BASELINE_FIELD_LIMIT)];
    }),
  );
}

function writeDiagnostic(level: LogLevel, event: DiagnosticEvent): void {
  if (!DETAILED_DIAGNOSTICS) {
    if (!BASELINE_DIAGNOSTICS || (level !== "warn" && level !== "error")) {
      return;
    }
  }
  const envelope: Record<string, unknown> = {
    schema: LOG_SCHEMA,
    timestamp: new Date().toISOString(),
    level,
    target: event.target,
    event_name: event.eventName,
    message: event.message,
    ...(event.outcome ? { outcome: event.outcome } : {}),
    ...(event.sessionId ? { session_id: event.sessionId } : {}),
    ...(event.requestId ? { request_id: event.requestId } : {}),
    ...(event.toolCallId ? { tool_call_id: event.toolCallId } : {}),
    ...(event.commandId ? { command_id: event.commandId } : {}),
    ...(event.terminalId ? { terminal_id: event.terminalId } : {}),
    ...(event.errorKind ? { error_kind: event.errorKind } : {}),
    ...(event.errorCode ? { error_code: event.errorCode } : {}),
    ...(event.durationMs !== undefined
      ? { duration_ms: event.durationMs }
      : {}),
    ...(event.count !== undefined ? { count: event.count } : {}),
    ...(event.sizeBytes !== undefined ? { size_bytes: event.sizeBytes } : {}),
  };
  const fields = definedFields(event.fields);
  if (fields) {
    envelope.fields = DETAILED_DIAGNOSTICS ? fields : baselineFields(fields);
  }
  process.stderr.write(`${JSON.stringify(envelope)}\n`);
}

function previewText(value: string, limit: number): string {
  return value.length > limit ? `${value.slice(0, limit)}...` : value;
}

function commandSessionId(command: BridgeCommand): string | undefined {
  switch (command.command) {
    case "resume_session":
    case "resume_session_at":
    case "prompt":
    case "side_question":
    case "cancel_turn":
    case "set_model":
    case "set_mode":
    case "set_effort":
    case "set_thinking":
    case "set_agent":
    case "set_ultracode":
    case "refresh_ultracode":
    case "inspect_settings":
    case "mutate_setting":
    case "set_fast_mode":
    case "generate_session_title":
    case "rename_session":
    case "permission_response":
    case "question_response":
    case "elicitation_response":
    case "get_status_snapshot":
    case "get_context_usage":
    case "get_usage":
    case "list_subagents":
    case "get_subagent_messages":
    case "stop_subagent":
    case "get_rewind_targets":
    case "export_session_history":
    case "rewind":
    case "reload_plugins":
    case "reload_skills_and_styles":
    case "mcp_status":
    case "mcp_reconnect":
    case "mcp_toggle":
    case "mcp_set_servers":
    case "mcp_authenticate":
    case "mcp_clear_auth":
    case "mcp_oauth_callback_url":
      return command.session_id;
    case "create_session":
      return command.resume;
    case "initialize":
    case "new_session":
    case "shutdown":
      return undefined;
  }
}

function commandToolCallId(command: BridgeCommand): string | undefined {
  switch (command.command) {
    case "permission_response":
    case "question_response":
      return command.tool_call_id;
    case "initialize":
    case "create_session":
    case "resume_session":
    case "resume_session_at":
    case "prompt":
    case "side_question":
    case "cancel_turn":
    case "set_model":
    case "set_mode":
    case "set_effort":
    case "set_thinking":
    case "set_agent":
    case "set_ultracode":
    case "refresh_ultracode":
    case "inspect_settings":
    case "mutate_setting":
    case "set_fast_mode":
    case "generate_session_title":
    case "rename_session":
    case "new_session":
    case "elicitation_response":
    case "get_status_snapshot":
    case "get_context_usage":
    case "get_usage":
    case "list_subagents":
    case "get_subagent_messages":
    case "stop_subagent":
    case "get_rewind_targets":
    case "export_session_history":
    case "rewind":
    case "reload_plugins":
    case "reload_skills_and_styles":
    case "mcp_status":
    case "mcp_reconnect":
    case "mcp_toggle":
    case "mcp_set_servers":
    case "mcp_authenticate":
    case "mcp_clear_auth":
    case "mcp_oauth_callback_url":
    case "shutdown":
      return undefined;
  }
}

function eventToolCallId(event: BridgeEvent): string | undefined {
  switch (event.event) {
    case "permission_request":
    case "question_request":
      return event.request.tool_call.tool_call_id;
    case "connected":
    case "auth_required":
    case "connection_failed":
    case "session_update":
    case "btw_result":
    case "btw_failed":
    case "user_dialog_request":
    case "elicitation_request":
    case "elicitation_complete":
    case "mcp_auth_redirect":
    case "mcp_operation_error":
    case "mcp_set_servers_result":
    case "user_message_queued":
    case "user_message_started":
    case "user_message_rejected":
    case "turn_interrupt_receipt":
    case "turn_complete":
    case "turn_error":
    case "slash_error":
    case "session_resume_failed":
    case "runtime_reload_completed":
    case "plugin_inventory":
    case "runtime_reload_held":
    case "runtime_reload_failed":
    case "skills_and_styles_reloaded":
    case "skills_and_styles_reload_expired":
    case "session_replaced":
    case "initialized":
    case "sessions_listed":
    case "status_snapshot":
    case "settings_result":
    case "interaction_cancelled":
    case "context_usage":
    case "usage_snapshot":
    case "subagents_listed":
    case "subagent_messages":
    case "subagent_stop_result":
    case "rewind_targets":
    case "session_history_export":
    case "rewind_result":
    case "mcp_snapshot":
      return undefined;
  }
}

function protocolCommandLevel(command: BridgeCommand): LogLevel {
  switch (command.command) {
    case "initialize":
    case "create_session":
    case "resume_session":
    case "resume_session_at":
    case "new_session":
    case "shutdown":
      return "info";
    default:
      return "debug";
  }
}

function protocolEventLevel(event: BridgeEvent): LogLevel {
  // An interrupted turn ends with an abort reason, and the user asked for that:
  // it is routine, not a warning in the default log.
  if (
    event.event === "turn_error" &&
    (event.terminal_reason === "aborted_streaming" || event.terminal_reason === "aborted_tools")
  ) {
    return "debug";
  }
  switch (event.event) {
    case "initialized":
    case "connected":
    case "session_replaced":
      return "info";
    case "connection_failed":
      return "error";
    case "auth_required":
    case "mcp_operation_error":
    case "turn_error":
    case "slash_error":
    case "session_resume_failed":
    case "runtime_reload_failed":
    case "runtime_reload_held":
      return "warn";
    case "session_update":
    case "permission_request":
    case "question_request":
    case "interaction_cancelled":
    case "user_dialog_request":
    case "elicitation_request":
    case "elicitation_complete":
    case "mcp_auth_redirect":
    case "user_message_started":
    case "turn_complete":
      return "trace";
    case "sessions_listed":
    case "status_snapshot":
    case "settings_result":
    case "context_usage":
    case "usage_snapshot":
    case "subagents_listed":
    case "subagent_messages":
    case "subagent_stop_result":
    case "rewind_targets":
    case "session_history_export":
    case "rewind_result":
    case "runtime_reload_completed":
    case "plugin_inventory":
    case "skills_and_styles_reloaded":
    case "skills_and_styles_reload_expired":
    case "mcp_set_servers_result":
    case "user_message_queued":
    case "user_message_rejected":
    case "turn_interrupt_receipt":
    case "mcp_snapshot":
    case "btw_result":
    case "btw_failed":
      return "debug";
  }
}

export const bridgeLogger = {
  error(event: DiagnosticEvent): void {
    writeDiagnostic("error", event);
  },

  warn(event: DiagnosticEvent): void {
    writeDiagnostic("warn", event);
  },

  info(event: DiagnosticEvent): void {
    writeDiagnostic("info", event);
  },

  debug(event: DiagnosticEvent): void {
    writeDiagnostic("debug", event);
  },

  trace(event: DiagnosticEvent): void {
    writeDiagnostic("trace", event);
  },
};

export function logSdkStderrLine(line: string, sessionId?: string): void {
  const trimmed = line.trim();
  if (trimmed.length === 0) {
    return;
  }

  const lowered = trimmed.toLowerCase();
  const level: LogLevel =
    lowered.startsWith("error") || lowered.includes("panic")
      ? "error"
      : lowered.startsWith("warn")
        ? "warn"
        : "debug";

  writeDiagnostic(level, {
    target: LOG_TARGETS.BRIDGE_SDK,
    eventName: "sdk_stderr_line",
    message: "SDK stderr line received",
    ...(sessionId ? { sessionId } : {}),
    fields: {
      preview: previewText(redactStartupDetail(trimmed), 240),
      preview_chars: Math.min(trimmed.length, 240),
      line_chars: trimmed.length,
    },
  });
}

export function logBridgeCommandReceived(
  command: BridgeCommand,
  requestId?: string,
): void {
  writeDiagnostic(protocolCommandLevel(command), {
    target: LOG_TARGETS.BRIDGE_PROTOCOL,
    eventName: "bridge_command_received",
    message: "bridge command received",
    outcome: "success",
    ...(requestId ? { requestId } : {}),
    ...(commandSessionId(command)
      ? { sessionId: commandSessionId(command) }
      : {}),
    ...(commandToolCallId(command)
      ? { toolCallId: commandToolCallId(command) }
      : {}),
    fields: { bridge_command: command.command },
  });
}

export function logBridgeEventSent(
  event: BridgeEvent,
  requestId: string | undefined,
  sizeBytes: number,
): void {
  writeDiagnostic(protocolEventLevel(event), {
    target: LOG_TARGETS.BRIDGE_PROTOCOL,
    eventName: "bridge_event_sent",
    message: "bridge event sent",
    outcome: event.event === "connection_failed" ? "failure" : "success",
    ...(requestId ? { requestId } : {}),
    ...("session_id" in event ? { sessionId: event.session_id } : {}),
    ...(eventToolCallId(event) ? { toolCallId: eventToolCallId(event) } : {}),
    sizeBytes,
    fields: { bridge_event: event.event },
  });
}
