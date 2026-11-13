// Runs only in a fresh child with an isolated profile and a loopback model endpoint.
import fs from "node:fs";
import path from "node:path";
import http from "node:http";
const { query } = await import("../../node_modules/@anthropic-ai/claude-agent-sdk/sdk.mjs");
const [type, policy] = process.argv.slice(2),
  cwd = process.cwd();
const citation = { type: "web_search_result_location", url: "https://example.test/citation-only", title: "Metadata source", encrypted_index: "fixture-only", cited_text: "Evidence" };
const reply = type === "citations" ? "Evidence: [source](https://example.test/source)." : "HOOK-PROBE-OK";
if (type === "idle" || type === "usage-context") {
  fs.writeFileSync(
    path.join(cwd, ".claude/settings.json"),
    JSON.stringify(policy === "default" ? {} : { idleCompaction: policy === "true" }),
  );
  // Initialization controls do not require submitting a prompt to a model.
  const q = query({
    prompt: (async function* () {
      if (type === "usage-context") {
        yield { type: "user", session_id: "", parent_tool_use_id: null, message: { role: "user", content: "/context" } };
      }
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
    if (type === "usage-context") {
      const usage = await q.usage_EXPERIMENTAL_MAY_CHANGE_DO_NOT_RELY_ON_THIS_API_YET();
      const context = await q.getContextUsage({ detail: "summary" });
      let markdown = "";
      for await (const message of q) {
        if (message.type === "assistant") {
          markdown += message.message.content.filter(block => block.type === "text").map(block => block.text).join("\n");
        }
        if (message.type === "result") break;
      }
      console.log(JSON.stringify({ usage, context, markdown, hasSettings: !!response.effective }));
    } else {
      console.log(
        JSON.stringify({ effective: response.effective.idleCompaction, applied: response.applied.idleCompaction }),
      );
    }
  } finally {
    clearTimeout(timeout);
    q.close();
  }
} else {
  let calls = 0,
    hooks = 0;
  const requests = [],
    messages = [];
  const outsidePath = path.resolve(cwd, "../outside-write.txt");
  const readPath = path.join(cwd, "repeated-read.txt");
  if (type === "repeated-read") fs.writeFileSync(readPath, "Repeated reads retain this exact text.\n");
  const toolProbe = {
    "outside-write": { name: "Write", input: { file_path: outsidePath, content: "fixture-only" }, turns: 1 },
    "task-create": { name: "TaskCreate", input: { subject: "Review", description: "Original request" }, turns: 1 },
    "repeated-read": { name: "Read", input: { file_path: readPath }, turns: 2 },
    "question": { name: "AskUserQuestion", input: { questions: [{ question: "Where?", header: "Target", multiSelect: false, options: [{ label: "Production", description: "First choice" }, { label: "Staging", description: "Second choice" }] }] }, turns: 1 },
  }[type];
  let capabilities = [];
  if (type === "outside-write") {
    fs.writeFileSync(path.join(cwd, ".claude/settings.json"), JSON.stringify({ permissions: { blockWritesOutsideWorkingDirectories: policy !== "false" } }));
    if (policy === "inherited") fs.writeFileSync(path.join(cwd, ".claude/settings.local.json"), JSON.stringify({ permissions: { blockWritesOutsideWorkingDirectories: false } }));
  }
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
      content: toolProbe && calls <= toolProbe.turns
        ? [{ type: "tool_use", id: type === "outside-write" ? "write-outside" : `${type}-${calls}`, name: toolProbe.name, input: toolProbe.input }]
        : [{ type: "text", text: reply, ...(type === "citations" ? { citations: [citation] } : {}) }],
      stop_reason: toolProbe && calls <= toolProbe.turns ? "tool_use" : "end_turn",
      stop_sequence: null,
      usage: { input_tokens: 10, output_tokens: 5 },
    };
    if (data.stream) {
      res.writeHead(200, { "Content-Type": "text/event-stream" });
      const send = (event, data) => res.write("event: " + event + "\ndata: " + JSON.stringify(data) + "\n\n");
      send("message_start", { type: "message_start", message: { ...message, content: [], stop_reason: null } });
      const block = message.content[0];
      send("content_block_start", { type: "content_block_start", index: 0, content_block: block.type === "tool_use" ? { ...block, input: {} } : { type: "text", text: "" } });
      send("content_block_delta", {
        type: "content_block_delta",
        index: 0,
        delta: block.type === "tool_use" ? { type: "input_json_delta", partial_json: JSON.stringify(block.input) } : { type: "text_delta", text: reply },
      });
      if (type === "citations") send("content_block_delta", { type: "content_block_delta", index: 0, delta: { type: "citations_delta", citation } });
      send("content_block_stop", { type: "content_block_stop", index: 0 });
      send("message_delta", {
        type: "message_delta",
        delta: { stop_reason: message.stop_reason, stop_sequence: null },
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
  if (type === "command" || type === "http") fs.writeFileSync(
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
      tools: toolProbe ? [toolProbe.name] : [],
      canUseTool: async (_name, input) => {
        if (type !== "question") return { behavior: "allow", updatedInput: input };
        const notes = policy === "notes" ? "Use canary instead" : undefined;
        const annotation = notes ? { notes } : capabilities.includes("ask_user_question_unanswered_v1") ? { unanswered: "skipped" } : undefined;
        return { behavior: "allow", updatedInput: { ...input, answers: { "Where?": notes ?? "Skipped" }, ...(annotation ? { annotations: { "Where?": annotation } } : {}) } };
      },
      maxTurns: toolProbe ? toolProbe.turns + 1 : 1,
      ...(type === "task-create" ? { hooks: { PreToolUse: [{ matcher: "TaskCreate", hooks: [async input => ({ hookSpecificOutput: { hookEventName: "PreToolUse", updatedInput: { ...input.tool_input, description: "Saved after the hook correction" } } })] }] } } : {}),
      persistSession: false,
      model: "claude-sonnet-4-6",
      ...(type === "citations" ? { includePartialMessages: true } : {}),
    },
  });
  const timer = setTimeout(() => q.close(), 15000);
  try {
    for await (const msg of q) {
      if (msg.type === "system" && msg.subtype === "init") capabilities = msg.capabilities ?? [];
      messages.push({
        type: msg.type,
        subtype: msg.subtype,
        hook: msg.hook_name,
        outcome: msg.outcome,
        error: msg.error,
        result: msg.result,
        errors: msg.errors,
        content: msg.message?.content,
        ...(toolProbe ? { toolUseResult: msg.tool_use_result ?? msg.toolUseResult } : {}),
        ...(type === "citations" ? { event: msg.event } : {}),
      });
    }
  } finally {
    clearTimeout(timer);
    q.close();
    server.closeAllConnections();
    await new Promise((r) => server.close(r));
  }
  console.log(JSON.stringify({ calls, hooks: hooks + (fs.existsSync(marker) ? 1 : 0), requests, messages, ...(type === "outside-write" ? { outsideWritten: fs.existsSync(outsidePath) } : {}) }));
}
