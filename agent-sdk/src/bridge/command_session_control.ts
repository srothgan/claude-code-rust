import type { Query, SDKUserMessage } from "@anthropic-ai/claude-agent-sdk";
import type { BridgeCommand, EffortLevel, FastModeSnapshot } from "../types.js";
import {
  buildModeState,
  markModeUnavailableForSession,
  permissionModeFailureLooksUnsupported,
  availableModesForSession,
  refreshSupportedModesForSession,
  toPermissionMode,
  beginSessionModeRead,
  observeSessionMode,
} from "./commands.js";
import { dispatchCancelTurnCommand } from "./command_dispatch.js";
import { beginFastModeRead, emitFastModeUpdate } from "./error_classification.js";
import { emitSessionUpdate, slashError, writeEvent } from "./events.js";
import { bridgeLogger, LOG_TARGETS } from "./logger.js";
import { refreshSessionEffort } from "./effort.js";
import { refreshSessionModel } from "./session_model.js";
import { readQuerySettings } from "./query_settings.js";
import { asRecordOrNull } from "./shared.js";
import { FastModeVerificationError } from "./session_preferences.js";
import { SessionObservations } from "./session_observations.js";

const thinkingObservations = new SessionObservations();
import {
  applyUltracode,
  emitUltracodeUpdate,
  logUltracodeFailure,
  readUltracodeState,
  refreshUltracode,
  beginUltracodeRead,
  ultracodeError,
  UltracodeVerificationError,
} from "./ultracode.js";
import {
  sessionById,
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
      | "set_thinking"
      | "set_agent"
      | "set_ultracode"
      | "refresh_ultracode"
      | "set_fast_mode"
      | "reload_plugins";
  }
>;

export type SessionControlCommandDeps = {
  buildPromptUserMessage: (
    command: Extract<BridgeCommand, { command: "prompt" }>,
    sessionId: string,
  ) => SDKUserMessage | undefined;
  applySessionEffort: (
    query: Query,
    effort: EffortLevel | null,
    ultracodeEffective?: boolean,
  ) => Promise<void>;
  applySessionAgent: (query: Query, agent: string | null) => Promise<void>;
  applySessionFastMode: (
    query: Query,
    enabled: boolean,
  ) => Promise<FastModeSnapshot>;
  emitAgentConfigOptionUpdate: (
    sessionId: string,
    agent: string | null,
  ) => void;
  handleReloadPluginsCommand: (
    session: SessionState,
    requestId?: string,
    force?: boolean,
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
    case "set_thinking":
      await setThinking(command, requestId);
      return;
    case "set_agent":
      await setAgent(command, requestId, deps);
      return;
    case "set_ultracode":
    case "refresh_ultracode":
      await handleUltracode(command, requestId);
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
    writeEvent(
      {
        event: "user_message_rejected",
        session_id: command.session_id,
        message_uuid: command.message_uuid,
        reason: "unknown session",
      },
      requestId,
    );
    return;
  }
  const message = deps.buildPromptUserMessage(command, session.sessionId);
  if (!message) {
    writeEvent(
      {
        event: "user_message_rejected",
        session_id: session.sessionId,
        message_uuid: command.message_uuid,
        reason: "prompt contained no supported content",
      },
      requestId,
    );
    return;
  }
  if (!session.input.enqueue(message)) {
    writeEvent(
      {
        event: "user_message_rejected",
        session_id: session.sessionId,
        message_uuid: command.message_uuid,
        reason: "session input is closed",
      },
      requestId,
    );
    return;
  }
  writeEvent(
    {
      event: "user_message_queued",
      session_id: session.sessionId,
      message_uuid: command.message_uuid,
    },
    requestId,
  );
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
      previous_resolved_runtime_model: session.resolvedRuntimeModelId,
      previous_current_model: session.currentModel?.resolved_id,
    },
  });
  try {
    if (!session.availableModels.some(model => model.id === command.model)) throw new Error("Choose an available model.");
    await session.query.setModel(command.model);
    if (session.closing) return;
    session.requestedModelId = command.model;
    await refreshModelControls(session);
    bridgeLogger.info({
      target: LOG_TARGETS.APP_SESSION,
      eventName: "set_model_succeeded",
      message: "set model completed",
      outcome: "success",
      sessionId: session.sessionId,
      requestId,
      fields: {
        requested_model: command.model,
        resolved_runtime_model_after: session.resolvedRuntimeModelId,
        current_model_after: session.currentModel?.resolved_id,
        current_model_display_short: session.currentModel?.display_name_short,
        current_model_display_long: session.currentModel?.display_name_long,
      },
    });
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
    refreshSupportedModesForSession(session);
    if (!availableModesForSession(session).some(entry => entry.id === mode)) throw new Error("Choose an available permission mode.");
    const current = beginSessionModeRead(session);
    await session.query.setPermissionMode(mode);
    if (session.closing || session.sessionId !== command.session_id) return;
    if (current()) observeSessionMode(session, mode);
    refreshSupportedModesForSession(session);
    emitSessionUpdate(session.sessionId, {
      type: "mode_state_update",
      mode: buildModeState(session, session.mode),
    });
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    if (permissionModeFailureLooksUnsupported(mode, message)) {
      const changed = markModeUnavailableForSession(session, mode);
      if (changed) {
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
    const model = session.currentModel;
    if (command.effort !== null && (!model?.is_authoritative || !model.supports_effort || !model.supported_effort_levels.includes(command.effort))) throw new Error("Choose an effort level supported by the current model.");
    await deps.applySessionEffort(
      session.query,
      command.effort,
      session.ultracode?.effective === true,
    );
    await refreshUltracode(session);
    if (!await refreshSessionEffort(session)) slashError(session.sessionId, "The effort change was accepted, but its applied level could not be verified.", requestId);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    slashError(
      command.session_id,
      `failed to set effort: ${message}`,
      requestId,
    );
    await refreshSessionEffort(session);
  }
}

async function setThinking(
  command: Extract<SessionControlCommand, { command: "set_thinking" }>,
  requestId: string | undefined,
): Promise<void> {
  const session = requireSession(command.session_id, requestId);
  if (!session) return;
  const current = thinkingObservations.begin(session);
  let accepted = false;
  try {
    await session.query.applyFlagSettings({ alwaysThinkingEnabled: command.enabled });
    accepted = true;
    const settings = await readQuerySettings(session.query);
    const effective = asRecordOrNull(settings.effective);
    if (!effective) throw new Error("The thinking preference could not be verified.");
    const value = effective.alwaysThinkingEnabled;
    if (value !== undefined && typeof value !== "boolean") throw new Error("The thinking preference could not be verified.");
    if (current()) emitSessionUpdate(session.sessionId, { type: "config_option_update", option_id: "alwaysThinkingEnabled", value: value ?? null });
  } catch (error) {
    if (!current()) return;
    if (accepted) emitSessionUpdate(session.sessionId, { type: "config_option_update", option_id: "alwaysThinkingEnabled", value: null });
    slashError(session.sessionId, `Failed to change thinking: ${String(error)}`, requestId);
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
    if (command.agent !== null && !session.availableAgents?.some(agent => agent.name === command.agent)) throw new Error("Choose an available agent.");
    await deps.applySessionAgent(session.query, command.agent);
    if (session.closing) return;
    deps.emitAgentConfigOptionUpdate(session.sessionId, command.agent);
    await refreshModelControls(session);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    slashError(
      command.session_id,
      `failed to set agent: ${message}`,
      requestId,
    );
  }
}

async function handleUltracode(
  command: Extract<SessionControlCommand, { command: "set_ultracode" | "refresh_ultracode" }>,
  requestId: string | undefined,
): Promise<void> {
  const session = sessionById(command.session_id);
  if (!session) {
    slashError(command.session_id, "Cannot change Ultracode: no active session.", requestId);
    return;
  }
  const current = beginUltracodeRead(session);
  if (command.command === "refresh_ultracode") {
    try {
      const state = await readUltracodeState(session.query);
      if (!current()) return;
      session.ultracode = state;
      emitUltracodeUpdate(session);
    } catch (error) {
      if (!current()) return;
      session.ultracode = undefined;
      logUltracodeFailure(session, error);
      emitUltracodeUpdate(session);
      slashError(command.session_id, ultracodeError(error), requestId);
    }
    return;
  }
  try {
    const state = await applyUltracode(session.query, command.enabled);
    if (!current()) return;
    session.ultracode = state;
    emitUltracodeUpdate(session);
    await refreshSessionEffort(session);
    if (command.enabled && !session.ultracode.effective) {
      slashError(
        command.session_id,
        "Cannot enable Ultracode: unavailable for this session. The SDK saved the request, but Ultracode remains inactive.",
        requestId,
      );
    }
  } catch (error) {
    if (!current()) return;
    logUltracodeFailure(session, error);
    // An accepted change with a failed read invalidates the previous snapshot.
    if (error instanceof UltracodeVerificationError) {
      session.ultracode = undefined;
      emitUltracodeUpdate(session);
      slashError(command.session_id, error.message, requestId);
    } else {
      slashError(command.session_id, ultracodeError(error), requestId);
    }
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
  const sessionId = session.sessionId;
  const current = beginFastModeRead(session);
  try {
    if (command.enabled && session.currentModel?.supports_fast_mode === false) throw new Error("The current model does not support fast mode.");
    const snapshot = await deps.applySessionFastMode(
      session.query,
      command.enabled,
    );
    if (session.closing || session.sessionId !== sessionId) return;
    if (current()) {
      session.fastModeState = snapshot.state;
      session.fastModeDisabledReason = snapshot.disabled_reason;
    }
    const state = session.fastModeState;
    emitFastModeUpdate(session);
    await refreshSessionEffort(session);
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
    if (session.closing || session.sessionId !== sessionId) return;
    if (!current()) emitFastModeUpdate(session);
    // An accepted change with failed verification cannot leave the previous badge active.
    if (error instanceof FastModeVerificationError && current()) {
      session.fastModeState = "unknown";
      session.fastModeDisabledReason = undefined;
      emitFastModeUpdate(session);
    }
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
    await deps.handleReloadPluginsCommand(session, requestId, command.force);
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

async function refreshModelControls(session: SessionState): Promise<void> {
  let failure: unknown;
  try { await refreshSessionModel(session); } catch (error) { failure = error; }
  await refreshUltracode(session);
  await refreshSessionEffort(session);
  refreshSupportedModesForSession(session);
  emitSessionUpdate(session.sessionId, { type: "mode_state_update", mode: buildModeState(session, session.mode) });
  if (failure) throw failure;
}
