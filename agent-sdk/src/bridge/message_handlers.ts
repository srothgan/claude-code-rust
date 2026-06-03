import type { SDKMessage } from "@anthropic-ai/claude-agent-sdk";
import type { ContentBlockParam } from "@anthropic-ai/sdk/resources/messages/messages";
import type {
  BridgeCommand,
  SystemNoticeSeverity,
  TaskMetadata,
  TerminalReason,
  ToolCallUpdateFields,
} from "../types.js";
import { asRecordOrNull } from "./shared.js";
import { toPermissionMode, buildModeState, refreshSupportedModesForSession } from "./commands.js";
import {
  writeEvent,
  emitSessionUpdate,
  emitConnectEvent,
  emitSessionReplacedEvent,
} from "./events.js";
import {
  TOOL_RESULT_TYPES,
  isToolSearchToolName,
  isToolSearchToolResultType,
  unwrapToolUseResult,
} from "./tooling.js";
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
  toolAcceptsTaskLifecycle,
  taskProgressText,
  taskUpdatedFields,
  type ToolCorrelationMetadata,
} from "./tool_calls.js";
import { emitAuthRequired, classifyTurnErrorKind, emitFastModeUpdateIfChanged } from "./error_classification.js";
import { mapAvailableAgentsFromNames, emitAvailableAgentsIfChanged, refreshAvailableAgents } from "./agents.js";
import {
  mapInitSlashCommands,
  mapSdkSlashCommands,
  updateAvailableCommands,
} from "./available_commands.js";
import {
  buildApiRetryUpdate,
  buildRateLimitUpdate,
  normalizeSettingsParseErrors,
  numberField,
  parseApiRetryError,
  parseRuntimeSessionState,
} from "./state_parsing.js";
import { looksLikeAuthRequired } from "./auth.js";
import type { SessionState } from "./session_lifecycle.js";
import { emitCurrentModelUpdate, refreshCurrentModel, updateSessionId } from "./session_lifecycle.js";
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

type SupportedImageMimeType = "image/png" | "image/jpeg" | "image/gif" | "image/webp";

function sdkCorrelationMetadata(msg: Record<string, unknown>): ToolCorrelationMetadata {
  return {
    requestId: typeof msg.request_id === "string" ? msg.request_id : undefined,
    subagentType: typeof msg.subagent_type === "string" ? msg.subagent_type : undefined,
    taskDescription: typeof msg.task_description === "string" ? msg.task_description : undefined,
  };
}

function sdkTaskMetadata(msg: Record<string, unknown>): TaskMetadata | undefined {
  const metadata = sdkCorrelationMetadata(msg);
  const taskMetadata: TaskMetadata = {
    ...(metadata.requestId ? { request_id: metadata.requestId } : {}),
    ...(metadata.subagentType ? { subagent_type: metadata.subagentType } : {}),
    ...(metadata.taskDescription ? { task_description: metadata.taskDescription } : {}),
  };
  return Object.keys(taskMetadata).length > 0 ? taskMetadata : undefined;
}

function sdkMessageOriginKind(msg: Record<string, unknown>): string | undefined {
  const origin = msg.origin && typeof msg.origin === "object" ? (msg.origin as Record<string, unknown>) : null;
  return typeof origin?.kind === "string" ? origin.kind : undefined;
}

function logSdkMessageOrigin(session: SessionState, msg: Record<string, unknown>): void {
  const originKind = sdkMessageOriginKind(msg);
  if (!originKind) {
    return;
  }
  bridgeLogger.debug({
    target: LOG_TARGETS.APP_SESSION,
    eventName: "sdk_message_origin_observed",
    message: "SDK message origin observed",
    outcome: originKind === "auto-continuation" ? "accepted" : "observed",
    sessionId: session.sessionId,
    fields: {
      message_type: typeof msg.type === "string" ? msg.type : undefined,
      origin_kind: originKind,
    },
  });
}

function emitSystemNoticeUpdate(
  session: SessionState,
  severity: SystemNoticeSeverity,
  message: string,
): void {
  const trimmed = message.trim();
  if (!trimmed) {
    return;
  }
  emitSessionUpdate(session.sessionId, { type: "system_notice_update", severity, message: trimmed });
}

function notificationSeverity(priority: unknown): SystemNoticeSeverity {
  return priority === "high" || priority === "immediate" ? "warning" : "info";
}

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
): ContentBlockParam[] {
  const chunks = command.chunks ?? [];
  const content: ContentBlockParam[] = [];

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
      const supportedMimeType = mimeType as SupportedImageMimeType;
      content.push({
        type: "image",
        source: {
          type: "base64",
          media_type: supportedMimeType,
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
): boolean {
  if (
    subtype !== "task_started" &&
    subtype !== "task_progress" &&
    subtype !== "task_updated" &&
    subtype !== "task_notification"
  ) {
    return false;
  }

  const taskId = typeof msg.task_id === "string" ? msg.task_id : "";
  const explicitToolUseId = typeof msg.tool_use_id === "string" ? msg.tool_use_id : "";
  const messageTaskMetadata = sdkTaskMetadata(msg);
  if (taskId && explicitToolUseId) {
    session.taskToolUseIds.set(taskId, explicitToolUseId);
  }
  const toolUseId = resolveTaskToolUseId(session, msg);
  bridgeLogger.debug({
    target: LOG_TARGETS.APP_TOOL,
    eventName: "sdk_task_linkage_observed",
    message: "SDK task lifecycle linkage observed",
    outcome: toolUseId ? "resolved" : "unresolved",
    sessionId: session.sessionId,
    toolCallId: toolUseId || explicitToolUseId || undefined,
    fields: {
      sdk_subtype: subtype,
      task_id: taskId || undefined,
      explicit_tool_use_id: explicitToolUseId || undefined,
      resolved_tool_use_id: toolUseId || undefined,
      task_status: typeof msg.status === "string" ? msg.status : undefined,
      has_description: typeof msg.description === "string" && msg.description.length > 0,
      has_summary: typeof msg.summary === "string" && msg.summary.length > 0,
      last_tool_name: typeof msg.last_tool_name === "string" ? msg.last_tool_name : undefined,
    },
  });
  if (subtype === "task_updated") {
    bridgeLogger.debug({
      target: LOG_TARGETS.APP_TOOL,
      eventName: "task_updated_received",
      message: "task update received",
      outcome: toolUseId ? "resolved" : "unresolved",
      sessionId: session.sessionId,
      toolCallId: toolUseId || undefined,
      fields: {
        task_id: taskId,
        explicit_tool_use_id: explicitToolUseId || undefined,
        patch_keys:
          msg.patch && typeof msg.patch === "object"
            ? Object.keys(msg.patch as Record<string, unknown>).sort()
            : undefined,
      },
    });
  }
  if (!toolUseId) {
    if (subtype === "task_updated" && taskId) {
      bridgeLogger.debug({
        target: LOG_TARGETS.APP_TOOL,
        eventName: "task_updated_unlinked",
        message: "task update skipped because no visible tool call was linked",
        outcome: "skipped",
        sessionId: session.sessionId,
        fields: { task_id: taskId, subtype },
      });
    }
    return true;
  }

  const toolCall = ensureToolCallVisible(session, toolUseId, "Agent", {});
  if (!toolAcceptsTaskLifecycle(toolCall)) {
    if (taskId) {
      session.taskToolUseIds.delete(taskId);
    }
    return true;
  }
  if (toolCall.status === "pending") {
    emitToolCallUpdate(session, toolUseId, { status: "in_progress" }, "progress");
  }

  if (subtype === "task_started") {
    const description = typeof msg.description === "string" ? msg.description : "";
    if (!description) {
      return true;
    }
    const fields: ToolCallUpdateFields = {
      status: "in_progress",
      raw_output: description,
      content: [{ type: "content", content: { type: "text", text: description } }],
      ...(messageTaskMetadata ? { task_metadata: messageTaskMetadata } : {}),
    };
    emitToolCallUpdate(
      session,
      toolUseId,
      fields,
      "task_started",
    );
    return true;
  }

  if (subtype === "task_progress") {
    const progress = taskProgressText(msg);
    if (!progress) {
      return true;
    }
    const fields: ToolCallUpdateFields = {
      status: "in_progress",
      raw_output: progress,
      content: [{ type: "content", content: { type: "text", text: progress } }],
      ...(messageTaskMetadata ? { task_metadata: messageTaskMetadata } : {}),
    };
    emitToolCallUpdate(
      session,
      toolUseId,
      fields,
      "task_progress",
    );
    return true;
  }

  if (subtype === "task_updated") {
    const fields = taskUpdatedFields(msg);
    if (messageTaskMetadata) {
      fields.task_metadata = { ...(fields.task_metadata ?? {}), ...messageTaskMetadata };
    }
    if (Object.keys(fields).length === 0) {
      return true;
    }
    bridgeLogger.debug({
      target: LOG_TARGETS.APP_TOOL,
      eventName: "task_updated_emitted",
      message: "task update mapped to tool call update",
      outcome: "success",
      sessionId: session.sessionId,
      toolCallId: toolUseId,
      fields: {
        task_id: taskId,
        mapped_status: fields.status,
        has_description: fields.content !== undefined,
        has_error: Boolean(fields.task_metadata?.error),
        is_backgrounded: fields.task_metadata?.is_backgrounded,
      },
    });
    emitToolCallUpdate(session, toolUseId, fields, "task_updated");
    return true;
  }

  const status = typeof msg.status === "string" ? msg.status : "";
  const summary = typeof msg.summary === "string" ? msg.summary : "";
  const finalStatus = status === "completed" ? "completed" : status === "stopped" ? "killed" : "failed";
  const fields: ToolCallUpdateFields = { status: finalStatus };
  if (messageTaskMetadata) {
    fields.task_metadata = messageTaskMetadata;
  }
  if (summary) {
    fields.raw_output = summary;
    fields.content = [{ type: "content", content: { type: "text", text: summary } }];
  }
  emitToolCallUpdate(session, toolUseId, fields, "task_notification");
  if (taskId) {
    session.taskToolUseIds.delete(taskId);
  }
  return true;
}

type ContentBlockLinkage = {
  source: "assistant" | "stream_event" | "user";
  parentToolUseId?: string;
  metadata?: ToolCorrelationMetadata;
};

function logContentBlockLinkage(
  session: SessionState,
  blockType: string,
  toolUseId: string,
  toolName: string | undefined,
  linkage: ContentBlockLinkage | undefined,
): void {
  if (!toolUseId && !linkage?.parentToolUseId) {
    return;
  }
  bridgeLogger.debug({
    target: LOG_TARGETS.APP_TOOL,
    eventName: "sdk_tool_linkage_observed",
    message: "SDK tool linkage observed",
    outcome: linkage?.parentToolUseId ? "child" : "root_or_unknown",
    sessionId: session.sessionId,
    toolCallId: toolUseId || undefined,
    fields: {
      source: linkage?.source,
      block_type: blockType || undefined,
      tool_name: toolName,
      tool_use_id: toolUseId || undefined,
      parent_tool_use_id: linkage?.parentToolUseId,
    },
  });
}

function hideToolUse(session: SessionState, toolUseId: string): void {
  if (toolUseId) {
    session.hiddenToolUseIds.add(toolUseId);
  }
}

function isHiddenToolUse(session: SessionState, toolUseId: string, toolName: string): boolean {
  if (!toolUseId) {
    return false;
  }
  if (isToolSearchToolName(toolName)) {
    hideToolUse(session, toolUseId);
    return true;
  }
  return session.hiddenToolUseIds.has(toolUseId);
}

function isHiddenToolResult(session: SessionState, toolUseId: string, blockType: string): boolean {
  if (!toolUseId) {
    return false;
  }
  if (isToolSearchToolResultType(blockType)) {
    hideToolUse(session, toolUseId);
    return true;
  }
  return session.hiddenToolUseIds.has(toolUseId);
}

export function handleContentBlock(
  session: SessionState,
  block: Record<string, unknown>,
  linkage?: ContentBlockLinkage,
): void {
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
    if (isHiddenToolUse(session, toolUseId, name)) {
      return;
    }
    logContentBlockLinkage(session, blockType, toolUseId, name, linkage);
    emitPlanIfTodoWrite(session, name, input);
    emitToolCall(session, toolUseId, name, input, linkage?.parentToolUseId ?? null, linkage?.metadata);
    return;
  }

  if (TOOL_RESULT_TYPES.has(blockType)) {
    const toolUseId = typeof block.tool_use_id === "string" ? block.tool_use_id : "";
    if (!toolUseId) {
      return;
    }
    if (isHiddenToolResult(session, toolUseId, blockType)) {
      return;
    }
    logContentBlockLinkage(session, blockType, toolUseId, undefined, linkage);
    const isError = Boolean(block.is_error);
    emitToolResultUpdate(session, toolUseId, isError, block.content, block);
  }
}

export function handleStreamEvent(
  session: SessionState,
  event: Record<string, unknown>,
  parentToolUseId?: string,
): void {
  const eventType = typeof event.type === "string" ? event.type : "";

  if (eventType === "content_block_start") {
    if (event.content_block && typeof event.content_block === "object") {
      handleContentBlock(session, event.content_block as Record<string, unknown>, {
        source: "stream_event",
        parentToolUseId,
      });
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
    session.lastAssistantError = parseApiRetryError(assistantError);
  }
  const metadata = sdkCorrelationMetadata(message);

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
      const parentToolUseId =
        typeof message.parent_tool_use_id === "string" ? message.parent_tool_use_id : undefined;
      handleContentBlock(session, blockRecord, { source: "assistant", parentToolUseId, metadata });
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
      const parentToolUseId =
        typeof message.parent_tool_use_id === "string" ? message.parent_tool_use_id : undefined;
      handleContentBlock(session, blockRecord, { source: "user", parentToolUseId });
    }
  }
}

export function handleResultMessage(session: SessionState, message: Record<string, unknown>): void {
  emitFastModeUpdateIfChanged(session, message.fast_mode_state);
  const terminalReason = terminalReasonFromValue(message.terminal_reason);

  const subtype = typeof message.subtype === "string" ? message.subtype : "";
  if (subtype === "success") {
    session.lastAssistantError = undefined;
    finalizeOpenToolCalls(session, "completed");
    writeEvent({
      event: "turn_complete",
      session_id: session.sessionId,
      ...(terminalReason ? { terminal_reason: terminalReason } : {}),
    });
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
  const apiErrorStatus = numberField(message, "api_error_status", "apiErrorStatus");
  writeEvent({
    event: "turn_error",
    session_id: session.sessionId,
    message: errors.length > 0 ? errors.join("\n") : fallback,
    error_kind: errorKind,
    ...(subtype ? { sdk_result_subtype: subtype } : {}),
    ...(assistantError ? { assistant_error: assistantError } : {}),
    ...(apiErrorStatus !== undefined ? { api_error_status: apiErrorStatus } : {}),
    ...(terminalReason ? { terminal_reason: terminalReason } : {}),
  });
  session.lastAssistantError = undefined;
}

function terminalReasonFromValue(value: unknown): TerminalReason | undefined {
  switch (value) {
    case "blocking_limit":
    case "rapid_refill_breaker":
    case "prompt_too_long":
    case "image_error":
    case "model_error":
    case "aborted_streaming":
    case "aborted_tools":
    case "stop_hook_prevented":
    case "hook_stopped":
    case "tool_deferred":
    case "max_turns":
    case "completed":
      return value;
    default:
      return undefined;
  }
}

export function handleSdkMessage(session: SessionState, message: SDKMessage): void {
  const msg = message as unknown as Record<string, unknown>;
  const type = typeof msg.type === "string" ? msg.type : "";
  logSdkMessageOrigin(session, msg);

  if (type === "system") {
    const subtype = typeof msg.subtype === "string" ? msg.subtype : "";
    if (subtype === "commands_changed") {
      updateAvailableCommands(session, "commands_changed", mapSdkSlashCommands(msg.commands));
      return;
    }

    if (subtype === "notification") {
      const text = typeof msg.text === "string" ? msg.text : "";
      emitSystemNoticeUpdate(session, notificationSeverity(msg.priority), text);
      return;
    }

    if (subtype === "mirror_error") {
      const error = typeof msg.error === "string" ? msg.error : "";
      const key = asRecordOrNull(msg.key);
      bridgeLogger.warn({
        target: LOG_TARGETS.APP_SESSION,
        eventName: "sdk_mirror_error_received",
        message: "SDK transcript mirror error received",
        outcome: "failure",
        sessionId: session.sessionId,
        fields: {
          error_message: error || undefined,
          project_key: typeof key?.projectKey === "string" ? key.projectKey : undefined,
          mirror_session_id: typeof key?.sessionId === "string" ? key.sessionId : undefined,
          subpath: typeof key?.subpath === "string" ? key.subpath : undefined,
        },
      });
      emitSystemNoticeUpdate(
        session,
        "warning",
        error ? `Transcript mirror failed: ${error}` : "Transcript mirror failed.",
      );
      return;
    }

    if (subtype === "plugin_install") {
      const status = typeof msg.status === "string" ? msg.status : "";
      const name = typeof msg.name === "string" ? msg.name : "";
      const error = typeof msg.error === "string" ? msg.error : "";
      bridgeLogger.info({
        target: LOG_TARGETS.BRIDGE_SDK,
        eventName: "sdk_plugin_install_received",
        message: "SDK plugin install event received",
        outcome: status === "failed" ? "failure" : status || "observed",
        sessionId: session.sessionId,
        fields: {
          plugin_status: status || undefined,
          plugin_name: name || undefined,
          error_message: error || undefined,
        },
      });
      if (status === "failed") {
        const subject = name ? ` ${name}` : "";
        const suffix = error ? `: ${error}` : ".";
        emitSystemNoticeUpdate(session, "warning", `Plugin install failed${subject}${suffix}`);
      }
      return;
    }

    if (subtype === "permission_denied") {
      bridgeLogger.info({
        target: LOG_TARGETS.BRIDGE_PERMISSION,
        eventName: "sdk_permission_denied_received",
        message: "SDK permission denied event received",
        outcome: "denied",
        sessionId: session.sessionId,
        toolCallId: typeof msg.tool_use_id === "string" ? msg.tool_use_id : undefined,
        fields: {
          tool_name: typeof msg.tool_name === "string" ? msg.tool_name : undefined,
          agent_id: typeof msg.agent_id === "string" ? msg.agent_id : undefined,
          decision_reason_type:
            typeof msg.decision_reason_type === "string" ? msg.decision_reason_type : undefined,
          decision_reason: typeof msg.decision_reason === "string" ? msg.decision_reason : undefined,
          denial_message: typeof msg.message === "string" ? msg.message : undefined,
        },
      });
      return;
    }

    if (subtype === "memory_recall" || subtype === "thinking_tokens") {
      bridgeLogger.debug({
        target: LOG_TARGETS.BRIDGE_SDK,
        eventName: "sdk_system_message_log_only",
        message: "SDK system message handled with log-only policy",
        outcome: "ignored",
        sessionId: session.sessionId,
        fields: {
          sdk_subtype: subtype,
          memory_count: Array.isArray(msg.memories) ? msg.memories.length : undefined,
          estimated_tokens: typeof msg.estimated_tokens === "number" ? msg.estimated_tokens : undefined,
          estimated_tokens_delta:
            typeof msg.estimated_tokens_delta === "number" ? msg.estimated_tokens_delta : undefined,
        },
      });
      return;
    }

    if (subtype === "api_retry") {
      const update = buildApiRetryUpdate(msg);
      if (update) {
        emitSessionUpdate(session.sessionId, update);
      }
      return;
    }

    if (subtype === "session_state_changed") {
      const state = parseRuntimeSessionState(msg.state);
      if (state) {
        emitSessionUpdate(session.sessionId, {
          type: "runtime_session_state_update",
          state,
        });
      }
      return;
    }

    if (subtype === "init") {
      const previousSessionId = session.sessionId;
      const incomingSessionId = typeof msg.session_id === "string" ? msg.session_id : session.sessionId;
      updateSessionId(session, incomingSessionId);
      const modelName = typeof msg.model === "string" ? msg.model : session.model;
      session.model = modelName;
      const currentModelChanged = refreshCurrentModel(session, false);

      const incomingMode = typeof msg.permissionMode === "string" ? toPermissionMode(msg.permissionMode) : null;
      if (incomingMode) {
        session.mode = incomingMode;
      }
      refreshSupportedModesForSession(session);
      emitFastModeUpdateIfChanged(session, msg.fast_mode_state);

      if (!session.connected) {
        emitConnectEvent(session);
      } else if (previousSessionId !== session.sessionId) {
        emitSessionReplacedEvent(session);
      } else {
        if (currentModelChanged) {
          emitCurrentModelUpdate(session);
        }
        if (incomingMode) {
          emitSessionUpdate(session.sessionId, {
            type: "mode_state_update",
            mode: buildModeState(session, incomingMode),
          });
        }
      }

      if (Array.isArray(msg.slash_commands)) {
        updateAvailableCommands(
          session,
          "init_slash_commands",
          mapInitSlashCommands(msg.slash_commands),
        );
      }

      if (session.lastAvailableAgentsSignature === undefined && Array.isArray(msg.agents)) {
        emitAvailableAgentsIfChanged(session, mapAvailableAgentsFromNames(msg.agents));
      }

      void session.query
        .supportedCommands()
        .then((commands) => {
          const mapped = mapSdkSlashCommands(commands);
          updateAvailableCommands(session, "supportedCommands", mapped);
        })
        .catch(() => {
          // Best-effort only; slash commands from init were already emitted.
        });
      refreshAvailableAgents(session);
      for (const settingsError of normalizeSettingsParseErrors(
        msg.settings_errors ?? msg.settingsErrors,
      )) {
        emitSessionUpdate(session.sessionId, {
          type: "settings_parse_error",
          ...settingsError,
        });
      }
      return;
    }

    if (subtype === "status") {
      const mode =
        typeof msg.permissionMode === "string" ? toPermissionMode(msg.permissionMode) : null;
      if (mode) {
        session.mode = mode;
        refreshSupportedModesForSession(session);
        emitSessionUpdate(session.sessionId, { type: "current_mode_update", current_mode_id: mode });
      }
      if (msg.status === "compacting") {
        emitSessionUpdate(session.sessionId, { type: "session_status_update", status: "compacting" });
      } else if (msg.status === "requesting") {
        emitSessionUpdate(session.sessionId, { type: "session_status_update", status: "requesting" });
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

    if (handleTaskSystemMessage(session, subtype, msg)) {
      return;
    }
    bridgeLogger.debug({
      target: LOG_TARGETS.BRIDGE_SDK,
      eventName: "sdk_system_message_unhandled",
      message: "SDK system message ignored by explicit fallback policy",
      outcome: "ignored",
      sessionId: session.sessionId,
      fields: {
        sdk_subtype: subtype || undefined,
      },
    });
    return;
  }

  if (type === "prompt_suggestion") {
    const suggestion = typeof msg.suggestion === "string" ? msg.suggestion.trim() : "";
    if (suggestion) {
      emitSessionUpdate(session.sessionId, { type: "prompt_suggestion_update", suggestion });
    }
    return;
  }

  if (type === "settings_parse_error") {
    for (const settingsError of normalizeSettingsParseErrors(msg)) {
      emitSessionUpdate(session.sessionId, {
        type: "settings_parse_error",
        ...settingsError,
      });
    }
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
      const parentToolUseId =
        typeof msg.parent_tool_use_id === "string" ? msg.parent_tool_use_id : undefined;
      handleStreamEvent(session, msg.event as Record<string, unknown>, parentToolUseId);
    }
    return;
  }

  if (type === "tool_progress") {
    const toolUseId = typeof msg.tool_use_id === "string" ? msg.tool_use_id : "";
    const toolName = typeof msg.tool_name === "string" ? msg.tool_name : "Tool";
    if (isHiddenToolUse(session, toolUseId, toolName)) {
      return;
    }
    bridgeLogger.debug({
      target: LOG_TARGETS.APP_TOOL,
      eventName: "sdk_tool_progress_linkage_observed",
      message: "SDK tool progress linkage observed",
      outcome: typeof msg.parent_tool_use_id === "string" ? "child" : "root_or_unknown",
      sessionId: session.sessionId,
      toolCallId: toolUseId || undefined,
      fields: {
        tool_name: toolName,
        tool_use_id: toolUseId || undefined,
        parent_tool_use_id:
          typeof msg.parent_tool_use_id === "string" ? msg.parent_tool_use_id : undefined,
        task_id: typeof msg.task_id === "string" ? msg.task_id : undefined,
      },
    });
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
        if (session.hiddenToolUseIds.has(toolUseId)) {
          continue;
        }
        emitToolSummaryUpdate(session, toolUseId, summary);
      }
    }
    return;
  }

  if (type === "rate_limit_event") {
    const rateLimitInfo = asRecordOrNull(msg.rate_limit_info);
    const update = buildRateLimitUpdate(msg.rate_limit_info);
    bridgeLogger.debug({
      target: LOG_TARGETS.APP_SESSION,
      eventName: "sdk_rate_limit_event_received",
      message: "SDK rate limit event received",
      outcome: update ? "success" : "dropped",
      sessionId: session.sessionId,
      fields: {
        raw_status: typeof rateLimitInfo?.status === "string" ? rateLimitInfo.status : undefined,
        raw_rate_limit_type:
          typeof rateLimitInfo?.rateLimitType === "string" ? rateLimitInfo.rateLimitType : undefined,
        raw_utilization: numberField(rateLimitInfo ?? {}, "utilization"),
        raw_resets_at: numberField(rateLimitInfo ?? {}, "resetsAt"),
        raw_overage_status:
          typeof rateLimitInfo?.overageStatus === "string" ? rateLimitInfo.overageStatus : undefined,
        raw_overage_resets_at: numberField(rateLimitInfo ?? {}, "overageResetsAt"),
        raw_is_using_overage:
          typeof rateLimitInfo?.isUsingOverage === "boolean" ? rateLimitInfo.isUsingOverage : undefined,
        raw_surpassed_threshold: numberField(rateLimitInfo ?? {}, "surpassedThreshold"),
        parsed_status: update?.status,
        parsed_rate_limit_type: update?.rate_limit_type,
        parsed_utilization: update?.utilization,
        parsed_resets_at: update?.resets_at,
        parsed_overage_status: update?.overage_status,
        parsed_overage_resets_at: update?.overage_resets_at,
        parsed_is_using_overage: update?.is_using_overage,
        parsed_surpassed_threshold: update?.surpassed_threshold,
      },
    });
    bridgeLogger.debug({
      target: LOG_TARGETS.APP_SESSION,
      eventName: "sdk_rate_limit_event_raw",
      message: "SDK rate limit event raw payload",
      outcome: rateLimitInfo ? "success" : "dropped",
      sessionId: session.sessionId,
      fields: {
        raw_rate_limit_info: msg.rate_limit_info,
      },
    });
    if (update) {
      emitSessionUpdate(session.sessionId, update);
    }
    return;
  }

  if (type === "user") {
    handleUserToolResultBlocks(session, msg);

    const toolUseId = typeof msg.parent_tool_use_id === "string" ? msg.parent_tool_use_id : "";
    if (toolUseId && "tool_use_result" in msg) {
      if (session.hiddenToolUseIds.has(toolUseId)) {
        return;
      }
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
