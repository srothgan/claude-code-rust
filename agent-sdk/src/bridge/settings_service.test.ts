import assert from "node:assert/strict";
import { execFile, spawnSync } from "node:child_process";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { afterEach, beforeEach, test } from "node:test";
import { promisify } from "node:util";
import { inspectSettings, mutateSetting } from "./settings_service.js";
import type {
  AvailableModel,
  Json,
  SettingsMutation,
  SettingsScope,
  SettingsSnapshot,
} from "../types.js";

let root: string;
let cwd: string;
const oldProfile = process.env.CLAUDE_CONFIG_DIR;
beforeEach(async () => {
  root = await fs.realpath(await fs.mkdtemp(path.join(os.tmpdir(), "claude-rs-settings-")));
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
function nativePath(scope: SettingsScope, project = cwd): string {
  return scope === "user"
    ? path.join(root, "profile", "settings.json")
    : path.join(project, ".claude", scope === "local" ? "settings.local.json" : "settings.json");
}
async function write(scope: SettingsScope, value: Json): Promise<void> {
  const file = nativePath(scope);
  await fs.mkdir(path.dirname(file), { recursive: true });
  await fs.writeFile(file, JSON.stringify(value));
}
async function document(file: string): Promise<Record<string, Json>> {
  return JSON.parse(await fs.readFile(file, "utf8"));
}
async function artifacts(file: string): Promise<string[]> {
  return (await fs.readdir(path.dirname(file))).filter((name) =>
    name.startsWith(`${path.basename(file)}.`),
  );
}

test("personal updater, presentation and notification edits change only the addressed app leaf", async () => {
  const appFile = path.join(root, "personal preferences.json");
  const native = {
    updates: { autoInstall: true },
    presentation: { copyFullResponse: true },
    notifications: { actionsRequired: true, turnComplete: false, modelDirected: true },
    showMessageTimestamps: false,
    inputNeededNotifEnabled: true,
    agentPushNotifEnabled: true,
    future: { keep: 17 },
  };
  const project = {
    showMessageTimestamps: true,
    autoScrollEnabled: false,
    timeFormat: "%Y-%m-%d %H:%M",
    timeZone: "Europe/Berlin",
    native: "project",
  };
  await write("user", native);
  await write("project", project);
  const nativeBytes = await fs.readFile(nativePath("user"), "utf8"),
    projectBytes = await fs.readFile(nativePath("project"), "utf8");
  const base = {
    updates: {
      last_result: { latest_version: "9.0.0" },
      skipped_version: "8.0.0",
      future: 42,
      autoInstall: false,
    },
    presentation: { future: "preserve", copyFullResponse: false },
    notifications: { future: 7, actionsRequired: false, turnComplete: true, modelDirected: false },
  };
  const rows = [
    {
      id: "updates.autoInstall",
      namespace: "updates",
      key: "autoInstall",
      saved: false,
      value: true,
    },
    {
      id: "presentation.copyFullResponse",
      namespace: "presentation",
      key: "copyFullResponse",
      saved: false,
      value: true,
    },
    {
      id: "notifications.actionsRequired",
      namespace: "notifications",
      key: "actionsRequired",
      saved: false,
      value: true,
    },
    {
      id: "notifications.modelDirected",
      namespace: "notifications",
      key: "modelDirected",
      saved: false,
      value: true,
    },
    {
      id: "notifications.turnComplete",
      namespace: "notifications",
      key: "turnComplete",
      saved: true,
      value: false,
    },
  ];
  const inspect = () => inspectSettings(cwd, [], [], appFile);
  const unchangedNative = async () => {
    assert.equal(await fs.readFile(nativePath("user"), "utf8"), nativeBytes);
    assert.equal(await fs.readFile(nativePath("project"), "utf8"), projectBytes);
    await assert.rejects(fs.access(nativePath("local")), { code: "ENOENT" });
  };
  for (const { id, namespace, key, saved, value } of rows) {
    await fs.writeFile(appFile, JSON.stringify(base));
    const shown = await inspect();
    assert.ok(shown.catalog.find((row) => row.id === id));
    assert.equal(shown.values.find((row) => row.id === id)?.value, saved);
    assert.equal(shown.provenance[id]?.path, appFile);
    assert.equal(shown.values.find((row) => row.id === "showMessageTimestamps")?.value, true);
    assert.equal(shown.values.find((row) => row.id === "autoScrollEnabled")?.value, false);
    assert.equal(shown.values.find((row) => row.id === "timeFormat")?.value, "%Y-%m-%d %H:%M");
    assert.equal(shown.time_zone, "Europe/Berlin");
    const expected: Record<string, Record<string, Json>> = structuredClone(base);
    expected[namespace][key] = value;
    const result = await mutateSetting(cwd, mutation(shown, id, "user", value), [], [], appFile);
    assert.equal(result.persistence, "saved");
    assert.equal(result.application, "host");
    assert.deepEqual(await document(appFile), expected);
    await unchangedNative();
    for (const scope of ["project", "local"] as const) {
      const before = await fs.readFile(appFile, "utf8");
      assert.equal(
        (await mutateSetting(cwd, mutation(await inspect(), id, scope, saved), [], [], appFile))
          .persistence,
        "failure",
      );
      assert.equal(await fs.readFile(appFile, "utf8"), before);
      await unchangedNative();
    }
    const stale = mutation(await inspect(), id, "user", saved);
    expected[namespace][key] = saved;
    await fs.writeFile(appFile, JSON.stringify(expected));
    const externalBytes = await fs.readFile(appFile, "utf8");
    assert.equal((await mutateSetting(cwd, stale, [], [], appFile)).persistence, "conflict");
    assert.equal(await fs.readFile(appFile, "utf8"), externalBytes);
    await unchangedNative();
    const reset = await mutateSetting(cwd, mutation(await inspect(), id, "user"), [], [], appFile);
    delete expected[namespace][key];
    assert.equal(reset.persistence, "saved");
    assert.equal(reset.snapshot?.values.find((row) => row.id === id)?.value, undefined);
    assert.deepEqual(await document(appFile), expected);
    await unchangedNative();
  }
  const appBefore = await fs.readFile(appFile, "utf8");
  for (const channel of [
    "auto",
    "iterm2",
    "terminal_bell",
    "iterm2_with_bell",
    "kitty",
    "ghostty",
    "notifications_disabled",
  ]) {
    const result = await mutateSetting(
      cwd,
      mutation(await inspect(), "preferredNotifChannel", "user", channel),
      [],
      [],
      appFile,
    );
    assert.equal(result.persistence, "saved");
    assert.equal(result.application, "host");
    assert.deepEqual(await document(nativePath("user")), {
      ...native,
      preferredNotifChannel: channel,
    });
    assert.equal(
      result.snapshot?.values.find((row) => row.id === "preferredNotifChannel")?.value,
      channel,
    );
    assert.equal(await fs.readFile(appFile, "utf8"), appBefore);
    assert.equal(await fs.readFile(nativePath("project"), "utf8"), projectBytes);
  }
  await fs.rm(appFile);
  assert.equal(
    (
      await mutateSetting(
        cwd,
        mutation(await inspect(), "updates.autoInstall", "user", true),
        [],
        [],
        appFile,
      )
    ).persistence,
    "saved",
  );
  assert.deepEqual(await document(appFile), { updates: { autoInstall: true } });
  const nativeAfter = await fs.readFile(nativePath("user"), "utf8");
  for (const invalid of ["{broken", "[]"]) {
    await fs.writeFile(appFile, invalid);
    const refused = await mutateSetting(
      cwd,
      mutation(await inspect(), "updates.autoInstall", "user", false),
      [],
      [],
      appFile,
    );
    assert.equal(refused.persistence, "failure");
    assert.equal(await fs.readFile(appFile, "utf8"), invalid);
    assert.equal(await fs.readFile(nativePath("user"), "utf8"), nativeAfter);
  }
  await fs.rm(appFile);
  await fs.mkdir(appFile);
  assert.equal(
    (
      await mutateSetting(
        cwd,
        mutation(await inspect(), "updates.autoInstall", "user", true),
        [],
        [],
        appFile,
      )
    ).persistence,
    "failure",
  );
  assert.equal((await fs.stat(appFile)).isDirectory(), true);
  assert.equal(await fs.readFile(nativePath("user"), "utf8"), nativeAfter);
});

function mutation(
  snapshot: SettingsSnapshot,
  id: string,
  scope: SettingsScope,
  value?: Json,
): SettingsMutation {
  const source = snapshot.sources.find((source) => source.scope === scope);
  const previous = source?.values.find((value) => value.id === id);
  assert.ok(previous);
  return {
    context: snapshot.context,
    id,
    scope,
    expected_revision: previous.revision,
    operation: value === undefined ? "remove" : "set",
    ...(value === undefined ? {} : { value }),
  };
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

test("targeted settings patches retry observed external edits and reset real inherited values", async (t) => {
  const project = nativePath("project"),
    user = nativePath("user"),
    local = nativePath("local");
  await write("user", { model: "opus", future: "user" });
  await write("local", { model: "haiku", future: "local" });
  const userBefore = await fs.readFile(user, "utf8"),
    localBefore = await fs.readFile(local, "utf8");
  for (const scenario of ["unrelated", "selected", "created", "repeated"]) {
    const original = {
      model: "sonnet",
      future: { opaque: [1, 2] },
      permissions: { deny: ["Bash(rm *)"] },
    };
    if (scenario === "created") await fs.rm(project, { force: true });
    else await write("project", original);
    const shown = await inspectSettings(cwd);
    const open = fs.open.bind(fs);
    let stages = 0;
    let external: Record<string, Json> = {};
    t.mock.method(fs, "open", async (...args: Parameters<typeof fs.open>) => {
      const handle = await open(...args);
      if (String(args[0]).startsWith(`${project}.`) && String(args[0]).endsWith(".tmp")) {
        const sync = handle.sync.bind(handle);
        t.mock.method(handle, "sync", async () => {
          await sync();
          stages++;
          if (stages === 1 || scenario === "repeated") {
            external = {
              ...(scenario === "created" ? { future: "created by another editor" } : original),
              external: `editor-${stages}`,
              ...(scenario === "selected" ? { worktree: { baseRef: "fresh" } } : {}),
            };
            await fs.writeFile(project, JSON.stringify(external));
          }
        });
      }
      return handle;
    });
    let result: Awaited<ReturnType<typeof mutateSetting>>;
    try {
      result = await mutateSetting(cwd, mutation(shown, "worktree.baseRef", "project", "head"));
    } finally {
      t.mock.restoreAll();
    }
    assert.ok(stages > 0, `${scenario} changed the actual staging/comparison boundary`);
    if (scenario === "selected" || scenario === "repeated") {
      assert.equal(result.persistence, "conflict");
      assert.deepEqual(await document(project), external);
    } else {
      assert.equal(result.persistence, "saved", result.error ?? scenario);
      assert.deepEqual(await document(project), { ...external, worktree: { baseRef: "head" } });
    }
    assert.equal(await fs.readFile(user, "utf8"), userBefore);
    assert.equal(await fs.readFile(local, "utf8"), localBefore);
    assert.deepEqual(await artifacts(project), []);
  }
  await write("project", { model: "sonnet", external: "keep" });
  const reset = await mutateSetting(cwd, mutation(await inspectSettings(cwd), "model", "local"));
  assert.equal(reset.persistence, "saved");
  assert.equal(reset.snapshot?.values.find((value) => value.id === "model")?.value, "sonnet");
  assert.deepEqual(await document(local), { future: "local" });
  assert.deepEqual(await document(project), { model: "sonnet", external: "keep" });
  assert.equal(await fs.readFile(user, "utf8"), userBefore);
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

test("real context changes and independent invalid inputs refuse writes and recover after repair", async () => {
  const user = nativePath("user"),
    project = nativePath("project"),
    local = nativePath("local");
  for (const bytes of ["{broken", "[]"]) {
    await fs.writeFile(user, bytes);
    const result = await mutateSetting(
      cwd,
      mutation(await inspectSettings(cwd), "language", "user", "French"),
    );
    assert.equal(result.persistence, "failure");
    assert.equal(result.application, "blocked");
    assert.ok(result.error);
    assert.equal(await fs.readFile(user, "utf8"), bytes);
    await assert.rejects(fs.access(project), { code: "ENOENT" });
    await assert.rejects(fs.access(local), { code: "ENOENT" });
    await write("user", { language: "German", future: "keep" });
    assert.equal(
      (await mutateSetting(cwd, mutation(await inspectSettings(cwd), "language", "user", "Greek")))
        .persistence,
      "saved",
    );
    assert.deepEqual(await document(user), { language: "Greek", future: "keep" });
    assert.deepEqual(await artifacts(user), []);
  }
  const shown = await inspectSettings(cwd),
    before = await fs.readFile(user, "utf8");
  for (const [id, scope, value] of [
    ["worktree.baseRef", "user", "invalid"],
    ["language", "user", 3],
    ["askUserQuestionTimeout", "project", "60s"],
  ] as const) {
    const result = await mutateSetting(cwd, mutation(shown, id, scope, value));
    assert.equal(result.persistence, "failure");
    assert.ok(result.error);
    assert.equal(await fs.readFile(user, "utf8"), before);
    await assert.rejects(fs.access(project), { code: "ENOENT" });
    await assert.rejects(fs.access(local), { code: "ENOENT" });
  }
  const otherProject = path.join(root, "second-project");
  await fs.mkdir(otherProject);
  const stale = mutation(shown, "language", "project", "French");
  const wrongProject = await mutateSetting(otherProject, stale);
  assert.equal(wrongProject.persistence, "failure");
  assert.match(wrongProject.error ?? "", /context/i);
  await assert.rejects(fs.access(nativePath("project", otherProject)), { code: "ENOENT" });
  assert.equal(
    (
      await mutateSetting(
        otherProject,
        mutation(await inspectSettings(otherProject), "language", "project", "French"),
      )
    ).persistence,
    "saved",
  );
  assert.deepEqual(await document(nativePath("project", otherProject)), { language: "French" });
  const profile = process.env.CLAUDE_CONFIG_DIR;
  try {
    process.env.CLAUDE_CONFIG_DIR = path.join(root, "other-profile");
    await fs.mkdir(process.env.CLAUDE_CONFIG_DIR);
    const result = await mutateSetting(cwd, mutation(shown, "language", "user", "French"));
    assert.equal(result.persistence, "failure");
    assert.match(result.error ?? "", /context/i);
    await assert.rejects(fs.access(path.join(process.env.CLAUDE_CONFIG_DIR, "settings.json")), {
      code: "ENOENT",
    });
    assert.equal(await fs.readFile(user, "utf8"), before);
    assert.equal(
      (await mutateSetting(cwd, mutation(await inspectSettings(cwd), "language", "user", "French")))
        .persistence,
      "saved",
    );
    assert.deepEqual(await document(path.join(process.env.CLAUDE_CONFIG_DIR, "settings.json")), {
      language: "French",
    });
  } finally {
    process.env.CLAUDE_CONFIG_DIR = profile;
  }
  const appA = path.join(root, "app-a.json"),
    appB = path.join(root, "app-b.json");
  await fs.writeFile(appA, '{"updates":{"autoInstall":false},"future":"a"}');
  await fs.writeFile(appB, '{"updates":{"autoInstall":false},"future":"b"}');
  const a = await fs.readFile(appA, "utf8"),
    b = await fs.readFile(appB, "utf8");
  const appDraft = mutation(
    await inspectSettings(cwd, [], [], appA),
    "updates.autoInstall",
    "user",
    true,
  );
  assert.equal((await mutateSetting(cwd, appDraft, [], [], appB)).persistence, "failure");
  assert.equal(await fs.readFile(appA, "utf8"), a);
  assert.equal(await fs.readFile(appB, "utf8"), b);
  assert.equal(
    (
      await mutateSetting(
        cwd,
        mutation(await inspectSettings(cwd, [], [], appB), "updates.autoInstall", "user", true),
        [],
        [],
        appB,
      )
    ).persistence,
    "saved",
  );
  assert.deepEqual(await document(appB), { updates: { autoInstall: true }, future: "b" });
});

test("authored primitive preferences persist at their intended scopes and reset to seeded inheritance", async () => {
  const rows: Array<{
    id: string;
    scope: SettingsScope;
    lower: Json;
    initial: Json;
    value: Json;
    expected: Json;
    inherited: Json | undefined;
  }> = [
    {
      id: "alwaysThinkingEnabled",
      scope: "project",
      lower: { alwaysThinkingEnabled: true },
      initial: { future: "keep" },
      value: false,
      expected: { future: "keep", alwaysThinkingEnabled: false },
      inherited: true,
    },
    {
      id: "worktree.baseRef",
      scope: "project",
      lower: { worktree: { baseRef: "fresh" } },
      initial: { worktree: { sparsePaths: ["src"] }, future: "keep" },
      value: "head",
      expected: { worktree: { sparsePaths: ["src"], baseRef: "head" }, future: "keep" },
      inherited: "fresh",
    },
    {
      id: "outputStyle",
      scope: "local",
      lower: { outputStyle: "Default style" },
      initial: { future: "keep" },
      value: "My custom response style",
      expected: { future: "keep", outputStyle: "My custom response style" },
      inherited: "Default style",
    },
    {
      id: "language",
      scope: "local",
      lower: { language: "German" },
      initial: { future: "keep" },
      value: "x",
      expected: { future: "keep", language: "x" },
      inherited: "German",
    },
    {
      id: "askUserQuestionTimeout",
      scope: "user",
      lower: {},
      initial: { future: "keep" },
      value: "5m",
      expected: { future: "keep", askUserQuestionTimeout: "5m" },
      inherited: undefined,
    },
  ];
  for (const row of rows) {
    await write("user", row.scope === "project" ? row.lower : {});
    await write("project", row.scope === "local" ? row.lower : {});
    await write("local", {});
    await write(row.scope, row.initial);
    const shown = await inspectSettings(cwd);
    assert.ok(shown.catalog.find((setting) => setting.id === row.id));
    const file = nativePath(row.scope),
      neighbors = await Promise.all(
        (["user", "project", "local"] as const)
          .filter((scope) => scope !== row.scope)
          .map(
            async (scope) =>
              [nativePath(scope), await fs.readFile(nativePath(scope), "utf8")] as const,
          ),
      );
    const result = await mutateSetting(cwd, mutation(shown, row.id, row.scope, row.value));
    assert.equal(result.persistence, "saved");
    assert.equal(result.application, "next_session");
    assert.deepEqual(await document(file), row.expected);
    const reopened = await inspectSettings(cwd);
    assert.equal(reopened.values.find((value) => value.id === row.id)?.value, row.value);
    assert.equal(
      reopened.sources
        .find((source) => source.scope === row.scope)
        ?.values.find((value) => value.id === row.id)?.value,
      row.value,
    );
    const formatted = `  ${JSON.stringify(row.expected, null, 3)}\n`;
    await fs.writeFile(file, formatted);
    assert.equal(
      (await mutateSetting(cwd, mutation(await inspectSettings(cwd), row.id, row.scope, row.value)))
        .persistence,
      "unchanged",
    );
    assert.equal(await fs.readFile(file, "utf8"), formatted);
    const reset = await mutateSetting(cwd, mutation(await inspectSettings(cwd), row.id, row.scope));
    assert.equal(reset.persistence, "saved");
    assert.deepEqual(await document(file), row.initial);
    assert.equal(reset.snapshot?.values.find((value) => value.id === row.id)?.value, row.inherited);
    const before = await fs.readFile(file, "utf8");
    assert.equal(
      (await mutateSetting(cwd, mutation(await inspectSettings(cwd), row.id, row.scope)))
        .persistence,
      "unchanged",
    );
    assert.equal(await fs.readFile(file, "utf8"), before);
    for (const [other, bytes] of neighbors) assert.equal(await fs.readFile(other, "utf8"), bytes);
  }
  await write("user", { idleCompaction: false, future: "user" });
  await write("project", { idleCompaction: true, future: "project" });
  await write("local", { future: "local" });
  const saved = await mutateSetting(
    cwd,
    mutation(await inspectSettings(cwd), "idleCompaction", "local", false),
  );
  assert.equal(saved.persistence, "saved");
  assert.equal(saved.application, "next_session");
  assert.deepEqual(await document(nativePath("local")), { future: "local", idleCompaction: false });
  const before = await fs.readFile(nativePath("local"), "utf8");
  assert.equal(
    (
      await mutateSetting(
        cwd,
        mutation(await inspectSettings(cwd), "idleCompaction", "local", "false"),
      )
    ).persistence,
    "failure",
  );
  assert.equal(await fs.readFile(nativePath("local"), "utf8"), before);
  assert.equal(
    (
      await mutateSetting(cwd, mutation(await inspectSettings(cwd), "idleCompaction", "local"))
    ).snapshot?.values.find((value) => value.id === "idleCompaction")?.value,
    true,
  );
  assert.deepEqual(await document(nativePath("local")), { future: "local" });
  assert.equal(
    (
      await mutateSetting(cwd, mutation(await inspectSettings(cwd), "idleCompaction", "project"))
    ).snapshot?.values.find((value) => value.id === "idleCompaction")?.value,
    false,
  );
  assert.deepEqual(await document(nativePath("project")), { future: "project" });
  assert.deepEqual(await document(nativePath("user")), { idleCompaction: false, future: "user" });
});

test("local settings and their real staged bytes are Git-private with literal-path and failure recovery", async (t) => {
  const repository = path.join(root, "git fixture"),
    project = path.join(repository, "project [private]!");
  await fs.mkdir(project, { recursive: true });
  const git = (args: string[]) =>
    promisify(execFile)("git", args, { cwd: repository, windowsHide: true });
  await git(["init"]);
  const emptyIgnore = path.join(root, "empty-global-ignore");
  await fs.writeFile(emptyIgnore, "");
  await git(["config", "core.excludesFile", emptyIgnore]);
  const exclude = path.join(repository, ".git", "info", "exclude"),
    target = nativePath("local", project);
  const relative = "project [private]!/.claude/settings.local.json";
  await fs.writeFile(exclude, "/private-existing.txt\n");
  const appendFile = fs.appendFile.bind(fs);
  t.mock.method(fs, "appendFile", async (...args: Parameters<typeof fs.appendFile>) => {
    if (path.resolve(String(args[0])) === exclude)
      throw new Error("injected exclude-update failure");
    return appendFile(...args);
  });
  try {
    const refused = await mutateSetting(
      project,
      mutation(await inspectSettings(project), "language", "local", "Greek"),
    );
    assert.equal(refused.persistence, "failure");
    assert.match(refused.error ?? "", /exclude-update/);
    await assert.rejects(fs.access(target), { code: "ENOENT" });
    assert.deepEqual(await artifacts(target), []);
    assert.equal(await fs.readFile(exclude, "utf8"), "/private-existing.txt\n");
  } finally {
    t.mock.restoreAll();
  }
  const open = fs.open.bind(fs),
    observed: string[] = [];
  t.mock.method(fs, "open", async (...args: Parameters<typeof fs.open>) => {
    if (String(args[0]).startsWith(`${target}.`) && String(args[0]).endsWith(".tmp")) {
      const staged = path.relative(repository, String(args[0])).split(path.sep).join("/");
      await git(["check-ignore", "--quiet", "--", relative]);
      await git(["check-ignore", "--quiet", "--", staged]);
      observed.push(staged);
    }
    return open(...args);
  });
  try {
    const saved = await mutateSetting(
      project,
      mutation(await inspectSettings(project), "language", "local", "Greek"),
    );
    assert.equal(saved.persistence, "saved", saved.error ?? "local save");
  } finally {
    t.mock.restoreAll();
  }
  assert.ok(observed.length > 0, "observed actual private staging before bytes were written");
  assert.deepEqual(await document(target), { language: "Greek" });
  assert.ok((await fs.readFile(exclude, "utf8")).startsWith("/private-existing.txt\n"));
  await git(["check-ignore", "--quiet", "--", relative]);
  await assert.rejects(
    git([
      "check-ignore",
      "--quiet",
      "--",
      "project [private]!/.claude/settings.local.json-neighbor",
    ]),
    (error) => (error as { code: number }).code === 1,
  );
  await assert.rejects(
    git(["check-ignore", "--quiet", "--", "project p!/.claude/settings.local.json"]),
    (error) => (error as { code: number }).code === 1,
  );
  await git(["add", "--force", "--", relative]);
  const before = await fs.readFile(target, "utf8");
  const blocked = await mutateSetting(
    project,
    mutation(await inspectSettings(project), "language", "local", "French"),
  );
  assert.equal(blocked.persistence, "failure");
  assert.match(blocked.error ?? "", /tracked by Git/);
  assert.equal(await fs.readFile(target, "utf8"), before);
  assert.deepEqual(await artifacts(target), []);
});

test("advertised model and agent identities save to separate leaves and refuse missing inventory", async () => {
  const models: AvailableModel[] = [
    {
      id: "opus",
      display_name: "Opus",
      supports_effort: true,
      supported_effort_levels: ["low", "high"],
    },
    {
      id: "sonnet",
      display_name: "Sonnet",
      supports_effort: true,
      supported_effort_levels: ["low", "high"],
    },
  ];
  const agents = [
    { name: "reviewer", description: "Review code" },
    { name: "builder", description: "Build code" },
  ];
  for (const row of [
    { id: "model", value: "sonnet", inherited: "opus" },
    { id: "agent", value: "builder", inherited: "reviewer" },
  ]) {
    await write("user", { model: "opus", agent: "reviewer", future: { keep: true } });
    await write("project", { future: "project" });
    await write("local", {});
    const userBefore = await fs.readFile(nativePath("user"), "utf8"),
      project = nativePath("project");
    const shown = await inspectSettings(cwd, models, agents);
    assert.deepEqual(
      shown.catalog.find((setting) => setting.id === row.id)?.options,
      row.id === "model" ? ["opus", "sonnet"] : ["reviewer", "builder"],
    );
    const saved = await mutateSetting(
      cwd,
      mutation(shown, row.id, "project", row.value),
      models,
      agents,
    );
    assert.equal(saved.persistence, "saved");
    assert.deepEqual(await document(project), { future: "project", [row.id]: row.value });
    assert.equal(
      (await inspectSettings(cwd, models, agents)).values.find((value) => value.id === row.id)
        ?.value,
      row.value,
    );
    const before = await fs.readFile(project, "utf8");
    for (const empty of [false, true]) {
      const current = await inspectSettings(cwd, empty ? [] : models, empty ? [] : agents);
      const refused = await mutateSetting(
        cwd,
        mutation(current, row.id, "project", empty ? row.inherited : "unadvertised"),
        empty ? [] : models,
        empty ? [] : agents,
      );
      assert.equal(refused.persistence, "failure");
      assert.equal(await fs.readFile(project, "utf8"), before);
      assert.equal(await fs.readFile(nativePath("user"), "utf8"), userBefore);
    }
    const reset = await mutateSetting(
      cwd,
      mutation(await inspectSettings(cwd, models, agents), row.id, "project"),
      models,
      agents,
    );
    assert.equal(reset.persistence, "saved");
    assert.equal(reset.snapshot?.values.find((value) => value.id === row.id)?.value, row.inherited);
    assert.deepEqual(await document(project), { future: "project" });
    assert.equal(await fs.readFile(nativePath("user"), "utf8"), userBefore);
  }
});

test("raw saves distinguish SDK-rejected and tolerated documents and repair the responsible setting", async () => {
  const file = nativePath("user");
  const rejected = { language: "German", alwaysThinkingEnabled: "invalid", future: { keep: true } };
  await write("user", rejected);
  const initial = await inspectSettings(cwd);
  assert.equal(initial.sources.find((source) => source.scope === "user")?.status, "not loaded");
  const saved = await mutateSetting(cwd, mutation(initial, "language", "user", "Greek"));
  assert.equal(saved.persistence, "saved");
  assert.equal(saved.application, "blocked");
  assert.ok(saved.error);
  assert.deepEqual(await document(file), { ...rejected, language: "Greek" });
  assert.equal(saved.snapshot?.values.find((value) => value.id === "language")?.value, undefined);
  const repaired = await mutateSetting(
    cwd,
    mutation(await inspectSettings(cwd), "alwaysThinkingEnabled", "user", true),
  );
  assert.equal(repaired.persistence, "saved");
  assert.equal(repaired.application, "next_session");
  assert.deepEqual(await document(file), {
    language: "Greek",
    alwaysThinkingEnabled: true,
    future: { keep: true },
  });
  const reopened = await inspectSettings(cwd);
  assert.equal(reopened.sources.find((source) => source.scope === "user")?.status, "valid");
  assert.equal(reopened.values.find((value) => value.id === "language")?.value, "Greek");
  assert.equal(reopened.values.find((value) => value.id === "alwaysThinkingEnabled")?.value, true);
  const sandbox = { sandbox: { enabled: "invalid" }, future: { keep: true } };
  await write("user", { ...sandbox, language: "German" });
  assert.equal(
    (await mutateSetting(cwd, mutation(await inspectSettings(cwd), "language", "user", "Greek")))
      .application,
    "blocked",
  );
  assert.deepEqual(await document(file), { ...sandbox, language: "Greek" });
  const reset = await mutateSetting(cwd, mutation(await inspectSettings(cwd), "language", "user"));
  assert.equal(reset.persistence, "saved");
  assert.deepEqual(await document(file), sandbox);
  assert.equal(
    reset.snapshot?.sources.find((source) => source.scope === "user")?.status,
    "not loaded",
  );
  assert.equal(reset.snapshot?.values.find((value) => value.id === "language")?.value, undefined);
  for (const hasLanguage of [true, false]) {
    const opaque = { hooks: "invalid", futureFeature: { keep: true } };
    await write("user", { ...opaque, ...(hasLanguage ? { language: "German" } : {}) });
    assert.equal(
      (await inspectSettings(cwd)).sources.find((source) => source.scope === "user")?.status,
      "valid",
    );
    const loaded = await mutateSetting(
      cwd,
      mutation(await inspectSettings(cwd), "language", "user", "Greek"),
    );
    assert.equal(loaded.persistence, "saved");
    assert.equal(loaded.application, "next_session");
    assert.equal(loaded.snapshot?.values.find((value) => value.id === "language")?.value, "Greek");
    assert.deepEqual(await document(file), { ...opaque, language: "Greek" });
    const removed = await mutateSetting(
      cwd,
      mutation(await inspectSettings(cwd), "language", "user"),
    );
    assert.equal(removed.persistence, "saved");
    assert.deepEqual(await document(file), opaque);
    assert.equal(
      removed.snapshot?.values.find((value) => value.id === "language")?.value,
      undefined,
    );
    assert.equal(
      removed.snapshot?.sources.find((source) => source.scope === "user")?.status,
      "valid",
    );
  }
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

test("structured setting edits retain full original objects and remove only the selected optional policy", async () => {
  const hooks: {
    PreToolUse: Array<{ matcher: string; futureGroup: Json; hooks: Array<Record<string, Json>> }>;
    Stop: Json[];
  } = {
    PreToolUse: [
      {
        matcher: "Write|Edit",
        futureGroup: { keep: true },
        hooks: [
          { type: "command", command: "unused-command", futureAction: "keep" },
          { type: "http", url: "https://example.com/hook", futureHttp: 42 },
        ],
      },
    ],
    Stop: [{ hooks: [{ type: "prompt", prompt: "Keep this event" }] }],
  };
  const ripgrep = { command: "rg", args: ["--hidden"], future: { keep: true } };
  for (const scope of ["user", "project", "local"] as const) {
    await write("user", {});
    await write("project", {});
    await write("local", {});
    const file = nativePath(scope),
      original = { hooks, sandbox: { ripgrep, enabled: true }, unrelated: { keep: "root" } };
    await write(scope, original);
    let expected = structuredClone(original);
    const policies: Array<[number, string | undefined]> =
      scope === "user"
        ? [
            [0, "continue"],
            [0, "block"],
            [0, undefined],
            [1, "block"],
            [1, "continue"],
            [1, undefined],
          ]
        : scope === "project"
          ? [
              [0, "block"],
              [0, undefined],
            ]
          : [
              [1, "continue"],
              [1, undefined],
            ];
    for (const [index, policy] of policies) {
      const edited = structuredClone(expected.hooks) as unknown as Record<string, Json>;
      const action = ((edited.PreToolUse as Json[])[0] as { hooks: Array<Record<string, Json>> })
        .hooks[index];
      if (policy === undefined) delete action.onFailure;
      else action.onFailure = policy;
      const saved = await mutateSetting(
        cwd,
        mutation(await inspectSettings(cwd), "hooks", scope, edited),
      );
      assert.equal(saved.persistence, "saved", saved.error ?? "policy save");
      expected = { ...expected, hooks: edited as unknown as typeof hooks };
      assert.deepEqual(await document(file), expected);
    }
    const editedHooks = structuredClone(expected.hooks);
    editedHooks.PreToolUse[0].hooks[0].command = "unused-new-command";
    assert.equal(
      (await mutateSetting(cwd, mutation(await inspectSettings(cwd), "hooks", scope, editedHooks)))
        .persistence,
      "saved",
    );
    expected = { ...expected, hooks: editedHooks };
    assert.deepEqual(await document(file), expected);
    if (scope === "user") {
      const editedRipgrep = { ...ripgrep, command: "custom-rg" };
      assert.equal(
        (
          await mutateSetting(
            cwd,
            mutation(await inspectSettings(cwd), "sandbox.ripgrep", scope, editedRipgrep),
          )
        ).persistence,
        "saved",
      );
      expected = { ...expected, sandbox: { ...expected.sandbox, ripgrep: editedRipgrep } };
      assert.deepEqual(await document(file), expected);
    }
    for (const action of [
      { type: "command", command: 42 },
      { type: "command", command: "unused", onFailure: "ignore" },
      { type: "http", url: "https://example.com", onFailure: true },
    ] as Array<Record<string, Json>>) {
      const bytes = await fs.readFile(file, "utf8");
      const result = await mutateSetting(
        cwd,
        mutation(await inspectSettings(cwd), "hooks", scope, { PreToolUse: [{ hooks: [action] }] }),
      );
      assert.equal(result.persistence, "failure");
      assert.equal(await fs.readFile(file, "utf8"), bytes);
      assert.deepEqual(await document(file), expected);
    }
  }
});

const effortModels: AvailableModel[] = [
  { id: "default", display_name: "Default", resolved_model: "claude-opus-5-5", supports_effort: true, supported_effort_levels: ["low", "medium", "high", "xhigh", "max"] },
  { id: "opus", display_name: "Opus", resolved_model: "claude-opus-5-5", supports_effort: true, supported_effort_levels: ["low", "medium", "high", "xhigh", "max"] },
  { id: "sonnet", display_name: "Sonnet", resolved_model: "claude-sonnet-4-6-20260217[1m]", supports_effort: true, supported_effort_levels: ["low", "medium", "high"] },
  { id: "claude-opus-4-6", display_name: "Opus 4.6", supports_effort: true, supported_effort_levels: ["low", "medium", "high", "max"] },
  { id: "haiku", display_name: "Haiku", resolved_model: "claude-haiku-4-5-20251001", supports_effort: false, supported_effort_levels: [] },
];

test("saved effort uses canonical model leaves and refuses stale or unsupported retargeting", async () => {
  await write("user", {
    model: "opus",
    modelSettings: {
      "claude-opus-5-5": { effortLevel: "medium", autoCompactWindow: 100000 },
      "claude-sonnet-4-6": { effortLevel: "low" },
    },
    future: { keep: true },
  });
  await write("project", {});
  await write("local", {});
  let shown = await inspectSettings(cwd, effortModels);
  const effort = (snapshot: SettingsSnapshot) =>
    snapshot.catalog.find((setting) => setting.id === "defaultEffort");
  assert.deepEqual(effort(shown)?.options, ["low", "medium", "high", "xhigh"]);
  assert.deepEqual(effort(shown)?.key_path, ["modelSettings", "claude-opus-5-5", "effortLevel"]);
  assert.equal(shown.values.find((value) => value.id === "defaultEffort")?.value, "medium");
  const saved = await mutateSetting(
    cwd,
    mutation(shown, "defaultEffort", "project", "high"),
    effortModels,
  );
  assert.equal(saved.persistence, "saved", saved.error ?? "save");
  assert.equal(saved.snapshot?.values.find((value) => value.id === "defaultEffort")?.value, "high");
  assert.ok(saved.snapshot);
  assert.deepEqual(await document(nativePath("project")), {
    modelSettings: { "claude-opus-5-5": { effortLevel: "high" } },
  });
  assert.deepEqual(await document(nativePath("user")), {
    model: "opus",
    modelSettings: {
      "claude-opus-5-5": { effortLevel: "medium", autoCompactWindow: 100000 },
      "claude-sonnet-4-6": { effortLevel: "low" },
    },
    future: { keep: true },
  });
  const opusDraft = mutation(saved.snapshot, "defaultEffort", "project", "low");
  const switched = await mutateSetting(
    cwd,
    mutation(saved.snapshot, "model", "user", "sonnet"),
    effortModels,
  );
  assert.equal(switched.persistence, "saved", switched.error ?? "save");
  assert.ok(switched.snapshot);
  shown = await inspectSettings(cwd, effortModels);
  assert.deepEqual(effort(shown)?.key_path, ["modelSettings", "claude-sonnet-4-6", "effortLevel"]);
  assert.deepEqual(effort(shown)?.options, ["low", "medium", "high"]);
  assert.equal(shown.values.find((value) => value.id === "defaultEffort")?.value, "low");
  const userBefore = await fs.readFile(nativePath("user"), "utf8"),
    projectBefore = await fs.readFile(nativePath("project"), "utf8");
  assert.deepEqual(await document(nativePath("user")), {
    model: "sonnet",
    modelSettings: {
      "claude-opus-5-5": { effortLevel: "medium", autoCompactWindow: 100000 },
      "claude-sonnet-4-6": { effortLevel: "low" },
    },
    future: { keep: true },
  });
  const stale = await mutateSetting(cwd, opusDraft, effortModels);
  assert.equal(
    stale.persistence,
    "failure",
    "a model change must not retarget an open effort draft",
  );
  assert.equal(await fs.readFile(nativePath("user"), "utf8"), userBefore);
  assert.equal(await fs.readFile(nativePath("project"), "utf8"), projectBefore);
  const rejected = await mutateSetting(
    cwd,
    mutation(shown, "defaultEffort", "user", "max"),
    effortModels,
  );
  assert.equal(rejected.persistence, "failure", "max stays session-only");
  assert.equal(await fs.readFile(nativePath("user"), "utf8"), userBefore);
  assert.equal(await fs.readFile(nativePath("project"), "utf8"), projectBefore);
  const changed = await mutateSetting(
    cwd,
    mutation(shown, "defaultEffort", "user", "high"),
    effortModels,
  );
  assert.equal(changed.persistence, "saved", changed.error ?? "save");
  const userPath = shown.sources.find((source) => source.scope === "user")?.path;
  assert.ok(userPath);
  assert.deepEqual(JSON.parse(await fs.readFile(userPath, "utf8")), {
    model: "sonnet",
    modelSettings: {
      "claude-opus-5-5": { effortLevel: "medium", autoCompactWindow: 100000 },
      "claude-sonnet-4-6": { effortLevel: "high" },
    },
    future: { keep: true },
  });
  const reset = await mutateSetting(
    cwd,
    mutation(await inspectSettings(cwd, effortModels), "defaultEffort", "user"),
    effortModels,
  );
  assert.equal(reset.persistence, "saved", reset.error ?? "save");
  assert.deepEqual(JSON.parse(await fs.readFile(userPath, "utf8")), {
    model: "sonnet",
    modelSettings: { "claude-opus-5-5": { effortLevel: "medium", autoCompactWindow: 100000 } },
    future: { keep: true },
  });
  const back = await mutateSetting(
    cwd,
    mutation(await inspectSettings(cwd, effortModels), "model", "user", "opus"),
    effortModels,
  );
  assert.equal(
    back.snapshot?.values.find((value) => value.id === "defaultEffort")?.value,
    "high",
    "the other model's scoped preference survives",
  );
  assert.ok(back.snapshot);
  const inherited = await mutateSetting(
    cwd,
    mutation(back.snapshot, "defaultEffort", "project"),
    effortModels,
  );
  assert.equal(
    inherited.snapshot?.values.find((value) => value.id === "defaultEffort")?.value,
    "medium",
  );
  assert.deepEqual(await document(nativePath("project")), {});
  assert.deepEqual(await document(nativePath("user")), {
    model: "opus",
    modelSettings: { "claude-opus-5-5": { effortLevel: "medium", autoCompactWindow: 100000 } },
    future: { keep: true },
  });
  await write("user", { future: "default-model" });
  await write("project", {});
  await write("local", {});
  const inferred = await inspectSettings(cwd, effortModels);
  assert.deepEqual(effort(inferred)?.key_path, ["modelSettings", "claude-opus-5-5", "effortLevel"]);
  assert.equal(
    (await mutateSetting(cwd, mutation(inferred, "defaultEffort", "user", "xhigh"), effortModels))
      .persistence,
    "saved",
  );
  assert.deepEqual(await document(nativePath("user")), {
    future: "default-model",
    modelSettings: { "claude-opus-5-5": { effortLevel: "xhigh" } },
  });
  assert.equal(
    (
      await mutateSetting(
        cwd,
        mutation(await inspectSettings(cwd, effortModels), "model", "user", "claude-opus-4-6"),
        effortModels,
      )
    ).persistence,
    "saved",
  );
  assert.equal(
    (
      await mutateSetting(
        cwd,
        mutation(await inspectSettings(cwd, effortModels), "defaultEffort", "user", "medium"),
        effortModels,
      )
    ).persistence,
    "saved",
  );
  assert.deepEqual(await document(nativePath("user")), {
    future: "default-model",
    model: "claude-opus-4-6",
    modelSettings: {
      "claude-opus-5-5": { effortLevel: "xhigh" },
      "claude-opus-4-6": { effortLevel: "medium" },
    },
  });
  assert.equal(
    (
      await mutateSetting(
        cwd,
        mutation(await inspectSettings(cwd, effortModels), "model", "user", "haiku"),
        effortModels,
      )
    ).persistence,
    "saved",
  );
  const unsupported = await inspectSettings(cwd, effortModels),
    unsupportedBytes = await fs.readFile(nativePath("user"), "utf8");
  assert.deepEqual(effort(unsupported)?.writable_scopes, []);
  const refused = await mutateSetting(
    cwd,
    mutation(unsupported, "defaultEffort", "user", "low"),
    effortModels,
  );
  assert.equal(refused.persistence, "failure");
  assert.equal(refused.application, "blocked");
  assert.equal(await fs.readFile(nativePath("user"), "utf8"), unsupportedBytes);
  assert.deepEqual(await document(nativePath("project")), {});
  assert.deepEqual(await document(nativePath("local")), {});
});

test("committed settings remain saved when snapshot refresh or owned-lock cleanup fails", async (t) => {
  const file = nativePath("user");
  const original = { language: "German", future: { untouched: [1, "two"] } };
  for (const phase of ["refresh", "lock-cleanup"]) {
    await write("user", original);
    let committed = false;
    const cause = new Error(`injected ${phase} after committed rename`);
    const models: AvailableModel[] = [
      {
        get id() {
          if (phase === "refresh" && committed) throw cause;
          return "opus";
        },
        display_name: "Opus",
        supports_effort: true,
        supported_effort_levels: ["low", "high"],
      },
    ];
    const shown = await inspectSettings(cwd, models);
    const rename = fs.rename.bind(fs),
      unlink = fs.unlink.bind(fs);
    t.mock.method(fs, "rename", async (...args: Parameters<typeof fs.rename>) => {
      await rename(...args);
      if (args[1] === file) committed = true;
    });
    t.mock.method(fs, "unlink", async (...args: Parameters<typeof fs.unlink>) => {
      if (phase === "lock-cleanup" && args[0] === `${file}.claude-rs.lock` && committed)
        throw cause;
      await unlink(...args);
    });
    try {
      const result = await mutateSetting(
        cwd,
        mutation(shown, "language", "user", "French"),
        models,
      );
      assert.equal(committed, true, "the real rename committed before the injected failure");
      assert.deepEqual(await document(file), {
        language: "French",
        future: { untouched: [1, "two"] },
      });
      assert.equal(result.persistence, "saved", phase);
      assert.equal(result.application, "blocked", phase);
      assert.match(result.error ?? "", new RegExp(phase));
      assert.equal(
        result.snapshot,
        undefined,
        "an old snapshot cannot masquerade as refreshed truth",
      );
      if (phase === "lock-cleanup")
        assert.deepEqual(await artifacts(file), [`${path.basename(file)}.claude-rs.lock`]);
    } finally {
      t.mock.restoreAll();
      // Only the lock created by this injected failure belongs to this fixture.
      if (phase === "lock-cleanup") await fs.unlink(`${file}.claude-rs.lock`);
    }
    assert.equal(
      (await inspectSettings(cwd)).values.find((value) => value.id === "language")?.value,
      "French",
    );
    assert.deepEqual(await artifacts(file), []);
  }
});

test("staging, synchronization, comparison and rename failures preserve files and allow deliberate retry", async (t) => {
  const file = nativePath("user");
  for (const exists of [true, false]) {
    for (const phase of ["sync", "comparison", "rename"]) {
      if (exists) await fs.writeFile(file, '{ "language": "German", "future": { "keep": 42 } }\n');
      else await fs.rm(file, { force: true });
      const before = exists ? await fs.readFile(file, "utf8") : undefined;
      const shown = await inspectSettings(cwd);
      const open = fs.open.bind(fs),
        readFile = fs.readFile.bind(fs),
        rename = fs.rename.bind(fs),
        lstat = fs.lstat.bind(fs);
      let staged = false;
      const stagedDocuments: unknown[] = [];
      t.mock.method(fs, "open", async (...args: Parameters<typeof fs.open>) => {
        const handle = await open(...args);
        if (String(args[0]).startsWith(`${file}.`) && String(args[0]).endsWith(".tmp")) {
          const sync = handle.sync.bind(handle);
          t.mock.method(handle, "sync", async () => {
            staged = true;
            stagedDocuments.push(JSON.parse(await readFile(args[0], "utf8")));
            if (phase === "sync") throw new Error("injected sync failure after staged bytes");
            await sync();
          });
        }
        return handle;
      });
      t.mock.method(fs, "lstat", async (...args: Parameters<typeof fs.lstat>) => {
        if (phase === "comparison" && staged && args[0] === file)
          throw new Error("injected comparison access failure");
        return lstat(...args);
      });
      t.mock.method(fs, "rename", async (...args: Parameters<typeof fs.rename>) => {
        if (phase === "rename" && args[1] === file) throw new Error("injected rename failure");
        return rename(...args);
      });
      let result: Awaited<ReturnType<typeof mutateSetting>>;
      try {
        result = await mutateSetting(cwd, mutation(shown, "language", "user", "French"));
      } finally {
        t.mock.restoreAll();
      }
      assert.ok(stagedDocuments.length > 0, `${phase} reaches real staged bytes`);
      for (const value of stagedDocuments)
        assert.deepEqual(
          value,
          exists ? { language: "French", future: { keep: 42 } } : { language: "French" },
        );
      assert.ok(
        result.persistence === "failure" || result.persistence === "conflict",
        `${phase}: ${result.persistence}`,
      );
      assert.equal(result.application, "blocked");
      assert.ok(result.error);
      if (before === undefined) await assert.rejects(fs.access(file), { code: "ENOENT" });
      else assert.equal(await fs.readFile(file, "utf8"), before);
      assert.deepEqual(await artifacts(file), [], `${phase} releases owned lock/staging files`);
      const retry = await mutateSetting(
        cwd,
        mutation(await inspectSettings(cwd), "language", "user", "French"),
      );
      assert.equal(retry.persistence, "saved", retry.error ?? "successful retry");
      assert.deepEqual(
        await document(file),
        exists ? { language: "French", future: { keep: 42 } } : { language: "French" },
      );
    }
  }
});

test("a competing settings writer cannot remove the active owner's lock or staged data", async (t) => {
  const file = nativePath("user"),
    lockPath = `${file}.claude-rs.lock`;
  await write("user", { language: "German", future: "retained" });
  const shown = await inspectSettings(cwd),
    before = await fs.readFile(file, "utf8");
  let release!: () => void, acquired!: () => void;
  const held = new Promise<void>((resolve) => {
    release = resolve;
  });
  const admitted = new Promise<void>((resolve) => {
    acquired = resolve;
  });
  const open = fs.open.bind(fs);
  let first = true;
  t.mock.method(fs, "open", async (...args: Parameters<typeof fs.open>) => {
    const handle = await open(...args);
    if (args[0] === lockPath && first) {
      first = false;
      acquired();
      await held;
    }
    return handle;
  });
  const owner = mutateSetting(cwd, mutation(shown, "language", "user", "French"));
  try {
    await Promise.race([
      admitted,
      owner.then(() => {
        throw new Error("owner finished before acquiring the gated lock");
      }),
    ]);
    const lock = await fs.stat(lockPath);
    const blocked = await mutateSetting(cwd, mutation(shown, "language", "user", "Greek"));
    assert.equal(blocked.persistence, "failure");
    assert.equal(blocked.application, "blocked");
    assert.match(blocked.error ?? "", /EEXIST|exist/i);
    assert.equal(await fs.readFile(file, "utf8"), before);
    assert.equal((await fs.stat(lockPath)).ino, lock.ino);
    assert.deepEqual(await artifacts(file), [path.basename(lockPath)]);
  } finally {
    release();
    try {
      assert.equal((await owner).persistence, "saved");
    } finally {
      t.mock.restoreAll();
    }
  }
  assert.deepEqual(await document(file), { language: "French", future: "retained" });
  assert.deepEqual(await artifacts(file), []);
  const retry = await mutateSetting(
    cwd,
    mutation(await inspectSettings(cwd), "language", "user", "Greek"),
  );
  assert.equal(retry.persistence, "saved");
  assert.deepEqual(await document(file), { language: "Greek", future: "retained" });
});

test("Unix settings staging and replacement retain actual owner-only and existing access modes", {
  skip: process.platform === "win32" ? "Unix mode bits are not Windows ACL evidence" : false,
}, () => {
  const profile = path.join(root, "mode-profile"),
    project = path.join(root, "mode-project");
  const script = `
    import fs from 'node:fs/promises';
    import {inspectSettings,mutateSetting} from ${JSON.stringify(new URL("./settings_service.js", import.meta.url).href)};
    process.umask(0o027);
    const profile=${JSON.stringify(profile)}, cwd=${JSON.stringify(project)};
    await fs.mkdir(profile,{recursive:true});await fs.mkdir(cwd,{recursive:true});
    process.env.CLAUDE_CONFIG_DIR=profile;
    const target=profile+'/settings.json', observations=[];
    const rename=fs.rename.bind(fs);
    fs.rename=async(from,to)=>{if(to===target)observations.push({phase:'stage',mode:(await fs.stat(from)).mode&0o777,document:JSON.parse(await fs.readFile(from,'utf8'))});return rename(from,to);};
    async function save(value){const shown=await inspectSettings(cwd);const entry=shown.sources.find(x=>x.scope==='user').values.find(x=>x.id==='language');const result=await mutateSetting(cwd,{context:shown.context,id:'language',scope:'user',expected_revision:entry.revision,operation:'set',value});observations.push({phase:'final',persistence:result.persistence,mode:(await fs.stat(target)).mode&0o777,document:JSON.parse(await fs.readFile(target,'utf8'))});}
    await save('French');
    await fs.writeFile(target,JSON.stringify({language:'German',future:42}));await fs.chmod(target,0o640);
    await save('Greek');
    console.log(JSON.stringify(observations));
  `;
  const child = spawnSync(process.execPath, ["--input-type=module", "-"], {
    input: script,
    encoding: "utf8",
    env: { ...process.env, CLAUDE_CONFIG_DIR: profile },
    timeout: 15_000,
    maxBuffer: 1024 * 1024,
    windowsHide: true,
  });
  assert.equal(child.error, undefined);
  assert.equal(child.signal, null);
  assert.equal(child.status, 0, child.stderr);
  assert.deepEqual(JSON.parse(child.stdout), [
    { phase: "stage", mode: 0o600, document: { language: "French" } },
    { phase: "final", persistence: "saved", mode: 0o600, document: { language: "French" } },
    { phase: "stage", mode: 0o640, document: { language: "Greek", future: 42 } },
    {
      phase: "final",
      persistence: "saved",
      mode: 0o640,
      document: { language: "Greek", future: 42 },
    },
  ]);
});


test("outside-write protection saves booleans in each scope and resets only that field", async () => {
  const id = "permissions.blockWritesOutsideWorkingDirectories";
  for (const scope of ["user", "project", "local"] as const) {
    await write(scope, { permissions: { deny: ["Bash(rm *)"] }, future: "keep" });
    const snapshot = await inspectSettings(cwd);
    const row = snapshot.catalog.find(row => row.id === id);
    assert.equal(row?.kind, "boolean");
    assert.equal(row?.category, "permissions");
    const invalid = await mutateSetting(cwd, mutation(snapshot, id, scope, "true"));
    assert.equal(invalid.persistence, "failure");
    const saved = await mutateSetting(cwd, mutation(await inspectSettings(cwd), id, scope, true));
    assert.equal(saved.persistence, "saved");
    const source = saved.snapshot?.sources.find(source => source.scope === scope);
    assert.ok(source);
    assert.deepEqual(JSON.parse(await fs.readFile(source.path, "utf8")), { permissions: { deny: ["Bash(rm *)"], blockWritesOutsideWorkingDirectories: true }, future: "keep" });
    const reset = await mutateSetting(cwd, mutation(await inspectSettings(cwd), id, scope));
    assert.equal(reset.persistence, "saved");
    assert.deepEqual(JSON.parse(await fs.readFile(source.path, "utf8")), { permissions: { deny: ["Bash(rm *)"] }, future: "keep" });
  }
});


test("outside-write settings retain source values and the SDK saved cascade rather than claiming runtime enforcement", async () => {
  const id = "permissions.blockWritesOutsideWorkingDirectories";
  await write("project", { permissions: { blockWritesOutsideWorkingDirectories: true } });
  await write("local", { permissions: { blockWritesOutsideWorkingDirectories: false } });
  const snapshot = await inspectSettings(cwd);
  assert.equal(snapshot.values.find(value => value.id === id)?.value, false);
  assert.equal(snapshot.sources.find(source => source.scope === "project")?.values.find(value => value.id === id)?.value, true);
  assert.equal(snapshot.sources.find(source => source.scope === "local")?.values.find(value => value.id === id)?.value, false);
  const reset = await mutateSetting(cwd, mutation(snapshot, id, "local"));
  assert.equal(reset.snapshot?.values.find(value => value.id === id)?.value, true);
});
