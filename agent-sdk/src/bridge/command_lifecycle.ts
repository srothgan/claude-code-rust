import {
  getSessionMessages,
  listSessions,
} from "@anthropic-ai/claude-agent-sdk";
import type { BridgeCommand } from "../types.js";
import {
  currentSessionListOptions,
  emitSessionsList,
  failConnection,
  setSessionListingDir,
  slashError,
  writeEvent,
} from "./events.js";
import { mapSessionMessagesToUpdates } from "./history.js";
import { bridgeLogger, LOG_TARGETS } from "./logger.js";
import {
  closeAllSessions,
  createSession,
  sessions,
} from "./session_lifecycle.js";

type LifecycleCommand = Extract<
  BridgeCommand,
  {
    command:
      | "initialize"
      | "create_session"
      | "resume_session"
      | "new_session"
      | "shutdown";
  }
>;

export async function handleLifecycleCommand(
  command: LifecycleCommand,
  requestId: string | undefined,
  sdkVersionError: string | undefined,
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
    case "new_session":
      await replace(command, requestId);
      return;
    case "shutdown":
      await shutdown(requestId);
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
