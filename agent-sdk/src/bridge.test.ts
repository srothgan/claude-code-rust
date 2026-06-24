import test from "node:test";
import assert from "node:assert/strict";
import {
  AsyncQueue,
  CACHE_SPLIT_POLICY,
  buildApiRetryUpdate,
  buildRateLimitUpdate,
  buildQueryOptions,
  canGenerateSessionTitle,
  generatePersistedSessionTitle,
  buildSessionMutationOptions,
  buildSessionListOptions,
  buildToolResultFields,
  createToolCall,
  applySessionAgent,
  applySessionEffort,
  emitAgentConfigOptionUpdate,
  emitEffortConfigOptionUpdate,
  handleTaskSystemMessage,
  handleSdkMessage,
  isShellToolName,
  mapSdkAccountInfo,
  mapAvailableAgents,
  mapAvailableModels,
  mapSessionMessagesToUpdates,
  mapSdkSessions,
  agentSdkVersionCompatibilityError,
  looksLikeAuthRequired,
  normalizeToolResultText,
  parseFastModeState,
  parseRuntimeSessionState,
  parseRateLimitStatus,
  bridgeMcpConfigToSdk,
  mapMcpServerStatus,
  mapMcpServerStatusConfig,
  normalizeSettingsParseError,
  normalizeToolKind,
  parseCommandEnvelope,
  permissionOptionsFromSuggestions,
  permissionResultFromOutcome,
  previewKilobyteLabel,
  staleMcpAuthCandidates,
  resolveInstalledAgentSdkVersion,
  unwrapToolUseResult,
  updateAvailableCommands,
  handleReloadPluginsCommand,
} from "./bridge.js";
import type { SessionState } from "./bridge.js";
import type { Options } from "@anthropic-ai/claude-agent-sdk";
import {
  availableModesForSession,
  buildModeState,
  markModeUnavailableForSession,
  permissionModeFailureLooksUnsupported,
  refreshSupportedModesForSession,
} from "./bridge/commands.js";
import { handleMcpSetServersCommand } from "./bridge/mcp.js";
import {
  emitCurrentModelUpdate,
  handleUserDialogResponse,
  refreshCurrentModel,
  resolveCurrentModel,
  sessions,
  shouldInvalidateResolvedRuntimeModel,
  shouldEmitStartupAuthRequiredForAccount,
} from "./bridge/session_lifecycle.js";
import { classifyTurnErrorKind } from "./bridge/error_classification.js";
import { emitToolCall, emitToolProgressUpdate, emitToolResultUpdate } from "./bridge/tool_calls.js";
import { linkTaskToolUse } from "./bridge/task_links.js";
import { requestAskUserQuestionAnswers } from "./bridge/user_interaction.js";
import { handleResultMessage } from "./bridge/message_handlers.js";

const BRIDGE_RUNTIME_PROCESS_NAME =
  process.platform === "win32" ? "claude-rs-bridge-node.exe" : "claude-rs-bridge-node";
const BRIDGE_RUNTIME_GUARD_PROMPT =
  `Do not terminate the Claude Rust bridge runtime process \`${BRIDGE_RUNTIME_PROCESS_NAME}\`; ` +
  "when cleaning up development servers, only stop processes by explicit PIDs you started in this session.";
const GERMAN_LANGUAGE_PROMPT =
  "Always respond to the user in German unless the user explicitly asks for a different language. " +
  "Keep code, shell commands, file paths, API names, tool names, and raw error text unchanged unless the user explicitly asks for translation.";

function makeSessionState(): SessionState {
  const input = new AsyncQueue<import("@anthropic-ai/claude-agent-sdk").SDKUserMessage>();
  return {
    sessionId: "session-1",
    cwd: "C:/work",
    model: "haiku",
    availableModels: [],
    mode: null,
    supportedModeIds: [],
    runtimeUnavailableModeIds: [],
    supportsBypassPermissionsMode: false,
    fastModeState: "off",
    query: {} as import("@anthropic-ai/claude-agent-sdk").Query,
    input,
    connected: true,
    connectEvent: "connected",
    toolCalls: new Map(),
    tasksById: new Map(),
    taskOrder: [],
    taskToolUseIds: new Map(),
    taskIdsByToolUseId: new Map(),
    pendingPermissions: new Map(),
    pendingQuestions: new Map(),
    pendingUserDialogs: new Map(),
    pendingElicitations: new Map(),
    mcpStatusRevalidatedAt: new Map(),
    hiddenToolUseIds: new Set(),
    authHintSent: false,
  };
}

test("availableModesForSession omits conditional modes when unsupported", () => {
  const session = makeSessionState();
  refreshSupportedModesForSession(session);

  assert.deepEqual(
    availableModesForSession(session).map((entry) => entry.id),
    ["default", "acceptEdits", "plan", "dontAsk"],
  );
});

test("buildModeState includes auto and bypassPermissions when supported", () => {
  const session = makeSessionState();
  session.mode = "default";
  session.model = "sonnet";
  session.supportsBypassPermissionsMode = true;
  session.availableModels = [
    {
      id: "sonnet",
      display_name: "Sonnet",
      supports_effort: true,
      supported_effort_levels: ["low", "medium", "high"],
      supports_auto_mode: true,
    },
  ];
  refreshSupportedModesForSession(session);

  const mode = buildModeState(session, "default");

  assert.deepEqual(
    mode.available_modes.map((entry) => entry.id),
    ["default", "auto", "acceptEdits", "plan", "dontAsk", "bypassPermissions"],
  );
});

test("refreshSupportedModesForSession uses resolved current model for auto-mode eligibility", () => {
  const session = makeSessionState();
  session.model = "sonnet";
  session.availableModels = [
    {
      id: "sonnet",
      display_name: "Claude Sonnet",
      supports_effort: true,
      supported_effort_levels: ["low", "medium", "high"],
      supports_auto_mode: true,
    },
  ];
  session.currentModel = {
    resolved_id: "claude-sonnet-4-7[1m]",
    display_name_short: "Sonnet 4.7 [1M]",
    display_name_long: "Sonnet 4.7 [1M]",
    supports_effort: true,
    supported_effort_levels: ["low", "medium", "high"],
    supports_auto_mode: false,
    is_authoritative: true,
  };

  refreshSupportedModesForSession(session);

  assert.deepEqual(
    availableModesForSession(session).map((entry) => entry.id),
    ["default", "acceptEdits", "plan", "dontAsk"],
  );
});

test("refreshSupportedModesForSession retains current mode before capability data arrives", () => {
  const session = makeSessionState();
  session.mode = "auto";

  refreshSupportedModesForSession(session);

  assert.deepEqual(
    session.supportedModeIds,
    ["default", "auto", "acceptEdits", "plan", "dontAsk"],
  );
});

test("markModeUnavailableForSession prunes rejected runtime mode from session list", () => {
  const session = makeSessionState();
  session.model = "sonnet";
  session.availableModels = [
    {
      id: "sonnet",
      display_name: "Sonnet",
      supports_effort: true,
      supported_effort_levels: ["low", "medium", "high"],
      supports_auto_mode: true,
    },
  ];
  refreshSupportedModesForSession(session);

  assert.equal(markModeUnavailableForSession(session, "auto"), true);
  assert.deepEqual(
    availableModesForSession(session).map((entry) => entry.id),
    ["default", "acceptEdits", "plan", "dontAsk"],
  );
});

test("permissionModeFailureLooksUnsupported detects SDK capability rejections", () => {
  assert.equal(
    permissionModeFailureLooksUnsupported(
      "auto",
      "Cannot set permission mode to auto: not available in my plan",
    ),
    true,
  );
  assert.equal(
    permissionModeFailureLooksUnsupported(
      "bypassPermissions",
      "Cannot set permission mode to bypassPermissions because the session was not launched with --dangerously-skip-permissions",
    ),
    true,
  );
  assert.equal(
    permissionModeFailureLooksUnsupported(
      "auto",
      "bridge disconnected before request completed",
    ),
    false,
  );
});

function captureBridgeEvents(run: () => void): Array<Record<string, unknown>> {
  const writes: string[] = [];
  const originalWrite = process.stdout.write;
  (process.stdout.write as unknown as (...args: unknown[]) => boolean) = (
    chunk: unknown,
  ): boolean => {
    const text = Buffer.isBuffer(chunk)
      ? chunk.toString("utf8")
      : typeof chunk === "string"
        ? chunk
        : String(chunk);
    if (text.trimStart().startsWith("{")) {
      writes.push(text);
      return true;
    }
    return originalWrite.call(process.stdout, chunk as never);
  };

  try {
    run();
  } finally {
    process.stdout.write = originalWrite;
  }

  return writes
    .map((line) => line.trim())
    .filter((line) => line.startsWith("{"))
    .flatMap((line) => {
      try {
        return [JSON.parse(line) as Record<string, unknown>];
      } catch {
        return [];
      }
    });
}

async function captureBridgeEventsAsync(
  run: () => Promise<void>,
): Promise<Array<Record<string, unknown>>> {
  const writes: string[] = [];
  const originalWrite = process.stdout.write;
  (process.stdout.write as unknown as (...args: unknown[]) => boolean) = (
    chunk: unknown,
  ): boolean => {
    const text = Buffer.isBuffer(chunk)
      ? chunk.toString("utf8")
      : typeof chunk === "string"
        ? chunk
        : String(chunk);
    if (text.trimStart().startsWith("{")) {
      writes.push(text);
      return true;
    }
    return originalWrite.call(process.stdout, chunk as never);
  };

  try {
    await run();
  } finally {
    process.stdout.write = originalWrite;
  }

  return writes
    .map((line) => line.trim())
    .filter((line) => line.startsWith("{"))
    .flatMap((line) => {
      try {
        return [JSON.parse(line) as Record<string, unknown>];
      } catch {
        return [];
      }
    });
}

test("parseCommandEnvelope validates initialize command", () => {
  const parsed = parseCommandEnvelope(
    JSON.stringify({
      request_id: "req-1",
      command: "initialize",
      cwd: "C:/work",
    }),
  );
  assert.equal(parsed.requestId, "req-1");
  assert.equal(parsed.command.command, "initialize");
  if (parsed.command.command !== "initialize") {
    throw new Error("unexpected command variant");
  }
  assert.equal(parsed.command.cwd, "C:/work");
});

test("parseCommandEnvelope validates resume_session command without cwd", () => {
  const parsed = parseCommandEnvelope(
    JSON.stringify({
      request_id: "req-2",
      command: "resume_session",
      session_id: "session-123",
      launch_settings: {
        language: "German",
        settings: {
          alwaysThinkingEnabled: true,
          model: "haiku",
          permissions: { defaultMode: "plan" },
          fastMode: false,
          effortLevel: "high",
          outputStyle: "Default",
          spinnerTipsEnabled: true,
          terminalProgressBarEnabled: true,
        },
        agent_progress_summaries: true,
      },
    }),
  );
  assert.equal(parsed.requestId, "req-2");
  assert.equal(parsed.command.command, "resume_session");
  if (parsed.command.command !== "resume_session") {
    throw new Error("unexpected command variant");
  }
  assert.equal(parsed.command.session_id, "session-123");
  assert.equal(parsed.command.launch_settings.language, "German");
  assert.deepEqual(parsed.command.launch_settings.settings, {
    alwaysThinkingEnabled: true,
    model: "haiku",
    permissions: { defaultMode: "plan" },
    fastMode: false,
    effortLevel: "high",
    outputStyle: "Default",
    spinnerTipsEnabled: true,
    terminalProgressBarEnabled: true,
  });
  assert.equal(parsed.command.launch_settings.agent_progress_summaries, true);
});

test("parseCommandEnvelope validates rename_session command", () => {
  const parsed = parseCommandEnvelope(
    JSON.stringify({
      request_id: "req-rename",
      command: "rename_session",
      session_id: "session-123",
      title: "Renamed session",
    }),
  );

  assert.equal(parsed.requestId, "req-rename");
  assert.equal(parsed.command.command, "rename_session");
  if (parsed.command.command !== "rename_session") {
    throw new Error("unexpected command variant");
  }
  assert.equal(parsed.command.session_id, "session-123");
  assert.equal(parsed.command.title, "Renamed session");
});

test("parseCommandEnvelope validates generate_session_title command", () => {
  const parsed = parseCommandEnvelope(
    JSON.stringify({
      request_id: "req-generate",
      command: "generate_session_title",
      session_id: "session-123",
      description: "Current custom title",
    }),
  );

  assert.equal(parsed.requestId, "req-generate");
  assert.equal(parsed.command.command, "generate_session_title");
  if (parsed.command.command !== "generate_session_title") {
    throw new Error("unexpected command variant");
  }
  assert.equal(parsed.command.session_id, "session-123");
  assert.equal(parsed.command.description, "Current custom title");
});

test("parseCommandEnvelope validates mcp_toggle command", () => {
  const parsed = parseCommandEnvelope(
    JSON.stringify({
      request_id: "req-mcp-toggle",
      command: "mcp_toggle",
      session_id: "session-123",
      server_name: "notion",
      enabled: false,
    }),
  );

  assert.equal(parsed.requestId, "req-mcp-toggle");
  assert.equal(parsed.command.command, "mcp_toggle");
  if (parsed.command.command !== "mcp_toggle") {
    throw new Error("unexpected command variant");
  }
  assert.equal(parsed.command.session_id, "session-123");
  assert.equal(parsed.command.server_name, "notion");
  assert.equal(parsed.command.enabled, false);
});

test("parseCommandEnvelope validates mcp_set_servers command", () => {
  const parsed = parseCommandEnvelope(
    JSON.stringify({
      request_id: "req-mcp-set",
      command: "mcp_set_servers",
      session_id: "session-123",
      servers: {
        notion: {
          type: "http",
          url: "https://mcp.notion.com/mcp",
          headers: {
            "X-Test": "1",
          },
          timeout: 5000,
          always_load: true,
          tools: [
            {
              name: "search",
            },
            {
              name: "read",
              permission_policy: "always_ask",
            },
            {
              name: "write",
              permission_policy: "always_deny",
              org_max_permission: "ask",
            },
          ],
        },
      },
    }),
  );

  assert.equal(parsed.requestId, "req-mcp-set");
  assert.equal(parsed.command.command, "mcp_set_servers");
  if (parsed.command.command !== "mcp_set_servers") {
    throw new Error("unexpected command variant");
  }
  assert.equal(parsed.command.session_id, "session-123");
  assert.deepEqual(parsed.command.servers, {
    notion: {
      type: "http",
      url: "https://mcp.notion.com/mcp",
      headers: {
        "X-Test": "1",
      },
      timeout: 5000,
      always_load: true,
      tools: [
        {
          name: "search",
        },
        {
          name: "read",
          permission_policy: "always_ask",
        },
        {
          name: "write",
          permission_policy: "always_deny",
          org_max_permission: "ask",
        },
      ],
    },
  });
});

test("parseCommandEnvelope rejects invalid latest MCP config fields", () => {
  assert.throws(
    () =>
      parseCommandEnvelope(
        JSON.stringify({
          command: "mcp_set_servers",
          session_id: "session-123",
          servers: {
            bad: {
              type: "http",
              url: "https://mcp.example.com",
              tools: [{ name: "read", org_max_permission: "deny" }],
            },
          },
        }),
      ),
    /org_max_permission must be one of allow, ask, blocked/,
  );

  assert.throws(
    () =>
      parseCommandEnvelope(
        JSON.stringify({
          command: "mcp_set_servers",
          session_id: "session-123",
          servers: { bad: { type: "http", url: "https://mcp.example.com", timeout: 999 } },
        }),
      ),
    /mcp_set_servers\.servers\.bad\.timeout must be an integer >= 1000/,
  );

  assert.throws(
    () =>
      parseCommandEnvelope(
        JSON.stringify({
          command: "mcp_set_servers",
          session_id: "session-123",
          servers: { bad: { type: "http", url: "https://mcp.example.com", always_load: "yes" } },
        }),
      ),
    /mcp_set_servers\.servers\.bad\.always_load must be a boolean/,
  );

  assert.throws(
    () =>
      parseCommandEnvelope(
        JSON.stringify({
          command: "mcp_set_servers",
          session_id: "session-123",
          servers: {
            bad: {
              type: "http",
              url: "https://mcp.example.com",
              tools: [{ name: "read", permission_policy: "sometimes" }],
            },
          },
        }),
      ),
    /permission_policy must be one of always_allow, always_ask, always_deny/,
  );

  assert.throws(
    () =>
      parseCommandEnvelope(
        JSON.stringify({
          command: "mcp_set_servers",
          session_id: "session-123",
          servers: {
            bad: {
              type: "stdio",
              command: "npx",
              tools: [{ name: "read", permission_policy: "always_allow" }],
            },
          },
        }),
      ),
    /tools is only supported for http and sse MCP servers/,
  );
});

test("handleMcpSetServersCommand emits SDK result", async () => {
  const session = makeSessionState();
  let receivedServers: unknown;
  session.query = {
    setMcpServers: async (servers: unknown) => {
      receivedServers = servers;
      return {
        added: ["docs"],
        removed: ["plugin:Notion:notion"],
        errors: { docs: "connection failed" },
      };
    },
    mcpServerStatus: async () => [
      {
        name: "docs",
        status: "connected",
        config: {
          type: "http",
          url: "https://example.test/mcp",
          alwaysLoad: true,
        },
        tools: [
          {
            name: "read_resource",
            description: "Read docs resources",
          },
        ],
      },
    ],
  } as unknown as import("@anthropic-ai/claude-agent-sdk").Query;

  const events = await captureBridgeEventsAsync(async () => {
    await handleMcpSetServersCommand(
      session,
      {
        command: "mcp_set_servers",
        session_id: "session-1",
        servers: {
          docs: {
            type: "http",
            url: "https://example.test/mcp",
            always_load: true,
          },
        },
      },
      "req-mcp-set",
    );
  });

  assert.deepEqual(receivedServers, {
    docs: {
      type: "http",
      url: "https://example.test/mcp",
      alwaysLoad: true,
    },
  });
  assert.deepEqual(events, [
    {
      request_id: "req-mcp-set",
      event: "mcp_set_servers_result",
      session_id: "session-1",
      result: {
        added: ["docs"],
        removed: ["plugin:Notion:notion"],
        errors: { docs: "connection failed" },
      },
    },
    {
      request_id: "req-mcp-set",
      event: "mcp_snapshot",
      session_id: "session-1",
      source: "mcp_set_servers",
      servers: [
        {
          name: "docs",
          status: "connected",
          config: {
            type: "http",
            url: "https://example.test/mcp",
            always_load: true,
          },
          tools: [
            {
              name: "read_resource",
              description: "Read docs resources",
            },
          ],
        },
      ],
    },
  ]);
});

test("handleMcpSetServersCommand emits MCP operation error on failure", async () => {
  const session = makeSessionState();
  session.query = {
    setMcpServers: async () => {
      throw new Error("dynamic update failed");
    },
  } as unknown as import("@anthropic-ai/claude-agent-sdk").Query;

  const events = await captureBridgeEventsAsync(async () => {
    await handleMcpSetServersCommand(
      session,
      {
        command: "mcp_set_servers",
        session_id: "session-1",
        servers: {},
      },
      "req-mcp-set",
    );
  });

  assert.deepEqual(events, [
    {
      request_id: "req-mcp-set",
      event: "mcp_operation_error",
      session_id: "session-1",
      error: {
        operation: "set-servers",
        message: "dynamic update failed",
      },
    },
    {
      request_id: "req-mcp-set",
      event: "slash_error",
      session_id: "session-1",
      message: "failed to set MCP servers: dynamic update failed",
    },
  ]);
});

test("bridgeMcpConfigToSdk maps latest MCP fields to SDK casing", () => {
  assert.deepEqual(
    bridgeMcpConfigToSdk({
      type: "sse",
      url: "https://mcp.example.com/sse",
      timeout: 2500,
      always_load: true,
      tools: [
        { name: "search" },
        { name: "write", permission_policy: "always_allow", org_max_permission: "blocked" },
      ],
    }),
    {
      type: "sse",
      url: "https://mcp.example.com/sse",
      timeout: 2500,
      alwaysLoad: true,
      tools: [
        { name: "search" },
        { name: "write", permission_policy: "always_allow", org_max_permission: "blocked" },
      ],
    },
  );
});

test("mapMcpServerStatus preserves latest MCP status config fields", () => {
  const mapped = mapMcpServerStatus({
    name: "notion",
    status: "connected",
    config: {
      type: "http",
      url: "https://mcp.notion.com/mcp",
      headers: { Authorization: "Bearer token" },
      timeout: 5000,
      alwaysLoad: true,
      tools: [
        { name: "search" },
        { name: "write", permission_policy: "always_deny", org_max_permission: "ask" },
      ],
    },
    tools: [],
  });

  assert.deepEqual(mapped.config, {
    type: "http",
    url: "https://mcp.notion.com/mcp",
    headers: { Authorization: "Bearer token" },
    timeout: 5000,
    always_load: true,
    tools: [
      { name: "search" },
      { name: "write", permission_policy: "always_deny", org_max_permission: "ask" },
    ],
  });
});

test("mapMcpServerStatusConfig maps unknown config types without throwing", () => {
  const mapped = mapMcpServerStatusConfig({
    type: "future-transport",
    url: "future://server",
  } as unknown as NonNullable<import("@anthropic-ai/claude-agent-sdk").McpServerStatus["config"]>);

  assert.deepEqual(mapped, { type: "unknown", raw_type: "future-transport" });
});

test("parseCommandEnvelope validates reload_plugins command", () => {
  const parsed = parseCommandEnvelope(
    JSON.stringify({
      request_id: "req-reload",
      command: "reload_plugins",
      session_id: "session-123",
    }),
  );

  assert.equal(parsed.requestId, "req-reload");
  assert.deepEqual(parsed.command, {
    command: "reload_plugins",
    session_id: "session-123",
  });
});

test("handleReloadPluginsCommand emits MCP snapshot from reload result", async () => {
  const session = makeSessionState();
  let mcpServerStatusCalls = 0;
  session.query = {
    reloadPlugins: async () => ({
      commands: [],
      agents: [],
      plugins: [],
      mcpServers: [
        {
          name: "docs",
          status: "connected",
          config: {
            type: "http",
            url: "https://example.test/mcp",
          },
          tools: [],
        },
      ],
      error_count: 0,
    }),
    mcpServerStatus: async () => {
      mcpServerStatusCalls += 1;
      throw new Error("mcpServerStatus should not be called");
    },
  } as unknown as import("@anthropic-ai/claude-agent-sdk").Query;

  const events = await captureBridgeEventsAsync(async () => {
    await handleReloadPluginsCommand(session, "req-reload");
  });

  assert.equal(mcpServerStatusCalls, 0);
  assert.deepEqual(
    events.filter((event) => event.event === "mcp_snapshot"),
    [
      {
        request_id: "req-reload",
        event: "mcp_snapshot",
        session_id: "session-1",
        source: "reload_plugins",
        servers: [
          {
            name: "docs",
            status: "connected",
            config: {
              type: "http",
              url: "https://example.test/mcp",
            },
            tools: [],
          },
        ],
      },
    ],
  );
  assert.deepEqual(
    events.filter((event) => event.event === "runtime_reload_completed"),
    [
      {
        request_id: "req-reload",
        event: "runtime_reload_completed",
        session_id: "session-1",
      },
    ],
  );
});

test("handleReloadPluginsCommand revalidates stale MCP auth statuses", async () => {
  const session = makeSessionState();
  const serverName = "reload-auth-revalidation";
  let reloadCalls = 0;
  let mcpServerStatusCalls = 0;
  const reconnectCalls: string[] = [];
  const connectedServer = {
    name: serverName,
    status: "connected" as const,
    config: {
      type: "http" as const,
      url: "https://example.test/mcp",
    },
    tools: [],
  };

  session.query = {
    reloadPlugins: async () => {
      reloadCalls += 1;
      return {
        commands: [],
        agents: [],
        plugins: [],
        mcpServers:
          reloadCalls === 1
            ? [connectedServer]
            : [
                {
                  ...connectedServer,
                  status: "needs-auth" as const,
                },
              ],
        error_count: 0,
      };
    },
    reconnectMcpServer: async (name: string) => {
      reconnectCalls.push(name);
    },
    mcpServerStatus: async () => {
      mcpServerStatusCalls += 1;
      return [connectedServer];
    },
  } as unknown as import("@anthropic-ai/claude-agent-sdk").Query;

  await captureBridgeEventsAsync(async () => {
    await handleReloadPluginsCommand(session, "req-reload-seed");
  });
  const events = await captureBridgeEventsAsync(async () => {
    await handleReloadPluginsCommand(session, "req-reload");
  });

  assert.deepEqual(reconnectCalls, [serverName]);
  assert.equal(mcpServerStatusCalls, 1);
  assert.deepEqual(
    events.filter((event) => event.event === "mcp_snapshot"),
    [
      {
        request_id: "req-reload",
        event: "mcp_snapshot",
        session_id: "session-1",
        source: "reload_plugins",
        servers: [connectedServer],
      },
    ],
  );
});

test("parseCommandEnvelope validates get_context_usage command", () => {
  const parsed = parseCommandEnvelope(
    JSON.stringify({
      request_id: "req-usage",
      command: "get_context_usage",
      session_id: "session-123",
    }),
  );

  assert.equal(parsed.requestId, "req-usage");
  assert.deepEqual(parsed.command, {
    command: "get_context_usage",
    session_id: "session-123",
  });
});

test("staleMcpAuthCandidates selects previously connected servers that regressed to needs-auth", () => {
  const candidates = staleMcpAuthCandidates(
    [
      {
        name: "supabase",
        status: "needs-auth",
        server_info: undefined,
        error: undefined,
        config: undefined,
        scope: undefined,
        tools: [],
      },
      {
        name: "notion",
        status: "needs-auth",
        server_info: undefined,
        error: undefined,
        config: undefined,
        scope: undefined,
        tools: [],
      },
    ],
    new Set(["supabase"]),
    new Map(),
    10_000,
    1_000,
  );

  assert.deepEqual(candidates, ["supabase"]);
});

test("staleMcpAuthCandidates respects the revalidation cooldown", () => {
  const candidates = staleMcpAuthCandidates(
    [
      {
        name: "supabase",
        status: "needs-auth",
        server_info: undefined,
        error: undefined,
        config: undefined,
        scope: undefined,
        tools: [],
      },
    ],
    new Set(["supabase"]),
    new Map([["supabase", 9_500]]),
    10_000,
    1_000,
  );

  assert.deepEqual(candidates, []);
});

test("buildSessionMutationOptions scopes rename requests to the session cwd", () => {
  assert.deepEqual(buildSessionMutationOptions("C:/worktree"), { dir: "C:/worktree" });
  assert.equal(buildSessionMutationOptions(undefined), undefined);
});

test("canGenerateSessionTitle detects supported query objects", () => {
  const query = {
    async generateSessionTitle(): Promise<string> {
      return "Generated";
    },
  } as unknown as import("@anthropic-ai/claude-agent-sdk").Query;

  assert.equal(canGenerateSessionTitle(query), true);
  assert.equal(canGenerateSessionTitle({} as import("@anthropic-ai/claude-agent-sdk").Query), false);
});

test("generatePersistedSessionTitle calls sdk query with persist true", async () => {
  const calls: Array<{ description: string; persist?: boolean }> = [];
  const query = {
    async generateSessionTitle(
      description: string,
      options?: { persist?: boolean },
    ): Promise<string> {
      calls.push({ description, persist: options?.persist });
      return "Generated title";
    },
  } as unknown as import("@anthropic-ai/claude-agent-sdk").Query;

  const title = await generatePersistedSessionTitle(query, "Current summary");

  assert.equal(title, "Generated title");
  assert.deepEqual(calls, [{ description: "Current summary", persist: true }]);
});

test("buildQueryOptions maps launch settings into sdk query options", () => {
  const input = new AsyncQueue<import("@anthropic-ai/claude-agent-sdk").SDKUserMessage>();
  const options = buildQueryOptions({
    cwd: "C:/work",
    launchSettings: {
      language: "German",
      settings: {
        alwaysThinkingEnabled: true,
        model: "haiku",
        permissions: { defaultMode: "plan" },
        fastMode: false,
        effortLevel: "medium",
        outputStyle: "Default",
        spinnerTipsEnabled: true,
        terminalProgressBarEnabled: true,
      },
      agent_progress_summaries: true,
    },
    provisionalSessionId: "session-1",
    input,
    canUseTool: async () => ({ behavior: "deny", message: "not used" }),
    enableSdkDebug: false,
    enableSpawnDebug: false,
    sessionIdForLogs: () => "session-1",
  });

  assert.deepEqual(options.settings, {
    alwaysThinkingEnabled: true,
    model: "haiku",
    permissions: { defaultMode: "plan" },
    fastMode: false,
    effortLevel: "medium",
    outputStyle: "Default",
    spinnerTipsEnabled: true,
    terminalProgressBarEnabled: true,
  });
  assert.deepEqual(options.systemPrompt, {
    type: "preset",
    preset: "claude_code",
    append: `${BRIDGE_RUNTIME_GUARD_PROMPT} ${GERMAN_LANGUAGE_PROMPT}`,
  });
  const _systemPrompt: NonNullable<Options["systemPrompt"]> = options.systemPrompt;
  assert.ok(_systemPrompt);
  assert.equal(options.model, "haiku");
  assert.equal(options.permissionMode, "plan");
  assert.equal("allowDangerouslySkipPermissions" in options, false);
  assert.equal("thinking" in options, false);
  assert.equal("effort" in options, false);
  assert.equal(options.agentProgressSummaries, true);
  assert.equal(options.promptSuggestions, true);
  assert.equal(options.sessionId, "session-1");
  assert.deepEqual(options.settingSources, ["user", "project", "local"]);
  assert.deepEqual(options.toolConfig, {
    askUserQuestion: { previewFormat: "markdown" },
  });
});

test("buildQueryOptions forwards settings and maps startup model and permission mode", () => {
  const input = new AsyncQueue<import("@anthropic-ai/claude-agent-sdk").SDKUserMessage>();
  const options = buildQueryOptions({
    cwd: "C:/work",
    launchSettings: {
      settings: {
        alwaysThinkingEnabled: false,
        permissions: { defaultMode: "default" },
        fastMode: true,
        effortLevel: "high",
        outputStyle: "Learning",
        spinnerTipsEnabled: false,
        terminalProgressBarEnabled: false,
      },
    },
    provisionalSessionId: "session-3",
    input,
    canUseTool: async () => ({ behavior: "deny", message: "not used" }),
    enableSdkDebug: false,
    enableSpawnDebug: false,
    sessionIdForLogs: () => "session-3",
  });

  assert.deepEqual(options.settings, {
    alwaysThinkingEnabled: false,
    permissions: { defaultMode: "default" },
    fastMode: true,
    effortLevel: "high",
    outputStyle: "Learning",
    spinnerTipsEnabled: false,
    terminalProgressBarEnabled: false,
  });
  assert.equal("model" in options, false);
  assert.equal(options.permissionMode, "default");
  assert.equal("allowDangerouslySkipPermissions" in options, false);
  assert.equal("thinking" in options, false);
  assert.equal("effort" in options, false);
});

test("buildQueryOptions trims startup model before passing sdk option", () => {
  const input = new AsyncQueue<import("@anthropic-ai/claude-agent-sdk").SDKUserMessage>();
  const options = buildQueryOptions({
    cwd: "C:/work",
    launchSettings: {
      settings: {
        model: "  claude-opus-4-7  ",
        permissions: { defaultMode: "plan" },
      },
    },
    provisionalSessionId: "session-model",
    input,
    canUseTool: async () => ({ behavior: "deny", message: "not used" }),
    enableSdkDebug: false,
    enableSpawnDebug: false,
    sessionIdForLogs: () => "session-model",
  });

  assert.equal(options.model, "claude-opus-4-7");
  assert.equal(options.permissionMode, "plan");
});

test("buildQueryOptions maps auto startup permission mode", () => {
  const input = new AsyncQueue<import("@anthropic-ai/claude-agent-sdk").SDKUserMessage>();
  const options = buildQueryOptions({
    cwd: "C:/work",
    launchSettings: {
      settings: {
        permissions: { defaultMode: "auto" },
      },
    },
    provisionalSessionId: "session-auto",
    input,
    canUseTool: async () => ({ behavior: "deny", message: "not used" }),
    enableSdkDebug: false,
    enableSpawnDebug: false,
    sessionIdForLogs: () => "session-auto",
  });

  assert.equal(options.permissionMode, "auto");
  assert.equal("allowDangerouslySkipPermissions" in options, false);
});

test("buildQueryOptions enables dangerous skip flag for bypass permissions startup mode", () => {
  const input = new AsyncQueue<import("@anthropic-ai/claude-agent-sdk").SDKUserMessage>();
  const options = buildQueryOptions({
    cwd: "C:/work",
    launchSettings: {
      settings: {
        permissions: { defaultMode: "bypassPermissions" },
      },
    },
    provisionalSessionId: "session-4",
    input,
    canUseTool: async () => ({ behavior: "deny", message: "not used" }),
    enableSdkDebug: false,
    enableSpawnDebug: false,
    sessionIdForLogs: () => "session-4",
  });

  assert.equal(options.permissionMode, "bypassPermissions");
  assert.equal(options.allowDangerouslySkipPermissions, true);
});

test("buildQueryOptions omits optional startup overrides but keeps bridge guard prompt", () => {
  const input = new AsyncQueue<import("@anthropic-ai/claude-agent-sdk").SDKUserMessage>();
  const options = buildQueryOptions({
    cwd: "C:/work",
    launchSettings: {},
    provisionalSessionId: "session-2",
    input,
    canUseTool: async () => ({ behavior: "deny", message: "not used" }),
    enableSdkDebug: false,
    enableSpawnDebug: false,
    sessionIdForLogs: () => "session-2",
  });

  assert.equal("model" in options, false);
  assert.equal("permissionMode" in options, false);
  assert.equal("allowDangerouslySkipPermissions" in options, false);
  assert.deepEqual(options.systemPrompt, {
    type: "preset",
    preset: "claude_code",
    append: BRIDGE_RUNTIME_GUARD_PROMPT,
  });
  assert.equal("agentProgressSummaries" in options, false);
});

test("buildQueryOptions forwards SDK-provided spawn env without passing top-level env", async () => {
  const input = new AsyncQueue<import("@anthropic-ai/claude-agent-sdk").SDKUserMessage>();
  const options = buildQueryOptions({
    cwd: "C:/work",
    launchSettings: {},
    provisionalSessionId: "session-spawn-env",
    input,
    canUseTool: async () => ({ behavior: "deny", message: "not used" }),
    enableSdkDebug: false,
    enableSpawnDebug: false,
    sessionIdForLogs: () => "session-spawn-env",
  });

  assert.equal("env" in options, false);

  const previousParentOnly = process.env.PHASE10_PARENT_ONLY;
  process.env.PHASE10_PARENT_ONLY = "must-not-leak";
  try {
    const child = options.spawnClaudeCodeProcess({
      command: process.execPath,
      args: [
        "-e",
        "process.stdout.write(JSON.stringify({check:process.env.PHASE10_ENV_CHECK??null,parent:process.env.PHASE10_PARENT_ONLY??null}))",
      ],
      cwd: process.cwd(),
      env: { PHASE10_ENV_CHECK: "forwarded" },
      signal: new AbortController().signal,
    });

    let stdout = "";
    child.stdout.setEncoding("utf8");
    child.stdout.on("data", (chunk) => {
      stdout += chunk;
    });

    const exitCode = await new Promise<number | null>((resolve, reject) => {
      child.on("error", reject);
      child.on("exit", (code) => resolve(code));
    });

    assert.equal(exitCode, 0);
    assert.deepEqual(JSON.parse(stdout), { check: "forwarded", parent: null });
  } finally {
    if (previousParentOnly === undefined) {
      delete process.env.PHASE10_PARENT_ONLY;
    } else {
      process.env.PHASE10_PARENT_ONLY = previousParentOnly;
    }
  }
});

test("buildQueryOptions makes sandbox fallback explicit when enabled", () => {
  const input = new AsyncQueue<import("@anthropic-ai/claude-agent-sdk").SDKUserMessage>();
  const options = buildQueryOptions({
    cwd: "C:/work",
    launchSettings: {
      settings: {
        sandbox: {
          enabled: true,
        },
      },
    },
    provisionalSessionId: "session-sandbox",
    input,
    canUseTool: async () => ({ behavior: "deny", message: "not used" }),
    enableSdkDebug: false,
    enableSpawnDebug: false,
    sessionIdForLogs: () => "session-sandbox",
  });

  assert.deepEqual(options.settings, {
    sandbox: {
      enabled: true,
      failIfUnavailable: false,
    },
  });
});

test("buildQueryOptions preserves explicit sandbox failIfUnavailable setting", () => {
  const input = new AsyncQueue<import("@anthropic-ai/claude-agent-sdk").SDKUserMessage>();
  const options = buildQueryOptions({
    cwd: "C:/work",
    launchSettings: {
      settings: {
        sandbox: {
          enabled: true,
          failIfUnavailable: true,
        },
      },
    },
    provisionalSessionId: "session-sandbox-explicit",
    input,
    canUseTool: async () => ({ behavior: "deny", message: "not used" }),
    enableSdkDebug: false,
    enableSpawnDebug: false,
    sessionIdForLogs: () => "session-sandbox-explicit",
  });

  assert.deepEqual(options.settings, {
    sandbox: {
      enabled: true,
      failIfUnavailable: true,
    },
  });
});

test("handleTaskSystemMessage prefers task_progress summary over fallback text", () => {
  const session = makeSessionState();

  const events = captureBridgeEvents(() => {
    handleTaskSystemMessage(session, "task_started", {
      task_id: "task-1",
      tool_use_id: "tool-1",
      description: "Initial task description",
    });
    handleTaskSystemMessage(session, "task_progress", {
      task_id: "task-1",
      summary: "Analyzing authentication flow",
      description: "Should not be shown",
      last_tool_name: "Read",
    });
  });

  const lastEvent = events.at(-1);
  assert.ok(lastEvent);
  assert.equal(lastEvent.event, "session_update");
  assert.deepEqual(lastEvent.update, {
    type: "tool_call_update",
    tool_call_update: {
      tool_call_id: "tool-1",
      fields: {
        status: "in_progress",
        raw_output: "Analyzing authentication flow",
        content: [
          {
            type: "content",
            content: { type: "text", text: "Analyzing authentication flow" },
          },
        ],
      },
    },
  });
});

test("handleTaskSystemMessage falls back to description and last tool when progress summary is absent", () => {
  const session = makeSessionState();

  const events = captureBridgeEvents(() => {
    handleTaskSystemMessage(session, "task_started", {
      task_id: "task-1",
      tool_use_id: "tool-1",
      description: "Initial task description",
    });
    handleTaskSystemMessage(session, "task_progress", {
      task_id: "task-1",
      description: "Inspecting auth code",
      last_tool_name: "Read",
    });
  });

  const lastEvent = events.at(-1);
  assert.ok(lastEvent);
  assert.equal(lastEvent.event, "session_update");
  assert.deepEqual(lastEvent.update, {
    type: "tool_call_update",
    tool_call_update: {
      tool_call_id: "tool-1",
      fields: {
        status: "in_progress",
        raw_output: "Inspecting auth code (last tool: Read)",
        content: [
          {
            type: "content",
            content: { type: "text", text: "Inspecting auth code (last tool: Read)" },
          },
        ],
      },
    },
  });
});

test("handleTaskSystemMessage keeps Agent completed notification provisional until tool result", () => {
  const session = makeSessionState();

  const events = captureBridgeEvents(() => {
    handleTaskSystemMessage(session, "task_started", {
      task_id: "task-1",
      tool_use_id: "tool-1",
      description: "Initial task description",
    });
    handleTaskSystemMessage(session, "task_progress", {
      task_id: "task-1",
      summary: "Analyzing authentication flow",
      description: "Should not be shown",
    });
    handleTaskSystemMessage(session, "task_notification", {
      task_id: "task-1",
      status: "completed",
      summary: "Found the auth bug and prepared the fix",
    });
  });

  const lastEvent = events.at(-1);
  assert.ok(lastEvent);
  assert.equal(lastEvent.event, "session_update");
  assert.deepEqual(lastEvent.update, {
    type: "tool_call_update",
    tool_call_update: {
      tool_call_id: "tool-1",
      fields: {
        raw_output: "Found the auth bug and prepared the fix",
        content: [
          {
            type: "content",
            content: { type: "text", text: "Found the auth bug and prepared the fix" },
          },
        ],
        task_metadata: {
          summary: "Found the auth bug and prepared the fix",
          terminal_status: "completed",
        },
      },
    },
  });
  assert.equal(session.toolCalls.get("tool-1")?.status, "in_progress");
  assert.equal(session.taskToolUseIds.get("task-1"), "tool-1");
  assert.equal(session.taskIdsByToolUseId.get("tool-1"), "task-1");
});

test("emitToolResultUpdate finalizes deferred Agent completion and unlinks lifecycle task", () => {
  const session = makeSessionState();

  const events = captureBridgeEvents(() => {
    handleTaskSystemMessage(session, "task_started", {
      task_id: "task-1",
      tool_use_id: "tool-1",
      description: "Initial task description",
    });
    handleTaskSystemMessage(session, "task_notification", {
      task_id: "task-1",
      status: "completed",
      summary: "Found the auth bug and prepared the fix",
    });
    emitToolResultUpdate(session, "tool-1", false, {
      agentId: "agent-1",
      agentType: "general-purpose",
      resolvedModel: "claude-opus-4-8",
      content: [{ type: "text", text: "Done" }],
      status: "completed",
      prompt: "Review the branch",
    });
  });

  const lastEvent = events.at(-1);
  assert.ok(lastEvent);
  assert.equal(lastEvent.event, "session_update");
  const update = lastEvent.update as Record<string, unknown>;
  assert.equal(update.type, "tool_call_update");
  const toolCallUpdate = update.tool_call_update as Record<string, unknown>;
  const fields = toolCallUpdate.fields as Record<string, unknown>;
  assert.equal(toolCallUpdate.tool_call_id, "tool-1");
  assert.equal(fields.status, "completed");
  assert.deepEqual(fields.output_metadata, {
    agent: {
      resolved_model: "claude-opus-4-8",
    },
  });

  const toolCall = session.toolCalls.get("tool-1");
  assert.equal(toolCall?.status, "completed");
  assert.deepEqual(toolCall?.output_metadata, {
    agent: {
      resolved_model: "claude-opus-4-8",
    },
  });
  assert.equal(session.taskToolUseIds.has("task-1"), false);
  assert.equal(session.taskIdsByToolUseId.has("tool-1"), false);
});

test("handleTaskSystemMessage ignores lifecycle content for concrete output tools", () => {
  const session = makeSessionState();
  const protectedTools = [
    createToolCall("tool-bash", "Bash", { command: "git status" }),
    createToolCall("tool-read", "Read", { file_path: "src/main.rs" }),
    createToolCall("tool-write", "Write", {
      file_path: "src/main.rs",
      content: "updated file contents",
    }),
  ];

  for (const toolCall of protectedTools) {
    toolCall.status = "in_progress";
    toolCall.raw_output = `actual output for ${toolCall.tool_call_id}`;
    session.toolCalls.set(toolCall.tool_call_id, toolCall);
  }

  const events = captureBridgeEvents(() => {
    for (const toolCall of protectedTools) {
      const taskId = `task-${toolCall.tool_call_id}`;
      handleTaskSystemMessage(session, "task_started", {
        task_id: taskId,
        tool_use_id: toolCall.tool_call_id,
        description: "Show working tree status",
      });
      handleTaskSystemMessage(session, "task_notification", {
        task_id: taskId,
        tool_use_id: toolCall.tool_call_id,
        status: "completed",
        summary: "Show diff summary for unstaged changes",
      });
    }
  });

  assert.deepEqual(events, []);
  for (const toolCall of protectedTools) {
    const stored = session.toolCalls.get(toolCall.tool_call_id);
    assert.equal(stored?.status, "in_progress");
    assert.equal(stored?.raw_output, `actual output for ${toolCall.tool_call_id}`);
  }
});

test("handleSdkMessage ignores tool_use_summary for Bash Read and Write tools", () => {
  const session = makeSessionState();
  const protectedTools = [
    createToolCall("tool-bash", "Bash", { command: "git diff" }),
    createToolCall("tool-read", "Read", { file_path: "src/main.rs" }),
    createToolCall("tool-write", "Write", {
      file_path: "src/main.rs",
      content: "updated file contents",
    }),
  ];

  for (const toolCall of protectedTools) {
    toolCall.status = "completed";
    toolCall.raw_output = `actual output for ${toolCall.tool_call_id}`;
    session.toolCalls.set(toolCall.tool_call_id, toolCall);
  }

  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "tool_use_summary",
      summary: "Show commits on this branch since diverging from main",
      preceding_tool_use_ids: protectedTools.map((toolCall) => toolCall.tool_call_id),
      uuid: "message-summary",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  assert.deepEqual(events, []);
  for (const toolCall of protectedTools) {
    assert.equal(
      session.toolCalls.get(toolCall.tool_call_id)?.raw_output,
      `actual output for ${toolCall.tool_call_id}`,
    );
  }
});

test("handleSdkMessage applies tool_use_summary for summary-oriented tools", () => {
  const session = makeSessionState();
  const toolCall = createToolCall("tool-agent", "Agent", { prompt: "Inspect auth flow" });
  session.toolCalls.set(toolCall.tool_call_id, toolCall);

  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "tool_use_summary",
      summary: "Inspected auth flow and found the failing check",
      preceding_tool_use_ids: [toolCall.tool_call_id],
      uuid: "message-summary",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  const lastEvent = events.at(-1);
  assert.ok(lastEvent);
  assert.equal(lastEvent.event, "session_update");
  assert.deepEqual(lastEvent.update, {
    type: "tool_call_update",
    tool_call_update: {
      tool_call_id: "tool-agent",
      fields: {
        status: "completed",
        raw_output: "Inspected auth flow and found the failing check",
        content: [
          {
            type: "content",
            content: { type: "text", text: "Inspected auth flow and found the failing check" },
          },
        ],
      },
    },
  });
  assert.equal(session.toolCalls.get(toolCall.tool_call_id)?.raw_output, "Inspected auth flow and found the failing check");
});

test("handleSdkMessage suppresses ToolSearch bridge events without denying SDK use", () => {
  const session = makeSessionState();

  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "stream_event",
      event: {
        type: "content_block_start",
        content_block: {
          type: "server_tool_use",
          id: "tool-search-1",
          name: "ToolSearch",
          input: { query: "src/" },
        },
      },
      uuid: "message-search-start",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(session, {
      type: "tool_progress",
      tool_use_id: "tool-search-1",
      tool_name: "ToolSearch",
      uuid: "message-search-progress",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(session, {
      type: "user",
      parent_tool_use_id: "tool-search-1",
      tool_use_result: { content: "matched src/main.rs", is_error: false },
      message: {
        role: "user",
        content: [
          {
            type: "tool_search_tool_result",
            tool_use_id: "tool-search-1",
            content: "matched src/main.rs",
            is_error: false,
          },
        ],
      },
      uuid: "message-search-result",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(session, {
      type: "tool_use_summary",
      summary: "Found source files",
      preceding_tool_use_ids: ["tool-search-1"],
      uuid: "message-search-summary",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  assert.deepEqual(events, []);
  assert.equal(session.hiddenToolUseIds.has("tool-search-1"), true);
  assert.equal(session.toolCalls.has("tool-search-1"), false);
});

test("handleSdkMessage emits transcript retraction for model_refusal_fallback", () => {
  const session = makeSessionState();

  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "system",
      subtype: "model_refusal_fallback",
      trigger: "refusal",
      direction: "retry",
      original_model: "claude-opus-4-1",
      fallback_model: "claude-sonnet-4-5",
      request_id: "req-1",
      api_refusal_category: "cyber",
      api_refusal_explanation: "policy text",
      retracted_message_uuids: ["assistant-old", "assistant-old", "", 7],
      content: "Retried with fallback model",
      uuid: "fallback-notice",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  assert.deepEqual(events.map((event) => event.update), [
    {
      type: "transcript_retraction",
      message_uuids: ["assistant-old"],
      reason: "model_refusal_fallback",
      request_id: "req-1",
      trigger: "refusal",
      direction: "retry",
      original_model: "claude-opus-4-1",
      fallback_model: "claude-sonnet-4-5",
      api_refusal_category: "cyber",
      api_refusal_explanation: "policy text",
      content: "Retried with fallback model",
    },
  ]);
  assert.equal(
    events.some(
      (event) =>
        (event.update as Record<string, unknown> | undefined)?.type === "system_notice_update",
    ),
    false,
  );
});

test("handleSdkMessage emits tolerant transcript retraction for model_fallback", () => {
  const session = makeSessionState();

  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "system",
      subtype: "model_fallback",
      original_model: "claude-opus-4-1",
      fallback_model: "claude-sonnet-4-5",
      retracted_message_uuids: ["old-1", "old-2"],
      uuid: "fallback-notice",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  assert.deepEqual(events.map((event) => event.update), [
    {
      type: "transcript_retraction",
      message_uuids: ["old-1", "old-2"],
      reason: "model_fallback",
      original_model: "claude-opus-4-1",
      fallback_model: "claude-sonnet-4-5",
    },
  ]);
});

test("handleSdkMessage emits assistant supersedes before replacement content", () => {
  const session = makeSessionState();

  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "assistant",
      uuid: "assistant-new",
      supersedes: ["assistant-old"],
      session_id: "session-1",
      message: {
        role: "assistant",
        content: [
          { type: "tool_use", id: "tool-1", name: "Bash", input: { command: "echo ok" } },
        ],
      },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  assert.deepEqual(events.map((event) => event.update), [
    {
      type: "transcript_retraction",
      message_uuids: ["assistant-old"],
      reason: "assistant_supersedes",
    },
    {
      type: "tool_call",
      tool_call: {
        tool_call_id: "tool-1",
        title: "echo ok",
        kind: "execute",
        status: "in_progress",
        source_message_uuid: "assistant-new",
        content: [],
        raw_input: { command: "echo ok" },
        locations: [],
        meta: { claudeCode: { toolName: "Bash", parentToolUseId: null } },
      },
    },
  ]);
});

test("handleSdkMessage propagates source UUIDs for stream text and tool results", () => {
  const session = makeSessionState();

  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "stream_event",
      uuid: "assistant-stream",
      session_id: "session-1",
      event: {
        type: "content_block_delta",
        delta: { type: "text_delta", text: "partial" },
      },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(session, {
      type: "stream_event",
      uuid: "assistant-tool",
      session_id: "session-1",
      event: {
        type: "content_block_start",
        content_block: {
          type: "tool_use",
          id: "tool-1",
          name: "Bash",
          input: { command: "echo ok" },
        },
      },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(session, {
      type: "user",
      uuid: "user-result",
      session_id: "session-1",
      parent_tool_use_id: "tool-1",
      message: {
        role: "user",
        content: [
          { type: "tool_result", tool_use_id: "tool-1", content: "ok", is_error: false },
        ],
      },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  assert.deepEqual(
    events.map((event) => event.update),
    [
      {
        type: "agent_message_chunk",
        content: { type: "text", text: "partial" },
        source_message_uuid: "assistant-stream",
      },
      {
        type: "tool_call",
        tool_call: {
          tool_call_id: "tool-1",
          title: "echo ok",
          kind: "execute",
          status: "in_progress",
          source_message_uuid: "assistant-tool",
          content: [],
          raw_input: { command: "echo ok" },
          locations: [],
          meta: { claudeCode: { toolName: "Bash", parentToolUseId: null } },
        },
      },
      {
        type: "tool_call_update",
        tool_call_update: {
          tool_call_id: "tool-1",
          source_message_uuid: "user-result",
          fields: {
            status: "completed",
            raw_output: "ok",
            content: [{ type: "content", content: { type: "text", text: "ok" } }],
          },
        },
      },
    ],
  );
});

test("handleSdkMessage refreshes Grep title when final assistant snapshot carries input", () => {
  const session = makeSessionState();

  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "stream_event",
      uuid: "assistant-stream",
      session_id: "session-1",
      event: {
        type: "content_block_start",
        content_block: {
          type: "tool_use",
          id: "tool-grep",
          name: "Grep",
          input: {},
        },
      },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(session, {
      type: "assistant",
      uuid: "assistant-final",
      session_id: "session-1",
      message: {
        role: "assistant",
        content: [
          {
            type: "tool_use",
            id: "tool-grep",
            name: "Grep",
            input: { pattern: "<rare string>", output_mode: "content", "-n": true },
          },
        ],
      },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  assert.deepEqual(events.map((event) => event.update), [
    {
      type: "tool_call",
      tool_call: {
        tool_call_id: "tool-grep",
        title: "Grep",
        kind: "search",
        status: "in_progress",
        source_message_uuid: "assistant-stream",
        content: [],
        raw_input: {},
        locations: [],
        meta: { claudeCode: { toolName: "Grep", parentToolUseId: null } },
      },
    },
    {
      type: "tool_call_update",
      tool_call_update: {
        tool_call_id: "tool-grep",
        source_message_uuid: "assistant-final",
        fields: {
          title: "Grep <rare string> (content)",
          kind: "search",
          status: "in_progress",
          raw_input: { pattern: "<rare string>", output_mode: "content", "-n": true },
          locations: [],
          meta: { claudeCode: { toolName: "Grep", parentToolUseId: null } },
        },
      },
    },
  ]);
});

test("handleSdkMessage refreshes Agent title when final assistant snapshot carries input", () => {
  const session = makeSessionState();

  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "stream_event",
      uuid: "assistant-stream",
      session_id: "session-1",
      event: {
        type: "content_block_start",
        content_block: {
          type: "tool_use",
          id: "tool-agent",
          name: "Agent",
          input: {},
        },
      },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(session, {
      type: "assistant",
      uuid: "assistant-final",
      session_id: "session-1",
      message: {
        role: "assistant",
        content: [
          {
            type: "tool_use",
            id: "tool-agent",
            name: "Agent",
            input: {
              description: "review changes",
              prompt: "Review the branch",
              name: "review-worker",
              subagent_type: "general-purpose",
              model: "opus",
            },
          },
        ],
      },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  assert.deepEqual(events.map((event) => event.update), [
    {
      type: "tool_call",
      tool_call: {
        tool_call_id: "tool-agent",
        title: "Agent",
        kind: "think",
        status: "in_progress",
        source_message_uuid: "assistant-stream",
        content: [],
        raw_input: {},
        locations: [],
        meta: { claudeCode: { toolName: "Agent", parentToolUseId: null } },
      },
    },
    {
      type: "tool_call_update",
      tool_call_update: {
        tool_call_id: "tool-agent",
        source_message_uuid: "assistant-final",
        fields: {
          title: "Agent: review-worker",
          kind: "think",
          status: "in_progress",
          raw_input: {
            description: "review changes",
            prompt: "Review the branch",
            name: "review-worker",
            subagent_type: "general-purpose",
            model: "opus",
          },
          locations: [],
          meta: { claudeCode: { toolName: "Agent", parentToolUseId: null } },
        },
      },
    },
  ]);
  assert.deepEqual(session.toolCalls.get("tool-agent")?.raw_input, {
    description: "review changes",
    prompt: "Review the branch",
    name: "review-worker",
    subagent_type: "general-purpose",
    model: "opus",
  });
});

test("handleTaskSystemMessage applies task_updated description patches to the linked task", () => {
  const session = makeSessionState();

  const events = captureBridgeEvents(() => {
    handleTaskSystemMessage(session, "task_started", {
      task_id: "task-1",
      tool_use_id: "tool-1",
      description: "Initial task description",
    });
    handleTaskSystemMessage(session, "task_updated", {
      task_id: "task-1",
      patch: {
        status: "running",
        description: "Refining the migration plan",
        is_backgrounded: true,
      },
    });
  });

  const lastEvent = events.at(-1);
  assert.ok(lastEvent);
  assert.equal(lastEvent.event, "session_update");
  assert.deepEqual(lastEvent.update, {
    type: "tool_call_update",
    tool_call_update: {
      tool_call_id: "tool-1",
      fields: {
        status: "in_progress",
        raw_output: "Refining the migration plan",
        content: [
          {
            type: "content",
            content: { type: "text", text: "Refining the migration plan" },
          },
        ],
        task_metadata: {
          is_backgrounded: true,
        },
      },
    },
  });
});

test("handleTaskSystemMessage uses task_updated terminal error text when description is absent", () => {
  const session = makeSessionState();

  const events = captureBridgeEvents(() => {
    handleTaskSystemMessage(session, "task_started", {
      task_id: "task-1",
      tool_use_id: "tool-1",
      description: "Initial task description",
    });
    handleTaskSystemMessage(session, "task_updated", {
      task_id: "task-1",
      patch: {
        status: "killed",
        error: "Task stopped by parent agent",
        end_time: 1234,
        total_paused_ms: 250,
      },
    });
  });

  const lastEvent = events.at(-1);
  assert.ok(lastEvent);
  assert.equal(lastEvent.event, "session_update");
  assert.deepEqual(lastEvent.update, {
    type: "tool_call_update",
    tool_call_update: {
      tool_call_id: "tool-1",
      fields: {
        status: "killed",
        raw_output: "Task stopped by parent agent",
        content: [
          {
            type: "content",
            content: { type: "text", text: "Task stopped by parent agent" },
          },
        ],
        task_metadata: {
          error: "Task stopped by parent agent",
          end_time: 1234,
          terminal_status: "killed",
          total_paused_ms: 250,
        },
      },
    },
  });
});

test("handleTaskSystemMessage merges task metadata patches into the linked task state", () => {
  const session = makeSessionState();

  captureBridgeEvents(() => {
    handleTaskSystemMessage(session, "task_started", {
      task_id: "task-1",
      tool_use_id: "tool-1",
      description: "Initial task description",
    });
    handleTaskSystemMessage(session, "task_updated", {
      task_id: "task-1",
      patch: {
        status: "running",
        is_backgrounded: true,
      },
    });
    handleTaskSystemMessage(session, "task_updated", {
      task_id: "task-1",
      patch: {
        error: "Task stopped by parent agent",
        end_time: 1234,
      },
    });
  });

  assert.deepEqual(session.toolCalls.get("tool-1")?.task_metadata, {
    is_backgrounded: true,
    error: "Task stopped by parent agent",
    end_time: 1234,
  });
});

test("Monitor launch links task id and accepts task lifecycle updates", () => {
  const session = makeSessionState();

  captureBridgeEvents(() => {
    emitToolCall(session, "tool-monitor", "Monitor", {
      description: "watch deploy logs",
      timeout_ms: 30000,
      persistent: false,
      command: "tail -f deploy.log",
    });
    emitToolResultUpdate(session, "tool-monitor", false, {
      taskId: "monitor-1",
      timeoutMs: 30000,
      persistent: false,
    });
  });

  assert.equal(session.taskToolUseIds.get("monitor-1"), "tool-monitor");
  assert.equal(session.taskIdsByToolUseId.get("tool-monitor"), "monitor-1");
  assert.equal(session.toolCalls.get("tool-monitor")?.status, "in_progress");

  captureBridgeEvents(() => {
    handleTaskSystemMessage(session, "task_updated", {
      task_id: "monitor-1",
      patch: {
        status: "running",
        description: "Monitor observed deploy log output",
        is_backgrounded: true,
      },
    });
  });

  const toolCall = session.toolCalls.get("tool-monitor");
  assert.equal(toolCall?.status, "in_progress");
  assert.equal(toolCall?.raw_output, "Monitor observed deploy log output");
  assert.equal(toolCall?.task_metadata?.is_backgrounded, true);
  assert.equal(session.taskToolUseIds.get("monitor-1"), "tool-monitor");
  assert.equal(session.taskIdsByToolUseId.get("tool-monitor"), "monitor-1");
});

test("Monitor launch stays in progress after successful assistant turn until final lifecycle notification", () => {
  const session = makeSessionState();

  captureBridgeEvents(() => {
    emitToolCall(session, "tool-monitor", "Monitor", {
      description: "watch deploy logs",
      timeout_ms: 30000,
      persistent: false,
      command: "tail -f deploy.log",
    });
    emitToolResultUpdate(session, "tool-monitor", false, {
      taskId: "monitor-1",
      timeoutMs: 30000,
      persistent: false,
    });
    handleResultMessage(session, {
      type: "result",
      subtype: "success",
      terminal_reason: "completed",
    });
  });

  assert.equal(session.toolCalls.get("tool-monitor")?.status, "in_progress");
  assert.equal(session.taskToolUseIds.get("monitor-1"), "tool-monitor");
  assert.equal(session.taskIdsByToolUseId.get("tool-monitor"), "monitor-1");

  captureBridgeEvents(() => {
    handleTaskSystemMessage(session, "task_notification", {
      task_id: "monitor-1",
      tool_use_id: "tool-monitor",
      status: "completed",
      output_file: "C:/tmp/monitor-1.output",
      summary: "Monitor completed",
    });
  });

  const toolCall = session.toolCalls.get("tool-monitor");
  assert.equal(toolCall?.status, "completed");
  assert.equal(toolCall?.raw_output, "Monitor completed");
  assert.equal(toolCall?.task_metadata?.output_file, "C:/tmp/monitor-1.output");
  assert.equal(session.taskToolUseIds.has("monitor-1"), false);
  assert.equal(session.taskIdsByToolUseId.has("tool-monitor"), false);
});

test("Workflow task notifications finish the linked root tool", () => {
  for (const [sdkStatus, expectedStatus] of [
    ["completed", "completed"],
    ["stopped", "killed"],
    ["failed", "failed"],
  ] as const) {
    const session = makeSessionState();
    captureBridgeEvents(() => {
      emitToolCall(session, `tool-workflow-${sdkStatus}`, "Workflow", {
        name: "spec",
      });
      emitToolResultUpdate(session, `tool-workflow-${sdkStatus}`, false, {
        status: "async_launched",
        taskId: `workflow-${sdkStatus}`,
        runId: `run-${sdkStatus}`,
      });
      handleTaskSystemMessage(session, "task_notification", {
        task_id: `workflow-${sdkStatus}`,
        status: sdkStatus,
        output_file: `C:/tmp/workflow-${sdkStatus}.output`,
        summary: `Workflow ${sdkStatus}`,
      });
    });

    const toolCall = session.toolCalls.get(`tool-workflow-${sdkStatus}`);
    assert.equal(toolCall?.status, expectedStatus);
    assert.equal(toolCall?.raw_output, `Workflow ${sdkStatus}`);
    assert.equal(toolCall?.task_metadata?.output_file, `C:/tmp/workflow-${sdkStatus}.output`);
    assert.equal(toolCall?.task_metadata?.summary, `Workflow ${sdkStatus}`);
    assert.equal(toolCall?.task_metadata?.terminal_status, sdkStatus);
    assert.equal(session.taskToolUseIds.has(`workflow-${sdkStatus}`), false);
    assert.equal(session.taskIdsByToolUseId.has(`tool-workflow-${sdkStatus}`), false);
  }
});

test("TaskCreate output emits task state and links lifecycle task id", () => {
  const session = makeSessionState();

  const events = captureBridgeEvents(() => {
    emitToolCall(session, "tool-create", "TaskCreate", {
      subject: "Audit state",
      description: "Check task reducer",
      activeForm: "Auditing state",
      metadata: { phase: "6A" },
    });
    emitToolResultUpdate(session, "tool-create", false, {
      task: { id: "task-1", subject: "Audit state" },
    });
  });

  const updates = events.map((event) => event.update as Record<string, unknown>);
  assert.equal(updates.some((update) => update.type === "tool_call"), true);
  const toolResult = updates.find((update) => {
    const toolCallUpdate = update.tool_call_update as Record<string, unknown> | undefined;
    return toolCallUpdate?.tool_call_id === "tool-create";
  })?.tool_call_update as Record<string, unknown> | undefined;
  const resultFields = toolResult?.fields as Record<string, unknown> | undefined;
  assert.equal(resultFields?.status, "completed");
  assert.equal(Object.hasOwn(resultFields ?? {}, "content"), false);
  assert.equal(Object.hasOwn(resultFields ?? {}, "raw_output"), false);

  const taskUpdate = updates.find((update) => update.type === "task_state_update");
  assert.deepEqual(taskUpdate, {
    type: "task_state_update",
    source: "task_create",
    tasks: [
      {
        task_id: "task-1",
        subject: "Audit state",
        description: "Check task reducer",
        active_form: "Auditing state",
        status: "pending",
        blocks: [],
        blocked_by: [],
        metadata: { phase: "6A" },
        source_tool_call_id: "tool-create",
      },
    ],
    removed_task_ids: [],
    is_complete_snapshot: false,
  });
  assert.equal(session.taskToolUseIds.get("task-1"), "tool-create");
});

test("TaskCreate transcript toolUseResult object emits typed task state", () => {
  const session = makeSessionState();

  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "assistant",
      message: {
        role: "assistant",
        content: [
          {
            type: "tool_use",
            id: "tool-create-transcript",
            name: "TaskCreate",
            input: {
              subject: "Scaffold Next.js app",
              description: "Run create-next-app",
              activeForm: "Scaffolding Next.js app",
            },
          },
        ],
      },
      uuid: "message-task-create",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(session, {
      type: "user",
      message: {
        role: "user",
        content: [
          {
            type: "tool_result",
            tool_use_id: "tool-create-transcript",
            content: "Task #1 created successfully: Scaffold Next.js app",
          },
        ],
      },
      toolUseResult: { task: { id: "1", subject: "Scaffold Next.js app" } },
      uuid: "message-task-create-result",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  const updates = events.map((event) => event.update as Record<string, unknown>);
  const result = updates.find((update) => {
    const toolCallUpdate = update.tool_call_update as Record<string, unknown> | undefined;
    return toolCallUpdate?.tool_call_id === "tool-create-transcript";
  })?.tool_call_update as Record<string, unknown> | undefined;
  const resultFields = result?.fields as Record<string, unknown> | undefined;
  assert.equal(resultFields?.status, "completed");
  assert.equal(Object.hasOwn(resultFields ?? {}, "content"), false);
  assert.equal(Object.hasOwn(resultFields ?? {}, "raw_output"), false);

  assert.deepEqual(updates.find((update) => update.type === "task_state_update"), {
    type: "task_state_update",
    source: "task_create",
    tasks: [
      {
        task_id: "1",
        subject: "Scaffold Next.js app",
        description: "Run create-next-app",
        active_form: "Scaffolding Next.js app",
        status: "pending",
        blocks: [],
        blocked_by: [],
        source_tool_call_id: "tool-create-transcript",
      },
    ],
    removed_task_ids: [],
    is_complete_snapshot: false,
  });
});

test("TodoWrite tool use remains generic and emits no plan or task state", () => {
  const session = makeSessionState();

  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "assistant",
      message: {
        role: "assistant",
        content: [
          {
            type: "tool_use",
            id: "tool-todo",
            name: "TodoWrite",
            input: {
              todos: [
                {
                  content: "Legacy todo",
                  status: "in_progress",
                  activeForm: "Working legacy todo",
                },
              ],
            },
          },
        ],
      },
      uuid: "message-todo",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  const updates = events.map((event) => event.update as Record<string, unknown>);
  assert.equal(updates.some((update) => update.type === "tool_call"), true);
  assert.equal(updates.some((update) => update.type === "plan"), false);
  assert.equal(updates.some((update) => update.type === "task_state_update"), false);
  assert.equal(session.tasksById.size, 0);
});

test("TaskUpdate success patches one task by task id", () => {
  const session = makeSessionState();
  session.tasksById.set("task-1", {
    task_id: "task-1",
    subject: "Old",
    status: "pending",
    blocks: [],
    blocked_by: [],
  });
  session.taskOrder.push("task-1");

  const events = captureBridgeEvents(() => {
    emitToolCall(session, "tool-update", "TaskUpdate", {
      taskId: "task-1",
      subject: "New",
      status: "in_progress",
      addBlocks: ["task-2"],
      metadata: { mode: "patch" },
    });
    emitToolResultUpdate(session, "tool-update", false, {
      success: true,
      taskId: "task-1",
      updatedFields: ["subject", "status", "addBlocks", "metadata"],
    });
  });

  const updates = events.map((event) => event.update as Record<string, unknown>);
  const result = updates.find((update) => {
    const toolCallUpdate = update.tool_call_update as Record<string, unknown> | undefined;
    return toolCallUpdate?.tool_call_id === "tool-update";
  })?.tool_call_update as Record<string, unknown> | undefined;
  const resultFields = result?.fields as Record<string, unknown> | undefined;
  assert.equal(resultFields?.status, "completed");
  assert.equal(Object.hasOwn(resultFields ?? {}, "content"), false);
  assert.equal(Object.hasOwn(resultFields ?? {}, "raw_output"), false);

  const taskUpdate = events
    .map((event) => event.update as Record<string, unknown>)
    .find((update) => update.type === "task_state_update");
  assert.deepEqual(taskUpdate, {
    type: "task_state_update",
    source: "task_update",
    tasks: [
      {
        task_id: "task-1",
        subject: "New",
        status: "in_progress",
        blocks: ["task-2"],
        blocked_by: [],
        metadata: { mode: "patch" },
      },
    ],
    removed_task_ids: [],
    is_complete_snapshot: false,
  });
});

test("TaskUpdate title uses known subject when input only has task id", () => {
  const session = makeSessionState();
  session.tasksById.set("task-1", {
    task_id: "task-1",
    subject: "Scaffold Next.js app via create-next-app CLI",
    status: "pending",
    blocks: [],
    blocked_by: [],
  });
  session.taskOrder.push("task-1");

  const events = captureBridgeEvents(() => {
    emitToolCall(session, "tool-update-title", "TaskUpdate", {
      taskId: "task-1",
      status: "in_progress",
    });
  });

  const toolCall = events
    .map((event) => event.update as Record<string, unknown>)
    .find((update) => update.type === "tool_call")?.tool_call as Record<string, unknown> | undefined;
  assert.equal(toolCall?.title, "Update task: Scaffold Next.js app via create-next-app CLI");
});

test("TaskOutput renders structured fields and deduplicates XML content without mutating task state", () => {
  const session = makeSessionState();
  session.tasksById.set("task-1", {
    task_id: "task-1",
    subject: "Watch build",
    status: "in_progress",
    blocks: [],
    blocked_by: [],
  });
  session.taskOrder.push("task-1");

  const events = captureBridgeEvents(() => {
    emitToolCall(session, "tool-output", "TaskOutput", {
      task_id: "task-1",
      block: true,
      timeout: 1000,
    });
    emitToolResultUpdate(
      session,
      "tool-output",
      false,
      "<retrieval_status>not_ready</retrieval_status>\n\n<task_id>task-1</task_id>\n\n<task_type>local_bash</task_type>\n\n<status>running</status>",
      {
        retrieval_status: "not_ready",
        task: {
          task_id: "task-1",
          task_type: "local_bash",
          status: "running",
          description: "Run a ticking loop in the background",
          output: "",
          exitCode: null,
        },
      },
    );
  });

  const updates = events.map((event) => event.update as Record<string, unknown>);
  const toolCall = updates.find((update) => update.type === "tool_call")?.tool_call as
    | Record<string, unknown>
    | undefined;
  assert.equal(toolCall?.kind, "other");
  assert.equal(toolCall?.title, "Task output: Watch build");

  const result = updates.find((update) => {
    const toolCallUpdate = update.tool_call_update as Record<string, unknown> | undefined;
    return toolCallUpdate?.tool_call_id === "tool-output";
  })?.tool_call_update as Record<string, unknown> | undefined;
  const fields = result?.fields as Record<string, unknown> | undefined;
  assert.equal(fields?.status, "completed");
  assert.equal(Object.hasOwn(fields ?? {}, "raw_output"), false);
  assert.deepEqual(fields?.content, [
    {
      type: "content",
      content: {
        type: "text",
        text: "Retrieval status: not ready\nTask type: local bash\nStatus: running\nDescription: Run a ticking loop in the background",
      },
    },
  ]);
  const text = (((fields?.content as Array<Record<string, unknown>> | undefined)?.[0]?.content as
    | Record<string, unknown>
    | undefined)?.text ?? "") as string;
  assert.equal(text.includes("<retrieval_status>"), false);
  assert.equal(text.includes("Task ID: task-1"), false);
  assert.equal(updates.some((update) => update.type === "task_state_update"), false);
  assert.equal(session.tasksById.get("task-1")?.status, "in_progress");
});

test("TaskOutput parses XML leaf fields when structured result is unavailable", () => {
  const session = makeSessionState();

  const events = captureBridgeEvents(() => {
    emitToolCall(session, "tool-output-xml", "TaskOutput", {
      task_id: "task-xml",
      block: false,
      timeout: 4000,
    });
    emitToolResultUpdate(
      session,
      "tool-output-xml",
      false,
      "<retrieval_status>not_ready</retrieval_status>\n\n<task_id>task-xml</task_id>\n\n<task_type>local_bash</task_type>\n\n<status>running</status>",
    );
  });

  const result = events
    .map((event) => event.update as Record<string, unknown>)
    .find((update) => {
      const toolCallUpdate = update.tool_call_update as Record<string, unknown> | undefined;
      return toolCallUpdate?.tool_call_id === "tool-output-xml";
    })?.tool_call_update as Record<string, unknown> | undefined;
  const fields = result?.fields as Record<string, unknown> | undefined;
  assert.equal(fields?.status, "completed");
  assert.equal(Object.hasOwn(fields ?? {}, "raw_output"), false);
  assert.deepEqual(fields?.content, [
    {
      type: "content",
      content: {
        type: "text",
        text: "Retrieval status: not ready\nTask type: local bash\nStatus: running",
      },
    },
  ]);
});

test("TaskStop renders structured output and marks the task terminal", () => {
  const session = makeSessionState();
  session.tasksById.set("task-1", {
    task_id: "task-1",
    subject: "Watch build",
    status: "in_progress",
    blocks: [],
    blocked_by: [],
  });
  session.taskOrder.push("task-1");
  linkTaskToolUse(session, "task-1", "tool-agent");

  const events = captureBridgeEvents(() => {
    emitToolCall(session, "tool-stop", "TaskStop", {
      task_id: "task-1",
    });
    emitToolResultUpdate(session, "tool-stop", false, {
      message: "Stopped task",
      task_id: "task-1",
      task_type: "bash",
      command: "npm run watch",
    });
  });

  const updates = events.map((event) => event.update as Record<string, unknown>);
  const toolCall = updates.find((update) => update.type === "tool_call")?.tool_call as
    | Record<string, unknown>
    | undefined;
  assert.equal(toolCall?.kind, "other");
  assert.equal(toolCall?.title, "Stop task: Watch build");

  const result = updates.find((update) => {
    const toolCallUpdate = update.tool_call_update as Record<string, unknown> | undefined;
    return toolCallUpdate?.tool_call_id === "tool-stop";
  })?.tool_call_update as Record<string, unknown> | undefined;
  const fields = result?.fields as Record<string, unknown> | undefined;
  assert.equal(fields?.status, "completed");
  assert.equal(Object.hasOwn(fields ?? {}, "raw_output"), false);
  assert.deepEqual(fields?.content, [
    {
      type: "content",
      content: {
        type: "text",
        text: "Message: Stopped task\nTask ID: task-1\nTask type: bash\nCommand: npm run watch",
      },
    },
  ]);

  assert.deepEqual(updates.find((update) => update.type === "task_state_update"), {
    type: "task_state_update",
    source: "task_lifecycle",
    tasks: [
      {
        task_id: "task-1",
        subject: "Watch build",
        status: "completed",
        blocks: [],
        blocked_by: [],
        metadata: {
          terminal_status: "stopped",
          task_type: "bash",
          command: "npm run watch",
        },
        source_tool_call_id: "tool-agent",
      },
    ],
    removed_task_ids: [],
    is_complete_snapshot: false,
  });
  assert.equal(session.taskToolUseIds.has("task-1"), false);
});

test("TaskStop result for an already-gone task does not create stale task state", () => {
  const session = makeSessionState();

  const events = captureBridgeEvents(() => {
    emitToolCall(session, "tool-stop-missing", "TaskStop", {
      task_id: "task-missing",
    });
    emitToolResultUpdate(session, "tool-stop-missing", false, {
      message: "Task was already stopped",
      task_id: "task-missing",
      task_type: "bash",
      command: "npm run watch",
    });
  });

  const updates = events.map((event) => event.update as Record<string, unknown>);
  const result = updates.find((update) => {
    const toolCallUpdate = update.tool_call_update as Record<string, unknown> | undefined;
    return toolCallUpdate?.tool_call_id === "tool-stop-missing";
  })?.tool_call_update as Record<string, unknown> | undefined;
  const fields = result?.fields as Record<string, unknown> | undefined;
  assert.equal(fields?.status, "completed");
  assert.equal(updates.some((update) => update.type === "task_state_update"), false);
  assert.equal(session.tasksById.has("task-missing"), false);
});

test("TaskUpdate in-progress result leaves activity rendering to app task state", () => {
  const session = makeSessionState();
  session.tasksById.set("task-1", {
    task_id: "task-1",
    subject: "Scaffold Next.js app via create-next-app CLI",
    active_form: "Scaffolding Next.js app",
    status: "pending",
    blocks: [],
    blocked_by: [],
  });
  session.taskOrder.push("task-1");

  const events = captureBridgeEvents(() => {
    emitToolCall(session, "tool-activity", "TaskUpdate", {
      taskId: "task-1",
      status: "in_progress",
    });
    emitToolResultUpdate(session, "tool-activity", false, {
      success: true,
      taskId: "task-1",
      updatedFields: ["status"],
    });
  });

  const updates = events.map((event) => event.update as Record<string, unknown>);
  const result = updates.find((update) => {
    const toolCallUpdate = update.tool_call_update as Record<string, unknown> | undefined;
    return toolCallUpdate?.tool_call_id === "tool-activity";
  })?.tool_call_update as Record<string, unknown> | undefined;
  const fields = result?.fields as Record<string, unknown> | undefined;
  assert.equal(fields?.status, "completed");
  assert.equal(Object.hasOwn(fields ?? {}, "raw_output"), false);
  assert.equal(Object.hasOwn(fields ?? {}, "content"), false);
  const taskUpdate = updates.find((update) => update.type === "task_state_update");
  assert.deepEqual(taskUpdate, {
    type: "task_state_update",
    source: "task_update",
    tasks: [
      {
        task_id: "task-1",
        subject: "Scaffold Next.js app via create-next-app CLI",
        active_form: "Scaffolding Next.js app",
        status: "in_progress",
        blocks: [],
        blocked_by: [],
      },
    ],
    removed_task_ids: [],
    is_complete_snapshot: false,
  });
});

test("TaskUpdate in-progress result omits activity when none is known", () => {
  const session = makeSessionState();
  session.tasksById.set("task-1", {
    task_id: "task-1",
    subject: "Scaffold Next.js app",
    status: "pending",
    blocks: [],
    blocked_by: [],
  });
  session.taskOrder.push("task-1");

  const events = captureBridgeEvents(() => {
    emitToolCall(session, "tool-no-activity", "TaskUpdate", {
      taskId: "task-1",
      status: "in_progress",
    });
    emitToolResultUpdate(session, "tool-no-activity", false, {
      success: true,
      taskId: "task-1",
      updatedFields: ["status"],
    });
  });

  const result = events
    .map((event) => event.update as Record<string, unknown>)
    .find((update) => {
      const toolCallUpdate = update.tool_call_update as Record<string, unknown> | undefined;
      return toolCallUpdate?.tool_call_id === "tool-no-activity";
    })?.tool_call_update as Record<string, unknown> | undefined;
  const fields = result?.fields as Record<string, unknown> | undefined;
  assert.equal(fields?.status, "completed");
  assert.equal(Object.hasOwn(fields ?? {}, "content"), false);
  assert.equal(Object.hasOwn(fields ?? {}, "raw_output"), false);
});

test("TaskUpdate deleted removes task without persisting deleted status", () => {
  const session = makeSessionState();
  session.tasksById.set("task-1", {
    task_id: "task-1",
    subject: "Delete me",
    status: "pending",
    blocks: [],
    blocked_by: [],
  });
  session.taskOrder.push("task-1");

  const events = captureBridgeEvents(() => {
    emitToolCall(session, "tool-delete", "TaskUpdate", {
      taskId: "task-1",
      status: "deleted",
    });
    emitToolResultUpdate(session, "tool-delete", false, {
      success: true,
      taskId: "task-1",
      updatedFields: ["status"],
    });
  });

  const updates = events.map((event) => event.update as Record<string, unknown>);
  const toolResult = updates.find((update) => {
    const toolCallUpdate = update.tool_call_update as Record<string, unknown> | undefined;
    return toolCallUpdate?.tool_call_id === "tool-delete";
  })?.tool_call_update as Record<string, unknown> | undefined;
  const resultFields = toolResult?.fields as Record<string, unknown> | undefined;
  assert.equal(resultFields?.status, "completed");
  assert.equal(Object.hasOwn(resultFields ?? {}, "raw_output"), false);
  assert.equal(Object.hasOwn(resultFields ?? {}, "content"), false);
  assert.deepEqual(updates.find((update) => update.type === "task_state_update"), {
    type: "task_state_update",
    source: "task_update",
    tasks: [],
    removed_task_ids: ["task-1"],
    is_complete_snapshot: false,
  });
  assert.equal(session.tasksById.has("task-1"), false);
});

test("failed TaskUpdate output renders failure but does not mutate task state", () => {
  const session = makeSessionState();
  session.tasksById.set("task-1", {
    task_id: "task-1",
    subject: "Stable",
    status: "pending",
    blocks: [],
    blocked_by: [],
  });
  session.taskOrder.push("task-1");

  const events = captureBridgeEvents(() => {
    emitToolCall(session, "tool-failed-update", "TaskUpdate", {
      taskId: "task-1",
      status: "completed",
    });
    emitToolResultUpdate(session, "tool-failed-update", false, {
      success: false,
      taskId: "task-1",
      updatedFields: [],
      error: "Task missing",
    });
  });

  const updates = events.map((event) => event.update as Record<string, unknown>);
  assert.equal(updates.some((update) => update.type === "task_state_update"), false);
  const toolResult = updates.find((update) => {
    const toolCallUpdate = update.tool_call_update as Record<string, unknown> | undefined;
    return toolCallUpdate?.tool_call_id === "tool-failed-update";
  })?.tool_call_update as Record<string, unknown> | undefined;
  assert.equal((toolResult?.fields as Record<string, unknown> | undefined)?.status, "failed");
  assert.equal(session.tasksById.get("task-1")?.status, "pending");
});

test("TaskList complete snapshot preserves richer retained fields", () => {
  const session = makeSessionState();
  session.tasksById.set("task-1", {
    task_id: "task-1",
    subject: "Existing",
    description: "Keep this",
    active_form: "Working",
    status: "in_progress",
    blocks: ["task-9"],
    blocked_by: [],
  });
  session.tasksById.set("task-2", {
    task_id: "task-2",
    subject: "Removed",
    status: "pending",
    blocks: [],
    blocked_by: [],
  });
  session.taskOrder.push("task-1", "task-2");

  const events = captureBridgeEvents(() => {
    emitToolCall(session, "tool-list", "TaskList", {});
    emitToolResultUpdate(session, "tool-list", false, {
      tasks: [
        {
          id: "task-1",
          subject: "Listed",
          status: "completed",
          owner: "agent",
          blockedBy: ["task-3"],
        },
      ],
    });
  });

  const taskUpdate = events
    .map((event) => event.update as Record<string, unknown>)
    .find((update) => update.type === "task_state_update");
  assert.deepEqual(taskUpdate, {
    type: "task_state_update",
    source: "task_list",
    tasks: [
      {
        task_id: "task-1",
        subject: "Listed",
        description: "Keep this",
        active_form: "Working",
        status: "completed",
        owner: "agent",
        blocks: ["task-9"],
        blocked_by: ["task-3"],
      },
    ],
    removed_task_ids: ["task-2"],
    is_complete_snapshot: true,
  });
});

test("TaskGet null emits removal or confirmed absence", () => {
  const session = makeSessionState();
  session.tasksById.set("task-1", {
    task_id: "task-1",
    subject: "Maybe gone",
    status: "pending",
    blocks: [],
    blocked_by: [],
  });
  session.taskOrder.push("task-1");

  const events = captureBridgeEvents(() => {
    emitToolCall(session, "tool-get", "TaskGet", { taskId: "task-1" });
    emitToolResultUpdate(session, "tool-get", false, { task: null });
  });

  assert.deepEqual(
    events.map((event) => event.update as Record<string, unknown>).find((update) => update.type === "task_state_update"),
    {
      type: "task_state_update",
      source: "task_get",
      tasks: [],
      removed_task_ids: ["task-1"],
      is_complete_snapshot: false,
    },
  );
  assert.equal(session.tasksById.has("task-1"), false);
});

test("handleTaskSystemMessage emits task state for unlinked task_updated messages", () => {
  const session = makeSessionState();

  const events = captureBridgeEvents(() => {
    handleTaskSystemMessage(session, "task_updated", {
      task_id: "task-missing",
      patch: {
        status: "running",
        description: "This should not be emitted",
      },
    });
  });

  assert.deepEqual(events.map((event) => event.update), [
    {
      type: "task_state_update",
      source: "task_lifecycle",
      tasks: [
        {
          task_id: "task-missing",
          subject: "This should not be emitted",
          description: "This should not be emitted",
          status: "in_progress",
          blocks: [],
          blocked_by: [],
        },
      ],
      removed_task_ids: [],
      is_complete_snapshot: false,
    },
  ]);
});

test("handleTaskSystemMessage maps stopped notifications to terminal task state", () => {
  const session = makeSessionState();
  session.tasksById.set("task-1", {
    task_id: "task-1",
    subject: "Watch build",
    status: "in_progress",
    blocks: [],
    blocked_by: [],
  });
  session.taskOrder.push("task-1");

  const events = captureBridgeEvents(() => {
    handleTaskSystemMessage(session, "task_notification", {
      task_id: "task-1",
      status: "stopped",
      output_file: "C:/tmp/task-1.txt",
      summary: "Stopped background watch",
    });
  });

  assert.deepEqual(events.map((event) => event.update), [
    {
      type: "task_state_update",
      source: "task_lifecycle",
      tasks: [
        {
          task_id: "task-1",
          subject: "Watch build",
          description: "Stopped background watch",
          status: "completed",
          blocks: [],
          blocked_by: [],
          metadata: {
            output_file: "C:/tmp/task-1.txt",
            summary: "Stopped background watch",
            terminal_status: "stopped",
          },
        },
      ],
      removed_task_ids: [],
      is_complete_snapshot: false,
    },
  ]);
});

test("handleSdkMessage emits MCP snapshot from init status payload", () => {
  const session = makeSessionState();
  session.query = {
    supportedCommands: async () => [],
  } as unknown as import("@anthropic-ai/claude-agent-sdk").Query;

  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "system",
      subtype: "init",
      session_id: "session-1",
      model: "sonnet",
      mcp_servers: [
        {
          name: "docs",
          status: "pending",
          config: {
            type: "stdio",
            command: "npx",
            args: ["-y", "@anthropic-ai/mcp-docs"],
            timeout: 3000,
            alwaysLoad: true,
          },
          tools: [],
        },
      ],
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  const snapshot = events.find((event) => event.event === "mcp_snapshot");
  assert.deepEqual(snapshot, {
    event: "mcp_snapshot",
    session_id: "session-1",
    source: "init",
    servers: [
      {
        name: "docs",
        status: "pending",
        config: {
          type: "stdio",
          command: "npx",
          args: ["-y", "@anthropic-ai/mcp-docs"],
          timeout: 3000,
          always_load: true,
        },
        tools: [],
      },
    ],
  });
});

test("emitToolProgressUpdate does not reopen completed tools", () => {
  const session = makeSessionState();
  session.toolCalls.set("tool-1", {
    tool_call_id: "tool-1",
    title: "Bash",
    kind: "execute",
    status: "completed",
    content: [],
    locations: [],
    meta: { claudeCode: { toolName: "Bash", parentToolUseId: null } },
  });

  const events = captureBridgeEvents(() => {
    emitToolProgressUpdate(session, "tool-1", "Bash");
  });

  assert.equal(events.length, 0);
  assert.equal(session.toolCalls.get("tool-1")?.status, "completed");
});

test("buildQueryOptions trims language before appending system prompt", () => {
  const input = new AsyncQueue<import("@anthropic-ai/claude-agent-sdk").SDKUserMessage>();
  const options = buildQueryOptions({
    cwd: "C:/work",
    launchSettings: {
      language: "  German  ",
    },
    provisionalSessionId: "session-4",
    input,
    canUseTool: async () => ({ behavior: "deny", message: "not used" }),
    enableSdkDebug: false,
    enableSpawnDebug: false,
    sessionIdForLogs: () => "session-4",
  });

  assert.deepEqual(options.systemPrompt, {
    type: "preset",
    preset: "claude_code",
    append: `${BRIDGE_RUNTIME_GUARD_PROMPT} ${GERMAN_LANGUAGE_PROMPT}`,
  });
});

test("parseCommandEnvelope rejects missing required fields", () => {
  assert.throws(
    () => parseCommandEnvelope(JSON.stringify({ command: "set_model", session_id: "s1" })),
    /set_model\.model must be a string/,
  );
});

test("parseCommandEnvelope validates question_response command", () => {
  const parsed = parseCommandEnvelope(
    JSON.stringify({
      request_id: "req-question",
      command: "question_response",
      session_id: "session-1",
      tool_call_id: "tool-1",
      outcome: {
        outcome: "answered",
        selected_option_ids: ["question_0", "question_2"],
        annotation: {
          preview: "Rendered preview",
          notes: "User note",
        },
      },
    }),
  );

  assert.equal(parsed.requestId, "req-question");
  assert.equal(parsed.command.command, "question_response");
  if (parsed.command.command !== "question_response") {
    throw new Error("unexpected command variant");
  }
  assert.deepEqual(parsed.command.outcome, {
    outcome: "answered",
    selected_option_ids: ["question_0", "question_2"],
    annotation: {
      preview: "Rendered preview",
      notes: "User note",
    },
  });
});

test("parseCommandEnvelope validates user_dialog_response selected command", () => {
  const parsed = parseCommandEnvelope(
    JSON.stringify({
      command: "user_dialog_response",
      session_id: "session-1",
      request_id: "dialog-1",
      outcome: {
        outcome: "selected",
        option_id: "retry_fallback",
      },
    }),
  );

  assert.equal(parsed.requestId, "dialog-1");
  assert.equal(parsed.command.command, "user_dialog_response");
  if (parsed.command.command !== "user_dialog_response") {
    throw new Error("unexpected command variant");
  }
  assert.equal(parsed.command.session_id, "session-1");
  assert.equal(parsed.command.request_id, "dialog-1");
  assert.deepEqual(parsed.command.outcome, {
    outcome: "selected",
    option_id: "retry_fallback",
  });
});

test("parseCommandEnvelope validates user_dialog_response cancelled command", () => {
  const parsed = parseCommandEnvelope(
    JSON.stringify({
      command: "user_dialog_response",
      session_id: "session-1",
      request_id: "dialog-1",
      outcome: { outcome: "cancelled" },
    }),
  );

  assert.equal(parsed.command.command, "user_dialog_response");
  if (parsed.command.command !== "user_dialog_response") {
    throw new Error("unexpected command variant");
  }
  assert.deepEqual(parsed.command.outcome, { outcome: "cancelled" });
});

test("parseCommandEnvelope rejects unsupported user_dialog_response choices", () => {
  assert.throws(
    () =>
      parseCommandEnvelope(
        JSON.stringify({
          command: "user_dialog_response",
          session_id: "session-1",
          request_id: "dialog-1",
          outcome: {
            outcome: "selected",
            option_id: "future_choice",
          },
        }),
      ),
    /user_dialog_response\.outcome\.option_id must be 'retry_fallback' or 'edit_prompt'/,
  );
});

test("requestAskUserQuestionAnswers preserves previews and annotations in updated input", async () => {
  const session = makeSessionState();
  const baseToolCall = {
    tool_call_id: "tool-question",
    title: "AskUserQuestion",
    kind: "other",
    status: "in_progress",
    content: [] as Array<import("./types.js").ToolCallContent>,
    locations: [] as Array<import("./types.js").ToolLocation>,
    meta: { claudeCode: { toolName: "AskUserQuestion", parentToolUseId: null } },
  };

  const events = await captureBridgeEventsAsync(async () => {
    const resultPromise = requestAskUserQuestionAnswers(
      session,
      "tool-question",
      {
        questions: [
          {
            question: "Pick deployment target",
            header: "Target",
            multiSelect: true,
            options: [
              {
                label: "Staging",
                description: "Low-risk validation",
                preview: "Deploy to staging first.",
              },
              {
                label: "Production",
                description: "Customer-facing rollout",
                preview: "Deploy to production after approval.",
              },
            ],
          },
        ],
      },
      baseToolCall,
    );

    await new Promise((resolve) => setImmediate(resolve));
    const pending = session.pendingQuestions.get("tool-question");
    assert.ok(pending, "expected pending question");
    pending.onOutcome({
      outcome: "answered",
      selected_option_ids: ["question_0", "question_1"],
      annotation: {
        notes: "Roll out in both environments",
      },
    });

    const result = await resultPromise;
    assert.equal(result.behavior, "allow");
    if (result.behavior !== "allow") {
      throw new Error("expected allow result");
    }
    assert.deepEqual(result.updatedInput, {
      questions: [
        {
          question: "Pick deployment target",
          header: "Target",
          multiSelect: true,
          options: [
            {
              label: "Staging",
              description: "Low-risk validation",
              preview: "Deploy to staging first.",
            },
            {
              label: "Production",
              description: "Customer-facing rollout",
              preview: "Deploy to production after approval.",
            },
          ],
        },
      ],
      answers: {
        "Pick deployment target": "Staging, Production",
      },
      annotations: {
        "Pick deployment target": {
          preview: "Deploy to staging first.\n\nDeploy to production after approval.",
          notes: "Roll out in both environments",
        },
      },
    });
  });

  const questionEvent = events.find((event) => event.event === "question_request");
  assert.ok(questionEvent, "expected question request event");
  assert.deepEqual(questionEvent.request, {
    tool_call: {
      tool_call_id: "tool-question",
      title: "Pick deployment target",
      kind: "other",
      status: "in_progress",
      content: [],
      locations: [],
      meta: { claudeCode: { toolName: "AskUserQuestion", parentToolUseId: null } },
      raw_input: {
        prompt: {
          question: "Pick deployment target",
          header: "Target",
          multi_select: true,
          options: [
            {
              option_id: "question_0",
              label: "Staging",
              description: "Low-risk validation",
              preview: "Deploy to staging first.",
            },
            {
              option_id: "question_1",
              label: "Production",
              description: "Customer-facing rollout",
              preview: "Deploy to production after approval.",
            },
          ],
        },
        question_index: 0,
        total_questions: 1,
      },
    },
    prompt: {
      question: "Pick deployment target",
      header: "Target",
      multi_select: true,
      options: [
        {
          option_id: "question_0",
          label: "Staging",
          description: "Low-risk validation",
          preview: "Deploy to staging first.",
        },
        {
          option_id: "question_1",
          label: "Production",
          description: "Customer-facing rollout",
          preview: "Deploy to production after approval.",
        },
      ],
    },
    question_index: 0,
    total_questions: 1,
  });
});

test("normalizeToolKind maps known tool names", () => {
  assert.equal(normalizeToolKind("Bash"), "execute");
  assert.equal(normalizeToolKind("PowerShell"), "execute");
  assert.equal(normalizeToolKind("Delete"), "delete");
  assert.equal(normalizeToolKind("Move"), "move");
  assert.equal(normalizeToolKind("EnterWorktree"), "other");
  assert.equal(normalizeToolKind("ExitWorktree"), "other");
  assert.equal(normalizeToolKind("CronCreate"), "other");
  assert.equal(normalizeToolKind("CronDelete"), "other");
  assert.equal(normalizeToolKind("CronList"), "other");
  assert.equal(normalizeToolKind("ScheduleWakeup"), "other");
  assert.equal(normalizeToolKind("PushNotification"), "other");
  assert.equal(normalizeToolKind("RemoteTrigger"), "other");
  assert.equal(normalizeToolKind("REPL"), "other");
  assert.equal(normalizeToolKind("Monitor"), "other");
  assert.equal(normalizeToolKind("Workflow"), "other");
  assert.equal(normalizeToolKind("Projects"), "other");
  assert.equal(normalizeToolKind("Artifact"), "other");
  assert.equal(normalizeToolKind("ShowOnboardingRolePicker"), "other");
  assert.equal(normalizeToolKind("TaskOutput"), "other");
  assert.equal(normalizeToolKind("TaskStop"), "other");
  assert.equal(normalizeToolKind("Task"), "think");
  assert.equal(normalizeToolKind("Agent"), "think");
  assert.equal(normalizeToolKind("EnterPlanMode"), "switch_mode");
  assert.equal(normalizeToolKind("ExitPlanMode"), "switch_mode");
  assert.equal(normalizeToolKind("TodoWrite"), normalizeToolKind("FutureUnknownTool"));
});

test("isShellToolName recognizes only supported shell tools", () => {
  assert.equal(isShellToolName("Bash"), true);
  assert.equal(isShellToolName("PowerShell"), true);
  assert.equal(isShellToolName("Shell"), false);
  assert.equal(isShellToolName("bash"), false);
});

test("shell tool titles use input command", () => {
  assert.equal(createToolCall("tc-bash-title", "Bash", { command: "git status" }).title, "git status");
  assert.equal(
    createToolCall("tc-powershell-title", "PowerShell", { command: "Get-ChildItem" }).title,
    "Get-ChildItem",
  );
  assert.equal(createToolCall("tc-powershell-empty", "PowerShell", {}).title, "Terminal");
});

test("parseFastModeState accepts known values and rejects unknown values", () => {
  assert.equal(parseFastModeState("off"), "off");
  assert.equal(parseFastModeState("cooldown"), "cooldown");
  assert.equal(parseFastModeState("on"), "on");
  assert.equal(parseFastModeState("CD"), null);
  assert.equal(parseFastModeState(undefined), null);
});

test("parseRateLimitStatus accepts known values and rejects unknown values", () => {
  assert.equal(parseRateLimitStatus("allowed"), "allowed");
  assert.equal(parseRateLimitStatus("allowed_warning"), "allowed_warning");
  assert.equal(parseRateLimitStatus("rejected"), "rejected");
  assert.equal(parseRateLimitStatus("warn"), null);
  assert.equal(parseRateLimitStatus(undefined), null);
});

test("parseRuntimeSessionState accepts known values and rejects unknown values", () => {
  assert.equal(parseRuntimeSessionState("idle"), "idle");
  assert.equal(parseRuntimeSessionState("running"), "running");
  assert.equal(parseRuntimeSessionState("requires_action"), "requires_action");
  assert.equal(parseRuntimeSessionState("blocked"), null);
  assert.equal(parseRuntimeSessionState(undefined), null);
});

test("buildRateLimitUpdate maps SDK fields to wire shape", () => {
  const update = buildRateLimitUpdate({
    status: "allowed_warning",
    resetsAt: 1_741_280_000,
    utilization: 0.92,
    rateLimitType: "five_hour",
    overageStatus: "rejected",
    overageResetsAt: 1_741_280_600,
    overageDisabledReason: "out_of_credits",
    isUsingOverage: false,
    surpassedThreshold: 0.9,
  });

  assert.deepEqual(update, {
    type: "rate_limit_update",
    status: "allowed_warning",
    resets_at: 1_741_280_000,
    utilization: 0.92,
    rate_limit_type: "five_hour",
    overage_status: "rejected",
    overage_resets_at: 1_741_280_600,
    overage_disabled_reason: "out_of_credits",
    is_using_overage: false,
    surpassed_threshold: 0.9,
  });
});

test("buildRateLimitUpdate normalizes SDK overage boolean spellings", () => {
  const cases = [
    ["old spelling true", { isUsingOverage: true }, true],
    ["old spelling false", { isUsingOverage: false }, false],
    ["new spelling true", { overageInUse: true }, true],
    ["new spelling false", { overageInUse: false }, false],
    ["both spellings true", { isUsingOverage: true, overageInUse: true }, true],
    ["both spellings false", { isUsingOverage: false, overageInUse: false }, false],
    ["conflicting spellings prefer new true", { isUsingOverage: false, overageInUse: true }, true],
    ["conflicting spellings prefer new false", { isUsingOverage: true, overageInUse: false }, false],
  ] as const;

  for (const [name, fields, expected] of cases) {
    const update = buildRateLimitUpdate({
      status: "allowed",
      ...fields,
    });
    assert.equal(update?.is_using_overage, expected, name);
  }
});

test("buildRateLimitUpdate rejects invalid payloads", () => {
  assert.equal(buildRateLimitUpdate(null), null);
  assert.equal(buildRateLimitUpdate({}), null);
  assert.equal(buildRateLimitUpdate({ status: "warning" }), null);
  assert.deepEqual(
    buildRateLimitUpdate({
      status: "rejected",
      overageStatus: "bad_status",
    }),
    { type: "rate_limit_update", status: "rejected" },
  );
});

test("buildApiRetryUpdate maps SDK api_retry messages to wire shape", () => {
  assert.deepEqual(
    buildApiRetryUpdate({
      attempt: 2,
      max_retries: 4,
      retry_delay_ms: 1500,
      error_status: 529,
      error: "server_error",
    }),
    {
      type: "api_retry_update",
      attempt: 2,
      max_retries: 4,
      retry_delay_ms: 1500,
      error_status: 529,
      error: "server_error",
    },
  );
  for (const error of [
    "model_not_found",
    "oauth_org_not_allowed",
    "overloaded",
  ] as const) {
    assert.deepEqual(
      buildApiRetryUpdate({
        attempt: 2,
        max_retries: 4,
        retry_delay_ms: 1500,
        error_status: 529,
        error,
      }),
      {
        type: "api_retry_update",
        attempt: 2,
        max_retries: 4,
        retry_delay_ms: 1500,
        error_status: 529,
        error,
      },
    );
  }

  assert.deepEqual(
    buildApiRetryUpdate({
      attempt: 1,
      maxRetries: 4,
      retryDelayMs: 1000,
      errorStatus: null,
      error: "unexpected",
    }),
    {
      type: "api_retry_update",
      attempt: 1,
      max_retries: 4,
      retry_delay_ms: 1000,
      error_status: null,
      error: "unknown",
    },
  );
  assert.deepEqual(
    buildApiRetryUpdate({
      attempt: 1,
      max_retries: 10,
      retry_delay_ms: 549.8881698459426,
      error_status: null,
      error: "unexpected",
    }),
    {
      type: "api_retry_update",
      attempt: 1,
      max_retries: 10,
      retry_delay_ms: 549.8881698459426,
      error_status: null,
      error: "unknown",
    },
  );
  assert.equal(buildApiRetryUpdate({ attempt: 1 }), null);
  assert.equal(
    buildApiRetryUpdate({
      attempt: 1,
      max_retries: 10,
      retry_delay_ms: -1,
      error_status: null,
      error: "server_error",
    }),
    null,
  );
});

test("normalizeSettingsParseError accepts only SDK-shaped errors", () => {
  assert.deepEqual(
    normalizeSettingsParseError({
      file: "C:/work/.claude/settings.json",
      path: "permissions.allow",
      message: "Expected array",
    }),
    {
      file: "C:/work/.claude/settings.json",
      path: "permissions.allow",
      message: "Expected array",
    },
  );
  assert.deepEqual(normalizeSettingsParseError({ path: "", message: "Invalid JSON" }), {
    path: "",
    message: "Invalid JSON",
  });
  assert.equal(normalizeSettingsParseError({ path: "", message: "" }), null);
  assert.equal(normalizeSettingsParseError("Invalid JSON"), null);
});

test("handleSdkMessage emits lifecycle compatibility session updates", () => {
  const session = makeSessionState();
  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "prompt_suggestion",
      suggestion: "Write tests for this change",
      uuid: "message-1",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(session, {
      type: "system",
      subtype: "api_retry",
      attempt: 1,
      max_retries: 4,
      retry_delay_ms: 1000,
      error_status: null,
      error: "server_error",
      uuid: "message-2",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(session, {
      type: "system",
      subtype: "session_state_changed",
      state: "idle",
      uuid: "message-3",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(session, {
      type: "system",
      subtype: "status",
      status: "requesting",
      uuid: "message-4",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  assert.deepEqual(
    events.map((event) => event.update),
    [
      { type: "prompt_suggestion_update", suggestion: "Write tests for this change" },
      {
        type: "api_retry_update",
        attempt: 1,
        max_retries: 4,
        retry_delay_ms: 1000,
        error_status: null,
        error: "server_error",
      },
      { type: "runtime_session_state_update", state: "idle" },
      { type: "session_status_update", status: "requesting" },
    ],
  );
});

test("classifyTurnErrorKind prefers SDK assistant error codes", () => {
  assert.equal(classifyTurnErrorKind("error_during_execution", [], "model_not_found"), "model_unavailable");
  assert.equal(classifyTurnErrorKind("error_during_execution", [], "oauth_org_not_allowed"), "account_access");
  assert.equal(classifyTurnErrorKind("error_during_execution", [], "overloaded"), "transient_service");
  assert.equal(classifyTurnErrorKind("error_during_execution", [], "server_error"), "transient_service");
  assert.equal(classifyTurnErrorKind("error_during_execution", [], "authentication_failed"), "auth_required");
  assert.equal(classifyTurnErrorKind("error_during_execution", [], "billing_error"), "plan_limit");
  assert.equal(classifyTurnErrorKind("error_during_execution", [], "rate_limit"), "plan_limit");
});

test("handleSdkMessage replaces available commands from commands_changed", () => {
  const session = makeSessionState();
  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "system",
      subtype: "commands_changed",
      commands: [
        { name: "/one", description: "First command", argumentHint: "<value>" },
        { name: "/two", description: undefined, argumentHint: undefined },
      ],
      uuid: "message-commands",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  assert.deepEqual(events.map((event) => event.update), [
    {
      type: "available_commands_update",
      commands: [
        { name: "/one", description: "First command", input_hint: "<value>" },
        { name: "/two", description: "" },
      ],
      source: "commands_changed",
      generation: 1,
    },
  ]);
});

test("handleSdkMessage accepts empty commands_changed replacement list", () => {
  const session = makeSessionState();
  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "system",
      subtype: "commands_changed",
      commands: [],
      uuid: "message-commands-empty",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  assert.deepEqual(events.map((event) => event.update), [
    {
      type: "available_commands_update",
      commands: [],
      source: "commands_changed",
      generation: 1,
    },
  ]);
});

test("available command registry blocks stale supportedCommands after dynamic updates", () => {
  const session = makeSessionState();
  const events = captureBridgeEvents(() => {
    assert.equal(
      updateAvailableCommands(session, "session_result_commands", [
        { name: "base", description: "Base command" },
      ]),
      true,
    );
    assert.equal(
      updateAvailableCommands(session, "commands_changed", [
        { name: "base", description: "Base command" },
        { name: "project-plugin", description: "Project plugin command" },
      ]),
      true,
    );
    assert.equal(
      updateAvailableCommands(session, "supportedCommands", [
        { name: "base", description: "Base command" },
      ]),
      false,
    );
  });

  assert.deepEqual(
    events.map((event) => event.update),
    [
      {
        type: "available_commands_update",
        commands: [{ name: "base", description: "Base command" }],
        source: "session_result_commands",
        generation: 1,
      },
      {
        type: "available_commands_update",
        commands: [
          { name: "base", description: "Base command" },
          { name: "project-plugin", description: "Project plugin command" },
        ],
        source: "commands_changed",
        generation: 2,
      },
    ],
  );
  assert.equal(session.availableCommands?.generation, 2);
  assert.equal(session.availableCommands?.source, "commands_changed");
  assert.deepEqual(
    session.availableCommands?.commands.map((command) => command.name),
    ["base", "project-plugin"],
  );
});

test("available command registry lets authoritative snapshots remove commands", () => {
  const session = makeSessionState();
  const events = captureBridgeEvents(() => {
    updateAvailableCommands(session, "reload_plugins", [
      { name: "base", description: "Base command" },
      { name: "removed-plugin", description: "Removed plugin command" },
    ]);
    updateAvailableCommands(session, "commands_changed", [
      { name: "base", description: "Base command" },
    ]);
  });

  assert.deepEqual(
    events.map((event) => event.update),
    [
      {
        type: "available_commands_update",
        commands: [
          { name: "base", description: "Base command" },
          { name: "removed-plugin", description: "Removed plugin command" },
        ],
        source: "reload_plugins",
        generation: 1,
      },
      {
        type: "available_commands_update",
        commands: [{ name: "base", description: "Base command" }],
        source: "commands_changed",
        generation: 2,
      },
    ],
  );
});

test("handleSdkMessage emits system notices for notifications and plugin failures", () => {
  const session = makeSessionState();
  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "system",
      subtype: "notification",
      key: "sync",
      text: "Sync completed",
      priority: "low",
      uuid: "message-notification",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(session, {
      type: "system",
      subtype: "plugin_install",
      status: "failed",
      name: "acme",
      error: "download failed",
      uuid: "message-plugin",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  assert.deepEqual(events.map((event) => event.update), [
    { type: "system_notice_update", severity: "info", message: "Sync completed" },
    { type: "system_notice_update", severity: "warning", message: "Plugin install failed acme: download failed" },
  ]);
});

test("handleSdkMessage treats mirror errors as log-only diagnostics", () => {
  const session = makeSessionState();
  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "system",
      subtype: "mirror_error",
      error: "append timed out",
      key: { projectKey: "project", sessionId: "session-1", subpath: "subagents/agent-1" },
      uuid: "message-mirror",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  assert.deepEqual(events, []);
});

test("handleSdkMessage keeps log-only system messages non-emitting", () => {
  const session = makeSessionState();
  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "system",
      subtype: "plugin_install",
      status: "completed",
      uuid: "message-plugin-complete",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(session, {
      type: "system",
      subtype: "permission_denied",
      tool_name: "Bash",
      tool_use_id: "tool-1",
      message: "denied",
      uuid: "message-permission",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(session, {
      type: "system",
      subtype: "memory_recall",
      mode: "select",
      memories: [],
      uuid: "message-memory",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(session, {
      type: "system",
      subtype: "thinking_tokens",
      estimated_tokens: 120,
      estimated_tokens_delta: 20,
      uuid: "message-thinking",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  assert.deepEqual(events, []);
});

test("handleSdkMessage accepts auto-continuation message origin without user output", () => {
  const session = makeSessionState();
  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "user",
      message: { role: "user", content: [{ type: "text", text: "continue" }] },
      parent_tool_use_id: null,
      origin: { kind: "auto-continuation" },
      uuid: "message-auto-continuation",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  assert.deepEqual(events, []);
});

test("handleSdkMessage preserves assistant correlation metadata on tool calls", () => {
  const session = makeSessionState();
  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "assistant",
      message: {
        content: [{ type: "tool_use", id: "tool-1", name: "Bash", input: { command: "npm test" } }],
      },
      parent_tool_use_id: null,
      request_id: "request-1",
      subagent_type: "code-review",
      task_description: "Review the bridge",
      uuid: "message-assistant",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  assert.deepEqual(
    events.map((event) => (event.update as Record<string, unknown>).tool_call),
    [
      {
        tool_call_id: "tool-1",
        title: "npm test",
        kind: "execute",
        status: "in_progress",
        source_message_uuid: "message-assistant",
        content: [],
        raw_input: { command: "npm test" },
        locations: [],
        meta: {
          claudeCode: {
            toolName: "Bash",
            parentToolUseId: null,
            requestId: "request-1",
            subagentType: "code-review",
            taskDescription: "Review the bridge",
          },
        },
      },
    ],
  );
});

test("handleTaskSystemMessage preserves task correlation metadata", () => {
  const session = makeSessionState();
  const events = captureBridgeEvents(() => {
    handleTaskSystemMessage(session, "task_started", {
      task_id: "task-1",
      tool_use_id: "tool-1",
      description: "Run checks",
      request_id: "request-1",
      subagent_type: "tester",
      task_description: "Validate the branch",
    });
  });

  assert.deepEqual(events.map((event) => event.update), [
    {
      type: "tool_call",
      tool_call: {
        tool_call_id: "tool-1",
        title: "Agent",
        kind: "think",
        status: "pending",
        content: [],
        raw_input: {},
        locations: [],
        meta: { claudeCode: { toolName: "Agent", parentToolUseId: null } },
      },
    },
    {
      type: "task_state_update",
      source: "task_lifecycle",
      tasks: [
        {
          task_id: "task-1",
          subject: "Validate the branch",
          description: "Run checks",
          status: "in_progress",
          blocks: [],
          blocked_by: [],
          metadata: {
            request_id: "request-1",
            subagent_type: "tester",
            task_description: "Validate the branch",
          },
          source_tool_call_id: "tool-1",
        },
      ],
      removed_task_ids: [],
      is_complete_snapshot: false,
    },
    {
      type: "tool_call_update",
      tool_call_update: {
        tool_call_id: "tool-1",
        fields: {
          status: "in_progress",
        },
      },
    },
    {
      type: "tool_call_update",
      tool_call_update: {
        tool_call_id: "tool-1",
        fields: {
          status: "in_progress",
          raw_output: "Run checks",
          content: [{ type: "content", content: { type: "text", text: "Run checks" } }],
          task_metadata: {
            request_id: "request-1",
            subagent_type: "tester",
            task_description: "Validate the branch",
          },
        },
      },
    },
  ]);
});

test("parseCommandEnvelope validates set_effort command", () => {
  for (const effort of ["low", "medium", "high", "xhigh", "max"] as const) {
    const parsed = parseCommandEnvelope(
      JSON.stringify({
        request_id: "req-effort",
        command: "set_effort",
        session_id: "session-1",
        effort,
      }),
    );
    assert.equal(parsed.requestId, "req-effort");
    assert.equal(parsed.command.command, "set_effort");
    if (parsed.command.command !== "set_effort") {
      throw new Error("unexpected command variant");
    }
    assert.equal(parsed.command.session_id, "session-1");
    assert.equal(parsed.command.effort, effort);
  }
});

test("parseCommandEnvelope rejects unsupported set_effort values", () => {
  assert.throws(
    () =>
      parseCommandEnvelope(
        JSON.stringify({
          command: "set_effort",
          session_id: "session-1",
          effort: "banana",
        }),
      ),
    /set_effort\.effort must be one of low, medium, high, xhigh, max/,
  );
});

test("parseCommandEnvelope validates set_agent command", () => {
  const parsed = parseCommandEnvelope(
    JSON.stringify({
      request_id: "req-agent",
      command: "set_agent",
      session_id: "session-1",
      agent: "reviewer",
    }),
  );

  assert.equal(parsed.requestId, "req-agent");
  assert.equal(parsed.command.command, "set_agent");
  if (parsed.command.command !== "set_agent") {
    throw new Error("unexpected command variant");
  }
  assert.equal(parsed.command.session_id, "session-1");
  assert.equal(parsed.command.agent, "reviewer");
});

test("parseCommandEnvelope validates set_agent reset", () => {
  const parsed = parseCommandEnvelope(
    JSON.stringify({
      command: "set_agent",
      session_id: "session-1",
      agent: null,
    }),
  );

  assert.equal(parsed.command.command, "set_agent");
  if (parsed.command.command !== "set_agent") {
    throw new Error("unexpected command variant");
  }
  assert.equal(parsed.command.agent, null);
});

test("parseCommandEnvelope rejects invalid set_agent values", () => {
  for (const agent of [undefined, "", "   ", 42, {}, []]) {
    assert.throws(
      () =>
        parseCommandEnvelope(
          JSON.stringify({
            command: "set_agent",
            session_id: "session-1",
            ...(agent !== undefined ? { agent } : {}),
          }),
        ),
      /set_agent\.agent must be a non-empty string or null/,
    );
  }
});

test("applySessionEffort uses live flag settings for xhigh and max", async () => {
  const calls: unknown[] = [];
  const query = {
    async applyFlagSettings(settings: unknown): Promise<void> {
      calls.push(settings);
    },
  } as import("@anthropic-ai/claude-agent-sdk").Query;

  await applySessionEffort(query, "xhigh");
  await applySessionEffort(query, "max");

  assert.deepEqual(calls, [{ effortLevel: "xhigh" }, { effortLevel: "max" }]);
});

test("applySessionAgent uses live flag settings for agent switch and reset", async () => {
  const calls: unknown[] = [];
  const query = {
    async applyFlagSettings(settings: unknown): Promise<void> {
      calls.push(settings);
    },
  } as import("@anthropic-ai/claude-agent-sdk").Query;

  await applySessionAgent(query, "reviewer");
  await applySessionAgent(query, null);

  assert.deepEqual(calls, [{ agent: "reviewer" }, { agent: null }]);
});

test("emitEffortConfigOptionUpdate publishes effortLevel config option", () => {
  const events = captureBridgeEvents(() => {
    emitEffortConfigOptionUpdate("session-1", "max");
  });

  assert.deepEqual(events.at(-1), {
    event: "session_update",
    session_id: "session-1",
    update: {
      type: "config_option_update",
      option_id: "effortLevel",
      value: "max",
    },
  });
});

test("emitAgentConfigOptionUpdate publishes agent config option", () => {
  const events = captureBridgeEvents(() => {
    emitAgentConfigOptionUpdate("session-1", null);
  });

  assert.deepEqual(events.at(-1), {
    event: "session_update",
    session_id: "session-1",
    update: {
      type: "config_option_update",
      option_id: "agent",
      value: null,
    },
  });
});

test("shouldEmitStartupAuthRequiredForAccount keeps legacy first-party behavior", () => {
  assert.equal(shouldEmitStartupAuthRequiredForAccount({}), true);
  assert.equal(
    shouldEmitStartupAuthRequiredForAccount({ apiProvider: "firstParty" }),
    true,
  );
  assert.equal(
    shouldEmitStartupAuthRequiredForAccount({
      apiProvider: "firstParty",
      apiKeySource: "oauth",
    }),
    false,
  );
  assert.equal(
    shouldEmitStartupAuthRequiredForAccount({
      apiProvider: "firstParty",
      email: "user@example.com",
    }),
    false,
  );
});

test("shouldEmitStartupAuthRequiredForAccount skips Claude OAuth hint for external providers", () => {
  for (const apiProvider of [
    "bedrock",
    "vertex",
    "foundry",
    "gateway",
    "anthropicAws",
    "mantle",
  ] as const) {
    assert.equal(shouldEmitStartupAuthRequiredForAccount({ apiProvider }), false);
  }
});

test("mapSdkAccountInfo normalizes SDK account metadata through one bridge DTO", () => {
  assert.deepEqual(
    mapSdkAccountInfo({
      email: " user@example.com ",
      organization: " org-1 ",
      subscriptionType: " Claude Max ",
      tokenSource: " oauth ",
      apiKeySource: " user ",
      apiProvider: "gateway",
    }),
    {
      email: "user@example.com",
      organization: "org-1",
      subscription_type: "Claude Max",
      token_source: "oauth",
      api_key_source: "user",
      api_provider: "gateway",
    },
  );
});

test("handleSdkMessage emits settings parse errors from defensive payloads", () => {
  const session = makeSessionState();
  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "settings_parse_error",
      file: "C:/work/.claude/settings.json",
      path: "permissions.allow",
      message: "Expected array",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  assert.deepEqual(events.at(-1)?.update, {
    type: "settings_parse_error",
    file: "C:/work/.claude/settings.json",
    path: "permissions.allow",
    message: "Expected array",
  });
});

test("mapAvailableAgents normalizes and deduplicates agents", () => {
  const agents = mapAvailableAgents([
    { name: "reviewer", description: "", model: "" },
    { name: "reviewer", description: "Reviews code", model: "haiku" },
    { name: "explore", description: "Explore codebase", model: "sonnet" },
    { name: "  ", description: "ignored" },
    {},
  ]);

  assert.deepEqual(agents, [
    { name: "explore", description: "Explore codebase", model: "sonnet" },
    { name: "reviewer", description: "Reviews code", model: "haiku" },
  ]);
});

test("mapAvailableAgents rejects non-array payload", () => {
  assert.deepEqual(mapAvailableAgents(null), []);
  assert.deepEqual(mapAvailableAgents({}), []);
});

test("createToolCall builds edit diff content", () => {
  const toolCall = createToolCall("tc-1", "Edit", {
    file_path: "src/main.rs",
    old_string: "old",
    new_string: "new",
  });
  assert.equal(toolCall.kind, "edit");
  assert.equal(toolCall.content.length, 1);
  assert.deepEqual(toolCall.content[0], {
    type: "diff",
    old_path: "src/main.rs",
    new_path: "src/main.rs",
    old: "old",
    new: "new",
  });
  assert.deepEqual(toolCall.meta, { claudeCode: { toolName: "Edit", parentToolUseId: null } });
});

test("createToolCall preserves parent tool linkage metadata", () => {
  const toolCall = createToolCall("tc-child", "Bash", { command: "echo hi" }, "tc-parent");

  assert.deepEqual(toolCall.meta, {
    claudeCode: { toolName: "Bash", parentToolUseId: "tc-parent" },
  });
});

test("createToolCall builds write preview diff content", () => {
  const toolCall = createToolCall("tc-w", "Write", {
    file_path: "src/new-file.ts",
    content: "export const x = 1;\n",
  });
  assert.equal(toolCall.kind, "edit");
  assert.deepEqual(toolCall.content, [
    {
      type: "diff",
      old_path: "src/new-file.ts",
      new_path: "src/new-file.ts",
      old: "",
      new: "export const x = 1;\n",
    },
  ]);
});

test("createToolCall includes search and webfetch context in title", () => {
  const glob = createToolCall("tc-g", "Glob", { pattern: "**/*.md", path: "notes" });
  assert.equal(glob.title, "Glob **/*.md in notes");

  const grep = createToolCall("tc-grep", "Grep", {
    pattern: "TODO",
    path: "src",
    glob: "**/*.rs",
    output_mode: "content",
    "-i": true,
    "-C": 2,
    type: "rust",
    head_limit: 10,
    offset: 5,
    multiline: true,
  });
  assert.equal(
    grep.title,
    "Grep TODO in src (glob **/*.rs, type rust, content, case-insensitive, context 2, limit 10, offset 5, multiline)",
  );

  const fetch = createToolCall("tc-f", "WebFetch", { url: "https://example.com" });
  assert.equal(fetch.title, "WebFetch https://example.com");
});

test("createToolCall builds Agent title from name and type without description fallback", () => {
  const named = createToolCall("tc-agent-name", "Agent", {
    description: "review changes",
    prompt: "Review the branch",
    name: " review-worker ",
    subagent_type: " general-purpose ",
    model: " opus ",
  });
  const typed = createToolCall("tc-agent-type", "Agent", {
    description: "inspect state",
    prompt: "Inspect the runtime",
    subagent_type: " general-purpose ",
    model: " sonnet ",
  });
  const describedOnly = createToolCall("tc-agent-description", "Agent", {
    description: "should not become title",
    prompt: "Review",
  });

  assert.equal(named.title, "Agent: review-worker");
  assert.equal(typed.title, "Agent: general-purpose");
  assert.equal(describedOnly.title, "Agent");
});

test("createToolCall builds worktree titles from input rules", () => {
  const namedEnter = createToolCall("tc-enter-name", "EnterWorktree", { name: "feature-auth" });
  assert.equal(namedEnter.kind, "other");
  assert.equal(namedEnter.title, "feature-auth");

  const pathEnter = createToolCall("tc-enter-path", "EnterWorktree", {
    path: "C:\\repo\\.worktrees\\feature-auth",
  });
  assert.equal(pathEnter.kind, "other");
  assert.equal(pathEnter.title, "EnterWorktree");

  const exit = createToolCall("tc-exit", "ExitWorktree", {
    action: "remove",
    discard_changes: true,
  });
  assert.equal(exit.kind, "other");
  assert.equal(exit.title, "ExitWorktree");
});

test("createToolCall maps cron tools to other kind with stable titles", () => {
  for (const toolName of ["CronCreate", "CronDelete", "CronList"]) {
    const toolCall = createToolCall(`tc-${toolName}`, toolName, {
      cron: "30 9 * * 1",
      prompt: "Send weekly status",
      id: "schedule-1",
    });
    assert.equal(toolCall.kind, "other");
    assert.equal(toolCall.title, toolName);
  }
});

test("createToolCall maps ScheduleWakeup to other kind with stable title", () => {
  const toolCall = createToolCall("tc-wakeup", "ScheduleWakeup", {
    delaySeconds: 90,
    reason: "Poll again after warmup",
    prompt: "/loop check status",
  });

  assert.equal(toolCall.kind, "other");
  assert.equal(toolCall.title, "ScheduleWakeup");
});

test("createToolCall maps PushNotification to other kind with stable title", () => {
  const toolCall = createToolCall("tc-push-notification", "PushNotification", {
    message: "Build finished",
    status: "proactive",
  });

  assert.equal(toolCall.kind, "other");
  assert.equal(toolCall.title, "PushNotification");
});

test("createToolCall maps RemoteTrigger to other kind and action title", () => {
  const toolCall = createToolCall("tc-remote-trigger", "RemoteTrigger", {
    action: " run ",
    trigger_id: "deploy-prod",
  });

  assert.equal(toolCall.kind, "other");
  assert.equal(toolCall.title, "RemoteTrigger: run");
});

test("createToolCall uses RemoteTrigger fallback title without action", () => {
  const toolCall = createToolCall("tc-remote-trigger-fallback", "RemoteTrigger", {
    trigger_id: "deploy-prod",
  });

  assert.equal(toolCall.kind, "other");
  assert.equal(toolCall.title, "RemoteTrigger");
});

test("createToolCall maps REPL to other kind and code title", () => {
  const toolCall = createToolCall("tc-repl", "REPL", {
    code: "  await inspectState()  ",
    description: "Inspect runtime state",
    timeout: 45_000,
  });

  assert.equal(toolCall.kind, "other");
  assert.equal(toolCall.title, "REPL: await inspectState()");
});

test("createToolCall uses REPL fallback title instead of description", () => {
  const toolCall = createToolCall("tc-repl-fallback", "REPL", {
    description: "Inspect runtime state",
  });

  assert.equal(toolCall.kind, "other");
  assert.equal(toolCall.title, "REPL");
});

test("createToolCall maps Monitor to other kind and description title", () => {
  const toolCall = createToolCall("tc-monitor", "Monitor", {
    description: "watch deploy logs",
    timeout_ms: 30000,
    persistent: false,
    command: "tail -f deploy.log",
  });

  assert.equal(toolCall.kind, "other");
  assert.equal(toolCall.title, "Monitor: watch deploy logs");
});

test("createToolCall maps Workflow to other kind and name title", () => {
  const namedWorkflow = createToolCall("tc-workflow", "Workflow", {
    name: "spec",
    args: { topic: "rendering" },
  });
  const fallbackWorkflow = createToolCall("tc-workflow-fallback", "Workflow", {
    script: "export const meta = { name: 'inline', description: 'Run', phases: [] };",
  });

  assert.equal(namedWorkflow.kind, "other");
  assert.equal(namedWorkflow.title, "Workflow: spec");
  assert.equal(fallbackWorkflow.kind, "other");
  assert.equal(fallbackWorkflow.title, "Workflow");
});

test("createToolCall maps project and artifact tools to compact titles", () => {
  const projectInfo = createToolCall("tc-project-info", "Projects", {
    method: "project_info",
  });
  const projectRead = createToolCall("tc-project-read", "Projects", {
    method: "project_read",
    path: "claude/notes.md",
  });
  const projectSearch = createToolCall("tc-project-search", "Projects", {
    method: "project_search",
    query: "migration",
  });
  const artifactWithLabel = createToolCall("tc-artifact-label", "Artifact", {
    file_path: "C:/work/report.html",
    favicon: "R",
    label: "report-v2",
  });
  const artifactFallback = createToolCall("tc-artifact-path", "Artifact", {
    file_path: "C:/work/report.html",
    favicon: "R",
  });
  const rolePicker = createToolCall("tc-role-picker", "ShowOnboardingRolePicker", {});

  assert.equal(projectInfo.kind, "other");
  assert.equal(projectInfo.title, "Projects: info");
  assert.equal(projectRead.title, "Projects: read claude/notes.md");
  assert.equal(projectSearch.title, "Projects: search migration");
  assert.equal(artifactWithLabel.kind, "other");
  assert.equal(artifactWithLabel.title, "Artifact: report-v2");
  assert.equal(artifactFallback.title, "Artifact: C:/work/report.html");
  assert.equal(rolePicker.kind, "other");
  assert.equal(rolePicker.title, "ShowOnboardingRolePicker");
});

test("createToolCall maps EnterPlanMode to switch_mode kind with stable title", () => {
  const toolCall = createToolCall("tc-enter-plan-mode", "EnterPlanMode", {});

  assert.equal(toolCall.kind, "switch_mode");
  assert.equal(toolCall.title, "EnterPlanMode");
});

test("buildToolResultFields extracts plain-text output", () => {
  const fields = buildToolResultFields(false, [{ text: "line 1" }, { text: "line 2" }]);
  assert.equal(fields.status, "completed");
  assert.equal(fields.raw_output, "line 1\nline 2");
  assert.deepEqual(fields.content, [
    { type: "content", content: { type: "text", text: "line 1\nline 2" } },
  ]);
});

test("buildToolResultFields renders structured Grep output", () => {
  const base = createToolCall("tc-grep", "Grep", {
    pattern: "TODO",
    path: "src",
    output_mode: "content",
  });
  const fields = buildToolResultFields(false, "raw SDK text", base, {
    mode: "content",
    numFiles: 2,
    filenames: ["src/a.rs", "src/b.rs"],
    content: "src/a.rs:1:TODO\nsrc/b.rs:2:TODO",
    numLines: 2,
    numMatches: 2,
    appliedLimit: 250,
  });

  const expected =
    "src/a.rs:1:TODO\nsrc/b.rs:2:TODO\nSummary: 2 files, 2 matches, 2 lines, mode content, limit 250";
  assert.equal(fields.status, "completed");
  assert.equal(fields.raw_output, expected);
  assert.deepEqual(fields.content, [
    { type: "content", content: { type: "text", text: expected } },
  ]);
});

test("buildToolResultFields renders structured empty Grep output", () => {
  const base = createToolCall("tc-grep-empty", "Grep", {
    pattern: "<rare string>",
    output_mode: "content",
  });
  const fields = buildToolResultFields(false, "No matches found", base, {
    mode: "content",
    numFiles: 0,
    filenames: [],
    content: "",
    numLines: 0,
  });

  const expected = "No matches found\nSummary: 0 files, 0 lines, mode content";
  assert.equal(fields.raw_output, expected);
  assert.deepEqual(fields.content, [
    { type: "content", content: { type: "text", text: expected } },
  ]);
});

test("buildToolResultFields renders structured Glob output", () => {
  const base = createToolCall("tc-glob", "Glob", { pattern: "**/*.rs", path: "src" });
  const fields = buildToolResultFields(false, "", base, {
    durationMs: 12,
    numFiles: 2,
    filenames: ["src/main.rs", "src/lib.rs"],
    truncated: false,
  });

  const expected = "2 files found\nsrc/main.rs\nsrc/lib.rs\nDuration: 12ms";
  assert.equal(fields.status, "completed");
  assert.equal(fields.raw_output, expected);
  assert.deepEqual(fields.content, [
    { type: "content", content: { type: "text", text: expected } },
  ]);
});

test("normalizeToolResultText collapses persisted-output payload to first meaningful line", () => {
  const normalized = normalizeToolResultText(`
<persisted-output>
  │ Output too large (132.5KB). Full output saved to: C:\\tmp\\tool-results\\bbf63b9.txt
  │
  │ Preview (first 2KB):
  │
  │ {"huge":"payload"}
  │ ...
  │ </persisted-output>
`);
  assert.equal(normalized, "Output too large (132.5KB). Full output saved to: C:\\tmp\\tool-results\\bbf63b9.txt");
});

test("normalizeToolResultText does not sanitize non-error output", () => {
  const text =
    "The user doesn't want to proceed with this tool use. The tool use was rejected (eg. if it was a file edit, the new_string was NOT written to the file). STOP what you are doing and wait for the user to tell you how to proceed.";
  assert.equal(normalizeToolResultText(text), text);
});

test("normalizeToolResultText sanitizes exact SDK rejection payloads for errors", () => {
  const cancelledText =
    "The user doesn't want to proceed with this tool use. The tool use was rejected (eg. if it was a file edit, the new_string was NOT written to the file). STOP what you are doing and wait for the user to tell you how to proceed.";
  assert.equal(normalizeToolResultText(cancelledText, true), "Cancelled by user.");

  const deniedText =
    "Permission for this tool use was denied. The tool use was rejected (eg. if it was a file edit, the new_string was NOT written to the file). Try a different approach or report the limitation to complete your task.";
  assert.equal(normalizeToolResultText(deniedText, true), "Permission denied.");
});

test("normalizeToolResultText sanitizes SDK rejection prefixes with user follow-up", () => {
  const cancelledWithUserMessage =
    "The user doesn't want to proceed with this tool use. The tool use was rejected (eg. if it was a file edit, the new_string was NOT written to the file). To tell you how to proceed, the user said:\nPlease skip this";
  assert.equal(normalizeToolResultText(cancelledWithUserMessage, true), "Cancelled by user.");

  const deniedWithUserMessage =
    "Permission for this tool use was denied. The tool use was rejected (eg. if it was a file edit, the new_string was NOT written to the file). The user said:\nNot now";
  assert.equal(normalizeToolResultText(deniedWithUserMessage, true), "Permission denied.");
});

test("normalizeToolResultText does not sanitize substring matches in error output", () => {
  const bashOutput = "grep output: doesn't want to proceed with this tool use";
  assert.equal(normalizeToolResultText(bashOutput, true), bashOutput);
});

test("cache split policy defaults stay aligned with UI thresholds", () => {
  assert.equal(CACHE_SPLIT_POLICY.softLimitBytes, 1536);
  assert.equal(CACHE_SPLIT_POLICY.hardLimitBytes, 4096);
  assert.equal(CACHE_SPLIT_POLICY.previewLimitBytes, 2048);
  assert.equal(previewKilobyteLabel(CACHE_SPLIT_POLICY), "2KB");
});

test("buildToolResultFields uses normalized persisted-output text", () => {
  const fields = buildToolResultFields(
    false,
    `<persisted-output>
      │ Output too large (14KB). Full output saved to: C:\\tmp\\tool-results\\x.txt
      │
      │ Preview (first 2KB):
      │ {"k":"v"}
      │ </persisted-output>`,
  );
  assert.equal(fields.raw_output, "Output too large (14KB). Full output saved to: C:\\tmp\\tool-results\\x.txt");
  assert.deepEqual(fields.content, [
    {
      type: "content",
      content: {
        type: "text",
        text: "Output too large (14KB). Full output saved to: C:\\tmp\\tool-results\\x.txt",
      },
    },
  ]);
});

test("buildToolResultFields sanitizes SDK rejection text only for failed results", () => {
  const sdkRejectionText =
    "The user doesn't want to proceed with this tool use. The tool use was rejected (eg. if it was a file edit, the new_string was NOT written to the file). STOP what you are doing and wait for the user to tell you how to proceed.";

  const successFields = buildToolResultFields(false, sdkRejectionText);
  assert.equal(successFields.raw_output, sdkRejectionText);

  const errorFields = buildToolResultFields(true, sdkRejectionText);
  assert.equal(errorFields.raw_output, "Cancelled by user.");
});

test("buildToolResultFields maps structured Write output to diff content", () => {
  const base = createToolCall("tc-w", "Write", {
    file_path: "src/main.ts",
    content: "new",
  });
  const fields = buildToolResultFields(
    false,
    {
      type: "update",
      filePath: "src/main.ts",
      content: "new",
      originalFile: "old",
      structuredPatch: [],
      gitDiff: {
        repository: "acme/project",
      },
    },
    base,
  );
  assert.equal(fields.status, "completed");
  assert.deepEqual(fields.content, [
    {
      type: "diff",
      old_path: "src/main.ts",
      new_path: "src/main.ts",
      old: "old",
      new: "new",
      repository: "acme/project",
    },
  ]);
});

test("buildToolResultFields preserves Edit diff content from input and structured repository", () => {
  const base = createToolCall("tc-e", "Edit", {
    file_path: "src/main.ts",
    old_string: "old",
    new_string: "new",
  });
  const fields = buildToolResultFields(
    false,
    [{ text: "Updated successfully" }],
    base,
    {
      result: {
        filePath: "src/main.ts",
        gitDiff: {
          repository: "acme/project",
        },
      },
    },
  );
  assert.equal(fields.status, "completed");
  assert.deepEqual(fields.content, [
    {
      type: "diff",
      old_path: "src/main.ts",
      new_path: "src/main.ts",
      old: "old",
      new: "new",
      repository: "acme/project",
    },
  ]);
});

test("buildToolResultFields ignores model-facing Bash stale read hints", () => {
  const base = createToolCall("tc-bash", "Bash", { command: "npm test" });
  const fields = buildToolResultFields(
    false,
    {
      stdout: "real stdout",
      stderr: "",
      interrupted: false,
      staleReadFileStateHint: "src/main.rs changed while command ran",
    },
    base,
    {
      result: {
        stdout: "real stdout",
        stderr: "",
        interrupted: false,
        staleReadFileStateHint: "src/main.rs changed while command ran",
      },
    },
  );

  assert.equal(fields.raw_output, "real stdout");
  assert.equal(fields.output_metadata, undefined);
});

test("buildToolResultFields maps PowerShell structured output like shell output", () => {
  const base = createToolCall("tc-powershell", "PowerShell", { command: "Get-ChildItem" });
  const fields = buildToolResultFields(
    false,
    {
      stdout: "stdout line",
      stderr: "stderr line",
      interrupted: true,
    },
    base,
    {
      result: {
        stdout: "stdout line",
        stderr: "stderr line",
        interrupted: true,
      },
    },
  );

  assert.equal(fields.raw_output, "stdout line\nstderr line\nCommand was aborted before completion.");
  assert.equal(fields.output_metadata, undefined);
});

test("buildToolResultFields adds Bash auto-backgrounded metadata and message", () => {
  const base = createToolCall("tc-bash-bg", "Bash", { command: "npm run watch" });
  const fields = buildToolResultFields(
    false,
    {
      stdout: "",
      stderr: "",
      interrupted: false,
      backgroundTaskId: "task-42",
      assistantAutoBackgrounded: true,
    },
    base,
    {
      result: {
        stdout: "",
        stderr: "",
        interrupted: false,
        backgroundTaskId: "task-42",
        assistantAutoBackgrounded: true,
      },
    },
  );

  assert.equal(
    fields.raw_output,
    "Command was auto-backgrounded by assistant mode with ID: task-42.",
  );
  assert.deepEqual(fields.output_metadata, {
    bash: {
      assistant_auto_backgrounded: true,
    },
  });
});

test("buildToolResultFields maps structured ReadMcpResource output to typed resource content", () => {
  const base = createToolCall("tc-mcp", "ReadMcpResource", {
    server: "docs",
    uri: "file://manual.pdf",
  });
  const fields = buildToolResultFields(
    false,
    {
      contents: [
        {
          uri: "file://manual.pdf",
          mimeType: "application/pdf",
          text: "[Resource from docs at file://manual.pdf] Saved to C:\\tmp\\manual.pdf",
          blobSavedTo: "C:\\tmp\\manual.pdf",
        },
      ],
    },
    base,
    {
      result: {
        contents: [
          {
            uri: "file://manual.pdf",
            mimeType: "application/pdf",
            text: "[Resource from docs at file://manual.pdf] Saved to C:\\tmp\\manual.pdf",
            blobSavedTo: "C:\\tmp\\manual.pdf",
          },
        ],
      },
    },
  );

  assert.equal(fields.status, "completed");
  assert.deepEqual(fields.content, [
    {
      type: "mcp_resource",
      uri: "file://manual.pdf",
      mime_type: "application/pdf",
      text: "[Resource from docs at file://manual.pdf] Saved to C:\\tmp\\manual.pdf",
      blob_saved_to: "C:\\tmp\\manual.pdf",
    },
  ]);
});

test("buildToolResultFields restores ReadMcpResource blob paths from transcript JSON text", () => {
  const base = createToolCall("tc-mcp-history", "ReadMcpResource", {
    server: "docs",
    uri: "file://manual.pdf",
  });
  const transcriptJson = JSON.stringify({
    contents: [
      {
        uri: "file://manual.pdf",
        mimeType: "application/pdf",
        text: "[Resource from docs at file://manual.pdf] Saved to C:\\tmp\\manual.pdf",
        blobSavedTo: "C:\\tmp\\manual.pdf",
      },
    ],
  });
  const fields = buildToolResultFields(false, transcriptJson, base, {
    type: "tool_result",
    tool_use_id: "tc-mcp-history",
    content: transcriptJson,
  });

  assert.deepEqual(fields.content, [
    {
      type: "mcp_resource",
      uri: "file://manual.pdf",
      mime_type: "application/pdf",
      text: "[Resource from docs at file://manual.pdf] Saved to C:\\tmp\\manual.pdf",
      blob_saved_to: "C:\\tmp\\manual.pdf",
    },
  ]);
});

test("buildToolResultFields marks ReadMcpResource error output as failed", () => {
  const base = createToolCall("tc-mcp-error", "ReadMcpResource", {
    server: "docs",
    uri: "file://missing.md",
  });
  const fields = buildToolResultFields(
    false,
    {
      contents: [],
      error: "resource not found",
    },
    base,
  );

  assert.equal(fields.status, "failed");
  assert.equal(fields.raw_output, "Error: resource not found");
  assert.deepEqual(fields.content, [
    {
      type: "content",
      content: { type: "text", text: "Error: resource not found" },
    },
  ]);
});

test("buildToolResultFields preserves WebFetch artifactRead only as metadata", () => {
  const base = createToolCall("tc-web-fetch-artifact", "WebFetch", {
    url: "https://artifact.local/dashboard",
  });
  const fields = buildToolResultFields(
    false,
    {
      bytes: 128,
      code: 200,
      codeText: "OK",
      durationMs: 42,
      result: "Artifact content summary",
      url: "https://artifact.local/dashboard",
      artifactRead: {
        slug: "dashboard",
        ver: "v3",
      },
    },
    base,
  );

  assert.equal(fields.raw_output, "Artifact content summary");
  assert.equal(fields.raw_output?.includes("artifactRead"), false);
  assert.deepEqual(fields.output_metadata, {
    web_fetch: {
      artifact_read: {
        slug: "dashboard",
        ver: "v3",
      },
    },
  });
});

test("buildToolResultFields preserves Agent resolvedModel metadata", () => {
  const base = createToolCall("tc-agent-model", "Agent", {
    description: "review changes",
    prompt: "Review the branch",
  });
  const fields = buildToolResultFields(
    false,
    {
      agentId: "agent-1",
      agentType: "reviewer",
      resolvedModel: "claude-sonnet-4-7",
      content: [{ type: "text", text: "Done" }],
      totalToolUseCount: 1,
      totalDurationMs: 100,
      totalTokens: 25,
      usage: {
        input_tokens: 10,
        output_tokens: 15,
        cache_creation_input_tokens: null,
        cache_read_input_tokens: null,
        server_tool_use: null,
        service_tier: null,
        cache_creation: null,
      },
      status: "completed",
      prompt: "Review the branch",
    },
    base,
  );

  assert.equal(fields.title, "Agent: reviewer");
  assert.deepEqual(fields.output_metadata, {
    agent: {
      resolved_model: "claude-sonnet-4-7",
    },
  });
});

test("buildToolResultFields keeps Agent input name while preserving resolvedModel metadata", () => {
  const base = createToolCall("tc-agent-named-model", "Agent", {
    description: "review changes",
    prompt: "Review the branch",
    name: "review-worker",
    subagent_type: "general-purpose",
    model: "opus",
  });
  const fields = buildToolResultFields(
    false,
    {
      agentId: "agent-1",
      agentType: "general-purpose",
      resolvedModel: "claude-opus-4-8",
      content: [{ type: "text", text: "Done" }],
      status: "completed",
      prompt: "Review the branch",
    },
    base,
  );

  assert.equal(fields.title, undefined);
  assert.deepEqual(fields.output_metadata, {
    agent: {
      resolved_model: "claude-opus-4-8",
    },
  });
});

test("unwrapToolUseResult extracts error/content payload", () => {
  const parsed = unwrapToolUseResult({
    is_error: true,
    content: [{ text: "failure output" }],
  });
  assert.equal(parsed.isError, true);
  assert.deepEqual(parsed.content, [{ text: "failure output" }]);
});

test("permissionResultFromOutcome maps selected and cancelled outcomes", () => {
  const allow = permissionResultFromOutcome(
    { outcome: "selected", option_id: "allow_always" },
    "tool-1",
    { command: "echo test" },
    [],
  );
  assert.equal(allow.behavior, "allow");
  if (allow.behavior === "allow") {
    assert.deepEqual(allow.updatedInput, { command: "echo test" });
  }

  const deny = permissionResultFromOutcome(
    { outcome: "selected", option_id: "reject_once" },
    "tool-1",
    { command: "echo test" },
  );
  assert.equal(deny.behavior, "deny");
  assert.match(String(deny.message), /Permission denied/);

  const cancelled = permissionResultFromOutcome(
    { outcome: "cancelled" },
    "tool-1",
    { command: "echo test" },
  );
  assert.equal(cancelled.behavior, "deny");
  assert.match(String(cancelled.message), /cancelled/i);
});

test("permissionOptionsFromSuggestions uses session label when only session scope is suggested", () => {
  const options = permissionOptionsFromSuggestions([
    {
      type: "setMode",
      mode: "acceptEdits",
      destination: "session",
    },
  ]);
  assert.deepEqual(options, [
    { option_id: "allow_once", name: "Allow once", kind: "allow_once" },
    { option_id: "allow_session", name: "Allow for session", kind: "allow_session" },
    { option_id: "reject_once", name: "Deny", kind: "reject_once" },
  ]);
});

test("permissionOptionsFromSuggestions uses persistent label when settings scope is suggested", () => {
  const options = permissionOptionsFromSuggestions([
    {
      type: "addRules",
      behavior: "allow",
      destination: "localSettings",
      rules: [{ toolName: "Bash", ruleContent: "npm install" }],
    },
  ]);
  assert.deepEqual(options, [
    { option_id: "allow_once", name: "Allow once", kind: "allow_once" },
    { option_id: "allow_always", name: "Always allow", kind: "allow_always" },
    { option_id: "reject_once", name: "Deny", kind: "reject_once" },
  ]);
});

test("permissionResultFromOutcome keeps Bash allow_always suggestions unchanged", () => {
  const allow = permissionResultFromOutcome(
    { outcome: "selected", option_id: "allow_always" },
    "tool-1",
    { command: "npm install" },
    [
      {
        type: "addRules",
        behavior: "allow",
        destination: "localSettings",
        rules: [
          { toolName: "Bash", ruleContent: "npm install" },
          { toolName: "WebFetch", ruleContent: "https://example.com" },
          { toolName: "Bash", ruleContent: "dir /B" },
        ],
      },
    ],
    "Bash",
  );

  assert.equal(allow.behavior, "allow");
  if (allow.behavior !== "allow") {
    throw new Error("expected allow permission result");
  }
  assert.deepEqual(allow.updatedPermissions, [
    {
      type: "addRules",
      behavior: "allow",
      destination: "localSettings",
      rules: [
        { toolName: "Bash", ruleContent: "npm install" },
        { toolName: "WebFetch", ruleContent: "https://example.com" },
        { toolName: "Bash", ruleContent: "dir /B" },
      ],
    },
  ]);
});

test("permissionResultFromOutcome keeps Write allow_session suggestions unchanged", () => {
  const suggestions = [
    {
      type: "addRules" as const,
      behavior: "allow" as const,
      destination: "session" as const,
      rules: [{ toolName: "Write", ruleContent: "C:\\work\\foo.txt" }],
    },
  ];
  const allow = permissionResultFromOutcome(
    { outcome: "selected", option_id: "allow_session" },
    "tool-2",
    { file_path: "C:\\work\\foo.txt" },
    suggestions,
    "Write",
  );

  assert.equal(allow.behavior, "allow");
  if (allow.behavior !== "allow") {
    throw new Error("expected allow permission result");
  }
  assert.deepEqual(allow.updatedPermissions, suggestions);
});

test("permissionResultFromOutcome falls back to session tool rule for allow_session when suggestions are missing", () => {
  const allow = permissionResultFromOutcome(
    { outcome: "selected", option_id: "allow_session" },
    "tool-3",
    { file_path: "C:\\work\\bar.txt" },
    undefined,
    "Write",
  );

  assert.equal(allow.behavior, "allow");
  if (allow.behavior !== "allow") {
    throw new Error("expected allow permission result");
  }
  assert.deepEqual(allow.updatedPermissions, [
    {
      type: "addRules",
      behavior: "allow",
      destination: "session",
      rules: [{ toolName: "Write" }],
    },
  ]);
});

test("permissionResultFromOutcome falls back to localSettings rule for allow_always when only session suggestions exist", () => {
  const allow = permissionResultFromOutcome(
    { outcome: "selected", option_id: "allow_always" },
    "tool-4",
    { file_path: "C:\\work\\baz.txt" },
    [
      {
        type: "addRules",
        behavior: "allow",
        destination: "session",
        rules: [{ toolName: "Write", ruleContent: "C:\\work\\baz.txt" }],
      },
    ],
    "Write",
  );

  assert.equal(allow.behavior, "allow");
  if (allow.behavior !== "allow") {
    throw new Error("expected allow permission result");
  }
  assert.deepEqual(allow.updatedPermissions, [
    {
      type: "addRules",
      rules: [{ toolName: "Write" }],
      behavior: "allow",
      destination: "localSettings",
    },
  ]);
});

test("looksLikeAuthRequired detects login hints", () => {
  assert.equal(looksLikeAuthRequired("Please run /login to continue"), true);
  assert.equal(looksLikeAuthRequired("normal tool output"), false);
});

test("agent sdk version compatibility check matches pinned version", () => {
  assert.equal(resolveInstalledAgentSdkVersion(), "0.3.190");
  assert.equal(agentSdkVersionCompatibilityError(), undefined);
});

test("mapSessionMessagesToUpdates maps message content blocks", () => {
  const updates = mapSessionMessagesToUpdates([
    {
      type: "user",
      uuid: "u1",
      session_id: "s1",
      parent_tool_use_id: null,
      message: {
        role: "user",
        content: [{ type: "text", text: "Top-level user prompt" }],
      },
    },
    {
      type: "assistant",
      uuid: "a1",
      session_id: "s1",
      parent_tool_use_id: null,
      message: {
        id: "msg-1",
        role: "assistant",
        content: [
          { type: "tool_use", id: "tool-1", name: "Bash", input: { command: "echo hello" } },
          { type: "text", text: "Nested assistant final" },
        ],
        usage: {
          input_tokens: 11,
          output_tokens: 7,
          cache_read_input_tokens: 5,
          cache_creation_input_tokens: 3,
        },
      },
    },
    {
      type: "user",
      uuid: "u2",
      session_id: "s1",
      parent_tool_use_id: null,
      message: {
        role: "user",
        content: [
          {
            type: "tool_result",
            tool_use_id: "tool-1",
            content: "ok",
            is_error: false,
          },
        ],
      },
    },
  ]);

  const variantCounts = new Map<string, number>();
  for (const update of updates) {
    variantCounts.set(update.type, (variantCounts.get(update.type) ?? 0) + 1);
  }

  assert.equal(variantCounts.get("user_message_chunk"), 1);
  assert.equal(variantCounts.get("agent_message_chunk"), 1);
  assert.equal(variantCounts.get("tool_call"), 1);
  assert.equal(variantCounts.get("tool_call_update"), 1);
  const userChunk = updates.find(
    (
      update,
    ): update is Extract<import("./types.js").SessionUpdate, { type: "user_message_chunk" }> =>
      update.type === "user_message_chunk",
  );
  const agentChunk = updates.find(
    (
      update,
    ): update is Extract<import("./types.js").SessionUpdate, { type: "agent_message_chunk" }> =>
      update.type === "agent_message_chunk",
  );
  const toolCall = updates.find(
    (update): update is Extract<import("./types.js").SessionUpdate, { type: "tool_call" }> =>
      update.type === "tool_call",
  );
  const toolCallUpdate = updates.find(
    (
      update,
    ): update is Extract<import("./types.js").SessionUpdate, { type: "tool_call_update" }> =>
      update.type === "tool_call_update",
  );
  assert.equal(userChunk?.source_message_uuid, "u1");
  assert.equal(agentChunk?.source_message_uuid, "a1");
  assert.equal(toolCall?.tool_call.source_message_uuid, "a1");
  assert.equal(
    toolCallUpdate?.tool_call_update.source_message_uuid,
    "u2",
  );
});

test("mapSessionMessagesToUpdates suppresses ToolSearch history blocks", () => {
  const updates = mapSessionMessagesToUpdates([
    {
      type: "assistant",
      uuid: "a1",
      session_id: "s1",
      parent_tool_use_id: null,
      message: {
        role: "assistant",
        content: [
          {
            type: "server_tool_use",
            id: "tool-search-1",
            name: "ToolSearch",
            input: { query: "src/" },
          },
          { type: "tool_use", id: "tool-bash", name: "Bash", input: { command: "echo ok" } },
        ],
      },
    },
    {
      type: "user",
      uuid: "u1",
      session_id: "s1",
      parent_tool_use_id: null,
      message: {
        role: "user",
        content: [
          {
            type: "tool_search_tool_result",
            tool_use_id: "tool-search-1",
            content: "matched src/main.rs",
            is_error: false,
          },
          {
            type: "tool_result",
            tool_use_id: "tool-bash",
            content: "ok",
            is_error: false,
          },
        ],
      },
    },
  ]);

  const toolCalls = updates.filter((update) => update.type === "tool_call");
  const toolUpdates = updates.filter((update) => update.type === "tool_call_update");

  assert.deepEqual(
    toolCalls.map((update) => update.tool_call.tool_call_id),
    ["tool-bash"],
  );
  assert.deepEqual(
    toolUpdates.map((update) => update.tool_call_update.tool_call_id),
    ["tool-bash"],
  );
});

test("mapSessionMessagesToUpdates preserves parallel tool results", () => {
  const updates = mapSessionMessagesToUpdates([
    {
      type: "assistant",
      uuid: "a1",
      session_id: "s1",
      parent_tool_use_id: null,
      message: {
        role: "assistant",
        content: [
          { type: "tool_use", id: "tool-a", name: "Bash", input: { command: "echo a" } },
          { type: "tool_use", id: "tool-b", name: "Bash", input: { command: "echo b" } },
        ],
      },
    },
    {
      type: "user",
      uuid: "u1",
      session_id: "s1",
      parent_tool_use_id: null,
      message: {
        role: "user",
        content: [
          {
            type: "tool_result",
            tool_use_id: "tool-b",
            content: "result b",
            is_error: false,
          },
          {
            type: "tool_result",
            tool_use_id: "tool-a",
            content: "result a",
            is_error: false,
          },
        ],
      },
    },
  ]);

  const toolCalls = updates.filter((update) => update.type === "tool_call");
  const toolUpdates = updates.filter((update) => update.type === "tool_call_update");

  assert.deepEqual(
    toolCalls.map((update) => update.tool_call.tool_call_id),
    ["tool-a", "tool-b"],
  );
  assert.deepEqual(
    toolUpdates.map((update) => update.tool_call_update.tool_call_id),
    ["tool-b", "tool-a"],
  );
  assert.deepEqual(
    toolUpdates.map((update) => update.tool_call_update.fields.raw_output),
    ["result b", "result a"],
  );
});

test("mapSessionMessagesToUpdates maps task system records from resume history", () => {
  const updates = mapSessionMessagesToUpdates([
    {
      type: "assistant",
      uuid: "assistant-agent",
      session_id: "s1",
      parent_tool_use_id: null,
      message: {
        role: "assistant",
        content: [
          {
            type: "tool_use",
            id: "tool-agent",
            name: "Agent",
            input: { prompt: "Inspect the migration smoke" },
          },
        ],
      },
    },
    {
      type: "system",
      uuid: "system-bg-start",
      session_id: "s1",
      parent_tool_use_id: null,
      message: {
        type: "system",
        subtype: "task_started",
        task_id: "task-bg",
        tool_use_id: "tool-agent",
        description: "Inspect the migration smoke",
        subagent_type: "general-purpose",
      },
    },
    {
      type: "system",
      uuid: "system-bg-update",
      session_id: "s1",
      parent_tool_use_id: null,
      message: {
        type: "system",
        subtype: "task_updated",
        task_id: "task-bg",
        patch: {
          status: "running",
          description: "Checking runtime MCP resources",
          is_backgrounded: true,
        },
      },
    },
    {
      type: "system",
      uuid: "system-remote-start",
      session_id: "s1",
      parent_tool_use_id: null,
      message: {
        type: "system",
        subtype: "task_started",
        task_id: "task-remote",
        description: "Remote agent is still running",
        task_type: "remote_agent",
        task_description: "Remote agent smoke",
        prompt: "Continue remotely",
      },
    },
    {
      type: "system",
      uuid: "system-mcp-start",
      session_id: "s1",
      parent_tool_use_id: null,
      message: {
        type: "system",
        subtype: "task_started",
        task_id: "task-mcp",
        description: "MCP task is still running",
        task_type: "mcp",
        workflow_name: "docs",
        prompt: "Read resource",
      },
    },
  ]);

  const taskUpdates = updates.filter((update) => update.type === "task_state_update");
  assert.equal(taskUpdates.length, 4);
  assert.deepEqual(taskUpdates.at(1), {
    type: "task_state_update",
    source: "task_lifecycle",
    tasks: [
      {
        task_id: "task-bg",
        subject: "Inspect the migration smoke",
        description: "Checking runtime MCP resources",
        status: "in_progress",
        blocks: [],
        blocked_by: [],
        metadata: {
          subagent_type: "general-purpose",
          is_backgrounded: true,
        },
        source_tool_call_id: "tool-agent",
      },
    ],
    removed_task_ids: [],
    is_complete_snapshot: false,
  });
  assert.deepEqual(taskUpdates.at(2), {
    type: "task_state_update",
    source: "task_lifecycle",
    tasks: [
      {
        task_id: "task-remote",
        subject: "Remote agent smoke",
        description: "Remote agent is still running",
        status: "in_progress",
        blocks: [],
        blocked_by: [],
        metadata: {
          task_description: "Remote agent smoke",
          task_type: "remote_agent",
          prompt: "Continue remotely",
        },
      },
    ],
    removed_task_ids: [],
    is_complete_snapshot: false,
  });
  assert.deepEqual(taskUpdates.at(3), {
    type: "task_state_update",
    source: "task_lifecycle",
    tasks: [
      {
        task_id: "task-mcp",
        subject: "docs",
        description: "MCP task is still running",
        status: "in_progress",
        blocks: [],
        blocked_by: [],
        metadata: {
          task_type: "mcp",
          workflow_name: "docs",
          prompt: "Read resource",
        },
      },
    ],
    removed_task_ids: [],
    is_complete_snapshot: false,
  });
});

test("handleResultMessage emits terminal reason on successful turn completion", () => {
  const session = makeSessionState();

  const events = captureBridgeEvents(() => {
    handleResultMessage(session, {
      type: "result",
      subtype: "success",
      terminal_reason: "completed",
    });
  });

  const lastEvent = events.at(-1);
  assert.deepEqual(lastEvent, {
    event: "turn_complete",
    session_id: "session-1",
    terminal_reason: "completed",
  });
});

test("handleResultMessage ignores success result telemetry fields", () => {
  const session = makeSessionState();

  const events = captureBridgeEvents(() => {
    handleResultMessage(session, {
      type: "result",
      subtype: "success",
      ttft_stream_ms: 42,
      time_to_request_ms: 12,
      time_to_request_from_spawn_ms: 7,
      warm_spare_claimed: true,
    });
  });

  assert.deepEqual(events.at(-1), {
    event: "turn_complete",
    session_id: "session-1",
  });
});

test("handleResultMessage emits repeated turn_complete while background work remains open", () => {
  const session = makeSessionState();

  const events = captureBridgeEvents(() => {
    emitToolCall(session, "tool-monitor", "Monitor", {
      description: "watch deploy logs",
      timeout_ms: 30000,
      persistent: false,
      command: "tail -f deploy.log",
    });
    emitToolResultUpdate(session, "tool-monitor", false, {
      taskId: "monitor-1",
      timeoutMs: 30000,
      persistent: false,
    });
    handleResultMessage(session, {
      type: "result",
      subtype: "success",
      terminal_reason: "completed",
    });
    handleResultMessage(session, {
      type: "result",
      subtype: "success",
      terminal_reason: "completed",
    });
  });

  assert.deepEqual(
    events.filter((event) => event.event === "turn_complete"),
    [
      {
        event: "turn_complete",
        session_id: "session-1",
        terminal_reason: "completed",
      },
      {
        event: "turn_complete",
        session_id: "session-1",
        terminal_reason: "completed",
      },
    ],
  );
  assert.equal(session.toolCalls.get("tool-monitor")?.status, "in_progress");
  assert.equal(session.taskToolUseIds.get("monitor-1"), "tool-monitor");
});

test("handleResultMessage emits terminal reason on turn errors", () => {
  const session = makeSessionState();

  const events = captureBridgeEvents(() => {
    handleResultMessage(session, {
      type: "result",
      subtype: "error_max_turns",
      terminal_reason: "max_turns",
      errors: ["max turns exceeded"],
    });
  });

  const lastEvent = events.at(-1);
  assert.deepEqual(lastEvent, {
    event: "turn_error",
    session_id: "session-1",
    message: "max turns exceeded",
    error_kind: "plan_limit",
    sdk_result_subtype: "error_max_turns",
    terminal_reason: "max_turns",
  });
});

test("handleResultMessage emits typed turn error classifications for SDK assistant errors", () => {
  const cases = [
    ["model_not_found", "model_unavailable"],
    ["oauth_org_not_allowed", "account_access"],
    ["overloaded", "transient_service"],
  ] as const;

  for (const [assistantError, errorKind] of cases) {
    const session = makeSessionState();
    session.lastAssistantError = assistantError;

    const events = captureBridgeEvents(() => {
      handleResultMessage(session, {
        type: "result",
        subtype: "error_during_execution",
        errors: [`failed with ${assistantError}`],
      });
    });

    assert.deepEqual(events.at(-1), {
      event: "turn_error",
      session_id: "session-1",
      message: `failed with ${assistantError}`,
      error_kind: errorKind,
      sdk_result_subtype: "error_during_execution",
      assistant_error: assistantError,
    });
  }
});

test("handleResultMessage preserves result api error status", () => {
  const session = makeSessionState();
  session.lastAssistantError = "overloaded";

  const events = captureBridgeEvents(() => {
    handleResultMessage(session, {
      type: "result",
      subtype: "error_during_execution",
      errors: ["service overloaded"],
      api_error_status: 529,
    });
  });

  assert.deepEqual(events.at(-1), {
    event: "turn_error",
    session_id: "session-1",
    message: "service overloaded",
    error_kind: "transient_service",
    sdk_result_subtype: "error_during_execution",
    assistant_error: "overloaded",
    api_error_status: 529,
  });
});

test("mapSessionMessagesToUpdates ignores unsupported records", () => {
  const updates = mapSessionMessagesToUpdates([
    {
      type: "user",
      uuid: "u1",
      session_id: "s1",
      parent_tool_use_id: null,
      message: {
        role: "assistant",
        content: [{ type: "thinking", thinking: "h" }],
      },
    },
    {
      type: "system",
      uuid: "system-unsupported",
      session_id: "s1",
      parent_tool_use_id: null,
      message: {
        type: "system",
        subtype: "compact_boundary",
        content: [{ type: "text", text: "internal system text" }],
      },
    },
  ]);
  assert.equal(updates.length, 0);
});

test("mapSdkSessions normalizes and sorts sessions", () => {
  const mapped = mapSdkSessions([
    {
      sessionId: "older",
      summary: " Older summary ",
      lastModified: 100,
      fileSize: 10,
      cwd: "C:/work",
    },
    {
      sessionId: "latest",
      summary: "",
      lastModified: 200,
      fileSize: 20,
      customTitle: "Custom title",
      gitBranch: "main",
      firstPrompt: "hello",
    },
  ]);

  assert.deepEqual(mapped, [
    {
      session_id: "latest",
      summary: "Custom title",
      last_modified_ms: 200,
      file_size_bytes: 20,
      git_branch: "main",
      custom_title: "Custom title",
      first_prompt: "hello",
    },
    {
      session_id: "older",
      summary: "Older summary",
      last_modified_ms: 100,
      file_size_bytes: 10,
      cwd: "C:/work",
    },
  ]);
});

test("buildSessionListOptions scopes repo-local listings to worktrees", () => {
  assert.deepEqual(buildSessionListOptions("C:/repo"), {
    dir: "C:/repo",
    includeWorktrees: true,
    limit: 50,
  });
  assert.deepEqual(buildSessionListOptions(undefined), {
    limit: 50,
  });
});

test("buildToolResultFields renders file_unchanged Read results compactly", () => {
  const base = createToolCall("tc-read", "Read", { file_path: "src/main.rs" });
  const fields = buildToolResultFields(
    false,
    {
      type: "file_unchanged",
      file: { filePath: "src/main.rs" },
    },
    base,
    {
      result: {
        type: "file_unchanged",
        file: { filePath: "src/main.rs" },
      },
    },
  );

  assert.equal(fields.raw_output, "File unchanged: src/main.rs");
  assert.deepEqual(fields.content, [
    { type: "content", content: { type: "text", text: "File unchanged: src/main.rs" } },
  ]);
});

test("buildToolResultFields renders array-wrapped file_unchanged Read results compactly", () => {
  const base = createToolCall("tc-read", "Read", { file_path: "src/lib.rs" });
  const fields = buildToolResultFields(
    false,
    [],
    base,
    {
      result: [
        {
          type: "file_unchanged",
          file: { filePath: "src/lib.rs" },
        },
      ],
    },
  );

  assert.equal(fields.raw_output, "File unchanged: src/lib.rs");
});

test("buildToolResultFields uses Agent output agentType as task title", () => {
  const base = createToolCall("tc-agent", "Agent", { prompt: "Review tests" });
  const fields = buildToolResultFields(
    false,
    {
      agentId: "agent-1",
      agentType: "reviewer",
      content: [{ type: "text", text: "Done" }],
      totalToolUseCount: 0,
      totalDurationMs: 10,
      totalTokens: 20,
      usage: {},
      status: "completed",
      prompt: "Review tests",
    },
    base,
  );

  assert.equal(fields.title, "Agent: reviewer");
});

test("buildToolResultFields reads array-wrapped Agent output agentType", () => {
  const base = createToolCall("tc-agent", "Agent", { prompt: "Review tests" });
  const fields = buildToolResultFields(
    false,
    [],
    base,
    {
      result: [
        {
          agentId: "agent-1",
          agentType: "planner",
          content: [{ type: "text", text: "Done" }],
          status: "completed",
        },
      ],
    },
  );

  assert.equal(fields.title, "Agent: planner");
});

test("buildToolResultFields leaves worktree title unchanged on completed output", () => {
  const enterBase = createToolCall("tc-enter", "EnterWorktree", { name: "feature-auth" });
  const enterFields = buildToolResultFields(
    false,
    {
      message: "Entered worktree feature-auth",
      worktreeBranch: "feature-auth",
      worktreePath: "C:\\repo\\.worktrees\\feature-auth",
    },
    enterBase,
  );
  assert.equal(enterFields.title, undefined);

  const exitBase = createToolCall("tc-exit", "ExitWorktree", { action: "keep" });
  const exitFields = buildToolResultFields(
    false,
    {
      message: "Exited worktree feature-auth",
      worktreePath: "C:\\repo\\.worktrees\\feature-auth",
    },
    exitBase,
  );
  assert.equal(exitFields.title, undefined);
});

test("buildToolResultFields renders worktree location without raw JSON", () => {
  const enterBase = createToolCall("tc-enter", "EnterWorktree", { name: "feature-auth" });
  const enterFields = buildToolResultFields(
    false,
    {
      message: "Entered worktree feature-auth",
      worktreeBranch: "feature-auth",
      worktreePath: "C:\\repo\\.worktrees\\feature-auth",
    },
    enterBase,
  );
  assert.equal(enterFields.raw_output, "Branch: feature-auth");
  assert.deepEqual(enterFields.content, [
    { type: "content", content: { type: "text", text: "Branch: feature-auth" } },
  ]);

  const exitBase = createToolCall("tc-exit", "ExitWorktree", { action: "keep" });
  const exitFields = buildToolResultFields(
    false,
    {
      message: "Exited worktree feature-auth",
      worktreePath: "C:\\repo\\.worktrees\\feature-auth",
    },
    exitBase,
  );
  assert.equal(exitFields.raw_output, "Path: C:\\repo\\.worktrees\\feature-auth");
  assert.deepEqual(exitFields.content, [
    {
      type: "content",
      content: { type: "text", text: "Path: C:\\repo\\.worktrees\\feature-auth" },
    },
  ]);
});

test("buildToolResultFields renders cron outputs as structured text without raw JSON", () => {
  const createBase = createToolCall("tc-cron-create", "CronCreate", {
    cron: "30 9 * * 1",
    prompt: "Send weekly status",
  });
  const createFields = buildToolResultFields(
    false,
    {
      id: "schedule-1",
      humanSchedule: "every Monday at 09:30",
      recurring: true,
      durable: false,
    },
    createBase,
  );
  assert.equal(
    createFields.raw_output,
    "Schedule ID: schedule-1\nSchedule: Every Monday at 09:30\nRecurring: yes\nDurable: no",
  );
  assert.deepEqual(createFields.content, [
    {
      type: "content",
      content: {
        type: "text",
        text: "Schedule ID: schedule-1\nSchedule: Every Monday at 09:30\nRecurring: yes\nDurable: no",
      },
    },
  ]);
  assert.equal(createFields.raw_output?.includes("{"), false);

  const deleteBase = createToolCall("tc-cron-delete", "CronDelete", { id: "schedule-1" });
  const deleteFields = buildToolResultFields(false, { id: "schedule-1" }, deleteBase);
  assert.equal(deleteFields.raw_output, "Schedule ID: schedule-1");

  const listBase = createToolCall("tc-cron-list", "CronList", {});
  const listFields = buildToolResultFields(false, { jobs: [] }, listBase);
  assert.equal(listFields.raw_output, "Jobs: none");

  const singleListFields = buildToolResultFields(
    false,
    {
      jobs: [
        {
          id: "schedule-2",
          cron: "7 * * * *",
          humanSchedule: "Every hour at :07",
          prompt: "Send hourly tick",
          recurring: true,
          durable: false,
        },
      ],
    },
    listBase,
  );
  assert.equal(
    singleListFields.raw_output,
    "Schedule ID: schedule-2\nCron: 7 * * * *\nSchedule: Every hour at minute 07\nPrompt: Send hourly tick\nRecurring: yes\nDurable: no",
  );
});

test("buildToolResultFields preserves full CronList prompt from transcript JSON", () => {
  const base = createToolCall("tc-cron-list-history", "CronList", {});
  const fullPrompt = `Review the branch and write a status update. ${"Keep every detail. ".repeat(80)}END`;
  const transcriptJson = JSON.stringify({
    jobs: [
      {
        id: "schedule-long",
        cron: "*/5 * * * *",
        humanSchedule: "every 5 minutes",
        prompt: fullPrompt,
        recurring: false,
        durable: true,
      },
    ],
  });

  const fields = buildToolResultFields(false, transcriptJson, base, {
    type: "tool_result",
    tool_use_id: "tc-cron-list-history",
    content: transcriptJson,
  });

  assert.equal(fields.raw_output?.includes(fullPrompt), true);
  assert.equal(fields.raw_output?.includes("END"), true);
  assert.equal(fields.raw_output?.includes('"jobs"'), false);
  assert.deepEqual(fields.content, [
    {
      type: "content",
      content: {
        type: "text",
        text: `Schedule ID: schedule-long\nCron: */5 * * * *\nSchedule: Every 5 minutes\nPrompt: ${fullPrompt}\nRecurring: no\nDurable: yes`,
      },
    },
  ]);
});

test("buildToolResultFields renders readable cron schedule text from common cron expressions", () => {
  const base = createToolCall("tc-cron-readable", "CronList", {});
  const fields = buildToolResultFields(false, {
    jobs: [
      { id: "every-minute", cron: "* * * * *", prompt: "minute", recurring: true },
      { id: "every-five-minutes", cron: "*/5 * * * *", prompt: "minutes", recurring: true },
      {
        id: "hourly-minute",
        cron: "7 * * * *",
        humanSchedule: "Every hour at :07",
        prompt: "hourly",
        recurring: true,
      },
      { id: "every-two-hours", cron: "0 */2 * * *", prompt: "hours", recurring: true },
      { id: "daily", cron: "30 9 * * *", prompt: "daily", recurring: true },
      { id: "weekly", cron: "30 9 * * 1", prompt: "weekly", recurring: true },
      { id: "monthly", cron: "30 9 15 * *", prompt: "monthly", recurring: true },
      { id: "yearly", cron: "30 9 15 6 *", prompt: "yearly", recurring: true },
      { id: "complex", cron: "0 9 1 * 1", prompt: "complex", recurring: true },
    ],
  }, base);

  assert.equal(fields.raw_output?.includes("Cron: 7 * * * *"), false);
  assert.equal(fields.raw_output?.includes("Recurring:"), false);
  assert.equal(fields.raw_output?.includes("Durable:"), false);
  assert.equal(fields.raw_output?.includes("Schedule: Every minute"), true);
  assert.equal(fields.raw_output?.includes("Schedule: Every 5 minutes"), true);
  assert.equal(fields.raw_output?.includes("Schedule: Every hour at minute 07"), true);
  assert.equal(fields.raw_output?.includes("Schedule: Every 2 hours on the hour"), true);
  assert.equal(fields.raw_output?.includes("Schedule: Every day at 09:30"), true);
  assert.equal(fields.raw_output?.includes("Schedule: Every Monday at 09:30"), true);
  assert.equal(fields.raw_output?.includes("Schedule: Every month on day 15 at 09:30"), true);
  assert.equal(fields.raw_output?.includes("Schedule: Every June 15 at 09:30"), true);
  assert.equal(fields.raw_output?.includes("Cron: 0 9 1 * 1"), true);
  assert.equal(fields.raw_output?.split("__cron_list_job_divider__").length, 9);
});

test("buildToolResultFields renders ScheduleWakeup output as structured text", () => {
  const base = createToolCall("tc-wakeup", "ScheduleWakeup", {
    delaySeconds: 30,
    reason: "Retry after runtime clamp",
    prompt: "/loop keep checking",
  });
  const fields = buildToolResultFields(
    false,
    {
      scheduledFor: 1_779_990_000_000,
      clampedDelaySeconds: 90,
      wasClamped: true,
    },
    base,
  );

  assert.match(
    fields.raw_output ?? "",
    /^Scheduled for: \d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2} local\nActual delay: 1m 30s\nClamped: yes$/,
  );
  assert.equal(fields.raw_output?.includes("{"), false);
  assert.equal(fields.raw_output?.includes("1779990000000"), false);
  assert.deepEqual(fields.content, [
    {
      type: "content",
      content: {
        type: "text",
        text: fields.raw_output,
      },
    },
  ]);
});

test("buildToolResultFields parses ScheduleWakeup transcript JSON", () => {
  const base = createToolCall("tc-wakeup-history", "ScheduleWakeup", {
    delaySeconds: 3600,
    reason: "Wake at the next loop interval",
    prompt: "/loop continue",
  });
  const transcriptJson = JSON.stringify({
    scheduledFor: 1_779_990_000_000,
    clampedDelaySeconds: 3600,
    wasClamped: false,
  });

  const fields = buildToolResultFields(false, transcriptJson, base, {
    type: "tool_result",
    tool_use_id: "tc-wakeup-history",
    content: transcriptJson,
  });

  assert.match(fields.raw_output ?? "", /Actual delay: 1h\nClamped: no$/);
  assert.equal(fields.raw_output?.includes('"scheduledFor"'), false);
});

test("buildToolResultFields renders PushNotification output as structured text", () => {
  const base = createToolCall("tc-push-notification", "PushNotification", {
    message: "Build finished",
    status: "proactive",
  });
  const fields = buildToolResultFields(
    false,
    {
      message: "Build finished",
      pushSent: false,
      localSent: true,
      disabledReason: "config_off",
      idleSec: 90,
      hasFocus: false,
      sentAt: "2026-06-05T12:34:56.000Z",
    },
    base,
  );

  assert.match(
    fields.raw_output ?? "",
    /^Push sent: no\nLocal sent: yes\nDisabled reason: notifications disabled\nIdle time: 1m 30s\nApp focused: no\nSent at: \d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2} local$/,
  );
  assert.equal(fields.raw_output?.includes("Result:"), false);
  assert.equal(fields.raw_output?.includes("{"), false);
  assert.deepEqual(fields.content, [
    {
      type: "content",
      content: {
        type: "text",
        text: fields.raw_output,
      },
    },
  ]);
});

test("buildToolResultFields parses PushNotification transcript JSON", () => {
  const base = createToolCall("tc-push-notification-history", "PushNotification", {
    message: "Deploy completed",
    status: "proactive",
  });
  const transcriptJson = JSON.stringify({
    message: "Notification queued",
    pushSent: true,
    localSent: false,
    disabledReason: "no_transport",
    idleSec: 3600,
    hasFocus: true,
    sentAt: "not an iso timestamp",
  });

  const fields = buildToolResultFields(false, transcriptJson, base, {
    type: "tool_result",
    tool_use_id: "tc-push-notification-history",
    content: transcriptJson,
  });

  assert.equal(
    fields.raw_output,
    "Result: Notification queued\nPush sent: yes\nLocal sent: no\nDisabled reason: no notification transport\nIdle time: 1h\nApp focused: yes\nSent at: not an iso timestamp",
  );
  assert.equal(fields.raw_output?.includes('"pushSent"'), false);
});

test("buildToolResultFields renders RemoteTrigger summary without raw JSON", () => {
  const base = createToolCall("tc-remote-trigger", "RemoteTrigger", {
    action: "run",
    trigger_id: "deploy-prod",
  });
  const fields = buildToolResultFields(
    false,
    {
      status: 200,
      json: '{\n  "ok": true,\n  "run_id": "run-1"\n}',
      summary: "Trigger completed",
    },
    base,
  );

  assert.equal(fields.status, "completed");
  assert.equal(fields.raw_output, "Status: 200\nSummary: Trigger completed");
  assert.equal(fields.raw_output?.includes("run_id"), false);
  assert.deepEqual(fields.content, [
    {
      type: "content",
      content: {
        type: "text",
        text: "Status: 200\nSummary: Trigger completed",
      },
    },
  ]);
});

test("buildToolResultFields renders RemoteTrigger response when summary is absent", () => {
  const base = createToolCall("tc-remote-trigger-response", "RemoteTrigger", {
    action: "get",
    trigger_id: "deploy-prod",
  });
  const fields = buildToolResultFields(
    false,
    {
      status: 200,
      json: '{\n  "ok": true,\n  "trigger_id": "deploy-prod"\n}',
    },
    base,
  );

  assert.equal(fields.status, "completed");
  assert.equal(fields.raw_output, 'Status: 200\nResponse: {"ok":true,"trigger_id":"deploy-prod"}');
  assert.equal(fields.raw_output?.includes('"json"'), false);
});

test("buildToolResultFields marks RemoteTrigger 4xx output failed", () => {
  const base = createToolCall("tc-remote-trigger-error", "RemoteTrigger", {
    action: "run",
    trigger_id: "missing-trigger",
  });
  const fields = buildToolResultFields(
    false,
    {
      status: 404,
      json: '{"error":"not_found"}',
      summary: "Trigger not found",
    },
    base,
  );

  assert.equal(fields.status, "failed");
  assert.equal(fields.raw_output, "Status: 404\nSummary: Trigger not found");
});

test("buildToolResultFields parses RemoteTrigger transcript JSON", () => {
  const base = createToolCall("tc-remote-trigger-history", "RemoteTrigger", {
    action: "get",
    trigger_id: "deploy-prod",
  });
  const transcriptJson = JSON.stringify({
    status: 200,
    json: '{\n  "enabled": true,\n  "name": "Deploy prod"\n}',
  });

  const fields = buildToolResultFields(false, transcriptJson, base, {
    type: "tool_result",
    tool_use_id: "tc-remote-trigger-history",
    content: transcriptJson,
  });

  assert.equal(fields.raw_output, 'Status: 200\nResponse: {"enabled":true,"name":"Deploy prod"}');
  assert.equal(fields.raw_output?.includes('"json"'), false);
});

test("buildToolResultFields renders REPL output as structured text without raw JSON", () => {
  const base = createToolCall("tc-repl", "REPL", {
    code: "await main()",
    description: "Run main function",
  });
  const fields = buildToolResultFields(
    false,
    {
      code: "await main()",
      stdout: "done",
      stderr: "warning",
      result: { ok: true },
      registeredTools: ["fetchDocs", "parse"],
      images: [
        { base64: "image-one-base64", mediaType: "image/png" },
        { base64: "image-two-base64", mediaType: "image/png" },
      ],
      documents: [{ base64: "document-base64" }],
    },
    base,
  );

  assert.equal(fields.status, "completed");
  assert.equal(
    fields.raw_output,
    "Stdout: done\nStderr: warning\nResult: {\"ok\":true}\nRegistered tools: fetchDocs, parse\nImages: 2\nDocuments: 1",
  );
  assert.equal(fields.raw_output?.includes("await main()"), false);
  assert.equal(fields.raw_output?.includes("image-one-base64"), false);
  assert.equal(fields.raw_output?.includes("document-base64"), false);
  assert.equal(fields.raw_output?.includes("{\"code\""), false);
  assert.deepEqual(fields.content, [
    {
      type: "content",
      content: {
        type: "text",
        text: fields.raw_output,
      },
    },
  ]);
});

test("buildToolResultFields marks REPL error output failed", () => {
  const base = createToolCall("tc-repl-error", "REPL", {
    code: "throw new Error('boom')",
  });
  const fields = buildToolResultFields(
    false,
    {
      code: "throw new Error('boom')",
      error: "boom",
      stdout: "",
      stderr: "stack trace",
      result: {},
    },
    base,
  );

  assert.equal(fields.status, "failed");
  assert.equal(fields.raw_output, "Error: boom\nStderr: stack trace");
});

test("buildToolResultFields parses REPL transcript JSON", () => {
  const base = createToolCall("tc-repl-history", "REPL", {
    code: "await load()",
  });
  const transcriptJson = JSON.stringify({
    code: "await load()",
    stdout: "loaded",
    stderr: "",
    result: { count: 2 },
    registeredTools: ["lookup"],
    images: [{ base64: "hidden-image", mediaType: "image/png" }],
    documents: [{ base64: "hidden-document" }, { base64: "hidden-document-2" }],
  });

  const fields = buildToolResultFields(false, transcriptJson, base, {
    type: "tool_result",
    tool_use_id: "tc-repl-history",
    content: transcriptJson,
  });

  assert.equal(
    fields.raw_output,
    "Stdout: loaded\nResult: {\"count\":2}\nRegistered tools: lookup\nImages: 1\nDocuments: 2",
  );
  assert.equal(fields.raw_output?.includes('"code"'), false);
  assert.equal(fields.raw_output?.includes("hidden-image"), false);
  assert.equal(fields.raw_output?.includes("hidden-document"), false);
});

test("buildToolResultFields renders Monitor launch output as structured text", () => {
  const base = createToolCall("tc-monitor", "Monitor", {
    description: "watch deploy logs",
    timeout_ms: 30000,
    persistent: false,
    command: "tail -f deploy.log",
  });
  const fields = buildToolResultFields(
    false,
    { taskId: "monitor-1", timeoutMs: 30000, persistent: false },
    base,
  );

  assert.equal(fields.status, "in_progress");
  assert.equal(fields.raw_output, "Task ID: monitor-1\nPersistent: no\nTimeout: 30s");
  assert.equal(fields.raw_output?.includes("{"), false);
  assert.deepEqual(fields.content, [
    {
      type: "content",
      content: {
        type: "text",
        text: fields.raw_output,
      },
    },
  ]);
});

test("buildToolResultFields renders Workflow launch output as structured text", () => {
  const base = createToolCall("tc-workflow", "Workflow", {
    name: "spec",
    args: { topic: "rendering" },
  });
  const fields = buildToolResultFields(
    false,
    {
      status: "async_launched",
      taskId: "workflow-1",
      taskType: "local_workflow",
      workflowName: "spec",
      runId: "run-1",
      summary: "Workflow started",
      transcriptDir: "C:/tmp/transcripts",
      scriptPath: "C:/tmp/workflow.js",
      warning: "branch diverged",
    },
    base,
  );

  assert.equal(fields.status, "in_progress");
  assert.equal(
    fields.raw_output,
    "Status: async launched\nTask ID: workflow-1\nTask type: local_workflow\nWorkflow name: spec\nRun ID: run-1\nSummary: Workflow started\nTranscript dir: C:/tmp/transcripts\nScript path: C:/tmp/workflow.js\nWarning: branch diverged",
  );
  assert.equal(fields.raw_output?.includes("{"), false);
  assert.equal(fields.raw_output?.includes('"status"'), false);
});

test("buildToolResultFields marks Workflow output with error as failed", () => {
  const base = createToolCall("tc-workflow-error", "Workflow", {
    script: "bad workflow script",
  });
  const fields = buildToolResultFields(
    false,
    {
      status: "async_launched",
      taskId: "workflow-err",
      error: "Syntax check failed",
    },
    base,
  );

  assert.equal(fields.status, "failed");
  assert.equal(
    fields.raw_output,
    "Status: async launched\nTask ID: workflow-err\nError: Syntax check failed",
  );
  assert.equal(fields.raw_output?.includes("bad workflow script"), false);
});

test("buildToolResultFields parses Workflow transcript JSON", () => {
  const base = createToolCall("tc-workflow-history", "Workflow", {
    name: "remote-spec",
  });
  const transcriptJson = JSON.stringify({
    status: "remote_launched",
    taskId: "workflow-remote",
    sessionUrl: "https://claude.ai/session/remote",
  });

  const fields = buildToolResultFields(false, transcriptJson, base, {
    type: "tool_result",
    tool_use_id: "tc-workflow-history",
    content: transcriptJson,
  });

  assert.equal(
    fields.raw_output,
    "Status: remote launched\nTask ID: workflow-remote\nSession URL: https://claude.ai/session/remote",
  );
  assert.equal(fields.raw_output?.includes('"taskId"'), false);
});

test("buildToolResultFields suppresses EnterPlanMode structured output body", () => {
  const base = createToolCall("tc-enter-plan-mode", "EnterPlanMode", {});
  const fields = buildToolResultFields(false, { message: "Plan mode entered" }, base);

  assert.equal(fields.status, "completed");
  assert.equal(fields.raw_output, undefined);
  assert.equal(fields.content, undefined);
});

test("buildToolResultFields suppresses EnterPlanMode transcript JSON body", () => {
  const base = createToolCall("tc-enter-plan-mode-history", "EnterPlanMode", {});
  const transcriptJson = JSON.stringify({ message: "Entered plan mode" });

  const fields = buildToolResultFields(false, transcriptJson, base, {
    type: "tool_result",
    tool_use_id: "tc-enter-plan-mode-history",
    content: transcriptJson,
  });

  assert.equal(fields.status, "completed");
  assert.equal(fields.raw_output, undefined);
  assert.equal(fields.content, undefined);
});

test("buildToolResultFields ignores removed TodoWrite verification metadata", () => {
  const base = createToolCall("tc-todo", "TodoWrite", {
    todos: [{ content: "Verify changes", status: "pending", activeForm: "Verifying changes" }],
  });
  const fields = buildToolResultFields(
    false,
    [{ text: "Todos have been modified successfully." }],
    base,
    {
      data: {
        oldTodos: [],
        newTodos: [],
        verificationNudgeNeeded: true,
      },
    },
  );

  assert.equal(fields.output_metadata, undefined);
});

test("mapAvailableModels preserves optional fast and auto mode metadata", () => {
  const mapped = mapAvailableModels([
    {
      value: "sonnet",
      displayName: "Claude Sonnet",
      description: "Balanced model",
      supportsEffort: true,
      supportedEffortLevels: ["low", "medium", "high", "xhigh", "max"],
      supportsAdaptiveThinking: true,
      supportsFastMode: true,
      supportsAutoMode: false,
    },
    {
      value: "haiku",
      displayName: "Claude Haiku",
      description: "Fast model",
      supportsEffort: false,
    },
  ]);

  assert.deepEqual(mapped, [
    {
      id: "sonnet",
      display_name: "Claude Sonnet",
      description: "Balanced model",
      supports_effort: true,
      supported_effort_levels: ["low", "medium", "high", "xhigh", "max"],
      supports_adaptive_thinking: true,
      supports_fast_mode: true,
      supports_auto_mode: false,
    },
    {
      id: "haiku",
      display_name: "Claude Haiku",
      description: "Fast model",
      supports_effort: false,
      supported_effort_levels: [],
    },
  ]);
});

test("mapAvailableModels filters unavailable Fable models while preserving unknown ids", () => {
  const mapped = mapAvailableModels([
    {
      value: "fable",
      displayName: "Claude Fable",
      description: "Unavailable model alias",
      supportsEffort: true,
    },
    {
      value: "claude-fable-5",
      displayName: "Claude Fable 5",
      description: "Unavailable model",
      supportsEffort: true,
    },
    {
      value: "claude-fable-5-20260612",
      displayName: "Claude Fable 5 dated",
      description: "Unavailable dated model",
      supportsEffort: true,
    },
    {
      value: "claude-unknown-1",
      displayName: "Claude Unknown",
      description: "Unrecognized but available model",
      supportsEffort: false,
    },
  ]);

  assert.deepEqual(mapped, [
    {
      id: "claude-unknown-1",
      display_name: "Claude Unknown",
      description: "Unrecognized but available model",
      supports_effort: false,
      supported_effort_levels: [],
    },
  ]);
});

test("resolveCurrentModel keeps 1M context suffix in short and long display names", () => {
  const session = makeSessionState();
  session.resolvedRuntimeModelId = "claude-opus-4-7[1m]";

  const currentModel = resolveCurrentModel(session);

  assert.equal(currentModel.display_name_short, "Opus 4.7 [1M]");
  assert.equal(currentModel.display_name_long, "Opus 4.7 [1M]");
});

test("resolveCurrentModel does not inherit standard Opus capabilities for 1M when sibling variants exist", () => {
  const session = makeSessionState();
  session.requestedModelId = "claude-opus-4-7";
  session.resolvedRuntimeModelId = "claude-opus-4-7[1m]";
  session.availableModels = [
    {
      id: "claude-opus-4-7",
      display_name: "Claude Opus",
      supports_effort: true,
      supported_effort_levels: ["low", "medium", "high"],
    },
    {
      id: "claude-opus-4-7[1m]",
      display_name: "Claude Opus 1M",
      supports_effort: false,
      supported_effort_levels: [],
    },
  ];

  const currentModel = resolveCurrentModel(session);

  assert.equal(currentModel.catalog_id, "claude-opus-4-7[1m]");
  assert.equal(currentModel.supports_effort, false);
});

test("resolveCurrentModel avoids suffix-insensitive fallback when sibling variants make it ambiguous", () => {
  const session = makeSessionState();
  session.requestedModelId = "claude-opus-4-7";
  session.resolvedRuntimeModelId = "claude-opus-4-7[1m]";
  session.availableModels = [
    {
      id: "claude-opus-4-7",
      display_name: "Claude Opus",
      supports_effort: true,
      supported_effort_levels: ["low", "medium", "high"],
    },
    {
      id: "claude-opus-4-7-alt[1m]",
      display_name: "Claude Opus Alt 1M",
      supports_effort: false,
      supported_effort_levels: [],
    },
  ];

  const currentModel = resolveCurrentModel(session);

  assert.equal(currentModel.catalog_id, undefined);
  assert.equal(currentModel.supports_effort, false);
});

test("emitCurrentModelUpdate can acknowledge a successful no-op set_model", () => {
  const session = makeSessionState();
  session.model = "opus";
  session.requestedModelId = "opus";
  session.resolvedRuntimeModelId = "claude-opus-4-7[1m]";
  refreshCurrentModel(session);

  const events = captureBridgeEvents(() => {
    const changed = refreshCurrentModel(session, true);
    const forced = !changed && emitCurrentModelUpdate(session);
    assert.equal(changed, false);
    assert.equal(forced, true);
  });

  const lastEvent = events.at(-1);
  assert.ok(lastEvent);
  assert.equal(lastEvent.event, "session_update");
  assert.deepEqual(lastEvent.update, {
    type: "current_model_update",
    current_model: {
      requested_id: "opus",
      resolved_id: "claude-opus-4-7[1m]",
      display_name_short: "Opus 4.7 [1M]",
      display_name_long: "Opus 4.7 [1M]",
      supports_effort: false,
      supported_effort_levels: [],
      is_authoritative: true,
    },
  });
});

test("emitCurrentModelUpdate can publish catalog-enriched current model metadata after connect", () => {
  const session = makeSessionState();
  session.model = "sonnet";
  refreshCurrentModel(session);
  session.availableModels = [
    {
      id: "sonnet",
      display_name: "Claude Sonnet",
      supports_effort: true,
      supported_effort_levels: ["low", "medium", "high"],
      supports_auto_mode: true,
    },
  ];

  const events = captureBridgeEvents(() => {
    const changed = refreshCurrentModel(session, false);
    assert.equal(changed, true);
    assert.equal(emitCurrentModelUpdate(session), true);
  });

  const lastEvent = events.at(-1);
  assert.ok(lastEvent);
  assert.equal(lastEvent.event, "session_update");
  assert.deepEqual(lastEvent.update, {
    type: "current_model_update",
    current_model: {
      resolved_id: "sonnet",
      display_name_short: "Sonnet",
      display_name_long: "Claude Sonnet",
      catalog_id: "sonnet",
      supports_effort: true,
      supported_effort_levels: ["low", "medium", "high"],
      supports_auto_mode: true,
      is_authoritative: true,
    },
  });
});

test("shouldInvalidateResolvedRuntimeModel invalidates stale runtime identity only when the request changes", () => {
  assert.equal(
    shouldInvalidateResolvedRuntimeModel("opus", "opus", "sonnet"),
    true,
  );
  assert.equal(
    shouldInvalidateResolvedRuntimeModel("sonnet", "sonnet", "haiku"),
    true,
  );
  assert.equal(
    shouldInvalidateResolvedRuntimeModel("opus", "opus", "opus"),
    false,
  );
});

test("resolveCurrentModel strips release date suffix from dated model ids", () => {
  const session = makeSessionState();
  session.model = "claude-opus-4-5-20251101";
  session.requestedModelId = "claude-opus-4-5-20251101";
  session.resolvedRuntimeModelId = "claude-opus-4-5-20251101";

  const currentModel = resolveCurrentModel(session);

  assert.equal(currentModel.display_name_short, "Opus 4.5");
  assert.equal(currentModel.display_name_long, "Opus 4.5");
});

test("resolveCurrentModel falls back to the requested model immediately after stale runtime identity is cleared", () => {
  const session = makeSessionState();
  session.requestedModelId = "sonnet";
  session.model = "sonnet";
  session.availableModels = [
    {
      id: "sonnet",
      display_name: "Claude Sonnet",
      supports_effort: true,
      supported_effort_levels: ["low", "medium", "high"],
    },
  ];

  const currentModel = resolveCurrentModel(session);

  assert.equal(currentModel.resolved_id, "sonnet");
  assert.equal(currentModel.display_name_short, "Sonnet");
  assert.equal(currentModel.display_name_long, "Claude Sonnet");
  assert.equal(currentModel.catalog_id, "sonnet");
  assert.equal(currentModel.supports_effort, true);
});

test("resolveCurrentModel keeps runtime version in short display while using catalog capabilities", () => {
  const session = makeSessionState();
  session.model = "opus";
  session.requestedModelId = "opus";
  session.resolvedRuntimeModelId = "claude-opus-4-7-20260101";
  session.availableModels = [
    {
      id: "opus",
      display_name: "Opus",
      supports_effort: true,
      supported_effort_levels: ["low", "medium", "high", "xhigh"],
    },
  ];

  const currentModel = resolveCurrentModel(session);

  assert.equal(currentModel.display_name_short, "Opus 4.7");
  assert.equal(currentModel.display_name_long, "Opus");
  assert.equal(currentModel.catalog_id, "opus");
  assert.equal(currentModel.supports_effort, true);
});

function userDialogHandlerForTest(): NonNullable<Options["onUserDialog"]> {
  const input = new AsyncQueue<import("@anthropic-ai/claude-agent-sdk").SDKUserMessage>();
  const options = buildQueryOptions({
    cwd: "C:/work",
    launchSettings: { language: "English" },
    provisionalSessionId: "session-dialog",
    input,
    canUseTool: async () => ({ behavior: "deny", message: "not used" }),
    enableSdkDebug: false,
    enableSpawnDebug: false,
    sessionIdForLogs: () => "session-dialog",
  });
  assert.deepEqual(options.supportedDialogKinds, ["refusal_fallback_prompt"]);
  const handler = options.onUserDialog;
  assert.ok(handler, "expected buildQueryOptions to declare onUserDialog");
  return handler;
}

function registerDialogSession(): SessionState {
  const session = makeSessionState();
  session.sessionId = "session-dialog";
  sessions.set("session-dialog", session);
  return session;
}

test("onUserDialog round-trips a retry_fallback selection", async () => {
  const handler = userDialogHandlerForTest();
  const session = registerDialogSession();
  try {
    const events = await captureBridgeEventsAsync(async () => {
      const resultPromise = handler(
        {
          dialogKind: "refusal_fallback_prompt",
          payload: {
            originalModel: "claude-opus-4-8",
            fallbackModel: "claude-sonnet-4-6",
            guidanceText: "This request was declined.",
          },
        },
        { signal: new AbortController().signal },
      );

      const requestId = [...session.pendingUserDialogs.keys()][0];
      assert.ok(requestId, "expected a pending user dialog resolver");

      handleUserDialogResponse({
        command: "user_dialog_response",
        session_id: "session-dialog",
        request_id: requestId,
        outcome: { outcome: "selected", option_id: "retry_fallback" },
      });

      assert.deepEqual(await resultPromise, {
        behavior: "completed",
        result: "retry_fallback",
      });
    });

    const dialogEvent = events.find((event) => event.event === "user_dialog_request");
    assert.ok(dialogEvent, "expected a user_dialog_request event");
    const request = dialogEvent.request as {
      dialog_kind: string;
      payload: Record<string, unknown>;
      options: Array<{ option_id: string; label: string }>;
    };
    assert.equal(request.dialog_kind, "refusal_fallback_prompt");
    assert.equal(request.payload.original_model, "claude-opus-4-8");
    assert.equal(request.payload.fallback_model, "claude-sonnet-4-6");
    assert.equal(request.payload.guidance_text, "This request was declined.");
    assert.deepEqual(
      request.options.map((option) => option.option_id),
      ["retry_fallback", "edit_prompt"],
    );
    assert.equal(request.options[0].label, "Switch to claude-sonnet-4-6");
    assert.equal(request.options[1].label, "Edit prompt and retry with claude-opus-4-8");
  } finally {
    sessions.delete("session-dialog");
  }
});

test("onUserDialog round-trips an edit_prompt selection", async () => {
  const handler = userDialogHandlerForTest();
  const session = registerDialogSession();
  try {
    const resultPromise = handler(
      {
        dialogKind: "refusal_fallback_prompt",
        payload: { originalModel: "claude-opus-4-8", fallbackModel: "claude-sonnet-4-6" },
      },
      { signal: new AbortController().signal },
    );

    const requestId = [...session.pendingUserDialogs.keys()][0];
    handleUserDialogResponse({
      command: "user_dialog_response",
      session_id: "session-dialog",
      request_id: requestId,
      outcome: { outcome: "selected", option_id: "edit_prompt" },
    });

    assert.deepEqual(await resultPromise, { behavior: "completed", result: "edit_prompt" });
  } finally {
    sessions.delete("session-dialog");
  }
});

test("onUserDialog cancels when the dialog is aborted", async () => {
  const handler = userDialogHandlerForTest();
  const session = registerDialogSession();
  const controller = new AbortController();
  try {
    const resultPromise = handler(
      {
        dialogKind: "refusal_fallback_prompt",
        payload: { originalModel: "claude-opus-4-8", fallbackModel: "claude-sonnet-4-6" },
      },
      { signal: controller.signal },
    );

    assert.equal(session.pendingUserDialogs.size, 1);
    controller.abort();

    assert.deepEqual(await resultPromise, { behavior: "cancelled" });
    assert.equal(session.pendingUserDialogs.size, 0);
  } finally {
    sessions.delete("session-dialog");
  }
});

test("onUserDialog fails closed on an unknown dialog kind without emitting", async () => {
  const handler = userDialogHandlerForTest();
  const session = registerDialogSession();
  try {
    const events = await captureBridgeEventsAsync(async () => {
      const result = await handler(
        { dialogKind: "some_future_dialog_kind", payload: { anything: true } },
        { signal: new AbortController().signal },
      );
      assert.deepEqual(result, { behavior: "cancelled" });
    });

    assert.equal(
      events.find((event) => event.event === "user_dialog_request"),
      undefined,
    );
    assert.equal(session.pendingUserDialogs.size, 0);
  } finally {
    sessions.delete("session-dialog");
  }
});

test("handleUserDialogResponse ignores a duplicate response for a resolved request", async () => {
  const handler = userDialogHandlerForTest();
  const session = registerDialogSession();
  try {
    const resultPromise = handler(
      {
        dialogKind: "refusal_fallback_prompt",
        payload: { originalModel: "claude-opus-4-8", fallbackModel: "claude-sonnet-4-6" },
      },
      { signal: new AbortController().signal },
    );

    const requestId = [...session.pendingUserDialogs.keys()][0];
    const response = {
      command: "user_dialog_response" as const,
      session_id: "session-dialog",
      request_id: requestId,
      outcome: { outcome: "selected" as const, option_id: "retry_fallback" as const },
    };

    handleUserDialogResponse(response);
    assert.deepEqual(await resultPromise, { behavior: "completed", result: "retry_fallback" });

    // A replayed pending_user_dialog_requests entry with the same id must be a
    // no-op now that the resolver is gone.
    assert.equal(session.pendingUserDialogs.has(requestId), false);
    assert.doesNotThrow(() => handleUserDialogResponse(response));
  } finally {
    sessions.delete("session-dialog");
  }
});
