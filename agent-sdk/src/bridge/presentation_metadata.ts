import type { SessionUpdate } from "../types.js";

/** SDK wall clocks and elapsed clocks are independent; never derive one from the other. */
export function messageTimestamp(value: unknown): string | undefined {
  return typeof value === "string" && Number.isFinite(Date.parse(value)) ? value : undefined;
}

export function elapsedNumber(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : undefined;
}

export function messageMetadata(message: Record<string, unknown>, role: "user" | "assistant"): SessionUpdate | undefined {
  if (typeof message.parent_tool_use_id === "string") return undefined;
  const nested = message.message && typeof message.message === "object" ? message.message as Record<string, unknown> : undefined;
  const timestamp = messageTimestamp(message.timestamp) ?? messageTimestamp(nested?.timestamp);
  const uuid = typeof message.uuid === "string" ? message.uuid : typeof nested?.id === "string" ? nested.id : undefined;
  return timestamp ? { type: "message_metadata", role, timestamp, ...(uuid ? { source_message_uuid: uuid } : {}) } : undefined;
}

export function turnTiming(message: Record<string, unknown>): SessionUpdate | undefined {
  const duration = elapsedNumber(message.duration_ms);
  const api = elapsedNumber(message.duration_api_ms);
  return duration !== undefined ? { type: "turn_timing", duration_ms: duration, ...(api !== undefined ? { api_duration_ms: api } : {}) } : undefined;
}
