import type { Query } from "@anthropic-ai/claude-agent-sdk";
import type { EffortLevel, FastModeSnapshot } from "../types.js";
import { parseFastModeDisabledReason, parseFastModeState } from "./state_parsing.js";
import { emitSessionUpdate } from "./events.js";

export class FastModeVerificationError extends Error {}

export async function applySessionEffort(
  query: Query,
  effort: EffortLevel | null,
  ultracodeEffective = false,
): Promise<void> {
  await query.applyFlagSettings({
    effortLevel: effort,
    ...(ultracodeEffective ? { ultracode: true } : {}),
  });
}

export async function applySessionFastMode(
  query: Query,
  enabled: boolean,
): Promise<FastModeSnapshot> {
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
    throw new FastModeVerificationError(`SDK accepted the fast-mode change but state verification failed: ${message}`);
  }

  const state = parseFastModeState(result.fast_mode_state);
  if (state) {
    const disabledReason = parseFastModeDisabledReason(
      result.fast_mode_disabled_reason,
    );
    return {
      state,
      ...(disabledReason ? { disabled_reason: disabledReason } : {}),
    };
  }
  if (!enabled && result.fast_mode_state === undefined) {
    return { state: "off" };
  }
  throw new FastModeVerificationError("SDK accepted the fast-mode change but did not report its resulting state");
}

export async function applySessionAgent(
  query: Query,
  agent: string | null,
): Promise<void> {
  await query.applyFlagSettings({ agent });
}

export function emitAgentConfigOptionUpdate(
  sessionId: string,
  agent: string | null,
): void {
  emitSessionUpdate(sessionId, {
    type: "config_option_update",
    option_id: "agent",
    value: agent,
  });
}

