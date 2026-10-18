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
  assert.equal(result.sdk, "0.3.302");
  type Observation = {
    name: string;
    held?: boolean;
    error_count?: number;
    plugins?: Array<{ name: string; path: string }>;
    commands?: Array<{ name: string }>;
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
  const held = observation("held-mcp");
  assert.equal(held.held, true);
  assert.equal(names(held).includes("fixture-mcp"), false);
  assert.ok(names(observation("apply-mcp")).includes("fixture-mcp"));
  assert.equal(names(observation("resume")).includes("fixture-a"), false);
  assert.ok(names(observation("resume-explicit")).includes("fixture-a"));
  assert.ok(
    observation("resume-explicit").commands?.some((command) => command.name === "fixture-a:hello"),
  );
  assert.equal(names(observation("new")).includes("fixture-a"), false);
});
