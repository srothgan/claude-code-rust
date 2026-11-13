import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { registerHooks } from "node:module";
import { AsyncQueue } from "../../agent-sdk/dist/bridge/shared.js";

const root = fs.mkdtempSync(path.join(os.tmpdir(), "bridge-close-interactions-"));
const controls = [],
  events = [],
  waiters = [],
  outcomes = [];
let lifecycle, restore;
function bounded(operation) {
  let timer;
  return Promise.race([
    operation,
    new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error("close-interaction fixture deadline")), 4000);
    }),
  ]).finally(() => clearTimeout(timer));
}
function waitForRequest(sessionId) {
  const matches = (event) =>
    event.session_id === sessionId &&
    ["permission_request", "question_request", "user_dialog_request"].includes(event.event);
  const existing = events.find(matches);
  if (existing) return Promise.resolve(existing);
  return bounded(new Promise((resolve) => waiters.push({ matches, resolve })));
}
try {
  for (const key of Object.keys(process.env))
    if (/^(ANTHROPIC_|CLAUDE_|CLAUDECODE$)/.test(key)) delete process.env[key];
  process.env.CLAUDE_CONFIG_DIR = path.join(root, "profile");
  fs.mkdirSync(process.env.CLAUDE_CONFIG_DIR);
  const cwd = path.join(root, "project");
  fs.mkdirSync(cwd);
  const sdkUrl = import.meta.resolve("@anthropic-ai/claude-agent-sdk");
  const wrapper = `export * from ${JSON.stringify(sdkUrl)};export const query=params=>globalThis.closeInteractionQuery(params);export const resolveSettings=async()=>({effective:{}});export const getSessionInfo=async()=>undefined;export const listSessions=async()=>[];`;
  const loader = registerHooks({
    resolve(specifier, context, next) {
      return specifier === "@anthropic-ai/claude-agent-sdk"
        ? { url: "fixture:close-interactions", shortCircuit: true }
        : next(specifier, context);
    },
    load(url, context, next) {
      return url === "fixture:close-interactions"
        ? { format: "module", source: wrapper, shortCircuit: true }
        : next(url, context);
    },
  });
  globalThis.closeInteractionQuery = ({ options }) => {
    const stream = new AsyncQueue();
    const control = { options, stream, hold: false };
    controls.push(control);
    return {
      initializationResult: async () => ({
        commands: [],
        agents: [],
        models: [],
        account: { apiKeySource: "fixture" },
      }),
      getSettings: async () => ({
        applied: {
          model: "haiku",
          effort: "medium",
          ultracodeAvailable: false,
          ultracodeRequested: false,
          ultracode: false,
        },
        sources: [],
      }),
      close() {
        if (!control.hold) stream.close();
      },
      [Symbol.asyncIterator]: () => stream[Symbol.asyncIterator](),
    };
  };
  lifecycle = await import("../../agent-sdk/dist/bridge/session_lifecycle.js");
  const protocol = await import("../../agent-sdk/dist/bridge/events.js");
  loader.deregister();
  restore = protocol.replaceProtocolEventWriter((line) => {
    const event = JSON.parse(line);
    events.push(event);
    for (const waiter of [...waiters])
      if (waiter.matches(event)) {
        waiters.splice(waiters.indexOf(waiter), 1);
        waiter.resolve(event);
      }
  });
  async function open(id) {
    const session = await lifecycle.createSession({
      cwd,
      sessionId: id,
      deferConnect: true,
      launchSettings: {},
      connectEvent: "connected",
    });
    await bounded(session.initializationTask);
    assert.equal(session.initializationError, undefined);
    lifecycle.commitDeferredSession(session);
    await protocol.emitSessionsList();
    return { session, control: controls.at(-1) };
  }
  function begin(kind, owner, signal, id) {
    if (kind === "dialog")
      return owner.control.options.onUserDialog(
        {
          dialogKind: "refusal_fallback_prompt",
          payload: {
            originalModel: "original-model",
            fallbackModel: "fallback-model",
            guidanceText: "Choose recovery",
          },
        },
        { signal, requestId: id },
      );
    const names = { permission: "Bash", plan: "ExitPlanMode", question: "AskUserQuestion" };
    const inputs = {
      permission: { command: "echo fixture" },
      plan: { plan: "Actual independently supplied plan" },
      question: {
        questions: [
          {
            question: "Choose target",
            header: "Target",
            multiSelect: false,
            options: [
              { label: "Yes", description: "Proceed" },
              { label: "No", description: "Stop" },
            ],
          },
        ],
      },
    };
    return owner.control.options.canUseTool(names[kind], inputs[kind], {
      signal,
      toolUseID: id,
      suggestions: [],
    });
  }
  function answer(kind, owner, id) {
    if (kind === "dialog")
      lifecycle.handleUserDialogResponse({
        command: "user_dialog_response",
        session_id: owner.session.sessionId,
        request_id: id,
        outcome: { outcome: "selected", option_id: "retry_fallback" },
      });
    else if (kind === "question")
      lifecycle.handleQuestionResponse({
        command: "question_response",
        session_id: owner.session.sessionId,
        tool_call_id: id,
        outcome: { outcome: "answered", selected_option_ids: ["question_0"] },
      });
    else
      lifecycle.handlePermissionResponse({
        command: "permission_response",
        session_id: owner.session.sessionId,
        tool_call_id: id,
        outcome: { outcome: "selected", option_id: kind === "plan" ? "approve" : "allow_once" },
      });
  }
  for (const kind of ["permission", "plan", "question", "dialog"]) {
    const target = await open(`close-${kind}`),
      sibling = await open(`sibling-${kind}`);
    const targetAbort = new AbortController(),
      siblingAbort = new AbortController();
    const targetId = `target-${kind}`,
      siblingId = `sibling-${kind}`;
    let closing,
      closed = false,
      siblingSettled = false;
    target.control.hold = true;
    const targetResult = begin(kind, target, targetAbort.signal, targetId);
    const siblingResult = begin(kind, sibling, siblingAbort.signal, siblingId).then((value) => {
      siblingSettled = true;
      return value;
    });
    try {
      const [targetRequest, siblingRequest] = await Promise.all([
        waitForRequest(target.session.sessionId),
        waitForRequest(sibling.session.sessionId),
      ]);
      assert.equal(
        kind === "dialog"
          ? targetRequest.request.request_id
          : targetRequest.request.tool_call.tool_call_id,
        targetId,
      );
      assert.equal(
        kind === "dialog"
          ? siblingRequest.request.request_id
          : siblingRequest.request.tool_call.tool_call_id,
        siblingId,
      );
      closing = lifecycle.closeSession(target.session).then(() => {
        closed = true;
      });
      const cancelled = await bounded(targetResult);
      assert.equal(cancelled.behavior, kind === "dialog" ? "cancelled" : "deny");
      if (kind === "dialog") assert.deepEqual(cancelled, { behavior: "cancelled" });
      assert.equal(closed, false, "callback is released before waiting on the gated consumer");
      await new Promise((resolve) => setImmediate(resolve));
      assert.equal(siblingSettled, false);
      const beforeLate = events.length;
      answer(kind, target, targetId);
      targetAbort.abort();
      await new Promise((resolve) => setImmediate(resolve));
      assert.deepEqual(
        events.slice(beforeLate),
        [],
        "late host/native answers do not publish twice",
      );
      answer(kind, sibling, siblingId);
      const accepted = await bounded(siblingResult);
      if (kind === "dialog")
        assert.deepEqual(accepted, { behavior: "completed", result: "retry_fallback" });
      else assert.equal(accepted.behavior, "allow");
      if (kind === "question")
        assert.deepEqual(accepted.updatedInput.answers, { "Choose target": "Yes" });
      target.control.stream.close();
      await bounded(closing);
      outcomes.push({
        kind,
        targetId,
        siblingId,
        cancelled,
        accepted,
        callbackBeforeConsumerClose: true,
        lateEvents: events
          .slice(beforeLate)
          .filter((event) => event.session_id === target.session.sessionId),
      });
    } finally {
      targetAbort.abort();
      siblingAbort.abort();
      target.control.stream.close();
      sibling.control.stream.close();
      await bounded(Promise.allSettled([targetResult, siblingResult, closing]));
      await bounded(lifecycle.closeSession(target.session));
      await bounded(lifecycle.closeSession(sibling.session));
      for (const owner of [target, sibling])
        if (lifecycle.sessions.get(owner.session.sessionId) === owner.session)
          lifecycle.sessions.delete(owner.session.sessionId);
    }
  }
  await lifecycle.closeAllSessions();
  await protocol.emitSessionsList();
  restore();
  restore = undefined;
  process.stdout.write(JSON.stringify({ outcomes }));
} finally {
  for (const control of controls) control.stream.close();
  try {
    await lifecycle?.closeAllSessions();
  } finally {
    restore?.();
    fs.rmSync(root, { recursive: true, force: true });
  }
}
