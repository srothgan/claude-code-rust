import assert from "node:assert/strict";
import test from "node:test";
import type { Query } from "@anthropic-ai/claude-agent-sdk";
import type { BridgeEvent } from "../types.js";
import { SideQuestionAdapter } from "./side_questions.js";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
function failure(
  events: BridgeEvent[],
  index: number,
  session: string,
  id: string,
  question: string,
  cause: RegExp,
) {
  const event = events[index];
  assert.ok(event?.event === "btw_failed", `${id}: correlated failure`);
  assert.match(event.error, cause);
  assert.deepEqual(event, {
    event: "btw_failed",
    session_id: session,
    btw_id: id,
    question,
    error: event.error,
  });
}
function result(
  session: string,
  id: string,
  question: string,
  answer: string,
  synthetic: boolean,
): BridgeEvent {
  return {
    event: "btw_result",
    session_id: session,
    btw_id: id,
    question,
    answer,
    metadata: { synthetic },
  };
}

test("the non-queuing adapter preserves exact native payloads and host-selected duplicate identities", {
  timeout: 2_000,
}, async () => {
  const first = deferred<unknown>(),
    second = deferred<unknown>();
  const question = "  same question\nwith a second line\t";
  const calls: Array<{ question: string; signal: AbortSignal }> = [];
  const query = {
    askSideQuestion(this: unknown, input: string, options: { signal: AbortSignal }) {
      assert.equal(this, query);
      assert.ok(options.signal instanceof AbortSignal);
      assert.equal(options.signal.aborted, false);
      calls.push({ question: input, signal: options.signal });
      return calls.length === 1 ? first.promise : second.promise;
    },
  };
  const events: BridgeEvent[] = [];
  const adapter = new SideQuestionAdapter("duplicate-session", query as unknown as Query, (event) =>
    events.push(event),
  );
  const firstDispatch = adapter.dispatch({ btwId: "first", question });
  let secondDispatch: Promise<void> | undefined;
  try {
    await adapter.dispatch({ btwId: "rejected-overlap", question: "  rejected\nquestion " });
    failure(
      events,
      0,
      "duplicate-session",
      "rejected-overlap",
      "  rejected\nquestion ",
      /already active/,
    );
    assert.deepEqual(
      calls.map((call) => call.question),
      [question],
      "rejected item never reaches the SDK",
    );
    first.resolve({ response: " first answer\n ", synthetic: false });
    await firstDispatch;
    assert.deepEqual(events.slice(1), [
      result("duplicate-session", "first", question, " first answer\n ", false),
    ]);
    secondDispatch = adapter.dispatch({ btwId: "second", question });
    second.resolve({ response: "\tindependent second\nanswer", synthetic: true });
    await secondDispatch;
    assert.deepEqual(events.slice(1), [
      result("duplicate-session", "first", question, " first answer\n ", false),
      result("duplicate-session", "second", question, "\tindependent second\nanswer", true),
    ]);
    assert.deepEqual(
      calls.map((call) => call.question),
      [question, question],
      "no automatic retry or queued overlap dispatch",
    );
    assert.notEqual(
      calls[0].signal,
      calls[1].signal,
      "each accepted call owns its cancellation signal",
    );
  } finally {
    adapter.close();
    first.resolve(null);
    second.resolve(null);
    await Promise.all([firstDispatch, secondDispatch]);
  }
});

test("invalid side-question results and capabilities fail their own item while supported dispatches remain usable", {
  timeout: 2_000,
}, async () => {
  const unsafe = "UNSAFE ANSWER MUST NOT BE PUBLISHED";
  const cases: Array<[string, unknown, RegExp]> = [
    ["null", null, /no side-question answer/],
    ["array", [unsafe], /malformed.*result/],
    ["scalar", unsafe, /malformed.*result/],
    ["missing-response", { synthetic: false }, /response/],
    ["nonstring-response", { response: 7, synthetic: false }, /response/],
    ["empty-response", { response: "", synthetic: false }, /response/],
    ["missing-synthetic", { response: unsafe }, /synthetic/],
    ["invalid-synthetic", { response: unsafe, synthetic: "false" }, /synthetic/],
    [
      "null-fallback",
      { response: unsafe, synthetic: false, refusalFallback: null },
      /refusal metadata/,
    ],
    [
      "missing-original",
      {
        response: unsafe,
        synthetic: true,
        refusalFallback: { fallbackModel: "sonnet", content: {} },
      },
      /refusal metadata/,
    ],
    [
      "missing-fallback",
      {
        response: unsafe,
        synthetic: true,
        refusalFallback: { originalModel: "opus", content: {} },
      },
      /refusal metadata/,
    ],
    [
      "nonstring-model",
      {
        response: unsafe,
        synthetic: true,
        refusalFallback: { originalModel: 7, fallbackModel: "sonnet", content: {} },
      },
      /refusal metadata/,
    ],
    [
      "missing-content",
      {
        response: unsafe,
        synthetic: true,
        refusalFallback: { originalModel: "opus", fallbackModel: "sonnet" },
      },
      /refusal metadata/,
    ],
  ];
  for (const [name, invalid, cause] of cases) {
    const calls: string[] = [];
    const values = [
      invalid,
      {
        response: ` safe ${name}\nanswer `,
        synthetic: true,
        refusalFallback: {
          originalModel: "opus-original",
          fallbackModel: "sonnet-fallback",
          content: { reason: "policy", details: [null, { retained: true }, 3] },
        },
      },
    ];
    const query = {
      askSideQuestion(this: unknown, question: string, options: { signal: AbortSignal }) {
        assert.equal(this, query);
        assert.ok(options.signal instanceof AbortSignal);
        calls.push(question);
        return Promise.resolve(values.shift());
      },
    };
    const events: BridgeEvent[] = [];
    const session = `invalid-${name}`;
    const adapter = new SideQuestionAdapter(session, query as unknown as Query, (event) =>
      events.push(event),
    );
    try {
      await adapter.dispatch({ btwId: `${name}-bad`, question: `bad ${name}\ninput` });
      failure(events, 0, session, `${name}-bad`, `bad ${name}\ninput`, cause);
      assert.equal(events.length, 1, "invalid result publishes one failure and no unsafe answer");
      await adapter.dispatch({ btwId: `${name}-good`, question: `next ${name}` });
      assert.deepEqual(events.slice(1), [
        {
          event: "btw_result",
          session_id: session,
          btw_id: `${name}-good`,
          question: `next ${name}`,
          answer: ` safe ${name}\nanswer `,
          metadata: {
            synthetic: true,
            refusal_fallback: {
              original_model: "opus-original",
              fallback_model: "sonnet-fallback",
              content: { reason: "policy", details: [null, { retained: true }, 3] },
            },
          },
        },
      ]);
      assert.deepEqual(calls, [`bad ${name}\ninput`, `next ${name}`]);
      assert.equal(JSON.stringify(events).includes(unsafe), false);
    } finally {
      adapter.close();
    }
  }
  for (const capability of [undefined, 42, {}]) {
    const events: BridgeEvent[] = [];
    const adapter = new SideQuestionAdapter(
      "unsupported",
      { askSideQuestion: capability } as unknown as Query,
      (event) => events.push(event),
    );
    try {
      await adapter.dispatch({
        btwId: "unsupported-item",
        question: " original unsupported question\n",
      });
      failure(
        events,
        0,
        "unsupported",
        "unsupported-item",
        " original unsupported question\n",
        /does not support side questions/,
      );
      assert.equal(events.length, 1);
    } finally {
      adapter.close();
    }
  }
  const supported: BridgeEvent[] = [];
  const adapter = new SideQuestionAdapter(
    "supported",
    {
      askSideQuestion: async () => ({ response: "supported answer", synthetic: false }),
    } as unknown as Query,
    (event) => supported.push(event),
  );
  try {
    await adapter.dispatch({ btwId: "supported-item", question: "supported question" });
    assert.deepEqual(supported, [
      result("supported", "supported-item", "supported question", "supported answer", false),
    ]);
  } finally {
    adapter.close();
  }
});

test("closing aborts only the active call and admits no later SDK work or terminal publication", {
  timeout: 2_000,
}, async () => {
  for (const mode of ["reject-on-abort", "ignore-abort"] as const) {
    const pending = deferred<unknown>();
    const calls: Array<{ question: string; signal: AbortSignal }> = [];
    let aborts = 0;
    const query = {
      askSideQuestion(this: unknown, question: string, options: { signal: AbortSignal }) {
        assert.equal(this, query);
        assert.ok(options.signal instanceof AbortSignal);
        calls.push({ question, signal: options.signal });
        options.signal.addEventListener(
          "abort",
          () => {
            aborts++;
            if (mode === "reject-on-abort") pending.reject(new Error("active call aborted"));
          },
          { once: true },
        );
        return pending.promise;
      },
    };
    const events: BridgeEvent[] = [];
    const adapter = new SideQuestionAdapter(mode, query as unknown as Query, (event) =>
      events.push(event),
    );
    const dispatch = adapter.dispatch({ btwId: "active", question: "active original question" });
    try {
      adapter.close();
      assert.equal(calls[0].signal.aborted, true);
      assert.equal(aborts, 1);
      await adapter.dispatch({ btwId: "after-close-pending", question: "must never execute" });
      if (mode === "ignore-abort") pending.resolve({ response: "stale success", synthetic: false });
      await dispatch;
      adapter.close();
      await adapter.dispatch({ btwId: "after-settlement", question: "must not execute either" });
      assert.deepEqual(
        calls.map((call) => call.question),
        ["active original question"],
      );
      assert.equal(aborts, 1);
      assert.deepEqual(events, []);
    } finally {
      adapter.close();
      pending.resolve(null);
      await dispatch;
    }
  }
});

test("either terminal outcome releases the active handle before a reentrant host dispatch", {
  timeout: 2_000,
}, async () => {
  for (const firstKind of ["success", "invalid-result"] as const) {
    const events: BridgeEvent[] = [],
      calls: string[] = [];
    let secondDispatch: Promise<void> | undefined;
    const query = {
      askSideQuestion(this: unknown, question: string, options: { signal: AbortSignal }) {
        assert.equal(this, query);
        assert.ok(options.signal instanceof AbortSignal);
        calls.push(question);
        return Promise.resolve(
          calls.length === 1
            ? firstKind === "success"
              ? { response: "first native answer", synthetic: false }
              : { response: "unsafe", synthetic: "invalid" }
            : { response: "second independent answer\n", synthetic: true },
        );
      },
    };
    const adapter = new SideQuestionAdapter(firstKind, query as unknown as Query, (event) => {
      events.push(event);
      if (
        (event.event === "btw_result" || event.event === "btw_failed") &&
        event.btw_id === "first"
      )
        secondDispatch = adapter.dispatch({
          btwId: "second",
          question: " second native question\n",
        });
    });
    try {
      await adapter.dispatch({ btwId: "first", question: "first native question" });
      assert.ok(secondDispatch, "host synchronously dispatched the successor");
      await secondDispatch;
      assert.deepEqual(calls, ["first native question", " second native question\n"]);
      if (firstKind === "success")
        assert.deepEqual(
          events[0],
          result(firstKind, "first", "first native question", "first native answer", false),
        );
      else failure(events, 0, firstKind, "first", "first native question", /synthetic/);
      assert.deepEqual(events.slice(1), [
        result(
          firstKind,
          "second",
          " second native question\n",
          "second independent answer\n",
          true,
        ),
      ]);
    } finally {
      adapter.close();
      await secondDispatch;
    }
  }
});

test("native side-question exceptions fail only their own item and preserve adapter and sibling reuse", {
  timeout: 2_000,
}, async () => {
  for (const kind of ["throw", "reject"] as const) {
    const events: BridgeEvent[] = [],
      calls: string[] = [];
    const cause = `controlled ${kind} transport failure`;
    const query = {
      askSideQuestion(this: unknown, question: string, options: { signal: AbortSignal }) {
        assert.equal(this, query);
        assert.ok(options.signal instanceof AbortSignal);
        calls.push(question);
        if (calls.length === 1) {
          if (kind === "throw") throw new Error(cause);
          return Promise.reject(new Error(cause));
        }
        return Promise.resolve({ response: " recovered native answer\n", synthetic: true });
      },
    };
    const adapter = new SideQuestionAdapter(`failure-${kind}`, query as unknown as Query, (event) =>
      events.push(event),
    );
    const sibling = new SideQuestionAdapter(
      `sibling-${kind}`,
      {
        askSideQuestion: async () => ({ response: "sibling independent answer", synthetic: false }),
      } as unknown as Query,
      (event) => events.push(event),
    );
    try {
      await adapter.dispatch({ btwId: "failed-item", question: " failed native question\n" });
      failure(
        events,
        0,
        `failure-${kind}`,
        "failed-item",
        " failed native question\n",
        new RegExp(cause),
      );
      assert.deepEqual(
        calls,
        [" failed native question\n"],
        "a failed native item is not retried automatically",
      );
      await sibling.dispatch({ btwId: "sibling-item", question: "sibling input" });
      await adapter.dispatch({ btwId: "recovered-item", question: "fresh explicit input" });
      assert.deepEqual(events.slice(1), [
        result(
          `sibling-${kind}`,
          "sibling-item",
          "sibling input",
          "sibling independent answer",
          false,
        ),
        result(
          `failure-${kind}`,
          "recovered-item",
          "fresh explicit input",
          " recovered native answer\n",
          true,
        ),
      ]);
      assert.deepEqual(calls, [" failed native question\n", "fresh explicit input"]);
    } finally {
      adapter.close();
      sibling.close();
    }
  }
});
