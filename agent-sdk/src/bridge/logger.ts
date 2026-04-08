const LOG_SCHEMA = "claude-rs-log/v1" as const;
const SDK_DEBUG_ENABLED = process.env.CLAUDE_RS_SDK_DEBUG === "1";
const PERMISSION_DEBUG_ENABLED =
  process.env.CLAUDE_RS_SDK_PERMISSION_DEBUG === "1" || SDK_DEBUG_ENABLED;

export const LOG_TARGETS = {
  APP_SESSION: "app.session",
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

type PermissionDebugContext = Omit<DiagnosticEvent, "target" | "eventName" | "message">;

function definedFields(fields?: DiagnosticFields): DiagnosticFields | undefined {
  if (!fields) {
    return undefined;
  }
  const entries = Object.entries(fields).filter(([, value]) => value !== undefined);
  return entries.length > 0 ? Object.fromEntries(entries) : undefined;
}

function writeDiagnostic(level: LogLevel, event: DiagnosticEvent): void {
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

export function logPermissionDebug(message: string, context: PermissionDebugContext = {}): void {
  if (!PERMISSION_DEBUG_ENABLED) {
    return;
  }
  bridgeLogger.debug({
    target: LOG_TARGETS.BRIDGE_PERMISSION,
    eventName: "permission_debug",
    message,
    ...context,
  });
}

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
