// Native SDK contract probe; every profile, endpoint and child belongs to this fixture.
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import http from "node:http";
import { spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import { AsyncQueue } from "../../agent-sdk/dist/bridge/shared.js";

const root = fs.mkdtempSync(path.join(os.tmpdir(), "claude-rs-plugin-probe-"));
const cwd = path.join(root, "project"),
  profile = path.join(root, "profile");
const journal = path.join(root, "mcp.jsonl"),
  stop = path.join(root, "mcp-stop");
const controller = new AbortController();
const queries = new Set(),
  children = [],
  inputs = [];
const results = [],
  diagnostics = [],
  requests = [];
let server,
  timer,
  timedOut = false,
  sdk = "unknown",
  phase = "setup";
const firstPrompt = "Original plugin turn: only reply LOCAL-PROBE-OK";
const resumePrompt = "Explicit resume turn: only reply LOCAL-PROBE-OK";
const sessionId = randomUUID();

function readJournal() {
  if (!fs.existsSync(journal)) return [];
  const raw = fs.readFileSync(journal, "utf8");
  return raw
    .slice(0, raw.lastIndexOf("\n") + 1)
    .split("\n")
    .filter(Boolean)
    .map((line) => JSON.parse(line));
}
function waitForJournal(
  predicate,
  milliseconds,
  failure,
  trigger = () => {},
  signal = controller.signal,
) {
  return new Promise((resolve, reject) => {
    let settled = false;
    const check = () => {
      try {
        if (predicate(readJournal())) finish();
      } catch (error) {
        finish(error);
      }
    };
    const watcher = fs.watch(root, check);
    const deadline = setTimeout(() => finish(new Error(failure)), milliseconds);
    const aborted = () => finish(new Error("Native plugin probe aborted"));
    function finish(error) {
      if (settled) return;
      settled = true;
      watcher.close();
      clearTimeout(deadline);
      signal?.removeEventListener("abort", aborted);
      if (error) reject(error);
      else resolve();
    }
    watcher.on("error", finish);
    signal?.addEventListener("abort", aborted, { once: true });
    try {
      if (signal?.aborted) {
        aborted();
        return;
      }
      trigger();
      check();
    } catch (error) {
      finish(error);
    }
  });
}
async function stopPeers() {
  await waitForJournal(
    (entries) =>
      entries
        .filter((entry) => entry.kind === "start")
        .every((start) =>
          entries.some((entry) => entry.kind === "exit" && entry.pid === start.pid),
        ),
    3_000,
    "Owned MCP peer did not exit",
    () => fs.writeFileSync(stop, "stop"),
    null,
  );
}
async function closeQuery(q) {
  if (!q || !queries.delete(q)) return;
  try {
    q.close();
  } finally {
    await drainChildren();
  }
}
async function drainChildren() {
  await Promise.all(
    children.map(async ({ child, closed }) => {
      const kill = setTimeout(() => child.kill(), 2_000);
      try {
        await closed;
      } finally {
        clearTimeout(kill);
      }
    }),
  );
}
function plugin(name, bad = false) {
  const folder = path.join(root, name);
  fs.mkdirSync(path.join(folder, ".claude-plugin"), { recursive: true });
  fs.mkdirSync(path.join(folder, "commands"), { recursive: true });
  fs.writeFileSync(
    path.join(folder, ".claude-plugin/plugin.json"),
    bad ? "{invalid" : JSON.stringify({ name, version: "1.2.3" }),
  );
  fs.writeFileSync(
    path.join(folder, "commands/hello.md"),
    "---\ndescription: Inert local fixture\n---\nSay LOCAL-PROBE-OK.",
  );
  return { type: "local", path: folder };
}
try {
  fs.mkdirSync(cwd);
  fs.mkdirSync(profile);
  for (const key of Object.keys(process.env))
    if (/^(ANTHROPIC_|CLAUDE_|CLAUDECODE$)/.test(key)) delete process.env[key];
  Object.assign(process.env, {
    CLAUDE_CONFIG_DIR: profile,
    CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: "1",
    ANTHROPIC_AUTH_TOKEN: "local-fixture-token",
  });
  server = http.createServer(async (req, res) => {
    try {
      let raw = "";
      for await (const chunk of req) raw += chunk;
      const route = new URL(req.url, "http://127.0.0.1").pathname;
      if (route === "/v1/messages/count_tokens") {
        res.end(JSON.stringify({ input_tokens: 10 }));
        return;
      }
      if (route !== "/v1/messages") {
        res.writeHead(404);
        res.end();
        return;
      }
      const data = JSON.parse(raw);
      requests.push({ phase, method: req.method, route, messages: data.messages });
      const message = {
        id: "msg_local",
        type: "message",
        role: "assistant",
        model: data.model,
        content: [],
        stop_reason: null,
        stop_sequence: null,
        usage: { input_tokens: 10, output_tokens: 5 },
      };
      res.writeHead(200, { "Content-Type": "text/event-stream" });
      const send = (type, data) =>
        res.write("event: " + type + "\ndata: " + JSON.stringify({ type, ...data }) + "\n\n");
      send("message_start", { message });
      send("content_block_start", { index: 0, content_block: { type: "text", text: "" } });
      send("content_block_delta", {
        index: 0,
        delta: { type: "text_delta", text: "LOCAL-PROBE-OK" },
      });
      send("content_block_stop", { index: 0 });
      send("message_delta", { delta: { stop_reason: "end_turn" }, usage: { output_tokens: 5 } });
      send("message_stop", {});
      res.end();
    } catch (error) {
      res.writeHead(500);
      res.end(String(error));
    }
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      server.off("error", reject);
      resolve();
    });
  });
  process.env.ANTHROPIC_BASE_URL = `http://127.0.0.1:${server.address().port}`;
  const { query } = await import("@anthropic-ai/claude-agent-sdk");
  sdk = JSON.parse(
    fs.readFileSync(
      new URL("../../node_modules/@anthropic-ai/claude-agent-sdk/package.json", import.meta.url),
      "utf8",
    ),
  ).version;
  const options = {
    abortController: controller,
    cwd,
    settingSources: [],
    mcpServers: {},
    strictMcpConfig: false,
    tools: [],
    model: "claude-sonnet-4-6",
    stderr: (text) => diagnostics.push({ phase, text }),
    spawnClaudeCodeProcess(options) {
      const child = spawn(options.command, options.args, {
        cwd: options.cwd,
        env: options.env,
        signal: options.signal,
        stdio: ["pipe", "pipe", "pipe"],
        windowsHide: true,
      });
      const closed = new Promise((resolve) => child.once("close", resolve));
      children.push({ child, closed });
      return child;
    },
  };
  function open(extra = {}) {
    const input = new AsyncQueue();
    inputs.push(input);
    const q = query({ prompt: input, options: { ...options, ...extra } });
    queries.add(q);
    return { q, input };
  }
  async function reload(q, name, args) {
    phase = name;
    const value = await q.reloadPlugins(args);
    assert.ok(
      Array.isArray(value.plugins) &&
        Array.isArray(value.commands) &&
        Array.isArray(value.mcpServers),
      `${name}: actual returned inventory`,
    );
    results.push({
      name,
      ...value,
      commands: value.commands.filter((command) => command.name.startsWith("fixture-")),
    });
    return value;
  }
  async function turn(q, input, text, name) {
    phase = name;
    input.enqueue({
      type: "user",
      session_id: sessionId,
      parent_tool_use_id: null,
      message: { role: "user", content: text },
    });
    const observed = [];
    // Keep the iterator open for subsequent reloads; for-await return closes the Query.
    const iterator = q[Symbol.asyncIterator]();
    while (true) {
      const { value: message, done } = await iterator.next();
      if (done) break;
      observed.push({
        type: message.type,
        subtype: message.subtype,
        session_id: message.session_id,
        content: message.message?.content,
        result: message.result,
        is_error: message.is_error,
      });
      if (message.type === "result") {
        assert.equal(message.subtype, "success", JSON.stringify(message));
        assert.equal(message.is_error, false, JSON.stringify(message));
        assert.equal(message.result, "LOCAL-PROBE-OK");
        results.push({ name, messages: observed });
        return;
      }
    }
    assert.fail(`${name}: stream ended before terminal result`);
  }
  const run = async () => {
    const a = plugin("fixture-a"),
      b = plugin("fixture-b", true);
    const first = open({ sessionId });
    await first.q.initializationResult();
    await reload(first.q, "initialization", { holdOnCacheImpact: true });
    await reload(first.q, "attach", { addPlugins: [a], holdOnCacheImpact: true });
    await reload(first.q, "partial", { addPlugins: [b], holdOnCacheImpact: true });
    plugin("fixture-b");
    await reload(first.q, "retry-repaired", { addPlugins: [b], holdOnCacheImpact: true });
    await reload(first.q, "duplicate", { addPlugins: [a], holdOnCacheImpact: true });
    await turn(first.q, first.input, firstPrompt, "completed-turn");
    const c = plugin("fixture-mcp"),
      mcp = path.join(root, "mcp.mjs");
    fs.writeFileSync(
      mcp,
      `
import fs from "node:fs";
import readline from "node:readline";
const journal = ${JSON.stringify(journal)}, stop = ${JSON.stringify(stop)};
const record = entry => fs.appendFileSync(journal, JSON.stringify({pid:process.pid,...entry})+"\\n");
record({kind:"start"});
process.on("exit", () => record({kind:"exit"}));
const watcher = fs.watch(${JSON.stringify(root)}, () => { if(fs.existsSync(stop)) process.exit(0); });
if(fs.existsSync(stop)) process.exit(0);
const input = readline.createInterface({input:process.stdin});
input.on("close", () => { watcher.close(); process.exit(0); });
input.on("line", line => {
  const request = JSON.parse(line); record({kind:"request",message:request});
  if(request.id === undefined) return;
  let result;
  if(request.method === "initialize") result = {protocolVersion:"2025-03-26",capabilities:{tools:{}},serverInfo:{name:"fixture-mcp-peer",version:"1.0.0"}};
  else if(request.method === "tools/list") result = {tools:[{name:"inspect_fixture",description:"Inert fixture",inputSchema:{type:"object",properties:{}}}]};
  const response = result ? {jsonrpc:"2.0",id:request.id,result} : {jsonrpc:"2.0",id:request.id,error:{code:-32601,message:"Unsupported fixture method"}};
  const wire = JSON.stringify(response)+"\\n"; record({kind:"response",wire}); process.stdout.write(wire);
});
`,
    );
    fs.writeFileSync(
      path.join(c.path, ".mcp.json"),
      JSON.stringify({ mcpServers: { fixture: { command: process.execPath, args: [mcp] } } }),
    );
    await reload(first.q, "held-mcp", { addPlugins: [c], holdOnCacheImpact: true });
    await reload(first.q, "apply-mcp", { addPlugins: [c] });
    // Wait for the peer's actual tool-list response, then the native connection outcome.
    await waitForJournal(
      (entries) =>
        entries.some((entry) => entry.kind === "response" && JSON.parse(entry.wire).result?.tools),
      5_000,
      "MCP peer did not complete tools/list",
    );
    let servers;
    const readyDeadline = Date.now() + 2_000;
    do {
      servers = await first.q.mcpServerStatus();
    } while (
      servers.some(
        (server) => server.name === "plugin:fixture-mcp:fixture" && server.status === "pending",
      ) &&
      Date.now() < readyDeadline
    );
    results.push({ name: "applied-status", servers });
    await closeQuery(first.q);
    for (const [name, extra] of [
      ["resume", { resume: sessionId }],
      ["resume-explicit", { resume: sessionId, plugins: [a, b] }],
      ["new", {}],
    ]) {
      const read = open(extra);
      try {
        await read.q.initializationResult();
        await reload(read.q, name, { holdOnCacheImpact: true });
        if (name === "resume-explicit")
          await turn(read.q, read.input, resumePrompt, "resumed-turn");
      } finally {
        await closeQuery(read.q);
      }
    }
  };
  const deadline = new Promise((_, reject) => {
    timer = setTimeout(() => {
      timedOut = true;
      controller.abort();
      reject(new Error(`Native plugin watchdog expired during ${phase}`));
    }, 45_000);
  });
  await Promise.race([run(), deadline]);
  assert.equal(timedOut, false);
} catch (error) {
  console.error(
    JSON.stringify({ sdk, root, phase, timedOut, error: String(error), diagnostics, results }),
  );
  process.exitCode = 1;
} finally {
  clearTimeout(timer);
  inputs.forEach((input) => input.close());
  try {
    for (const q of queries) await closeQuery(q);
    await drainChildren();
    await stopPeers();
  } finally {
    try {
      if (server) {
        server.closeAllConnections();
        if (server.listening) await new Promise((resolve) => server.close(resolve));
      }
    } finally {
      let peer;
      try {
        peer = readJournal();
      } finally {
        await fs.promises.rm(root, {
          recursive: true,
          force: true,
          maxRetries: 10,
          retryDelay: 100,
        });
      }
      if (!process.exitCode && !timedOut && results.some((entry) => entry.name === "new"))
        console.log(JSON.stringify({ sdk, root, sessionId, results, requests, diagnostics, peer }));
    }
  }
}
