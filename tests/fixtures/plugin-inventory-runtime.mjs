// Controlled SDK boundary for the real replacement and same-key handoff callers.
import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { AsyncQueue } from "../../agent-sdk/dist/bridge/shared.js";

const mode = process.argv[2];
assert.ok(["replacement", "omitted", "handoff"].includes(mode));
const root = fs.mkdtempSync(path.join(os.tmpdir(), "claude-rs-plugin-inventory-"));
let closeAll, restore;
const controls = [],
  events = [],
  sessionsCreated = [],
  snapshots = [];
let confirmedEvents;
let releaseOldStatus, titleRead;
globalThis.pluginTitle = () => {
  titleRead = Promise.resolve(undefined);
  return titleRead;
};
const folder = path.join(root, "Local plugin folder"),
  replacementFolder = path.join(root, "Replacement folder");
const id = "11111111-5555-4555-8555-111111111111";
const confirmed = [
  { name: "confirmed-local", path: folder, source: "confirmed-local@inline", version: "1.2.3" },
];
try {
  const cwd = path.join(root, "project"),
    profile = path.join(root, "profile");
  fs.mkdirSync(cwd);
  fs.mkdirSync(profile);
  for (const key of Object.keys(process.env))
    if (/^(ANTHROPIC_|CLAUDE_|CLAUDECODE$)/.test(key)) delete process.env[key];
  process.env.CLAUDE_CONFIG_DIR = profile;
  const sdkUrl = import.meta.resolve("@anthropic-ai/claude-agent-sdk");
  const wrapper = `
    export * from ${JSON.stringify(sdkUrl)};
    export const query = params => globalThis.pluginQuery(params);
    export const resolveSettings = async () => ({effective:{}});
    export const getSessionInfo = () => globalThis.pluginTitle();
    export const listSessions = async () => [];
  `;
  registerHooks({
    resolve(specifier, context, next) {
      return specifier === "@anthropic-ai/claude-agent-sdk"
        ? { url: "fixture:plugins", shortCircuit: true }
        : next(specifier, context);
    },
    load(url, context, next) {
      return url === "fixture:plugins"
        ? { format: "module", source: wrapper, shortCircuit: true }
        : next(url, context);
    },
  });
  globalThis.pluginQuery = ({ options }) => {
    const stream = new AsyncQueue(),
      acknowledgements = new WeakMap();
    const control = {
      name: `query-${controls.length}`,
      options,
      stream,
      closeCalls: 0,
      holdClose: false,
      status: undefined,
    };
    const q = {
      async initializationResult() {
        return {
          commands: [],
          agents: [],
          models: [],
          account: { apiKeySource: "fixture" },
          current_permission_mode: "default",
        };
      },
      async getSettings() {
        assert.equal(control.closeCalls, 0, "closed native query is not usable");
        return {
          applied: {
            model: "claude-sonnet-4-6",
            effort: "medium",
            ultracodeAvailable: false,
            ultracodeRequested: false,
            ultracode: false,
          },
        };
      },
      async supportedAgents() {
        return [];
      },
      async supportedCommands() {
        return [];
      },
      async mcpServerStatus() {
        return control.status
          ? control.status()
          : [
              {
                name: "replacement-server",
                status: "connected",
                serverInfo: { name: "replacement", version: "fresh" },
              },
            ];
      },
      close() {
        control.closeCalls++;
        if (!control.holdClose) stream.close();
      },
      async *[Symbol.asyncIterator]() {
        for await (const message of stream) {
          yield message;
          acknowledgements.get(message)?.();
        }
      },
    };
    control.send = (message) =>
      new Promise((resolve, reject) => {
        const timer = setTimeout(
          () => reject(new Error(`${control.name}: native fixture message was not processed`)),
          2_000,
        );
        acknowledgements.set(message, () => {
          clearTimeout(timer);
          resolve();
        });
        if (!stream.enqueue(message)) {
          clearTimeout(timer);
          reject(new Error("closed fixture input"));
        }
      });
    controls.push(control);
    return q;
  };
  const lifecycle = await import("../../agent-sdk/dist/bridge/session_lifecycle.js");
  const protocol = await import("../../agent-sdk/dist/bridge/events.js");
  const { sessionPluginFolders } = await import("../../agent-sdk/dist/bridge/plugin_inventory.js");
  const { startMcpAuthSnapshotMonitor } = await import("../../agent-sdk/dist/bridge/mcp.js");
  closeAll = lifecycle.closeAllSessions;
  restore = protocol.replaceProtocolEventWriter((line) => events.push(JSON.parse(line)));
  async function open(previous) {
    const session = await lifecycle.createSession({
      cwd,
      sessionId: id,
      resume: previous?.sessionId,
      ...(previous ? {} : { pluginPaths: [folder] }),
      deferConnect: true,
      launchSettings: {},
      connectEvent: previous ? "session_replaced" : "connected",
      requestId: `connect-${controls.length}`,
      ...(previous ? { sessionsToCloseAfterConnect: [previous] } : {}),
    });
    sessionsCreated.push(session);
    await session.initializationTask;
    assert.equal(session.initializationError, undefined);
    lifecycle.commitDeferredSession(session);
    await titleRead;
    assert.strictEqual(lifecycle.sessions.get(id), session);
    return session;
  }
  const init = (plugins) => ({
    type: "system",
    subtype: "init",
    session_id: id,
    model: "claude-sonnet-4-6",
    tools: [],
    commands: [],
    mcp_servers: [],
    ...(plugins === undefined ? {} : { plugins }),
  });
  const initial = await open();
  const confirmationStart = events.length;
  await controls[0].send(init(confirmed));
  confirmedEvents = events.slice(confirmationStart);
  assert.deepEqual(sessionPluginFolders(initial), [folder]);
  if (mode === "handoff") {
    controls[0].holdClose = true;
    let started;
    const entered = new Promise((resolve) => (started = resolve));
    const pending = new Promise((resolve) => (releaseOldStatus = resolve));
    controls[0].status = async () => {
      started();
      return pending;
    };
    const oldMonitor = startMcpAuthSnapshotMonitor(initial, "old-server", {
      maxAttempts: 1,
      sleep: async () => {},
    });
    await entered;
    const before = events.length;
    const replacement = await open(initial);
    assert.ok(
      events
        .slice(before)
        .some((event) => event.event === "session_replaced" && event.session_id === id),
      "replacement connected before stale evidence releases",
    );
    const incoming = [
      {
        name: "replacement-local",
        path: replacementFolder,
        source: "replacement-local@inline",
        version: "2.0.0",
      },
    ];
    await controls[1].send(init(incoming));
    const afterReplacement = events.length;
    releaseOldStatus([
      { name: "old-server", status: "connected", serverInfo: { name: "stale", version: "old" } },
    ]);
    await controls[0].send(
      init([
        { name: "late-old", path: path.join(root, "Old late folder"), source: "late-old@inline" },
      ]),
    );
    controls[0].stream.close();
    const oldOutcome = await oldMonitor;
    await initial.queryConsumerTask;
    await lifecycle.closeSessionWithLogging(initial, { reason: "fixture_stale_settled" });
    const freshOutcome = await startMcpAuthSnapshotMonitor(replacement, "replacement-server", {
      maxAttempts: 1,
      sleep: async () => {},
    });
    assert.strictEqual(lifecycle.sessions.get(id), replacement);
    assert.equal((await replacement.query.getSettings()).applied.model, "claude-sonnet-4-6");
    snapshots.push({
      kind: "handoff",
      events: events.slice(before),
      lateEvents: events.slice(afterReplacement),
      oldOutcome,
      freshOutcome,
      carried: sessionPluginFolders(replacement),
      registered: lifecycle.sessions.get(id) === replacement,
      oldClosed: controls[0].closeCalls > 0,
    });
  } else {
    if (mode === "omitted") {
      const before = events.length;
      await controls[0].send(init(undefined));
      snapshots.push({
        kind: "omitted",
        events: events.slice(before),
        folders: sessionPluginFolders(initial),
      });
    }
    let current = initial;
    for (let index = 0; index < 2; index++) {
      const before = events.length;
      current = await open(current);
      snapshots.push({
        kind: "replacement",
        name: controls.at(-1).name,
        plugins: controls.at(-1).options.plugins,
        events: events.slice(before),
        folders: sessionPluginFolders(current),
      });
    }
    const before = events.length;
    await controls.at(-1).send(init([]));
    snapshots.push({
      kind: "empty",
      events: events.slice(before),
      folders: sessionPluginFolders(current),
    });
    const next = await open(current);
    snapshots.push({
      kind: "after-empty",
      plugins: controls.at(-1).options.plugins,
      folders: sessionPluginFolders(next),
    });
  }
  await closeAll();
  await Promise.all(
    sessionsCreated.flatMap((session) => [session.initializationTask, session.queryConsumerTask]),
  );
  await protocol.emitSessionsList();
  restore();
  restore = undefined;
  console.log(
    JSON.stringify({
      id,
      folder,
      replacementFolder,
      confirmedEvents,
      snapshots,
      queries: controls.map((control) => ({
        name: control.name,
        plugins: control.options.plugins,
        closed: control.closeCalls > 0,
      })),
    }),
  );
} finally {
  releaseOldStatus?.([]);
  for (const control of controls) control.stream.close();
  try {
    await closeAll?.();
  } finally {
    restore?.();
    fs.rmSync(root, { recursive: true, force: true });
  }
}
