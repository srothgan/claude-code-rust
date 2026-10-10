import type { SessionState } from "./session_lifecycle.js";
import { emitSessionUpdate } from "./events.js";

/** Only live, top-level streaming block boundaries own the observed phase. */
export type MainAgentResponse = {
  messageId: string;
  thinkingBlock?: number;
};

/**
 * Whether a completed top-level assistant message belongs to a response whose
 * stream this session never saw begin. Its blocks arrive in order inside that
 * response, before the next one starts, so the last started response is the
 * only one a completed message can continue. Unlike the activity scope, a
 * stop, retry or reset does not forget it. A message without a response ID
 * cannot be correlated and counts as streamed.
 */
export function responseNeverStreamed(session: SessionState, responseId: unknown): boolean {
  return typeof responseId === "string" && responseId.length > 0 &&
    responseId !== session.lastStreamedResponseId;
}

export function resetMainAgentActivity(session: SessionState): void {
  if (session.mainAgentResponse?.thinkingBlock !== undefined) {
    emitSessionUpdate(session.sessionId, { type: "agent_activity_update", phase: "working" });
  }
  session.mainAgentResponse = undefined;
}

export function observeMainAgentStream(
  session: SessionState,
  event: Record<string, unknown>,
  parentToolUseId?: string,
): void {
  if (parentToolUseId || session.closing) return;
  if (event.type === "message_start") {
    const message = event.message as Record<string, unknown> | undefined;
    if (typeof message?.id !== "string" || !message.id) return;
    session.lastStreamedResponseId = message.id;
    if (session.mainAgentResponse?.messageId === message.id) return;
    resetMainAgentActivity(session);
    session.mainAgentResponse = { messageId: message.id };
    emitSessionUpdate(session.sessionId, { type: "agent_response_started" });
    return;
  }
  const response = session.mainAgentResponse;
  if (!response) return;
  // The SDK UUID identifies this frame, not the API response. Boundaries are
  // ordered inside the message_start/message_stop scope.
  if (event.type === "message_stop" || event.type === "error") {
    resetMainAgentActivity(session);
    return;
  }
  if (!Number.isInteger(event.index) || (event.index as number) < 0) return;
  const index = event.index as number;
  const wasThinking = response.thinkingBlock !== undefined;
  if (event.type === "content_block_start") {
    const block = event.content_block as Record<string, unknown> | undefined;
    response.thinkingBlock = block?.type === "thinking" || block?.type === "redacted_thinking" ? index : undefined;
  } else if (event.type === "content_block_stop") {
    if (response.thinkingBlock === index) response.thinkingBlock = undefined;
  }
  const thinking = response.thinkingBlock !== undefined;
  if (thinking !== wasThinking) {
    emitSessionUpdate(session.sessionId, { type: "agent_activity_update", phase: thinking ? "thinking" : "working" });
  }
}
