import assert from "node:assert/strict";
import { appendFileSync, mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import type { SDKMessage } from "@anthropic-ai/claude-agent-sdk";
import { buildSessionMutationOptions } from "../bridge.js";
import { handleSessionDataCommand } from "./command_session_data.js";
import { replaceProtocolEventWriter } from "./events.js";
import { handleSdkMessage } from "./message_handlers.js";
import { sessions, type SessionState } from "./session_lifecycle.js";
import { emitSessionTitle } from "./session_title.js";

const TITLED = "44444444-4444-4444-8444-444444444444";
const UNTITLED = "55555555-5555-4555-8555-555555555555";

async function withTranscripts(
  run: (cwd: string, projectDir: string) => Promise<void>,
): Promise<void> {
  const directory = realpathSync(mkdtempSync(path.join(os.tmpdir(), "session-title-")));
  const cwd = path.join(directory, "project");
  const projectDir = path.join(directory, "config", "projects", cwd.replace(/[^a-zA-Z0-9]/g, "-"));
  mkdirSync(cwd);
  mkdirSync(projectDir, { recursive: true });
  const transcript = (sessionId: string, records: Array<Record<string, unknown>>) =>
    writeFileSync(
      path.join(projectDir, `${sessionId}.jsonl`),
      records
        .map((record) => JSON.stringify({ ...record, sessionId, cwd, timestamp: "2026-10-10T00:00:00.000Z" }))
        .join("\n"),
    );
  const user = { type: "user", uuid: "66666666-6666-4666-8666-666666666666", parentUuid: null, message: { role: "user", content: "hello" } };
  transcript(TITLED, [user, { type: "custom-title", customTitle: "persisted name" }]);
  transcript(UNTITLED, [user]);
  const previous = process.env.CLAUDE_CONFIG_DIR;
  process.env.CLAUDE_CONFIG_DIR = path.join(directory, "config");
  try {
    await run(cwd, projectDir);
  } finally {
    if (previous === undefined) {
      delete process.env.CLAUDE_CONFIG_DIR;
    } else {
      process.env.CLAUDE_CONFIG_DIR = previous;
    }
    rmSync(directory, { recursive: true, force: true });
  }
}

async function titleUpdates(run: () => Promise<unknown>): Promise<Array<Record<string, unknown>>> {
  const writes: string[] = [];
  const restore = replaceProtocolEventWriter((line) => {
    writes.push(line);
  });
  try {
    await run();
  } finally {
    restore();
  }
  return writes
    .map((line) => JSON.parse(line) as Record<string, unknown>)
    .filter((event) => (event.update as Record<string, unknown> | undefined)?.type === "session_title_update")
    .map((event) => ({ session_id: event.session_id, title: (event.update as Record<string, unknown>).title }));
}

const session = (sessionId: string, cwd: string) =>
  ({ sessionId, cwd, connected: true, closing: false }) as unknown as SessionState;

test("the title sent for a session is the one Claude Code persisted for its id", async () => {
  await withTranscripts(async (cwd) => {
    assert.deepEqual(await titleUpdates(() => emitSessionTitle(session(TITLED, cwd), "reset")), [
      { session_id: TITLED, title: "persisted name" },
    ]);
    assert.deepEqual(await titleUpdates(() => emitSessionTitle(session(UNTITLED, cwd), "reset")), []);
  });
});

test("a title read for a session that was replaced meanwhile is not sent", async () => {
  await withTranscripts(async (cwd) => {
    const replaced = session(TITLED, cwd);
    const updates = await titleUpdates(async () => {
      const read = emitSessionTitle(replaced, "reset");
      replaced.sessionId = UNTITLED;
      await read;
    });
    assert.deepEqual(updates, []);
  });
});

test("after a turn only a changed title is sent, and a reset sends it again", async () => {
  await withTranscripts(async (cwd) => {
    const titled = session(TITLED, cwd);
    const updates = await titleUpdates(async () => {
      await emitSessionTitle(titled, "reset");
      await emitSessionTitle(titled, "refresh");
      await emitSessionTitle(titled, "reset");
    });
    assert.deepEqual(updates, [
      { session_id: TITLED, title: "persisted name" },
      { session_id: TITLED, title: "persisted name" },
    ]);
  });
});

test("of two overlapping reads only the later one is sent", async () => {
  await withTranscripts(async (cwd) => {
    const titled = session(TITLED, cwd);
    const updates = await titleUpdates(async () => {
      await Promise.all([emitSessionTitle(titled, "reset"), emitSessionTitle(titled, "refresh")]);
    });
    assert.deepEqual(updates, [{ session_id: TITLED, title: "persisted name" }]);
  });
});

test("a conversation reset sends the title the API reports after it, even an unchanged one", { timeout: 10_000 }, async () => {
  await withTranscripts(async (cwd) => {
    const titled = session(TITLED, cwd);
    const titles: unknown[] = [];
    let titleSent = () => {};
    const restore = replaceProtocolEventWriter((line) => {
      const update = (JSON.parse(line) as Record<string, unknown>).update as Record<string, unknown> | undefined;
      if (update?.type === "session_title_update") {
        titles.push(update.title);
        titleSent();
      }
    });
    try {
      await emitSessionTitle(titled, "reset");
      // The app drops the title on the reset, so the same title is sent again.
      const sent = new Promise<void>((resolve) => {
        titleSent = resolve;
      });
      handleSdkMessage(titled, {
        type: "conversation_reset",
        session_id: TITLED,
        new_conversation_id: "conversation-2",
      } as unknown as SDKMessage);
      await sent;
    } finally {
      restore();
    }
    assert.deepEqual(titles, ["persisted name", "persisted name"]);
  });
});

test("a rename or a generated title from the Status tab is sent once it is persisted", async () => {
  await withTranscripts(async (cwd, projectDir) => {
    const titled = session(TITLED, cwd);
    const untitled = session(UNTITLED, cwd);
    sessions.set(TITLED, titled);
    sessions.set(UNTITLED, untitled);
    const deps = {
      buildSessionMutationOptions,
      // Stands in for Query.generateSessionTitle(description, { persist: true }),
      // which needs a running Claude Code. The pinned CLI persists a generated
      // title as an ai-title record, and only for a session without a title.
      generatePersistedSessionTitle: async () => {
        appendFileSync(
          path.join(projectDir, `${UNTITLED}.jsonl`),
          `\n${JSON.stringify({ type: "ai-title", aiTitle: "Generated name", sessionId: UNTITLED })}`,
        );
        return "Generated name";
      },
      rewindTargetsFromSessionMessages: () => [],
      handleRewind: async () => undefined,
    };
    try {
      const updates = await titleUpdates(async () => {
        await emitSessionTitle(titled, "reset");
        await handleSessionDataCommand(
          { command: "rename_session", session_id: TITLED, title: "Status name" },
          "request-rename",
          deps,
        );
        await handleSessionDataCommand(
          { command: "generate_session_title", session_id: UNTITLED, description: "hello" },
          "request-generate",
          deps,
        );
      });
      assert.deepEqual(updates, [
        { session_id: TITLED, title: "persisted name" },
        { session_id: TITLED, title: "Status name" },
        { session_id: UNTITLED, title: "Generated name" },
      ]);
    } finally {
      sessions.delete(TITLED);
      sessions.delete(UNTITLED);
    }
  });
});
