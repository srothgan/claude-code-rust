import type {
  Json,
  TaskItem,
  TaskStatus,
  TaskUpdateSource,
  ToolCall,
} from "../types.js";
import type { SessionState } from "./session_lifecycle.js";
import { emitSessionUpdate } from "./events.js";
import { asRecordOrNull } from "./shared.js";
import { linkTaskToolUse, unlinkTaskToolUse } from "./task_links.js";

type TaskPatch = {
  task_id: string;
  run_id?: string;
  parent_task_id?: string;
  resetExecution?: boolean;
  subject?: string;
  description?: string;
  active_form?: string;
  status?: TaskStatus;
  owner?: string;
  blocks?: string[];
  blocked_by?: string[];
  metadata?: Json;
  source_tool_call_id?: string;
  source_tool_call_run_id?: string;
};

const TASK_TOOL_NAMES = new Set([
  "TaskCreate",
  "TaskUpdate",
  "TaskGet",
  "TaskList",
  "TaskOutput",
  "TaskStop",
]);

export type TaskTitleContext = {
  taskSubject?: string;
};

const BACKGROUND_TASK_METADATA_KEY = "sdk_background_task";
// Set by every lifecycle edge (start, progress, update, notification, stop):
// the row is a runtime task the lifecycle owns, never a TaskList plan step,
// and background inventory snapshots can no longer remove it.
const LIFECYCLE_TASK_METADATA_KEY = "sdk_lifecycle_task";
const INTERRUPTED_TERMINAL_STATUS = "interrupted";

export function isTaskToolName(name: string): boolean {
  return TASK_TOOL_NAMES.has(name);
}

export function taskToolTitle(
  name: string,
  input: Record<string, unknown>,
  context: TaskTitleContext = {},
): string | undefined {
  if (name === "TaskCreate") {
    const subject = typeof input.subject === "string" ? input.subject : "";
    return subject ? `Create task: ${subject}` : "Create task";
  }
  if (name === "TaskUpdate") {
    const subject = typeof input.subject === "string" ? input.subject : "";
    const taskId = typeof input.taskId === "string" ? input.taskId : "";
    const label = subject || context.taskSubject || taskId;
    return label ? `Update task: ${label}` : "Update task";
  }
  if (name === "TaskGet") {
    const taskId = typeof input.taskId === "string" ? input.taskId : "";
    return taskId ? `Get task: ${taskId}` : "Get task";
  }
  if (name === "TaskList") {
    return "List tasks";
  }
  if (name === "TaskOutput") {
    const taskId = nonEmptyString(input.task_id) ?? "";
    const label = context.taskSubject || taskId;
    return label ? `Task output: ${label}` : "Task output";
  }
  if (name === "TaskStop") {
    const taskId =
      nonEmptyString(input.task_id) ?? nonEmptyString(input.shell_id) ?? "";
    const label = context.taskSubject || taskId;
    return label ? `Stop task: ${label}` : "Stop task";
  }
  return undefined;
}

function cloneTask(task: TaskItem): TaskItem {
  return {
    ...task,
    blocks: [...task.blocks],
    blocked_by: [...task.blocked_by],
  };
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

function jsonRecord(value: unknown): Record<string, Json> | undefined {
  const json = jsonValue(value);
  return json && typeof json === "object" && !Array.isArray(json)
    ? (json as Record<string, Json>)
    : undefined;
}

function nonEmptyString(value: unknown): string | undefined {
  return typeof value === "string" && value.trim().length > 0
    ? value
    : undefined;
}

function stringArray(value: unknown): string[] {
  if (!Array.isArray(value)) {
    return [];
  }
  return value.filter(
    (entry): entry is string => typeof entry === "string" && entry.length > 0,
  );
}

function uniqueStrings(values: string[]): string[] {
  return [...new Set(values)];
}

function normalizeTaskStatus(value: unknown): TaskStatus | undefined {
  switch (value) {
    case "pending":
      return "pending";
    case "running":
    case "in_progress":
      return "in_progress";
    case "completed":
      return "completed";
    default:
      return undefined;
  }
}

function normalizeLifecycleTaskStatus(value: unknown): TaskStatus | undefined {
  const status = normalizeTaskStatus(value);
  if (status) {
    return status;
  }
  switch (value) {
    case "failed":
    case "killed":
    case "stopped":
      return "completed";
    default:
      return undefined;
  }
}

function toolNameFromToolCall(base?: ToolCall): string {
  const meta = asRecordOrNull(base?.meta);
  const claudeCode = asRecordOrNull(meta?.claudeCode);
  return nonEmptyString(claudeCode?.toolName) ?? "";
}

function inputRecord(base?: ToolCall): Record<string, unknown> {
  return asRecordOrNull(base?.raw_input) ?? {};
}

function extractText(value: unknown): string {
  if (typeof value === "string") {
    return value;
  }
  if (Array.isArray(value)) {
    return value
      .map((entry) => {
        if (typeof entry === "string") {
          return entry;
        }
        const record = asRecordOrNull(entry);
        return typeof record?.text === "string" ? record.text : "";
      })
      .filter((part) => part.length > 0)
      .join("\n");
  }
  const record = asRecordOrNull(value);
  return typeof record?.text === "string" ? record.text : "";
}

function visitCandidate(
  value: unknown,
  records: Record<string, unknown>[],
  depth = 0,
): void {
  if (value === undefined || value === null || depth > 6) {
    return;
  }

  if (typeof value === "string") {
    const trimmed = value.trim();
    if (!(trimmed.startsWith("{") || trimmed.startsWith("["))) {
      return;
    }
    try {
      visitCandidate(JSON.parse(trimmed), records, depth + 1);
    } catch {
      return;
    }
    return;
  }

  if (Array.isArray(value)) {
    for (const entry of value) {
      visitCandidate(entry, records, depth + 1);
    }
    return;
  }

  const record = asRecordOrNull(value);
  if (!record) {
    return;
  }

  records.push(record);
  for (const key of ["result", "data", "content"]) {
    visitCandidate(record[key], records, depth + 1);
  }
  if (typeof record.text === "string") {
    visitCandidate(record.text, records, depth + 1);
  }
}

function resultCandidates(
  rawResult: unknown,
  rawContent: unknown,
): Record<string, unknown>[] {
  const records: Record<string, unknown>[] = [];
  visitCandidate(rawResult, records);
  visitCandidate(rawContent, records);
  const text = extractText(rawContent);
  if (text) {
    visitCandidate(text, records);
  }
  return records;
}

function normalizeFieldKey(key: string): string {
  return key.replace(/[^a-zA-Z0-9]+/g, "").toLowerCase();
}

function humanizeFieldLabel(key: string): string {
  const spaced = key
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .replace(/[_-]+/g, " ")
    .trim()
    .toLowerCase();
  if (!spaced) {
    return "";
  }
  return spaced
    .split(/\s+/)
    .map((word, index) => {
      if (word === "id") {
        return "ID";
      }
      return index === 0
        ? `${word.charAt(0).toUpperCase()}${word.slice(1)}`
        : word;
    })
    .join(" ");
}

function displayScalarValue(value: unknown): string | undefined {
  if (typeof value === "string") {
    const trimmed = value.trim();
    if (!trimmed) {
      return undefined;
    }
    return /^[a-z][a-z0-9]*(?:_[a-z0-9]+)+$/u.test(trimmed)
      ? trimmed.replace(/_/g, " ")
      : trimmed;
  }
  if (typeof value === "boolean") {
    return value ? "yes" : "no";
  }
  if (typeof value === "number" && Number.isFinite(value)) {
    return `${value}`;
  }
  return undefined;
}

function decodeXmlText(value: string): string {
  return value.replace(
    /&(?:#(\d+)|#x([0-9a-fA-F]+)|amp|lt|gt|quot|apos);/g,
    (match, dec, hex) => {
      if (typeof dec === "string" && dec.length > 0) {
        const codePoint = Number.parseInt(dec, 10);
        return Number.isInteger(codePoint) &&
          codePoint >= 0 &&
          codePoint <= 0x10ffff
          ? String.fromCodePoint(codePoint)
          : match;
      }
      if (typeof hex === "string" && hex.length > 0) {
        const codePoint = Number.parseInt(hex, 16);
        return Number.isInteger(codePoint) &&
          codePoint >= 0 &&
          codePoint <= 0x10ffff
          ? String.fromCodePoint(codePoint)
          : match;
      }
      switch (match) {
        case "&amp;":
          return "&";
        case "&lt;":
          return "<";
        case "&gt;":
          return ">";
        case "&quot;":
          return '"';
        case "&apos;":
          return "'";
        default:
          return match;
      }
    },
  );
}

function xmlLeafFields(text: string): Array<[string, string]> {
  const fields: Array<[string, string]> = [];
  if (text.length > 20_000 || !text.includes("<")) {
    return fields;
  }
  const pattern = /<([A-Za-z][\w.-]{0,79})>([\s\S]*?)<\/\1>/gu;
  for (const match of text.matchAll(pattern)) {
    if (fields.length >= 100) {
      break;
    }
    const [, key, rawValue] = match;
    if (
      !key ||
      rawValue === undefined ||
      /<[A-Za-z][\w.-]{0,79}[\s>]/u.test(rawValue)
    ) {
      continue;
    }
    const value = decodeXmlText(rawValue).trim();
    if (value) {
      fields.push([key, value]);
    }
  }
  return fields;
}

function firstTaskRecord(
  candidates: Record<string, unknown>[],
): Record<string, unknown> | undefined {
  for (const candidate of candidates) {
    const nestedTask = asRecordOrNull(candidate.task);
    if (nestedTask && typeof nestedTask.id === "string") {
      return nestedTask;
    }
    if (
      typeof candidate.id === "string" &&
      typeof candidate.subject === "string"
    ) {
      return candidate;
    }
  }
  return undefined;
}

function firstRecordWithTaskProperty(
  candidates: Record<string, unknown>[],
): Record<string, unknown> | undefined {
  return candidates.find((candidate) => Object.hasOwn(candidate, "task"));
}

function firstTaskUpdateOutput(
  candidates: Record<string, unknown>[],
): Record<string, unknown> | undefined {
  return candidates.find(
    (candidate) =>
      typeof candidate.success === "boolean" &&
      typeof candidate.taskId === "string",
  );
}

function firstTaskListOutput(
  candidates: Record<string, unknown>[],
): Record<string, unknown> | undefined {
  return candidates.find((candidate) => Array.isArray(candidate.tasks));
}

function firstTaskStopOutput(
  candidates: Record<string, unknown>[],
): Record<string, unknown> | undefined {
  return candidates.find(
    (candidate) =>
      typeof candidate.message === "string" &&
      typeof candidate.task_id === "string" &&
      typeof candidate.task_type === "string",
  );
}

function taskOutputResultText(
  rawResult: unknown,
  rawContent: unknown,
  rawInput: unknown,
): string {
  const lines: string[] = [];
  const seen = new Set<string>();
  const input = asRecordOrNull(rawInput);

  const markSeen = (key: string, value: string): void => {
    seen.add(`${normalizeFieldKey(key)}\0${value}`);
  };
  const pushField = (key: string, value: unknown): void => {
    const label = humanizeFieldLabel(key);
    const displayValue = displayScalarValue(value);
    if (!label || displayValue === undefined) {
      return;
    }
    const seenKey = `${normalizeFieldKey(key)}\0${displayValue}`;
    if (seen.has(seenKey)) {
      return;
    }
    seen.add(seenKey);
    lines.push(`${label}: ${displayValue}`);
  };

  const inputTaskId = displayScalarValue(input?.task_id);
  if (inputTaskId) {
    markSeen("task_id", inputTaskId);
  }

  const candidates = resultCandidates(rawResult, undefined);
  for (const candidate of candidates) {
    if (Object.hasOwn(candidate, "retrieval_status")) {
      pushField("retrieval_status", candidate.retrieval_status);
    }
    const task = asRecordOrNull(candidate.task);
    if (task) {
      for (const [key, value] of Object.entries(task)) {
        pushField(key, value);
      }
    }
    if (
      !task &&
      (Object.hasOwn(candidate, "task_id") ||
        Object.hasOwn(candidate, "task_type") ||
        Object.hasOwn(candidate, "status"))
    ) {
      for (const [key, value] of Object.entries(candidate)) {
        pushField(key, value);
      }
    }
  }

  const rawText = extractText(rawContent);
  for (const [key, value] of xmlLeafFields(rawText)) {
    pushField(key, value);
  }

  return lines.join("\n") || rawText.trim() || extractText(rawResult).trim();
}

export function upsertTask(session: Pick<SessionState, "tasksById" | "taskOrder">, patch: TaskPatch): TaskItem {
  const previous = session.tasksById.get(patch.task_id);
  const existing = patch.resetExecution ? resetTaskExecution(previous) : previous;
  const task = upsertFromExisting(existing, patch);

  session.tasksById.set(task.task_id, task);
  if (!existing && !session.taskOrder.includes(task.task_id)) {
    session.taskOrder.push(task.task_id);
  }
  return cloneTask(task);
}

function removeTasks(session: SessionState, taskIds: string[]): string[] {
  const removed: string[] = [];
  for (const taskId of uniqueStrings(taskIds)) {
    if (session.tasksById.delete(taskId)) {
      removed.push(taskId);
    } else {
      removed.push(taskId);
    }
    unlinkTaskToolUse(session, taskId);
  }
  if (removed.length > 0) {
    const removedSet = new Set(removed);
    session.taskOrder = session.taskOrder.filter(
      (taskId) => !removedSet.has(taskId),
    );
  }
  return removed;
}

function orderedTasks(session: SessionState): TaskItem[] {
  return session.taskOrder
    .map((taskId) => session.tasksById.get(taskId))
    .filter((task): task is TaskItem => Boolean(task))
    .map(cloneTask);
}

function emitTaskStateUpdate(
  session: SessionState,
  source: TaskUpdateSource,
  tasks: TaskItem[],
  removedTaskIds: string[] = [],
  isCompleteSnapshot = false,
): void {
  emitSessionUpdate(session.sessionId, {
    type: "task_state_update",
    source,
    tasks: tasks.map(cloneTask),
    removed_task_ids: uniqueStrings(removedTaskIds),
    is_complete_snapshot: isCompleteSnapshot,
  });
}

function mergeMetadata(
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

function taskCreatePatch(
  taskRecord: Record<string, unknown>,
  input: Record<string, unknown>,
  toolUseId: string,
): TaskPatch | undefined {
  const taskId = nonEmptyString(taskRecord.id);
  if (!taskId) {
    return undefined;
  }
  const metadata = jsonValue(input.metadata);
  return {
    task_id: taskId,
    subject:
      nonEmptyString(taskRecord.subject) ??
      nonEmptyString(input.subject) ??
      taskId,
    description: nonEmptyString(input.description),
    active_form: nonEmptyString(input.activeForm),
    status: "pending",
    blocks: [],
    blocked_by: [],
    ...(metadata !== undefined ? { metadata } : {}),
    source_tool_call_id: toolUseId,
  };
}

function taskUpdatePatch(
  session: SessionState,
  taskId: string,
  input: Record<string, unknown>,
  output: Record<string, unknown>,
): TaskPatch {
  const existing = session.tasksById.get(taskId);
  const metadata = mergeMetadata(
    existing?.metadata,
    jsonRecord(input.metadata),
  );
  const status =
    normalizeTaskStatus(input.status) ??
    normalizeTaskStatus(asRecordOrNull(output.statusChange)?.to);
  const addBlocks = stringArray(input.addBlocks);
  const addBlockedBy = stringArray(input.addBlockedBy);
  const patch: TaskPatch = {
    task_id: taskId,
    subject: nonEmptyString(input.subject),
    description: nonEmptyString(input.description),
    active_form: nonEmptyString(input.activeForm),
    status,
    owner: nonEmptyString(input.owner),
    blocks:
      addBlocks.length > 0
        ? uniqueStrings([...(existing?.blocks ?? []), ...addBlocks])
        : undefined,
    blocked_by:
      addBlockedBy.length > 0
        ? uniqueStrings([...(existing?.blocked_by ?? []), ...addBlockedBy])
        : undefined,
    metadata,
  };
  if (Object.hasOwn(input, "metadata") && metadata === undefined) {
    patch.metadata = undefined;
  }
  return patch;
}

function taskGetPatch(
  taskRecord: Record<string, unknown>,
  existing?: TaskItem,
): TaskPatch | undefined {
  const taskId = nonEmptyString(taskRecord.id);
  const subject = nonEmptyString(taskRecord.subject);
  const status = normalizeTaskStatus(taskRecord.status);
  if (!taskId || !subject || !status) {
    return undefined;
  }
  return {
    task_id: taskId,
    subject,
    description: nonEmptyString(taskRecord.description),
    active_form: existing?.active_form,
    status,
    owner: existing?.owner,
    blocks: stringArray(taskRecord.blocks),
    blocked_by: stringArray(taskRecord.blockedBy),
    metadata: existing?.metadata,
    source_tool_call_id: existing?.source_tool_call_id,
  };
}

function taskListPatch(
  taskRecord: Record<string, unknown>,
  existing?: TaskItem,
): TaskPatch | undefined {
  const taskId = nonEmptyString(taskRecord.id);
  const subject = nonEmptyString(taskRecord.subject);
  const status = normalizeTaskStatus(taskRecord.status);
  if (!taskId || !subject || !status) {
    return undefined;
  }
  return {
    task_id: taskId,
    subject,
    description: existing?.description,
    active_form: existing?.active_form,
    status,
    owner: nonEmptyString(taskRecord.owner) ?? existing?.owner,
    blocks: existing?.blocks ?? [],
    blocked_by: stringArray(taskRecord.blockedBy),
    metadata: existing?.metadata,
    source_tool_call_id: existing?.source_tool_call_id,
  };
}

function replaceTaskSnapshot(
  session: SessionState,
  patches: TaskPatch[],
): { tasks: TaskItem[]; removedTaskIds: string[] } {
  const previousIds = session.taskOrder;
  const nextIds: string[] = [];
  const nextTasks = new Map<string, TaskItem>();
  const emitted: TaskItem[] = [];

  for (const patch of patches) {
    if (nextTasks.has(patch.task_id)) {
      continue;
    }
    const existing = session.tasksById.get(patch.task_id);
    const task = upsertFromExisting(existing, patch);
    nextTasks.set(task.task_id, task);
    nextIds.push(task.task_id);
    emitted.push(cloneTask(task));
  }

  // TaskList is the plan-task inventory. Runtime tasks are reported by their
  // own lifecycle and must remain in the complete snapshot sent to the UI.
  for (const taskId of previousIds) {
    if (nextTasks.has(taskId)) continue;
    const task = session.tasksById.get(taskId);
    const metadata = asRecordOrNull(task?.metadata);
    if (!task || (metadata?.[LIFECYCLE_TASK_METADATA_KEY] !== true && metadata?.[BACKGROUND_TASK_METADATA_KEY] !== true)) continue;
    nextTasks.set(taskId, task);
    nextIds.push(taskId);
    emitted.push(cloneTask(task));
  }

  const retained = new Set(nextIds);
  const removedTaskIds = previousIds.filter((taskId) => !retained.has(taskId));
  for (const taskId of removedTaskIds) {
    unlinkTaskToolUse(session, taskId);
  }
  session.tasksById = nextTasks;
  session.taskOrder = nextIds;
  return { tasks: emitted, removedTaskIds };
}

function upsertFromExisting(
  existing: TaskItem | undefined,
  patch: TaskPatch,
): TaskItem {
  const task: TaskItem = {
    task_id: patch.task_id,
    run_id: patch.run_id ?? existing?.run_id,
    parent_task_id: existing?.parent_task_id ?? patch.parent_task_id,
    source_tool_call_run_id: patch.source_tool_call_run_id ?? existing?.source_tool_call_run_id,
    subject: patch.subject ?? existing?.subject ?? patch.task_id,
    description: patch.description ?? existing?.description,
    active_form: patch.active_form ?? existing?.active_form,
    status: patch.status ?? existing?.status ?? "pending",
    owner: patch.owner ?? existing?.owner,
    blocks: patch.blocks ?? existing?.blocks ?? [],
    blocked_by: patch.blocked_by ?? existing?.blocked_by ?? [],
    metadata: patch.metadata !== undefined ? patch.metadata : existing?.metadata,
    source_tool_call_id: patch.source_tool_call_id ?? existing?.source_tool_call_id,
  };
  for (const key of ["run_id", "parent_task_id", "source_tool_call_run_id", "description", "active_form", "owner", "metadata", "source_tool_call_id"] as const) {
    if (task[key] === undefined) delete task[key];
  }
  return task;
}

function taskStatusMarker(status: unknown): string {
  switch (status) {
    case "completed":
      return "■";
    case "in_progress":
    case "running":
      return "▣";
    default:
      return "□";
  }
}

function taskRecordLine(record: Record<string, unknown>): string {
  const subject =
    typeof record.subject === "string" && record.subject.trim()
      ? record.subject
      : "Task";
  return `${taskStatusMarker(record.status)} ${subject}`;
}

function taskListWindow(lines: string[]): string[] {
  if (lines.length <= 9) {
    return lines;
  }
  const firstUnfinished = lines.findIndex((line) => !line.startsWith("■ "));
  const anchor = firstUnfinished >= 0 ? firstUnfinished : lines.length - 1;
  const start = Math.min(
    Math.max(anchor - 4, 0),
    Math.max(lines.length - 9, 0),
  );
  const end = Math.min(start + 9, lines.length);
  const visible = lines.slice(start, end);
  if (start > 0) {
    visible[0] = "...";
  }
  if (end < lines.length) {
    visible[visible.length - 1] = "...";
  }
  return visible;
}

export function taskToolResultText(
  toolName: string,
  rawResult: unknown,
  rawContent: unknown,
  rawInput?: unknown,
): string {
  const candidates = resultCandidates(rawResult, rawContent);

  if (toolName === "TaskCreate") {
    return "";
  }

  if (toolName === "TaskUpdate") {
    const output = firstTaskUpdateOutput(candidates);
    if (!output) {
      return "";
    }
    if (output.success !== true) {
      const error =
        typeof output.error === "string" && output.error.trim()
          ? output.error
          : "Task update failed";
      return error;
    }
    return "";
  }

  if (toolName === "TaskGet") {
    const output = firstRecordWithTaskProperty(candidates);
    if (!output) {
      return "";
    }
    const task = asRecordOrNull(output.task);
    if (!task) {
      return "Task not found";
    }
    const lines = [taskRecordLine(task)];
    if (typeof task.description === "string" && task.description.trim()) {
      lines.push(task.description);
    }
    const blockedBy = Array.isArray(task.blockedBy)
      ? task.blockedBy.filter(
          (entry): entry is string => typeof entry === "string",
        )
      : [];
    if (blockedBy.length > 0) {
      lines.push(`Blocked by: ${blockedBy.join(", ")}`);
    }
    return lines.join("\n");
  }

  if (toolName === "TaskList") {
    const output = firstTaskListOutput(candidates);
    if (!output || !Array.isArray(output.tasks)) {
      return "";
    }
    const lines = output.tasks
      .map((entry) => asRecordOrNull(entry))
      .filter((entry): entry is Record<string, unknown> => Boolean(entry))
      .map(taskRecordLine);
    return lines.length > 0 ? taskListWindow(lines).join("\n") : "No tasks";
  }

  if (toolName === "TaskOutput") {
    return taskOutputResultText(rawResult, rawContent, rawInput);
  }

  if (toolName === "TaskStop") {
    const output = firstTaskStopOutput(candidates);
    if (!output) {
      return "";
    }
    const lines = [
      `Message: ${output.message}`,
      `Task ID: ${output.task_id}`,
      `Task type: ${output.task_type}`,
    ];
    const command = nonEmptyString(output.command);
    if (command) {
      lines.push(`Command: ${command}`);
    }
    return lines.join("\n");
  }

  return "";
}

export function taskUpdateSucceeded(
  rawResult: unknown,
  rawContent: unknown,
): boolean | undefined {
  return firstTaskUpdateOutput(resultCandidates(rawResult, rawContent))
    ?.success as boolean | undefined;
}

export function applyTaskToolResult(
  session: SessionState,
  toolUseId: string,
  isError: boolean,
  rawContent: unknown,
  rawResult: unknown,
): void {
  if (isError) {
    return;
  }
  const base = session.toolCalls.get(toolUseId);
  const toolName = toolNameFromToolCall(base);
  if (!isTaskToolName(toolName)) {
    return;
  }
  const input = inputRecord(base);
  const candidates = resultCandidates(rawResult, rawContent);

  if (toolName === "TaskCreate") {
    const taskRecord = firstTaskRecord(candidates);
    const patch = taskRecord
      ? taskCreatePatch(taskRecord, input, toolUseId)
      : undefined;
    if (!patch) {
      return;
    }
    linkTaskToolUse(session, patch.task_id, toolUseId);
    emitTaskStateUpdate(session, "task_create", [upsertTask(session, patch)]);
    return;
  }

  if (toolName === "TaskUpdate") {
    const output = firstTaskUpdateOutput(candidates);
    const taskId =
      nonEmptyString(output?.taskId) ?? nonEmptyString(input.taskId);
    if (!output || !taskId || output.success !== true) {
      return;
    }
    if (input.status === "deleted") {
      const removed = removeTasks(session, [taskId]);
      emitTaskStateUpdate(session, "task_update", [], removed);
      return;
    }
    emitTaskStateUpdate(session, "task_update", [
      upsertTask(session, taskUpdatePatch(session, taskId, input, output)),
    ]);
    return;
  }

  if (toolName === "TaskGet") {
    const output = firstRecordWithTaskProperty(candidates);
    const taskId = nonEmptyString(input.taskId);
    if (!output) {
      return;
    }
    const taskRecord = asRecordOrNull(output.task);
    if (!taskRecord) {
      if (taskId) {
        emitTaskStateUpdate(
          session,
          "task_get",
          [],
          removeTasks(session, [taskId]),
        );
      }
      return;
    }
    const patch = taskGetPatch(
      taskRecord,
      session.tasksById.get(nonEmptyString(taskRecord.id) ?? ""),
    );
    if (patch) {
      emitTaskStateUpdate(session, "task_get", [upsertTask(session, patch)]);
    }
    return;
  }

  if (toolName === "TaskList") {
    const output = firstTaskListOutput(candidates);
    if (!output || !Array.isArray(output.tasks)) {
      return;
    }
    const patches = output.tasks
      .map((entry) => {
        const record = asRecordOrNull(entry);
        const taskId = nonEmptyString(record?.id);
        return record
          ? taskListPatch(
              record,
              taskId ? session.tasksById.get(taskId) : undefined,
            )
          : undefined;
      })
      .filter((patch): patch is TaskPatch => Boolean(patch));
    const snapshot = replaceTaskSnapshot(session, patches);
    emitTaskStateUpdate(
      session,
      "task_list",
      snapshot.tasks,
      snapshot.removedTaskIds,
      true,
    );
    return;
  }

  if (toolName === "TaskOutput") {
    return;
  }

  if (toolName === "TaskStop") {
    const output = firstTaskStopOutput(candidates);
    if (!output) {
      return;
    }
    const taskId = nonEmptyString(output.task_id);
    if (!taskId) {
      return;
    }
    const existing = session.tasksById.get(taskId);
    const sourceToolCallId =
      existing?.source_tool_call_id ?? session.taskToolUseIds.get(taskId);
    if (!existing && !sourceToolCallId) {
      return;
    }
    const metadata = mergeMetadata(existing?.metadata, {
      [LIFECYCLE_TASK_METADATA_KEY]: true,
      [BACKGROUND_TASK_METADATA_KEY]: null,
      terminal_status: "stopped",
      task_type: output.task_type as Json,
      ...(typeof output.command === "string"
        ? { command: output.command }
        : {}),
    });
    emitTaskStateUpdate(session, "task_lifecycle", [
      upsertTask(session, {
        task_id: taskId,
        subject:
          existing?.subject ??
          nonEmptyString(output.command) ??
          nonEmptyString(output.task_type) ??
          taskId,
        status: "completed",
        metadata,
        source_tool_call_id: sourceToolCallId,
      }),
    ]);
    unlinkTaskToolUse(session, taskId);
  }
}

function lifecycleTaskStatus(
  subtype: string,
  msg: Record<string, unknown>,
): TaskStatus | undefined {
  const patch = asRecordOrNull(msg.patch);
  const explicit =
    normalizeLifecycleTaskStatus(msg.status) ??
    normalizeLifecycleTaskStatus(patch?.status);
  if (explicit) {
    return explicit;
  }
  if (subtype === "task_started" || subtype === "task_progress") {
    return "in_progress";
  }
  return undefined;
}

function lifecycleMetadata(
  msg: Record<string, unknown>,
): Record<string, Json> | undefined {
  const patch = asRecordOrNull(msg.patch) ?? undefined;
  const metadata: Record<string, Json> = {};
  const copyValue = (
    from: Record<string, unknown> | undefined,
    key: string,
    outKey = key,
  ): void => {
    if (!from || !Object.hasOwn(from, key)) {
      return;
    }
    const value = jsonValue(from[key]);
    if (value !== undefined) {
      metadata[outKey] = value;
    }
  };

  for (const key of [
    "error",
    "is_backgrounded",
    "spawn_depth",
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
    "blocked",
    "parent_agent_id",
    "ambient",
  ]) {
    copyValue(msg, key);
    copyValue(patch, key);
  }
  const terminalStatus =
    nonEmptyString(msg.status) ?? nonEmptyString(patch?.status);
  if (
    terminalStatus === "completed" ||
    terminalStatus === "failed" ||
    terminalStatus === "killed" ||
    terminalStatus === "stopped"
  ) {
    metadata.terminal_status = terminalStatus;
  }
  const terminalReason =
    nonEmptyString(msg.reason) ?? nonEmptyString(patch?.reason);
  if (terminalReason) {
    metadata.terminal_reason = terminalReason;
  }
  return Object.keys(metadata).length > 0 ? metadata : undefined;
}

/** SDK run IDs order only within one task, using plain string comparison. */
function admitTaskRun(
  existing: TaskItem | undefined,
  subtype: string,
  msg: Record<string, unknown>,
  removedRun?: string,
): { runId?: string; reset: boolean } | undefined {
  const runId = nonEmptyString(msg.run_id);
  const invocation = nonEmptyString(msg.tool_use_id);
  const changedInvocation = Boolean(invocation && existing?.source_tool_call_id && invocation !== existing.source_tool_call_id);
  if (!existing && removedRun && (!runId || runId < removedRun)) return undefined;
  if (existing?.run_id) {
    if (runId && runId < existing.run_id) return undefined;
    if (!runId && (!invocation || invocation !== existing.source_tool_call_id || existing.source_tool_call_run_id !== existing.run_id)) return undefined;
    return { runId: runId ?? existing.run_id, reset: runId ? runId > existing.run_id : changedInvocation };
  }
  return { runId, reset: Boolean(existing && (changedInvocation || (subtype === "task_started" && msg.synthesized_launch !== true && existing.status === "completed"))) };
}

/** The invocation's provenance survives a resume that has no new launch. */
function taskInvocationRun(existing: TaskItem | undefined, admission: { runId?: string; reset: boolean }, invocation?: string): string | undefined {
  if (invocation && invocation !== existing?.source_tool_call_id) return admission.runId;
  if (!admission.reset && !existing?.run_id && existing?.source_tool_call_id) return admission.runId;
  return existing?.source_tool_call_run_id;
}

function resetTaskExecution(task: TaskItem | undefined): TaskItem | undefined {
  if (!task) return undefined;
  const metadata = { ...asRecordOrNull(task.metadata) };
  for (const key of ["error", "summary", "terminal_status", "terminal_reason", "end_time", "total_paused_ms", "subagent_retry", "output_file", "blocked", "is_backgrounded"]) delete metadata[key];
  return { ...task, status: "in_progress", description: undefined, active_form: undefined, metadata: metadata as Json };
}

export function applyTaskLifecycleState(
  session: SessionState,
  subtype: string,
  msg: Record<string, unknown>,
): boolean {
  const taskId = nonEmptyString(msg.task_id);
  if (!taskId) {
    return false;
  }
  const task = taskLifecyclePatch(session.tasksById.get(taskId), subtype, msg, session.taskToolUseIds.get(taskId), session.removedTaskRuns?.get(taskId));
  if (!task) return false;
  if (task.resetExecution) unlinkTaskToolUse(session, taskId);
  const explicitToolUseId = nonEmptyString(msg.tool_use_id);
  if (explicitToolUseId) {
    linkTaskToolUse(session, taskId, explicitToolUseId);
  }

  session.removedTaskRuns?.delete(taskId);
  emitTaskStateUpdate(session, "task_lifecycle", [upsertTask(session, task)]);
  return true;
}

export function taskLifecyclePatch(
  existing: TaskItem | undefined,
  subtype: string,
  msg: Record<string, unknown>,
  sourceToolCallId?: string,
  removedRun?: string,
): TaskPatch | undefined {
  const taskId = nonEmptyString(msg.task_id);
  if (!taskId) return undefined;
  const admission = admitTaskRun(existing, subtype, msg, removedRun);
  if (!admission) return undefined;
  const invocationRun = taskInvocationRun(existing, admission, nonEmptyString(msg.tool_use_id));
  if (admission.reset) existing = resetTaskExecution(existing);
  const patch = asRecordOrNull(msg.patch);
  const status = lifecycleTaskStatus(subtype, msg);
  // Progress and updates can be queued behind the outcome. Only a new start
  // or another outcome may change a settled row.
  // A row settled only by our own inference (terminal_status "interrupted")
  // is the exception: live evidence for it revives it.
  const revived = isInterruptedRow(existing) && status === "in_progress";
  if (existing?.status === "completed" && !revived && status !== "completed" && (existing.run_id || subtype !== "task_started" || msg.synthesized_launch === true)) {
    return undefined;
  }
  const description =
    nonEmptyString(patch?.description) ??
    nonEmptyString(msg.description) ??
    nonEmptyString(msg.summary);
  const activeForm = nonEmptyString(patch?.activeForm);
  const subject =
    nonEmptyString(patch?.subject) ??
    nonEmptyString(msg.subject) ??
    existing?.subject ??
    nonEmptyString(msg.workflow_name) ??
    nonEmptyString(msg.task_description) ??
    description ??
    taskId;
  // SDK 0.3.288 sends background snapshots before or after lifecycle edges.
  // Once an edge owns a row, the inventory can no longer remove it.
  const existingMetadata = asRecordOrNull(existing?.metadata);
  const fields: Record<string, Json> = {
    ...lifecycleMetadata(msg),
    ...(revived ? { terminal_status: null } : {}),
    ...(existingMetadata?.[LIFECYCLE_TASK_METADATA_KEY] === true ? {} : { [LIFECYCLE_TASK_METADATA_KEY]: true }),
    ...(existingMetadata?.[BACKGROUND_TASK_METADATA_KEY] === true ? { [BACKGROUND_TASK_METADATA_KEY]: null, is_backgrounded: true } : {}),
  };
  const metadata = mergeMetadata(existing?.metadata, Object.keys(fields).length > 0 ? fields : undefined);

  if (
    !status &&
    !description &&
    !activeForm &&
    metadata === existing?.metadata &&
    !sourceToolCallId
  ) {
    return;
  }

  return {
      task_id: taskId,
      run_id: admission.runId,
      parent_task_id: nonEmptyString(msg.parent_task_id) ?? nonEmptyString(patch?.parent_task_id),
      resetExecution: admission.reset,
      source_tool_call_run_id: invocationRun,
      subject,
      description,
      active_form: activeForm,
      status,
      metadata,
      source_tool_call_id: nonEmptyString(msg.tool_use_id) ?? existing?.source_tool_call_id ?? sourceToolCallId,
    };
}

function isInterruptedRow(task: TaskItem | undefined): boolean {
  return (
    task?.status === "completed" &&
    asRecordOrNull(task.metadata)?.terminal_status === INTERRUPTED_TERMINAL_STATUS
  );
}

/**
 * A row that was running but that no live process reports any more. The wire
 * has no "unknown" status, so it settles as `completed` carrying
 * `terminal_status: "interrupted"`: not running, with no outcome claimed. A
 * later real outcome still replaces it.
 */
export function interruptedTaskPatch(task: TaskItem): TaskPatch {
  return {
    task_id: task.task_id,
    status: "completed",
    metadata: mergeMetadata(task.metadata, { terminal_status: INTERRUPTED_TERMINAL_STATUS }),
  };
}

export function applyBackgroundTasksChanged(
  session: SessionState,
  msg: Record<string, unknown>,
): void {
  const rawTasks = Array.isArray(msg.tasks) ? msg.tasks : [];
  const nextTasks = rawTasks
    .map((entry): TaskPatch | undefined => {
      const task = asRecordOrNull(entry);
      const taskId = nonEmptyString(task?.task_id);
      if (!task || !taskId) {
        return undefined;
      }
      const taskType = nonEmptyString(task.task_type);
      const description = nonEmptyString(task.description);
      const ambient = typeof task.ambient === "boolean" ? task.ambient : undefined;
      const previous = session.tasksById.get(taskId);
      const admission = admitTaskRun(previous, "background_tasks_changed", task, session.removedTaskRuns?.get(taskId));
      if (!admission) return previous ? { ...previous } : undefined;
      const existing = admission.reset ? resetTaskExecution(previous) : previous;
      const inventoryOnly = !existing || asRecordOrNull(existing.metadata)?.[BACKGROUND_TASK_METADATA_KEY] === true;
      // Listed means running in the background, whichever edge created the row.
      // The SDK replaces the set with every live task, so a relisted row that
      // we only inferred to be interrupted is live again. A recorded outcome
      // is never undone by a listing.
      const revived = isInterruptedRow(existing);
      const listedByLifecycle = !inventoryOnly && (existing?.status === "in_progress" || revived);
      return {
        task_id: taskId,
        run_id: admission.runId,
        parent_task_id: nonEmptyString(task.parent_task_id),
        resetExecution: admission.reset,
        source_tool_call_run_id: taskInvocationRun(previous, admission),
        subject: description ?? taskType ?? taskId,
        description,
        status: revived ? "in_progress" : existing?.status ?? "in_progress",
        metadata: mergeMetadata(existing?.metadata, {
          ...(revived ? { terminal_status: null } : {}),
          ...(inventoryOnly ? { [BACKGROUND_TASK_METADATA_KEY]: true } : {}),
          ...(taskType ? { task_type: taskType } : {}),
          ...(nonEmptyString(task.subagent_type) ? { subagent_type: task.subagent_type as string } : {}),
          ...(ambient !== undefined ? { ambient } : {}),
          ...(listedByLifecycle ? { is_backgrounded: true } : {}),
        }),
      };
    })
    .filter((task): task is TaskPatch => Boolean(task));

  // A stale listed run is evidence of presence, not evidence that the newer
  // execution disappeared. Empty snapshots retain process-local semantics.
  const nextIds = new Set(rawTasks.flatMap(entry => {
    const id = nonEmptyString(asRecordOrNull(entry)?.task_id);
    return id ? [id] : [];
  }));
  // The snapshot only says what is still in the background, so it removes
  // only rows it alone owns. A lifecycle-owned row stays until its own outcome
  // settles it; one that was listed and is still running when the snapshot no
  // longer lists it is the missed-notification case the snapshot exists for,
  // so it stops counting as running without claiming an outcome.
  const removedTaskIds: string[] = [];
  const unlisted: TaskPatch[] = [];
  for (const taskId of session.taskOrder) {
    if (nextIds.has(taskId)) continue;
    const task = session.tasksById.get(taskId);
    const metadata = asRecordOrNull(task?.metadata);
    if (metadata?.[BACKGROUND_TASK_METADATA_KEY] === true) {
      if (task?.run_id) (session.removedTaskRuns ??= new Map()).set(taskId, task.run_id);
      removedTaskIds.push(taskId);
    } else if (task?.status === "in_progress" && metadata?.[LIFECYCLE_TASK_METADATA_KEY] === true && metadata.is_backgrounded === true) {
      unlisted.push(interruptedTaskPatch(task));
    }
  }
  const tasks = [...nextTasks, ...unlisted].map((task) => {
    if (task.resetExecution) unlinkTaskToolUse(session, task.task_id);
    session.removedTaskRuns?.delete(task.task_id);
    return upsertTask(session, task);
  });
  const removed = removeTasks(session, removedTaskIds);
  emitTaskStateUpdate(session, "background_tasks", tasks, removed, true);
}

export function currentTaskSnapshot(session: SessionState): TaskItem[] {
  return orderedTasks(session);
}
