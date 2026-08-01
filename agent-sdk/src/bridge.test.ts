import test from "node:test";
import assert from "node:assert/strict";
import {
  AsyncQueue,
  buildApiRetryUpdate,
  buildRateLimitUpdate,
  buildRewindConversationPlan,
  buildQueryOptions,
  buildPromptUserMessage,
  resolveClaudeCodeSpawnCommand,
  canGenerateSessionTitle,
  generatePersistedSessionTitle,
  buildSessionMutationOptions,
  buildSessionListOptions,
  createToolCall,
  applySessionAgent,
  applySessionEffort,
  applySessionFastMode,
  emitAgentConfigOptionUpdate,
  emitEffortConfigOptionUpdate,
  handleTaskSystemMessage,
  handleSdkMessage,
  isShellToolName,
  mapSdkAccountInfo,
  mapRewindFilesResult,
  mapAvailableAgents,
  mapAvailableModels,
  mapSessionMessagesToUpdates,
  mapSdkSessions,
  agentSdkVersionCompatibilityError,
  looksLikeAuthRequired,
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
  staleMcpAuthCandidates,
  resolveInstalledAgentSdkVersion,
  rewindTargetsFromSessionMessages,
  updateAvailableCommands,
  handleReloadPluginsCommand,
} from "./bridge.js";
import type { SessionState } from "./bridge.js";
import type { Options, SessionMessage } from "@anthropic-ai/claude-agent-sdk";
import {
  availableModesForSession,
  buildModeState,
  markModeUnavailableForSession,
  permissionModeFailureLooksUnsupported,
  refreshSupportedModesForSession,
} from "./bridge/commands.js";
import {
  emitMcpSnapshotEvent,
  emitMcpSnapshotFromStatuses,
  handleMcpAuthenticateCommand,
  handleMcpSetServersCommand,
  startMcpAuthSnapshotMonitor,
} from "./bridge/mcp.js";
import {
  emitCurrentModelUpdate,
  beginSessionClose,
  closeAllSessions,
  handleUserDialogResponse,
  closeSession,
  closeSessionsBeforeRegister,
  refreshCurrentModel,
  resolveCurrentModel,
  sessions,
  shouldInvalidateResolvedRuntimeModel,
  shouldEmitStartupAuthRequiredForAccount,
  trackSessionCloseTask,
} from "./bridge/session_lifecycle.js";
import {
  classifyTurnErrorKind,
  setFastModeSnapshotIfChanged,
} from "./bridge/error_classification.js";
import {
  buildConnectBridgeEvent,
  replaceProtocolEventWriter,
} from "./bridge/events.js";
import { emitToolCall, emitToolProgressUpdate, emitToolResultUpdate } from "./bridge/tool_calls.js";
import { linkTaskToolUse } from "./bridge/task_links.js";
import { requestAskUserQuestionAnswers } from "./bridge/user_interaction.js";
import { flushPendingWorkerShutdown, handleResultMessage } from "./bridge/message_handlers.js";
import { dispatchCancelTurnCommand } from "./bridge/command_dispatch.js";

const BRIDGE_RUNTIME_PROCESS_NAME =
  process.platform === "win32" ? "claude-rs-bridge-bun.exe" : "claude-rs-bridge-bun";
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
    closing: false,
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
    informationalDedupKeys: new Set(),
    knownConnectedMcpServers: new Set(),
    mcpStatusRevalidatedAt: new Map(),
    mcpAuthMonitors: new Map(),
    hiddenToolUseIds: new Set(),
    authHintSent: false,
  };
}

test("closeSessionsBeforeRegister closes same-key stale session before replacement registration", async () => {
  sessions.clear();
  let staleClosed = 0;
  const stale = makeSessionState();
  stale.sessionId = "session-1";
  stale.query = {
    close: () => {
      staleClosed += 1;
    },
  } as import("@anthropic-ai/claude-agent-sdk").Query;
  const replacement = makeSessionState();
  replacement.sessionId = "session-1";

  sessions.set(stale.sessionId, stale);
  await closeSessionsBeforeRegister(replacement, [stale], "req-1");

  assert.equal(staleClosed, 1);
  assert.equal(sessions.has("session-1"), false);

  sessions.set(replacement.sessionId, replacement);
  await closeSessionsBeforeRegister(replacement, [stale], "req-2");

  assert.equal(staleClosed, 2);
  assert.equal(sessions.get("session-1"), replacement);
  sessions.clear();
});

test("closeSession waits for owned query lifecycle tasks", async () => {
  const session = makeSessionState();
  let finishConsumer: (() => void) | undefined;
  let closeResolved = false;
  session.query = {
    close: () => {},
  } as import("@anthropic-ai/claude-agent-sdk").Query;
  session.initializationTask = Promise.resolve();
  session.queryConsumerTask = new Promise<void>((resolve) => {
    finishConsumer = resolve;
  });

  const closePromise = closeSession(session).then(() => {
    closeResolved = true;
  });
  await Promise.resolve();
  assert.equal(closeResolved, false);

  finishConsumer?.();
  await closePromise;
  assert.equal(closeResolved, true);
});

test("closeAllSessions waits for tracked stale-session cleanup", async () => {
  sessions.clear();
  let finishCleanup!: () => void;
  let closeResolved = false;
  const cleanup = new Promise<void>((resolve) => {
    finishCleanup = resolve;
  });
  trackSessionCloseTask(cleanup);

  const closePromise = closeAllSessions({ reason: "test_shutdown" }).then(() => {
    closeResolved = true;
  });
  await Promise.resolve();
  assert.equal(closeResolved, false);

  finishCleanup();
  await closePromise;
  assert.equal(closeResolved, true);
});

test("closeSession cancels and awaits one coalesced MCP auth monitor per server", async () => {
  const session = makeSessionState();
  let statusCalls = 0;
  let delayStarted!: () => void;
  const started = new Promise<void>((resolve) => {
    delayStarted = resolve;
  });
  session.query = {
    close: () => {},
    mcpServerStatus: async () => {
      statusCalls += 1;
      return [];
    },
  } as unknown as import("@anthropic-ai/claude-agent-sdk").Query;

  const timing = {
    sleep: async (_delayMs: number, signal: AbortSignal) => {
      delayStarted();
      await new Promise<void>((resolve) => {
        signal.addEventListener("abort", () => resolve(), { once: true });
      });
    },
  };
  const first = startMcpAuthSnapshotMonitor(session, "docs", timing);
  const repeated = startMcpAuthSnapshotMonitor(session, "docs", timing);

  assert.equal(repeated, first);
  assert.equal(session.mcpAuthMonitors.size, 1);
  await started;
  await closeSession(session);

  assert.deepEqual(await first, { outcome: "cancelled", attempts: 0 });
  assert.equal(statusCalls, 0);
  assert.equal(session.mcpAuthMonitors.size, 0);
});

test("replacement handoff suppresses an old session's in-flight MCP monitor snapshot", async () => {
  const session = makeSessionState();
  type SdkMcpServerStatus = import("@anthropic-ai/claude-agent-sdk").McpServerStatus;
  let resolveStatus!: (statuses: SdkMcpServerStatus[]) => void;
  let statusStarted!: () => void;
  const started = new Promise<void>((resolve) => {
    statusStarted = resolve;
  });
  const pendingStatus = new Promise<SdkMcpServerStatus[]>((resolve) => {
    resolveStatus = resolve;
  });
  session.query = {
    close: () => {},
    mcpServerStatus: async () => {
      statusStarted();
      return await pendingStatus;
    },
  } as unknown as import("@anthropic-ai/claude-agent-sdk").Query;

  const events = await captureBridgeEventsAsync(async () => {
    const monitor = startMcpAuthSnapshotMonitor(session, "docs", {
      maxAttempts: 1,
      sleep: async () => undefined,
    });
    await started;
    beginSessionClose(session);
    resolveStatus([{ name: "docs", status: "connected" } as SdkMcpServerStatus]);
    const result = await monitor;
    assert.equal(result.outcome, "cancelled");
    assert.equal(session.closing, true);
    await closeSession(session);
  });

  assert.deepEqual(events, []);
});

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
  const restoreWriter = replaceProtocolEventWriter((line) => {
    writes.push(line);
  });

  try {
    run();
  } finally {
    restoreWriter();
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
  const restoreWriter = replaceProtocolEventWriter((line) => {
    writes.push(line);
  });

  try {
    await run();
  } finally {
    restoreWriter();
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

test("parseCommandEnvelope validates mcp_status command", () => {
  const parsed = parseCommandEnvelope(
    JSON.stringify({
      request_id: "req-mcp-status",
      command: "mcp_status",
      session_id: "session-123",
    }),
  );

  assert.equal(parsed.requestId, "req-mcp-status");
  assert.deepEqual(parsed.command, {
    command: "mcp_status",
    session_id: "session-123",
  });
});

test("parseCommandEnvelope normalizes get_mcp_snapshot command", () => {
  const parsed = parseCommandEnvelope(
    JSON.stringify({
      request_id: "req-mcp-snapshot",
      command: "get_mcp_snapshot",
      session_id: "session-123",
    }),
  );

  assert.equal(parsed.requestId, "req-mcp-snapshot");
  assert.deepEqual(parsed.command, {
    command: "mcp_status",
    session_id: "session-123",
  });
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
          request_timeout_ms: 30000,
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
      request_timeout_ms: 30000,
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
          servers: {
            bad: {
              type: "http",
              url: "https://mcp.example.com",
              request_timeout_ms: 999,
            },
          },
        }),
      ),
    /mcp_set_servers\.servers\.bad\.request_timeout_ms must be an integer >= 1000/,
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
            request_timeout_ms: 30000,
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
      requestTimeoutMs: 30000,
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
      auth_capabilities: {
        authenticate: false,
        clear_auth: false,
        submit_oauth_callback_url: false,
      },
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

test("handleMcpAuthenticateCommand emits a structured error when the runtime method is absent", async () => {
  const session = makeSessionState();

  const events = await captureBridgeEventsAsync(async () => {
    await handleMcpAuthenticateCommand(
      session,
      {
        command: "mcp_authenticate",
        session_id: "session-1",
        server_name: "docs",
      },
      "req-mcp-auth",
    );
  });

  assert.deepEqual(events, [
    {
      request_id: "req-mcp-auth",
      event: "mcp_operation_error",
      session_id: "session-1",
      error: {
        server_name: "docs",
        operation: "authenticate",
        message: "installed SDK does not support mcpAuthenticate",
      },
    },
  ]);
});

test("handleMcpAuthenticateCommand emits a structured error for an incompatible response", async () => {
  const session = makeSessionState();
  session.query = {
    async mcpAuthenticate() {
      return { authUrl: 42 };
    },
  } as unknown as import("@anthropic-ai/claude-agent-sdk").Query;

  const events = await captureBridgeEventsAsync(async () => {
    await handleMcpAuthenticateCommand(
      session,
      {
        command: "mcp_authenticate",
        session_id: "session-1",
        server_name: "docs",
      },
      "req-mcp-auth",
    );
  });

  assert.deepEqual(events, [
    {
      request_id: "req-mcp-auth",
      event: "mcp_operation_error",
      session_id: "session-1",
      error: {
        server_name: "docs",
        operation: "authenticate",
        message: "installed SDK returned an invalid mcpAuthenticate authUrl",
      },
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
      request_timeout_ms: 30000,
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
      requestTimeoutMs: 30000,
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
      requestTimeoutMs: 30000,
      alwaysLoad: true,
      tools: [
        { name: "search" },
        { name: "write", permission_policy: "always_deny", org_max_permission: "ask" },
      ],
    } as unknown as NonNullable<import("@anthropic-ai/claude-agent-sdk").McpServerStatus["config"]>,
    tools: [],
  });

  assert.deepEqual(mapped.config, {
    type: "http",
    url: "https://mcp.notion.com/mcp",
    headers: { Authorization: "Bearer token" },
    timeout: 5000,
    request_timeout_ms: 30000,
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
        auth_capabilities: {
          authenticate: false,
          clear_auth: false,
          submit_oauth_callback_url: false,
        },
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
        auth_capabilities: {
          authenticate: false,
          clear_auth: false,
          submit_oauth_callback_url: false,
        },
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

test("parseCommandEnvelope validates get_rewind_targets command", () => {
  const parsed = parseCommandEnvelope(
    JSON.stringify({
      request_id: "req-rewind-targets",
      command: "get_rewind_targets",
      session_id: "session-123",
    }),
  );

  assert.equal(parsed.requestId, "req-rewind-targets");
  assert.deepEqual(parsed.command, {
    command: "get_rewind_targets",
    session_id: "session-123",
  });
});

test("mapRewindFilesResult preserves valid skipped link counts", () => {
  const base = {
    canRewind: true,
    filesChanged: ["src/main.rs"],
  };

  assert.deepEqual(mapRewindFilesResult(base), {
    can_rewind: true,
    files_changed: ["src/main.rs"],
  });
  assert.equal(mapRewindFilesResult({ ...base, skippedLinks: 0 }).skipped_links, 0);
  assert.equal(mapRewindFilesResult({ ...base, skippedLinks: 2 }).skipped_links, 2);
});

test("mapRewindFilesResult drops malformed skipped link counts", () => {
  const base = {
    canRewind: true,
    filesChanged: [],
  };
  for (const skippedLinks of [-1, 1.5, Number.NaN, Number.POSITIVE_INFINITY]) {
    assert.equal(mapRewindFilesResult({ ...base, skippedLinks }).skipped_links, undefined);
  }
});

test("parseCommandEnvelope validates rewind command modes", () => {
  for (const restoreMode of ["both", "conversation", "code"] as const) {
    const parsed = parseCommandEnvelope(
      JSON.stringify({
        request_id: "req-rewind",
        command: "rewind",
        session_id: "session-123",
        target_user_message_id: "user-1",
        restore_mode: restoreMode,
        launch_settings: {
          language: "German",
        },
      }),
    );

    assert.equal(parsed.requestId, "req-rewind");
    assert.deepEqual(parsed.command, {
      command: "rewind",
      session_id: "session-123",
      target_user_message_id: "user-1",
      restore_mode: restoreMode,
      launch_settings: { language: "German" },
    });
  }
});

test("parseCommandEnvelope rejects invalid rewind mode", () => {
  assert.throws(
    () =>
      parseCommandEnvelope(
        JSON.stringify({
          command: "rewind",
          session_id: "session-123",
          target_user_message_id: "user-1",
          restore_mode: "files",
        }),
      ),
    /rewind\.restore_mode must be one of both, conversation, code/,
  );
});

test("rewindTargetsFromSessionMessages filters user text messages with UUIDs", () => {
  const messages = [
    {
      type: "user",
      uuid: "user-1",
      message: { role: "user", content: [{ type: "text", text: " first prompt\nline " }] },
    },
    {
      type: "assistant",
      uuid: "assistant-1",
      message: { role: "assistant", content: [{ type: "text", text: "ignored" }] },
    },
    {
      type: "user",
      uuid: "tool-result",
      message: { role: "user", content: [{ type: "tool_result", content: "ignored" }] },
    },
    {
      type: "user",
      uuid: "user-2",
      message: { role: "user", content: [{ type: "text", text: "second prompt" }] },
    },
  ] as unknown as SessionMessage[];

  assert.deepEqual(rewindTargetsFromSessionMessages(messages), [
    {
      uuid: "user-2",
      first_text: "second prompt",
      input_text: "second prompt",
      index: 3,
      previous_assistant_uuid: "assistant-1",
    },
    {
      uuid: "user-1",
      first_text: "first prompt line",
      input_text: "first prompt\nline",
      index: 0,
    },
  ]);
});

test("buildRewindConversationPlan anchors at previous assistant message", () => {
  const messages = [
    {
      type: "user",
      uuid: "user-1",
      message: { role: "user", content: "first prompt" },
    },
    {
      type: "assistant",
      uuid: "assistant-1",
      message: { role: "assistant", content: [{ type: "text", text: "reply" }] },
    },
    {
      type: "user",
      uuid: "user-2",
      message: { role: "user", content: "second prompt" },
    },
  ] as unknown as SessionMessage[];

  const plan = buildRewindConversationPlan(messages, "user-2");

  assert.ok(plan);
  assert.equal(plan.inputText, "second prompt");
  assert.equal(plan.previousAssistantUuid, "assistant-1");
  assert.equal(plan.targetIndex, 2);
  assert.deepEqual(
    plan.retainedMessages.map((message) => message.uuid),
    ["user-1", "assistant-1"],
  );
  assert.ok(plan.resumeUpdates.length > 0);
});

test("buildRewindConversationPlan treats first user message as fresh replacement", () => {
  const messages = [
    {
      type: "user",
      uuid: "user-1",
      message: { role: "user", content: " first prompt\nline " },
    },
    {
      type: "assistant",
      uuid: "assistant-1",
      message: { role: "assistant", content: [{ type: "text", text: "reply" }] },
    },
  ] as unknown as SessionMessage[];

  const plan = buildRewindConversationPlan(messages, "user-1");

  assert.ok(plan);
  assert.equal(plan.inputText, " first prompt\nline ");
  assert.equal(plan.previousAssistantUuid, undefined);
  assert.equal(plan.targetIndex, 0);
  assert.deepEqual(plan.retainedMessages, []);
  assert.deepEqual(plan.resumeUpdates, []);
});

test("buildRewindConversationPlan rejects stale or inconsistent targets", () => {
  const messages = [
    {
      type: "user",
      uuid: "user-1",
      message: { role: "user", content: "first prompt" },
    },
    {
      type: "user",
      uuid: "user-2",
      message: { role: "user", content: "second prompt" },
    },
  ] as unknown as SessionMessage[];

  assert.equal(buildRewindConversationPlan(messages, "missing-user"), null);
  assert.equal(buildRewindConversationPlan(messages, "user-2"), null);
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

test("MCP connection history is isolated between sessions with the same server name", async () => {
  type SdkMcpServerStatus = import("@anthropic-ai/claude-agent-sdk").McpServerStatus;
  const connected = {
    name: "docs",
    status: "connected",
  } as SdkMcpServerStatus;
  const needsAuth = {
    name: "docs",
    status: "needs-auth",
  } as SdkMcpServerStatus;
  const first = makeSessionState();
  first.sessionId = "session-first";
  const second = makeSessionState();
  second.sessionId = "session-second";
  let reconnectCalls = 0;
  second.query = {
    mcpServerStatus: async () => [needsAuth],
    reconnectMcpServer: async () => {
      reconnectCalls += 1;
    },
  } as unknown as import("@anthropic-ai/claude-agent-sdk").Query;

  await captureBridgeEventsAsync(async () => {
    emitMcpSnapshotFromStatuses(first, [connected], "mcp_status");
    await emitMcpSnapshotEvent(second);
  });

  assert.deepEqual(first.knownConnectedMcpServers, new Set(["docs"]));
  assert.deepEqual(second.knownConnectedMcpServers, new Set());
  assert.equal(reconnectCalls, 0);
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
    feedbackDrafts: "off",
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
  assert.equal(options.enableFileCheckpointing, true);
  assert.deepEqual(options.disallowedTools, ["ProposeSkills"]);
  assert.equal(options.executable, "bun");
  assert.equal(options.sessionId, "session-1");
  assert.deepEqual(options.settingSources, ["user", "project", "local"]);
  assert.deepEqual(options.toolConfig, {
    askUserQuestion: { previewFormat: "markdown" },
  });
});

test("buildQueryOptions includes resumeSessionAt when provided", () => {
  const input = new AsyncQueue<import("@anthropic-ai/claude-agent-sdk").SDKUserMessage>();
  const options = buildQueryOptions({
    cwd: "C:/work",
    resume: "session-1",
    resumeSessionAt: "assistant-1",
    launchSettings: {},
    provisionalSessionId: "session-1",
    input,
    canUseTool: async () => ({ behavior: "deny", message: "not used" }),
    enableSdkDebug: false,
    enableSpawnDebug: false,
    sessionIdForLogs: () => "session-1",
  });

  assert.equal(options.resume, "session-1");
  assert.equal(options.resumeSessionAt, "assistant-1");
  assert.equal("sessionId" in options, false);
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
    feedbackDrafts: "off",
  });
  assert.equal("model" in options, false);
  assert.equal(options.permissionMode, "default");
  assert.equal("allowDangerouslySkipPermissions" in options, false);
  assert.equal("thinking" in options, false);
  assert.equal("effort" in options, false);
});

test("buildQueryOptions normalizes manual startup permission mode to default", () => {
  const input = new AsyncQueue<import("@anthropic-ai/claude-agent-sdk").SDKUserMessage>();
  const options = buildQueryOptions({
    cwd: "C:/work",
    launchSettings: {
      settings: {
        permissions: { defaultMode: "manual" },
      },
    },
    provisionalSessionId: "session-manual",
    input,
    canUseTool: async () => ({ behavior: "deny", message: "not used" }),
    enableSdkDebug: false,
    enableSpawnDebug: false,
    sessionIdForLogs: () => "session-manual",
  });

  assert.equal(options.permissionMode, "default");
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
  assert.equal("canUseTool" in options, false);
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
  assert.deepEqual(options.settings, { feedbackDrafts: "off" });
});

test("buildQueryOptions disables feedback drafts without mutating launch settings", () => {
  const input = new AsyncQueue<import("@anthropic-ai/claude-agent-sdk").SDKUserMessage>();
  const settings = {
    feedbackDrafts: "notify",
    spinnerTipsEnabled: true,
  };
  const options = buildQueryOptions({
    cwd: "C:/work",
    launchSettings: { settings },
    provisionalSessionId: "session-feedback",
    input,
    canUseTool: async () => ({ behavior: "deny", message: "not used" }),
    enableSdkDebug: false,
    enableSpawnDebug: false,
    sessionIdForLogs: () => "session-feedback",
  });

  assert.deepEqual(options.settings, {
    feedbackDrafts: "off",
    spinnerTipsEnabled: true,
  });
  assert.deepEqual(settings, {
    feedbackDrafts: "notify",
    spinnerTipsEnabled: true,
  });
});

test("dispatchCancelTurnCommand interrupts the matching session query", async () => {
  let interruptCount = 0;
  let receiptReturned = false;
  const slashErrors: Array<{ sessionId: string; message: string; requestId?: string }> = [];

  await dispatchCancelTurnCommand(
    { command: "cancel_turn", session_id: "session-1" },
    {
      requestId: "request-1",
      sessionById: (sessionId) =>
        sessionId === "session-1"
          ? {
              query: {
                interrupt: async () => {
                  interruptCount += 1;
                  receiptReturned = true;
                  return { still_queued: ["user-message-1"] };
                },
              },
            }
          : undefined,
      slashError: (sessionId, message, requestId) => {
        slashErrors.push({ sessionId, message, requestId });
      },
    },
  );

  assert.equal(interruptCount, 1);
  assert.equal(receiptReturned, true);
  assert.deepEqual(slashErrors, []);
});

test("dispatchCancelTurnCommand emits slash error for unknown session", async () => {
  const interruptCount = 0;
  const slashErrors: Array<{ sessionId: string; message: string; requestId?: string }> = [];

  await dispatchCancelTurnCommand(
    { command: "cancel_turn", session_id: "missing-session" },
    {
      requestId: "request-2",
      sessionById: () => undefined,
      slashError: (sessionId, message, requestId) => {
        slashErrors.push({ sessionId, message, requestId });
      },
    },
  );

  assert.equal(interruptCount, 0);
  assert.deepEqual(slashErrors, [
    {
      sessionId: "missing-session",
      message: "unknown session: missing-session",
      requestId: "request-2",
    },
  ]);
});

test("resolveClaudeCodeSpawnCommand remaps bare bun to the bridge runtime executable", () => {
  assert.equal(resolveClaudeCodeSpawnCommand("bun"), process.execPath);
});

test("resolveClaudeCodeSpawnCommand preserves commands with path separators", () => {
  assert.equal(resolveClaudeCodeSpawnCommand("/opt/runtime/bun"), "/opt/runtime/bun");
  assert.equal(resolveClaudeCodeSpawnCommand("C:\\runtime\\bun.exe"), "C:\\runtime\\bun.exe");
});

test("buildQueryOptions spawn hook remaps bare bun command", async () => {
  const input = new AsyncQueue<import("@anthropic-ai/claude-agent-sdk").SDKUserMessage>();
  const options = buildQueryOptions({
    cwd: "C:/work",
    launchSettings: {},
    provisionalSessionId: "session-spawn-bun",
    input,
    canUseTool: async () => ({ behavior: "deny", message: "not used" }),
    enableSdkDebug: false,
    enableSpawnDebug: false,
    sessionIdForLogs: () => "session-spawn-bun",
  });

  const child = options.spawnClaudeCodeProcess({
    command: "bun",
    args: ["-e", "process.stdout.write(process.execPath)"],
    cwd: process.cwd(),
    env: {},
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
  assert.equal(stdout, process.execPath);
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
    feedbackDrafts: "off",
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
    feedbackDrafts: "off",
    sandbox: {
      enabled: true,
      failIfUnavailable: true,
    },
  });
});

test("buildQueryOptions preserves target sandbox network and filesystem fields", () => {
  const input = new AsyncQueue<import("@anthropic-ai/claude-agent-sdk").SDKUserMessage>();
  const options = buildQueryOptions({
    cwd: "C:/work",
    launchSettings: {
      settings: {
        sandbox: {
          network: { strictAllowlist: true },
          filesystem: { disabled: true },
        },
      },
    },
    provisionalSessionId: "session-sandbox-target-fields",
    input,
    canUseTool: async () => ({ behavior: "deny", message: "not used" }),
    enableSdkDebug: false,
    enableSpawnDebug: false,
    sessionIdForLogs: () => "session-sandbox-target-fields",
  });

  assert.deepEqual(options.settings?.sandbox, {
    network: { strictAllowlist: true },
    filesystem: { disabled: true },
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

test("handleTaskSystemMessage preserves blocked and parent agent metadata", () => {
  const session = makeSessionState();

  const events = captureBridgeEvents(() => {
    handleTaskSystemMessage(session, "task_started", {
      task_id: "task-1",
      tool_use_id: "tool-1",
      description: "Investigate release blocker",
      blocked: true,
      parent_agent_id: "agent-parent",
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
        raw_output: "Investigate release blocker",
        content: [
          {
            type: "content",
            content: { type: "text", text: "Investigate release blocker" },
          },
        ],
        task_metadata: {
          blocked: true,
          parent_agent_id: "agent-parent",
        },
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

test("handleSdkMessage emits warning notice for model_refusal_no_fallback with explanation", () => {
  const session = makeSessionState();

  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "system",
      subtype: "model_refusal_no_fallback",
      original_model: "claude-opus-4-1",
      request_id: "req-1",
      api_refusal_category: "cyber",
      api_refusal_explanation: "policy text",
      refused_user_message_uuid: "user-1",
      content: "raw content",
      uuid: "refusal-notice",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  assert.deepEqual(events.map((event) => event.update), [
    {
      type: "system_notice_update",
      severity: "warning",
      message:
        "Could not continue with claude-opus-4-1: model refused the request and no fallback model is configured. Reason: policy text.",
    },
  ]);
});

test("handleSdkMessage emits warning notice for model_refusal_no_fallback with category only", () => {
  const session = makeSessionState();

  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "system",
      subtype: "model_refusal_no_fallback",
      original_model: "claude-opus-4-1",
      request_id: "req-1",
      api_refusal_category: "cyber",
      uuid: "refusal-notice",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  assert.deepEqual(events.map((event) => event.update), [
    {
      type: "system_notice_update",
      severity: "warning",
      message:
        "Could not continue with claude-opus-4-1: model refused the request and no fallback model is configured. Refusal category: cyber.",
    },
  ]);
});

test("handleSdkMessage emits warning notice for model_refusal_no_fallback with content detail", () => {
  const session = makeSessionState();

  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "system",
      subtype: "model_refusal_no_fallback",
      original_model: "claude-opus-4-1",
      request_id: "req-1",
      content: "Refused by policy!",
      uuid: "refusal-notice",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  assert.deepEqual(events.map((event) => event.update), [
    {
      type: "system_notice_update",
      severity: "warning",
      message:
        "Could not continue with claude-opus-4-1: model refused the request and no fallback model is configured. Refused by policy!",
    },
  ]);
});

test("handleSdkMessage emits readable model_refusal_no_fallback notice for empty metadata", () => {
  const session = makeSessionState();

  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "system",
      subtype: "model_refusal_no_fallback",
      original_model: " ",
      request_id: null,
      api_refusal_category: " ",
      api_refusal_explanation: null,
      refused_user_message_uuid: null,
      content: " ",
      uuid: "refusal-notice",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  assert.deepEqual(events.map((event) => event.update), [
    {
      type: "system_notice_update",
      severity: "warning",
      message:
        "Could not continue with the selected model: model refused the request and no fallback model is configured.",
    },
  ]);
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
    auth_capabilities: {
      authenticate: false,
      clear_auth: false,
      submit_oauth_callback_url: false,
    },
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

test("authority snapshots publish fast mode set during initialization", () => {
  const session = makeSessionState();
  assert.equal(setFastModeSnapshotIfChanged(session, "on", "model_not_allowed"), true);

  for (const connectEvent of ["connected", "session_replaced"] as const) {
    session.connectEvent = connectEvent;
    const authorityEvent = buildConnectBridgeEvent(session, connectEvent);
    assert.equal(authorityEvent.event, connectEvent);
    assert.equal(authorityEvent.fast_mode_state, "on");
    assert.equal(authorityEvent.fast_mode_disabled_reason, "model_not_allowed");
  }
});

test("handleSdkMessage emits fast mode as a delta after authority exists", () => {
  const session = makeSessionState();
  session.query = {
    supportedCommands: async () => [],
  } as unknown as import("@anthropic-ai/claude-agent-sdk").Query;

  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "system",
      subtype: "init",
      session_id: session.sessionId,
      model: "sonnet",
      fast_mode_state: "cooldown",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  const fastModeUpdates = events.filter(
    (event) =>
      event.event === "session_update" &&
      (event.update as { type?: unknown } | undefined)?.type === "fast_mode_update",
  );
  assert.deepEqual(fastModeUpdates, [
    {
      event: "session_update",
      session_id: "session-1",
      update: {
        type: "fast_mode_update",
        fast_mode_state: "cooldown",
      },
    },
  ]);
});

test("fast mode snapshots deduplicate state and reason together and clear stale reasons", () => {
  const session = makeSessionState();
  const reasons = [
    "free",
    "preference",
    "extra_usage_disabled",
    "network_error",
    "unknown",
    "not_first_party",
    "disabled_by_env",
    "model_not_allowed",
    "sdk_opt_in_required",
    "pending",
    "future-reason",
  ];

  const events = captureBridgeEvents(() => {
    for (const reason of reasons) {
      handleSdkMessage(session, {
        type: "system",
        subtype: "status",
        fast_mode_state: "off",
        fast_mode_disabled_reason: reason,
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
      handleSdkMessage(session, {
        type: "system",
        subtype: "status",
        fast_mode_state: "off",
        fast_mode_disabled_reason: reason,
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    }
    handleSdkMessage(session, {
      type: "system",
      subtype: "status",
      fast_mode_state: "off",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  const updates = events
    .filter((event) => event.event === "session_update")
    .map((event) => event.update as import("./types.js").SessionUpdate)
    .filter(
      (update): update is Extract<import("./types.js").SessionUpdate, { type: "fast_mode_update" }> =>
        update.type === "fast_mode_update",
    );
  assert.deepEqual(updates, [
    ...reasons.map((reason) => ({
      type: "fast_mode_update" as const,
      fast_mode_state: "off" as const,
      fast_mode_disabled_reason: reason,
    })),
    {
      type: "fast_mode_update",
      fast_mode_state: "off",
    },
  ]);
  assert.equal(session.fastModeDisabledReason, undefined);
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
    emitToolProgressUpdate(session, "tool-1");
  });

  assert.equal(events.length, 0);
  assert.equal(session.toolCalls.get("tool-1")?.status, "completed");
});

test("emitToolProgressUpdate does not create tools from progress", () => {
  const session = makeSessionState();

  const events = captureBridgeEvents(() => {
    emitToolProgressUpdate(session, "tool-progress-only");
  });

  assert.equal(events.length, 0);
  assert.equal(session.toolCalls.has("tool-progress-only"), false);
});

test("handleSdkMessage correlates heartbeat progress to its existing parent tool", () => {
  const session = makeSessionState();
  const parentToolCall = createToolCall("tool-shell-parent", "PowerShell", {
    command: "cargo test --all-features",
  });
  session.toolCalls.set(parentToolCall.tool_call_id, parentToolCall);

  const events = captureBridgeEvents(() => {
    for (let heartbeat = 0; heartbeat < 3; heartbeat += 1) {
      handleSdkMessage(session, {
        type: "tool_progress",
        tool_use_id: `tool-shell-parent-heartbeat-${heartbeat}`,
        tool_name: "PowerShell",
        parent_tool_use_id: parentToolCall.tool_call_id,
        elapsed_time_seconds: heartbeat + 1,
        heartbeat: true,
        uuid: `message-heartbeat-${heartbeat}`,
        session_id: "session-1",
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    }
  });

  assert.equal(events.length, 1);
  assert.deepEqual(events[0], {
    event: "session_update",
    session_id: "session-1",
    update: {
      type: "tool_call_update",
      tool_call_update: {
        tool_call_id: "tool-shell-parent",
        fields: { status: "in_progress" },
      },
    },
  });
  assert.equal(session.toolCalls.size, 1);
  assert.equal(session.toolCalls.get(parentToolCall.tool_call_id)?.status, "in_progress");
  for (let heartbeat = 0; heartbeat < 3; heartbeat += 1) {
    assert.equal(session.toolCalls.has(`tool-shell-parent-heartbeat-${heartbeat}`), false);
  }
});

test("handleSdkMessage ignores orphaned tool progress", () => {
  const session = makeSessionState();

  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "tool_progress",
      tool_use_id: "tool-orphaned-heartbeat",
      tool_name: "PowerShell",
      parent_tool_use_id: "tool-missing-parent",
      elapsed_time_seconds: 1,
      heartbeat: true,
      uuid: "message-orphaned-heartbeat",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  assert.equal(events.length, 0);
  assert.equal(session.toolCalls.size, 0);
});

test("handleSdkMessage updates and clears subagent retry progress in place", () => {
  const session = makeSessionState();
  const toolCall = createToolCall("tool-agent-retry", "Agent", {
    description: "Review changes",
    prompt: "Review the branch",
  });
  toolCall.status = "in_progress";
  session.toolCalls.set(toolCall.tool_call_id, toolCall);

  const waitingEvents = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "tool_progress",
      tool_use_id: toolCall.tool_call_id,
      tool_name: "Agent",
      parent_tool_use_id: null,
      elapsed_time_seconds: 5,
      heartbeat: true,
      subagent_type: "reviewer",
      subagent_retry: {
        agent_id: "agent-1",
        attempt: 2,
        max_retries: 4,
        retry_delay_ms: 1_500,
        error_status: 429,
        error_category: "rate_limit",
      },
      uuid: "message-retry-waiting",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  assert.deepEqual(waitingEvents.at(-1), {
    event: "session_update",
    session_id: "session-1",
    update: {
      type: "tool_call_update",
      tool_call_update: {
        tool_call_id: "tool-agent-retry",
        fields: {
          task_metadata: {
            subagent_retry: {
              state: "waiting",
              agent_id: "agent-1",
              attempt: 2,
              max_retries: 4,
              retry_delay_ms: 1_500,
              error_status: 429,
              error_category: "rate_limit",
            },
            subagent_type: "reviewer",
          },
        },
      },
    },
  });
  assert.equal(session.toolCalls.get(toolCall.tool_call_id)?.status, "in_progress");
  assert.equal(
    session.toolCalls.get(toolCall.tool_call_id)?.task_metadata?.subagent_retry?.state,
    "waiting",
  );

  const clearEvents = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "tool_progress",
      tool_use_id: toolCall.tool_call_id,
      tool_name: "Agent",
      parent_tool_use_id: null,
      elapsed_time_seconds: 7,
      heartbeat: true,
      subagent_type: "reviewer",
      uuid: "message-retry-cleared",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  assert.deepEqual(clearEvents.at(-1), {
    event: "session_update",
    session_id: "session-1",
    update: {
      type: "tool_call_update",
      tool_call_update: {
        tool_call_id: "tool-agent-retry",
        fields: {
          task_metadata: { subagent_retry: { state: "clear" } },
        },
      },
    },
  });
  assert.equal(
    session.toolCalls.get(toolCall.tool_call_id)?.task_metadata?.subagent_retry,
    undefined,
  );
});

test("handleSdkMessage ignores malformed subagent retry progress", () => {
  const session = makeSessionState();
  const toolCall = createToolCall("tool-agent-invalid-retry", "Agent", {
    prompt: "Review the branch",
  });
  toolCall.status = "in_progress";
  session.toolCalls.set(toolCall.tool_call_id, toolCall);

  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "tool_progress",
      tool_use_id: toolCall.tool_call_id,
      tool_name: "Agent",
      parent_tool_use_id: null,
      elapsed_time_seconds: 5,
      subagent_retry: {
        agent_id: "agent-1",
        attempt: 1.5,
        max_retries: 4,
        retry_delay_ms: -1,
        error_status: null,
        error_category: "rate_limit",
      },
      uuid: "message-invalid-retry",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  assert.deepEqual(events, []);
  assert.equal(session.toolCalls.get(toolCall.tool_call_id)?.task_metadata, undefined);
});

test("emitToolResultUpdate clears active subagent retry metadata", () => {
  const session = makeSessionState();
  const toolCall = createToolCall("tool-agent-retry-result", "Agent", {
    prompt: "Review the branch",
  });
  toolCall.status = "in_progress";
  toolCall.task_metadata = {
    subagent_retry: {
      state: "waiting",
      agent_id: "agent-1",
      attempt: 2,
      max_retries: 4,
      retry_delay_ms: 1_500,
    },
  };
  session.toolCalls.set(toolCall.tool_call_id, toolCall);

  const events = captureBridgeEvents(() => {
    emitToolResultUpdate(session, toolCall.tool_call_id, false, {
      agentId: "agent-1",
      content: [{ type: "text", text: "Done" }],
      status: "completed",
      prompt: "Review the branch",
    });
  });

  const update = events.at(-1)?.update as Record<string, unknown>;
  const toolCallUpdate = update.tool_call_update as Record<string, unknown>;
  const fields = toolCallUpdate.fields as Record<string, unknown>;
  assert.deepEqual(fields.task_metadata, { subagent_retry: { state: "clear" } });
  assert.equal(session.toolCalls.get(toolCall.tool_call_id)?.task_metadata?.subagent_retry, undefined);
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

  const completedQuestionUpdate = events
    .map((event) => (event.event === "session_update" ? (event.update as Record<string, unknown>) : undefined))
    .find((update) => {
      const toolCallUpdate = update?.tool_call_update as Record<string, unknown> | undefined;
      const fields = toolCallUpdate?.fields as Record<string, unknown> | undefined;
      return toolCallUpdate?.tool_call_id === "tool-question" && fields?.status === "completed";
    })?.tool_call_update as Record<string, unknown> | undefined;
  const completedFields = completedQuestionUpdate?.fields as Record<string, unknown> | undefined;
  assert.deepEqual(completedFields?.raw_input, {
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
    question_results: [
      {
        question: "Pick deployment target",
        header: "Target",
        question_index: 0,
        total_questions: 1,
        selected_options: [
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
        annotation: {
          preview: "Deploy to staging first.\n\nDeploy to production after approval.",
          notes: "Roll out in both environments",
        },
      },
    ],
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
  assert.equal(normalizeToolKind("ReadMcpResourceDir"), "read");
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

test("ReadMcpResourceDir titles include server and URI context", () => {
  assert.equal(
    createToolCall("tc-mcp-dir-title", "ReadMcpResourceDir", {
      server: "docs",
      uri: "file://manuals/",
    }).title,
    "ReadMcpResourceDir docs file://manuals/",
  );
  assert.equal(
    createToolCall("tc-mcp-dir-uri-title", "ReadMcpResourceDir", {
      uri: "file://manuals/",
    }).title,
    "ReadMcpResourceDir file://manuals/",
  );
  assert.equal(
    createToolCall("tc-mcp-dir-fallback-title", "ReadMcpResourceDir", {}).title,
    "ReadMcpResourceDir",
  );
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
    errorCode: "credits_required",
    canUserPurchaseCredits: true,
    hasChargeableSavedPaymentMethod: false,
  });

  assert.deepEqual(update, {
    type: "rate_limit_update",
    status: "allowed_warning",
    error_code: "credits_required",
    resets_at: 1_741_280_000,
    utilization: 0.92,
    rate_limit_type: "five_hour",
    overage_status: "rejected",
    overage_resets_at: 1_741_280_600,
    overage_disabled_reason: "out_of_credits",
    is_using_overage: false,
    surpassed_threshold: 0.9,
    can_user_purchase_credits: true,
    has_chargeable_saved_payment_method: false,
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
  assert.deepEqual(buildRateLimitUpdate({ status: "rejected", errorCode: "other" }), {
    type: "rate_limit_update",
    status: "rejected",
  });
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

test("handleSdkMessage emits one typed compaction lifecycle", () => {
  const session = makeSessionState();
  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "system",
      subtype: "status",
      status: "compacting",
      uuid: "compact-started",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(session, {
      type: "system",
      subtype: "compact_boundary",
      compact_metadata: {
        trigger: "auto",
        pre_tokens: 180_000,
        post_tokens: 22_000,
        duration_ms: 1_250,
      },
      uuid: "compact-boundary",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(session, {
      type: "system",
      subtype: "status",
      status: null,
      compact_result: "success",
      uuid: "compact-finished",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(session, {
      type: "system",
      subtype: "status",
      status: null,
      compact_result: "failed",
      compact_error: "  Prompt is too long  ",
      uuid: "compact-failed",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  assert.deepEqual(
    events.map((event) => event.update),
    [
      { type: "compaction_update", phase: "started" },
      {
        type: "compaction_update",
        phase: "boundary",
        trigger: "auto",
        pre_tokens: 180_000,
        post_tokens: 22_000,
        duration_ms: 1_250,
      },
      { type: "compaction_update", phase: "finished", result: "success" },
      { type: "session_status_update", status: "idle" },
      {
        type: "compaction_update",
        phase: "finished",
        result: "failed",
        error_code: "unknown",
        error: "Prompt is too long",
      },
      { type: "session_status_update", status: "idle" },
    ],
  );
});

test("handleSdkMessage classifies too_few_groups compaction failures", () => {
  const session = makeSessionState();
  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "system",
      subtype: "status",
      status: null,
      compact_result: "failed",
      compact_error: "too_few_groups",
      uuid: "compact-failed",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  assert.deepEqual(
    events.map((event) => event.update),
    [
      {
        type: "compaction_update",
        phase: "finished",
        result: "failed",
        error_code: "too_few_groups",
        error: "too_few_groups",
      },
      { type: "session_status_update", status: "idle" },
    ],
  );
});

test("handleSdkMessage rejects invalid compaction boundary counters", () => {
  const session = makeSessionState();
  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "system",
      subtype: "compact_boundary",
      compact_metadata: { trigger: "manual", pre_tokens: -1 },
      uuid: "compact-boundary",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  assert.deepEqual(events, []);
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

test("commands_changed wins an actual supportedCommands bootstrap race", async () => {
  const session = makeSessionState();
  let resolveSupportedCommands:
    | ((commands: import("@anthropic-ai/claude-agent-sdk").SlashCommand[]) => void)
    | undefined;
  session.query = {
    supportedCommands: () =>
      new Promise<import("@anthropic-ai/claude-agent-sdk").SlashCommand[]>((resolve) => {
        resolveSupportedCommands = resolve;
      }),
  } as unknown as import("@anthropic-ai/claude-agent-sdk").Query;

  captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "system",
      subtype: "init",
      session_id: "session-1",
      model: "haiku",
      slash_commands: ["/bootstrap"],
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(session, {
      type: "system",
      subtype: "commands_changed",
      session_id: "session-1",
      commands: [{ name: "/dynamic", description: "Dynamic command" }],
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  assert.ok(resolveSupportedCommands);
  resolveSupportedCommands([
    { name: "/stale", description: "Stale bootstrap", argumentHint: "" },
  ]);
  await new Promise<void>((resolve) => setImmediate(resolve));

  assert.equal(session.availableCommands?.source, "commands_changed");
  assert.deepEqual(session.availableCommands?.commands, [
    { name: "/dynamic", description: "Dynamic command", input_hint: undefined },
  ]);
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

test("handleSdkMessage maps informational system messages to notices by level", () => {
  const session = makeSessionState();
  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "system",
      subtype: "informational",
      content: "  Sync ready  ",
      level: "notice",
      uuid: "message-info-notice",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(session, {
      type: "system",
      subtype: "informational",
      content: "Try /compact",
      level: "suggestion",
      uuid: "message-info-suggestion",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(session, {
      type: "system",
      subtype: "informational",
      content: "Hook blocked continuation",
      level: "warning",
      uuid: "message-info-warning",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  assert.deepEqual(events.map((event) => event.update), [
    { type: "system_notice_update", severity: "info", message: "Sync ready" },
    { type: "system_notice_update", severity: "info", message: "Suggestion: Try /compact" },
    { type: "system_notice_update", severity: "warning", message: "Hook blocked continuation" },
  ]);
});

test("handleSdkMessage keeps informational info log-only unless continuation is prevented", () => {
  const session = makeSessionState();
  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "system",
      subtype: "informational",
      content: "Transcript-only progress",
      level: "info",
      uuid: "message-info-log-only",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(session, {
      type: "system",
      subtype: "informational",
      content: "Stop hook denied continuation",
      level: "info",
      prevent_continuation: true,
      uuid: "message-info-prevented",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  assert.deepEqual(events.map((event) => event.update), [
    {
      type: "system_notice_update",
      severity: "warning",
      message: "Stop hook denied continuation",
    },
  ]);
});

test("handleSdkMessage deduplicates informational messages by tool use, level, and content", () => {
  const session = makeSessionState();
  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "system",
      subtype: "informational",
      content: "Progress",
      level: "notice",
      tool_use_id: "tool-1",
      uuid: "message-info-1",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(session, {
      type: "system",
      subtype: "informational",
      content: " Progress ",
      level: "notice",
      tool_use_id: "tool-1",
      uuid: "message-info-2",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(session, {
      type: "system",
      subtype: "informational",
      content: "Progress updated",
      level: "notice",
      tool_use_id: "tool-1",
      uuid: "message-info-3",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  assert.deepEqual(events.map((event) => event.update), [
    { type: "system_notice_update", severity: "info", message: "Progress" },
    { type: "system_notice_update", severity: "info", message: "Progress updated" },
  ]);
});

test("handleSdkMessage does not deduplicate informational messages without a tool use id", () => {
  const session = makeSessionState();
  const events = captureBridgeEvents(() => {
    for (const uuid of ["message-info-1", "message-info-2"]) {
      handleSdkMessage(session, {
        type: "system",
        subtype: "informational",
        content: "Repeated global notice",
        level: "notice",
        uuid,
        session_id: "session-1",
      } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    }
  });

  assert.deepEqual(events.map((event) => event.update), [
    { type: "system_notice_update", severity: "info", message: "Repeated global notice" },
    { type: "system_notice_update", severity: "info", message: "Repeated global notice" },
  ]);
});

test("handleSdkMessage treats worker shutdown before connect as log-only", () => {
  const session = makeSessionState();
  session.connected = false;
  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "system",
      subtype: "worker_shutting_down",
      reason: "host_exit",
      uuid: "message-worker-shutdown",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    flushPendingWorkerShutdown(session);
  });

  assert.deepEqual(events, []);
  assert.equal(session.pendingWorkerShutdown, undefined);
});

test("handleSdkMessage flushes connected worker shutdown only when stream ends", () => {
  const session = makeSessionState();
  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "system",
      subtype: "worker_shutting_down",
      reason: "host_exit",
      uuid: "message-worker-shutdown",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    flushPendingWorkerShutdown(session);
  });

  assert.deepEqual(events.map((event) => event.update), [
    {
      type: "system_notice_update",
      severity: "warning",
      message: "Claude worker is shutting down: host_exit",
    },
  ]);
});

test("handleSdkMessage cancels pending worker shutdown after later SDK activity", () => {
  const session = makeSessionState();
  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "system",
      subtype: "worker_shutting_down",
      reason: "host_exit",
      uuid: "message-worker-shutdown",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(session, {
      type: "system",
      subtype: "notification",
      text: "Still running",
      priority: "low",
      uuid: "message-notification",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    flushPendingWorkerShutdown(session);
  });

  assert.deepEqual(events.map((event) => event.update), [
    { type: "system_notice_update", severity: "info", message: "Still running" },
  ]);
});

test("handleSdkMessage ignores unknown future system subtypes", () => {
  const session = makeSessionState();
  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "system",
      subtype: "future_subtype",
      content: "Unknown",
      uuid: "message-future",
      session_id: "session-1",
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  assert.deepEqual(events, []);
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

test("parseCommandEnvelope validates set_fast_mode command", () => {
  for (const enabled of [true, false]) {
    const parsed = parseCommandEnvelope(
      JSON.stringify({
        request_id: "req-fast",
        command: "set_fast_mode",
        session_id: "session-1",
        enabled,
      }),
    );

    assert.equal(parsed.requestId, "req-fast");
    assert.equal(parsed.command.command, "set_fast_mode");
    if (parsed.command.command !== "set_fast_mode") {
      throw new Error("unexpected command variant");
    }
    assert.equal(parsed.command.session_id, "session-1");
    assert.equal(parsed.command.enabled, enabled);
  }
});

test("parseCommandEnvelope rejects invalid set_fast_mode values", () => {
  for (const enabled of [undefined, "true", 1, null, {}, []]) {
    assert.throws(
      () =>
        parseCommandEnvelope(
          JSON.stringify({
            command: "set_fast_mode",
            session_id: "session-1",
            ...(enabled !== undefined ? { enabled } : {}),
          }),
        ),
      /set_fast_mode\.enabled must be a boolean/,
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

test("applySessionFastMode applies live settings and decodes the SDK's sparse off state", async () => {
  const calls: unknown[] = [];
  const responses = [
    { fast_mode_state: "on", fast_mode_disabled_reason: "pending" },
    { fast_mode_state: "cooldown" },
    {},
    { fast_mode_state: "off" },
  ] as const;
  const query = {
    async applyFlagSettings(settings: unknown): Promise<void> {
      calls.push(settings);
    },
    async reinitialize(): Promise<(typeof responses)[number]> {
      return responses[calls.length - 1];
    },
  } as unknown as import("@anthropic-ai/claude-agent-sdk").Query;

  assert.deepEqual(await applySessionFastMode(query, true), {
    state: "on",
    disabled_reason: "pending",
  });
  assert.deepEqual(await applySessionFastMode(query, true), { state: "cooldown" });
  assert.deepEqual(await applySessionFastMode(query, false), { state: "off" });
  assert.deepEqual(await applySessionFastMode(query, false), { state: "off" });
  assert.deepEqual(calls, [
    { fastMode: true },
    { fastMode: true },
    { fastMode: false },
    { fastMode: false },
  ]);
});

test("applySessionFastMode reports apply, refresh, and missing enabled-state failures", async () => {
  const rejectedApply = {
    async applyFlagSettings(): Promise<void> {
      throw new Error("not entitled");
    },
  } as unknown as import("@anthropic-ai/claude-agent-sdk").Query;
  await assert.rejects(
    applySessionFastMode(rejectedApply, true),
    /SDK rejected the fast-mode change: not entitled/,
  );

  const rejectedRefresh = {
    async applyFlagSettings(): Promise<void> {},
    async reinitialize(): Promise<never> {
      throw new Error("transport closed");
    },
  } as unknown as import("@anthropic-ai/claude-agent-sdk").Query;
  await assert.rejects(
    applySessionFastMode(rejectedRefresh, true),
    /SDK accepted the fast-mode change but state verification failed: transport closed/,
  );

  const missingEnabledState = {
    async applyFlagSettings(): Promise<void> {},
    async reinitialize(): Promise<Record<string, never>> {
      return {};
    },
  } as unknown as import("@anthropic-ai/claude-agent-sdk").Query;
  await assert.rejects(
    applySessionFastMode(missingEnabledState, true),
    /SDK accepted the fast-mode change but did not report its resulting state/,
  );
});

test("buildPromptUserMessage attributes keyboard text input to a human", () => {
  assert.deepEqual(
    buildPromptUserMessage(
      {
        command: "prompt",
        session_id: "session-1",
        chunks: [{ kind: "text", value: "hello" }],
      },
      "session-1",
    ),
    {
      type: "user",
      session_id: "session-1",
      parent_tool_use_id: null,
      origin: { kind: "human" },
      message: {
        role: "user",
        content: [{ type: "text", text: "hello" }],
      },
    },
  );
});

test("buildPromptUserMessage attributes structured keyboard input to a human", () => {
  const message = buildPromptUserMessage(
    {
      command: "prompt",
      session_id: "session-1",
      chunks: [
        { kind: "text", value: "inspect this" },
        {
          kind: "image",
          value: { mime_type: "image/png", data: "aGVsbG8=" },
        },
      ],
    },
    "session-1",
  );

  assert.equal(message?.origin?.kind, "human");
  assert.equal(Array.isArray(message?.message.content), true);
  assert.equal(message?.message.content.length, 2);
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
    "anthropicGoogleCloud",
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
  assert.equal(resolveInstalledAgentSdkVersion(), "0.3.220");
  assert.equal(agentSdkVersionCompatibilityError(), undefined);
});

test("mapSessionMessagesToUpdates maps message content blocks", () => {
  const updates = mapSessionMessagesToUpdates([
    {
      type: "user",
      uuid: "u1",
      session_id: "s1",
      parent_tool_use_id: null,
      parent_agent_id: null,
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
      parent_agent_id: null,
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
      parent_agent_id: null,
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
      parent_agent_id: null,
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
      parent_agent_id: null,
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
      parent_agent_id: null,
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
      parent_agent_id: null,
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

test("handleSdkMessage correlates tool non-execution metadata by tool ID", () => {
  const session = makeSessionState();
  const cases = [
    ["tool-user-rejected", "user-rejected", "failed"],
    ["tool-permission-rule", "permission-rule", "failed"],
    ["tool-automode-unavailable", "automode-unavailable", "failed"],
    ["tool-automode-parsing-error", "automode-parsing-error", "failed"],
    ["tool-automode-blocked", "automode-blocked", "failed"],
    ["tool-cancelled", "cancelled", "killed"],
    ["tool-interrupted", "interrupted", "killed"],
    ["tool-future", "future-reason", "completed"],
  ] as const;

  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "assistant",
      uuid: "assistant-tools",
      session_id: "session-1",
      message: {
        role: "assistant",
        content: cases.map(([id]) => ({
          type: "tool_use",
          id,
          name: "Bash",
          input: { command: `echo ${id}` },
        })),
      },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(session, {
      type: "user",
      uuid: "user-results",
      session_id: "session-1",
      message: {
        role: "user",
        content: [...cases].reverse().map(([id]) => ({
          type: "tool_result",
          tool_use_id: id,
          content: `raw output for ${id}`,
          is_error: false,
        })),
      },
      tool_result_meta: [
        ...cases.map(([id, kind]) => ({
          id,
          non_execution_kind: kind,
          ...(id === "tool-user-rejected" ? { user_feedback: "Please use a safer command." } : {}),
        })),
      ].reverse(),
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  const updates = events
    .map((event) => event.update as import("./types.js").SessionUpdate | undefined)
    .filter(
      (update): update is Extract<import("./types.js").SessionUpdate, { type: "tool_call_update" }> =>
        update?.type === "tool_call_update",
    );
  assert.equal(updates.length, cases.length);
  for (const [id, kind, status] of cases) {
    const fields = updates.find((update) => update.tool_call_update.tool_call_id === id)
      ?.tool_call_update.fields;
    assert.equal(fields?.status, status);
    assert.equal(fields?.raw_output, `raw output for ${id}`);
    assert.equal(fields?.output_metadata?.non_execution?.kind, kind);
  }
  assert.equal(
    updates.find(
      (update) => update.tool_call_update.tool_call_id === "tool-user-rejected",
    )?.tool_call_update.fields.output_metadata?.non_execution?.user_feedback,
    "Please use a safer command.",
  );
});

test("handleSdkMessage ignores malformed and mismatched tool non-execution metadata", () => {
  const session = makeSessionState();
  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "assistant",
      uuid: "assistant-tool",
      session_id: "session-1",
      message: {
        role: "assistant",
        content: [{ type: "tool_use", id: "tool-ok", name: "Bash", input: { command: "echo ok" } }],
      },
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
    handleSdkMessage(session, {
      type: "user",
      uuid: "user-result",
      session_id: "session-1",
      message: {
        role: "user",
        content: [{ type: "tool_result", tool_use_id: "tool-ok", content: "ok", is_error: false }],
      },
      tool_result_meta: [
        null,
        {},
        { id: "", non_execution_kind: "cancelled" },
        { id: "tool-ok", non_execution_kind: "" },
        { id: "other-tool", non_execution_kind: "cancelled" },
      ],
    } as unknown as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  const update = events
    .map((event) => event.update as import("./types.js").SessionUpdate | undefined)
    .find((candidate) => candidate?.type === "tool_call_update");
  assert.equal(update?.type, "tool_call_update");
  if (update?.type !== "tool_call_update") {
    throw new Error("expected tool update");
  }
  assert.equal(update.tool_call_update.fields.status, "completed");
  assert.equal(update.tool_call_update.fields.output_metadata, undefined);
});

test("mapSessionMessagesToUpdates carries tool non-execution metadata from history", () => {
  const updates = mapSessionMessagesToUpdates([
    {
      type: "assistant",
      uuid: "a1",
      session_id: "s1",
      parent_tool_use_id: null,
      parent_agent_id: null,
      message: {
        role: "assistant",
        content: [
          { type: "tool_use", id: "tool-1", name: "Bash", input: { command: "echo ok" } },
        ],
      },
    },
    {
      type: "user",
      uuid: "u1",
      session_id: "s1",
      parent_tool_use_id: null,
      parent_agent_id: null,
      tool_result_meta: [
        {
          id: "tool-1",
          non_execution_kind: "interrupted",
          user_feedback: "Stopped intentionally.",
        },
      ],
      message: {
        role: "user",
        content: [
          {
            type: "tool_result",
            tool_use_id: "tool-1",
            content: "partial output",
            is_error: false,
          },
        ],
      },
    },
  ] as unknown as SessionMessage[]);

  const update = updates.find((candidate) => candidate.type === "tool_call_update");
  assert.equal(update?.type, "tool_call_update");
  if (update?.type !== "tool_call_update") {
    throw new Error("expected tool update");
  }
  assert.equal(update.tool_call_update.fields.status, "killed");
  assert.deepEqual(update.tool_call_update.fields.output_metadata?.non_execution, {
    kind: "interrupted",
    user_feedback: "Stopped intentionally.",
  });
});

test("mapSessionMessagesToUpdates maps task system records from resume history", () => {
  const updates = mapSessionMessagesToUpdates([
    {
      type: "assistant",
      uuid: "assistant-agent",
      session_id: "s1",
      parent_tool_use_id: null,
      parent_agent_id: null,
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
      parent_agent_id: null,
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
      parent_agent_id: null,
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
      parent_agent_id: null,
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
      parent_agent_id: null,
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

test("handleSdkMessage maps background_tasks_changed with background-only replacement semantics", () => {
  const session = makeSessionState();
  session.tasksById.set("task-list", {
    task_id: "task-list",
    subject: "From task list",
    status: "pending",
    blocks: [],
    blocked_by: [],
  });
  session.tasksById.set("bg-old", {
    task_id: "bg-old",
    subject: "Old background task",
    status: "in_progress",
    blocks: [],
    blocked_by: [],
    metadata: { sdk_background_task: true },
  });
  session.taskOrder.push("task-list", "bg-old");

  const events = captureBridgeEvents(() => {
    handleSdkMessage(session, {
      type: "system",
      subtype: "background_tasks_changed",
      tasks: [
        {
          task_id: "bg-new",
          task_type: "workflow_agent",
          description: "Run release checks",
        },
      ],
    } as import("@anthropic-ai/claude-agent-sdk").SDKMessage);
  });

  assert.deepEqual(events.at(-1), {
    event: "session_update",
    session_id: "session-1",
    update: {
      type: "task_state_update",
      source: "background_tasks",
      tasks: [
        {
          task_id: "bg-new",
          subject: "Run release checks",
          description: "Run release checks",
          status: "in_progress",
          blocks: [],
          blocked_by: [],
          metadata: {
            sdk_background_task: true,
            task_type: "workflow_agent",
          },
        },
      ],
      removed_task_ids: ["bg-old"],
      is_complete_snapshot: true,
    },
  });
  assert.equal(session.tasksById.has("task-list"), true);
  assert.equal(session.tasksById.has("bg-old"), false);
  assert.equal(session.tasksById.has("bg-new"), true);
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

test("handleResultMessage preserves new SDK terminal reasons", () => {
  const terminalReasons = [
    "api_error",
    "malformed_tool_use_exhausted",
    "budget_exhausted",
    "structured_output_retry_exhausted",
    "tool_deferred_unavailable",
    "turn_setup_failed",
  ] as const;

  for (const terminalReason of terminalReasons) {
    const session = makeSessionState();
    const events = captureBridgeEvents(() => {
      handleResultMessage(session, {
        type: "result",
        subtype: "success",
        terminal_reason: terminalReason,
      });
    });

    assert.equal(events.at(-1)?.event, "turn_complete");
    assert.equal(
      (events.at(-1) as { terminal_reason?: string } | undefined)?.terminal_reason,
      terminalReason,
    );
  }
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

test("handleResultMessage preserves target api error status shapes", () => {
  for (const apiErrorStatus of [429, 529, null, undefined]) {
    const session = makeSessionState();
    session.lastAssistantError = "overloaded";

    const events = captureBridgeEvents(() => {
      handleResultMessage(session, {
        type: "result",
        subtype: "error_during_execution",
        errors: ["service overloaded"],
        ...(apiErrorStatus !== undefined ? { api_error_status: apiErrorStatus } : {}),
      });
    });

    assert.deepEqual(events.at(-1), {
      event: "turn_error",
      session_id: "session-1",
      message: "service overloaded",
      error_kind: "transient_service",
      sdk_result_subtype: "error_during_execution",
      assistant_error: "overloaded",
      ...(typeof apiErrorStatus === "number" ? { api_error_status: apiErrorStatus } : {}),
    });
  }
});

test("mapSessionMessagesToUpdates ignores unsupported records", () => {
  const updates = mapSessionMessagesToUpdates([
    {
      type: "user",
      uuid: "u1",
      session_id: "s1",
      parent_tool_use_id: null,
      parent_agent_id: null,
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
      parent_agent_id: null,
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

test("buildSessionListOptions includes SDK-created sessions for resume listings", () => {
  assert.deepEqual(buildSessionListOptions("C:/repo"), {
    dir: "C:/repo",
    includeProgrammatic: true,
    includeWorktrees: true,
    limit: 50,
  });
  assert.deepEqual(buildSessionListOptions(undefined), {
    includeProgrammatic: true,
    limit: 50,
  });
});

test("mapAvailableModels preserves optional fast and auto mode metadata", () => {
  const mapped = mapAvailableModels([
    {
      value: "sonnet",
      resolvedModel: "claude-sonnet-5",
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
      resolved_model: "claude-sonnet-5",
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

test("mapAvailableModels preserves Fable models and unknown ids", () => {
  const mapped = mapAvailableModels([
    {
      value: "fable",
      resolvedModel: "claude-fable-5",
      displayName: "Claude Fable",
      description: "Default model alias",
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
      id: "fable",
      resolved_model: "claude-fable-5",
      display_name: "Claude Fable",
      description: "Default model alias",
      supports_effort: true,
      supported_effort_levels: [],
    },
    {
      id: "claude-fable-5",
      display_name: "Claude Fable 5",
      description: "Unavailable model",
      supports_effort: true,
      supported_effort_levels: [],
    },
    {
      id: "claude-fable-5-20260612",
      display_name: "Claude Fable 5 dated",
      description: "Unavailable dated model",
      supports_effort: true,
      supported_effort_levels: [],
    },
    {
      id: "claude-unknown-1",
      display_name: "Claude Unknown",
      description: "Unrecognized but available model",
      supports_effort: false,
      supported_effort_levels: [],
    },
  ]);
});

test("resolveCurrentModel matches full Fable runtime ids to the fable alias", () => {
  const session = makeSessionState();
  session.model = "fable";
  session.requestedModelId = "fable";
  session.resolvedRuntimeModelId = "claude-fable-5-20260612";
  session.availableModels = [
    {
      id: "fable",
      resolved_model: "claude-fable-5",
      display_name: "Claude Fable 5",
      supports_effort: true,
      supported_effort_levels: ["low", "medium", "high", "xhigh", "max"],
    },
  ];

  const currentModel = resolveCurrentModel(session);

  assert.equal(currentModel.display_name_short, "Fable 5");
  assert.equal(currentModel.display_name_long, "Claude Fable 5");
  assert.equal(currentModel.catalog_id, "fable");
  assert.equal(currentModel.supports_effort, true);
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
