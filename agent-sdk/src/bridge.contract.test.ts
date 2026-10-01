import test from "node:test";
import assert from "node:assert/strict";
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import readline from "node:readline";
import { fileURLToPath, pathToFileURL } from "node:url";

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
    export async function listSessions() { return [...saved.keys()].map(sessionId => ({ sessionId, cwd: process.cwd(), lastModified: 1 })); }
    export async function getSessionMessages() { return []; }
    const saved = new Map();
    export function query({ prompt, options }) {
      const state = saved.get(options.resume) ?? { requested: false, effort: "high" };
      saved.set(options.sessionId ?? options.resume, state);
      let model = options.model ?? "opus";
      let done = false;
      let waiter;
      const queue = [];
      function push(value) { if (waiter) { const resolve = waiter; waiter = undefined; resolve({ value, done: false }); } else queue.push(value); }
      void (async () => {
        for await (const message of prompt) {
          if (JSON.stringify(message.message?.content).includes("fixture conversation reset")) {
            push({ type: "conversation_reset", new_conversation_id: "fixture-conversation-2" });
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
        async initializationResult() { return { models: [], commands: [], agents: [], account: { apiKeySource: "fixture" }, fast_mode_state: "off" }; },
        async setModel(value) { model = value; },
        async getSettings() {
          if (model === "fixture-read-failure") throw new Error("fixture read failed");
          const available = model !== "haiku" && model !== "fixture-unavailable" && process.env.ULTRACODE_FIXTURE_WORKFLOWS !== "off";
          return { applied: { ultracodeAvailable: available, ultracodeRequested: state.requested, ultracode: state.requested && available, effort: state.effort } };
        },
        async applyFlagSettings(settings) {
          if (settings.ultracode === true && process.env.ULTRACODE_FIXTURE_WORKFLOWS === "off") throw new Error("apply_flag_settings: ultracode is not available for this session (dynamic workflows are off)");
          if (settings.ultracode === true && model === "haiku") throw new Error("apply_flag_settings: ultracode is not available for this session (haiku does not support it)");
          if (settings.effortLevel !== undefined) {
            state.effort = settings.effortLevel;
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

test("spawned bridge Ultracode workflow covers enable, effort, model, reset, resume and replacement", async () => {
  const { bridge, cleanup } = ultracodeFixtureBridge();
  try {
    bridge.writeCommand({ command: "create_session", cwd: process.cwd(), launch_settings: { settings: { model: "opus" } } });
    const connected = await nextMatching(bridge, event => event.event === "connected");
    const sessionId = connected.session_id;
    assert.deepEqual(connected.ultracode, { available: true, requested: false, effective: false });
    const active = { available: true, requested: true, effective: true };
    bridge.writeCommand({ command: "set_ultracode", session_id: sessionId, enabled: true });
    // Back-to-back commands exercise the production per-session scheduler.
    bridge.writeCommand({ command: "set_effort", session_id: sessionId, effort: "low" });
    assert.deepEqual(await nextUltracode(bridge), active);
    assert.deepEqual(await nextUltracode(bridge), active);
    const effort = await nextMatching(bridge, event => (event.update as BridgeEnvelope)?.type === "config_option_update");
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
    bridge.writeCommand({ command: "resume_session", session_id: sessionId, launch_settings: { settings: { model: "opus" } } });
    const resumed = await nextMatching(bridge, event => event.event === "session_replaced");
    assert.equal(resumed.session_id, sessionId);
    assert.deepEqual(resumed.ultracode, active);
    bridge.writeCommand({ command: "set_ultracode", session_id: sessionId, enabled: false });
    assert.deepEqual(await nextUltracode(bridge), { available: true, requested: false, effective: false });
    bridge.writeCommand({ command: "new_session", cwd: process.cwd(), launch_settings: { settings: { model: "opus" } } });
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
    bridge.writeCommand({ command: "create_session", cwd: process.cwd(), launch_settings: { settings: { model: "fixture-unavailable" } } });
    const connected = await nextMatching(bridge, event => event.event === "connected");
    const sessionId = connected.session_id;
    assert.deepEqual(connected.ultracode, { available: false, requested: false, effective: false });
    const requested = { available: false, requested: true, effective: false };
    for (const requestId of ["inactive-enable", "inactive-retry"]) {
      bridge.writeCommand({ command: "set_ultracode", session_id: sessionId, enabled: true, request_id: requestId });
      assert.deepEqual(await nextUltracode(bridge), requested);
      const error = await bridge.nextEnvelope(5_000);
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
      bridge.writeCommand({ command: "create_session", cwd: process.cwd(), launch_settings: { settings: { model: "opus" } } });
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
