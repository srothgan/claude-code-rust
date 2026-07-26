import type { McpAuthCapabilities, McpAuthRedirect } from "../types.js";

type RuntimeMethod = (...args: unknown[]) => unknown;

const MCP_AUTH_METHODS = {
  authenticate: "mcpAuthenticate",
  clear_auth: "mcpClearAuth",
  submit_oauth_callback_url: "mcpSubmitOAuthCallbackUrl",
} as const;

function runtimeMethod(query: unknown, methodName: string): RuntimeMethod | undefined {
  if ((!query || typeof query !== "object") && typeof query !== "function") {
    return undefined;
  }
  const method = Reflect.get(query, methodName);
  return typeof method === "function" ? (method as RuntimeMethod) : undefined;
}

function requiredRuntimeMethod(query: unknown, methodName: string): RuntimeMethod {
  const method = runtimeMethod(query, methodName);
  if (!method) {
    throw new Error(`installed SDK does not support ${methodName}`);
  }
  return method;
}

async function invokeRuntimeMethod(
  query: unknown,
  methodName: string,
  args: string[],
): Promise<unknown> {
  const method = requiredRuntimeMethod(query, methodName);
  return await Reflect.apply(method, query, args);
}

export function detectMcpAuthCapabilities(query: unknown): McpAuthCapabilities {
  return {
    authenticate: runtimeMethod(query, MCP_AUTH_METHODS.authenticate) !== undefined,
    clear_auth: runtimeMethod(query, MCP_AUTH_METHODS.clear_auth) !== undefined,
    submit_oauth_callback_url:
      runtimeMethod(query, MCP_AUTH_METHODS.submit_oauth_callback_url) !== undefined,
  };
}

function parseMcpAuthRedirect(serverName: string, value: unknown): McpAuthRedirect | null {
  if (value === undefined || value === null) {
    return null;
  }
  if (typeof value !== "object" || Array.isArray(value)) {
    throw new Error("installed SDK returned an invalid mcpAuthenticate response");
  }

  const authUrl = Reflect.get(value, "authUrl");
  if (authUrl === undefined || authUrl === null) {
    return null;
  }
  if (typeof authUrl !== "string" || authUrl.trim().length === 0) {
    throw new Error("installed SDK returned an invalid mcpAuthenticate authUrl");
  }

  const requiresUserAction = Reflect.get(value, "requiresUserAction");
  if (requiresUserAction !== undefined && typeof requiresUserAction !== "boolean") {
    throw new Error("installed SDK returned an invalid mcpAuthenticate requiresUserAction");
  }

  return {
    server_name: serverName,
    auth_url: authUrl,
    requires_user_action: requiresUserAction === true,
  };
}

export async function authenticateMcpServer(
  query: unknown,
  serverName: string,
): Promise<McpAuthRedirect | null> {
  const result = await invokeRuntimeMethod(query, MCP_AUTH_METHODS.authenticate, [serverName]);
  return parseMcpAuthRedirect(serverName, result);
}

export async function clearMcpServerAuth(query: unknown, serverName: string): Promise<void> {
  await invokeRuntimeMethod(query, MCP_AUTH_METHODS.clear_auth, [serverName]);
}

export async function submitMcpOAuthCallbackUrl(
  query: unknown,
  serverName: string,
  callbackUrl: string,
): Promise<void> {
  await invokeRuntimeMethod(query, MCP_AUTH_METHODS.submit_oauth_callback_url, [
    serverName,
    callbackUrl,
  ]);
}
