import type { SessionUpdate } from "../types.js";
import { writeEvent } from "./events.js";
import { bridgeLogger, LOG_TARGETS } from "./logger.js";
import { getSessionTranscriptMessages, mapSessionMessagesToUpdates } from "./history.js";

export type HistoryExportLimits = {
  /** A chunk is flushed once adding the next update would pass this many bytes. */
  chunkBytes: number;
  /**
   * Updates past this many bytes are left out and counted. The transcript is already
   * loaded and translated by then, so this bounds what the app receives, holds and
   * renders, not the bridge's own memory.
   */
  maxBytes: number;
};

export const DEFAULT_HISTORY_EXPORT_LIMITS: HistoryExportLimits = {
  chunkBytes: 256 * 1024,
  maxBytes: 32 * 1024 * 1024,
};

/**
 * Send a session's complete saved history to the app for /export, translated by the
 * same mapping a resume uses so history has one translation authority. The
 * in-memory transcript the app holds is capped and cannot be the source.
 */
export async function exportSessionHistory(
  sessionId: string,
  cwd: string | undefined,
  requestId: string | undefined,
  limits: HistoryExportLimits = DEFAULT_HISTORY_EXPORT_LIMITS,
): Promise<void> {
  try {
    const messages = await getSessionTranscriptMessages(
      sessionId,
      cwd ? { dir: cwd } : {},
    );
    const updates = mapSessionMessagesToUpdates(messages);
    let chunk: SessionUpdate[] = [];
    let chunkBytes = 0;
    let totalBytes = 0;
    let included = 0;
    const flush = (last: boolean, omitted: number): void => {
      writeEvent(
        {
          event: "session_history_export",
          session_id: sessionId,
          updates: chunk,
          last,
          omitted_updates: omitted,
        },
        requestId,
      );
      chunk = [];
      chunkBytes = 0;
    };
    for (const update of updates) {
      const size = Buffer.byteLength(JSON.stringify(update));
      if (totalBytes + size > limits.maxBytes) {
        break;
      }
      if (chunk.length > 0 && chunkBytes + size > limits.chunkBytes) {
        flush(false, 0);
      }
      chunk.push(update);
      chunkBytes += size;
      totalBytes += size;
      included += 1;
    }
    flush(true, updates.length - included);
    bridgeLogger.info({
      target: LOG_TARGETS.APP_SESSION,
      eventName: "session_history_exported",
      message: "session history sent for export",
      outcome: "success",
      ...(requestId ? { requestId } : {}),
      sessionId,
      fields: {
        history_message_count: messages.length,
        history_update_count: updates.length,
        omitted_update_count: updates.length - included,
      },
    });
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    bridgeLogger.warn({
      target: LOG_TARGETS.APP_SESSION,
      eventName: "session_history_export_failed",
      message: "failed to load session history for export",
      outcome: "failure",
      ...(requestId ? { requestId } : {}),
      sessionId,
      fields: { error_message: message },
    });
    emitHistoryExportFailure(
      sessionId,
      `failed to load session history: ${message}`,
      requestId,
    );
  }
}

/**
 * Report a failed export on the request-scoped event the app is waiting for, so every
 * failure reaches the one pending export and none reaches unrelated slash handling.
 */
export function emitHistoryExportFailure(
  sessionId: string,
  error: string,
  requestId: string | undefined,
): void {
  writeEvent(
    {
      event: "session_history_export",
      session_id: sessionId,
      updates: [],
      last: true,
      omitted_updates: 0,
      error,
    },
    requestId,
  );
}
