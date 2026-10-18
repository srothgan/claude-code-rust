import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";
import { mapSessionMessagesToEntries, mapSessionMessagesToUpdates } from "./history.js";

test("replay admits notification origin.runId before replacing links or historical tool output", () => {
  const base = { session_id: "fixture", parent_tool_use_id: null, parent_agent_id: null };
  const launch = (id: string) => ({ ...base, type: "assistant" as const, uuid: id, message: { role: "assistant", content: [{ type: "tool_use", id, name: "Agent", input: { description: id, prompt: id, subagent_type: "fork" } }] } });
  const edge = (subtype: string, run_id: string, tool_use_id: string, extra = {}) => ({ ...base, type: "system" as const, uuid: crypto.randomUUID(), message: { subtype, task_id: "worker", run_id, tool_use_id, ...extra } });
  const notification = (runId: string, tool: string, summary: string) => ({ ...base, type: "user" as const, uuid: crypto.randomUUID(), origin: { kind: "task-notification", producer: "session-task", runId }, message: { role: "user", content: `<task-notification><task-id>worker</task-id><tool-use-id>${tool}</tool-use-id><status>completed</status><summary>${summary}</summary></task-notification>` } });
  const prefix = [launch("launch-A"), edge("task_started", "A", "launch-A", { parent_task_id: "parent", task_type: "local_agent", description: "A" }), notification("A", "launch-A", "A report"), launch("launch-B"), edge("task_started", "B", "launch-B", { description: "B" }), notification("B", "launch-B", "B report")];
  const accepted = mapSessionMessagesToUpdates(prefix);
  const replayed = mapSessionMessagesToUpdates([...prefix, notification("A", "launch-A", "late A"), edge("task_progress", "A", "launch-A", { description: "late progress" })]);
  assert.deepEqual(replayed, accepted, "old run notifications produce no row or tool changes");
  const row = replayed.filter(update => update.type === "task_state_update").flatMap(update => update.tasks).at(-1);
  assert.equal(row?.run_id, "B");
  assert.equal(row?.parent_task_id, "parent");
  assert.equal(row?.source_tool_call_id, "launch-B");
  assert.equal((row?.metadata as Record<string, unknown>).summary, "B report");
});

test("history attributes child tools through their producer while a transcript retains the full saved conversation", () => {
  const base = { session_id: "fixture", parent_tool_use_id: null, parent_agent_id: null };
  const history = [
    { ...base, type: "system" as const, uuid: "start", message: { subtype: "task_started", task_id: "worker", run_id: "A", tool_use_id: "launch", task_type: "local_agent", description: "Worker" } },
    { ...base, type: "assistant" as const, uuid: "child", agent_id: "worker", message: { role: "assistant", content: [{ type: "text", text: "Child report" }, { type: "tool_use", id: "nested", name: "Agent", input: { description: "Nested" } }] } },
  ];
  const chat = mapSessionMessagesToUpdates(history);
  assert.ok(!chat.some(update => update.type === "agent_message_chunk"));
  const tool = chat.filter(update => update.type === "tool_call").find(update => update.tool_call.tool_call_id === "nested");
  assert.equal((tool?.tool_call.meta as { claudeCode: { parentToolUseId: string } }).claudeCode.parentToolUseId, "launch");
  const transcript = mapSessionMessagesToEntries(history, true);
  assert.ok(transcript.some(update => update.type === "agent_message_chunk" && update.content.type === "text" && update.content.text === "Child report"));
});

test("resume restores compaction segments and Unicode separators with SDK branch filtering", () => {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), "claude-rs-transcript-")));
  try {
    const cwd = join(directory, "project");
    const configDir = join(directory, "config");
    const projectDirName = "history-export-fixture";
    const projectDir = join(configDir, "projects", projectDirName);
    mkdirSync(cwd, { recursive: true });
    mkdirSync(projectDir, { recursive: true });
    const sessionId = "11111111-1111-4111-8111-111111111111";
    const records: Record<string, unknown>[] = [];
    const source = "# Notes\r\n\r\n\t**literal**\u2028retained\u2029too  \r\n";
    const add = (uuid: string, parentUuid: string | null, type: string, message: unknown, extra = {}) => {
      records.push({ uuid, parentUuid, type, message, sessionId, cwd, timestamp: `2026-10-06T10:00:${String(records.length).padStart(2, "0")}.000Z`, ...extra });
    };
    add("prompt-1", null, "user", { role: "user", content: "First\u2028prompt\u2029retained" });
    add("tool", "prompt-1", "assistant", { id: "assistant-1", role: "assistant", content: [{ type: "tool_use", id: "read-1", name: "Read", input: { file_path: "notes.md", offset: 60 } }] });
    add("result", "tool", "user", { role: "user", content: [{ type: "tool_result", tool_use_id: "read-1", content: "60\t# Notes\n61\t\n62\t\t**literal**  " }] }, {
      toolUseResult: { type: "text", file: { filePath: "notes.md", content: source, numLines: 3, startLine: 60, totalLines: 100 } },
    });
    add("answer-1", "result", "assistant", { id: "assistant-2", role: "assistant", content: [{ type: "text", text: "First\u2028answer\u2029retained" }] });
    add("boundary-1", null, "system", undefined, { subtype: "compact_boundary", logicalParentUuid: "answer-1", compactMetadata: { trigger: "auto", preservedMessages: { anchorUuid: "summary-1", uuids: ["tool", "result", "answer-1"] } } });
    add("summary-1", "boundary-1", "user", { role: "user", content: "Internal summary one" }, { isCompactSummary: true });
    add("prompt-2", "summary-1", "user", { role: "user", content: [{ type: "text", text: "Second prompt" }] });
    add("command-caveat", "prompt-2", "user", { role: "user", content: "<local-command-caveat>Internal context only</local-command-caveat>" }, { isMeta: true });
    add("rename", "command-caveat", "user", { role: "user", content: "<command-name>/rename</command-name>\n<command-message>rename</command-message>\n<command-args>review-444</command-args>" });
    add("color", "rename", "user", { role: "user", content: "<command-name>/color</command-name>\n<command-message>color</command-message>\n<command-args>blue</command-args>" });
    add("discarded", "color", "assistant", { id: "discarded-branch", role: "assistant", content: [{ type: "text", text: "Discarded answer" }] });
    add("answer-2", "color", "assistant", { id: "assistant-3", role: "assistant", content: [{ type: "text", text: "Second answer" }] });
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
      import { rewindTargetsFromSessionMessages } from ${JSON.stringify(new URL("../bridge.js", import.meta.url).href)};
      const messages = await getSessionTranscriptMessages(${JSON.stringify(sessionId)});
      const updates = mapSessionMessagesToUpdates(messages);
      console.log(JSON.stringify({
        uuids: messages.map(message => message.uuid), updates,
        targets: rewindTargetsFromSessionMessages(messages).map(target => [target.uuid, target.input_text])
      }));
    `;
    const child = spawnSync(process.execPath, ["--input-type=module", "-"], {
      input: script,
      encoding: "utf8",
      env: { ...process.env, CLAUDE_CONFIG_DIR: configDir, CLAUDE_CODE_PROJECT_DIR_NAME: projectDirName },
      windowsHide: true,
    });
    assert.equal(child.status, 0, child.stderr);
    const result = JSON.parse(child.stdout);
    assert.deepEqual(result.uuids, ["prompt-1", "tool", "result", "answer-1", "boundary-1", "summary-1", "prompt-2", "rename", "color", "answer-2", "boundary-2", "summary-2", "prompt-3", "answer-3"]);
    const chunks = result.updates.filter((update: { type: string }) => update.type === "user_message_chunk" || update.type === "agent_message_chunk");
    assert.deepEqual(chunks.map((update: { content: { text: string } }) => update.content.text), ["First\u2028prompt\u2029retained", "First\u2028answer\u2029retained", "Second prompt", "/rename review-444", "/color blue", "Second answer", "Third prompt", "Third answer"]);
    assert.deepEqual(result.targets, [["prompt-3", "Third prompt"], ["prompt-2", "Second prompt"], ["prompt-1", "First\u2028prompt\u2029retained"]]);
    const tools = result.updates.filter((update: { type: string }) => update.type === "tool_call");
    assert.equal(tools.length, 1);
    assert.equal(tools[0].tool_call.tool_call_id, "read-1");
    assert.equal(tools[0].tool_call.status, "in_progress");
    const resultUpdate = result.updates.find((update: { type: string; tool_call_update?: { tool_call_id: string } }) => update.type === "tool_call_update" && update.tool_call_update?.tool_call_id === "read-1");
    assert.equal(resultUpdate.tool_call_update.fields.status, "completed");
    assert.equal(resultUpdate.tool_call_update.fields.raw_output, source);
    assert.deepEqual(resultUpdate.tool_call_update.fields.content, [{ type: "content", content: { type: "text", text: source } }]);
    assert.deepEqual(resultUpdate.tool_call_update.fields.output_metadata, { read: { file_path: "notes.md", start_line: 60 } });
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
      assert.deepEqual(tasks.map(task => [task.task_id, task.status, task.source_tool_call_id, task.description, task.metadata]), [[`task-${status}`, "completed", "monitor", summary, { output_file: outputFile, summary, terminal_status: status, sdk_lifecycle_task: true }]]);
      assert.ok(!updates.some(update => update.type === "user_message_chunk"));
    }
  }
  const minimal = mapSessionMessagesToUpdates([
    { ...base, type: "user", uuid: "minimal", ...{ origin: { kind: "task-notification" } }, message: { role: "user", content: "</task-notification><task-notification><task-id>minimal</task-id><status>completed</status></task-notification>" } },
  ]);
  assert.deepEqual(minimal.filter(update => update.type === "task_state_update").flatMap(update => update.tasks).map(task => [task.task_id, task.status, task.source_tool_call_id, task.description]), [["minimal", "completed", undefined, undefined]]);
});

test("resume settles a subagent task after its tool result has completed", () => {
  const base = { session_id: "fixture", parent_tool_use_id: null, parent_agent_id: null };
  const notification = "<task-notification><task-id>agent-1</task-id><tool-use-id>tool-1</tool-use-id><status>completed</status><summary>Finished work</summary></task-notification>";
  const updates = mapSessionMessagesToUpdates([
    { ...base, type: "assistant", uuid: "launch", message: { role: "assistant", content: [{ type: "tool_use", id: "tool-1", name: "Agent", input: { description: "Work", prompt: "Work" } }] } },
    { ...base, type: "system", uuid: "started", message: { subtype: "task_started", task_id: "agent-1", tool_use_id: "tool-1", task_type: "local_agent", description: "Work" } },
    { ...base, type: "user", uuid: "result", message: { role: "user", content: [{ type: "tool_result", tool_use_id: "tool-1", content: "Done" }] } },
    { ...base, type: "user", uuid: "notification", ...{ origin: { kind: "task-notification", producer: "session-task" } }, message: { role: "user", content: notification } },
    { ...base, type: "system", uuid: "late-progress", message: { subtype: "task_progress", task_id: "agent-1", tool_use_id: "tool-1", description: "Stale progress" } },
  ]);
  const results = updates.filter(update => update.type === "tool_call_update").map(update => update.tool_call_update).filter(update => update.tool_call_id === "tool-1");
  assert.equal(results.length, 2, "only the start and result update the tool");
  assert.equal(results.at(-1)?.fields.status, "completed");
  const rows = updates.filter(update => update.type === "task_state_update").flatMap(update => update.tasks);
  assert.deepEqual(rows.map(row => [row.task_id, row.status, (row.metadata as Record<string, unknown>)?.task_type, (row.metadata as Record<string, unknown>)?.terminal_status]), [
    ["agent-1", "in_progress", "local_agent", undefined],
    ["agent-1", "completed", "local_agent", "completed"],
  ]);
  assert.equal(rows.at(-1)?.description, "Finished work");
});

// Claude Code 2.1.300 saves no task_started record for a background Agent call
// (measured 2026-10-16): only the tool_use, its async-launch result and one
// task-notification per outcome. A user-stopped agent is saved as "killed".
test("resume rebuilds background subagent rows from their Agent calls and notifications", () => {
  const base = { session_id: "fixture", parent_tool_use_id: null, parent_agent_id: null };
  const saved = (toolUseId: string, agentId: string, description: string, status: string, summary: string) => [
    { ...base, type: "assistant" as const, uuid: `launch-${agentId}`, message: { role: "assistant", content: [{ type: "tool_use", id: toolUseId, name: "Agent", input: { description, prompt: `Prompt of ${description}`, subagent_type: "general-purpose", model: "haiku", run_in_background: true } }] } },
    { ...base, type: "user" as const, uuid: `result-${agentId}`, ...{ toolUseResult: { isAsync: true, status: "async_launched", agentId, description, prompt: `Prompt of ${description}`, outputFile: `/tmp/tasks/${agentId}.output` } }, message: { role: "user", content: [{ type: "tool_result", tool_use_id: toolUseId, content: [{ type: "text", text: `Async agent launched successfully.\nagentId: ${agentId} (internal ID - do not mention to user.)\nThe agent is working in the background.` }] }] } },
    { ...base, type: "user" as const, uuid: `notification-${agentId}`, ...{ origin: { kind: "task-notification", producer: "session-task", runId: `run-${agentId}` } }, message: { role: "user", content: `<task-notification>\n<task-id>${agentId}</task-id>\n<tool-use-id>${toolUseId}</tool-use-id>\n<output-file>/tmp/tasks/${agentId}.output</output-file>\n<status>${status}</status>\n<summary>${summary}</summary>\n</task-notification>` } },
  ];
  const updates = mapSessionMessagesToUpdates([
    ...saved("toolu-quick", "a-quick", "Quick reply", "completed", "Agent \"Quick reply\" finished"),
    ...saved("toolu-slow", "a-slow", "Slow facts", "killed", "Agent \"Slow facts\" was stopped by user"),
  ]);
  const rows = new Map<string, { status: string; subject: string; source_tool_call_id?: string; metadata?: unknown }>();
  for (const update of updates) {
    if (update.type === "task_state_update") for (const task of update.tasks) rows.set(task.task_id, task);
  }
  assert.deepEqual([...rows.entries()].map(([id, row]) => {
    const metadata = row.metadata as Record<string, unknown> | undefined;
    return [id, row.subject, row.status, row.source_tool_call_id, metadata?.task_type, metadata?.subagent_type, metadata?.terminal_status];
  }), [
    ["a-quick", "Quick reply", "completed", "toolu-quick", "local_agent", "general-purpose", "completed"],
    ["a-slow", "Slow facts", "completed", "toolu-slow", "local_agent", "general-purpose", "killed"],
  ]);
});

test("a resumed notification for a non-Agent tool stays a plain runtime task", () => {
  const base = { session_id: "fixture", parent_tool_use_id: null, parent_agent_id: null };
  const updates = mapSessionMessagesToUpdates([
    { ...base, type: "assistant", uuid: "launch", message: { role: "assistant", content: [{ type: "tool_use", id: "shell", name: "Bash", input: { command: "sleep 1", run_in_background: true } }] } },
    { ...base, type: "user", uuid: "notification", ...{ origin: { kind: "task-notification", producer: "session-task" } }, message: { role: "user", content: "<task-notification><task-id>bash-1</task-id><tool-use-id>shell</tool-use-id><status>killed</status><summary>stopped</summary></task-notification>" } },
  ]);
  const row = updates.filter(update => update.type === "task_state_update").flatMap(update => update.tasks).at(-1);
  assert.equal(row?.status, "completed");
  const metadata = row?.metadata as Record<string, unknown> | undefined;
  assert.equal(metadata?.terminal_status, "killed");
  assert.equal(metadata?.task_type, undefined);
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
      const brokenCommand = "<command-name>/rename</command-name><command-message>rename</command-message><command-args>".repeat(100_000);
      const base = { session_id: "fixture", parent_tool_use_id: null, parent_agent_id: null };
      const updates = mapSessionMessagesToUpdates([
        ...broken.map((content, index) => ({ ...base, type: "user", uuid: "broken-" + index, origin: { kind: "task-notification" }, message: { role: "user", content } })),
        { ...base, type: "user", uuid: "broken-command", message: { role: "user", content: brokenCommand } },
        { ...base, type: "user", uuid: "after", message: { role: "user", content: "Still restored" } },
      ]);
      process.stdout.write(JSON.stringify({
        chunks: updates.filter(update => update.type === "user_message_chunk").map(update => ({ uuid: update.source_message_uuid, preserved: update.content.text === brokenCommand, tail: update.source_message_uuid === "after" ? update.content : undefined })),
        tasks: updates.filter(update => update.type === "task_state_update").flatMap(update => update.tasks),
      }));
    `,
    encoding: "utf8",
    timeout: 10_000,
    windowsHide: true,
  });
  assert.equal(child.error, undefined, String(child.error));
  assert.equal(child.status, 0, child.stderr);
  const result = JSON.parse(child.stdout);
  assert.deepEqual(result.chunks, [
    { uuid: "broken-command", preserved: true },
    { uuid: "after", preserved: false, tail: { type: "text", text: "Still restored" } },
  ]);
  assert.deepEqual(result.tasks.map((task: { task_id: string; status: string; source_tool_call_id?: string; description?: string }) => [task.task_id, task.status, task.source_tool_call_id, task.description]), [["valid", "completed", undefined, undefined]]);
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

// A background subagent still running when the CLI exited has a launch and no
// notification. On resume nothing owns its process.
test("a background subagent with no recorded outcome is rebuilt as interrupted, not running", () => {
  const base = { session_id: "fixture", parent_tool_use_id: null, parent_agent_id: null };
  const saved = (toolUseId: string, agentId: string, withOutcome: boolean) => [
    { ...base, type: "assistant" as const, uuid: `launch-${agentId}`, message: { role: "assistant", content: [{ type: "tool_use", id: toolUseId, name: "Agent", input: { description: agentId, prompt: "p", subagent_type: "general-purpose", run_in_background: true } }] } },
    { ...base, type: "user" as const, uuid: `result-${agentId}`, ...{ toolUseResult: { isAsync: true, status: "async_launched", agentId, description: agentId, prompt: "p", outputFile: `/tmp/tasks/${agentId}.output` } }, message: { role: "user", content: [{ type: "tool_result", tool_use_id: toolUseId, content: [{ type: "text", text: `Async agent launched successfully.\nagentId: ${agentId}` }] }] } },
    ...(withOutcome ? [{ ...base, type: "user" as const, uuid: `notification-${agentId}`, ...{ origin: { kind: "task-notification", producer: "session-task" } }, message: { role: "user", content: `<task-notification><task-id>${agentId}</task-id><tool-use-id>${toolUseId}</tool-use-id><status>completed</status><summary>ok</summary></task-notification>` } }] : []),
  ];
  const rows = new Map<string, { status: string; metadata?: unknown }>();
  for (const update of mapSessionMessagesToUpdates([...saved("t-open", "a-open", false), ...saved("t-done", "a-done", true)])) {
    if (update.type === "task_state_update") for (const task of update.tasks) rows.set(task.task_id, task);
  }
  const open = rows.get("a-open");
  assert.ok(open, "the launched subagent still gets a row");
  assert.notEqual(open.status, "in_progress", "nothing owns the process after a resume");
  const metadata = open.metadata as Record<string, unknown>;
  assert.equal(metadata.terminal_status, "interrupted");
  assert.equal(metadata.task_type, "local_agent");
  const done = rows.get("a-done");
  assert.ok(done, "the finished subagent has a row");
  assert.equal((done.metadata as Record<string, unknown>).terminal_status, "completed", "a recorded outcome is kept");
});
