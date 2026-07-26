import {
  getSessionMessages,
  renameSession,
} from "@anthropic-ai/claude-agent-sdk";
import type {
  Query,
  SessionMessage,
  SessionMutationOptions,
} from "@anthropic-ai/claude-agent-sdk";
import type {
  BridgeCommand,
  RewindTarget,
} from "../types.js";
import { mapSdkAccountInfo } from "./account_metadata.js";
import {
  emitSessionsList,
  setSessionListingDir,
  slashError,
  writeEvent,
} from "./events.js";
import { bridgeLogger, LOG_TARGETS } from "./logger.js";
import {
  refreshCurrentModel,
  sessionById,
  type SessionState,
} from "./session_lifecycle.js";

type SessionDataCommand = Extract<
  BridgeCommand,
  {
    command:
      | "generate_session_title"
      | "rename_session"
      | "get_status_snapshot"
      | "get_context_usage"
      | "get_rewind_targets"
      | "rewind";
  }
>;

export type SessionDataCommandDeps = {
  generatePersistedSessionTitle: (query: Query, description: string) => Promise<string>;
  buildSessionMutationOptions: (cwd?: string) => SessionMutationOptions | undefined;
  rewindTargetsFromSessionMessages: (messages: SessionMessage[]) => RewindTarget[];
  handleRewind: (
    command: Extract<BridgeCommand, { command: "rewind" }>,
    requestId?: string,
  ) => Promise<void>;
};

export async function handleSessionDataCommand(
  command: SessionDataCommand,
  requestId: string | undefined,
  deps: SessionDataCommandDeps,
): Promise<void> {
  switch (command.command) {
    case "generate_session_title":
      await generateTitle(command, requestId, deps);
      return;
    case "rename_session":
      await rename(command, requestId, deps);
      return;
    case "get_status_snapshot":
      await getStatusSnapshot(command, requestId);
      return;
    case "get_context_usage":
      await getContextUsage(command, requestId);
      return;
    case "get_rewind_targets":
      await getRewindTargets(command, requestId, deps);
      return;
    case "rewind":
      await deps.handleRewind(command, requestId);
  }
}

async function generateTitle(
  command: Extract<SessionDataCommand, { command: "generate_session_title" }>,
  requestId: string | undefined,
  deps: SessionDataCommandDeps,
): Promise<void> {
  const session = requireSession(command.session_id, requestId);
  if (!session) {
    return;
  }
  try {
    await deps.generatePersistedSessionTitle(session.query, command.description);
    setSessionListingDir(session.cwd);
    await emitSessionsList(requestId);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    slashError(command.session_id, `failed to generate session title: ${message}`, requestId);
  }
}

async function rename(
  command: Extract<SessionDataCommand, { command: "rename_session" }>,
  requestId: string | undefined,
  deps: SessionDataCommandDeps,
): Promise<void> {
  const session = requireSession(command.session_id, requestId);
  if (!session) {
    return;
  }
  try {
    await renameSession(
      command.session_id,
      command.title,
      deps.buildSessionMutationOptions(session.cwd),
    );
    setSessionListingDir(session.cwd);
    await emitSessionsList(requestId);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    slashError(command.session_id, `failed to rename session: ${message}`, requestId);
  }
}

async function getStatusSnapshot(
  command: Extract<SessionDataCommand, { command: "get_status_snapshot" }>,
  requestId: string | undefined,
): Promise<void> {
  const session = requireSession(command.session_id, requestId);
  if (!session) {
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
}

async function getContextUsage(
  command: Extract<SessionDataCommand, { command: "get_context_usage" }>,
  requestId: string | undefined,
): Promise<void> {
  const session = requireSession(command.session_id, requestId);
  if (!session) {
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
}

async function getRewindTargets(
  command: Extract<SessionDataCommand, { command: "get_rewind_targets" }>,
  requestId: string | undefined,
  deps: SessionDataCommandDeps,
): Promise<void> {
  const session = requireSession(command.session_id, requestId);
  if (!session) {
    return;
  }
  try {
    const historyMessages = await getSessionMessages(command.session_id, {
      dir: session.cwd,
      includeSystemMessages: true,
    });
    const targets = deps.rewindTargetsFromSessionMessages(historyMessages);
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
}

function requireSession(sessionId: string, requestId?: string): SessionState | null {
  const session = sessionById(sessionId);
  if (!session) {
    slashError(sessionId, `unknown session: ${sessionId}`, requestId);
  }
  return session;
}
