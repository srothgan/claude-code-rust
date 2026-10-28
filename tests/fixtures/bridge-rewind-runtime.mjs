import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { registerHooks } from "node:module";
import { AsyncQueue } from "../../agent-sdk/dist/bridge/shared.js";

assert.equal(process.argv[2], "targets");
const root = fs.mkdtempSync(path.join(os.tmpdir(), "bridge-rewind-"));
const ids = {
  active: "11111111-0000-4000-8000-000000000001",
  peer: "11111111-0000-4000-8000-000000000002",
  saved: "11111111-0000-4000-8000-000000000003",
  empty: "11111111-0000-4000-8000-000000000004",
  failure: "11111111-0000-4000-8000-000000000005",
};
const dirs = Object.fromEntries(
  ["active", "peer", "saved", "empty", "failure", "listing"].map((name) => [
    name,
    path.join(root, name),
  ]),
);
const events = [],
  journal = [],
  streams = [];
let lifecycle,
  protocol,
  restore,
  activeRequest,
  failRead = true;
function history(prefix, text) {
  return [
    {
      type: "user",
      uuid: `${prefix}-first`,
      message: { role: "user", content: [{ type: "text", text: `${text} first input` }] },
    },
    {
      type: "assistant",
      uuid: `${prefix}-reply`,
      message: { role: "assistant", content: [{ type: "text", text: `${text} kept reply` }] },
    },
    {
      type: "user",
      uuid: `${prefix}-tool`,
      message: {
        role: "user",
        content: [{ type: "tool_result", tool_use_id: "prior-tool", content: "Kept tool receipt" }],
      },
    },
    {
      type: "user",
      uuid: `${prefix}-latest`,
      message: { role: "user", content: [{ type: "text", text: `${text} latest\ninput` }] },
    },
  ];
}
try {
  for (const key of Object.keys(process.env))
    if (/^(ANTHROPIC_|CLAUDE_|CLAUDECODE$)/.test(key)) delete process.env[key];
  process.env.CLAUDE_CONFIG_DIR = path.join(root, "profile");
  fs.mkdirSync(process.env.CLAUDE_CONFIG_DIR);
  for (const dir of Object.values(dirs)) fs.mkdirSync(dir);
  const sdkUrl = import.meta.resolve("@anthropic-ai/claude-agent-sdk");
  const wrapper = `export * from ${JSON.stringify(sdkUrl)};export const resolveSettings=async()=>({effective:{}});export const query=()=>globalThis.rewindQuery();export const getSessionInfo=async()=>undefined;export const listSessions=options=>globalThis.rewindList(options);export const getSessionMessages=(id,options)=>globalThis.rewindMessages(id,options);`;
  const loader = registerHooks({
    resolve(specifier, context, next) {
      return specifier === "@anthropic-ai/claude-agent-sdk"
        ? { url: "fixture:rewind", shortCircuit: true }
        : next(specifier, context);
    },
    load(url, context, next) {
      return url === "fixture:rewind"
        ? { format: "module", source: wrapper, shortCircuit: true }
        : next(url, context);
    },
  });
  globalThis.rewindList = async (options) => {
    journal.push({ request: activeRequest, method: "listSessions", options });
    return ["saved", "empty", "failure"].map((name) => ({
      sessionId: ids[name],
      cwd: dirs[name],
      summary: `${name} history`,
      lastModified: 1780662896000,
    }));
  };
  globalThis.rewindMessages = async (id, options) => {
    journal.push({ request: activeRequest, method: "getSessionMessages", id, options });
    if (id === ids.active) return history("active", "Active");
    if (id === ids.saved) return history("saved", "Saved");
    if (id === ids.empty) return [];
    if (id === ids.failure) {
      if (failRead) {
        failRead = false;
        throw new Error("OWNED-HISTORY-READ-CAUSE");
      }
      return [
        {
          type: "user",
          uuid: "recovered-user",
          message: { role: "user", content: [{ type: "text", text: "Recovered input" }] },
        },
      ];
    }
    throw new Error("wrong history session");
  };
  globalThis.rewindQuery = () => {
    const stream = new AsyncQueue();
    streams.push(stream);
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
      }),
      close: () => stream.close(),
      [Symbol.asyncIterator]: () => stream[Symbol.asyncIterator](),
    };
  };
  lifecycle = await import("../../agent-sdk/dist/bridge/session_lifecycle.js");
  protocol = await import("../../agent-sdk/dist/bridge/events.js");
  const { handleSessionDataCommand } = await import(
    "../../agent-sdk/dist/bridge/command_session_data.js"
  );
  const { parseCommandEnvelope } = await import("../../agent-sdk/dist/bridge/commands.js");
  const { rewindTargetsFromSessionMessages } = await import("../../agent-sdk/dist/bridge.js");
  loader.deregister();
  restore = protocol.replaceProtocolEventWriter((line) => events.push(JSON.parse(line)));
  protocol.setSessionListingDir(dirs.listing);
  for (const name of ["active", "peer"]) {
    const session = await lifecycle.createSession({
      cwd: dirs[name],
      sessionId: ids[name],
      deferConnect: true,
      launchSettings: {},
      connectEvent: "connected",
    });
    await session.initializationTask;
    assert.equal(session.initializationError, undefined);
    lifecycle.commitDeferredSession(session);
    await protocol.emitSessionsList();
  }
  const deps = {
    rewindTargetsFromSessionMessages,
    generatePersistedSessionTitle: async () => assert.fail("unexpected title"),
    buildSessionMutationOptions: () => undefined,
    handleRewind: async () => assert.fail("unexpected rewind"),
  };
  for (const [key, name] of [
    ["active", "active"],
    ["saved", "saved"],
    ["empty", "empty"],
    ["failure", "failure"],
    ["recovered", "failure"],
  ]) {
    activeRequest = `targets-${key}`;
    const parsed = parseCommandEnvelope(
      JSON.stringify({
        command: "get_rewind_targets",
        session_id: ids[name],
        request_id: activeRequest,
      }),
    );
    await handleSessionDataCommand(parsed.command, parsed.requestId, deps);
    activeRequest = undefined;
  }
  await lifecycle.closeAllSessions();
  await protocol.emitSessionsList();
  restore();
  restore = undefined;
  process.stdout.write(
    JSON.stringify({
      ids,
      dirs,
      journal,
      responses: events.filter((event) => event.event === "rewind_targets"),
    }),
  );
} finally {
  for (const stream of streams) stream.close();
  try {
    await lifecycle?.closeAllSessions();
  } finally {
    restore?.();
    fs.rmSync(root, { recursive: true, force: true });
  }
}
