import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import type { TaskItem } from "../types.js";
import type { SessionState } from "./session_lifecycle.js";
import { replaceProtocolEventWriter } from "./events.js";
import { restoreSavedSubagentTask } from "./tasks.js";

function session(): SessionState {
  return {
    sessionId: "fixture", cwd: "fixture", connected: true, closing: false,
    tasksById: new Map(), taskOrder: [], toolCalls: new Map(),
    taskToolUseIds: new Map(), taskIdsByToolUseId: new Map(),
  } as unknown as SessionState;
}

test("saved discovery never replaces a live or recorded newer execution and can enrich ancestry", () => {
  const state = session();
  const current: TaskItem = {
    task_id: "child", run_id: "B", subject: "Current child", status: "in_progress", blocks: [], blocked_by: [],
    source_tool_call_id: "launch-B", source_tool_call_run_id: "B", metadata: { task_type: "local_agent", summary: "B working" },
  };
  state.tasksById.set("child", current);
  state.taskOrder.push("child");
  state.taskToolUseIds.set("child", "launch-B");
  const writes: string[] = [];
  const restore = replaceProtocolEventWriter(line => { writes.push(line); });
  try {
    for (const status of ["in_progress", "completed"] as const) {
      current.status = status;
      const existing = state.tasksById.get("child");
      assert.ok(existing);
      state.tasksById.set("child", { ...existing, status });
      restoreSavedSubagentTask(state, "child", "parent", [{ subtype: "task_notification", message: { run_id: "A", tool_use_id: "launch-A", status: "failed", summary: "Old A" } }]);
      const task = state.tasksById.get("child");
      assert.deepEqual(task, { ...current, parent_task_id: "parent" });
      assert.equal(state.taskToolUseIds.get("child"), "launch-B");
      assert.deepEqual(state.taskOrder, ["child"]);
    }
    assert.equal(writes.length, 1, "only the missing ancestry is published");
    state.tasksById.clear();
    state.removedTaskRuns = new Map([["child", "B"]]);
    restoreSavedSubagentTask(state, "child", "parent", [{ subtype: "task_notification", message: { run_id: "A", status: "completed" } }]);
    assert.equal(state.tasksById.has("child"), false, "removed newer runs cannot be resurrected by old saved evidence");
  } finally { restore(); }
});

test("an incomplete saved row can recover its metadata and outcome on refresh", () => {
  const state = session();
  const restore = replaceProtocolEventWriter(() => {});
  try {
    restoreSavedSubagentTask(state, "child", undefined, [{ subtype: "task_started", message: { task_type: "local_agent", synthesized_launch: true } }]);
    assert.equal((state.tasksById.get("child")?.metadata as Record<string, unknown>)?.terminal_status, "interrupted");
    restoreSavedSubagentTask(state, "child", "parent", [
      { subtype: "task_started", message: { task_type: "local_agent", subject: "Recovered child", tool_use_id: "launch", synthesized_launch: true } },
      { subtype: "task_notification", message: { tool_use_id: "launch", status: "completed" } },
    ]);
    assert.equal(state.tasksById.get("child")?.subject, "Recovered child");
    assert.equal(state.tasksById.get("child")?.parent_task_id, "parent");
    assert.equal((state.tasksById.get("child")?.metadata as Record<string, unknown>)?.terminal_status, "completed");
    assert.deepEqual(state.taskOrder, ["child"]);
  } finally { restore(); }
});

test("SDK saved discovery restores all depths independently of chat replay across repeated resumes", () => {
  const directory = mkdtempSync(join(tmpdir(), "claude-rs-saved-agents-"));
  const profile = join(directory, "profile");
  const projectName = "saved-agent-fixture";
  const project = join(profile, "projects", projectName);
  const id = "11111111-2222-4333-8444-555555555555";
  const subagents = join(project, id, "subagents");
  mkdirSync(subagents, { recursive: true });
  const paths: string[] = [];
  const writeHistory = (path: string, records: Record<string, unknown>[]) => {
    let previous: string | null = null;
    const lines = records.map((record, index) => {
      const uuid = String(record.uuid ?? `record-${index}`);
      const row = { parentUuid: previous, sessionId: id, cwd: directory, timestamp: new Date(1_700_000_000_000 + index * 1000).toISOString(), ...record, uuid };
      previous = uuid;
      return JSON.stringify(row);
    });
    paths.push(path); writeFileSync(path, `${lines.join("\n")}\n`);
  };
  const call = (tool: string, name: string, input: Record<string, unknown>) => ({ type: "assistant", message: { role: "assistant", content: [{ type: "tool_use", id: tool, name, input }] } });
  const result = (tool: string, content: unknown) => ({ type: "user", message: { role: "user", content: [{ type: "tool_result", tool_use_id: tool, content }] } });
  const prompt = (content: string) => ({ type: "user", message: { role: "user", content } });
  const notification = (task: string, runId: string, tool: string, status: string) => ({
    ...prompt(`<task-notification><task-id>${task}</task-id><tool-use-id>${tool}</tool-use-id><status>${status}</status><summary>${task} ${status}</summary></task-notification>`),
    origin: { kind: "task-notification", producer: "session-task", runId },
  });
  try {
    writeHistory(join(project, `${id}.jsonl`), [
      prompt("Main prompt"),
      call("parent-launch", "Agent", { description: "Migration Parent", subagent_type: "general-purpose", run_in_background: true }),
      result("parent-launch", "Async agent launched"),
      notification("parent", "A", "parent-launch", "stopped"),
      { type: "assistant", message: { role: "assistant", content: [{ type: "text", text: "Main reply only" }] } },
    ]);
    writeHistory(join(subagents, "agent-parent.jsonl"), [
      prompt("Private parent prompt"),
      call("child-launch", "Agent", { description: "Migration Child", subagent_type: "reviewer", prompt: "Private child instructions", run_in_background: false }),
      result("child-launch", [{ type: "text", text: "agentId: child\nChild finished." }]),
      call("unfinished-launch", "Agent", { description: "Unfinished child", run_in_background: true }),
      result("unfinished-launch", "Async agent launched successfully"),
      call("ack-launch", "Agent", { description: "Foreground call returned asynchronously", run_in_background: false }),
      result("ack-launch", [{ type: "text", text: "Async agent launched successfully.\nagentId: acknowledgement" }]),
      call("structured-ack-launch", "Agent", { description: "Structured async acknowledgement", run_in_background: false }),
      result("structured-ack-launch", [{ type: "text", text: JSON.stringify({ status: "async_launched", agentId: "structured-ack" }) }]),
      call("resumed-launch", "Agent", { description: "Resumed child", run_in_background: true }),
      result("resumed-launch", "Async agent launched"),
      notification("resumed", "A", "resumed-launch", "failed"),
      call("resume-B", "SendMessage", { to: "resumed", message: "Continue" }),
      result("resume-B", [{ type: "text", text: JSON.stringify({ success: true, resumedAgentId: "resumed" }) }]),
      notification("resumed", "B", "resume-B", "completed"),
      notification("resumed", "A", "resumed-launch", "stopped"),
    ]);
    for (const [agent, tool] of [["child", "child-launch"], ["unfinished", "unfinished-launch"], ["acknowledgement", "ack-launch"], ["structured-ack", "structured-ack-launch"], ["resumed", "resumed-launch"]]) {
      writeHistory(join(subagents, `agent-${agent}.jsonl`), [prompt("Private saved conversation"), call(`${agent}-report`, "SubagentHandback", { message: "Saved report alone is not an outcome" })]);
      writeFileSync(join(subagents, `agent-${agent}.meta.json`), JSON.stringify({ parentAgentId: "parent", toolUseId: tool, agentType: "general-purpose" }));
    }
    writeFileSync(join(subagents, "agent-parent.meta.json"), JSON.stringify({ toolUseId: "parent-launch", agentType: "general-purpose" }));
    const before = paths.map(path => readFileSync(path, "utf8"));
    const script = `
      import { listSubagents } from '@anthropic-ai/claude-agent-sdk';
      import { restoreSavedSubagents } from ${JSON.stringify(new URL("./saved_subagents.js", import.meta.url).href)};
      import { replaceProtocolEventWriter } from ${JSON.stringify(new URL("./events.js", import.meta.url).href)};
      import { getSessionTranscriptMessages, mapSessionMessagesToUpdates } from ${JSON.stringify(new URL("./history.js", import.meta.url).href)};
      const make = () => ({ sessionId: ${JSON.stringify(id)}, cwd: ${JSON.stringify(directory)}, connected: true, closing: false, tasksById: new Map(), taskOrder: [], toolCalls: new Map(), taskToolUseIds: new Map(), taskIdsByToolUseId: new Map() });
      const events=[]; const restore=replaceProtocolEventWriter(line=>events.push(JSON.parse(line)));
      const baseline=mapSessionMessagesToUpdates(await getSessionTranscriptMessages(${JSON.stringify(id)}));
      const ids=await listSubagents(${JSON.stringify(id)});
      const states=[];
      for(let resume=0; resume<2; resume++) {
        const state=make();
        await restoreSavedSubagents(state,ids);
        await restoreSavedSubagents(state,ids);
        states.push({tasks:[...state.tasksById.values()],order:state.taskOrder});
      }
      const state=make();
      const pending=restoreSavedSubagents(state,ids);
      state.tasksById.set('child',{task_id:'child',run_id:'Z',subject:'Live Z',status:'in_progress',blocks:[],blocked_by:[],metadata:{task_type:'local_agent'},source_tool_call_id:'launch-Z'});
      await pending;
      const closing=make();
      const closingRead=restoreSavedSubagents(closing,ids);
      closing.closing=true;
      await closingRead;
      const after=mapSessionMessagesToUpdates(await getSessionTranscriptMessages(${JSON.stringify(id)}));
      restore(); console.log(JSON.stringify({states,events,baseline,after,racingChild:state.tasksById.get('child'),closedTaskCount:closing.tasksById.size}));
    `;
    const child = spawnSync(process.execPath, ["--input-type=module", "-"], {
      input: script, encoding: "utf8", windowsHide: true,
      env: { ...process.env, CLAUDE_CONFIG_DIR: profile, CLAUDE_CODE_PROJECT_DIR_NAME: projectName },
    });
    assert.equal(child.status, 0, child.stderr);
    const output = JSON.parse(child.stdout);
    assert.deepEqual(output.baseline, output.after, "agent discovery does not change chat replay");
    assert.deepEqual(output.states[0], output.states[1], "reopening the session rebuilds the same inventory");
    const tasks = new Map<string, TaskItem>(output.states[0].tasks.map((task: TaskItem) => [task.task_id, task]));
    const metadata = (id: string) => {
      const task = tasks.get(id);
      assert.ok(task);
      assert.ok(task.metadata);
      return task.metadata as Record<string, unknown>;
    };
    assert.deepEqual([...tasks.keys()].sort(), ["acknowledgement", "child", "parent", "resumed", "structured-ack", "unfinished"]);
    assert.equal(output.states[0].order.length, tasks.size, "refreshing does not duplicate rows");
    assert.equal(metadata("parent").terminal_status, "stopped");
    assert.equal(tasks.get("child")?.subject, "Migration Child");
    assert.equal(tasks.get("child")?.parent_task_id, "parent");
    assert.equal(tasks.get("child")?.source_tool_call_id, "child-launch");
    assert.equal(metadata("child").subagent_type, "reviewer");
    assert.equal(metadata("child").terminal_status, "completed");
    assert.equal(metadata("unfinished").terminal_status, "interrupted");
    assert.equal(metadata("acknowledgement").terminal_status, "interrupted", "a foreground request can return an async launch acknowledgement");
    assert.equal(metadata("structured-ack").terminal_status, "interrupted");
    assert.equal(tasks.get("resumed")?.run_id, "B");
    assert.equal(tasks.get("resumed")?.source_tool_call_id, "resume-B");
    assert.equal(metadata("resumed").terminal_status, "completed");
    assert.equal(output.racingChild.run_id, "Z");
    assert.equal(output.racingChild.status, "in_progress");
    assert.equal(output.racingChild.subject, "Live Z");
    assert.equal(output.closedTaskCount, 0, "a session closed during discovery receives no task mutations");
    assert.ok(output.events.every((event: { update: { type: string } }) => event.update.type === "task_state_update"), "metadata discovery emits no chat text, tool cards or notifications");
    assert.deepEqual(paths.map(path => readFileSync(path, "utf8")), before, "discovery never rewrites saved conversations");
  } finally { rmSync(directory, { recursive: true, force: true }); }
});
