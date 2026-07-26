import assert from "node:assert/strict";
import test from "node:test";
import { runMcpAuthMonitor } from "./mcp_monitor.js";

test("MCP auth monitor uses bounded backoff and reports status exhaustion once", async () => {
  const delays: number[] = [];
  let polls = 0;

  const result = await runMcpAuthMonitor({
    signal: new AbortController().signal,
    maxAttempts: 4,
    initialDelayMs: 100,
    maxDelayMs: 250,
    sleep: async (delayMs) => {
      delays.push(delayMs);
    },
    poll: async () => {
      polls += 1;
      return "continue";
    },
  });

  assert.deepEqual(delays, [100, 200, 250, 250]);
  assert.equal(polls, 4);
  assert.deepEqual(result, {
    outcome: "exhausted",
    attempts: 4,
    reason: "status",
  });
});

test("MCP auth monitor reports the final polling error at exhaustion", async () => {
  let polls = 0;

  const result = await runMcpAuthMonitor({
    signal: new AbortController().signal,
    maxAttempts: 3,
    sleep: async () => undefined,
    poll: async () => {
      polls += 1;
      throw new Error(`failure-${polls}`);
    },
  });

  assert.equal(polls, 3);
  assert.deepEqual(result, {
    outcome: "exhausted",
    attempts: 3,
    reason: "error",
    lastError: "failure-3",
  });
});

test("MCP auth monitor cancellation interrupts its pending delay before polling", async () => {
  const controller = new AbortController();
  let polls = 0;
  let delayStarted!: () => void;
  const started = new Promise<void>((resolve) => {
    delayStarted = resolve;
  });

  const task = runMcpAuthMonitor({
    signal: controller.signal,
    sleep: async (_delayMs, signal) => {
      delayStarted();
      await new Promise<void>((resolve) => {
        signal.addEventListener("abort", () => resolve(), { once: true });
      });
    },
    poll: async () => {
      polls += 1;
      return "continue";
    },
  });

  await started;
  controller.abort();

  assert.deepEqual(await task, { outcome: "cancelled", attempts: 0 });
  assert.equal(polls, 0);
});
