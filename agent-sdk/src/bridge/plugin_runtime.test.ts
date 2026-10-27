import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

test("native plugin attachment, partial application, held Apply and explicit resume preserve SDK semantics", async () => {
  const env = { ...process.env };
  for (const key of Object.keys(env)) {
    if (/^(ANTHROPIC_|CLAUDE_|CLAUDECODE$)/.test(key)) delete env[key];
  }
  const fixture = fileURLToPath(
    new URL("../../../tests/fixtures/sdk-plugin-runtime.mjs", import.meta.url),
  );
  const { stdout } = await promisify(execFile)(process.execPath, [fixture], {
    env,
    encoding: "utf8",
    timeout: 60_000,
    windowsHide: true,
  });
  const result = JSON.parse(stdout);
  assert.equal(result.sdk, "0.3.311");
  type Observation = {
    name: string;
    held?: boolean;
    error_count?: number;
    plugins?: Array<{ name: string; path: string }>;
    commands?: Array<{ name: string }>;
    subtype?: string;
    is_error?: boolean;
    result?: string;
    cache_impact?: { mcp_servers_added: string[] };
  };
  const observation = (name: string): Observation => {
    const entry = (result.results as Observation[]).find((entry) => entry.name === name);
    assert.ok(entry, name);
    return entry;
  };
  const names = (entry: Observation) => entry.plugins?.map((plugin) => plugin.name) ?? [];
  assert.ok(names(observation("attach")).includes("fixture-a"));
  assert.equal(names(observation("duplicate")).filter((name) => name === "fixture-a").length, 1);
  const partial = observation("partial");
  assert.ok((partial.error_count ?? 0) > 0);
  assert.ok(names(partial).includes("fixture-a"));
  assert.equal(names(partial).includes("fixture-b"), false);
  assert.ok(names(observation("retry-repaired")).includes("fixture-b"));
  assert.equal(observation("turn").subtype, "success");
  assert.equal(observation("turn").is_error, false);
  assert.equal(observation("turn").result, "LOCAL-PROBE-OK");
  const held = observation("held-mcp");
  assert.equal(held.held, true);
  assert.deepEqual(held.cache_impact?.mcp_servers_added, ["plugin:fixture-mcp:fixture"]);
  assert.equal(names(held).includes("fixture-mcp"), false);
  assert.ok(names(observation("apply-mcp")).includes("fixture-mcp"));
  assert.equal(names(observation("resume")).includes("fixture-a"), false);
  assert.ok(names(observation("resume-explicit")).includes("fixture-a"));
  assert.ok(
    observation("resume-explicit").commands?.some((command) => command.name === "fixture-a:hello"),
  );
  assert.equal(names(observation("new")).includes("fixture-a"), false);
});
