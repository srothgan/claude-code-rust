import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

const fixture = fileURLToPath(
  new URL("../../../tests/fixtures/sdk-settings-runtime.mjs", import.meta.url),
);

// Each probe owns a native SDK process, profile and project. No user hooks or credentials are used.
async function probe(
  type: string,
  policy: string,
): Promise<Record<string, unknown>> {
  const root = await fs.mkdtemp(
    path.join(os.tmpdir(), "claude-rs-settings-runtime-"),
  );
  const cwd = path.join(root, "project");
  const profile = path.join(root, "profile");
  const env = { ...process.env };
  for (const key of Object.keys(env)) {
    if (/^(ANTHROPIC_|CLAUDE_|CLAUDECODE$)/.test(key)) delete env[key];
  }
  Object.assign(env, {
    CLAUDE_CONFIG_DIR: profile,
    CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: "1",
    ANTHROPIC_AUTH_TOKEN: "local-fixture-token",
    ANTHROPIC_BASE_URL: "http://127.0.0.1:1",
  });
  try {
    await fs.mkdir(path.join(cwd, ".claude"), { recursive: true });
    await fs.mkdir(profile);
    const { stdout } = await promisify(execFile)(
      process.execPath,
      [fixture, type, policy],
      {
        cwd,
        env,
        encoding: "utf8",
        windowsHide: true,
        timeout: 30_000,
      },
    );
    const line = stdout.trim().split("\n").at(-1);
    assert.ok(line, "native probe response");
    return { ...JSON.parse(line), cwd };
  } finally {
    await fs.rm(root, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
  }
}

type NativeMessage = {
  type: string;
  subtype?: string;
  outcome?: string;
  result?: string;
  is_error?: boolean;
  errors?: string[];
  content?: Array<{ type: string; text?: string }>;
};
type HookInput = { hook_event_name: string; prompt: string };
type ModelRequest = {
  method: string;
  route: string;
  body: {
    model: string;
    messages: Array<{
      role: string;
      content: string | Array<{ text?: string }>;
    }>;
  };
};

test("host-saved command and HTTP failure policies govern the native turn", async () => {
  for (const type of ["command", "http"]) {
    for (const policy of ["default", "continue", "block"]) {
      const result = await probe(type, policy);
      const context = `${type}/${policy}`;
      const prompt = `Native settings probe ${context}: only reply HOOK-PROBE-OK`;
      const hookRequests = result.hookRequests as Array<{
        method: string;
        route: string;
        body: HookInput;
      }>;
      const commandHooks = result.commandHooks as HookInput[];
      const invocations =
        type === "command"
          ? commandHooks
          : hookRequests.map((request) => request.body);
      assert.equal(invocations.length, 1, `${context}: actual hook invocation`);
      assert.equal(invocations[0].hook_event_name, "UserPromptSubmit", context);
      assert.equal(invocations[0].prompt, prompt, context);
      let handler: Record<string, unknown>;
      if (type === "command") {
        handler = {
          type,
          command: `"${process.execPath}" "${path.join(result.cwd as string, "fail.mjs")}"`,
        };
        assert.deepEqual(hookRequests, [], context);
      } else {
        handler = { type, url: `${result.endpoint}/hook` };
        assert.deepEqual(commandHooks, [], context);
        assert.equal(hookRequests[0].method, "POST", context);
        assert.equal(hookRequests[0].route, "/hook", context);
      }
      if (policy !== "default") handler.onFailure = policy;
      assert.deepEqual(
        result.saved,
        { hooks: { UserPromptSubmit: [{ hooks: [handler] }] } },
        context,
      );
      const generation = result.generation as ModelRequest[];
      const messages = result.messages as NativeMessage[];
      const terminal = messages.filter((message) => message.type === "result");
      assert.equal(terminal.length, 1, `${context}: terminal result`);
      if (policy === "block") {
        assert.deepEqual(
          generation,
          [],
          `${context}: refuse before model generation`,
        );
        assert.deepEqual(
          messages.filter((message) => message.type === "assistant"),
          [],
          context,
        );
        assert.match(terminal[0].result ?? "", /hook/i, context);
        assert.match(
          terminal[0].result ?? "",
          type === "command" ? /fixture hook exit 7/ : /500/,
          context,
        );
      } else {
        assert.equal(generation.length, 1, context);
        assert.equal(generation[0].method, "POST", context);
        assert.equal(generation[0].route, "/v1/messages", context);
        assert.equal(generation[0].body.model, "claude-sonnet-4-6", context);
        assert.ok(
          generation[0].body.messages.some(
            (message) =>
              message.role === "user" &&
              (typeof message.content === "string"
                ? message.content.includes(prompt)
                : message.content.some((block) =>
                    block.text?.includes(prompt),
                  )),
          ),
          `${context}: submitted prompt reaches model`,
        );
        assert.deepEqual(
          messages
            .filter((message) => message.type === "assistant")
            .map((message) => message.content),
          [[{ type: "text", text: "HOOK-PROBE-OK" }]],
          context,
        );
        assert.equal(terminal[0].subtype, "success", context);
        assert.equal(terminal[0].is_error, false, context);
        assert.equal(terminal[0].result, "HOOK-PROBE-OK", context);
      }
    }
  }
});

test("fresh native sessions observe host-saved idle preferences and a real reset", async () => {
  const result = await probe("idle", "reset");
  assert.deepEqual(result.observations, [
    { saved: { idleCompaction: false }, effective: false },
    { saved: { idleCompaction: true }, effective: true },
    { saved: {} },
  ]);
});

test("the installed SDK supports usage, settings, and SDK-rendered context without a model request", async () => {
  const result = await probe("usage-context", "default");
  assert.equal(result.hasSettings, true);
  const requests = result.requests as Array<{ method: string; route: string }>;
  assert.deepEqual(
    requests.filter(request => request.route.split("?")[0] === "/v1/messages"),
    [],
    "native usage/settings/context controls make no model generation requests",
  );
  const usage = result.usage as { rate_limits_available: boolean; behaviors: unknown; session: { total_cost_usd: number } };
  assert.equal(usage.rate_limits_available, false);
  assert.equal(usage.behaviors, null);
  assert.equal(usage.session.total_cost_usd, 0);
  const context = result.context as { percentage: number; totalTokens: number; maxTokens: number };
  assert.ok(Number.isFinite(context.percentage));
  assert.ok(context.totalTokens >= 0);
  assert.ok(context.maxTokens > 0);
  assert.match(String(result.markdown), /context usage/i);
});

test("native SDK streaming accumulates citation deltas into completed text blocks", async () => {
  const result = await probe("citations", "default");
  assert.equal(result.calls, 1);
  const messages = result.messages as Array<{ type: string; content?: unknown; event?: { type?: string; delta?: { type?: string } } }>;
  assert.ok(messages.some(message => message.event?.delta?.type === "citations_delta"));
  assert.deepEqual(messages.filter(message => message.type === "assistant").map(message => message.content), [[{
    type: "text", text: "Evidence: [source](https://example.test/source).", citations: [{
      type: "web_search_result_location", url: "https://example.test/citation-only", title: "Metadata source",
      encrypted_index: "fixture-only", cited_text: "Evidence",
    }],
  }]]);
});


test("native outside-write protection blocks writes even when a narrower scope sets false", async () => {
  for (const policy of ["true", "false", "inherited"]) {
    const result = await probe("outside-write", policy);
    assert.equal(result.outsideWritten, policy === "false", JSON.stringify(result));
    const messages = result.messages as Array<{ type: string; content?: Array<{ type: string; tool_use_id?: string; is_error?: boolean }> }>;
    const response = messages.flatMap(message => message.content ?? []).find(block => block.type === "tool_result" && block.tool_use_id === "write-outside");
    assert.ok(response, JSON.stringify(result));
    assert.equal(response.is_error === true, policy !== "false");
  }
});


test("native TaskCreate reports the description saved after hook input correction", async () => {
  const result = await probe("task-create", "default");
  const messages = result.messages as Array<{ toolUseResult?: { task?: { description?: string } } }>;
  const saved = messages.find(message => message.toolUseResult?.task)?.toolUseResult?.task;
  assert.equal(saved?.description, "Saved after the hook correction", JSON.stringify(result));
});


test("native questions accept note-only text and echo the advertised skip annotation", async () => {
  for (const policy of ["notes", "skip"]) {
    const result = await probe("question", policy);
    const messages = result.messages as Array<{ toolUseResult?: { answers?: Record<string, string>; annotations?: Record<string, { notes?: string; unanswered?: string }> } }>;
    const output = messages.find(message => message.toolUseResult?.answers)?.toolUseResult;
    assert.equal(output?.answers?.["Where?"], policy === "notes" ? "Use canary instead" : "Skipped", JSON.stringify(result));
    assert.deepEqual(output?.annotations?.["Where?"], policy === "notes" ? { notes: "Use canary instead" } : { unanswered: "skipped" });
  }
});


test("repeated native Reads both return file text instead of an unchanged-file sentinel", async () => {
  const result = await probe("repeated-read", "default");
  const messages = result.messages as Array<{ content?: Array<{ type: string; tool_use_id?: string; content?: unknown }> }>;
  for (const id of ["repeated-read-1", "repeated-read-2"]) {
    const response = messages.flatMap(message => message.content ?? []).find(block => block.type === "tool_result" && block.tool_use_id === id);
    assert.ok(response, JSON.stringify(result));
    assert.match(JSON.stringify(response.content), /Repeated reads retain this exact text/);
    assert.doesNotMatch(JSON.stringify(response.content), /file_unchanged/);
  }
});
