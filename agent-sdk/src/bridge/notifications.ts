import type { SdkNotification, ToolCall } from "../types.js";
import { asRecordOrNull } from "./shared.js";
import { pushNotificationOutput } from "./tooling.js";

export function nativeNotification(message: Record<string, unknown>): SdkNotification | undefined {
  if (typeof message.text !== "string" || !message.text.trim() ||
      typeof message.uuid !== "string" || typeof message.session_id !== "string") return undefined;
  return {
    origin: "sdk_notice", session_id: message.session_id, uuid: message.uuid,
    key: typeof message.key === "string" ? message.key : "",
    text: message.text, priority: typeof message.priority === "string" ? message.priority : "low",
    ...(typeof message.color === "string" ? { color: message.color } : {}),
    ...(typeof message.timeout_ms === "number" && Number.isFinite(message.timeout_ms) && message.timeout_ms >= 0
      ? { timeout_ms: message.timeout_ms } : {}),
  };
}

export function proactiveNotification(sessionId: string, toolName: string, tool: ToolCall, rawResult: unknown, rawContent: unknown): SdkNotification | undefined {
  const input = asRecordOrNull(tool.raw_input);
  if (toolName !== "PushNotification" || input?.status !== "proactive") return undefined;
  const output = pushNotificationOutput(rawResult, rawContent);
  if (!output || typeof output.message !== "string" || !output.message.trim()) return undefined;
  return {
    origin: "model_tool", session_id: sessionId, tool_use_id: tool.tool_call_id, text: output.message,
    ...(typeof output.pushSent === "boolean" ? { push_sent: output.pushSent } : {}),
    ...(typeof output.localSent === "boolean" ? { local_sent: output.localSent } : {}),
    ...(typeof output.disabledReason === "string" ? { disabled_reason: output.disabledReason } : {}),
    ...(typeof output.sentAt === "string" ? { sent_at: output.sentAt } : {}),
  };
}
