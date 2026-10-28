import assert from "node:assert/strict";
import fs from "node:fs/promises";
import { test } from "node:test";
import { HOOK_EVENTS } from "@anthropic-ai/claude-agent-sdk";
import { settingsCatalog } from "./settings_catalog.js";
import { SETTINGS_CATEGORIES } from "./settings_layout.js";
import { validateSettingValue } from "./settings_values.js";
import type { Json, SettingDescriptor, SettingsEditorSchema } from "../types.js";

function semanticSchema(schema: SettingsEditorSchema | undefined): unknown {
  if (!schema) return undefined;
  return {
    type: schema.type,
    options: schema.options,
    keys: schema.keys,
    item: semanticSchema(schema.item),
    fields: schema.fields
      ? Object.fromEntries(
          schema.fields.map((field) => [
            field.key,
            { required: field.required === true, schema: semanticSchema(field.schema) },
          ]),
        )
      : undefined,
    variants: schema.variants
      ? Object.fromEntries(
          Object.entries(schema.variants).map(([key, value]) => [key, semanticSchema(value)]),
        )
      : undefined,
  };
}

function editorField(
  schema: SettingsEditorSchema | undefined,
  key: string,
  type: SettingsEditorSchema["type"],
  required = false,
): SettingsEditorSchema {
  const field = schema?.fields?.find((field) => field.key === key);
  assert.ok(field, `missing editor field ${key}`);
  assert.equal(field.schema.type, type, key);
  assert.equal(field.required === true, required, `${key} requiredness`);
  return field.schema;
}

function descriptor(id: string): SettingDescriptor {
  const setting = settingsCatalog().find((setting) => setting.id === id);
  assert.ok(setting, `missing setting ${id}`);
  return setting;
}

function hookEditor(): SettingsEditorSchema {
  const groups = descriptor("hooks").editor?.item?.item;
  const hook = editorField(groups, "hooks", "array").item;
  assert.ok(hook?.variants);
  return hook;
}

test("settings fixture agrees on semantic forms and independently named pane associations", async () => {
  const fixture = JSON.parse(
    await fs.readFile(
      new URL("../../../tests/fixtures/settings-ui-catalog.json", import.meta.url),
      "utf8",
    ),
  ) as { categories: Array<{ id: string }>; catalog: SettingDescriptor[] };
  assert.deepEqual(
    fixture.categories.map((category) => category.id).sort(),
    SETTINGS_CATEGORIES.map((category) => category.id).sort(),
  );
  const catalog = settingsCatalog();
  for (const setting of catalog) {
    assert.equal(
      SETTINGS_CATEGORIES.filter((category) => category.id === setting.category).length,
      1,
      setting.id,
    );
    if (setting.kind === "json" || setting.kind === "string_list")
      assert.ok(setting.editor, setting.id);
  }
  const associations = {
    hooks: "hooks",
    "sandbox.credentials": "sandbox",
    "sandbox.ripgrep": "sandbox",
    idleCompaction: "memory",
    "permissions.allow": "permissions",
    model: "general",
  };
  for (const [id, category] of Object.entries(associations)) {
    const actual = descriptor(id);
    const saved = fixture.catalog.find((setting) => setting.id === id);
    assert.ok(saved, `fixture missing ${id}`);
    assert.equal(actual.category, category, id);
    const semantic = (setting: SettingDescriptor) => ({
      category: setting.category,
      kind: setting.kind,
      key_path: setting.key_path,
      writable_scopes: setting.writable_scopes,
      application: setting.application,
      options: setting.options,
      allows_custom: setting.allows_custom,
      editor: semanticSchema(setting.editor),
    });
    assert.deepEqual(semantic(actual), semantic(saved), id);
  }
  assert.deepEqual(descriptor("hooks").editor?.keys, [...HOOK_EVENTS]);
  assert.equal(descriptor("hooks").editor?.type, "map");
  assert.equal(descriptor("hooks").editor?.item?.type, "array");
  assert.equal(hookEditor().type, "variant");
  assert.equal(
    editorField(descriptor("sandbox.credentials").editor, "files", "array").item?.type,
    "object",
  );
  assert.equal(descriptor("idleCompaction").application, "next_session");
});

test("independently named hook forms expose optional failure policy only where supported", async () => {
  const setting = descriptor("hooks");
  const variants = hookEditor().variants;
  assert.ok(variants);
  for (const type of ["command", "http", "prompt", "agent", "mcp_tool"]) {
    const schema: SettingsEditorSchema | undefined = variants[type];
    assert.ok(schema, `missing ${type} hook form`);
    if (type !== "command" && type !== "http") {
      assert.equal(
        schema.fields?.find((field) => field.key === "onFailure"),
        undefined,
        type,
      );
      continue;
    }
    assert.deepEqual(editorField(schema, "onFailure", "string").options, ["continue", "block"]);
    const action: Record<string, Json> =
      type === "command"
        ? { type, command: "unused-command" }
        : { type, url: "https://example.com/hook" };
    for (const onFailure of [undefined, "continue", "block"])
      await validateSettingValue(setting, {
        PreToolUse: [{ hooks: [{ ...action, ...(onFailure === undefined ? {} : { onFailure }) }] }],
      });
    for (const onFailure of ["ignore", true, 2, null])
      await assert.rejects(
        validateSettingValue(setting, { PreToolUse: [{ hooks: [{ ...action, onFailure }] }] }),
        /cannot load/,
        type,
      );
  }
});

test("each pane presents its controls in alphabetical order", () => {
  const catalog = settingsCatalog();
  for (const pane of SETTINGS_CATEGORIES) {
    const labels = catalog.filter(setting => setting.category === pane.id).map(setting => setting.label);
    assert.deepEqual(labels, [...labels].sort((a, b) => a.localeCompare(b)), pane.label);
  }
});

test("guided hook and sandbox forms expose independently required and optional SDK inputs", async () => {
  const hooks = descriptor("hooks");
  const variants = hookEditor().variants;
  assert.ok(variants);
  const forms: Array<{
    type: string;
    required: string[];
    value: Record<string, Json>;
    optional: Record<string, SettingsEditorSchema["type"]>;
  }> = [
    {
      type: "command",
      required: ["command"],
      value: {
        command: "unused-command",
        args: ["--literal", "space here"],
        shell: "bash",
        async: true,
        asyncRewake: true,
        timeout: 10,
        once: true,
        statusMessage: "Check",
        if: "Bash(git *)",
        onFailure: "block",
      },
      optional: {
        args: "array",
        shell: "string",
        async: "boolean",
        asyncRewake: "boolean",
        timeout: "number",
        once: "boolean",
        statusMessage: "string",
        if: "string",
        onFailure: "string",
      },
    },
    {
      type: "prompt",
      required: ["prompt"],
      value: {
        prompt: "Check completion",
        model: "haiku",
        continueOnBlock: true,
        timeout: 10,
        once: true,
      },
      optional: { model: "string", continueOnBlock: "boolean", timeout: "number", once: "boolean" },
    },
    {
      type: "agent",
      required: ["prompt"],
      value: { prompt: "Verify", model: "haiku", timeout: 10 },
      optional: { model: "string", timeout: "number" },
    },
    {
      type: "http",
      required: ["url"],
      value: {
        url: "https://example.com/hook",
        headers: { Authorization: "Bearer $TOKEN" },
        allowedEnvVars: ["TOKEN"],
        timeout: 10,
      },
      optional: { headers: "map", allowedEnvVars: "array", timeout: "number", onFailure: "string" },
    },
    {
      type: "mcp_tool",
      required: ["server", "tool"],
      value: {
        server: "configured-server",
        tool: "inspect",
        input: { file: `\${tool_input.file_path}`, count: 2 },
        timeout: 10,
      },
      optional: { input: "map", timeout: "number" },
    },
  ];
  for (const { type, required, value, optional } of forms) {
    const schema: SettingsEditorSchema | undefined = variants[type];
    assert.ok(schema, `missing ${type}`);
    for (const key of required) editorField(schema, key, "string", true);
    for (const [key, fieldType] of Object.entries(optional)) editorField(schema, key, fieldType);
    const candidate: Record<string, Json> = {
      type,
      ...Object.fromEntries(required.map((key) => [key, value[key]])),
    };
    await validateSettingValue(hooks, { PreToolUse: [{ hooks: [candidate] }] });
    await validateSettingValue(hooks, {
      PreToolUse: [{ matcher: "Bash", hooks: [{ type, ...value }] }],
      Stop: [],
    });
    for (const key of required) {
      const incomplete: Record<string, Json> = { ...candidate };
      delete incomplete[key];
      await assert.rejects(
        validateSettingValue(hooks, { PreToolUse: [{ hooks: [incomplete] }] }),
        /cannot load/,
        `${type}.${key}`,
      );
    }
  }
  assert.equal(editorField(variants.command, "args", "array").item?.type, "string");
  assert.deepEqual(editorField(variants.command, "shell", "string").options, [
    "bash",
    "powershell",
  ]);
  assert.equal(editorField(variants.http, "headers", "map").item?.type, "string");
  assert.equal(editorField(variants.http, "allowedEnvVars", "array").item?.type, "string");
  assert.equal(editorField(variants.mcp_tool, "input", "map").item?.type, "json");
  const credentials = descriptor("sandbox.credentials");
  const collections = [
    {
      key: "files",
      required: ["path", "mode"],
      value: { path: "/unread/secret", mode: "mask" },
      optional: {
        extract: "string",
        onExtractNoMatch: "string",
        decode: "string",
        maskClaims: "array",
        maskDuplicates: "boolean",
        injectHosts: "array",
      },
    },
    {
      key: "envVars",
      required: ["name", "mode"],
      value: { name: "NOT_RESOLVED", mode: "deny" },
      optional: { extract: "string", onExtractNoMatch: "string", injectHosts: "array" },
    },
    {
      key: "awsPairs",
      required: ["accessKeyIdVar", "secretAccessKeyVar"],
      value: { accessKeyIdVar: "KEY", secretAccessKeyVar: "SECRET" },
      optional: { sessionTokenVar: "string" },
    },
  ] as const;
  for (const { key, required, value, optional } of collections) {
    const schema = editorField(credentials.editor, key, "array").item;
    assert.ok(schema);
    for (const field of required) editorField(schema, field, "string", true);
    for (const [field, type] of Object.entries(optional)) editorField(schema, field, type);
    if (key !== "awsPairs")
      assert.deepEqual(editorField(schema, "mode", "string", true).options, ["deny", "mask"]);
    await validateSettingValue(credentials, { [key]: [value] });
    for (const field of required) {
      const incomplete: Record<string, Json> = { ...value };
      delete incomplete[field];
      await assert.rejects(
        validateSettingValue(credentials, { [key]: [incomplete] }),
        /cannot load/,
        `${key}.${field}`,
      );
    }
  }
  const policies = editorField(credentials.editor, "sigv4", "object");
  for (const key of ["streaming", "presigned", "sigv4a"])
    assert.deepEqual(editorField(policies, key, "string").options, ["deny", "passthrough"]);
  editorField(credentials.editor, "allowPlaintextInject", "boolean");
  await validateSettingValue(credentials, {
    files: [
      {
        path: "/unread/secret",
        mode: "mask",
        extract: "token=(.+)",
        onExtractNoMatch: "deny",
        decode: "jwt",
        maskClaims: ["sub"],
        maskDuplicates: true,
        injectHosts: ["example.com"],
      },
    ],
    envVars: [{ name: "NOT_RESOLVED", mode: "deny" }],
    awsPairs: [{ accessKeyIdVar: "KEY", secretAccessKeyVar: "SECRET", sessionTokenVar: "SESSION" }],
    sigv4: { streaming: "deny", presigned: "passthrough", sigv4a: "deny" },
    allowPlaintextInject: false,
  });
  const tls = descriptor("sandbox.network.tlsTerminate");
  editorField(tls.editor, "caCertPath", "string");
  editorField(tls.editor, "caKeyPath", "string");
  await validateSettingValue(tls, { caCertPath: "/unread/cert", caKeyPath: "/unread/key" });
  await validateSettingValue(tls, {});
  for (const value of [{ caCertPath: "/unread/cert" }, { caKeyPath: "/unread/key" }] as Json[])
    await assert.rejects(validateSettingValue(tls, value), /both.*paths/);
  const ripgrep = descriptor("sandbox.ripgrep");
  editorField(ripgrep.editor, "command", "string", true);
  assert.equal(editorField(ripgrep.editor, "args", "array").item?.type, "string");
  await validateSettingValue(ripgrep, { command: "unused-rg", args: ["--literal"] });
  await assert.rejects(validateSettingValue(ripgrep, { args: ["--literal"] }), /cannot load/);
  const ignore = descriptor("sandbox.ignoreViolations");
  assert.equal(ignore.editor?.type, "map");
  assert.equal(ignore.editor.item?.type, "array");
  assert.equal(ignore.editor.item.item?.type, "string");
  await validateSettingValue(ignore, { "git *": ["/tmp"] });
  await assert.rejects(validateSettingValue(ignore, { "git *": "/tmp" }), /cannot load/);
});
