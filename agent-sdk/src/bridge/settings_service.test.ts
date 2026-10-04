import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { afterEach, beforeEach, test } from "node:test";
import { promisify } from "node:util";
import { inspectSettings, mutateSetting } from "./settings_service.js";
import type { AvailableModel, Json, SettingsMutation, SettingsScope, SettingsSnapshot } from "../types.js";

let root: string;
let cwd: string;
const oldProfile = process.env.CLAUDE_CONFIG_DIR;
beforeEach(async () => {
  root = await fs.mkdtemp(path.join(os.tmpdir(), "claude-rs-settings-"));
  cwd = path.join(root, "project");
  process.env.CLAUDE_CONFIG_DIR = path.join(root, "profile");
  await fs.mkdir(path.join(cwd, ".claude"), { recursive: true });
  await fs.mkdir(process.env.CLAUDE_CONFIG_DIR, { recursive: true });
});
afterEach(async () => {
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

test("automatic updates are personal app preferences with targeted saves, reset and conflicts", async () => {
  const appFile = path.join(root, "updater-settings.json");
  await write("user", { updates: { autoInstall: true }, future: "Claude data" });
  await fs.writeFile(appFile, JSON.stringify({ updates: { last_result: { latest_version: "9.0.0" }, future: 42 }, notifications: { actionsRequired: false } }));
  const inspect = () => inspectSettings(cwd, [], [], appFile);
  const initial = await inspect();
  const setting = initial.catalog.find(setting => setting.id === "updates.autoInstall");
  assert.ok(setting);
  assert.deepEqual(setting.options, [true, false]);
  assert.deepEqual(setting.writable_scopes, ["user"]);
  assert.equal(setting.application, "host");
  assert.equal(initial.values.find(value => value.id === setting.id)?.value, undefined);
  const changed = await mutateSetting(cwd, mutation(initial, setting.id, "user", true), [], [], appFile);
  assert.equal(changed.persistence, "saved");
  assert.equal(changed.snapshot?.values.find(value => value.id === setting.id)?.value, true);
  assert.equal((await mutateSetting(cwd, mutation(await inspect(), setting.id, "project", false), [], [], appFile)).persistence, "failure");
  const stale = mutation(await inspect(), setting.id, "user", false);
  const document = JSON.parse(await fs.readFile(appFile, "utf8"));
  document.updates.autoInstall = false;
  await fs.writeFile(appFile, JSON.stringify(document));
  assert.equal((await mutateSetting(cwd, stale, [], [], appFile)).persistence, "conflict");
  const reset = await mutateSetting(cwd, mutation(await inspect(), setting.id, "user"), [], [], appFile);
  assert.equal(reset.persistence, "saved");
  assert.equal(reset.snapshot?.values.find(value => value.id === setting.id)?.value, undefined);
  assert.deepEqual(JSON.parse(await fs.readFile(appFile, "utf8")), { updates: { last_result: { latest_version: "9.0.0" }, future: 42 }, notifications: { actionsRequired: false } });
});
function mutation(snapshot: SettingsSnapshot, id: string, scope: SettingsScope, value?: Json): SettingsMutation {
  const source = snapshot.sources.find(source => source.scope === scope);
  const previous = source?.values.find(value => value.id === id);
  assert.ok(previous);
  return { context: snapshot.context, id, scope, expected_revision: previous.revision, operation: value === undefined ? "remove" : "set", ...(value === undefined ? {} : { value }) };
}

test("presentation preferences share targeted saves while retaining their distinct storage and scopes", async () => {
  const appFile = path.join(root, "app-settings.json");
  await write("user", { presentation: { copyFullResponse: true }, showMessageTimestamps: false, future: 17 });
  await write("project", { showMessageTimestamps: true, autoScrollEnabled: false, timeFormat: "%Y-%m-%d %H:%M", timeZone: "Europe/Berlin" });
  await fs.writeFile(appFile, JSON.stringify({ updates: { skipped_version: "keep" }, presentation: { future: "preserve", copyFullResponse: false } }));
  const inspect = () => inspectSettings(cwd, [], [], appFile);
  const shown = await inspect();
  assert.equal(shown.values.find(value => value.id === "presentation.copyFullResponse")?.value, false);
  assert.equal(shown.values.find(value => value.id === "showMessageTimestamps")?.value, true);
  assert.equal(shown.values.find(value => value.id === "autoScrollEnabled")?.value, false);
  assert.equal(shown.values.find(value => value.id === "timeFormat")?.value, "%Y-%m-%d %H:%M");
  assert.equal(shown.time_zone, "Europe/Berlin");
  assert.equal(shown.provenance["presentation.copyFullResponse"]?.path, appFile);
  const change = mutation(shown, "presentation.copyFullResponse", "user", true);
  const result = await mutateSetting(cwd, change, [], [], appFile);
  assert.equal(result.persistence, "saved");
  assert.equal(result.application, "host");
  const saved = JSON.parse(await fs.readFile(appFile, "utf8"));
  assert.deepEqual(saved, { updates: { skipped_version: "keep" }, presentation: { future: "preserve", copyFullResponse: true } });
  const userSource = shown.sources.find(source => source.scope === "user");
  assert.ok(userSource);
  assert.equal(JSON.parse(await fs.readFile(userSource.path, "utf8")).future, 17);
  assert.equal((await mutateSetting(cwd, mutation(await inspect(), "presentation.copyFullResponse", "project", false), [], [], appFile)).persistence, "failure");
  const stale = mutation(await inspect(), "presentation.copyFullResponse", "user", false);
  saved.presentation.copyFullResponse = false;
  await fs.writeFile(appFile, JSON.stringify(saved));
  assert.equal((await mutateSetting(cwd, stale, [], [], appFile)).persistence, "conflict");
  const reset = await mutateSetting(cwd, mutation(await inspect(), "presentation.copyFullResponse", "user"), [], [], appFile);
  assert.equal(reset.persistence, "saved");
  assert.equal(reset.snapshot?.values.find(value => value.id === "presentation.copyFullResponse")?.value, undefined);
  assert.equal(JSON.parse(await fs.readFile(appFile, "utf8")).presentation.future, "preserve");
});

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

test("concurrent edits to the selected setting are preserved and reported as conflicts", async () => {
  await write("user", { language: "German", untouched: true });
  const shown = await inspectSettings(cwd);
  await write("user", { language: "Japanese", untouched: true });
  const result = await mutateSetting(cwd, mutation(shown, "language", "user", "French"));
  assert.equal(result.persistence, "conflict");
  assert.equal(result.snapshot?.values.find(value => value.id === "language")?.value, "Japanese");
  const user = shown.sources.find(source => source.scope === "user");
  assert.ok(user);
  assert.deepEqual(JSON.parse(await fs.readFile(user.path, "utf8")), { language: "Japanese", untouched: true });
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
  const original = await fs.readFile(source.path, "utf8");
  const context = await mutateSetting(cwd, { ...mutation(valid, "model", "user", "haiku"), context: "stale-context" });
  assert.equal(context.persistence, "failure");
  const invalid = await mutateSetting(cwd, mutation(valid, "worktree.baseRef", "user", "invalid"));
  assert.equal(invalid.persistence, "failure");
  const readOnly = await mutateSetting(cwd, mutation(valid, "askUserQuestionTimeout", "project", "60s"));
  assert.equal(readOnly.persistence, "failure");
  assert.equal(await fs.readFile(source.path, "utf8"), original);
  const project = valid.sources.find(source => source.scope === "project");
  assert.ok(project);
  await assert.rejects(fs.access(project.path), { code: "ENOENT" });
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
        const reopened = await inspectSettings(cwd);
        assert.equal(reopened.sources.find(source => source.scope === scope)?.values.find(value => value.id === setting.id)?.value, value, `${setting.id}/${scope}: persisted value`);
        assert.equal(reopened.values.find(entry => entry.id === setting.id)?.value, value, `${setting.id}/${scope}: SDK cascade`);
      }
      const reset = await mutateSetting(cwd, mutation(await inspectSettings(cwd), setting.id, scope));
      assert.ok(["saved", "unchanged"].includes(reset.persistence));
      const reopened = await inspectSettings(cwd);
      assert.equal(reopened.sources.find(source => source.scope === scope)?.values.find(value => value.id === setting.id)?.value, undefined, `${setting.id}/${scope}: reset removes the saved value`);
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
  const general = shown.catalog.filter(setting => setting.category === "general");
  assert.ok(general.some(setting => setting.id === "model"));
  assert.ok(general.some(setting => setting.id === "updates.autoInstall"));
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

test("hook and sandbox form saves validate supported edits and preserve extra object fields", async () => {
  const hooks: { PreToolUse: Array<{ matcher: string; futureGroup: Json; hooks: Array<Record<string, Json>> }> } = { PreToolUse: [{ matcher: "Write|Edit", futureGroup: { keep: true }, hooks: [{ type: "command", command: "never-execute", futureAction: "keep" }, { type: "http", url: "https://example.com/hook" }] }] };
  const ripgrep = { command: "rg", args: ["--hidden"], future: { keep: true } };
  await write("user", { hooks, sandbox: { ripgrep, enabled: true }, unrelated: "keep" });
  await write("project", {});
  await write("local", {});
  const editedHooks = structuredClone(hooks);
  editedHooks.PreToolUse[0].hooks[0].command = "never-execute-new";
  const first = await mutateSetting(cwd, mutation(await inspectSettings(cwd), "hooks", "user", editedHooks));
  assert.equal(first.persistence, "saved", first.error ?? "hooks save");
  assert.equal(first.application, "next_session", first.error ?? "hooks application");
  const editedRipgrep = { ...ripgrep, command: "custom-rg" };
  const second = await mutateSetting(cwd, mutation(await inspectSettings(cwd), "sandbox.ripgrep", "user", editedRipgrep));
  assert.equal(second.persistence, "saved", second.error ?? "ripgrep save");
  const source = second.snapshot?.sources.find(source => source.scope === "user");
  assert.ok(source);
  assert.deepEqual(JSON.parse(await fs.readFile(source.path, "utf8")), { hooks: editedHooks, sandbox: { ripgrep: editedRipgrep, enabled: true }, unrelated: "keep" });
  const invalid = await mutateSetting(cwd, mutation(await inspectSettings(cwd), "hooks", "user", { ...editedHooks, PreToolUse: [{ ...editedHooks.PreToolUse[0], hooks: [{ type: "command", command: 42, futureAction: "keep" }] }] }));
  assert.equal(invalid.persistence, "failure");
  const stored = JSON.parse(await fs.readFile(source.path, "utf8"));
  assert.deepEqual(stored.hooks, editedHooks);
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

const effortModels: AvailableModel[] = [
  { id: "default", display_name: "Default", resolved_model: "claude-opus-5-5", supports_effort: true, supported_effort_levels: ["low", "medium", "high", "xhigh", "max"] },
  { id: "opus", display_name: "Opus", resolved_model: "claude-opus-5-5", supports_effort: true, supported_effort_levels: ["low", "medium", "high", "xhigh", "max"] },
  { id: "sonnet", display_name: "Sonnet", resolved_model: "claude-sonnet-4-6-20260217[1m]", supports_effort: true, supported_effort_levels: ["low", "medium", "high"] },
  { id: "claude-opus-4-6", display_name: "Opus 4.6", supports_effort: true, supported_effort_levels: ["low", "medium", "high", "max"] },
  { id: "haiku", display_name: "Haiku", resolved_model: "claude-haiku-4-5-20251001", supports_effort: false, supported_effort_levels: [] },
];

test("default effort follows the saved model through scoped save, switch, reopen and reset", async () => {
  await write("user", { model: "opus", modelSettings: { "claude-opus-5-5": { effortLevel: "medium", autoCompactWindow: 100000 }, "claude-sonnet-4-6": { effortLevel: "low" } }, future: { keep: true } });
  await write("project", {});
  await write("local", {});
  let shown = await inspectSettings(cwd, effortModels);
  const effort = (snapshot: SettingsSnapshot) => snapshot.catalog.find(setting => setting.id === "defaultEffort");
  assert.deepEqual(effort(shown)?.options, ["low", "medium", "high", "xhigh"]);
  assert.deepEqual(effort(shown)?.key_path, ["modelSettings", "claude-opus-5-5", "effortLevel"]);
  assert.equal(shown.values.find(value => value.id === "defaultEffort")?.value, "medium");
  const saved = await mutateSetting(cwd, mutation(shown, "defaultEffort", "project", "high"), effortModels);
  assert.equal(saved.persistence, "saved", saved.error ?? "save");
  assert.equal(saved.snapshot?.values.find(value => value.id === "defaultEffort")?.value, "high");
  assert.ok(saved.snapshot);
  const opusDraft = mutation(saved.snapshot, "defaultEffort", "project", "low");
  const switched = await mutateSetting(cwd, mutation(saved.snapshot, "model", "user", "sonnet"), effortModels);
  assert.equal(switched.persistence, "saved", switched.error ?? "save");
  assert.ok(switched.snapshot);
  shown = await inspectSettings(cwd, effortModels);
  assert.deepEqual(effort(shown)?.key_path, ["modelSettings", "claude-sonnet-4-6", "effortLevel"]);
  assert.deepEqual(effort(shown)?.options, ["low", "medium", "high"]);
  assert.equal(shown.values.find(value => value.id === "defaultEffort")?.value, "low");
  const stale = await mutateSetting(cwd, opusDraft, effortModels);
  assert.equal(stale.persistence, "failure", "a model change must not retarget an open effort draft");
  const rejected = await mutateSetting(cwd, mutation(shown, "defaultEffort", "user", "max"), effortModels);
  assert.equal(rejected.persistence, "failure", "max stays session-only");
  const changed = await mutateSetting(cwd, mutation(shown, "defaultEffort", "user", "high"), effortModels);
  assert.equal(changed.persistence, "saved", changed.error ?? "save");
  const userPath = shown.sources.find(source => source.scope === "user")?.path;
  assert.ok(userPath);
  assert.deepEqual(JSON.parse(await fs.readFile(userPath, "utf8")), { model: "sonnet", modelSettings: { "claude-opus-5-5": { effortLevel: "medium", autoCompactWindow: 100000 }, "claude-sonnet-4-6": { effortLevel: "high" } }, future: { keep: true } });
  const reset = await mutateSetting(cwd, mutation(await inspectSettings(cwd, effortModels), "defaultEffort", "user"), effortModels);
  assert.equal(reset.persistence, "saved", reset.error ?? "save");
  assert.deepEqual(JSON.parse(await fs.readFile(userPath, "utf8")), { model: "sonnet", modelSettings: { "claude-opus-5-5": { effortLevel: "medium", autoCompactWindow: 100000 } }, future: { keep: true } });
  const back = await mutateSetting(cwd, mutation(await inspectSettings(cwd, effortModels), "model", "user", "opus"), effortModels);
  assert.equal(back.snapshot?.values.find(value => value.id === "defaultEffort")?.value, "high", "the other model's scoped preference survives");
  assert.ok(back.snapshot);
  const inherited = await mutateSetting(cwd, mutation(back.snapshot, "defaultEffort", "project"), effortModels);
  assert.equal(inherited.snapshot?.values.find(value => value.id === "defaultEffort")?.value, "medium");
});

test("default-model effort uses SDK resolution and capabilities, including unsupported models", async () => {
  await write("user", {}); await write("project", {}); await write("local", {});
  const inferred = await inspectSettings(cwd, effortModels);
  assert.deepEqual(inferred.catalog.find(setting => setting.id === "defaultEffort")?.key_path, ["modelSettings", "claude-opus-5-5", "effortLevel"]);
  await write("user", { model: "claude-opus-4-6" });
  const explicit = await inspectSettings(cwd, effortModels);
  assert.deepEqual(explicit.catalog.find(setting => setting.id === "defaultEffort")?.key_path, ["modelSettings", "claude-opus-4-6", "effortLevel"]);
  await write("user", {});
  const saved = await mutateSetting(cwd, mutation(inferred, "defaultEffort", "user", "xhigh"), effortModels);
  assert.equal(saved.persistence, "saved", saved.error ?? "save");
  const unsupported = await mutateSetting(cwd, mutation(await inspectSettings(cwd, effortModels), "model", "user", "haiku"), effortModels);
  assert.ok(unsupported.snapshot);
  assert.deepEqual(unsupported.snapshot.catalog.find(setting => setting.id === "defaultEffort")?.writable_scopes, []);
  assert.match(unsupported.snapshot.catalog.find(setting => setting.id === "defaultEffort")?.unavailable ?? "", /does not offer/);
});

test("default agent is chosen from SDK inventory and scoped reset restores the saved agent", async () => {
  const agents = [{ name: "reviewer", description: "Review code" }, { name: "builder", description: "Build code" }];
  await write("user", { agent: "reviewer", future: { keep: true } }); await write("project", {}); await write("local", {});
  const shown = await inspectSettings(cwd, effortModels, agents);
  assert.deepEqual(shown.catalog.find(setting => setting.id === "agent")?.options, ["reviewer", "builder"]);
  const invalid = await mutateSetting(cwd, mutation(shown, "agent", "project", "missing-agent"), effortModels, agents);
  assert.equal(invalid.persistence, "failure");
  const saved = await mutateSetting(cwd, mutation(shown, "agent", "project", "builder"), effortModels, agents);
  assert.equal(saved.persistence, "saved", saved.error ?? "save");
  assert.equal(saved.snapshot?.values.find(value => value.id === "agent")?.value, "builder");
  const reset = await mutateSetting(cwd, mutation(await inspectSettings(cwd, effortModels, agents), "agent", "project"), effortModels, agents);
  assert.equal(reset.persistence, "saved", reset.error ?? "save");
  assert.equal(reset.snapshot?.values.find(value => value.id === "agent")?.value, "reviewer");
});


test("notification controls save, reset and reject concurrent edits without changing mobile or unrelated preferences", async () => {
  const appFile = path.join(root, "notification-settings.json");
  await fs.writeFile(appFile, JSON.stringify({ notifications: { future: 7 }, presentation: { copyFullResponse: true }, updates: { skipped_version: "keep" } }));
  await write("user", { inputNeededNotifEnabled: true, agentPushNotifEnabled: true, future: 17 });
  const inspect = () => inspectSettings(cwd, [], [], appFile);
  for (const id of ["notifications.actionsRequired", "notifications.modelDirected", "notifications.turnComplete"]) {
    const shown = await inspect();
    const row = shown.catalog.find(row => row.id === id);
    assert.ok(row);
    assert.deepEqual(row.writable_scopes, ["user"]);
    assert.deepEqual(row.options, [true, false]);
    const saved = await mutateSetting(cwd, mutation(shown, id, "user", false), [], [], appFile);
    assert.equal(saved.persistence, "saved");
    assert.equal(saved.application, "host");
    assert.equal(saved.snapshot?.values.find(value => value.id === id)?.value, false);
    assert.equal((await mutateSetting(cwd, mutation(await inspect(), id, "local", true), [], [], appFile)).persistence, "failure");
    const stale = mutation(await inspect(), id, "user", true);
    const document = JSON.parse(await fs.readFile(appFile, "utf8"));
    document.notifications[id.split(".")[1]] = true;
    await fs.writeFile(appFile, JSON.stringify(document));
    assert.equal((await mutateSetting(cwd, stale, [], [], appFile)).persistence, "conflict");
    const reset = await mutateSetting(cwd, mutation(await inspect(), id, "user"), [], [], appFile);
    assert.equal(reset.persistence, "saved");
    assert.equal(reset.snapshot?.values.find(value => value.id === id)?.value, undefined);
  }
  for (const method of ["auto", "iterm2", "terminal_bell", "iterm2_with_bell", "kitty", "ghostty", "notifications_disabled"]) {
    const saved = await mutateSetting(cwd, mutation(await inspect(), "preferredNotifChannel", "user", method), [], [], appFile);
    assert.equal(saved.application, "host");
    assert.equal(saved.snapshot?.values.find(value => value.id === "preferredNotifChannel")?.value, method);
  }
  const app = JSON.parse(await fs.readFile(appFile, "utf8"));
  assert.deepEqual(app, { notifications: { future: 7 }, presentation: { copyFullResponse: true }, updates: { skipped_version: "keep" } });
  const user = (await inspect()).sources.find(source => source.scope === "user");
  assert.ok(user);
  const native = JSON.parse(await fs.readFile(user.path, "utf8"));
  assert.equal(native.inputNeededNotifEnabled, true);
  assert.equal(native.agentPushNotifEnabled, true);
  assert.equal(native.future, 17);
});
