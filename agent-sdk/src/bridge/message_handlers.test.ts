import assert from "node:assert/strict";
import { mkdtempSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test, { after } from "node:test";
import { registerHooks } from "node:module";
import type { SDKMessage } from "@anthropic-ai/claude-agent-sdk";
// This suite owns ingress/publication. Title/list APIs are controlled here so
// result ingress cannot read a user's profile or outlive the captured writer.
const sdkUrl = import.meta.resolve("@anthropic-ai/claude-agent-sdk");
const controlledSdkUrl = `data:text/javascript,${encodeURIComponent(`
  export * from ${JSON.stringify(sdkUrl)};
  const pending = new Set();
  function read(value) { const operation = Promise.resolve().then(() => value);pending.add(operation);void operation.finally(() => pending.delete(operation));return operation; }
  export function getSessionInfo() { return read(undefined); }
  export function listSessions() { return read([]); }
  export async function settleReads() { while (pending.size) await Promise.all([...pending]); }
`)}`;
const loader = registerHooks({
  resolve(specifier, context, next) {
    return specifier === "@anthropic-ai/claude-agent-sdk" && !context.conditions.includes("require")
      ? { url: controlledSdkUrl, shortCircuit: true }
      : next(specifier, context);
  },
});
const controlledSdk = await import(controlledSdkUrl);
const settleSdkReads: () => Promise<void> = controlledSdk.settleReads;
const scratch = realpathSync(mkdtempSync(join(tmpdir(), "handler-ingress-")));
const { emitConnectEvent, replaceProtocolEventWriter } = await import("./events.js");
const { bridgeLogger } = await import("./logger.js");
const { handleSdkMessage } = await import("./message_handlers.js");
import type { SessionState } from "./session_lifecycle.js";
const { handleSubagentCommand } = await import("./subagents.js");
const { sessions } = await import("./session_lifecycle.js");

loader.deregister();
after(async () => {
  await settleSdkReads();
  rmSync(scratch, { recursive: true, force: true });
});

async function taskScenario(
  run: (
    session: SessionState,
    emit: (message: Record<string, unknown>) => void,
    writes: string[],
  ) => void | Promise<void>,
): Promise<void> {
  const session = makeSession();
  const writes: string[] = [];
  const restore = replaceProtocolEventWriter((line) => {
    writes.push(line);
  });
  try {
    await run(
      session,
      (message) =>
        handleSdkMessage(session, {
          session_id: session.sessionId,
          uuid: crypto.randomUUID(),
          ...message,
        } as unknown as SDKMessage),
      writes,
    );
  } finally {
    await settleSdkReads();
    restore();
  }
}

type Wire = Record<string, unknown>;
const envelopes = (writes: string[], start = 0): Wire[] =>
  writes.slice(start).map((line) => JSON.parse(line));
const publicUpdates = (writes: string[], start = 0): Wire[] =>
  envelopes(writes, start).flatMap((event) => (event.update ? [event.update as Wire] : []));
const taskRows = (writes: string[], id: string, start = 0): Wire[] =>
  publicUpdates(writes, start).flatMap((update) =>
    update.type === "task_state_update"
      ? (update.tasks as Wire[]).filter((task) => task.task_id === id)
      : [],
  );
function publicTool(writes: string[], id: string): Wire | undefined {
  let tool: Wire | undefined;
  for (const update of publicUpdates(writes)) {
    if (update.type === "tool_call" && (update.tool_call as Wire).tool_call_id === id)
      tool = { ...(update.tool_call as Wire) };
    if (update.type === "tool_call_update" && (update.tool_call_update as Wire).tool_call_id === id)
      tool = { ...tool, ...((update.tool_call_update as Wire).fields as Wire) };
  }
  return tool;
}
const toolDeltas = (writes: string[], id: string, start = 0): Wire[] =>
  publicUpdates(writes, start).flatMap((update) =>
    update.type === "tool_call_update" && (update.tool_call_update as Wire).tool_call_id === id
      ? [(update.tool_call_update as Wire).fields as Wire]
      : [],
  );
type Ingress = (message: Wire) => void;
function invoke(
  emit: Ingress,
  id: string,
  name = "Agent",
  input: Wire = { description: "Work", prompt: "Perform the work" },
  parent: string | null = null,
): void {
  emit({
    type: "assistant",
    uuid: `source-${id}`,
    parent_tool_use_id: parent,
    message: {
      id: `response-${id}`,
      role: "assistant",
      content: [{ type: "tool_use", id, name, input }],
    },
  });
}

test("each API-rejected send reports its error even when only the first emits a rate limit update", () => taskScenario((session, emit, writes) => {
  const errorText = "You've hit your session limit · resets 1:30pm (Europe/Berlin)";
  for (const [index, prompt] of ["hello", "okay", "hello"].entries()) {
    const before = writes.length;
    const userUuid = `user-${index}`;
    emit({ type: "user", uuid: userUuid, message: { role: "user", content: prompt } });
    emit({ type: "assistant", error: "rate_limit", user_message_uuid: userUuid, message: { id: `reply-${index}`, content: [{ type: "text", text: errorText }] } });
    if (index === 0) {
      emit({ type: "rate_limit_event", rate_limit_info: { status: "rejected", rateLimitType: "five_hour", resetsAt: 1792409400 } });
    }
    emit({ type: "result", subtype: "success", is_error: true, terminal_reason: "api_error", result: errorText, user_message_uuid: userUuid, queued_turn_count: 0 });
    const events = writes.slice(before).map(line => JSON.parse(line));
    assert.deepEqual(events.filter(event => event.event === "turn_error" || event.event === "turn_complete"), [{
      event: "turn_error", session_id: session.sessionId, message: errorText,
      error_kind: "plan_limit", assistant_error: "rate_limit", sdk_result_subtype: "success",
      terminal_reason: "api_error", queued_turn_count: 0,
    }]);
    assert.equal(events.filter(event => event.update?.type === "rate_limit_update").length, index === 0 ? 1 : 0);
    assert.equal(events.some(event => event.update?.type === "agent_message_chunk"), false, "the error is carried once through turn_error");
  }
}));

test("either native failure signal settles the actual foreground tool and its own failed turn", async () => {
  for (const signal of [{ is_error: true }, { terminal_reason: "api_error" }, {}])
    await taskScenario((session, emit, writes) => {
      invoke(emit, "selected-read", "Read", { file_path: "notes.md" });
      const phase = writes.length,
        failed = Object.keys(signal).length > 0;
      emit({
        type: "result",
        uuid: "selected-result",
        user_message_uuid: "selected-turn",
        subtype: "success",
        result: failed ? "HTTP 429: request rejected" : "Read completed",
        api_error_status: failed ? 429 : undefined,
        ...signal,
      });
      const events = envelopes(writes, phase),
        terminals = events.filter(
          (event) => event.event === "turn_error" || event.event === "turn_complete",
        );
      assert.equal(terminals.length, 1);
      assert.equal(terminals[0].session_id, session.sessionId);
      assert.equal(terminals[0].event, failed ? "turn_error" : "turn_complete");
      if (failed) {
        assert.equal(terminals[0].message, "HTTP 429: request rejected");
        assert.equal(terminals[0].error_kind, "plan_limit");
        assert.equal(terminals[0].api_error_status, 429);
      }
      assert.ok(
        toolDeltas(writes, "selected-read", phase).some(
          (fields) => fields.status === (failed ? "failed" : "completed"),
        ),
      );
    });
});

test("recovery and a separately identified later failure do not inherit the earlier assistant error", () =>
  taskScenario((session, emit, writes) => {
    emit({
      type: "user",
      uuid: "earlier-user",
      message: { role: "user", content: "Earlier request" },
    });
    emit({
      type: "assistant",
      uuid: "earlier-error",
      error: "rate_limit",
      message: { id: "earlier-response", content: [{ type: "text", text: "temporary rejection" }] },
    });
    emit({
      type: "assistant",
      uuid: "recovered-source",
      user_message_uuid: "earlier-user",
      message: { id: "recovered-response", content: [{ type: "text", text: "Recovered answer" }] },
    });
    emit({
      type: "result",
      uuid: "recovered-result",
      user_message_uuid: "earlier-user",
      subtype: "success",
      is_error: false,
      terminal_reason: "completed",
      result: "Recovered answer",
    });
    assert.deepEqual(
      envelopes(writes)
        .filter((event) => event.event === "turn_complete" || event.event === "turn_error")
        .map((event) => [event.event, event.session_id]),
      [["turn_complete", session.sessionId]],
    );
    assert.deepEqual(
      publicUpdates(writes)
        .filter((update) => update.type === "agent_message_chunk")
        .map((update) => [update.source_message_uuid, update.content]),
      [["recovered-source", { type: "text", text: "Recovered answer" }]],
    );
    emit({
      type: "user",
      uuid: "later-user",
      message: { role: "user", content: "Independent later request" },
    });
    const phase = writes.length;
    emit({
      type: "result",
      uuid: "later-result",
      user_message_uuid: "later-user",
      subtype: "error_during_execution",
      errors: ["A separate failure"],
    });
    const errors = envelopes(writes, phase).filter((event) => event.event === "turn_error");
    assert.equal(errors.length, 1);
    assert.equal(errors[0].session_id, session.sessionId);
    assert.equal(errors[0].message, "A separate failure");
    assert.equal(errors[0].error_kind, "other");
    assert.equal(errors[0].assistant_error, undefined);
    assert.ok(!JSON.stringify(envelopes(writes, phase)).includes("temporary rejection"));
  }));

test("stale lifecycle and inventory frames cannot publish into a newer run or rewrite historical output", async () => {
  for (const [taskType, toolName] of [
    ["local_agent", "Agent"],
    ["local_bash", "Bash"],
    ["local_workflow", "Workflow"],
    ["local_mcp", "mcp__test__work"],
  ])
    await taskScenario((session, emit, writes) => {
      const input = (label: string) =>
        toolName === "Agent"
          ? { description: label, prompt: `Perform ${label}` }
          : toolName === "Bash"
            ? { command: `echo ${label}` }
            : {};
      invoke(emit, "launch-A", toolName, input("A"));
      const edge = (subtype: string, run_id: string, fields: Wire = {}) =>
        emit({ type: "system", subtype, task_id: "worker", run_id, ...fields });
      edge("task_started", "run-A", {
        tool_use_id: "launch-A",
        task_type: taskType,
        description: "A",
        parent_task_id: "parent",
      });
      emit({
        type: "user",
        message: {
          role: "user",
          content: [
            { type: "tool_result", tool_use_id: "launch-A", is_error: true, content: "A report" },
          ],
        },
      });
      edge("task_notification", "run-A", {
        status: "failed",
        summary: "A report",
        error: "A error",
        end_time: 123,
      });
      assert.ok(envelopes(writes).every(event => event.session_id === session.sessionId));
      const historical = structuredClone(publicTool(writes, "launch-A"));
      assert.equal(historical?.status, "failed");
      assert.ok(
        String(historical?.raw_output).includes("A report"),
        JSON.stringify({ toolName, historical, task: taskRows(writes, "worker").at(-1) }),
      );
      invoke(emit, "launch-B", toolName, input("B"));
      edge("task_started", "run-B", {
        tool_use_id: "launch-B",
        task_type: taskType,
        description: "B",
        is_backgrounded: true,
      });
      emit({
        type: "system",
        subtype: "task_started",
        task_id: "sibling",
        run_id: "sibling-run",
        task_type: "local_agent",
        description: "Sibling stays active",
      });
      const stale = [
        {
          type: "system",
          subtype: "task_started",
          task_id: "worker",
          run_id: "run-A",
          tool_use_id: "launch-A",
          description: "STALE-RUN start",
          parent_task_id: "wrong-parent",
        },
        {
          type: "system",
          subtype: "task_progress",
          task_id: "worker",
          run_id: "run-A",
          tool_use_id: "launch-A",
          description: "STALE-RUN progress",
          usage: { duration_ms: 999 },
        },
        {
          type: "system",
          subtype: "task_updated",
          task_id: "worker",
          run_id: "run-A",
          patch: { status: "failed", description: "STALE-RUN update", error: "STALE-RUN error" },
        },
        {
          type: "system",
          subtype: "task_notification",
          task_id: "worker",
          run_id: "run-A",
          tool_use_id: "launch-A",
          status: "stopped",
          summary: "STALE-RUN outcome",
        },
        {
          type: "system",
          subtype: "background_tasks_changed",
          tasks: [
            {
              task_id: "worker",
              run_id: "run-A",
              task_type: taskType,
              description: "STALE-RUN inventory",
            },
          ],
        },
      ];
      for (const frame of stale) {
        const phase = writes.length;
        emit(frame);
        assert.ok(
          !JSON.stringify(envelopes(writes, phase)).includes("STALE-RUN"),
          `${toolName}/${frame.subtype}: stale payload never crosses the wire`,
        );
        for (const row of taskRows(writes, "worker", phase)) {
          assert.equal(row.run_id, "run-B");
          assert.equal(row.source_tool_call_id, "launch-B");
          assert.equal(row.parent_task_id, "parent");
          assert.equal((row.metadata as Wire)?.error, undefined);
          assert.equal((row.metadata as Wire)?.summary, undefined);
        }
        assert.deepEqual(publicTool(writes, "launch-A"), historical);
        assert.equal(publicTool(writes, "launch-B")?.source_message_uuid, "source-launch-B");
        assert.ok(
          !["completed", "failed", "killed"].includes(
            String(publicTool(writes, "launch-B")?.status),
          ),
        );
        assert.equal(taskRows(writes, "sibling").at(-1)?.status, "in_progress");
        assert.ok(
          !toolDeltas(writes, "launch-B", phase).some(
            (fields) => ((fields.output_metadata as Wire)?.timing as Wire)?.duration_ms === 999,
          ),
        );
      }
      const phase = writes.length;
      edge("task_notification", "run-B", { status: "completed", summary: "B report" });
      edge("task_started", "run-B", { description: "Delayed same-run start" });
      for (const row of taskRows(writes, "worker", phase)) {
        assert.equal(row.run_id, "run-B");
        assert.equal(row.status, "completed");
        assert.equal((row.metadata as Wire)?.summary, "B report");
      }
      assert.equal(publicTool(writes, "launch-B")?.status, "completed");
      assert.equal((publicTool(writes, "launch-B")?.task_metadata as Wire)?.summary, "B report");
      assert.deepEqual(publicTool(writes, "launch-A"), historical);
    });
  for (const run_id of [undefined, "recorded-run"])
    await taskScenario((_session, emit, writes) => {
      invoke(emit, "late-launch");
      emit({
        type: "system",
        subtype: "task_started",
        task_id: "late-task",
        run_id,
        tool_use_id: "late-launch",
        task_type: "local_agent",
        description: "Launch",
      });
      emit({
        type: "user",
        message: {
          role: "user",
          content: [{ type: "tool_result", tool_use_id: "late-launch", content: "Actual report" }],
        },
      });
      emit({
        type: "system",
        subtype: "task_notification",
        task_id: "late-task",
        run_id,
        tool_use_id: "late-launch",
        status: "completed",
        summary: "Actual report",
      });
      const final = structuredClone(publicTool(writes, "late-launch")),
        phase = writes.length;
      assert.equal(final?.raw_output, "Actual report");
      emit({
        type: "user",
        tool_use_result: {
          isAsync: true,
          status: "async_launched",
          agentId: "late-task",
          description: "Late launch ack",
          outputFile: "worker.output",
        },
        message: {
          role: "user",
          content: [
            { type: "tool_result", tool_use_id: "late-launch", content: "Late launch ack" },
          ],
        },
      });
      assert.deepEqual(publicTool(writes, "late-launch"), final);
      for (const row of taskRows(writes, "late-task", phase)) {
        assert.equal(row.status, "completed");
        assert.equal((row.metadata as Wire)?.summary, "Actual report");
      }
    });
});

test("a new completed run publishes its own report without reopening the historical invocation", () =>
  taskScenario((session, emit, writes) => {
    invoke(emit, "launch");
    emit({
      type: "system",
      subtype: "task_started",
      task_id: "worker",
      tool_use_id: "launch",
      run_id: "run-A",
      task_type: "local_agent",
      description: "Worker",
    });
    emit({
      type: "system",
      subtype: "task_notification",
      task_id: "worker",
      run_id: "run-A",
      status: "stopped",
      summary: "A stopped",
      error: "A error",
    });
    const historical = structuredClone(publicTool(writes, "launch")),
      phase = writes.length;
    emit({
      type: "system",
      subtype: "task_notification",
      task_id: "worker",
      run_id: "run-B",
      status: "completed",
      summary: "B done",
    });
    const row = taskRows(writes, "worker", phase).at(-1);
    assert.equal(row?.run_id, "run-B");
    assert.equal(row?.status, "completed");
    assert.equal(row?.source_tool_call_id, "launch");
    assert.equal(row?.source_tool_call_run_id, "run-A");
    assert.equal((row?.metadata as Wire)?.summary, "B done");
    assert.equal((row?.metadata as Wire)?.error, undefined);
    assert.deepEqual(publicTool(writes, "launch"), historical);
    assert.deepEqual(toolDeltas(writes, "launch", phase), []);
    emit({
      type: "assistant",
      uuid: "child-source",
      agent_id: "worker",
      message: {
        id: "child-response",
        content: [{ type: "tool_use", id: "child-B", name: "Read", input: { file_path: "notes" } }],
      },
    });
    assert.equal(
      ((publicTool(writes, "child-B")?.meta as Wire)?.claudeCode as Wire)?.parentToolUseId,
      "launch",
    );
    assert.equal(publicTool(writes, "child-B")?.source_message_uuid, "child-source");
    const stale = writes.length;
    emit({
      type: "system",
      subtype: "task_notification",
      task_id: "worker",
      tool_use_id: "launch",
      status: "failed",
      summary: "unversioned A",
    });
    assert.ok(!JSON.stringify(envelopes(writes, stale)).includes("unversioned A"));
    assert.deepEqual(publicTool(writes, "launch"), historical);
    assert.equal(taskRows(writes, "worker").at(-1)?.run_id, "run-B");
    assert.ok(envelopes(writes).every(event => event.session_id === session.sessionId));
  }));

test("actual parent result ingress settles foreground work while resumed background descendants keep their own outcomes", async () => {
  for (const failed of [false, true])
    await taskScenario((session, emit, writes) => {
      invoke(emit, "launch");
      emit({
        type: "system",
        subtype: "task_started",
        task_id: "worker",
        run_id: "A",
        tool_use_id: "launch",
        task_type: "local_agent",
        description: "Worker",
      });
      emit({
        type: "system",
        subtype: "task_notification",
        task_id: "worker",
        run_id: "A",
        status: "stopped",
        summary: "Historical A stopped",
      });
      const historical = structuredClone(publicTool(writes, "launch"));
      emit({
        type: "system",
        subtype: "task_started",
        task_id: "worker",
        run_id: "B",
        is_backgrounded: true,
        description: "B resumed",
      });
      emit({
        type: "assistant",
        uuid: "child-source",
        agent_id: "worker",
        message: {
          id: "child-response",
          content: [{ type: "tool_use", id: "child", name: "Read", input: { file_path: "notes" } }],
        },
      });
      invoke(emit, "foreground", "Read", { file_path: "root-notes" });
      const phase = writes.length;
      emit({
        type: "result",
        uuid: "root-result",
        subtype: failed ? "error_during_execution" : "success",
        ...(failed ? { errors: ["Root failed"] } : { result: "Root complete" }),
      });
      assert.ok(
        envelopes(writes, phase).some(
          (event) =>
            event.session_id === session.sessionId &&
            event.event === (failed ? "turn_error" : "turn_complete"),
        ),
      );
      assert.equal(publicTool(writes, "foreground")?.status, failed ? "failed" : "completed");
      assert.equal(publicTool(writes, "child")?.status, "in_progress");
      assert.deepEqual(toolDeltas(writes, "child", phase), []);
      assert.deepEqual(publicTool(writes, "launch"), historical);
      emit({
        type: "user",
        uuid: "child-result",
        agent_id: "worker",
        parent_tool_use_id: "launch",
        message: {
          role: "user",
          content: [
            { type: "tool_result", tool_use_id: "child", content: "Independent child output" },
          ],
        },
      });
      assert.equal(publicTool(writes, "child")?.status, "completed");
      assert.equal(publicTool(writes, "child")?.raw_output, "Independent child output");
      assert.deepEqual(publicTool(writes, "launch"), historical);
    });
});

test("first run observation retains invocation provenance and only matching legacy outcomes are admitted", () =>
  taskScenario((session, emit, writes) => {
    invoke(emit, "launch", "Agent", { description: "Original launch", prompt: "Original prompt" });
    emit({
      type: "system",
      subtype: "task_started",
      task_id: "worker",
      tool_use_id: "launch",
      task_type: "local_agent",
      description: "Original launch",
      prompt: "Original prompt",
    });
    emit({
      type: "system",
      subtype: "task_progress",
      task_id: "worker",
      run_id: "B",
      description: "Working",
    });
    const current = taskRows(writes, "worker").at(-1);
    assert.equal(current?.run_id, "B");
    assert.equal(current?.source_tool_call_id, "launch");
    assert.equal((current?.metadata as Wire)?.prompt, "Original prompt");
    for (const tool_use_id of [undefined, "old-launch"]) {
      const phase = writes.length;
      emit({
        type: "system",
        subtype: "task_notification",
        task_id: "worker",
        tool_use_id,
        status: "failed",
        summary: "unidentifiable stale report",
      });
      assert.ok(!JSON.stringify(envelopes(writes, phase)).includes("unidentifiable stale report"));
      assert.deepEqual(taskRows(writes, "worker").at(-1), current);
      assert.equal(publicTool(writes, "launch")?.status, "in_progress");
    }
    const phase = writes.length;
    emit({
      type: "system",
      subtype: "task_notification",
      task_id: "worker",
      tool_use_id: "launch",
      status: "completed",
      summary: "Matched legacy outcome",
    });
    const completed = taskRows(writes, "worker", phase).at(-1);
    assert.equal(completed?.run_id, "B");
    assert.equal(completed?.status, "completed");
    assert.equal((completed?.metadata as Wire)?.summary, "Matched legacy outcome");
    assert.equal(completed?.source_tool_call_id, "launch");
    assert.equal((completed?.metadata as Wire)?.prompt, "Original prompt");
    assert.ok(envelopes(writes).every(event => event.session_id === session.sessionId));
  }));

test("inventory removals reject older runs but admit newer runs and the removed run's real final outcome", async () => {
  await taskScenario((session, emit, writes) => {
    const listed = (run_id: string) =>
      emit({
        type: "system",
        subtype: "background_tasks_changed",
        tasks: [
          { task_id: "worker", run_id, task_type: "local_agent", description: `Listed ${run_id}` },
        ],
      });
    listed("B");
    assert.equal(taskRows(writes, "worker").at(-1)?.run_id, "B");
    let phase = writes.length;
    emit({ type: "system", subtype: "background_tasks_changed", tasks: [] });
    assert.ok(
      publicUpdates(writes, phase).some((update) =>
        (update.removed_task_ids as string[] | undefined)?.includes("worker"),
      ),
    );
    phase = writes.length;
    listed("A");
    assert.equal(taskRows(writes, "worker", phase).length, 0, "older absent run cannot reappear");
    phase = writes.length;
    listed("C");
    const revived = taskRows(writes, "worker", phase).at(-1);
    assert.equal(revived?.run_id, "C");
    assert.equal(revived?.status, "in_progress");
    emit({
      type: "system",
      subtype: "task_started",
      task_id: "worker",
      run_id: "C",
      task_type: "local_agent",
      description: "C live",
    });
    phase = writes.length;
    emit({ type: "system", subtype: "background_tasks_changed", tasks: [] });
    const interrupted = taskRows(writes, "worker", phase).at(-1);
    assert.equal(interrupted?.run_id, "C");
    assert.equal(interrupted?.status, "completed");
    assert.equal((interrupted?.metadata as Wire)?.terminal_status, "interrupted");
    phase = writes.length;
    listed("C");
    assert.equal(taskRows(writes, "worker", phase).at(-1)?.status, "in_progress");
    for (const event of envelopes(writes)) assert.equal(event.session_id, session.sessionId);
  });
  await taskScenario((_session, emit, writes) => {
    emit({
      type: "system",
      subtype: "background_tasks_changed",
      tasks: [{ task_id: "removed", run_id: "B", task_type: "local_agent" }],
    });
    emit({ type: "system", subtype: "background_tasks_changed", tasks: [] });
    let phase = writes.length;
    emit({
      type: "system",
      subtype: "task_notification",
      task_id: "removed",
      run_id: "A",
      status: "completed",
      summary: "STALE removed run",
    });
    assert.equal(taskRows(writes, "removed", phase).length, 0);
    assert.ok(!JSON.stringify(envelopes(writes, phase)).includes("STALE removed run"));
    phase = writes.length;
    emit({
      type: "system",
      subtype: "task_notification",
      task_id: "removed",
      run_id: "B",
      status: "completed",
      summary: "Actual removed-run B report",
      task_type: "local_agent",
    });
    const restored = taskRows(writes, "removed", phase).at(-1);
    assert.equal(restored?.run_id, "B");
    assert.equal(restored?.status, "completed");
    assert.equal((restored?.metadata as Wire)?.summary, "Actual removed-run B report");
  });
});

test("published child tools retain producer and explicit parent identities without contaminating a later root reply", () =>
  taskScenario((session, emit, writes) => {
    invoke(emit, "launch-B");
    emit({
      type: "system",
      subtype: "task_started",
      task_id: "B",
      run_id: "B1",
      tool_use_id: "launch-B",
      task_type: "local_agent",
      description: "B",
    });
    const phase = writes.length;
    emit({
      type: "assistant",
      uuid: "child-source",
      agent_id: "B",
      parent_tool_use_id: null,
      error: "authentication_failed",
      context_usage: {},
      message: {
        id: "child",
        content: [
          { type: "text", text: "Private child reply" },
          {
            type: "tool_use",
            id: "launch-C",
            name: "Agent",
            input: { description: "C", prompt: "Nested work" },
          },
        ],
      },
    });
    emit({
      type: "user",
      uuid: "child-user",
      agent_id: "B",
      message: { role: "user", content: "Child user prompt" },
    });
    assert.equal(publicTool(writes, "launch-C")?.source_message_uuid, "child-source");
    assert.equal(
      ((publicTool(writes, "launch-C")?.meta as Wire)?.claudeCode as Wire)?.parentToolUseId,
      "launch-B",
    );
    assert.equal(
      taskRows(writes, "C").length,
      0,
      "a description alone cannot identify a new child task",
    );
    emit({
      type: "assistant",
      uuid: "explicit-child-source",
      agent_id: "B",
      parent_tool_use_id: "explicit-parent",
      message: {
        id: "explicit-child",
        content: [{ type: "tool_use", id: "nested", name: "Bash", input: { command: "pwd" } }],
      },
    });
    assert.equal(publicTool(writes, "nested")?.source_message_uuid, "explicit-child-source");
    assert.equal(
      ((publicTool(writes, "nested")?.meta as Wire)?.claudeCode as Wire)?.parentToolUseId,
      "explicit-parent",
    );
    emit({
      type: "assistant",
      uuid: "root-source",
      message: {
        id: "root-response",
        role: "assistant",
        content: [{ type: "text", text: "Independent root answer" }],
      },
    });
    emit({
      type: "result",
      uuid: "root-result",
      subtype: "success",
      result: "Independent root answer",
    });
    const events = envelopes(writes, phase);
    assert.equal(
      events.some((event) => event.event === "auth_required" || event.event === "turn_error"),
      false,
    );
    assert.ok(!JSON.stringify(events).includes("Private child reply"));
    assert.ok(!JSON.stringify(events).includes("Child user prompt"));
    assert.ok(
      events.some(
        (event) => event.session_id === session.sessionId && event.event === "turn_complete",
      ),
    );
    assert.deepEqual(
      publicUpdates(writes, phase)
        .filter((update) => update.type === "agent_message_chunk")
        .map((update) => [update.source_message_uuid, update.content]),
      [["root-source", { type: "text", text: "Independent root answer" }]],
    );
  }));

test("stop admission refuses stale intent and a gated native stop keeps its dispatched run after a real newer start", async () =>
  taskScenario(async (session, emit, writes) => {
    emit({
      type: "system",
      subtype: "task_started",
      task_id: "worker",
      run_id: "B",
      task_type: "local_agent",
      description: "B running",
    });
    const calls: string[] = [];
    let release!: () => void, admitted!: () => void;
    const held = new Promise<void>((resolve) => {
        release = resolve;
      }),
      entered = new Promise<void>((resolve) => {
        admitted = resolve;
      });
    session.query = {
      stopTask: async (id: string) => {
        calls.push(id);
        admitted();
        await held;
      },
    } as unknown as SessionState["query"];
    sessions.set(session.sessionId, session);
    let pending: Promise<void> | undefined;
    try {
      for (const expected_run_id of ["A", undefined]) {
        const request = expected_run_id ? "stale-A" : "missing-run",
          phase = writes.length;
        await handleSubagentCommand(
          {
            command: "stop_subagent",
            session_id: session.sessionId,
            task_id: "worker",
            expected_run_id,
          },
          request,
        );
        const replies = envelopes(writes, phase).filter(
          (event) => event.event === "subagent_stop_result",
        );
        assert.equal(replies.length, 1);
        assert.equal(replies[0].request_id, request);
        assert.equal(replies[0].session_id, session.sessionId);
        assert.equal(replies[0].task_id, "worker");
        assert.equal(replies[0].run_id, expected_run_id);
        assert.ok(replies[0].error);
        assert.deepEqual(calls, []);
      }
      const phase = writes.length;
      pending = handleSubagentCommand(
        {
          command: "stop_subagent",
          session_id: session.sessionId,
          task_id: "worker",
          expected_run_id: "B",
        },
        "stop-B",
      );
      let timer: ReturnType<typeof setTimeout> | undefined;
      try {
        await Promise.race([
          entered,
          pending.then(() => {
            throw new Error("stop finished before the gated SDK call");
          }),
          new Promise<never>((_, reject) => {
            timer = setTimeout(() => reject(new Error("native stop admission deadline")), 5_000);
          }),
        ]);
      } finally {
        clearTimeout(timer);
      }
      assert.deepEqual(calls, ["worker"]);
      emit({
        type: "system",
        subtype: "task_started",
        task_id: "worker",
        run_id: "C",
        task_type: "local_agent",
        description: "C running",
      });
      release();
      await pending;
      const replies = envelopes(writes, phase).filter(
        (event) => event.event === "subagent_stop_result",
      );
      assert.deepEqual(replies, [
        {
          event: "subagent_stop_result",
          request_id: "stop-B",
          session_id: session.sessionId,
          task_id: "worker",
          run_id: "B",
        },
      ]);
      const rows = taskRows(writes, "worker", phase);
      assert.ok(rows.length > 0);
      for (const row of rows) {
        assert.equal(row.run_id, "C");
        assert.equal(row.status, "in_progress");
      }
      assert.equal(rows.at(-1)?.description, "C running");
    } finally {
      release();
      try {
        await pending;
      } finally {
        sessions.delete(session.sessionId);
      }
    }
  }));

function makeSession(): SessionState {
  return {
    sessionId: "session-1",
    cwd: scratch,
    availableModels: [],
    mode: null,
    supportedModeIds: [],
    runtimeUnavailableModeIds: [],
    supportsBypassPermissionsMode: false,
    fastModeState: "off",
    connected: true,
    closing: false,
    connectEvent: "connected",
    toolCalls: new Map(),
    tasksById: new Map(),
    taskOrder: [],
    taskToolUseIds: new Map(),
    taskIdsByToolUseId: new Map(),
    pendingPermissions: new Map(),
    pendingQuestions: new Map(),
    pendingUserDialogs: new Map(),
    pendingElicitations: new Map(),
    informationalDedupKeys: new Set(),
    knownConnectedMcpServers: new Set(),
    mcpStatusRevalidatedAt: new Map(),
    mcpAuthMonitors: new Map(),
    hiddenToolUseIds: new Set(),
    authHintSent: false,
  } as unknown as SessionState;
}

/** The text of every `agent_message_chunk` the bridge wrote while `run` ran. */
function replyChunks(run: () => void): Array<Record<string, unknown>> {
  const writes: string[] = [];
  const restore = replaceProtocolEventWriter((line) => {
    writes.push(line);
  });
  try {
    run();
  } finally {
    restore();
  }
  return writes
    .map((line) => JSON.parse(line) as Record<string, unknown>)
    .map((event) => event.update as Record<string, unknown> | undefined)
    .filter((update): update is Record<string, unknown> => update?.type === "agent_message_chunk");
}

// Measured on Claude Code 2.1.288, the version the pinned SDK bundles (2.1.296
// behaves the same): a local command (/rename, /color, /usage, /context)
// replies with one complete top-level assistant message and no stream events,
// and no system/local_command_output message, so its text exists only in that
// message.

test("a complete top-level reply whose response never streamed is shown once", () => {
  const session = makeSession();
  const reply = (uuid: string, responseId: string, text: string) => ({
    type: "assistant",
    uuid,
    session_id: "session-1",
    parent_tool_use_id: null,
    message: {
      id: responseId,
      role: "assistant",
      stop_reason: "end_turn",
      content: [{ type: "text", text }],
    },
  });

  const chunks = replyChunks(() => {
    for (const message of [
      reply("rename-reply", "a4cc84fe-7eb1-48bf-8fcd-0a11a7d5e251", "Session renamed to: probe"),
      reply("color-reply", "5142ce8e-bb1c-4d99-a882-5752dd24280b", "Session color set to: blue"),
    ]) {
      handleSdkMessage(session, message as unknown as SDKMessage);
    }
  });

  assert.deepEqual(chunks, [
    {
      type: "agent_message_chunk",
      content: { type: "text", text: "Session renamed to: probe" },
      source_message_uuid: "rename-reply",
    },
    {
      type: "agent_message_chunk",
      content: { type: "text", text: "Session color set to: blue" },
      source_message_uuid: "color-reply",
    },
  ]);
});

test("stream completions are suppressed while valid nonstreamed replies survive irrelevant invalid context markers", () =>
  taskScenario((session, emit, writes) => {
    const stream = (event: Wire) =>
      emit({ type: "stream_event", uuid: "stream-source", parent_tool_use_id: null, event });
    const complete = (uuid: string, message: Wire, extra: Wire = {}) =>
      emit({
        type: "assistant",
        uuid,
        parent_tool_use_id: null,
        message: { role: "assistant", ...message },
        ...extra,
      });
    stream({ type: "message_start", message: { id: "streamed-response" } });
    stream({ type: "content_block_start", index: 0, content_block: { type: "text", text: "" } });
    stream({
      type: "content_block_delta",
      index: 0,
      delta: { type: "text_delta", text: "already streamed" },
    });
    complete("block-completion", {
      id: "streamed-response",
      stop_reason: null,
      content: [{ type: "text", text: "already streamed" }],
    });
    stream({ type: "content_block_stop", index: 0 });
    stream({ type: "message_stop" });
    complete("final-completion", {
      id: "streamed-response",
      stop_reason: "end_turn",
      content: [{ type: "text", text: "already streamed" }],
    });
    complete(
      "child",
      { id: "child-response", content: [{ type: "text", text: "private child text" }] },
      { parent_tool_use_id: "child-parent" },
    );
    complete("blank", { id: "blank-response", content: [{ type: "text", text: "   " }] });
    complete(
      "error",
      { id: "error-response", content: [{ type: "text", text: "API error text" }] },
      { error: "rate_limit" },
    );
    complete("ordinary-no-id", { content: [{ type: "text", text: "uncorrelated ordinary text" }] });
    for (const [index, marker] of ["invalid", [], null, 3].entries())
      complete(
        `invalid-context-${index}`,
        { content: [{ type: "text", text: "not context Markdown" }] },
        { context_usage: marker },
      );
    complete("empty-context", { content: [{ type: "text", text: "   " }] }, { context_usage: {} });
    complete(
      "valid-local-source",
      {
        id: "normal-local-response",
        stop_reason: "end_turn",
        content: [{ type: "text", text: "Valid local reply" }],
      },
      { context_usage: "irrelevant invalid metadata" },
    );
    const chunks = publicUpdates(writes).filter((update) => update.type === "agent_message_chunk");
    assert.deepEqual(
      chunks.map((update) => [update.source_message_uuid, update.content]),
      [
        ["stream-source", { type: "text", text: "already streamed" }],
        ["valid-local-source", { type: "text", text: "Valid local reply" }],
      ],
    );
    assert.ok(
      envelopes(writes)
        .filter((event) => (event.update as Wire)?.type === "agent_message_chunk")
        .every((event) => event.session_id === session.sessionId),
    );
  }));

test("a non-streaming fallback after a partial stream is shown in full", () => {
  const session = makeSession();
  const emit = (message: Record<string, unknown>) =>
    handleSdkMessage(session, {
      session_id: "session-1",
      parent_tool_use_id: null,
      ...message,
    } as unknown as SDKMessage);
  const stream = (event: Record<string, unknown>) => emit({ type: "stream_event", uuid: "frame", event });

  const texts = replyChunks(() => {
    // The stream breaks off after a partial delta; the deltas already showed it.
    stream({ type: "message_start", message: { id: "msg_partial" } });
    stream({ type: "content_block_start", index: 0, content_block: { type: "text", text: "" } });
    stream({ type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "partial" } });
    // The retried request answers without streaming, under a new response ID.
    emit({
      type: "assistant",
      uuid: "fallback",
      message: { id: "msg_fallback", role: "assistant", stop_reason: "end_turn", content: [{ type: "text", text: "the full reply" }] },
    });
  }).map((chunk) => (chunk.content as Record<string, unknown>).text);

  assert.deepEqual(texts, ["partial", "the full reply"]);
});

// A foreground Agent call can finish through its tool result before the task
// notification arrives. The finished tool call then rejects the notification,
// which must still settle the separate task row the footer and /agents count.
test("task outcomes after concrete tool completion publish their own cause without rewriting the finished output", async () => {
  for (const outcome of ["completed", "failed", "stopped"])
    await taskScenario((session, emit, writes) => {
      invoke(emit, "finished-tool", "Agent", { description: "Work", prompt: "Perform work" });
      emit({
        type: "system",
        subtype: "task_started",
        task_id: "agent-1",
        tool_use_id: "finished-tool",
        task_type: "local_agent",
        description: "Work",
      });
      emit({
        type: "user",
        message: {
          role: "user",
          content: [
            {
              type: "tool_result",
              tool_use_id: "finished-tool",
              content: "Concrete foreground output",
            },
          ],
        },
      });
      assert.equal(publicTool(writes, "finished-tool")?.status, "completed");
      assert.equal(publicTool(writes, "finished-tool")?.raw_output, "Concrete foreground output");
      const phase = writes.length;
      emit({
        type: "system",
        subtype: "task_notification",
        task_id: "agent-1",
        tool_use_id: "finished-tool",
        status: outcome,
        summary: `Actual ${outcome} report`,
        output_file: "",
      });
      emit({
        type: "system",
        subtype: "task_progress",
        task_id: "agent-1",
        tool_use_id: "finished-tool",
        description: "Stale progress",
        usage: { duration_ms: 999 },
      });
      const rows = taskRows(writes, "agent-1", phase);
      assert.ok(rows.length > 0);
      for (const row of rows) {
        assert.equal(row.status, "completed");
        assert.equal((row.metadata as Wire)?.terminal_status, outcome);
        assert.equal((row.metadata as Wire)?.summary, `Actual ${outcome} report`);
        assert.notEqual(row.description, "Stale progress");
      }
      for (const fields of toolDeltas(writes, "finished-tool", phase)) {
        assert.ok(fields.status === undefined || fields.status === "completed");
        assert.ok(
          fields.raw_output === undefined || fields.raw_output === "Concrete foreground output",
        );
      }
      assert.equal(publicTool(writes, "finished-tool")?.status, "completed");
      assert.equal(publicTool(writes, "finished-tool")?.raw_output, "Concrete foreground output");
      assert.ok(envelopes(writes, phase).every((event) => event.session_id === session.sessionId));
    });
});

// Measured on Claude Code 2.1.288: a backgrounded subagent is listed by
// background_tasks_changed, and the snapshot that drops it arrives right after
// its task_notification (or after its killed patch when stopped).



test("background snapshots publish owned removals and inferred interruption without overriding real lifecycle outcomes", async () => {
  for (const route of ["already-terminal", "progress", "listing", "direct-outcome"])
    await taskScenario((session, emit, writes) => {
      const edge = (subtype: string, fields: Wire = {}) =>
        emit({ type: "system", subtype, task_id: "worker", ...fields });
      const listed = [
        { task_id: "worker", task_type: "local_agent", description: "Background worker" },
        { task_id: "inventory-only", task_type: "local_bash", description: "Inventory peer" },
      ];
      edge("task_started", { task_type: "local_agent", description: "Background worker" });
      emit({
        type: "system",
        subtype: "task_started",
        task_id: "foreground",
        task_type: "local_agent",
        description: "Never listed foreground",
      });
      emit({ type: "system", subtype: "background_tasks_changed", tasks: listed });
      const expected = route === "progress" || route === "listing" ? "failed" : "completed",
        summary = `${route} actual outcome`;
      if (route === "already-terminal") edge("task_notification", { status: expected, summary });
      let phase = writes.length;
      emit({ type: "system", subtype: "background_tasks_changed", tasks: [] });
      const removal = publicUpdates(writes, phase)
        .filter((update) => update.type === "task_state_update")
        .flatMap((update) => (update.removed_task_ids as string[]) ?? []);
      assert.ok(removal.includes("inventory-only"));
      assert.ok(!removal.includes("worker"));
      assert.ok(!removal.includes("foreground"));
      const disappeared =
        taskRows(writes, "worker", phase).at(-1) ?? taskRows(writes, "worker").at(-1);
      assert.equal(disappeared?.status, "completed");
      assert.equal(
        (disappeared?.metadata as Wire)?.terminal_status,
        route === "already-terminal" ? "completed" : "interrupted",
      );
      assert.equal(taskRows(writes, "foreground").at(-1)?.status, "in_progress");
      if (route === "progress" || route === "listing") {
        phase = writes.length;
        if (route === "progress")
          edge("task_progress", { description: "Live progress resumes worker" });
        else emit({ type: "system", subtype: "background_tasks_changed", tasks: [listed[0]] });
        const revived = taskRows(writes, "worker", phase).at(-1);
        assert.equal(revived?.status, "in_progress");
        assert.equal((revived?.metadata as Wire)?.terminal_status, undefined);
      }
      if (route !== "already-terminal") edge("task_notification", { status: expected, summary });
      phase = writes.length;
      emit({ type: "system", subtype: "background_tasks_changed", tasks: [listed[0]] });
      edge("task_progress", { description: "Late progress must not replace report" });
      const final = taskRows(writes, "worker").at(-1);
      assert.equal(final?.status, "completed");
      assert.equal((final?.metadata as Wire)?.terminal_status, expected);
      assert.equal((final?.metadata as Wire)?.summary, summary);
      for (const row of taskRows(writes, "worker", phase)) {
        assert.equal(row.status, "completed");
        assert.equal((row.metadata as Wire)?.summary, summary);
        assert.equal((row.metadata as Wire)?.terminal_status, expected);
      }
      assert.equal(taskRows(writes, "foreground").at(-1)?.status, "in_progress");
      assert.ok(envelopes(writes).every((event) => event.session_id === session.sessionId));
    });
});











/** Every `system_notice_update` the bridge wrote while `run` ran. */


function hookResponse(
  hookEvent: string,
  fields: Record<string, unknown>,
  hookId = "hook-1",
): SDKMessage {
  return {
    type: "system",
    subtype: "hook_response",
    hook_id: hookId,
    hook_name: `${hookEvent}:Bash`,
    hook_event: hookEvent,
    output: "",
    stdout: "",
    stderr: "",
    session_id: "session-1",
    uuid: `uuid-${hookId}`,
    ...fields,
  } as unknown as SDKMessage;
}

// Measured on SDK 0.3.288 with a PreToolUse command hook that exits 1: the
// tool still runs and nothing else reports the failure.
type CapturedLog = { level: string; eventName: string; sessionId?: string; fields?: Wire; message?: string };
async function loggedScenario(run: (records: CapturedLog[]) => void | Promise<void>): Promise<void> {
  const records: CapturedLog[] = [], originals = { debug: bridgeLogger.debug, info: bridgeLogger.info, warn: bridgeLogger.warn, error: bridgeLogger.error };
  for (const level of ["debug", "info", "warn", "error"] as const) bridgeLogger[level] = event => { records.push({ ...event, level }); };
  try { await run(records);await settleSdkReads(); } finally { Object.assign(bridgeLogger, originals); }
}
const notices = (writes: string[], start = 0): Wire[] => envelopes(writes, start).filter(event => (event.update as Wire)?.type === "system_notice_update");

test("hook warnings deduplicate one incident while distinct hooks and sessions remain visible beside quiet controls", () =>
  taskScenario(async (session, emit, writes) =>
    loggedScenario((records) => {
      for (const subtype of ["hook_started", "hook_progress"])
        emit({
          type: "system",
          subtype,
          hook_id: subtype,
          hook_name: "PreToolUse:quiet",
          hook_event: "PreToolUse",
        });
      handleSdkMessage(
        session,
        hookResponse("PreToolUse", { outcome: "success", exit_code: 0, output: "ok" }, "success"),
      );
      handleSdkMessage(session, hookResponse("Stop", { outcome: "cancelled" }, "cancelled"));
      assert.deepEqual(notices(writes), []);
      const failure = hookResponse(
        "PreToolUse",
        {
          hook_name: "PreToolUse:shared",
          outcome: "error",
          exit_code: 1,
          stderr: "shared diagnostic cause",
        },
        "incident-one",
      );
      handleSdkMessage(session, failure);
      handleSdkMessage(session, failure);
      assert.equal(notices(writes).length, 1);
      handleSdkMessage(
        session,
        hookResponse(
          "PreToolUse",
          {
            hook_name: "PreToolUse:shared",
            outcome: "error",
            exit_code: 1,
            stderr: "shared diagnostic cause",
          },
          "incident-two",
        ),
      );
      const other = makeSession();
      other.sessionId = "other-session";
      handleSdkMessage(
        other,
        hookResponse(
          "PreToolUse",
          {
            session_id: other.sessionId,
            hook_name: "PreToolUse:shared",
            outcome: "error",
            exit_code: 1,
            stderr: "shared diagnostic cause",
          },
          "incident-one",
        ),
      );
      const delivered = notices(writes);
      assert.equal(delivered.filter((event) => event.session_id === session.sessionId).length, 2);
      assert.equal(delivered.filter((event) => event.session_id === other.sessionId).length, 1);
      for (const event of delivered) {
        const update = event.update as Wire;
        assert.equal(update.severity, "warning");
        assert.match(String(update.message), /PreToolUse:shared/);
        assert.match(String(update.message), /exit 1/);
        assert.ok(String(update.message).includes("shared diagnostic cause"));
      }
      for (const [owner, id] of [
        [session.sessionId, "incident-one"],
        [session.sessionId, "incident-two"],
        [other.sessionId, "incident-one"],
      ])
        assert.ok(
          records.some(
            (record) =>
              record.level === "warn" &&
              record.sessionId === owner &&
              record.fields?.hook_id === id &&
              record.fields?.exit_code === 1,
          ),
        );
    }),
  ));



/** The level and event name of every info, warning and error record logged while `run` ran. */


/** Log records and notices for one exit-2 `hook_response` per event, in order. */


// The SDK reports every exit 2 as outcome "error". On these events the hook
// reference makes it the hook's decision (block the tool call, the prompt or
// the compaction, keep Claude working, or hand the output to Claude), so a gate
// must not read "failed" each turn. Measured on SDK 0.3.288, the SDK already
// shows the blocked tool call, prompt, task, compaction or model switch; Stop
// and SubagentStop keep Claude working, and the rest could not be measured, so
// none of them gets a notice that could duplicate what the user already sees.

test("independently named exit-two hook events retain identity, reporting severity and measured notice policy", async () => {
  // Supplied SDK frames exercise host reporting; this table does not execute hooks.
  const rows: Array<{ event: string; level: string; notice?: string }> = [
    ...[
      "PreToolUse",
      "UserPromptSubmit",
      "UserPromptExpansion",
      "Stop",
      "SubagentStop",
      "TeammateIdle",
      "TaskCreated",
      "TaskCompleted",
      "ConfigChange",
      "PreCompact",
      "PreModelSwitch",
      "Elicitation",
      "ElicitationResult",
    ].map((event) => ({ event, level: "info" })),
    ...["PostToolUse", "PostToolUseFailure", "PostToolBatch"].map((event) => ({
      event,
      level: "info",
      notice: "info",
    })),
    ...["Notification", "PermissionRequest", "WorktreeCreate", "SomeFutureEvent"].map((event) => ({
      event,
      level: "warn",
      notice: "warning",
    })),
  ];
  for (const row of rows)
    await taskScenario(async (session, _emit, writes) =>
      loggedScenario((records) => {
        const id = `exit-two-${row.event}`,
          name = `${row.event}:selected-gate`,
          cause = `${row.event} independent decision diagnostic`;
        handleSdkMessage(
          session,
          hookResponse(
            row.event,
            { hook_name: name, outcome: "error", exit_code: 2, stderr: cause },
            id,
          ),
        );
        const diagnostic = records.filter(
          (record) =>
            record.fields?.hook_id === id &&
            (record.eventName === "sdk_hook_decision" || record.eventName === "sdk_hook_failed"),
        );
        assert.equal(diagnostic.length, 1);
        assert.equal(diagnostic[0].level, row.level);
        assert.equal(diagnostic[0].sessionId, session.sessionId);
        assert.equal(diagnostic[0].fields?.hook_name, name);
        assert.equal(diagnostic[0].fields?.hook_event, row.event);
        assert.equal(diagnostic[0].fields?.exit_code, 2);
        assert.equal(diagnostic[0].fields?.output_excerpt, cause);
        const delivered = notices(writes);
        assert.equal(delivered.length, row.notice ? 1 : 0, row.event);
        if (row.notice) {
          assert.equal(delivered[0].session_id, session.sessionId);
          const update = delivered[0].update as Wire;
          assert.equal(update.severity, row.notice);
          assert.ok(String(update.message).includes(name));
          assert.ok(String(update.message).includes(cause));
        }
      }),
    );
});

// Measured on SDK 0.3.288: after these hooks exit 2 only the hook_response
// carries the output (the tool result is the tool's own, or the turn ends with
// an empty result), so one info notice is the only place the user sees it.



// Measured on SDK 0.3.288 with a SessionStart command hook that exits 2: the
// session starts and the SDK sends nothing but this hook_response, so the
// failure notice is the only place the user sees the hook's error.


// Notification and PermissionRequest ignore exit 2, WorktreeCreate fails on any
// non-zero exit, and an event the hook reference does not list (one a newer SDK
// adds) has no documented decision: all of them stay failures.




test("targeted hook diagnostics select a useful fallback and bound Unicode output without credential disclosure", async () => {
  const oversized = `deploy failed api_key=sk-ant-verysecret ${"\ud83d\ude00".repeat(500)} TAIL-MUST-BE-CLIPPED`;
  const rows = [
    {
      stderr: oversized,
      output: "wrong-output-source",
      stdout: "wrong-stdout-source",
      expected: "deploy failed",
    },
    {
      stderr: " \n ",
      output: "output failed api_key=sk-ant-verysecret",
      stdout: "wrong-stdout-source",
      expected: "output failed",
    },
    {
      stderr: " ",
      output: " ",
      stdout: "stdout failed api_key=sk-ant-verysecret",
      expected: "stdout failed",
    },
  ];
  for (const [index, row] of rows.entries())
    await taskScenario(async (session, _emit, writes) =>
      loggedScenario((records) => {
        const id = `privacy-${index}`;
        handleSdkMessage(
          session,
          hookResponse(
            "PostToolUse",
            {
              hook_name: `PostToolUse:diagnostic-${index}`,
              outcome: "error",
              exit_code: 1,
              stderr: row.stderr,
              output: row.output,
              stdout: row.stdout,
            },
            id,
          ),
        );
        const delivered = notices(writes);
        assert.equal(delivered.length, 1);
        assert.equal(delivered[0].session_id, session.sessionId);
        const notice = delivered[0].update as Wire;
        assert.equal(notice.severity, "warning");
        assert.ok(String(notice.message).includes(row.expected));
        assert.ok(String(notice.message).includes("api_key=[redacted]"));
        const diagnostic = records.find(
          (record) => record.eventName === "sdk_hook_failed" && record.fields?.hook_id === id,
        );
        assert.ok(diagnostic);
        assert.equal(diagnostic.level, "warn");
        const excerpt = String(diagnostic.fields?.output_excerpt);
        assert.ok(excerpt.startsWith(row.expected));
        assert.ok(excerpt.includes("api_key=[redacted]"));
        assert.ok(
          Array.from(excerpt).length <= 240,
          "the hook diagnostic has a 240-code-point budget",
        );
        assert.equal(
          Buffer.from(excerpt, "utf8").toString("utf8"),
          excerpt,
          "truncation cannot leave half a surrogate pair",
        );
        assert.ok(String(notice.message).includes(excerpt));
        const captured = JSON.stringify({ delivered, records });
        for (const secret of [
          "sk-ant-verysecret",
          "wrong-output-source",
          "wrong-stdout-source",
          "TAIL-MUST-BE-CLIPPED",
        ])
          assert.ok(!captured.includes(secret), secret);
        if (index === 0) assert.ok(excerpt.includes("\ud83d\ude00"));
      }),
    );
});

// SessionStart hooks finish before the connect event, and the app ignores
// updates for a session it has not been told about.



// Connection awaits the controlled SDK title read before restoring the writer.
test("real pre-connect hook failures publish only after connection and do not redeliver their original incidents", () =>
  taskScenario(async (session, _emit, writes) =>
    loggedScenario(async (records) => {
      session.connected = false;
      const failures = [1, 2].map((exit_code) =>
        hookResponse(
          "SessionStart",
          {
            hook_name: `SessionStart:startup-${exit_code}`,
            outcome: "error",
            exit_code,
            stderr: `startup diagnostic ${exit_code}`,
          },
          `startup-${exit_code}`,
        ),
      );
      for (const failure of failures) handleSdkMessage(session, failure);
      assert.deepEqual(notices(writes), []);
      for (const exit of [1, 2])
        assert.ok(
          records.some(
            (record) =>
              record.level === "warn" &&
              record.sessionId === session.sessionId &&
              record.fields?.hook_id === `startup-${exit}` &&
              record.fields?.exit_code === exit,
          ),
        );
      await emitConnectEvent(session);
      const events = envelopes(writes),
        connected = events.findIndex(
          (event) => event.event === "connected" && event.session_id === session.sessionId,
        ),
        delivered = events.filter(
          (event) => (event.update as Wire)?.type === "system_notice_update",
        );
      assert.ok(connected >= 0);
      assert.equal(delivered.length, 2);
      for (const exit of [1, 2]) {
        const event = delivered.find((event) =>
          String((event.update as Wire).message).includes(`startup diagnostic ${exit}`),
        );
        assert.ok(event);
        assert.equal(event.session_id, session.sessionId);
        assert.equal((event.update as Wire).severity, "warning");
        assert.ok(
          events.indexOf(event) > connected,
          "the actual connection envelope precedes each queued failure",
        );
      }
      const phase = writes.length;
      for (const failure of failures) handleSdkMessage(session, failure);
      assert.deepEqual(notices(writes, phase), []);
      handleSdkMessage(
        session,
        hookResponse(
          "PreToolUse",
          {
            hook_name: "PreToolUse:fresh-gate",
            outcome: "error",
            exit_code: 1,
            stderr: "fresh diagnostic after connect",
          },
          "fresh-incident",
        ),
      );
      const fresh = notices(writes, phase);
      assert.equal(fresh.length, 1);
      assert.equal(fresh[0].session_id, session.sessionId);
      assert.ok(
        String((fresh[0].update as Wire).message).includes("fresh diagnostic after connect"),
      );
    }),
  ));
