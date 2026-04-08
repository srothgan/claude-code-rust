import type { BridgeCommand, McpServerConfig, McpServerStatus } from "../types.js";
import { emitMcpOperationError, slashError, writeEvent } from "./events.js";
import { bridgeLogger, LOG_TARGETS } from "./logger.js";
import type { SessionState } from "./session_lifecycle.js";

type QueryWithMcpAuth = import("@anthropic-ai/claude-agent-sdk").Query & {
  mcpAuthenticate?: (serverName: string) => Promise<unknown>;
  mcpClearAuth?: (serverName: string) => Promise<unknown>;
  mcpSubmitOAuthCallbackUrl?: (serverName: string, callbackUrl: string) => Promise<unknown>;
};

type McpAuthMethodName =
  | "mcpAuthenticate"
  | "mcpClearAuth"
  | "mcpSubmitOAuthCallbackUrl";

export const MCP_STALE_STATUS_REVALIDATION_COOLDOWN_MS = 30_000;
const knownConnectedMcpServers = new Set<string>();

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

function queryWithMcpAuth(session: SessionState): QueryWithMcpAuth {
  return session.query as QueryWithMcpAuth;
}

async function callMcpAuthMethod(
  session: SessionState,
  methodName: McpAuthMethodName,
  args: string[],
): Promise<unknown> {
  const query = queryWithMcpAuth(session);
  switch (methodName) {
    case "mcpAuthenticate":
      if (typeof query.mcpAuthenticate !== "function") {
        throw new Error("installed SDK does not support mcpAuthenticate");
      }
      return await query.mcpAuthenticate(args[0] ?? "");
    case "mcpClearAuth":
      if (typeof query.mcpClearAuth !== "function") {
        throw new Error("installed SDK does not support mcpClearAuth");
      }
      return await query.mcpClearAuth(args[0] ?? "");
    case "mcpSubmitOAuthCallbackUrl":
      if (typeof query.mcpSubmitOAuthCallbackUrl !== "function") {
        throw new Error("installed SDK does not support mcpSubmitOAuthCallbackUrl");
      }
      return await query.mcpSubmitOAuthCallbackUrl(args[0] ?? "", args[1] ?? "");
  }
}

function extractMcpAuthRedirect(
  serverName: string,
  value: unknown,
): import("../types.js").McpAuthRedirect | null {
  if (!value || typeof value !== "object") {
    return null;
  }
  const authUrl = Reflect.get(value, "authUrl");
  if (typeof authUrl !== "string" || authUrl.trim().length === 0) {
    return null;
  }
  const requiresUserAction = Reflect.get(value, "requiresUserAction");
  return {
    server_name: serverName,
    auth_url: authUrl,
    requires_user_action: requiresUserAction === true,
  };
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
): Promise<McpServerStatus[]> {
  const servers = await session.query.mcpServerStatus();
  let mapped = servers.map(mapMcpServerStatus);
  mapped = await reconcileSuspiciousMcpStatuses(session, mapped);
  rememberKnownConnectedMcpServers(mapped);
  logMcpSuccess("mcp_snapshot_emitted", "MCP snapshot emitted", session.sessionId, requestId, {
    server_count: mapped.length,
  });
  writeEvent(
    {
      event: "mcp_snapshot",
      session_id: session.sessionId,
      servers: mapped,
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

function rememberKnownConnectedMcpServers(servers: readonly McpServerStatus[]): void {
  for (const server of servers) {
    if (server.status === "connected") {
      knownConnectedMcpServers.add(server.name);
    }
  }
}

function forgetKnownConnectedMcpServer(serverName: string): void {
  knownConnectedMcpServers.delete(serverName);
}

async function reconcileSuspiciousMcpStatuses(
  session: SessionState,
  servers: McpServerStatus[],
): Promise<McpServerStatus[]> {
  const candidates = staleMcpAuthCandidates(
    servers,
    knownConnectedMcpServers,
    session.mcpStatusRevalidatedAt,
  );
  if (candidates.length === 0) {
    return servers;
  }

  const now = Date.now();
  for (const serverName of candidates) {
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
    } catch (error) {
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

  return (await session.query.mcpServerStatus()).map(mapMcpServerStatus);
}

function shouldKeepMonitoringMcpAuth(server: McpServerStatus | undefined): boolean {
  return server?.status === "needs-auth" || server?.status === "pending";
}

function scheduleMcpAuthSnapshotMonitor(
  session: SessionState,
  serverName: string,
  attempt = 0,
): void {
  const maxAttempts = 180;
  const delayMs = 1000;
  setTimeout(() => {
    void monitorMcpAuthSnapshot(session, serverName, attempt + 1, maxAttempts, delayMs);
  }, delayMs);
}

async function monitorMcpAuthSnapshot(
  session: SessionState,
  serverName: string,
  attempt: number,
  maxAttempts: number,
  delayMs: number,
): Promise<void> {
  try {
    const servers = await emitMcpSnapshotEvent(session);
    const server = servers.find((candidate) => candidate.name === serverName);
    if (attempt < maxAttempts && shouldKeepMonitoringMcpAuth(server)) {
      setTimeout(() => {
        void monitorMcpAuthSnapshot(session, serverName, attempt + 1, maxAttempts, delayMs);
      }, delayMs);
    }
  } catch {
    if (attempt < maxAttempts) {
      setTimeout(() => {
        void monitorMcpAuthSnapshot(session, serverName, attempt + 1, maxAttempts, delayMs);
      }, delayMs);
    }
  }
}

export async function handleMcpStatusCommand(
  session: SessionState,
  requestId?: string,
): Promise<void> {
  try {
    await emitMcpSnapshotEvent(session, requestId);
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
        servers: [],
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
    await session.query.setMcpServers(command.servers as Record<string, McpServerConfig>);
    logMcpSuccess(
      "mcp_servers_set_completed",
      "MCP server configuration updated",
      command.session_id,
      requestId,
      { server_count: Object.keys(command.servers).length },
    );
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
    slashError(command.session_id, `failed to set MCP servers: ${message}`, requestId);
  }
}

export async function handleMcpAuthenticateCommand(
  session: SessionState,
  command: Extract<BridgeCommand, { command: "mcp_authenticate" }>,
  requestId?: string,
): Promise<void> {
  try {
    const result = await callMcpAuthMethod(session, "mcpAuthenticate", [command.server_name]);
    const redirect = extractMcpAuthRedirect(command.server_name, result);
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
    scheduleMcpAuthSnapshotMonitor(session, command.server_name);
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
    await callMcpAuthMethod(session, "mcpClearAuth", [command.server_name]);
    forgetKnownConnectedMcpServer(command.server_name);
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
    await callMcpAuthMethod(session, "mcpSubmitOAuthCallbackUrl", [
      command.server_name,
      command.callback_url,
    ]);
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

function mapMcpServerStatus(
  status: Awaited<ReturnType<import("@anthropic-ai/claude-agent-sdk").Query["mcpServerStatus"]>>[number],
): McpServerStatus {
  return {
    name: status.name,
    status: status.status,
    ...(status.serverInfo
      ? {
          server_info: {
            name: status.serverInfo.name,
            version: status.serverInfo.version,
          },
        }
      : {}),
    ...(status.error ? { error: status.error } : {}),
    ...(status.config ? { config: mapMcpServerStatusConfig(status.config) } : {}),
    ...(status.scope ? { scope: status.scope } : {}),
    tools: Array.isArray(status.tools)
      ? status.tools.map((tool) => ({
          name: tool.name,
          ...(tool.description ? { description: tool.description } : {}),
          ...(tool.annotations
            ? {
                annotations: {
                  ...(typeof tool.annotations.readOnly === "boolean"
                    ? { read_only: tool.annotations.readOnly }
                    : {}),
                  ...(typeof tool.annotations.destructive === "boolean"
                    ? { destructive: tool.annotations.destructive }
                    : {}),
                  ...(typeof tool.annotations.openWorld === "boolean"
                    ? { open_world: tool.annotations.openWorld }
                    : {}),
                },
              }
            : {}),
        }))
      : [],
  };
}

function mapMcpServerStatusConfig(
  config: NonNullable<
    Awaited<ReturnType<import("@anthropic-ai/claude-agent-sdk").Query["mcpServerStatus"]>>[number]["config"]
  >,
): import("../types.js").McpServerStatusConfig {
  switch (config.type) {
    case "stdio":
      return {
        type: "stdio",
        command: config.command,
        ...(Array.isArray(config.args) && config.args.length > 0 ? { args: config.args } : {}),
        ...(config.env ? { env: config.env } : {}),
      };
    case "sse":
      return {
        type: "sse",
        url: config.url,
        ...(config.headers ? { headers: config.headers } : {}),
      };
    case "http":
      return {
        type: "http",
        url: config.url,
        ...(config.headers ? { headers: config.headers } : {}),
      };
    case "sdk":
      return {
        type: "sdk",
        name: config.name,
      };
    case "claudeai-proxy":
      return {
        type: "claudeai-proxy",
        url: config.url,
        id: config.id,
      };
    default:
      throw new Error(`unsupported MCP status config: ${JSON.stringify(config)}`);
  }
}
