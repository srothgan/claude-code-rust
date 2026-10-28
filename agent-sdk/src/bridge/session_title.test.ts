import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import {
  existsSync,
  mkdtempSync,
  mkdirSync,
  readFileSync,
  realpathSync,
  rmSync,
  writeFileSync,
  watch,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import readline from "node:readline";
import test from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";

type Envelope = Record<string, unknown>;
const SESSION_ID = "11111111-1111-4111-8111-111111111111";
const CLEARED_SESSION_ID = "22222222-2222-4222-8222-222222222222";
const isTitleUpdate = (event: Envelope) =>
  event.event === "session_update" && (event.update as Envelope)?.type === "session_title_update";
const titleOf = (event: Envelope) => (event.update as Envelope).title;

function startLaunchBridge(fixtureEnv: Record<string, string>) {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), "bridge-title-")));
  let owned: ReturnType<typeof spawn> | undefined;
  try {
    const cwd = join(directory, "project"),
      otherCwd = join(directory, "other-project");
    mkdirSync(cwd);
    mkdirSync(otherCwd);
    const store = join(directory, "titles.json"),
      calls = join(directory, "calls.jsonl"),
      renames = join(directory, "renames.txt"),
      release = join(directory, "release-held");
    writeFileSync(
      store,
      JSON.stringify([
        { session_id: SESSION_ID, dir: cwd, title: "Saved name" },
        { session_id: SESSION_ID, dir: otherCwd, title: "Other project name" },
      ]),
    );
    const sdkUrl = import.meta.resolve("@anthropic-ai/claude-agent-sdk");
    const sdkPackage = join(dirname(fileURLToPath(sdkUrl)), "package.json");
    writeFileSync(
      join(directory, "package.json"),
      JSON.stringify({ version: JSON.parse(readFileSync(sdkPackage, "utf8")).version }),
    );
    const sdk = join(directory, "sdk.mjs");
    const template = readFileSync(
      new URL("../../../tests/fixtures/session-title-sdk-template.mjs", import.meta.url),
      "utf8",
    );
    writeFileSync(sdk, template.replace('"SDK_ORIGINAL_PLACEHOLDER"', JSON.stringify(sdkUrl)));
    const loader = join(directory, "loader.mjs");
    writeFileSync(
      loader,
      `import {registerHooks} from 'node:module';registerHooks({resolve(specifier,context,next){return specifier==='@anthropic-ai/claude-agent-sdk'?{url:${JSON.stringify(pathToFileURL(sdk).href)},shortCircuit:true}:next(specifier,context);}});`,
    );
    const env = { ...process.env };
    for (const key of Object.keys(env))
      if (/^(ANTHROPIC_|CLAUDE_|CLAUDECODE$)/.test(key)) delete env[key];
    Object.assign(env, {
      CLAUDE_CONFIG_DIR: join(directory, "profile"),
      CLAUDE_CODE_EXECUTABLE: "",
      TITLE_PROJECT: cwd,
      TITLE_STORE: store,
      TITLE_CALLS: calls,
      TITLE_RELEASE: release,
      TITLE_SESSION: SESSION_ID,
      TITLE_CLEARED: CLEARED_SESSION_ID,
      RENAME_JOURNAL: renames,
      ...fixtureEnv,
    });
    const child = spawn(
      process.execPath,
      [
        "--import",
        pathToFileURL(loader).href,
        fileURLToPath(new URL("../bridge.js", import.meta.url)),
      ],
      { cwd, env, stdio: "pipe", windowsHide: true },
    );
    owned = child;
    const events: Envelope[] = [],
      stderr: string[] = [];
    const waiters = new Set<{
      predicate: (event: Envelope) => boolean;
      start: number;
      resolve: (event: Envelope) => void;
      reject: (error: Error) => void;
      timer: ReturnType<typeof setTimeout>;
    }>();
    const journalWaiters = new Set<(error: Error) => void>();
    let fatal: Error | undefined,
      timedOut = false,
      closing = false,
      barrierNumber = 0;
    const fail = (error: Error) => {
      fatal ??= error;
      for (const waiter of waiters) {
        clearTimeout(waiter.timer);
        waiter.reject(error);
      }
      waiters.clear();
    };
    const output = readline.createInterface({ input: child.stdout });
    output.on("line", (line) => {
      try {
        const event = JSON.parse(line) as Envelope;
        events.push(event);
        for (const waiter of waiters)
          if (events.length > waiter.start && waiter.predicate(event)) {
            clearTimeout(waiter.timer);
            waiters.delete(waiter);
            waiter.resolve(event);
          }
      } catch (error) {
        fail(error instanceof Error ? error : new Error(String(error)));
      }
    });
    child.stderr.on("data", (chunk) => stderr.push(String(chunk)));
    child.on("error", fail);
    child.stdin.on("error", (error) => {
      if (!closing) fail(error);
    });
    const closed = new Promise<void>((resolve) =>
      child.once("close", () => {
        output.close();
        if (!closing) fail(new Error(`bridge closed: ${stderr.join("")}`));
        resolve();
      }),
    );
    const watchdog = setTimeout(() => {
      timedOut = true;
      fail(new Error("title fixture watchdog expired"));
      child.kill();
    }, 15_000);
    const send = (message: Envelope) => {
      if (fatal) throw fatal;
      child.stdin.write(`${JSON.stringify(message)}\n`);
    };
    function readUntil(predicate: (event: Envelope) => boolean, start = 0): Promise<Envelope> {
      if (fatal) return Promise.reject(fatal);
      const found = events.slice(start).find(predicate);
      if (found) return Promise.resolve(found);
      return new Promise((resolve, reject) => {
        const waiter = {
          predicate,
          start,
          resolve,
          reject,
          timer: setTimeout(() => {
            waiters.delete(waiter);
            reject(
              new Error(
                `protocol event deadline: ${JSON.stringify(events.slice(-8))} ${stderr.join("")}`,
              ),
            );
          }, 5_000),
        };
        waiters.add(waiter);
      });
    }
    const entries = (): Envelope[] => {
      if (!existsSync(calls)) return [];
      const raw = readFileSync(calls, "utf8");
      return raw
        .slice(0, raw.lastIndexOf("\n") + 1)
        .split("\n")
        .filter(Boolean)
        .map((line) => JSON.parse(line));
    };
    function waitCall(predicate: (entry: Envelope) => boolean): Promise<Envelope> {
      return new Promise((resolve, reject) => {
        let settled = false;
        const watcher = watch(directory, check);
        const timer = setTimeout(() => finish(new Error("SDK call deadline")), 5_000);
        function finish(error?: Error, value?: Envelope) {
          if (settled) return;
          settled = true;
          watcher.close();
          clearTimeout(timer);
          journalWaiters.delete(cancel);
          if (error) reject(error);
          else resolve(value as Envelope);
        }
        const cancel = (error: Error) => finish(error);
        function check() {
          try {
            const found = entries().find(predicate);
            if (found) finish(undefined, found);
          } catch (error) {
            finish(error instanceof Error ? error : new Error(String(error)));
          }
        }
        watcher.on("error", cancel);
        journalWaiters.add(cancel);
        check();
      });
    }
    const barrier = async () => {
      const request = `title-barrier-${++barrierNumber}`;
      send({ command: "initialize", cwd, request_id: request });
      await readUntil((event) => event.event === "initialized" && event.request_id === request);
    };
    const title = (id = SESSION_ID, dir = cwd) => {
      const rows = JSON.parse(readFileSync(store, "utf8")) as Array<{
        session_id: string;
        dir: string;
        title: string;
      }>;
      return rows.find((row) => row.session_id === id && row.dir === dir)?.title;
    };
    return {
      directory,
      cwd,
      otherCwd,
      events,
      renames,
      send,
      readUntil,
      entries,
      waitCall,
      barrier,
      title,
      releaseHeld: () => writeFileSync(release, "release"),
      async turn(uuid: string, text: string, id = SESSION_ID) {
        const start = events.length;
        send({
          command: "prompt",
          session_id: id,
          message_uuid: uuid,
          chunks: [{ kind: "text", value: text }],
        });
        await waitCall(
          (entry) => entry.method === "prompt" && entry.uuid === uuid && entry.text === text,
        );
        await readUntil(
          (event) =>
            event.event === "turn_complete" &&
            event.session_id === (text === "/clear" ? CLEARED_SESSION_ID : id),
          start,
        );
        assert.ok(
          events
            .slice(start)
            .some(
              (event) =>
                event.event === "session_update" &&
                (event.update as Envelope)?.type === "agent_message_chunk" &&
                ((event.update as Envelope).content as Envelope)?.text === "TITLE TURN ANSWER",
            ),
          "the addressed fixture turn delivered its actual assistant answer",
        );
      },
      async close() {
        closing = true;
        try {
          if (child.exitCode === null && child.signalCode === null) {
            if (existsSync(release) === false) writeFileSync(release, "cleanup");
            child.stdin.write(
              `${JSON.stringify({ command: "shutdown", request_id: "fixture-shutdown" })}\n`,
            );
          }
          await closed;
        } finally {
          clearTimeout(watchdog);
          for (const waiter of waiters) {
            clearTimeout(waiter.timer);
            waiter.reject(new Error("fixture closed"));
          }
          waiters.clear();
          for (const cancel of journalWaiters) cancel(new Error("fixture closed"));
          rmSync(directory, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
        }
        assert.equal(timedOut, false, "watchdog termination is not success");
        if (fatal) throw fatal;
        assert.equal(child.exitCode, 0, stderr.join(""));
      },
    };
  } catch (error) {
    owned?.kill();
    rmSync(directory, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
    throw error;
  }
}

const resumeWithLaunchName = {
  command: "resume_session",
  session_id: SESSION_ID,
  request_id: "launch-name",
  launch_settings: { name: "Launch rename" },
};

function assertTitles(events: Envelope[], id: string, title: string, required = true) {
  const updates = events.filter(isTitleUpdate);
  if (required) assert.ok(updates.length > 0, `authoritative publication for ${title}`);
  assert.ok(
    updates.every((event) => event.session_id === id && titleOf(event) === title),
    "inspect all raw title emissions, including repetitions",
  );
}

test("title lifecycle reads and Status mutations target the selected project and publish raw authoritative updates", async () => {
  for (const command of ["resume_session", "create_session"] as const) {
    const bridge = startLaunchBridge({});
    try {
      bridge.send(
        command === "resume_session"
          ? { command, session_id: SESSION_ID, request_id: "title-start", launch_settings: {} }
          : {
              command,
              cwd: bridge.cwd,
              resume: SESSION_ID,
              request_id: "title-start",
              launch_settings: {},
            },
      );
      await bridge.readUntil((event) => isTitleUpdate(event) && titleOf(event) === "Saved name");
      await bridge.barrier();
      assertTitles(bridge.events, SESSION_ID, "Saved name");
      let start = bridge.events.length;
      await bridge.turn("failed-read", "/test-fail-read");
      await bridge.waitCall(
        (entry) =>
          entry.method === "read" &&
          entry.stage === "end" &&
          entry.phase === "failed-read" &&
          entry.outcome === "failure",
      );
      await bridge.barrier();
      assert.deepEqual(bridge.events.slice(start).filter(isTitleUpdate), []);
      start = bridge.events.length;
      await bridge.turn("first-real-turn", "first turn");
      await bridge.readUntil(
        (event) => isTitleUpdate(event) && titleOf(event) === "Launch name",
        start,
      );
      await bridge.barrier();
      assertTitles(bridge.events.slice(start), SESSION_ID, "Launch name");
      start = bridge.events.length;
      await bridge.turn("held-read", "/test-hold-read");
      await bridge.waitCall(
        (entry) => entry.method === "read" && entry.stage === "held" && entry.phase === "held-read",
      );
      await bridge.turn("newer-read", "/rename Fresh name");
      await bridge.readUntil(
        (event) => isTitleUpdate(event) && titleOf(event) === "Fresh name",
        start,
      );
      await bridge.waitCall(
        (entry) =>
          entry.method === "read" &&
          entry.stage === "end" &&
          entry.phase === "held-read" &&
          entry.outcome === "released",
      );
      await bridge.barrier();
      assertTitles(bridge.events.slice(start), SESSION_ID, "Fresh name");
      start = bridge.events.length;
      bridge.send({
        command: "rename_session",
        session_id: SESSION_ID,
        title: "Status name",
        request_id: "status-rename",
      });
      await bridge.readUntil(
        (event) => event.event === "sessions_listed" && event.request_id === "status-rename",
      );
      assertTitles(bridge.events.slice(start), SESSION_ID, "Status name");
      assert.equal(bridge.title(), "Status name");
      start = bridge.events.length;
      bridge.send({
        command: "generate_session_title",
        session_id: SESSION_ID,
        description: "independent description",
        request_id: "status-generate",
      });
      await bridge.readUntil(
        (event) => event.event === "sessions_listed" && event.request_id === "status-generate",
      );
      assertTitles(bridge.events.slice(start), SESSION_ID, "Generated name");
      assert.equal(bridge.title(), "Generated name");
      start = bridge.events.length;
      await bridge.turn("reset", "/test-reset-conversation");
      await bridge.barrier();
      assert.ok(
        bridge.events
          .slice(start)
          .some(
            (event) =>
              event.event === "session_update" &&
              (event.update as Envelope)?.type === "conversation_reset",
          ),
      );
      assertTitles(bridge.events.slice(start), SESSION_ID, "Generated name");
      start = bridge.events.length;
      await bridge.turn("clear", "/clear");
      await bridge.readUntil(
        (event) => isTitleUpdate(event) && event.session_id === CLEARED_SESSION_ID,
        start,
      );
      await bridge.barrier();
      assertTitles(bridge.events.slice(start), CLEARED_SESSION_ID, "Generated name");
      assert.equal(bridge.title(CLEARED_SESSION_ID), "Generated name");
      const calls = bridge.entries();
      assert.ok(
        calls.some(
          (entry) =>
            entry.method === "query" && entry.resume === SESSION_ID && entry.cwd === bridge.cwd,
        ),
      );
      assert.deepEqual(
        calls
          .filter((entry) => entry.method === "rename")
          .map((entry) => [entry.id, entry.title, entry.options]),
        [[SESSION_ID, "Status name", { dir: bridge.cwd }]],
      );
      assert.deepEqual(
        calls
          .filter((entry) => entry.method === "generate")
          .map((entry) => [entry.sessionId, entry.description, entry.options]),
        [[SESSION_ID, "independent description", { persist: true }]],
      );
      assert.ok(
        calls
          .filter(
            (entry) => entry.method === "read" && (entry.options as Envelope)?.dir !== undefined,
          )
          .every((entry) => (entry.options as Envelope).dir === bridge.cwd),
      );
      assert.equal(bridge.title(SESSION_ID, bridge.otherCwd), "Other project name");
      assert.equal(
        JSON.stringify(bridge.events.filter(isTitleUpdate)).includes("STALE HELD TITLE"),
        false,
      );
    } finally {
      await bridge.close();
    }
  }
});

test("startup refusal cannot rename a saved title across construction, initialization and authentication branches", async () => {
  for (const [name, env, event, cause] of [
    ["construct", { QUERY_FAIL: "1" }, "slash_error", /MCP server definition is invalid/],
    ["initialize", { INIT_FAIL: "1" }, "connection_failed", /MCP server docs failed to start/],
    ["authenticate", { SIGNED_OUT: "1" }, "auth_required", /./],
  ] as const) {
    const bridge = startLaunchBridge(env);
    try {
      bridge.send(resumeWithLaunchName);
      await bridge.readUntil((item) => item.event === event);
      await bridge.barrier();
      assert.equal(bridge.title(), "Saved name");
      assert.equal(bridge.title(SESSION_ID, bridge.otherCwd), "Other project name");
      assert.deepEqual(
        bridge.entries().filter((entry) => entry.method === "rename"),
        [],
      );
      assert.ok(
        bridge.entries().some((entry) => entry.method === "query"),
        `${name}: query construction was actually attempted`,
      );
      const failures = bridge.events.filter((item) =>
        ["slash_error", "connection_failed", "session_resume_failed"].includes(
          item.event as string,
        ),
      );
      if (name === "authenticate") assert.deepEqual(failures, []);
      else {
        assert.equal(failures.length, 1);
        assert.equal(failures[0].request_id, "launch-name");
        assert.match(String(failures[0].message), cause);
      }
      if (name === "construct") assert.equal(failures[0].session_id, SESSION_ID);
      if (name !== "authenticate")
        assert.ok(!bridge.events.some((item) => item.event === "connected"));
      else
        assert.ok(
          bridge
            .entries()
            .some((entry) => entry.method === "initialize" && entry.outcome === "signed-out"),
        );
    } finally {
      await bridge.close();
    }
  }
});

test("post-connect rename failure preserves the saved title and session through a real turn and explicit retry", async () => {
  const bridge = startLaunchBridge({ RENAME_FAIL: "1" });
  try {
    bridge.send(resumeWithLaunchName);
    await bridge.readUntil(
      (event) => event.event === "connected" && event.session_id === SESSION_ID,
    );
    await bridge.turn("after-failed-rename", "independent next-turn input");
    await bridge.barrier();
    const errors = bridge.events.filter((event) => event.event === "slash_error");
    assert.equal(errors.length, 1);
    assert.equal(errors[0].session_id, SESSION_ID);
    assert.equal(errors[0].request_id, "launch-name");
    assert.match(String(errors[0].message), /session file is read-only/);
    assert.equal(bridge.title(), "Saved name");
    assert.ok(
      !bridge.events.some((event) =>
        ["connection_failed", "session_replaced"].includes(event.event as string),
      ),
    );
    const start = bridge.events.length;
    bridge.send({
      command: "rename_session",
      session_id: SESSION_ID,
      title: "Explicit retry title",
      request_id: "retry-rename",
    });
    await bridge.readUntil(
      (event) => event.event === "sessions_listed" && event.request_id === "retry-rename",
    );
    assertTitles(bridge.events.slice(start), SESSION_ID, "Explicit retry title");
    assert.equal(bridge.title(), "Explicit retry title");
    assert.deepEqual(
      bridge
        .entries()
        .filter((entry) => entry.method === "rename")
        .map((entry) => [entry.id, entry.title, entry.options, entry.outcome]),
      [
        [SESSION_ID, "Launch rename", { dir: bridge.cwd }, "failure"],
        [SESSION_ID, "Explicit retry title", { dir: bridge.cwd }, "success"],
      ],
    );
  } finally {
    await bridge.close();
  }
});

test("pending title reads cannot publish after a real close or identity replacement", async () => {
  for (const mode of ["close", "replace"] as const) {
    const bridge = startLaunchBridge({});
    try {
      bridge.send({
        command: "resume_session",
        session_id: SESSION_ID,
        request_id: "lifetime-start",
        launch_settings: {},
      });
      await bridge.readUntil((event) => isTitleUpdate(event) && titleOf(event) === "Saved name");
      await bridge.barrier();
      await bridge.turn(
        "pending-lifetime",
        mode === "close" ? "/test-hold-close" : "/test-hold-read",
      );
      await bridge.waitCall(
        (entry) =>
          entry.method === "read" && entry.stage === "held" && entry.phase === "pending-lifetime",
      );
      const start = bridge.events.length;
      if (mode === "close") {
        bridge.send({
          command: "new_session",
          cwd: bridge.cwd,
          request_id: "close-with-refused-successor",
          launch_settings: {},
        });
        await bridge.readUntil(
          (event) =>
            event.event === "connection_failed" &&
            event.request_id === "close-with-refused-successor",
        );
        await bridge.waitCall(
          (entry) => entry.method === "close" && entry.sessionId === SESSION_ID,
        );
        bridge.releaseHeld();
        await bridge.waitCall(
          (entry) =>
            entry.method === "read" &&
            entry.stage === "end" &&
            entry.phase === "pending-lifetime" &&
            entry.outcome === "released",
        );
        await bridge.barrier();
        assert.deepEqual(
          bridge.events.slice(start).filter(isTitleUpdate),
          [],
          "close with no successor read suppresses the late title",
        );
        bridge.send({
          command: "resume_session",
          session_id: SESSION_ID,
          request_id: "survivor",
          launch_settings: {},
        });
        await bridge.readUntil(
          (event) => isTitleUpdate(event) && titleOf(event) === "Saved name",
          start,
        );
      } else {
        await bridge.turn("replacement-turn", "/clear");
        await bridge.readUntil(
          (event) => isTitleUpdate(event) && event.session_id === CLEARED_SESSION_ID,
          start,
        );
        await bridge.waitCall(
          (entry) =>
            entry.method === "read" &&
            entry.stage === "end" &&
            entry.phase === "pending-lifetime" &&
            entry.outcome === "released",
        );
        assertTitles(bridge.events.slice(start), CLEARED_SESSION_ID, "Saved name");
      }
      await bridge.barrier();
      assert.equal(
        JSON.stringify(bridge.events.slice(start).filter(isTitleUpdate)).includes(
          "STALE HELD TITLE",
        ),
        false,
      );
    } finally {
      await bridge.close();
    }
  }
});
