import { getSubagentMessages, listSubagents } from "@anthropic-ai/claude-agent-sdk";
import type { BridgeCommand, SubagentMessage, ToolCall } from "../types.js";
import { writeEvent } from "./events.js";
import { mapSessionMessagesToEntries } from "./history.js";
import { sessionById, type SessionState } from "./session_lifecycle.js";
import { asRecordOrNull } from "./shared.js";
import { toolNameFromMeta } from "./tool_calls.js";

type SubagentCommand = Extract<
  BridgeCommand,
  { command: "list_subagents" | "get_subagent_messages" | "stop_subagent" }
>;

const NO_SESSION = "No active session is available.";

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function activeSession(sessionId: string): SessionState | undefined {
  const session = sessionById(sessionId);
  return session && !session.closing ? session : undefined;
}

/**
 * Transcript reads and stop requests for the active session's subagents. Live
 * status stays with the task lifecycle: a stop result only acknowledges the
 * request, and the lifecycle messages that follow report the outcome.
 */
export async function handleSubagentCommand(
  command: SubagentCommand,
  requestId?: string,
): Promise<void> {
  switch (command.command) {
    case "list_subagents":
      await listTranscripts(command, requestId);
      return;
    case "get_subagent_messages":
      await readTranscript(command, requestId);
      return;
    case "stop_subagent":
      await requestStop(command, requestId);
  }
}

async function listTranscripts(
  command: Extract<SubagentCommand, { command: "list_subagents" }>,
  requestId?: string,
): Promise<void> {
  const session = activeSession(command.session_id);
  try {
    if (!session) throw new Error(NO_SESSION);
    const agentIds = await listSubagents(session.sessionId, { dir: session.cwd });
    writeEvent({ event: "subagents_listed", session_id: command.session_id, agent_ids: agentIds }, requestId);
  } catch (error) {
    writeEvent(
      { event: "subagents_listed", session_id: command.session_id, agent_ids: [], error: errorMessage(error) },
      requestId,
    );
  }
}

async function readTranscript(
  command: Extract<SubagentCommand, { command: "get_subagent_messages" }>,
  requestId?: string,
): Promise<void> {
  const session = activeSession(command.session_id);
  const result = { event: "subagent_messages" as const, session_id: command.session_id, agent_id: command.agent_id };
  try {
    if (!session) throw new Error(NO_SESSION);
    const history = await getSubagentMessages(session.sessionId, command.agent_id, { dir: session.cwd });
    writeEvent({ ...result, messages: transcriptEntries(history) }, requestId);
  } catch (error) {
    writeEvent({ ...result, messages: [], error: errorMessage(error) }, requestId);
  }
}

/**
 * The report a subagent writes for its caller through the `SubagentHandback`
 * tool (as Claude Code 2.1.300 saves it), often its only answer: no text reply follows it.
 */
function handbackReport(tool: ToolCall): string | undefined {
  if (toolNameFromMeta(tool.meta) !== "SubagentHandback") return undefined;
  const message = asRecordOrNull(tool.raw_input)?.message;
  return typeof message === "string" && message.trim() !== "" ? message : undefined;
}

/** The text a reader follows: prompts, SDK reminders, replies and the titles of tool calls. */
function transcriptEntries(history: Parameters<typeof mapSessionMessagesToEntries>[0]): SubagentMessage[] {
  return mapSessionMessagesToEntries(history, true).flatMap((update): SubagentMessage[] => {
    if (update.type === "system_reminder") return [{ role: "system_reminder", text: update.text }];
    if (update.type === "tool_call") {
      const report = handbackReport(update.tool_call);
      return [report === undefined ? { role: "tool", text: update.tool_call.title } : { role: "assistant", text: report }];
    }
    if (update.type !== "user_message_chunk" && update.type !== "agent_message_chunk") return [];
    if (update.content.type !== "text" || update.content.text.trim() === "") return [];
    return [{ role: update.type === "user_message_chunk" ? "user" : "assistant", text: update.content.text }];
  });
}

async function requestStop(
  command: Extract<SubagentCommand, { command: "stop_subagent" }>,
  requestId?: string,
): Promise<void> {
  const session = activeSession(command.session_id);
  // Capture the operation before awaiting; the task may resume meanwhile.
  const result = { event: "subagent_stop_result" as const, session_id: command.session_id, task_id: command.task_id, run_id: command.expected_run_id };
  try {
    if (!session) throw new Error(NO_SESSION);
    const task = session.tasksById.get(command.task_id);
    if (task?.run_id !== command.expected_run_id) throw new Error("This subagent's execution has changed.");
    if (
      task?.status !== "in_progress" ||
      asRecordOrNull(task.metadata)?.task_type !== "local_agent"
    ) {
      throw new Error("This subagent is not running.");
    }
    await session.query.stopTask(command.task_id);
    writeEvent(result, requestId);
  } catch (error) {
    writeEvent({ ...result, error: errorMessage(error) }, requestId);
  }
}
