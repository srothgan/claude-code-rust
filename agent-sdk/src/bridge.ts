import { createRequire } from "node:module";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import readline from "node:readline";
import { pathToFileURL } from "node:url";
import {
  getSessionMessages,
  listSessions,
  renameSession,
} from "@anthropic-ai/claude-agent-sdk";
import type {
  RewindFilesResult as SdkRewindFilesResult,
  SessionMessage,
} from "@anthropic-ai/claude-agent-sdk";
import type {
  BridgeCommand,
  BridgeEvent,
  RewindFilesResult,
  RewindRestoreMode,
  RewindTarget,
} from "./types.js";
import type { EffortLevel } from "./types.js";
import {
  buildModeState,
  markModeUnavailableForSession,
  parseCommandEnvelope,
  permissionModeFailureLooksUnsupported,
  refreshSupportedModesForSession,
  toPermissionMode,
} from "./bridge/commands.js";
import {
  writeEvent,
  failConnection,
  slashError,
  emitRuntimeReloadCompleted,
  emitRuntimeReloadFailed,
  emitSessionUpdate,
  emitSessionsList,
  currentSessionListOptions,
  setSessionListingDir,
} from "./bridge/events.js";
import { contentFromPrompt } from "./bridge/message_handlers.js";
import {
  sessions,
  sessionById,
  createSession,
  closeAllSessions,
  handleElicitationResponse,
  handlePermissionResponse,
  handleQuestionResponse,
  handleUserDialogResponse,
  emitCurrentModelUpdate,
  refreshCurrentModel,
  shouldInvalidateResolvedRuntimeModel,
  type PendingRewindResult,
  type SessionState,
} from "./bridge/session_lifecycle.js";
import { mapSessionMessagesToUpdates } from "./bridge/history.js";
import { emitAvailableAgentsIfChanged, mapAvailableAgents } from "./bridge/agents.js";
import { mapSdkSlashCommands, updateAvailableCommands } from "./bridge/available_commands.js";
import { mapSdkAccountInfo } from "./bridge/account_metadata.js";
import {
  MCP_STALE_STATUS_REVALIDATION_COOLDOWN_MS,
  emitReconciledMcpSnapshotFromStatuses,
  handleMcpAuthenticateCommand,
  handleMcpClearAuthCommand,
  handleMcpOauthCallbackUrlCommand,
  handleMcpReconnectCommand,
  handleMcpSetServersCommand,
  handleMcpStatusCommand,
  handleMcpToggleCommand,
  staleMcpAuthCandidates,
} from "./bridge/mcp.js";
import { bridgeLogger, LOG_TARGETS, logBridgeCommandReceived } from "./bridge/logger.js";
import { dispatchCancelTurnCommand } from "./bridge/command_dispatch.js";

// Re-exports: all symbols that tests and external consumers import from bridge.js.
export { AsyncQueue } from "./bridge/shared.js";
export { asRecordOrNull } from "./bridge/shared.js";
export { CACHE_SPLIT_POLICY, previewKilobyteLabel } from "./bridge/cache_policy.js";
export {
  buildToolResultFields,
  createToolCall,
  isShellToolName,
  normalizeToolKind,
  normalizeToolResultText,
  unwrapToolUseResult,
} from "./bridge/tooling.js";
export { looksLikeAuthRequired } from "./bridge/auth.js";
export { parseCommandEnvelope } from "./bridge/commands.js";
export { buildSessionListOptions } from "./bridge/events.js";
export {
  mapInitSlashCommands,
  mapSdkSlashCommands,
  updateAvailableCommands,
} from "./bridge/available_commands.js";
export {
  permissionOptionsFromSuggestions,
  permissionResultFromOutcome,
} from "./bridge/permissions.js";
export {
  mapSessionMessagesToUpdates,
  mapSdkSessions,
} from "./bridge/history.js";
export { handleSdkMessage, handleTaskSystemMessage } from "./bridge/message_handlers.js";
export { mapAvailableAgents } from "./bridge/agents.js";
export {
  buildQueryOptions,
  resolveClaudeCodeSpawnCommand,
} from "./bridge/session_lifecycle.js";
export { mapAvailableModels } from "./bridge/model_metadata.js";
export {
  bridgeMcpConfigToSdk,
  mapMcpServerStatus,
  mapMcpServerStatusConfig,
} from "./bridge/mcp_metadata.js";
export {
  apiProviderIsExternal,
  isKnownApiProvider,
  mapSdkAccountInfo,
  shouldEmitStartupAuthRequiredForAccount,
} from "./bridge/account_metadata.js";
export {
  parseFastModeState,
  parseRateLimitStatus,
  parseRuntimeSessionState,
  parseApiRetryError,
  buildRateLimitUpdate,
  buildApiRetryUpdate,
  normalizeSettingsParseError,
  normalizeSettingsParseErrors,
} from "./bridge/state_parsing.js";
export { MCP_STALE_STATUS_REVALIDATION_COOLDOWN_MS, staleMcpAuthCandidates };
export type {
  SessionState,
  ConnectEventKind,
  PendingPermission,
  PendingQuestion,
} from "./bridge/session_lifecycle.js";

export function buildSessionMutationOptions(
  cwd?: string,
): import("@anthropic-ai/claude-agent-sdk").SessionMutationOptions | undefined {
  return cwd ? { dir: cwd } : undefined;
}

function normalizeRewindTargetText(text: string): string {
  return text.replace(/\s+/g, " ").trim();
}

type UserTextParts = {
  firstText: string;
  inputText: string;
};

function userTextPartsFromSessionMessage(message: SessionMessage): UserTextParts | undefined {
  const record = message as unknown as Record<string, unknown>;
  if (record.type !== "user") {
    return undefined;
  }
  const payload = record.message;
  if (!payload || typeof payload !== "object" || Array.isArray(payload)) {
    return undefined;
  }
  const content = (payload as Record<string, unknown>).content;
  if (typeof content === "string") {
    const firstText = normalizeRewindTargetText(content);
    return firstText ? { firstText, inputText: content } : undefined;
  }
  if (!Array.isArray(content)) {
    return undefined;
  }
  const textBlocks: string[] = [];
  for (const block of content) {
    if (!block || typeof block !== "object" || Array.isArray(block)) {
      continue;
    }
    const blockRecord = block as Record<string, unknown>;
    if (blockRecord.type === "text" && typeof blockRecord.text === "string") {
      textBlocks.push(blockRecord.text);
    }
  }
  const firstText = textBlocks.map(normalizeRewindTargetText).find((text) => text.length > 0);
  return firstText ? { firstText, inputText: textBlocks.join("\n").trim() } : undefined;
}

export function rewindTargetsFromSessionMessages(messages: SessionMessage[]): RewindTarget[] {
  const targets: RewindTarget[] = [];
  let previousAssistantUuid: string | undefined;

  messages.forEach((message, index) => {
    const record = message as unknown as Record<string, unknown>;
    const uuid = typeof record.uuid === "string" ? record.uuid.trim() : "";
    if (record.type === "assistant" && uuid) {
      previousAssistantUuid = uuid;
      return;
    }
    const textParts = userTextPartsFromSessionMessage(message);
    if (!uuid || !textParts) {
      return;
    }
    targets.push({
      uuid,
      first_text: textParts.firstText,
      input_text: textParts.inputText,
      index,
      ...(previousAssistantUuid ? { previous_assistant_uuid: previousAssistantUuid } : {}),
    });
  });

  return targets.reverse();
}

type SessionTitleGeneratingQuery = import("@anthropic-ai/claude-agent-sdk").Query & {
  generateSessionTitle: (
    description: string,
    options?: { persist?: boolean },
  ) => Promise<string | null | undefined>;
};

export function canGenerateSessionTitle(
  query: import("@anthropic-ai/claude-agent-sdk").Query,
): query is SessionTitleGeneratingQuery {
  return typeof (query as { generateSessionTitle?: unknown }).generateSessionTitle === "function";
}

export async function generatePersistedSessionTitle(
  query: import("@anthropic-ai/claude-agent-sdk").Query,
  description: string,
): Promise<string> {
  if (!canGenerateSessionTitle(query)) {
    throw new Error("SDK query does not support generateSessionTitle");
  }
  const title = await query.generateSessionTitle(description, { persist: true });
  if (typeof title !== "string" || title.trim().length === 0) {
    throw new Error("SDK did not return a generated session title");
  }
  return title;
}

export async function applySessionEffort(
  query: import("@anthropic-ai/claude-agent-sdk").Query,
  effort: EffortLevel,
): Promise<void> {
  const settings = { effortLevel: effort } as Parameters<typeof query.applyFlagSettings>[0];
  // applyFlagSettings controls live session settings; SDK Settings typings model persisted effort levels.
  await query.applyFlagSettings(settings);
}

export async function applySessionAgent(
  query: import("@anthropic-ai/claude-agent-sdk").Query,
  agent: string | null,
): Promise<void> {
  const settings = { agent } as Parameters<typeof query.applyFlagSettings>[0];
  await query.applyFlagSettings(settings);
}

export function emitEffortConfigOptionUpdate(sessionId: string, effort: EffortLevel): void {
  emitSessionUpdate(sessionId, {
    type: "config_option_update",
    option_id: "effortLevel",
    value: effort,
  });
}

export function emitAgentConfigOptionUpdate(sessionId: string, agent: string | null): void {
  emitSessionUpdate(sessionId, {
    type: "config_option_update",
    option_id: "agent",
    value: agent,
  });
}

const EXPECTED_AGENT_SDK_VERSION = "0.3.198";
const require = createRequire(import.meta.url);

export function resolveInstalledAgentSdkVersion(): string | undefined {
  try {
    const entryPath = require.resolve("@anthropic-ai/claude-agent-sdk");
    const packageJsonPath = join(dirname(entryPath), "package.json");
    const pkg = JSON.parse(readFileSync(packageJsonPath, "utf8")) as { version?: unknown };
    return typeof pkg.version === "string" ? pkg.version : undefined;
  } catch {
    return undefined;
  }
}

export function agentSdkVersionCompatibilityError(): string | undefined {
  const installed = resolveInstalledAgentSdkVersion();
  if (!installed) {
    return (
      `Agent SDK version check failed: unable to resolve installed ` +
      `@anthropic-ai/claude-agent-sdk package.json (expected ${EXPECTED_AGENT_SDK_VERSION}).`
    );
  }
  if (installed === EXPECTED_AGENT_SDK_VERSION) {
    return undefined;
  }
  return (
    `Unsupported @anthropic-ai/claude-agent-sdk version: expected ${EXPECTED_AGENT_SDK_VERSION}, ` +
    `found ${installed}.`
  );
}

export async function handleReloadPluginsCommand(
  session: SessionState,
  requestId?: string,
): Promise<void> {
  try {
    const result = await session.query.reloadPlugins();
    updateAvailableCommands(session, "reload_plugins", mapSdkSlashCommands(result.commands));
    emitAvailableAgentsIfChanged(session, mapAvailableAgents(result.agents));
    await emitReconciledMcpSnapshotFromStatuses(
      session,
      result.mcpServers,
      "reload_plugins",
      requestId,
    );
    emitRuntimeReloadCompleted(session.sessionId, requestId);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    bridgeLogger.warn({
      target: LOG_TARGETS.APP_SESSION,
      eventName: "reload_plugins_failed",
      message: "failed to reload session plugins",
      outcome: "failure",
      ...(requestId ? { requestId } : {}),
      sessionId: session.sessionId,
      fields: { error_message: message },
    });
    emitRuntimeReloadFailed(session.sessionId, message, requestId);
  }
}

type ResolvedRewindTarget = {
  inputText: string;
  previousAssistantUuid?: string;
  targetIndex: number;
  retainedMessages: SessionMessage[];
};

export type RewindConversationPlan = ResolvedRewindTarget & {
  resumeUpdates: ReturnType<typeof mapSessionMessagesToUpdates>;
};

function resolveRewindTarget(
  messages: SessionMessage[],
  targetUserMessageId: string,
): ResolvedRewindTarget | null {
  let previousAssistantUuid: string | undefined;
  let textUserCountBeforeTarget = 0;

  for (let index = 0; index < messages.length; index += 1) {
    const message = messages[index];
    const record = message as unknown as Record<string, unknown>;
    const uuid = typeof record.uuid === "string" ? record.uuid.trim() : "";
    if (record.type === "assistant" && uuid) {
      previousAssistantUuid = uuid;
      continue;
    }

    const textParts = userTextPartsFromSessionMessage(message);
    if (!textParts) {
      continue;
    }
    if (uuid === targetUserMessageId) {
      if (!previousAssistantUuid && textUserCountBeforeTarget > 0) {
        return null;
      }
      if (previousAssistantUuid) {
        const anchorIndex = messages.findIndex((candidate) => {
          const candidateRecord = candidate as unknown as Record<string, unknown>;
          return candidateRecord.uuid === previousAssistantUuid;
        });
        if (anchorIndex < 0) {
          return null;
        }
        return {
          inputText: textParts.inputText,
          previousAssistantUuid,
          targetIndex: index,
          retainedMessages: messages.slice(0, anchorIndex + 1),
        };
      }
      return {
        inputText: textParts.inputText,
        targetIndex: index,
        retainedMessages: [],
      };
    }
    if (uuid) {
      textUserCountBeforeTarget += 1;
    }
  }

  return null;
}

export function buildRewindConversationPlan(
  messages: SessionMessage[],
  targetUserMessageId: string,
): RewindConversationPlan | null {
  const resolved = resolveRewindTarget(messages, targetUserMessageId);
  if (!resolved) {
    return null;
  }
  return {
    ...resolved,
    resumeUpdates: mapSessionMessagesToUpdates(resolved.retainedMessages),
  };
}

function mapRewindFilesResult(result: SdkRewindFilesResult): RewindFilesResult {
  return {
    can_rewind: result.canRewind,
    ...(result.error ? { error: result.error } : {}),
    files_changed: result.filesChanged ?? [],
    ...(result.insertions !== undefined ? { insertions: result.insertions } : {}),
    ...(result.deletions !== undefined ? { deletions: result.deletions } : {}),
  };
}

async function rewindFiles(
  session: SessionState,
  targetUserMessageId: string,
  dryRun: boolean,
): Promise<RewindFilesResult> {
  if (typeof session.query.rewindFiles !== "function") {
    return {
      can_rewind: false,
      error: "file rewind is not supported by this SDK runtime",
      files_changed: [],
    };
  }
  return mapRewindFilesResult(await session.query.rewindFiles(targetUserMessageId, { dryRun }));
}

function emitRewindResult(
  sessionId: string,
  restoreMode: RewindRestoreMode,
  status: Extract<BridgeEvent, { event: "rewind_result" }>["status"],
  requestId: string | undefined,
  fileResult?: RewindFilesResult,
  message?: string,
): void {
  writeEvent(
    {
      event: "rewind_result",
      session_id: sessionId,
      restore_mode: restoreMode,
      status,
      ...(fileResult ? { file_result: fileResult } : {}),
      ...(message ? { message } : {}),
    },
    requestId,
  );
}

function requestIdFromCommandLine(line: string): string | undefined {
  try {
    const parsed = JSON.parse(line) as unknown;
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
      return undefined;
    }
    const requestId = (parsed as Record<string, unknown>).request_id;
    return typeof requestId === "string" ? requestId : undefined;
  } catch {
    return undefined;
  }
}

async function replaceConversationForRewind(
  command: Extract<BridgeCommand, { command: "rewind" }>,
  session: SessionState,
  targetUserMessageId: string,
  requestId: string | undefined,
  pendingRewindResult?: PendingRewindResult,
): Promise<void> {
  const historyMessages = await getSessionMessages(command.session_id, {
    dir: session.cwd,
    includeSystemMessages: true,
  });
  const resolved = buildRewindConversationPlan(historyMessages, targetUserMessageId);
  if (!resolved) {
    bridgeLogger.warn({
      target: LOG_TARGETS.APP_SESSION,
      eventName: "rewind_failed",
      message: "conversation rewind target was stale or inconsistent",
      outcome: "failure",
      ...(requestId ? { requestId } : {}),
      sessionId: session.sessionId,
      fields: {
        target_user_message_id: targetUserMessageId,
        history_message_count: historyMessages.length,
        reason: "target_not_found_or_inconsistent",
      },
    });
    throw new Error(`unknown rewind target: ${targetUserMessageId}`);
  }

  const resumeUpdates = resolved.resumeUpdates;
  const staleSessions = Array.from(sessions.values());
  setSessionListingDir(session.cwd);
  bridgeLogger.info({
    target: LOG_TARGETS.APP_SESSION,
    eventName: "rewind_target_resolved",
    message: "conversation rewind target resolved",
    outcome: "success",
    ...(requestId ? { requestId } : {}),
    sessionId: session.sessionId,
    fields: {
      target_user_message_id: targetUserMessageId,
      target_index: resolved.targetIndex,
      previous_assistant_uuid: resolved.previousAssistantUuid ?? "<none>",
      history_message_count: historyMessages.length,
      retained_message_count: resolved.retainedMessages.length,
      retained_update_count: resumeUpdates.length,
      stale_session_count: staleSessions.length,
    },
  });

  if (!resolved.previousAssistantUuid) {
    bridgeLogger.info({
      target: LOG_TARGETS.APP_SESSION,
      eventName: "rewind_first_message_branch",
      message: "conversation rewind target is first user message; creating fresh replacement",
      outcome: "start",
      ...(requestId ? { requestId } : {}),
      sessionId: session.sessionId,
      fields: {
        target_user_message_id: targetUserMessageId,
        history_message_count: historyMessages.length,
        stale_session_count: staleSessions.length,
      },
    });
    await createSession({
      cwd: session.cwd,
      launchSettings: command.launch_settings,
      connectEvent: "session_replaced",
      requestId,
      restoredInput: resolved.inputText,
      ...(pendingRewindResult ? { pendingRewindResult } : {}),
      ...(staleSessions.length > 0 ? { sessionsToCloseAfterConnect: staleSessions } : {}),
    });
    bridgeLogger.info({
      target: LOG_TARGETS.APP_SESSION,
      eventName: "rewind_replacement_created",
      message: "conversation rewind replacement session created",
      outcome: "success",
      ...(requestId ? { requestId } : {}),
      fields: {
        target_user_message_id: targetUserMessageId,
        resume_session_at: "<none>",
        retained_message_count: 0,
        retained_update_count: 0,
        stale_session_count: staleSessions.length,
      },
    });
    return;
  }

  const sessionsToCloseAfterConnect = staleSessions.filter((stale) => stale !== session);
  await createSession({
    cwd: session.cwd,
    resume: command.session_id,
    resumeSessionAt: resolved.previousAssistantUuid,
    launchSettings: command.launch_settings,
    connectEvent: "session_replaced",
    requestId,
    sessionsToCloseBeforeRegister: [session],
    ...(resumeUpdates.length > 0 ? { resumeUpdates } : {}),
    restoredInput: resolved.inputText,
    ...(pendingRewindResult ? { pendingRewindResult } : {}),
    ...(sessionsToCloseAfterConnect.length > 0 ? { sessionsToCloseAfterConnect } : {}),
  });
  bridgeLogger.info({
    target: LOG_TARGETS.APP_SESSION,
    eventName: "rewind_replacement_created",
    message: "conversation rewind replacement session created",
    outcome: "success",
    ...(requestId ? { requestId } : {}),
    sessionId: command.session_id,
    fields: {
      target_user_message_id: targetUserMessageId,
      resume_session_at: resolved.previousAssistantUuid,
      retained_message_count: resolved.retainedMessages.length,
      retained_update_count: resumeUpdates.length,
      stale_session_count: staleSessions.length,
    },
  });
}

async function handleRewind(
  command: Extract<BridgeCommand, { command: "rewind" }>,
  requestId?: string,
): Promise<void> {
  const session = sessionById(command.session_id);
  if (!session) {
    slashError(command.session_id, `unknown session: ${command.session_id}`, requestId);
    return;
  }
  const targetUserMessageId = command.target_user_message_id.trim();
  if (!targetUserMessageId) {
    slashError(command.session_id, "rewind target cannot be empty", requestId);
    return;
  }

  bridgeLogger.info({
    target: LOG_TARGETS.APP_SESSION,
    eventName: "rewind_requested",
    message: "rewind requested",
    outcome: "start",
    ...(requestId ? { requestId } : {}),
    sessionId: session.sessionId,
    fields: {
      target_user_message_id: targetUserMessageId,
      restore_mode: command.restore_mode,
    },
  });

  if (command.restore_mode === "conversation") {
    try {
      await replaceConversationForRewind(command, session, targetUserMessageId, requestId);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      bridgeLogger.error({
        target: LOG_TARGETS.APP_SESSION,
        eventName: "rewind_failed",
        message: "conversation rewind failed",
        outcome: "failure",
        ...(requestId ? { requestId } : {}),
        sessionId: session.sessionId,
        fields: {
          target_user_message_id: targetUserMessageId,
          restore_mode: command.restore_mode,
          error_message: message,
        },
      });
      slashError(command.session_id, `failed to rewind conversation: ${message}`, requestId);
    }
    return;
  }

  let appliedFileResult: RewindFilesResult;
  try {
    const dryRunResult = await rewindFiles(session, targetUserMessageId, true);
    if (!dryRunResult.can_rewind) {
      slashError(
        command.session_id,
        dryRunResult.error ?? "failed to dry-run file rewind",
        requestId,
      );
      return;
    }
    appliedFileResult = await rewindFiles(session, targetUserMessageId, false);
    if (!appliedFileResult.can_rewind) {
      slashError(command.session_id, appliedFileResult.error ?? "failed to restore code", requestId);
      return;
    }
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    bridgeLogger.error({
      target: LOG_TARGETS.APP_SESSION,
      eventName: "rewind_failed",
      message: "file rewind failed",
      outcome: "failure",
      ...(requestId ? { requestId } : {}),
      sessionId: session.sessionId,
      fields: {
        target_user_message_id: targetUserMessageId,
        restore_mode: command.restore_mode,
        error_message: message,
      },
    });
    slashError(command.session_id, `failed to restore code: ${message}`, requestId);
    return;
  }

  if (command.restore_mode === "code") {
    emitRewindResult(
      session.sessionId,
      command.restore_mode,
      "success",
      requestId,
      appliedFileResult,
    );
    return;
  }

  try {
    await replaceConversationForRewind(command, session, targetUserMessageId, requestId, {
      event: "rewind_result",
      restore_mode: command.restore_mode,
      status: "success",
      file_result: appliedFileResult,
    });
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    bridgeLogger.error({
      target: LOG_TARGETS.APP_SESSION,
      eventName: "rewind_failed",
      message: "conversation rewind failed after file restore",
      outcome: "partial_failure",
      ...(requestId ? { requestId } : {}),
      sessionId: session.sessionId,
      fields: {
        target_user_message_id: targetUserMessageId,
        restore_mode: command.restore_mode,
        error_message: message,
      },
    });
    emitRewindResult(
      session.sessionId,
      command.restore_mode,
      "partial_failure",
      requestId,
      appliedFileResult,
      `Code was restored, but the conversation could not be rewound: ${message}`,
    );
  }
}

async function handleCommand(command: BridgeCommand, requestId?: string): Promise<void> {
  logBridgeCommandReceived(command, requestId);
  const sdkVersionError = agentSdkVersionCompatibilityError();
  if (sdkVersionError && command.command !== "initialize" && command.command !== "shutdown") {
    bridgeLogger.error({
      target: LOG_TARGETS.BRIDGE_LIFECYCLE,
      eventName: "bridge_command_rejected",
      message: "bridge command rejected due to unsupported SDK version",
      outcome: "failure",
      ...(requestId ? { requestId } : {}),
      fields: {
        bridge_command: command.command,
        error_message: sdkVersionError,
      },
    });
    failConnection(sdkVersionError, requestId);
    return;
  }

  switch (command.command) {
    case "initialize":
      if (sdkVersionError) {
        bridgeLogger.error({
          target: LOG_TARGETS.BRIDGE_LIFECYCLE,
          eventName: "bridge_initialize_failed",
          message: "bridge initialization failed due to unsupported SDK version",
          outcome: "failure",
          ...(requestId ? { requestId } : {}),
          fields: { error_message: sdkVersionError },
        });
        failConnection(sdkVersionError, requestId);
        return;
      }
      setSessionListingDir(command.cwd);
      writeEvent(
        {
          event: "initialized",
          result: {
            agent_name: "claude-rs-agent-bridge",
            agent_version: "0.1.0",
            auth_methods: [
              {
                id: "claude-login",
                name: "Log in with Claude",
                description: "Run `claude /login` in a terminal",
              },
            ],
            capabilities: {
              prompt_image: true,
              prompt_embedded_context: true,
              supports_session_listing: true,
              supports_resume_session: true,
            },
          },
        },
        requestId,
      );
      await emitSessionsList(requestId);
      return;

    case "create_session":
      bridgeLogger.info({
        target: LOG_TARGETS.APP_SESSION,
        eventName: "session_create_requested",
        message: "session creation requested",
        outcome: "start",
        ...(requestId ? { requestId } : {}),
        fields: {
          cwd: command.cwd,
          resume_requested: command.resume !== undefined,
        },
      });
      setSessionListingDir(command.cwd);
      await createSession({
        cwd: command.cwd,
        resume: command.resume,
        launchSettings: command.launch_settings,
        connectEvent: "connected",
        requestId,
      });
      return;

    case "resume_session": {
      bridgeLogger.info({
        target: LOG_TARGETS.APP_SESSION,
        eventName: "session_resume_requested",
        message: "session resume requested",
        outcome: "start",
        ...(requestId ? { requestId } : {}),
        sessionId: command.session_id,
      });
      try {
        const sdkSessions = await listSessions(currentSessionListOptions());
        const matched = sdkSessions.find((entry) => entry.sessionId === command.session_id);
        if (!matched) {
          bridgeLogger.warn({
            target: LOG_TARGETS.APP_SESSION,
            eventName: "session_resume_lookup_failed",
            message: "session resume requested for an unknown session",
            outcome: "failure",
            ...(requestId ? { requestId } : {}),
            sessionId: command.session_id,
            fields: { reason: "unknown_session" },
          });
          slashError(command.session_id, `unknown session: ${command.session_id}`, requestId);
          return;
        }
        setSessionListingDir(matched.cwd ?? process.cwd());
        const historyMessages = await getSessionMessages(
          command.session_id,
          matched.cwd
            ? { dir: matched.cwd, includeSystemMessages: true }
            : { includeSystemMessages: true },
        );
        const resumeUpdates = mapSessionMessagesToUpdates(historyMessages);
        const staleSessions = Array.from(sessions.values());
        const hadActiveSession = staleSessions.length > 0;
        bridgeLogger.info({
          target: LOG_TARGETS.APP_SESSION,
          eventName: "session_resume_history_loaded",
          message: "session resume history loaded",
          outcome: "success",
          ...(requestId ? { requestId } : {}),
          sessionId: command.session_id,
          fields: {
            history_update_count: resumeUpdates.length,
            stale_session_count: staleSessions.length,
          },
        });
        await createSession({
          cwd: matched.cwd ?? process.cwd(),
          resume: command.session_id,
          launchSettings: command.launch_settings,
          ...(resumeUpdates.length > 0 ? { resumeUpdates } : {}),
          connectEvent: hadActiveSession ? "session_replaced" : "connected",
          requestId,
          ...(hadActiveSession ? { sessionsToCloseAfterConnect: staleSessions } : {}),
        });
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        bridgeLogger.error({
          target: LOG_TARGETS.APP_SESSION,
          eventName: "session_resume_failed",
          message: "session resume failed",
          outcome: "failure",
          ...(requestId ? { requestId } : {}),
          sessionId: command.session_id,
          fields: { error_message: message },
        });
        slashError(command.session_id, `failed to resume session: ${message}`, requestId);
      }
      return;
    }

    case "new_session":
      bridgeLogger.info({
        target: LOG_TARGETS.APP_SESSION,
        eventName: "session_new_requested",
        message: "replacement session requested",
        outcome: "start",
        ...(requestId ? { requestId } : {}),
        fields: { cwd: command.cwd },
      });
      await closeAllSessions({ reason: "new_session_requested", requestId });
      setSessionListingDir(command.cwd);
      await createSession({
        cwd: command.cwd,
        launchSettings: command.launch_settings,
        connectEvent: "session_replaced",
        requestId,
      });
      return;

    case "prompt": {
      const session = sessionById(command.session_id);
      if (!session) {
        slashError(command.session_id, `unknown session: ${command.session_id}`, requestId);
        return;
      }
      const content = contentFromPrompt(command);
      if (content.length === 0) {
        return;
      }
      const message: import("@anthropic-ai/claude-agent-sdk").SDKUserMessage = {
        type: "user",
        session_id: session.sessionId,
        parent_tool_use_id: null,
        message: {
          role: "user",
          content,
        },
      };
      session.input.enqueue(message);
      return;
    }

    case "cancel_turn": {
      await dispatchCancelTurnCommand(command, { requestId, sessionById, slashError });
      return;
    }

    case "set_model": {
      const session = sessionById(command.session_id);
      if (!session) {
        slashError(command.session_id, `unknown session: ${command.session_id}`, requestId);
        return;
      }
      bridgeLogger.info({
        target: LOG_TARGETS.APP_SESSION,
        eventName: "set_model_started",
        message: "set model started",
        outcome: "start",
        sessionId: session.sessionId,
        requestId,
        fields: {
          requested_model: command.model,
          previous_requested_model: session.requestedModelId,
          previous_session_model: session.model,
          previous_resolved_runtime_model: session.resolvedRuntimeModelId,
          previous_current_model: session.currentModel?.resolved_id,
        },
      });
      try {
        const previousRequestedModel = session.requestedModelId;
        const previousSessionModel = session.model;
        await session.query.setModel(command.model);
        session.requestedModelId = command.model;
        session.model = command.model;
        const invalidatedResolvedRuntimeModel = shouldInvalidateResolvedRuntimeModel(
          previousRequestedModel,
          previousSessionModel,
          command.model,
        );
        if (invalidatedResolvedRuntimeModel) {
          session.resolvedRuntimeModelId = undefined;
        }
        const changed = refreshCurrentModel(session, true);
        const forcedCurrentModelUpdate = !changed && emitCurrentModelUpdate(session);
        bridgeLogger.info({
          target: LOG_TARGETS.APP_SESSION,
          eventName: "set_model_succeeded",
          message: "set model completed",
          outcome: "success",
          sessionId: session.sessionId,
          requestId,
          fields: {
            requested_model: command.model,
            session_model_after: session.model,
            resolved_runtime_model_after: session.resolvedRuntimeModelId,
            current_model_after: session.currentModel?.resolved_id,
            current_model_display_short: session.currentModel?.display_name_short,
            current_model_display_long: session.currentModel?.display_name_long,
            current_model_update_emitted: changed || forcedCurrentModelUpdate,
            current_model_update_forced: forcedCurrentModelUpdate,
            resolved_runtime_model_invalidated: invalidatedResolvedRuntimeModel,
          },
        });
        refreshSupportedModesForSession(session);
        if (session.mode) {
          emitSessionUpdate(session.sessionId, {
            type: "mode_state_update",
            mode: buildModeState(session, session.mode),
          });
        }
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        bridgeLogger.warn({
          target: LOG_TARGETS.APP_SESSION,
          eventName: "set_model_failed",
          message: "set model failed",
          outcome: "failure",
          sessionId: session.sessionId,
          requestId,
          fields: {
            requested_model: command.model,
            error_message: message,
            previous_requested_model: session.requestedModelId,
            previous_session_model: session.model,
            previous_resolved_runtime_model: session.resolvedRuntimeModelId,
            previous_current_model: session.currentModel?.resolved_id,
          },
        });
        slashError(command.session_id, `failed to set model: ${message}`, requestId);
      }
      return;
    }

    case "set_mode": {
      const session = sessionById(command.session_id);
      if (!session) {
        slashError(command.session_id, `unknown session: ${command.session_id}`, requestId);
        return;
      }
      const mode = toPermissionMode(command.mode);
      if (!mode) {
        slashError(command.session_id, `unsupported mode: ${command.mode}`, requestId);
        return;
      }
      try {
        await session.query.setPermissionMode(mode);
        session.mode = mode;
        refreshSupportedModesForSession(session);
        emitSessionUpdate(session.sessionId, {
          type: "current_mode_update",
          current_mode_id: mode,
        });
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        if (permissionModeFailureLooksUnsupported(mode, message)) {
          const changed = markModeUnavailableForSession(session, mode);
          if (changed && session.mode) {
            emitSessionUpdate(session.sessionId, {
              type: "mode_state_update",
              mode: buildModeState(session, session.mode),
            });
          }
        }
        slashError(command.session_id, `failed to set mode to ${mode}: ${message}`, requestId);
      }
      return;
    }

    case "set_effort": {
      const session = sessionById(command.session_id);
      if (!session) {
        slashError(command.session_id, `unknown session: ${command.session_id}`, requestId);
        return;
      }
      try {
        await applySessionEffort(session.query, command.effort);
        emitEffortConfigOptionUpdate(session.sessionId, command.effort);
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        slashError(command.session_id, `failed to set effort: ${message}`, requestId);
      }
      return;
    }

    case "set_agent": {
      const session = sessionById(command.session_id);
      if (!session) {
        slashError(command.session_id, `unknown session: ${command.session_id}`, requestId);
        return;
      }
      try {
        await applySessionAgent(session.query, command.agent);
        emitAgentConfigOptionUpdate(session.sessionId, command.agent);
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        slashError(command.session_id, `failed to set agent: ${message}`, requestId);
      }
      return;
    }

    case "generate_session_title": {
      const session = sessionById(command.session_id);
      if (!session) {
        slashError(command.session_id, `unknown session: ${command.session_id}`, requestId);
        return;
      }
      try {
        await generatePersistedSessionTitle(session.query, command.description);
        setSessionListingDir(session.cwd);
        await emitSessionsList(requestId);
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        slashError(command.session_id, `failed to generate session title: ${message}`, requestId);
      }
      return;
    }

    case "rename_session": {
      const session = sessionById(command.session_id);
      if (!session) {
        slashError(command.session_id, `unknown session: ${command.session_id}`, requestId);
        return;
      }
      try {
        await renameSession(
          command.session_id,
          command.title,
          buildSessionMutationOptions(session.cwd),
        );
        setSessionListingDir(session.cwd);
        await emitSessionsList(requestId);
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        slashError(command.session_id, `failed to rename session: ${message}`, requestId);
      }
      return;
    }

    case "get_status_snapshot": {
      const session = sessionById(command.session_id);
      if (!session) {
        slashError(command.session_id, `unknown session: ${command.session_id}`, requestId);
        return;
      }
      try {
        const account = await session.query.accountInfo();
        bridgeLogger.info({
          target: LOG_TARGETS.APP_AUTH,
          eventName: "status_snapshot_emitted",
          message: "status snapshot emitted",
          outcome: "success",
          ...(requestId ? { requestId } : {}),
          sessionId: session.sessionId,
          fields: {
            has_email: typeof account.email === "string" && account.email.trim().length > 0,
            has_organization: account.organization !== undefined,
            subscription_type: account.subscriptionType,
            token_source: account.tokenSource,
            api_key_source: account.apiKeySource,
            api_provider: account.apiProvider,
          },
        });
        writeEvent(
          {
            event: "status_snapshot",
            session_id: session.sessionId,
            account: mapSdkAccountInfo(account),
          },
          requestId,
        );
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        bridgeLogger.warn({
          target: LOG_TARGETS.APP_AUTH,
          eventName: "status_snapshot_failed",
          message: "failed to build status snapshot",
          outcome: "failure",
          ...(requestId ? { requestId } : {}),
          sessionId: session.sessionId,
          fields: { error_message: message },
        });
        throw error;
      }
      return;
    }

    case "get_context_usage": {
      const session = sessionById(command.session_id);
      if (!session) {
        slashError(command.session_id, `unknown session: ${command.session_id}`, requestId);
        return;
      }
      try {
        const usage = await session.query.getContextUsage();
        if (typeof usage.model === "string" && usage.model.trim().length > 0) {
          session.resolvedRuntimeModelId = usage.model.trim();
          refreshCurrentModel(session, true);
        }
        const rawPercentage = typeof usage.percentage === "number" ? usage.percentage : undefined;
        const normalizedPercentage =
          rawPercentage === undefined || !Number.isFinite(rawPercentage)
            ? undefined
            : Math.max(0, Math.min(100, Math.round(rawPercentage)));
        bridgeLogger.debug({
          target: LOG_TARGETS.APP_SESSION,
          eventName: "context_usage_succeeded",
          message: "session context usage received from SDK",
          outcome: "success",
          ...(requestId ? { requestId } : {}),
          sessionId: session.sessionId,
          fields: {
            raw_percentage: rawPercentage,
            normalized_percentage: normalizedPercentage,
            total_tokens: typeof usage.totalTokens === "number" ? usage.totalTokens : undefined,
            max_tokens: typeof usage.maxTokens === "number" ? usage.maxTokens : undefined,
            raw_max_tokens: typeof usage.rawMaxTokens === "number" ? usage.rawMaxTokens : undefined,
            model: typeof usage.model === "string" ? usage.model : undefined,
          },
        });
        writeEvent(
          {
            event: "context_usage",
            session_id: session.sessionId,
            ...(normalizedPercentage !== undefined ? { percentage: normalizedPercentage } : {}),
          },
          requestId,
        );
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        bridgeLogger.warn({
          target: LOG_TARGETS.APP_SESSION,
          eventName: "context_usage_failed",
          message: "failed to get session context usage",
          outcome: "failure",
          ...(requestId ? { requestId } : {}),
          sessionId: session.sessionId,
          fields: { error_message: message },
        });
        writeEvent(
          {
            event: "context_usage",
            session_id: session.sessionId,
          },
          requestId,
        );
      }
      return;
    }

    case "get_rewind_targets": {
      const session = sessionById(command.session_id);
      if (!session) {
        slashError(command.session_id, `unknown session: ${command.session_id}`, requestId);
        return;
      }
      try {
        const historyMessages = await getSessionMessages(command.session_id, {
          dir: session.cwd,
          includeSystemMessages: true,
        });
        const targets = rewindTargetsFromSessionMessages(historyMessages);
        bridgeLogger.info({
          target: LOG_TARGETS.APP_SESSION,
          eventName: "rewind_targets_loaded",
          message: "rewind targets loaded from session history",
          outcome: "success",
          ...(requestId ? { requestId } : {}),
          sessionId: session.sessionId,
          fields: {
            history_message_count: historyMessages.length,
            target_count: targets.length,
          },
        });
        writeEvent(
          {
            event: "rewind_targets",
            session_id: session.sessionId,
            targets,
          },
          requestId,
        );
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        bridgeLogger.warn({
          target: LOG_TARGETS.APP_SESSION,
          eventName: "rewind_targets_failed",
          message: "failed to load rewind targets",
          outcome: "failure",
          ...(requestId ? { requestId } : {}),
          sessionId: session.sessionId,
          fields: { error_message: message },
        });
        slashError(command.session_id, `failed to load rewind targets: ${message}`, requestId);
      }
      return;
    }

    case "rewind":
      await handleRewind(command, requestId);
      return;

    case "reload_plugins": {
      const session = sessionById(command.session_id);
      if (!session) {
        slashError(command.session_id, `unknown session: ${command.session_id}`, requestId);
        return;
      }
      await handleReloadPluginsCommand(session, requestId);
      return;
    }

    case "mcp_status": {
      const session = sessionById(command.session_id);
      if (!session) {
        slashError(command.session_id, `unknown session: ${command.session_id}`, requestId);
        return;
      }
      await handleMcpStatusCommand(session, requestId);
      return;
    }

    case "mcp_reconnect": {
      const session = sessionById(command.session_id);
      if (!session) {
        slashError(command.session_id, `unknown session: ${command.session_id}`, requestId);
        return;
      }
      await handleMcpReconnectCommand(session, command, requestId);
      return;
    }

    case "mcp_toggle": {
      const session = sessionById(command.session_id);
      if (!session) {
        slashError(command.session_id, `unknown session: ${command.session_id}`, requestId);
        return;
      }
      await handleMcpToggleCommand(session, command, requestId);
      return;
    }

    case "mcp_set_servers": {
      const session = sessionById(command.session_id);
      if (!session) {
        slashError(command.session_id, `unknown session: ${command.session_id}`, requestId);
        return;
      }
      await handleMcpSetServersCommand(session, command, requestId);
      return;
    }

    case "mcp_authenticate": {
      const session = sessionById(command.session_id);
      if (!session) {
        slashError(command.session_id, `unknown session: ${command.session_id}`, requestId);
        return;
      }
      await handleMcpAuthenticateCommand(session, command, requestId);
      return;
    }

    case "mcp_clear_auth": {
      const session = sessionById(command.session_id);
      if (!session) {
        slashError(command.session_id, `unknown session: ${command.session_id}`, requestId);
        return;
      }
      await handleMcpClearAuthCommand(session, command, requestId);
      return;
    }

    case "mcp_oauth_callback_url": {
      const session = sessionById(command.session_id);
      if (!session) {
        slashError(command.session_id, `unknown session: ${command.session_id}`, requestId);
        return;
      }
      await handleMcpOauthCallbackUrlCommand(session, command, requestId);
      return;
    }

    case "permission_response":
      handlePermissionResponse(command);
      return;

    case "question_response":
      handleQuestionResponse(command);
      return;

    case "user_dialog_response":
      handleUserDialogResponse(command);
      return;

    case "elicitation_response":
      handleElicitationResponse(command);
      return;

    case "shutdown":
      bridgeLogger.info({
        target: LOG_TARGETS.BRIDGE_LIFECYCLE,
        eventName: "bridge_shutdown_requested",
        message: "bridge shutdown requested",
        outcome: "start",
        ...(requestId ? { requestId } : {}),
      });
      await closeAllSessions({ reason: "bridge_shutdown_requested", requestId });
      bridgeLogger.info({
        target: LOG_TARGETS.BRIDGE_LIFECYCLE,
        eventName: "bridge_shutdown_completed",
        message: "bridge shutdown completed",
        outcome: "success",
        ...(requestId ? { requestId } : {}),
      });
      process.exit(0);
      return;

    default:
      bridgeLogger.error({
        target: LOG_TARGETS.BRIDGE_PROTOCOL,
        eventName: "bridge_command_rejected",
        message: "received unsupported bridge command",
        outcome: "failure",
        ...(requestId ? { requestId } : {}),
        fields: {
          bridge_command: (command as { command?: string }).command ?? "unknown",
          reason: "unsupported_command",
        },
      });
      failConnection(`unhandled command: ${(command as { command?: string }).command ?? "unknown"}`, requestId);
  }
}

function main(): void {
  bridgeLogger.info({
    target: LOG_TARGETS.BRIDGE_LIFECYCLE,
    eventName: "bridge_process_started",
    message: "bridge process started",
    outcome: "start",
    fields: { pid: process.pid },
  });

  const rl = readline.createInterface({
    input: process.stdin,
    crlfDelay: Number.POSITIVE_INFINITY,
  });

  rl.on("line", (line) => {
    if (line.trim().length === 0) {
      return;
    }
    void (async () => {
      let parsed: { requestId?: string; command: BridgeCommand };
      try {
        parsed = parseCommandEnvelope(line);
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        const requestId = requestIdFromCommandLine(line);
        bridgeLogger.error({
          target: LOG_TARGETS.BRIDGE_PROTOCOL,
          eventName: "bridge_command_decode_failed",
          message: "failed to decode bridge command envelope",
          outcome: "failure",
          ...(requestId ? { requestId } : {}),
          sizeBytes: Buffer.byteLength(line),
          fields: {
            preview: line.slice(0, 240),
            preview_chars: Math.min(line.length, 240),
            error_message: message,
          },
        });
        failConnection(`invalid command envelope: ${message}`, requestId);
        return;
      }

      try {
        await handleCommand(parsed.command, parsed.requestId);
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        bridgeLogger.error({
          target: LOG_TARGETS.BRIDGE_PROTOCOL,
          eventName: "bridge_command_failed",
          message: "bridge command handler failed",
          outcome: "failure",
          ...(parsed.requestId ? { requestId: parsed.requestId } : {}),
          ...(parsed.command.command === "create_session" || parsed.command.command === "new_session"
            ? {}
            : "session_id" in parsed.command
              ? { sessionId: parsed.command.session_id }
              : {}),
          fields: {
            bridge_command: parsed.command.command,
            error_message: message,
          },
        });
        failConnection(
          `bridge command failed (${parsed.command.command}): ${message}`,
          parsed.requestId,
        );
      }
    })();
  });

  rl.on("close", () => {
    bridgeLogger.info({
      target: LOG_TARGETS.BRIDGE_LIFECYCLE,
      eventName: "bridge_input_closed",
      message: "bridge stdin closed",
      outcome: "success",
    });
    void closeAllSessions({ reason: "bridge_stdin_closed" }).finally(() => {
      bridgeLogger.info({
        target: LOG_TARGETS.BRIDGE_LIFECYCLE,
        eventName: "bridge_shutdown_completed",
        message: "bridge shutdown completed after stdin close",
        outcome: "success",
      });
      process.exit(0);
    });
  });
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main();
}
