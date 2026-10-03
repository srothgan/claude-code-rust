import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { after, before, test } from "node:test";
import { promisify } from "node:util";
import { inspectSettings, mutateSetting } from "./settings_service.js";
import type { Json, SettingsMutation, SettingsScope, SettingsSnapshot } from "../types.js";

let root: string;
let cwd: string;
const oldProfile = process.env.CLAUDE_CONFIG_DIR;
before(async () => {
  root = await fs.mkdtemp(path.join(os.tmpdir(), "claude-rs-settings-"));
  cwd = path.join(root, "project");
  process.env.CLAUDE_CONFIG_DIR = path.join(root, "profile");
  await fs.mkdir(path.join(cwd, ".claude"), { recursive: true });
  await fs.mkdir(process.env.CLAUDE_CONFIG_DIR, { recursive: true });
});
after(async () => {
  if (oldProfile === undefined) delete process.env.CLAUDE_CONFIG_DIR;
  else process.env.CLAUDE_CONFIG_DIR = oldProfile;
  await fs.rm(root, { recursive: true, force: true });
});
async function write(scope: SettingsScope, value: Json): Promise<void> {
  const snapshot = await inspectSettings(cwd);
  const source = snapshot.sources.find(source => source.scope === scope);
  assert.ok(source);
  await fs.writeFile(source.path, JSON.stringify(value));
}
function mutation(snapshot: SettingsSnapshot, id: string, scope: SettingsScope, value?: Json): SettingsMutation {
  const source = snapshot.sources.find(source => source.scope === scope);
  const previous = source?.values.find(value => value.id === id);
  assert.ok(previous);
  return { context: snapshot.context, id, scope, expected_revision: previous.revision, operation: value === undefined ? "remove" : "set", ...(value === undefined ? {} : { value }) };
}

test("SDK resolution exposes explicit false, scoped values, and the native cascade", async () => {
  await write("user", { model: "opus", fileCheckpointingEnabled: true, language: "German", env: { PRIVATE_TOKEN: "private-secret" } });
  await write("project", { model: "sonnet", fileCheckpointingEnabled: false });
  await write("local", { model: "haiku", outputStyle: "Custom style" });
  const snapshot = await inspectSettings(cwd);
  assert.equal(snapshot.values.find(value => value.id === "model")?.value, "haiku");
  assert.deepEqual(snapshot.values.find(value => value.id === "model")?.contributors, ["user", "project", "local"]);
  assert.equal(snapshot.values.find(value => value.id === "fileCheckpointingEnabled")?.value, false);
  assert.equal(snapshot.sources.find(source => source.scope === "user")?.values.find(value => value.id === "model")?.value, "opus");
  assert.equal(snapshot.values.find(value => value.id === "outputStyle")?.value, "Custom style");
  assert.ok(!JSON.stringify(snapshot).includes("private-secret"));
});

test("targeted edits preserve another owner's changes and reset exposes inheritance", async () => {
  await write("user", { model: "opus" });
  await write("project", { model: "sonnet", permissions: { defaultMode: "plan", deny: ["Bash(rm *)"] }, future: { opaque: [1, 2] } });
  await write("local", { model: "haiku" });
  const shown = await inspectSettings(cwd);
  const project = shown.sources.find(source => source.scope === "project");
  assert.ok(project);
  const external = JSON.parse(await fs.readFile(project.path, "utf8"));
  external.external = "keep me";
  await fs.writeFile(project.path, JSON.stringify(external));
  const saved = await mutateSetting(cwd, mutation(shown, "worktree.baseRef", "project", "head"));
  assert.equal(saved.persistence, "saved");
  assert.equal(saved.application, "next_session");
  const stored = JSON.parse(await fs.readFile(project.path, "utf8"));
  assert.equal(stored.external, "keep me");
  assert.deepEqual(stored.future, { opaque: [1, 2] });
  assert.deepEqual(stored.permissions.deny, ["Bash(rm *)"]);
  const reset = await mutateSetting(cwd, mutation(await inspectSettings(cwd), "model", "local"));
  assert.equal(reset.persistence, "saved");
  assert.equal(reset.snapshot?.values.find(value => value.id === "model")?.value, "sonnet");
});

test("same-value external edits conflict without overwriting the new value", async () => {
  await write("user", { language: "German", untouched: true });
  const shown = await inspectSettings(cwd);
  await write("user", { language: "Japanese", untouched: true });
  const result = await mutateSetting(cwd, mutation(shown, "language", "user", "French"));
  assert.equal(result.persistence, "conflict");
  assert.equal(result.snapshot?.values.find(value => value.id === "language")?.value, "Japanese");
});

test("invalid files, context changes, catalog scopes, and invalid options protect persisted data", async () => {
  const shown = await inspectSettings(cwd);
  const source = shown.sources.find(source => source.scope === "user");
  assert.ok(source);
  await fs.writeFile(source.path, "{broken");
  const failed = await mutateSetting(cwd, mutation(shown, "model", "user", "opus"));
  assert.equal(failed.persistence, "failure");
  assert.equal(failed.snapshot?.sources.find(source => source.scope === "user")?.status, "invalid");
  assert.equal(await fs.readFile(source.path, "utf8"), "{broken");
  await write("user", { model: "opus" });
  const valid = await inspectSettings(cwd);
  const context = await mutateSetting(cwd, { ...mutation(valid, "model", "user", "haiku"), context: "stale-context" });
  assert.equal(context.persistence, "failure");
  const invalid = await mutateSetting(cwd, mutation(valid, "worktree.baseRef", "user", "invalid"));
  assert.equal(invalid.persistence, "failure");
  const readOnly = await mutateSetting(cwd, mutation(valid, "askUserQuestionTimeout", "project", "60s"));
  assert.equal(readOnly.persistence, "failure");
});

test("catalog boolean and finite choices round-trip at their supported scopes", async () => {
  await write("user", {});
  await write("project", {});
  await write("local", {});
  const initial = await inspectSettings(cwd);
  for (const setting of initial.catalog) {
    for (const scope of setting.writable_scopes) {
      for (const value of setting.options) {
        const result = await mutateSetting(cwd, mutation(await inspectSettings(cwd), setting.id, scope, value));
        assert.ok(["saved", "unchanged"].includes(result.persistence), `${setting.id}/${scope}: ${result.error}`);
        assert.equal(result.snapshot?.sources.find(source => source.scope === scope)?.values.find(value => value.id === setting.id)?.value, value);
        assert.equal(result.snapshot?.values.find(entry => entry.id === setting.id)?.value, value, `${setting.id}/${scope}: SDK cascade`);
      }
      const reset = await mutateSetting(cwd, mutation(await inspectSettings(cwd), setting.id, scope));
      assert.ok(["saved", "unchanged"].includes(reset.persistence));
    }
  }
});

test("local saves protect private settings from Git and preserve existing excludes", async () => {
  const directory = path.join(root, "git-project");
  await fs.mkdir(directory);
  const git = (args: string[]) => promisify(execFile)("git", args, { cwd: directory, windowsHide: true });
  await git(["init"]);
  const exclude = path.join(directory, ".git", "info", "exclude");
  await fs.writeFile(exclude, "/private-existing.txt\n");
  const shown = await inspectSettings(directory);
  const saved = await mutateSetting(directory, mutation(shown, "language", "local", "Greek"));
  assert.equal(saved.persistence, "saved", saved.error ?? "local save");
  await git(["check-ignore", "--quiet", ".claude/settings.local.json"]);
  await git(["check-ignore", "--quiet", ".claude/settings.local.json.example.tmp"]);
  assert.ok((await fs.readFile(exclude, "utf8")).startsWith("/private-existing.txt\n"));
  // Already tracked local settings cannot be made private with an ignore rule.
  await git(["add", "--force", ".claude/settings.local.json"]);
  const blocked = await mutateSetting(directory, mutation(await inspectSettings(directory), "language", "local", "French"));
  assert.equal(blocked.persistence, "failure");
  assert.ok(blocked.error?.includes("tracked by Git"));
  assert.equal((await inspectSettings(directory)).values.find(value => value.id === "language")?.value, "Greek");
});

test("custom styles and short language values save exactly and reset to inherit", async () => {
  await write("user", {});
  await write("project", {});
  await write("local", {});
  for (const [id, value] of [["outputStyle", "My custom response style"], ["language", "x"]]) {
    const saved = await mutateSetting(cwd, mutation(await inspectSettings(cwd), id, "local", value));
    assert.equal(saved.persistence, "saved");
    assert.equal(saved.snapshot?.values.find(entry => entry.id === id)?.value, value);
    const reset = await mutateSetting(cwd, mutation(await inspectSettings(cwd), id, "local"));
    assert.equal(reset.persistence, "saved");
    assert.equal(reset.snapshot?.values.find(entry => entry.id === id)?.value, undefined);
  }
});

test("alphabetical settings expose SDK model choices for validated save and reset", async () => {
  await write("user", {});
  await write("project", {});
  await write("local", {});
  const models = ["opus", "sonnet"].map(id => ({ id, display_name: id, supports_effort: true, supported_effort_levels: [] }));
  let shown = await inspectSettings(cwd, models);
  assert.deepEqual(shown.catalog.slice(0, 4).map(setting => setting.label), ["Auto compact", "Auto mode during planning", "Continue at usage limit", "Default model"]);
  assert.deepEqual(shown.catalog.find(setting => setting.id === "model")?.options, ["opus", "sonnet"]);
  for (const id of ["opus", "sonnet"]) {
    const saved = await mutateSetting(cwd, mutation(shown, "model", "user", id), models);
    assert.equal(saved.persistence, "saved", saved.error ?? "model save");
    assert.equal(saved.snapshot?.values.find(value => value.id === "model")?.value, id);
    assert.ok(saved.snapshot);
    shown = saved.snapshot;
  }
  const rejected = await mutateSetting(cwd, mutation(shown, "model", "user", "custom-model-id"), models);
  assert.equal(rejected.persistence, "failure");
  assert.equal(rejected.snapshot?.values.find(value => value.id === "model")?.value, "sonnet");
  const missing = await mutateSetting(cwd, mutation(shown, "model", "user", "opus"));
  assert.equal(missing.persistence, "failure", "an empty SDK inventory cannot accept new model values");
  const reset = await mutateSetting(cwd, mutation(shown, "model", "user"), models);
  assert.equal(reset.persistence, "saved");
  assert.equal(reset.snapshot?.sources.find(source => source.scope === "user")?.values.find(value => value.id === "model")?.value, undefined);
});

test("schema-invalid files distinguish saving from application and can be corrected in the editor", async () => {
  await write("user", { language: "German", alwaysThinkingEnabled: "invalid" });
  await write("project", {});
  await write("local", {});
  const shown = await inspectSettings(cwd);
  assert.equal(shown.sources.find(source => source.scope === "user")?.status, "not loaded");
  const saved = await mutateSetting(cwd, mutation(shown, "language", "user", "Greek"));
  assert.equal(saved.persistence, "saved");
  assert.equal(saved.application, "blocked");
  assert.ok(saved.error?.includes("was saved"));
  const userPath = shown.sources.find(source => source.scope === "user")?.path;
  assert.ok(userPath);
  assert.deepEqual(JSON.parse(await fs.readFile(userPath, "utf8")), { language: "Greek", alwaysThinkingEnabled: "invalid" });
  const fixed = await mutateSetting(cwd, mutation(await inspectSettings(cwd), "alwaysThinkingEnabled", "user", true));
  assert.equal(fixed.persistence, "saved");
  assert.equal(fixed.application, "next_session");
  assert.equal(fixed.snapshot?.sources.find(source => source.scope === "user")?.status, "valid");
  assert.equal(fixed.snapshot?.values.find(value => value.id === "language")?.value, "Greek");
  assert.equal(fixed.snapshot?.values.find(value => value.id === "alwaysThinkingEnabled")?.value, true);
});

test("unknown fields alone remain editable and survive save, resolution and reset", async () => {
  const opaque = { futureFeature: { choices: [null, false, { newOption: "custom" }] }, newSetting: 42 };
  await write("user", opaque);
  await write("project", {});
  await write("local", {});
  const shown = await inspectSettings(cwd);
  const source = shown.sources.find(source => source.scope === "user");
  assert.ok(source);
  assert.equal(source.status, "valid");
  const saved = await mutateSetting(cwd, mutation(shown, "language", "user", "Greek"));
  assert.equal(saved.persistence, "saved");
  assert.equal(saved.application, "next_session");
  assert.equal(saved.snapshot?.values.find(value => value.id === "language")?.value, "Greek");
  assert.deepEqual(JSON.parse(await fs.readFile(source.path, "utf8")), { ...opaque, language: "Greek" });
  const reset = await mutateSetting(cwd, mutation(await inspectSettings(cwd), "language", "user"));
  assert.equal(reset.persistence, "saved");
  assert.deepEqual(JSON.parse(await fs.readFile(source.path, "utf8")), opaque);
  assert.equal(reset.snapshot?.sources.find(source => source.scope === "user")?.status, "valid");
});

test("tolerated invalid unrelated entries do not block saved values from taking effect", async () => {
  const opaque = { hooks: "invalid", futureFeature: { keep: true } };
  await write("user", { ...opaque, language: "German" });
  await write("project", {});
  await write("local", {});
  const shown = await inspectSettings(cwd);
  const source = shown.sources.find(source => source.scope === "user");
  assert.ok(source);
  assert.equal(source.status, "valid");
  const saved = await mutateSetting(cwd, mutation(shown, "language", "user", "Greek"));
  assert.equal(saved.persistence, "saved");
  assert.equal(saved.application, "next_session");
  assert.equal(saved.snapshot?.values.find(value => value.id === "language")?.value, "Greek");
  assert.deepEqual(JSON.parse(await fs.readFile(source.path, "utf8")), { ...opaque, language: "Greek" });
});

test("unrelated rejected settings do not prevent targeted saves or reset", async () => {
  const opaque = { sandbox: { enabled: "invalid" }, futureFeature: { keep: true } };
  await write("user", { ...opaque, language: "German" });
  await write("project", {});
  await write("local", {});
  const shown = await inspectSettings(cwd);
  const source = shown.sources.find(source => source.scope === "user");
  assert.ok(source);
  assert.equal(source.status, "not loaded");
  const saved = await mutateSetting(cwd, mutation(shown, "language", "user", "Greek"));
  assert.equal(saved.persistence, "saved");
  assert.equal(saved.application, "blocked");
  assert.deepEqual(JSON.parse(await fs.readFile(source.path, "utf8")), { ...opaque, language: "Greek" });
  const reset = await mutateSetting(cwd, mutation(await inspectSettings(cwd), "language", "user"));
  assert.equal(reset.persistence, "saved");
  assert.equal(reset.application, "next_session");
  assert.deepEqual(JSON.parse(await fs.readFile(source.path, "utf8")), opaque);
});

test("files containing only ignored entries can acquire and reset supported settings", async () => {
  await write("user", { hooks: "invalid" });
  await write("project", {});
  await write("local", {});
  const shown = await inspectSettings(cwd);
  const saved = await mutateSetting(cwd, mutation(shown, "language", "user", "Greek"));
  assert.equal(saved.persistence, "saved");
  assert.equal(saved.application, "next_session");
  assert.equal(saved.snapshot?.values.find(value => value.id === "language")?.value, "Greek");
  const reset = await mutateSetting(cwd, mutation(await inspectSettings(cwd), "language", "user"));
  assert.equal(reset.persistence, "saved");
  assert.equal(reset.application, "next_session");
  const source = reset.snapshot?.sources.find(source => source.scope === "user");
  assert.ok(source);
  assert.deepEqual(JSON.parse(await fs.readFile(source.path, "utf8")), { hooks: "invalid" });
});
