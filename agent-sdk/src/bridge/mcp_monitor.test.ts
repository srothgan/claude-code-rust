import assert from "node:assert/strict";
import test from "node:test";
import { runMcpAuthMonitor } from "./mcp_monitor.js";
import { startMcpAuthSnapshotMonitor } from "./mcp.js";
import { replaceProtocolEventWriter } from "./events.js";
import type { SessionState } from "./session_lifecycle.js";
import type { BridgeEvent } from "../types.js";
import type { McpServerStatus } from "@anthropic-ai/claude-agent-sdk";

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

async function settles<T>(task: Promise<T>, description: string): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      task,
      new Promise<never>((_resolve, reject) => {
        timer = setTimeout(
          () =>
            reject(
              new assert.AssertionError({
                message: description,
                actual: "pending",
                expected: "settled",
              }),
            ),
          2_000,
        );
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

test("MCP auth monitor cancellation interrupts the production delay before polling", async () => {
  const controller = new AbortController();
  let polls = 0;
  // Starting the async runner enters its default delay before returning this task.
  const task = runMcpAuthMonitor({
    signal: controller.signal,
    initialDelayMs: 30_000,
    poll: async () => {
      polls++;
      return "continue";
    },
  });
  try {
    let settled = false;
    void task.then(() => { settled = true; });
    await new Promise<void>((resolve) => setImmediate(resolve));
    assert.equal(settled, false, "the active monitor must be waiting before cancellation");
    controller.abort();
    const result = await settles(
      task,
      "aborting must settle before the long production delay expires",
    );
    assert.deepEqual(result, { outcome: "cancelled", attempts: 0 });
    assert.equal(polls, 0);
  } finally {
    controller.abort();
  }
});

function controlledClock() {
  let now = 0;
  const waiting = new Set<{ due: number; finish: () => void }>();
  return {
    sleep: (duration: number, signal: AbortSignal) =>
      new Promise<void>((resolve, reject) => {
        const aborted = () => {
          waiting.delete(entry);
          reject(signal.reason);
        };
        const entry = {
          due: now + duration,
          finish: () => {
            waiting.delete(entry);
            signal.removeEventListener("abort", aborted);
            resolve();
          },
        };
        if (signal.aborted) reject(signal.reason);
        else {
          waiting.add(entry);
          signal.addEventListener("abort", aborted, { once: true });
        }
      }),
    advance: async (duration: number) => {
      now += duration;
      for (const entry of [...waiting]) if (entry.due <= now) entry.finish();
      // Fixture status reads resolve immediately; drain their continuations.
      await new Promise<void>((resolve) => setImmediate(resolve));
    },
    release: () => {
      for (const entry of [...waiting]) entry.finish();
    },
  };
}

test("successful MCP authentication stops polling and a fresh same-server cycle recovers and publishes new data", async () => {
  for (const transientFailure of [false, true]) {
    const clock = controlledClock();
    const sibling: McpServerStatus = {
      name: "unrelated-pending",
      status: "pending",
    };
    const status = (
      state: McpServerStatus["status"],
      version: string,
    ): McpServerStatus[] => [
      sibling,
      {
        name: "docs",
        status: state,
        serverInfo: { name: "Controlled docs", version },
      },
    ];
    const script: Array<McpServerStatus[] | Error> = [
      ...(transientFailure
        ? [new Error("recoverable selected status read")]
        : []),
      status("needs-auth", "first-pending"),
      status("connected", "first-connected"),
      status("pending", "second-pending"),
      status("connected", "second-connected"),
    ];
    let reads = 0;
    const reconnects: string[] = [];
    const session = {
      sessionId: transientFailure ? "mcp-recovery" : "mcp-success",
      closing: false,
      query: {
        mcpServerStatus: async () => {
          const next = script[reads++];
          assert.ok(next, "no unplanned SDK status read");
          if (next instanceof Error) throw next;
          return next;
        },
        reconnectMcpServer: async (name: string) => {
          reconnects.push(name);
        },
      },
      knownConnectedMcpServers: new Set(),
      mcpStatusRevalidatedAt: new Map(),
      mcpAuthMonitors: new Map(),
    } as unknown as SessionState;
    const events: BridgeEvent[] = [];
    const restore = replaceProtocolEventWriter((line) => {
      events.push(JSON.parse(line));
    });
    const tasks: Array<ReturnType<typeof startMcpAuthSnapshotMonitor>> = [];
    const timing = {
      initialDelayMs: 10,
      maxDelayMs: 20,
      maxAttempts: 4,
      sleep: clock.sleep,
    };
    const snapshots = () =>
      events.flatMap((event) => {
        if (event.event !== "mcp_snapshot") return [];
        assert.equal(event.session_id, session.sessionId);
        assert.equal(event.source, "mcp_status");
        assert.equal(
          event.servers.find((server) => server.name === "unrelated-pending")
            ?.status,
          "pending",
        );
        const target = event.servers.find((server) => server.name === "docs");
        assert.ok(
          target,
          "the selected server is present in its actual emitted snapshot",
        );
        return [
          { status: target.status, version: target.server_info?.version },
        ];
      });
    try {
      const first = startMcpAuthSnapshotMonitor(session, "docs", timing);
      tasks.push(first);
      if (transientFailure) {
        await clock.advance(20);
        assert.equal(reads, 1);
        assert.deepEqual(
          snapshots(),
          [],
          "a rejected read cannot publish a successful status",
        );
      }
      await clock.advance(20);
      assert.equal(reads, transientFailure ? 2 : 1);
      assert.deepEqual(snapshots(), [
        { status: "needs-auth", version: "first-pending" },
      ]);
      await clock.advance(20);
      assert.deepEqual(
        await settles(
          first,
          "the selected connected server must complete despite its pending sibling",
        ),
        {
          outcome: "completed",
          attempts: transientFailure ? 3 : 2,
        },
      );
      const firstSnapshots = [
        { status: "needs-auth", version: "first-pending" },
        { status: "connected", version: "first-connected" },
      ];
      assert.deepEqual(snapshots(), firstSnapshots);
      const firstReads = reads;
      const firstEvents = structuredClone(events);
      await clock.advance(10_000);
      assert.equal(
        reads,
        firstReads,
        "completed monitoring performs no more SDK reads",
      );
      assert.deepEqual(
        reconnects,
        [],
        "completion must not reconnect the pending sibling",
      );
      assert.deepEqual(
        events,
        firstEvents,
        "completed monitoring publishes no later snapshot",
      );

      const second = startMcpAuthSnapshotMonitor(session, "docs", timing);
      tasks.push(second);
      await clock.advance(20);
      assert.equal(
        reads,
        firstReads + 1,
        "same-server restart must perform new query work",
      );
      assert.deepEqual(snapshots(), [
        ...firstSnapshots,
        { status: "pending", version: "second-pending" },
      ]);
      await clock.advance(20);
      assert.deepEqual(
        await settles(
          second,
          "the new monitoring cycle must finish independently",
        ),
        { outcome: "completed", attempts: 2 },
      );
      assert.deepEqual(snapshots(), [
        ...firstSnapshots,
        { status: "pending", version: "second-pending" },
        { status: "connected", version: "second-connected" },
      ]);
      const finalReads = reads;
      const finalEvents = structuredClone(events);
      await clock.advance(10_000);
      assert.equal(reads, finalReads);
      assert.deepEqual(reconnects, []);
      assert.deepEqual(events, finalEvents);
    } finally {
      session.closing = true;
      for (const monitor of session.mcpAuthMonitors.values())
        monitor.controller.abort();
      clock.release();
      await Promise.allSettled(tasks);
      restore();
    }
  }
});
