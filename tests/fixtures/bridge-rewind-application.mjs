import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { registerHooks } from "node:module";
import { AsyncQueue } from "../../agent-sdk/dist/bridge/shared.js";

const root = fs.mkdtempSync(path.join(os.tmpdir(), "bridge-rewind-application-"));
const sourceId = "22222222-0000-4000-8000-000000000001";
const before = "original file\nkept before rewind\n";
const after = "restored checkpoint\nindependent file bytes\n";
const history = [
  {
    type: "user",
    uuid: "first-input",
    message: { role: "user", content: [{ type: "text", text: "FIRST input\n exact  " }] },
  },
  {
    type: "assistant",
    uuid: "kept-assistant",
    message: {
      role: "assistant",
      content: [
        { type: "text", text: "Retained answer" },
        { type: "tool_use", id: "kept-call", name: "Bash", input: { command: "true" } },
      ],
    },
  },
  {
    type: "user",
    uuid: "kept-tool-anchor",
    message: {
      role: "user",
      content: [{ type: "tool_result", tool_use_id: "kept-call", content: "Retained receipt" }],
    },
  },
  {
    type: "user",
    uuid: "selected-input",
    message: { role: "user", content: [{ type: "text", text: "SELECTED input\n exact  " }] },
  },
];
function gate() {
  let resolve;
  const promise = new Promise((done) => {
    resolve = done;
  });
  return { promise, resolve };
}
let lifecycle, protocol, restore, current;
const reports = [];
try {
  for (const key of Object.keys(process.env))
    if (/^(ANTHROPIC_|CLAUDE_|CLAUDECODE$)/.test(key)) delete process.env[key];
  process.env.CLAUDE_CONFIG_DIR = path.join(root, "profile");
  fs.mkdirSync(process.env.CLAUDE_CONFIG_DIR);
  const sdkUrl = import.meta.resolve("@anthropic-ai/claude-agent-sdk");
  const wrapper = `export * from ${JSON.stringify(sdkUrl)};export const resolveSettings=async()=>({effective:{}});export const query=params=>globalThis.rewindApplicationQuery(params);export const getSessionInfo=async()=>undefined;export const listSessions=async()=>[];export const getSessionMessages=(id,options)=>globalThis.rewindApplicationMessages(id,options);`;
  const loader = registerHooks({
    resolve(specifier, context, next) {
      return specifier === "@anthropic-ai/claude-agent-sdk"
        ? { url: "fixture:rewind-application", shortCircuit: true }
        : next(specifier, context);
    },
    load(url, context, next) {
      return url === "fixture:rewind-application"
        ? { format: "module", source: wrapper, shortCircuit: true }
        : next(url, context);
    },
  });
  globalThis.rewindApplicationMessages = async (id, options) => {
    current.journal.push({ method: "history", id, options });
    assert.equal(id, sourceId);
    return structuredClone(history);
  };
  globalThis.rewindApplicationQuery = ({ options }) => {
    const run = current,
      index = run.controls.length,
      stream = new AsyncQueue();
    const control = { stream, closed: false, failStream: false };
    run.controls.push(control);
    run.journal.push({
      method: "query",
      index,
      options: {
        cwd: options.cwd,
        sessionId: options.sessionId,
        resume: options.resume,
        resumeSessionAt: options.resumeSessionAt,
        resumeDropsTurn: options.resumeDropsTurn,
        forkSession: options.forkSession,
      },
    });
    const query = {
      initializationResult: async () => {
        run.journal.push({ method: "init-start", index });
        if (index === 1) {
          run.initEntered.resolve();
          if (run.name === "both") await run.initRelease.promise;
          if (run.name === "init-failure") throw new Error("OWNED-CANDIDATE-INIT-CAUSE");
        }
        run.journal.push({ method: "init-complete", index });
        return { commands: [], agents: [], models: [], account: { apiKeySource: "fixture" } };
      },
      getSettings: async () => ({
        applied: {
          model: "haiku",
          effort: "medium",
          ultracodeAvailable: false,
          ultracodeRequested: false,
          ultracode: false,
        },
      }),
      getContextUsage: async function (options) {
        assert.equal(this, query);
        run.journal.push({ method: "context", index, options });
        if (index === 1 && run.name === "both") {
          run.guardEntered.resolve();
          await run.guardRelease.promise;
        }
        if (index === 1 && run.name === "guard-failure") throw new Error("OWNED-GUARD-CAUSE");
        return { percentage: 23, model: { id: "haiku" } };
      },
      rewindFiles: async function (target, options) {
        assert.equal(this, query);
        run.journal.push({ method: "rewindFiles", index, phase: run.phase, target, options });
        assert.equal(index, 0);
        assert.equal(target, "selected-input");
        assert.deepEqual(options, { dryRun: options.dryRun });
        if (options.dryRun) {
          if (run.name === "dry-refusal" && run.phase === "operation")
            return { canRewind: false, error: "OWNED-DRY-REFUSAL" };
        } else {
          if (run.name === "apply-failure" && run.phase === "operation")
            throw new Error("OWNED-APPLY-CAUSE");
          if (run.name === "post-apply-failure" && run.phase === "operation") {
            run.applyEntered.resolve();
            await run.applyRelease.promise;
          }
          fs.writeFileSync(run.file, after);
        }
        return {
          canRewind: true,
          filesChanged: [run.file],
          insertions: 13,
          deletions: 4,
          skippedLinks: 2,
        };
      },
      close: () => {
        control.closed = true;
        run.journal.push({ method: "close", index });
        stream.close();
      },
      async *[Symbol.asyncIterator]() {
        for await (const message of stream) yield message;
        if (control.failStream) throw new Error("OWNED-CANDIDATE-STREAM-CAUSE");
      },
    };
    control.query = query;
    return query;
  };
  lifecycle = await import("../../agent-sdk/dist/bridge/session_lifecycle.js");
  protocol = await import("../../agent-sdk/dist/bridge/events.js");
  const { parseCommandEnvelope } = await import("../../agent-sdk/dist/bridge/commands.js");
  const { BridgeCommandScheduler } = await import(
    "../../agent-sdk/dist/bridge/command_scheduler.js"
  );
  const { scheduleBridgeCommand } = await import("../../agent-sdk/dist/bridge.js");
  loader.deregister();
  for (const name of [
    "conversation",
    "code",
    "both",
    "first-prompt",
    "dry-refusal",
    "init-failure",
    "guard-failure",
    "apply-failure",
    "post-apply-failure",
  ]) {
    const cwd = path.join(root, name);
    fs.mkdirSync(cwd);
    const file = path.join(cwd, "owned.txt");
    fs.writeFileSync(file, before);
    const run = (current = {
      name,
      file,
      cwd,
      phase: "operation",
      journal: [],
      events: [],
      controls: [],
      initEntered: gate(),
      initRelease: gate(),
      guardEntered: gate(),
      guardRelease: gate(),
      applyEntered: gate(),
      applyRelease: gate(),
    });
    restore = protocol.replaceProtocolEventWriter((line) => run.events.push(JSON.parse(line)));
    protocol.setSessionListingDir(cwd);
    const source = await lifecycle.createSession({
      cwd,
      sessionId: sourceId,
      deferConnect: true,
      launchSettings: {},
      connectEvent: "connected",
    });
    await lifecycle.awaitSessionInitialization(source);
    lifecycle.commitDeferredSession(source);
    await protocol.emitSessionsList();
    run.events.length = 0;
    const scheduler = new BridgeCommandScheduler();
    const dispatch = (command) => {
      const parsed = parseCommandEnvelope(JSON.stringify(command));
      return scheduleBridgeCommand(scheduler, parsed);
    };
    const mode =
      name === "conversation" || name === "first-prompt"
        ? "conversation"
        : name === "code"
          ? "code"
          : "both";
    const pending = dispatch({
      command: "rewind",
      session_id: sourceId,
      request_id: `operation-${name}`,
      target_user_message_id: name === "first-prompt" ? "first-input" : "selected-input",
      restore_mode: mode,
    });
    if (name === "both") {
      await run.initEntered.promise;
      await new Promise((resolve) => setImmediate(resolve));
      run.validationBarriers = [
        {
          stage: "initialization",
          bytes: fs.readFileSync(file, "utf8"),
          applications: run.journal.filter(
            (call) => call.method === "rewindFiles" && call.options.dryRun === false,
          ).length,
        },
      ];
      run.initRelease.resolve();
      await run.guardEntered.promise;
      await new Promise((resolve) => setImmediate(resolve));
      run.validationBarriers.push({
        stage: "guard",
        bytes: fs.readFileSync(file, "utf8"),
        applications: run.journal.filter(
          (call) => call.method === "rewindFiles" && call.options.dryRun === false,
        ).length,
      });
      run.guardRelease.resolve();
    }
    if (name === "post-apply-failure") {
      await run.applyEntered.promise;
      const candidate = run.controls[1];
      assert.ok(candidate);
      const session = [...lifecycle.sessions.values()].find(
        (session) => session.query === candidate.query,
      );
      assert.ok(session?.queryConsumerTask);
      candidate.failStream = true;
      candidate.stream.close();
      await session.queryConsumerTask;
      run.applyRelease.resolve();
    }
    await pending;
    await protocol.emitSessionsList();
    const operation = {
      bytes: fs.readFileSync(file, "utf8"),
      events: [...run.events],
      journal: structuredClone(run.journal),
      candidatesClosed: run.controls.slice(1).every((control) => control.closed),
      activeIds: [...lifecycle.sessions.keys()],
    };
    if (
      [
        "dry-refusal",
        "init-failure",
        "guard-failure",
        "apply-failure",
        "post-apply-failure",
      ].includes(name)
    ) {
      run.phase = "recovery";
      await dispatch({
        command: "get_context_usage",
        session_id: sourceId,
        request_id: `usable-${name}`,
      });
      await dispatch({
        command: "rewind",
        session_id: sourceId,
        request_id: `retry-${name}`,
        target_user_message_id: "selected-input",
        restore_mode: "code",
      });
    }
    await lifecycle.closeAllSessions();
    await protocol.emitSessionsList();
    reports.push({
      name,
      sourceId,
      cwd,
      file,
      operation,
      validationBarriers: run.validationBarriers,
      finalBytes: fs.readFileSync(file, "utf8"),
      events: run.events,
      journal: run.journal,
      allQueriesClosed: run.controls.every((control) => control.closed),
    });
    restore();
    restore = undefined;
  }
  process.stdout.write(JSON.stringify(reports));
} finally {
  current?.initRelease.resolve();
  current?.guardRelease.resolve();
  current?.applyRelease.resolve();
  for (const control of current?.controls ?? []) control.stream.close();
  try {
    await lifecycle?.closeAllSessions();
  } finally {
    restore?.();
    fs.rmSync(root, { recursive: true, force: true });
  }
}
