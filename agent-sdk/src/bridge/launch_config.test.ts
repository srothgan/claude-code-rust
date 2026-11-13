import assert from "node:assert/strict";
import test from "node:test";
import { parseCommandEnvelope } from "./commands.js";
import {
  buildFlagSettings,
  buildSystemPrompt,
  fileCheckpointingEnabled,
  mergeMcpServers,
} from "./launch_config.js";

const GUARD = "guard text";

test("host restrictions preserve user settings and inbound policy precedence without mutating inputs", () => {
  const rows = [
    { flag: {}, saved: {}, expected: { crossSessionInbound: "refuse" } },
    {
      flag: { crossSessionInbound: "accept" },
      saved: { crossSessionInbound: "refuse" },
      expected: { crossSessionInbound: "accept" },
    },
    {
      flag: { crossSessionInbound: "refuse" },
      saved: { crossSessionInbound: "accept" },
      expected: { crossSessionInbound: "refuse" },
    },
    { flag: {}, saved: { crossSessionInbound: "accept" }, expected: {} },
    { flag: {}, saved: { crossSessionInbound: "refuse" }, expected: {} },
    {
      flag: {},
      saved: { crossSessionInbound: "ask" },
      expected: { crossSessionInbound: "refuse" },
    },
    {
      flag: { crossSessionInbound: "ask" },
      saved: { crossSessionInbound: "accept" },
      expected: { crossSessionInbound: "refuse" },
    },
  ];
  for (const row of rows) {
    // Generic flag settings are admitted as JSON by the actual command parser.
    const decoded = parseCommandEnvelope(
      launchCommand({
        settings: {
          ...row.flag,
          spinnerTipsEnabled: false,
          feedbackDrafts: "on",
          custom: { token: "keep", list: [1, "two"] },
        },
      }),
    ).command;
    assert.equal(decoded.command, "create_session");
    if (decoded.command !== "create_session") return;
    const saved = JSON.parse(JSON.stringify(row.saved));
    const before = structuredClone({ launch: decoded.launch_settings, saved });
    assert.deepEqual(buildFlagSettings(decoded.launch_settings, saved), {
      spinnerTipsEnabled: false,
      feedbackDrafts: "off",
      custom: { token: "keep", list: [1, "two"] },
      ...row.expected,
    });
    assert.deepEqual({ launch: decoded.launch_settings, saved }, before);
  }
});

test("file checkpointing honors opposing explicit values, saved values and the opt-in default", () => {
  const rows = [
    { flag: true, saved: false, expected: true },
    { flag: false, saved: true, expected: false },
    { flag: undefined, saved: true, expected: true },
    { flag: undefined, saved: false, expected: false },
    { flag: undefined, saved: undefined, expected: true },
    { flag: "invalid", saved: false, expected: false },
    { flag: null, saved: true, expected: true },
  ];
  for (const { flag, saved, expected } of rows) {
    assert.equal(
      fileCheckpointingEnabled(
        { settings: flag === undefined ? {} : { fileCheckpointingEnabled: flag } },
        { ...(saved === undefined ? {} : { fileCheckpointingEnabled: saved }) },
      ),
      expected,
    );
  }
});

test("MCP merge preserves exact last-writer payloads and literal keys without inherited servers", () => {
  assert.equal(mergeMcpServers(undefined), undefined);
  assert.equal(mergeMcpServers([]), undefined);
  const first = Object.assign(
    Object.create({ inherited: { command: "must-not-launch" } }),
    JSON.parse(
      '{"docs":{"command":"old-docs","args":["--old"]},"search":{"command":"search","env":{"MODE":"local"}},"__proto__":{"command":"old-special"}}',
    ),
  );
  const second = JSON.parse(
    '{"docs":{"command":"new-docs","args":["--new","literal space"]},"__proto__":{"command":"special","args":["--literal"],"env":{"TOKEN":"value"}}}',
  );
  const before = JSON.stringify([first, second]);
  const merged = mergeMcpServers([first, second]);
  assert.ok(merged);
  assert.equal(Object.hasOwn(merged, "__proto__"), true);
  assert.equal(Object.hasOwn(merged, "inherited"), false);
  assert.deepEqual(
    JSON.parse(JSON.stringify(merged)),
    JSON.parse(
      '{"docs":{"command":"new-docs","args":["--new","literal space"]},"search":{"command":"search","env":{"MODE":"local"}},"__proto__":{"command":"special","args":["--literal"],"env":{"TOKEN":"value"}}}',
    ),
  );
  assert.equal(JSON.stringify([first, second]), before);
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




function launchCommand(launchSettings: unknown): string {
  return JSON.stringify({ command: "create_session", cwd: "/work", launch_settings: launchSettings });
}

test("launch envelope decoding preserves authored payloads, empty prompts and lifecycle identity", () => {
  const raw =
    '{"command":"create_session","request_id":"launch-request","cwd":"/work","launch_settings":{"name":"Release prep","fork_session":true,"additional_directories":["/work/shared"],"settings":{"spinnerTipsEnabled":false,"nested":{"literal":[1,"two"]}},"mcp_configs":[{"docs":{"command":"docs-mcp","args":["--literal"]}}],"strict_mcp_config":true,"system_prompt":"  First\\nSecond  ","append_system_prompt":""}}';
  const parsed = parseCommandEnvelope(raw);
  assert.equal(parsed.requestId, "launch-request");
  assert.equal(parsed.command.command, "create_session");
  assert.deepEqual(parsed.command.launch_settings, {
    name: "Release prep",
    fork_session: true,
    additional_directories: ["/work/shared"],
    settings: { spinnerTipsEnabled: false, nested: { literal: [1, "two"] } },
    mcp_configs: [{ docs: { command: "docs-mcp", args: ["--literal"] } }],
    strict_mcp_config: true,
    system_prompt: "  First\nSecond  ",
    append_system_prompt: "",
  });
  for (const base of [
    { command: "create_session", cwd: "/work" },
    { command: "new_session", cwd: "/work" },
    { command: "resume_session", session_id: "saved" },
    { command: "resume_session_at", session_id: "saved", target_user_message_id: "turn-2" },
  ]) {
    const decoded = parseCommandEnvelope(
      JSON.stringify({
        ...base,
        launch_settings: {
          system_prompt: "",
          append_system_prompt: "  tail\n",
          settings: {},
          mcp_configs: [],
          additional_directories: [],
          strict_mcp_config: false,
          fork_session: false,
        },
      }),
    ).command;
    assert.equal(decoded.command, base.command);
    assert.ok("launch_settings" in decoded);
    assert.deepEqual(decoded.launch_settings, {
      system_prompt: "",
      append_system_prompt: "  tail\n",
      settings: {},
    });
    const absent = parseCommandEnvelope(JSON.stringify(base)).command;
    assert.ok("launch_settings" in absent);
    assert.deepEqual(absent.launch_settings, {});
  }
  const rows = [
    ["mcp_configs", "docs", [{ docs: { command: "mcp" } }]],
    ["mcp_configs", ["docs"], [{}]],
    ["settings", ["a"], { literal: ["a"] }],
    ["additional_directories", [1], ["/shared"]],
    ["strict_mcp_config", "yes", true],
    ["fork_session", "yes", true],
    ["system_prompt", 3, "prompt"],
    ["append_system_prompt", false, "tail"],
    ["name", 3, "name"],
  ] as const;
  for (const [field, invalid, valid] of rows) {
    assert.doesNotThrow(() => parseCommandEnvelope(launchCommand({ [field]: valid })));
    assert.throws(
      () => parseCommandEnvelope(launchCommand({ [field]: invalid })),
      new RegExp(field),
    );
  }
});
