import { randomUUID } from "node:crypto";
import { spawn as spawnChild } from "node:child_process";
import fs from "node:fs";
import {
  getSessionMessages,
  listSessions,
  query,
  type CanUseTool,
  type ModelInfo,
  type PermissionMode,
  type PermissionResult,
  type PermissionUpdate,
  type Query,
  type SDKUserMessage,
  type SettingSource,
} from "@anthropic-ai/claude-agent-sdk";
import type {
  AvailableCommand,
  AvailableModel,
  BridgeCommand,
  ElicitationAction,
  ElicitationRequest,
  FastModeState,
  Json,
  PermissionOutcome,
  PermissionRequest,
  QuestionOutcome,
  SessionLaunchSettings,
  SessionUpdate,
  ToolCall,
} from "../types.js";
import { bridgeLogger, LOG_TARGETS, logSdkStderrLine } from "./logger.js";
import { AsyncQueue } from "./shared.js";
import {
  permissionOptionsFromSuggestions,
  permissionResultFromOutcome,
} from "./permissions.js";
import { mapSessionMessagesToUpdates } from "./history.js";
import {
  writeEvent,
  failConnection,
  slashError,
  emitSessionUpdate,
  emitConnectEvent,
  emitSessionsList,
  refreshSessionsList,
  emitPermissionRequestEvent,
  emitElicitationRequestEvent,
} from "./events.js";
import {
  ensureToolCallVisible,
  setToolCallStatus,
} from "./tool_calls.js";
import {
  requestExitPlanModeApproval,
  requestAskUserQuestionAnswers,
  EXIT_PLAN_MODE_TOOL_NAME,
  ASK_USER_QUESTION_TOOL_NAME,
} from "./user_interaction.js";
import { mapAvailableAgents, emitAvailableAgentsIfChanged, refreshAvailableAgents } from "./agents.js";
import { emitAuthRequired, emitFastModeUpdateIfChanged } from "./error_classification.js";

export type ConnectEventKind = "connected" | "session_replaced";

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

export type PendingElicitation = {
  resolve: (result: {
    action: ElicitationAction;
    content?: Record<string, unknown>;
  }) => void;
  serverName: string;
  elicitationId?: string;
};

export type SessionState = {
  sessionId: string;
  cwd: string;
  model: string;
  availableModels: AvailableModel[];
  mode: PermissionMode | null;
  fastModeState: FastModeState;
  query: Query;
  input: AsyncQueue<SDKUserMessage>;
  connected: boolean;
  connectEvent: ConnectEventKind;
  connectRequestId?: string;
  toolCalls: Map<string, ToolCall>;
  taskToolUseIds: Map<string, string>;
  pendingPermissions: Map<string, PendingPermission>;
  pendingQuestions: Map<string, PendingQuestion>;
  pendingElicitations: Map<string, PendingElicitation>;
  mcpStatusRevalidatedAt: Map<string, number>;
  authHintSent: boolean;
  lastAvailableAgentsSignature?: string;
  lastAssistantError?: string;
  sessionsToCloseAfterConnect?: SessionState[];
  resumeUpdates?: SessionUpdate[];
};

export const sessions = new Map<string, SessionState>();
const DEFAULT_SETTING_SOURCES: SettingSource[] = ["user", "project", "local"];
const DEFAULT_MODEL_NAME = "default";
const DEFAULT_PERMISSION_MODE: PermissionMode = "default";

type CloseSessionOptions = {
  reason?: string;
  requestId?: string;
};

function settingsObjectFromLaunchSettings(
  launchSettings: SessionLaunchSettings,
): Record<string, unknown> | undefined {
  return launchSettings.settings;
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

export async function closeSession(session: SessionState): Promise<void> {
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
  for (const pending of session.pendingElicitations.values()) {
    pending.resolve({ action: "cancel" });
  }
  session.pendingElicitations.clear();
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
  launchSettings: SessionLaunchSettings;
  connectEvent: ConnectEventKind;
  requestId?: string;
  sessionsToCloseAfterConnect?: SessionState[];
  resumeUpdates?: SessionUpdate[];
}): Promise<void> {
  const input = new AsyncQueue<SDKUserMessage>();
  const provisionalSessionId = params.resume ?? randomUUID();
  const initialModel = initialSessionModel(params.launchSettings);
  const initialMode = initialSessionMode(params.launchSettings);
  const historyUpdateCount = params.resumeUpdates?.length ?? 0;
  const staleSessionCount = params.sessionsToCloseAfterConnect?.length ?? 0;

  let session!: SessionState;
  const sessionIdForLogs = () => session?.sessionId ?? provisionalSessionId;
  const canUseTool: CanUseTool = async (toolName, inputData, options) => {
    const toolUseId = options.toolUseID;
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

    const request: PermissionRequest = {
      tool_call: existing,
      options: permissionOptionsFromSuggestions(options.suggestions),
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
      history_update_count: historyUpdateCount,
      stale_session_count: staleSessionCount,
    },
  });
  try {
    queryHandle = query({
      prompt: input,
      options: buildQueryOptions({
        cwd: params.cwd,
        resume: params.resume,
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
        error_message: message,
      },
    });
    throw new Error(
      `query() failed: node_executable=${process.execPath}; cwd=${params.cwd}; ` +
        `resume=${params.resume ?? "<none>"}; ` +
        `CLAUDE_CODE_EXECUTABLE=${claudeCodeExecutable ?? "<unset>"}; error=${message}`,
    );
  }

  session = {
    sessionId: provisionalSessionId,
    cwd: params.cwd,
    model: initialModel,
    availableModels: [],
    mode: initialMode,
    fastModeState: "off",
    query: queryHandle,
    input,
    connected: false,
    connectEvent: params.connectEvent,
    connectRequestId: params.requestId,
    toolCalls: new Map<string, ToolCall>(),
    taskToolUseIds: new Map<string, string>(),
    pendingPermissions: new Map<string, PendingPermission>(),
    pendingQuestions: new Map<string, PendingQuestion>(),
    pendingElicitations: new Map<string, PendingElicitation>(),
    mcpStatusRevalidatedAt: new Map<string, number>(),
    authHintSent: false,
    ...(params.resumeUpdates && params.resumeUpdates.length > 0
      ? { resumeUpdates: params.resumeUpdates }
      : {}),
    ...(params.sessionsToCloseAfterConnect
      ? { sessionsToCloseAfterConnect: params.sessionsToCloseAfterConnect }
      : {}),
  };
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
  void session.query
    .initializationResult()
    .then((result) => {
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
      if (!session.connected) {
        emitConnectEvent(session);
      }
      // Proactively detect missing auth from account info so the UI can
      // show the login hint immediately, without waiting for the first prompt.
      const acct = result.account;
      const hasCredentials =
        (typeof acct.email === "string" && acct.email.trim().length > 0) ||
        (typeof acct.apiKeySource === "string" && acct.apiKeySource.trim().length > 0);
      if (!hasCredentials) {
        emitAuthRequired(session);
      }
      emitFastModeUpdateIfChanged(session, result.fast_mode_state);

      const commands = Array.isArray(result.commands)
        ? result.commands.map((command) => ({
            name: command.name,
            description: command.description ?? "",
            input_hint: command.argumentHint ?? undefined,
          }))
        : [];
      if (commands.length > 0) {
        emitSessionUpdate(session.sessionId, { type: "available_commands_update", commands });
      }
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

  void (async () => {
    try {
      for await (const message of session.query) {
        // Lazy import to break circular dependency at module-evaluation time.
        const { handleSdkMessage } = await import("./message_handlers.js");
        handleSdkMessage(session, message);
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
}

type QueryOptionsBuilderParams = {
  cwd: string;
  resume?: string;
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
    case "default":
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
  return model || DEFAULT_MODEL_NAME;
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
):
  | {
      type: "preset";
      preset: "claude_code";
      append: string;
    }
  | undefined {
  const language = launchSettings.language?.trim();
  if (!language) {
    return undefined;
  }

  return {
    type: "preset",
    preset: "claude_code",
    append:
      `Always respond to the user in ${language} unless the user explicitly asks for a different language. ` +
      `Keep code, shell commands, file paths, API names, tool names, and raw error text unchanged unless the user explicitly asks for translation.`,
  };
}

export function buildQueryOptions(params: QueryOptionsBuilderParams) {
  const systemPrompt = systemPromptFromLaunchSettings(params.launchSettings);
  const modelOption = startupModelOption(params.launchSettings);
  const permissionModeOptions = startupPermissionModeOptions(params.launchSettings);
  return {
    cwd: params.cwd,
    includePartialMessages: true,
    executable: "node" as const,
    ...(params.resume ? {} : { sessionId: params.provisionalSessionId }),
    ...(params.launchSettings.settings ? { settings: params.launchSettings.settings } : {}),
    ...modelOption,
    ...permissionModeOptions,
    toolConfig: { askUserQuestion: { previewFormat: "markdown" as const } },
    ...(systemPrompt ? { systemPrompt } : {}),
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
      logSdkProcessSpawnStarted(options, params.enableSpawnDebug);
      const child = spawnChild(options.command, options.args, {
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
    canUseTool: params.canUseTool,
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
        content?: Record<string, unknown>;
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
  };
}

export function mapAvailableModels(models: ModelInfo[] | undefined): AvailableModel[] {
  if (!Array.isArray(models)) {
    return [];
  }

  return models
    .filter((entry): entry is ModelInfo & { value: string; displayName: string } => {
      return (
        typeof entry?.value === "string" &&
        entry.value.trim().length > 0 &&
        typeof entry.displayName === "string" &&
        entry.displayName.trim().length > 0
      );
    })
    .map((entry) => ({
      id: entry.value,
      display_name: entry.displayName,
      supports_effort: entry.supportsEffort === true,
      supported_effort_levels: Array.isArray(entry.supportedEffortLevels)
        ? entry.supportedEffortLevels.filter(
            (level): level is "low" | "medium" | "high" =>
              level === "low" || level === "medium" || level === "high",
          )
        : [],
      ...(typeof entry.supportsAdaptiveThinking === "boolean"
        ? { supports_adaptive_thinking: entry.supportsAdaptiveThinking }
        : {}),
      ...(typeof entry.supportsFastMode === "boolean"
        ? { supports_fast_mode: entry.supportsFastMode }
        : {}),
      ...(typeof entry.supportsAutoMode === "boolean"
        ? { supports_auto_mode: entry.supportsAutoMode }
        : {}),
      ...(typeof entry.description === "string" && entry.description.trim().length > 0
        ? { description: entry.description }
        : {}),
    }));
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
    ...(command.content ? { content: command.content } : {}),
  });
}
