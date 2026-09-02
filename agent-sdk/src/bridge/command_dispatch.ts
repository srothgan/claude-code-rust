import type { BridgeCommand } from "../types.js";
import { writeEvent } from "./events.js";
import { bridgeLogger, LOG_TARGETS } from "./logger.js";

type CancelTurnCommand = Extract<BridgeCommand, { command: "cancel_turn" }>;

export type InterruptibleSession = {
  query: {
    interrupt: () => Promise<{ still_queued?: string[] } | undefined>;
  };
};

type CancelTurnDispatchDeps = {
  requestId?: string;
  sessionById: (sessionId: string) => InterruptibleSession | null | undefined;
  slashError: (sessionId: string, message: string, requestId?: string) => void;
};

export async function dispatchCancelTurnCommand(
  command: CancelTurnCommand,
  deps: CancelTurnDispatchDeps,
): Promise<void> {
  const session = deps.sessionById(command.session_id);
  if (!session) {
    deps.slashError(
      command.session_id,
      `unknown session: ${command.session_id}`,
      deps.requestId,
    );
    return;
  }
  const receipt = await session.query.interrupt();
  const stillQueued = Array.isArray(receipt?.still_queued)
    ? receipt.still_queued.filter(
        (entry): entry is string => typeof entry === "string",
      )
    : [];
  writeEvent(
    {
      event: "turn_interrupt_receipt",
      session_id: command.session_id,
      still_queued: stillQueued,
    },
    deps.requestId,
  );
  if (stillQueued.length > 0) {
    bridgeLogger.info({
      target: LOG_TARGETS.APP_SESSION,
      eventName: "interrupt_receipt_still_queued",
      message: "interrupt receipt reported queued async messages",
      outcome: "success",
      sessionId: command.session_id,
      ...(deps.requestId ? { requestId: deps.requestId } : {}),
      fields: {
        still_queued_count: stillQueued.length,
        still_queued: stillQueued,
      },
    });
  }
}
