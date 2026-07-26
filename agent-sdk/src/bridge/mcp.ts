import type {
  BridgeCommand,
  McpSetServersResult,
  McpServerStatus,
  McpSnapshotSource,
} from "../types.js";
import { emitMcpOperationError, slashError, writeEvent } from "./events.js";
import { bridgeLogger, LOG_TARGETS } from "./logger.js";
import {
  bridgeMcpServersToSdk,
  mapMcpServerStatus,
  summarizeMcpServersForDiagnostics,
} from "./mcp_metadata.js";
import {
  authenticateMcpServer,
  clearMcpServerAuth,
  detectMcpAuthCapabilities,
  submitMcpOAuthCallbackUrl,
} from "./mcp_auth_adapter.js";
import {
  runMcpAuthMonitor,
  type McpAuthMonitorHandle,
  type McpAuthMonitorResult,
  type McpAuthMonitorTiming,
} from "./mcp_monitor.js";
import type { SessionState } from "./session_lifecycle.js";

type SdkMcpServerStatus = import("@anthropic-ai/claude-agent-sdk").McpServerStatus;

export const MCP_STALE_STATUS_REVALIDATION_COOLDOWN_MS = 30_000;

function logMcpSuccess(
  eventName: string,
  message: string,
  sessionId: string,
  requestId?: string,
  fields?: Record<string, unknown>,
): void {
  bridgeLogger.info({
    target: LOG_TARGETS.BRIDGE_MCP,
    eventName,
    message,
    outcome: "success",
    sessionId,
    ...(requestId ? { requestId } : {}),
    ...(fields ? { fields } : {}),
  });
}

function logMcpFailure(
  eventName: string,
  message: string,
  sessionId: string,
  errorMessage: string,
  requestId?: string,
  fields?: Record<string, unknown>,
): void {
  bridgeLogger.warn({
    target: LOG_TARGETS.BRIDGE_MCP,
    eventName,
    message,
    outcome: "failure",
    sessionId,
    ...(requestId ? { requestId } : {}),
    fields: {
      ...(fields ?? {}),
      error_message: errorMessage,
    },
  });
}

function mapMcpSetServersResult(result: unknown): McpSetServersResult {
  if (!result || typeof result !== "object" || Array.isArray(result)) {
    return { added: [], removed: [], errors: {} };
  }
  const record = result as Record<string, unknown>;
  const added = Array.isArray(record.added)
    ? record.added.filter((value): value is string => typeof value === "string")
    : [];
  const removed = Array.isArray(record.removed)
    ? record.removed.filter((value): value is string => typeof value === "string")
    : [];
  const errors =
    record.errors && typeof record.errors === "object" && !Array.isArray(record.errors)
      ? Object.fromEntries(
          Object.entries(record.errors as Record<string, unknown>).filter(
            (entry): entry is [string, string] => typeof entry[1] === "string",
          ),
        )
      : {};
  return { added, removed, errors };
}

function emitMcpCommandError(
  sessionId: string,
  operation: string,
  message: string,
  requestId?: string,
  serverName?: string,
): void {
  emitMcpOperationError(
    sessionId,
    {
      ...(serverName ? { server_name: serverName } : {}),
      operation,
      message,
    },
    requestId,
  );
}

export async function emitMcpSnapshotEvent(
  session: SessionState,
  requestId?: string,
  source: McpSnapshotSource = "mcp_status",
): Promise<McpServerStatus[]> {
  const mapped = await loadReconciledMcpStatuses(session);
  if (!mapped) {
    return [];
  }
  return emitMcpSnapshotFromMappedStatuses(session, mapped, source, requestId);
}

export function emitMcpSnapshotFromStatuses(
  session: SessionState,
  servers: readonly SdkMcpServerStatus[],
  source: McpSnapshotSource,
  requestId?: string,
): McpServerStatus[] {
  return emitMcpSnapshotFromMappedStatuses(
    session,
    servers.map(mapMcpServerStatus),
    source,
    requestId,
  );
}

export async function emitReconciledMcpSnapshotFromStatuses(
  session: SessionState,
  servers: readonly SdkMcpServerStatus[],
  source: McpSnapshotSource,
  requestId?: string,
): Promise<McpServerStatus[]> {
  let mapped = servers.map(mapMcpServerStatus);
  mapped = await reconcileSuspiciousMcpStatuses(session, mapped);
  return emitMcpSnapshotFromMappedStatuses(session, mapped, source, requestId);
}

function emitMcpSnapshotFromMappedStatuses(
  session: SessionState,
  mapped: McpServerStatus[],
  source: McpSnapshotSource,
  requestId?: string,
): McpServerStatus[] {
  rememberKnownConnectedMcpServers(session, mapped);
  logMcpSuccess("mcp_snapshot_emitted", "MCP snapshot emitted", session.sessionId, requestId, {
    source,
    server_count: mapped.length,
    servers: summarizeMcpServersForDiagnostics(mapped),
  });
  writeEvent(
    {
      event: "mcp_snapshot",
      session_id: session.sessionId,
      source,
      servers: mapped,
      auth_capabilities: detectMcpAuthCapabilities(session.query),
    },
    requestId,
  );
  return mapped;
}

export function staleMcpAuthCandidates(
  servers: readonly McpServerStatus[],
  knownConnectedServerNames: ReadonlySet<string>,
  lastRevalidatedAt: ReadonlyMap<string, number>,
  now = Date.now(),
  cooldownMs = MCP_STALE_STATUS_REVALIDATION_COOLDOWN_MS,
): string[] {
  return servers
    .filter((server) => {
      if (server.status !== "needs-auth") {
        return false;
      }
      if (!knownConnectedServerNames.has(server.name)) {
        return false;
      }
      const lastAttempt = lastRevalidatedAt.get(server.name) ?? 0;
      return now - lastAttempt >= cooldownMs;
    })
    .map((server) => server.name);
}

function rememberKnownConnectedMcpServers(
  session: SessionState,
  servers: readonly McpServerStatus[],
): void {
  for (const server of servers) {
    if (server.status === "connected") {
      session.knownConnectedMcpServers.add(server.name);
    }
  }
}

function forgetKnownConnectedMcpServer(session: SessionState, serverName: string): void {
  session.knownConnectedMcpServers.delete(serverName);
}

type McpMonitorActivityCheck = () => boolean;

async function loadReconciledMcpStatuses(session: SessionState): Promise<McpServerStatus[]>;
async function loadReconciledMcpStatuses(
  session: SessionState,
  isActive: McpMonitorActivityCheck,
): Promise<McpServerStatus[] | undefined>;
async function loadReconciledMcpStatuses(
  session: SessionState,
  isActive: McpMonitorActivityCheck = () => true,
): Promise<McpServerStatus[] | undefined> {
  const servers = await session.query.mcpServerStatus();
  if (!isActive()) {
    return undefined;
  }
  return await reconcileSuspiciousMcpStatuses(
    session,
    servers.map(mapMcpServerStatus),
    isActive,
  );
}

async function reconcileSuspiciousMcpStatuses(
  session: SessionState,
  servers: McpServerStatus[],
): Promise<McpServerStatus[]>;
async function reconcileSuspiciousMcpStatuses(
  session: SessionState,
  servers: McpServerStatus[],
  isActive: McpMonitorActivityCheck,
): Promise<McpServerStatus[] | undefined>;
async function reconcileSuspiciousMcpStatuses(
  session: SessionState,
  servers: McpServerStatus[],
  isActive: McpMonitorActivityCheck = () => true,
): Promise<McpServerStatus[] | undefined> {
  const candidates = staleMcpAuthCandidates(
    servers,
    session.knownConnectedMcpServers,
    session.mcpStatusRevalidatedAt,
  );
  if (candidates.length === 0) {
    return servers;
  }

  const now = Date.now();
  for (const serverName of candidates) {
    if (!isActive()) {
      return undefined;
    }
    session.mcpStatusRevalidatedAt.set(serverName, now);
    bridgeLogger.info({
      target: LOG_TARGETS.BRIDGE_MCP,
      eventName: "mcp_auth_revalidation_started",
      message: "revalidating stale MCP auth status",
      outcome: "start",
      sessionId: session.sessionId,
      fields: {
        server_name: serverName,
        status: "needs-auth",
        reason: "previously_connected",
        action: "reconnect",
      },
    });
    try {
      await session.query.reconnectMcpServer(serverName);
      if (!isActive()) {
        return undefined;
      }
    } catch (error) {
      if (!isActive()) {
        return undefined;
      }
      const message = error instanceof Error ? error.message : String(error);
      bridgeLogger.warn({
        target: LOG_TARGETS.BRIDGE_MCP,
        eventName: "mcp_auth_revalidation_failed",
        message: "failed to revalidate MCP auth status",
        outcome: "failure",
        sessionId: session.sessionId,
        fields: {
          server_name: serverName,
          action: "reconnect",
          error_message: message,
        },
      });
    }
  }

  const refreshed = await session.query.mcpServerStatus();
  return isActive() ? refreshed.map(mapMcpServerStatus) : undefined;
}

function shouldKeepMonitoringMcpAuth(server: McpServerStatus | undefined): boolean {
  return server?.status === "needs-auth" || server?.status === "pending";
}

function logMcpAuthMonitorExhausted(
  session: SessionState,
  serverName: string,
  result: Extract<McpAuthMonitorResult, { outcome: "exhausted" }>,
): void {
  bridgeLogger.warn({
    target: LOG_TARGETS.BRIDGE_MCP,
    eventName: "mcp_auth_monitor_exhausted",
    message: "MCP authentication status monitor exhausted",
    outcome: "failure",
    sessionId: session.sessionId,
    count: result.attempts,
    fields: {
      server_name: serverName,
      attempts: result.attempts,
      reason: result.reason,
      ...(result.lastError ? { error_message: result.lastError } : {}),
    },
  });
}

export function startMcpAuthSnapshotMonitor(
  session: SessionState,
  serverName: string,
  timing: McpAuthMonitorTiming = {},
): Promise<McpAuthMonitorResult> {
  if (session.closing) {
    return Promise.resolve({ outcome: "cancelled", attempts: 0 });
  }
  const existing = session.mcpAuthMonitors.get(serverName);
  if (existing) {
    return existing.task;
  }

  const controller = new AbortController();
  let monitor!: McpAuthMonitorHandle;
  const isActive = () =>
    !session.closing &&
    !controller.signal.aborted &&
    session.mcpAuthMonitors.get(serverName) === monitor;
  const task = runMcpAuthMonitor({
    ...timing,
    signal: controller.signal,
    poll: async () => {
      const servers = await loadReconciledMcpStatuses(session, isActive);
      if (!isActive() || !servers) {
        return "complete";
      }
      emitMcpSnapshotFromMappedStatuses(session, servers, "mcp_status");
      const server = servers.find((candidate) => candidate.name === serverName);
      return shouldKeepMonitoringMcpAuth(server) ? "continue" : "complete";
    },
  }).then((result) => {
    if (result.outcome === "exhausted") {
      logMcpAuthMonitorExhausted(session, serverName, result);
    }
    return result;
  });
  monitor = { controller, task };
  session.mcpAuthMonitors.set(serverName, monitor);
  void task.then(() => {
    if (session.mcpAuthMonitors.get(serverName) === monitor) {
      session.mcpAuthMonitors.delete(serverName);
    }
  });
  return task;
}

export async function handleMcpStatusCommand(
  session: SessionState,
  requestId?: string,
  source: McpSnapshotSource = "mcp_status",
): Promise<void> {
  try {
    await emitMcpSnapshotEvent(session, requestId, source);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    logMcpFailure(
      "mcp_snapshot_failed",
      "failed to emit MCP snapshot",
      session.sessionId,
      message,
      requestId,
    );
    writeEvent(
      {
        event: "mcp_snapshot",
        session_id: session.sessionId,
        source,
        servers: [],
        auth_capabilities: detectMcpAuthCapabilities(session.query),
        error: message,
      },
      requestId,
    );
  }
}

export async function handleMcpReconnectCommand(
  session: SessionState,
  command: Extract<BridgeCommand, { command: "mcp_reconnect" }>,
  requestId?: string,
): Promise<void> {
  try {
    await session.query.reconnectMcpServer(command.server_name);
    logMcpSuccess("mcp_reconnect_completed", "MCP reconnect completed", command.session_id, requestId, {
      server_name: command.server_name,
    });
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    logMcpFailure(
      "mcp_reconnect_failed",
      "MCP reconnect failed",
      command.session_id,
      message,
      requestId,
      { server_name: command.server_name },
    );
    emitMcpCommandError(
      command.session_id,
      "reconnect",
      message,
      requestId,
      command.server_name,
    );
  }
}

export async function handleMcpToggleCommand(
  session: SessionState,
  command: Extract<BridgeCommand, { command: "mcp_toggle" }>,
  requestId?: string,
): Promise<void> {
  try {
    await session.query.toggleMcpServer(command.server_name, command.enabled);
    logMcpSuccess(
      "mcp_toggle_completed",
      "MCP server toggle completed",
      command.session_id,
      requestId,
      { server_name: command.server_name, enabled: command.enabled },
    );
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    logMcpFailure(
      "mcp_toggle_failed",
      "MCP server toggle failed",
      command.session_id,
      message,
      requestId,
      { server_name: command.server_name, enabled: command.enabled },
    );
    emitMcpCommandError(command.session_id, "toggle", message, requestId, command.server_name);
  }
}

export async function handleMcpSetServersCommand(
  session: SessionState,
  command: Extract<BridgeCommand, { command: "mcp_set_servers" }>,
  requestId?: string,
): Promise<void> {
  try {
    const result = mapMcpSetServersResult(
      await session.query.setMcpServers(bridgeMcpServersToSdk(command.servers)),
    );
    logMcpSuccess(
      "mcp_servers_set_completed",
      "MCP server configuration updated",
      command.session_id,
      requestId,
      {
        server_count: Object.keys(command.servers).length,
        added_count: result.added.length,
        removed_count: result.removed.length,
        error_count: Object.keys(result.errors).length,
      },
    );
    writeEvent(
      {
        event: "mcp_set_servers_result",
        session_id: command.session_id,
        result,
      },
      requestId,
    );
    await handleMcpStatusCommand(session, requestId, "mcp_set_servers");
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    logMcpFailure(
      "mcp_servers_set_failed",
      "failed to update MCP server configuration",
      command.session_id,
      message,
      requestId,
      { server_count: Object.keys(command.servers).length },
    );
    emitMcpOperationError(
      command.session_id,
      { operation: "set-servers", message },
      requestId,
    );
    slashError(command.session_id, `failed to set MCP servers: ${message}`, requestId);
  }
}

export async function handleMcpAuthenticateCommand(
  session: SessionState,
  command: Extract<BridgeCommand, { command: "mcp_authenticate" }>,
  requestId?: string,
): Promise<void> {
  try {
    const redirect = await authenticateMcpServer(session.query, command.server_name);
    if (redirect) {
      logMcpSuccess(
        "mcp_auth_redirect_emitted",
        "MCP auth redirect emitted",
        command.session_id,
        requestId,
        { server_name: command.server_name, requires_user_action: redirect.requires_user_action },
      );
      writeEvent(
        {
          event: "mcp_auth_redirect",
          session_id: command.session_id,
          redirect,
        },
        requestId,
      );
    } else {
      logMcpSuccess(
        "mcp_authenticate_completed",
        "MCP authentication command completed",
        command.session_id,
        requestId,
        { server_name: command.server_name },
      );
    }
    startMcpAuthSnapshotMonitor(session, command.server_name);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    logMcpFailure(
      "mcp_authenticate_failed",
      "MCP authentication command failed",
      command.session_id,
      message,
      requestId,
      { server_name: command.server_name },
    );
    emitMcpCommandError(
      command.session_id,
      "authenticate",
      message,
      requestId,
      command.server_name,
    );
  }
}

export async function handleMcpClearAuthCommand(
  session: SessionState,
  command: Extract<BridgeCommand, { command: "mcp_clear_auth" }>,
  requestId?: string,
): Promise<void> {
  try {
    await clearMcpServerAuth(session.query, command.server_name);
    forgetKnownConnectedMcpServer(session, command.server_name);
    session.mcpStatusRevalidatedAt.delete(command.server_name);
    logMcpSuccess("mcp_clear_auth_completed", "MCP auth cleared", command.session_id, requestId, {
      server_name: command.server_name,
    });
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    logMcpFailure(
      "mcp_clear_auth_failed",
      "failed to clear MCP auth",
      command.session_id,
      message,
      requestId,
      { server_name: command.server_name },
    );
    emitMcpCommandError(
      command.session_id,
      "clear-auth",
      message,
      requestId,
      command.server_name,
    );
  }
}

export async function handleMcpOauthCallbackUrlCommand(
  session: SessionState,
  command: Extract<BridgeCommand, { command: "mcp_oauth_callback_url" }>,
  requestId?: string,
): Promise<void> {
  try {
    await submitMcpOAuthCallbackUrl(session.query, command.server_name, command.callback_url);
    logMcpSuccess(
      "mcp_oauth_callback_completed",
      "MCP OAuth callback URL submitted",
      command.session_id,
      requestId,
      {
        server_name: command.server_name,
        callback_url_chars: command.callback_url.length,
      },
    );
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    logMcpFailure(
      "mcp_oauth_callback_failed",
      "failed to submit MCP OAuth callback URL",
      command.session_id,
      message,
      requestId,
      {
        server_name: command.server_name,
        callback_url_chars: command.callback_url.length,
      },
    );
    emitMcpCommandError(
      command.session_id,
      "submit-callback-url",
      message,
      requestId,
      command.server_name,
    );
  }
}
