import { getSessionMessages, getSubagentMessages } from "@anthropic-ai/claude-agent-sdk";
import type { SessionMessage } from "@anthropic-ai/claude-agent-sdk";
import type { ToolCall } from "../types.js";
import { bridgeLogger, LOG_TARGETS } from "./logger.js";
import type { SessionState } from "./session_lifecycle.js";
import { asRecordOrNull } from "./shared.js";
import { restoreSavedSubagentTask, type SavedTaskObservation } from "./tasks.js";
import { toolNameFromMeta } from "./tool_calls.js";
import { agentTaskLaunchFields, buildToolResultFields, createToolCall, isAgentLikeToolName, isDeferredAgentResult, resumedAgentTaskIdFromResult, TOOL_RESULT_TYPES } from "./tooling.js";
import { transcriptPresentation } from "./transcript_presentation.js";

function text(value: unknown): string | undefined {
  return typeof value === "string" && value.trim() ? value.trim() : undefined;
}

function blocks(row: SessionMessage): Record<string, unknown>[] {
  const content = asRecordOrNull(row.message)?.content;
  return Array.isArray(content) ? content.flatMap(block => {
    const record = asRecordOrNull(block);
    return record ? [record] : [];
  }) : [];
}

function findCall(messages: SessionMessage[], id: string | undefined): ToolCall | undefined {
  if (!id) return undefined;
  for (const row of messages) {
    if (row.type !== "assistant") continue;
    const block = blocks(row).find(block => block.type === "tool_use" && block.id === id);
    if (block) return createToolCall(id, text(block.name) ?? "Tool", asRecordOrNull(block.input) ?? {}, row.parent_tool_use_id);
  }
  return undefined;
}

/** Task evidence only: none of these conversations are replayed into main chat. */
function savedObservations(taskId: string, toolId: string | undefined, tool: ToolCall | undefined, messages: SessionMessage[]): SavedTaskObservation[] {
  const input = asRecordOrNull(tool?.raw_input);
  const launch = agentTaskLaunchFields(tool);
  const fields = {
    ...launch,
    task_type: "local_agent",
    subject: launch.task_description,
    prompt: text(input?.prompt),
  };
  const observations: SavedTaskObservation[] = [{
    subtype: "task_started",
    message: { ...fields, tool_use_id: toolId, synthesized_launch: true },
  }];
  // A retained invocation can supply execution evidence when its original
  // record is no longer in the SDK's compacted history projection.
  const terminal = tool?.task_metadata?.terminal_status ?? (input?.run_in_background !== true ? tool?.status : undefined);
  if (terminal === "completed" || terminal === "failed" || terminal === "killed" || terminal === "stopped") {
    observations.push({ subtype: "task_notification", message: { ...fields, tool_use_id: tool?.tool_call_id, status: terminal } });
  }
  for (const row of messages) {
    const presentation = transcriptPresentation(row);
    if (presentation.kind === "task_notification" && presentation.notification?.task_id === taskId) {
      observations.push({ subtype: "task_notification", message: { ...fields, ...presentation.notification } });
    }
    if (row.type !== "user") continue;
    for (const block of blocks(row)) {
      if (!TOOL_RESULT_TYPES.has(String(block.type))) continue;
      const id = text(block.tool_use_id);
      const call = id === tool?.tool_call_id ? tool : findCall(messages, id);
      const raw = asRecordOrNull(row)?.tool_use_result ?? asRecordOrNull(row)?.toolUseResult ?? block;
      if (!block.is_error && resumedAgentTaskIdFromResult(toolNameFromMeta(call?.meta) ?? "", raw, block.content) === taskId) {
        observations.push({ subtype: "task_started", message: { ...fields, tool_use_id: id } });
      } else if (id && id === tool?.tool_call_id && input?.run_in_background !== true && isAgentLikeToolName(toolNameFromMeta(tool.meta) ?? "") && !isDeferredAgentResult(raw, block.content)) {
        const result = buildToolResultFields(Boolean(block.is_error), block.content, tool, raw);
        if (result.status === "completed" || result.status === "failed" || result.status === "killed") {
          observations.push({ subtype: "task_notification", message: { ...fields, tool_use_id: id, status: result.status } });
        }
      }
    }
  }
  return observations;
}

/** Discover every saved depth uniformly; lifecycle state remains in tasksById. */
export async function restoreSavedSubagents(session: SessionState, agentIds: string[]): Promise<void> {
  if (agentIds.length === 0) return;
  const saved: Array<{ id: string; messages: SessionMessage[] }> = [];
  let main: SessionMessage[] = [];
  try {
    main = await getSessionMessages(session.sessionId, { dir: session.cwd, includeSystemMessages: true });
  } catch (error) {
    bridgeLogger.warn({
      target: LOG_TARGETS.APP_SESSION, eventName: "saved_subagent_metadata_failed", message: "Could not read saved agent launch metadata", outcome: "failure",
      sessionId: session.sessionId, fields: { error_message: String(error) },
    });
  }
  // Bound metadata reads without adding a persistent transcript cache or another task store.
  const uniqueIds = [...new Set(agentIds)];
  for (let offset = 0; offset < uniqueIds.length; offset += 8) {
    const batch = uniqueIds.slice(offset, offset + 8);
    const results = await Promise.allSettled(batch.map(id => getSubagentMessages(session.sessionId, id, { dir: session.cwd })));
    results.forEach((result, index) => {
      const id = batch[index];
      saved.push({ id, messages: result.status === "fulfilled" ? result.value : [] });
      if (result.status === "rejected") bridgeLogger.warn({
        target: LOG_TARGETS.APP_SESSION, eventName: "saved_subagent_metadata_failed", message: "Could not read saved subagent metadata", outcome: "failure",
        sessionId: session.sessionId, fields: { task_id: id, error_message: String(result.reason) },
      });
    });
  }
  if (session.closing) return;
  for (const agent of saved) {
    const toolId = agent.messages.map(row => text(row.parent_tool_use_id)).find(Boolean);
    const parentId = agent.messages.map(row => text(row.parent_agent_id)).find(Boolean);
    const parent = saved.find(candidate => candidate.id === parentId) ?? saved.find(candidate => findCall(candidate.messages, toolId));
    const ownerMessages = parent?.messages ?? (parentId ? [] : main);
    const tool = findCall(ownerMessages, toolId) ?? (toolId ? session.toolCalls.get(toolId) : undefined);
    const before = session.tasksById.get(agent.id);
    restoreSavedSubagentTask(session, agent.id, parentId ?? parent?.id, savedObservations(agent.id, toolId, tool, ownerMessages));
    const after = session.tasksById.get(agent.id);
    if (after && after !== before) bridgeLogger.debug({
      target: LOG_TARGETS.APP_SESSION, eventName: "saved_subagent_task_restored", message: "Saved subagent restored into task inventory", outcome: "success",
      sessionId: session.sessionId, fields: { task_id: after.task_id, parent_task_id: after.parent_task_id, run_id: after.run_id, status: after.status, terminal_status: asRecordOrNull(after.metadata)?.terminal_status },
    });
  }
}
