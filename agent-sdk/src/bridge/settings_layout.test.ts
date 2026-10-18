import assert from "node:assert/strict";
import fs from "node:fs/promises";
import { test } from "node:test";
import { HOOK_EVENTS } from "@anthropic-ai/claude-agent-sdk";
import { settingsCatalog } from "./settings_catalog.js";
import { SETTINGS_CATEGORIES } from "./settings_layout.js";
import { validateSettingValue } from "./settings_values.js";
import type { Json } from "../types.js";

test("every control has exactly one pane and structured metadata survives the cross-language fixture", async () => {
  const fixture = JSON.parse(await fs.readFile(new URL("../../../tests/fixtures/settings-ui-catalog.json", import.meta.url), "utf8"));
  const catalog = settingsCatalog();
  assert.deepEqual(fixture.categories, SETTINGS_CATEGORIES);
  for (const setting of catalog) {
    assert.equal(SETTINGS_CATEGORIES.filter(category => category.id === setting.category).length, 1, setting.id);
    if (setting.kind === "json" || setting.kind === "string_list") assert.ok(setting.editor, setting.id);
  }
  for (const setting of fixture.catalog) assert.deepEqual(setting, catalog.find(candidate => candidate.id === setting.id));
  const hooks = catalog.find(setting => setting.id === "hooks")?.editor;
  assert.deepEqual(hooks?.keys, [...HOOK_EVENTS]);
});

test("all offered hook action forms validate through the pinned SDK without execution", async () => {
  const setting = settingsCatalog().find(setting => setting.id === "hooks");
  assert.ok(setting);
  const handlers: Json[] = [
    { type: "command", command: "never-execute-this", args: ["--literal"], shell: "bash", async: true, asyncRewake: true, timeout: 10, once: true, statusMessage: "Check", if: "Bash(git *)" },
    { type: "prompt", prompt: "Check completion", model: "haiku", continueOnBlock: true, timeout: 10, once: true },
    { type: "agent", prompt: "Verify", model: "haiku", timeout: 10 },
    { type: "http", url: "https://example.com", headers: { Authorization: "Bearer $TOKEN" }, allowedEnvVars: ["TOKEN"], timeout: 10 },
    { type: "mcp_tool", server: "configured-server", tool: "inspect", input: { file: `\${tool_input.file_path}` }, timeout: 10 },
  ];
  for (const handler of handlers) await validateSettingValue(setting, { PreToolUse: [{ matcher: "", hooks: [handler] }], Stop: [] });
});

test("failure policy is editable only on command and HTTP hooks and validated by the SDK", async () => {
  const setting = settingsCatalog().find(setting => setting.id === "hooks");
  const variants = setting?.editor?.item?.item?.fields?.find(field => field.key === "hooks")?.schema.item?.variants;
  assert.ok(setting && variants);
  for (const [type, schema] of Object.entries(variants)) {
    const policy = schema.fields?.find(field => field.key === "onFailure");
    if (type !== "command" && type !== "http") {
      assert.equal(policy, undefined, type);
      continue;
    }
    assert.deepEqual(policy?.schema.options, ["continue", "block"]);
    assert.equal(policy?.required, undefined);
    const action: Record<string, Json> = type === "command"
      ? { type, command: "never-execute" } : { type, url: "https://example.com/hook" };
    for (const onFailure of [undefined, "continue", "block"]) {
      await validateSettingValue(setting, { PreToolUse: [{ hooks: [{ ...action, ...(onFailure ? { onFailure } : {}) }] }] });
    }
    for (const onFailure of ["ignore", true, 2, null]) {
      await assert.rejects(validateSettingValue(setting, { PreToolUse: [{ hooks: [{ ...action, onFailure }] }] }), /cannot load/, type);
    }
  }
});

test("idle compaction is an optional scoped preference in Memory with next-session application", () => {
  const setting = settingsCatalog().find(setting => setting.id === "idleCompaction");
  assert.ok(setting);
  assert.equal(setting.category, "memory");
  assert.equal(setting.kind, "boolean");
  assert.deepEqual(setting.options, [true, false]);
  assert.deepEqual(setting.writable_scopes, ["user", "project", "local"]);
  assert.equal(setting.application, "next_session");
});

test("sandbox forms expose the supported native fields and their candidate values validate", async () => {
  const values: Array<[string, Json]> = [
    ["sandbox.ripgrep", { command: "never-execute-this", args: ["--literal"] }],
    ["sandbox.network.tlsTerminate", { caCertPath: "/unread/cert", caKeyPath: "/unread/key" }],
    ["sandbox.ignoreViolations", { "git *": ["/tmp"] }],
    ["sandbox.credentials", { files: [{ path: "/unread/secret", mode: "mask", extract: "token=(.+)", onExtractNoMatch: "deny", injectHosts: ["example.com"] }], envVars: [{ name: "NOT_RESOLVED", mode: "deny" }], awsPairs: [{ accessKeyIdVar: "KEY", secretAccessKeyVar: "SECRET" }], sigv4: { streaming: "deny", presigned: "passthrough", sigv4a: "deny" }, allowPlaintextInject: false }],
  ];
  for (const [id, value] of values) {
    const setting = settingsCatalog().find(setting => setting.id === id);
    assert.ok(setting?.editor);
    await validateSettingValue(setting, value);
  }
});

test("each pane presents its controls in alphabetical order", () => {
  const catalog = settingsCatalog();
  for (const pane of SETTINGS_CATEGORIES) {
    const labels = catalog.filter(setting => setting.category === pane.id).map(setting => setting.label);
    assert.deepEqual(labels, [...labels].sort((a, b) => a.localeCompare(b)), pane.label);
  }
});

test("guided creation metadata supplies exactly the SDK-required hook and credential fields", async () => {
  const catalog = settingsCatalog();
  const hookSetting = catalog.find(setting => setting.id === "hooks");
  const hook = hookSetting?.editor?.item?.item?.fields?.find(field => field.key === "hooks")?.schema.item;
  assert.ok(hookSetting && hook?.variants);
  const values: Record<string, string> = { command: "never-execute", prompt: "Check completion", url: "https://example.com/hook", server: "configured-server", tool: "inspect" };
  for (const [type, schema] of Object.entries(hook.variants)) {
    const required = schema.fields?.filter(field => field.required) ?? [];
    const candidate: Record<string, Json> = { type };
    for (const field of required) candidate[field.key] = values[field.key];
    await validateSettingValue(hookSetting, { PreToolUse: [{ hooks: [candidate] }] });
    for (const field of required) {
      const incomplete = { ...candidate };
      delete incomplete[field.key];
      await assert.rejects(validateSettingValue(hookSetting, { PreToolUse: [{ hooks: [incomplete] }] }), /cannot load/, `${type}.${field.key}`);
    }
  }
  const credentials = catalog.find(setting => setting.id === "sandbox.credentials");
  assert.ok(credentials?.editor?.fields);
  for (const collection of credentials.editor.fields.filter(field => field.schema.type === "array")) {
    const required = collection.schema.item?.fields?.filter(field => field.required) ?? [];
    const candidate: Record<string, Json> = {};
    for (const field of required) candidate[field.key] = field.schema.options?.[0] ?? (field.key === "path" ? "/unread/secret" : field.key.toUpperCase());
    await validateSettingValue(credentials, { [collection.key]: [candidate] });
    for (const field of required) {
      const incomplete = { ...candidate };
      delete incomplete[field.key];
      await assert.rejects(validateSettingValue(credentials, { [collection.key]: [incomplete] }), /cannot load/, `${collection.key}.${field.key}`);
    }
  }
});
