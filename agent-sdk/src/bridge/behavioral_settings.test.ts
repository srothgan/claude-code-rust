import assert from "node:assert/strict";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { after, before, test } from "node:test";
import type { Json, SettingsScope, SettingsSnapshot } from "../types.js";
import { inspectSettings, mutateSetting } from "./settings_service.js";

let root: string;
let cwd: string;
const oldProfile = process.env.CLAUDE_CONFIG_DIR;
const oldPreviewMarker = process.env.CLAUDE_RS_SETTINGS_PREVIEW_MARKER;
before(async () => {
  root = await fs.mkdtemp(path.join(os.tmpdir(), "claude-rs-behavior-"));
  cwd = path.join(root, "project");
  process.env.CLAUDE_CONFIG_DIR = path.join(root, "profile");
  await fs.mkdir(path.join(cwd, ".claude"), { recursive: true });
  await fs.mkdir(process.env.CLAUDE_CONFIG_DIR);
  process.env.CLAUDE_RS_SETTINGS_PREVIEW_MARKER = path.join(root, "hook-executed");
});
after(async () => {
  if (oldProfile === undefined) delete process.env.CLAUDE_CONFIG_DIR;
  else process.env.CLAUDE_CONFIG_DIR = oldProfile;
  if (oldPreviewMarker === undefined) delete process.env.CLAUDE_RS_SETTINGS_PREVIEW_MARKER;
  else process.env.CLAUDE_RS_SETTINGS_PREVIEW_MARKER = oldPreviewMarker;
  await fs.rm(root, { recursive: true, force: true });
});
function change(snapshot: SettingsSnapshot, id: string, scope: SettingsScope, value?: Json) {
  const saved = snapshot.sources.find(source => source.scope === scope)?.values.find(value => value.id === id);
  assert.ok(saved);
  return { context: snapshot.context, id, scope, expected_revision: saved.revision, operation: value === undefined ? "remove" as const : "set" as const, ...(value === undefined ? {} : { value }) };
}

test("structured behavioral settings save, resolve and reset without losing siblings or running hooks", async () => {
  const localFile = path.join(cwd, ".claude", "settings.local.json");
  const untouched = { future: { value: [1, "keep"] }, sandbox: { futureOption: "keep" } };
  await fs.writeFile(localFile, JSON.stringify(untouched));
  const cases: Array<[string, Json, SettingsScope?]> = [
    ["permissions.allow", ["Bash(npm run test *)", "Read(./docs/**)"]],
    ["permissions.ask", ["Bash(git push *)"]],
    ["permissions.deny", ["Read(./.env)"]],
    ["permissions.additionalDirectories", ["../shared"]],
    ["worktree.symlinkDirectories", ["node_modules", ".cache"]],
    ["worktree.sparsePaths", ["src", "docs"]],
    ["autoMemoryDirectory", "~/custom-memory"],
    ["claudeMdExcludes", ["**/generated/CLAUDE.md"]],
    ["plansDirectory", ".plans"],
    ["hooks", { SessionStart: [{ hooks: [{ type: "command", command: "node -e \"require('node:fs').writeFileSync(process.env.CLAUDE_RS_SETTINGS_PREVIEW_MARKER,'executed')\"" }] }], PreToolUse: [{ matcher: "Bash", hooks: [{ type: "command", command: "unused-preview-command", args: ["preserve spaces"], timeout: 10 }] }], Stop: [{ hooks: [{ type: "http", url: "https://hooks.example.com/stop", headers: { Authorization: "Bearer $HOOK_TOKEN" }, allowedEnvVars: ["HOOK_TOKEN"] }] }] }],
    ["allowedHttpHookUrls", ["https://hooks.example.com/*"]],
    ["httpHookAllowedEnvVars", ["HOOK_TOKEN"]],
    ["sandbox.filesystem.allowRead", ["./docs"]],
    ["sandbox.filesystem.allowWrite", ["./build"]],
    ["sandbox.filesystem.denyRead", ["./.env"]],
    ["sandbox.filesystem.denyWrite", ["./.git"]],
    ["sandbox.network.allowedDomains", ["*.example.com"]],
    ["sandbox.network.deniedDomains", ["blocked.example.com"]],
    ["sandbox.network.allowUnixSockets", ["/tmp/example.sock"]],
    ["sandbox.network.allowMachLookup", ["com.apple.coresimulator.*"]],
    ["sandbox.network.httpProxyPort", 3128],
    ["sandbox.network.socksProxyPort", 1080],
    ["sandbox.network.tlsTerminate", { caCertPath: "~/ca.pem", caKeyPath: "~/ca.key" }, "user"],
    ["sandbox.credentials", { files: [{ path: "~/.secret", mode: "deny" }], envVars: [{ name: "PRIVATE_TOKEN", mode: "deny" }] }, "user"],
    ["sandbox.ignoreViolations", { "*": ["/tmp"] }],
    ["sandbox.excludedCommands", ["git status"]],
    ["sandbox.ripgrep", { command: "rg", args: ["--hidden"] }, "user"],
  ];
  for (const [id, value, scope = "local"] of cases) {
    const saved = await mutateSetting(cwd, change(await inspectSettings(cwd), id, scope, value));
    assert.equal(saved.persistence, "saved", `${id}: ${saved.error}`);
    assert.equal(saved.application, "next_session", id);
    assert.deepEqual(saved.snapshot?.values.find(entry => entry.id === id)?.value, value, id);
    const reset = await mutateSetting(cwd, change(await inspectSettings(cwd), id, scope));
    assert.equal(reset.persistence, "saved", id);
  }
  assert.deepEqual(JSON.parse(await fs.readFile(localFile, "utf8")), untouched);
  await assert.rejects(fs.access(path.join(root, "hook-executed")), { code: "ENOENT" });
});

test("empty lists are explicit values and reset restores merged permission rules", async () => {
  await fs.writeFile(path.join(process.env.CLAUDE_CONFIG_DIR as string, "settings.json"), JSON.stringify({ permissions: { deny: ["Read(./.env)"] } }));
  const snapshot = await inspectSettings(cwd);
  const empty = await mutateSetting(cwd, change(snapshot, "permissions.deny", "local", []));
  assert.equal(empty.persistence, "saved");
  assert.deepEqual(empty.snapshot?.values.find(value => value.id === "permissions.deny")?.value, ["Read(./.env)"]);
  const additional = await mutateSetting(cwd, change(await inspectSettings(cwd), "permissions.deny", "local", ["Bash(rm *)"]));
  assert.deepEqual(additional.snapshot?.values.find(value => value.id === "permissions.deny")?.value, ["Read(./.env)", "Bash(rm *)"]);
  const reset = await mutateSetting(cwd, change(await inspectSettings(cwd), "permissions.deny", "local"));
  assert.deepEqual(reset.snapshot?.values.find(value => value.id === "permissions.deny")?.value, ["Read(./.env)"]);
});

test("native schema rejects invalid selected structures before touching the saved file", async () => {
  const original = await fs.readFile(path.join(cwd, ".claude", "settings.local.json"), "utf8");
  for (const [id, value] of [
    ["permissions.allow", [true]],
    ["hooks", { NotAnEvent: [] }],
    ["hooks", { PreToolUse: [{ hooks: [{ type: "command" }] }] }],
    ["sandbox.credentials", { files: [{ path: "~/.secret", mode: "allow" }] }],
    ["sandbox.ignoreViolations", { "*": true }],
    ["sandbox.network.httpProxyPort", 0],
    ["sandbox.network.socksProxyPort", 65536],
    ["sandbox.network.httpProxyPort", 2.5],
    ["sandbox.network.tlsTerminate", { caCertPath: "~/ca.pem" }],
  ] as Array<[string, Json]>) {
    const scope = (await inspectSettings(cwd)).catalog.find(setting => setting.id === id)?.writable_scopes.includes("local") ? "local" : "user";
    const result = await mutateSetting(cwd, change(await inspectSettings(cwd), id, scope, value));
    assert.equal(result.persistence, "failure", `${id}: ${result.error}`);
  }
  assert.equal(await fs.readFile(path.join(cwd, ".claude", "settings.local.json"), "utf8"), original);
});

test("structured conflicts preserve externally added rules and unsupported objects", async () => {
  const file = path.join(cwd, ".claude", "settings.local.json");
  await fs.writeFile(file, JSON.stringify({ permissions: { allow: ["Read(./a)"] }, future: "preserve" }));
  const shown = await inspectSettings(cwd);
  await fs.writeFile(file, JSON.stringify({ permissions: { allow: ["Read(./b)"] }, future: "preserve" }));
  const result = await mutateSetting(cwd, change(shown, "permissions.allow", "local", ["Read(./c)"]));
  assert.equal(result.persistence, "conflict");
  assert.deepEqual(JSON.parse(await fs.readFile(file, "utf8")), { permissions: { allow: ["Read(./b)"] }, future: "preserve" });
});
