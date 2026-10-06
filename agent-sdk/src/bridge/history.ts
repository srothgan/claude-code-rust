import { messageMetadata } from "./presentation_metadata.js";
import { nativeNotification } from "./notifications.js";
import { getSessionMessages, importSessionToStore } from "@anthropic-ai/claude-agent-sdk";
import type {
  SDKSessionInfo,
  SessionMessage,
  SessionStore,
  SessionStoreEntry,
} from "@anthropic-ai/claude-agent-sdk";
import type {
  SessionListEntry,
  SessionUpdate,
  TaskItem,
  ToolCall,
} from "../types.js";
import { asRecordOrNull } from "./shared.js";
import { applyFieldsToBase, normalizeToolCallUpdateFields, toolAcceptsTaskLifecycle, toolNameFromMeta } from "./tool_calls.js";
import { taskLifecyclePatch, upsertTask } from "./tasks.js";
import { linkTaskToolUse } from "./task_links.js";
import type { SessionState } from "./session_lifecycle.js";
import { taskSystemToolFields } from "./message_handlers.js";
import {
  applyToolNonExecutionMetadata,
  TOOL_RESULT_TYPES,
  buildToolResultFields,
  backgroundToolLaunchTaskIdFromResult,
  createToolCall,
  isToolSearchToolName,
  isToolSearchToolResultType,
  isToolUseBlockType,
  parseToolNonExecutionMetadata,
} from "./tooling.js";

/** Load display history across compactions without changing the model's saved context. */
export async function getSessionTranscriptMessages(
  sessionId: string,
  options: { dir?: string } = {},
): Promise<SessionMessage[]> {
  const entries: SessionStoreEntry[] = [];
  const store: SessionStore = {
    append: async (_key, batch) => { entries.push(...batch); },
    load: async () => {
      const uuids = new Set(entries.map(entry => entry.uuid));
      return entries.map(entry => {
        if (
          entry.type !== "system" || entry.subtype !== "compact_boundary" ||
          typeof entry.logicalParentUuid !== "string" || !uuids.has(entry.logicalParentUuid)
        ) {
          return entry;
        }
        // Preserved messages already occur in the original transcript chain.
        // Reparenting them after the summary would create a cycle across this link.
        const compactMetadata = { ...asRecordOrNull(entry.compactMetadata) };
        delete compactMetadata.preservedMessages;
        delete compactMetadata.preservedSegment;
        return { ...entry, parentUuid: entry.logicalParentUuid, compactMetadata };
      });
    },
  };
  // Import streams the complete file, including history before the SDK's large-file cutoff.
  await importSessionToStore(sessionId, store, { ...options, includeSubagents: false });
  const messages = await getSessionMessages(sessionId, { ...options, includeSystemMessages: true, sessionStore: store });
  const records = new Map(entries.map(entry => [entry.uuid, entry]));
  return messages.map(message => {
    const record = records.get(message.uuid);
    const rawResult = record?.toolUseResult ?? record?.tool_use_result;
    return rawResult === undefined ? message : { ...message, tool_use_result: rawResult };
  });
}

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

function pushResumeTaskSystemUpdate(
  updates: SessionUpdate[],
  tasksById: Map<string, TaskItem>,
  taskToolUseIds: Map<string, string>,
  toolCalls: Map<string, ToolCall>,
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
  const toolUseId = explicitToolUseId ?? taskToolUseIds.get(taskId);
  const tool = toolUseId ? toolCalls.get(toolUseId) : undefined;
  if (tool && toolUseId && toolAcceptsTaskLifecycle(tool)) {
    const fields = taskSystemToolFields(tool, subtype, msg);
    if (!normalizeToolCallUpdateFields(tool, fields ?? {}, subtype as "task_started" | "task_progress" | "task_updated" | "task_notification")) return true;
    if (fields) {
      applyFieldsToBase(tool, fields);
      updates.push({ type: "tool_call_update", tool_call_update: { tool_call_id: toolUseId, fields } });
    }
  }

  const patch = taskLifecyclePatch(tasksById.get(taskId), subtype, msg, toolUseId);
  if (patch) {
    const task = upsertTask({ tasksById, taskOrder: [] }, patch);
    updates.push({ type: "task_state_update", source: "task_lifecycle", tasks: [task], removed_task_ids: [], is_complete_snapshot: false });
  }
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
  updates.push({ type: "tool_call", tool_call: structuredClone(toolCall) });
}

function pushResumeToolResult(
  updates: SessionUpdate[],
  toolCalls: Map<string, ToolCall>,
  hiddenToolUseIds: Set<string>,
  block: Record<string, unknown>,
  nonExecutionByToolUseId: Map<
    string,
    import("../types.js").ToolNonExecutionMetadata
  >,
  sourceMessageUuid: string | undefined,
  rawResult: unknown,
  taskToolUseIds: Map<string, string>,
  tasksById: Map<string, TaskItem>,
): void {
  const toolUseId =
    typeof block.tool_use_id === "string" ? block.tool_use_id : "";
  if (!toolUseId) {
    return;
  }
  const blockType = typeof block.type === "string" ? block.type : "";
  if (
    isToolSearchToolResultType(blockType) ||
    hiddenToolUseIds.has(toolUseId)
  ) {
    hiddenToolUseIds.add(toolUseId);
    return;
  }
  const isError = Boolean(block.is_error);
  const base = toolCalls.get(toolUseId);
  const fields = buildToolResultFields(isError, block.content, base, rawResult ?? block);
  applyToolNonExecutionMetadata(fields, nonExecutionByToolUseId.get(toolUseId));
  if (!normalizeToolCallUpdateFields(base, fields, "result")) return;
  const taskId = !isError ? backgroundToolLaunchTaskIdFromResult(
    toolNameFromMeta(base?.meta) ?? "", rawResult ?? block, block.content,
  ) : undefined;
  if (taskId) {
    taskToolUseIds.set(taskId, toolUseId);
    const patch = taskLifecyclePatch(tasksById.get(taskId), "task_started", {
      task_id: taskId, description: base?.title, is_backgrounded: true,
    }, toolUseId);
    if (patch) updates.push({ type: "task_state_update", source: "task_lifecycle", tasks: [upsertTask({ tasksById, taskOrder: [] }, patch)], removed_task_ids: [], is_complete_snapshot: false });
  }
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
  applyFieldsToBase(base, fields);
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
    ...(nonEmptyTrimmed(info.gitBranch)
      ? { git_branch: info.gitBranch?.trim() }
      : {}),
    ...(nonEmptyTrimmed(info.customTitle)
      ? { custom_title: info.customTitle?.trim() }
      : {}),
    ...(nonEmptyTrimmed(info.firstPrompt)
      ? { first_prompt: info.firstPrompt?.trim() }
      : {}),
  };
}

export function mapSdkSessions(
  infos: SDKSessionInfo[],
  limit = 50,
): SessionListEntry[] {
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

export function mapSessionMessagesToUpdates(
  messages: SessionMessage[],
): SessionUpdate[] {
  const updates: SessionUpdate[] = [];
  const toolCalls = new Map<string, ToolCall>();
  const hiddenToolUseIds = new Set<string>();
  const tasksById = new Map<string, TaskItem>();
  const taskToolUseIds = new Map<string, string>();

  for (const entry of messages) {
    const record = asRecordOrNull(entry);
    const origin = asRecordOrNull(record?.origin);
    // Background-task inputs belong to Claude's context, not the visible user transcript.
    // Peer and scheduled messages carry a subkind and remain eligible for display.
    if (record?.isCompactSummary === true) continue;
    if (
      entry.type === "user" && origin?.kind === "task-notification" &&
      origin.subkind === undefined &&
      (origin.producer === undefined || origin.producer === "session-task")
    ) {
      const notification = resumedTaskNotification(entry.message);
      if (notification) pushResumeTaskSystemUpdate(updates, tasksById, taskToolUseIds, toolCalls, notification);
      continue;
    }
    const fallbackRole = entry.type === "assistant" ? "assistant" : "user";
    const entrySourceMessageUuid =
      typeof entry.uuid === "string" ? entry.uuid : undefined;
    const candidates = messageCandidates(entry.message);
    if (entry.type === "system") {
      for (const message of candidates) {
        if (message.subtype === "notification") {
          const notification = nativeNotification({ ...entry, ...message });
          if (notification) updates.push({ type: "notification_update", notification, replay: true });
          break;
        }
        if (
          pushResumeTaskSystemUpdate(
            updates,
            tasksById,
            taskToolUseIds,
            toolCalls,
            message,
          )
        ) {
          break;
        }
      }
      continue;
    }
    for (const message of candidates) {
      const sourceMessageUuid =
        typeof message.uuid === "string"
          ? message.uuid
          : entrySourceMessageUuid;
      const roleCandidate = message.role;
      const role =
        roleCandidate === "assistant" || roleCandidate === "user"
          ? roleCandidate
          : fallbackRole;
      const parentToolUseId =
        typeof entry.parent_tool_use_id === "string"
          ? entry.parent_tool_use_id
          : typeof message.parent_tool_use_id === "string"
            ? message.parent_tool_use_id
            : null;

      const content = typeof message.content === "string"
        ? [{ type: "text", text: message.content }]
        : Array.isArray(message.content) ? message.content : [];
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
            record?.tool_use_result ?? record?.toolUseResult,
            taskToolUseIds,
            tasksById,
          );
          continue;
        }
        if (blockType === "image") {
          pushResumeTextChunk(updates, role, "[image]", sourceMessageUuid);
        }
      }
    }
    const metadata = messageMetadata({ ...entry, ...asRecordOrNull(entry.message) }, fallbackRole);
    if (metadata) updates.push(metadata);
  }

  return updates;
}

/** Restore runtime correlations from the same chronological updates shown by the UI. */
export function restoreResumedToolState(session: SessionState, updates: SessionUpdate[]): void {
  for (const update of updates) {
    if (update.type === "tool_call") session.toolCalls.set(update.tool_call.tool_call_id, structuredClone(update.tool_call));
    if (update.type === "tool_call_update") {
      const base = session.toolCalls.get(update.tool_call_update.tool_call_id);
      if (base) applyFieldsToBase(base, structuredClone(update.tool_call_update.fields));
    }
    if (update.type === "task_state_update") {
      for (const task of update.tasks) upsertTask(session, structuredClone(task));
      for (const id of update.removed_task_ids) session.tasksById.delete(id);
    }
  }
  for (const task of session.tasksById.values()) {
    const tool = task.source_tool_call_id ? session.toolCalls.get(task.source_tool_call_id) : undefined;
    if (tool && !["completed", "failed", "killed"].includes(tool.status)) linkTaskToolUse(session, task.task_id, tool.tool_call_id);
  }
}

// Literal searches avoid retrying overlapping suffixes when a closing tag is missing.
function notificationTagContent(text: string, name: string): string | undefined {
  const opening = `<${name}>`;
  const start = text.indexOf(opening);
  if (start === -1) return undefined;
  const bodyStart = start + opening.length;
  const end = text.indexOf(`</${name}>`, bodyStart);
  return end === -1 ? undefined : text.slice(bodyStart, end);
}

function resumedTaskNotification(raw: unknown): Record<string, unknown> | undefined {
  for (const message of messageCandidates(raw)) {
    const content = typeof message.content === "string" ? message.content : Array.isArray(message.content)
      ? message.content.map(block => asRecordOrNull(block)?.text ?? "").join("\n") : "";
    const body = notificationTagContent(content, "task-notification");
    if (!body) continue;
    const field = (name: string) => notificationTagContent(body, name)?.trim();
    const taskId = field("task-id");
    const status = field("status");
    if (taskId && (status === "completed" || status === "failed" || status === "stopped")) {
      return { subtype: "task_notification", task_id: taskId, status, tool_use_id: field("tool-use-id"), summary: field("summary"), output_file: field("output-file") };
    }
  }
  return undefined;
}
