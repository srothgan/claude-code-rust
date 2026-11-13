import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { registerHooks } from "node:module";
import { AsyncQueue } from "../../agent-sdk/dist/bridge/shared.js";

const root = fs.mkdtempSync(path.join(os.tmpdir(), "bridge-tool-policy-"));
const stream = new AsyncQueue(),
  events = [];
let installed,
  closed = false,
  lifecycle,
  restore;
try {
  for (const key of Object.keys(process.env))
    if (/^(ANTHROPIC_|CLAUDE_|CLAUDECODE$)/.test(key)) delete process.env[key];
  process.env.CLAUDE_CONFIG_DIR = path.join(root, "profile");
  fs.mkdirSync(process.env.CLAUDE_CONFIG_DIR);
  const sdkUrl = import.meta.resolve("@anthropic-ai/claude-agent-sdk");
  const wrapper = `export * from ${JSON.stringify(sdkUrl)};export const resolveSettings=async()=>({effective:{}});export const query=params=>globalThis.policyQuery(params);export const getSessionInfo=async()=>undefined;export const listSessions=async()=>[];`;
  const loader = registerHooks({
    resolve(specifier, context, next) {
      return specifier === "@anthropic-ai/claude-agent-sdk"
        ? { url: "fixture:tool-policy", shortCircuit: true }
        : next(specifier, context);
    },
    load(url, context, next) {
      return url === "fixture:tool-policy"
        ? { format: "module", source: wrapper, shortCircuit: true }
        : next(url, context);
    },
  });
  globalThis.policyQuery = ({ options }) => {
    installed = options;
    const queryStream = process.argv[2] === "permissions" ? new AsyncQueue() : stream;
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
      close: () => {
        closed = true;
        queryStream.close();
      },
      [Symbol.asyncIterator]: () => queryStream[Symbol.asyncIterator](),
    };
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
  const session = await lifecycle.createSession({
    cwd: root,
    sessionId: "33333333-0000-4000-8000-000000000001",
    deferConnect: true,
    launchSettings: {},
    connectEvent: "connected",
  });
  await lifecycle.awaitSessionInitialization(session);
  lifecycle.commitDeferredSession(session);
  await protocol.emitSessionsList();
  events.length = 0;
  assert.equal(typeof installed?.canUseTool, "function");
  if (process.argv[2] === "permissions") {
    const targetOptions = installed;
    const peer = await lifecycle.createSession({
      cwd: root,
      sessionId: "33333333-0000-4000-8000-000000000002",
      deferConnect: true,
      launchSettings: {},
      connectEvent: "connected",
    });
    await lifecycle.awaitSessionInitialization(peer);
    lifecycle.commitDeferredSession(peer);
    await protocol.emitSessionsList();
    const peerOptions = installed;
    events.length = 0;
    const scheduler = new BridgeCommandScheduler();
    const { runPermissionAudit } = await import("./permission-audit-cases.mjs");
    const rows = await runPermissionAudit({
      session,
      targetOptions,
      peer,
      peerOptions,
      events,
      mode: process.argv[3],
      respond: async (sessionId, toolId, outcome) =>
        scheduleBridgeCommand(
          scheduler,
          parseCommandEnvelope(
            JSON.stringify({
              command: "permission_response",
              session_id: sessionId,
              tool_call_id: toolId,
              outcome,
            }),
          ),
        ),
    });
    await lifecycle.closeAllSessions();
    await protocol.emitSessionsList();
    restore();
    restore = undefined;
    process.stdout.write(JSON.stringify({ rows, allQueriesClosed: closed }));
  } else {
    const hiddenInput = { query: "Independent indexed search", limit: 7 };
    const hiddenResult = await installed.canUseTool("ToolSearch", hiddenInput, {
      toolUseID: "hidden-search",
      requestId: "hidden-permission",
      signal: new AbortController().signal,
    });
    for (const frame of [
      {
        type: "stream_event",
        uuid: "hidden-start",
        event: {
          type: "content_block_start",
          content_block: {
            type: "server_tool_use",
            id: "hidden-search",
            name: "ToolSearch",
            input: hiddenInput,
          },
        },
      },
      {
        type: "tool_progress",
        uuid: "hidden-progress",
        tool_use_id: "hidden-search",
        tool_name: "ToolSearch",
        elapsed_time_seconds: 1,
      },
      {
        type: "user",
        uuid: "hidden-result",
        parent_tool_use_id: "hidden-search",
        tool_use_result: { content: "HIDDEN-SEARCH-OUTPUT", is_error: false },
        message: {
          role: "user",
          content: [
            {
              type: "tool_search_tool_result",
              tool_use_id: "hidden-search",
              content: "HIDDEN-SEARCH-OUTPUT",
              is_error: false,
            },
          ],
        },
      },
      {
        type: "tool_use_summary",
        uuid: "hidden-summary",
        summary: "HIDDEN-SEARCH-SUMMARY",
        preceding_tool_use_ids: ["hidden-search"],
      },
    ])
      handleSdkMessage(session, { ...frame, session_id: session.sessionId });
    const hiddenEvents = [...events];
    events.length = 0;
    const normalInput = { command: "printf ordinary-control" };
    const normalPending = installed.canUseTool("Bash", normalInput, {
      toolUseID: "ordinary-tool",
      requestId: "ordinary-permission",
      signal: new AbortController().signal,
    });
    const requested = events.find((event) => event.event === "permission_request");
    assert.ok(requested);
    const scheduler = new BridgeCommandScheduler();
    await scheduleBridgeCommand(
      scheduler,
      parseCommandEnvelope(
        JSON.stringify({
          command: "permission_response",
          session_id: session.sessionId,
          request_id: "ordinary-answer",
          tool_call_id: "ordinary-tool",
          outcome: { outcome: "selected", option_id: "allow_once" },
        }),
      ),
    );
    const normalResult = await normalPending;
    handleSdkMessage(session, {
      type: "user",
      uuid: "ordinary-result-source",
      session_id: session.sessionId,
      tool_use_result: { stdout: "ORDINARY-ACTUAL-OUTPUT\n", stderr: "", interrupted: false },
      message: {
        role: "user",
        content: [
          {
            type: "tool_result",
            tool_use_id: "ordinary-tool",
            content: "ORDINARY-ACTUAL-OUTPUT\n",
            is_error: false,
          },
        ],
      },
    });
    await lifecycle.closeAllSessions();
    await protocol.emitSessionsList();
    restore();
    restore = undefined;
    process.stdout.write(
      JSON.stringify({
        sessionId: session.sessionId,
        hiddenResult,
        hiddenEvents,
        normalResult,
        normalEvents: events,
        allQueriesClosed: closed,
      }),
    );
  }
} finally {
  stream.close();
  try {
    await lifecycle?.closeAllSessions();
  } finally {
    restore?.();
    fs.rmSync(root, { recursive: true, force: true });
  }
}
