import assert from "node:assert/strict";
import test from "node:test";
import type { Query } from "@anthropic-ai/claude-agent-sdk";
import type { BridgeEvent } from "../types.js";
import { SideQuestionAdapter } from "./side_questions.js";

function deferred<T>(): {
  promise: Promise<T>;
  resolve: (value: T) => void;
} {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((settle) => {
    resolve = settle;
  });
  return { promise, resolve };
}

test("the non-queuing adapter rejects overlap and preserves host-selected duplicate identities", async () => {
  const first = deferred<unknown>();
  const second = deferred<unknown>();
  const calls: Array<{ question: string; options: unknown }> = [];
  const query = {
    askSideQuestion(question: string, options: unknown) {
      calls.push({ question, options });
      return calls.length === 1 ? first.promise : second.promise;
    },
  } as unknown as Query;
  const events: BridgeEvent[] = [];
  const adapter = new SideQuestionAdapter("session-1", query, (event) => {
    events.push(event);
  });

  const firstDispatch = adapter.dispatch({ btwId: "btw-1", question: "same" });
  await adapter.dispatch({ btwId: "invalid-overlap", question: "same" });
  assert.equal(events[0]?.event, "btw_failed");
  assert.match(events[0]?.event === "btw_failed" ? events[0].error : "", /already active/);
  assert.equal(calls.length, 1);
  assert.deepEqual(Object.keys(calls[0]?.options ?? {}), ["signal"]);

  first.resolve({ response: "first answer", synthetic: false });
  await firstDispatch;
  assert.equal(calls.length, 1, "the adapter must not dispatch rejected or waiting work");
  assert.deepEqual(events[1], {
    event: "btw_result",
    session_id: "session-1",
    btw_id: "btw-1",
    question: "same",
    answer: "first answer",
    metadata: { synthetic: false },
  });

  const secondDispatch = adapter.dispatch({ btwId: "btw-2", question: "same" });
  assert.equal(calls.length, 2);
  second.resolve({ response: "second answer", synthetic: true });
  await secondDispatch;
  assert.equal(events.length, 3);
  assert.equal(
    events[2]?.event === "btw_result" ? events[2].btw_id : undefined,
    "btw-2",
  );
});

test("null results fail and leave the adapter ready for the next host dispatch", async () => {
  const results: unknown[] = [null, { response: "ok", synthetic: false }];
  const query = {
    async askSideQuestion() {
      return results.shift();
    },
  } as unknown as Query;
  const events: BridgeEvent[] = [];
  const adapter = new SideQuestionAdapter("session-1", query, (event) => {
    events.push(event);
  });

  await adapter.dispatch({ btwId: "btw-1", question: "first" });
  await adapter.dispatch({ btwId: "btw-2", question: "second" });

  assert.equal(events.length, 2);
  assert.equal(events[0]?.event, "btw_failed");
  assert.equal(events[1]?.event, "btw_result");
});

test("malformed result metadata fails without blocking the next question", async () => {
  const results: unknown[] = [
    {
      response: "not safe to emit",
      synthetic: false,
      refusalFallback: { originalModel: "opus" },
    },
    {
      response: "fallback answer",
      synthetic: true,
      refusalFallback: {
        originalModel: "opus",
        fallbackModel: "sonnet",
        content: { reason: "policy" },
      },
    },
  ];
  const query = {
    async askSideQuestion() {
      return results.shift();
    },
  } as unknown as Query;
  const events: BridgeEvent[] = [];
  const adapter = new SideQuestionAdapter("session-1", query, (event) => {
    events.push(event);
  });

  await adapter.dispatch({ btwId: "btw-1", question: "first" });
  await adapter.dispatch({ btwId: "btw-2", question: "second" });

  assert.equal(events[0]?.event, "btw_failed");
  assert.deepEqual(events[1], {
    event: "btw_result",
    session_id: "session-1",
    btw_id: "btw-2",
    question: "second",
    answer: "fallback answer",
    metadata: {
      synthetic: true,
      refusal_fallback: {
        original_model: "opus",
        fallback_model: "sonnet",
        content: { reason: "policy" },
      },
    },
  });
});

test("closing aborts the active call and suppresses stale events", async () => {
  let signal: AbortSignal | undefined;
  const query = {
    askSideQuestion(_question: string, options: { signal: AbortSignal }) {
      signal = options.signal;
      return new Promise((_resolve, reject) => {
        options.signal.addEventListener("abort", () => reject(new Error("aborted")));
      });
    },
  } as unknown as Query;
  const events: BridgeEvent[] = [];
  const adapter = new SideQuestionAdapter("session-1", query, (event) => {
    events.push(event);
  });

  const dispatch = adapter.dispatch({ btwId: "btw-1", question: "first" });
  adapter.close();
  await dispatch;
  await adapter.dispatch({ btwId: "btw-2", question: "second" });

  assert.equal(signal?.aborted, true);
  assert.deepEqual(events, []);
});

test("an absent runtime capability yields an actionable failure", async () => {
  const events: BridgeEvent[] = [];
  const adapter = new SideQuestionAdapter(
    "session-1",
    {} as Query,
    (event) => events.push(event),
  );

  await adapter.dispatch({ btwId: "btw-1", question: "first" });

  assert.equal(events[0]?.event, "btw_failed");
  assert.match(
    events[0]?.event === "btw_failed" ? events[0].error : "",
    /does not support side questions/,
  );
});

test("a terminal event releases the active handle before the host dispatches again", async () => {
  const events: BridgeEvent[] = [];
  let secondDispatch: Promise<void> | undefined;
  const query = {
    async askSideQuestion(question: string) {
      return { response: question, synthetic: false };
    },
  } as unknown as Query;
  const adapter = new SideQuestionAdapter("session-1", query, (event) => {
    events.push(event);
    if (event.event === "btw_result" && event.btw_id === "first") {
      secondDispatch = adapter.dispatch({ btwId: "second", question: "second" });
    }
  });
  await adapter.dispatch({ btwId: "first", question: "first" });
  await secondDispatch;
  assert.deepEqual(events.map((event) => event.event), ["btw_result", "btw_result"]);
});
