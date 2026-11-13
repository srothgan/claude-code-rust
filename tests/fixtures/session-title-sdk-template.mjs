// Replaced SDK module template; the test substitutes the installed module URL.
export * from "SDK_ORIGINAL_PLACEHOLDER";
import fs from "node:fs";
import path from "node:path";
const cwd = process.env.TITLE_PROJECT;
let queryOpened = false,
  phase = "lookup",
  failNextRead = false,
  holdNextRead = false;
let failNextQuery = false,
  failRename = Boolean(process.env.RENAME_FAIL),
  held,
  readNumber = 0;
const journal = (entry) =>
  fs.appendFileSync(process.env.TITLE_CALLS, JSON.stringify({ ...entry, phase }) + "\n");
const load = () => JSON.parse(fs.readFileSync(process.env.TITLE_STORE, "utf8"));
const lookup = (id, dir = cwd) => load().find((row) => row.session_id === id && row.dir === dir);
function save(id, title, dir = cwd) {
  const rows = load();
  const previous = rows.find((row) => row.session_id === id && row.dir === dir);
  if (previous) previous.title = title;
  else rows.push({ session_id: id, dir, title });
  fs.writeFileSync(process.env.TITLE_STORE, JSON.stringify(rows));
}
export async function getSessionInfo(id, options = {}) {
  const read = ++readNumber,
    startedPhase = phase;
  journal({ method: "read", stage: "start", read, id, options });
  const release = held;
  held = undefined;
  release?.();
  const dir = options.dir ?? cwd;
  if (queryOpened && options.dir !== cwd) throw new Error("wrong title project directory");
  if (failNextRead) {
    failNextRead = false;
    journal({ method: "read", stage: "end", read, id, options, outcome: "failure" });
    throw new Error("session info unavailable");
  }
  if (holdNextRead) {
    holdNextRead = false;
    journal({ method: "read", stage: "held", read, id, options });
    return new Promise((resolve) => {
      const watcher = fs.watch(path.dirname(process.env.TITLE_RELEASE), () => {
        if (fs.existsSync(process.env.TITLE_RELEASE)) {
          fs.unlinkSync(process.env.TITLE_RELEASE);
          releaseHeld();
        }
      });
      const releaseHeld = () => {
        watcher.close();
        fs.appendFileSync(
          process.env.TITLE_CALLS,
          JSON.stringify({
            method: "read",
            stage: "end",
            read,
            id,
            options,
            outcome: "released",
            phase: startedPhase,
          }) + "\n",
        );
        resolve({
          sessionId: id,
          cwd: dir,
          summary: "STALE HELD TITLE",
          customTitle: "STALE HELD TITLE",
          lastModified: 1,
        });
      };
      held = releaseHeld;
    });
  }
  const row = lookup(id, dir);
  journal({
    method: "read",
    stage: "end",
    read,
    id,
    options,
    outcome: "success",
    title: row?.title,
  });
  return row
    ? { sessionId: id, cwd: dir, summary: row.title, customTitle: row.title, lastModified: 1 }
    : undefined;
}
export async function renameSession(id, title, options = {}) {
  phase = `rename:${title}`;
  journal({ method: "rename", id, title, options, outcome: failRename ? "failure" : "success" });
  if (options.dir !== cwd) throw new Error("wrong rename project directory");
  if (failRename) {
    failRename = false;
    throw new Error("session file is read-only");
  }
  save(id, title, options.dir);
  fs.appendFileSync(process.env.RENAME_JOURNAL, title + "\n");
}
export async function importSessionToStore() {}
export async function getSessionMessages() {
  return [];
}
export async function listSessions() {
  return [
    {
      sessionId: process.env.TITLE_SESSION,
      cwd,
      lastModified: 1,
      customTitle: lookup(process.env.TITLE_SESSION).title,
    },
  ];
}
export function query({ prompt, options }) {
  journal({ method: "query", resume: options.resume, cwd: options.cwd });
  if (process.env.QUERY_FAIL || failNextQuery) {
    failNextQuery = false;
    throw new Error("MCP server definition is invalid");
  }
  queryOpened = true;
  phase = "connect";
  const input = prompt[Symbol.asyncIterator](),
    pending = [];
  let sessionId = options.resume ?? options.sessionId,
    replies = 0;
  return {
    [Symbol.asyncIterator]() {
      return this;
    },
    async next() {
      if (pending.length === 0) {
        const next = await input.next();
        if (next.done) return { done: true, value: undefined };
        const content = next.value.message.content;
        const text =
          typeof content === "string" ? content : content.map((block) => block.text ?? "").join("");
        phase = next.value.uuid;
        replies++;
        journal({ method: "prompt", sessionId, uuid: next.value.uuid, text });
        if (text.startsWith("/rename ")) save(sessionId, text.slice(8));
        else if (text === "first turn") save(sessionId, "Launch name");
        else if (text === "/test-fail-read") failNextRead = true;
        else if (text === "/test-hold-read" || text === "/test-hold-close") {
          holdNextRead = true;
          if (text === "/test-hold-close") failNextQuery = true;
        } else if (text === "/test-reset-conversation")
          pending.push({
            type: "conversation_reset",
            session_id: sessionId,
            uuid: `reset-${replies}`,
            new_conversation_id: `conversation-${replies}`,
          });
        else if (text === "/clear") {
          const title = lookup(sessionId).title;
          sessionId = process.env.TITLE_CLEARED;
          save(sessionId, title);
          pending.push({
            type: "system",
            subtype: "init",
            session_id: sessionId,
            uuid: `init-${replies}`,
          });
        }
        pending.push({
          type: "assistant",
          session_id: sessionId,
          uuid: `reply-${replies}`,
          parent_tool_use_id: null,
          message: {
            id: `message-${replies}`,
            role: "assistant",
            content: [{ type: "text", text: "TITLE TURN ANSWER" }],
          },
        });
        pending.push({
          type: "result",
          subtype: "success",
          session_id: sessionId,
          uuid: `result-${replies}`,
          parent_tool_use_id: null,
          result: "TITLE TURN ANSWER",
          user_message_uuid: next.value.uuid,
          user_message_uuids: [next.value.uuid],
        });
      }
      return { done: false, value: pending.shift() };
    },
    close() {
      journal({ method: "close", sessionId });
    },
    async generateSessionTitle(description, options) {
      phase = "generate";
      journal({ method: "generate", sessionId, description, options });
      if (options?.persist !== true) throw new Error("generation must persist");
      save(sessionId, "Generated name");
      return "Generated name";
    },
    async initializationResult() {
      journal({
        method: "initialize",
        sessionId,
        outcome: process.env.INIT_FAIL
          ? "failure"
          : process.env.SIGNED_OUT
            ? "signed-out"
            : "success",
      });
      if (process.env.INIT_FAIL) throw new Error("MCP server docs failed to start");
      return {
        current_permission_mode: "default",
        models: [],
        commands: [],
        agents: [],
        account: process.env.SIGNED_OUT ? {} : { apiKeySource: "fixture" },
        fast_mode_state: "off",
      };
    },
    async getSettings() {
      return {
        applied: {
          model: "opus",
          effort: "high",
          ultracodeAvailable: false,
          ultracodeRequested: false,
          ultracode: false,
        },
      };
    },
    async supportedCommands() {
      return [];
    },
  };
}
