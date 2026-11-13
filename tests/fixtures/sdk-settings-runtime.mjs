// Runs only in a fresh child with an isolated profile and loopback endpoints.
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import http from "node:http";
import { query, resolveSettings } from "@anthropic-ai/claude-agent-sdk";
import { inspectSettings, mutateSetting } from "../../agent-sdk/dist/bridge/settings_service.js";
import { buildQueryOptions } from "../../agent-sdk/dist/bridge/session_lifecycle.js";
import { AsyncQueue } from "../../agent-sdk/dist/bridge/shared.js";

const [type, policy] = process.argv.slice(2);
const cwd = process.cwd();
const settingsFile = path.join(cwd, ".claude/settings.json");
const prompt = `Native settings probe ${type}/${policy}: only reply HOOK-PROBE-OK`;
const citation = {
  type: "web_search_result_location",
  url: "https://example.test/citation-only",
  title: "Metadata source",
  encrypted_index: "fixture-only",
  cited_text: "Evidence",
};
const reply =
  type === "citations" ? "Evidence: [source](https://example.test/source)." : "HOOK-PROBE-OK";

async function save(id, value) {
  const snapshot = await inspectSettings(cwd);
  const entry = snapshot.sources
    .find((source) => source.scope === "project")
    .values.find((entry) => entry.id === id);
  assert.ok(entry, `editable ${id}`);
  const result = await mutateSetting(cwd, {
    context: snapshot.context,
    scope: "project",
    id,
    expected_revision: entry.revision,
    operation: value === undefined ? "remove" : "set",
    ...(value === undefined ? {} : { value }),
  });
  assert.equal(result.persistence, "saved", `${id}: ${result.error ?? result.persistence}`);
  assert.notEqual(result.application, "blocked", result.error);
  const saved = JSON.parse(fs.readFileSync(settingsFile, "utf8"));
  const descriptor = snapshot.catalog.find(setting => setting.id === id);
  assert.ok(descriptor, `catalog entry ${id}`);
  const persisted = descriptor.key_path.reduce((object, key) => object?.[key], saved);
  if (value === undefined)
    assert.equal(persisted, undefined, `${id}: reset removes leaf`);
  else assert.deepEqual(persisted, value, `${id}: persisted selected leaf before launch`);
  return saved;
}

async function nativeTurn(observe, submitPrompt = true, probeOptions = {}) {
  const input = new AsyncQueue();
  const children = [];
  let q,
    timer,
    timedOut = false;
  try {
    const { effective: resolvedSettings } = await resolveSettings({
      cwd,
      settingSources: ["user", "project", "local"],
    });
    const host = buildQueryOptions({
      cwd,
      resolvedSettings,
      launchSettings: { model: "claude-sonnet-4-6" },
      provisionalSessionId: randomUUID(),
      input,
      canUseTool: async () => ({ behavior: "deny", message: "No tools in this fixture" }),
      enableSdkDebug: false,
      enableSpawnDebug: false,
      sessionIdForLogs: () => "",
    });
    q = query({
      prompt: submitPrompt ? prompt : input,
      options: {
        ...host,
        mcpServers: {},
        strictMcpConfig: true,
        tools: [],
        maxTurns: 1,
        ...probeOptions,
        persistSession: false,
        spawnClaudeCodeProcess(options) {
          const child = host.spawnClaudeCodeProcess(options);
          const closed = new Promise((resolve) => child.once("close", resolve));
          children.push({ child, closed });
          return child;
        },
      },
    });
    const deadline = new Promise((_, reject) => {
      timer = setTimeout(() => {
        timedOut = true;
        reject(new Error("Native settings probe watchdog expired"));
        q.close();
      }, 15_000);
    });
    const result = await Promise.race([observe(q, input), deadline]);
    assert.equal(timedOut, false, "watchdog closure is not a completed observation");
    return result;
  } finally {
    clearTimeout(timer);
    input.close();
    try {
      q?.close();
    } finally {
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
  }
}

let server;
try {
  if (type === "idle") {
    const observations = [];
    for (const value of [false, true, undefined]) {
      const saved = await save("idleCompaction", value);
      const effective = await nativeTurn(async (q) => {
        await q.initializationResult();
        const response = await q.getSettings();
        return response.effective.idleCompaction;
      }, false);
      observations.push({ saved, effective });
    }
    console.log(JSON.stringify({ observations }));
  } else if (type === "usage-context") {
    const requests = [];
    server = http.createServer((request, response) => {
      requests.push({ method: request.method, route: request.url });
      if (new URL(request.url, "http://127.0.0.1").pathname === "/v1/messages/count_tokens") {
        response.writeHead(200, { "Content-Type": "application/json" });
        response.end(JSON.stringify({ input_tokens: 10 }));
        return;
      }
      response.writeHead(500);
      response.end("Context and usage controls must not request a model");
    });
    await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
    process.env.ANTHROPIC_BASE_URL = `http://127.0.0.1:${server.address().port}`;
    const result = await nativeTurn(async (q, input) => {
      await q.initializationResult();
      const settings = await q.getSettings();
      const usage = await q.usage_EXPERIMENTAL_MAY_CHANGE_DO_NOT_RELY_ON_THIS_API_YET();
      const context = await q.getContextUsage({ detail: "summary" });
      input.enqueue({
        type: "user", session_id: "", parent_tool_use_id: null,
        message: { role: "user", content: "/context" },
      });
      let markdown = "";
      for await (const message of q) {
        if (message.type === "assistant") {
          markdown += message.message.content.filter(block => block.type === "text")
            .map(block => block.text).join("\n");
        }
        if (message.type === "result") {
          assert.equal(message.is_error, false);
          break;
        }
      }
      return { usage, context, markdown, hasSettings: !!settings.effective };
    }, false);
    console.log(JSON.stringify({ ...result, requests }));
  } else {
    const generation = [],
      hookRequests = [],
      otherRequests = [],
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
      await save("permissions.blockWritesOutsideWorkingDirectories", policy !== "false");
      if (policy === "inherited") fs.writeFileSync(path.join(cwd, ".claude/settings.local.json"), JSON.stringify({ permissions: { blockWritesOutsideWorkingDirectories: false } }));
    }
    server = http.createServer(async (req, res) => {
      try {
        let raw = "";
        for await (const chunk of req) raw += chunk;
        const route = new URL(req.url, "http://127.0.0.1").pathname;
        const request = { method: req.method, route, body: JSON.parse(raw || "{}") };
        if (route === "/hook") {
          hookRequests.push(request);
          res.writeHead(500);
          res.end("controlled harmless failure");
          return;
        }
        if (route !== "/v1/messages") {
          otherRequests.push(request);
          res.writeHead(route === "/v1/messages/count_tokens" ? 200 : 404, {
            "Content-Type": "application/json",
          });
          res.end(JSON.stringify({ input_tokens: 10 }));
          return;
        }
        generation.push(request);
        const data = request.body;
        const message = {
          id: "msg_fixture",
          type: "message",
          role: "assistant",
          model: data.model,
          content: [
            {
              type: "text",
              text: reply,
              ...(type === "citations" ? { citations: [citation] } : {}),
            },
          ],
          stop_reason: toolProbe && generation.length <= toolProbe.turns ? "tool_use" : "end_turn",
          stop_sequence: null,
          usage: { input_tokens: 10, output_tokens: 5 },
        };
        if (toolProbe && generation.length <= toolProbe.turns) {
          message.content = [{ type: "tool_use", id: type === "outside-write" ? "write-outside" : `${type}-${generation.length}`, name: toolProbe.name, input: toolProbe.input }];
        }
        if (data.stream) {
          res.writeHead(200, { "Content-Type": "text/event-stream" });
          const send = (event, data) =>
            res.write("event: " + event + "\ndata: " + JSON.stringify(data) + "\n\n");
          send("message_start", {
            type: "message_start",
            message: { ...message, content: [], stop_reason: null },
          });
          send("content_block_start", {
            type: "content_block_start",
            index: 0,
            content_block: message.content[0].type === "tool_use" ? { ...message.content[0], input: {} } : { type: "text", text: "" },
          });
          send("content_block_delta", {
            type: "content_block_delta",
            index: 0,
            delta: message.content[0].type === "tool_use" ? { type: "input_json_delta", partial_json: JSON.stringify(message.content[0].input) } : { type: "text_delta", text: reply },
          });
          if (type === "citations")
            send("content_block_delta", {
              type: "content_block_delta",
              index: 0,
              delta: { type: "citations_delta", citation },
            });
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
      } catch (error) {
        console.error(error);
        res.writeHead(500);
        res.end("Local fixture request failed");
      }
    });
    await new Promise((resolve, reject) => {
      server.once("error", reject);
      server.listen(0, "127.0.0.1", () => {
        server.off("error", reject);
        resolve();
      });
    });
    const base = `http://127.0.0.1:${server.address().port}`;
    process.env.ANTHROPIC_BASE_URL = base;
    const journal = path.join(cwd, "hook-invocations.jsonl");
    let saved, handler;
    if (type === "command" || type === "http") {
      const failure = path.join(cwd, "fail.mjs");
      fs.writeFileSync(
        failure,
        `import fs from "node:fs"; fs.appendFileSync(${JSON.stringify(journal)}, JSON.stringify(JSON.parse(fs.readFileSync(0,"utf8")))+"\\n"); process.stderr.write("fixture hook exit 7"); process.exit(7);`,
      );
      handler =
        type === "command"
          ? { type, command: `"${process.execPath}" "${failure}"` }
          : { type, url: `${base}/hook` };
      if (policy !== "default") handler.onFailure = policy;
      saved = await save("hooks", { UserPromptSubmit: [{ hooks: [handler] }] });
    }
    await nativeTurn(async (q) => {
      for await (const msg of q) {
        if (msg.type === "system" && msg.subtype === "init") capabilities = msg.capabilities ?? [];
        messages.push({
          type: msg.type,
          subtype: msg.subtype,
          hook: msg.hook_name,
          outcome: msg.outcome,
          error: msg.error,
          result: msg.result,
          is_error: msg.is_error,
          errors: msg.errors,
          content: msg.message?.content,
          ...(toolProbe ? { toolUseResult: msg.tool_use_result ?? msg.toolUseResult } : {}),
          ...(type === "citations" ? { event: msg.event } : {}),
        });
      }
    }, true, toolProbe ? {
      tools: [toolProbe.name], maxTurns: toolProbe.turns + 1,
      canUseTool: async (_name, input) => {
        if (type !== "question") return { behavior: "allow", updatedInput: input };
        const notes = policy === "notes" ? "Use canary instead" : undefined;
        const annotation = notes ? { notes } : capabilities.includes("ask_user_question_unanswered_v1") ? { unanswered: "skipped" } : undefined;
        return { behavior: "allow", updatedInput: { ...input, answers: { "Where?": notes ?? "Skipped" }, ...(annotation ? { annotations: { "Where?": annotation } } : {}) } };
      },
      ...(type === "task-create" ? { hooks: { PreToolUse: [{ matcher: "TaskCreate", hooks: [async input => ({ hookSpecificOutput: { hookEventName: "PreToolUse", updatedInput: { ...input.tool_input, description: "Saved after the hook correction" } } })] }] } } : {}),
    } : {});
    // The retained citation test also requires a completed successful native turn.
    if (type === "citations") {
      assert.ok(
        messages.some(
          (msg) => msg.type === "result" && msg.subtype === "success" && msg.is_error === false,
        ),
      );
    }
    const commandHooks = fs.existsSync(journal)
      ? fs
          .readFileSync(journal, "utf8")
          .trim()
          .split("\n")
          .map((line) => JSON.parse(line))
      : [];
    console.log(
      JSON.stringify({
        saved,
        endpoint: base,
        generation,
        hookRequests,
        commandHooks,
      ...(type === "outside-write" ? { outsideWritten: fs.existsSync(outsidePath) } : {}),
        otherRequests,
        messages,
        calls: generation.length,
      }),
    );
  }
} finally {
  if (server) {
    server.closeAllConnections();
    if (server.listening) await new Promise((resolve) => server.close(resolve));
  }
}
