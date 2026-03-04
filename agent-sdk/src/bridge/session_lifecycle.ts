import { randomUUID } from "node:crypto";
import { spawn as spawnChild } from "node:child_process";
import fs from "node:fs";
import {
  getSessionMessages,
  listSessions,
  query,
  type CanUseTool,
  type PermissionMode,
  type PermissionResult,
  type PermissionUpdate,
  type Query,
  type SDKUserMessage,
} from "@anthropic-ai/claude-agent-sdk";
import type {
  AvailableCommand,
  BridgeCommand,
  FastModeState,
  PermissionOutcome,
  PermissionRequest,
  SessionUpdate,
  ToolCall,
} from "../types.js";
import { AsyncQueue, logPermissionDebug } from "./shared.js";
import { toPermissionMode, buildModeState } from "./commands.js";
import {
  formatPermissionUpdates,
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

export type SessionState = {
  sessionId: string;
  cwd: string;
  model: string;
  mode: PermissionMode;
  fastModeState: FastModeState;
  yolo: boolean;
  query: Query;
  input: AsyncQueue<SDKUserMessage>;
  connected: boolean;
  connectEvent: ConnectEventKind;
  connectRequestId?: string;
  toolCalls: Map<string, ToolCall>;
  taskToolUseIds: Map<string, string>;
  pendingPermissions: Map<string, PendingPermission>;
  authHintSent: boolean;
  lastAvailableAgentsSignature?: string;
  lastAssistantError?: string;
  lastTotalCostUsd?: number;
  sessionsToCloseAfterConnect?: SessionState[];
  resumeUpdates?: SessionUpdate[];
};

export const sessions = new Map<string, SessionState>();

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
}

export async function closeAllSessions(): Promise<void> {
  const active = Array.from(sessions.values());
  sessions.clear();
  await Promise.all(active.map((session) => closeSession(session)));
}

export async function createSession(params: {
  cwd: string;
  yolo: boolean;
  model?: string;
  resume?: string;
  connectEvent: ConnectEventKind;
  requestId?: string;
  sessionsToCloseAfterConnect?: SessionState[];
  resumeUpdates?: SessionUpdate[];
}): Promise<void> {
  const input = new AsyncQueue<SDKUserMessage>();
  const startMode: PermissionMode = params.yolo ? "bypassPermissions" : "default";
  const provisionalSessionId = params.resume ?? randomUUID();

  let session!: SessionState;
  const canUseTool: CanUseTool = async (toolName, inputData, options) => {
    const toolUseId = options.toolUseID;
    if (toolName === EXIT_PLAN_MODE_TOOL_NAME) {
      const existing = ensureToolCallVisible(session, toolUseId, toolName, inputData);
      return await requestExitPlanModeApproval(session, toolUseId, inputData, existing);
    }
    logPermissionDebug(
      `request tool_use_id=${toolUseId} tool=${toolName} blocked_path=${options.blockedPath ?? "<none>"} ` +
        `decision_reason=${options.decisionReason ?? "<none>"} suggestions=${formatPermissionUpdates(options.suggestions)}`,
    );
    const existing = ensureToolCallVisible(session, toolUseId, toolName, inputData);

    if (toolName === ASK_USER_QUESTION_TOOL_NAME) {
      return await requestAskUserQuestionAnswers(
        session,
        toolUseId,
        toolName,
        inputData,
        existing,
      );
    }

    const request: PermissionRequest = {
      tool_call: existing,
      options: permissionOptionsFromSuggestions(options.suggestions),
    };
    writeEvent({ event: "permission_request", session_id: session.sessionId, request });

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
  try {
    queryHandle = query({
      prompt: input,
      options: {
        cwd: params.cwd,
        includePartialMessages: true,
        executable: "node",
        ...(params.resume ? {} : { sessionId: provisionalSessionId }),
        ...(claudeCodeExecutable
          ? { pathToClaudeCodeExecutable: claudeCodeExecutable }
          : {}),
        ...(enableSdkDebug ? { debug: true } : {}),
        ...(sdkDebugFile ? { debugFile: sdkDebugFile } : {}),
        stderr: (line: string) => {
          if (line.trim().length > 0) {
            console.error(`[sdk stderr] ${line}`);
          }
        },
        ...(enableSpawnDebug
          ? {
              spawnClaudeCodeProcess: (options: {
                command: string;
                args: string[];
                cwd?: string;
                env: Record<string, string | undefined>;
                signal: AbortSignal;
              }) => {
                console.error(
                  `[sdk spawn] command=${options.command} args=${JSON.stringify(options.args)} cwd=${options.cwd ?? "<none>"}`,
                );
                const child = spawnChild(options.command, options.args, {
                  cwd: options.cwd,
                  env: options.env,
                  signal: options.signal,
                  stdio: ["pipe", "pipe", "pipe"],
                  windowsHide: true,
                });
                child.on("error", (error) => {
                  console.error(
                    `[sdk spawn error] code=${(error as NodeJS.ErrnoException).code ?? "<none>"} message=${error.message}`,
                  );
                });
                return child;
              },
            }
          : {}),
        // Match claude-agent-acp defaults to avoid emitting an empty
        // --setting-sources argument.
        settingSources: ["user", "project", "local"],
        permissionMode: startMode,
        allowDangerouslySkipPermissions: params.yolo,
        resume: params.resume,
        model: params.model,
        canUseTool,
        onElicitation: async (request) => {
          const requestMode = typeof request.mode === "string" ? request.mode : "unknown";
          const requestServer =
            typeof request.serverName === "string" && request.serverName.trim().length > 0
              ? request.serverName
              : "unknown";
          const requestMessage =
            typeof request.message === "string" && request.message.trim().length > 0
              ? request.message
              : "<no message>";
          console.error(
            `[sdk warn] elicitation unsupported without MCP settings UI; ` +
              `auto-canceling session_id=${session.sessionId} server=${requestServer} ` +
              `mode=${requestMode} message=${JSON.stringify(requestMessage)}`,
          );
          return { action: "cancel" as const };
        },
      },
    });
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    throw new Error(
      `query() failed: node_executable=${process.execPath}; cwd=${params.cwd}; ` +
        `resume=${params.resume ?? "<none>"}; model=${params.model ?? "<none>"}; ` +
        `CLAUDE_CODE_EXECUTABLE=${claudeCodeExecutable ?? "<unset>"}; error=${message}`,
    );
  }

  session = {
    sessionId: provisionalSessionId,
    cwd: params.cwd,
    model: params.model ?? "default",
    mode: startMode,
    fastModeState: "off",
    yolo: params.yolo,
    query: queryHandle,
    input,
    connected: false,
    connectEvent: params.connectEvent,
    connectRequestId: params.requestId,
    toolCalls: new Map<string, ToolCall>(),
    taskToolUseIds: new Map<string, string>(),
    pendingPermissions: new Map<string, PendingPermission>(),
    authHintSent: false,
    ...(params.resumeUpdates && params.resumeUpdates.length > 0
      ? { resumeUpdates: params.resumeUpdates }
      : {}),
    ...(params.sessionsToCloseAfterConnect
      ? { sessionsToCloseAfterConnect: params.sessionsToCloseAfterConnect }
      : {}),
  };
  sessions.set(provisionalSessionId, session);

  // In stream-input mode the SDK may defer init until input arrives.
  // Trigger initialization explicitly so the Rust UI can receive `connected`
  // before the first user prompt.
  void session.query
    .initializationResult()
    .then((result) => {
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
        failConnection("agent stream ended before session initialization", params.requestId);
      }
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      failConnection(`agent stream failed: ${message}`, params.requestId);
    }
  })();
}

export function handlePermissionResponse(command: Extract<BridgeCommand, { command: "permission_response" }>): void {
  const session = sessionById(command.session_id);
  if (!session) {
    logPermissionDebug(
      `response dropped: unknown session session_id=${command.session_id} tool_call_id=${command.tool_call_id}`,
    );
    return;
  }
  const resolver = session.pendingPermissions.get(command.tool_call_id);
  if (!resolver) {
    logPermissionDebug(
      `response dropped: no pending resolver session_id=${command.session_id} tool_call_id=${command.tool_call_id}`,
    );
    return;
  }
  session.pendingPermissions.delete(command.tool_call_id);

  const outcome = command.outcome as PermissionOutcome;
  if (resolver.onOutcome) {
    resolver.onOutcome(outcome);
    return;
  }
  if (!resolver.resolve) {
    logPermissionDebug(
      `response dropped: resolver missing callback session_id=${command.session_id} tool_call_id=${command.tool_call_id}`,
    );
    return;
  }
  const selectedOption = outcome.outcome === "selected" ? outcome.option_id : "cancelled";
  logPermissionDebug(
    `response session_id=${command.session_id} tool_call_id=${command.tool_call_id} tool=${resolver.toolName} ` +
      `selected=${selectedOption} suggestions=${formatPermissionUpdates(resolver.suggestions)}`,
  );
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
  if (permissionResult.behavior === "allow") {
    logPermissionDebug(
      `result tool_call_id=${command.tool_call_id} behavior=allow updated_permissions=` +
        `${formatPermissionUpdates(permissionResult.updatedPermissions)}`,
    );
  } else {
    logPermissionDebug(
      `result tool_call_id=${command.tool_call_id} behavior=deny message=${permissionResult.message}`,
    );
  }
  resolver.resolve(permissionResult);
}
