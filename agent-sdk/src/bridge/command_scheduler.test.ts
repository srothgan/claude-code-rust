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

// Abort a stranded dependency wait on the test deadline so finally can release owned gates.
async function observed<T>(promise: Promise<T>, signal: AbortSignal): Promise<T> {
  signal.throwIfAborted();
  let onAbort!: () => void;
  const aborted = new Promise<never>((_, reject) => {
    onAbort = () => reject(signal.reason);
    signal.addEventListener("abort", onAbort, { once: true });
  });
  try {
    return await Promise.race([promise, aborted]);
  } finally {
    signal.removeEventListener("abort", onAbort);
  }
}

// Check absence after promise dependencies have had an event-loop turn to settle.
function checkpoint(): Promise<void> {
  return new Promise((resolve) => setImmediate(resolve));
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

test("new-session is an exclusive barrier between session operations", { timeout: 5_000 }, async (t) => {
  const scheduler = new BridgeCommandScheduler();
  const releaseA = deferred();
  const releaseB = deferred();
  const enteredA = deferred();
  const enteredB = deferred();
  const replacementEntered = deferred();
  const releaseReplacement = deferred();
  const completed = new Set<string>();
  let replacementStarted = false;
  let laterStarted = false;

  const beforeA = scheduler.schedule(command("set_model", "session-a"), async () => {
    enteredA.resolve();
    await releaseA.promise;
    completed.add("session-a");
  });
  const beforeB = scheduler.schedule(command("set_mode", "session-b"), async () => {
    enteredB.resolve();
    await releaseB.promise;
    completed.add("session-b");
  });
  const replacement = scheduler.schedule(command("new_session"), async () => {
    replacementStarted = true;
    replacementEntered.resolve();
    await releaseReplacement.promise;
    completed.add("replacement");
  });
  const after = scheduler.schedule(command("prompt", "session-c"), async () => {
    laterStarted = true;
    completed.add("later");
  });

  try {
    await observed(Promise.all([enteredA.promise, enteredB.promise]), t.signal);
    releaseA.resolve();
    await observed(Promise.resolve(beforeA), t.signal);
    await checkpoint();
    assert.deepEqual(completed, new Set(["session-a"]));
    assert.equal(replacementStarted, false, "replacement must also drain session B");
    assert.equal(laterStarted, false);

    releaseB.resolve();
    await observed(replacementEntered.promise, t.signal);
    await checkpoint();
    assert.deepEqual(completed, new Set(["session-a", "session-b"]));
    assert.equal(laterStarted, false, "later work must wait for replacement completion");

    releaseReplacement.resolve();
    await observed(Promise.all([beforeA, beforeB, replacement, after]), t.signal);
    assert.deepEqual(completed, new Set(["session-a", "session-b", "replacement", "later"]));
  } finally {
    releaseA.resolve();
    releaseB.resolve();
    releaseReplacement.resolve();
  }
});

test("commands are FIFO within a session and concurrent across sessions", { timeout: 5_000 }, async (t) => {
  const scheduler = new BridgeCommandScheduler();
  const firstSession = deferred();
  const firstEntered = deferred();
  const otherCompleted = deferred();
  const sessionEffects: string[] = [];
  let otherRan = false;

  const first = scheduler.schedule(
    command("set_mode", "session-1"),
    async () => {
      sessionEffects.push("first:start");
      firstEntered.resolve();
      await firstSession.promise;
      sessionEffects.push("first:end");
    },
  );
  const sameSession = scheduler.schedule(
    command("set_effort", "session-1"),
    async () => {
      sessionEffects.push("next");
    },
  );
  const otherSession = scheduler.schedule(
    command("set_model", "session-2"),
    async () => {
      otherRan = true;
      otherCompleted.resolve();
    },
  );

  try {
    await observed(Promise.all([firstEntered.promise, otherCompleted.promise]), t.signal);
    assert.equal(otherRan, true, "another session must finish while session one is held");
    assert.deepEqual(sessionEffects, ["first:start"]);
    firstSession.resolve();
    await observed(Promise.all([first, sameSession, otherSession]), t.signal);
    assert.deepEqual(sessionEffects, ["first:start", "first:end", "next"]);
  } finally {
    firstSession.resolve();
  }
});

test("unblocking commands bypass held session and lifecycle work without holding later work", { timeout: 5_000 }, async (t) => {
  const cases: { name: BridgeCommand["command"]; blocker: "set_model" | "new_session" }[] = [
    { name: "permission_response", blocker: "set_model" },
    { name: "question_response", blocker: "new_session" },
    { name: "user_dialog_response", blocker: "set_model" },
    { name: "elicitation_response", blocker: "new_session" },
    { name: "cancel_turn", blocker: "set_model" },
    { name: "mcp_oauth_callback_url", blocker: "new_session" },
    { name: "side_question", blocker: "set_model" },
    { name: "stop_subagent", blocker: "new_session" },
  ];
  for (const { name, blocker } of cases) {
    await t.test(`${name} bypasses ${blocker}`, { timeout: 1_000 }, async (row) => {
      const scheduler = new BridgeCommandScheduler();
      const releaseParent = deferred();
      const parentEntered = deferred();
      const replyEntered = deferred();
      const effects: string[] = [];
      const parent = scheduler.schedule(command(blocker), async () => {
        effects.push("parent:start");
        parentEntered.resolve();
        await releaseParent.promise;
        effects.push("parent:end");
      });
      try {
        await observed(parentEntered.promise, row.signal);
        const reply = scheduler.schedule(command(name), async () => {
          effects.push(name);
          replyEntered.resolve();
          // Exercise a real waiter release, rather than merely labelling a callback a response.
          if (name === "permission_response") releaseParent.resolve();
        });
        assert(reply, `${name} must be admitted`);
        await observed(replyEntered.promise, row.signal);
        await observed(reply, row.signal);
        if (name !== "permission_response") {
          assert.deepEqual(effects, ["parent:start", name]);
          releaseParent.resolve();
        }
        await observed(Promise.resolve(parent), row.signal);
        assert.deepEqual(effects, ["parent:start", name, "parent:end"]);
      } finally {
        releaseParent.resolve();
      }
    });
  }

  await t.test("a held stop_subagent permits a later ordinary callback", { timeout: 1_000 }, async (row) => {
    const scheduler = new BridgeCommandScheduler();
    const releaseStop = deferred();
    const stopEntered = deferred();
    const nextCompleted = deferred();
    const effects: string[] = [];
    const stop = scheduler.schedule(command("stop_subagent"), async () => {
      effects.push("stop:start");
      stopEntered.resolve();
      await releaseStop.promise;
      effects.push("stop:end");
    });
    try {
      await observed(stopEntered.promise, row.signal);
      const next = scheduler.schedule(command("set_effort"), async () => {
        effects.push("next");
        nextCompleted.resolve();
      });
      await observed(nextCompleted.promise, row.signal);
      assert.deepEqual(effects, ["stop:start", "next"]);
      releaseStop.resolve();
      await observed(Promise.all([stop, next]), row.signal);
      assert.deepEqual(effects, ["stop:start", "next", "stop:end"]);
    } finally {
      releaseStop.resolve();
    }
  });
});

test("shutdown drains prior lanes and queued work, admitting replies only until it starts", { timeout: 5_000 }, async (t) => {
  for (const firstReleased of ["session-b", "queued-a"] as const) {
    await t.test(`shutdown still waits after ${firstReleased} completes`, { timeout: 1_000 }, async (row) => {
      const scheduler = new BridgeCommandScheduler();
      const response = deferred();
      const releaseB = deferred();
      const releaseQueued = deferred();
      const releaseShutdown = deferred();
      const enteredA = deferred();
      const enteredB = deferred();
      const queuedEntered = deferred();
      const shutdownEntered = deferred();
      const completed = new Set<string>();
      const refusedEffects: string[] = [];
      let shutdownRuns = 0;
      let shutdownCompleted = false;
      let drainedAtShutdown: Set<string> | undefined;

      const first = scheduler.schedule(command("set_model", "session-a"), async () => {
        enteredA.resolve();
        await response.promise;
        completed.add("session-a");
      });
      const other = scheduler.schedule(command("set_mode", "session-b"), async () => {
        enteredB.resolve();
        await releaseB.promise;
        completed.add("session-b");
      });
      const queued = scheduler.schedule(command("set_effort", "session-a"), async () => {
        queuedEntered.resolve();
        await releaseQueued.promise;
        completed.add("queued-a");
      });
      const shutdown = scheduler.schedule(command("shutdown"), async () => {
        shutdownRuns += 1;
        drainedAtShutdown = new Set(completed);
        shutdownEntered.resolve();
        await releaseShutdown.promise;
        shutdownCompleted = true;
      });
      const expectRefused = (phase: string, name: BridgeCommand["command"]) => {
        assert.equal(scheduler.schedule(command(name), async () => {
          refusedEffects.push(`${phase}:${name}`);
        }), undefined, `${name} must be refused while shutdown is ${phase}`);
      };

      try {
        await observed(Promise.all([enteredA.promise, enteredB.promise]), row.signal);
        expectRefused("queued", "prompt");
        expectRefused("queued", "shutdown");
        let replyRan = false;
        const reply = scheduler.schedule(command("question_response", "session-a"), async () => {
          replyRan = true;
          response.resolve();
        });
        assert(reply, "a reply must remain admitted while shutdown waits");
        await observed(Promise.all([reply, queuedEntered.promise]), row.signal);
        assert.equal(replyRan, true);
        assert.deepEqual(completed, new Set(["session-a"]));

        if (firstReleased === "session-b") {
          releaseB.resolve();
          await observed(Promise.resolve(other), row.signal);
        } else {
          releaseQueued.resolve();
          await observed(Promise.resolve(queued), row.signal);
        }
        await checkpoint();
        assert.equal(shutdownRuns, 0, "shutdown must drain both lanes, including already queued work");
        releaseB.resolve();
        releaseQueued.resolve();
        await observed(shutdownEntered.promise, row.signal);
        assert.deepEqual(drainedAtShutdown, new Set(["session-a", "session-b", "queued-a"]));
        assert.equal(shutdownRuns, 1);
        assert.equal(shutdownCompleted, false);
        expectRefused("running", "prompt");
        expectRefused("running", "cancel_turn");
        expectRefused("running", "shutdown");

        releaseShutdown.resolve();
        await observed(Promise.all([first, other, queued, shutdown, reply]), row.signal);
        assert.equal(shutdownCompleted, true);
        expectRefused("complete", "prompt");
        expectRefused("complete", "permission_response");
        expectRefused("complete", "shutdown");
        await checkpoint();
        assert.deepEqual(refusedEffects, []);
        assert.equal(shutdownRuns, 1);
      } finally {
        response.resolve();
        releaseB.resolve();
        releaseQueued.resolve();
        releaseShutdown.resolve();
      }
    });
  }

  await t.test("shutdown also drains a held prior lifecycle operation", { timeout: 1_000 }, async (row) => {
    const scheduler = new BridgeCommandScheduler();
    const releaseLifecycle = deferred();
    const lifecycleEntered = deferred();
    const effects: string[] = [];
    const prior = scheduler.schedule(command("resume_session"), async () => {
      effects.push("lifecycle:start");
      lifecycleEntered.resolve();
      await releaseLifecycle.promise;
      effects.push("lifecycle:end");
    });
    const shutdown = scheduler.schedule(command("shutdown"), async () => {
      effects.push("shutdown");
    });
    try {
      await observed(lifecycleEntered.promise, row.signal);
      await checkpoint();
      assert.deepEqual(effects, ["lifecycle:start"]);
      releaseLifecycle.resolve();
      await observed(Promise.all([prior, shutdown]), row.signal);
      assert.deepEqual(effects, ["lifecycle:start", "lifecycle:end", "shutdown"]);
    } finally {
      releaseLifecycle.resolve();
    }
  });
});

test("async session and lifecycle failures preserve their errors without poisoning later work", { timeout: 5_000 }, async (t) => {
  for (const lane of ["session", "lifecycle"] as const) {
    await t.test(`${lane} rejection preserves later same-lane and cross-lane work`, { timeout: 1_000 }, async (row) => {
      const scheduler = new BridgeCommandScheduler();
      const releaseFailure = deferred();
      const failureEntered = deferred();
      const originalError = new Error(`${lane} operation failed`);
      const effects: string[] = [];
      const failed = scheduler.schedule(command(lane === "session" ? "set_model" : "new_session", "session-a"), async () => {
        effects.push("failed:start");
        failureEntered.resolve();
        await releaseFailure.promise;
        effects.push("failed:reject");
        throw originalError;
      });
      assert(failed);
      // Observe the caller's rejection before allowing the operation to fail.
      const rejection = assert.rejects(failed, (error: unknown) => error === originalError);
      const sameSession = scheduler.schedule(command("set_mode", "session-a"), async () => {
        effects.push("session-a:next");
      });
      const otherSession = scheduler.schedule(command("set_effort", "session-b"), async () => {
        effects.push("session-b:next");
      });
      const barrier = scheduler.schedule(command("resume_session"), async () => {
        effects.push("lifecycle:next");
      });
      try {
        await observed(failureEntered.promise, row.signal);
        assert.equal(effects.includes("session-a:next"), false);
        assert.equal(effects.includes("lifecycle:next"), false);
        if (lane === "lifecycle") assert.equal(effects.includes("session-b:next"), false);
        releaseFailure.resolve();
        await observed(Promise.all([rejection, sameSession, otherSession, barrier]), row.signal);
        assert.deepEqual(effects.filter((effect) => effect.startsWith("failed:")), ["failed:start", "failed:reject"]);
        for (const effect of ["session-a:next", "session-b:next", "lifecycle:next"]) {
          assert.equal(effects.filter((actual) => actual === effect).length, 1, `${effect} must execute exactly once`);
        }
        assert(effects.indexOf("failed:reject") < effects.indexOf("session-a:next"));
        assert(effects.indexOf("session-a:next") < effects.indexOf("lifecycle:next"));
        assert(effects.indexOf("session-b:next") < effects.indexOf("lifecycle:next"));
        if (lane === "lifecycle") assert(effects.indexOf("failed:reject") < effects.indexOf("session-b:next"));
      } finally {
        releaseFailure.resolve();
      }
    });
  }
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
