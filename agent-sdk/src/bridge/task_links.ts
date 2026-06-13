import type { SessionState } from "./session_lifecycle.js";

export function linkTaskToolUse(session: SessionState, taskId: string, toolUseId: string): void {
  if (!taskId || !toolUseId) {
    return;
  }

  const previousToolUseId = session.taskToolUseIds.get(taskId);
  if (previousToolUseId && previousToolUseId !== toolUseId) {
    const reverseTaskId = session.taskIdsByToolUseId.get(previousToolUseId);
    if (reverseTaskId === taskId) {
      session.taskIdsByToolUseId.delete(previousToolUseId);
    }
  }

  const previousTaskId = session.taskIdsByToolUseId.get(toolUseId);
  if (previousTaskId && previousTaskId !== taskId) {
    const forwardToolUseId = session.taskToolUseIds.get(previousTaskId);
    if (forwardToolUseId === toolUseId) {
      session.taskToolUseIds.delete(previousTaskId);
    }
  }

  session.taskToolUseIds.set(taskId, toolUseId);
  session.taskIdsByToolUseId.set(toolUseId, taskId);
}

export function unlinkTaskToolUse(session: SessionState, taskId: string): void {
  if (!taskId) {
    return;
  }
  const toolUseId = session.taskToolUseIds.get(taskId);
  session.taskToolUseIds.delete(taskId);
  if (toolUseId && session.taskIdsByToolUseId.get(toolUseId) === taskId) {
    session.taskIdsByToolUseId.delete(toolUseId);
  }
}

export function activeTaskIdForToolUse(session: SessionState, toolUseId: string): string | undefined {
  return toolUseId ? session.taskIdsByToolUseId.get(toolUseId) : undefined;
}
