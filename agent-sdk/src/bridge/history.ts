import type { SDKSessionInfo, SessionMessage } from "@anthropic-ai/claude-agent-sdk";
import type { Json, SessionListEntry, SessionUpdate, TaskItem, TaskStatus, ToolCall } from "../types.js";
import { asRecordOrNull } from "./shared.js";
import {
  applyToolNonExecutionMetadata,
  TOOL_RESULT_TYPES,
  buildToolResultFields,
  createToolCall,
  isToolSearchToolName,
  isToolSearchToolResultType,
  isToolUseBlockType,
  parseToolNonExecutionMetadata,
} from "./tooling.js";

function nonEmptyTrimmed(value: unknown): string | undefined {
  if (typeof value !== "string") {
    return undefined;
  }
  const trimmed = value.trim();
  return trimmed.length > 0 ? trimmed : undefined;
}

function messageCandidates(raw: unknown): Record<string, unknown>[] {
  const candidates: Record<string, unknown>[] = [];
  const topLevel = asRecordOrNull(raw);
  if (topLevel) {
    candidates.push(topLevel);
    const nested = asRecordOrNull(topLevel.message);
    if (nested) {
      candidates.push(nested);
    }
  }
  return candidates;
}

function jsonValue(value: unknown): Json | undefined {
  if (value === undefined) {
    return undefined;
  }
  try {
    const text = JSON.stringify(value);
    if (text === undefined) {
      return undefined;
    }
    return JSON.parse(text) as Json;
  } catch {
    return undefined;
  }
}

function mergeTaskMetadata(
  existing: Json | undefined,
  patch: Record<string, Json> | undefined,
): Json | undefined {
  if (!patch) {
    return existing;
  }
  const existingRecord =
    existing && typeof existing === "object" && !Array.isArray(existing)
      ? { ...(existing as Record<string, Json>) }
      : {};
  for (const [key, value] of Object.entries(patch)) {
    if (value === null) {
      delete existingRecord[key];
    } else {
      existingRecord[key] = value;
    }
  }
  return Object.keys(existingRecord).length > 0 ? existingRecord : null;
}

function normalizeLifecycleTaskStatus(value: unknown): TaskStatus | undefined {
  switch (value) {
    case "pending":
      return "pending";
    case "running":
    case "in_progress":
      return "in_progress";
    case "completed":
    case "failed":
    case "killed":
    case "stopped":
      return "completed";
    default:
      return undefined;
  }
}

function taskSystemMetadata(
  msg: Record<string, unknown>,
  patch: Record<string, unknown> | undefined,
): Record<string, Json> | undefined {
  const metadata: Record<string, Json> = {};
  const copyValue = (from: Record<string, unknown> | undefined, key: string): void => {
    if (!from || !Object.hasOwn(from, key)) {
      return;
    }
    const value = jsonValue(from[key]);
    if (value !== undefined) {
      metadata[key] = value;
    }
  };

  for (const key of [
    "error",
    "is_backgrounded",
    "request_id",
    "subagent_type",
    "task_description",
    "task_type",
    "workflow_name",
    "prompt",
    "output_file",
    "summary",
    "end_time",
    "total_paused_ms",
  ]) {
    copyValue(msg, key);
    copyValue(patch, key);
  }
  const terminalStatus = nonEmptyTrimmed(msg.status) ?? nonEmptyTrimmed(patch?.status);
  if (
    terminalStatus === "completed" ||
    terminalStatus === "failed" ||
    terminalStatus === "killed" ||
    terminalStatus === "stopped"
  ) {
    metadata.terminal_status = terminalStatus;
  }
  return Object.keys(metadata).length > 0 ? metadata : undefined;
}

function pushResumeTaskSystemUpdate(
  updates: SessionUpdate[],
  tasksById: Map<string, TaskItem>,
  taskToolUseIds: Map<string, string>,
  msg: Record<string, unknown>,
): boolean {
  const subtype = nonEmptyTrimmed(msg.subtype);
  if (
    subtype !== "task_started" &&
    subtype !== "task_progress" &&
    subtype !== "task_updated" &&
    subtype !== "task_notification"
  ) {
    return false;
  }

  const taskId = nonEmptyTrimmed(msg.task_id);
  if (!taskId) {
    return true;
  }
  const explicitToolUseId = nonEmptyTrimmed(msg.tool_use_id);
  if (explicitToolUseId) {
    taskToolUseIds.set(taskId, explicitToolUseId);
  }

  const existing = tasksById.get(taskId);
  const patch = asRecordOrNull(msg.patch) ?? undefined;
  const status =
    normalizeLifecycleTaskStatus(msg.status) ??
    normalizeLifecycleTaskStatus(patch?.status) ??
    (subtype === "task_started" || subtype === "task_progress" ? "in_progress" : undefined);
  const description =
    nonEmptyTrimmed(patch?.description) ??
    nonEmptyTrimmed(msg.description) ??
    nonEmptyTrimmed(msg.summary);
  const activeForm = nonEmptyTrimmed(patch?.activeForm) ?? nonEmptyTrimmed(patch?.active_form);
  const subject =
    nonEmptyTrimmed(patch?.subject) ??
    nonEmptyTrimmed(msg.subject) ??
    existing?.subject ??
    nonEmptyTrimmed(msg.workflow_name) ??
    nonEmptyTrimmed(msg.task_description) ??
    description ??
    taskId;
  const metadata = mergeTaskMetadata(existing?.metadata, taskSystemMetadata(msg, patch));
  const sourceToolCallId = taskToolUseIds.get(taskId) ?? existing?.source_tool_call_id;

  const task: TaskItem = {
    task_id: taskId,
    subject,
    ...(description !== undefined ? { description } : existing?.description !== undefined ? { description: existing.description } : {}),
    ...(activeForm !== undefined ? { active_form: activeForm } : existing?.active_form !== undefined ? { active_form: existing.active_form } : {}),
    status: status ?? existing?.status ?? "pending",
    ...(existing?.owner !== undefined ? { owner: existing.owner } : {}),
    blocks: existing ? [...existing.blocks] : [],
    blocked_by: existing ? [...existing.blocked_by] : [],
    ...(metadata !== undefined ? { metadata } : {}),
    ...(sourceToolCallId !== undefined ? { source_tool_call_id: sourceToolCallId } : {}),
  };
  tasksById.set(taskId, task);
  updates.push({
    type: "task_state_update",
    source: "task_lifecycle",
    tasks: [task],
    removed_task_ids: [],
    is_complete_snapshot: false,
  });
  return true;
}

function pushResumeTextChunk(
  updates: SessionUpdate[],
  role: "user" | "assistant",
  text: string,
  sourceMessageUuid?: string,
): void {
  if (!text.trim()) {
    return;
  }
  if (role === "assistant") {
    updates.push({
      type: "agent_message_chunk",
      content: { type: "text", text },
      ...(sourceMessageUuid ? { source_message_uuid: sourceMessageUuid } : {}),
    });
    return;
  }
  updates.push({
    type: "user_message_chunk",
    content: { type: "text", text },
    ...(sourceMessageUuid ? { source_message_uuid: sourceMessageUuid } : {}),
  });
}

function pushResumeToolUse(
  updates: SessionUpdate[],
  toolCalls: Map<string, ToolCall>,
  hiddenToolUseIds: Set<string>,
  block: Record<string, unknown>,
  parentToolUseId: string | null,
  sourceMessageUuid?: string,
): void {
  const toolUseId = typeof block.id === "string" ? block.id : "";
  if (!toolUseId) {
    return;
  }
  const name = typeof block.name === "string" ? block.name : "Tool";
  const input = asRecordOrNull(block.input) ?? {};

  if (isToolSearchToolName(name)) {
    hiddenToolUseIds.add(toolUseId);
    return;
  }

  const toolCall = createToolCall(toolUseId, name, input, parentToolUseId);
  toolCall.status = "in_progress";
  if (sourceMessageUuid) {
    toolCall.source_message_uuid = sourceMessageUuid;
  }
  toolCalls.set(toolUseId, toolCall);
  updates.push({ type: "tool_call", tool_call: toolCall });
}

function pushResumeToolResult(
  updates: SessionUpdate[],
  toolCalls: Map<string, ToolCall>,
  hiddenToolUseIds: Set<string>,
  block: Record<string, unknown>,
  nonExecutionByToolUseId: Map<string, import("../types.js").ToolNonExecutionMetadata>,
  sourceMessageUuid?: string,
): void {
  const toolUseId = typeof block.tool_use_id === "string" ? block.tool_use_id : "";
  if (!toolUseId) {
    return;
  }
  const blockType = typeof block.type === "string" ? block.type : "";
  if (isToolSearchToolResultType(blockType) || hiddenToolUseIds.has(toolUseId)) {
    hiddenToolUseIds.add(toolUseId);
    return;
  }
  const isError = Boolean(block.is_error);
  const base = toolCalls.get(toolUseId);
  const fields = buildToolResultFields(isError, block.content, base, block);
  applyToolNonExecutionMetadata(fields, nonExecutionByToolUseId.get(toolUseId));
  updates.push({
    type: "tool_call_update",
    tool_call_update: {
      tool_call_id: toolUseId,
      ...(sourceMessageUuid ? { source_message_uuid: sourceMessageUuid } : {}),
      fields,
    },
  });

  if (!base) {
    return;
  }
  base.status = fields.status ?? base.status;
  if (fields.raw_output) {
    base.raw_output = fields.raw_output;
  }
  if (fields.content) {
    base.content = fields.content;
  }
  if (fields.output_metadata) {
    base.output_metadata = fields.output_metadata;
  }
}

function summaryFromSession(info: SDKSessionInfo): string {
  return (
    nonEmptyTrimmed(info.summary) ??
    nonEmptyTrimmed(info.customTitle) ??
    nonEmptyTrimmed(info.firstPrompt) ??
    info.sessionId
  );
}

export function mapSdkSessionInfo(info: SDKSessionInfo): SessionListEntry {
  return {
    session_id: info.sessionId,
    summary: summaryFromSession(info),
    last_modified_ms: info.lastModified,
    file_size_bytes: info.fileSize ?? 0,
    ...(nonEmptyTrimmed(info.cwd) ? { cwd: info.cwd?.trim() } : {}),
    ...(nonEmptyTrimmed(info.gitBranch) ? { git_branch: info.gitBranch?.trim() } : {}),
    ...(nonEmptyTrimmed(info.customTitle) ? { custom_title: info.customTitle?.trim() } : {}),
    ...(nonEmptyTrimmed(info.firstPrompt) ? { first_prompt: info.firstPrompt?.trim() } : {}),
  };
}

export function mapSdkSessions(infos: SDKSessionInfo[], limit = 50): SessionListEntry[] {
  const sorted = [...infos].sort((a, b) => b.lastModified - a.lastModified);
  const entries: SessionListEntry[] = [];
  const seen = new Set<string>();
  for (const info of sorted) {
    if (!info.sessionId || seen.has(info.sessionId)) {
      continue;
    }
    seen.add(info.sessionId);
    entries.push(mapSdkSessionInfo(info));
    if (entries.length >= limit) {
      break;
    }
  }
  return entries;
}

export function mapSessionMessagesToUpdates(messages: SessionMessage[]): SessionUpdate[] {
  const updates: SessionUpdate[] = [];
  const toolCalls = new Map<string, ToolCall>();
  const hiddenToolUseIds = new Set<string>();
  const tasksById = new Map<string, TaskItem>();
  const taskToolUseIds = new Map<string, string>();

  for (const entry of messages) {
    const fallbackRole = entry.type === "assistant" ? "assistant" : "user";
    const entrySourceMessageUuid = typeof entry.uuid === "string" ? entry.uuid : undefined;
    const candidates = messageCandidates(entry.message);
    if (entry.type === "system") {
      for (const message of candidates) {
        if (pushResumeTaskSystemUpdate(updates, tasksById, taskToolUseIds, message)) {
          break;
        }
      }
      continue;
    }
    for (const message of candidates) {
      const sourceMessageUuid =
        typeof message.uuid === "string" ? message.uuid : entrySourceMessageUuid;
      const roleCandidate = message.role;
      const role = roleCandidate === "assistant" || roleCandidate === "user" ? roleCandidate : fallbackRole;
      const parentToolUseId =
        typeof entry.parent_tool_use_id === "string"
          ? entry.parent_tool_use_id
          : typeof message.parent_tool_use_id === "string"
            ? message.parent_tool_use_id
            : null;

      const content = Array.isArray(message.content) ? message.content : [];
      const nonExecutionByToolUseId = parseToolNonExecutionMetadata(
        Object.hasOwn(entry, "tool_result_meta")
          ? (entry as unknown as Record<string, unknown>).tool_result_meta
          : message.tool_result_meta,
      );
      for (const item of content) {
        const block = asRecordOrNull(item);
        if (!block) {
          continue;
        }
        const blockType = typeof block.type === "string" ? block.type : "";
        if (blockType === "thinking") {
          continue;
        }
        if (blockType === "text" && typeof block.text === "string") {
          pushResumeTextChunk(updates, role, block.text, sourceMessageUuid);
          continue;
        }
        if (isToolUseBlockType(blockType) && role === "assistant") {
          pushResumeToolUse(
            updates,
            toolCalls,
            hiddenToolUseIds,
            block,
            parentToolUseId,
            sourceMessageUuid,
          );
          continue;
        }
        if (TOOL_RESULT_TYPES.has(blockType)) {
          pushResumeToolResult(
            updates,
            toolCalls,
            hiddenToolUseIds,
            block,
            nonExecutionByToolUseId,
            sourceMessageUuid,
          );
          continue;
        }
        if (blockType === "image") {
          pushResumeTextChunk(updates, role, "[image]", sourceMessageUuid);
        }
      }
    }
  }

  return updates;
}
