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

test("plugin-authored names, paths and versions cannot inject terminal controls", () => {
  const folder = path.resolve("safe");
  const session = { sessionId: "session", connected: true } as SessionState;
  const events: Array<Record<string, unknown>> = [];
  const restore = replaceProtocolEventWriter((line) => {
    events.push(JSON.parse(line));
  });
  try {
    updateSessionPlugins(session, [
      {
        name: "safe",
        path: folder,
        source: "safe@inline",
        version: "\u001b[2J",
      },
      { name: "bad\nname", path: folder, source: "bad@inline" },
      { name: "badpath", path: `${folder}\u001b[2J`, source: "badpath@inline" },
    ]);
    assert.deepEqual(events[0].plugins, [{ name: "safe", path: folder, source: "safe@inline" }]);
  } finally {
    restore();
  }
});

test("successive replacements retain launch intent before SDK inventory arrives without announcing loaded folders", () => {
  const folder = path.resolve("previously-confirmed");
  const session = {
    sessionId: "replacement",
    connected: true,
    pluginLaunchPaths: [folder],
  } as SessionState;
  const events: Array<Record<string, unknown>> = [];
  const restore = replaceProtocolEventWriter((line) => {
    events.push(JSON.parse(line));
  });
  try {
    assert.deepEqual(sessionPluginFolders(session), [folder]);
    assert.equal(session.pluginInventory, undefined);
    assert.equal(events.length, 0);
    updateSessionPlugins(session, []);
    assert.deepEqual(
      sessionPluginFolders(session),
      [],
      "an observed empty inventory supersedes launch intent",
    );
    assert.deepEqual(events.at(-1)?.plugins, []);
  } finally {
    restore();
  }
});

test("a closing query cannot publish late init inventory into its replacement with the same session ID", () => {
  const events: Array<Record<string, unknown>> = [];
  const restore = replaceProtocolEventWriter(line => { events.push(JSON.parse(line)); });
  try {
    const old = { sessionId: "same-session", connected: true, closing: true } as SessionState;
    updateSessionPlugins(old, [{ name: "old", path: path.resolve("old"), source: "old@inline" }]);
    assert.equal(events.length, 0);
    const current = { sessionId: "same-session", connected: true } as SessionState;
    updateSessionPlugins(current, []);
    assert.equal(events.length, 1);
    assert.deepEqual(events[0].plugins, []);
  } finally { restore(); }
});
