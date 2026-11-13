import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import type { SessionUpdate } from "../types.js";
import { parseCommandEnvelope } from "./commands.js";

const SESSION_ID = "11111111-1111-4111-8111-111111111111";
const SIBLING_ID = "22222222-2222-4222-8222-222222222222";
const MISSING_ID = "33333333-3333-4333-8333-333333333333";
const STAMP = "2026-10-06T10:00:00.000Z";
const UNICODE = "界🙂é".repeat(40);
const LONG_COMMAND = `echo ${"a".repeat(300)}`;
type RecordInput = {
  uuid: string;
  type: "user" | "assistant";
  message: unknown;
  timestamp: string;
  isCompactSummary?: boolean;
};
type Envelope = {
  event: string;
  session_id: string;
  request_id: string;
  updates: SessionUpdate[];
  last: boolean;
  omitted_updates: number;
  error?: string;
};
type Operation = {
  request: string;
  session: string;
  dispatch?: boolean;
  rejectRead?: boolean;
  limits?: { chunkBytes: number; maxBytes: number };
};
const record = (
  uuid: string,
  type: RecordInput["type"],
  content: unknown,
  timestamp = STAMP,
): RecordInput => ({
  uuid,
  type,
  timestamp,
  message: { role: type, ...(type === "assistant" ? { id: `message-${uuid}` } : {}), content },
});
const textHistory = [
  record("budget-prompt", "user", "Start"),
  record("budget-reply", "assistant", [{ type: "text", text: UNICODE }]),
  record("budget-tail", "user", "End"),
];
// Authored wire expectations: never obtain budgets, prefixes or counts from another export.
const textUpdates: SessionUpdate[] = [
  {
    type: "user_message_chunk",
    content: { type: "text", text: "Start" },
    source_message_uuid: "budget-prompt",
  },
  {
    type: "message_metadata",
    role: "user",
    timestamp: STAMP,
    source_message_uuid: "budget-prompt",
  },
  {
    type: "agent_message_chunk",
    content: { type: "text", text: UNICODE },
    source_message_uuid: "budget-reply",
  },
  {
    type: "message_metadata",
    role: "assistant",
    timestamp: STAMP,
    source_message_uuid: "budget-reply",
  },
  {
    type: "user_message_chunk",
    content: { type: "text", text: "End" },
    source_message_uuid: "budget-tail",
  },
  { type: "message_metadata", role: "user", timestamp: STAMP, source_message_uuid: "budget-tail" },
];
const bytes = (updates: SessionUpdate[]) =>
  updates.reduce((total, update) => total + Buffer.byteLength(JSON.stringify(update)), 0);

function exportFromSavedSession(records: RecordInput[], operations: Operation[]) {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), "claude-rs-export-")));
  try {
    const cwd = join(directory, "project"),
      siblingCwd = join(directory, "sibling");
    const config = join(directory, "config"),
      projectKey = "history-export-fixture";
    const projectDir = join(config, "projects", projectKey);
    for (const folder of [cwd, siblingCwd, projectDir]) mkdirSync(folder, { recursive: true });
    const originals = new Map<string, Buffer>();
    for (const [sessionId, project, input] of [
      [SESSION_ID, cwd, records],
      [SIBLING_ID, siblingCwd, [record("sibling-prompt", "user", "Independent sibling history")]],
    ] as const) {
      const file = join(projectDir, `${sessionId}.jsonl`);
      const body = input
        .map((entry, index) =>
          JSON.stringify({
            ...entry,
            parentUuid: index === 0 ? null : input[index - 1].uuid,
            sessionId,
            cwd: project,
          }),
        )
        .join("\n");
      const original = Buffer.from(body ? `${body}\n` : "");
      writeFileSync(file, original);
      originals.set(file, original);
    }
    const sdkUrl = import.meta.resolve("@anthropic-ai/claude-agent-sdk");
    const wrapper = `
      export * from ${JSON.stringify(sdkUrl)};
      import { importSessionToStore as read } from ${JSON.stringify(sdkUrl)};
      export async function importSessionToStore(session, store, options) {
        globalThis.loads.push({ session, options });
        if (globalThis.rejectRead) {
          globalThis.rejectRead = false;
          throw new Error("fixture archive temporarily unavailable");
        }
        return read(session, store, options);
      }
    `;
    const script = `
      import assert from "node:assert/strict";
      import { registerHooks } from "node:module";
      globalThis.loads = [];
      globalThis.rejectRead = false;
      registerHooks({
        resolve(specifier, context, next) {
          return specifier === "@anthropic-ai/claude-agent-sdk"
            ? {url:"fixture:export-sdk",shortCircuit:true} : next(specifier, context);
        },
        load(url, context, next) {
          return url === "fixture:export-sdk"
            ? {format:"module",source:${JSON.stringify(wrapper)},shortCircuit:true} : next(url, context);
        }
      });
      const { exportSessionHistory } = await import(${JSON.stringify(new URL("./history_export.js", import.meta.url).href)});
      const { handleSessionDataCommand } = await import(${JSON.stringify(new URL("./command_session_data.js", import.meta.url).href)});
      const { parseCommandEnvelope } = await import(${JSON.stringify(new URL("./commands.js", import.meta.url).href)});
      const { sessions } = await import(${JSON.stringify(new URL("./session_lifecycle.js", import.meta.url).href)});
      const selected = {sessionId:${JSON.stringify(SESSION_ID)},cwd:${JSON.stringify(cwd)}};
      const sibling = {sessionId:${JSON.stringify(SIBLING_ID)},cwd:${JSON.stringify(siblingCwd)}};
      const sentinel = {sessionId:"export-fixture-sentinel",cwd:"unused"};
      const registrations = [selected, sibling, sentinel];
      for(const session of registrations) { assert.equal(sessions.has(session.sessionId),false); sessions.set(session.sessionId,session); }
      const unused = async () => { throw new Error("unrelated command dependency invoked"); };
      try {
        for (const operation of ${JSON.stringify(operations)}) {
          globalThis.rejectRead = operation.rejectRead === true;
          if(operation.dispatch) {
            const parsed = parseCommandEnvelope(JSON.stringify({command:"export_session_history",session_id:operation.session,request_id:operation.request}));
            assert.equal(parsed.command.command,"export_session_history");
            await handleSessionDataCommand(parsed.command, parsed.requestId, {
              generatePersistedSessionTitle:unused, buildSessionMutationOptions:unused,
              rewindTargetsFromSessionMessages:unused,handleRewind:unused,
            });
          } else {
            await exportSessionHistory(operation.session, selected.cwd, operation.request, operation.limits);
          }
          for(const session of registrations) assert.strictEqual(sessions.get(session.sessionId),session,"export preserves session registrations");
        }
      } finally {
        for(const session of registrations) sessions.delete(session.sessionId);
      }
      process.stderr.write(JSON.stringify({loads:globalThis.loads})+"\\n");
    `;
    const env = { ...process.env };
    for (const key of Object.keys(env))
      if (/^(ANTHROPIC_|CLAUDE_|CLAUDECODE$)/.test(key)) delete env[key];
    Object.assign(env, { CLAUDE_CONFIG_DIR: config, CLAUDE_CODE_PROJECT_DIR_NAME: projectKey });
    const child = spawnSync(process.execPath, ["--input-type=module", "-"], {
      cwd,
      input: script,
      env,
      encoding: "utf8",
      windowsHide: true,
      timeout: 15_000,
      maxBuffer: 2 * 1024 * 1024,
    });
    for (const [file, original] of originals)
      assert.deepEqual(readFileSync(file), original, "export preserves saved source bytes");
    assert.equal(child.error, undefined, String(child.error));
    assert.equal(child.signal, null, child.stderr);
    assert.equal(child.status, 0, child.stderr);
    assert.ok(child.stdout.endsWith("\n"), "complete protocol lines");
    const lines = child.stdout.trimEnd().split("\n");
    const events = lines.map((line) => JSON.parse(line) as Envelope);
    const diagnostic = JSON.parse(child.stderr.trim().split("\n").at(-1) as string) as {
      loads: Array<{ session: string; options: { dir: string; includeSubagents: boolean } }>;
    };
    return { events, loads: diagnostic.loads, cwd, siblingCwd };
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

function stream(events: Envelope[], request: string, session = SESSION_ID) {
  const selected = events.filter((event) => event.request_id === request);
  assert.ok(selected.length > 0, `${request}: terminal protocol response`);
  for (const [index, event] of selected.entries()) {
    assert.equal(event.event, "session_history_export");
    assert.equal(event.session_id, session);
    assert.equal(
      event.last,
      index === selected.length - 1,
      `${request}: only the terminal chunk is final`,
    );
    if (!event.last) assert.equal(event.omitted_updates, 0);
    assert.ok(Array.isArray(event.updates));
  }
  return selected;
}
function successful(
  events: Envelope[],
  request: string,
  expected: SessionUpdate[],
  omitted = 0,
  session = SESSION_ID,
) {
  const chunks = stream(events, request, session);
  assert.ok(
    chunks.every((chunk) => chunk.error === undefined),
    JSON.stringify(chunks),
  );
  assert.deepEqual(
    chunks.flatMap((chunk) => chunk.updates),
    expected,
    request,
  );
  assert.equal(chunks[chunks.length - 1].omitted_updates, omitted, request);
  return chunks;
}
function failed(events: Envelope[], request: string, session: string, cause: RegExp) {
  const chunks = stream(events, request, session);
  assert.equal(chunks.length, 1, `${request}: one terminal failure`);
  assert.deepEqual(chunks[0].updates, []);
  assert.equal(chunks[0].omitted_updates, 0);
  assert.match(chunks[0].error ?? "", cause);
}

test("export delivers the complete saved sequence with byte-bounded batching and one final chunk", () => {
  const firstPair: SessionUpdate[] = [
    {
      type: "user_message_chunk",
      content: { type: "text", text: "First prompt" },
      source_message_uuid: "p1",
    },
    { type: "message_metadata", role: "user", timestamp: STAMP, source_message_uuid: "p1" },
  ];
  const records = [
    record("p1", "user", "First prompt"),
    record("a1", "assistant", [{ type: "text", text: "First answer" }]),
    record("p2", "user", "Run both"),
    record("tools", "assistant", [
      { type: "tool_use", id: "bash-a", name: "Bash", input: { command: LONG_COMMAND } },
      { type: "tool_use", id: "bash-b", name: "Bash", input: { command: "echo beta" } },
    ]),
    record("results", "user", [
      { type: "tool_result", tool_use_id: "bash-b", content: "β failed", is_error: true },
      { type: "tool_result", tool_use_id: "bash-a", content: "α output" },
    ]),
    record("a2", "assistant", [{ type: "text", text: "Both outcomes retained" }]),
    record("p3", "user", "Third prompt"),
    record("a3", "assistant", [{ type: "text", text: "Third answer" }]),
  ];
  const chunkBytes = bytes(firstPair);
  const { events } = exportFromSavedSession(records, [
    { session: SESSION_ID, request: "req-full", limits: { chunkBytes, maxBytes: 1024 * 1024 } },
  ]);
  const chunks = stream(events, "req-full");
  assert.equal(chunks.length, events.length, "no unrelated output");
  assert.ok(chunks.every((chunk) => chunk.error === undefined && chunk.omitted_updates === 0));
  assert.deepEqual(chunks[0].updates, firstPair, "exact-fit first pair shares a chunk");
  const updates = chunks.flatMap((chunk) => chunk.updates);
  const identities = updates.map((update) => {
    switch (update.type) {
      case "user_message_chunk":
      case "agent_message_chunk":
        return [update.type, update.source_message_uuid, update.content];
      case "message_metadata":
        return [update.type, update.source_message_uuid, update.role, update.timestamp];
      case "tool_call":
        return [
          update.type,
          update.tool_call.source_message_uuid,
          update.tool_call.tool_call_id,
          update.tool_call.raw_input,
          update.tool_call.status,
        ];
      case "tool_call_update":
        return [
          update.type,
          update.tool_call_update.source_message_uuid,
          update.tool_call_update.tool_call_id,
          update.tool_call_update.fields.raw_output,
          update.tool_call_update.fields.status,
        ];
      default:
        return [update.type];
    }
  });
  assert.deepEqual(identities, [
    ["user_message_chunk", "p1", { type: "text", text: "First prompt" }],
    ["message_metadata", "p1", "user", STAMP],
    ["agent_message_chunk", "a1", { type: "text", text: "First answer" }],
    ["message_metadata", "a1", "assistant", STAMP],
    ["user_message_chunk", "p2", { type: "text", text: "Run both" }],
    ["message_metadata", "p2", "user", STAMP],
    ["tool_call", "tools", "bash-a", { command: LONG_COMMAND }, "in_progress"],
    ["tool_call", "tools", "bash-b", { command: "echo beta" }, "in_progress"],
    ["message_metadata", "tools", "assistant", STAMP],
    ["tool_call_update", "results", "bash-b", "β failed", "failed"],
    ["tool_call_update", "results", "bash-a", "α output", "completed"],
    ["message_metadata", "results", "user", STAMP],
    ["agent_message_chunk", "a2", { type: "text", text: "Both outcomes retained" }],
    ["message_metadata", "a2", "assistant", STAMP],
    ["user_message_chunk", "p3", { type: "text", text: "Third prompt" }],
    ["message_metadata", "p3", "user", STAMP],
    ["agent_message_chunk", "a3", { type: "text", text: "Third answer" }],
    ["message_metadata", "a3", "assistant", STAMP],
  ]);
  assert.ok(
    chunks.some((chunk) => chunk.updates.length === 1 && bytes(chunk.updates) > chunkBytes),
    "oversized single update stays intact",
  );
  for (const [index, chunk] of chunks.entries()) {
    assert.ok(
      bytes(chunk.updates) <= chunkBytes || chunk.updates.length === 1,
      "flush threshold bounds grouped updates",
    );
    const next = chunks[index + 1];
    if (next?.updates.length)
      assert.ok(
        bytes([...chunk.updates, next.updates[0]]) > chunkBytes,
        "do not flush before the next update exceeds the threshold",
      );
  }
  // A valid summary-only session has no display updates; a zero-byte JSONL is not a saved session.
  const empty = exportFromSavedSession(
    [{ ...record("summary-only", "user", "Saved compaction summary"), isCompactSummary: true }],
    [{ session: SESSION_ID, request: "req-empty", limits: { chunkBytes, maxBytes: 1024 } }],
  );
  const emptyChunks = successful(empty.events, "req-empty", []);
  assert.equal(emptyChunks.length, 1, "empty saved history still terminates");
});

test("export caps the chronological prefix at independently authored UTF-8 boundaries", () => {
  const firstTwo = bytes(textUpdates.slice(0, 2));
  const third = bytes([textUpdates[2]]);
  assert.ok(
    third > JSON.stringify(textUpdates[2]).length,
    "fixture distinguishes UTF-8 bytes from JS length",
  );
  for (const [name, maxBytes, included] of [
    ["exact", firstTwo + third, 3],
    ["one-byte-short", firstTwo + third - 1, 2],
    ["js-length-is-too-small", firstTwo + JSON.stringify(textUpdates[2]).length, 2],
    ["do-not-skip-large-update", firstTwo + bytes([textUpdates[3]]), 2],
    ["nothing-fits", bytes([textUpdates[0]]) - 1, 0],
  ] as const) {
    const { events } = exportFromSavedSession(textHistory, [
      { session: SESSION_ID, request: name, limits: { chunkBytes: firstTwo, maxBytes } },
    ]);
    const chunks = successful(
      events,
      name,
      textUpdates.slice(0, included),
      textUpdates.length - included,
    );
    assert.equal(chunks.length, events.length, "no unrelated output");
    if (included === 0)
      assert.equal(chunks.length, 1, "no fitting update still produces a terminal empty chunk");
  }
});

test("a parsed missing-session export fails only its request and leaves registered sessions usable", () => {
  assert.throws(
    () => parseCommandEnvelope(JSON.stringify({ command: "export_session_history" })),
    /session_id/,
  );
  const { events, loads, cwd } = exportFromSavedSession(textHistory, [
    { session: MISSING_ID, request: "req-missing", dispatch: true },
    { session: SESSION_ID, request: "req-known", dispatch: true },
  ]);
  failed(events, "req-missing", MISSING_ID, /unknown.*session/i);
  assert.ok(
    events.find((event) => event.request_id === "req-missing")?.error?.includes(MISSING_ID),
  );
  successful(events, "req-known", textUpdates);
  assert.deepEqual(
    loads,
    [{ session: SESSION_ID, options: { dir: cwd, includeSubagents: false } }],
    "only registered sessions reach the SDK loader",
  );
  assert.ok(
    events.every(
      (event) =>
        event.event === "session_history_export" &&
        ["req-missing", "req-known"].includes(event.request_id),
    ),
    "no generic slash error or sibling output",
  );
});

test("a registered-session read failure terminates export and permits independent sibling and retry requests", () => {
  const { events, loads, cwd, siblingCwd } = exportFromSavedSession(textHistory, [
    { session: SESSION_ID, request: "req-read-failed", dispatch: true, rejectRead: true },
    { session: SIBLING_ID, request: "req-sibling", dispatch: true },
    { session: SESSION_ID, request: "req-retry", dispatch: true },
  ]);
  failed(events, "req-read-failed", SESSION_ID, /fixture archive temporarily unavailable/);
  successful(
    events,
    "req-sibling",
    [
      {
        type: "user_message_chunk",
        content: { type: "text", text: "Independent sibling history" },
        source_message_uuid: "sibling-prompt",
      },
      {
        type: "message_metadata",
        role: "user",
        timestamp: STAMP,
        source_message_uuid: "sibling-prompt",
      },
    ],
    0,
    SIBLING_ID,
  );
  successful(events, "req-retry", textUpdates);
  assert.deepEqual(loads, [
    { session: SESSION_ID, options: { dir: cwd, includeSubagents: false } },
    { session: SIBLING_ID, options: { dir: siblingCwd, includeSubagents: false } },
    { session: SESSION_ID, options: { dir: cwd, includeSubagents: false } },
  ]);
  assert.ok(
    events.every(
      (event) =>
        event.event === "session_history_export" &&
        ["req-read-failed", "req-sibling", "req-retry"].includes(event.request_id),
    ),
    "no misleading successful chunks or generic errors",
  );
});
