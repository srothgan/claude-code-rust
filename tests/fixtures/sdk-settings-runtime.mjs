// Runs only in a fresh child with an isolated profile and a loopback model endpoint.
import fs from "node:fs";
import path from "node:path";
import http from "node:http";
const { query } = await import("../../agent-sdk/node_modules/@anthropic-ai/claude-agent-sdk/sdk.mjs");
const [type, policy] = process.argv.slice(2),
  cwd = process.cwd();
const citation = { type: "web_search_result_location", url: "https://example.test/citation-only", title: "Metadata source", encrypted_index: "fixture-only", cited_text: "Evidence" };
const reply = type === "citations" ? "Evidence: [source](https://example.test/source)." : "HOOK-PROBE-OK";
if (type === "idle") {
  fs.writeFileSync(
    path.join(cwd, ".claude/settings.json"),
    JSON.stringify(policy === "default" ? {} : { idleCompaction: policy === "true" }),
  );
  // Initialization controls do not require submitting a prompt to a model.
  const q = query({
    prompt: (async function* () {
      await new Promise(() => {});
    })(),
    options: {
      cwd,
      settingSources: ["project"],
      mcpServers: {},
      strictMcpConfig: true,
      tools: [],
      persistSession: false,
    },
  });
  const timeout = setTimeout(() => q.close(), 15_000);
  try {
    await q.initializationResult();
    const response = await q.getSettings();
    console.log(
      JSON.stringify({ effective: response.effective.idleCompaction, applied: response.applied.idleCompaction }),
    );
  } finally {
    clearTimeout(timeout);
    q.close();
  }
} else {
  let calls = 0,
    hooks = 0;
  const requests = [],
    messages = [];
  const server = http.createServer(async (req, res) => {
    let raw = "";
    for await (const c of req) raw += c;
    requests.push(req.url);
    if (req.url === "/hook") {
      hooks++;
      res.writeHead(500);
      res.end("controlled harmless failure");
      return;
    }
    if (req.url.includes("count_tokens")) {
      res.setHeader("Content-Type", "application/json");
      res.end(JSON.stringify({ input_tokens: 10 }));
      return;
    }
    calls++;
    const data = JSON.parse(raw || "{}");
    const message = {
      id: "msg_fixture",
      type: "message",
      role: "assistant",
      model: data.model,
      content: [{ type: "text", text: reply, ...(type === "citations" ? { citations: [citation] } : {}) }],
      stop_reason: "end_turn",
      stop_sequence: null,
      usage: { input_tokens: 10, output_tokens: 5 },
    };
    if (data.stream) {
      res.writeHead(200, { "Content-Type": "text/event-stream" });
      const send = (event, data) => res.write("event: " + event + "\ndata: " + JSON.stringify(data) + "\n\n");
      send("message_start", { type: "message_start", message: { ...message, content: [], stop_reason: null } });
      send("content_block_start", { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } });
      send("content_block_delta", {
        type: "content_block_delta",
        index: 0,
        delta: { type: "text_delta", text: reply },
      });
      if (type === "citations") send("content_block_delta", { type: "content_block_delta", index: 0, delta: { type: "citations_delta", citation } });
      send("content_block_stop", { type: "content_block_stop", index: 0 });
      send("message_delta", {
        type: "message_delta",
        delta: { stop_reason: "end_turn", stop_sequence: null },
        usage: { output_tokens: 5 },
      });
      send("message_stop", { type: "message_stop" });
      res.end();
    } else {
      res.setHeader("Content-Type", "application/json");
      res.end(JSON.stringify(message));
    }
  });
  await new Promise((r) => server.listen(0, "127.0.0.1", r));
  const base = "http://127.0.0.1:" + server.address().port;
  process.env.ANTHROPIC_BASE_URL = base;
  const marker = path.join(cwd, "hook-ran"),
    failure = path.join(cwd, "fail.mjs");
  fs.writeFileSync(
    failure,
    'import fs from "node:fs"; fs.writeFileSync(' + JSON.stringify(marker) + ',"ran"); process.exit(7);',
  );
  const handler =
    type === "command"
      ? { type, command: '"' + process.execPath + '" "' + failure + '"' }
      : { type, url: base + "/hook" };
  if (policy !== "default") handler.onFailure = policy;
  if (type !== "citations") fs.writeFileSync(
    path.join(cwd, ".claude/settings.json"),
    JSON.stringify({ hooks: { UserPromptSubmit: [{ hooks: [handler] }] } }),
  );
  const q = query({
    prompt: "Only reply HOOK-PROBE-OK",
    options: {
      cwd,
      settingSources: ["project"],
      mcpServers: {},
      strictMcpConfig: true,
      tools: [],
      maxTurns: 1,
      persistSession: false,
      model: "claude-sonnet-4-6",
      ...(type === "citations" ? { includePartialMessages: true } : {}),
    },
  });
  const timer = setTimeout(() => q.close(), 15000);
  try {
    for await (const msg of q) {
      messages.push({
        type: msg.type,
        subtype: msg.subtype,
        hook: msg.hook_name,
        outcome: msg.outcome,
        error: msg.error,
        result: msg.result,
        errors: msg.errors,
        content: msg.message?.content,
        ...(type === "citations" ? { event: msg.event } : {}),
      });
    }
  } finally {
    clearTimeout(timer);
    q.close();
    server.closeAllConnections();
    await new Promise((r) => server.close(r));
  }
  console.log(JSON.stringify({ calls, hooks: hooks + (fs.existsSync(marker) ? 1 : 0), requests, messages }));
}
