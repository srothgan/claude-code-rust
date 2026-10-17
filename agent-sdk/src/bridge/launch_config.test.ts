import assert from "node:assert/strict";
import test from "node:test";
import { parseCommandEnvelope } from "./commands.js";
import {
  buildFlagSettings,
  buildLaunchQueryOptions,
  buildSystemPrompt,
  fileCheckpointingEnabled,
  mergeMcpServers,
} from "./launch_config.js";

const GUARD = "guard text";

test("host restrictions are laid over the user's --settings layer", () => {
  const layered = buildFlagSettings(
    { settings: { spinnerTipsEnabled: false, feedbackDrafts: "on" } },
    {},
  );
  assert.deepEqual(layered, {
    spinnerTipsEnabled: false,
    feedbackDrafts: "off",
    crossSessionInbound: "refuse",
  });
});

test("an explicit crossSessionInbound survives from the flag layer and from the saved cascade", () => {
  assert.equal(
    buildFlagSettings({ settings: { crossSessionInbound: "accept" } }, { crossSessionInbound: "refuse" })
      .crossSessionInbound,
    "accept",
  );
  // Nothing in the flag layer: the saved value decides, and the SDK keeps reading it.
  assert.equal(
    "crossSessionInbound" in buildFlagSettings({}, { crossSessionInbound: "accept" }),
    false,
  );
  // A flag value outside the two accepted ones is replaced by the restriction.
  assert.equal(
    buildFlagSettings({ settings: { crossSessionInbound: "ask" as never } }, { crossSessionInbound: "accept" })
      .crossSessionInbound,
    "refuse",
  );
});

test("fileCheckpointingEnabled follows the flag layer, then the saved cascade, then on", () => {
  assert.equal(fileCheckpointingEnabled({ settings: { fileCheckpointingEnabled: false } }, { fileCheckpointingEnabled: true }), false);
  assert.equal(fileCheckpointingEnabled({}, { fileCheckpointingEnabled: false }), false);
  assert.equal(fileCheckpointingEnabled({}, {}), true);
});

test("later MCP configs replace earlier servers of the same name and keep the rest", () => {
  assert.equal(mergeMcpServers(undefined), undefined);
  assert.equal(mergeMcpServers([]), undefined);
  const merged = mergeMcpServers([
    { docs: { command: "docs-v1" }, search: { command: "search-v1" } },
    { docs: { command: "docs-v2" } },
  ]);
  assert.deepEqual(merged, { docs: { command: "docs-v2" }, search: { command: "search-v1" } });
});

test("an MCP server named __proto__ is an ordinary entry", () => {
  const config = JSON.parse('{"__proto__":{"command":"x"}}') as { [key: string]: never };
  const merged = mergeMcpServers([config]);
  assert.equal(Object.hasOwn(merged ?? {}, "__proto__"), true);
  assert.equal(Object.getPrototypeOf(merged), Object.prototype);
});

test("the system prompt keeps the runtime guard whichever flags are given", () => {
  assert.deepEqual(buildSystemPrompt({}, GUARD), {
    type: "preset",
    preset: "claude_code",
    append: GUARD,
    snapshot: true,
  });
  assert.deepEqual(buildSystemPrompt({ append_system_prompt: "Answer in German." }, GUARD), {
    type: "preset",
    preset: "claude_code",
    append: `${GUARD}\n\nAnswer in German.`,
    snapshot: true,
  });
  assert.deepEqual(buildSystemPrompt({ system_prompt: "You are a release bot." }, GUARD), {
    type: "custom",
    prompt: ["You are a release bot.", GUARD],
    snapshot: true,
  });
  assert.deepEqual(
    buildSystemPrompt({ system_prompt: "You are a release bot.", append_system_prompt: "End with DONE." }, GUARD),
    { type: "custom", prompt: ["You are a release bot.", `${GUARD}\n\nEnd with DONE.`], snapshot: true },
  );
});

// sdk.d.ts documents `title` for a new session; a real `--continue --fork-session -n` run
// stored it as the fork's custom title, so the option is passed for forks too.
test("the SDK title option names a new or forked session, never a plain resume", () => {
  const launch = { name: "Experiment" };
  assert.deepEqual(buildLaunchQueryOptions(launch, {}), { title: "Experiment" });
  assert.deepEqual(buildLaunchQueryOptions(launch, { resume: "s1", forkSession: true }), {
    title: "Experiment",
  });
  // The SDK would ignore it for a resumed session, which the resume path renames instead.
  assert.deepEqual(buildLaunchQueryOptions(launch, { resume: "s1" }), {});
});

test("launch query options carry directories, servers and strict mode, and nothing else when unset", () => {
  assert.deepEqual(buildLaunchQueryOptions({}, {}), {});
  assert.deepEqual(
    buildLaunchQueryOptions(
      {
        additional_directories: ["/work/shared"],
        mcp_configs: [{ docs: { command: "docs-mcp" } }],
        strict_mcp_config: true,
      },
      {},
    ),
    {
      additionalDirectories: ["/work/shared"],
      mcpServers: { docs: { command: "docs-mcp" } },
      strictMcpConfig: true,
    },
  );
});

function launchCommand(launchSettings: unknown): string {
  return JSON.stringify({ command: "create_session", cwd: "/work", launch_settings: launchSettings });
}

test("the command parser accepts the launch configuration fields as the app sends them", () => {
  const launchSettings = {
    name: "Release prep",
    fork_session: true,
    additional_directories: ["/work/shared"],
    settings: { spinnerTipsEnabled: false },
    mcp_configs: [{ docs: { command: "docs-mcp" } }],
    strict_mcp_config: true,
    system_prompt: "You are a release bot.",
    append_system_prompt: "End with DONE.",
  };
  const parsed = parseCommandEnvelope(launchCommand(launchSettings)).command;
  assert.equal(parsed.command, "create_session");
  assert.deepEqual(parsed.launch_settings, launchSettings);
});

test("the command parser rejects launch configuration of the wrong shape", () => {
  for (const launchSettings of [
    { mcp_configs: "docs" },
    { mcp_configs: ["docs"] },
    { settings: ["a"] },
    { additional_directories: [1] },
    { strict_mcp_config: "yes" },
    { system_prompt: 3 },
  ]) {
    assert.throws(() => parseCommandEnvelope(launchCommand(launchSettings)), `${JSON.stringify(launchSettings)}`);
  }
});
