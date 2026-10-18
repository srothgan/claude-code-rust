import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

const fixture = fileURLToPath(new URL("../../../tests/fixtures/sdk-settings-runtime.mjs", import.meta.url));

// Each probe owns a native SDK process, profile and project. No user hooks or credentials are used.
async function probe(type: string, policy: string): Promise<Record<string, unknown>> {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "claude-rs-settings-runtime-"));
  const cwd = path.join(root, "project");
  const profile = path.join(root, "profile");
  await fs.mkdir(path.join(cwd, ".claude"), { recursive: true });
  await fs.mkdir(profile);
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
    const { stdout } = await promisify(execFile)(process.execPath, [fixture, type, policy], {
      cwd, env, encoding: "utf8", windowsHide: true, timeout: 30_000,
    });
    const line = stdout.trim().split("\n").at(-1);
    assert.ok(line, "native probe response");
    return JSON.parse(line);
  } finally {
    await fs.rm(root, { recursive: true, force: true });
  }
}

test("the installed SDK enforces command and HTTP failure policy before making a model request", async () => {
  for (const type of ["command", "http"]) {
    for (const policy of ["default", "continue", "block"]) {
      const result = await probe(type, policy);
      const context = [type, policy].join("/");
      assert.equal(result.hooks, 1, context);
      assert.equal(result.calls, policy === "block" ? 0 : 1, context);
      const messages = result.messages as Array<Record<string, unknown>>;
      assert.ok(messages.some(message => message.type === "result"), JSON.stringify(messages));
      if (policy === "block") {
        assert.ok(messages.some(message => typeof message.result === "string" && message.result.includes("blocked by hook")), JSON.stringify(messages));
        assert.equal(messages.some(message => message.type === "assistant"), false);
      } else {
        assert.ok(messages.some(message => message.type === "assistant"), JSON.stringify(messages));
      }
    }
  }
});

test("a fresh native session reports the saved idle preference without an applied activation flag", async () => {
  for (const value of ["false", "true", "default"]) {
    const result = await probe("idle", value);
    assert.equal(result.effective, value === "default" ? undefined : value === "true");
    assert.equal(result.applied, undefined, "saved preference must not be presented as active feature evidence");
  }
});

test("SDK 0.3.302 native streaming accumulates citation deltas into completed text blocks", async () => {
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
