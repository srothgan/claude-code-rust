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
import type { BridgeCommand } from "./types.js";
import { parseCommandEnvelope, toPermissionMode } from "./bridge/commands.js";
import {
  writeEvent,
  failConnection,
  slashError,
  emitSessionUpdate,
  emitSessionsList,
  currentSessionListOptions,
  setSessionListingDir,
} from "./bridge/events.js";
import { textFromPrompt, contentFromPrompt } from "./bridge/message_handlers.js";
import {
  sessions,
  sessionById,
  createSession,
  closeSession,
  closeAllSessions,
  handleElicitationResponse,
  handlePermissionResponse,
  handleQuestionResponse,
} from "./bridge/session_lifecycle.js";
import { mapSessionMessagesToUpdates } from "./bridge/history.js";
import {
  MCP_STALE_STATUS_REVALIDATION_COOLDOWN_MS,
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

// Re-exports: all symbols that tests and external consumers import from bridge.js.
export { AsyncQueue } from "./bridge/shared.js";
export { asRecordOrNull } from "./bridge/shared.js";
export { CACHE_SPLIT_POLICY, previewKilobyteLabel } from "./bridge/cache_policy.js";
export {
  buildToolResultFields,
  createToolCall,
  normalizeToolKind,
  normalizeToolResultText,
  unwrapToolUseResult,
} from "./bridge/tooling.js";
export { looksLikeAuthRequired } from "./bridge/auth.js";
export { parseCommandEnvelope } from "./bridge/commands.js";
export { buildSessionListOptions } from "./bridge/events.js";
export {
  permissionOptionsFromSuggestions,
  permissionResultFromOutcome,
} from "./bridge/permissions.js";
export {
  mapSessionMessagesToUpdates,
  mapSdkSessions,
} from "./bridge/history.js";
export { handleTaskSystemMessage } from "./bridge/message_handlers.js";
export { mapAvailableAgents } from "./bridge/agents.js";
export { buildQueryOptions, mapAvailableModels } from "./bridge/session_lifecycle.js";
export {
  parseFastModeState,
  parseRateLimitStatus,
  buildRateLimitUpdate,
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

const EXPECTED_AGENT_SDK_VERSION = "0.2.74";
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
          matched.cwd ? { dir: matched.cwd } : undefined,
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
      session.input.enqueue({
        type: "user",
        session_id: session.sessionId,
        parent_tool_use_id: null,
        message: {
          role: "user",
          content,
        },
      } as import("@anthropic-ai/claude-agent-sdk").SDKUserMessage);
      return;
    }

    case "cancel_turn": {
      const session = sessionById(command.session_id);
      if (!session) {
        slashError(command.session_id, `unknown session: ${command.session_id}`, requestId);
        return;
      }
      await session.query.interrupt();
      return;
    }

    case "set_model": {
      const session = sessionById(command.session_id);
      if (!session) {
        slashError(command.session_id, `unknown session: ${command.session_id}`, requestId);
        return;
      }
      await session.query.setModel(command.model);
      session.model = command.model;
      emitSessionUpdate(session.sessionId, {
        type: "config_option_update",
        option_id: "model",
        value: command.model,
      });
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
      await session.query.setPermissionMode(mode);
      session.mode = mode;
      emitSessionUpdate(session.sessionId, {
        type: "current_mode_update",
        current_mode_id: mode,
      });
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
          },
        });
        writeEvent(
          {
            event: "status_snapshot",
            session_id: session.sessionId,
            account: {
              email: account.email,
              organization: account.organization,
              subscription_type: account.subscriptionType,
              token_source: account.tokenSource,
              api_key_source: account.apiKeySource,
            },
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
        bridgeLogger.error({
          target: LOG_TARGETS.BRIDGE_PROTOCOL,
          eventName: "bridge_command_decode_failed",
          message: "failed to decode bridge command envelope",
          outcome: "failure",
          sizeBytes: Buffer.byteLength(line),
          fields: {
            preview: line.slice(0, 240),
            preview_chars: Math.min(line.length, 240),
            error_message: message,
          },
        });
        failConnection(`invalid command envelope: ${message}`);
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
