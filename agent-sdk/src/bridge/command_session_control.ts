import type { Query, SDKUserMessage } from "@anthropic-ai/claude-agent-sdk";
import type { BridgeCommand, EffortLevel, FastModeSnapshot } from "../types.js";
import {
  buildModeState,
  markModeUnavailableForSession,
  permissionModeFailureLooksUnsupported,
  refreshSupportedModesForSession,
  toPermissionMode,
} from "./commands.js";
import { dispatchCancelTurnCommand } from "./command_dispatch.js";
import { emitFastModeUpdate } from "./error_classification.js";
import { emitSessionUpdate, slashError } from "./events.js";
import { bridgeLogger, LOG_TARGETS } from "./logger.js";
import {
  emitCurrentModelUpdate,
  refreshCurrentModel,
  sessionById,
  shouldInvalidateResolvedRuntimeModel,
  type SessionState,
} from "./session_lifecycle.js";

type SessionControlCommand = Extract<
  BridgeCommand,
  {
    command:
      | "prompt"
      | "cancel_turn"
      | "set_model"
      | "set_mode"
      | "set_effort"
      | "set_agent"
      | "set_fast_mode"
      | "reload_plugins";
  }
>;

export type SessionControlCommandDeps = {
  buildPromptUserMessage: (
    command: Extract<BridgeCommand, { command: "prompt" }>,
    sessionId: string,
  ) => SDKUserMessage | undefined;
  applySessionEffort: (query: Query, effort: EffortLevel) => Promise<void>;
  applySessionAgent: (query: Query, agent: string | null) => Promise<void>;
  applySessionFastMode: (
    query: Query,
    enabled: boolean,
  ) => Promise<FastModeSnapshot>;
  emitEffortConfigOptionUpdate: (
    sessionId: string,
    effort: EffortLevel,
  ) => void;
  emitAgentConfigOptionUpdate: (
    sessionId: string,
    agent: string | null,
  ) => void;
  handleReloadPluginsCommand: (
    session: SessionState,
    requestId?: string,
  ) => Promise<void>;
};

export async function handleSessionControlCommand(
  command: SessionControlCommand,
  requestId: string | undefined,
  deps: SessionControlCommandDeps,
): Promise<void> {
  switch (command.command) {
    case "prompt":
      handlePrompt(command, requestId, deps);
      return;
    case "cancel_turn":
      await dispatchCancelTurnCommand(command, {
        requestId,
        sessionById,
        slashError,
      });
      return;
    case "set_model":
      await setModel(command, requestId);
      return;
    case "set_mode":
      await setMode(command, requestId);
      return;
    case "set_effort":
      await setEffort(command, requestId, deps);
      return;
    case "set_agent":
      await setAgent(command, requestId, deps);
      return;
    case "set_fast_mode":
      await setFastMode(command, requestId, deps);
      return;
    case "reload_plugins":
      await reloadPlugins(command, requestId, deps);
  }
}

function handlePrompt(
  command: Extract<SessionControlCommand, { command: "prompt" }>,
  requestId: string | undefined,
  deps: SessionControlCommandDeps,
): void {
  const session = requireSession(command.session_id, requestId);
  if (!session) {
    return;
  }
  const message = deps.buildPromptUserMessage(command, session.sessionId);
  if (message) {
    session.input.enqueue(message);
  }
}

async function setModel(
  command: Extract<SessionControlCommand, { command: "set_model" }>,
  requestId: string | undefined,
): Promise<void> {
  const session = requireSession(command.session_id, requestId);
  if (!session) {
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
    const invalidatedResolvedRuntimeModel =
      shouldInvalidateResolvedRuntimeModel(
        previousRequestedModel,
        previousSessionModel,
        command.model,
      );
    if (invalidatedResolvedRuntimeModel) {
      session.resolvedRuntimeModelId = undefined;
    }
    const changed = refreshCurrentModel(session, true);
    const forcedCurrentModelUpdate =
      !changed && emitCurrentModelUpdate(session);
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
    slashError(
      command.session_id,
      `failed to set model: ${message}`,
      requestId,
    );
  }
}

async function setMode(
  command: Extract<SessionControlCommand, { command: "set_mode" }>,
  requestId: string | undefined,
): Promise<void> {
  const session = requireSession(command.session_id, requestId);
  if (!session) {
    return;
  }
  const mode = toPermissionMode(command.mode);
  if (!mode) {
    slashError(
      command.session_id,
      `unsupported mode: ${command.mode}`,
      requestId,
    );
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
    slashError(
      command.session_id,
      `failed to set mode to ${mode}: ${message}`,
      requestId,
    );
  }
}

async function setEffort(
  command: Extract<SessionControlCommand, { command: "set_effort" }>,
  requestId: string | undefined,
  deps: SessionControlCommandDeps,
): Promise<void> {
  const session = requireSession(command.session_id, requestId);
  if (!session) {
    return;
  }
  try {
    await deps.applySessionEffort(session.query, command.effort);
    deps.emitEffortConfigOptionUpdate(session.sessionId, command.effort);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    slashError(
      command.session_id,
      `failed to set effort: ${message}`,
      requestId,
    );
  }
}

async function setAgent(
  command: Extract<SessionControlCommand, { command: "set_agent" }>,
  requestId: string | undefined,
  deps: SessionControlCommandDeps,
): Promise<void> {
  const session = requireSession(command.session_id, requestId);
  if (!session) {
    return;
  }
  try {
    await deps.applySessionAgent(session.query, command.agent);
    deps.emitAgentConfigOptionUpdate(session.sessionId, command.agent);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    slashError(
      command.session_id,
      `failed to set agent: ${message}`,
      requestId,
    );
  }
}

async function setFastMode(
  command: Extract<SessionControlCommand, { command: "set_fast_mode" }>,
  requestId: string | undefined,
  deps: SessionControlCommandDeps,
): Promise<void> {
  const session = requireSession(command.session_id, requestId);
  if (!session) {
    return;
  }
  bridgeLogger.info({
    target: LOG_TARGETS.APP_SESSION,
    eventName: "set_fast_mode_started",
    message: "set fast mode started",
    outcome: "start",
    sessionId: session.sessionId,
    requestId,
    fields: {
      requested_enabled: command.enabled,
      previous_state: session.fastModeState,
    },
  });
  try {
    const snapshot = await deps.applySessionFastMode(
      session.query,
      command.enabled,
    );
    const state = snapshot.state;
    session.fastModeState = state;
    session.fastModeDisabledReason = snapshot.disabled_reason;
    emitFastModeUpdate(session);
    const reportedEnabled = state !== "off";
    if (reportedEnabled !== command.enabled) {
      bridgeLogger.warn({
        target: LOG_TARGETS.APP_SESSION,
        eventName: "set_fast_mode_mismatch",
        message:
          "SDK reported a fast-mode state that did not match the request",
        outcome: "failure",
        sessionId: session.sessionId,
        requestId,
        fields: {
          requested_enabled: command.enabled,
          reported_state: state,
        },
      });
      const action = command.enabled ? "enable" : "disable";
      slashError(
        command.session_id,
        `failed to ${action} fast mode: SDK reported state ${state}`,
        requestId,
      );
      return;
    }
    bridgeLogger.info({
      target: LOG_TARGETS.APP_SESSION,
      eventName: "set_fast_mode_succeeded",
      message: "set fast mode completed",
      outcome: "success",
      sessionId: session.sessionId,
      requestId,
      fields: {
        requested_enabled: command.enabled,
        reported_state: state,
      },
    });
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    bridgeLogger.warn({
      target: LOG_TARGETS.APP_SESSION,
      eventName: "set_fast_mode_failed",
      message: "set fast mode failed",
      outcome: "failure",
      sessionId: session.sessionId,
      requestId,
      fields: {
        requested_enabled: command.enabled,
        previous_state: session.fastModeState,
        error_message: message,
      },
    });
    slashError(
      command.session_id,
      `failed to set fast mode: ${message}`,
      requestId,
    );
  }
}

async function reloadPlugins(
  command: Extract<SessionControlCommand, { command: "reload_plugins" }>,
  requestId: string | undefined,
  deps: SessionControlCommandDeps,
): Promise<void> {
  const session = requireSession(command.session_id, requestId);
  if (session) {
    await deps.handleReloadPluginsCommand(session, requestId);
  }
}

function requireSession(
  sessionId: string,
  requestId?: string,
): SessionState | null {
  const session = sessionById(sessionId);
  if (!session) {
    slashError(sessionId, `unknown session: ${sessionId}`, requestId);
  }
  return session;
}
