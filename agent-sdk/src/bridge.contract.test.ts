import test from "node:test";
import assert from "node:assert/strict";
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { appendFileSync, mkdtempSync, mkdirSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import readline from "node:readline";
import { fileURLToPath, pathToFileURL } from "node:url";
import type { SettingsMutation, SettingsResult, SettingsSnapshot } from "./types.js";
import { Readable, Writable } from "node:stream";
import { validateSettingValue } from "./bridge/settings_values.js";
import type { SessionUpdate } from "./types.js";

type BridgeEnvelope = Record<string, unknown>;

const bridgePath = join(dirname(fileURLToPath(import.meta.url)), "bridge.js");

class SpawnedBridge {
  readonly child: ChildProcessWithoutNullStreams;
  readonly stderrLines: string[] = [];
  readonly events: BridgeEnvelope[] = [];
  readonly cwd: string;
  readonly directory: string;
  #stdoutQueue: BridgeEnvelope[] = [];
  #stdoutWaiters: Array<{
    resolve: (envelope: BridgeEnvelope) => void;
    reject: (error: Error) => void;
  }> = [];
  #exitCode: number | null | undefined;
  #failure: Error | undefined;
  #closed: Promise<number | null>;
  #stopping = false;
  #stop: Promise<void> | undefined;

  constructor(env: NodeJS.ProcessEnv = {}, nodeArgs: string[] = [], controlPipes = false) {
    this.directory = realpathSync(mkdtempSync(join(tmpdir(), "bridge-contract-")));
    this.cwd = join(this.directory, "project");
    mkdirSync(this.cwd);
    mkdirSync(join(this.directory, "profile"));
    const inherited = { ...process.env };
    for (const key of Object.keys(inherited))
      if (/^(ANTHROPIC_|CLAUDE_|CLAUDECODE$)/.test(key)) delete inherited[key];
    this.child = spawn(process.execPath, [...nodeArgs, bridgePath], {
      cwd: this.cwd,
      env: {
        ...inherited,
        CLAUDE_CONFIG_DIR: join(this.directory, "profile"),
        CLAUDE_RS_BRIDGE_DIAGNOSTICS: "1",
        ...env,
      },
      stdio: controlPipes ? ["pipe", "pipe", "pipe", "pipe", "pipe"] : "pipe",
      windowsHide: true,
    }) as ChildProcessWithoutNullStreams;
    const stdout = this.child.stdout;
    stdout.setEncoding("utf8");
    let buffered = "";
    const acceptLine = (line: string) => {
      let envelope: BridgeEnvelope;
      try {
        envelope = JSON.parse(line) as BridgeEnvelope;
      } catch (error) {
        this.#fail(error instanceof Error ? error : new Error(String(error)));
        return;
      }
      this.events.push(envelope);
      const waiter = this.#stdoutWaiters.shift();
      if (waiter) waiter.resolve(envelope);
      else this.#stdoutQueue.push(envelope);
    };
    stdout.on("data", (chunk: string) => {
      buffered += chunk;
      for (let newline = buffered.indexOf("\n"); newline >= 0; newline = buffered.indexOf("\n")) {
        const line = buffered.slice(0, newline);
        buffered = buffered.slice(newline + 1);
        acceptLine(line);
      }
    });
    stdout.on("end", () => {
      if (buffered) acceptLine(buffered);
    });
    const stderr = readline.createInterface({ input: this.child.stderr });
    stderr.on("line", (line) => this.stderrLines.push(line));
    this.child.on("error", (error) => this.#fail(error));
    this.child.stdin.on("error", (error) => {
      if (!this.#stopping) this.#fail(error);
    });
    this.#closed = new Promise((resolve) =>
      this.child.once("close", (code, signal) => {
        stderr.close();
        this.#exitCode = code;
        this.#rejectWaiters(
          new Error(`bridge closed before next protocol event: ${code}/${signal}`),
        );
        resolve(code);
      }),
    );
  }
  writeCommand(command: BridgeEnvelope): void {
    this.writeRaw(`${JSON.stringify(command)}\n`);
  }
  writeRaw(line: string | Uint8Array): void {
    if (this.#failure) throw this.#failure;
    this.child.stdin.write(line);
  }
  nextEnvelope(timeoutMs = 2_000): Promise<BridgeEnvelope> {
    if (this.#failure) return Promise.reject(this.#failure);
    const queued = this.#stdoutQueue.shift();
    if (queued) return Promise.resolve(queued);
    if (this.#exitCode !== undefined)
      return Promise.reject(new Error(`bridge already closed: ${this.#exitCode}`));
    return new Promise((resolve, reject) => {
      const remove = () => {
        const index = this.#stdoutWaiters.indexOf(waiter);
        if (index >= 0) this.#stdoutWaiters.splice(index, 1);
      };
      const timeout = setTimeout(() => {
        remove();
        reject(new Error("timed out waiting for bridge protocol event"));
      }, timeoutMs);
      const waiter = {
        resolve: (event: BridgeEnvelope) => {
          clearTimeout(timeout);
          remove();
          resolve(event);
        },
        reject: (error: Error) => {
          clearTimeout(timeout);
          remove();
          reject(error);
        },
      };
      this.#stdoutWaiters.push(waiter);
    });
  }
  async waitForExit(timeoutMs = 5_000): Promise<number | null> {
    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
      return await Promise.race([
        this.#closed,
        new Promise<never>((_, reject) => {
          timer = setTimeout(
            () =>
              reject(
                new Error(
                  `timed out waiting for drained bridge close: exit=${this.child.exitCode}, signal=${this.child.signalCode}; ${this.stderrLines.slice(-2).join(";")}`,
                ),
              ),
            timeoutMs,
          );
        }),
      ]);
    } finally {
      clearTimeout(timer);
    }
  }
  async stop(): Promise<void> {
    this.#stop ??= (async () => {
      this.#stopping = true;
      if (this.#exitCode === undefined) this.child.kill();
      await this.waitForExit();
      rmSync(this.directory, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
    })();
    return this.#stop;
  }
  #fail(error: Error): void {
    this.#failure ??= error;
    this.#rejectWaiters(this.#failure);
  }
  #rejectWaiters(error: Error): void {
    for (const waiter of this.#stdoutWaiters.splice(0)) waiter.reject(error);
  }
}

function controlChannel(bridge: SpawnedBridge) {
  const input = bridge.child.stdio[3],
    output = bridge.child.stdio[4];
  assert.ok(input instanceof Readable && output instanceof Writable);
  const lines = readline.createInterface({ input }),
    records: BridgeEnvelope[] = [];
  let failure: Error | undefined;
  const waiting = new Set<() => void>();
  lines.on("line", (line) => {
    try {
      records.push(JSON.parse(line));
    } catch (error) {
      failure = error instanceof Error ? error : new Error(String(error));
    }
    for (const wake of [...waiting]) wake();
  });
  lines.on("close", () => {
    failure ??= new Error("control channel closed before expected receipt");
    for (const wake of [...waiting]) wake();
  });
  const until = (predicate: (records: BridgeEnvelope[]) => boolean): Promise<void> => {
    if (predicate(records)) return Promise.resolve();
    if (failure) return Promise.reject(failure);
    return new Promise((resolve, reject) => {
      const finish = (error?: Error) => {
        clearTimeout(timer);
        waiting.delete(check);
        if (error) reject(error);
        else resolve();
      };
      const check = () => {
        if (predicate(records)) finish();
        else if (failure) finish(failure);
      };
      const timer = setTimeout(
        () => finish(new Error(`control receipt deadline: ${JSON.stringify(records)}`)),
        5_000,
      );
      waiting.add(check);
      check();
    });
  };
  return {
    records,
    until,
    send: (type: string) => {
      if (!output.destroyed) output.write(`${JSON.stringify({ type })}\n`);
    },
    close: () => {
      lines.close();
      input.destroy();
      output.destroy();
    },
  };
}

function assertProtocolEvent(
  envelope: BridgeEnvelope,
  eventName: string,
): asserts envelope is BridgeEnvelope & { event: string } {
  assert.equal(envelope.event, eventName);
}

test("confirmed background tools retain launch output and independent descendants across foreground interruption", async () => {
  for (const toolName of ["Bash", "Agent"]) {
    for (const initiallyBackgrounded of [true, false]) {
      for (const outcome of ["completed", "failed", "stopped"]) {
        const directory = realpathSync(mkdtempSync(join(tmpdir(), "background-contract-"))),
          journal = join(directory, "calls.jsonl");
        const toolId = "primary-tool";
        const taskId = "primary-task";
        const system = (subtype: string, fields: BridgeEnvelope) => ({
          type: "system",
          subtype,
          uuid: crypto.randomUUID(),
          task_id: taskId,
          ...fields,
        });
        const toolResult = (raw: BridgeEnvelope) => ({
          type: "user",
          uuid: crypto.randomUUID(),
          tool_use_result: raw,
          message: {
            role: "user",
            content: [
              { type: "tool_result", tool_use_id: toolId, content: "Launch acknowledgement" },
            ],
          },
        });
        const turnEnd = {
          type: "result",
          subtype: "success",
          is_error: false,
          duration_ms: 1,
          num_turns: 1,
          result: "Foreground finished",
          usage: {},
          modelUsage: {},
          permission_denials: [],
        };
        const fixture = {
          start: [
            {
              type: "assistant",
              message: {
                id: "assistant-1",
                role: "assistant",
                content: [
                  {
                    type: "tool_use",
                    id: toolId,
                    name: toolName,
                    input:
                      toolName === "Bash"
                        ? { command: "long command", run_in_background: initiallyBackgrounded }
                        : {
                            description: "Investigate",
                            prompt: "Investigate",
                            run_in_background: initiallyBackgrounded,
                          },
                  },
                ],
              },
            },
            system("task_started", {
              tool_use_id: toolId,
              description: "Investigate",
              task_type: toolName === "Bash" ? "local_bash" : "local_agent",
              is_backgrounded: initiallyBackgrounded,
            }),
            ...(!initiallyBackgrounded
              ? [system("task_updated", { patch: { status: "running", is_backgrounded: true } })]
              : []),
            toolResult(
              toolName === "Bash"
                ? {
                    stdout: "Launch stdout retained",
                    stderr: "",
                    interrupted: false,
                    backgroundTaskId: taskId,
                    ...(!initiallyBackgrounded ? { timedOutAfterMs: 120000 } : {}),
                  }
                : {
                    status: "async_launched",
                    isAsync: true,
                    agentId: taskId,
                    description: "Investigate",
                    prompt: "Investigate",
                    outputFile: "task.output",
                  },
            ),
            system("task_progress", {
              description: "Still working",
              usage: { total_tokens: 1, tool_uses: 1, duration_ms: 10 },
            }),
            ...(toolName === "Agent"
              ? [
                  {
                    type: "assistant",
                    parent_tool_use_id: toolId,
                    message: {
                      role: "assistant",
                      content: [
                        {
                          type: "tool_use",
                          id: "nested-agent",
                          name: "Agent",
                          input: { description: "Nested reviewer", prompt: "Review" },
                        },
                      ],
                    },
                  },
                  {
                    type: "assistant",
                    parent_tool_use_id: "nested-agent",
                    message: {
                      role: "assistant",
                      content: [
                        {
                          type: "tool_use",
                          id: "nested-bash",
                          name: "Bash",
                          input: { command: "Nested command" },
                        },
                      ],
                    },
                  },
                ]
              : []),
            {
              ...turnEnd,
              subtype: "error_during_execution",
              is_error: true,
              errors: ["Interrupted by user"],
            },
          ],
          finish: [
            ...(toolName === "Agent"
              ? [
                  {
                    type: "user",
                    parent_tool_use_id: "nested-agent",
                    message: {
                      role: "user",
                      content: [
                        {
                          type: "tool_result",
                          tool_use_id: "nested-bash",
                          content: "Nested command finished",
                        },
                      ],
                    },
                  },
                  {
                    type: "user",
                    parent_tool_use_id: toolId,
                    message: {
                      role: "user",
                      content: [
                        {
                          type: "tool_result",
                          tool_use_id: "nested-agent",
                          content: "Nested review finished",
                        },
                      ],
                    },
                  },
                ]
              : []),
            system("task_updated", {
              patch: { status: outcome === "stopped" ? "killed" : outcome, end_time: 10 },
            }),
            system("task_notification", {
              status: outcome,
              summary: "Actual task result",
              output_file: "task.output",
            }),
            system("task_notification", {
              status: outcome,
              summary: "Actual task result",
              output_file: "task.output",
            }),
            toolResult(
              toolName === "Bash"
                ? { backgroundTaskId: taskId }
                : { status: "async_launched", isAsync: true, agentId: taskId },
            ),
            // A queued progress event must never revive a settled task.
            system("task_progress", {
              description: "Stale progress",
              usage: { total_tokens: 1, tool_uses: 1, duration_ms: 11 },
            }),
            turnEnd,
          ],
          rejected: [
            {
              type: "assistant",
              message: {
                role: "assistant",
                content: [
                  {
                    type: "tool_use",
                    id: "rejected-launch",
                    name: toolName,
                    input:
                      toolName === "Bash"
                        ? { command: "rejected", run_in_background: true }
                        : {
                            description: "Rejected agent",
                            prompt: "Do rejected work",
                            run_in_background: true,
                          },
                  },
                ],
              },
            },
            {
              type: "user",
              tool_use_result: { backgroundTaskId: "rejected-task" },
              message: {
                role: "user",
                content: [
                  {
                    type: "tool_result",
                    tool_use_id: "rejected-launch",
                    is_error: true,
                    content: "Permission denied",
                  },
                ],
              },
            },
            turnEnd,
          ],
          requested: [
            {
              type: "assistant",
              message: {
                role: "assistant",
                content: [
                  {
                    type: "tool_use",
                    id: "requested-only",
                    name: toolName,
                    input:
                      toolName === "Bash"
                        ? { command: "requested", run_in_background: true }
                        : {
                            description: "Requested agent",
                            prompt: "Do foreground work",
                            run_in_background: true,
                          },
                  },
                ],
              },
            },
            {
              type: "system",
              subtype: "task_started",
              task_id: "requested-task",
              tool_use_id: "requested-only",
              is_backgrounded: false,
              description: "Foreground execution",
            },
            turnEnd,
          ],
        };
        const { bridge, cleanup } = ultracodeFixtureBridge({
          STARTUP_JOURNAL: journal,
          BACKGROUND_TASK_FIXTURE: JSON.stringify(fixture),
        });
        try {
          bridge.writeCommand({
            command: "create_session",
            request_id: "background-session",
            cwd: bridge.cwd,
            launch_settings: {},
          });
          const connected = await nextMatching(
            bridge,
            (event) => event.event === "connected" && event.request_id === "background-session",
          );
          const collect = async (step: string) => {
            const phase = bridge.events.length,
              offset = launchJournal(journal).length;
            bridge.writeCommand({
              command: "prompt",
              request_id: `background-${step}`,
              session_id: connected.session_id,
              message_uuid: `background-${step}`,
              chunks: [{ kind: "text", value: `fixture task ${step}` }],
            });
            const terminal = await phaseEvent(
              bridge,
              phase,
              (event) =>
                event.session_id === connected.session_id &&
                event.event === (step === "start" ? "turn_error" : "turn_complete"),
            );
            if (step === "start") assert.match(String(terminal.message), /Interrupted by user/);
            assert.ok(
              launchJournal(journal)
                .slice(offset)
                .some(
                  (call) =>
                    call.type === "prompt" &&
                    call.queryId === connected.session_id &&
                    call.uuid === `background-${step}` &&
                    call.text === `fixture task ${step}`,
                ),
            );
            return bridge.events
              .slice(phase)
              .filter((event) => event.session_id === connected.session_id);
          };
          const fields = (events: BridgeEnvelope[]) =>
            events.flatMap((event) => {
              const update = event.update as SessionUpdate | undefined;
              return update?.type === "tool_call_update" &&
                update.tool_call_update.tool_call_id === toolId
                ? [update.tool_call_update.fields]
                : [];
            });
          const launchEvents = await collect("start");
          const launchUpdates = fields(launchEvents);
          if (toolName === "Agent") {
            for (const id of ["nested-agent", "nested-bash"]) {
              const created = launchEvents
                .map((event) => event.update as SessionUpdate)
                .find(
                  (update) => update?.type === "tool_call" && update.tool_call.tool_call_id === id,
                );
              assert.ok(
                created?.type === "tool_call",
                `${id} must actually exist before the interruption assertion`,
              );
              assert.equal(
                ((created.tool_call.meta as BridgeEnvelope)?.claudeCode as BridgeEnvelope)
                  ?.parentToolUseId,
                id === "nested-agent" ? toolId : "nested-agent",
              );
              const updates = launchEvents.flatMap((event) => {
                const update = event.update as SessionUpdate | undefined;
                return update?.type === "tool_call_update" &&
                  update.tool_call_update.tool_call_id === id
                  ? [update.tool_call_update.fields]
                  : [];
              });
              assert.ok(
                !updates.some((update) =>
                  ["completed", "failed", "killed"].includes(update.status ?? ""),
                ),
                "foreground interruption must not finalize descendants of a background agent",
              );
            }
          }
          assert.ok(
            launchUpdates.some((update) => update.status === "detached"),
            `${toolName}: confirmed launch must detach`,
          );
          if (toolName === "Bash")
            assert.ok(
              launchUpdates.some((update) => update.raw_output?.includes("Launch stdout retained")),
            );
          const detachment = launchUpdates.findIndex((update) => update.status === "detached");
          assert.ok(
            launchUpdates
              .slice(detachment)
              .every((update) => update.status === undefined || update.status === "detached"),
            `${toolName}: launch/progress/interruption must not settle or reattach`,
          );
          const completionEvents = await collect("finish");
          const completionUpdates = fields(completionEvents);
          if (toolName === "Agent") {
            for (const id of ["nested-agent", "nested-bash"]) {
              const updates = completionEvents.flatMap((event) => {
                const update = event.update as SessionUpdate | undefined;
                return update?.type === "tool_call_update" &&
                  update.tool_call_update.tool_call_id === id
                  ? [update.tool_call_update.fields]
                  : [];
              });
              assert.equal(updates.filter((update) => update.status === "completed").length, 1);
              assert.ok(
                updates.some(
                  (update) =>
                    update.raw_output ===
                    (id === "nested-agent" ? "Nested review finished" : "Nested command finished"),
                ),
                "the exact independently authored descendant result must reach its own tool",
              );
            }
          }
          const inventory = completionEvents.flatMap((event) => {
            const update = event.update as SessionUpdate | undefined;
            return update?.type === "task_state_update"
              ? update.tasks.filter((task) => task.task_id === taskId)
              : [];
          });
          assert.equal((inventory.at(-1)?.metadata as BridgeEnvelope)?.terminal_status, outcome);
          assert.equal(inventory.at(-1)?.source_tool_call_id, toolId);
          assert.equal(
            inventory.at(-1)?.status,
            "completed",
            "stale progress and duplicate launch acknowledgements must not revive task inventory",
          );
          assert.equal(
            completionUpdates.filter(
              (update) => update.status === (outcome === "stopped" ? "killed" : outcome),
            ).length,
            1,
            `${toolName}: duplicate notifications must produce one terminal outcome on the original tool`,
          );
          assert.ok(
            !completionUpdates
              .slice(
                completionUpdates.findIndex(
                  (update) => update.status === (outcome === "stopped" ? "killed" : outcome),
                ),
              )
              .some((update) => update.status === "in_progress" || update.status === "detached"),
            `${toolName}: stale progress must not revive completion`,
          );
          assert.ok(
            completionUpdates.some((update) => update.raw_output?.includes("Actual task result")),
            `${toolName}: render actual completion, not the launch placeholder`,
          );
          if (toolName === "Bash")
            assert.ok(
              completionUpdates.some(
                (update) =>
                  update.raw_output?.includes("Launch stdout retained") &&
                  update.raw_output.includes("Actual task result"),
              ),
              "final output retains both launch stdout and native outcome",
            );
          for (const step of initiallyBackgrounded && outcome === "completed"
            ? ["rejected", "requested"]
            : []) {
            const id = step === "rejected" ? "rejected-launch" : "requested-only";
            const events = await collect(step);
            const updates = events.flatMap((event) => {
              const update = event.update as SessionUpdate | undefined;
              return update?.type === "tool_call_update" &&
                update.tool_call_update.tool_call_id === id
                ? [update.tool_call_update.fields]
                : [];
            });
            assert.ok(
              !updates.some((update) => update.status === "detached"),
              "requesting background execution or rejecting a launch must not detach",
            );
            if (step === "rejected")
              assert.ok(updates.some((update) => update.status === "failed"));
            else assert.ok(updates.some((update) => update.status === "in_progress"));
          }
        } finally {
          await cleanup();
          rmSync(directory, { recursive: true, force: true });
        }
      }
    }
  }
});

test("one prompt can deliver a separately activated autonomous response for its exact background tool", async () => {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), "autonomous-response-"))),
    journal = join(directory, "calls.jsonl");
  const response = (id: string, event: BridgeEnvelope) => ({
    type: "stream_event",
    uuid: `${id}-stream`,
    parent_tool_use_id: null,
    event,
  });
  const textResponse = (id: string, text: string) => [
    response(id, { type: "message_start", message: { id } }),
    response(id, {
      type: "content_block_start",
      index: 0,
      content_block: { type: "text", text: "" },
    }),
    response(id, { type: "content_block_delta", index: 0, delta: { type: "text_delta", text } }),
    response(id, { type: "message_stop" }),
  ];
  const turnEnd = {
    type: "result",
    subtype: "success",
    is_error: false,
    duration_ms: 1,
    num_turns: 1,
    result: "",
    usage: {},
    modelUsage: {},
    permission_denials: [],
  };
  const fixture = {
    start: [
      {
        type: "assistant",
        message: {
          role: "assistant",
          content: [
            {
              type: "tool_use",
              id: "independent-tool",
              name: "Bash",
              input: { command: "background work" },
            },
          ],
        },
      },
      {
        type: "user",
        tool_use_result: { backgroundTaskId: "independent-task" },
        message: {
          role: "user",
          content: [
            {
              type: "tool_result",
              tool_use_id: "independent-tool",
              content: "Launch acknowledgement",
            },
          ],
        },
      },
      ...textResponse("foreground-response", "The background work was launched."),
      {
        ...turnEnd,
        uuid: "foreground-result",
        user_message_uuid: "single-prompt",
        user_message_uuids: ["single-prompt"],
      },
      {
        type: "system",
        subtype: "task_notification",
        task_id: "independent-task",
        tool_use_id: "independent-tool",
        status: "completed",
        summary: "Background command completed",
      },
      ...textResponse("autonomous-response", "The task finished completely."),
      { ...turnEnd, uuid: "autonomous-result", origin: { kind: "task-notification" } },
    ],
  };
  const { bridge, cleanup } = ultracodeFixtureBridge({
    STARTUP_JOURNAL: journal,
    BACKGROUND_TASK_FIXTURE: JSON.stringify(fixture),
  });
  try {
    bridge.writeCommand({
      command: "create_session",
      request_id: "autonomous-session",
      cwd: bridge.cwd,
      launch_settings: {},
    });
    const connected = await nextMatching(
      bridge,
      (event) => event.event === "connected" && event.request_id === "autonomous-session",
    );
    const start = bridge.events.length;
    bridge.writeCommand({
      command: "prompt",
      request_id: "single-prompt-request",
      session_id: connected.session_id,
      message_uuid: "single-prompt",
      chunks: [{ kind: "text", value: "fixture task start" }],
    });
    const terminal = (event: BridgeEnvelope) =>
      event.event === "turn_complete" && event.session_id === connected.session_id;
    const firstEnd = await phaseEvent(bridge, start, terminal),
      boundary = bridge.events.indexOf(firstEnd) + 1;
    await phaseEvent(bridge, boundary, terminal);
    bridge.writeCommand({ command: "shutdown" });
    assert.equal(await bridge.waitForExit(), 0);
    const first = bridge.events
        .slice(start, boundary)
        .filter((event) => event.session_id === connected.session_id),
      second = bridge.events
        .slice(boundary)
        .filter((event) => event.session_id === connected.session_id),
      updates = (events: BridgeEnvelope[]) =>
        events.flatMap((event) => (event.update ? [event.update as SessionUpdate] : [])),
      firstUpdates = updates(first),
      secondUpdates = updates(second);
    const body = (rows: SessionUpdate[]) =>
      rows
        .flatMap((update) =>
          update.type === "agent_message_chunk" && update.content.type === "text"
            ? [update.content.text]
            : [],
        )
        .join("");
    assert.equal(body(firstUpdates), "The background work was launched.");
    assert.equal(body(secondUpdates), "The task finished completely.");
    assert.ok(
      firstUpdates.some(
        (update) =>
          update.type === "tool_call" &&
          update.tool_call.tool_call_id === "independent-tool" &&
          (update.tool_call.raw_input as BridgeEnvelope)?.command === "background work",
      ),
    );
    assert.ok(
      firstUpdates.some(
        (update) =>
          update.type === "tool_call_update" &&
          update.tool_call_update.tool_call_id === "independent-tool" &&
          update.tool_call_update.fields.status === "detached",
      ),
    );
    const activity = secondUpdates.findIndex((update) => update.type === "agent_response_started"),
      text = secondUpdates.findIndex((update) => update.type === "agent_message_chunk");
    assert.ok(
      activity >= 0 && activity < text,
      "the autonomous response activates before its own first visible text",
    );
    const terminalTools = secondUpdates.filter(
      (update) =>
        update.type === "tool_call_update" &&
        update.tool_call_update.tool_call_id === "independent-tool" &&
        update.tool_call_update.fields.status === "completed",
    );
    assert.equal(terminalTools.length, 1);
    assert.ok(terminalTools[0]?.type === "tool_call_update");
    assert.ok(
      terminalTools[0].tool_call_update.fields.raw_output?.endsWith(
        "\nBackground command completed",
      ),
    );
    assert.equal(
      terminalTools[0].tool_call_update.fields.task_metadata?.terminal_status,
      "completed",
    );
    assert.equal(
      second.some((event) => event.event === "user_message_started"),
      false,
    );
    assert.deepEqual(
      launchJournal(journal)
        .filter((call) => call.type === "prompt")
        .map((call) => [call.queryId, call.uuid, call.text]),
      [[connected.session_id, "single-prompt", "fixture task start"]],
    );
  } finally {
    await cleanup();
    rmSync(directory, { recursive: true, force: true });
  }
});

test("saved background identities settle once from live outcomes without altering their completed sibling or transcript", async () => {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), "claude-rs-background-resume-")));
  const cwd = join(directory, "project");
  mkdirSync(cwd);
  const configDir = join(directory, "config");
  const projectDir = join(configDir, "projects", "bridge-background-resume");
  mkdirSync(projectDir, { recursive: true });
  const sessionId = "22222222-2222-4222-8222-222222222222";
  const records: BridgeEnvelope[] = [];
  const append = (type: string, message: unknown, extra: BridgeEnvelope = {}) => {
    records.push({
      uuid: `record-${records.length}`,
      parentUuid: records.at(-1)?.uuid ?? null,
      type,
      message,
      sessionId,
      cwd,
      timestamp: "2026-10-06T10:55:30.787Z",
      ...extra,
    });
  };
  append("user", { role: "user", content: "Launch background work" });
  append("assistant", {
    id: "launch",
    role: "assistant",
    content: [
      {
        type: "tool_use",
        id: "completed-shell",
        name: "Bash",
        input: { command: "saved command", run_in_background: true },
      },
      {
        type: "tool_use",
        id: "pending-agent",
        name: "Agent",
        input: { description: "Saved agent", prompt: "Work", run_in_background: true },
      },
    ],
  });
  append(
    "user",
    {
      role: "user",
      content: [
        { type: "tool_result", tool_use_id: "completed-shell", content: "Launch acknowledgement" },
      ],
    },
    { toolUseResult: { stdout: "partial stdout", stderr: "", backgroundTaskId: "saved-shell" } },
  );
  append(
    "user",
    {
      role: "user",
      content: [
        {
          type: "tool_result",
          tool_use_id: "pending-agent",
          content: "Agent launch acknowledgement",
        },
      ],
    },
    {
      toolUseResult: {
        status: "async_launched",
        isAsync: true,
        agentId: "saved-agent",
        description: "Saved agent",
        outputFile: "saved.output",
      },
    },
  );
  append(
    "user",
    {
      role: "user",
      content:
        "<task-notification><task-id>saved-shell</task-id><status>completed</status><summary>Saved command result</summary></task-notification>",
    },
    { origin: { kind: "task-notification", producer: "session-task" } },
  );
  append("assistant", {
    id: "after-launch",
    role: "assistant",
    content: [{ type: "text", text: "Foreground continued" }],
  });
  const transcriptPath = join(projectDir, `${sessionId}.jsonl`);
  const transcript = `${records.map((record) => JSON.stringify(record)).join("\n")}\n`;
  writeFileSync(transcriptPath, transcript);
  const completion = {
    type: "system",
    subtype: "task_notification",
    task_id: "saved-agent",
    status: "stopped",
    summary: "Saved agent stopped",
    output_file: "saved.output",
  };
  const { bridge, cleanup } = ultracodeFixtureBridge({
    STARTUP_JOURNAL: join(directory, "calls.jsonl"),
    CLAUDE_CONFIG_DIR: configDir,
    CLAUDE_CODE_PROJECT_DIR_NAME: "bridge-background-resume",
    RESUME_TRANSCRIPT_FIXTURE: "1",
    STARTUP_SESSIONS_FIXTURE: JSON.stringify([{ sessionId, cwd, lastModified: 1 }]),
    BACKGROUND_TASK_FIXTURE: JSON.stringify({
      finish: [
        completion,
        completion,
        {
          type: "system",
          subtype: "task_progress",
          task_id: "saved-agent",
          description: "Stale resumed progress",
        },
        {
          type: "result",
          subtype: "success",
          is_error: false,
          duration_ms: 1,
          num_turns: 1,
          result: "Finished",
          uuid: "live-result",
          user_message_uuid: "resume-finish",
          user_message_uuids: ["resume-finish"],
          usage: {},
          modelUsage: {},
          permission_denials: [],
        },
      ],
    }),
  });
  try {
    bridge.writeCommand({
      command: "resume_session",
      request_id: "resume-background",
      session_id: sessionId,
      launch_settings: {},
    });
    const connected = await nextMatching(
      bridge,
      (event) => event.event === "connected" && event.request_id === "resume-background",
    );
    assertProtocolEvent(connected, "connected");
    const history = connected.history_updates as SessionUpdate[];
    const fold = (updates: SessionUpdate[], initial = new Map<string, BridgeEnvelope>()) => {
      const result = new Map(initial);
      for (const update of updates) {
        if (update.type === "tool_call")
          result.set(update.tool_call.tool_call_id, { ...update.tool_call });
        if (update.type === "tool_call_update")
          result.set(update.tool_call_update.tool_call_id, {
            ...result.get(update.tool_call_update.tool_call_id),
            ...update.tool_call_update.fields,
          });
      }
      return result;
    };
    const restored = fold(history),
      completed = restored.get("completed-shell"),
      pending = restored.get("pending-agent");
    assert.ok(completed && pending);
    assert.equal(completed.status, "completed");
    assert.equal(pending.status, "detached");
    assert.equal((completed.raw_input as BridgeEnvelope)?.command, "saved command");
    assert.equal((pending.raw_input as BridgeEnvelope)?.prompt, "Work");
    assert.ok(String(completed.raw_output).includes("partial stdout"));
    assert.ok(String(completed.raw_output).endsWith("\nSaved command result"));
    const savedTasks = history.flatMap((update) =>
      update.type === "task_state_update" ? update.tasks : [],
    );
    assert.ok(
      savedTasks.some(
        (task) => task.task_id === "saved-agent" && task.source_tool_call_id === "pending-agent",
      ),
    );
    assert.ok(
      savedTasks.some(
        (task) => task.task_id === "saved-shell" && task.source_tool_call_id === "completed-shell",
      ),
    );
    assert.ok(
      !history.some(
        (update) =>
          update.type === "user_message_chunk" &&
          update.content.type === "text" &&
          update.content.text.includes("<task-notification>"),
      ),
    );
    assert.equal(readFileSync(transcriptPath, "utf8"), transcript);
    const phase = bridge.events.length;
    bridge.writeCommand({
      command: "prompt",
      request_id: "resume-outcome",
      session_id: sessionId,
      message_uuid: "resume-finish",
      chunks: [{ kind: "text", value: "fixture task finish" }],
    });
    await phaseEvent(
      bridge,
      phase,
      (event) => event.event === "turn_complete" && event.session_id === sessionId,
    );
    const results = bridge.events
        .slice(phase)
        .filter((event) => event.session_id === sessionId && event.event === "session_update")
        .map((event) => event.update as SessionUpdate),
      outcomes = results.flatMap((update) =>
        update.type === "tool_call_update" &&
        update.tool_call_update.tool_call_id === "pending-agent"
          ? [update.tool_call_update.fields]
          : [],
      );
    assert.equal(
      outcomes.filter((fields) => fields.status === "killed").length,
      1,
      "one final outcome on the original pending invocation",
    );
    assert.ok(
      outcomes.every((fields) => fields.status === undefined || fields.status === "killed"),
      "duplicate and stale progress cannot revive the settled Agent",
    );
    const final = fold(results, restored);
    assert.equal(final.get("pending-agent")?.status, "killed");
    assert.equal(final.get("pending-agent")?.raw_output, "Saved agent stopped");
    assert.deepEqual(final.get("completed-shell"), completed);
    assert.equal(
      results.some((update) => update.type === "tool_call"),
      false,
      "no replacement invocation is fabricated",
    );
    assert.equal(
      results.some(
        (update) =>
          update.type === "tool_call_update" &&
          update.tool_call_update.tool_call_id === "completed-shell",
      ),
      false,
    );
    const tasks = results.flatMap((update) =>
      update.type === "task_state_update"
        ? update.tasks.filter((task) => task.task_id === "saved-agent")
        : [],
    );
    assert.equal(tasks.at(-1)?.status, "completed");
    assert.equal((tasks.at(-1)?.metadata as BridgeEnvelope)?.terminal_status, "stopped");
    assert.equal(tasks.at(-1)?.source_tool_call_id, "pending-agent");
    assert.ok(
      launchJournal(join(directory, "calls.jsonl")).some(
        (call) =>
          call.type === "prompt" && call.uuid === "resume-finish" && call.queryId === sessionId,
      ),
    );
    bridge.writeCommand({ command: "shutdown" });
    assert.equal(await bridge.waitForExit(), 0);
    assert.equal(readFileSync(transcriptPath, "utf8"), transcript);
  } finally {
    await cleanup();
    assert.equal(dirname(directory), realpathSync(tmpdir()));
    rmSync(directory, { recursive: true, force: true });
  }
});

test("ingress refusals preserve correlation and default privacy while detailed previews are intentional", async () => {
  const privateLine =
    '{"command":"prompt","request_id":"private-malformed","prompt":"private plan \u00e9\ud83d\ude00 ' +
    "x".repeat(300) +
    '"}';
  for (const detailed of [false, true]) {
    const bridge = new SpawnedBridge({ CLAUDE_RS_BRIDGE_DIAGNOSTICS: detailed ? "1" : "0" });
    try {
      bridge.writeRaw(`${privateLine}\n`);
      const malformed = await nextMatching(
        bridge,
        (event) => event.event === "connection_failed" && event.request_id === "private-malformed",
      );
      assert.match(String(malformed.message), /prompt|session_id/);
      bridge.writeRaw("{ not-json\n");
      assert.match(
        String(
          (
            await nextMatching(
              bridge,
              (event) => event.event === "connection_failed" && !event.request_id,
            )
          ).message,
        ),
        /invalid command envelope/,
      );
      bridge.writeCommand({ command: "future_command", request_id: "unknown-envelope" });
      assert.match(
        String(
          (
            await nextMatching(
              bridge,
              (event) =>
                event.event === "connection_failed" && event.request_id === "unknown-envelope",
            )
          ).message,
        ),
        /future_command/,
      );
      bridge.writeCommand({ command: "initialize", request_id: "after-refusals", cwd: bridge.cwd });
      await nextMatching(
        bridge,
        (event) => event.event === "initialized" && event.request_id === "after-refusals",
      );
      await nextMatching(
        bridge,
        (event) => event.event === "sessions_listed" && event.request_id === "after-refusals",
      );
      bridge.writeCommand({ command: "shutdown", request_id: "privacy-shutdown" });
      assert.equal(await bridge.waitForExit(), 0);
      const records = bridge.stderrLines.map((line) => JSON.parse(line) as BridgeEnvelope),
        decode = records.find(
          (record) =>
            record.event_name === "bridge_command_decode_failed" &&
            record.request_id === "private-malformed",
        );
      assert.ok(decode);
      assert.equal(decode.size_bytes, Buffer.from(privateLine, "utf8").length);
      assert.equal(
        bridge.events.filter(
          (event) =>
            event.event === "connection_failed" && event.request_id === "private-malformed",
        ).length,
        1,
      );
      assert.equal(
        bridge.events.filter(
          (event) => event.event === "connection_failed" && event.request_id === "unknown-envelope",
        ).length,
        1,
      );
      assert.ok(!JSON.stringify(bridge.events).includes("private plan"));
      if (detailed) {
        const preview = String((decode.fields as BridgeEnvelope).preview);
        assert.ok(preview.includes("private plan"));
        assert.equal(preview, privateLine.slice(0, 240));
      } else {
        assert.equal(Object.hasOwn(decode.fields as BridgeEnvelope, "preview"), false);
        assert.ok(!bridge.stderrLines.join("\n").includes("private plan"));
      }
    } finally {
      await bridge.stop();
    }
  }
});

test("initialization exposes consumed capabilities and a correlated empty isolated listing", async () => {
  const bridge = new SpawnedBridge();
  try {
    bridge.writeCommand({ command: "initialize", request_id: "req-init", cwd: bridge.cwd });
    const initialized = await nextMatching(
        bridge,
        (event) => event.event === "initialized" && event.request_id === "req-init",
      ),
      result = initialized.result as BridgeEnvelope;
    assert.equal(result.agent_name, "claude-rs-agent-bridge");
    const capabilities = result.capabilities as BridgeEnvelope;
    for (const feature of [
      "prompt_image",
      "prompt_embedded_context",
      "supports_session_listing",
      "supports_resume_session",
    ])
      assert.equal(capabilities[feature], true, feature);
    assert.ok(
      (result.auth_methods as BridgeEnvelope[]).some((method) => method.id === "claude-login"),
    );
    const listing = await nextMatching(
      bridge,
      (event) => event.event === "sessions_listed" && event.request_id === "req-init",
    );
    assert.deepEqual(listing.sessions, []);
    bridge.writeCommand({
      command: "side_question",
      session_id: "isolated-missing",
      btw_id: "init-control",
      question: "Can the next command dispatch?",
    });
    const response = await nextMatching(
      bridge,
      (event) => event.event === "btw_failed" && event.btw_id === "init-control",
    );
    assert.equal(response.session_id, "isolated-missing");
    assert.equal(response.question, "Can the next command dispatch?");
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

test("explicit shutdown closes idle and active queries only after owned consumer completion", async () => {
  const idle = new SpawnedBridge();
  try {
    idle.writeCommand({ command: "shutdown" });
    assert.equal(await idle.waitForExit(), 0);
  } finally {
    await idle.stop();
  }
  await verifyOwnedCompletion("shutdown");
});

test("bridge process reports actionable session initialization failure", async () => {
  const directory = mkdtempSync(join(tmpdir(), "claude-rs-missing-executable-"));
  const missingClaudeExecutable = join(directory, "missing-claude-code");
  const bridge = new SpawnedBridge({
    CLAUDE_CONFIG_DIR: directory,
    CLAUDE_CODE_EXECUTABLE: missingClaudeExecutable,
  });
  try {
    bridge.writeCommand({
      request_id: "req-create",
      command: "create_session",
      cwd: directory,
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
    rmSync(directory, { recursive: true, force: true });
  }
});

test("real SDK transports one correlated startup failure from a controlled CLI child", async () => {
  const directory = mkdtempSync(join(tmpdir(), "claude-rs-startup-"));
  const receipt = join(directory, "startup-receipt.json");
  const cliPath = join(directory, "startup-cli.js");
  writeFileSync(
    cliPath,
    `
    require("node:readline").createInterface({input:process.stdin}).once("line", line => {
      const request=JSON.parse(line);require("node:fs").writeFileSync(${JSON.stringify(receipt)},JSON.stringify({type:request.type,subtype:request.request?.subtype}));
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
  `,
  );
  const bridge = new SpawnedBridge({ CLAUDE_CODE_EXECUTABLE: cliPath });
  try {
    bridge.writeCommand({
      command: "create_session",
      request_id: "startup-connect",
      cwd: bridge.cwd,
      launch_settings: {},
    });
    const failure = await bridge.nextEnvelope(5_000);
    assert.deepEqual(failure, {
      event: "connection_failed",
      request_id: "startup-connect",
      message: "Claude Code startup failed.",
      startup_failure: {
        reason: "provider_not_allowed",
        errors: ["Original startup guidance ANTHROPIC_API_KEY=[redacted] Please login."],
      },
    });
    bridge.writeCommand({ command: "shutdown" });
    assert.equal(await bridge.waitForExit(5_000), 0);
    assert.deepEqual(JSON.parse(readFileSync(receipt, "utf8")), {
      type: "control_request",
      subtype: "initialize",
    });
    assert.deepEqual(
      bridge.events.filter((event) => event.event === "connection_failed"),
      [failure],
      "one classified failure after full output drain",
    );
    assert.equal(
      bridge.events.some((event) => event.event === "connected"),
      false,
    );
    assert.ok(!JSON.stringify(bridge.events).includes("private-key"));
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

// Only the external SDK is replaced. The spawned process runs the production
// readline parser, command scheduler, session lifecycle and NDJSON writer.
function ultracodeFixtureBridge(env: NodeJS.ProcessEnv = {}): {
  bridge: SpawnedBridge;
  cleanup: () => Promise<void>;
} {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), "claude-rs-ultracode-")));
  const sdkPath = import.meta.resolve("@anthropic-ai/claude-agent-sdk");
  const sdkPackage = join(dirname(fileURLToPath(sdkPath)), "package.json");
  writeFileSync(
    join(directory, "package.json"),
    JSON.stringify({ version: JSON.parse(readFileSync(sdkPackage, "utf8")).version }),
  );
  const fixturePath = join(directory, "sdk-fixture.mjs");
  writeFileSync(
    fixturePath,
    `
    export * from ${JSON.stringify(sdkPath)};
    import { appendFileSync, createReadStream, writeSync } from "node:fs";
    import readline from "node:readline";
    import { resolveSettings as sdkResolveSettings } from ${JSON.stringify(sdkPath)};
    import { getSessionMessages as sdkGetSessionMessages, importSessionToStore as sdkImportSessionToStore } from ${JSON.stringify(sdkPath)};
    export async function resolveSettings(options) {
      const managedSettings = process.env.SETTINGS_POLICY_FIXTURE;
      return sdkResolveSettings({ ...options, ...(managedSettings ? { serverManagedSettings: JSON.parse(managedSettings) } : {}) });
    }
    function record(value) { if (process.env.STARTUP_JOURNAL) appendFileSync(process.env.STARTUP_JOURNAL, JSON.stringify(value) + "\\n"); }
    const saved = new Map();
    function sessionEntries() {
      const seeded = JSON.parse(process.env.STARTUP_SESSIONS_FIXTURE ?? "[]");
      const entries = new Map(seeded.map(entry => [entry.sessionId, entry]));
      for (const [sessionId, state] of saved) {
        const entry = entries.get(sessionId) ?? { sessionId, cwd: state.cwd, lastModified: 1 };
        entries.set(sessionId, { ...entry, ...(Object.hasOwn(state, "title") ? { customTitle: state.title } : {}) });
      }
      return [...entries.values()];
    }
    export async function listSessions(options) {
      record({ type: "list", options });
      return sessionEntries().filter(entry => !options?.dir || entry.cwd === options.dir);
    }
    export async function getSessionInfo(sessionId, options) {
      const entry = sessionEntries().find(candidate => candidate.sessionId === sessionId);
      if (!entry || (options?.dir && entry.cwd !== options.dir)) return undefined;
      record({ type: "title-read", sessionId, dir: options?.dir, title: entry.customTitle });
      return entry;
    }
    let subagentListings = 0;
    export async function listSubagents(sessionId, options) {
      record({ type: "listSubagents", sessionId, dir: options?.dir });
      // The transcript directory becomes unreadable after the first listing.
      if (++subagentListings > 1) throw new Error("subagent directory unreadable");
      return ["agent-running", "agent-done"];
    }
    export async function getSubagentMessages(sessionId, agentId, options) {
      record({ type: "getSubagentMessages", sessionId, agentId, dir: options?.dir });
      if (agentId === "agent-unreadable") throw new Error("transcript unreadable");
      const entry = (type, uuid, parentUuid, content) => ({ type, uuid, parentUuid, session_id: sessionId, parent_tool_use_id: null, message: { role: type, content } });
      if (agentId === "agent-handback") return [
        entry("user", "handback-prompt", null, "Survey the docs. Quote <system-reminder>tags</system-reminder> as written."),
        { ...entry("user", "handback-reminder", "handback-prompt", "<system-reminder>\\nReport through SubagentHandback.\\n</system-reminder>\\n"), is_meta: true },
        entry("user", "handback-follow-up", "handback-reminder", "<system-reminder>typed by the user</system-reminder>"),
        entry("assistant", "handback-note", "handback-follow-up", [{ type: "text", text: "Keeping <system-reminder>as written</system-reminder>" }]),
        entry("assistant", "handback-read", "handback-note", [{ type: "tool_use", id: "handback-glob", name: "Glob", input: { pattern: "docs/**" } }]),
        entry("assistant", "handback-call", "handback-read", [{ type: "tool_use", id: "handback-1", name: "SubagentHandback", input: { message: "## Docs survey\\n\\n- 12 pages\\n- 2 stale links" } }]),
        entry("user", "handback-ack", "handback-call", [{ type: "tool_result", tool_use_id: "handback-1", content: [{ type: "text", text: "Report delivered to your caller." }] }]),
      ];
      return [
        entry("user", "agent-prompt", null, [{ type: "text", text: "Inspect the parser" }]),
        entry("assistant", "agent-tool", "agent-prompt", [{ type: "tool_use", id: "agent-bash", name: "Bash", input: { command: "ls parser" } }]),
        entry("assistant", "agent-reply", "agent-tool", [{ type: "text", text: "Parser checked" }]),
      ];
    }
    export async function getSessionMessages(...args) { return process.env.RESUME_TRANSCRIPT_FIXTURE === "1" ? sdkGetSessionMessages(...args) : []; }
    export async function importSessionToStore(...args) { if (process.env.RESUME_TRANSCRIPT_FIXTURE === "1") await sdkImportSessionToStore(...args); }
    export async function renameSession(sessionId, title, options) {
      record({ type: "rename", sessionId, title, dir: options?.dir });
      const existing = saved.get(sessionId) ?? {};
      saved.set(sessionId, { ...existing, cwd: existing.cwd ?? options?.dir, title });
    }
    export function query({ prompt, options }) {
      record({ type: "query", cwd: options.cwd, resume: options.resume, model: options.model, effort: options.effort, permissionMode: options.permissionMode, agent: options.agent });
      record({ type: "launch", sessionId: options.sessionId ?? options.resume, resume: options.resume, forkSession: options.forkSession, title: options.title, additionalDirectories: options.additionalDirectories, mcpServers: options.mcpServers, strictMcpConfig: options.strictMcpConfig, settings: options.settings, systemPrompt: options.systemPrompt, enableFileCheckpointing: options.enableFileCheckpointing });
      const previous = saved.get(options.resume);
      const seeded = sessionEntries().find(entry => entry.sessionId === options.resume);
      const state = { ...(previous ?? { requested: false, effort: "high", ...(seeded?.customTitle !== undefined ? { title: seeded.customTitle } : {}) }), cwd: options.cwd };
      if (options.title !== undefined) state.title = options.title;
      saved.set(options.sessionId ?? options.resume, state);
      let model = options.model ?? "opus";
      let effective = {};
      let fastMode = false;
      let fastReadFails = process.env.FAST_READ_FAILURE_FIXTURE === "1";
      const sessionFixture = process.env.SESSION_SETTINGS_FIXTURE === "1";
      let nativeFailure = Boolean(process.env.ULTRACODE_NATIVE_FAILURE);
      let done = false;
      let waiter;
      const queue = [];
      const gatedClose = process.env.OWNED_CLOSE_GATE === "1";
      let closeRequested = false, releaseOwned;
      const ownedCompletion = new Promise(resolve => { releaseOwned = resolve; });
      const receipt = type => writeSync(3, JSON.stringify({ type, sessionId: options.sessionId ?? options.resume }) + "\\n");
      if (gatedClose) {
        const control = readline.createInterface({ input: createReadStream(null, { fd: 4, autoClose: false }) });
        control.on("line", line => { const command = JSON.parse(line);if (command.type === "probe" && closeRequested && waiter) receipt("held-after-close");if (command.type === "release") releaseOwned(); });
      }
      const commandFixture = process.env.SLASH_COMMANDS_FIXTURE === "1";
      const bootstrapCommands = [
        { name: "clear", description: "Clear conversation", argumentHint: "[name]", aliases: ["reset", "new"], builtin: true },
        { name: "deploy", description: "Deploy", argumentHint: "<target>", aliases: ["ship"] },
        { name: "removed-plugin", description: "Old plugin", argumentHint: "" },
      ];
      const currentCommands = [...bootstrapCommands.slice(0, 2), { name: "current-plugin", description: "Current plugin", argumentHint: "[optional reason]" }];
      let clearCount = 0;
      let permissionAbort;
      function push(value) { if (waiter) { const resolve = waiter; waiter = undefined; resolve({ value, done: false }); } else queue.push(value); }
      void (async () => {
        for await (const message of prompt) {
          const text = message.message?.content?.filter(block => block.type === "text").map(block => block.text).join("").trim();
          record({ type: "prompt", queryId: options.sessionId ?? options.resume, uuid: message.uuid, text });
          if (text.startsWith("fixture permission ")) {
            const toolUseID = text.slice("fixture permission ".length);
            permissionAbort = new AbortController();
            const result = await options.canUseTool("Bash", { command: "echo permission-fixture" }, {
              toolUseID, signal: permissionAbort.signal,
            });
            record({ type: "permission-result", toolUseID, result });
            push({ type: "result", subtype: "success", session_id: options.sessionId ?? options.resume,
              uuid: "result-" + toolUseID, result: "Permission settled", user_message_uuid: message.uuid,
              user_message_uuids: [message.uuid], permission_denials: [], usage: {}, modelUsage: {}, duration_ms: 1, num_turns: 1 });
          }
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
            saved.set("fixture-conversation-2", state);
            push({ type: "conversation_reset", new_conversation_id: "fixture-conversation-2", trigger: "clear" });
            push({ type: "system", subtype: "init", session_id: "fixture-conversation-2", model, slash_commands: [] });
          }
          if (text === "fixture observe permissions") {
            push({ type: "system", subtype: "init", session_id: options.resume || options.sessionId, model, permissionMode: effective.permissions?.defaultMode ?? "default" });
          }
          if (text === "fixture per-turn effort") {
            record({ type: "turn-init", queryId: options.sessionId ?? options.resume, promptUuid: message.uuid, effort: "xhigh" });
            push({ type: "system", subtype: "init", session_id: options.resume || options.sessionId, model, effort: "xhigh" });
          }
        }
      })();
      return {
        [Symbol.asyncIterator]() { return this; },
        next() {
          if (queue.length) return Promise.resolve({ value: queue.shift(), done: false });
          if (done) return Promise.resolve({ done: true });
          if (gatedClose) receipt("consumer-waiting");
          return new Promise(resolve => { waiter = resolve; });
        },
        close() { if (closeRequested) return;closeRequested = true;done = true;if (gatedClose) { receipt("native-close");void ownedCompletion.then(() => { receipt("consumer-released");waiter?.({ done: true });waiter = undefined; }); } else waiter?.({ done: true }); },
        async interrupt() { record({ type: "interrupt", sessionId: options.sessionId ?? options.resume });permissionAbort?.abort(); return { still_queued: [] }; },
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
            record({ type: "settings", sessionId: options.sessionId ?? options.resume, effective: resolved.effective, checkpointing: options.enableFileCheckpointing });
          }
          return { current_permission_mode: process.env.PERMISSION_MODE_FIXTURE ?? effective.permissions?.defaultMode ?? "default", models: ["default", "opus", "sonnet", "haiku", "fixture-unavailable", "fixture-read-failure"].map(value => ({ value, resolvedModel: value === "default" || value === "opus" ? "claude-opus-5-5" : value === "sonnet" ? "claude-sonnet-5-5" : value, displayName: value, supportsEffort: value !== "haiku", supportsFastMode: value !== "haiku", supportsAutoMode: value !== "haiku", supportedEffortLevels: value === "haiku" ? [] : ["low", "medium", "high", "xhigh", "max"] })), commands: commandFixture ? bootstrapCommands : [], agents: sessionFixture ? [{ name: "reviewer", description: "Review code", model: "sonnet" }] : [], account: { apiKeySource: "fixture" }, fast_mode_state: "off" }; },
        async stopTask(taskId) {
          record({ type: "stopTask", sessionId: options.sessionId ?? options.resume, taskId });
          if (taskId === "agent-closing") throw new Error("Query closed before response received");
        },
        async supportedCommands() { return bootstrapCommands; },
        async setModel(value) { model = process.env.MODEL_STEP_DOWN_FIXTURE === "1" && value === "opus" ? "sonnet" : value;record({ type: "model", sessionId: options.sessionId ?? options.resume, requested: value, observed: model }); },
        async setPermissionMode(value) { record({ type: "mode", sessionId: options.sessionId ?? options.resume, value });if (sessionFixture && value === "auto") throw new Error("Cannot set permission mode to auto: account restriction"); },
        async reinitialize() { record({ type: "fast-read", sessionId: options.sessionId ?? options.resume, fastMode, failed: fastMode && fastReadFails });if (fastMode && fastReadFails) { fastReadFails = false; throw new Error("fixture state read failed"); } return { fast_mode_state: fastMode ? "on" : "off" }; },
        async getSettings() {
          record({ type: "applied-read", sessionId: options.sessionId ?? options.resume, model, requested: state.requested, effort: state.effort });
          if (model === "fixture-read-failure") throw new Error("fixture read failed");
          const available = model !== "haiku" && model !== "fixture-unavailable" && process.env.ULTRACODE_FIXTURE_WORKFLOWS !== "off";
          return { effective, applied: { model: sessionFixture ? model === "opus" ? "claude-opus-5-5" : model === "sonnet" ? "claude-sonnet-5-5" : model : model, ultracodeAvailable: available, ultracodeRequested: state.requested, ultracode: state.requested && available, effort: model === "haiku" ? null : state.effort } };
        },
        async applyFlagSettings(settings) {
          record({ type: "flags", settings });
          if (settings.alwaysThinkingEnabled !== undefined) { if (settings.alwaysThinkingEnabled === null) effective = (await sdkResolveSettings({ cwd: options.cwd, settingSources: options.settingSources })).effective; else effective = { ...effective, alwaysThinkingEnabled: settings.alwaysThinkingEnabled }; }
          if (settings.fastMode !== undefined) fastMode = settings.fastMode;
          if (settings.agent !== undefined) { if (settings.agent === "reviewer") model = "sonnet"; }
          if (settings.ultracode === true && state.requested && nativeFailure) {
            nativeFailure = false;
            const failure = process.env.ULTRACODE_NATIVE_FAILURE;
            throw new Error(failure === "workflows" ? "apply_flag_settings: ultracode is not available for this session (dynamic workflows are off)" : failure === "model" ? "apply_flag_settings: ultracode is not available for this session (fixture-only-model does not support it)" : "RAW_PROTOCOL_PRIVATE_SENTINEL: ultracode must be a boolean");
          }
          if (settings.ultracode === true && process.env.ULTRACODE_FIXTURE_WORKFLOWS === "off") throw new Error("apply_flag_settings: ultracode is not available for this session (dynamic workflows are off)");
          if (settings.ultracode === true && model === "haiku") throw new Error("apply_flag_settings: ultracode is not available for this session (haiku does not support it)");
          if (settings.effortLevel !== undefined) {
            state.effort = settings.effortLevel === null ? "high" : settings.effortLevel === "max" && process.env.EFFORT_FIXTURE_CAP === "high" ? "high" : settings.effortLevel;
            state.requested = settings.ultracode === true;
          } else if (settings.ultracode !== undefined) state.requested = settings.ultracode;
        },
      };
    }
  `,
  );
  const loaderPath = join(directory, "loader.mjs");
  writeFileSync(
    loaderPath,
    `
    import { registerHooks } from "node:module";
    // Only ESM imports are redirected. The bridge's version check calls
    // require.resolve on the SDK to read its package.json; redirecting that
    // into the temp fixture directory made the check fail with "unable to
    // resolve installed ... package.json" before any session started.
    registerHooks({ resolve(specifier, context, nextResolve) {
      return specifier === "@anthropic-ai/claude-agent-sdk" && !context.conditions.includes("require")
        ? { url: ${JSON.stringify(pathToFileURL(fixturePath).href)}, shortCircuit: true }
        : nextResolve(specifier, context);
    } });
  `,
  );
  const bridge = new SpawnedBridge(
    { CLAUDE_CODE_EXECUTABLE: "", ...env },
    ["--import", pathToFileURL(loaderPath).href],
    env.OWNED_CLOSE_GATE === "1",
  );
  return {
    bridge,
    cleanup: async () => {
      await bridge.stop();
      rmSync(directory, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
    },
  };
}

async function nextMatching(
  bridge: SpawnedBridge,
  matches: (event: BridgeEnvelope) => boolean,
): Promise<BridgeEnvelope> {
  const deadline = Date.now() + 5_000;
  while (Date.now() < deadline) {
    const event = await bridge
      .nextEnvelope(Math.max(1, deadline - Date.now()))
      .catch((error: unknown) => {
        throw new Error(
          `Waiting for ${matches.toString()}: ${String(error)}; diagnostics: ${bridge.stderrLines.slice(-4).join("\n")}`,
        );
      });
    if (matches(event)) return event;
  }
  throw new Error("expected bridge event was not emitted before its deadline");
}

async function phaseEvent(
  bridge: SpawnedBridge,
  start: number,
  matches: (event: BridgeEnvelope) => boolean,
): Promise<BridgeEnvelope> {
  const observed = bridge.events.slice(start).find(matches);
  return (
    observed ??
    nextMatching(bridge, (event) => bridge.events.indexOf(event) >= start && matches(event))
  );
}

for (const scenario of ["allow", "deny", "cancel-before-answer", "answer-before-cancel"] as const) {
  test(`permission callbacks correlate answer/cancel settlement and isolate stale replies: ${scenario}`, async () => {
    const directory = realpathSync(mkdtempSync(join(tmpdir(), "permission-settlement-"))),
      journal = join(directory, "outcomes.jsonl");
    const { bridge, cleanup } = ultracodeFixtureBridge({ STARTUP_JOURNAL: journal });
    const outcomes = () =>
      launchJournal(journal).filter((entry) => entry.type === "permission-result");
    try {
      bridge.writeCommand({
        command: "create_session",
        request_id: "permission-session",
        cwd: bridge.cwd,
        launch_settings: { permission_mode: "default" },
      });
      const connected = await nextMatching(
          bridge,
          (event) => event.event === "connected" && event.request_id === "permission-session",
        ),
        sessionId = connected.session_id;
      const prompt = (id: string) =>
        bridge.writeCommand({
          command: "prompt",
          request_id: `prompt-${id}`,
          session_id: sessionId,
          message_uuid: id,
          chunks: [{ kind: "text", value: `fixture permission ${id}` }],
        });
      const answer = (id: string, option: string, request: string) => ({
        command: "permission_response",
        request_id: request,
        session_id: sessionId,
        tool_call_id: id,
        outcome: { outcome: "selected", option_id: option },
      });
      const isRequest = (id: string) => (event: BridgeEnvelope) =>
        event.event === "permission_request" &&
        event.session_id === sessionId &&
        ((event.request as BridgeEnvelope)?.tool_call as BridgeEnvelope)?.tool_call_id === id;
      prompt("first-tool");
      await nextMatching(bridge, isRequest("first-tool"));
      assert.deepEqual(outcomes(), [], "the native callback is actually waiting");
      const reply = answer(
          "first-tool",
          scenario === "deny" ? "reject_once" : "allow_once",
          "first-answer",
        ),
        cancel = { command: "cancel_turn", session_id: sessionId, request_id: "ordered-cancel" };
      const commands =
          scenario === "cancel-before-answer"
            ? [cancel, reply]
            : scenario === "answer-before-cancel"
              ? [reply, cancel]
              : [reply],
        phase = bridge.events.length;
      bridge.writeRaw(commands.map((command) => `${JSON.stringify(command)}\n`).join(""));
      await phaseEvent(
        bridge,
        phase,
        (event) => event.event === "turn_complete" && event.session_id === sessionId,
      );
      const cancelled = scenario === "cancel-before-answer" || scenario === "answer-before-cancel";
      if (cancelled)
        await phaseEvent(
          bridge,
          phase,
          (event) =>
            event.event === "turn_interrupt_receipt" &&
            event.session_id === sessionId &&
            event.request_id === "ordered-cancel",
        );
      const first = outcomes(),
        allowed = scenario === "allow" || scenario === "answer-before-cancel";
      assert.equal(first.length, 1);
      assert.equal(first[0].toolUseID, "first-tool");
      const result = first[0].result as BridgeEnvelope;
      assert.equal(result.toolUseID, "first-tool");
      assert.equal(result.behavior, allowed ? "allow" : "deny");
      if (allowed) assert.deepEqual(result.updatedInput, { command: "echo permission-fixture" });
      else assert.equal(Object.hasOwn(result, "updatedInput"), false);
      assert.equal(
        Object.hasOwn(result, "updatedPermissions"),
        false,
        "once answers never fabricate persistent grants",
      );
      assert.deepEqual(
        launchJournal(journal).filter((entry) => entry.type === "interrupt"),
        cancelled ? [{ type: "interrupt", sessionId }] : [],
      );
      prompt("next-tool");
      await nextMatching(bridge, isRequest("next-tool"));
      assert.equal(outcomes().length, 1, "the second callback is independently pending");
      const nextPhase = bridge.events.length;
      bridge.writeRaw(
        [
          answer("first-tool", "allow_once", "stale-first-answer"),
          answer("next-tool", "reject_once", "next-answer"),
        ]
          .map((command) => `${JSON.stringify(command)}\n`)
          .join(""),
      );
      await phaseEvent(
        bridge,
        nextPhase,
        (event) => event.event === "turn_complete" && event.session_id === sessionId,
      );
      bridge.writeCommand({ command: "shutdown", request_id: "permission-shutdown" });
      assert.equal(await bridge.waitForExit(), 0);
      const completed = outcomes();
      assert.deepEqual(
        completed.map((entry) => [entry.toolUseID, (entry.result as BridgeEnvelope).behavior]),
        [
          ["first-tool", allowed ? "allow" : "deny"],
          ["next-tool", "deny"],
        ],
      );
      const denied = completed[1].result as BridgeEnvelope;
      assert.equal(denied.toolUseID, "next-tool");
      assert.equal(Object.hasOwn(denied, "updatedInput"), false);
      assert.equal(Object.hasOwn(denied, "updatedPermissions"), false);
      const cancellations = bridge.events.filter(
        (event) => event.event === "interaction_cancelled" && event.session_id === sessionId,
      );
      assert.deepEqual(
        cancellations.map((event) => event.interaction_id),
        scenario === "cancel-before-answer" ? ["first-tool"] : [],
      );
      assert.equal(
        bridge.events.some((event) => event.event === "connection_failed"),
        false,
      );
    } finally {
      await cleanup();
      rmSync(directory, { recursive: true, force: true });
    }
  });
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

test("command inventories stay authoritative across repeated reset identities and a genuinely fresh query", async () => {
  const { bridge, cleanup } = ultracodeFixtureBridge({ SLASH_COMMANDS_FIXTURE: "1" });
  const bootstrap = [
    {
      name: "clear",
      description: "Clear conversation",
      input_hint: "[name]",
      aliases: ["reset", "new"],
      builtin: true,
    },
    { name: "deploy", description: "Deploy", input_hint: "<target>", aliases: ["ship"] },
    { name: "removed-plugin", description: "Old plugin" },
  ];
  const dynamicCommands = [
    {
      name: "clear",
      description: "Clear conversation",
      input_hint: "[name]",
      aliases: ["reset", "new"],
      builtin: true,
    },
    { name: "deploy", description: "Deploy", input_hint: "<target>", aliases: ["ship"] },
    { name: "current-plugin", description: "Current plugin", input_hint: "[optional reason]" },
  ];
  const commandsFor = (session: unknown) =>
    nextMatching(
      bridge,
      (event) =>
        event.event === "session_update" &&
        event.session_id === session &&
        (event.update as BridgeEnvelope)?.type === "available_commands_update",
    );
  try {
    bridge.writeCommand({
      command: "create_session",
      request_id: "command-owner",
      cwd: bridge.cwd,
      launch_settings: {},
    });
    const connected = await nextMatching(
      bridge,
      (event) => event.event === "connected" && event.request_id === "command-owner",
    );
    let sessionId = connected.session_id;
    const initial = (await commandsFor(sessionId)).update as BridgeEnvelope;
    assert.deepEqual(initial.commands, bootstrap);
    assert.equal(typeof initial.generation, "number");
    bridge.writeCommand({
      command: "prompt",
      session_id: sessionId,
      message_uuid: "command-refresh",
      chunks: [{ kind: "text", value: "/fixture-refresh-commands" }],
    });
    const authoritative = (await commandsFor(sessionId)).update as BridgeEnvelope;
    assert.equal(authoritative.source, "commands_changed");
    assert.ok(Number(authoritative.generation) > Number(initial.generation));
    assert.deepEqual(authoritative.commands, dynamicCommands);
    for (const iteration of [1, 2]) {
      const previous = sessionId;
      bridge.writeCommand({
        command: "prompt",
        session_id: sessionId,
        message_uuid: `clear-${iteration}`,
        chunks: [{ kind: "text", value: "/clear" }],
      });
      const reset = await nextMatching(
        bridge,
        (event) =>
          event.session_id === previous &&
          (event.update as BridgeEnvelope)?.type === "conversation_reset",
      );
      assert.ok(reset);
      const replaced = await nextMatching(bridge, (event) => event.event === "session_replaced");
      assert.equal(replaced.session_id, `fixture-cleared-${iteration}`);
      sessionId = replaced.session_id;
      assert.notEqual(sessionId, previous);
      const replay = (await commandsFor(sessionId)).update as BridgeEnvelope;
      assert.deepEqual(replay.commands, dynamicCommands);
      assert.equal(
        replay.generation,
        authoritative.generation,
        "the same inventory retains semantic generation through replay",
      );
    }
    bridge.writeCommand({
      command: "new_session",
      request_id: "new-command-owner",
      cwd: bridge.cwd,
      launch_settings: {},
    });
    const fresh = await nextMatching(
      bridge,
      (event) => event.event === "session_replaced" && event.request_id === "new-command-owner",
    );
    assert.notEqual(fresh.session_id, sessionId);
    const current = (await commandsFor(fresh.session_id)).update as BridgeEnvelope;
    assert.deepEqual(current.commands, bootstrap);
    assert.equal(typeof current.generation, "number");
    assert.ok(
      !(current.commands as BridgeEnvelope[]).some((command) => command.name === "current-plugin"),
    );
  } finally {
    await cleanup();
  }
});

test("continue selects the newest eligible project session with correlated fork and empty-history variants", async () => {
  for (const mode of ["resume", "fork", "fresh"]) {
    const directory = realpathSync(mkdtempSync(join(tmpdir(), "continue-selection-"))),
      cwd = join(directory, "project"),
      other = join(directory, "other"),
      journal = join(directory, "calls.jsonl");
    mkdirSync(cwd);
    mkdirSync(other);
    const candidates =
      mode === "fresh"
        ? [{ sessionId: "other-project", cwd: other, lastModified: 20 }]
        : [
            { sessionId: "older", cwd, lastModified: 1 },
            { sessionId: "newest-here", cwd, lastModified: 10 },
            { sessionId: "other-project", cwd: other, lastModified: 20 },
          ];
    const { bridge, cleanup } = ultracodeFixtureBridge({
      STARTUP_JOURNAL: journal,
      STARTUP_SESSIONS_FIXTURE: JSON.stringify(candidates),
    });
    try {
      bridge.writeCommand({ command: "initialize", request_id: `unscoped-${mode}`, cwd: "" });
      const unscoped = await nextMatching(
        bridge,
        (event) => event.event === "sessions_listed" && event.request_id === `unscoped-${mode}`,
      );
      assert.deepEqual(
        (unscoped.sessions as BridgeEnvelope[]).map((entry) => entry.session_id).sort(),
        mode === "fresh" ? ["other-project"] : ["newest-here", "older", "other-project"],
      );
      const unscopedCall = launchJournal(journal)
        .filter((entry) => entry.type === "list")
        .at(-1)?.options as BridgeEnvelope;
      assert.equal(unscopedCall.includeProgrammatic, true);
      assert.equal(Object.hasOwn(unscopedCall, "dir"), false);
      assert.equal(Object.hasOwn(unscopedCall, "includeWorktrees"), false);
      bridge.writeCommand({
        command: "create_session",
        request_id: `continue-${mode}`,
        cwd,
        continue_session: true,
        launch_settings: {
          model: "opus",
          permission_mode: "plan",
          effort: "max",
          agent: "reviewer",
          ...(mode === "fork" ? { fork_session: true } : {}),
        },
      });
      const connected = await nextMatching(
          bridge,
          (event) => event.event === "connected" && event.request_id === `continue-${mode}`,
        ),
        entries = launchJournal(journal),
        query = entries.find((entry) => entry.type === "query"),
        launch = entries.find((entry) => entry.type === "launch");
      assert.deepEqual(query, {
        type: "query",
        cwd,
        ...(mode === "fresh" ? {} : { resume: "newest-here" }),
        model: "opus",
        effort: "max",
        permissionMode: "plan",
        agent: "reviewer",
      });
      assert.equal(launch?.forkSession, mode === "fork" ? true : undefined);
      assert.equal(launch?.sessionId, connected.session_id);
      const scoped = entries
        .filter((entry) => entry.type === "list")
        .map((entry) => entry.options as BridgeEnvelope)
        .find((options) => options.dir === cwd);
      assert.ok(scoped);
      assert.equal(scoped.includeProgrammatic, true);
      assert.equal(scoped.includeWorktrees, true);
      if (mode === "resume") assert.equal(connected.session_id, "newest-here");
      else {
        assert.equal(typeof connected.session_id, "string");
        assert.ok(connected.session_id);
        assert.notEqual(connected.session_id, "newest-here");
        assert.notEqual(connected.session_id, "other-project");
      }
    } finally {
      await cleanup();
      rmSync(directory, { recursive: true, force: true });
    }
  }
});

function launchJournal(journal: string): BridgeEnvelope[] {
  const source = readFileSync(journal, "utf8");
  // A native call can still be appending the last record during this snapshot.
  // Only newline-terminated records have crossed the journal boundary.
  const end = source.lastIndexOf("\n");
  if (end < 0) return [];
  return source.slice(0, end)
    .split("\n")
    .filter((line) => line.length > 0)
    .map((line) => JSON.parse(line) as BridgeEnvelope);
}

test("launch journal exposes complete records while native calls are still appending", () => {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), "launch-journal-")));
  const journal = join(directory, "calls.jsonl");
  try {
    writeFileSync(journal, "");
    assert.deepEqual(launchJournal(journal), []);
    const first = '{"type":"query","queryId":"initial"}\n';
    const next = Buffer.from('{"type":"query","queryId":"next-🦀"}\n');
    const split = next.indexOf(Buffer.from("🦀")) + 2;
    writeFileSync(journal, Buffer.concat([Buffer.from(first), next.subarray(0, split)]));
    assert.deepEqual(launchJournal(journal), [{ type: "query", queryId: "initial" }]);
    appendFileSync(journal, next.subarray(split));
    assert.deepEqual(launchJournal(journal), [
      { type: "query", queryId: "initial" },
      { type: "query", queryId: "next-🦀" },
    ]);
    appendFileSync(journal, '{"invalid"\n');
    assert.throws(() => launchJournal(journal), SyntaxError, "complete malformed records remain errors");
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("launch dispatch forwards explicit payloads and preserves SDK defaults when options are omitted", async () => {
  for (const explicit of [true, false]) {
    const directory = realpathSync(mkdtempSync(join(tmpdir(), "launch-forwarding-")));
    const journal = join(directory, "calls.jsonl");
    const { bridge, cleanup } = ultracodeFixtureBridge({ STARTUP_JOURNAL: journal });
    try {
      const request = explicit ? "launch-explicit" : "launch-defaults";
      bridge.writeCommand({
        command: "create_session",
        request_id: request,
        cwd: bridge.cwd,
        launch_settings: explicit
          ? {
              name: "Release prep",
              additional_directories: ["/work/shared", "/work/docs"],
              settings: {
                spinnerTipsEnabled: false,
                feedbackDrafts: "on",
                crossSessionInbound: "accept",
                custom: { preserved: [1, "two"] },
              },
              mcp_configs: [
                {
                  docs: { command: "docs-mcp", args: ["--literal"] },
                  search: { command: "search-v1" },
                },
                { search: { command: "search-v2", env: { MODE: "local" } } },
              ],
              strict_mcp_config: true,
              system_prompt: "  You are a release bot.\nRetain whitespace.  ",
              append_system_prompt: "End every reply with DONE.",
            }
          : {},
      });
      const connected = await nextMatching(
        bridge,
        (event) => event.event === "connected" && event.request_id === request,
      );
      const entries = launchJournal(journal),
        query = entries.find((entry) => entry.type === "query"),
        launch = entries.find((entry) => entry.type === "launch");
      assert.ok(launch && query);
      assert.equal(query.cwd, bridge.cwd);
      assert.equal(launch.sessionId, connected.session_id);
      assert.equal(launch.forkSession, undefined);
      assert.equal(launch.resume, undefined);
      if (explicit) {
        assert.equal(launch.title, "Release prep");
        assert.deepEqual(launch.additionalDirectories, ["/work/shared", "/work/docs"]);
        assert.deepEqual(launch.mcpServers, {
          docs: { command: "docs-mcp", args: ["--literal"] },
          search: { command: "search-v2", env: { MODE: "local" } },
        });
        assert.equal(launch.strictMcpConfig, true);
        assert.deepEqual(launch.settings, {
          spinnerTipsEnabled: false,
          feedbackDrafts: "off",
          crossSessionInbound: "accept",
          custom: { preserved: [1, "two"] },
        });
        const prompt = launch.systemPrompt as { type: string; prompt: string[]; snapshot: boolean };
        assert.equal(prompt.type, "custom");
        assert.equal(prompt.snapshot, true);
        assert.equal(prompt.prompt[0], "  You are a release bot.\nRetain whitespace.  ");
        assert.ok(prompt.prompt[1].endsWith("\n\nEnd every reply with DONE."));
        assert.ok(
          prompt.prompt[1].slice(0, -"\n\nEnd every reply with DONE.".length).trim().length > 0,
          "host guard precedes appended user text",
        );
      } else {
        for (const field of ["title", "additionalDirectories", "mcpServers", "strictMcpConfig"])
          assert.equal(Object.hasOwn(launch, field), false, field);
        assert.deepEqual(launch.settings, { feedbackDrafts: "off", crossSessionInbound: "refuse" });
        const prompt = launch.systemPrompt as {
          type: string;
          preset: string;
          append: string;
          snapshot: boolean;
        };
        assert.equal(prompt.type, "preset");
        assert.equal(prompt.preset, "claude_code");
        assert.equal(prompt.snapshot, true);
        assert.ok(prompt.append.trim().length > 0);
      }
    } finally {
      await cleanup();
      rmSync(directory, { recursive: true, force: true });
    }
  }
});

test("launch titles belong to the fresh or forked identity while plain resume renames its saved owner", async () => {
  for (const mode of ["fresh", "resume", "fork"]) {
    const directory = realpathSync(mkdtempSync(join(tmpdir(), "title-ownership-"))),
      journal = join(directory, "calls.jsonl"),
      cwd = join(directory, "project");
    mkdirSync(cwd);
    const { bridge, cleanup } = ultracodeFixtureBridge({
      STARTUP_JOURNAL: journal,
      STARTUP_SESSIONS_FIXTURE: JSON.stringify([
        { sessionId: "saved-session", cwd, lastModified: 1, customTitle: "Original" },
      ]),
    });
    try {
      const request = `title-${mode}`;
      bridge.writeCommand({
        command: mode === "fresh" ? "create_session" : "resume_session",
        request_id: request,
        ...(mode === "fresh" ? { cwd } : { session_id: "saved-session" }),
        launch_settings: { name: "Experiment", ...(mode === "fork" ? { fork_session: true } : {}) },
      });
      const connected = await nextMatching(
          bridge,
          (event) => event.event === "connected" && event.request_id === request,
        ),
        sessionId = connected.session_id;
      assert.equal(typeof sessionId, "string");
      assert.ok(sessionId);
      if (mode === "resume") assert.equal(sessionId, "saved-session");
      else assert.notEqual(sessionId, "saved-session");
      const reported = await nextMatching(
        bridge,
        (event) =>
          event.event === "session_update" &&
          event.session_id === sessionId &&
          (event.update as BridgeEnvelope)?.type === "session_title_update" &&
          (event.update as BridgeEnvelope).title === "Experiment",
      );
      assert.equal((reported.update as BridgeEnvelope).title, "Experiment");
      const calls = launchJournal(journal),
        launch = calls.find((entry) => entry.type === "launch");
      assert.ok(launch);
      assert.equal(launch.sessionId, sessionId);
      assert.equal(launch.title, mode === "resume" ? undefined : "Experiment");
      assert.equal(launch.forkSession, mode === "fork" ? true : undefined);
      assert.deepEqual(
        calls.filter((entry) => entry.type === "rename"),
        mode === "resume"
          ? [{ type: "rename", sessionId: "saved-session", title: "Experiment", dir: cwd }]
          : [],
      );
      assert.ok(
        calls.some(
          (entry) =>
            entry.type === "title-read" &&
            entry.sessionId === sessionId &&
            entry.dir === cwd &&
            entry.title === "Experiment",
        ),
      );
      bridge.writeCommand({ command: "initialize", request_id: `readback-${mode}`, cwd });
      const listed = await nextMatching(
          bridge,
          (event) => event.event === "sessions_listed" && event.request_id === `readback-${mode}`,
        ),
        sessions = listed.sessions as BridgeEnvelope[];
      assert.equal(
        sessions.find((entry) => entry.session_id === sessionId)?.custom_title,
        "Experiment",
      );
      if (mode !== "resume")
        assert.equal(
          sessions.find((entry) => entry.session_id === "saved-session")?.custom_title,
          "Original",
          "naming a new identity cannot mutate the original saved title",
        );
      const phase = bridge.events.length;
      bridge.writeCommand({
        command: "rename_session",
        session_id: sessionId,
        title: "",
        request_id: `clear-${mode}`,
      });
      const cleared = await nextMatching(
        bridge,
        (event) =>
          event.event === "session_update" &&
          event.session_id === sessionId &&
          (event.update as BridgeEnvelope)?.type === "session_title_update",
      );
      assert.equal((cleared.update as BridgeEnvelope).title, "");
      const clearedList = await nextMatching(
        bridge,
        (event) => event.event === "sessions_listed" && event.request_id === `clear-${mode}`,
      );
      assert.equal(
        (clearedList.sessions as BridgeEnvelope[]).find((entry) => entry.session_id === sessionId)
          ?.custom_title,
        undefined,
      );
      assert.deepEqual(
        launchJournal(journal)
          .filter((entry) => entry.type === "rename")
          .at(-1),
        { type: "rename", sessionId, title: "", dir: cwd },
      );
      assert.ok(
        bridge.events
          .slice(phase)
          .filter((event) => (event.update as BridgeEnvelope)?.type === "session_title_update")
          .every(
            (event) =>
              event.session_id === sessionId && (event.update as BridgeEnvelope).title === "",
          ),
      );
      if (mode !== "resume")
        assert.equal(
          (clearedList.sessions as BridgeEnvelope[]).find(
            (entry) => entry.session_id === "saved-session",
          )?.custom_title,
          "Original",
        );
    } finally {
      await cleanup();
      rmSync(directory, { recursive: true, force: true });
    }
  }
});

test("applied effort publications follow native requests, capped reads, turn observations and new query identity", async () => {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), "effort-phases-"))),
    journal = join(directory, "calls.jsonl"),
    { bridge, cleanup } = ultracodeFixtureBridge({
      EFFORT_FIXTURE_CAP: "high",
      STARTUP_JOURNAL: journal,
    });
  const effort = async (start: number, session: unknown, expected: unknown) => {
    const event = await phaseEvent(
      bridge,
      start,
      (event) =>
        event.session_id === session &&
        (event.update as BridgeEnvelope)?.type === "config_option_update" &&
        (event.update as BridgeEnvelope).option_id === "effortLevel",
    );
    assert.equal((event.update as BridgeEnvelope).value, expected);
  };
  try {
    const initial = bridge.events.length;
    bridge.writeCommand({
      command: "create_session",
      request_id: "effort-create",
      cwd: bridge.cwd,
      launch_settings: { model: "opus" },
    });
    const connected = await nextMatching(
      bridge,
      (event) => event.event === "connected" && event.request_id === "effort-create",
    );
    await effort(initial, connected.session_id, "high");
    let phase = bridge.events.length,
      offset = launchJournal(journal).length;
    bridge.writeCommand({
      command: "set_effort",
      request_id: "capped-max",
      session_id: connected.session_id,
      effort: "max",
    });
    await effort(phase, connected.session_id, "high");
    assert.deepEqual(
      launchJournal(journal)
        .slice(offset)
        .filter((call) => call.type === "flags")
        .map((call) => call.settings),
      [{ effortLevel: "max" }],
    );
    assert.ok(
      launchJournal(journal)
        .slice(offset)
        .some((call) => call.type === "applied-read" && call.effort === "high"),
    );
    phase = bridge.events.length;
    bridge.writeCommand({
      command: "set_model",
      request_id: "unsupported-effort-model",
      session_id: connected.session_id,
      model: "haiku",
    });
    await effort(phase, connected.session_id, null);
    phase = bridge.events.length;
    bridge.writeCommand({
      command: "set_model",
      request_id: "restore-effort-model",
      session_id: connected.session_id,
      model: "opus",
    });
    await effort(phase, connected.session_id, "high");
    phase = bridge.events.length;
    offset = launchJournal(journal).length;
    bridge.writeCommand({
      command: "prompt",
      request_id: "effort-turn",
      session_id: connected.session_id,
      message_uuid: "per-turn",
      chunks: [{ kind: "text", value: "fixture per-turn effort" }],
    });
    await effort(phase, connected.session_id, "xhigh");
    assert.ok(
      launchJournal(journal)
        .slice(offset)
        .some(
          (call) =>
            call.type === "prompt" &&
            call.uuid === "per-turn" &&
            call.queryId === connected.session_id &&
            call.text === "fixture per-turn effort",
        ),
    );
    assert.ok(
      launchJournal(journal)
        .slice(offset)
        .some(
          (call) =>
            call.type === "turn-init" &&
            call.queryId === connected.session_id &&
            call.promptUuid === "per-turn" &&
            call.effort === "xhigh",
        ),
    );
    phase = bridge.events.length;
    offset = launchJournal(journal).length;
    bridge.writeCommand({
      command: "resume_session",
      request_id: "effort-resume",
      session_id: connected.session_id,
      launch_settings: { model: "opus" },
    });
    const resumed = await nextMatching(
      bridge,
      (event) => event.event === "session_replaced" && event.request_id === "effort-resume",
    );
    assert.equal(resumed.session_id, connected.session_id);
    await effort(phase, resumed.session_id, "high");
    assert.equal(
      launchJournal(journal)
        .slice(offset)
        .find((call) => call.type === "query")?.resume,
      connected.session_id,
    );
    phase = bridge.events.length;
    offset = launchJournal(journal).length;
    bridge.writeCommand({
      command: "new_session",
      request_id: "effort-fresh",
      cwd: bridge.cwd,
      launch_settings: { model: "opus" },
    });
    const fresh = await nextMatching(
      bridge,
      (event) => event.event === "session_replaced" && event.request_id === "effort-fresh",
    );
    assert.notEqual(fresh.session_id, connected.session_id);
    await effort(phase, fresh.session_id, "high");
    assert.equal(
      Object.hasOwn(
        launchJournal(journal)
          .slice(offset)
          .find((call) => call.type === "query") ?? {},
        "resume",
      ),
      false,
    );
  } finally {
    await cleanup();
    rmSync(directory, { recursive: true, force: true });
  }
});

test("scheduled Ultracode commands preserve native state across reset, resume, replacement and inactive acceptance", async () => {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), "ultracode-lifecycle-")));
  try {
    for (const unavailable of [false, true]) {
      const journal = join(directory, `${unavailable}.jsonl`),
        { bridge, cleanup } = ultracodeFixtureBridge({ STARTUP_JOURNAL: journal });
      try {
        bridge.writeCommand({
          command: "create_session",
          request_id: "ultra-create",
          cwd: bridge.cwd,
          launch_settings: { model: unavailable ? "fixture-unavailable" : "opus" },
        });
        const connected = await nextMatching(
          bridge,
          (event) => event.event === "connected" && event.request_id === "ultra-create",
        );
        let sessionId = connected.session_id;
        const originalId = sessionId;
        assert.deepEqual(
          connected.ultracode,
          unavailable
            ? { available: false, requested: false, effective: false }
            : { available: true, requested: false, effective: false },
        );
        const observe = async (phase: number, expected: unknown) => {
          const event = await phaseEvent(
            bridge,
            phase,
            (event) =>
              event.session_id === sessionId &&
              (event.update as BridgeEnvelope)?.type === "ultracode_update",
          );
          assert.deepEqual((event.update as BridgeEnvelope).ultracode, expected);
        };
        const active = { available: true, requested: true, effective: true },
          requested = { available: false, requested: true, effective: false };
        if (unavailable) {
          for (const request of ["inactive-enable", "inactive-retry"]) {
            const phase = bridge.events.length,
              offset = launchJournal(journal).length;
            bridge.writeCommand({
              command: "set_ultracode",
              request_id: request,
              session_id: sessionId,
              enabled: true,
            });
            await observe(phase, requested);
            const error = await phaseEvent(
              bridge,
              phase,
              (event) =>
                event.event === "slash_error" &&
                event.session_id === sessionId &&
                event.request_id === request,
            );
            assert.match(String(error.message), /saved.*request.*inactive/i);
            assert.deepEqual(
              launchJournal(journal)
                .slice(offset)
                .filter((call) => call.type === "flags")
                .map((call) => call.settings),
              [{ ultracode: true }],
            );
          }
          let phase = bridge.events.length;
          bridge.writeCommand({
            command: "refresh_ultracode",
            request_id: "inactive-read",
            session_id: sessionId,
          });
          await observe(phase, requested);
          phase = bridge.events.length;
          bridge.writeCommand({
            command: "set_ultracode",
            request_id: "inactive-disable",
            session_id: sessionId,
            enabled: false,
          });
          await observe(phase, { available: false, requested: false, effective: false });
        } else {
          let phase = bridge.events.length,
            offset = launchJournal(journal).length;
          bridge.writeRaw(
            [
              {
                command: "set_ultracode",
                session_id: sessionId,
                enabled: true,
                request_id: "ordered-enable",
              },
              {
                command: "set_effort",
                session_id: sessionId,
                effort: "low",
                request_id: "ordered-effort",
              },
            ]
              .map((command) => `${JSON.stringify(command)}\n`)
              .join(""),
          );
          await phaseEvent(
            bridge,
            phase,
            (event) =>
              event.session_id === sessionId &&
              (event.update as BridgeEnvelope)?.type === "config_option_update" &&
              (event.update as BridgeEnvelope).option_id === "effortLevel" &&
              (event.update as BridgeEnvelope).value === "low",
          );
          await observe(phase, active);
          assert.deepEqual(
            launchJournal(journal)
              .slice(offset)
              .filter((call) => call.type === "flags")
              .map((call) => call.settings),
            [{ ultracode: true }, { effortLevel: "low", ultracode: true }],
          );
          phase = bridge.events.length;
          bridge.writeCommand({
            command: "prompt",
            request_id: "ultra-reset",
            session_id: sessionId,
            message_uuid: "reset-message",
            chunks: [{ kind: "text", value: "fixture conversation reset" }],
          });
          await phaseEvent(
            bridge,
            phase,
            (event) =>
              event.session_id === originalId &&
              (event.update as BridgeEnvelope)?.type === "conversation_reset",
          );
          const reset = await phaseEvent(
            bridge,
            phase,
            (event) => event.event === "session_replaced",
          );
          assert.equal(reset.session_id, "fixture-conversation-2");
          sessionId = reset.session_id;
          await observe(phase, active);
          phase = bridge.events.length;
          bridge.writeCommand({
            command: "set_model",
            request_id: "ultra-haiku",
            session_id: sessionId,
            model: "haiku",
          });
          await observe(phase, requested);
          phase = bridge.events.length;
          offset = launchJournal(journal).length;
          bridge.writeCommand({
            command: "set_effort",
            request_id: "unsupported-effort",
            session_id: sessionId,
            effort: "max",
          });
          await phaseEvent(
            bridge,
            phase,
            (event) =>
              event.event === "slash_error" &&
              event.session_id === sessionId &&
              event.request_id === "unsupported-effort",
          );
          assert.equal(
            launchJournal(journal)
              .slice(offset)
              .some((call) => call.type === "flags"),
            false,
            "unsupported effort cannot reach native mutation",
          );
          phase = bridge.events.length;
          bridge.writeCommand({
            command: "set_ultracode",
            request_id: "unsupported-model",
            session_id: sessionId,
            enabled: true,
          });
          const error = await phaseEvent(
            bridge,
            phase,
            (event) =>
              event.event === "slash_error" &&
              event.session_id === sessionId &&
              event.request_id === "unsupported-model",
          );
          assert.match(String(error.message), /haiku.*not support/i);
          phase = bridge.events.length;
          bridge.writeCommand({
            command: "set_model",
            request_id: "ultra-opus",
            session_id: sessionId,
            model: "opus",
          });
          await observe(phase, active);
          phase = bridge.events.length;
          bridge.writeCommand({
            command: "resume_session",
            request_id: "ultra-resume",
            session_id: sessionId,
            launch_settings: { model: "opus" },
          });
          const resumed = await phaseEvent(
            bridge,
            phase,
            (event) => event.event === "session_replaced" && event.request_id === "ultra-resume",
          );
          assert.equal(resumed.session_id, sessionId);
          assert.deepEqual(resumed.ultracode, active);
          phase = bridge.events.length;
          offset = launchJournal(journal).length;
          bridge.writeCommand({
            command: "set_ultracode",
            request_id: "ultra-disable",
            session_id: sessionId,
            enabled: false,
          });
          await observe(phase, { available: true, requested: false, effective: false });
          assert.deepEqual(
            launchJournal(journal)
              .slice(offset)
              .filter((call) => call.type === "flags")
              .map((call) => call.settings),
            [{ ultracode: false }],
          );
          phase = bridge.events.length;
          bridge.writeCommand({
            command: "new_session",
            request_id: "ultra-fresh",
            cwd: bridge.cwd,
            launch_settings: { model: "opus" },
          });
          const fresh = await phaseEvent(
            bridge,
            phase,
            (event) => event.event === "session_replaced" && event.request_id === "ultra-fresh",
          );
          assert.notEqual(fresh.session_id, sessionId);
          assert.deepEqual(fresh.ultracode, {
            available: true,
            requested: false,
            effective: false,
          });
          for (const closed of [originalId, sessionId]) {
            bridge.writeCommand({
              command: "refresh_ultracode",
              request_id: `closed-${closed}`,
              session_id: closed,
            });
            const refused = await nextMatching(
              bridge,
              (event) =>
                event.event === "slash_error" &&
                event.session_id === closed &&
                event.request_id === `closed-${closed}`,
            );
            assert.match(String(refused.message), /no active session/i);
          }
        }
        bridge.writeCommand({ command: "shutdown" });
        assert.equal(await bridge.waitForExit(), 0);
      } finally {
        await cleanup();
      }
    }
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("Ultracode wire failures distinguish rejected mutation from accepted unknown state and recover", async () => {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), "ultracode-failures-")));
  try {
    for (const failure of ["workflows", "model", "protocol", "unverified"]) {
      const journal = join(directory, `${failure}.jsonl`),
        { bridge, cleanup } = ultracodeFixtureBridge({
          STARTUP_JOURNAL: journal,
          ...(failure === "unverified" ? {} : { ULTRACODE_NATIVE_FAILURE: failure }),
        });
      try {
        bridge.writeCommand({
          command: "create_session",
          request_id: `ultra-${failure}`,
          cwd: bridge.cwd,
          launch_settings: { model: "opus" },
        });
        const connected = await nextMatching(
            bridge,
            (event) => event.event === "connected" && event.request_id === `ultra-${failure}`,
          ),
          sessionId = connected.session_id;
        const state = () =>
          nextMatching(
            bridge,
            (event) =>
              event.session_id === sessionId &&
              (event.update as BridgeEnvelope)?.type === "ultracode_update",
          );
        assert.deepEqual(connected.ultracode, {
          available: true,
          requested: false,
          effective: false,
        });
        bridge.writeCommand({
          command: "set_ultracode",
          session_id: sessionId,
          enabled: true,
          request_id: "establish-active",
        });
        assert.deepEqual(((await state()).update as BridgeEnvelope).ultracode, {
          available: true,
          requested: true,
          effective: true,
        });
        if (failure === "unverified") {
          bridge.writeCommand({
            command: "set_model",
            session_id: sessionId,
            model: "fixture-read-failure",
            request_id: "break-read",
          });
          assert.equal(((await state()).update as BridgeEnvelope).ultracode, null);
          await nextMatching(
            bridge,
            (event) =>
              event.event === "slash_error" &&
              event.session_id === sessionId &&
              event.request_id === "break-read",
          );
          const offset = launchJournal(journal).length;
          bridge.writeCommand({
            command: "set_ultracode",
            session_id: sessionId,
            enabled: false,
            request_id: "accepted-unverified",
          });
          assert.equal(((await state()).update as BridgeEnvelope).ultracode, null);
          const error = await nextMatching(
            bridge,
            (event) =>
              event.event === "slash_error" &&
              event.session_id === sessionId &&
              event.request_id === "accepted-unverified",
          );
          assert.match(String(error.message), /accepted.*could not be verified/i);
          const calls = launchJournal(journal).slice(offset),
            apply = calls.findIndex(
              (call) =>
                call.type === "flags" && (call.settings as BridgeEnvelope)?.ultracode === false,
            ),
            read = calls.findIndex(
              (call) => call.type === "applied-read" && call.model === "fixture-read-failure",
            );
          assert.ok(apply >= 0 && read > apply, "native acceptance precedes its failing read");
          bridge.writeCommand({
            command: "set_model",
            session_id: sessionId,
            model: "opus",
            request_id: "restore-readable",
          });
          assert.deepEqual(((await state()).update as BridgeEnvelope).ultracode, {
            available: true,
            requested: false,
            effective: false,
          });
        } else {
          const phase = bridge.events.length,
            offset = launchJournal(journal).length;
          bridge.writeCommand({
            command: "set_ultracode",
            session_id: sessionId,
            enabled: true,
            request_id: `rejected-${failure}`,
          });
          const error = await nextMatching(
            bridge,
            (event) =>
              event.event === "slash_error" &&
              event.session_id === sessionId &&
              event.request_id === `rejected-${failure}`,
          );
          assert.match(
            String(error.message),
            failure === "workflows"
              ? /workflows.*disabled/i
              : failure === "model"
                ? /fixture-only-model.*not support/i
                : /protocol error/i,
          );
          assert.ok(
            !JSON.stringify(bridge.events.slice(phase)).includes("RAW_PROTOCOL_PRIVATE_SENTINEL"),
          );
          assert.equal(
            bridge.events
              .slice(phase)
              .some(
                (event) =>
                  event.session_id === sessionId &&
                  (event.update as BridgeEnvelope)?.type === "ultracode_update",
              ),
            false,
            "a native rejection cannot clear the verified snapshot",
          );
          const calls = launchJournal(journal).slice(offset);
          assert.deepEqual(
            calls.filter((call) => call.type === "flags").map((call) => call.settings),
            [{ ultracode: true }],
          );
          assert.equal(
            calls.some((call) => call.type === "applied-read"),
            false,
          );
          bridge.writeCommand({
            command: "refresh_ultracode",
            session_id: sessionId,
            request_id: "verify-retained",
          });
          assert.deepEqual(((await state()).update as BridgeEnvelope).ultracode, {
            available: true,
            requested: true,
            effective: true,
          });
        }
        bridge.writeCommand({
          command: "set_ultracode",
          session_id: sessionId,
          enabled: true,
          request_id: "explicit-retry",
        });
        assert.deepEqual(((await state()).update as BridgeEnvelope).ultracode, {
          available: true,
          requested: true,
          effective: true,
        });
        bridge.writeCommand({ command: "shutdown" });
        assert.equal(await bridge.waitForExit(), 0);
        assert.equal(
          bridge.events.some((event) => event.event === "connection_failed"),
          false,
        );
      } finally {
        await cleanup();
      }
    }
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("real SDK resolution and host checkpoint options remain associated with each query lifecycle phase", async () => {
  const directory = mkdtempSync(join(tmpdir(), "claude-rs-settings-"));
  const profile = join(directory, "profile");
  const cwd = join(directory, "project");
  mkdirSync(profile);
  mkdirSync(join(cwd, ".claude"), { recursive: true });
  const journal = join(directory, "query-options.jsonl");
  const user = {
    model: "haiku",
    alwaysThinkingEnabled: true,
    fastMode: true,
    outputStyle: "User custom style",
    language: "German",
    fileCheckpointingEnabled: true,
    sandbox: { enabled: true, failIfUnavailable: true },
    modelSettings: { "claude-opus-5-5": { effortLevel: "low" } },
  };
  const project = { model: "sonnet", language: "Japanese", permissions: { defaultMode: "plan" } };
  const local = {
    model: "opus",
    alwaysThinkingEnabled: false,
    fastMode: false,
    outputStyle: "Project custom style",
    fileCheckpointingEnabled: false,
    modelSettings: { "claude-opus-5-5": { effortLevel: "xhigh" } },
    crossSessionInbound: "refuse",
  };
  const userPath = join(profile, "settings.json");
  const projectPath = join(cwd, ".claude", "settings.json");
  const localPath = join(cwd, ".claude", "settings.local.json");
  for (const [path, value] of [
    [userPath, user],
    [projectPath, project],
    [localPath, local],
  ] as const) {
    writeFileSync(path, JSON.stringify(value));
  }
  const savedFiles = [userPath, projectPath, localPath].map((path) => readFileSync(path, "utf8"));
  const { bridge, cleanup } = ultracodeFixtureBridge({
    CLAUDE_CONFIG_DIR: profile,
    STARTUP_JOURNAL: journal,
    SETTINGS_JOURNAL: "1",
  });
  const snapshots = () =>
    readFileSync(journal, "utf8")
      .trim()
      .split("\n")
      .map((line) => JSON.parse(line) as BridgeEnvelope)
      .filter((entry) => entry.type === "settings");
  try {
    let sourceId: unknown;
    for (const operation of ["create_session", "resume_session", "new_session"]) {
      const offset = operation === "create_session" ? 0 : launchJournal(journal).length;
      bridge.writeCommand({
        command: operation,
        request_id: `layers-${operation}`,
        ...(operation === "resume_session" ? { session_id: sourceId } : { cwd }),
      });
      const connected = await nextMatching(
        bridge,
        (event) =>
          event.event === (operation === "create_session" ? "connected" : "session_replaced") &&
          event.request_id === `layers-${operation}`,
      );
      const entries = launchJournal(journal).slice(offset),
        snapshot = entries.find(
          (entry) => entry.type === "settings" && entry.sessionId === connected.session_id,
        ),
        query = entries.find((entry) => entry.type === "query");
      assert.ok(snapshot);
      assert.equal(snapshot.checkpointing, false);
      assert.equal(query?.cwd, cwd);
      assert.equal(query?.resume, operation === "resume_session" ? sourceId : undefined);
      assert.equal(
        entries.find((entry) => entry.type === "launch")?.sessionId,
        connected.session_id,
      );
      if (operation === "create_session") sourceId = connected.session_id;
      assert.deepEqual(
        [userPath, projectPath, localPath].map((path) => readFileSync(path, "utf8")),
        savedFiles,
      );
    }
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
    assert.deepEqual(
      [userPath, projectPath, localPath].map((path) => readFileSync(path, "utf8")),
      savedFiles,
    );
    writeFileSync(localPath, JSON.stringify({ ...local, fileCheckpointingEnabled: true }));
    const changedBytes = readFileSync(localPath, "utf8"),
      offset = launchJournal(journal).length;
    bridge.writeCommand({ command: "new_session", request_id: "changed-checkpointing", cwd });
    const changed = await nextMatching(
      bridge,
      (event) => event.event === "session_replaced" && event.request_id === "changed-checkpointing",
    );
    assert.equal(
      launchJournal(journal)
        .slice(offset)
        .find((entry) => entry.type === "settings" && entry.sessionId === changed.session_id)
        ?.checkpointing,
      true,
    );
    assert.equal(readFileSync(userPath, "utf8"), savedFiles[0]);
    assert.equal(readFileSync(projectPath, "utf8"), savedFiles[1]);
    assert.equal(readFileSync(localPath, "utf8"), changedBytes);
  } finally {
    await cleanup();
    rmSync(directory, { recursive: true, force: true });
  }
});

test("scoped settings results correlate with exact saved, refused and inherited disk outcomes", async () => {
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
    const connected = await nextMatching(bridge, (event) => event.event === "connected");
    const session_id = connected.session_id;
    async function result(request_id: string, command: BridgeEnvelope): Promise<SettingsResult> {
      bridge.writeCommand({ ...command, session_id, request_id });
      const event = await nextMatching(
        bridge,
        (event) =>
          event.event === "settings_result" &&
          event.request_id === request_id &&
          event.session_id === session_id,
      );
      assert.equal(event.request_id, request_id);
      assert.equal(event.session_id, session_id);
      return event.result as SettingsResult;
    }
    const inspected = await result("inspect", { command: "inspect_settings" });
    assert.equal(inspected.persistence, "not_requested");
    assert.ok(inspected.snapshot);
    assert.equal(
      inspected.snapshot.values.find((value) => value.id === "language")?.value,
      "Japanese",
    );
    assert.equal(
      inspected.snapshot.values.find((value) => value.id === "fileCheckpointingEnabled")?.value,
      false,
    );
    function change(snapshot: SettingsSnapshot, value?: string): SettingsMutation {
      const previous = snapshot.sources
        .find((source) => source.scope === "local")
        ?.values.find((value) => value.id === "language");
      assert.ok(previous);
      return {
        context: snapshot.context,
        id: "language",
        scope: "local",
        expected_revision: previous.revision,
        operation: value === undefined ? "remove" : "set",
        ...(value === undefined ? {} : { value }),
      };
    }
    // A change to another owner's key between display and save must survive.
    writeFileSync(
      localPath,
      JSON.stringify({ language: "Japanese", future: { keep: true }, external: "preserve" }),
    );
    const saved = await result("save", {
      command: "mutate_setting",
      mutation: change(inspected.snapshot, "Greek"),
    });
    assert.equal(saved.persistence, "saved");
    assert.equal(saved.application, "next_session");
    assert.ok(saved.snapshot);
    assert.equal(saved.snapshot.values.find((value) => value.id === "language")?.value, "Greek");
    assert.deepEqual(JSON.parse(readFileSync(localPath, "utf8")), {
      language: "Greek",
      future: { keep: true },
      external: "preserve",
    });
    writeFileSync(
      localPath,
      JSON.stringify({ language: "French", future: { keep: true }, external: "preserve" }),
    );
    const externalBytes = readFileSync(localPath, "utf8");
    const conflict = await result("conflict", {
      command: "mutate_setting",
      mutation: change(saved.snapshot, "Italian"),
    });
    assert.equal(conflict.persistence, "conflict");
    assert.equal(readFileSync(localPath, "utf8"), externalBytes);
    assert.equal(
      readFileSync(userPath, "utf8"),
      JSON.stringify({ language: "German", fileCheckpointingEnabled: false }),
    );
    assert.ok(conflict.snapshot);
    assert.equal(
      conflict.snapshot.values.find((value) => value.id === "language")?.value,
      "French",
    );
    const reset = await result("reset", {
      command: "mutate_setting",
      mutation: change(conflict.snapshot),
    });
    assert.equal(reset.persistence, "saved");
    assert.equal(reset.snapshot?.values.find((value) => value.id === "language")?.value, "German");
    assert.deepEqual(JSON.parse(readFileSync(localPath, "utf8")), {
      future: { keep: true },
      external: "preserve",
    });
    for (const [id, value] of [
      ["permissions.deny", ["Read(./.env)"]],
      ["worktree.sparsePaths", ["src", "docs"]],
      ["hooks", { Stop: [{ hooks: [{ type: "command", command: "unused-preview-command" }] }] }],
    ] as const) {
      const fresh = await result(`inspect-${id}`, { command: "inspect_settings" });
      assert.ok(fresh.snapshot);
      const scoped = fresh.snapshot.sources
        .find((source) => source.scope === "local")
        ?.values.find((entry) => entry.id === id);
      assert.ok(scoped);
      const mutation = {
        context: fresh.snapshot.context,
        id,
        scope: "local",
        expected_revision: scoped.revision,
        operation: "set",
        value,
      };
      const saved = await result(`save-${id}`, { command: "mutate_setting", mutation });
      assert.equal(saved.persistence, "saved", saved.error ?? id);
      assert.deepEqual(saved.snapshot?.values.find((entry) => entry.id === id)?.value, value);
      const expected = {
        future: { keep: true },
        external: "preserve",
        permissions: { deny: ["Read(./.env)"] },
        ...(id !== "permissions.deny" ? { worktree: { sparsePaths: ["src", "docs"] } } : {}),
        ...(id === "hooks"
          ? {
              hooks: {
                Stop: [{ hooks: [{ type: "command", command: "unused-preview-command" }] }],
              },
            }
          : {}),
      };
      assert.deepEqual(JSON.parse(readFileSync(localPath, "utf8")), expected);
      assert.equal(
        readFileSync(userPath, "utf8"),
        JSON.stringify({ language: "German", fileCheckpointingEnabled: false }),
      );
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

test("managed policy refuses otherwise-valid scoped writes while unrelated settings remain editable", async () => {
  const directory = mkdtempSync(join(tmpdir(), "claude-rs-policy-flow-"));
  const profile = join(directory, "profile");
  const cwd = join(directory, "project");
  mkdirSync(profile);
  mkdirSync(cwd);
  const file = join(profile, "settings.json");
  const original = JSON.stringify({ language: "German", unrelated: true });
  writeFileSync(file, original);
  const { bridge, cleanup } = ultracodeFixtureBridge({
    CLAUDE_CONFIG_DIR: profile,
    SETTINGS_POLICY_FIXTURE: JSON.stringify({
      language: "Policy language",
      allowManagedPermissionRulesOnly: true,
      allowManagedHooksOnly: true,
      sandbox: {
        network: { allowManagedDomainsOnly: true },
        filesystem: { allowManagedReadPathsOnly: true },
      },
    }),
  });
  try {
    bridge.writeCommand({ command: "create_session", cwd });
    const connected = await nextMatching(bridge, (event) => event.event === "connected");
    bridge.writeCommand({
      command: "inspect_settings",
      session_id: connected.session_id,
      request_id: "policy-inspect",
    });
    const inspected = await nextMatching(
      bridge,
      (event) =>
        event.event === "settings_result" &&
        event.session_id === connected.session_id &&
        event.request_id === "policy-inspect",
    );
    const snapshot = (inspected.result as SettingsResult).snapshot;
    assert.ok(snapshot);
    assert.equal(
      snapshot.values.find((value) => value.id === "language")?.value,
      "Policy language",
    );
    assert.equal(snapshot.values.find((value) => value.id === "language")?.policy_restricted, true);
    assert.equal(snapshot.provenance.language?.source, "managed");
    assert.ok(snapshot.resolution_sources.some((source) => source.source === "managed"));
    assert.equal(
      snapshot.resolution_sources.find((source) => source.source === "managed")?.policy_origin,
      "remote",
    );
    assert.deepEqual(
      snapshot.catalog.find((setting) => setting.id === "language")?.writable_scopes,
      [],
    );
    const values: Array<[string, unknown]> = [
      ["permissions.allow", ["Bash(git status)"]],
      ["permissions.ask", ["Read(./private)"]],
      ["permissions.deny", ["Write(./locked)"]],
      ["hooks", { Stop: [{ hooks: [{ type: "command", command: "unused-hook" }] }] }],
      ["sandbox.network.allowedDomains", ["example.com"]],
      ["sandbox.network.httpProxyPort", 3128],
      ["sandbox.network.socksProxyPort", 1080],
      ["sandbox.filesystem.allowRead", ["/owned-extra"]],
    ];
    for (const [id, value] of values) {
      const descriptor: SettingsSnapshot["catalog"][number] | undefined = snapshot.catalog.find(
        (setting) => setting.id === id,
      );
      assert.ok(descriptor);
      assert.equal(snapshot.values.find((row) => row.id === id)?.policy_restricted, true);
      assert.deepEqual(descriptor.writable_scopes, []);
      await validateSettingValue(descriptor, value as import("./types.js").Json);
      const previous: SettingsSnapshot["sources"][number]["values"][number] | undefined =
        snapshot.sources
          .find((source) => source.scope === "user")
          ?.values.find((row) => row.id === id);
      assert.ok(previous);
      bridge.writeCommand({
        command: "mutate_setting",
        session_id: connected.session_id,
        request_id: id,
        mutation: {
          context: snapshot.context,
          id,
          scope: "user",
          expected_revision: previous.revision,
          operation: "set",
          value,
        },
      });
      const rejected = await nextMatching(
        bridge,
        (event) =>
          event.event === "settings_result" &&
          event.session_id === connected.session_id &&
          event.request_id === id,
      );
      assert.equal((rejected.result as SettingsResult).persistence, "failure", id);
      assert.equal((rejected.result as SettingsResult).application, "blocked");
      assert.equal(readFileSync(file, "utf8"), original, id);
    }
    assert.deepEqual(
      snapshot.catalog.find((setting) => setting.id === "permissions.additionalDirectories")
        ?.writable_scopes,
      ["user", "project", "local"],
    );
    const previous = snapshot.sources
      .find((source) => source.scope === "user")
      ?.values.find((value) => value.id === "language");
    assert.ok(previous);
    bridge.writeCommand({
      command: "mutate_setting",
      session_id: connected.session_id,
      request_id: "policy-save",
      mutation: {
        context: snapshot.context,
        id: "language",
        scope: "user",
        expected_revision: previous.revision,
        operation: "set",
        value: "French",
      },
    });
    const rejected = await nextMatching(
      bridge,
      (event) =>
        event.event === "settings_result" &&
        event.session_id === connected.session_id &&
        event.request_id === "policy-save",
    );
    assert.equal(rejected.request_id, "policy-save");
    assert.equal((rejected.result as SettingsResult).persistence, "failure");
    assert.equal(readFileSync(file, "utf8"), original);
    const editable = snapshot.sources
      .find((source) => source.scope === "user")
      ?.values.find((row) => row.id === "permissions.additionalDirectories");
    assert.ok(editable);
    bridge.writeCommand({
      command: "mutate_setting",
      session_id: connected.session_id,
      request_id: "policy-unrelated",
      mutation: {
        context: snapshot.context,
        id: "permissions.additionalDirectories",
        scope: "user",
        expected_revision: editable.revision,
        operation: "set",
        value: ["/owned-extra"],
      },
    });
    const permitted = await nextMatching(
      bridge,
      (event) =>
        event.event === "settings_result" &&
        event.session_id === connected.session_id &&
        event.request_id === "policy-unrelated",
    );
    assert.equal((permitted.result as SettingsResult).persistence, "saved");
    assert.deepEqual(JSON.parse(readFileSync(file, "utf8")), {
      language: "German",
      unrelated: true,
      permissions: { additionalDirectories: ["/owned-extra"] },
    });
    assert.throws(() => readFileSync(join(cwd, ".claude", "settings.json")), { code: "ENOENT" });
    assert.throws(() => readFileSync(join(cwd, ".claude", "settings.local.json")), {
      code: "ENOENT",
    });
  } finally {
    await cleanup();
    rmSync(directory, { recursive: true, force: true });
  }
});

test("acknowledged session controls publish native outcomes without rewriting saved defaults", async () => {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), "session-controls-"))),
    profile = join(directory, "profile"),
    cwd = join(directory, "project"),
    journal = join(directory, "calls.jsonl");
  mkdirSync(profile);
  mkdirSync(cwd);
  const file = join(profile, "settings.json"),
    persisted = {
      model: "opus",
      modelSettings: { "claude-opus-5-5": { effortLevel: "low" } },
      alwaysThinkingEnabled: true,
      agent: "reviewer",
      fastMode: false,
      permissions: { defaultMode: "plan" },
    },
    bytes = JSON.stringify(persisted);
  writeFileSync(file, bytes);
  try {
    const { bridge, cleanup } = ultracodeFixtureBridge({
      CLAUDE_CONFIG_DIR: profile,
      STARTUP_JOURNAL: journal,
      SESSION_SETTINGS_FIXTURE: "1",
      MODEL_STEP_DOWN_FIXTURE: "1",
      FAST_READ_FAILURE_FIXTURE: "1",
    });
    try {
      bridge.writeCommand({
        command: "create_session",
        request_id: "controls-create",
        cwd,
        launch_settings: {},
      });
      const connected = await nextMatching(
        bridge,
        (event) => event.event === "connected" && event.request_id === "controls-create",
      );
      let sessionId = connected.session_id;
      assert.equal((connected.mode as BridgeEnvelope).current_mode_id, "plan");
      assert.ok(
        ((connected.mode as BridgeEnvelope).available_modes as BridgeEnvelope[]).some(
          (mode) => mode.id === "auto",
        ),
      );
      const agents = (
        await phaseEvent(
          bridge,
          0,
          (event) =>
            event.session_id === sessionId &&
            (event.update as BridgeEnvelope)?.type === "available_agents_update",
        )
      ).update as BridgeEnvelope;
      assert.deepEqual(
        (agents.agents as BridgeEnvelope[]).find((agent) => agent.name === "reviewer"),
        { name: "reviewer", description: "Review code", model: "sonnet" },
      );
      assert.equal(
        (
          (
            await phaseEvent(
              bridge,
              0,
              (event) =>
                event.session_id === sessionId &&
                (event.update as BridgeEnvelope)?.type === "config_option_update" &&
                (event.update as BridgeEnvelope).option_id === "effortLevel",
            )
          ).update as BridgeEnvelope
        ).value,
        "low",
      );
      const control = async (
        request: string,
        command: BridgeEnvelope,
        type: string,
        option?: string,
      ) => {
        const phase = bridge.events.length,
          offset = launchJournal(journal).length;
        bridge.writeCommand({ ...command, session_id: sessionId, request_id: request });
        const event = await phaseEvent(
          bridge,
          phase,
          (event) =>
            event.session_id === sessionId &&
            (type === "slash_error"
              ? event.event === type && event.request_id === request
              : (event.update as BridgeEnvelope)?.type === type &&
                (!option || (event.update as BridgeEnvelope).option_id === option)),
        );
        // Lifecycle scheduling waits for the active session-command tail. Its
        // correlated listing fences every publication from this control operation.
        const settledRequest = `settled-${request}`;
        bridge.writeCommand({ command: "initialize", request_id: settledRequest, cwd });
        await phaseEvent(bridge, phase, event => event.event === "sessions_listed" && event.request_id === settledRequest);
        assert.equal(readFileSync(file, "utf8"), bytes, request);
        return {
          event,
          update: event.update as BridgeEnvelope,
          calls: launchJournal(journal).slice(offset),
          phase,
        };
      };
      const restricted = await control(
        "restricted-auto",
        { command: "set_mode", mode: "auto" },
        "mode_state_update",
      );
      assert.equal((restricted.update.mode as BridgeEnvelope).current_mode_id, "plan");
      assert.ok(
        !((restricted.update.mode as BridgeEnvelope).available_modes as BridgeEnvelope[]).some(
          (mode) => mode.id === "auto",
        ),
      );
      assert.deepEqual(
        restricted.calls.filter((call) => call.type === "mode"),
        [{ type: "mode", sessionId, value: "auto" }],
      );
      const modeError = await phaseEvent(
        bridge,
        restricted.phase,
        (event) =>
          event.event === "slash_error" &&
          event.session_id === sessionId &&
          event.request_id === "restricted-auto",
      );
      assert.match(String(modeError.message), /account restriction/);
      const observed = await control(
        "observe-mode",
        {
          command: "prompt",
          message_uuid: "observe-mode",
          chunks: [{ kind: "text", value: "fixture observe permissions" }],
        },
        "mode_state_update",
      );
      assert.equal((observed.update.mode as BridgeEnvelope).current_mode_id, "plan");
      for (const [enabled, expected] of [
        [false, false],
        [null, true],
      ]) {
        const result = await control(
          `thinking-${enabled}`,
          { command: "set_thinking", enabled },
          "config_option_update",
          "alwaysThinkingEnabled",
        );
        assert.equal(result.update.value, expected);
        assert.deepEqual(
          result.calls.filter((call) => call.type === "flags").map((call) => call.settings),
          [{ alwaysThinkingEnabled: enabled }],
        );
      }
      for (const [effort, expected] of [
        ["max", "max"],
        [null, "high"],
      ]) {
        const result = await control(
          `effort-${effort}`,
          { command: "set_effort", effort },
          "config_option_update",
          "effortLevel",
        );
        assert.equal(result.update.value, expected);
        assert.deepEqual(
          result.calls.filter((call) => call.type === "flags").map((call) => call.settings),
          [{ effortLevel: effort }],
        );
      }
      const model = await control(
        "stepped-model",
        { command: "set_model", model: "opus" },
        "current_model_update",
      );
      assert.equal((model.update.current_model as BridgeEnvelope).resolved_id, "claude-sonnet-5-5");
      assert.deepEqual(
        model.calls.filter((call) => call.type === "model"),
        [{ type: "model", sessionId, requested: "opus", observed: "sonnet" }],
      );
      const fast = await control(
        "fast-unverified",
        { command: "set_fast_mode", enabled: true },
        "fast_mode_update",
      );
      assert.equal(fast.update.fast_mode_state, "unknown");
      await phaseEvent(
        bridge,
        fast.phase,
        (event) =>
          event.event === "slash_error" &&
          event.session_id === sessionId &&
          event.request_id === "fast-unverified",
      );
      assert.deepEqual(
        fast.calls.filter((call) => call.type === "flags").map((call) => call.settings),
        [{ fastMode: true }],
      );
      assert.ok(fast.calls.some((call) => call.type === "fast-read" && call.failed === true));
      assert.equal(
        (
          await control(
            "fast-recover",
            { command: "set_fast_mode", enabled: false },
            "fast_mode_update",
          )
        ).update.fast_mode_state,
        "off",
      );
      const missing = await control(
        "missing-agent",
        { command: "set_agent", agent: "missing" },
        "slash_error",
      );
      assert.equal(
        missing.calls.some((call) => call.type === "flags"),
        false,
      );
      const agent = await control(
        "selected-agent",
        { command: "set_agent", agent: "reviewer" },
        "config_option_update",
        "agent",
      );
      assert.equal(agent.update.value, "reviewer");
      assert.deepEqual(
        agent.calls.filter((call) => call.type === "flags").map((call) => call.settings),
        [{ agent: "reviewer" }],
      );
      const mode = await control(
        "set-plan",
        { command: "set_mode", mode: "plan" },
        "mode_state_update",
      );
      assert.equal((mode.update.mode as BridgeEnvelope).current_mode_id, "plan");
      assert.deepEqual(
        mode.calls.filter((call) => call.type === "mode"),
        [{ type: "mode", sessionId, value: "plan" }],
      );
      const phase = bridge.events.length;
      bridge.writeCommand({
        command: "new_session",
        request_id: "controls-fresh",
        cwd,
        launch_settings: {},
      });
      const replacement = await nextMatching(
        bridge,
        (event) => event.event === "session_replaced" && event.request_id === "controls-fresh",
      );
      assert.notEqual(replacement.session_id, sessionId);
      sessionId = replacement.session_id;
      assert.equal(
        (
          (
            await phaseEvent(
              bridge,
              phase,
              (event) =>
                event.session_id === sessionId &&
                (event.update as BridgeEnvelope)?.type === "config_option_update" &&
                (event.update as BridgeEnvelope).option_id === "effortLevel",
            )
          ).update as BridgeEnvelope
        ).value,
        "low",
      );
      const freshAgents = (
        await phaseEvent(
          bridge,
          phase,
          (event) =>
            event.session_id === sessionId &&
            (event.update as BridgeEnvelope)?.type === "available_agents_update",
        )
      ).update as BridgeEnvelope;
      assert.ok(
        (freshAgents.agents as BridgeEnvelope[]).some((agent) => agent.name === "reviewer"),
      );
      const unsupported = await control(
        "haiku-model",
        { command: "set_model", model: "haiku" },
        "current_model_update",
      );
      assert.equal((unsupported.update.current_model as BridgeEnvelope).supports_effort, false);
      for (const [request, command, cause] of [
        [
          "haiku-effort",
          { command: "set_effort", effort: "low" },
          /supported by the current model/,
        ],
        ["haiku-fast", { command: "set_fast_mode", enabled: true }, /does not support fast mode/],
      ] as const) {
        const rejected = await control(request, command, "slash_error");
        assert.match(String(rejected.event.message), cause);
        assert.equal(
          rejected.calls.some((call) => call.type === "flags"),
          false,
        );
      }
    } finally {
      await cleanup();
    }
    const observed = ultracodeFixtureBridge({
      CLAUDE_CONFIG_DIR: profile,
      SESSION_SETTINGS_FIXTURE: "1",
      PERMISSION_MODE_FIXTURE: "auto",
    });
    try {
      observed.bridge.writeCommand({
        command: "create_session",
        cwd,
        request_id: "native-auto",
        launch_settings: {},
      });
      const connected = await nextMatching(
        observed.bridge,
        (event) => event.event === "connected" && event.request_id === "native-auto",
      );
      assert.equal((connected.mode as BridgeEnvelope).current_mode_id, "auto");
      observed.bridge.writeCommand({
        command: "inspect_settings",
        session_id: connected.session_id,
        request_id: "saved-plan",
      });
      const inspected = await nextMatching(
        observed.bridge,
        (event) =>
          event.event === "settings_result" &&
          event.session_id === connected.session_id &&
          event.request_id === "saved-plan",
      );
      assert.equal(
        (inspected.result as SettingsResult).snapshot?.values.find(
          (value) => value.id === "permissions.defaultMode",
        )?.value,
        "plan",
      );
      observed.bridge.writeCommand({
        command: "new_session",
        cwd,
        request_id: "native-auto-replace",
        launch_settings: {},
      });
      const replacement = await nextMatching(
        observed.bridge,
        (event) => event.event === "session_replaced" && event.request_id === "native-auto-replace",
      );
      assert.equal((replacement.mode as BridgeEnvelope).current_mode_id, "auto");
      assert.equal(readFileSync(file, "utf8"), bytes);
    } finally {
      await observed.cleanup();
    }
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("subagent wire commands target exact transcripts and settle stops only from native lifecycle outcomes", async () => {
  const directory = mkdtempSync(join(tmpdir(), "claude-rs-subagents-"));
  const journal = join(directory, "sdk.jsonl");
  const started = (taskId: string, taskType: string) => ({
    type: "system",
    subtype: "task_started",
    uuid: crypto.randomUUID(),
    task_id: taskId,
    ...(taskId === "agent-running" ? { run_id: "B" } : {}),
    task_type: taskType,
    description: `Run ${taskId}`,
    ...(taskType === "local_agent" ? { subagent_type: "general-purpose" } : {}),
  });
  const { bridge, cleanup } = ultracodeFixtureBridge({
    STARTUP_JOURNAL: journal,
    BACKGROUND_TASK_FIXTURE: JSON.stringify({
      start: [
        started("agent-running", "local_agent"),
        started("agent-closing", "local_agent"),
        started("shell-1", "local_bash"),
        {
          type: "result",
          subtype: "success",
          session_id: "ignored",
          uuid: "start-result",
          result: "Tasks admitted",
          user_message_uuid: "subagent-start",
          user_message_uuids: ["subagent-start"],
          usage: {},
          modelUsage: {},
          permission_denials: [],
          duration_ms: 1,
          num_turns: 1,
        },
      ],
      barrier: [
        {
          type: "system",
          subtype: "task_progress",
          task_id: "agent-closing",
          description: "Sibling observed after stop acknowledgement",
        },
        {
          type: "result",
          subtype: "success",
          uuid: "barrier-result",
          result: "Observed",
          user_message_uuid: "stop-barrier",
          user_message_uuids: ["stop-barrier"],
          usage: {},
          modelUsage: {},
          permission_denials: [],
          duration_ms: 1,
          num_turns: 1,
        },
      ],
      finish: [
        {
          type: "system",
          subtype: "task_notification",
          task_id: "agent-running",
          run_id: "B",
          status: "stopped",
          summary: "Stopped by native lifecycle",
        },
        {
          type: "result",
          subtype: "success",
          uuid: "finish-result",
          result: "Outcome delivered",
          user_message_uuid: "stop-outcome",
          user_message_uuids: ["stop-outcome"],
          usage: {},
          modelUsage: {},
          permission_denials: [],
          duration_ms: 1,
          num_turns: 1,
        },
      ],
    }),
  });
  const sdkCalls = (type: string) =>
    readFileSync(journal, "utf8")
      .trim()
      .split("\n")
      .map((line) => JSON.parse(line) as BridgeEnvelope)
      .filter((call) => call.type === type);
  try {
    bridge.writeCommand({
      command: "create_session",
      request_id: "subagent-session",
      cwd: bridge.cwd,
      launch_settings: {},
    });
    const connected = await nextMatching(
      bridge,
      (event) => event.event === "connected" && event.request_id === "subagent-session",
    );
    const sessionId = String(connected.session_id);
    const result = async (name: string, command: BridgeEnvelope) => {
      const requestId = crypto.randomUUID();
      bridge.writeCommand({ ...command, session_id: sessionId, request_id: requestId });
      const event = await nextMatching(
        bridge,
        (event) =>
          event.event === name && event.request_id === requestId && event.session_id === sessionId,
      );
      assert.equal(event.request_id, requestId);
      assert.equal(event.session_id, sessionId);
      return event;
    };

    const listed = await result("subagents_listed", { command: "list_subagents" });
    assert.deepEqual(listed.agent_ids, ["agent-running", "agent-done"]);
    assert.equal(listed.error, undefined);
    assert.deepEqual(
      sdkCalls("listSubagents").map((call) => [call.sessionId, call.dir]),
      [[sessionId, bridge.cwd]],
    );

    const read = await result("subagent_messages", {
      command: "get_subagent_messages",
      agent_id: "agent-done",
    });
    assert.equal(read.agent_id, "agent-done");
    assert.deepEqual(sdkCalls("getSubagentMessages").at(-1), {
      type: "getSubagentMessages",
      sessionId,
      agentId: "agent-done",
      dir: bridge.cwd,
    });
    const messages = read.messages as Array<{ role: string; text: string }>;
    assert.deepEqual(
      messages.filter((message) => message.role !== "tool"),
      [
        { role: "user", text: "Inspect the parser" },
        { role: "assistant", text: "Parser checked" },
      ],
    );
    assert.ok(
      messages.some((message) => message.role === "tool" && message.text.includes("ls parser")),
      JSON.stringify(messages),
    );
    // An agent whose only answer is its hand-back report: the reader gets the full report as its final reply.
    const handback = await result("subagent_messages", {
      command: "get_subagent_messages",
      agent_id: "agent-handback",
    });
    assert.deepEqual(sdkCalls("getSubagentMessages").at(-1), {
      type: "getSubagentMessages",
      sessionId,
      agentId: "agent-handback",
      dir: bridge.cwd,
    });
    const handbackMessages = handback.messages as Array<{ role: string; text: string }>;
    assert.deepEqual(handbackMessages.at(-1), {
      role: "assistant",
      text: "## Docs survey\n\n- 12 pages\n- 2 stale links",
    });
    // Only the SDK's own reminder loses its wrapper; tags a user or the agent wrote stay.
    assert.deepEqual(
      handbackMessages.filter((message) => message.role !== "tool"),
      [
        {
          role: "user",
          text: "Survey the docs. Quote <system-reminder>tags</system-reminder> as written.",
        },
        { role: "system_reminder", text: "Report through SubagentHandback." },
        { role: "user", text: "<system-reminder>typed by the user</system-reminder>" },
        { role: "assistant", text: "Keeping <system-reminder>as written</system-reminder>" },
        { role: "assistant", text: "## Docs survey\n\n- 12 pages\n- 2 stale links" },
      ],
    );
    assert.ok(
      !handbackMessages.some(
        (message) => message.role === "tool" && message.text.includes("SubagentHandback"),
      ),
      JSON.stringify(handbackMessages),
    );
    const unreadable = await result("subagent_messages", {
      command: "get_subagent_messages",
      agent_id: "agent-unreadable",
    });
    assert.deepEqual(
      [unreadable.agent_id, unreadable.messages, unreadable.error],
      ["agent-unreadable", [], "transcript unreadable"],
    );
    assert.deepEqual(sdkCalls("getSubagentMessages").at(-1), {
      type: "getSubagentMessages",
      sessionId,
      agentId: "agent-unreadable",
      dir: bridge.cwd,
    });

    // Nothing has started yet, so there is nothing to stop.
    const early = await result("subagent_stop_result", {
      command: "stop_subagent",
      task_id: "agent-running",
    });
    assert.match(String(early.error), /not running/);

    const rows = () => {
      const result = new Map<string, BridgeEnvelope>();
      for (const event of bridge.events)
        if (
          event.session_id === sessionId &&
          (event.update as BridgeEnvelope)?.type === "task_state_update"
        )
          for (const task of (event.update as BridgeEnvelope).tasks as BridgeEnvelope[])
            result.set(String(task.task_id), task);
      return result;
    };
    const startPhase = bridge.events.length;
    bridge.writeCommand({
      command: "prompt",
      request_id: "start-tasks",
      session_id: sessionId,
      message_uuid: "subagent-start",
      chunks: [{ kind: "text", value: "fixture task start" }],
    });
    await phaseEvent(
      bridge,
      startPhase,
      (event) => event.event === "turn_complete" && event.session_id === sessionId,
    );
    assert.equal(rows().get("agent-running")?.status, "in_progress");
    assert.equal(rows().get("agent-running")?.run_id, "B");
    assert.equal(rows().get("agent-closing")?.status, "in_progress");
    assert.equal(rows().get("shell-1")?.status, "in_progress");

    const shell = await result("subagent_stop_result", {
      command: "stop_subagent",
      task_id: "shell-1",
    });
    assert.equal(shell.task_id, "shell-1");
    assert.match(String(shell.error), /not running/, "a background shell is not a subagent");

    const stale = await result("subagent_stop_result", {
      command: "stop_subagent",
      task_id: "agent-running",
      expected_run_id: "A",
    });
    assert.match(String(stale.error), /execution has changed/);
    assert.equal(stale.run_id, "A");
    assert.deepEqual(sdkCalls("stopTask"), [], "a stale command must not reach the SDK");

    const stopRequestId = crypto.randomUUID();
    bridge.writeCommand({
      command: "stop_subagent",
      session_id: sessionId,
      task_id: "agent-running",
      expected_run_id: "B",
      request_id: stopRequestId,
    });
    const beforeAcknowledgement: BridgeEnvelope[] = [];
    const acknowledged = await nextMatching(bridge, (event) => {
      beforeAcknowledgement.push(event);
      return (
        event.event === "subagent_stop_result" &&
        event.request_id === stopRequestId &&
        event.session_id === sessionId
      );
    });
    assert.deepEqual(
      [acknowledged.request_id, acknowledged.task_id, acknowledged.error],
      [stopRequestId, "agent-running", undefined],
    );
    assert.equal(acknowledged.run_id, "B");
    assert.ok(
      !beforeAcknowledgement.some(
        (event) => (event.update as BridgeEnvelope)?.type === "task_state_update",
      ),
      "acknowledging a stop request must not report the task as stopped",
    );

    const barrierPhase = bridge.events.length;
    bridge.writeCommand({
      command: "prompt",
      request_id: "after-stop",
      session_id: sessionId,
      message_uuid: "stop-barrier",
      chunks: [{ kind: "text", value: "fixture task barrier" }],
    });
    await phaseEvent(
      bridge,
      barrierPhase,
      (event) => event.event === "turn_complete" && event.session_id === sessionId,
    );
    assert.equal(
      rows().get("agent-running")?.status,
      "in_progress",
      "stop acknowledgement cannot manufacture a terminal lifecycle",
    );
    assert.equal(rows().get("agent-running")?.run_id, "B");
    assert.equal(
      rows().get("agent-closing")?.description,
      "Sibling observed after stop acknowledgement",
    );
    const sibling = structuredClone(rows().get("agent-closing"));
    const finishPhase = bridge.events.length;
    bridge.writeCommand({
      command: "prompt",
      request_id: "actual-outcome",
      session_id: sessionId,
      message_uuid: "stop-outcome",
      chunks: [{ kind: "text", value: "fixture task finish" }],
    });
    await phaseEvent(
      bridge,
      finishPhase,
      (event) => event.event === "turn_complete" && event.session_id === sessionId,
    );
    const terminal = rows().get("agent-running");
    assert.equal(terminal?.status, "completed");
    assert.equal(terminal?.run_id, "B");
    assert.equal((terminal?.metadata as BridgeEnvelope)?.terminal_status, "stopped");
    assert.deepEqual(rows().get("agent-closing"), sibling);
    const terminalRefusal = await result("subagent_stop_result", {
      command: "stop_subagent",
      task_id: "agent-running",
      expected_run_id: "B",
    });
    assert.match(String(terminalRefusal.error), /not running/);
    assert.deepEqual(sdkCalls("stopTask"), [
      { type: "stopTask", sessionId, taskId: "agent-running" },
    ]);

    const rejected = await result("subagent_stop_result", {
      command: "stop_subagent",
      task_id: "agent-closing",
    });
    assert.equal(rejected.error, "Query closed before response received");
    assert.deepEqual(
      sdkCalls("stopTask").map((call) => call.taskId),
      ["agent-running", "agent-closing"],
    );

    const failedList = await result("subagents_listed", { command: "list_subagents" });
    assert.deepEqual(
      [failedList.agent_ids, failedList.error],
      [[], "subagent directory unreadable"],
    );
  } finally {
    await cleanup();
    rmSync(directory, { recursive: true, force: true });
  }
});

async function verifyOwnedCompletion(trigger: "shutdown" | "EOF"): Promise<void> {
  const { bridge, cleanup } = ultracodeFixtureBridge({ OWNED_CLOSE_GATE: "1" });
  const control = controlChannel(bridge);
  try {
    bridge.writeCommand({
      command: "create_session",
      cwd: bridge.cwd,
      request_id: `owned-${trigger}`,
      launch_settings: {},
    });
    const connected = await nextMatching(
      bridge,
      (event) => event.event === "connected" && event.request_id === `owned-${trigger}`,
    );
    await control.until((records) =>
      records.some(
        (record) => record.type === "consumer-waiting" && record.sessionId === connected.session_id,
      ),
    );
    if (trigger === "EOF") bridge.child.stdin.end();
    else bridge.writeCommand({ command: "shutdown", request_id: "owned-shutdown" });
    await control.until((records) =>
      records.some(
        (record) => record.type === "native-close" && record.sessionId === connected.session_id,
      ),
    );
    control.send("probe");
    await control.until((records) =>
      records.some(
        (record) => record.type === "held-after-close" && record.sessionId === connected.session_id,
      ),
    );
    assert.equal(
      bridge.child.exitCode,
      null,
      "bridge processed a separate control receipt while its consumer was still held",
    );
    assert.equal(
      control.records.some((record) => record.type === "consumer-released"),
      false,
    );
    control.send("release");
    await control.until((records) =>
      records.some(
        (record) =>
          record.type === "consumer-released" && record.sessionId === connected.session_id,
      ),
    );
    control.close();
    assert.equal(await bridge.waitForExit(), 0);
    assert.equal(
      bridge.events.some((event) => event.event === "connection_failed"),
      false,
    );
  } finally {
    control.send("release");
    control.close();
    await cleanup();
  }
}

test("stdin EOF closes an active session and waits for its genuinely pending owned consumer", async () => {
  await verifyOwnedCompletion("EOF");
});

test("NDJSON ingress preserves fragmented UTF-8 and coalesced CRLF records exactly once", async () => {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), "framing-receipts-"))),
    preload = join(directory, "receipt.mjs");
  writeFileSync(
    preload,
    `import {writeSync} from 'node:fs';const push=process.stdin.push;process.stdin.push=function(chunk,...args){const result=push.call(this,chunk,...args);if(chunk!==null)writeSync(3,JSON.stringify({type:'bytes',data:Buffer.from(chunk).toString('base64')})+'\\n');return result;};`,
  );
  const bridge = new SpawnedBridge({}, ["--import", pathToFileURL(preload).href], true),
    control = controlChannel(bridge);
  const received = () =>
    Buffer.concat(
      control.records
        .filter((record) => record.type === "bytes")
        .map((record) => Buffer.from(String(record.data), "base64")),
    );
  try {
    const ready = Buffer.from(
      JSON.stringify({ command: "initialize", cwd: bridge.cwd, request_id: "framing-ready" }) +
        "\n",
    );
    bridge.writeRaw(ready);
    await control.until(() => received().length >= ready.length);
    await nextMatching(
      bridge,
      (event) => event.event === "sessions_listed" && event.request_id === "framing-ready",
    );
    const rows = [
      {
        id: "btw-fragment",
        session: "missing-one",
        question: "Literal \u00e9 \ud83d\ude00 and two  spaces\nSecond line",
      },
      {
        id: "btw-coalesced",
        session: "missing-two",
        question: "Other \u2028 and \u2029 preserved",
      },
      { id: "btw-third", session: "missing-three", question: "Last \u20ac question" },
    ];
    const encoded = rows.map((row) =>
      Buffer.from(
        JSON.stringify({
          command: "side_question",
          session_id: row.session,
          btw_id: row.id,
          question: row.question,
        }),
        "utf8",
      ),
    );
    const first = encoded[0],
      splitJson = first.indexOf(Buffer.from('"btw_id"')) + 4,
      splitUtf8 = first.indexOf(Buffer.from("\ud83d\ude00")) + 2;
    assert.ok(splitUtf8 > splitJson);
    bridge.writeRaw(first.subarray(0, splitJson));
    await control.until(() => received().length >= ready.length + splitJson);
    assert.deepEqual(received(), Buffer.concat([ready, first.subarray(0, splitJson)]));
    bridge.writeRaw(first.subarray(splitJson, splitUtf8));
    await control.until(() => received().length >= ready.length + splitUtf8);
    assert.deepEqual(received(), Buffer.concat([ready, first.subarray(0, splitUtf8)]));
    const rest = Buffer.concat([
      first.subarray(splitUtf8),
      Buffer.from("\r\n"),
      encoded[1],
      Buffer.from("\n"),
      encoded[2],
      Buffer.from("\r\n"),
    ]);
    bridge.writeRaw(rest);
    for (const row of rows) {
      const answer = await nextMatching(
        bridge,
        (event) => event.event === "btw_failed" && event.btw_id === row.id,
      );
      assert.equal(answer.session_id, row.session);
      assert.equal(answer.question, row.question);
    }
    bridge.writeCommand({ command: "shutdown" });
    if (bridge.child.exitCode === null)
      await new Promise<void>((resolve, reject) => {
        const timer = setTimeout(() => reject(new Error("framing shutdown did not exit")), 5_000);
        bridge.child.once("exit", () => {
          clearTimeout(timer);
          resolve();
        });
      });
    control.close();
    assert.equal(await bridge.waitForExit(), 0);
    for (const row of rows)
      assert.equal(
        bridge.events.filter((event) => event.event === "btw_failed" && event.btw_id === row.id)
          .length,
        1,
        row.id,
      );
    assert.equal(
      bridge.events.some((event) => event.event === "connection_failed"),
      false,
      "complete frames cannot fabricate decode failures",
    );
  } finally {
    control.close();
    await bridge.stop();
    rmSync(directory, { recursive: true, force: true });
  }
});
