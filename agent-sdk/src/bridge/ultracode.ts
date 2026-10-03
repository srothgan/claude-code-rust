import type { Query } from "@anthropic-ai/claude-agent-sdk";
import type { UltracodeSnapshot } from "../types.js";
import { emitSessionUpdate } from "./events.js";
import { bridgeLogger, LOG_TARGETS } from "./logger.js";
import type { SessionState } from "./session_lifecycle.js";
import { readAppliedSettings } from "./query_settings.js";

export class UltracodeVerificationError extends Error {
  constructor(cause: unknown) {
    super("The SDK accepted the Ultracode change, but its resulting state could not be verified.", { cause });
  }
}

export async function readUltracodeState(query: Query): Promise<UltracodeSnapshot> {
  const applied = await readAppliedSettings(query);
  const available = applied?.ultracodeAvailable;
  const requested = applied?.ultracodeRequested;
  const effective = applied?.ultracode;
  if (
    typeof available !== "boolean" ||
    typeof requested !== "boolean" ||
    typeof effective !== "boolean" ||
    effective !== (available && requested)
  ) {
    throw new Error(`Invalid Ultracode settings: ${JSON.stringify({ available, requested, effective })}`);
  }
  return { available, requested, effective };
}

export function ultracodeError(error: unknown): string {
  const message = error instanceof Error ? error.message : String(error);
  if (message.includes("ultracode is not available for this session (dynamic workflows are off)")) {
    return "Cannot enable Ultracode: dynamic workflows are disabled for this session.";
  }
  const model = /ultracode is not available for this session \((.+) does not support it\)/.exec(message)?.[1];
  if (model) {
    return `Cannot enable Ultracode: ${model} does not support it.`;
  }
  if (message === "Applied settings are unavailable with the installed Agent SDK runtime.") {
    return message;
  }
  return "Cannot change Ultracode: an Agent SDK bridge/protocol error occurred.";
}

export async function applyUltracode(query: Query, enabled: boolean): Promise<UltracodeSnapshot> {
  await query.applyFlagSettings({ ultracode: enabled });
  try {
    const state = await readUltracodeState(query);
    // readUltracodeState owns consistency between requested, available and effective.
    if (state.requested !== enabled) {
      throw new Error(`Ultracode request mismatch: ${JSON.stringify(state)}`);
    }
    return state;
  } catch (error) {
    throw new UltracodeVerificationError(error);
  }
}

export function emitUltracodeUpdate(session: SessionState): void {
  emitSessionUpdate(session.sessionId, {
    type: "ultracode_update",
    ultracode: session.ultracode ?? null,
  });
}

export function logUltracodeFailure(session: SessionState, error: unknown): void {
  bridgeLogger.warn({
    target: LOG_TARGETS.APP_SESSION,
    eventName: "ultracode_verification_failed",
    message: "Ultracode state could not be verified",
    outcome: "failure",
    sessionId: session.sessionId,
    fields: {
      error_message: error instanceof Error ? error.message : String(error),
      cause: error instanceof Error && error.cause instanceof Error ? error.cause.message : undefined,
    },
  });
}

export async function refreshUltracode(session: SessionState, emit = true): Promise<boolean> {
  const sessionId = session.sessionId;
  let state: UltracodeSnapshot | undefined;
  try {
    state = await readUltracodeState(session.query);
  } catch (error) {
    logUltracodeFailure(session, error);
  }
  // A read from a closing or replaced query cannot establish current state.
  if (session.closing || session.sessionId !== sessionId) {
    return false;
  }
  session.ultracode = state;
  if (emit) {
    emitUltracodeUpdate(session);
  }
  return state !== undefined;
}
