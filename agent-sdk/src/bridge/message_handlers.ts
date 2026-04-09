import type { SDKMessage } from "@anthropic-ai/claude-agent-sdk";
import type { AvailableCommand, BridgeCommand, ToolCallUpdateFields } from "../types.js";
import { asRecordOrNull } from "./shared.js";
import { toPermissionMode, buildModeState } from "./commands.js";
import {
  writeEvent,
  emitSessionUpdate,
  emitConnectEvent,
  emitSessionReplacedEvent,
} from "./events.js";
import { TOOL_RESULT_TYPES, unwrapToolUseResult } from "./tooling.js";
import {
  emitToolCall,
  emitToolCallUpdate,
  emitPlanIfTodoWrite,
  emitToolResultUpdate,
  finalizeOpenToolCalls,
  emitToolProgressUpdate,
  emitToolSummaryUpdate,
  ensureToolCallVisible,
  resolveTaskToolUseId,
  taskProgressText,
} from "./tool_calls.js";
import { emitAuthRequired, classifyTurnErrorKind, emitFastModeUpdateIfChanged } from "./error_classification.js";
import { mapAvailableAgents, mapAvailableAgentsFromNames, emitAvailableAgentsIfChanged, refreshAvailableAgents } from "./agents.js";
import { buildRateLimitUpdate, numberField } from "./state_parsing.js";
import { looksLikeAuthRequired } from "./auth.js";
import type { SessionState } from "./session_lifecycle.js";
import { updateSessionId } from "./session_lifecycle.js";
import { bridgeLogger, LOG_TARGETS } from "./logger.js";

export function textFromPrompt(command: Extract<BridgeCommand, { command: "prompt" }>): string {
  const chunks = command.chunks ?? [];
  return chunks
    .map((chunk) => {
      if (chunk.kind !== "text") {
        return "";
      }
      return typeof chunk.value === "string" ? chunk.value : "";
    })
    .filter((part) => part.length > 0)
    .join("");
}

/** MIME types supported by the Anthropic Vision API.
 *  NOTE: Keep in sync with `SUPPORTED_IMAGE_MIME_TYPES` in
 *  `src/app/clipboard_image.rs`. */
const SUPPORTED_IMAGE_MIME_TYPES = new Set([
  "image/png",
  "image/jpeg",
  "image/gif",
  "image/webp",
]);

/** Fast check that a string looks like valid base64 (non-empty, correct charset & padding). */
function isValidBase64(data: string): boolean {
  if (!data) return false;
  const clean = data.replace(/\s/g, "");
  if (clean.length % 4 !== 0) return false;
  // Padding ('=') must only appear at the end and be at most 2 characters.
  return /^[A-Za-z0-9+/]+={0,2}$/.test(clean);
}

/**
 * Build a content array from prompt chunks, supporting both text and image blocks.
 * Returns the Anthropic API content block format expected by MessageParam.
 */
export function contentFromPrompt(
  command: Extract<BridgeCommand, { command: "prompt" }>,
): Array<Record<string, unknown>> {
  const chunks = command.chunks ?? [];
  const content: Array<Record<string, unknown>> = [];

  for (const chunk of chunks) {
    if (chunk.kind === "text") {
      const text = typeof chunk.value === "string" ? chunk.value : "";
      if (text.trim()) {
        content.push({ type: "text", text });
      }
    } else if (chunk.kind === "image") {
      const val =
        chunk.value && typeof chunk.value === "object" ? (chunk.value as Record<string, unknown>) : null;
      if (!val) continue;
      const data = typeof val.data === "string" ? val.data : "";
      const mimeType = typeof val.mime_type === "string" ? val.mime_type : "image/png";
      if (!SUPPORTED_IMAGE_MIME_TYPES.has(mimeType)) {
        bridgeLogger.warn({
          target: LOG_TARGETS.BRIDGE_PROTOCOL,
          eventName: "prompt_image_skipped",
          message: "skipping unsupported prompt image type",
          outcome: "skipped",
          fields: { mime_type: mimeType },
        });
        continue;
      }
      if (!isValidBase64(data)) {
        bridgeLogger.warn({
          target: LOG_TARGETS.BRIDGE_PROTOCOL,
          eventName: "prompt_image_skipped",
          message: "skipping prompt image with invalid base64 data",
          outcome: "skipped",
          fields: { mime_type: mimeType, reason: "invalid_base64" },
        });
        continue;
      }
      content.push({
        type: "image",
        source: {
          type: "base64",
          media_type: mimeType,
          data,
        },
      });
    }
  }

  return content;
}

export function handleTaskSystemMessage(
  session: SessionState,
  subtype: string,
  msg: Record<string, unknown>,
): void {
  if (subtype !== "task_started" && subtype !== "task_progress" && subtype !== "task_notification") {
    return;
  }

  const taskId = typeof msg.task_id === "string" ? msg.task_id : "";
  const explicitToolUseId = typeof msg.tool_use_id === "string" ? msg.tool_use_id : "";
  if (taskId && explicitToolUseId) {
    session.taskToolUseIds.set(taskId, explicitToolUseId);
  }
  const toolUseId = resolveTaskToolUseId(session, msg);
  if (!toolUseId) {
    return;
  }

  const toolCall = ensureToolCallVisible(session, toolUseId, "Agent", {});
  if (toolCall.status === "pending") {
    emitToolCallUpdate(session, toolUseId, { status: "in_progress" }, "progress");
  }

  if (subtype === "task_started") {
    const description = typeof msg.description === "string" ? msg.description : "";
    if (!description) {
      return;
    }
    emitToolCallUpdate(
      session,
      toolUseId,
      {
        status: "in_progress",
        raw_output: description,
        content: [{ type: "content", content: { type: "text", text: description } }],
      },
      "task_started",
    );
    return;
  }

  if (subtype === "task_progress") {
    const progress = taskProgressText(msg);
    if (!progress) {
      return;
    }
    emitToolCallUpdate(
      session,
      toolUseId,
      {
        status: "in_progress",
        raw_output: progress,
        content: [{ type: "content", content: { type: "text", text: progress } }],
      },
      "task_progress",
    );
    return;
  }

  const status = typeof msg.status === "string" ? msg.status : "";
  const summary = typeof msg.summary === "string" ? msg.summary : "";
  const finalStatus = status === "completed" ? "completed" : "failed";
  const fields: ToolCallUpdateFields = { status: finalStatus };
  if (summary) {
    fields.raw_output = summary;
    fields.content = [{ type: "content", content: { type: "text", text: summary } }];
  }
  emitToolCallUpdate(session, toolUseId, fields, "task_notification");
  if (taskId) {
    session.taskToolUseIds.delete(taskId);
  }
}

export function handleContentBlock(session: SessionState, block: Record<string, unknown>): void {
  const blockType = typeof block.type === "string" ? block.type : "";

  if (blockType === "text") {
    const text = typeof block.text === "string" ? block.text : "";
    if (text) {
      emitSessionUpdate(session.sessionId, { type: "agent_message_chunk", content: { type: "text", text } });
    }
    return;
  }

  if (blockType === "thinking") {
    const text = typeof block.thinking === "string" ? block.thinking : "";
    if (text) {
      emitSessionUpdate(session.sessionId, { type: "agent_thought_chunk", content: { type: "text", text } });
    }
    return;
  }

  if (blockType === "tool_use" || blockType === "server_tool_use" || blockType === "mcp_tool_use") {
    const toolUseId = typeof block.id === "string" ? block.id : "";
    const name = typeof block.name === "string" ? block.name : "Tool";
    const input =
      block.input && typeof block.input === "object" ? (block.input as Record<string, unknown>) : {};
    if (!toolUseId) {
      return;
    }
    emitPlanIfTodoWrite(session, name, input);
    emitToolCall(session, toolUseId, name, input);
    return;
  }

  if (TOOL_RESULT_TYPES.has(blockType)) {
    const toolUseId = typeof block.tool_use_id === "string" ? block.tool_use_id : "";
    if (!toolUseId) {
      return;
    }
    const isError = Boolean(block.is_error);
    emitToolResultUpdate(session, toolUseId, isError, block.content, block);
  }
}

export function handleStreamEvent(session: SessionState, event: Record<string, unknown>): void {
  const eventType = typeof event.type === "string" ? event.type : "";

  if (eventType === "content_block_start") {
    if (event.content_block && typeof event.content_block === "object") {
      handleContentBlock(session, event.content_block as Record<string, unknown>);
    }
    return;
  }

  if (eventType === "content_block_delta") {
    if (!event.delta || typeof event.delta !== "object") {
      return;
    }
    const delta = event.delta as Record<string, unknown>;
    const deltaType = typeof delta.type === "string" ? delta.type : "";
    if (deltaType === "text_delta") {
      const text = typeof delta.text === "string" ? delta.text : "";
      if (text) {
        emitSessionUpdate(session.sessionId, { type: "agent_message_chunk", content: { type: "text", text } });
      }
    } else if (deltaType === "thinking_delta") {
      const text = typeof delta.thinking === "string" ? delta.thinking : "";
      if (text) {
        emitSessionUpdate(session.sessionId, { type: "agent_thought_chunk", content: { type: "text", text } });
      }
    }
  }
}

export function handleAssistantMessage(session: SessionState, message: Record<string, unknown>): void {
  const assistantError = typeof message.error === "string" ? message.error : "";
  if (assistantError.length > 0) {
    session.lastAssistantError = assistantError;
  }

  const messageObject =
    message.message && typeof message.message === "object"
      ? (message.message as Record<string, unknown>)
      : null;
  if (!messageObject) {
    return;
  }
  const content = Array.isArray(messageObject.content) ? messageObject.content : [];
  for (const block of content) {
    if (!block || typeof block !== "object") {
      continue;
    }
    const blockRecord = block as Record<string, unknown>;
    const blockType = typeof blockRecord.type === "string" ? blockRecord.type : "";
    if (
      blockType === "tool_use" ||
      blockType === "server_tool_use" ||
      blockType === "mcp_tool_use" ||
      TOOL_RESULT_TYPES.has(blockType)
    ) {
      handleContentBlock(session, blockRecord);
    }
  }
}

export function handleUserToolResultBlocks(session: SessionState, message: Record<string, unknown>): void {
  const messageObject =
    message.message && typeof message.message === "object"
      ? (message.message as Record<string, unknown>)
      : null;
  if (!messageObject) {
    return;
  }
  const content = Array.isArray(messageObject.content) ? messageObject.content : [];
  for (const block of content) {
    if (!block || typeof block !== "object") {
      continue;
    }
    const blockRecord = block as Record<string, unknown>;
    const blockType = typeof blockRecord.type === "string" ? blockRecord.type : "";
    if (TOOL_RESULT_TYPES.has(blockType)) {
      handleContentBlock(session, blockRecord);
    }
  }
}

export function handleResultMessage(session: SessionState, message: Record<string, unknown>): void {
  emitFastModeUpdateIfChanged(session, message.fast_mode_state);

  const subtype = typeof message.subtype === "string" ? message.subtype : "";
  if (subtype === "success") {
    session.lastAssistantError = undefined;
    finalizeOpenToolCalls(session, "completed");
    writeEvent({ event: "turn_complete", session_id: session.sessionId });
    return;
  }

  const errors =
    Array.isArray(message.errors) && message.errors.every((entry) => typeof entry === "string")
      ? (message.errors as string[])
      : [];
  const assistantError = session.lastAssistantError;
  const authHint = errors.find((entry) => looksLikeAuthRequired(entry));
  if (authHint) {
    emitAuthRequired(session, authHint);
  }
  if (assistantError === "authentication_failed") {
    emitAuthRequired(session);
  }
  finalizeOpenToolCalls(session, "failed");
  const errorKind = classifyTurnErrorKind(subtype, errors, assistantError);
  const fallback = subtype ? `turn failed: ${subtype}` : "turn failed";
  writeEvent({
    event: "turn_error",
    session_id: session.sessionId,
    message: errors.length > 0 ? errors.join("\n") : fallback,
    error_kind: errorKind,
    ...(subtype ? { sdk_result_subtype: subtype } : {}),
    ...(assistantError ? { assistant_error: assistantError } : {}),
  });
  session.lastAssistantError = undefined;
}

export function handleSdkMessage(session: SessionState, message: SDKMessage): void {
  const msg = message as unknown as Record<string, unknown>;
  const type = typeof msg.type === "string" ? msg.type : "";

  if (type === "system") {
    const subtype = typeof msg.subtype === "string" ? msg.subtype : "";
    if (subtype === "init") {
      const previousSessionId = session.sessionId;
      const incomingSessionId = typeof msg.session_id === "string" ? msg.session_id : session.sessionId;
      updateSessionId(session, incomingSessionId);
      const previousModelName = session.model;
      const modelName = typeof msg.model === "string" ? msg.model : session.model;
      session.model = modelName;

      const incomingMode = typeof msg.permissionMode === "string" ? toPermissionMode(msg.permissionMode) : null;
      if (incomingMode) {
        session.mode = incomingMode;
      }
      emitFastModeUpdateIfChanged(session, msg.fast_mode_state);

      if (!session.connected) {
        emitConnectEvent(session);
      } else if (previousSessionId !== session.sessionId) {
        emitSessionReplacedEvent(session);
      } else {
        if (session.model !== previousModelName) {
          emitSessionUpdate(session.sessionId, {
            type: "config_option_update",
            option_id: "model",
            value: session.model,
          });
        }
        if (incomingMode) {
          emitSessionUpdate(session.sessionId, {
            type: "mode_state_update",
            mode: buildModeState(incomingMode),
          });
        }
      }

      if (Array.isArray(msg.slash_commands)) {
        const commands: AvailableCommand[] = msg.slash_commands
          .filter((entry): entry is string => typeof entry === "string")
          .map((name) => ({ name, description: "", input_hint: undefined }));
        if (commands.length > 0) {
          emitSessionUpdate(session.sessionId, { type: "available_commands_update", commands });
        }
      }

      if (session.lastAvailableAgentsSignature === undefined && Array.isArray(msg.agents)) {
        emitAvailableAgentsIfChanged(session, mapAvailableAgentsFromNames(msg.agents));
      }

      void session.query
        .supportedCommands()
        .then((commands) => {
          const mapped: AvailableCommand[] = commands.map((command) => ({
            name: command.name,
            description: command.description ?? "",
            input_hint: command.argumentHint ?? undefined,
          }));
          emitSessionUpdate(session.sessionId, { type: "available_commands_update", commands: mapped });
        })
        .catch(() => {
          // Best-effort only; slash commands from init were already emitted.
        });
      refreshAvailableAgents(session);
      return;
    }

    if (subtype === "status") {
      const mode =
        typeof msg.permissionMode === "string" ? toPermissionMode(msg.permissionMode) : null;
      if (mode) {
        session.mode = mode;
        emitSessionUpdate(session.sessionId, { type: "current_mode_update", current_mode_id: mode });
      }
      if (msg.status === "compacting") {
        emitSessionUpdate(session.sessionId, { type: "session_status_update", status: "compacting" });
      } else if (msg.status === null) {
        emitSessionUpdate(session.sessionId, { type: "session_status_update", status: "idle" });
      }
      emitFastModeUpdateIfChanged(session, msg.fast_mode_state);
      return;
    }

    if (subtype === "compact_boundary") {
      const compactMetadata = asRecordOrNull(msg.compact_metadata);
      if (!compactMetadata) {
        return;
      }
      const trigger = compactMetadata.trigger;
      const preTokens = numberField(compactMetadata, "pre_tokens", "preTokens");
      if ((trigger === "manual" || trigger === "auto") && preTokens !== undefined) {
        emitSessionUpdate(session.sessionId, {
          type: "compaction_boundary",
          trigger,
          pre_tokens: preTokens,
        });
      }
      return;
    }

    if (subtype === "local_command_output") {
      const content = typeof msg.content === "string" ? msg.content : "";
      if (content.trim().length > 0) {
        emitSessionUpdate(session.sessionId, {
          type: "agent_message_chunk",
          content: { type: "text", text: content },
        });
      }
      return;
    }

    if (subtype === "elicitation_complete") {
      const elicitationId = typeof msg.elicitation_id === "string" ? msg.elicitation_id : "";
      if (!elicitationId) {
        return;
      }
      writeEvent({
        event: "elicitation_complete",
        session_id: session.sessionId,
        completion: {
          elicitation_id: elicitationId,
          ...(typeof msg.mcp_server_name === "string" ? { server_name: msg.mcp_server_name } : {}),
        },
      });
      return;
    }

    handleTaskSystemMessage(session, subtype, msg);
    return;
  }

  if (type === "auth_status") {
    const output = Array.isArray(msg.output)
      ? msg.output.filter((entry): entry is string => typeof entry === "string").join("\n")
      : "";
    const errorText = typeof msg.error === "string" ? msg.error : "";
    const combined = [errorText, output].filter((entry) => entry.length > 0).join("\n");
    if (combined && looksLikeAuthRequired(combined)) {
      emitAuthRequired(session, combined);
    }
    return;
  }

  if (type === "stream_event") {
    if (msg.event && typeof msg.event === "object") {
      handleStreamEvent(session, msg.event as Record<string, unknown>);
    }
    return;
  }

  if (type === "tool_progress") {
    const toolUseId = typeof msg.tool_use_id === "string" ? msg.tool_use_id : "";
    const toolName = typeof msg.tool_name === "string" ? msg.tool_name : "Tool";
    if (toolUseId) {
      emitToolProgressUpdate(session, toolUseId, toolName);
    }
    return;
  }

  if (type === "tool_use_summary") {
    const summary = typeof msg.summary === "string" ? msg.summary : "";
    const toolIds = Array.isArray(msg.preceding_tool_use_ids)
      ? msg.preceding_tool_use_ids.filter((id): id is string => typeof id === "string")
      : [];
    if (summary && toolIds.length > 0) {
      for (const toolUseId of toolIds) {
        emitToolSummaryUpdate(session, toolUseId, summary);
      }
    }
    return;
  }

  if (type === "rate_limit_event") {
    const update = buildRateLimitUpdate(msg.rate_limit_info);
    if (update) {
      emitSessionUpdate(session.sessionId, update);
    }
    return;
  }

  if (type === "user") {
    handleUserToolResultBlocks(session, msg);

    const toolUseId = typeof msg.parent_tool_use_id === "string" ? msg.parent_tool_use_id : "";
    if (toolUseId && "tool_use_result" in msg) {
      const parsed = unwrapToolUseResult(msg.tool_use_result);
      emitToolResultUpdate(session, toolUseId, parsed.isError, parsed.content, msg.tool_use_result);
    }
    return;
  }

  if (type === "assistant") {
    if (msg.error === "authentication_failed") {
      emitAuthRequired(session);
    }
    handleAssistantMessage(session, msg);
    return;
  }

  if (type === "result") {
    handleResultMessage(session, msg);
  }
}
