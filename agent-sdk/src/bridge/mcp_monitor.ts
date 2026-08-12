import { setTimeout as delay } from "node:timers/promises";

export type McpAuthMonitorPollResult = "continue" | "complete";

export type McpAuthMonitorResult =
  | { outcome: "cancelled"; attempts: number }
  | { outcome: "completed"; attempts: number }
  | {
      outcome: "exhausted";
      attempts: number;
      reason: "error" | "status";
      lastError?: string;
    };

export type McpAuthMonitorHandle = {
  controller: AbortController;
  task: Promise<McpAuthMonitorResult>;
};

export type McpAuthMonitorTiming = {
  maxAttempts?: number;
  initialDelayMs?: number;
  maxDelayMs?: number;
  sleep?: (delayMs: number, signal: AbortSignal) => Promise<void>;
};

type McpAuthMonitorParams = McpAuthMonitorTiming & {
  signal: AbortSignal;
  poll: () => Promise<McpAuthMonitorPollResult>;
};

const DEFAULT_MAX_ATTEMPTS = 24;
const DEFAULT_INITIAL_DELAY_MS = 1_000;
const DEFAULT_MAX_DELAY_MS = 10_000;

async function abortableDelay(
  delayMs: number,
  signal: AbortSignal,
): Promise<void> {
  await delay(delayMs, undefined, { signal, ref: false });
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export async function runMcpAuthMonitor({
  signal,
  poll,
  maxAttempts = DEFAULT_MAX_ATTEMPTS,
  initialDelayMs = DEFAULT_INITIAL_DELAY_MS,
  maxDelayMs = DEFAULT_MAX_DELAY_MS,
  sleep = abortableDelay,
}: McpAuthMonitorParams): Promise<McpAuthMonitorResult> {
  let nextDelayMs = initialDelayMs;
  let lastError: string | undefined;

  for (let attempt = 1; attempt <= maxAttempts; attempt += 1) {
    try {
      await sleep(nextDelayMs, signal);
    } catch (error) {
      if (signal.aborted) {
        return { outcome: "cancelled", attempts: attempt - 1 };
      }
      lastError = errorMessage(error);
      if (attempt === maxAttempts) {
        return {
          outcome: "exhausted",
          attempts: attempt,
          reason: "error",
          lastError,
        };
      }
      nextDelayMs = Math.min(nextDelayMs * 2, maxDelayMs);
      continue;
    }

    if (signal.aborted) {
      return { outcome: "cancelled", attempts: attempt - 1 };
    }

    try {
      const result = await poll();
      if (signal.aborted) {
        return { outcome: "cancelled", attempts: attempt };
      }
      if (result === "complete") {
        return { outcome: "completed", attempts: attempt };
      }
      lastError = undefined;
    } catch (error) {
      if (signal.aborted) {
        return { outcome: "cancelled", attempts: attempt };
      }
      lastError = errorMessage(error);
    }

    if (attempt === maxAttempts) {
      return {
        outcome: "exhausted",
        attempts: attempt,
        reason: lastError ? "error" : "status",
        ...(lastError ? { lastError } : {}),
      };
    }
    nextDelayMs = Math.min(nextDelayMs * 2, maxDelayMs);
  }

  return { outcome: "exhausted", attempts: 0, reason: "status" };
}
