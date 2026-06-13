import type {
  McpServerConfig,
  McpServerOrgMaxPermission,
  McpServerStatus,
  McpServerStatusConfig,
  McpServerToolPermissionPolicy,
  McpServerToolPolicy,
} from "../types.js";
import { bridgeLogger, LOG_TARGETS } from "./logger.js";

type SdkMcpServerConfig = import("@anthropic-ai/claude-agent-sdk").McpServerConfig;
type SdkMcpServerStatus = import("@anthropic-ai/claude-agent-sdk").McpServerStatus;
type SdkMcpServerStatusConfig = NonNullable<SdkMcpServerStatus["config"]>;

type McpServerDiagnosticSummary = {
  name: string;
  status: McpServerStatus["status"];
  config_type: string;
  scope?: string;
  timeout_ms?: number;
  always_load?: boolean;
  tool_count: number;
  configured_tool_policy_count: number;
  has_error: boolean;
  has_server_info: boolean;
};

const TOOL_PERMISSION_POLICIES = new Set<McpServerToolPermissionPolicy>([
  "always_allow",
  "always_ask",
  "always_deny",
]);

const ORG_MAX_PERMISSIONS = new Set<McpServerOrgMaxPermission>([
  "allow",
  "ask",
  "blocked",
]);

function asRecord(value: unknown, context: string): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${context} must be an object`);
  }
  return value as Record<string, unknown>;
}

function optionalStringArray(
  record: Record<string, unknown>,
  key: string,
  context: string,
): string[] | undefined {
  const value = record[key];
  if (value === undefined) {
    return undefined;
  }
  if (!Array.isArray(value) || !value.every((entry) => typeof entry === "string")) {
    throw new Error(`${context}.${key} must be an array of strings`);
  }
  return value;
}

function optionalStringMap(
  record: Record<string, unknown>,
  key: string,
  context: string,
): Record<string, string> | undefined {
  const value = record[key];
  if (value === undefined) {
    return undefined;
  }
  const map = asRecord(value, `${context}.${key}`);
  for (const [entryKey, entryValue] of Object.entries(map)) {
    if (typeof entryValue !== "string") {
      throw new Error(`${context}.${key}.${entryKey} must be a string`);
    }
  }
  return map as Record<string, string>;
}

function optionalTimeout(
  record: Record<string, unknown>,
  context: string,
): number | undefined {
  const value = record.timeout;
  if (value === undefined) {
    return undefined;
  }
  if (typeof value !== "number" || !Number.isFinite(value) || !Number.isInteger(value) || value < 1000) {
    throw new Error(`${context}.timeout must be an integer >= 1000`);
  }
  return value;
}

function optionalAlwaysLoad(
  record: Record<string, unknown>,
  context: string,
): boolean | undefined {
  const value = record.always_load;
  if (value === undefined) {
    return undefined;
  }
  if (typeof value !== "boolean") {
    throw new Error(`${context}.always_load must be a boolean`);
  }
  return value;
}

function optionalToolPolicies(
  record: Record<string, unknown>,
  context: string,
): McpServerToolPolicy[] | undefined {
  const value = record.tools;
  if (value === undefined) {
    return undefined;
  }
  if (!Array.isArray(value)) {
    throw new Error(`${context}.tools must be an array`);
  }
  return value.map((entry, index) => {
    const item = asRecord(entry, `${context}.tools[${index}]`);
    const name = item.name;
    if (typeof name !== "string") {
      throw new Error(`${context}.tools[${index}].name must be a string`);
    }
    const policy: McpServerToolPolicy = { name };
    const permissionPolicy = item.permission_policy;
    if (permissionPolicy !== undefined) {
      if (typeof permissionPolicy !== "string" || !TOOL_PERMISSION_POLICIES.has(permissionPolicy as McpServerToolPermissionPolicy)) {
        throw new Error(`${context}.tools[${index}].permission_policy must be one of always_allow, always_ask, always_deny`);
      }
      policy.permission_policy = permissionPolicy as McpServerToolPermissionPolicy;
    }
    const orgMaxPermission = item.org_max_permission;
    if (orgMaxPermission !== undefined) {
      if (typeof orgMaxPermission !== "string" || !ORG_MAX_PERMISSIONS.has(orgMaxPermission as McpServerOrgMaxPermission)) {
        throw new Error(`${context}.tools[${index}].org_max_permission must be one of allow, ask, blocked`);
      }
      policy.org_max_permission = orgMaxPermission as McpServerOrgMaxPermission;
    }
    return policy;
  });
}

export function parseMcpServerConfig(value: unknown, context: string): McpServerConfig {
  const record = asRecord(value, context);
  const rawType = record.type;
  const type = rawType === undefined ? "stdio" : rawType;
  if (typeof type !== "string") {
    throw new Error(`${context}.type must be a string`);
  }

  const timeout = optionalTimeout(record, context);
  const alwaysLoad = optionalAlwaysLoad(record, context);

  switch (type) {
    case "stdio": {
      if (record.tools !== undefined) {
        throw new Error(`${context}.tools is only supported for http and sse MCP servers`);
      }
      const command = record.command;
      if (typeof command !== "string") {
        throw new Error(`${context}.command must be a string`);
      }
      return {
        type,
        command,
        ...(optionalStringArray(record, "args", context) ? { args: optionalStringArray(record, "args", context) } : {}),
        ...(optionalStringMap(record, "env", context) ? { env: optionalStringMap(record, "env", context) } : {}),
        ...(timeout === undefined ? {} : { timeout }),
        ...(alwaysLoad === undefined ? {} : { always_load: alwaysLoad }),
      };
    }
    case "sse":
    case "http": {
      const url = record.url;
      if (typeof url !== "string") {
        throw new Error(`${context}.url must be a string`);
      }
      const tools = optionalToolPolicies(record, context);
      return {
        type,
        url,
        ...(optionalStringMap(record, "headers", context) ? { headers: optionalStringMap(record, "headers", context) } : {}),
        ...(tools === undefined ? {} : { tools }),
        ...(timeout === undefined ? {} : { timeout }),
        ...(alwaysLoad === undefined ? {} : { always_load: alwaysLoad }),
      };
    }
    default:
      throw new Error(`${context}.type must be one of stdio, sse, http`);
  }
}

export function parseMcpServersRecord(
  value: unknown,
  context: string,
): Record<string, McpServerConfig> {
  const record = asRecord(value, context);
  return Object.fromEntries(
    Object.entries(record).map(([key, entry]) => [key, parseMcpServerConfig(entry, `${context}.${key}`)]),
  );
}

function toSdkToolPolicies(tools?: McpServerToolPolicy[]): import("@anthropic-ai/claude-agent-sdk").McpServerToolPolicy[] | undefined {
  return tools?.map((tool) => ({
    name: tool.name,
    ...(tool.permission_policy === undefined ? {} : { permission_policy: tool.permission_policy }),
    ...(tool.org_max_permission === undefined ? {} : { org_max_permission: tool.org_max_permission }),
  }));
}

export function bridgeMcpConfigToSdk(config: McpServerConfig): SdkMcpServerConfig {
  switch (config.type) {
    case "stdio":
      return {
        type: "stdio",
        command: config.command,
        ...(config.args ? { args: config.args } : {}),
        ...(config.env ? { env: config.env } : {}),
        ...(config.timeout === undefined ? {} : { timeout: config.timeout }),
        ...(config.always_load === undefined ? {} : { alwaysLoad: config.always_load }),
      };
    case "sse":
      return {
        type: "sse",
        url: config.url,
        ...(config.headers ? { headers: config.headers } : {}),
        ...(config.tools ? { tools: toSdkToolPolicies(config.tools) } : {}),
        ...(config.timeout === undefined ? {} : { timeout: config.timeout }),
        ...(config.always_load === undefined ? {} : { alwaysLoad: config.always_load }),
      };
    case "http":
      return {
        type: "http",
        url: config.url,
        ...(config.headers ? { headers: config.headers } : {}),
        ...(config.tools ? { tools: toSdkToolPolicies(config.tools) } : {}),
        ...(config.timeout === undefined ? {} : { timeout: config.timeout }),
        ...(config.always_load === undefined ? {} : { alwaysLoad: config.always_load }),
      };
  }
}

export function bridgeMcpServersToSdk(
  servers: Record<string, McpServerConfig>,
): Record<string, SdkMcpServerConfig> {
  return Object.fromEntries(
    Object.entries(servers).map(([name, config]) => [name, bridgeMcpConfigToSdk(config)]),
  );
}

function mapSdkToolPolicies(tools: unknown): McpServerToolPolicy[] | undefined {
  if (!Array.isArray(tools)) {
    return undefined;
  }
  return tools
    .map((tool) => {
      if (!tool || typeof tool !== "object" || Array.isArray(tool)) {
        return null;
      }
      const raw = tool as Record<string, unknown>;
      if (typeof raw.name !== "string") {
        return null;
      }
      const policy: McpServerToolPolicy = { name: raw.name };
      if (raw.permission_policy !== undefined) {
        if (typeof raw.permission_policy !== "string" || !TOOL_PERMISSION_POLICIES.has(raw.permission_policy as McpServerToolPermissionPolicy)) {
          return null;
        }
        policy.permission_policy = raw.permission_policy as McpServerToolPermissionPolicy;
      }
      if (raw.org_max_permission !== undefined) {
        if (typeof raw.org_max_permission !== "string" || !ORG_MAX_PERMISSIONS.has(raw.org_max_permission as McpServerOrgMaxPermission)) {
          return null;
        }
        policy.org_max_permission = raw.org_max_permission as McpServerOrgMaxPermission;
      }
      return policy;
    })
    .filter((tool): tool is McpServerToolPolicy => tool !== null);
}

export function mapMcpServerStatus(status: SdkMcpServerStatus): McpServerStatus {
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

export function mapMcpServerStatusConfig(config: SdkMcpServerStatusConfig): McpServerStatusConfig {
  switch (config.type) {
    case "stdio":
      return {
        type: "stdio",
        command: config.command,
        ...(Array.isArray(config.args) && config.args.length > 0 ? { args: config.args } : {}),
        ...(config.env ? { env: config.env } : {}),
        ...(config.timeout === undefined ? {} : { timeout: config.timeout }),
        ...(config.alwaysLoad === undefined ? {} : { always_load: config.alwaysLoad }),
      };
    case "sse": {
      const tools = mapSdkToolPolicies(config.tools);
      return {
        type: "sse",
        url: config.url,
        ...(config.headers ? { headers: config.headers } : {}),
        ...(tools === undefined ? {} : { tools }),
        ...(config.timeout === undefined ? {} : { timeout: config.timeout }),
        ...(config.alwaysLoad === undefined ? {} : { always_load: config.alwaysLoad }),
      };
    }
    case "http": {
      const tools = mapSdkToolPolicies(config.tools);
      return {
        type: "http",
        url: config.url,
        ...(config.headers ? { headers: config.headers } : {}),
        ...(tools === undefined ? {} : { tools }),
        ...(config.timeout === undefined ? {} : { timeout: config.timeout }),
        ...(config.alwaysLoad === undefined ? {} : { always_load: config.alwaysLoad }),
      };
    }
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
        ...(config.timeout === undefined ? {} : { timeout: config.timeout }),
      };
    default: {
      const raw = config as Record<string, unknown>;
      const rawType = typeof raw.type === "string" ? raw.type : "unknown";
      bridgeLogger.warn({
        target: LOG_TARGETS.BRIDGE_MCP,
        eventName: "mcp_status_config_unknown",
        message: "unknown MCP status config type",
        outcome: "ignored",
        fields: { config_type: rawType },
      });
      return { type: "unknown", raw_type: rawType };
    }
  }
}

function mcpStatusConfigDiagnostics(config: McpServerStatusConfig | undefined): {
  config_type: string;
  timeout_ms?: number;
  always_load?: boolean;
  configured_tool_policy_count: number;
} {
  if (!config) {
    return {
      config_type: "missing",
      configured_tool_policy_count: 0,
    };
  }

  switch (config.type) {
    case "stdio":
      return {
        config_type: "stdio",
        ...(config.timeout === undefined ? {} : { timeout_ms: config.timeout }),
        ...(config.always_load === undefined ? {} : { always_load: config.always_load }),
        configured_tool_policy_count: 0,
      };
    case "sse":
    case "http":
      return {
        config_type: config.type,
        ...(config.timeout === undefined ? {} : { timeout_ms: config.timeout }),
        ...(config.always_load === undefined ? {} : { always_load: config.always_load }),
        configured_tool_policy_count: config.tools?.length ?? 0,
      };
    case "sdk":
      return {
        config_type: "sdk",
        configured_tool_policy_count: 0,
      };
    case "claudeai-proxy":
      return {
        config_type: "claudeai-proxy",
        ...(config.timeout === undefined ? {} : { timeout_ms: config.timeout }),
        configured_tool_policy_count: 0,
      };
    case "unknown":
      return {
        config_type: `unknown:${config.raw_type}`,
        configured_tool_policy_count: 0,
      };
  }
}

export function summarizeMcpServersForDiagnostics(
  servers: readonly McpServerStatus[],
): McpServerDiagnosticSummary[] {
  return servers.map((server) => {
    const config = mcpStatusConfigDiagnostics(server.config);
    return {
      name: server.name,
      status: server.status,
      config_type: config.config_type,
      ...(server.scope ? { scope: server.scope } : {}),
      ...(config.timeout_ms === undefined ? {} : { timeout_ms: config.timeout_ms }),
      ...(config.always_load === undefined ? {} : { always_load: config.always_load }),
      tool_count: server.tools.length,
      configured_tool_policy_count: config.configured_tool_policy_count,
      has_error: typeof server.error === "string" && server.error.length > 0,
      has_server_info: server.server_info !== undefined,
    };
  });
}
