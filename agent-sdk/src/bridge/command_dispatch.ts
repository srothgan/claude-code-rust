import type { BridgeCommand } from "../types.js";

type CancelTurnCommand = Extract<BridgeCommand, { command: "cancel_turn" }>;

export type InterruptibleSession = {
  query: {
    interrupt: () => Promise<void>;
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
    deps.slashError(command.session_id, `unknown session: ${command.session_id}`, deps.requestId);
    return;
  }
  await session.query.interrupt();
}
