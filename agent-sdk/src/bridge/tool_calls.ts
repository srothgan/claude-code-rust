import type { PlanEntry, ToolCall, ToolCallUpdateFields } from "../types.js";
import { emitSessionUpdate } from "./events.js";
import { bridgeLogger, LOG_TARGETS } from "./logger.js";
import type { SessionState } from "./session_lifecycle.js";
import { buildToolResultFields, createToolCall } from "./tooling.js";

type ToolUpdateKind =
  | "initial"
  | "refresh"
  | "progress"
  | "result"
  | "summary"
  | "status"
  | "finalize"
  | "task_started"
  | "task_progress"
  | "task_notification";

function jsonSize(value: unknown): number | undefined {
  if (value === undefined) {
    return undefined;
  }
  try {
    return Buffer.byteLength(JSON.stringify(value));
  } catch {
    return undefined;
  }
}

function toolNameFromMeta(meta: ToolCall["meta"] | undefined): string | undefined {
  if (!meta || typeof meta !== "object") {
    return undefined;
  }
  const claudeCode =
    "claudeCode" in meta && meta.claudeCode && typeof meta.claudeCode === "object" ? meta.claudeCode : undefined;
  const toolName =
    claudeCode && "toolName" in claudeCode && typeof claudeCode.toolName === "string" ? claudeCode.toolName : "";
  return toolName || undefined;
}

function toolName(base: ToolCall | undefined, fields?: ToolCallUpdateFields): string | undefined {
  const fieldMeta = fields?.meta;
  if (fieldMeta && typeof fieldMeta === "object") {
    const fromFields = toolNameFromMeta(fieldMeta);
    if (fromFields) {
      return fromFields;
    }
  }
  return toolNameFromMeta(base?.meta);
}

function classifyFailureKind(rawOutput: string | undefined): "refused" | "timeout" | "failed" {
  if (!rawOutput) {
    return "failed";
  }
  const normalized = rawOutput.trim().toLowerCase();
  if (
    normalized.includes("permission denied") ||
    normalized.includes("cancelled by user") ||
    normalized.includes("plan rejected") ||
    normalized.includes("question cancelled")
  ) {
    return "refused";
  }
  if (normalized.includes("timed out") || normalized.includes("timeout")) {
    return "timeout";
  }
  return "failed";
}

function updateOutcome(status: ToolCall["status"] | undefined): string {
  switch (status) {
    case "completed":
      return "success";
    case "failed":
      return "failure";
    case "in_progress":
      return "partial";
    case "pending":
      return "start";
    default:
      return "partial";
  }
}

function applyFieldsToBase(base: ToolCall, fields: ToolCallUpdateFields): void {
  if (fields.title !== undefined) {
    base.title = fields.title;
  }
  if (fields.kind !== undefined) {
    base.kind = fields.kind;
  }
  if (fields.status !== undefined) {
    base.status = fields.status;
  }
  if (fields.raw_input !== undefined) {
    base.raw_input = fields.raw_input;
  }
  if (fields.raw_output !== undefined) {
    base.raw_output = fields.raw_output;
  }
  if (fields.locations !== undefined) {
    base.locations = fields.locations;
  }
  if (fields.output_metadata !== undefined) {
    base.output_metadata = fields.output_metadata;
  }
  if (fields.meta !== undefined) {
    base.meta = fields.meta;
  }
  if (fields.content !== undefined) {
    base.content = fields.content;
  }
}

function logToolCallSubmitted(
  sessionId: string,
  toolCall: ToolCall,
  updateKind: "initial" | "refresh",
): void {
  bridgeLogger.info({
    target: LOG_TARGETS.APP_TOOL,
    eventName: "tool_call_submitted",
    message: "tool call submitted",
    outcome: "success",
    sessionId,
    toolCallId: toolCall.tool_call_id,
    sizeBytes: jsonSize(toolCall.raw_input),
    fields: {
      submission_kind: updateKind,
      tool_name: toolNameFromMeta(toolCall.meta),
      tool_title: toolCall.title,
      tool_kind: toolCall.kind,
      status: toolCall.status,
      content_block_count: toolCall.content.length,
      location_count: toolCall.locations.length,
      has_output_metadata: toolCall.output_metadata !== undefined,
    },
  });
}

function logToolCallUpdateEmitted(
  sessionId: string,
  toolUseId: string,
  fields: ToolCallUpdateFields,
  base: ToolCall | undefined,
  updateKind: ToolUpdateKind,
): void {
  const nextStatus = fields.status ?? base?.status;
  const rawOutput = typeof fields.raw_output === "string" ? fields.raw_output : base?.raw_output;
  const failureKind = nextStatus === "failed" ? classifyFailureKind(rawOutput) : undefined;
  const commonEvent = {
    target: LOG_TARGETS.APP_TOOL,
    eventName: "tool_call_update_emitted",
    message: "tool call update emitted",
    outcome: updateOutcome(nextStatus),
    sessionId,
    toolCallId: toolUseId,
    sizeBytes: jsonSize(fields.raw_input),
    fields: {
      update_kind: updateKind,
      tool_name: toolName(base, fields),
      previous_status: base?.status,
      next_status: nextStatus,
      title_changed: fields.title !== undefined && fields.title !== base?.title,
      content_block_count: fields.content?.length,
      location_count: fields.locations?.length,
      raw_output_chars: rawOutput?.length,
      has_output_metadata: fields.output_metadata !== undefined || base?.output_metadata !== undefined,
      failure_kind: failureKind,
    },
  } as const;

  if (nextStatus === "failed") {
    bridgeLogger.warn(commonEvent);
    return;
  }
  if (nextStatus === "completed") {
    bridgeLogger.info(commonEvent);
    return;
  }
  if (nextStatus === "in_progress" || nextStatus === "pending") {
    bridgeLogger.debug(commonEvent);
    return;
  }
  bridgeLogger.debug(commonEvent);
}

function emitInitialToolCall(
  session: SessionState,
  toolCall: ToolCall,
  updateKind: "initial" | "refresh" = "initial",
): void {
  session.toolCalls.set(toolCall.tool_call_id, toolCall);
  logToolCallSubmitted(session.sessionId, toolCall, updateKind);
  emitSessionUpdate(session.sessionId, { type: "tool_call", tool_call: toolCall });
}

export function emitToolCallUpdate(
  session: SessionState,
  toolUseId: string,
  fields: ToolCallUpdateFields,
  updateKind: ToolUpdateKind,
): void {
  const base = session.toolCalls.get(toolUseId);
  logToolCallUpdateEmitted(session.sessionId, toolUseId, fields, base, updateKind);
  emitSessionUpdate(session.sessionId, {
    type: "tool_call_update",
    tool_call_update: { tool_call_id: toolUseId, fields },
  });
  if (base) {
    applyFieldsToBase(base, fields);
  }
}

export function emitToolCall(session: SessionState, toolUseId: string, name: string, input: Record<string, unknown>): void {
  const toolCall = createToolCall(toolUseId, name, input);
  const status: ToolCall["status"] = "in_progress";
  toolCall.status = status;

  const existing = session.toolCalls.get(toolUseId);
  if (!existing) {
    emitInitialToolCall(session, toolCall);
    return;
  }

  const fields: ToolCallUpdateFields = {
    title: toolCall.title,
    kind: toolCall.kind,
    status,
    raw_input: toolCall.raw_input,
    locations: toolCall.locations,
    meta: toolCall.meta,
  };
  if (toolCall.content.length > 0) {
    fields.content = toolCall.content;
  }
  emitToolCallUpdate(session, toolUseId, fields, "refresh");
}

export function ensureToolCallVisible(
  session: SessionState,
  toolUseId: string,
  toolName: string,
  input: Record<string, unknown>,
): ToolCall {
  const existing = session.toolCalls.get(toolUseId);
  if (existing) {
    return existing;
  }
  const toolCall = createToolCall(toolUseId, toolName, input);
  emitInitialToolCall(session, toolCall);
  return toolCall;
}

export function emitPlanIfTodoWrite(session: SessionState, name: string, input: Record<string, unknown>): void {
  if (name !== "TodoWrite" || !Array.isArray(input.todos)) {
    return;
  }
  const entries: PlanEntry[] = input.todos
    .map((todo) => {
      if (!todo || typeof todo !== "object") {
        return null;
      }
      const todoObj = todo as Record<string, unknown>;
      const content = typeof todoObj.content === "string" ? todoObj.content : "";
      const status = typeof todoObj.status === "string" ? todoObj.status : "pending";
      if (!content) {
        return null;
      }
      return { content, status, active_form: status };
    })
    .filter((entry): entry is PlanEntry => entry !== null);

  if (entries.length > 0) {
    emitSessionUpdate(session.sessionId, { type: "plan", entries });
  }
}

export function emitToolResultUpdate(
  session: SessionState,
  toolUseId: string,
  isError: boolean,
  rawContent: unknown,
  rawResult: unknown = rawContent,
): void {
  const fields = buildToolResultFields(isError, rawContent, session.toolCalls.get(toolUseId), rawResult);
  emitToolCallUpdate(session, toolUseId, fields, "result");
}

export function finalizeOpenToolCalls(session: SessionState, status: "completed" | "failed"): void {
  for (const [toolUseId, toolCall] of session.toolCalls) {
    if (toolCall.status !== "pending" && toolCall.status !== "in_progress") {
      continue;
    }
    emitToolCallUpdate(session, toolUseId, { status }, "finalize");
  }
}

export function emitToolProgressUpdate(session: SessionState, toolUseId: string, toolName: string): void {
  const existing = session.toolCalls.get(toolUseId);
  if (!existing) {
    emitToolCall(session, toolUseId, toolName, {});
    return;
  }
  if (
    existing.status === "in_progress" ||
    existing.status === "completed" ||
    existing.status === "failed"
  ) {
    return;
  }

  emitToolCallUpdate(session, toolUseId, { status: "in_progress" }, "progress");
}

export function emitToolSummaryUpdate(session: SessionState, toolUseId: string, summary: string): void {
  const base = session.toolCalls.get(toolUseId);
  if (!base) {
    return;
  }
  const fields: ToolCallUpdateFields = {
    status: base.status === "failed" ? "failed" : "completed",
    raw_output: summary,
    content: [{ type: "content", content: { type: "text", text: summary } }],
  };
  emitToolCallUpdate(session, toolUseId, fields, "summary");
}

export function setToolCallStatus(
  session: SessionState,
  toolUseId: string,
  status: "pending" | "in_progress" | "completed" | "failed",
  message?: string,
): void {
  const base = session.toolCalls.get(toolUseId);
  if (!base) {
    return;
  }

  const fields: ToolCallUpdateFields = { status };
  if (message && message.length > 0) {
    fields.raw_output = message;
    fields.content = [{ type: "content", content: { type: "text", text: message } }];
  }
  emitToolCallUpdate(session, toolUseId, fields, "status");
}

export function resolveTaskToolUseId(session: SessionState, msg: Record<string, unknown>): string {
  const direct = typeof msg.tool_use_id === "string" ? msg.tool_use_id : "";
  if (direct) {
    return direct;
  }
  const taskId = typeof msg.task_id === "string" ? msg.task_id : "";
  if (!taskId) {
    return "";
  }
  return session.taskToolUseIds.get(taskId) ?? "";
}

export function taskProgressText(msg: Record<string, unknown>): string {
  const summary = typeof msg.summary === "string" ? msg.summary.trim() : "";
  if (summary) {
    return summary;
  }
  const description = typeof msg.description === "string" ? msg.description : "";
  const lastTool = typeof msg.last_tool_name === "string" ? msg.last_tool_name : "";
  if (description && lastTool) {
    return `${description} (last tool: ${lastTool})`;
  }
  return description || lastTool;
}
