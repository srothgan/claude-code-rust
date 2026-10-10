import { getSessionInfo } from "@anthropic-ai/claude-agent-sdk";
import { emitSessionUpdate } from "./events.js";
import { bridgeLogger, LOG_TARGETS } from "./logger.js";
import type { SessionState } from "./session_lifecycle.js";

/**
 * Send the title Claude Code persisted for the session, read through the
 * public session API, so the app never keeps a title of its own making.
 * Resolves to whether a title was sent.
 *
 * After a "reset" (a connect, a replacement or a conversation reset) the app
 * has forgotten the title, so whatever the API reports is sent; after a
 * "refresh" only a changed title is.
 *
 * Measured on Claude Code 2.1.296: a launch-time `-n` name and a /rename reach
 * the transcript only with a turn (`getSessionInfo` reports nothing right
 * after init), so the title is read again after every top-level turn and
 * every rename the app requests. After /clear the new session id already
 * reports the old title at init.
 * `customTitle` also falls back to Claude Code's generated title, so an
 * unnamed session shows that title after its first turn.
 */
export async function emitSessionTitle(
  session: SessionState,
  after: "reset" | "refresh",
): Promise<boolean> {
  if (after === "reset") {
    session.sentTitle = undefined;
  }
  const sessionId = session.sessionId;
  const read = (session.titleReads ?? 0) + 1;
  session.titleReads = read;
  let title: string | undefined;
  try {
    title = (await getSessionInfo(sessionId, { dir: session.cwd }))?.customTitle;
  } catch (error) {
    bridgeLogger.warn({
      target: LOG_TARGETS.APP_SESSION,
      eventName: "session_title_read_failed",
      message: "failed to read the session title",
      outcome: "failure",
      sessionId,
      fields: { error_message: error instanceof Error ? error.message : String(error) },
    });
    return false;
  }
  // A later read, a replacement or a close while reading supersedes this one.
  if (
    title === undefined ||
    title === session.sentTitle ||
    session.closing ||
    session.sessionId !== sessionId ||
    session.titleReads !== read
  ) {
    return false;
  }
  session.sentTitle = title;
  emitSessionUpdate(sessionId, { type: "session_title_update", title });
  return true;
}
