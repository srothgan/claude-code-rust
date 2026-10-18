// Native SDK plugin contract fixture. All storage and model/MCP endpoints are local to this process.
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import http from "node:http";
import { randomUUID } from "node:crypto";
const root = fs.mkdtempSync(path.join(os.tmpdir(), "claude-rs-plugin-probe-"));
const cwd = path.join(root, "project"),
  profile = path.join(root, "profile");
fs.mkdirSync(cwd);
fs.mkdirSync(profile);
process.env.CLAUDE_CONFIG_DIR = profile;
for (const key of Object.keys(process.env))
  if (/^(ANTHROPIC_|CLAUDE_CODE_USE_|CLAUDE_CODE_OAUTH_TOKEN|CLAUDECODE$)/.test(key))
    delete process.env[key];
process.env.CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC = "1";
process.env.ANTHROPIC_AUTH_TOKEN = "local-fixture-token";
const server = http.createServer(async (req, res) => {
  let raw = "";
  for await (const c of req) raw += c;
  const data = JSON.parse(raw || "{}");
  if (req.url.includes("count_tokens")) {
    res.end(JSON.stringify({ input_tokens: 10 }));
    return;
  }
  const msg = {
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
  send("message_start", { message: msg });
  send("content_block_start", {
    index: 0,
    content_block: { type: "text", text: "" },
  });
  send("content_block_delta", {
    index: 0,
    delta: { type: "text_delta", text: "LOCAL-PROBE-OK" },
  });
  send("content_block_stop", { index: 0 });
  send("message_delta", {
    delta: { stop_reason: "end_turn" },
    usage: { output_tokens: 5 },
  });
  send("message_stop", {});
  res.end();
});
await new Promise((r) => server.listen(0, "127.0.0.1", r));
process.env.ANTHROPIC_BASE_URL = "http://127.0.0.1:" + server.address().port;
const { query } = await import(
  "../../agent-sdk/node_modules/@anthropic-ai/claude-agent-sdk/sdk.mjs"
);
const options = {
  abortController: new AbortController(),
  cwd,
  settingSources: [],
  mcpServers: {},
  strictMcpConfig: false,
  tools: [],
  model: "claude-sonnet-4-6",
};
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
const a = plugin("fixture-a"),
  b = plugin("fixture-b", true);
const results = [];
const summarize = (r) => ({
  held: r.held,
  error_count: r.error_count,
  plugins: r.plugins,
  commands: r.commands.filter((c) => c.name.includes("fixture")),
  cache_impact: r.cache_impact,
});
const sessionId = randomUUID();
let inputResolve;
const inputReady = new Promise((r) => (inputResolve = r));
let q;
const deadline = setTimeout(() => options.abortController.abort(), 45_000);
try {
  q = query({
    prompt: (async function* () {
      await inputReady;
      yield {
        type: "user",
        session_id: sessionId,
        parent_tool_use_id: null,
        message: { role: "user", content: "Only reply LOCAL-PROBE-OK" },
      };
      await new Promise(() => {});
    })(),
    options: { ...options, sessionId },
  });
  const initialization = await q.initializationResult();
  results.push({
    name: "initialization",
    keys: Object.keys(initialization),
    plugins: initialization.plugins,
  });
  for (const [name, args] of [
    ["attach", { addPlugins: [a], holdOnCacheImpact: true }],
    ["duplicate", { addPlugins: [a], holdOnCacheImpact: true }],
    ["partial", { addPlugins: [b], holdOnCacheImpact: true }],
    [
      "missing",
      {
        addPlugins: [{ type: "local", path: path.join(root, "absent") }],
        holdOnCacheImpact: true,
      },
    ],
  ]) {
    try {
      results.push({ name, ...summarize(await q.reloadPlugins(args)) });
    } catch (e) {
      results.push({ name, error: e.message });
    }
  }
  plugin("fixture-b");
  results.push({
    name: "retry-repaired",
    ...summarize(await q.reloadPlugins({ addPlugins: [b], holdOnCacheImpact: true })),
  });
  inputResolve();
  for await (const m of q) {
    if (m.type === "system" && m.subtype === "init")
      results.push({ name: "init-message", plugins: m.plugins });
    if (m.type === "result") {
      const c = plugin("fixture-mcp");
      const mcp = path.join(root, "mcp.mjs");
      fs.writeFileSync(
        mcp,
        'import readline from "node:readline"; const rl=readline.createInterface({input:process.stdin});rl.on("line",line=>{const r=JSON.parse(line);if(r.id===undefined)return;let result={};if(r.method==="initialize")result={protocolVersion:"2025-03-26",capabilities:{tools:{}},serverInfo:{name:"fixture",version:"1"}};if(r.method==="tools/list")result={tools:[{name:"inspect_fixture",description:"Inert fixture",inputSchema:{type:"object",properties:{}}}]};process.stdout.write(JSON.stringify({jsonrpc:"2.0",id:r.id,result})+"\\n");});',
      );
      fs.writeFileSync(
        path.join(c.path, ".mcp.json"),
        JSON.stringify({
          mcpServers: { fixture: { command: process.execPath, args: [mcp] } },
        }),
      );
      const held = await q.reloadPlugins({
        addPlugins: [c],
        holdOnCacheImpact: true,
      });
      results.push({ name: "held-mcp", ...summarize(held) });
      if (held.held)
        results.push({
          name: "apply-mcp",
          ...summarize(await q.reloadPlugins({ addPlugins: [c] })),
        });
      break;
    }
  }
  q.close();
  q = null;
  for (const [name, extra] of [
    ["resume", { resume: sessionId }],
    ["resume-explicit", { resume: sessionId, plugins: [a, b] }],
    ["new", {}],
  ]) {
    let read;
    try {
      read = query({
        prompt: (async function* () {
          await new Promise(() => {});
        })(),
        options: { ...options, ...extra },
      });
      await read.initializationResult();
      results.push({
        name,
        ...summarize(await read.reloadPlugins({ holdOnCacheImpact: true })),
      });
    } catch (e) {
      results.push({ name, error: e.message });
    } finally {
      read?.close();
    }
  }
  console.log(
    JSON.stringify(
      {
        sdk: JSON.parse(
          fs.readFileSync(
            new URL(
              "../../agent-sdk/node_modules/@anthropic-ai/claude-agent-sdk/package.json",
              import.meta.url,
            ),
          ),
        ).version,
        results,
      },
      null,
      2,
    ),
  );
} finally {
  clearTimeout(deadline);
  q?.close();
  server.closeAllConnections();
  await new Promise((r) => server.close(r));
  await fs.promises.rm(root, {
    recursive: true,
    force: true,
    maxRetries: 10,
    retryDelay: 100,
  });
}
