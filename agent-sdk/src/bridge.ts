import { createRequire } from "node:module";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import readline from "node:readline";
import { pathToFileURL } from "node:url";
import { getSessionMessages } from "@anthropic-ai/claude-agent-sdk";
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
import { parseCommandEnvelope } from "./bridge/commands.js";
import {
  parseFastModeDisabledReason,
  parseFastModeState,
} from "./bridge/state_parsing.js";
import {
  writeEvent,
  failConnection,
  slashError,
  emitRuntimeReloadCompleted,
  emitRuntimeReloadFailed,
  emitSessionUpdate,
  setSessionListingDir,
} from "./bridge/events.js";
import { contentFromPrompt } from "./bridge/message_handlers.js";
import {
  sessions,
  sessionById,
  createSession,
  closeAllSessions,
  type PendingRewindResult,
  type SessionState,
} from "./bridge/session_lifecycle.js";
import { mapSessionMessagesToUpdates } from "./bridge/history.js";
import { emitAvailableAgentsIfChanged, mapAvailableAgents } from "./bridge/agents.js";
import { mapSdkSlashCommands, updateAvailableCommands } from "./bridge/available_commands.js";
import {
  MCP_STALE_STATUS_REVALIDATION_COOLDOWN_MS,
  emitReconciledMcpSnapshotFromStatuses,
  staleMcpAuthCandidates,
} from "./bridge/mcp.js";
import { bridgeLogger, LOG_TARGETS, logBridgeCommandReceived } from "./bridge/logger.js";
import { BridgeCommandScheduler } from "./bridge/command_scheduler.js";
import { handleLifecycleCommand } from "./bridge/command_lifecycle.js";
import { handleInteractionCommand } from "./bridge/command_interactions.js";
import { handleMcpCommand } from "./bridge/command_mcp.js";
import { handleSessionControlCommand } from "./bridge/command_session_control.js";
import { handleSessionDataCommand } from "./bridge/command_session_data.js";

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
  await query.applyFlagSettings({ effortLevel: effort });
}

export async function applySessionFastMode(
  query: import("@anthropic-ai/claude-agent-sdk").Query,
  enabled: boolean,
): Promise<import("./types.js").FastModeSnapshot> {
  try {
    await query.applyFlagSettings({ fastMode: enabled });
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    throw new Error(`SDK rejected the fast-mode change: ${message}`);
  }

  let result: import("@anthropic-ai/claude-agent-sdk").SDKControlInitializeResponse;
  try {
    result = await query.reinitialize();
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    throw new Error(`SDK accepted the fast-mode change but state verification failed: ${message}`);
  }

  const state = parseFastModeState(result.fast_mode_state);
  if (state) {
    const disabledReason = parseFastModeDisabledReason(result.fast_mode_disabled_reason);
    return {
      state,
      ...(disabledReason ? { disabled_reason: disabledReason } : {}),
    };
  }
  if (!enabled && result.fast_mode_state === undefined) {
    return { state: "off" };
  }
  throw new Error("SDK accepted the fast-mode change but did not report its resulting state");
}

export function buildPromptUserMessage(
  command: Extract<BridgeCommand, { command: "prompt" }>,
  sessionId: string,
): import("@anthropic-ai/claude-agent-sdk").SDKUserMessage | undefined {
  const content = contentFromPrompt(command);
  if (content.length === 0) {
    return undefined;
  }
  return {
    type: "user",
    session_id: sessionId,
    parent_tool_use_id: null,
    origin: { kind: "human" },
    message: {
      role: "user",
      content,
    },
  };
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

const EXPECTED_AGENT_SDK_VERSION = "0.3.220";
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

export function mapRewindFilesResult(result: SdkRewindFilesResult): RewindFilesResult {
  const skippedLinks =
    Number.isFinite(result.skippedLinks) &&
    Number.isInteger(result.skippedLinks) &&
    (result.skippedLinks ?? -1) >= 0
      ? result.skippedLinks
      : undefined;
  return {
    can_rewind: result.canRewind,
    ...(result.error ? { error: result.error } : {}),
    files_changed: result.filesChanged ?? [],
    ...(result.insertions !== undefined ? { insertions: result.insertions } : {}),
    ...(result.deletions !== undefined ? { deletions: result.deletions } : {}),
    ...(skippedLinks !== undefined ? { skipped_links: skippedLinks } : {}),
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
    case "create_session":
    case "resume_session":
    case "new_session":
    case "shutdown":
      await handleLifecycleCommand(command, requestId, sdkVersionError);
      return;
    case "prompt":
    case "cancel_turn":
    case "set_model":
    case "set_mode":
    case "set_effort":
    case "set_agent":
    case "set_fast_mode":
    case "reload_plugins":
      await handleSessionControlCommand(command, requestId, {
        buildPromptUserMessage,
        applySessionEffort,
        applySessionAgent,
        applySessionFastMode,
        emitEffortConfigOptionUpdate,
        emitAgentConfigOptionUpdate,
        handleReloadPluginsCommand,
      });
      return;
    case "generate_session_title":
    case "rename_session":
    case "get_status_snapshot":
    case "get_context_usage":
    case "get_rewind_targets":
    case "rewind":
      await handleSessionDataCommand(command, requestId, {
        generatePersistedSessionTitle,
        buildSessionMutationOptions,
        rewindTargetsFromSessionMessages,
        handleRewind,
      });
      return;
    case "mcp_status":
    case "mcp_reconnect":
    case "mcp_toggle":
    case "mcp_set_servers":
    case "mcp_authenticate":
    case "mcp_clear_auth":
    case "mcp_oauth_callback_url":
      await handleMcpCommand(command, requestId);
      return;
    case "permission_response":
    case "question_response":
    case "user_dialog_response":
    case "elicitation_response":
      handleInteractionCommand(command);
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
      failConnection(
        `unhandled command: ${(command as { command?: string }).command ?? "unknown"}`,
        requestId,
      );
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
  const commandScheduler = new BridgeCommandScheduler();

  rl.on("line", (line) => {
    if (line.trim().length === 0) {
      return;
    }
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

    commandScheduler.schedule(parsed.command, async () => {
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
          ...(parsed.command.command === "create_session" ||
          parsed.command.command === "new_session"
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
    });
  });

  rl.on("close", () => {
    commandScheduler.stopAccepting();
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
