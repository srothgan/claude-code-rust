import assert from "node:assert/strict";
import { mkdtempSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import type { SDKMessage } from "@anthropic-ai/claude-agent-sdk";
import { emitConnectEvent, replaceProtocolEventWriter } from "./events.js";
import { bridgeLogger } from "./logger.js";
import { handleSdkMessage } from "./message_handlers.js";
import type { SessionState } from "./session_lifecycle.js";

function makeSession(): SessionState {
  return {
    sessionId: "session-1",
    cwd: "C:/work",
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

test("a streamed, subagent, empty, error or uncorrelated reply is never shown again", () => {
  const session = makeSession();
  const emit = (message: Record<string, unknown>) =>
    handleSdkMessage(session, {
      session_id: "session-1",
      parent_tool_use_id: null,
      ...message,
    } as unknown as SDKMessage);
  const stream = (event: Record<string, unknown>) => emit({ type: "stream_event", uuid: "frame", event });
  const complete = (uuid: string, message: Record<string, unknown>, extra = {}) =>
    emit({ type: "assistant", uuid, message: { role: "assistant", ...message }, ...extra });

  const texts = replyChunks(() => {
    stream({ type: "message_start", message: { id: "msg_streamed" } });
    stream({ type: "content_block_start", index: 0, content_block: { type: "text", text: "" } });
    stream({ type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "already streamed" } });
    // The per-block message arrives inside the response; a terminal one may
    // follow its stop. Neither is the reply's first appearance.
    complete("streamed-block", { id: "msg_streamed", stop_reason: null, content: [{ type: "text", text: "already streamed" }] });
    stream({ type: "content_block_stop", index: 0 });
    stream({ type: "message_stop" });
    complete("streamed-final", { id: "msg_streamed", stop_reason: "end_turn", content: [{ type: "text", text: "already streamed" }] });
    complete("subagent", { id: "msg_subagent", content: [{ type: "text", text: "subagent text" }] }, { parent_tool_use_id: "tool-1" });
    complete("empty", { id: "local-empty", content: [{ type: "text", text: "   " }] });
    // A synthetic API error is reported through the turn result.
    complete("synthetic-error", { id: "local-error", content: [{ type: "text", text: "API Error: rate limited" }] }, { error: "rate_limit" });
    // Without a response ID nothing proves the text was not streamed.
    complete("no-response-id", { content: [{ type: "text", text: "uncorrelated" }] });
  }).map((chunk) => (chunk.content as Record<string, unknown>).text);

  assert.deepEqual(texts, ["already streamed"]);
});

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

/** The last `task_state_update` row the bridge wrote for `taskId` while `run` ran. */
function lastTaskRow(taskId: string, run: () => void): Record<string, unknown> | undefined {
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
    .map((line) => (JSON.parse(line) as { update?: { type?: string; tasks?: Array<Record<string, unknown>> } }).update)
    .filter((update) => update?.type === "task_state_update")
    .flatMap((update) => update?.tasks ?? [])
    .filter((task) => task.task_id === taskId)
    .at(-1);
}

// A foreground Agent call can finish through its tool result before the task
// notification arrives. The finished tool call then rejects the notification,
// which must still settle the separate task row the footer and /agents count.
test("a subagent task row settles when its notification follows the finished tool call", () => {
  for (const outcome of ["completed", "failed", "stopped"]) {
    const session = makeSession();
    const emit = (message: Record<string, unknown>) =>
      handleSdkMessage(session, {
        session_id: "session-1",
        parent_tool_use_id: null,
        uuid: crypto.randomUUID(),
        ...message,
      } as unknown as SDKMessage);
    const system = (subtype: string, fields: Record<string, unknown>) =>
      emit({ type: "system", subtype, task_id: "agent-1", ...fields });

    const row = lastTaskRow("agent-1", () => {
      emit({ type: "assistant", message: { id: "response-1", role: "assistant", content: [{ type: "tool_use", id: "tool-1", name: "Agent", input: { description: "Sleep", prompt: "Sleep" } }] } });
      system("task_started", { tool_use_id: "tool-1", task_type: "local_agent", subagent_type: "general-purpose", description: "Sleep" });
      if (outcome === "stopped") system("task_updated", { patch: { status: "killed", end_time: 1 } });
      else emit({ type: "user", message: { role: "user", content: [{ type: "tool_result", tool_use_id: "tool-1", content: "ok" }] } });
      system("task_notification", { tool_use_id: "tool-1", status: outcome, summary: "Slept", output_file: "" });
      // A queued progress frame must not revive the settled row.
      system("task_progress", { tool_use_id: "tool-1", description: "Stale progress", usage: { total_tokens: 1, tool_uses: 1, duration_ms: 1 } });
    });

    assert.ok(row, `${outcome}: the task row was emitted`);
    assert.equal(row.status, "completed", `${outcome}: the row is settled`);
    const metadata = row.metadata as Record<string, unknown>;
    assert.equal(metadata.terminal_status, outcome, `${outcome}: the notification's outcome is kept`);
    assert.equal(metadata.summary, "Slept");
    assert.notEqual(row.description, "Stale progress", `${outcome}: stale progress does not rewrite the row`);
  }
});

// Measured on Claude Code 2.1.288: a backgrounded subagent is listed by
// background_tasks_changed, and the snapshot that drops it arrives right after
// its task_notification (or after its killed patch when stopped).
test("a backgrounded subagent keeps its lifecycle row after it leaves the background snapshot", () => {
  const session = makeSession();
  const emit = (message: Record<string, unknown>) =>
    handleSdkMessage(session, { session_id: "session-1", uuid: crypto.randomUUID(), ...message } as unknown as SDKMessage);
  const background = (tasks: Array<Record<string, unknown>>) => emit({ type: "system", subtype: "background_tasks_changed", tasks });
  const listed = [
    { task_id: "agent-1", task_type: "local_agent", description: "Background sleep" },
    { task_id: "shell-1", task_type: "local_bash", description: "Watch logs" },
  ];
  const writes: string[] = [];
  const restore = replaceProtocolEventWriter((line) => {
    writes.push(line);
  });
  try {
    background(listed);
    emit({ type: "system", subtype: "task_started", task_id: "agent-1", task_type: "local_agent", subagent_type: "general-purpose", description: "Background sleep" });
    background(listed);
    emit({ type: "system", subtype: "task_notification", task_id: "agent-1", status: "completed", summary: "ok", output_file: "" });
    background([]);
  } finally {
    restore();
  }

  type Update = { source?: string; removed_task_ids?: string[]; tasks?: Array<Record<string, unknown>> };
  const updates = writes
    .map((line) => (JSON.parse(line) as { update?: Update & { type?: string } }).update)
    .filter((update): update is Update & { type: string } => update?.type === "task_state_update");
  const final = updates.at(-1);
  assert.equal(final?.source, "background_tasks");
  assert.deepEqual(final?.removed_task_ids, ["shell-1"], "a task with no reported outcome still leaves with the snapshot");
  const row = updates.flatMap((update) => update.tasks ?? []).filter((task) => task.task_id === "agent-1").at(-1);
  assert.equal(row?.status, "completed");
  assert.deepEqual(row?.metadata, {
    task_type: "local_agent",
    subagent_type: "general-purpose",
    sdk_lifecycle_task: true,
    is_backgrounded: true,
    output_file: "",
    summary: "ok",
    terminal_status: "completed",
  }, "the settled row keeps its lifecycle metadata and drops only the background marker");
  assert.equal(session.tasksById.get("agent-1")?.status, "completed");
  assert.equal(session.tasksById.has("shell-1"), false);
});

test("a subagent that leaves the background snapshot before its outcome stops counting as running, then takes its real outcome", () => {
  const session = makeSession();
  const emit = (message: Record<string, unknown>) =>
    handleSdkMessage(session, { session_id: "session-1", uuid: crypto.randomUUID(), ...message } as unknown as SDKMessage);
  const writes: string[] = [];
  const restore = replaceProtocolEventWriter((line) => { writes.push(line); });
  try {
    emit({ type: "system", subtype: "task_started", task_id: "agent-1", task_type: "local_agent", description: "Background sleep" });
    emit({ type: "system", subtype: "background_tasks_changed", tasks: [
      { task_id: "agent-1", task_type: "local_agent", description: "Background sleep" },
      { task_id: "snapshot-only", task_type: "local_agent", description: "No lifecycle start" },
    ] });
    assert.equal(session.tasksById.get("agent-1")?.status, "in_progress", "listed rows keep running");
    emit({ type: "system", subtype: "background_tasks_changed", tasks: [] });
    const unlisted = session.tasksById.get("agent-1");
    assert.equal(unlisted?.status, "completed", "a running row the snapshot no longer lists is not running");
    assert.deepEqual(unlisted?.metadata, { task_type: "local_agent", sdk_lifecycle_task: true, is_backgrounded: true, terminal_status: "interrupted" }, "no outcome is invented");
    assert.equal(session.tasksById.has("snapshot-only"), false, "a background-only row leaves with the snapshot");
    emit({ type: "system", subtype: "task_notification", task_id: "agent-1", status: "completed", summary: "ok" });
  } finally {
    restore();
  }

  const updates = writes.map(line => (JSON.parse(line) as { update?: { type?: string; removed_task_ids?: string[]; tasks?: Array<Record<string, unknown>> } }).update)
    .filter(update => update?.type === "task_state_update");
  assert.ok(!updates.some(update => update?.removed_task_ids?.includes("agent-1")));
  const final = updates.flatMap(update => update?.tasks ?? []).filter(task => task.task_id === "agent-1").at(-1);
  assert.equal(final?.status, "completed");
  assert.deepEqual(final?.metadata, { task_type: "local_agent", sdk_lifecycle_task: true, is_backgrounded: true, summary: "ok", terminal_status: "completed" });
});

function terminalStatus(metadata: unknown): unknown {
  return (metadata as Record<string, unknown> | undefined)?.terminal_status;
}

test("a relisted interrupted subagent runs again and its real outcome still wins", () => {
  const session = makeSession();
  const emit = (message: Record<string, unknown>) =>
    handleSdkMessage(session, { session_id: "session-1", uuid: crypto.randomUUID(), ...message } as unknown as SDKMessage);
  const restore = replaceProtocolEventWriter(() => {});
  try {
    emit({ type: "system", subtype: "task_started", task_id: "agent-1", task_type: "local_agent", description: "Background sleep" });
    emit({ type: "system", subtype: "background_tasks_changed", tasks: [{ task_id: "agent-1", task_type: "local_agent" }] });
    emit({ type: "system", subtype: "background_tasks_changed", tasks: [] });
    assert.equal(session.tasksById.get("agent-1")?.status, "completed", "dropped: settled as interrupted");
    emit({ type: "system", subtype: "task_progress", task_id: "agent-1", summary: "still going" });
    assert.equal(session.tasksById.get("agent-1")?.status, "in_progress", "progress revives an interrupted row");
    emit({ type: "system", subtype: "background_tasks_changed", tasks: [] });
    assert.equal(session.tasksById.get("agent-1")?.status, "completed", "dropped again");
    emit({ type: "system", subtype: "background_tasks_changed", tasks: [{ task_id: "agent-1", task_type: "local_agent" }] });
    const relisted = session.tasksById.get("agent-1");
    assert.equal(relisted?.status, "in_progress", "a relisted row is live");
    assert.equal(terminalStatus(relisted?.metadata), undefined, "the inferred status is cleared");
    emit({ type: "system", subtype: "task_notification", task_id: "agent-1", status: "failed", summary: "boom" });
    const final = session.tasksById.get("agent-1");
    assert.equal(final?.status, "completed", "the wire has no failed status");
    assert.equal(terminalStatus(final?.metadata), "failed");
  } finally {
    restore();
  }
});

test("a subagent with a recorded outcome is not revived by a relisting or later progress", () => {
  const session = makeSession();
  const emit = (message: Record<string, unknown>) =>
    handleSdkMessage(session, { session_id: "session-1", uuid: crypto.randomUUID(), ...message } as unknown as SDKMessage);
  const restore = replaceProtocolEventWriter(() => {});
  try {
    emit({ type: "system", subtype: "task_started", task_id: "agent-1", task_type: "local_agent", description: "Background sleep" });
    emit({ type: "system", subtype: "task_notification", task_id: "agent-1", status: "completed", summary: "ok" });
    emit({ type: "system", subtype: "background_tasks_changed", tasks: [{ task_id: "agent-1", task_type: "local_agent" }] });
    emit({ type: "system", subtype: "task_progress", task_id: "agent-1", summary: "late" });
    const row = session.tasksById.get("agent-1");
    assert.equal(row?.status, "completed");
    assert.equal(terminalStatus(row?.metadata), "completed");
  } finally {
    restore();
  }
});

test("a foreground subagent the snapshot never listed keeps running when a snapshot arrives", () => {
  const session = makeSession();
  const emit = (message: Record<string, unknown>) =>
    handleSdkMessage(session, { session_id: "session-1", uuid: crypto.randomUUID(), ...message } as unknown as SDKMessage);
  const restore = replaceProtocolEventWriter(() => {});
  try {
    emit({ type: "system", subtype: "task_started", task_id: "agent-fg", task_type: "local_agent", description: "Foreground" });
    emit({ type: "system", subtype: "background_tasks_changed", tasks: [] });
  } finally {
    restore();
  }
  assert.equal(session.tasksById.get("agent-fg")?.status, "in_progress");
});

test("SDK context usage Markdown is shown even without a response ID", () => {
  const session = makeSession();
  const texts = replyChunks(() => {
    handleSdkMessage(session, {
      type: "assistant",
      uuid: "context-reply",
      session_id: "session-1",
      parent_tool_use_id: null,
      context_usage: { categories: [] },
      message: { role: "assistant", content: [{ type: "text", text: "## Context Usage" }] },
    } as unknown as SDKMessage);
  }).map((chunk) => (chunk.content as Record<string, unknown>).text);

  assert.deepEqual(texts, ["## Context Usage"]);
});

/** Every `system_notice_update` the bridge wrote while `run` ran. */
function noticesWritten(run: () => void): Array<{ severity: unknown; message: unknown }> {
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
    .map((line) => (JSON.parse(line) as Record<string, unknown>).update as Record<string, unknown> | undefined)
    .filter((update): update is Record<string, unknown> => update?.type === "system_notice_update")
    .map((update) => ({ severity: update.severity, message: update.message }));
}

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
test("a failed hook with no other feedback is one warning, once", () => {
  const session = makeSession();
  const failed = hookResponse("PreToolUse", {
    outcome: "error",
    exit_code: 1,
    stderr: "lint gate: forbidden pattern found\n",
  });
  const notices = noticesWritten(() => {
    handleSdkMessage(session, failed);
    handleSdkMessage(session, failed);
  });

  assert.deepEqual(notices, [
    {
      severity: "warning",
      message: "Hook PreToolUse:Bash failed (exit 1): lint gate: forbidden pattern found",
    },
  ]);
});

type LoggedRecord = { level: string; eventName: string };

/** The level and event name of every info, warning and error record logged while `run` ran. */
function recordsLogged(run: () => void): LoggedRecord[] {
  const logged: LoggedRecord[] = [];
  const originals = { info: bridgeLogger.info, warn: bridgeLogger.warn, error: bridgeLogger.error };
  for (const level of ["info", "warn", "error"] as const) {
    bridgeLogger[level] = (event) => {
      logged.push({ level, eventName: event.eventName });
    };
  }
  try {
    run();
  } finally {
    Object.assign(bridgeLogger, originals);
  }
  return logged;
}

/** Log records and notices for one exit-2 `hook_response` per event, in order. */
function exitTwoOn(
  session: SessionState,
  events: string[],
): { logged: LoggedRecord[]; notices: ReturnType<typeof noticesWritten> } {
  let notices: ReturnType<typeof noticesWritten> = [];
  const logged = recordsLogged(() => {
    notices = noticesWritten(() => {
      events.forEach((event, index) => {
        handleSdkMessage(
          session,
          hookResponse(event, { outcome: "error", exit_code: 2, stderr: "gate says no" }, `exit-2-${index}`),
        );
      });
    });
  });
  return { logged, notices };
}

// The SDK reports every exit 2 as outcome "error". On these events the hook
// reference makes it the hook's decision (block the tool call, the prompt or
// the compaction, keep Claude working, or hand the output to Claude), so a gate
// must not read "failed" each turn. Measured on SDK 0.3.288, the SDK already
// shows the blocked tool call, prompt, task, compaction or model switch; Stop
// and SubagentStop keep Claude working, and the rest could not be measured, so
// none of them gets a notice that could duplicate what the user already sees.
test("a hook that exits 2 where exit 2 is a decision is logged at info, with no notice", () => {
  const events = [
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
  ];
  const { logged, notices } = exitTwoOn(makeSession(), events);

  assert.deepEqual(notices, []);
  // Info stays out of the default log, which keeps only warnings and errors.
  assert.deepEqual(
    logged,
    events.map(() => ({ level: "info", eventName: "sdk_hook_decision" })),
  );
});

// Measured on SDK 0.3.288: after these hooks exit 2 only the hook_response
// carries the output (the tool result is the tool's own, or the turn ends with
// an empty result), so one info notice is the only place the user sees it.
test("a decision nothing else shows gets one info notice, not a failure", () => {
  const events = ["PostToolUse", "PostToolUseFailure", "PostToolBatch"];
  const { logged, notices } = exitTwoOn(makeSession(), events);

  assert.deepEqual(
    notices,
    events.map((event) => ({ severity: "info", message: `Hook ${event}:Bash returned a blocking error: gate says no` })),
  );
  assert.deepEqual(
    logged,
    events.map(() => ({ level: "info", eventName: "sdk_hook_decision" })),
  );
});

// Measured on SDK 0.3.288 with a SessionStart command hook that exits 2: the
// session starts and the SDK sends nothing but this hook_response, so the
// failure notice is the only place the user sees the hook's error.
test("a SessionStart hook that exits 2 cannot block startup, so it is a failure warning and a notice", () => {
  const session = makeSession();
  session.connected = false;
  let logged: LoggedRecord[] = [];
  noticesWritten(() => {
    logged = recordsLogged(() => {
      handleSdkMessage(
        session,
        hookResponse("SessionStart", {
          hook_name: "SessionStart:startup",
          outcome: "error",
          exit_code: 2,
          stderr: "start gate: env missing\n",
        }),
      );
    });
  });

  assert.deepEqual(logged, [{ level: "warn", eventName: "sdk_hook_failed" }]);
  assert.deepEqual(session.pendingHookNotices, [
    { severity: "warning", message: "Hook SessionStart:startup failed (exit 2): start gate: env missing" },
  ]);
});

// Notification and PermissionRequest ignore exit 2, WorktreeCreate fails on any
// non-zero exit, and an event the hook reference does not list (one a newer SDK
// adds) has no documented decision: all of them stay failures.
test("exit 2 on an event that ignores it, fails on it, or is unknown is a failure", () => {
  const events = ["Notification", "PermissionRequest", "WorktreeCreate", "SomeFutureEvent"];
  const { logged, notices } = exitTwoOn(makeSession(), events);

  assert.deepEqual(
    logged,
    events.map(() => ({ level: "warn", eventName: "sdk_hook_failed" })),
  );
  assert.deepEqual(
    notices,
    events.map((event) => ({ severity: "warning", message: `Hook ${event}:Bash failed (exit 2): gate says no` })),
  );
});

test("routine hook activity and a cancelled hook never become notices", () => {
  const session = makeSession();
  const notices = noticesWritten(() => {
    handleSdkMessage(session, {
      type: "system",
      subtype: "hook_started",
      hook_id: "hook-1",
      hook_name: "PreToolUse:Bash",
      hook_event: "PreToolUse",
      session_id: "session-1",
    } as unknown as SDKMessage);
    handleSdkMessage(session, hookResponse("PreToolUse", { outcome: "success", exit_code: 0, output: "ok" }, "ok"));
    handleSdkMessage(session, hookResponse("Stop", { outcome: "cancelled" }, "cancelled"));
  });

  assert.deepEqual(notices, []);
});

test("a hook failure notice carries a bounded, redacted excerpt", () => {
  const session = makeSession();
  const notices = noticesWritten(() => {
    handleSdkMessage(
      session,
      hookResponse("PostToolUse", {
        outcome: "error",
        exit_code: 1,
        stderr: `deploy failed api_key=sk-ant-verysecret ${"x".repeat(1_000)}`,
      }),
    );
  });

  const message = String(notices[0]?.message);
  assert.ok(!message.includes("verysecret"));
  assert.ok(message.includes("api_key=[redacted]"));
  assert.ok(message.length < 300, `notice was ${message.length} characters`);
});

// SessionStart hooks finish before the connect event, and the app ignores
// updates for a session it has not been told about.
test("a hook failure before the session connects is held, not written", () => {
  const session = makeSession();
  session.connected = false;
  const notices = noticesWritten(() => {
    handleSdkMessage(
      session,
      hookResponse("SessionStart", { outcome: "error", exit_code: 1, stderr: "missing env" }),
    );
  });

  assert.deepEqual(notices, []);
  assert.deepEqual(session.pendingHookNotices, [
    { severity: "warning", message: "Hook SessionStart:Bash failed (exit 1): missing env" },
  ]);
});

// emitConnectEvent starts an async session-title read that outlives the call and
// returns it. The read is pointed at an empty scratch config directory and awaited
// before the writer and the environment are restored, so the test never reads the
// real ~/.claude and does not depend on how the SDK maps a directory to a project
// folder (which differs on Windows).
test("held hook failures are written after the connect event, then cleared", async () => {
  const configDir = realpathSync(mkdtempSync(join(tmpdir(), "claude-rs-config-")));
  const session = makeSession();
  session.sessionId = "2f6c1f0e-1111-4111-8111-aaaaaaaaaaaa";
  session.cwd = configDir;
  session.connected = false;
  session.pendingHookNotices = [{ severity: "warning", message: "Hook SessionStart:Bash failed (exit 1): missing env" }];
  const previousConfigDir = process.env.CLAUDE_CONFIG_DIR;
  process.env.CLAUDE_CONFIG_DIR = configDir;
  const writes: string[] = [];
  const restore = replaceProtocolEventWriter((line) => {
    writes.push(line);
  });
  try {
    await emitConnectEvent(session);
  } finally {
    restore();
    if (previousConfigDir === undefined) {
      delete process.env.CLAUDE_CONFIG_DIR;
    } else {
      process.env.CLAUDE_CONFIG_DIR = previousConfigDir;
    }
    rmSync(configDir, { recursive: true, force: true });
  }

  const events = writes.map((line) => JSON.parse(line) as Record<string, unknown>);
  const connected = events.findIndex((event) => event.event === "connected");
  const notice = events.findIndex(
    (event) => (event.update as Record<string, unknown> | undefined)?.type === "system_notice_update",
  );
  assert.ok(connected >= 0 && notice > connected, JSON.stringify(events.map((event) => event.event)));
  const written = events[notice]?.update as Record<string, unknown>;
  assert.deepEqual(
    { severity: written.severity, message: written.message },
    { severity: "warning", message: "Hook SessionStart:Bash failed (exit 1): missing env" },
  );
  assert.equal(session.pendingHookNotices, undefined);
});
