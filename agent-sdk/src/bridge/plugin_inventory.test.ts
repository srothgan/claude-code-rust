import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import assert from "node:assert/strict";
import path from "node:path";
import { test } from "node:test";
import { replaceProtocolEventWriter } from "./events.js";
import { sessionPluginFolders, updateSessionPlugins } from "./plugin_inventory.js";
import type { SessionState } from "./session_lifecycle.js";

test("only SDK-confirmed session folders reach inventory and replacement launch intent", () => {
  const folder = path.resolve("confirmed-local");
  const requested = path.resolve("requested-but-absent");
  const restored = path.resolve("restored");
  const session = {
    sessionId: "session",
    cwd: process.cwd(),
    connected: true,
    pluginLaunchPaths: [requested, restored],
  } as SessionState;
  const events: Array<Record<string, unknown>> = [];
  const restore = replaceProtocolEventWriter((line) => {
    events.push(JSON.parse(line));
  });
  try {
    updateSessionPlugins(session, [
      {
        name: "confirmed",
        path: folder,
        source: "confirmed@inline",
        version: "1.2.3",
      },
      { name: "restored", path: restored },
      {
        name: "installed",
        path: path.resolve("installed"),
        source: "installed@marketplace",
      },
      { name: "builtin", path: "builtin", source: "builtin@builtin" },
    ]);
    assert.deepEqual(sessionPluginFolders(session), [folder, restored]);
    assert.deepEqual(events[0].plugins, [
      {
        name: "confirmed",
        path: folder,
        source: "confirmed@inline",
        version: "1.2.3",
      },
      { name: "restored", path: restored },
    ]);
    updateSessionPlugins(session, []);
    assert.deepEqual(sessionPluginFolders(session), []);
    assert.deepEqual(events.at(-1)?.plugins, []);
  } finally {
    restore();
  }
});

type PluginEvent = { event: string; session_id?: string; plugins?: unknown };
type Probe = {
  id: string;
  folder: string;
  confirmedEvents: PluginEvent[];
  snapshots: Array<{
    kind: string;
    name?: string;
    plugins?: unknown;
    folders: string[];
    events?: PluginEvent[];
  }>;
  queries: Array<{ name: string; plugins?: unknown; closed: boolean }>;
};
function probe(mode: string): Probe {
  const fixture = fileURLToPath(
    new URL("../../../tests/fixtures/plugin-inventory-runtime.mjs", import.meta.url),
  );
  const child = spawnSync(process.execPath, [fixture, mode], {
    encoding: "utf8",
    windowsHide: true,
    timeout: 15_000,
  });
  assert.equal(child.error, undefined, String(child.error));
  assert.equal(child.signal, null, child.stderr);
  assert.equal(child.status, 0, child.stderr);
  return JSON.parse(child.stdout);
}
function replacements(output: Probe) {
  const confirmed = [
    {
      name: "confirmed-local",
      path: output.folder,
      source: "confirmed-local@inline",
      version: "1.2.3",
    },
  ];
  assert.deepEqual(
    output.confirmedEvents.filter((event) => event.event === "plugin_inventory"),
    [{ event: "plugin_inventory", session_id: output.id, plugins: confirmed }],
    "native init confirms the intended folder before replacement",
  );
  const carried = output.snapshots.filter((snapshot) => snapshot.kind === "replacement");
  assert.equal(carried.length, 2, "two successive replacements precede the next inventory");
  assert.equal(
    new Set(carried.map((snapshot) => snapshot.name)).size,
    2,
    "successive replacements create distinguishable native queries",
  );
  for (const snapshot of carried) {
    assert.deepEqual(
      snapshot.plugins,
      [{ type: "local", path: output.folder }],
      "actual SDK query receives carried local folder",
    );
    assert.deepEqual(snapshot.folders, [output.folder]);
    assert.ok(
      snapshot.events?.some(
        (event) => event.event === "session_replaced" && event.session_id === output.id,
      ),
    );
    assert.deepEqual(
      snapshot.events?.filter((event) => event.event === "plugin_inventory"),
      [],
      "connection does not announce unobserved plugins as loaded",
    );
  }
  const empty = output.snapshots.find((snapshot) => snapshot.kind === "empty");
  assert.ok(empty);
  assert.deepEqual(empty.folders, []);
  assert.deepEqual(
    empty.events?.filter((event) => event.event === "plugin_inventory"),
    [{ event: "plugin_inventory", session_id: output.id, plugins: [] }],
  );
  const after = output.snapshots.find((snapshot) => snapshot.kind === "after-empty");
  assert.ok(after);
  assert.equal(
    after.plugins,
    undefined,
    "explicit empty inventory removes folder from the next actual query",
  );
  assert.deepEqual(after.folders, []);
  assert.ok(
    output.queries.every((query) => query.closed),
    "owned SDK queries close before the child finishes",
  );
}

test("unsafe plugin identities are excluded while optional fields preserve safe Unicode neighbors and attachment evidence", () => {
  const folder = path.resolve("Candidate folder"),
    safeFolder = path.resolve("Valid 插件 folder");
  const safe = {
    name: "Valid 插件",
    path: safeFolder,
    source: "Valid 插件@inline",
    version: "2.0.1",
  };
  const candidate = {
    name: "candidate",
    path: folder,
    source: "candidate@inline",
    version: "1.2.3",
  };
  const cases: Array<{
    name: string;
    field: string;
    value: unknown;
    optional?: boolean;
    withoutIntent?: boolean;
  }> = [
    { name: "newline-name", field: "name", value: "bad\nname" },
    { name: "escape-path", field: "path", value: `${folder}\u001b[2J` },
    { name: "format-name", field: "name", value: "bad\u202ename" },
    { name: "separator-path", field: "path", value: `${folder}\u2028bad` },
    { name: "scalar-name", field: "name", value: 42 },
    { name: "null-path", field: "path", value: null },
    { name: "unsafe-version", field: "version", value: "unsafe\u001b[2J", optional: true },
    { name: "scalar-version", field: "version", value: 42, optional: true },
    { name: "unsafe-source", field: "source", value: "unsafe\u202e@inline", optional: true },
    { name: "null-source", field: "source", value: null, optional: true },
    {
      name: "source-only-not-evidence",
      field: "source",
      value: "unsafe\u202e@inline",
      withoutIntent: true,
    },
  ];
  for (const row of cases) {
    const state = {
      sessionId: row.name,
      connected: true,
      pluginLaunchPaths: row.withoutIntent ? [] : [folder],
    } as unknown as SessionState;
    const events: PluginEvent[] = [];
    const restore = replaceProtocolEventWriter((line) => events.push(JSON.parse(line)));
    try {
      updateSessionPlugins(state, [safe, { ...candidate, [row.field]: row.value }]);
      const optional =
        row.field === "source"
          ? { name: "candidate", path: folder, version: "1.2.3" }
          : { name: "candidate", path: folder, source: "candidate@inline" };
      const expected = row.optional ? [safe, optional] : [safe];
      assert.deepEqual(
        events,
        [{ event: "plugin_inventory", session_id: row.name, plugins: expected }],
        row.name,
      );
      assert.deepEqual(
        sessionPluginFolders(state),
        row.optional ? [safeFolder, folder] : [safeFolder],
        row.name,
      );
    } finally {
      restore();
    }
  }
});

test("successive real replacements carry local launch intent without announcing unobserved inventory", () => {
  replacements(probe("replacement"));
});

test("omitted native inventory preserves confirmed attachment until an explicit empty snapshot", () => {
  const output = probe("omitted");
  const omitted = output.snapshots.find((snapshot) => snapshot.kind === "omitted");
  assert.ok(omitted);
  assert.deepEqual(
    omitted.folders,
    [output.folder],
    "omitted observation preserves confirmed folder knowledge",
  );
  for (const event of omitted.events?.filter((event) => event.event === "plugin_inventory") ?? []) {
    assert.equal(event.session_id, output.id);
    assert.deepEqual(
      event.plugins,
      [
        {
          name: "confirmed-local",
          path: output.folder,
          source: "confirmed-local@inline",
          version: "1.2.3",
        },
      ],
      "an absent observation must not fabricate empty inventory",
    );
  }
  replacements(output);
});
