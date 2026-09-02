import test from "node:test";
import assert from "node:assert/strict";
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { readdirSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import readline from "node:readline";
import { fileURLToPath } from "node:url";

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

  constructor(env: NodeJS.ProcessEnv = {}) {
    this.child = spawn(process.execPath, [bridgePath], {
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
