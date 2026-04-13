import type { BridgeCommand, BridgeEvent } from "../types.js";

const LOG_SCHEMA = "claude-rs-log/v1" as const;
const DIAGNOSTICS_ENABLED = process.env.CLAUDE_RS_BRIDGE_DIAGNOSTICS === "1";

export const LOG_TARGETS = {
  APP_AUTH: "app.auth",
  APP_CACHE: "app.cache",
  APP_CONFIG: "app.config",
  APP_COMMAND: "app.command",
  APP_NETWORK: "app.network",
  APP_INPUT: "app.input",
  APP_PASTE: "app.paste",
  APP_PERF: "app.perf",
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

function definedFields(fields?: DiagnosticFields): DiagnosticFields | undefined {
  if (!fields) {
    return undefined;
  }
  const entries = Object.entries(fields).filter(([, value]) => value !== undefined);
  return entries.length > 0 ? Object.fromEntries(entries) : undefined;
}

function writeDiagnostic(level: LogLevel, event: DiagnosticEvent): void {
  if (!DIAGNOSTICS_ENABLED) {
    return;
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
    ...(event.durationMs !== undefined ? { duration_ms: event.durationMs } : {}),
    ...(event.count !== undefined ? { count: event.count } : {}),
    ...(event.sizeBytes !== undefined ? { size_bytes: event.sizeBytes } : {}),
  };
  const fields = definedFields(event.fields);
  if (fields) {
    envelope.fields = fields;
  }
  process.stderr.write(`${JSON.stringify(envelope)}\n`);
}

function previewText(value: string, limit: number): string {
  return value.length > limit ? `${value.slice(0, limit)}...` : value;
}

function commandSessionId(command: BridgeCommand): string | undefined {
  switch (command.command) {
    case "resume_session":
    case "prompt":
    case "cancel_turn":
    case "set_model":
    case "set_mode":
    case "generate_session_title":
    case "rename_session":
    case "permission_response":
    case "question_response":
    case "elicitation_response":
    case "get_status_snapshot":
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
    case "prompt":
    case "cancel_turn":
    case "set_model":
    case "set_mode":
    case "generate_session_title":
    case "rename_session":
    case "new_session":
    case "elicitation_response":
    case "get_status_snapshot":
    case "get_context_usage":
    case "reload_plugins":
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
    case "elicitation_request":
    case "elicitation_complete":
    case "mcp_auth_redirect":
    case "mcp_operation_error":
    case "turn_complete":
    case "turn_error":
    case "slash_error":
    case "runtime_reload_completed":
    case "runtime_reload_failed":
    case "session_replaced":
    case "initialized":
    case "sessions_listed":
    case "status_snapshot":
    case "context_usage":
    case "mcp_snapshot":
      return undefined;
  }
}

function protocolCommandLevel(command: BridgeCommand): LogLevel {
  switch (command.command) {
    case "initialize":
    case "create_session":
    case "resume_session":
    case "new_session":
    case "shutdown":
      return "info";
    default:
      return "debug";
  }
}

function protocolEventLevel(event: BridgeEvent): LogLevel {
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
    case "runtime_reload_failed":
      return "warn";
    case "session_update":
    case "permission_request":
    case "question_request":
    case "elicitation_request":
    case "elicitation_complete":
    case "mcp_auth_redirect":
    case "turn_complete":
      return "trace";
    case "sessions_listed":
    case "status_snapshot":
    case "context_usage":
    case "runtime_reload_completed":
    case "mcp_snapshot":
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
      preview: previewText(trimmed, 240),
      preview_chars: Math.min(trimmed.length, 240),
      line_chars: trimmed.length,
    },
  });
}

export function logBridgeCommandReceived(command: BridgeCommand, requestId?: string): void {
  writeDiagnostic(protocolCommandLevel(command), {
    target: LOG_TARGETS.BRIDGE_PROTOCOL,
    eventName: "bridge_command_received",
    message: "bridge command received",
    outcome: "success",
    ...(requestId ? { requestId } : {}),
    ...(commandSessionId(command) ? { sessionId: commandSessionId(command) } : {}),
    ...(commandToolCallId(command) ? { toolCallId: commandToolCallId(command) } : {}),
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
