import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { parseCommandEnvelope } from "./commands.js";
import { handleSessionDataCommand } from "./command_session_data.js";
import { replaceProtocolEventWriter } from "./events.js";
import { sessions } from "./session_lifecycle.js";

const SESSION_ID = "11111111-1111-4111-8111-111111111111";

type HistoryUpdate = {
  type: string;
  content?: { text?: string };
  tool_call?: { tool_call_id?: string; raw_input?: unknown };
  tool_call_update?: { fields?: { raw_output?: unknown } };
};

type Envelope = Record<string, unknown> & { updates?: HistoryUpdate[] };

/**
 * Write a saved session with three exchanges and one tool call, then run
 * exportSessionHistory against it in a child process with the real SDK reader,
 * returning the protocol events it wrote.
 */
function exportFromSavedSession(limits: { chunkBytes: number; maxBytes: number }): Envelope[] {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), "claude-rs-export-")));
  try {
    const cwd = join(directory, "project");
    const configDir = join(directory, "config");
    const projectKey = "history-export-fixture";
    const projectDir = join(configDir, "projects", projectKey);
    mkdirSync(cwd, { recursive: true });
    mkdirSync(projectDir, { recursive: true });
    const records: Record<string, unknown>[] = [];
    const add = (uuid: string, parentUuid: string | null, type: string, message: unknown) => {
      records.push({ uuid, parentUuid, type, message, sessionId: SESSION_ID, cwd, timestamp: `2026-10-06T10:00:${String(records.length).padStart(2, "0")}.000Z` });
    };
    add("p1", null, "user", { role: "user", content: "First prompt" });
    add("a1", "p1", "assistant", { id: "m1", role: "assistant", content: [{ type: "text", text: "First answer" }] });
    add("p2", "a1", "user", { role: "user", content: "Run it" });
    add("t1", "p2", "assistant", { id: "m2", role: "assistant", content: [{ type: "tool_use", id: "bash-1", name: "Bash", input: { command: "echo hi" } }] });
    add("r1", "t1", "user", { role: "user", content: [{ type: "tool_result", tool_use_id: "bash-1", content: "hi" }] });
    add("a2", "r1", "assistant", { id: "m3", role: "assistant", content: [{ type: "text", text: "It printed hi" }] });
    add("p3", "a2", "user", { role: "user", content: "Third prompt" });
    add("a3", "p3", "assistant", { id: "m4", role: "assistant", content: [{ type: "text", text: "Third answer" }] });
    writeFileSync(join(projectDir, `${SESSION_ID}.jsonl`), `${records.map(record => JSON.stringify(record)).join("\n")}\n`);

    const script = `
      import { exportSessionHistory } from ${JSON.stringify(new URL("./history_export.js", import.meta.url).href)};
      await exportSessionHistory(${JSON.stringify(SESSION_ID)}, ${JSON.stringify(cwd)}, "req-export", ${JSON.stringify(limits)});
    `;
    const childEnv: NodeJS.ProcessEnv = {
      ...process.env,
      CLAUDE_CONFIG_DIR: configDir,
      // Measured 2026-10-13 on windows-latest: manually mirroring the SDK's derived
      // project key left this fixture unreadable. Its supported fixed key avoids that
      // duplicated path normalization across Windows and POSIX paths.
      CLAUDE_CODE_PROJECT_DIR_NAME: projectKey,
    };
    const child = spawnSync(process.execPath, ["--input-type=module", "-"], {
      input: script,
      encoding: "utf8",
      env: childEnv,
      windowsHide: true,
    });
    assert.equal(child.status, 0, child.stderr);
    return child.stdout
      .split("\n")
      .filter(line => line.startsWith("{"))
      .map(line => JSON.parse(line) as Envelope);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

const texts = (events: Envelope[]) =>
  events
    .flatMap(event => event.updates ?? [])
    .filter(update => update.type === "user_message_chunk" || update.type === "agent_message_chunk")
    .map(update => update.content?.text);

test("export streams the whole saved history in chunks and marks only the last", () => {
  // A chunk budget of one byte puts every update in its own event.
  const events = exportFromSavedSession({ chunkBytes: 1, maxBytes: 64 * 1024 * 1024 });

  assert.ok(events.length > 3, `history spans several chunks: ${JSON.stringify(events)}`);
  assert.ok(events.every(event => event.event === "session_history_export" && event.request_id === "req-export" && event.session_id === SESSION_ID));
  assert.deepEqual(events.map(event => event.last), events.map((_, index) => index === events.length - 1));
  assert.ok(events.every(event => event.omitted_updates === 0 && event.error === undefined));
  assert.deepEqual(texts(events), ["First prompt", "First answer", "Run it", "It printed hi", "Third prompt", "Third answer"]);
  const updates = events.flatMap(event => event.updates ?? []);
  const call = updates.find(update => update.type === "tool_call");
  assert.equal(call?.tool_call?.tool_call_id, "bash-1");
  assert.deepEqual(call?.tool_call?.raw_input, { command: "echo hi" });
  const result = updates.find(update => update.type === "tool_call_update");
  assert.equal(result?.tool_call_update?.fields?.raw_output, "hi");
});

test("export keeps the beginning and counts what a very large session leaves out", () => {
  const full = exportFromSavedSession({ chunkBytes: 1024 * 1024, maxBytes: 64 * 1024 * 1024 });
  assert.equal(full.length, 1, "a small history is one chunk");
  assert.ok(full[0].updates?.length, `history export did not produce updates: ${JSON.stringify(full)}`);
  const total = full[0].updates?.length ?? 0;

  // Room for the first two updates only.
  const sizeOf = (index: number) => Buffer.byteLength(JSON.stringify(full[0].updates?.[index]));
  const capped = exportFromSavedSession({ chunkBytes: 1024 * 1024, maxBytes: sizeOf(0) + sizeOf(1) });

  const last = capped[capped.length - 1];
  assert.equal(last.last, true);
  assert.deepEqual(capped.flatMap(event => event.updates ?? []), full[0].updates?.slice(0, 2));
  assert.equal(last.omitted_updates, total - 2);
});

test("export_session_history is parsed and an unknown session fails the export request", async () => {
  const parsed = parseCommandEnvelope(
    JSON.stringify({ request_id: "req-1", command: "export_session_history", session_id: "s1" }),
  );
  assert.equal(parsed.requestId, "req-1");
  assert.deepEqual(parsed.command, { command: "export_session_history", session_id: "s1" });
  assert.throws(() => parseCommandEnvelope(JSON.stringify({ command: "export_session_history" })), /session_id/);

  sessions.clear();
  const writes: string[] = [];
  const restore = replaceProtocolEventWriter(line => { writes.push(line); });
  try {
    await handleSessionDataCommand(
      { command: "export_session_history", session_id: "missing" },
      "req-missing",
      {
        generatePersistedSessionTitle: async () => "unused",
        buildSessionMutationOptions: () => undefined,
        rewindTargetsFromSessionMessages: () => [],
        handleRewind: async () => undefined,
      },
    );
  } finally {
    restore();
  }
  // The failure travels on the request-scoped export event, never as a generic slash error.
  assert.deepEqual(writes.map(line => JSON.parse(line)), [
    {
      request_id: "req-missing",
      event: "session_history_export",
      session_id: "missing",
      updates: [],
      last: true,
      omitted_updates: 0,
      error: "unknown session: missing",
    },
  ]);
});
