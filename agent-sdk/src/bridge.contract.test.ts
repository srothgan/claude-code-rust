import test from "node:test";
import assert from "node:assert/strict";
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { mkdtempSync, mkdirSync, readdirSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import readline from "node:readline";
import { fileURLToPath, pathToFileURL } from "node:url";
import type { SettingsMutation, SettingsResult, SettingsSnapshot } from "./types.js";
import type { SessionUpdate } from "./types.js";

type BridgeEnvelope = Record<string, unknown>;

const bridgePath = join(dirname(fileURLToPath(import.meta.url)), "bridge.js");

function productionTypeScriptFiles(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) {
      return productionTypeScriptFiles(path);
    }
    return entry.isFile() && entry.name.endsWith(".ts") && !entry.name.endsWith(".test.ts")
      ? [path]
      : [];
  });
}

class SpawnedBridge {
  readonly child: ChildProcessWithoutNullStreams;
  readonly stderrLines: string[] = [];

  #stdoutQueue: BridgeEnvelope[] = [];
  #stdoutWaiters: Array<{
    resolve: (envelope: BridgeEnvelope) => void;
    reject: (error: Error) => void;
  }> = [];
  #exitCode: number | null | undefined;

  constructor(env: NodeJS.ProcessEnv = {}, nodeArgs: string[] = []) {
    this.child = spawn(process.execPath, [...nodeArgs, bridgePath], {
      env: {
        ...process.env,
        CLAUDE_RS_BRIDGE_DIAGNOSTICS: "1",
        ...env,
      },
      stdio: "pipe",
      windowsHide: true,
    });

    const stdout = readline.createInterface({ input: this.child.stdout });
    stdout.on("line", (line) => {
      let envelope: BridgeEnvelope;
      try {
        envelope = JSON.parse(line) as BridgeEnvelope;
      } catch (error) {
        this.#rejectWaiters(
          error instanceof Error
            ? error
            : new Error(`failed to parse bridge stdout line: ${String(error)}`),
        );
        return;
      }
      const waiter = this.#stdoutWaiters.shift();
      if (waiter) {
        waiter.resolve(envelope);
      } else {
        this.#stdoutQueue.push(envelope);
      }
    });

    const stderr = readline.createInterface({ input: this.child.stderr });
    stderr.on("line", (line) => {
      this.stderrLines.push(line);
    });

    this.child.once("exit", (code) => {
      this.#exitCode = code;
      this.#rejectWaiters(
        new Error(`bridge exited before next protocol event: ${code}`),
      );
    });
  }

  writeCommand(command: BridgeEnvelope): void {
    this.writeRaw(`${JSON.stringify(command)}\n`);
  }

  writeRaw(line: string): void {
    this.child.stdin.write(line);
  }

  nextEnvelope(timeoutMs = 2_000): Promise<BridgeEnvelope> {
    const queued = this.#stdoutQueue.shift();
    if (queued) {
      return Promise.resolve(queued);
    }
    if (this.#exitCode !== undefined) {
      return Promise.reject(
        new Error(`bridge already exited: ${this.#exitCode}`),
      );
    }

    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        const index = this.#stdoutWaiters.findIndex(
          (waiter) => waiter.resolve === resolve,
        );
        if (index >= 0) {
          this.#stdoutWaiters.splice(index, 1);
        }
        reject(new Error("timed out waiting for bridge protocol event"));
      }, timeoutMs);

      this.#stdoutWaiters.push({
        resolve: (envelope) => {
          clearTimeout(timeout);
          resolve(envelope);
        },
        reject: (error) => {
          clearTimeout(timeout);
          reject(error);
        },
      });
    });
  }

  async waitForExit(timeoutMs = 2_000): Promise<number | null> {
    if (this.#exitCode !== undefined) {
      return this.#exitCode;
    }
    return await new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        reject(new Error("timed out waiting for bridge exit"));
      }, timeoutMs);
      this.child.once("exit", (code) => {
        clearTimeout(timeout);
        resolve(code);
      });
    });
  }

  async stop(): Promise<void> {
    if (this.#exitCode !== undefined) {
      return;
    }
    this.child.kill();
    await this.waitForExit().catch(() => undefined);
  }

  #rejectWaiters(error: Error): void {
    const waiters = this.#stdoutWaiters.splice(0);
    for (const waiter of waiters) {
      waiter.reject(error);
    }
  }
}

function assertProtocolEvent(
  envelope: BridgeEnvelope,
  eventName: string,
): asserts envelope is BridgeEnvelope & { event: string } {
  assert.equal(envelope.event, eventName);
}

test("bridge process keeps initial and later background tasks linked across turns and settles actual outcomes", async () => {
  for (const toolName of ["Bash", "Agent"]) {
    for (const initiallyBackgrounded of [true, false]) {
    for (const outcome of ["completed", "failed", "stopped"]) {
      const toolId = "primary-tool";
      const taskId = "primary-task";
      const system = (subtype: string, fields: BridgeEnvelope) => ({ type: "system", subtype, uuid: crypto.randomUUID(), task_id: taskId, ...fields });
      const toolResult = (raw: BridgeEnvelope) => ({ type: "user", uuid: crypto.randomUUID(), tool_use_result: raw, message: { role: "user", content: [{ type: "tool_result", tool_use_id: toolId, content: "Launch acknowledgement" }] } });
      const turnEnd = { type: "result", subtype: "success", is_error: false, duration_ms: 1, num_turns: 1, result: "Foreground finished", usage: {}, modelUsage: {}, permission_denials: [] };
      const fixture = {
        start: [
          { type: "assistant", message: { id: "assistant-1", role: "assistant", content: [{ type: "tool_use", id: toolId, name: toolName, input: toolName === "Bash" ? { command: "long command", run_in_background: initiallyBackgrounded } : { description: "Investigate", prompt: "Investigate", run_in_background: initiallyBackgrounded } }] } },
          system("task_started", { tool_use_id: toolId, description: "Investigate", task_type: toolName === "Bash" ? "local_bash" : "local_agent", is_backgrounded: initiallyBackgrounded }),
          ...(!initiallyBackgrounded ? [system("task_updated", { patch: { status: "running", is_backgrounded: true } })] : []),
          toolResult(toolName === "Bash" ? { stdout: "", stderr: "", interrupted: false, backgroundTaskId: taskId, ...(!initiallyBackgrounded ? { timedOutAfterMs: 120000 } : {}) } : { status: "async_launched", isAsync: true, agentId: taskId, description: "Investigate", prompt: "Investigate", outputFile: "task.output" }),
          system("task_progress", { description: "Still working", usage: { total_tokens: 1, tool_uses: 1, duration_ms: 10 } }),
          ...(toolName === "Agent" ? [
            { type: "assistant", parent_tool_use_id: toolId, message: { role: "assistant", content: [{ type: "tool_use", id: "nested-agent", name: "Agent", input: { description: "Nested reviewer", prompt: "Review" } }] } },
            { type: "assistant", parent_tool_use_id: "nested-agent", message: { role: "assistant", content: [{ type: "tool_use", id: "nested-bash", name: "Bash", input: { command: "Nested command" } }] } },
          ] : []),
          { ...turnEnd, subtype: "error_during_execution", is_error: true, errors: ["Interrupted by user"] },
        ],
        finish: [
          ...(toolName === "Agent" ? [
            { type: "user", parent_tool_use_id: "nested-agent", message: { role: "user", content: [{ type: "tool_result", tool_use_id: "nested-bash", content: "Nested command finished" }] } },
            { type: "user", parent_tool_use_id: toolId, message: { role: "user", content: [{ type: "tool_result", tool_use_id: "nested-agent", content: "Nested review finished" }] } },
          ] : []),
          system("task_updated", { patch: { status: outcome === "stopped" ? "killed" : outcome, end_time: 10 } }),
          system("task_notification", { status: outcome, summary: "Actual task result", output_file: "task.output" }),
          system("task_notification", { status: outcome, summary: "Actual task result", output_file: "task.output" }),
          toolResult(toolName === "Bash" ? { backgroundTaskId: taskId } : { status: "async_launched", isAsync: true, agentId: taskId }),
          // A queued progress event must never revive a settled task.
          system("task_progress", { description: "Stale progress", usage: { total_tokens: 1, tool_uses: 1, duration_ms: 11 } }),
          turnEnd,
        ],
        rejected: [
          { type: "assistant", message: { role: "assistant", content: [{ type: "tool_use", id: "rejected-launch", name: toolName, input: { command: "rejected", run_in_background: true } }] } },
          { type: "user", tool_use_result: { backgroundTaskId: "rejected-task" }, message: { role: "user", content: [{ type: "tool_result", tool_use_id: "rejected-launch", is_error: true, content: "Permission denied" }] } },
          turnEnd,
        ],
        requested: [
          { type: "assistant", message: { role: "assistant", content: [{ type: "tool_use", id: "requested-only", name: toolName, input: { command: "requested", run_in_background: true } }] } },
          { type: "system", subtype: "task_started", task_id: "requested-task", tool_use_id: "requested-only", is_backgrounded: false, description: "Foreground execution" },
          turnEnd,
        ],
      };
      const { bridge, cleanup } = ultracodeFixtureBridge({ BACKGROUND_TASK_FIXTURE: JSON.stringify(fixture) });
      try {
        bridge.writeCommand({ command: "create_session", cwd: process.cwd(), launch_settings: {} });
        const connected = await nextMatching(bridge, event => event.event === "connected");
        const collect = async (step: string) => {
          bridge.writeCommand({ command: "prompt", session_id: connected.session_id, message_uuid: crypto.randomUUID(), chunks: [{ kind: "text", value: `fixture task ${step}` }] });
          const events: BridgeEnvelope[] = [];
          while (true) {
            const event = await bridge.nextEnvelope(5_000);
            events.push(event);
            if (event.event === "turn_complete" || event.event === "turn_error") return events;
          }
        };
        const fields = (events: BridgeEnvelope[]) => events.flatMap(event => {
          const update = event.update as SessionUpdate | undefined;
          return update?.type === "tool_call_update" && update.tool_call_update.tool_call_id === toolId ? [update.tool_call_update.fields] : [];
        });
        const launchEvents = await collect("start");
        const launchUpdates = fields(launchEvents);
        if (toolName === "Agent") {
          for (const id of ["nested-agent", "nested-bash"]) {
            const updates = launchEvents.flatMap(event => {
              const update = event.update as SessionUpdate | undefined;
              return update?.type === "tool_call_update" && update.tool_call_update.tool_call_id === id ? [update.tool_call_update.fields] : [];
            });
            assert.ok(!updates.some(update => ["completed", "failed", "killed"].includes(update.status ?? "")), "foreground interruption must not finalize descendants of a background agent");
          }
        }
        assert.ok(launchUpdates.some(update => update.status === "detached"), `${toolName}: confirmed launch must detach`);
        const detachment = launchUpdates.findIndex(update => update.status === "detached");
        assert.ok(launchUpdates.slice(detachment).every(update => update.status === undefined || update.status === "detached"), `${toolName}: launch/progress/interruption must not settle or reattach`);
        const completionEvents = await collect("finish");
        const completionUpdates = fields(completionEvents);
        if (toolName === "Agent") {
          for (const id of ["nested-agent", "nested-bash"]) {
            const updates = completionEvents.flatMap(event => {
              const update = event.update as SessionUpdate | undefined;
              return update?.type === "tool_call_update" && update.tool_call_update.tool_call_id === id ? [update.tool_call_update.fields] : [];
            });
            assert.equal(updates.filter(update => update.status === "completed").length, 1);
            assert.ok(updates.some(update => update.raw_output?.includes("finished")), "the real descendant result must remain deliverable");
          }
        }
        const inventory = completionEvents.flatMap(event => {
          const update = event.update as SessionUpdate | undefined;
          return update?.type === "task_state_update" ? update.tasks.filter(task => task.task_id === taskId) : [];
        });
        assert.equal(inventory.at(-1)?.status, "completed", "stale progress and duplicate launch acknowledgements must not revive task inventory");
        assert.equal(completionUpdates.filter(update => update.status === (outcome === "stopped" ? "killed" : outcome)).length, 1, `${toolName}: duplicate notifications must produce one terminal outcome on the original tool`);
        assert.ok(!completionUpdates.slice(completionUpdates.findIndex(update => update.status === (outcome === "stopped" ? "killed" : outcome))).some(update => update.status === "in_progress" || update.status === "detached"), `${toolName}: stale progress must not revive completion`);
        assert.ok(completionUpdates.some(update => update.raw_output?.includes("Actual task result")), `${toolName}: render actual completion, not the launch placeholder`);
        for (const step of ["rejected", "requested"]) {
          const id = step === "rejected" ? "rejected-launch" : "requested-only";
          const events = await collect(step);
          const updates = events.flatMap(event => {
            const update = event.update as SessionUpdate | undefined;
            return update?.type === "tool_call_update" && update.tool_call_update.tool_call_id === id ? [update.tool_call_update.fields] : [];
          });
          assert.ok(!updates.some(update => update.status === "detached"), "requesting background execution or rejecting a launch must not detach");
          if (step === "rejected") assert.ok(updates.some(update => update.status === "failed"));
          else assert.ok(updates.some(update => update.status === "in_progress"));
        }
      } finally {
        await cleanup();
      }
    }
  }
  }
});

test("bridge announces an autonomous completion reply before streaming text without another prompt", async () => {
  const response = (event: BridgeEnvelope) => ({ type: "stream_event", uuid: crypto.randomUUID(), parent_tool_use_id: null, event });
  const turnEnd = { type: "result", subtype: "success", is_error: false, duration_ms: 1, num_turns: 1, result: "", usage: {}, modelUsage: {}, permission_denials: [] };
  const fixture = { start: [
    { type: "assistant", message: { role: "assistant", content: [{ type: "tool_use", id: "independent-tool", name: "Bash", input: { command: "background work" } }] } },
    { type: "user", tool_use_result: { backgroundTaskId: "independent-task" }, message: { role: "user", content: [{ type: "tool_result", tool_use_id: "independent-tool", content: "Launch acknowledgement" }] } },
    turnEnd,
    { type: "system", subtype: "task_notification", task_id: "independent-task", tool_use_id: "independent-tool", status: "completed", summary: "Background command completed" },
    response({ type: "message_start", message: { id: "autonomous-response" } }),
    response({ type: "content_block_start", index: 0, content_block: { type: "text", text: "" } }),
    response({ type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "The" } }),
    response({ type: "content_block_delta", index: 0, delta: { type: "text_delta", text: " task finished completely." } }),
    response({ type: "message_stop" }),
    { ...turnEnd, origin: { kind: "task-notification" } },
  ] };
  const { bridge, cleanup } = ultracodeFixtureBridge({ BACKGROUND_TASK_FIXTURE: JSON.stringify(fixture) });
  try {
    bridge.writeCommand({ command: "create_session", cwd: process.cwd(), launch_settings: {} });
    const connected = await nextMatching(bridge, event => event.event === "connected");
    bridge.writeCommand({ command: "prompt", session_id: connected.session_id, message_uuid: crypto.randomUUID(), chunks: [{ kind: "text", value: "fixture task start" }] });
    await nextMatching(bridge, event => event.event === "turn_complete");
    const events: BridgeEnvelope[] = [];
    while (true) {
      const event = await bridge.nextEnvelope(5_000);
      events.push(event);
      if (event.event === "turn_complete") break;
    }
    const updates = events.flatMap(event => event.update ? [event.update as SessionUpdate] : []);
    const activity = updates.findIndex(update => update.type === "agent_response_started");
    const text = updates.findIndex(update => update.type === "agent_message_chunk");
    assert.ok(activity >= 0 && activity < text, "main response activation must precede text on NDJSON");
    assert.deepEqual(updates.flatMap(update => update.type === "agent_message_chunk" ? [update.content] : []), [{ type: "text", text: "The" }, { type: "text", text: " task finished completely." }]);
    assert.ok(!events.some(event => event.event === "user_message_started"), "an autonomous reply must not fabricate another user turn");
    assert.equal(updates.filter(update => update.type === "tool_call_update" && update.tool_call_update.fields.status === "completed").length, 1);
  } finally {
    await cleanup();
  }
});

test("resume restores background acknowledgements and routes task-only outcomes to their original tools", async () => {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), "claude-rs-background-resume-")));
  const cwd = process.cwd();
  const configDir = join(directory, "config");
  const projectDir = join(configDir, "projects", cwd.replace(/[^a-zA-Z0-9]/g, "-"));
  mkdirSync(projectDir, { recursive: true });
  const sessionId = "22222222-2222-4222-8222-222222222222";
  const records: BridgeEnvelope[] = [];
  const append = (type: string, message: unknown, extra: BridgeEnvelope = {}) => {
    records.push({ uuid: `record-${records.length}`, parentUuid: records.at(-1)?.uuid ?? null, type, message, sessionId, cwd, timestamp: "2026-10-06T10:55:30.787Z", ...extra });
  };
  append("user", { role: "user", content: "Launch background work" });
  append("assistant", { id: "launch", role: "assistant", content: [
    { type: "tool_use", id: "completed-shell", name: "Bash", input: { command: "saved command", run_in_background: true } },
    { type: "tool_use", id: "pending-agent", name: "Agent", input: { description: "Saved agent", prompt: "Work", run_in_background: true } },
  ] });
  append("user", { role: "user", content: [{ type: "tool_result", tool_use_id: "completed-shell", content: "Launch acknowledgement" }] }, { toolUseResult: { stdout: "partial stdout", stderr: "", backgroundTaskId: "saved-shell" } });
  append("user", { role: "user", content: [{ type: "tool_result", tool_use_id: "pending-agent", content: "Agent launch acknowledgement" }] }, { toolUseResult: { status: "async_launched", isAsync: true, agentId: "saved-agent", description: "Saved agent", outputFile: "saved.output" } });
  append("user", { role: "user", content: "<task-notification><task-id>saved-shell</task-id><status>completed</status><summary>Saved command result</summary></task-notification>" }, { origin: { kind: "task-notification", producer: "session-task" } });
  append("assistant", { id: "after-launch", role: "assistant", content: [{ type: "text", text: "Foreground continued" }] });
  const transcriptPath = join(projectDir, `${sessionId}.jsonl`);
  const transcript = `${records.map(record => JSON.stringify(record)).join("\n")}\n`;
  writeFileSync(transcriptPath, transcript);
  const completion = { type: "system", subtype: "task_notification", task_id: "saved-agent", status: "stopped", summary: "Saved agent stopped", output_file: "saved.output" };
  const { bridge, cleanup } = ultracodeFixtureBridge({
    CLAUDE_CONFIG_DIR: configDir, CLAUDE_CODE_PROJECT_DIR_NAME: undefined, RESUME_TRANSCRIPT_FIXTURE: "1",
    STARTUP_SESSIONS_FIXTURE: JSON.stringify([{ sessionId, cwd, lastModified: 1 }]),
    BACKGROUND_TASK_FIXTURE: JSON.stringify({ finish: [completion, completion,
      { type: "system", subtype: "task_progress", task_id: "saved-agent", description: "Stale resumed progress" },
      { type: "result", subtype: "success", is_error: false, duration_ms: 1, num_turns: 1, result: "Finished", usage: {}, modelUsage: {}, permission_denials: [] },
    ] }),
  });
  try {
    bridge.writeCommand({ command: "resume_session", session_id: sessionId, launch_settings: {} });
    const connected = await nextMatching(bridge, event => event.event === "connected" || event.event === "slash_command_error");
    assertProtocolEvent(connected, "connected");
    const history = connected.history_updates as SessionUpdate[];
    const statuses = (id: string) => history.flatMap(update => update.type === "tool_call" && update.tool_call.tool_call_id === id ? [update.tool_call.status]
      : update.type === "tool_call_update" && update.tool_call_update.tool_call_id === id && update.tool_call_update.fields.status ? [update.tool_call_update.fields.status] : []);
    assert.deepEqual(statuses("completed-shell"), ["in_progress", "detached", "completed"]);
    assert.deepEqual(statuses("pending-agent"), ["in_progress", "detached"]);
    const shellResult = history.slice().reverse().find(update => update.type === "tool_call_update" && update.tool_call_update.tool_call_id === "completed-shell");
    assert.ok(shellResult?.type === "tool_call_update");
    assert.match(shellResult.tool_call_update.fields.raw_output ?? "", /partial stdout/);
    assert.match(shellResult.tool_call_update.fields.raw_output ?? "", /Saved command result/);
    assert.ok(!history.some(update => update.type === "user_message_chunk" && update.content.type === "text" && update.content.text.includes("<task-notification>")));
    bridge.writeCommand({ command: "prompt", session_id: sessionId, message_uuid: crypto.randomUUID(), chunks: [{ kind: "text", value: "fixture task finish" }] });
    const results: SessionUpdate[] = [];
    while (true) {
      const event = await bridge.nextEnvelope(5_000);
      if (event.event === "session_update") results.push(event.update as SessionUpdate);
      if (event.event === "turn_complete" || event.event === "turn_error") break;
    }
    const outcomes = results.filter(update => update.type === "tool_call_update" && update.tool_call_update.tool_call_id === "pending-agent");
    assert.equal(outcomes.length, 1, "duplicate and stale resumed notifications must not create another result");
    assert.ok(outcomes[0]?.type === "tool_call_update");
    assert.equal(outcomes[0].tool_call_update.fields.status, "killed");
    assert.equal(outcomes[0].tool_call_update.fields.raw_output, "Saved agent stopped");
    assert.ok(!results.some(update => update.type === "tool_call"), "resume must not invent a replacement Agent for an existing shell or agent");
    assert.equal(readFileSync(transcriptPath, "utf8"), transcript);
  } finally {
    await cleanup();
    assert.equal(dirname(directory), realpathSync(tmpdir()));
    rmSync(directory, { recursive: true, force: true });
  }
});

test("bridge process emits connection_failed for malformed JSON", async () => {
  const bridge = new SpawnedBridge();
  try {
    bridge.writeRaw("{ not-json\n");

    const envelope = await bridge.nextEnvelope();

    assertProtocolEvent(envelope, "connection_failed");
    assert.match(String(envelope.message), /invalid command envelope/);
    assert.equal("schema" in envelope, false);
  } finally {
    await bridge.stop();
  }
});

test("bridge process initializes with stable capability envelope", async () => {
  const bridge = new SpawnedBridge();
  try {
    bridge.writeCommand({
      request_id: "req-init",
      command: "initialize",
      cwd: process.cwd(),
    });

    const envelope = await bridge.nextEnvelope();

    assertProtocolEvent(envelope, "initialized");
    assert.equal(envelope.request_id, "req-init");
    assert.deepEqual(envelope.result, {
      agent_name: "claude-rs-agent-bridge",
      agent_version: "0.1.0",
      auth_methods: [
        {
          id: "claude-login",
          name: "Log in with Claude",
          description: "Run `claude /login` in a terminal",
        },
      ],
      capabilities: {
        prompt_image: true,
        prompt_embedded_context: true,
        supports_session_listing: true,
        supports_resume_session: true,
      },
    });
    assert.ok(
      bridge.stderrLines.every((line) => {
        const parsed = JSON.parse(line) as { schema?: unknown };
        return parsed.schema === "claude-rs-log/v1";
      }),
    );
  } finally {
    await bridge.stop();
  }
});

test("bridge process preserves request_id for unsupported commands", async () => {
  const bridge = new SpawnedBridge();
  try {
    bridge.writeCommand({
      request_id: "req-unsupported",
      command: "future_command",
    });

    const envelope = await bridge.nextEnvelope();

    assertProtocolEvent(envelope, "connection_failed");
    assert.equal(envelope.request_id, "req-unsupported");
    assert.match(
      String(envelope.message),
      /unsupported command: future_command/,
    );
  } finally {
    await bridge.stop();
  }
});

test("bridge process correlates a side-question failure across NDJSON", async () => {
  const bridge = new SpawnedBridge();
  try {
    bridge.writeCommand({
      command: "side_question",
      session_id: "missing-session",
      btw_id: "btw-1",
      question: "Why  preserve spaces?\nAnd lines?",
    });

    const envelope = await bridge.nextEnvelope();

    assertProtocolEvent(envelope, "btw_failed");
    assert.equal(envelope.session_id, "missing-session");
    assert.equal(envelope.btw_id, "btw-1");
    assert.equal(envelope.question, "Why  preserve spaces?\nAnd lines?");
    assert.match(String(envelope.error), /active SDK query/);
  } finally {
    await bridge.stop();
  }
});

test("bridge process exits cleanly on shutdown", async () => {
  const bridge = new SpawnedBridge();
  try {
    bridge.writeCommand({ command: "shutdown" });

    assert.equal(await bridge.waitForExit(), 0);
  } finally {
    await bridge.stop();
  }
});

test("bridge process reports actionable session initialization failure", async () => {
  const missingClaudeExecutable = join(
    tmpdir(),
    `claude-rs-missing-claude-code-${process.pid}`,
  );
  const bridge = new SpawnedBridge({
    CLAUDE_CODE_EXECUTABLE: missingClaudeExecutable,
  });
  try {
    bridge.writeCommand({
      request_id: "req-init",
      command: "initialize",
      cwd: process.cwd(),
    });
    assertProtocolEvent(await bridge.nextEnvelope(), "initialized");
    const sessions = await bridge.nextEnvelope();
    assertProtocolEvent(sessions, "sessions_listed");
    assert.equal(sessions.request_id, "req-init");

    bridge.writeCommand({
      request_id: "req-create",
      command: "create_session",
      cwd: process.cwd(),
      launch_settings: {},
    });

    const envelope = await bridge.nextEnvelope();

    assertProtocolEvent(envelope, "connection_failed");
    assert.equal(envelope.request_id, "req-create");
    assert.match(
      String(envelope.message),
      /bridge command failed \(create_session\)/,
    );
    assert.match(
      String(envelope.message),
      /CLAUDE_CODE_EXECUTABLE does not exist/,
    );
  } finally {
    await bridge.stop();
  }
});

test("real SDK transports one correlated startup failure from a controlled CLI child", async () => {
  const directory = mkdtempSync(join(tmpdir(), "claude-rs-startup-"));
  const cliPath = join(directory, "startup-cli.js");
  writeFileSync(cliPath, `
    process.stdin.once("data", () => {
      const errors = ["Original startup guidance ANTHROPIC_API_KEY=private-key. Please login."];
      process.stderr.write(errors[0] + "\\n");
      if (process.env.CLAUDE_CODE_STARTUP_FAILURE_RESULTS !== "1") process.exit(1);
      process.stdout.write(JSON.stringify({
        type: "result", subtype: "error_during_execution", is_error: true,
        startup_failure_reason: "provider_not_allowed", errors,
        duration_ms: 0, duration_api_ms: 0, num_turns: 0,
        total_cost_usd: 0, usage: {}, modelUsage: {}, permission_denials: [],
        uuid: "startup-result", session_id: "startup-session", stop_reason: null,
      }) + "\\n", () => process.exit(1));
    });
  `);
  const bridge = new SpawnedBridge({ CLAUDE_CODE_EXECUTABLE: cliPath });
  try {
    bridge.writeCommand({ command: "create_session", request_id: "startup-connect", cwd: process.cwd(), launch_settings: {} });
    const failure = await bridge.nextEnvelope(5_000);
    assert.deepEqual(failure, {
      event: "connection_failed", request_id: "startup-connect",
      message: "Claude Code startup failed.",
      startup_failure: { reason: "provider_not_allowed", errors: ["Original startup guidance ANTHROPIC_API_KEY=[redacted] Please login."] },
    });
    bridge.writeCommand({ command: "shutdown" });
    assert.equal(await bridge.waitForExit(5_000), 0);
    await assert.rejects(bridge.nextEnvelope(), /exited/);
    const logs = bridge.stderrLines.join("\n");
    assert.ok(logs.includes('"startup_failure_reason":"provider_not_allowed"'));
    assert.ok(logs.includes("Original startup guidance"));
    assert.ok(!logs.includes("private-key"));
    assert.ok(!logs.includes('"event_name":"session_stream_ended_before_connect"'));
    assert.ok(!logs.includes('"event_name":"session_initialization_failed"'));
  } finally {
    await bridge.stop();
    rmSync(directory, { recursive: true, force: true });
  }
});

test("production bridge source stays on the public main SDK export", () => {
  const sourceDirectory = resolve(dirname(fileURLToPath(import.meta.url)), "../src");
  const forbiddenDeepImport = /@anthropic-ai\/claude-agent-sdk\/(?:browser|bridge)/;
  const forbiddenFactory = /\bcreateSdkMcpServer\b/;

  for (const path of productionTypeScriptFiles(sourceDirectory)) {
    const source = readFileSync(path, "utf8");
    assert.doesNotMatch(source, forbiddenDeepImport, path);
    assert.doesNotMatch(source, forbiddenFactory, path);
  }
});


// Only the external SDK is replaced. The spawned process runs the production
// readline parser, command scheduler, session lifecycle and NDJSON writer.
function ultracodeFixtureBridge(env: NodeJS.ProcessEnv = {}): { bridge: SpawnedBridge; cleanup: () => Promise<void> } {
  const directory = mkdtempSync(join(tmpdir(), "claude-rs-ultracode-"));
  const sdkPath = import.meta.resolve("@anthropic-ai/claude-agent-sdk");
  const fixturePath = join(directory, "sdk-fixture.mjs");
  writeFileSync(fixturePath, `
    export * from ${JSON.stringify(sdkPath)};
    import { appendFileSync } from "node:fs";
    import { resolveSettings as sdkResolveSettings } from ${JSON.stringify(sdkPath)};
    import { getSessionMessages as sdkGetSessionMessages, importSessionToStore as sdkImportSessionToStore } from ${JSON.stringify(sdkPath)};
    export async function resolveSettings(options) {
      const managedSettings = process.env.SETTINGS_POLICY_FIXTURE;
      return sdkResolveSettings({ ...options, ...(managedSettings ? { serverManagedSettings: JSON.parse(managedSettings) } : {}) });
    }
    function record(value) { if (process.env.STARTUP_JOURNAL) appendFileSync(process.env.STARTUP_JOURNAL, JSON.stringify(value) + "\\n"); }
    export async function listSessions(options) {
      record({ type: "list", options });
      const seeded = JSON.parse(process.env.STARTUP_SESSIONS_FIXTURE ?? "[]");
      return [...seeded, ...[...saved.keys()].map((sessionId, index) => ({ sessionId, cwd: saved.get(sessionId).cwd, lastModified: index + 1 }))].filter(entry => !options?.dir || entry.cwd === options.dir);
    }
    export async function getSessionMessages(...args) { return process.env.RESUME_TRANSCRIPT_FIXTURE === "1" ? sdkGetSessionMessages(...args) : []; }
    export async function importSessionToStore(...args) { if (process.env.RESUME_TRANSCRIPT_FIXTURE === "1") await sdkImportSessionToStore(...args); }
    const saved = new Map();
    export function query({ prompt, options }) {
      record({ type: "query", cwd: options.cwd, resume: options.resume, model: options.model, effort: options.effort, permissionMode: options.permissionMode, agent: options.agent });
      const state = saved.get(options.resume) ?? { requested: false, effort: "high", cwd: options.cwd };
      saved.set(options.sessionId ?? options.resume, state);
      let model = options.model ?? "opus";
      let effective = {};
      let fastMode = false;
      let fastReadFails = process.env.FAST_READ_FAILURE_FIXTURE === "1";
      const sessionFixture = process.env.SESSION_SETTINGS_FIXTURE === "1";
      let done = false;
      let waiter;
      const queue = [];
      const commandFixture = process.env.SLASH_COMMANDS_FIXTURE === "1";
      const bootstrapCommands = [
        { name: "clear", description: "Clear conversation", argumentHint: "[name]", aliases: ["reset", "new"], builtin: true },
        { name: "deploy", description: "Deploy", argumentHint: "<target>", aliases: ["ship"] },
        { name: "removed-plugin", description: "Old plugin", argumentHint: "" },
      ];
      const currentCommands = [...bootstrapCommands.slice(0, 2), { name: "current-plugin", description: "Current plugin", argumentHint: "[optional reason]" }];
      let clearCount = 0;
      function push(value) { if (waiter) { const resolve = waiter; waiter = undefined; resolve({ value, done: false }); } else queue.push(value); }
      void (async () => {
        for await (const message of prompt) {
          const text = message.message?.content?.filter(block => block.type === "text").map(block => block.text).join("").trim();
          if (process.env.BACKGROUND_TASK_FIXTURE && text.startsWith("fixture task ")) {
            const steps = JSON.parse(process.env.BACKGROUND_TASK_FIXTURE);
            for (const event of steps[text.slice("fixture task ".length)] ?? []) push(event);
          }
          if (commandFixture && text === "/fixture-refresh-commands") {
            push({ type: "system", subtype: "commands_changed", commands: currentCommands });
          }
          if (commandFixture && text === "/clear") {
            const sessionId = "fixture-cleared-" + ++clearCount;
            push({ type: "conversation_reset", new_conversation_id: sessionId, trigger: "clear" });
            push({ type: "system", subtype: "init", session_id: sessionId, model, slash_commands: bootstrapCommands.map(command => command.name) });
          }
          if (JSON.stringify(message.message?.content).includes("fixture conversation reset")) {
            push({ type: "conversation_reset", new_conversation_id: "fixture-conversation-2" });
          }
          if (text === "fixture observe permissions") {
            push({ type: "system", subtype: "init", session_id: options.resume || options.sessionId, model, permissionMode: effective.permissions?.defaultMode ?? "default" });
          }
          if (text === "fixture per-turn effort") {
            push({ type: "system", subtype: "init", session_id: options.resume || options.sessionId, model, effort: "xhigh" });
          }
        }
      })();
      return {
        [Symbol.asyncIterator]() { return this; },
        next() {
          if (queue.length) return Promise.resolve({ value: queue.shift(), done: false });
          if (done) return Promise.resolve({ done: true });
          return new Promise(resolve => { waiter = resolve; });
        },
        close() { done = true; waiter?.({ done: true }); },
        async initializationResult() {
          if (sessionFixture) {
            effective = (await sdkResolveSettings({ cwd: options.cwd, settingSources: options.settingSources })).effective;
            model = options.model ?? effective.model ?? "opus";
            const canonical = model === "opus" ? "claude-opus-5-5" : "claude-sonnet-5-5";
            state.effort = options.effort ?? effective.modelSettings?.[canonical]?.effortLevel ?? "high";
            fastMode = effective.fastMode ?? false;
          }
          if (process.env.SETTINGS_JOURNAL === "1") {
            const resolved = await sdkResolveSettings({ cwd: options.cwd, settingSources: options.settingSources, settings: options.settings });
            record({ type: "settings", effective: resolved.effective, checkpointing: options.enableFileCheckpointing });
          }
          return { current_permission_mode: process.env.PERMISSION_MODE_FIXTURE ?? effective.permissions?.defaultMode ?? "default", models: ["default", "opus", "sonnet", "haiku", "fixture-unavailable", "fixture-read-failure"].map(value => ({ value, resolvedModel: value === "default" || value === "opus" ? "claude-opus-5-5" : value === "sonnet" ? "claude-sonnet-5-5" : value, displayName: value, supportsEffort: value !== "haiku", supportsFastMode: value !== "haiku", supportsAutoMode: value !== "haiku", supportedEffortLevels: value === "haiku" ? [] : ["low", "medium", "high", "xhigh", "max"] })), commands: commandFixture ? bootstrapCommands : [], agents: sessionFixture ? [{ name: "reviewer", description: "Review code", model: "sonnet" }] : [], account: { apiKeySource: "fixture" }, fast_mode_state: "off" }; },
        async supportedCommands() { return bootstrapCommands; },
        async setModel(value) { model = process.env.MODEL_STEP_DOWN_FIXTURE === "1" && value === "opus" ? "sonnet" : value; },
        async setPermissionMode(value) { if (sessionFixture && value === "auto") throw new Error("Cannot set permission mode to auto: account restriction"); record({ type: "mode", value }); },
        async reinitialize() { if (fastMode && fastReadFails) { fastReadFails = false; throw new Error("fixture state read failed"); } return { fast_mode_state: fastMode ? "on" : "off" }; },
        async getSettings() {
          if (model === "fixture-read-failure") throw new Error("fixture read failed");
          const available = model !== "haiku" && model !== "fixture-unavailable" && process.env.ULTRACODE_FIXTURE_WORKFLOWS !== "off";
          return { effective, applied: { model: sessionFixture ? model === "opus" ? "claude-opus-5-5" : model === "sonnet" ? "claude-sonnet-5-5" : model : model, ultracodeAvailable: available, ultracodeRequested: state.requested, ultracode: state.requested && available, effort: model === "haiku" ? null : state.effort } };
        },
        async applyFlagSettings(settings) {
          record({ type: "flags", settings });
          if (settings.alwaysThinkingEnabled !== undefined) { if (settings.alwaysThinkingEnabled === null) effective = (await sdkResolveSettings({ cwd: options.cwd, settingSources: options.settingSources })).effective; else effective = { ...effective, alwaysThinkingEnabled: settings.alwaysThinkingEnabled }; }
          if (settings.fastMode !== undefined) fastMode = settings.fastMode;
          if (settings.agent !== undefined) { if (settings.agent === "reviewer") model = "sonnet"; }
          if (settings.ultracode === true && process.env.ULTRACODE_FIXTURE_WORKFLOWS === "off") throw new Error("apply_flag_settings: ultracode is not available for this session (dynamic workflows are off)");
          if (settings.ultracode === true && model === "haiku") throw new Error("apply_flag_settings: ultracode is not available for this session (haiku does not support it)");
          if (settings.effortLevel !== undefined) {
            state.effort = settings.effortLevel === null ? "high" : settings.effortLevel === "max" && process.env.EFFORT_FIXTURE_CAP === "high" ? "high" : settings.effortLevel;
            state.requested = settings.ultracode === true;
          } else if (settings.ultracode !== undefined) state.requested = settings.ultracode;
        },
      };
    }
  `);
  const loaderPath = join(directory, "loader.mjs");
  writeFileSync(loaderPath, `
    import { registerHooks } from "node:module";
    registerHooks({ resolve(specifier, context, nextResolve) {
      return specifier === "@anthropic-ai/claude-agent-sdk"
        ? { url: ${JSON.stringify(pathToFileURL(fixturePath).href)}, shortCircuit: true }
        : nextResolve(specifier, context);
    } });
  `);
  const bridge = new SpawnedBridge({ CLAUDE_CODE_EXECUTABLE: "", ...env }, ["--import", pathToFileURL(loaderPath).href]);
  return { bridge, cleanup: async () => { await bridge.stop(); rmSync(directory, { recursive: true, force: true }); } };
}

async function nextMatching(bridge: SpawnedBridge, matches: (event: BridgeEnvelope) => boolean): Promise<BridgeEnvelope> {
  for (let index = 0; index < 30; index++) {
    const event = await bridge.nextEnvelope(5_000).catch((error: unknown) => {
      throw new Error(`Waiting for ${matches.toString()}: ${String(error)}; diagnostics: ${bridge.stderrLines.slice(-4).join("\n")}`);
    });
    if (matches(event)) return event;
  }
  throw new Error("expected bridge event was not emitted");
}

async function nextUltracode(bridge: SpawnedBridge): Promise<unknown> {
  const event = await nextMatching(bridge, event => (event.update as BridgeEnvelope)?.type === "ultracode_update");
  return (event.update as BridgeEnvelope).ultracode;
}

test("resume command restores the screenshot conversation without internal task XML", async () => {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), "claude-rs-resume-")));
  const cwd = process.cwd();
  const configDir = join(directory, "config");
  const projectDir = join(configDir, "projects", cwd.replace(/[^a-zA-Z0-9]/g, "-"));
  mkdirSync(projectDir, { recursive: true });
  const sessionId = "f96f66ae-b5d4-4bc2-b5a3-08336cf4e850";
  const completed = `<task-notification>
<task-id>bbwikd6z7</task-id>
<tool-use-id>toolu_01DExEGoUHANdvdafPRrG1ks</tool-use-id>
<output-file>/fixture/tasks/bbwikd6z7.output</output-file>
<status>completed</status>

<summary>Background command "Check docs references to render events and read the fake auth CLI" completed (exit code 0)</summary>
</task-notification>`;
  const stopped = `<task-notification>
<task-id>bdxdmq200</task-id>
<tool-use-id>toolu_01CeX92NEHVLshRmsFsVRMX5</tool-use-id>
<status>stopped</status>
<summary>Background shell command didn't finish before the previous session ended</summary>
<note>No completion record was found for it in the previous session. It may have been stopped (via the UI, Monitor timeout, or agent teardown — these leave no transcript marker), or it may have been running when the previous Claude Code process exited. Check the output file for partial results before assuming it completed.</note>
</task-notification>`;
  const records: Record<string, unknown>[] = [];
  const add = (uuid: string, parentUuid: string | null, type: string, message: unknown, extra = {}) => {
    records.push({ uuid, parentUuid, type, message, sessionId, cwd, timestamp: `2026-10-06T11:07:${String(records.length).padStart(2, "0")}.000Z`, ...extra });
  };
  add("earlier-user", null, "user", { role: "user", content: [{ type: "text", text: "Earlier conversation" }] });
  add("earlier-answer", "earlier-user", "assistant", { id: "earlier", role: "assistant", content: [{ type: "text", text: "Earlier response" }] });
  add("boundary", null, "system", undefined, { subtype: "compact_boundary", logicalParentUuid: "earlier-answer" });
  add("compact-summary", "boundary", "user", { role: "user", content: "Internal compaction summary" }, { isCompactSummary: true });
  add("check", "compact-summary", "user", { role: "user", content: "any other places/test that had this place useless sloppy issue?" }, { origin: { kind: "human" } });
  add("tool", "check", "assistant", { id: "read", role: "assistant", content: [{ type: "tool_use", id: "read-source", name: "Read", input: { file_path: "fixture.xml" } }] });
  add("tool-result", "tool", "user", { role: "user", content: [{ type: "tool_result", tool_use_id: "read-source", content: "<source>legitimate XML tool output</source>" }] });
  add("completed", "tool-result", "user", { role: "user", content: completed }, { origin: { kind: "task-notification", producer: "session-task" } });
  add("interrupted", "completed", "user", { role: "user", content: [{ type: "text", text: "[Request interrupted by user]" }] });
  add("no-response", "interrupted", "assistant", { id: "interrupted-response", role: "assistant", content: [{ type: "text", text: "No response requested." }] });
  add("stopped", "no-response", "user", { role: "user", content: stopped }, { origin: { kind: "task-notification" } });
  add("hello", "stopped", "user", { role: "user", content: [{ type: "text", text: "hello? does this session onyl hold two messages?" }] }, { origin: { kind: "human" } });
  add("answer", "hello", "assistant", { id: "answer", role: "assistant", content: [{ type: "text", text: "No, the session holds everything." }] });
  add("user-xml", "answer", "user", { role: "user", content: [{ type: "text", text: "<task-notification>user-provided XML</task-notification>" }] }, { origin: { kind: "human" } });
  const transcriptPath = join(projectDir, `${sessionId}.jsonl`);
  const transcript = `${records.map(record => JSON.stringify(record)).join("\n")}\n`;
  writeFileSync(transcriptPath, transcript);
  const { bridge, cleanup } = ultracodeFixtureBridge({
    CLAUDE_CONFIG_DIR: configDir,
    CLAUDE_CODE_PROJECT_DIR_NAME: undefined,
    RESUME_TRANSCRIPT_FIXTURE: "1",
    STARTUP_SESSIONS_FIXTURE: JSON.stringify([{ sessionId, cwd, lastModified: 1 }]),
  });
  try {
    bridge.writeCommand({ command: "resume_session", session_id: sessionId, request_id: "resume-screenshot", launch_settings: {} });
    const connected = await nextMatching(bridge, event => event.event === "connected" || event.event === "slash_command_error");
    assertProtocolEvent(connected, "connected");
    assert.equal(connected.session_id, sessionId);
    assert.equal(connected.request_id, "resume-screenshot");
    const updates = connected.history_updates as SessionUpdate[];
    const text = updates.filter(update => update.type === "user_message_chunk" || update.type === "agent_message_chunk");
    assert.deepEqual(text.map(update => [update.type, update.source_message_uuid, update.content]), [
      ["user_message_chunk", "earlier-user", { type: "text", text: "Earlier conversation" }],
      ["agent_message_chunk", "earlier-answer", { type: "text", text: "Earlier response" }],
      ["user_message_chunk", "check", { type: "text", text: "any other places/test that had this place useless sloppy issue?" }],
      ["user_message_chunk", "interrupted", { type: "text", text: "[Request interrupted by user]" }],
      ["agent_message_chunk", "no-response", { type: "text", text: "No response requested." }],
      ["user_message_chunk", "hello", { type: "text", text: "hello? does this session onyl hold two messages?" }],
      ["agent_message_chunk", "answer", { type: "text", text: "No, the session holds everything." }],
      ["user_message_chunk", "user-xml", { type: "text", text: "<task-notification>user-provided XML</task-notification>" }],
    ]);
    assert.deepEqual(updates.filter(update => update.type === "message_metadata").map(update => update.source_message_uuid), ["earlier-user", "earlier-answer", "check", "tool", "tool-result", "interrupted", "no-response", "hello", "answer", "user-xml"]);
    const tools = updates.filter(update => update.type === "tool_call");
    assert.equal(tools.length, 1);
    assert.equal(tools[0].tool_call.tool_call_id, "read-source");
    assert.equal(tools[0].tool_call.status, "in_progress");
    const result = updates.find(update => update.type === "tool_call_update" && update.tool_call_update.tool_call_id === "read-source");
    assert.ok(result?.type === "tool_call_update");
    assert.equal(result.tool_call_update.fields.status, "completed");
    assert.equal(result.tool_call_update.fields.raw_output, "<source>legitimate XML tool output</source>");
    assert.equal(readFileSync(transcriptPath, "utf8"), transcript);
  } finally {
    await cleanup();
    assert.equal(dirname(directory), realpathSync(tmpdir()));
    rmSync(directory, { recursive: true, force: true });
  }
});

test("spawned bridge restores normalized command inventory after repeated clear and a new session", async () => {
  const { bridge, cleanup } = ultracodeFixtureBridge({ SLASH_COMMANDS_FIXTURE: "1" });
  const commandUpdate = (event: BridgeEnvelope) => (event.update as BridgeEnvelope)?.type === "available_commands_update";
  try {
    bridge.writeCommand({ command: "create_session", cwd: process.cwd(), launch_settings: {} });
    const connected = await nextMatching(bridge, event => event.event === "connected");
    let sessionId = connected.session_id;
    const initial = await nextMatching(bridge, commandUpdate);
    const bootstrapCommands = (initial.update as BridgeEnvelope).commands;
    assert.deepEqual(bootstrapCommands, [
      { name: "clear", description: "Clear conversation", input_hint: "[name]", aliases: ["reset", "new"], builtin: true },
      { name: "deploy", description: "Deploy", input_hint: "<target>", aliases: ["ship"] },
      { name: "removed-plugin", description: "Old plugin" },
    ]);
    bridge.writeCommand({ command: "prompt", session_id: sessionId, message_uuid: "command-refresh", chunks: [{ kind: "text", value: "/fixture-refresh-commands" }] });
    const dynamic = await nextMatching(bridge, commandUpdate);
    const authoritative = dynamic.update as BridgeEnvelope;
    assert.equal(authoritative.source, "commands_changed");
    assert.equal(authoritative.generation, 2);
    assert.deepEqual(authoritative.commands, [
      ...(bootstrapCommands as unknown[]).slice(0, 2),
      { name: "current-plugin", description: "Current plugin", input_hint: "[optional reason]" },
    ]);
    for (let iteration = 1; iteration <= 2; iteration++) {
      bridge.writeCommand({ command: "prompt", session_id: sessionId, message_uuid: `clear-${iteration}`, chunks: [{ kind: "text", value: "/clear" }] });
      await nextMatching(bridge, event => (event.update as BridgeEnvelope)?.type === "conversation_reset");
      const replaced = await nextMatching(bridge, event => event.event === "session_replaced");
      assert.equal(replaced.session_id, `fixture-cleared-${iteration}`);
      sessionId = replaced.session_id;
      const replay = await nextMatching(bridge, commandUpdate);
      assert.equal(replay.session_id, sessionId);
      assert.deepEqual(replay.update, authoritative);
    }
    // A genuinely new query must receive its own inventory rather than the old cache.
    bridge.writeCommand({ command: "new_session", cwd: process.cwd(), launch_settings: {} });
    const fresh = await nextMatching(bridge, event => event.event === "session_replaced");
    assert.notEqual(fresh.session_id, sessionId);
    const freshCommands = await nextMatching(bridge, commandUpdate);
    assert.equal(freshCommands.session_id, fresh.session_id);
    assert.deepEqual(freshCommands.update, initial.update);
  } finally {
    await cleanup();
  }
});

test("startup overrides and continue use the production session lifecycle", async () => {
  const directory = mkdtempSync(join(tmpdir(), "claude-rs-startup-"));
  const journal = join(directory, "query-options.jsonl");
  const cwd = process.cwd();
  const { bridge, cleanup } = ultracodeFixtureBridge({
    STARTUP_JOURNAL: journal,
    STARTUP_SESSIONS_FIXTURE: JSON.stringify([
      { sessionId: "older", cwd, lastModified: 1 },
      { sessionId: "newest-here", cwd, lastModified: 10 },
      { sessionId: "other-project", cwd: join(cwd, "other"), lastModified: 20 },
    ]),
  });
  try {
    bridge.writeCommand({ command: "create_session", cwd, continue_session: true, launch_settings: { model: "opus", permission_mode: "plan", effort: "max", agent: "reviewer" } });
    const connected = await nextMatching(bridge, event => event.event === "connected");
    assert.equal(connected.session_id, "newest-here");
    const entries = readFileSync(journal, "utf8").trim().split("\n").map(line => JSON.parse(line) as BridgeEnvelope);
    assert.deepEqual(entries.find(entry => entry.type === "query"), { type: "query", cwd, resume: "newest-here", model: "opus", effort: "max", permissionMode: "plan", agent: "reviewer" });
    const list = entries.find(entry => entry.type === "list");
    assert.deepEqual(list?.options, { dir: cwd, includeProgrammatic: true, includeWorktrees: true, limit: 50 });
  } finally {
    await cleanup();
    rmSync(directory, { recursive: true, force: true });
  }
});

test("continue with no previous session starts a new session", async () => {
  const { bridge, cleanup } = ultracodeFixtureBridge();
  try {
    bridge.writeCommand({ command: "create_session", cwd: process.cwd(), continue_session: true, launch_settings: {} });
    const connected = await nextMatching(bridge, event => event.event === "connected");
    assert.equal(typeof connected.session_id, "string");
    assert.notEqual(connected.session_id, "");
  } finally { await cleanup(); }
});

test("spawned bridge reports applied effort at startup, after caps and model changes, per turn and after resume", async () => {
  const { bridge, cleanup } = ultracodeFixtureBridge({ EFFORT_FIXTURE_CAP: "high" });
  const nextEffort = async (value: unknown) => {
    const event = await nextMatching(bridge, event => {
      const update = event.update as BridgeEnvelope | undefined;
      return update?.type === "config_option_update" && update.option_id === "effortLevel" && update.value === value;
    });
    return event.session_id;
  };
  try {
    bridge.writeCommand({ command: "create_session", cwd: process.cwd(), launch_settings: { model: "opus" } });
    const connected = await nextMatching(bridge, event => event.event === "connected");
    assert.equal(await nextEffort("high"), connected.session_id);
    bridge.writeCommand({ command: "set_effort", session_id: connected.session_id, effort: "max" });
    assert.equal(await nextEffort("high"), connected.session_id);
    bridge.writeCommand({ command: "set_model", session_id: connected.session_id, model: "haiku" });
    assert.equal(await nextEffort(null), connected.session_id);
    bridge.writeCommand({ command: "set_model", session_id: connected.session_id, model: "opus" });
    assert.equal(await nextEffort("high"), connected.session_id);
    bridge.writeCommand({ command: "prompt", session_id: connected.session_id, message_uuid: "per-turn", chunks: [{ kind: "text", value: "fixture per-turn effort" }] });
    assert.equal(await nextEffort("xhigh"), connected.session_id);
    bridge.writeCommand({ command: "resume_session", session_id: connected.session_id, launch_settings: { model: "opus" } });
    const resumed = await nextMatching(bridge, event => event.event === "session_replaced");
    assert.equal(await nextEffort("high"), resumed.session_id);
    bridge.writeCommand({ command: "new_session", cwd: process.cwd(), launch_settings: { model: "opus" } });
    const replacement = await nextMatching(bridge, event => event.event === "session_replaced");
    assert.notEqual(replacement.session_id, connected.session_id);
    assert.equal(await nextEffort("high"), replacement.session_id);
  } finally { await cleanup(); }
});

test("spawned bridge Ultracode workflow covers enable, effort, model, reset, resume and replacement", async () => {
  const { bridge, cleanup } = ultracodeFixtureBridge();
  try {
    bridge.writeCommand({ command: "create_session", cwd: process.cwd(), launch_settings: { model: "opus" } });
    const connected = await nextMatching(bridge, event => event.event === "connected");
    const sessionId = connected.session_id;
    assert.deepEqual(connected.ultracode, { available: true, requested: false, effective: false });
    const active = { available: true, requested: true, effective: true };
    bridge.writeCommand({ command: "set_ultracode", session_id: sessionId, enabled: true });
    // Back-to-back commands exercise the production per-session scheduler.
    bridge.writeCommand({ command: "set_effort", session_id: sessionId, effort: "low" });
    assert.deepEqual(await nextUltracode(bridge), active);
    assert.deepEqual(await nextUltracode(bridge), active);
    const effort = await nextMatching(bridge, event => (event.update as BridgeEnvelope)?.type === "config_option_update" && (event.update as BridgeEnvelope).value === "low");
    assert.equal((effort.update as BridgeEnvelope).value, "low");
    bridge.writeCommand({ command: "prompt", session_id: sessionId, message_uuid: "reset-message", chunks: [{ kind: "text", value: "fixture conversation reset" }] });
    await nextMatching(bridge, event => (event.update as BridgeEnvelope)?.type === "conversation_reset");
    assert.deepEqual(await nextUltracode(bridge), active);
    bridge.writeCommand({ command: "set_model", session_id: sessionId, model: "haiku" });
    assert.deepEqual(await nextUltracode(bridge), { available: false, requested: true, effective: false });
    bridge.writeCommand({ command: "set_ultracode", session_id: sessionId, enabled: true, request_id: "unsupported-model" });
    const unsupported = await nextMatching(bridge, event => event.event === "slash_error");
    assert.equal(unsupported.request_id, "unsupported-model");
    assert.equal(unsupported.message, "Cannot enable Ultracode: haiku does not support it.");
    bridge.writeCommand({ command: "set_model", session_id: sessionId, model: "opus" });
    assert.deepEqual(await nextUltracode(bridge), active);
    bridge.writeCommand({ command: "resume_session", session_id: sessionId, launch_settings: { model: "opus" } });
    const resumed = await nextMatching(bridge, event => event.event === "session_replaced");
    assert.equal(resumed.session_id, sessionId);
    assert.deepEqual(resumed.ultracode, active);
    bridge.writeCommand({ command: "set_ultracode", session_id: sessionId, enabled: false });
    assert.deepEqual(await nextUltracode(bridge), { available: true, requested: false, effective: false });
    bridge.writeCommand({ command: "new_session", cwd: process.cwd(), launch_settings: { model: "opus" } });
    const replaced = await nextMatching(bridge, event => event.event === "session_replaced");
    assert.notEqual(replaced.session_id, sessionId);
    assert.deepEqual(replaced.ultracode, { available: true, requested: false, effective: false });
    bridge.writeCommand({ command: "refresh_ultracode", session_id: sessionId, request_id: "closed-session" });
    const closed = await nextMatching(bridge, event => event.event === "slash_error");
    assert.equal(closed.request_id, "closed-session");
    assert.equal(closed.message, "Cannot change Ultracode: no active session.");
    bridge.writeCommand({ command: "shutdown" });
    assert.equal(await bridge.waitForExit(), 0);
  } finally { await cleanup(); }
});

test("spawned bridge reports accepted inactive Ultracode requests and preserves verified state", async () => {
  const { bridge, cleanup } = ultracodeFixtureBridge();
  try {
    bridge.writeCommand({ command: "create_session", cwd: process.cwd(), launch_settings: { model: "fixture-unavailable" } });
    const connected = await nextMatching(bridge, event => event.event === "connected");
    const sessionId = connected.session_id;
    assert.deepEqual(connected.ultracode, { available: false, requested: false, effective: false });
    const requested = { available: false, requested: true, effective: false };
    for (const requestId of ["inactive-enable", "inactive-retry"]) {
      bridge.writeCommand({ command: "set_ultracode", session_id: sessionId, enabled: true, request_id: requestId });
      assert.deepEqual(await nextUltracode(bridge), requested);
      const error = await nextMatching(bridge, event => event.event === "slash_error");
      assert.equal(error.event, "slash_error");
      assert.equal(error.session_id, sessionId);
      assert.equal(error.request_id, requestId);
      assert.equal(error.message, "Cannot enable Ultracode: unavailable for this session. The SDK saved the request, but Ultracode remains inactive.");
    }
    bridge.writeCommand({ command: "refresh_ultracode", session_id: sessionId });
    assert.deepEqual(await nextUltracode(bridge), requested);
    bridge.writeCommand({ command: "set_ultracode", session_id: sessionId, enabled: false });
    assert.deepEqual(await nextUltracode(bridge), { available: false, requested: false, effective: false });
    bridge.writeCommand({ command: "shutdown" });
    assert.equal(await bridge.waitForExit(), 0);
  } finally { await cleanup(); }
});

test("spawned bridge Ultracode failures report disabled workflows and clear unverifiable state", async () => {
  for (const workflows of ["off", "on"]) {
    const { bridge, cleanup } = ultracodeFixtureBridge({ ULTRACODE_FIXTURE_WORKFLOWS: workflows });
    try {
      bridge.writeCommand({ command: "create_session", cwd: process.cwd(), launch_settings: { model: "opus" } });
      const connected = await nextMatching(bridge, event => event.event === "connected");
      const sessionId = connected.session_id;
      if (workflows === "off") {
        assert.deepEqual(connected.ultracode, { available: false, requested: false, effective: false });
        bridge.writeCommand({ command: "set_ultracode", session_id: sessionId, enabled: true });
        const error = await nextMatching(bridge, event => event.event === "slash_error");
        assert.equal(error.message, "Cannot enable Ultracode: dynamic workflows are disabled for this session.");
      } else {
        bridge.writeCommand({ command: "set_ultracode", session_id: sessionId, enabled: true });
        await nextUltracode(bridge);
        bridge.writeCommand({ command: "set_model", session_id: sessionId, model: "fixture-read-failure" });
        assert.equal(await nextUltracode(bridge), null);
        bridge.writeCommand({ command: "set_ultracode", session_id: sessionId, enabled: false });
        assert.equal(await nextUltracode(bridge), null);
        const error = await nextMatching(bridge, event => event.event === "slash_error");
        assert.equal(error.message, "The SDK accepted the Ultracode change, but its resulting state could not be verified.");
        assert.ok(bridge.stderrLines.some(line => line.includes("fixture read failed")));
      }
    } finally { await cleanup(); }
  }
});


test("SDK settings inheritance applies user, project and local preferences across create, resume and replacement", async () => {
  const directory = mkdtempSync(join(tmpdir(), "claude-rs-settings-"));
  const profile = join(directory, "profile");
  const cwd = join(directory, "project");
  mkdirSync(profile);
  mkdirSync(join(cwd, ".claude"), { recursive: true });
  const journal = join(directory, "query-options.jsonl");
  const user = {
    model: "haiku", alwaysThinkingEnabled: true, fastMode: true,
    outputStyle: "User custom style", language: "German", fileCheckpointingEnabled: true,
    sandbox: { enabled: true, failIfUnavailable: true },
    modelSettings: { "claude-opus-5-5": { effortLevel: "low" } },
  };
  const project = { model: "sonnet", language: "Japanese", permissions: { defaultMode: "plan" } };
  const local = {
    model: "opus", alwaysThinkingEnabled: false, fastMode: false,
    outputStyle: "Project custom style", fileCheckpointingEnabled: false,
    modelSettings: { "claude-opus-5-5": { effortLevel: "xhigh" } },
    crossSessionInbound: "refuse",
  };
  const userPath = join(profile, "settings.json");
  const projectPath = join(cwd, ".claude", "settings.json");
  const localPath = join(cwd, ".claude", "settings.local.json");
  for (const [path, value] of [[userPath, user], [projectPath, project], [localPath, local]] as const) {
    writeFileSync(path, JSON.stringify(value));
  }
  const savedFiles = [userPath, projectPath, localPath].map(path => readFileSync(path, "utf8"));
  const { bridge, cleanup } = ultracodeFixtureBridge({
    CLAUDE_CONFIG_DIR: profile, STARTUP_JOURNAL: journal, SETTINGS_JOURNAL: "1",
  });
  const snapshots = () => readFileSync(journal, "utf8").trim().split("\n")
    .map(line => JSON.parse(line) as BridgeEnvelope).filter(entry => entry.type === "settings");
  try {
    bridge.writeCommand({ command: "create_session", cwd });
    const connected = await nextMatching(bridge, event => event.event === "connected");
    bridge.writeCommand({ command: "resume_session", session_id: connected.session_id });
    await nextMatching(bridge, event => event.event === "session_replaced");
    bridge.writeCommand({ command: "new_session", cwd });
    await nextMatching(bridge, event => event.event === "session_replaced");
    assert.equal(snapshots().length, 3);
    for (const snapshot of snapshots()) {
      const effective = snapshot.effective as BridgeEnvelope;
      assert.equal(effective.model, "opus");
      assert.equal(effective.alwaysThinkingEnabled, false);
      assert.equal(effective.fastMode, false);
      assert.equal(effective.outputStyle, "Project custom style");
      assert.equal(effective.language, "Japanese");
      assert.deepEqual(effective.permissions, { defaultMode: "plan" });
      assert.deepEqual(effective.sandbox, { enabled: true, failIfUnavailable: true });
      assert.deepEqual(effective.modelSettings, { "claude-opus-5-5": { effortLevel: "xhigh" } });
      assert.equal(snapshot.checkpointing, false);
    }
    assert.deepEqual([userPath, projectPath, localPath].map(path => readFileSync(path, "utf8")), savedFiles);
    writeFileSync(localPath, JSON.stringify({ ...local, fileCheckpointingEnabled: true }));
    bridge.writeCommand({ command: "new_session", cwd });
    await nextMatching(bridge, event => event.event === "session_replaced");
    assert.equal(snapshots().at(-1)?.checkpointing, true);
  } finally {
    await cleanup();
    rmSync(directory, { recursive: true, force: true });
  }
});

test("spawned bridge inspects, saves, conflicts and resets scoped settings over NDJSON", async () => {
  const directory = mkdtempSync(join(tmpdir(), "claude-rs-config-flow-"));
  const profile = join(directory, "profile");
  const cwd = join(directory, "project");
  mkdirSync(profile);
  mkdirSync(join(cwd, ".claude"), { recursive: true });
  const userPath = join(profile, "settings.json");
  const localPath = join(cwd, ".claude", "settings.local.json");
  writeFileSync(userPath, JSON.stringify({ language: "German", fileCheckpointingEnabled: false }));
  writeFileSync(localPath, JSON.stringify({ language: "Japanese", future: { keep: true } }));
  const { bridge, cleanup } = ultracodeFixtureBridge({ CLAUDE_CONFIG_DIR: profile });
  try {
    bridge.writeCommand({ command: "create_session", cwd });
    const connected = await nextMatching(bridge, event => event.event === "connected");
    const session_id = connected.session_id;
    async function result(request_id: string, command: BridgeEnvelope): Promise<SettingsResult> {
      bridge.writeCommand({ ...command, session_id, request_id });
      const event = await nextMatching(bridge, event => event.event === "settings_result");
      assert.equal(event.request_id, request_id);
      assert.equal(event.session_id, session_id);
      return event.result as SettingsResult;
    }
    const inspected = await result("inspect", { command: "inspect_settings" });
    assert.equal(inspected.persistence, "not_requested");
    assert.ok(inspected.snapshot);
    assert.equal(inspected.snapshot.values.find(value => value.id === "language")?.value, "Japanese");
    assert.equal(inspected.snapshot.values.find(value => value.id === "fileCheckpointingEnabled")?.value, false);
    function change(snapshot: SettingsSnapshot, value?: string): SettingsMutation {
      const previous = snapshot.sources.find(source => source.scope === "local")?.values.find(value => value.id === "language");
      assert.ok(previous);
      return { context: snapshot.context, id: "language", scope: "local", expected_revision: previous.revision, operation: value === undefined ? "remove" : "set", ...(value === undefined ? {} : { value }) };
    }
    // A change to another owner's key between display and save must survive.
    writeFileSync(localPath, JSON.stringify({ language: "Japanese", future: { keep: true }, external: "preserve" }));
    const saved = await result("save", { command: "mutate_setting", mutation: change(inspected.snapshot, "Greek") });
    assert.equal(saved.persistence, "saved");
    assert.equal(saved.application, "next_session");
    assert.ok(saved.snapshot);
    assert.equal(saved.snapshot.values.find(value => value.id === "language")?.value, "Greek");
    assert.deepEqual(JSON.parse(readFileSync(localPath, "utf8")), { language: "Greek", future: { keep: true }, external: "preserve" });
    writeFileSync(localPath, JSON.stringify({ language: "French", future: { keep: true }, external: "preserve" }));
    const conflict = await result("conflict", { command: "mutate_setting", mutation: change(saved.snapshot, "Italian") });
    assert.equal(conflict.persistence, "conflict");
    assert.ok(conflict.snapshot);
    assert.equal(conflict.snapshot.values.find(value => value.id === "language")?.value, "French");
    const reset = await result("reset", { command: "mutate_setting", mutation: change(conflict.snapshot) });
    assert.equal(reset.persistence, "saved");
    assert.equal(reset.snapshot?.values.find(value => value.id === "language")?.value, "German");
    assert.deepEqual(JSON.parse(readFileSync(localPath, "utf8")), { future: { keep: true }, external: "preserve" });
    for (const [id, value] of [
      ["permissions.deny", ["Read(./.env)"]],
      ["worktree.sparsePaths", ["src", "docs"]],
      ["hooks", { Stop: [{ hooks: [{ type: "command", command: "unused-preview-command" }] }] }],
    ] as const) {
      const fresh = await result(`inspect-${id}`, { command: "inspect_settings" });
      assert.ok(fresh.snapshot);
      const scoped = fresh.snapshot.sources.find(source => source.scope === "local")?.values.find(entry => entry.id === id);
      assert.ok(scoped);
      const mutation = { context: fresh.snapshot.context, id, scope: "local", expected_revision: scoped.revision, operation: "set", value };
      const saved = await result(`save-${id}`, { command: "mutate_setting", mutation });
      assert.equal(saved.persistence, "saved", saved.error ?? id);
      assert.deepEqual(saved.snapshot?.values.find(entry => entry.id === id)?.value, value);
    }
    const document = JSON.parse(readFileSync(localPath, "utf8"));
    assert.deepEqual(document.future, { keep: true });
    assert.equal(document.external, "preserve");
    assert.deepEqual(document.permissions.deny, ["Read(./.env)"]);
    bridge.writeCommand({ command: "shutdown" });
    assert.equal(await bridge.waitForExit(), 0);
  } finally {
    await cleanup();
    rmSync(directory, { recursive: true, force: true });
  }
});

test("spawned bridge displays SDK managed policy and blocks scoped edits to its values", async () => {
  const directory = mkdtempSync(join(tmpdir(), "claude-rs-policy-flow-"));
  const profile = join(directory, "profile");
  const cwd = join(directory, "project");
  mkdirSync(profile);
  mkdirSync(cwd);
  const file = join(profile, "settings.json");
  const original = JSON.stringify({ language: "German", unrelated: true });
  writeFileSync(file, original);
  const { bridge, cleanup } = ultracodeFixtureBridge({ CLAUDE_CONFIG_DIR: profile, SETTINGS_POLICY_FIXTURE: JSON.stringify({ language: "Policy language", allowManagedPermissionRulesOnly: true, allowManagedHooksOnly: true, sandbox: { network: { allowManagedDomainsOnly: true }, filesystem: { allowManagedReadPathsOnly: true } } }) });
  try {
    bridge.writeCommand({ command: "create_session", cwd });
    const connected = await nextMatching(bridge, event => event.event === "connected");
    bridge.writeCommand({ command: "inspect_settings", session_id: connected.session_id, request_id: "policy-inspect" });
    const inspected = await nextMatching(bridge, event => event.event === "settings_result");
    const snapshot = (inspected.result as SettingsResult).snapshot;
    assert.ok(snapshot);
    assert.equal(snapshot.values.find(value => value.id === "language")?.value, "Policy language");
    assert.equal(snapshot.values.find(value => value.id === "language")?.policy_restricted, true);
    assert.equal(snapshot.provenance.language?.source, "managed");
    assert.ok(snapshot.resolution_sources.some(source => source.source === "managed"));
    assert.equal(snapshot.resolution_sources.find(source => source.source === "managed")?.policy_origin, "remote");
    assert.deepEqual(snapshot.catalog.find(setting => setting.id === "language")?.writable_scopes, []);
    for (const id of ["permissions.allow", "permissions.ask", "permissions.deny", "hooks", "sandbox.network.allowedDomains", "sandbox.network.httpProxyPort", "sandbox.network.socksProxyPort", "sandbox.filesystem.allowRead"]) {
      assert.equal(snapshot.values.find(value => value.id === id)?.policy_restricted, true, id);
      assert.deepEqual(snapshot.catalog.find(setting => setting.id === id)?.writable_scopes, [], id);
      const saved: SettingsSnapshot["sources"][number]["values"][number] | undefined = snapshot.sources.find(source => source.scope === "user")?.values.find(value => value.id === id);
      assert.ok(saved);
      bridge.writeCommand({ command: "mutate_setting", session_id: connected.session_id, request_id: id, mutation: { context: snapshot.context, id, scope: "user", expected_revision: saved.revision, operation: "set", value: id.endsWith("ProxyPort") ? 3128 : id === "hooks" ? {} : [] } });
      const rejected = await nextMatching(bridge, event => event.event === "settings_result");
      assert.equal((rejected.result as SettingsResult).persistence, "failure", id);
    }
    assert.deepEqual(snapshot.catalog.find(setting => setting.id === "permissions.additionalDirectories")?.writable_scopes, ["user", "project", "local"]);
    const previous = snapshot.sources.find(source => source.scope === "user")?.values.find(value => value.id === "language");
    assert.ok(previous);
    bridge.writeCommand({ command: "mutate_setting", session_id: connected.session_id, request_id: "policy-save", mutation: { context: snapshot.context, id: "language", scope: "user", expected_revision: previous.revision, operation: "set", value: "French" } });
    const rejected = await nextMatching(bridge, event => event.event === "settings_result");
    assert.equal(rejected.request_id, "policy-save");
    assert.equal((rejected.result as SettingsResult).persistence, "failure");
    assert.equal(readFileSync(file, "utf8"), original);
  } finally {
    await cleanup();
    rmSync(directory, { recursive: true, force: true });
  }
});


test("spawned bridge keeps saved model defaults separate from acknowledged session controls and new sessions", async () => {
  const directory = mkdtempSync(join(tmpdir(), "claude-rs-session-settings-"));
  const profile = join(directory, "profile");
  const cwd = join(directory, "project");
  mkdirSync(profile); mkdirSync(cwd);
  const settingsFile = join(profile, "settings.json");
  const persisted = { model: "opus", modelSettings: { "claude-opus-5-5": { effortLevel: "low" } }, alwaysThinkingEnabled: true, agent: "reviewer", fastMode: false, permissions: { defaultMode: "plan" } };
  writeFileSync(settingsFile, JSON.stringify(persisted));
  const { bridge, cleanup } = ultracodeFixtureBridge({ CLAUDE_CONFIG_DIR: profile, SESSION_SETTINGS_FIXTURE: "1", MODEL_STEP_DOWN_FIXTURE: "1", FAST_READ_FAILURE_FIXTURE: "1" });
  const nextUpdate = async (type: string, option?: string) => (await nextMatching(bridge, event => {
    const update = event.update as BridgeEnvelope | undefined;
    return update?.type === type && (!option || update.option_id === option);
  })).update as BridgeEnvelope;
  try {
    bridge.writeCommand({ command: "create_session", cwd, launch_settings: {} });
    const connected = await nextMatching(bridge, event => event.event === "connected");
    const sessionId = connected.session_id;
    assert.equal((connected.mode as BridgeEnvelope).current_mode_name, "Plan", "report the SDK's starting mode before any prompt or /mode command");
    assert.ok(((connected.mode as BridgeEnvelope).available_modes as BridgeEnvelope[]).some(mode => mode.id === "plan"));
    assert.ok(((connected.mode as BridgeEnvelope).available_modes as BridgeEnvelope[]).some(mode => mode.id === "auto"));
    assert.deepEqual((await nextUpdate("available_agents_update")).agents, [{ name: "reviewer", description: "Review code", model: "sonnet" }], "publish the SDK inventory after connection so the host can keep it");
    assert.equal((await nextUpdate("config_option_update", "effortLevel")).value, "low");
    bridge.writeCommand({ command: "set_mode", session_id: sessionId, mode: "auto", request_id: "restricted-auto" });
    const restrictedMode = (await nextUpdate("mode_state_update")).mode as BridgeEnvelope;
    assert.equal(restrictedMode.current_mode_name, "Plan");
    assert.ok(!(restrictedMode.available_modes as BridgeEnvelope[]).some(mode => mode.id === "auto"));
    const restrictedError = await nextMatching(bridge, event => event.event === "slash_error");
    assert.equal(restrictedError.request_id, "restricted-auto");
    assert.match(String(restrictedError.message), /account restriction/);
    bridge.writeCommand({ command: "prompt", session_id: sessionId, message_uuid: "observe-mode", chunks: [{ kind: "text", value: "fixture observe permissions" }] });
    assert.equal(((await nextUpdate("mode_state_update")).mode as BridgeEnvelope).current_mode_id, "plan");
    for (const [enabled, value] of [[false, false], [null, true]]) {
      bridge.writeCommand({ command: "set_thinking", session_id: sessionId, enabled });
      assert.equal((await nextUpdate("config_option_update", "alwaysThinkingEnabled")).value, value);
    }
    bridge.writeCommand({ command: "set_effort", session_id: sessionId, effort: "max" });
    assert.equal((await nextUpdate("config_option_update", "effortLevel")).value, "max");
    bridge.writeCommand({ command: "set_effort", session_id: sessionId, effort: null });
    assert.equal((await nextUpdate("config_option_update", "effortLevel")).value, "high", "native reset differs from the saved default");
    bridge.writeCommand({ command: "set_model", session_id: sessionId, model: "opus" });
    const appliedModel = (await nextUpdate("current_model_update")).current_model as BridgeEnvelope;
    assert.equal(appliedModel.resolved_id, "claude-sonnet-5-5", "report SDK step-down rather than the requested alias");
    await nextUpdate("mode_state_update");
    bridge.writeCommand({ command: "set_fast_mode", session_id: sessionId, enabled: true });
    assert.equal((await nextUpdate("fast_mode_update")).fast_mode_state, "unknown");
    await nextMatching(bridge, event => event.event === "slash_error");
    bridge.writeCommand({ command: "set_fast_mode", session_id: sessionId, enabled: false });
    assert.equal((await nextUpdate("fast_mode_update")).fast_mode_state, "off");
    bridge.writeCommand({ command: "set_agent", session_id: sessionId, agent: "missing" });
    await nextMatching(bridge, event => event.event === "slash_error");
    bridge.writeCommand({ command: "set_agent", session_id: sessionId, agent: "reviewer" });
    assert.equal((await nextUpdate("config_option_update", "agent")).value, "reviewer");
    await nextUpdate("mode_state_update");
    bridge.writeCommand({ command: "set_mode", session_id: sessionId, mode: "plan" });
    assert.equal(((await nextUpdate("mode_state_update")).mode as BridgeEnvelope).current_mode_id, "plan");
    assert.deepEqual(JSON.parse(readFileSync(settingsFile, "utf8")), persisted);
    bridge.writeCommand({ command: "new_session", cwd, launch_settings: {} });
    const replacement = await nextMatching(bridge, event => event.event === "session_replaced");
    assert.notEqual(replacement.session_id, sessionId);
    assert.equal(((await nextUpdate("available_agents_update")).agents as BridgeEnvelope[])[0].name, "reviewer");
    assert.equal((await nextUpdate("config_option_update", "effortLevel")).value, "low");
    bridge.writeCommand({ command: "set_model", session_id: replacement.session_id, model: "haiku" });
    const unsupported = (await nextUpdate("current_model_update")).current_model as BridgeEnvelope;
    assert.equal(unsupported.supports_effort, false);
    await nextUpdate("mode_state_update");
    bridge.writeCommand({ command: "set_effort", session_id: replacement.session_id, effort: "low" });
    const effortError = await nextMatching(bridge, event => event.event === "slash_error");
    assert.match(String(effortError.message), /supported by the current model/);
    bridge.writeCommand({ command: "set_fast_mode", session_id: replacement.session_id, enabled: true });
    const fastError = await nextMatching(bridge, event => event.event === "slash_error");
    assert.match(String(fastError.message), /does not support fast mode/);
    assert.deepEqual(JSON.parse(readFileSync(settingsFile, "utf8")), persisted);
  } finally { await cleanup(); rmSync(directory, { recursive: true, force: true }); }
});


test("spawned bridge publishes the native starting mode independently of saved mode defaults", async () => {
  const directory = mkdtempSync(join(tmpdir(), "claude-rs-mode-observation-"));
  const profile = join(directory, "profile"); const cwd = join(directory, "project");
  mkdirSync(profile); mkdirSync(cwd);
  writeFileSync(join(profile, "settings.json"), JSON.stringify({ permissions: { defaultMode: "plan" } }));
  const { bridge, cleanup } = ultracodeFixtureBridge({ CLAUDE_CONFIG_DIR: profile, SESSION_SETTINGS_FIXTURE: "1", PERMISSION_MODE_FIXTURE: "auto" });
  try {
    bridge.writeCommand({ command: "create_session", cwd, launch_settings: {} });
    const connected = await nextMatching(bridge, event => event.event === "connected");
    assert.equal((connected.mode as BridgeEnvelope).current_mode_id, "auto");
    assert.equal((connected.mode as BridgeEnvelope).current_mode_name, "Auto");
    bridge.writeCommand({ command: "inspect_settings", session_id: connected.session_id });
    const result = await nextMatching(bridge, event => event.event === "settings_result");
    const shown = (result.result as SettingsResult).snapshot;
    assert.equal(shown?.values.find(value => value.id === "permissions.defaultMode")?.value, "plan");
    assert.ok(shown?.catalog.find(setting => setting.id === "prefersReducedMotion")?.writable_scopes.includes("user"));
    bridge.writeCommand({ command: "new_session", cwd, launch_settings: {} });
    const replacement = await nextMatching(bridge, event => event.event === "session_replaced");
    assert.equal((replacement.mode as BridgeEnvelope).current_mode_id, "auto");
  } finally { await cleanup(); rmSync(directory, { recursive: true, force: true }); }
});
