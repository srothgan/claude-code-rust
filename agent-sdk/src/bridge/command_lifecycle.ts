import {
  getSessionMessages,
  listSessions,
} from "@anthropic-ai/claude-agent-sdk";
import type { BridgeCommand } from "../types.js";
import {
  currentSessionListOptions,
  emitSessionResumeFailed,
  emitSessionsList,
  failConnection,
  setSessionListingDir,
  slashError,
  writeEvent,
} from "./events.js";
import { mapSessionMessagesToUpdates } from "./history.js";
import { bridgeLogger, LOG_TARGETS } from "./logger.js";
import {
  awaitSessionInitialization,
  closeAllSessions,
  closeSessionWithLogging,
  commitDeferredSession,
  createSession,
  detachSessionForClose,
  sessions,
} from "./session_lifecycle.js";
import type { RewindConversationPlan } from "../bridge.js";

type LifecycleCommand = Extract<
  BridgeCommand,
  {
    command:
      | "initialize"
      | "create_session"
      | "resume_session"
      | "resume_session_at"
      | "new_session"
      | "shutdown";
  }
>;

export async function handleLifecycleCommand(
  command: LifecycleCommand,
  requestId: string | undefined,
  sdkVersionError: string | undefined,
  deps: {
    buildRewindConversationPlan: (
      messages: import("@anthropic-ai/claude-agent-sdk").SessionMessage[],
      targetUserMessageId: string,
    ) => RewindConversationPlan | null;
  },
): Promise<void> {
  switch (command.command) {
    case "initialize":
      await initialize(command, requestId, sdkVersionError);
      return;
    case "create_session":
      await create(command, requestId);
      return;
    case "resume_session":
      await resume(command, requestId);
      return;
    case "resume_session_at":
      await resumeAt(command, requestId, deps.buildRewindConversationPlan);
      return;
    case "new_session":
      await replace(command, requestId);
      return;
    case "shutdown":
      await shutdown(requestId);
  }
}

async function resumeAt(
  command: Extract<LifecycleCommand, { command: "resume_session_at" }>,
  requestId: string | undefined,
  buildPlan: (
    messages: import("@anthropic-ai/claude-agent-sdk").SessionMessage[],
    targetUserMessageId: string,
  ) => RewindConversationPlan | null,
): Promise<void> {
  if (!requestId) {
    slashError(
      command.session_id,
      "resume before a selected message requires an operation id",
    );
    return;
  }
  const targetUserMessageId = command.target_user_message_id.trim();
  if (!targetUserMessageId) {
    emitSessionResumeFailed(
      command.session_id,
      requestId,
      "resume target cannot be empty",
    );
    return;
  }

  let candidate: import("./session_lifecycle.js").SessionState | undefined;
  try {
    const sdkSessions = await listSessions(currentSessionListOptions());
    const matched = sdkSessions.find(
      (entry) => entry.sessionId === command.session_id,
    );
    if (!matched) {
      emitSessionResumeFailed(
        command.session_id,
        requestId,
        `unknown session: ${command.session_id}`,
      );
      return;
    }
    const cwd = matched.cwd ?? process.cwd();
    setSessionListingDir(cwd);
    const historyMessages = await getSessionMessages(command.session_id, {
      dir: cwd,
      includeSystemMessages: true,
    });
    const plan = buildPlan(historyMessages, targetUserMessageId);
    if (!plan) {
      throw new Error(
        `stale or inconsistent resume target: ${targetUserMessageId}`,
      );
    }

    const staleSessions = Array.from(sessions.values());
    const connectEvent =
      staleSessions.length > 0 ? "session_replaced" : "connected";
    candidate = plan.resumeSessionAtUuid
      ? await createSession({
          cwd,
          resume: command.session_id,
          resumeSessionAt: plan.resumeSessionAtUuid,
          resumeDropsTurn: plan.resumeDropsTurnId,
          forkSession: true,
          launchSettings: command.launch_settings,
          connectEvent,
          requestId,
          deferConnect: true,
          resumeUpdates: plan.resumeUpdates,
          sessionsToCloseAfterConnect: staleSessions,
        })
      : await createSession({
          cwd,
          launchSettings: command.launch_settings,
          connectEvent,
          requestId,
          deferConnect: true,
          sessionsToCloseAfterConnect: staleSessions,
        });

    await awaitSessionInitialization(candidate);
    commitDeferredSession(candidate);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    if (candidate) {
      detachSessionForClose(candidate);
      await closeSessionWithLogging(candidate, {
        reason: "resume_at_candidate_rejected",
        requestId,
      });
    }
    const userMessage = message.startsWith(
      "Resume rejected by --resume-drops-turn:",
    )
      ? "The session changed while you were selecting a message, so Claude Code refused to discard the newer turn. Reopen Resume and try again."
      : `Failed to fork before selected message: ${message}`;
    emitSessionResumeFailed(command.session_id, requestId, userMessage);
  }
}

async function initialize(
  command: Extract<LifecycleCommand, { command: "initialize" }>,
  requestId: string | undefined,
  sdkVersionError: string | undefined,
): Promise<void> {
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
}

async function create(
  command: Extract<LifecycleCommand, { command: "create_session" }>,
  requestId: string | undefined,
): Promise<void> {
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
}

async function resume(
  command: Extract<LifecycleCommand, { command: "resume_session" }>,
  requestId: string | undefined,
): Promise<void> {
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
    const matched = sdkSessions.find(
      (entry) => entry.sessionId === command.session_id,
    );
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
      slashError(
        command.session_id,
        `unknown session: ${command.session_id}`,
        requestId,
      );
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
      ...(hadActiveSession
        ? { sessionsToCloseAfterConnect: staleSessions }
        : {}),
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
    slashError(
      command.session_id,
      `failed to resume session: ${message}`,
      requestId,
    );
  }
}

async function replace(
  command: Extract<LifecycleCommand, { command: "new_session" }>,
  requestId: string | undefined,
): Promise<void> {
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
}

async function shutdown(requestId: string | undefined): Promise<void> {
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
}
