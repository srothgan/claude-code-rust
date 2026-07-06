#!/usr/bin/env node
import assert from "node:assert/strict";
import { spawn } from "node:child_process";

const options = parseArgs(process.argv.slice(2));
if (options.help) {
  printHelp();
  process.exit(0);
}

if (!options.runtime || !options.bridge) {
  printHelp();
  process.exit(1);
}

try {
  await runContract(options);
  console.log(`Bridge runtime contract passed for ${options.runtime} ${options.bridge}`);
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exit(1);
}

async function runContract({ runtime, bridge }) {
  const child = spawn(runtime, [bridge], {
    env: {
      ...process.env,
      CLAUDE_RS_BRIDGE_DIAGNOSTICS: "1",
    },
    stdio: ["pipe", "pipe", "pipe"],
    windowsHide: true,
  });

  const stdoutEvents = [];
  const stderrLines = [];
  const pending = [];
  let stdoutRemainder = "";
  let stderrRemainder = "";

  child.stdout.setEncoding("utf8");
  child.stdout.on("data", (chunk) => {
    stdoutRemainder += chunk;
    const lines = stdoutRemainder.split(/\r?\n/);
    stdoutRemainder = lines.pop() ?? "";
    for (const line of lines) {
      if (line.trim()) {
        const event = JSON.parse(line);
        stdoutEvents.push(event);
        resolvePending(stdoutEvents, pending);
      }
    }
  });

  child.stderr.setEncoding("utf8");
  child.stderr.on("data", (chunk) => {
    stderrRemainder += chunk;
    const lines = stderrRemainder.split(/\r?\n/);
    stderrRemainder = lines.pop() ?? "";
    for (const line of lines) {
      if (line.trim()) {
        stderrLines.push(line);
      }
    }
  });

  child.stdin.write("{not json\n");
  const malformed = await readEvent(
    stdoutEvents,
    pending,
    (event) => event.event === "connection_failed",
    "connection_failed after malformed JSON",
  );
  assert.match(malformed.message, /invalid command envelope/);

  child.stdin.write(`${JSON.stringify({ request_id: "init-1", command: "initialize", cwd: process.cwd() })}\n`);
  const initialized = await readEvent(
    stdoutEvents,
    pending,
    (event) => event.event === "initialized" && event.request_id === "init-1",
    "initialized event",
  );
  assert.deepEqual(initialized.result.capabilities, {
    prompt_image: true,
    prompt_embedded_context: true,
    supports_session_listing: true,
    supports_resume_session: true,
  });
  assert.equal(initialized.result.agent_name, "claude-rs-agent-bridge");

  await readEvent(
    stdoutEvents,
    pending,
    (event) => event.event === "sessions_listed" && event.request_id === "init-1",
    "sessions_listed event",
  );

  child.stdin.write(`${JSON.stringify({ request_id: "shutdown-1", command: "shutdown" })}\n`);
  child.stdin.end();
  const exit = await waitForExit(child);
  assert.equal(exit.code, 0, `bridge exited with code ${exit.code} signal ${exit.signal ?? ""}`);

  if (stderrRemainder.trim()) {
    stderrLines.push(stderrRemainder.trim());
  }
  assert(stderrLines.length > 0, "bridge diagnostics should emit JSON lines to stderr");
  for (const line of stderrLines) {
    const diagnostic = JSON.parse(line);
    assert.equal(diagnostic.schema, "claude-rs-log/v1");
    assert.equal(typeof diagnostic.timestamp, "string");
    assert.equal(typeof diagnostic.level, "string");
    assert.equal(typeof diagnostic.target, "string");
    assert.equal(typeof diagnostic.event_name, "string");
  }
}

function readEvent(events, pending, predicate, label) {
  const found = events.find(predicate);
  if (found) {
    return Promise.resolve(found);
  }

  return new Promise((resolve, reject) => {
    const timeout = setTimeout(() => {
      reject(new Error(`Timed out waiting for ${label}`));
    }, 10_000);
    pending.push({ predicate, resolve, timeout });
  });
}

function resolvePending(events, pending) {
  for (let index = 0; index < pending.length; index += 1) {
    const waiter = pending[index];
    const found = events.find(waiter.predicate);
    if (!found) {
      continue;
    }
    clearTimeout(waiter.timeout);
    pending.splice(index, 1);
    waiter.resolve(found);
    index -= 1;
  }
}

function waitForExit(child) {
  return new Promise((resolve, reject) => {
    child.on("error", reject);
    child.on("exit", (code, signal) => resolve({ code, signal }));
  });
}

function parseArgs(args) {
  const parsed = {
    runtime: undefined,
    bridge: undefined,
    help: false,
  };

  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    switch (arg) {
      case "--runtime":
        parsed.runtime = readArgValue(args, ++index, arg);
        break;
      case "--bridge":
        parsed.bridge = readArgValue(args, ++index, arg);
        break;
      case "--help":
      case "-h":
        parsed.help = true;
        break;
      default:
        throw new Error(`Unknown argument: ${arg}`);
    }
  }

  return parsed;
}

function readArgValue(args, index, flag) {
  const value = args[index];
  if (!value || value.startsWith("--")) {
    throw new Error(`Missing value for ${flag}`);
  }
  return value;
}

function printHelp() {
  console.log(`Usage: node agent-sdk/scripts/bridge-runtime-contract.mjs --runtime <path> --bridge <path>

Options:
  --runtime <path>    Runtime executable, such as bundled Bun.
  --bridge <path>     Built agent-sdk/dist/bridge.js path.
  -h, --help          Show this help.
`);
}
