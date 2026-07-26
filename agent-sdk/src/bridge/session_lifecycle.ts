import { randomUUID } from "node:crypto";
import { spawn as spawnChild } from "node:child_process";
import fs from "node:fs";
import {
  query,
  type CanUseTool,
  type Options,
  type PermissionMode,
  type PermissionResult,
  type PermissionUpdate,
  type Query,
  type SDKUserMessage,
  type SettingSource,
  type UserDialogRequest,
  type UserDialogResult,
} from "@anthropic-ai/claude-agent-sdk";
import type {
  CurrentModel,
  AvailableModel,
  BridgeCommand,
  BridgeEvent,
  ElicitationAction,
  ElicitationRequest,
  FastModeState,
  ApiRetryError,
  Json,
  PermissionOutcome,
  PermissionDisplay,
  PermissionRequest,
  QuestionOutcome,
  RefusalFallbackPromptChoice,
  RefusalFallbackPromptPayload,
  SessionLaunchSettings,
  SessionUpdate,
  TaskItem,
  ToolCall,
  UserDialogOption,
} from "../types.js";
import { bridgeLogger, LOG_TARGETS, logSdkStderrLine } from "./logger.js";
import { AsyncQueue } from "./shared.js";
import {
  permissionOptionsFromSuggestions,
  permissionResultFromOutcome,
} from "./permissions.js";
import {
  failConnection,
  emitSessionUpdate,
  emitConnectEvent,
  emitPermissionRequestEvent,
  emitElicitationRequestEvent,
  emitUserDialogRequestEvent,
} from "./events.js";
import {
  ensureToolCallVisible,
  setToolCallStatus,
} from "./tool_calls.js";
import { isToolSearchToolName } from "./tooling.js";
import {
  requestExitPlanModeApproval,
  requestAskUserQuestionAnswers,
  EXIT_PLAN_MODE_TOOL_NAME,
  ASK_USER_QUESTION_TOOL_NAME,
} from "./user_interaction.js";
import { mapAvailableAgents, emitAvailableAgentsIfChanged, refreshAvailableAgents } from "./agents.js";
import {
  mapSdkSlashCommands,
  updateAvailableCommands,
  type AvailableCommandsSnapshot,
} from "./available_commands.js";
import {
  emitAuthRequired,
  emitFastModeUpdate,
  setFastModeStateIfChanged,
} from "./error_classification.js";
import {
  mapAvailableModels,
  resolveCurrentModel,
  currentModelsEqual,
} from "./model_metadata.js";
import { shouldEmitStartupAuthRequiredForAccount } from "./account_metadata.js";
import type { McpAuthMonitorHandle } from "./mcp_monitor.js";

export { mapAvailableModels, resolveCurrentModel } from "./model_metadata.js";
export { shouldEmitStartupAuthRequiredForAccount } from "./account_metadata.js";

export type ConnectEventKind = "connected" | "session_replaced";

export type PendingRewindResult = Omit<
  Extract<BridgeEvent, { event: "rewind_result" }>,
  "session_id"
>;

const BRIDGE_RUNTIME_PROCESS_NAME =
  process.platform === "win32" ? "claude-rs-bridge-bun.exe" : "claude-rs-bridge-bun";
const BRIDGE_RUNTIME_GUARD_PROMPT =
  `Do not terminate the Claude Rust bridge runtime process \`${BRIDGE_RUNTIME_PROCESS_NAME}\`; ` +
  "when cleaning up development servers, only stop processes by explicit PIDs you started in this session.";
const STARTUP_FALLBACK_MODEL_ALIAS = "fable";

function permissionDisplayFromCanUseOptions(
  options: Parameters<CanUseTool>[2],
): PermissionDisplay | undefined {
  const title = typeof options.title === "string" ? options.title.trim() : "";
  const displayName = typeof options.displayName === "string" ? options.displayName.trim() : "";
  const description = typeof options.description === "string" ? options.description.trim() : "";
  if (!title && !displayName && !description) {
    return undefined;
  }
  return {
    ...(title ? { title } : {}),
    ...(displayName ? { display_name: displayName } : {}),
    ...(description ? { description } : {}),
  };
}

export type PendingPermission = {
  resolve?: (result: PermissionResult) => void;
  onOutcome?: (outcome: PermissionOutcome) => void;
  toolName: string;
  inputData: Record<string, unknown>;
  suggestions?: PermissionUpdate[];
};

export type PendingQuestion = {
  onOutcome: (outcome: QuestionOutcome) => void;
  toolName: string;
  inputData: Record<string, unknown>;
};

export type PendingUserDialog = {
  resolve: (choice: RefusalFallbackPromptChoice | "cancelled") => void;
};

export type PendingElicitation = {
  resolve: (result: {
    action: ElicitationAction;
    content?: Record<string, string | number | boolean | string[]>;
  }) => void;
  serverName: string;
  elicitationId?: string;
};

export type PendingWorkerShutdown = {
  reason: string;
};

export type SessionState = {
  sessionId: string;
  cwd: string;
  model: string;
  requestedModelId?: string;
  resolvedRuntimeModelId?: string;
  currentModel?: CurrentModel;
  availableModels: AvailableModel[];
  mode: PermissionMode | null;
  supportedModeIds: PermissionMode[];
  runtimeUnavailableModeIds: PermissionMode[];
  supportsBypassPermissionsMode: boolean;
  fastModeState: FastModeState;
  query: Query;
  initializationTask?: Promise<void>;
  queryConsumerTask?: Promise<void>;
  input: AsyncQueue<SDKUserMessage>;
  connected: boolean;
  closing: boolean;
  connectEvent: ConnectEventKind;
  connectRequestId?: string;
  toolCalls: Map<string, ToolCall>;
  tasksById: Map<string, TaskItem>;
  taskOrder: string[];
  taskToolUseIds: Map<string, string>;
  taskIdsByToolUseId: Map<string, string>;
  pendingPermissions: Map<string, PendingPermission>;
  pendingQuestions: Map<string, PendingQuestion>;
  pendingUserDialogs: Map<string, PendingUserDialog>;
  pendingElicitations: Map<string, PendingElicitation>;
  informationalDedupKeys: Set<string>;
  pendingWorkerShutdown?: PendingWorkerShutdown;
  knownConnectedMcpServers: Set<string>;
  mcpStatusRevalidatedAt: Map<string, number>;
  mcpAuthMonitors: Map<string, McpAuthMonitorHandle>;
  hiddenToolUseIds: Set<string>;
  authHintSent: boolean;
  lastAvailableAgentsSignature?: string;
  availableCommands?: AvailableCommandsSnapshot;
  lastAssistantError?: ApiRetryError;
  sessionsToCloseAfterConnect?: SessionState[];
  resumeUpdates?: SessionUpdate[];
  restoredInput?: string;
  pendingRewindResult?: PendingRewindResult;
};

export const sessions = new Map<string, SessionState>();
const pendingSessionCloseTasks = new Set<Promise<void>>();

const DEFAULT_SETTING_SOURCES: SettingSource[] = ["user", "project", "local"];
const DEFAULT_PERMISSION_MODE: PermissionMode = "default";

function isSdkElicitationContentValue(value: Json): value is string | number | boolean | string[] {
  return (
    typeof value === "string" ||
    typeof value === "number" ||
    typeof value === "boolean" ||
    (Array.isArray(value) && value.every((entry) => typeof entry === "string"))
  );
}

function normalizeSdkElicitationContent(
  content: Record<string, Json> | undefined,
): Record<string, string | number | boolean | string[]> | undefined {
  if (!content) {
    return undefined;
  }
  const normalized: Record<string, string | number | boolean | string[]> = {};
  for (const [key, value] of Object.entries(content)) {
    if (isSdkElicitationContentValue(value)) {
      normalized[key] = value;
    }
  }
  return Object.keys(normalized).length > 0 ? normalized : undefined;
}

const REFUSAL_FALLBACK_DIALOG_KIND = "refusal_fallback_prompt";

function optionalString(value: unknown): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

function optionalStringArray(value: unknown): string[] | undefined {
  if (!Array.isArray(value)) {
    return undefined;
  }
  const entries = value.filter((entry): entry is string => typeof entry === "string");
  return entries.length > 0 ? entries : undefined;
}

/**
 * Normalize the camelCase `refusal_fallback_prompt` payload built by the CLI
 * into the snake-case host wire shape. The dialog descriptor requires
 * `originalModel`/`fallbackModel`; the rest are optional metadata.
 */
function normalizeRefusalFallbackPayload(
  payload: Record<string, unknown>,
): RefusalFallbackPromptPayload {
  return {
    original_model: typeof payload.originalModel === "string" ? payload.originalModel : "",
    fallback_model: typeof payload.fallbackModel === "string" ? payload.fallbackModel : "",
    ...(optionalString(payload.apiRefusalCategory) !== undefined
      ? { api_refusal_category: optionalString(payload.apiRefusalCategory) }
      : {}),
    ...(optionalString(payload.guidanceText) !== undefined
      ? { guidance_text: optionalString(payload.guidanceText) }
      : {}),
    ...(optionalStringArray(payload.retractedMessageUuids) !== undefined
      ? { retracted_message_uuids: optionalStringArray(payload.retractedMessageUuids) }
      : {}),
  };
}

/**
 * Build the selectable options the host renders, mirroring the labels the CLI
 * itself produces (`function Wg$`). `cancelled` is the Esc/decline default and
 * is not a listed option.
 */
function buildRefusalFallbackOptions(payload: RefusalFallbackPromptPayload): UserDialogOption[] {
  return [
    { option_id: "retry_fallback", label: `Switch to ${payload.fallback_model}` },
    { option_id: "edit_prompt", label: `Edit prompt and retry with ${payload.original_model}` },
  ];
}

type CloseSessionOptions = {
  reason?: string;
  requestId?: string;
};

function settingsObjectFromLaunchSettings(
  launchSettings: SessionLaunchSettings,
): Record<string, unknown> | undefined {
  return launchSettings.settings;
}

function normalizedSettingsFromLaunchSettings(
  launchSettings: SessionLaunchSettings,
): Record<string, unknown> {
  const settings = settingsObjectFromLaunchSettings(launchSettings) ?? {};
  // SendFeedback queues a local draft that can only be reviewed, edited, and
  // discarded through the native /feedback surface. Agent SDK command
  // snapshots do not expose that command to this host, so enabling drafts
  // would create content that claude-rs cannot safely let the user approve.
  const hostSettings = {
    ...settings,
    feedbackDrafts: "off" as const,
  };

  const sandbox =
    settings.sandbox && typeof settings.sandbox === "object" && !Array.isArray(settings.sandbox)
      ? (settings.sandbox as Record<string, unknown>)
      : undefined;
  if (sandbox?.enabled === true && sandbox.failIfUnavailable === undefined) {
    return {
      ...hostSettings,
      sandbox: {
        ...sandbox,
        failIfUnavailable: false,
      },
    };
  }

  return hostSettings;
}

export function sessionById(sessionId: string): SessionState | null {
  return sessions.get(sessionId) ?? null;
}

export function updateSessionId(session: SessionState, newSessionId: string): void {
  if (session.sessionId === newSessionId) {
    return;
  }
  sessions.delete(session.sessionId);
  session.sessionId = newSessionId;
  sessions.set(newSessionId, session);
}

export function beginSessionClose(session: SessionState): void {
  session.closing = true;
  for (const monitor of session.mcpAuthMonitors.values()) {
    monitor.controller.abort();
  }
}

export function detachSessionForClose(session: SessionState): void {
  beginSessionClose(session);
  if (sessions.get(session.sessionId) === session) {
    sessions.delete(session.sessionId);
  }
}

export function trackSessionCloseTask(task: Promise<void>): void {
  const ownedTask = task.catch((error: unknown) => {
    bridgeLogger.error({
      target: LOG_TARGETS.APP_SESSION,
      eventName: "session_close_task_failed",
      message: "background session cleanup failed",
      outcome: "failure",
      fields: {
        error_message: error instanceof Error ? error.message : String(error),
      },
    });
  });
  pendingSessionCloseTasks.add(ownedTask);
  void ownedTask.then(() => {
    pendingSessionCloseTasks.delete(ownedTask);
  });
}

async function waitForPendingSessionCloseTasks(): Promise<void> {
  while (pendingSessionCloseTasks.size > 0) {
    await Promise.all(Array.from(pendingSessionCloseTasks));
  }
}

export async function closeSession(session: SessionState): Promise<void> {
  beginSessionClose(session);
  const mcpAuthMonitors = Array.from(session.mcpAuthMonitors.values());
  session.mcpAuthMonitors.clear();
  session.input.close();
  session.query.close();
  for (const pending of session.pendingPermissions.values()) {
    pending.resolve?.({ behavior: "deny", message: "Session closed" });
    pending.onOutcome?.({ outcome: "cancelled" });
  }
  session.pendingPermissions.clear();
  for (const pending of session.pendingQuestions.values()) {
    pending.onOutcome({ outcome: "cancelled" });
  }
  session.pendingQuestions.clear();
  for (const pending of session.pendingUserDialogs.values()) {
    pending.resolve("cancelled");
  }
  session.pendingUserDialogs.clear();
  for (const pending of session.pendingElicitations.values()) {
    pending.resolve({ action: "cancel" });
  }
  session.pendingElicitations.clear();
  await Promise.all(
    [
      session.initializationTask,
      session.queryConsumerTask,
      ...mcpAuthMonitors.map((monitor) => monitor.task),
    ].filter((task) => task !== undefined),
  );
}

export async function closeSessionWithLogging(
  session: SessionState,
  options: CloseSessionOptions = {},
): Promise<void> {
  await closeSession(session);
  bridgeLogger.info({
    target: LOG_TARGETS.APP_SESSION,
    eventName: "session_closed",
    message: "session closed",
    outcome: "success",
    sessionId: session.sessionId,
    ...(options.requestId ? { requestId: options.requestId } : {}),
    fields: { reason: options.reason ?? "unspecified" },
  });
}

export async function closeSessionsBeforeRegister(
  replacementSession: SessionState,
  staleSessions: SessionState[] | undefined,
  requestId?: string,
): Promise<void> {
  if (!staleSessions || staleSessions.length === 0) {
    return;
  }

  for (const stale of staleSessions) {
    if (stale === replacementSession) {
      continue;
    }
    if (sessions.get(stale.sessionId) === stale) {
      sessions.delete(stale.sessionId);
    }
    await closeSessionWithLogging(stale, {
      reason: "stale_before_register",
      requestId,
    });
  }
}

export async function closeAllSessions(options: CloseSessionOptions = {}): Promise<void> {
  const active = Array.from(sessions.values());
  sessions.clear();
  await Promise.all(
    active.map((session) =>
      closeSessionWithLogging(session, {
        reason: options.reason ?? "bulk_close",
        requestId: options.requestId,
      }),
    ),
  );
  await waitForPendingSessionCloseTasks();
  bridgeLogger.info({
    target: LOG_TARGETS.APP_SESSION,
    eventName: "all_sessions_closed",
    message: "all sessions closed",
    outcome: "success",
    ...(options.requestId ? { requestId: options.requestId } : {}),
    count: active.length,
    fields: { reason: options.reason ?? "bulk_close" },
  });
}

export async function createSession(params: {
  cwd: string;
  resume?: string;
  resumeSessionAt?: string;
  launchSettings: SessionLaunchSettings;
  connectEvent: ConnectEventKind;
  requestId?: string;
  sessionsToCloseBeforeRegister?: SessionState[];
  sessionsToCloseAfterConnect?: SessionState[];
  resumeUpdates?: SessionUpdate[];
  restoredInput?: string;
  pendingRewindResult?: PendingRewindResult;
}): Promise<SessionState> {
  const input = new AsyncQueue<SDKUserMessage>();
  const provisionalSessionId = params.resume ?? randomUUID();
  const initialModel = initialSessionModel(params.launchSettings);
  const initialMode = initialSessionMode(params.launchSettings);
  const supportsBypassPermissionsMode =
    startupPermissionModeOptions(params.launchSettings).allowDangerouslySkipPermissions === true;
  const historyUpdateCount = params.resumeUpdates?.length ?? 0;
  const staleSessionCount = params.sessionsToCloseAfterConnect?.length ?? 0;
  const staleSessionBeforeRegisterCount = params.sessionsToCloseBeforeRegister?.length ?? 0;

  let session!: SessionState;
  const sessionIdForLogs = () => session?.sessionId ?? provisionalSessionId;
  const canUseTool: CanUseTool = async (toolName, inputData, options) => {
    const toolUseId = options.toolUseID;
    if (isToolSearchToolName(toolName)) {
      session?.hiddenToolUseIds.add(toolUseId);
      return { behavior: "allow", updatedInput: inputData, toolUseID: toolUseId };
    }
    if (toolName === EXIT_PLAN_MODE_TOOL_NAME) {
      const existing = ensureToolCallVisible(session, toolUseId, toolName, inputData);
      return await requestExitPlanModeApproval(session, toolUseId, inputData, existing);
    }
    const existing = ensureToolCallVisible(session, toolUseId, toolName, inputData);

    if (toolName === ASK_USER_QUESTION_TOOL_NAME) {
      return await requestAskUserQuestionAnswers(
        session,
        toolUseId,
        inputData,
        existing,
      );
    }

    const display = permissionDisplayFromCanUseOptions(options);
    const request: PermissionRequest = {
      tool_call: existing,
      options: permissionOptionsFromSuggestions(options.suggestions),
      ...(display ? { display } : {}),
    };
    bridgeLogger.info({
      target: LOG_TARGETS.BRIDGE_PERMISSION,
      eventName: "permission_request_created",
      message: "permission request created",
      outcome: "start",
      sessionId: session.sessionId,
      toolCallId: toolUseId,
      count: request.options.length,
      fields: {
        tool_name: toolName,
        agent_id: options.agentID,
        blocked_path: options.blockedPath ?? "<none>",
        decision_reason: options.decisionReason ?? "<none>",
      },
    });
    emitPermissionRequestEvent(session.sessionId, request);

    return await new Promise<PermissionResult>((resolve) => {
      session.pendingPermissions.set(toolUseId, {
        resolve,
        toolName,
        inputData: inputData,
        suggestions: options.suggestions,
      });
    });
  };

  const claudeCodeExecutable = process.env.CLAUDE_CODE_EXECUTABLE;
  const sdkDebugFile = process.env.CLAUDE_RS_SDK_DEBUG_FILE;
  const enableSdkDebug = process.env.CLAUDE_RS_SDK_DEBUG === "1" || Boolean(sdkDebugFile);
  const enableSpawnDebug = process.env.CLAUDE_RS_SDK_SPAWN_DEBUG === "1";
  if (claudeCodeExecutable && !fs.existsSync(claudeCodeExecutable)) {
    throw new Error(`CLAUDE_CODE_EXECUTABLE does not exist: ${claudeCodeExecutable}`);
  }

  let queryHandle: Query;
  bridgeLogger.info({
    target: LOG_TARGETS.APP_SESSION,
    eventName: "session_create_started",
    message: "session creation started",
    outcome: "start",
    ...(params.requestId ? { requestId: params.requestId } : {}),
    sessionId: provisionalSessionId,
    fields: {
      cwd: params.cwd,
      connect_event: params.connectEvent,
      resume_requested: params.resume !== undefined,
      resume_session_at: params.resumeSessionAt ?? "<none>",
      history_update_count: historyUpdateCount,
      stale_session_count: staleSessionCount,
      stale_session_before_register_count: staleSessionBeforeRegisterCount,
    },
  });
  try {
    queryHandle = query({
      prompt: input,
      options: buildQueryOptions({
        cwd: params.cwd,
        resume: params.resume,
        resumeSessionAt: params.resumeSessionAt,
        launchSettings: params.launchSettings,
        provisionalSessionId,
        input,
        canUseTool,
        claudeCodeExecutable,
        sdkDebugFile,
        enableSdkDebug,
        enableSpawnDebug,
        sessionIdForLogs,
      }),
    });
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    bridgeLogger.error({
      target: LOG_TARGETS.APP_SESSION,
      eventName: "session_query_failed",
      message: "session query creation failed",
      outcome: "failure",
      ...(params.requestId ? { requestId: params.requestId } : {}),
      sessionId: provisionalSessionId,
      fields: {
        cwd: params.cwd,
        resume_requested: params.resume !== undefined,
        resume_session_at: params.resumeSessionAt ?? "<none>",
        error_message: message,
      },
    });
    throw new Error(
      `query() failed: runtime_executable=${process.execPath}; cwd=${params.cwd}; ` +
        `resume=${params.resume ?? "<none>"}; ` +
        `CLAUDE_CODE_EXECUTABLE=${claudeCodeExecutable ?? "<unset>"}; error=${message}`,
    );
  }

  session = {
    sessionId: provisionalSessionId,
    cwd: params.cwd,
    model: initialModel,
    ...(initialModel ? { requestedModelId: initialModel } : {}),
    availableModels: [],
    mode: initialMode,
    supportedModeIds: [],
    runtimeUnavailableModeIds: [],
    supportsBypassPermissionsMode,
    fastModeState: "off",
    query: queryHandle,
    input,
    connected: false,
    closing: false,
    connectEvent: params.connectEvent,
    connectRequestId: params.requestId,
    toolCalls: new Map<string, ToolCall>(),
    tasksById: new Map<string, TaskItem>(),
    taskOrder: [],
    taskToolUseIds: new Map<string, string>(),
    taskIdsByToolUseId: new Map<string, string>(),
    pendingPermissions: new Map<string, PendingPermission>(),
    pendingQuestions: new Map<string, PendingQuestion>(),
    pendingUserDialogs: new Map<string, PendingUserDialog>(),
    pendingElicitations: new Map<string, PendingElicitation>(),
    informationalDedupKeys: new Set<string>(),
    knownConnectedMcpServers: new Set<string>(),
    mcpStatusRevalidatedAt: new Map<string, number>(),
    mcpAuthMonitors: new Map<string, McpAuthMonitorHandle>(),
    hiddenToolUseIds: new Set<string>(),
    authHintSent: false,
    ...(params.resumeUpdates && params.resumeUpdates.length > 0
      ? { resumeUpdates: params.resumeUpdates }
      : {}),
    ...(params.restoredInput !== undefined ? { restoredInput: params.restoredInput } : {}),
    ...(params.pendingRewindResult !== undefined
      ? { pendingRewindResult: params.pendingRewindResult }
      : {}),
    ...(params.sessionsToCloseAfterConnect
      ? { sessionsToCloseAfterConnect: params.sessionsToCloseAfterConnect }
      : {}),
  };
  refreshCurrentModel(session);
  const { refreshSupportedModesForSession } = await import("./commands.js");
  refreshSupportedModesForSession(session);
  await closeSessionsBeforeRegister(session, params.sessionsToCloseBeforeRegister, params.requestId);
  sessions.set(provisionalSessionId, session);
  bridgeLogger.info({
    target: LOG_TARGETS.APP_SESSION,
    eventName: "session_query_started",
    message: "session query started",
    outcome: "success",
    ...(params.requestId ? { requestId: params.requestId } : {}),
    sessionId: session.sessionId,
    fields: {
      cwd: session.cwd,
      connect_event: session.connectEvent,
      resume_requested: params.resume !== undefined,
      resume_session_at: params.resumeSessionAt ?? "<none>",
    },
  });
  bridgeLogger.info({
    target: LOG_TARGETS.APP_SESSION,
    eventName: "session_create_registered",
    message: "session registered in bridge state",
    outcome: "success",
    ...(params.requestId ? { requestId: params.requestId } : {}),
    sessionId: session.sessionId,
    count: sessions.size,
    fields: {
      active_session_count: sessions.size,
      connect_event: session.connectEvent,
    },
  });

  // In stream-input mode the SDK may defer init until input arrives.
  // Trigger initialization explicitly so the Rust UI can receive `connected`
  // before the first user prompt.
  session.initializationTask = session.query
    .initializationResult()
    .then(async (result) => {
      bridgeLogger.info({
        target: LOG_TARGETS.APP_SESSION,
        eventName: "session_initialization_completed",
        message: "session initialization completed",
        outcome: "success",
        ...(session.connectRequestId ? { requestId: session.connectRequestId } : {}),
        sessionId: session.sessionId,
        fields: {
          available_model_count: Array.isArray(result.models) ? result.models.length : 0,
          connect_event: session.connectEvent,
          history_update_count: session.resumeUpdates?.length ?? 0,
        },
      });
      session.availableModels = mapAvailableModels(result.models);
      const currentModelChanged = refreshCurrentModel(session);
      const { buildModeState, refreshSupportedModesForSession } = await import("./commands.js");
      refreshSupportedModesForSession(session);
      const fastModeChanged = setFastModeStateIfChanged(session, result.fast_mode_state);
      if (!session.connected) {
        emitConnectEvent(session);
      } else {
        if (currentModelChanged) {
          emitCurrentModelUpdate(session);
        }
        if (session.mode) {
          emitSessionUpdate(session.sessionId, {
            type: "mode_state_update",
            mode: buildModeState(session, session.mode),
          });
        }
        if (fastModeChanged) {
          emitFastModeUpdate(session);
        }
      }
      // Proactively detect missing auth from account info so the UI can
      // show the login hint immediately, without waiting for the first prompt.
      if (shouldEmitStartupAuthRequiredForAccount(result.account)) {
        emitAuthRequired(session);
      }
      updateAvailableCommands(
        session,
        "session_result_commands",
        mapSdkSlashCommands(result.commands),
      );
      emitAvailableAgentsIfChanged(session, mapAvailableAgents(result.agents));
      refreshAvailableAgents(session);
    })
    .catch((error) => {
      if (session.connected) {
        return;
      }
      const message = error instanceof Error ? error.message : String(error);
      bridgeLogger.error({
        target: LOG_TARGETS.APP_SESSION,
        eventName: "session_initialization_failed",
        message: "session initialization failed before connect",
        outcome: "failure",
        ...(session.connectRequestId ? { requestId: session.connectRequestId } : {}),
        sessionId: session.sessionId,
        fields: { error_message: message },
      });
      failConnection(`agent initialization failed: ${message}`, session.connectRequestId);
      session.connectRequestId = undefined;
    });

  session.queryConsumerTask = (async () => {
    try {
      for await (const message of session.query) {
        // Lazy import to break circular dependency at module-evaluation time.
        const { handleSdkMessage } = await import("./message_handlers.js");
        handleSdkMessage(session, message);
      }
      {
        // Lazy import to break circular dependency at module-evaluation time.
        const { flushPendingWorkerShutdown } = await import("./message_handlers.js");
        flushPendingWorkerShutdown(session);
      }
      if (!session.connected) {
        bridgeLogger.error({
          target: LOG_TARGETS.APP_SESSION,
          eventName: "session_stream_ended_before_connect",
          message: "session stream ended before connect",
          outcome: "failure",
          ...(params.requestId ? { requestId: params.requestId } : {}),
          sessionId: session.sessionId,
        });
        failConnection("agent stream ended before session initialization", params.requestId);
      }
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      bridgeLogger.error({
        target: LOG_TARGETS.APP_SESSION,
        eventName: "session_stream_failed_before_connect",
        message: "session stream failed before connect",
        outcome: "failure",
        ...(params.requestId ? { requestId: params.requestId } : {}),
        sessionId: session.sessionId,
        fields: { error_message: message },
      });
      failConnection(`agent stream failed: ${message}`, params.requestId);
    }
  })();

  return session;
}

type QueryOptionsBuilderParams = {
  cwd: string;
  resume?: string;
  resumeSessionAt?: string;
  launchSettings: SessionLaunchSettings;
  provisionalSessionId: string;
  input: AsyncQueue<SDKUserMessage>;
  canUseTool: CanUseTool;
  claudeCodeExecutable?: string;
  sdkDebugFile?: string;
  enableSdkDebug: boolean;
  enableSpawnDebug: boolean;
  sessionIdForLogs: () => string;
};

function logSdkProcessSpawnStarted(
  options: {
    command: string;
    args: string[];
    cwd?: string;
  },
  includeArgsPreview: boolean,
): void {
  bridgeLogger.info({
    target: LOG_TARGETS.BRIDGE_SDK,
    eventName: "sdk_spawn_started",
    message: "spawning Claude Code process",
    outcome: "start",
    fields: {
      command: options.command,
      cwd: options.cwd ?? "<none>",
      arg_count: options.args.length,
      ...(includeArgsPreview ? { args_preview: options.args.slice(0, 5) } : {}),
    },
  });
}

export function resolveClaudeCodeSpawnCommand(command: string): string {
  return command === "bun" ? process.execPath : command;
}

function logSdkProcessSpawned(
  sessionId: string | undefined,
  child: ReturnType<typeof spawnChild>,
  cwd: string | undefined,
): void {
  bridgeLogger.info({
    target: LOG_TARGETS.BRIDGE_SDK,
    eventName: "sdk_spawned",
    message: "Claude Code process spawned",
    outcome: "success",
    ...(sessionId ? { sessionId } : {}),
    fields: {
      cwd: cwd ?? "<none>",
      pid: child.pid ?? "<none>",
    },
  });
}

function logSdkProcessExit(
  sessionId: string | undefined,
  code: number | null,
  signal: NodeJS.Signals | null,
): void {
  const exitedCleanly = code === 0 && signal === null;
  const logger = exitedCleanly ? bridgeLogger.info : bridgeLogger.warn;
  logger({
    target: LOG_TARGETS.BRIDGE_SDK,
    eventName: "sdk_process_exited",
    message: "Claude Code process exited",
    outcome: exitedCleanly ? "success" : "failure",
    ...(sessionId ? { sessionId } : {}),
    fields: {
      exit_code: code ?? "<none>",
      exit_signal: signal ?? "<none>",
    },
  });
}

function permissionModeFromSettingsValue(rawMode: unknown): PermissionMode | undefined {
  if (typeof rawMode !== "string") {
    return undefined;
  }
  switch (rawMode) {
    case "manual":
      return "default";
    case "default":
    case "auto":
    case "acceptEdits":
    case "bypassPermissions":
    case "plan":
    case "dontAsk":
      return rawMode;
    default:
      throw new Error(`unsupported launch_settings.settings.permissions.defaultMode: ${rawMode}`);
  }
}

function initialSessionModel(launchSettings: SessionLaunchSettings): string {
  const settings = settingsObjectFromLaunchSettings(launchSettings);
  const model = typeof settings?.model === "string" ? settings.model.trim() : "";
  return model || STARTUP_FALLBACK_MODEL_ALIAS;
}

function startupModelOption(
  launchSettings: SessionLaunchSettings,
): {
  model?: string;
} {
  const settings = settingsObjectFromLaunchSettings(launchSettings);
  const model = typeof settings?.model === "string" ? settings.model.trim() : "";
  return model ? { model } : {};
}

function initialSessionMode(launchSettings: SessionLaunchSettings): PermissionMode {
  const settings = settingsObjectFromLaunchSettings(launchSettings);
  const permissions =
    settings?.permissions && typeof settings.permissions === "object" && !Array.isArray(settings.permissions)
      ? (settings.permissions as Record<string, unknown>)
      : undefined;
  return permissionModeFromSettingsValue(permissions?.defaultMode) ?? DEFAULT_PERMISSION_MODE;
}

function startupPermissionModeOptions(
  launchSettings: SessionLaunchSettings,
): {
  permissionMode?: PermissionMode;
  allowDangerouslySkipPermissions?: boolean;
} {
  const settings = settingsObjectFromLaunchSettings(launchSettings);
  const permissions =
    settings?.permissions && typeof settings.permissions === "object" && !Array.isArray(settings.permissions)
      ? (settings.permissions as Record<string, unknown>)
      : undefined;
  const permissionMode = permissionModeFromSettingsValue(permissions?.defaultMode);
  if (!permissionMode) {
    return {};
  }
  return permissionMode === "bypassPermissions"
    ? {
        permissionMode,
        allowDangerouslySkipPermissions: true,
      }
    : { permissionMode };
}

function systemPromptFromLaunchSettings(
  launchSettings: SessionLaunchSettings,
): NonNullable<Options["systemPrompt"]> {
  const language = launchSettings.language?.trim();
  const appendLines = [BRIDGE_RUNTIME_GUARD_PROMPT];

  if (language) {
    appendLines.push(
      `Always respond to the user in ${language} unless the user explicitly asks for a different language. ` +
        `Keep code, shell commands, file paths, API names, tool names, and raw error text unchanged unless the user explicitly asks for translation.`,
    );
  }

  return {
    type: "preset",
    preset: "claude_code",
    append: appendLines.join(" "),
  };
}

export function buildQueryOptions(params: QueryOptionsBuilderParams) {
  const systemPrompt = systemPromptFromLaunchSettings(params.launchSettings);
  const modelOption = startupModelOption(params.launchSettings);
  const permissionModeOptions = startupPermissionModeOptions(params.launchSettings);
  const shouldPassCanUseTool = permissionModeOptions.permissionMode !== "bypassPermissions";
  const settings = normalizedSettingsFromLaunchSettings(params.launchSettings);
  return {
    cwd: params.cwd,
    includePartialMessages: true,
    promptSuggestions: true,
    enableFileCheckpointing: true,
    // ProposeSkills reports only a proposal count and expects a native review
    // surface. Keep it out of this host until claude-rs can display the actual
    // proposal input and accept/reject it without losing content.
    disallowedTools: ["ProposeSkills"],
    executable: "bun" as const,
    ...(params.resume ? {} : { sessionId: params.provisionalSessionId }),
    settings,
    ...modelOption,
    ...permissionModeOptions,
    toolConfig: { askUserQuestion: { previewFormat: "markdown" as const } },
    systemPrompt,
    ...(params.launchSettings.agent_progress_summaries !== undefined
      ? { agentProgressSummaries: params.launchSettings.agent_progress_summaries }
      : {}),
    ...(params.claudeCodeExecutable
      ? { pathToClaudeCodeExecutable: params.claudeCodeExecutable }
      : {}),
    ...(params.enableSdkDebug ? { debug: true } : {}),
    ...(params.sdkDebugFile ? { debugFile: params.sdkDebugFile } : {}),
    stderr: (line: string) => {
      if (line.trim().length > 0) {
        logSdkStderrLine(line);
      }
    },
    spawnClaudeCodeProcess: (options: {
      command: string;
      args: string[];
      cwd?: string;
      env: Record<string, string | undefined>;
      signal: AbortSignal;
    }) => {
      const command = resolveClaudeCodeSpawnCommand(options.command);
      const spawnOptions = { ...options, command };
      logSdkProcessSpawnStarted(spawnOptions, params.enableSpawnDebug);
      const child = spawnChild(command, options.args, {
        cwd: options.cwd,
        env: options.env,
        signal: options.signal,
        stdio: ["pipe", "pipe", "pipe"],
        windowsHide: true,
      });
      logSdkProcessSpawned(params.sessionIdForLogs() || undefined, child, options.cwd);
      child.on("error", (error) => {
        const sessionId = params.sessionIdForLogs();
        bridgeLogger.error({
          target: LOG_TARGETS.BRIDGE_SDK,
          eventName: "sdk_spawn_failed",
          message: "Claude Code process spawn failed",
          outcome: "failure",
          ...(sessionId ? { sessionId } : {}),
          errorCode: (error as NodeJS.ErrnoException).code ?? "<none>",
          fields: { error_message: error.message },
        });
      });
      child.on("exit", (code, signal) => {
        logSdkProcessExit(params.sessionIdForLogs() || undefined, code, signal);
      });
      return child;
    },
    // Match the Claude Code CLI defaults to avoid emitting an empty
    // --setting-sources argument.
    settingSources: DEFAULT_SETTING_SOURCES,
    resume: params.resume,
    ...(params.resumeSessionAt ? { resumeSessionAt: params.resumeSessionAt } : {}),
    ...(shouldPassCanUseTool ? { canUseTool: params.canUseTool } : {}),
    onElicitation: async (request: {
      mode?: string;
      serverName?: string;
      message?: string;
      url?: string;
      elicitationId?: string;
      requestedSchema?: Record<string, unknown>;
    }) => {
      const requestId = randomUUID();
      const mode =
        request.mode === "form" || request.mode === "url"
          ? request.mode
          : typeof request.url === "string" && request.url.trim().length > 0
            ? "url"
            : "form";
      const normalized: ElicitationRequest = {
        request_id: requestId,
        server_name:
          typeof request.serverName === "string" && request.serverName.trim().length > 0
            ? request.serverName
            : "unknown",
        message:
          typeof request.message === "string" && request.message.trim().length > 0
            ? request.message
            : "<no message>",
        mode,
        ...(typeof request.url === "string" && request.url.trim().length > 0
          ? { url: request.url }
          : {}),
        ...(typeof request.elicitationId === "string" && request.elicitationId.trim().length > 0
          ? { elicitation_id: request.elicitationId }
          : {}),
        ...(request.requestedSchema
          ? { requested_schema: request.requestedSchema as Record<string, Json> }
          : {}),
      };
      bridgeLogger.info({
        target: LOG_TARGETS.BRIDGE_PERMISSION,
        eventName: "elicitation_request_created",
        message: "elicitation request created",
        outcome: "start",
        sessionId: params.sessionIdForLogs(),
        requestId,
        fields: {
          server_name: normalized.server_name,
          mode: normalized.mode,
          has_url: normalized.url !== undefined,
        },
      });
      emitElicitationRequestEvent(params.sessionIdForLogs(), normalized);
      return await new Promise<{
        action: ElicitationAction;
        content?: Record<string, string | number | boolean | string[]>;
      }>((resolve) => {
        const currentSession = sessions.get(params.sessionIdForLogs());
        if (!currentSession) {
          bridgeLogger.warn({
            target: LOG_TARGETS.BRIDGE_PERMISSION,
            eventName: "elicitation_request_dropped",
            message: "elicitation request dropped without an active session",
            outcome: "dropped",
            sessionId: params.sessionIdForLogs(),
            requestId,
            fields: { reason: "unknown_session" },
          });
          resolve({ action: "cancel" });
          return;
        }
        currentSession.pendingElicitations.set(requestId, {
          resolve,
          serverName: normalized.server_name,
          elicitationId: normalized.elicitation_id,
        });
      });
    },
    // The SDK "fails closed" and never emits a dialog kind unless it is declared
    // here. We declare the one refusal-related kind we render: when the API
    // returns a hard `stop_reason: "refusal"` and a fallback model is configured,
    // the CLI emits `request_user_dialog` and the host renders the chooser below.
    supportedDialogKinds: [REFUSAL_FALLBACK_DIALOG_KIND],
    // Host policy for `request_user_dialog` control requests. Unknown kinds are
    // logged and answered `{ behavior: "cancelled" }` (the spec-required answer
    // for unrecognized kinds). For `refusal_fallback_prompt`, we surface an
    // interactive chooser in the TUI and route the user's decision back to the
    // CLI as `{ behavior: "completed", result: <choice> }` (or cancelled on
    // decline/abort/teardown).
    onUserDialog: async (
      request: UserDialogRequest,
      options: { signal: AbortSignal },
    ): Promise<UserDialogResult> => {
      if (request.dialogKind !== REFUSAL_FALLBACK_DIALOG_KIND) {
        bridgeLogger.warn({
          target: LOG_TARGETS.APP_SESSION,
          eventName: "user_dialog_received",
          message: "request_user_dialog received for unknown kind; cancelled",
          outcome: "cancelled",
          sessionId: params.sessionIdForLogs(),
          ...(typeof request.toolUseID === "string" ? { toolCallId: request.toolUseID } : {}),
          fields: { dialog_kind: request.dialogKind },
        });
        return { behavior: "cancelled" };
      }

      const payload = normalizeRefusalFallbackPayload(request.payload);
      const requestId = randomUUID();
      const dialogRequest = {
        request_id: requestId,
        dialog_kind: REFUSAL_FALLBACK_DIALOG_KIND as typeof REFUSAL_FALLBACK_DIALOG_KIND,
        payload,
        options: buildRefusalFallbackOptions(payload),
      };
      bridgeLogger.info({
        target: LOG_TARGETS.APP_SESSION,
        eventName: "user_dialog_received",
        message: "request_user_dialog received; awaiting host decision",
        outcome: "start",
        sessionId: params.sessionIdForLogs(),
        requestId,
        ...(typeof request.toolUseID === "string" ? { toolCallId: request.toolUseID } : {}),
        fields: {
          dialog_kind: request.dialogKind,
          original_model: payload.original_model,
          fallback_model: payload.fallback_model,
        },
      });

      const choice = await new Promise<RefusalFallbackPromptChoice | "cancelled">((resolve) => {
        const currentSession = sessions.get(params.sessionIdForLogs());
        if (!currentSession) {
          bridgeLogger.warn({
            target: LOG_TARGETS.APP_SESSION,
            eventName: "user_dialog_request_dropped",
            message: "user dialog request dropped without an active session",
            outcome: "dropped",
            sessionId: params.sessionIdForLogs(),
            requestId,
            fields: { reason: "unknown_session" },
          });
          resolve("cancelled");
          return;
        }
        let settled = false;
        const settle = (value: RefusalFallbackPromptChoice | "cancelled") => {
          if (settled) {
            return;
          }
          settled = true;
          currentSession.pendingUserDialogs.delete(requestId);
          options.signal.removeEventListener("abort", onAbort);
          resolve(value);
        };
        const onAbort = () => settle("cancelled");
        if (options.signal.aborted) {
          settle("cancelled");
          return;
        }
        options.signal.addEventListener("abort", onAbort);
        currentSession.pendingUserDialogs.set(requestId, { resolve: settle });
        emitUserDialogRequestEvent(params.sessionIdForLogs(), dialogRequest);
      });

      return choice === "cancelled"
        ? { behavior: "cancelled" }
        : { behavior: "completed", result: choice };
    },
  };
}

export function handlePermissionResponse(command: Extract<BridgeCommand, { command: "permission_response" }>): void {
  bridgeLogger.info({
    target: LOG_TARGETS.BRIDGE_PERMISSION,
    eventName: "permission_response_received",
    message: "permission response received",
    outcome: "success",
    sessionId: command.session_id,
    toolCallId: command.tool_call_id,
    fields: {
      response_kind: command.outcome.outcome,
      selected_option:
        command.outcome.outcome === "selected" ? command.outcome.option_id : "cancelled",
    },
  });
  const session = sessionById(command.session_id);
  if (!session) {
    bridgeLogger.warn({
      target: LOG_TARGETS.BRIDGE_PERMISSION,
      eventName: "permission_response_dropped",
      message: "permission response dropped for unknown session",
      outcome: "dropped",
      sessionId: command.session_id,
      toolCallId: command.tool_call_id,
      fields: { reason: "unknown_session" },
    });
    return;
  }
  const resolver = session.pendingPermissions.get(command.tool_call_id);
  if (!resolver) {
    bridgeLogger.warn({
      target: LOG_TARGETS.BRIDGE_PERMISSION,
      eventName: "permission_response_dropped",
      message: "permission response dropped without a pending resolver",
      outcome: "dropped",
      sessionId: command.session_id,
      toolCallId: command.tool_call_id,
      fields: { reason: "missing_pending_resolver" },
    });
    return;
  }
  session.pendingPermissions.delete(command.tool_call_id);

  const outcome = command.outcome as PermissionOutcome;
  if (resolver.onOutcome) {
    bridgeLogger.info({
      target: LOG_TARGETS.BRIDGE_PERMISSION,
      eventName: "permission_response_applied",
      message: "permission response applied to outcome callback",
      outcome: "success",
      sessionId: command.session_id,
      toolCallId: command.tool_call_id,
      fields: {
        tool_name: resolver.toolName,
        response_kind: outcome.outcome,
        selected_option: outcome.outcome === "selected" ? outcome.option_id : "cancelled",
      },
    });
    resolver.onOutcome(outcome);
    return;
  }
  if (!resolver.resolve) {
    bridgeLogger.warn({
      target: LOG_TARGETS.BRIDGE_PERMISSION,
      eventName: "permission_response_dropped",
      message: "permission response dropped because resolver callback was missing",
      outcome: "dropped",
      sessionId: command.session_id,
      toolCallId: command.tool_call_id,
      fields: { reason: "missing_resolver_callback" },
    });
    return;
  }
  const selectedOption = outcome.outcome === "selected" ? outcome.option_id : "cancelled";
  if (
    outcome.outcome === "selected" &&
    (outcome.option_id === "allow_once" ||
      outcome.option_id === "allow_session" ||
      outcome.option_id === "allow_always")
  ) {
    setToolCallStatus(session, command.tool_call_id, "in_progress");
  } else if (outcome.outcome === "selected") {
    setToolCallStatus(session, command.tool_call_id, "failed", "Permission denied");
  } else {
    setToolCallStatus(session, command.tool_call_id, "failed", "Permission cancelled");
  }

  const permissionResult = permissionResultFromOutcome(
    outcome,
    command.tool_call_id,
    resolver.inputData,
    resolver.suggestions,
    resolver.toolName,
  );
  bridgeLogger.info({
    target: LOG_TARGETS.BRIDGE_PERMISSION,
    eventName: "permission_response_applied",
    message: "permission response applied",
    outcome: "success",
    sessionId: command.session_id,
    toolCallId: command.tool_call_id,
    fields: {
      tool_name: resolver.toolName,
      response_kind: outcome.outcome,
      selected_option: selectedOption,
      behavior: permissionResult.behavior,
    },
  });
  resolver.resolve(permissionResult);
}

export function handleQuestionResponse(command: Extract<BridgeCommand, { command: "question_response" }>): void {
  bridgeLogger.info({
    target: LOG_TARGETS.BRIDGE_PERMISSION,
    eventName: "question_response_received",
    message: "question response received",
    outcome: "success",
    sessionId: command.session_id,
    toolCallId: command.tool_call_id,
    fields: { response_kind: command.outcome.outcome },
  });
  const session = sessionById(command.session_id);
  if (!session) {
    bridgeLogger.warn({
      target: LOG_TARGETS.BRIDGE_PERMISSION,
      eventName: "question_response_dropped",
      message: "question response dropped for unknown session",
      outcome: "dropped",
      sessionId: command.session_id,
      toolCallId: command.tool_call_id,
      fields: { reason: "unknown_session" },
    });
    return;
  }
  const resolver = session.pendingQuestions.get(command.tool_call_id);
  if (!resolver) {
    bridgeLogger.warn({
      target: LOG_TARGETS.BRIDGE_PERMISSION,
      eventName: "question_response_dropped",
      message: "question response dropped without a pending resolver",
      outcome: "dropped",
      sessionId: command.session_id,
      toolCallId: command.tool_call_id,
      fields: { reason: "missing_pending_resolver" },
    });
    return;
  }
  session.pendingQuestions.delete(command.tool_call_id);
  bridgeLogger.info({
    target: LOG_TARGETS.BRIDGE_PERMISSION,
    eventName: "question_response_applied",
    message: "question response applied",
    outcome: "success",
    sessionId: command.session_id,
    toolCallId: command.tool_call_id,
    fields: {
      tool_name: resolver.toolName,
      response_kind: command.outcome.outcome,
      selected_option_count:
        command.outcome.outcome === "answered" ? command.outcome.selected_option_ids.length : 0,
      has_annotation:
        command.outcome.outcome === "answered" && command.outcome.annotation !== undefined,
    },
  });
  resolver.onOutcome(command.outcome);
}

export function handleUserDialogResponse(
  command: Extract<BridgeCommand, { command: "user_dialog_response" }>,
): void {
  bridgeLogger.info({
    target: LOG_TARGETS.BRIDGE_PERMISSION,
    eventName: "user_dialog_response_received",
    message: "user dialog response received",
    outcome: "success",
    sessionId: command.session_id,
    requestId: command.request_id,
    fields: {
      response_kind: command.outcome.outcome,
      selected_option:
        command.outcome.outcome === "selected" ? command.outcome.option_id : "cancelled",
    },
  });
  const session = sessionById(command.session_id);
  if (!session) {
    bridgeLogger.warn({
      target: LOG_TARGETS.BRIDGE_PERMISSION,
      eventName: "user_dialog_response_dropped",
      message: "user dialog response dropped for unknown session",
      outcome: "dropped",
      sessionId: command.session_id,
      requestId: command.request_id,
      fields: { reason: "unknown_session" },
    });
    return;
  }
  const pending = session.pendingUserDialogs.get(command.request_id);
  if (!pending) {
    // Idempotent: a late or duplicate response for an already-resolved request
    // (e.g. the dialog was cancelled on abort/teardown first) is a no-op.
    bridgeLogger.warn({
      target: LOG_TARGETS.BRIDGE_PERMISSION,
      eventName: "user_dialog_response_dropped",
      message: "user dialog response dropped without a pending request",
      outcome: "dropped",
      sessionId: command.session_id,
      requestId: command.request_id,
      fields: { reason: "missing_pending_request" },
    });
    return;
  }
  session.pendingUserDialogs.delete(command.request_id);
  const choice = command.outcome.outcome === "selected" ? command.outcome.option_id : "cancelled";
  bridgeLogger.info({
    target: LOG_TARGETS.BRIDGE_PERMISSION,
    eventName: "user_dialog_response_applied",
    message: "user dialog response applied",
    outcome: "success",
    sessionId: command.session_id,
    requestId: command.request_id,
    fields: { choice },
  });
  pending.resolve(choice);
}

export function handleElicitationResponse(
  command: Extract<BridgeCommand, { command: "elicitation_response" }>,
): void {
  bridgeLogger.info({
    target: LOG_TARGETS.BRIDGE_PERMISSION,
    eventName: "elicitation_response_received",
    message: "elicitation response received",
    outcome: "success",
    sessionId: command.session_id,
    requestId: command.elicitation_request_id,
    fields: {
      action: command.action,
      has_content: command.content !== undefined,
    },
  });
  const session = sessionById(command.session_id);
  if (!session) {
    bridgeLogger.warn({
      target: LOG_TARGETS.BRIDGE_PERMISSION,
      eventName: "elicitation_response_dropped",
      message: "elicitation response dropped for unknown session",
      outcome: "dropped",
      sessionId: command.session_id,
      requestId: command.elicitation_request_id,
      fields: { reason: "unknown_session" },
    });
    return;
  }
  const pending = session.pendingElicitations.get(command.elicitation_request_id);
  if (!pending) {
    bridgeLogger.warn({
      target: LOG_TARGETS.BRIDGE_PERMISSION,
      eventName: "elicitation_response_dropped",
      message: "elicitation response dropped without pending request",
      outcome: "dropped",
      sessionId: command.session_id,
      requestId: command.elicitation_request_id,
      fields: { reason: "missing_pending_request" },
    });
    return;
  }
  session.pendingElicitations.delete(command.elicitation_request_id);
  bridgeLogger.info({
    target: LOG_TARGETS.BRIDGE_PERMISSION,
    eventName: "elicitation_response_applied",
    message: "elicitation response applied",
    outcome: "success",
    sessionId: command.session_id,
    requestId: command.elicitation_request_id,
    fields: {
      action: command.action,
      server_name: pending.serverName,
      has_content: command.content !== undefined,
    },
  });
  pending.resolve({
    action: command.action,
    ...(normalizeSdkElicitationContent(command.content) ? {
      content: normalizeSdkElicitationContent(command.content),
    } : {}),
  });
}
export function shouldInvalidateResolvedRuntimeModel(
  previousRequestedId: string | undefined,
  previousSessionModel: string,
  nextRequestedId: string,
): boolean {
  const previousRequested = previousRequestedId?.trim() || previousSessionModel.trim();
  return previousRequested !== nextRequestedId.trim();
}
export function emitCurrentModelUpdate(session: SessionState): boolean {
  if (!session.connected || !session.currentModel) {
    return false;
  }
  emitSessionUpdate(session.sessionId, {
    type: "current_model_update",
    current_model: session.currentModel,
  });
  return true;
}

export function refreshCurrentModel(session: SessionState, emitUpdate = false): boolean {
  const nextModel = resolveCurrentModel(session);
  if (currentModelsEqual(session.currentModel, nextModel)) {
    return false;
  }
  session.currentModel = nextModel;
  if (emitUpdate) {
    emitCurrentModelUpdate(session);
  }
  return true;
}
