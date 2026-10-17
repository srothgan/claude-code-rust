import assert from "node:assert/strict";
import test, { mock } from "node:test";
import type { BridgeCommand } from "../types.js";
import { BridgeCommandScheduler } from "./command_scheduler.js";

function command(
  name: BridgeCommand["command"],
  sessionId = "session-1",
): BridgeCommand {
  return { command: name, session_id: sessionId } as BridgeCommand;
}

function deferred(): { promise: Promise<void>; resolve: () => void } {
  let resolve!: () => void;
  const promise = new Promise<void>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

test("lifecycle commands complete in arrival order when the older operation is delayed", async () => {
  const scheduler = new BridgeCommandScheduler();
  const older = deferred();
  const order: string[] = [];
  let authority = "";

  const olderTask = scheduler.schedule(
    command("resume_session", "older"),
    async () => {
      order.push("older:start");
      await older.promise;
      authority = "older";
      order.push("older:end");
    },
  );
  const newerTask = scheduler.schedule(
    command("resume_session", "newer"),
    async () => {
      order.push("newer:start");
      authority = "newer";
      order.push("newer:end");
    },
  );

  await Promise.resolve();
  assert.deepEqual(order, ["older:start"]);
  older.resolve();
  await Promise.all([olderTask, newerTask]);

  assert.deepEqual(order, [
    "older:start",
    "older:end",
    "newer:start",
    "newer:end",
  ]);
  assert.equal(authority, "newer");
});

test("new-session is an exclusive barrier between session operations", async () => {
  const scheduler = new BridgeCommandScheduler();
  const sessionMutation = deferred();
  const order: string[] = [];

  const before = scheduler.schedule(command("set_model"), async () => {
    order.push("mutation:start");
    await sessionMutation.promise;
    order.push("mutation:end");
  });
  const replacement = scheduler.schedule(command("new_session"), async () => {
    order.push("new-session");
  });
  const after = scheduler.schedule(command("prompt"), async () => {
    order.push("prompt");
  });

  await Promise.resolve();
  assert.deepEqual(order, ["mutation:start"]);
  sessionMutation.resolve();
  await Promise.all([before, replacement, after]);

  assert.deepEqual(order, [
    "mutation:start",
    "mutation:end",
    "new-session",
    "prompt",
  ]);
});

test("commands are FIFO within a session and concurrent across sessions", async () => {
  const scheduler = new BridgeCommandScheduler();
  const firstSession = deferred();
  const order: string[] = [];

  const first = scheduler.schedule(
    command("set_mode", "session-1"),
    async () => {
      order.push("one:start");
      await firstSession.promise;
      order.push("one:end");
    },
  );
  const sameSession = scheduler.schedule(
    command("set_effort", "session-1"),
    async () => {
      order.push("one:next");
    },
  );
  const otherSession = scheduler.schedule(
    command("set_model", "session-2"),
    async () => {
      order.push("two");
    },
  );

  await Promise.resolve();
  await Promise.resolve();
  assert.deepEqual(order, ["one:start", "two"]);
  firstSession.resolve();
  await Promise.all([first, sameSession, otherSession]);

  assert.deepEqual(order, ["one:start", "two", "one:end", "one:next"]);
});

test("interactive responses bypass a blocked session operation", async () => {
  const scheduler = new BridgeCommandScheduler();
  const response = deferred();
  const order: string[] = [];

  const parent = scheduler.schedule(command("set_model"), async () => {
    order.push("parent:start");
    await response.promise;
    order.push("parent:end");
  });
  const reply = scheduler.schedule(command("permission_response"), async () => {
    order.push("response");
    response.resolve();
  });

  await Promise.all([parent, reply]);
  assert.deepEqual(order, ["parent:start", "response", "parent:end"]);
});

test("a pending subagent stop does not hold back later session commands", async () => {
  const scheduler = new BridgeCommandScheduler();
  const stopAnswered = deferred();
  const order: string[] = [];

  const stop = scheduler.schedule(command("stop_subagent"), async () => {
    order.push("stop:start");
    await stopAnswered.promise;
    order.push("stop:end");
  });
  const next = scheduler.schedule(command("set_effort"), async () => {
    order.push("next");
  });

  await next;
  assert.deepEqual(order, ["stop:start", "next"]);
  stopAnswered.resolve();
  await stop;
  assert.deepEqual(order, ["stop:start", "next", "stop:end"]);
});

test("shutdown drains prior work and drops later ordinary commands", async () => {
  const scheduler = new BridgeCommandScheduler();
  const inFlight = deferred();
  const order: string[] = [];

  const first = scheduler.schedule(command("set_model"), async () => {
    order.push("first:start");
    await inFlight.promise;
    order.push("first:end");
  });
  const queued = scheduler.schedule(command("set_mode"), async () => {
    order.push("queued");
  });
  const shutdown = scheduler.schedule(command("shutdown"), async () => {
    order.push("shutdown");
  });
  const dropped = scheduler.schedule(command("prompt"), async () => {
    order.push("post-shutdown");
  });

  assert.equal(dropped, undefined);
  inFlight.resolve();
  await Promise.all([first, queued, shutdown]);
  await scheduler.whenIdle();

  assert.deepEqual(order, ["first:start", "first:end", "queued", "shutdown"]);
});

test("unblocking commands remain admitted while shutdown is queued", async () => {
  const scheduler = new BridgeCommandScheduler();
  const response = deferred();
  const order: string[] = [];

  const parent = scheduler.schedule(command("set_model"), async () => {
    order.push("parent:start");
    await response.promise;
    order.push("parent:end");
  });
  const shutdown = scheduler.schedule(command("shutdown"), async () => {
    order.push("shutdown");
  });
  const reply = scheduler.schedule(command("question_response"), async () => {
    order.push("response");
    response.resolve();
  });

  await Promise.all([parent, reply, shutdown]);
  assert.deepEqual(order, [
    "parent:start",
    "response",
    "parent:end",
    "shutdown",
  ]);
  assert.equal(
    scheduler.schedule(command("cancel_turn"), async () => undefined),
    undefined,
  );
});

test("a failed operation does not poison its scheduling lane", async () => {
  const scheduler = new BridgeCommandScheduler();
  const order: string[] = [];

  const failed = scheduler.schedule(command("set_model"), async () => {
    order.push("failed");
    throw new Error("expected");
  });
  const next = scheduler.schedule(command("set_mode"), async () => {
    order.push("next");
  });

  assert(failed);
  await assert.rejects(failed, /expected/);
  await next;
  assert.deepEqual(order, ["failed", "next"]);
});

// The runner's own timeout turns a lane stranded by the skipped command into a failure, not a hang.
test("a command still queued at its deadline is answered once and skipped when its turn comes", { timeout: 5_000 }, async () => {
  mock.timers.enable({ apis: ["setTimeout"] });
  try {
    const scheduler = new BridgeCommandScheduler();
    const stalled = deferred();
    const order: string[] = [];

    const blocker = scheduler.schedule(command("get_context_usage"), async () => {
      order.push("blocker:start");
      await stalled.promise;
      order.push("blocker:end");
    });
    const queued = scheduler.schedule(
      command("reload_skills_and_styles"),
      async () => {
        order.push("queued:ran");
      },
      { ms: 10_000, onExpired: () => order.push("queued:expired") },
    );
    // A skipped command still settles its place in the lane, so later work is not stranded.
    const later = scheduler.schedule(command("set_model"), async () => {
      order.push("later");
    });

    mock.timers.tick(9_999);
    assert.deepEqual(order, ["blocker:start"]);
    mock.timers.tick(1);
    assert.deepEqual(order, ["blocker:start", "queued:expired"]);

    // The earlier request finishes late: the expired command must not run after its answer.
    stalled.resolve();
    await Promise.all([blocker, queued, later]);
    assert.deepEqual(order, [
      "blocker:start",
      "queued:expired",
      "blocker:end",
      "later",
    ]);
  } finally {
    mock.timers.reset();
  }
});

test("a command that starts before its queue deadline runs to the end without expiring", async () => {
  mock.timers.enable({ apis: ["setTimeout"] });
  try {
    const scheduler = new BridgeCommandScheduler();
    const earlier = deferred();
    const started = deferred();
    const running = deferred();
    const order: string[] = [];

    const blocker = scheduler.schedule(command("get_context_usage"), async () => {
      await earlier.promise;
      order.push("blocker:end");
    });
    const queued = scheduler.schedule(
      command("reload_skills_and_styles"),
      async () => {
        order.push("queued:start");
        started.resolve();
        await running.promise;
        order.push("queued:end");
      },
      { ms: 10_000, onExpired: () => order.push("queued:expired") },
    );

    mock.timers.tick(5_000);
    earlier.resolve();
    await started.promise;
    assert.deepEqual(order, ["blocker:end", "queued:start"]);

    // The deadline covers the wait only; execution has its own bounds.
    mock.timers.tick(10_000);
    running.resolve();
    await Promise.all([blocker, queued]);
    assert.deepEqual(order, ["blocker:end", "queued:start", "queued:end"]);
  } finally {
    mock.timers.reset();
  }
});
