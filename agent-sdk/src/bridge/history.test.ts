import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";
import type { SessionUpdate } from "../types.js";
import { mapSessionMessagesToUpdates } from "./history.js";

test("resume display restores compaction segments once and preserves SDK branch filtering", () => {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), "claude-rs-transcript-")));
  try {
    const cwd = join(directory, "project");
    const configDir = join(directory, "config");
    const projectDir = join(configDir, "projects", cwd.replace(/[^a-zA-Z0-9]/g, "-"));
    mkdirSync(cwd, { recursive: true });
    mkdirSync(projectDir, { recursive: true });
    const sessionId = "11111111-1111-4111-8111-111111111111";
    const records: Record<string, unknown>[] = [];
    const add = (uuid: string, parentUuid: string | null, type: string, message: unknown, extra = {}) => {
      records.push({ uuid, parentUuid, type, message, sessionId, cwd, timestamp: `2026-10-06T10:00:${String(records.length).padStart(2, "0")}.000Z`, ...extra });
    };
    add("prompt-1", null, "user", { role: "user", content: "First prompt" });
    add("tool", "prompt-1", "assistant", { id: "assistant-1", role: "assistant", content: [{ type: "tool_use", id: "read-1", name: "Read", input: { file_path: "fixture.rs" } }] });
    add("result", "tool", "user", { role: "user", content: [{ type: "tool_result", tool_use_id: "read-1", content: "file contents" }] });
    add("answer-1", "result", "assistant", { id: "assistant-2", role: "assistant", content: [{ type: "text", text: "First answer" }] });
    add("boundary-1", null, "system", undefined, { subtype: "compact_boundary", logicalParentUuid: "answer-1", compactMetadata: { trigger: "auto", preservedMessages: { anchorUuid: "summary-1", uuids: ["tool", "result", "answer-1"] } } });
    add("summary-1", "boundary-1", "user", { role: "user", content: "Internal summary one" }, { isCompactSummary: true });
    add("prompt-2", "summary-1", "user", { role: "user", content: [{ type: "text", text: "Second prompt" }] });
    add("discarded", "prompt-2", "assistant", { id: "discarded-branch", role: "assistant", content: [{ type: "text", text: "Discarded answer" }] });
    add("answer-2", "prompt-2", "assistant", { id: "assistant-3", role: "assistant", content: [{ type: "text", text: "Second answer" }] });
    add("boundary-2", null, "system", undefined, { subtype: "compact_boundary", logicalParentUuid: "answer-2", compactMetadata: { trigger: "auto", preservedSegment: { headUuid: "prompt-2", anchorUuid: "summary-2", tailUuid: "answer-2" } } });
    add("summary-2", "boundary-2", "user", { role: "user", content: [{ type: "text", text: "Internal summary two" }] }, { isCompactSummary: true });
    add("prompt-3", "summary-2", "user", { role: "user", content: "Third prompt" });
    add("answer-3", "prompt-3", "assistant", { id: "assistant-4", role: "assistant", content: [{ type: "text", text: "Third answer" }] });
    add("sidechain", "answer-3", "assistant", { id: "sidechain", role: "assistant", content: [{ type: "text", text: "Subagent output" }] }, { isSidechain: true });
    // Exercise the SDK's five-megabyte pre-compaction read optimization too.
    records.splice(4, 0, { type: "file-history-snapshot", padding: "x".repeat(6 * 1024 * 1024) });
    const transcript = join(projectDir, `${sessionId}.jsonl`);
    const before = `${records.map(record => JSON.stringify(record)).join("\n")}\n`;
    writeFileSync(transcript, before);
    const script = `
      import { getSessionTranscriptMessages, mapSessionMessagesToUpdates } from ${JSON.stringify(new URL("./history.js", import.meta.url).href)};
      const messages = await getSessionTranscriptMessages(${JSON.stringify(sessionId)});
      const updates = mapSessionMessagesToUpdates(messages);
      console.log(JSON.stringify({
        uuids: messages.map(message => message.uuid), updates
      }));
    `;
    const child = spawnSync(process.execPath, ["--input-type=module", "-"], {
      input: script,
      encoding: "utf8",
      env: { ...process.env, CLAUDE_CONFIG_DIR: configDir, CLAUDE_CODE_PROJECT_DIR_NAME: undefined },
      windowsHide: true,
    });
    assert.equal(child.status, 0, child.stderr);
    const result = JSON.parse(child.stdout);
    assert.deepEqual(result.uuids, ["prompt-1", "tool", "result", "answer-1", "boundary-1", "summary-1", "prompt-2", "answer-2", "boundary-2", "summary-2", "prompt-3", "answer-3"]);
    const chunks = result.updates.filter((update: { type: string }) => update.type === "user_message_chunk" || update.type === "agent_message_chunk");
    assert.deepEqual(chunks.map((update: { content: { text: string } }) => update.content.text), ["First prompt", "First answer", "Second prompt", "Second answer", "Third prompt", "Third answer"]);
    const tools = result.updates.filter((update: { type: string }) => update.type === "tool_call");
    assert.equal(tools.length, 1);
    assert.equal(tools[0].tool_call.tool_call_id, "read-1");
    assert.equal(tools[0].tool_call.status, "in_progress");
    const resultUpdate = result.updates.find((update: { type: string; tool_call_update?: { tool_call_id: string } }) => update.type === "tool_call_update" && update.tool_call_update?.tool_call_id === "read-1");
    assert.equal(resultUpdate.tool_call_update.fields.status, "completed");
    assert.equal(resultUpdate.tool_call_update.fields.raw_output, "file contents");
    assert.equal(readFileSync(transcript, "utf8"), before);
  } finally {
    assert.equal(dirname(directory), realpathSync(tmpdir()));
    rmSync(directory, { recursive: true, force: true });
  }
});

test("resume display excludes internal task notifications while preserving user XML and external messages", () => {
  const base = { session_id: "fixture", parent_tool_use_id: null, parent_agent_id: null, timestamp: "2026-10-06T11:10:22.531Z" };
  const completed = "<task-notification>\n<task-id>task-1</task-id>\n<status>completed</status>\n<summary>Background command completed</summary>\n</task-notification>";
  const stopped = "<task-notification>\n<task-id>task-2</task-id>\n<status>stopped</status>\n<summary>Background shell command stopped</summary>\n</task-notification>";
  const messages = [
    { ...base, type: "user" as const, uuid: "before", message: { role: "user", content: "Before notification" } },
    { ...base, type: "user" as const, uuid: "completed", origin: { kind: "task-notification", producer: "session-task" }, message: { role: "user", content: completed } },
    { ...base, type: "user" as const, uuid: "stopped", origin: { kind: "task-notification" }, message: { role: "user", content: [{ type: "text", text: stopped }] } },
    { ...base, type: "user" as const, uuid: "after", origin: { kind: "human" }, message: { role: "user", content: "hello?" } },
    { ...base, type: "user" as const, uuid: "user-xml", origin: { kind: "human" }, message: { role: "user", content: completed } },
    { ...base, type: "assistant" as const, uuid: "assistant-xml", message: { role: "assistant", content: "<example>XML output</example>" } },
    { ...base, type: "user" as const, uuid: "peer", origin: { kind: "task-notification", subkind: "peer-send-message" }, message: { role: "user", content: "Peer message" } },
    { ...base, type: "user" as const, uuid: "scheduled", origin: { kind: "task-notification", subkind: "scheduled-trigger" }, message: { role: "user", content: "Scheduled message" } },
  ];
  const before = structuredClone(messages);
  const updates = mapSessionMessagesToUpdates(messages);
  const chunks = updates.filter(update => update.type === "user_message_chunk" || update.type === "agent_message_chunk");
  assert.deepEqual(chunks.map(update => [update.source_message_uuid, update.content]), [
    ["before", { type: "text", text: "Before notification" }],
    ["after", { type: "text", text: "hello?" }],
    ["user-xml", { type: "text", text: completed }],
    ["assistant-xml", { type: "text", text: "<example>XML output</example>" }],
    ["peer", { type: "text", text: "Peer message" }],
    ["scheduled", { type: "text", text: "Scheduled message" }],
  ]);
  assert.deepEqual(updates.filter(update => update.type === "message_metadata").map(update => update.source_message_uuid), ["before", "after", "user-xml", "assistant-xml", "peer", "scheduled"]);
  assert.deepEqual(messages, before);
});

test("resume notifications preserve terminal outcomes, linked tools and optional fields", () => {
  const base = { session_id: "fixture", parent_tool_use_id: null, parent_agent_id: null };
  for (const [status, expectedStatus] of [["completed", "completed"], ["failed", "failed"], ["stopped", "killed"]] as const) {
    const outputFile = "C:\\Temp\\claude\\tasks\\monitor.output";
    const summary = "Background command finished\nSecond line with <example>literal XML</example>";
    const notification = `<task-notification>\n<task-id> task-${status} </task-id>\n<tool-use-id> monitor </tool-use-id>\n<output-file> ${outputFile} </output-file>\n<status> ${status} </status>\n<summary> ${summary} </summary>\n<note>Extra SDK field</note>\n</task-notification>`;
    for (const content of [notification, [{ type: "text", text: notification.slice(0, notification.indexOf("<status>")) }, { type: "text", text: notification.slice(notification.indexOf("<status>")) }]]) {
      const updates = mapSessionMessagesToUpdates([
        { ...base, type: "assistant", uuid: "launch", message: { role: "assistant", content: [{ type: "tool_use", id: "monitor", name: "Monitor", input: { command: "check" } }] } },
        { ...base, type: "user", uuid: "result", ...{ origin: { kind: "task-notification", producer: "session-task" } }, message: { role: "user", content } },
      ]);
      const results = updates.filter(update => update.type === "tool_call_update");
      assert.equal(results.length, 1);
      assert.equal(results[0].tool_call_update.tool_call_id, "monitor");
      assert.equal(results[0].tool_call_update.fields.status, expectedStatus);
      assert.equal(results[0].tool_call_update.fields.raw_output, summary);
      assert.equal(results[0].tool_call_update.fields.task_metadata?.output_file, outputFile);
      assert.equal(results[0].tool_call_update.fields.task_metadata?.terminal_status, status);
      const tasks = updates.filter(update => update.type === "task_state_update").flatMap(update => update.tasks);
      assert.deepEqual(tasks.map(task => [task.task_id, task.status, task.source_tool_call_id, task.description, task.metadata]), [[`task-${status}`, "completed", "monitor", summary, { output_file: outputFile, summary, terminal_status: status }]]);
      assert.ok(!updates.some(update => update.type === "user_message_chunk"));
    }
  }
  const minimal = mapSessionMessagesToUpdates([
    { ...base, type: "user", uuid: "minimal", ...{ origin: { kind: "task-notification" } }, message: { role: "user", content: "</task-notification><task-notification><task-id>minimal</task-id><status>completed</status></task-notification>" } },
  ]);
  assert.deepEqual(minimal.filter(update => update.type === "task_state_update").flatMap(update => update.tasks).map(task => [task.task_id, task.status, task.source_tool_call_id, task.description]), [["minimal", "completed", undefined, undefined]]);
});

test("resume skips malformed internal notifications and restores later conversation and task outcomes", () => {
  const base = { session_id: "fixture", parent_tool_use_id: null, parent_agent_id: null };
  const malformed = [
    "<task-id>bad</task-id><status>completed</status></task-notification>",
    "<task-notification><task-id>bad</task-id><status>completed</status>",
    "<task-notification></task-notification>",
    "<task-notification><task-id> </task-id><status>completed</status></task-notification>",
    "<task-notification><task-id>bad<status>completed</status></task-notification>",
    "<task-notification><task-id>bad</task-id></task-notification>",
    "<task-notification><task-id>bad</task-id><status>running</status></task-notification>",
    "<task-notification><task-id>bad</task-id><status>completed</task-notification>",
    "<task-notification><task-id>bad</task-id></status><status>completed</task-notification>",
  ];
  const updates = mapSessionMessagesToUpdates([
    ...malformed.map((content, index) => ({ ...base, type: "user" as const, uuid: `broken-${index}`, origin: { kind: "task-notification" }, message: { role: "user", content } })),
    { ...base, type: "user", uuid: "after", message: { role: "user", content: "Continue after the damaged record" } },
    { ...base, type: "assistant", uuid: "reply", message: { role: "assistant", content: "Conversation restored" } },
    { ...base, type: "user", uuid: "valid", ...{ origin: { kind: "task-notification" } }, message: { role: "user", content: "<task-notification><task-id>valid</task-id><status>completed</status></task-notification>" } },
  ]);
  assert.deepEqual(updates.filter(update => update.type === "user_message_chunk" || update.type === "agent_message_chunk").map(update => [update.source_message_uuid, update.content]), [
    ["after", { type: "text", text: "Continue after the damaged record" }],
    ["reply", { type: "text", text: "Conversation restored" }],
  ]);
  assert.deepEqual(updates.filter(update => update.type === "task_state_update").flatMap(update => update.tasks).map(task => [task.task_id, task.status]), [["valid", "completed"]]);
});

test("resume handles many unclosed notification and field tags without blocking the remaining transcript", () => {
  // A process timeout bounds the regression even if synchronous parsing stalls.
  const child = spawnSync(process.execPath, ["--input-type=module", "-"], {
    input: `
      import { mapSessionMessagesToUpdates } from ${JSON.stringify(new URL("./history.js", import.meta.url).href)};
      const broken = [
        "<task-notification>a".repeat(200_000),
        "<task-notification>" + "<task-id>a".repeat(200_000) + "<status>completed</status></task-notification>",
        "<task-notification><task-id>bad</task-id>" + "<status>a".repeat(200_000) + "</task-notification>",
        "<task-notification><task-id>valid</task-id><status>completed</status>" + "<tool-use-id>a<summary>b<output-file>c".repeat(200_000) + "</task-notification>",
      ];
      const base = { session_id: "fixture", parent_tool_use_id: null, parent_agent_id: null };
      const updates = mapSessionMessagesToUpdates([
        ...broken.map((content, index) => ({ ...base, type: "user", uuid: "broken-" + index, origin: { kind: "task-notification" }, message: { role: "user", content } })),
        { ...base, type: "user", uuid: "after", message: { role: "user", content: "Still restored" } },
      ]);
      process.stdout.write(JSON.stringify(updates));
    `,
    encoding: "utf8",
    timeout: 10_000,
    windowsHide: true,
  });
  assert.equal(child.error, undefined, String(child.error));
  assert.equal(child.status, 0, child.stderr);
  const updates = JSON.parse(child.stdout) as SessionUpdate[];
  assert.deepEqual(updates.filter(update => update.type === "user_message_chunk").map(update => [update.source_message_uuid, update.content]), [["after", { type: "text", text: "Still restored" }]]);
  assert.deepEqual(updates.filter(update => update.type === "task_state_update").flatMap(update => update.tasks).map(task => [task.task_id, task.status, task.source_tool_call_id, task.description]), [["valid", "completed", undefined, undefined]]);
});

test("resume display maps plain string messages and hides internal compact summaries", () => {
  const base = { session_id: "fixture", parent_tool_use_id: null, parent_agent_id: null };
  const updates = mapSessionMessagesToUpdates([
    { ...base, type: "user", uuid: "user", message: { role: "user", content: "User prompt" } },
    { ...base, type: "assistant", uuid: "assistant", message: { role: "assistant", content: "Assistant response" } },
    { ...base, type: "user", uuid: "summary", message: { role: "user", content: [{ type: "text", text: "Internal summary" }] }, ...{ isCompactSummary: true } },
  ]);
  assert.deepEqual(updates.filter(update => update.type === "user_message_chunk" || update.type === "agent_message_chunk").map(update => update.content), [
    { type: "text", text: "User prompt" },
    { type: "text", text: "Assistant response" },
  ]);
  assert.ok(!updates.some(update => update.type === "message_metadata" && update.source_message_uuid === "summary"));
});
