import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import readline from "node:readline";
import test from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";

type Envelope = Record<string, unknown>;

const SESSION_ID = "11111111-1111-4111-8111-111111111111";
const CLEARED_SESSION_ID = "22222222-2222-4222-8222-222222222222";

/**
 * A stand-in for the SDK's query: every prompt is a local command, answered
 * the way Claude Code 2.1.296 answers one (one complete assistant message, then
 * the result naming the prompt's uuid). /rename appends the `custom-title`
 * record Claude Code writes, and so does a first turn under a launch-time `-n`
 * name; /clear starts a new session id whose transcript carries the title over. The public session APIs stay the real SDK's.
 */
function writeSdkFixture(fixturePath: string, projectDir: string, cwd: string): void {
  const sdkUrl = import.meta.resolve("@anthropic-ai/claude-agent-sdk");
  writeFileSync(fixturePath, `
    export * from ${JSON.stringify(sdkUrl)};
    import { appendFileSync, writeFileSync } from "node:fs";
    import { join } from "node:path";
    const projectDir = ${JSON.stringify(projectDir)};
    const cwd = ${JSON.stringify(cwd)};
    const record = (sessionId, entry) => JSON.stringify({ ...entry, sessionId, cwd, timestamp: "2026-10-10T00:00:00.000Z" }) + "\\n";
    export async function listSessions() {
      return [{ sessionId: ${JSON.stringify(SESSION_ID)}, cwd, lastModified: 1 }];
    }
    export function query({ prompt, options }) {
      appendFileSync(process.env.QUERY_JOURNAL, JSON.stringify({ resume: options.resume }) + "\\n");
      const input = prompt[Symbol.asyncIterator]();
      const pending = [];
      let sessionId = options.resume;
      let title;
      let replies = 0;
      return {
        [Symbol.asyncIterator]() { return this; },
        async next() {
          if (pending.length === 0) {
            const next = await input.next();
            if (next.done) return { done: true, value: undefined };
            const content = next.value.message.content;
            const text = typeof content === "string" ? content : content.map((block) => block.text ?? "").join("");
            let reply = "";
            if (text.startsWith("/rename ")) {
              title = text.slice("/rename ".length);
              appendFileSync(join(projectDir, sessionId + ".jsonl"), record(sessionId, { type: "custom-title", customTitle: title }));
              reply = "Session renamed to: " + title;
            } else if (text === "first turn") {
              title = "Launch name";
              appendFileSync(join(projectDir, sessionId + ".jsonl"), record(sessionId, { type: "custom-title", customTitle: title }));
              reply = "ok";
            } else if (text === "/clear") {
              sessionId = ${JSON.stringify(CLEARED_SESSION_ID)};
              writeFileSync(join(projectDir, sessionId + ".jsonl"),
                record(sessionId, { type: "user", uuid: "33333333-3333-4333-8333-333333333333", parentUuid: null, message: { role: "user", content: "/clear" } }) +
                record(sessionId, { type: "custom-title", customTitle: title }));
              pending.push({ type: "system", subtype: "init", session_id: sessionId, uuid: "init-" + replies });
            } else {
              reply = "Unknown command: " + text;
            }
            replies += 1;
            if (reply) {
              pending.push({ type: "assistant", uuid: "reply-" + replies, session_id: sessionId, parent_tool_use_id: null,
                message: { id: "local-" + replies, role: "assistant", content: [{ type: "text", text: reply }] } });
            }
            pending.push({ type: "result", subtype: "success", uuid: "result-" + replies, session_id: sessionId, parent_tool_use_id: null,
              result: reply, user_message_uuid: next.value.uuid, user_message_uuids: [next.value.uuid] });
          }
          return { done: false, value: pending.shift() };
        },
        close() {},
        async initializationResult() { return { current_permission_mode: "default", models: [], commands: [], agents: [], account: { apiKeySource: "fixture" }, fast_mode_state: "off" }; },
        async getSettings() { return { applied: { model: "opus", effort: "high", ultracodeAvailable: false, ultracodeRequested: false, ultracode: false } }; },
        async supportedCommands() { return []; },
      };
    }
  `);
}

const isTitleUpdate = (event: Envelope) =>
  event.event === "session_update" && (event.update as Envelope)?.type === "session_title_update";

test("the session title follows Claude Code's persisted title through resume, a first turn, /rename and /clear", async () => {
  for (const command of ["resume_session", "create_session"] as const) {
    const directory = realpathSync(mkdtempSync(join(tmpdir(), "bridge-title-")));
    const cwd = join(directory, "project");
    const profile = join(directory, "config");
    const projectDir = join(profile, "projects", cwd.replace(/[^a-zA-Z0-9]/g, "-"));
    mkdirSync(cwd);
    mkdirSync(projectDir, { recursive: true });
    const records = [
      { type: "custom-title", customTitle: "Saved name" },
      { type: "user", uuid: "user-1", parentUuid: null, message: { role: "user", content: "hello" } },
    ].map((record) => ({ ...record, sessionId: SESSION_ID, cwd, timestamp: "2026-10-09T00:00:00.000Z" }));
    writeFileSync(join(projectDir, `${SESSION_ID}.jsonl`), `${records.map((record) => JSON.stringify(record)).join("\n")}\n`);

    const journal = join(directory, "query.jsonl");
    const fixturePath = join(directory, "sdk-fixture.mjs");
    // The bridge checks the SDK version beside the module it resolves, which
    // is the stand-in here, so it gets the installed SDK's version.
    const sdkPackage = join(dirname(fileURLToPath(import.meta.resolve("@anthropic-ai/claude-agent-sdk"))), "package.json");
    writeFileSync(join(directory, "package.json"), JSON.stringify({ version: JSON.parse(readFileSync(sdkPackage, "utf8")).version }));
    writeSdkFixture(fixturePath, projectDir, cwd);
    const loaderPath = join(directory, "loader.mjs");
    writeFileSync(loaderPath, `
      import { registerHooks } from "node:module";
      registerHooks({ resolve(specifier, context, nextResolve) {
        return specifier === "@anthropic-ai/claude-agent-sdk"
          ? { url: ${JSON.stringify(pathToFileURL(fixturePath).href)}, shortCircuit: true }
          : nextResolve(specifier, context);
      } });
    `);
    const bridgePath = join(dirname(fileURLToPath(import.meta.url)), "../bridge.js");
    const child = spawn(process.execPath, ["--import", pathToFileURL(loaderPath).href, bridgePath], {
      env: { ...process.env, CLAUDE_CONFIG_DIR: profile, CLAUDE_CODE_PROJECT_DIR_NAME: undefined, QUERY_JOURNAL: journal, CLAUDE_CODE_EXECUTABLE: "" },
      stdio: "pipe",
    });
    const output = readline.createInterface({ input: child.stdout });
    const events: Envelope[] = [];
    const errors: string[] = [];
    child.stderr.on("data", (chunk: Buffer) => errors.push(chunk.toString()));
    const timeout = setTimeout(() => child.kill(), 10_000);
    const send = (message: Envelope) => child.stdin.write(`${JSON.stringify(message)}\n`);
    const prompt = (uuid: string, text: string) =>
      send({ command: "prompt", session_id: SESSION_ID, message_uuid: uuid, chunks: [{ kind: "text", value: text }] });
    const lines = output[Symbol.asyncIterator]();
    const readUntil = async (done: (event: Envelope) => boolean) => {
      for (;;) {
        const next = await lines.next();
        assert.ok(!next.done, `${command}: bridge closed: ${JSON.stringify(events)} ${errors.join("")}`);
        const event = JSON.parse(next.value) as Envelope;
        events.push(event);
        if (done(event)) return;
      }
    };
    try {
      send(command === "resume_session"
        ? { command, session_id: SESSION_ID, request_id: "title-resume", launch_settings: {} }
        : { command, cwd, resume: SESSION_ID, request_id: "title-resume", launch_settings: {} });
      await readUntil(isTitleUpdate);
      assert.ok(events.some((event) => event.event === "connected"), `${command}: ${JSON.stringify(events)}`);
      const query = JSON.parse(readFileSync(journal, "utf8").trim()) as Envelope;
      assert.equal(query.resume, SESSION_ID);

      // Claude Code writes a launch-time name with the first turn, so it shows
      // once that turn's result is in. A turn that changes nothing sends no
      // update; the /rename turn sends the new title.
      prompt("prompt-first", "first turn");
      await readUntil((event) => isTitleUpdate(event) && (event.update as Envelope).title === "Launch name");
      prompt("prompt-other", "/renamed elsewhere");
      await readUntil((event) => event.event === "turn_complete");
      prompt("prompt-rename", "/rename Fresh name");
      await readUntil((event) => isTitleUpdate(event) && (event.update as Envelope).title === "Fresh name");

      // /clear replaces the session id; the new id's persisted title is sent.
      prompt("prompt-clear", "/clear");
      await readUntil((event) => isTitleUpdate(event) && event.session_id === CLEARED_SESSION_ID);

      assert.deepEqual(
        events.filter(isTitleUpdate).map((event) => [event.session_id, (event.update as Envelope).title]),
        [[SESSION_ID, "Saved name"], [SESSION_ID, "Launch name"], [SESSION_ID, "Fresh name"], [CLEARED_SESSION_ID, "Fresh name"]],
        command,
      );
      assert.ok(events.some((event) => event.event === "session_replaced" && event.session_id === CLEARED_SESSION_ID));
    } finally {
      clearTimeout(timeout);
      output.close();
      if (child.exitCode === null && child.signalCode === null) {
        child.kill();
        await new Promise<void>((resolve) => child.once("exit", () => resolve()));
      }
      rmSync(directory, { recursive: true, force: true });
    }
  }
});
