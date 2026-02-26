import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import type { SessionUpdate, ToolCall } from "../types.js";
import { TOOL_RESULT_TYPES, buildToolResultFields, createToolCall, isToolUseBlockType } from "./tooling.js";
import { asRecordOrNull } from "./shared.js";
import { buildUsageUpdateFromResult } from "./usage.js";

export type PersistedSessionEntry = {
  session_id: string;
  cwd: string;
  file_path: string;
  title?: string;
  updated_at?: string;
  sort_ms: number;
};

function normalizeUserPromptText(raw: string): string {
  let text = raw.replace(/<context[\s\S]*/gi, " ");
  text = text.replace(/\[([^\]]+)\]\([^)]+\)/g, "$1");
  text = text.replace(/\s+/g, " ").trim();
  return text;
}

function truncateTextByChars(text: string, maxChars: number): string {
  const chars = Array.from(text);
  if (chars.length <= maxChars) {
    return text;
  }
  return chars.slice(0, maxChars).join("");
}

function firstUserMessageTitleFromRecord(record: Record<string, unknown>): string | undefined {
  if (record.type !== "user") {
    return undefined;
  }
  const message = asRecordOrNull(record.message);
  if (!message || message.role !== "user" || !Array.isArray(message.content)) {
    return undefined;
  }

  const parts: string[] = [];
  for (const item of message.content) {
    const block = asRecordOrNull(item);
    if (!block || block.type !== "text" || typeof block.text !== "string") {
      continue;
    }
    const cleaned = normalizeUserPromptText(block.text);
    if (!cleaned) {
      continue;
    }
    parts.push(cleaned);
    const combined = parts.join(" ");
    if (Array.from(combined).length >= 180) {
      return truncateTextByChars(combined, 180);
    }
  }

  if (parts.length === 0) {
    return undefined;
  }
  return truncateTextByChars(parts.join(" "), 180);
}

function extractSessionPreviewFromJsonl(filePath: string): { cwd?: string; title?: string } {
  let text: string;
  try {
    text = fs.readFileSync(filePath, "utf8");
  } catch {
    return {};
  }

  let cwd: string | undefined;
  let title: string | undefined;
  const lines = text.split(/\r?\n/);
  for (const rawLine of lines) {
    const line = rawLine.trim();
    if (line.length === 0) {
      continue;
    }
    let parsed: unknown;
    try {
      parsed = JSON.parse(line);
    } catch {
      continue;
    }
    const record = asRecordOrNull(parsed);
    if (!record) {
      continue;
    }

    if (!cwd && typeof record.cwd === "string" && record.cwd.trim().length > 0) {
      cwd = record.cwd;
    }
    if (!title) {
      title = firstUserMessageTitleFromRecord(record);
    }
    if (cwd && title) {
      break;
    }
  }

  return {
    ...(cwd ? { cwd } : {}),
    ...(title ? { title } : {}),
  };
}

export function listRecentPersistedSessions(limit = 8): PersistedSessionEntry[] {
  const root = path.join(os.homedir(), ".claude", "projects");
  if (!fs.existsSync(root)) {
    return [];
  }

  const candidates: PersistedSessionEntry[] = [];
  let projectDirs: fs.Dirent[];
  try {
    projectDirs = fs.readdirSync(root, { withFileTypes: true }).filter((dirent) => dirent.isDirectory());
  } catch {
    return [];
  }

  for (const dirent of projectDirs) {
    const projectDir = path.join(root, dirent.name);

    let sessionFiles: fs.Dirent[];
    try {
      sessionFiles = fs
        .readdirSync(projectDir, { withFileTypes: true })
        .filter((entry) => entry.isFile() && entry.name.endsWith(".jsonl"));
    } catch {
      sessionFiles = [];
    }

    for (const sessionFile of sessionFiles) {
      const sessionId = sessionFile.name.slice(0, -".jsonl".length);
      if (!sessionId) {
        continue;
      }
      let mtimeMs = 0;
      try {
        mtimeMs = fs.statSync(path.join(projectDir, sessionFile.name)).mtimeMs;
      } catch {
        continue;
      }
      if (!Number.isFinite(mtimeMs) || mtimeMs <= 0) {
        continue;
      }
      candidates.push({
        session_id: sessionId,
        cwd: "",
        file_path: path.join(projectDir, sessionFile.name),
        updated_at: new Date(mtimeMs).toISOString(),
        sort_ms: mtimeMs,
      });
    }
  }

  candidates.sort((a, b) => b.sort_ms - a.sort_ms);
  const deduped: PersistedSessionEntry[] = [];
  const seen = new Set<string>();
  for (const candidate of candidates) {
    if (seen.has(candidate.session_id)) {
      continue;
    }
    seen.add(candidate.session_id);

    const preview = extractSessionPreviewFromJsonl(candidate.file_path);
    const cwd = preview.cwd?.trim();
    if (!cwd) {
      continue;
    }

    deduped.push({
      session_id: candidate.session_id,
      cwd,
      file_path: candidate.file_path,
      ...(preview.title ? { title: preview.title } : {}),
      ...(candidate.updated_at ? { updated_at: candidate.updated_at } : {}),
      sort_ms: candidate.sort_ms,
    });
    if (deduped.length >= limit) {
      break;
    }
  }
  return deduped;
}

export function resolvePersistedSessionEntry(sessionId: string): PersistedSessionEntry | null {
  if (
    sessionId.trim().length === 0 ||
    sessionId.includes("/") ||
    sessionId.includes("\\") ||
    sessionId.includes("..")
  ) {
    return null;
  }
  const root = path.join(os.homedir(), ".claude", "projects");
  if (!fs.existsSync(root)) {
    return null;
  }

  let projectDirs: fs.Dirent[];
  try {
    projectDirs = fs.readdirSync(root, { withFileTypes: true }).filter((dirent) => dirent.isDirectory());
  } catch {
    return null;
  }

  let best: PersistedSessionEntry | null = null;
  for (const dirent of projectDirs) {
    const filePath = path.join(root, dirent.name, `${sessionId}.jsonl`);
    if (!fs.existsSync(filePath)) {
      continue;
    }
    const preview = extractSessionPreviewFromJsonl(filePath);
    const cwd = preview.cwd?.trim();
    if (!cwd) {
      continue;
    }

    let mtimeMs = 0;
    try {
      mtimeMs = fs.statSync(filePath).mtimeMs;
    } catch {
      mtimeMs = 0;
    }
    if (!best || mtimeMs >= best.sort_ms) {
      best = {
        session_id: sessionId,
        cwd,
        file_path: filePath,
        sort_ms: mtimeMs,
      };
    }
  }
  return best;
}

function persistedMessageCandidates(record: Record<string, unknown>): Record<string, unknown>[] {
  const candidates: Record<string, unknown>[] = [];

  const topLevel = asRecordOrNull(record.message);
  if (topLevel) {
    candidates.push(topLevel);
  }

  const nested = asRecordOrNull(asRecordOrNull(asRecordOrNull(record.data)?.message)?.message);
  if (nested) {
    candidates.push(nested);
  }

  return candidates;
}

function pushResumeTextChunk(updates: SessionUpdate[], role: "user" | "assistant", text: string): void {
  if (!text.trim()) {
    return;
  }
  if (role === "assistant") {
    updates.push({ type: "agent_message_chunk", content: { type: "text", text } });
    return;
  }
  updates.push({ type: "user_message_chunk", content: { type: "text", text } });
}

function pushResumeToolUse(
  updates: SessionUpdate[],
  toolCalls: Map<string, ToolCall>,
  block: Record<string, unknown>,
): void {
  const toolUseId = typeof block.id === "string" ? block.id : "";
  if (!toolUseId) {
    return;
  }
  const name = typeof block.name === "string" ? block.name : "Tool";
  const input = asRecordOrNull(block.input) ?? {};

  const toolCall = createToolCall(toolUseId, name, input);
  toolCall.status = "in_progress";
  toolCalls.set(toolUseId, toolCall);
  updates.push({ type: "tool_call", tool_call: toolCall });
}

function pushResumeToolResult(
  updates: SessionUpdate[],
  toolCalls: Map<string, ToolCall>,
  block: Record<string, unknown>,
): void {
  const toolUseId = typeof block.tool_use_id === "string" ? block.tool_use_id : "";
  if (!toolUseId) {
    return;
  }
  const isError = Boolean(block.is_error);
  const base = toolCalls.get(toolUseId);
  const fields = buildToolResultFields(isError, block.content, base);
  updates.push({ type: "tool_call_update", tool_call_update: { tool_call_id: toolUseId, fields } });

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
}

function pushResumeUsageUpdate(
  updates: SessionUpdate[],
  message: Record<string, unknown>,
  emittedUsageMessageIds: Set<string>,
): void {
  const messageId = typeof message.id === "string" ? message.id : "";
  if (messageId && emittedUsageMessageIds.has(messageId)) {
    return;
  }

  const usageUpdate = buildUsageUpdateFromResult(message);
  if (!usageUpdate) {
    return;
  }

  updates.push(usageUpdate);
  if (messageId) {
    emittedUsageMessageIds.add(messageId);
  }
}

export function extractSessionHistoryUpdatesFromJsonl(filePath: string): SessionUpdate[] {
  let text: string;
  try {
    text = fs.readFileSync(filePath, "utf8");
  } catch {
    return [];
  }

  const updates: SessionUpdate[] = [];
  const toolCalls = new Map<string, ToolCall>();
  const emittedUsageMessageIds = new Set<string>();
  const lines = text.split(/\r?\n/);
  for (const rawLine of lines) {
    const line = rawLine.trim();
    if (line.length === 0) {
      continue;
    }
    let parsed: unknown;
    try {
      parsed = JSON.parse(line);
    } catch {
      continue;
    }
    const record = asRecordOrNull(parsed);
    if (!record) {
      continue;
    }
    for (const message of persistedMessageCandidates(record)) {
      const role = message.role;
      if (role !== "user" && role !== "assistant") {
        continue;
      }
      const content = Array.isArray(message.content) ? message.content : [];
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
          pushResumeTextChunk(updates, role, block.text);
          continue;
        }
        if (isToolUseBlockType(blockType) && role === "assistant") {
          pushResumeToolUse(updates, toolCalls, block);
          continue;
        }
        if (TOOL_RESULT_TYPES.has(blockType)) {
          pushResumeToolResult(updates, toolCalls, block);
          continue;
        }
        if (blockType === "image") {
          pushResumeTextChunk(updates, role, "[image]");
        }
      }
      pushResumeUsageUpdate(updates, message, emittedUsageMessageIds);
    }
  }
  return updates;
}

