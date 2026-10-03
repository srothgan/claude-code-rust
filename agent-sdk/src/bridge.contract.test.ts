import test from "node:test";
import assert from "node:assert/strict";
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { mkdtempSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import readline from "node:readline";
import { fileURLToPath, pathToFileURL } from "node:url";
import type { SettingsMutation, SettingsResult, SettingsSnapshot } from "./types.js";

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
    export async function getSessionMessages() { return []; }
    const saved = new Map();
    export function query({ prompt, options }) {
      record({ type: "query", cwd: options.cwd, resume: options.resume, model: options.model, effort: options.effort, permissionMode: options.permissionMode, agent: options.agent });
      const state = saved.get(options.resume) ?? { requested: false, effort: "high", cwd: options.cwd };
      saved.set(options.sessionId ?? options.resume, state);
      let model = options.model ?? "opus";
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
          if (process.env.SETTINGS_JOURNAL === "1") {
            const resolved = await sdkResolveSettings({ cwd: options.cwd, settingSources: options.settingSources, settings: options.settings });
            record({ type: "settings", effective: resolved.effective, checkpointing: options.enableFileCheckpointing });
          }
          return { models: [], commands: commandFixture ? bootstrapCommands : [], agents: [], account: { apiKeySource: "fixture" }, fast_mode_state: "off" }; },
        async supportedCommands() { return bootstrapCommands; },
        async setModel(value) { model = value; },
        async getSettings() {
          if (model === "fixture-read-failure") throw new Error("fixture read failed");
          const available = model !== "haiku" && model !== "fixture-unavailable" && process.env.ULTRACODE_FIXTURE_WORKFLOWS !== "off";
          return { applied: { ultracodeAvailable: available, ultracodeRequested: state.requested, ultracode: state.requested && available, effort: model === "haiku" ? null : state.effort } };
        },
        async applyFlagSettings(settings) {
          if (settings.ultracode === true && process.env.ULTRACODE_FIXTURE_WORKFLOWS === "off") throw new Error("apply_flag_settings: ultracode is not available for this session (dynamic workflows are off)");
          if (settings.ultracode === true && model === "haiku") throw new Error("apply_flag_settings: ultracode is not available for this session (haiku does not support it)");
          if (settings.effortLevel !== undefined) {
            state.effort = settings.effortLevel === "max" && process.env.EFFORT_FIXTURE_CAP === "high" ? "high" : settings.effortLevel;
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
  const { bridge, cleanup } = ultracodeFixtureBridge({ CLAUDE_CONFIG_DIR: profile, SETTINGS_POLICY_FIXTURE: JSON.stringify({ language: "Policy language" }) });
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
