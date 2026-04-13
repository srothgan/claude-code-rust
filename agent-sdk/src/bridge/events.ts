import { listSessions, type ListSessionsOptions } from "@anthropic-ai/claude-agent-sdk";
import type {
  BridgeEvent,
  BridgeEventEnvelope,
  McpOperationError,
  SessionUpdate,
} from "../types.js";
import { buildModeState } from "./commands.js";
import { mapSdkSessions } from "./history.js";
import { bridgeLogger, LOG_TARGETS, logBridgeEventSent } from "./logger.js";
import { resolveCurrentModel, type SessionState } from "./session_lifecycle.js";

const SESSION_LIST_LIMIT = 50;
let sessionListingDir: string | undefined;

export function buildSessionListOptions(
  dir: string | undefined,
  limit = SESSION_LIST_LIMIT,
): ListSessionsOptions {
  return dir ? { dir, includeWorktrees: true, limit } : { limit };
}

export function setSessionListingDir(dir: string | undefined): void {
  sessionListingDir = dir;
}

export function currentSessionListOptions(): ListSessionsOptions {
  return buildSessionListOptions(sessionListingDir);
}

export function writeEvent(event: BridgeEvent, requestId?: string): void {
  const envelope: BridgeEventEnvelope = {
    ...(requestId ? { request_id: requestId } : {}),
    ...event,
  };
  const serialized = JSON.stringify(envelope);
  logBridgeEventSent(event, requestId, Buffer.byteLength(serialized) + 1);
  process.stdout.write(`${serialized}\n`);
}

export function failConnection(message: string, requestId?: string): void {
  writeEvent({ event: "connection_failed", message }, requestId);
}

export function slashError(sessionId: string, message: string, requestId?: string): void {
  writeEvent({ event: "slash_error", session_id: sessionId, message }, requestId);
}

export function emitRuntimeReloadCompleted(sessionId: string, requestId?: string): void {
  writeEvent({ event: "runtime_reload_completed", session_id: sessionId }, requestId);
}

export function emitRuntimeReloadFailed(
  sessionId: string,
  message: string,
  requestId?: string,
): void {
  writeEvent({ event: "runtime_reload_failed", session_id: sessionId, message }, requestId);
}

export function emitMcpOperationError(
  sessionId: string,
  error: McpOperationError,
  requestId?: string,
): void {
  writeEvent({ event: "mcp_operation_error", session_id: sessionId, error }, requestId);
}

export function emitSessionUpdate(sessionId: string, update: SessionUpdate): void {
  writeEvent({ event: "session_update", session_id: sessionId, update });
}

export function emitPermissionRequestEvent(
  sessionId: string,
  request: Extract<BridgeEvent, { event: "permission_request" }>["request"],
): void {
  bridgeLogger.info({
    target: LOG_TARGETS.BRIDGE_PERMISSION,
    eventName: "permission_request_emitted",
    message: "permission request emitted",
    outcome: "success",
    sessionId,
    toolCallId: request.tool_call.tool_call_id,
    count: request.options.length,
    fields: {
      option_count: request.options.length,
      tool_title: request.tool_call.title,
    },
  });
  writeEvent({ event: "permission_request", session_id: sessionId, request });
}

export function emitQuestionRequestEvent(
  sessionId: string,
  request: Extract<BridgeEvent, { event: "question_request" }>["request"],
): void {
  bridgeLogger.info({
    target: LOG_TARGETS.BRIDGE_PERMISSION,
    eventName: "question_request_emitted",
    message: "question request emitted",
    outcome: "success",
    sessionId,
    toolCallId: request.tool_call.tool_call_id,
    count: request.prompt.options.length,
    fields: {
      question_index: request.question_index,
      total_questions: request.total_questions,
      option_count: request.prompt.options.length,
      header: request.prompt.header,
    },
  });
  writeEvent({ event: "question_request", session_id: sessionId, request });
}

export function emitElicitationRequestEvent(
  sessionId: string,
  request: Extract<BridgeEvent, { event: "elicitation_request" }>["request"],
): void {
  bridgeLogger.info({
    target: LOG_TARGETS.BRIDGE_PERMISSION,
    eventName: "elicitation_request_emitted",
    message: "elicitation request emitted",
    outcome: "success",
    sessionId,
    requestId: request.request_id,
    fields: {
      server_name: request.server_name,
      mode: request.mode,
      has_url: request.url !== undefined,
      has_requested_schema: request.requested_schema !== undefined,
    },
  });
  writeEvent({ event: "elicitation_request", session_id: sessionId, request });
}

function buildConnectBridgeEvent(
  session: SessionState,
  eventName: "connected" | "session_replaced",
): BridgeEvent {
  const historyUpdates = session.resumeUpdates;
  return eventName === "session_replaced"
    ? {
        event: "session_replaced",
        session_id: session.sessionId,
        cwd: session.cwd,
        current_model: session.currentModel ?? resolveCurrentModel(session),
        available_models: session.availableModels,
        mode: session.mode ? buildModeState(session, session.mode) : null,
        ...(historyUpdates && historyUpdates.length > 0 ? { history_updates: historyUpdates } : {}),
      }
    : {
        event: "connected",
        session_id: session.sessionId,
        cwd: session.cwd,
        current_model: session.currentModel ?? resolveCurrentModel(session),
        available_models: session.availableModels,
        mode: session.mode ? buildModeState(session, session.mode) : null,
        ...(historyUpdates && historyUpdates.length > 0 ? { history_updates: historyUpdates } : {}),
      };
}

function logConnectEventEmission(
  session: SessionState,
  eventName: "connected" | "session_replaced",
  requestId?: string,
): void {
  bridgeLogger.info({
    target: LOG_TARGETS.APP_SESSION,
    eventName: eventName === "session_replaced" ? "session_replaced_emitted" : "session_connected_emitted",
    message: eventName === "session_replaced" ? "session replaced event emitted" : "session connected event emitted",
    outcome: "success",
    ...(requestId ? { requestId } : {}),
    sessionId: session.sessionId,
    fields: {
      history_update_count: session.resumeUpdates?.length ?? 0,
      available_model_count: session.availableModels.length,
      stale_session_count: session.sessionsToCloseAfterConnect?.length ?? 0,
    },
  });
}

export function emitConnectEvent(session: SessionState): void {
  const bridgeEvent = buildConnectBridgeEvent(session, session.connectEvent);
  logConnectEventEmission(session, session.connectEvent, session.connectRequestId);
  writeEvent(bridgeEvent, session.connectRequestId);
  session.connectRequestId = undefined;
  session.connected = true;
  session.authHintSent = false;
  session.resumeUpdates = undefined;

  const staleSessions = session.sessionsToCloseAfterConnect;
  session.sessionsToCloseAfterConnect = undefined;
  if (!staleSessions || staleSessions.length === 0) {
    refreshSessionsList();
    return;
  }
  void (async () => {
    // Lazy import to break circular dependency at module-evaluation time.
    const { sessions, closeSessionWithLogging } = await import("./session_lifecycle.js");
    for (const stale of staleSessions) {
      if (stale === session) {
        continue;
      }
      if (sessions.get(stale.sessionId) === stale) {
        sessions.delete(stale.sessionId);
      }
      await closeSessionWithLogging(stale, { reason: "stale_after_connect" });
    }
    refreshSessionsList();
  })();
}

export function emitSessionReplacedEvent(session: SessionState, requestId?: string): void {
  const bridgeEvent = buildConnectBridgeEvent(session, "session_replaced");
  logConnectEventEmission(session, "session_replaced", requestId);
  writeEvent(bridgeEvent, requestId);
  session.resumeUpdates = undefined;
  refreshSessionsList();
}

export async function emitSessionsList(requestId?: string): Promise<void> {
  bridgeLogger.debug({
    target: LOG_TARGETS.APP_SESSION,
    eventName: "sessions_list_requested",
    message: "sessions list requested",
    outcome: "start",
    ...(requestId ? { requestId } : {}),
    fields: {
      has_listing_dir: sessionListingDir !== undefined,
      limit: SESSION_LIST_LIMIT,
    },
  });
  try {
    const sdkSessions = await listSessions(currentSessionListOptions());
    const sessions = mapSdkSessions(sdkSessions, SESSION_LIST_LIMIT);
    bridgeLogger.info({
      target: LOG_TARGETS.APP_SESSION,
      eventName: "sessions_list_completed",
      message: "sessions list completed",
      outcome: "success",
      ...(requestId ? { requestId } : {}),
      count: sessions.length,
      fields: { session_count: sessions.length },
    });
    writeEvent({ event: "sessions_listed", sessions }, requestId);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    bridgeLogger.warn({
      target: LOG_TARGETS.APP_SESSION,
      eventName: "sessions_list_failed",
      message: "failed to list SDK sessions",
      outcome: "failure",
      ...(requestId ? { requestId } : {}),
      fields: { error_message: message },
    });
    writeEvent({ event: "sessions_listed", sessions: [] }, requestId);
  }
}

export function refreshSessionsList(): void {
  void emitSessionsList().catch(() => {
    // Defensive no-op.
  });
}
