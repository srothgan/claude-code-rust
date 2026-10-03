import type { Query } from "@anthropic-ai/claude-agent-sdk";
import { EFFORT_LEVELS, type EffortLevel } from "../types.js";
import { emitSessionUpdate } from "./events.js";
import { bridgeLogger, LOG_TARGETS } from "./logger.js";
import { readAppliedSettings } from "./query_settings.js";
import type { SessionState } from "./session_lifecycle.js";

export function isEffortLevel(value: unknown): value is EffortLevel {
  return typeof value === "string" && EFFORT_LEVELS.some(level => level === value);
}

function resolvedEffort(value: unknown): EffortLevel | null {
  if (value === null || isEffortLevel(value)) return value;
  throw new Error("The Agent SDK did not report a valid applied effort level.");
}

export async function readSessionEffort(query: Query): Promise<EffortLevel | null> {
  return resolvedEffort((await readAppliedSettings(query)).effort);
}

export function emitEffortConfigOptionUpdate(sessionId: string, effort: EffortLevel | null): void {
  emitSessionUpdate(sessionId, { type: "config_option_update", option_id: "effortLevel", value: effort });
}

// Observations may arrive while a control read is in flight. A later init message wins.
const observations = new WeakMap<SessionState, number>();
function beginObservation(session: SessionState): number {
  const generation = (observations.get(session) ?? 0) + 1;
  observations.set(session, generation);
  return generation;
}

export function observeSessionEffort(session: SessionState, value: unknown): void {
  if (value !== null && !isEffortLevel(value)) return;
  beginObservation(session);
  emitEffortConfigOptionUpdate(session.sessionId, value);
}

export async function refreshSessionEffort(session: SessionState): Promise<void> {
  const sessionId = session.sessionId;
  const generation = beginObservation(session);
  let effort: EffortLevel | null = null;
  try {
    effort = await readSessionEffort(session.query);
  } catch (error) {
    bridgeLogger.warn({ target: LOG_TARGETS.APP_SESSION, eventName: "effort_verification_failed", message: "Applied effort could not be verified", outcome: "failure", sessionId, fields: { error_message: error instanceof Error ? error.message : String(error) } });
  }
  if (!session.closing && session.sessionId === sessionId && observations.get(session) === generation) {
    emitEffortConfigOptionUpdate(sessionId, effort);
  }
}
