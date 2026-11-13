import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { registerHooks } from "node:module";
import { AsyncQueue } from "../../agent-sdk/dist/bridge/shared.js";

const root = fs.mkdtempSync(path.join(os.tmpdir(), "bridge-authority-"));
const originalId = "44444444-0000-4000-8000-000000000001";
const replacementId = "44444444-0000-4000-8000-000000000002";
const stream = new AsyncQueue(),
  events = [],
  journal = [],
  pending = new Set();
let lifecycle,
  restore,
  closed = false;
const phases = {};
function read(method, value) {
  journal.push({ method });
  const operation = Promise.resolve(value);
  pending.add(operation);
  void operation.finally(() => pending.delete(operation));
  return operation;
}
async function settle() {
  while (pending.size) await Promise.all([...pending]);
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(pending.size, 0);
}
try {
  for (const key of Object.keys(process.env))
    if (/^(ANTHROPIC_|CLAUDE_|CLAUDECODE$)/.test(key)) delete process.env[key];
  process.env.CLAUDE_CONFIG_DIR = path.join(root, "profile");
  fs.mkdirSync(process.env.CLAUDE_CONFIG_DIR);
  const sdkUrl = import.meta.resolve("@anthropic-ai/claude-agent-sdk");
  const wrapper = `export * from ${JSON.stringify(sdkUrl)};export const resolveSettings=async()=>({effective:{}});export const query=()=>globalThis.authorityQuery();export const getSessionInfo=async()=>undefined;export const listSessions=async()=>[];`;
  const loader = registerHooks({
    resolve(specifier, context, next) {
      return specifier === "@anthropic-ai/claude-agent-sdk"
        ? { url: "fixture:authority", shortCircuit: true }
        : next(specifier, context);
    },
    load(url, context, next) {
      return url === "fixture:authority"
        ? { format: "module", source: wrapper, shortCircuit: true }
        : next(url, context);
    },
  });
  globalThis.authorityQuery = () => {
    const query = {
      initializationResult: () =>
        read("initializationResult", {
          commands: [],
          agents: [],
          models: [],
          account: { apiKeySource: "fixture" },
          fast_mode_state: "off",
          fast_mode_disabled_reason: "model_not_allowed",
        }),
      getSettings: function () {
        assert.equal(this, query);
        return read("getSettings", {
          applied: {
            model: "haiku",
            effort: "medium",
            ultracodeAvailable: false,
            ultracodeRequested: false,
            ultracode: false,
          },
        });
      },
      supportedCommands: function () {
        assert.equal(this, query);
        return read("supportedCommands", []);
      },
      supportedAgents: function () {
        assert.equal(this, query);
        return read("supportedAgents", []);
      },
      setPermissionMode: async function (mode) {
        assert.equal(this, query);
        journal.push({ method: "setPermissionMode", mode });
      },
      mcpAuthenticate: async () => assert.fail("unexpected authentication"),
      mcpClearAuth: false,
      mcpSubmitOAuthCallbackUrl: async () => assert.fail("unexpected callback"),
      close: () => {
        closed = true;
        stream.close();
      },
      [Symbol.asyncIterator]: () => stream[Symbol.asyncIterator](),
    };
    return query;
  };
  lifecycle = await import("../../agent-sdk/dist/bridge/session_lifecycle.js");
  const protocol = await import("../../agent-sdk/dist/bridge/events.js");
  const { handleSdkMessage } = await import("../../agent-sdk/dist/bridge/message_handlers.js");
  const { parseCommandEnvelope } = await import("../../agent-sdk/dist/bridge/commands.js");
  const { BridgeCommandScheduler } = await import(
    "../../agent-sdk/dist/bridge/command_scheduler.js"
  );
  const { scheduleBridgeCommand } = await import("../../agent-sdk/dist/bridge.js");
  loader.deregister();
  restore = protocol.replaceProtocolEventWriter((line) => events.push(JSON.parse(line)));
  protocol.setSessionListingDir(root);
  const collect = async (name, operation) => {
    const eventStart = events.length,
      journalStart = journal.length;
    await operation();
    await settle();
    await protocol.emitSessionsList();
    phases[name] = { events: events.slice(eventStart), journal: journal.slice(journalStart) };
  };
  let session;
  await collect("initial", async () => {
    session = await lifecycle.createSession({
      cwd: root,
      sessionId: originalId,
      launchSettings: {},
      connectEvent: "connected",
      requestId: "initial-authority-request",
    });
    await lifecycle.awaitSessionInitialization(session);
  });
  const scheduler = new BridgeCommandScheduler();
  await collect("command", async () => {
    await scheduleBridgeCommand(
      scheduler,
      parseCommandEnvelope(
        JSON.stringify({
          command: "set_mode",
          session_id: originalId,
          request_id: "authority-mode-command",
          mode: "plan",
        }),
      ),
    );
  });
  await collect("same", async () =>
    handleSdkMessage(session, {
      type: "system",
      subtype: "init",
      uuid: "same-mode-native-init",
      session_id: originalId,
      model: "haiku",
      permissionMode: "plan",
      fast_mode_state: "on",
      mcp_servers: [
        { name: "project-docs", status: "connected", source: "project" },
        { name: "plugin-search", status: "pending", source: "plugin" },
      ],
      slash_commands: [],
      agents: [],
    }),
  );
  await collect("different", async () =>
    handleSdkMessage(session, {
      type: "system",
      subtype: "init",
      uuid: "different-mode-native-init",
      session_id: originalId,
      model: "haiku",
      permissionMode: "acceptEdits",
      fast_mode_state: "on",
      slash_commands: [],
      agents: [],
    }),
  );
  await collect("replacement", async () =>
    handleSdkMessage(session, {
      type: "system",
      subtype: "init",
      uuid: "replacement-native-init",
      session_id: replacementId,
      model: "haiku",
      permissionMode: "default",
      fast_mode_state: "cooldown",
      fast_mode_disabled_reason: "network_error",
      slash_commands: [],
      agents: [],
    }),
  );
  await collect("live", async () =>
    handleSdkMessage(session, {
      type: "system",
      subtype: "init",
      uuid: "live-native-init",
      session_id: replacementId,
      model: "haiku",
      permissionMode: "plan",
      fast_mode_state: "off",
      fast_mode_disabled_reason: "future-native-reason",
      slash_commands: [],
      agents: [],
    }),
  );
  await lifecycle.closeAllSessions();
  await settle();
  await protocol.emitSessionsList();
  restore();
  restore = undefined;
  process.stdout.write(
    JSON.stringify({ originalId, replacementId, phases, allQueriesClosed: closed }),
  );
} finally {
  stream.close();
  try {
    await lifecycle?.closeAllSessions();
  } finally {
    restore?.();
    fs.rmSync(root, { recursive: true, force: true });
  }
}
