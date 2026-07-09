import type { Json, ToolCall, ToolCallUpdateFields } from "../types.js";
import { asRecordOrNull } from "./shared.js";
import { CACHE_SPLIT_POLICY, previewKilobyteLabel } from "./cache_policy.js";
import {
  isTaskToolName,
  taskToolResultText,
  taskToolTitle,
  taskUpdateSucceeded,
  type TaskTitleContext,
} from "./tasks.js";

export const TOOL_RESULT_TYPES = new Set([
  "tool_result",
  "tool_search_tool_result",
  "web_fetch_tool_result",
  "web_search_tool_result",
  "code_execution_tool_result",
  "bash_code_execution_tool_result",
  "text_editor_code_execution_tool_result",
  "mcp_tool_result",
]);

const CRON_TOOL_NAMES = new Set(["CronCreate", "CronDelete", "CronList"]);
const CRON_LIST_DIVIDER = "__cron_list_job_divider__";
const SCHEDULE_WAKEUP_TOOL_NAME = "ScheduleWakeup";
const PUSH_NOTIFICATION_TOOL_NAME = "PushNotification";
const REMOTE_TRIGGER_TOOL_NAME = "RemoteTrigger";
const ENTER_PLAN_MODE_TOOL_NAME = "EnterPlanMode";
const REPL_TOOL_NAME = "REPL";
const MONITOR_TOOL_NAME = "Monitor";
const WORKFLOW_TOOL_NAME = "Workflow";
const PROJECTS_TOOL_NAME = "Projects";
const ARTIFACT_TOOL_NAME = "Artifact";
const SHOW_ONBOARDING_ROLE_PICKER_TOOL_NAME = "ShowOnboardingRolePicker";
const READ_MCP_RESOURCE_TOOL_NAME = "ReadMcpResource";
const READ_MCP_RESOURCE_DIR_TOOL_NAME = "ReadMcpResourceDir";
const SEARCH_OUTPUT_MODES = new Set(["content", "files_with_matches", "count"]);

function isCronToolName(name: string): boolean {
  return CRON_TOOL_NAMES.has(name);
}

export function isToolSearchToolName(name: string): boolean {
  const normalized = name.replace(/[\s_-]+/g, "").toLowerCase();
  return normalized === "toolsearch" || normalized === "toolsearchtool";
}

export function isToolSearchToolResultType(blockType: string): boolean {
  return blockType === "tool_search_tool_result";
}

export function isToolUseBlockType(blockType: string): boolean {
  return blockType === "tool_use" || blockType === "server_tool_use" || blockType === "mcp_tool_use";
}

function inputString(input: Record<string, unknown>, key: string): string {
  return typeof input[key] === "string" ? input[key].trim() : "";
}

function inputNumber(input: Record<string, unknown>, key: string): number | undefined {
  const value = input[key];
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

function inputBoolean(input: Record<string, unknown>, key: string): boolean | undefined {
  return typeof input[key] === "boolean" ? input[key] : undefined;
}

function isAgentLikeToolName(name: string): boolean {
  return name === "Agent" || name === "Task";
}

export function isShellToolName(name: string): boolean {
  return name === "Bash" || name === "PowerShell";
}

function isMcpResourceReadToolName(name: string): boolean {
  return name === READ_MCP_RESOURCE_TOOL_NAME || name === READ_MCP_RESOURCE_DIR_TOOL_NAME;
}

function agentInputTitle(name: string, input: Record<string, unknown>): string | undefined {
  if (!isAgentLikeToolName(name)) {
    return undefined;
  }
  const agentName = nonEmptyString(input.name);
  if (agentName) {
    return `${name}: ${agentName}`;
  }
  const subagentType = nonEmptyString(input.subagent_type);
  return subagentType ? `${name}: ${subagentType}` : undefined;
}

function searchModeLabel(value: unknown): string {
  if (typeof value !== "string" || !SEARCH_OUTPUT_MODES.has(value)) {
    return "";
  }
  switch (value) {
    case "files_with_matches":
      return "files";
    case "content":
      return "content";
    case "count":
      return "count";
    default:
      return "";
  }
}

function grepContextValue(input: Record<string, unknown>): number | undefined {
  return (
    inputNumber(input, "context") ??
    inputNumber(input, "-C") ??
    inputNumber(input, "-A") ??
    inputNumber(input, "-B")
  );
}

function formatGlobTitle(input: Record<string, unknown>): string {
  const pattern = inputString(input, "pattern");
  const path = inputString(input, "path");
  if (pattern && path) {
    return `Glob ${pattern} in ${path}`;
  }
  if (pattern) {
    return `Glob ${pattern}`;
  }
  if (path) {
    return `Glob ${path}`;
  }
  return "Glob";
}

function formatGrepTitle(input: Record<string, unknown>): string {
  const pattern = inputString(input, "pattern");
  const path = inputString(input, "path");
  const glob = inputString(input, "glob");
  const fileType = inputString(input, "type");
  const outputMode = searchModeLabel(input.output_mode);
  const headLimit = inputNumber(input, "head_limit");
  const offset = inputNumber(input, "offset");
  const context = grepContextValue(input);
  const flags: string[] = [];

  if (glob) {
    flags.push(`glob ${glob}`);
  }
  if (fileType) {
    flags.push(`type ${fileType}`);
  }
  if (outputMode) {
    flags.push(outputMode);
  }
  if (inputBoolean(input, "-i") === true) {
    flags.push("case-insensitive");
  }
  if (context !== undefined) {
    flags.push(`context ${context}`);
  }
  if (headLimit !== undefined) {
    flags.push(`limit ${headLimit}`);
  }
  if (offset !== undefined && offset > 0) {
    flags.push(`offset ${offset}`);
  }
  if (inputBoolean(input, "multiline") === true) {
    flags.push("multiline");
  }

  const base = pattern ? `Grep ${pattern}` : "Grep";
  const scoped = path ? `${base} in ${path}` : base;
  return flags.length > 0 ? `${scoped} (${flags.join(", ")})` : scoped;
}

export function normalizeToolKind(name: string): string {
  if (isShellToolName(name)) {
    return "execute";
  }
  switch (name) {
    case "Read":
    case READ_MCP_RESOURCE_TOOL_NAME:
    case READ_MCP_RESOURCE_DIR_TOOL_NAME:
      return "read";
    case "Write":
    case "Edit":
      return "edit";
    case "Delete":
      return "delete";
    case "Move":
      return "move";
    case "Glob":
    case "Grep":
      return "search";
    case "WebFetch":
      return "fetch";
    case "TaskCreate":
    case "TaskUpdate":
    case "TaskGet":
    case "TaskList":
    case "TaskOutput":
    case "TaskStop":
    case "CronCreate":
    case "CronDelete":
    case "CronList":
    case "ScheduleWakeup":
    case "PushNotification":
    case "RemoteTrigger":
    case "EnterWorktree":
    case "ExitWorktree":
    case "REPL":
    case "Monitor":
    case "Workflow":
    case "Projects":
    case "Artifact":
    case "ShowOnboardingRolePicker":
      return "other";
    case "Task":
    case "Agent":
      return "think";
    case "EnterPlanMode":
    case "ExitPlanMode":
      return "switch_mode";
    default:
      return "think";
  }
}

export function toolTitle(
  name: string,
  input: Record<string, unknown>,
  context: TaskTitleContext = {},
): string {
  const agentTitle = agentInputTitle(name, input);
  if (agentTitle) {
    return agentTitle;
  }
  if (isShellToolName(name)) {
    const command = typeof input.command === "string" ? input.command : "";
    return command || "Terminal";
  }
  if (name === "Glob") {
    return formatGlobTitle(input);
  }
  if (name === "Grep") {
    return formatGrepTitle(input);
  }
  if (name === "WebFetch") {
    const url = typeof input.url === "string" ? input.url : "";
    if (url) {
      return `WebFetch ${url}`;
    }
  }
  if (name === "WebSearch") {
    const query = typeof input.query === "string" ? input.query : "";
    if (query) {
      return `WebSearch ${query}`;
    }
  }
  const taskTitle = taskToolTitle(name, input, context);
  if (taskTitle) {
    return taskTitle;
  }
  if (isCronToolName(name)) {
    return name;
  }
  if (name === SCHEDULE_WAKEUP_TOOL_NAME) {
    return name;
  }
  if (name === PUSH_NOTIFICATION_TOOL_NAME) {
    return name;
  }
  if (name === REMOTE_TRIGGER_TOOL_NAME) {
    const action = typeof input.action === "string" ? input.action.trim() : "";
    return action ? `${REMOTE_TRIGGER_TOOL_NAME}: ${action}` : REMOTE_TRIGGER_TOOL_NAME;
  }
  if (name === ENTER_PLAN_MODE_TOOL_NAME) {
    return name;
  }
  if (name === REPL_TOOL_NAME) {
    const code = typeof input.code === "string" ? input.code.trim() : "";
    return code ? `REPL: ${code}` : REPL_TOOL_NAME;
  }
  if (name === MONITOR_TOOL_NAME) {
    const description = nonEmptyString(input.description);
    return description ? `${MONITOR_TOOL_NAME}: ${description}` : MONITOR_TOOL_NAME;
  }
  if (name === WORKFLOW_TOOL_NAME) {
    const workflowName = nonEmptyString(input.name);
    return workflowName ? `${WORKFLOW_TOOL_NAME}: ${workflowName}` : WORKFLOW_TOOL_NAME;
  }
  if (name === PROJECTS_TOOL_NAME) {
    return formatProjectsTitle(input);
  }
  if (name === ARTIFACT_TOOL_NAME) {
    const label = nonEmptyString(input.label) ?? nonEmptyString(input.file_path);
    return label ? `${ARTIFACT_TOOL_NAME}: ${label}` : ARTIFACT_TOOL_NAME;
  }
  if (name === SHOW_ONBOARDING_ROLE_PICKER_TOOL_NAME) {
    return SHOW_ONBOARDING_ROLE_PICKER_TOOL_NAME;
  }
  if (name === "EnterWorktree") {
    const worktreeName = typeof input.name === "string" ? input.name.trim() : "";
    return worktreeName || "EnterWorktree";
  }
  if (name === "ExitWorktree") {
    return "ExitWorktree";
  }
  if ((name === "Read" || name === "Write" || name === "Edit") && typeof input.file_path === "string") {
    return `${name} ${input.file_path}`;
  }
  if (isMcpResourceReadToolName(name)) {
    const uri = typeof input.uri === "string" ? input.uri : "";
    const server = typeof input.server === "string" ? input.server : "";
    if (server && uri) {
      return `${name} ${server} ${uri}`;
    }
    if (uri) {
      return `${name} ${uri}`;
    }
  }
  return name;
}

function formatProjectsTitle(input: Record<string, unknown>): string {
  const method = nonEmptyString(input.method);
  const action = method?.startsWith("project_") ? method.slice("project_".length) : method;
  const suffix = nonEmptyString(input.path) ?? nonEmptyString(input.query);
  const base = action ? `${PROJECTS_TOOL_NAME}: ${action}` : PROJECTS_TOOL_NAME;
  return suffix ? `${base} ${suffix}` : base;
}

function editDiffContent(name: string, input: Record<string, unknown>): ToolCall["content"] {
  const filePath = typeof input.file_path === "string" ? input.file_path : "";
  if (!filePath) {
    return [];
  }

  if (name === "Edit") {
    const oldText = typeof input.old_string === "string" ? input.old_string : "";
    const newText = typeof input.new_string === "string" ? input.new_string : "";
    if (!oldText && !newText) {
      return [];
    }
    return [{ type: "diff", old_path: filePath, new_path: filePath, old: oldText, new: newText }];
  }

  if (name === "Write") {
    const newText = typeof input.content === "string" ? input.content : "";
    if (!newText) {
      return [];
    }
    return [{ type: "diff", old_path: filePath, new_path: filePath, old: "", new: newText }];
  }

  return [];
}

export function createToolCall(
  toolUseId: string,
  name: string,
  input: Record<string, unknown>,
  parentToolUseId: string | null = null,
  titleContext: TaskTitleContext = {},
): ToolCall {
  return {
    tool_call_id: toolUseId,
    title: toolTitle(name, input, titleContext),
    kind: normalizeToolKind(name),
    status: "pending",
    content: editDiffContent(name, input),
    raw_input: input as unknown as Json,
    locations: typeof input.file_path === "string" ? [{ path: input.file_path }] : [],
    meta: {
      claudeCode: {
        toolName: name,
        parentToolUseId,
      },
    },
  };
}

function resultRecordCandidates(rawResult: unknown, rawContent: unknown): Record<string, unknown>[] {
  const candidates: Record<string, unknown>[] = [];

  const pushRecord = (value: unknown): void => {
    const record = asRecordOrNull(value);
    if (record) {
      candidates.push(record);
    }
  };

  const pushRecords = (value: unknown): void => {
    if (Array.isArray(value)) {
      for (const entry of value) {
        pushRecord(entry);
      }
      return;
    }
    pushRecord(value);
  };

  const pushNestedRecords = (value: unknown): void => {
    if (Array.isArray(value)) {
      for (const entry of value) {
        pushNestedRecords(entry);
      }
      return;
    }
    const record = asRecordOrNull(value);
    if (!record) {
      return;
    }
    pushRecords(record.result);
    pushRecords(record.data);
    pushRecords(record.content);
  };

  pushRecords(rawResult);
  pushNestedRecords(rawResult);
  pushRecords(rawContent);
  pushNestedRecords(rawContent);

  return candidates;
}

function parseJsonCandidate(value: unknown): unknown {
  const text = typeof value === "string" ? value : extractText(value);
  const trimmed = text.trim();
  if (!(trimmed.startsWith("{") || trimmed.startsWith("["))) {
    return undefined;
  }
  try {
    return JSON.parse(trimmed);
  } catch {
    return undefined;
  }
}

function pushStructuredRecordCandidates(
  candidates: Record<string, unknown>[],
  value: unknown,
): void {
  const record = asRecordOrNull(value);
  if (!record) {
    return;
  }
  candidates.push(record);

  const nestedResult = asRecordOrNull(record.result);
  if (nestedResult) {
    candidates.push(nestedResult);
  }
  const nestedData = asRecordOrNull(record.data);
  if (nestedData) {
    candidates.push(nestedData);
  }
  const nestedContent = asRecordOrNull(record.content);
  if (nestedContent) {
    candidates.push(nestedContent);
  }
}

function mcpResourceContentFromResult(rawResult: unknown, rawContent: unknown): ToolCall["content"] {
  const candidates: Record<string, unknown>[] = [];
  for (const candidate of [rawResult, rawContent, parseJsonCandidate(rawResult), parseJsonCandidate(rawContent)]) {
    pushStructuredRecordCandidates(candidates, candidate);
  }

  for (const candidate of candidates) {
    const contents = Array.isArray(candidate.contents) ? candidate.contents : null;
    if (!contents || contents.length === 0) {
      continue;
    }

    const mapped: ToolCall["content"] = [];
    for (const entry of contents) {
      const record = asRecordOrNull(entry);
      if (!record) {
        continue;
      }
      const uri = typeof record.uri === "string" ? record.uri : "";
      if (!uri) {
        continue;
      }
      const text =
        typeof record.text === "string" && record.text.length > 0 ? record.text : undefined;
      const mimeType =
        typeof record.mimeType === "string" && record.mimeType.trim().length > 0
          ? record.mimeType.trim()
          : undefined;
      const blobSavedTo =
        typeof record.blobSavedTo === "string" && record.blobSavedTo.trim().length > 0
          ? record.blobSavedTo.trim()
          : undefined;
      if (!text && !blobSavedTo) {
        continue;
      }
      mapped.push({
        type: "mcp_resource",
        uri,
        ...(mimeType ? { mime_type: mimeType } : {}),
        ...(text ? { text } : {}),
        ...(blobSavedTo ? { blob_saved_to: blobSavedTo } : {}),
      });
    }

    if (mapped.length > 0) {
      return mapped;
    }
  }

  return [];
}

function mcpResourceDirTextFromResult(
  toolName: string,
  rawResult: unknown,
  rawContent: unknown,
): string | undefined {
  if (toolName !== READ_MCP_RESOURCE_DIR_TOOL_NAME) {
    return undefined;
  }

  for (const candidate of collectResultCandidates(rawResult, rawContent)) {
    if (!Array.isArray(candidate.resources)) {
      continue;
    }

    const lines: string[] = [];
    for (const entry of candidate.resources) {
      const record = asRecordOrNull(entry);
      if (!record) {
        continue;
      }
      const name = nonEmptyString(record.name);
      const uri = nonEmptyString(record.uri);
      if (!name || !uri) {
        continue;
      }

      const mimeType = nonEmptyString(record.mimeType);
      const suffix = mimeType
        ? mimeType === "inode/directory"
          ? " (directory)"
          : ` (${mimeType})`
        : "";
      lines.push(`${name} - ${uri}${suffix}`);
    }

    return lines.length > 0 ? lines.join("\n") : "No resources found.";
  }

  return undefined;
}

function extractToolOutputMetadata(
  toolName: string,
  rawResult: unknown,
  rawContent: unknown,
): import("../types.js").ToolOutputMetadata | undefined {
  const candidates = collectResultCandidates(rawResult, rawContent);
  const metadata: import("../types.js").ToolOutputMetadata = {};

  if (toolName === "Bash") {
    for (const candidate of candidates) {
      const hasAssistantAutoBackgrounded = typeof candidate.assistantAutoBackgrounded === "boolean";
      if (hasAssistantAutoBackgrounded) {
        const bashMetadata: import("../types.js").BashOutputMetadata = {};
        bashMetadata.assistant_auto_backgrounded = candidate.assistantAutoBackgrounded as boolean;
        metadata.bash = bashMetadata;
        break;
      }
    }
  }

  if (toolName === "Agent" || toolName === "Task") {
    for (const candidate of candidates) {
      const resolvedModel = nonEmptyString(candidate.resolvedModel);
      if (resolvedModel) {
        const agentMetadata: import("../types.js").AgentOutputMetadata = {
          resolved_model: resolvedModel,
        };
        metadata.agent = agentMetadata;
        break;
      }
    }
  }

  if (toolName === "WebFetch") {
    for (const candidate of candidates) {
      const artifactRead = asRecordOrNull(candidate.artifactRead);
      const slug = nonEmptyString(artifactRead?.slug);
      const ver = nonEmptyString(artifactRead?.ver);
      if (slug && ver) {
        const webFetchMetadata: import("../types.js").WebFetchOutputMetadata = {
          artifact_read: { slug, ver },
        };
        metadata.web_fetch = webFetchMetadata;
        break;
      }
    }
  }

  return metadata.bash || metadata.agent || metadata.web_fetch ? metadata : undefined;
}

export function extractText(value: unknown): string {
  if (typeof value === "string") {
    return value;
  }
  if (Array.isArray(value)) {
    return value
      .map((entry) => {
        if (typeof entry === "string") {
          return entry;
        }
        if (entry && typeof entry === "object" && "text" in entry && typeof entry.text === "string") {
          return entry.text;
        }
        return "";
      })
      .filter((part) => part.length > 0)
      .join("\n");
  }
  if (value && typeof value === "object" && "text" in value && typeof value.text === "string") {
    return value.text;
  }
  return "";
}

const PERSISTED_OUTPUT_OPEN_TAG = "<persisted-output>";
const PERSISTED_OUTPUT_CLOSE_TAG = "</persisted-output>";
const EXPECTED_PREVIEW_LINE = `preview (first ${previewKilobyteLabel(CACHE_SPLIT_POLICY).toLowerCase()}):`;

function extractPersistedOutputInnerText(text: string): string | null {
  const lower = text.toLowerCase();
  const openIdx = lower.indexOf(PERSISTED_OUTPUT_OPEN_TAG);
  if (openIdx < 0) {
    return null;
  }
  const bodyStart = openIdx + PERSISTED_OUTPUT_OPEN_TAG.length;
  const closeIdx = lower.indexOf(PERSISTED_OUTPUT_CLOSE_TAG, bodyStart);
  if (closeIdx < 0) {
    return null;
  }
  return text.slice(bodyStart, closeIdx);
}

function persistedOutputFirstLine(text: string): string | null {
  const inner = extractPersistedOutputInnerText(text);
  if (inner === null) {
    return null;
  }

  for (const line of inner.split(/\r?\n/)) {
    const cleaned = line.replace(/^[\s|│┃║]+/u, "").trim();
    if (cleaned.length > 0) {
      if (cleaned.toLowerCase() === EXPECTED_PREVIEW_LINE) {
        continue;
      }
      return cleaned;
    }
  }
  return null;
}

/**
 * Replace verbose SDK-internal tool rejection messages with short user-facing text.
 * The SDK sends these as tool result content meant for Claude, not for the user.
 */
const USER_REJECTED_TOOL_USE_EXACT =
  "The user doesn't want to proceed with this tool use. The tool use was rejected (eg. if it was a file edit, the new_string was NOT written to the file). STOP what you are doing and wait for the user to tell you how to proceed.";
const USER_REJECTED_TOOL_USE_PREFIX =
  "The user doesn't want to proceed with this tool use. The tool use was rejected (eg. if it was a file edit, the new_string was NOT written to the file). To tell you how to proceed, the user said:";
const PERMISSION_DENIED_TOOL_USE_EXACT =
  "Permission for this tool use was denied. The tool use was rejected (eg. if it was a file edit, the new_string was NOT written to the file). Try a different approach or report the limitation to complete your task.";
const PERMISSION_DENIED_TOOL_USE_PREFIX =
  "Permission for this tool use was denied. The tool use was rejected (eg. if it was a file edit, the new_string was NOT written to the file). The user said:";

function sanitizeSdkRejectionText(text: string): string {
  const normalized = text.trim();
  if (
    normalized === USER_REJECTED_TOOL_USE_EXACT ||
    normalized.startsWith(USER_REJECTED_TOOL_USE_PREFIX)
  ) {
    return "Cancelled by user.";
  }
  if (
    normalized === PERMISSION_DENIED_TOOL_USE_EXACT ||
    normalized.startsWith(PERMISSION_DENIED_TOOL_USE_PREFIX)
  ) {
    return "Permission denied.";
  }
  return text;
}

export function normalizeToolResultText(value: unknown, isError = false): string {
  const text = extractText(value);
  if (!text) {
    return "";
  }
  const persistedLine = persistedOutputFirstLine(text);
  const normalized = persistedLine || text;
  if (!isError) {
    return normalized;
  }
  return sanitizeSdkRejectionText(normalized);
}

function resolveToolName(toolCall: ToolCall | undefined): string {
  const meta = asRecordOrNull(toolCall?.meta);
  const claudeCode = asRecordOrNull(meta?.claudeCode);
  const toolName = claudeCode?.toolName;
  return typeof toolName === "string" ? toolName : "";
}

function writeDiffFromInput(rawInput: Json | undefined): ToolCall["content"] {
  const input = asRecordOrNull(rawInput);
  if (!input) {
    return [];
  }
  const filePath = typeof input.file_path === "string" ? input.file_path : "";
  const content = typeof input.content === "string" ? input.content : "";
  if (!filePath || !content) {
    return [];
  }
  return [{ type: "diff", old_path: filePath, new_path: filePath, old: "", new: content }];
}

function editDiffFromInput(rawInput: Json | undefined): ToolCall["content"] {
  const input = asRecordOrNull(rawInput);
  if (!input) {
    return [];
  }
  const filePath = typeof input.file_path === "string" ? input.file_path : "";
  const oldText =
    typeof input.old_string === "string"
      ? input.old_string
      : typeof input.oldString === "string"
        ? input.oldString
        : "";
  const newText =
    typeof input.new_string === "string"
      ? input.new_string
      : typeof input.newString === "string"
        ? input.newString
        : "";
  if (!filePath || (!oldText && !newText)) {
    return [];
  }
  return [{ type: "diff", old_path: filePath, new_path: filePath, old: oldText, new: newText }];
}

function writeDiffFromResult(rawContent: unknown): ToolCall["content"] {
  const candidates = Array.isArray(rawContent) ? rawContent : [rawContent];
  for (const candidate of candidates) {
    const record = asRecordOrNull(candidate);
    if (!record) {
      continue;
    }
    const filePath =
      typeof record.filePath === "string"
        ? record.filePath
        : typeof record.file_path === "string"
          ? record.file_path
          : "";
    const content = typeof record.content === "string" ? record.content : "";
    const originalRaw =
      "originalFile" in record ? record.originalFile : "original_file" in record ? record.original_file : undefined;
    const gitDiff = asRecordOrNull(record.gitDiff);
    const repository =
      typeof gitDiff?.repository === "string" && gitDiff.repository.trim().length > 0
        ? gitDiff.repository.trim()
        : undefined;
    if (!filePath || !content || originalRaw === undefined) {
      continue;
    }
    const original = typeof originalRaw === "string" ? originalRaw : originalRaw === null ? "" : "";
    return [
      {
        type: "diff",
        old_path: filePath,
        new_path: filePath,
        old: original,
        new: content,
        ...(repository ? { repository } : {}),
      },
    ];
  }
  return [];
}

function editDiffFromResult(rawResult: unknown, rawInput: Json | undefined): ToolCall["content"] {
  const input = asRecordOrNull(rawInput);
  const filePath = typeof input?.file_path === "string" ? input.file_path : "";
  const oldText =
    typeof input?.old_string === "string"
      ? input.old_string
      : typeof input?.oldString === "string"
        ? input.oldString
        : "";
  const newText =
    typeof input?.new_string === "string"
      ? input.new_string
      : typeof input?.newString === "string"
        ? input.newString
        : "";
  if (!filePath || (!oldText && !newText)) {
    return [];
  }

  for (const candidate of resultRecordCandidates(rawResult, undefined)) {
    const candidatePath =
      typeof candidate.filePath === "string"
        ? candidate.filePath
        : typeof candidate.file_path === "string"
          ? candidate.file_path
          : "";
    const gitDiff = asRecordOrNull(candidate.gitDiff);
    if (!candidatePath && !gitDiff) {
      continue;
    }
    if (candidatePath && candidatePath !== filePath) {
      continue;
    }
    const repository =
      typeof gitDiff?.repository === "string" && gitDiff.repository.trim().length > 0
        ? gitDiff.repository.trim()
        : undefined;
    return [
      {
        type: "diff",
        old_path: filePath,
        new_path: filePath,
        old: oldText,
        new: newText,
        ...(repository ? { repository } : {}),
      },
    ];
  }

  return editDiffFromInput(rawInput);
}

function findShellResultRecord(
  rawResult: unknown,
  rawContent: unknown,
): Record<string, unknown> | undefined {
  return resultRecordCandidates(rawResult, rawContent).find(
    (candidate) =>
      "stdout" in candidate ||
      "stderr" in candidate ||
      "backgroundTaskId" in candidate ||
      "backgroundedByUser" in candidate ||
      "assistantAutoBackgrounded" in candidate,
  );
}

function shellBackgroundMessage(record: Record<string, unknown>): string {
  const backgroundTaskId =
    typeof record.backgroundTaskId === "string" ? record.backgroundTaskId : "";
  if (!backgroundTaskId) {
    return "";
  }
  if (record.assistantAutoBackgrounded === true) {
    return `Command was auto-backgrounded by assistant mode with ID: ${backgroundTaskId}.`;
  }
  if (record.backgroundedByUser === true) {
    return `Command was backgrounded by user with ID: ${backgroundTaskId}.`;
  }
  return `Command is running in background with ID: ${backgroundTaskId}.`;
}

function buildShellDisplayOutput(record: Record<string, unknown>): string {
  const segments: string[] = [];
  const stdout = typeof record.stdout === "string" ? record.stdout : "";
  const stderr = typeof record.stderr === "string" ? record.stderr : "";
  if (stdout) {
    segments.push(stdout);
  }
  if (stderr) {
    segments.push(stderr);
  }
  if (record.interrupted === true) {
    segments.push("Command was aborted before completion.");
  }
  const backgroundMessage = shellBackgroundMessage(record);
  if (backgroundMessage) {
    segments.push(backgroundMessage);
  }
  return segments.join("\n");
}

function fileUnchangedResultText(rawResult: unknown, rawContent: unknown): string {
  for (const candidate of resultRecordCandidates(rawResult, rawContent)) {
    if (candidate.type !== "file_unchanged") {
      continue;
    }
    const file = asRecordOrNull(candidate.file);
    const filePath = typeof file?.filePath === "string" ? file.filePath.trim() : "";
    if (filePath) {
      return `File unchanged: ${filePath}`;
    }
  }
  return "";
}

function agentTitleFromAgentOutput(rawResult: unknown, rawContent: unknown, base?: ToolCall): string {
  const inputAgentName = nonEmptyString(asRecordOrNull(base?.raw_input)?.name);
  if (inputAgentName) {
    return "";
  }
  for (const candidate of resultRecordCandidates(rawResult, rawContent)) {
    const agentType = typeof candidate.agentType === "string" ? candidate.agentType.trim() : "";
    if (agentType) {
      return `Agent: ${agentType}`;
    }
  }
  return "";
}

function firstSearchRecord(toolName: string, rawResult: unknown, rawContent: unknown): Record<string, unknown> | undefined {
  if (toolName !== "Glob" && toolName !== "Grep") {
    return undefined;
  }

  const candidates = resultRecordCandidates(rawResult, rawContent);
  for (const parsed of [parseJsonCandidate(rawResult), parseJsonCandidate(rawContent)]) {
    candidates.push(...resultRecordCandidates(parsed, undefined));
  }

  return candidates.find((candidate) => {
    if (toolName === "Glob") {
      return Array.isArray(candidate.filenames) || "numFiles" in candidate || "truncated" in candidate;
    }
    return (
      Array.isArray(candidate.filenames) ||
      "numFiles" in candidate ||
      "content" in candidate ||
      "numLines" in candidate ||
      "numMatches" in candidate
    );
  });
}

function recordNumber(record: Record<string, unknown>, key: string): number | undefined {
  const value = record[key];
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

function recordString(record: Record<string, unknown>, key: string): string | undefined {
  const value = record[key];
  return typeof value === "string" ? value : undefined;
}

function searchFilenames(record: Record<string, unknown>): string[] {
  return Array.isArray(record.filenames)
    ? record.filenames.filter((entry): entry is string => typeof entry === "string" && entry.trim().length > 0)
    : [];
}

function pluralize(count: number, singular: string, plural = `${singular}s`): string {
  return count === 1 ? singular : plural;
}

function truncateList(values: string[], limit: number): { visible: string[]; hidden: number } {
  if (values.length <= limit) {
    return { visible: values, hidden: 0 };
  }
  return { visible: values.slice(0, limit), hidden: values.length - limit };
}

function globResultText(record: Record<string, unknown>): string | undefined {
  const filenames = searchFilenames(record);
  const numFiles = recordNumber(record, "numFiles") ?? filenames.length;
  const truncated = record.truncated === true;
  const lines: string[] = [];

  if (numFiles === 0 && filenames.length === 0) {
    lines.push("No files found");
  } else {
    lines.push(`${numFiles} ${pluralize(numFiles, "file")} found${truncated ? " (truncated)" : ""}`);
  }

  if (filenames.length > 0) {
    const { visible, hidden } = truncateList(filenames, 20);
    lines.push(...visible);
    if (hidden > 0) {
      lines.push(`... ${hidden} more ${pluralize(hidden, "file")} hidden`);
    }
  }

  const durationMs = recordNumber(record, "durationMs");
  if (durationMs !== undefined) {
    lines.push(`Duration: ${durationMs}ms`);
  }

  return lines.join("\n");
}

function grepResultText(record: Record<string, unknown>): string | undefined {
  const filenames = searchFilenames(record);
  const content = recordString(record, "content") ?? "";
  const numFiles = recordNumber(record, "numFiles") ?? filenames.length;
  const numLines = recordNumber(record, "numLines");
  const numMatches = recordNumber(record, "numMatches");
  const appliedLimit = recordNumber(record, "appliedLimit");
  const appliedOffset = recordNumber(record, "appliedOffset");
  const mode = searchModeLabel(record.mode) || "files";
  const lines: string[] = [];

  if (content.trim().length > 0) {
    lines.push(content);
  } else if (filenames.length > 0) {
    const { visible, hidden } = truncateList(filenames, 20);
    lines.push(...visible);
    if (hidden > 0) {
      lines.push(`... ${hidden} more ${pluralize(hidden, "file")} hidden`);
    }
  } else {
    lines.push("No matches found");
  }

  const summaryParts: string[] = [];
  summaryParts.push(`${numFiles} ${pluralize(numFiles, "file")}`);
  if (numMatches !== undefined) {
    summaryParts.push(`${numMatches} ${pluralize(numMatches, "match", "matches")}`);
  }
  if (numLines !== undefined) {
    summaryParts.push(`${numLines} ${pluralize(numLines, "line")}`);
  }
  summaryParts.push(`mode ${mode}`);
  if (appliedLimit !== undefined) {
    summaryParts.push(`limit ${appliedLimit}`);
  }
  if (appliedOffset !== undefined && appliedOffset > 0) {
    summaryParts.push(`offset ${appliedOffset}`);
  }

  if (summaryParts.length > 0) {
    lines.push(`Summary: ${summaryParts.join(", ")}`);
  }

  return lines.join("\n");
}

function searchResultText(toolName: string, rawResult: unknown, rawContent: unknown): string | undefined {
  const record = firstSearchRecord(toolName, rawResult, rawContent);
  if (!record) {
    return undefined;
  }
  return toolName === "Glob" ? globResultText(record) : grepResultText(record);
}

function worktreeResultFields(
  toolName: string,
  rawResult: unknown,
  rawContent: unknown,
): { output?: string } | undefined {
  if (toolName !== "EnterWorktree" && toolName !== "ExitWorktree") {
    return undefined;
  }

  const candidates = resultRecordCandidates(rawResult, rawContent);
  for (const parsed of [parseJsonCandidate(rawResult), parseJsonCandidate(rawContent)]) {
    candidates.push(...resultRecordCandidates(parsed, undefined));
  }

  for (const candidate of candidates) {
    const branch =
      typeof candidate.worktreeBranch === "string" ? candidate.worktreeBranch.trim() : "";
    const path = typeof candidate.worktreePath === "string" ? candidate.worktreePath.trim() : "";
    const output = branch ? `Branch: ${branch}` : path ? `Path: ${path}` : "";
    const isStructuredWorktreeOutput =
      "message" in candidate ||
      "worktreeBranch" in candidate ||
      "worktreePath" in candidate ||
      "originalCwd" in candidate;
    if (output || isStructuredWorktreeOutput) {
      return output ? { output } : {};
    }
  }

  return undefined;
}

function booleanLabel(value: boolean): string {
  return value ? "yes" : "no";
}

function pushBooleanField(lines: string[], label: string, value: unknown): void {
  if (typeof value === "boolean") {
    lines.push(`${label}: ${booleanLabel(value)}`);
  }
}

function nonEmptyString(value: unknown): string | undefined {
  return typeof value === "string" && value.trim() ? value.trim() : undefined;
}

const CRON_MONTH_NAMES = [
  "January",
  "February",
  "March",
  "April",
  "May",
  "June",
  "July",
  "August",
  "September",
  "October",
  "November",
  "December",
] as const;

const CRON_WEEKDAY_NAMES = [
  "Sunday",
  "Monday",
  "Tuesday",
  "Wednesday",
  "Thursday",
  "Friday",
  "Saturday",
] as const;

const CRON_MONTH_ALIASES = new Map(
  ["JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC"].map(
    (name, index) => [name, index + 1],
  ),
);

const CRON_WEEKDAY_ALIASES = new Map(
  ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"].map((name, index) => [name, index]),
);

type CronField =
  | { kind: "any"; raw: string }
  | { kind: "single"; raw: string; value: number }
  | { kind: "step"; raw: string; step: number }
  | { kind: "list"; raw: string; values: number[] }
  | { kind: "range"; raw: string; start: number; end: number }
  | { kind: "unsupported"; raw: string };

function parseCronValue(
  value: string,
  min: number,
  max: number,
  aliases?: Map<string, number>,
): number | undefined {
  const normalized = value.trim().toUpperCase();
  const aliased = aliases?.get(normalized);
  const parsed = aliased ?? Number(normalized);
  if (!Number.isInteger(parsed) || parsed < min || parsed > max) {
    return undefined;
  }
  return parsed;
}

function parseCronField(
  rawField: string,
  min: number,
  max: number,
  aliases?: Map<string, number>,
): CronField {
  const raw = rawField.trim();
  if (raw === "*") {
    return { kind: "any", raw };
  }
  const stepMatch = raw.match(/^\*\/(\d+)$/);
  if (stepMatch) {
    const step = Number(stepMatch[1]);
    return Number.isInteger(step) && step > 0 ? { kind: "step", raw, step } : { kind: "unsupported", raw };
  }
  if (raw.includes(",")) {
    const values = raw
      .split(",")
      .map((part) => parseCronValue(part, min, max, aliases))
      .filter((value): value is number => value !== undefined);
    return values.length === raw.split(",").length ? { kind: "list", raw, values } : { kind: "unsupported", raw };
  }
  const rangeMatch = raw.match(/^([^/-]+)-([^/-]+)$/);
  if (rangeMatch) {
    const start = parseCronValue(rangeMatch[1], min, max, aliases);
    const end = parseCronValue(rangeMatch[2], min, max, aliases);
    return start !== undefined && end !== undefined && start <= end
      ? { kind: "range", raw, start, end }
      : { kind: "unsupported", raw };
  }
  const value = parseCronValue(raw, min, max, aliases);
  return value !== undefined ? { kind: "single", raw, value } : { kind: "unsupported", raw };
}

function isCronAny(field: CronField): boolean {
  return field.kind === "any";
}

function isCronUnsupported(...fields: CronField[]): boolean {
  return fields.some((field) => field.kind === "unsupported");
}

function padCronNumber(value: number): string {
  return value.toString().padStart(2, "0");
}

function cronTime(hour: CronField, minute: CronField): string | undefined {
  if (hour.kind !== "single" || minute.kind !== "single") {
    return undefined;
  }
  return `${padCronNumber(hour.value)}:${padCronNumber(minute.value)}`;
}

function pluralUnit(value: number, unit: string): string {
  return value === 1 ? unit : `${unit}s`;
}

function joinEnglishList(values: string[]): string {
  if (values.length <= 2) {
    return values.join(" and ");
  }
  return `${values.slice(0, -1).join(", ")}, and ${values.at(-1)}`;
}

function weekdayName(value: number): string | undefined {
  const normalized = value === 7 ? 0 : value;
  return CRON_WEEKDAY_NAMES[normalized];
}

function weekdayDescription(field: CronField): string | undefined {
  if (field.kind === "single") {
    return weekdayName(field.value);
  }
  if (field.kind === "range" && field.start === 1 && field.end === 5) {
    return "weekday";
  }
  if (field.kind === "range" && field.start === 0 && field.end === 6) {
    return "day";
  }
  if (field.kind === "list") {
    const normalized = [...new Set(field.values.map((value) => (value === 7 ? 0 : value)))].sort(
      (left, right) => left - right,
    );
    if (normalized.length === 2 && normalized[0] === 0 && normalized[1] === 6) {
      return "weekend day";
    }
    const names = normalized.map(weekdayName);
    return names.every((name): name is string => name !== undefined) ? joinEnglishList(names) : undefined;
  }
  return undefined;
}

function monthName(value: number): string | undefined {
  return CRON_MONTH_NAMES[value - 1];
}

function hourlyScheduleText(minute: CronField): string | undefined {
  if (minute.kind !== "single") {
    return undefined;
  }
  return minute.value === 0 ? "Every hour on the hour" : `Every hour at minute ${padCronNumber(minute.value)}`;
}

function cronScheduleFromExpression(cron: string): string | undefined {
  const parts = cron.trim().split(/\s+/);
  if (parts.length !== 5) {
    return undefined;
  }

  const [minute, hour, dayOfMonth, month, dayOfWeek] = [
    parseCronField(parts[0], 0, 59),
    parseCronField(parts[1], 0, 23),
    parseCronField(parts[2], 1, 31),
    parseCronField(parts[3], 1, 12, CRON_MONTH_ALIASES),
    parseCronField(parts[4], 0, 7, CRON_WEEKDAY_ALIASES),
  ];
  if (isCronUnsupported(minute, hour, dayOfMonth, month, dayOfWeek)) {
    return undefined;
  }

  const everyDay = isCronAny(dayOfMonth) && isCronAny(month) && isCronAny(dayOfWeek);
  if (everyDay && minute.kind === "any" && hour.kind === "any") {
    return "Every minute";
  }
  if (everyDay && minute.kind === "step" && hour.kind === "any") {
    return `Every ${minute.step} ${pluralUnit(minute.step, "minute")}`;
  }
  if (everyDay && minute.kind === "single" && hour.kind === "any") {
    return hourlyScheduleText(minute);
  }
  if (everyDay && minute.kind === "single" && hour.kind === "step") {
    const suffix = minute.value === 0 ? "on the hour" : `at minute ${padCronNumber(minute.value)}`;
    return `Every ${hour.step} ${pluralUnit(hour.step, "hour")} ${suffix}`;
  }

  const time = cronTime(hour, minute);
  if (!time) {
    return undefined;
  }

  if (everyDay) {
    return `Every day at ${time}`;
  }

  if (isCronAny(dayOfMonth) && isCronAny(month) && !isCronAny(dayOfWeek)) {
    const weekday = weekdayDescription(dayOfWeek);
    return weekday ? `Every ${weekday} at ${time}` : undefined;
  }

  if (isCronAny(month) && isCronAny(dayOfWeek)) {
    if (dayOfMonth.kind === "single") {
      return `Every month on day ${dayOfMonth.value} at ${time}`;
    }
    if (dayOfMonth.kind === "step") {
      return `Every ${dayOfMonth.step} ${pluralUnit(dayOfMonth.step, "day")} at ${time}`;
    }
  }

  if (dayOfMonth.kind === "single" && isCronAny(dayOfWeek)) {
    if (month.kind === "single") {
      const monthLabel = monthName(month.value);
      return monthLabel ? `Every ${monthLabel} ${dayOfMonth.value} at ${time}` : undefined;
    }
    if (month.kind === "step") {
      return `Every ${month.step} ${pluralUnit(month.step, "month")} on day ${dayOfMonth.value} at ${time}`;
    }
  }

  if (isCronAny(dayOfMonth) && month.kind === "single" && isCronAny(dayOfWeek)) {
    const monthLabel = monthName(month.value);
    return monthLabel ? `Every day in ${monthLabel} at ${time}` : undefined;
  }

  return undefined;
}

function normalizeHumanSchedule(value: unknown): string | undefined {
  const text = nonEmptyString(value);
  if (!text) {
    return undefined;
  }
  const hourlyMinute = text.match(/^Every hour at :(\d{1,2})$/i);
  if (hourlyMinute) {
    const minute = Number(hourlyMinute[1]);
    if (Number.isInteger(minute) && minute >= 0 && minute <= 59) {
      return hourlyScheduleText({ kind: "single", raw: hourlyMinute[1], value: minute });
    }
  }
  return text;
}

function readableCronSchedule(cron: unknown, humanSchedule: unknown): string | undefined {
  const cronText = nonEmptyString(cron);
  if (cronText) {
    const derived = cronScheduleFromExpression(cronText);
    if (derived) {
      return derived;
    }
  }
  return normalizeHumanSchedule(humanSchedule);
}

function cronCreateResultText(
  candidate: Record<string, unknown>,
  rawInput: Json | undefined,
): string | undefined {
  const input = asRecordOrNull(rawInput);
  const schedule = readableCronSchedule(input?.cron, candidate.humanSchedule);
  if (
    typeof candidate.id !== "string" ||
    typeof candidate.recurring !== "boolean" ||
    !schedule
  ) {
    return undefined;
  }

  const lines = [
    `Schedule ID: ${candidate.id}`,
    `Schedule: ${schedule}`,
    `Recurring: ${booleanLabel(candidate.recurring)}`,
  ];
  pushBooleanField(lines, "Durable", candidate.durable);
  return lines.join("\n");
}

function cronDeleteResultText(candidate: Record<string, unknown>): string | undefined {
  return typeof candidate.id === "string" ? `Schedule ID: ${candidate.id}` : undefined;
}

function cronListResultText(candidate: Record<string, unknown>): string | undefined {
  if (!Array.isArray(candidate.jobs)) {
    return undefined;
  }
  const jobs = candidate.jobs.map(asRecordOrNull).filter((job): job is Record<string, unknown> => job !== null);
  if (jobs.length === 0) {
    return "Jobs: none";
  }

  const lines: string[] = [];
  if (jobs.length === 1) {
    const [job] = jobs;
    if (typeof job.id === "string") {
      lines.push(`Schedule ID: ${job.id}`);
    }
    if (typeof job.cron === "string" && job.cron.trim()) {
      lines.push(`Cron: ${job.cron.trim()}`);
    }
    const schedule = readableCronSchedule(job.cron, job.humanSchedule);
    if (schedule) {
      lines.push(`Schedule: ${schedule}`);
    }
    if (typeof job.prompt === "string") {
      lines.push(`Prompt: ${job.prompt}`);
    }
    pushBooleanField(lines, "Recurring", job.recurring);
    pushBooleanField(lines, "Durable", job.durable);
  }

  if (jobs.length > 1) {
    for (const [index, job] of jobs.entries()) {
      if (typeof job.id === "string") {
        lines.push(`Schedule ID: ${job.id}`);
      }
      const schedule = readableCronSchedule(job.cron, job.humanSchedule);
      if (schedule) {
        lines.push(`Schedule: ${schedule}`);
      } else if (typeof job.cron === "string" && job.cron.trim()) {
        lines.push(`Cron: ${job.cron.trim()}`);
      }
      if (typeof job.prompt === "string") {
        lines.push(`Prompt: ${job.prompt}`);
      }
      if (index < jobs.length - 1) {
        lines.push(CRON_LIST_DIVIDER);
      }
    }
  }

  return lines.length > 0 ? lines.join("\n") : "Jobs: none";
}

function cronResultText(
  toolName: string,
  rawResult: unknown,
  rawContent: unknown,
  rawInput: Json | undefined,
): string | undefined {
  if (!isCronToolName(toolName)) {
    return undefined;
  }

  const candidates = resultRecordCandidates(rawResult, rawContent);
  for (const parsed of [parseJsonCandidate(rawResult), parseJsonCandidate(rawContent)]) {
    candidates.push(...resultRecordCandidates(parsed, undefined));
  }

  for (const candidate of candidates) {
    const output =
      toolName === "CronCreate"
        ? cronCreateResultText(candidate, rawInput)
        : toolName === "CronDelete"
          ? cronDeleteResultText(candidate)
          : cronListResultText(candidate);
    if (output !== undefined) {
      return output;
    }
  }

  return undefined;
}

function formatDurationSeconds(seconds: number): string {
  const rounded = Math.max(0, Math.trunc(seconds));
  const hours = Math.floor(rounded / 3600);
  const minutes = Math.floor((rounded % 3600) / 60);
  const remainingSeconds = rounded % 60;
  const parts: string[] = [];
  if (hours > 0) {
    parts.push(`${hours}h`);
  }
  if (minutes > 0) {
    parts.push(`${minutes}m`);
  }
  if (remainingSeconds > 0 || parts.length === 0) {
    parts.push(`${remainingSeconds}s`);
  }
  return parts.join(" ");
}

function formatDurationMilliseconds(milliseconds: number): string {
  const rounded = Math.max(0, Math.trunc(milliseconds));
  if (rounded < 1000) {
    return `${rounded}ms`;
  }
  return formatDurationSeconds(rounded / 1000);
}

function formatLocalTimestamp(epochMs: number): string | undefined {
  if (!Number.isFinite(epochMs)) {
    return undefined;
  }
  const date = new Date(epochMs);
  if (!Number.isFinite(date.getTime())) {
    return undefined;
  }
  const year = date.getFullYear().toString().padStart(4, "0");
  const month = (date.getMonth() + 1).toString().padStart(2, "0");
  const day = date.getDate().toString().padStart(2, "0");
  const hour = date.getHours().toString().padStart(2, "0");
  const minute = date.getMinutes().toString().padStart(2, "0");
  const second = date.getSeconds().toString().padStart(2, "0");
  return `${year}-${month}-${day} ${hour}:${minute}:${second} local`;
}

function formatIsoTimestamp(value: unknown): string | undefined {
  const raw = nonEmptyString(value);
  if (!raw) {
    return undefined;
  }
  const parsed = Date.parse(raw);
  return Number.isFinite(parsed) ? formatLocalTimestamp(parsed) : raw;
}

function scheduleWakeupResultText(
  toolName: string,
  rawResult: unknown,
  rawContent: unknown,
): string | undefined {
  if (toolName !== SCHEDULE_WAKEUP_TOOL_NAME) {
    return undefined;
  }

  const candidates = resultRecordCandidates(rawResult, rawContent);
  for (const parsed of [parseJsonCandidate(rawResult), parseJsonCandidate(rawContent)]) {
    candidates.push(...resultRecordCandidates(parsed, undefined));
  }

  for (const candidate of candidates) {
    const scheduledFor =
      typeof candidate.scheduledFor === "number"
        ? formatLocalTimestamp(candidate.scheduledFor)
        : undefined;
    const clampedDelaySeconds =
      typeof candidate.clampedDelaySeconds === "number"
        ? formatDurationSeconds(candidate.clampedDelaySeconds)
        : undefined;
    if (!scheduledFor || !clampedDelaySeconds || typeof candidate.wasClamped !== "boolean") {
      continue;
    }
    return [
      `Scheduled for: ${scheduledFor}`,
      `Actual delay: ${clampedDelaySeconds}`,
      `Clamped: ${booleanLabel(candidate.wasClamped)}`,
    ].join("\n");
  }

  return undefined;
}

function pushNotificationDisabledReason(value: unknown): string | undefined {
  switch (value) {
    case "config_off":
      return "notifications disabled";
    case "user_present":
      return "user present";
    case "no_transport":
      return "no notification transport";
    default:
      return undefined;
  }
}

function pushNotificationResultText(
  toolName: string,
  rawResult: unknown,
  rawContent: unknown,
  rawInput: Json | undefined,
): string | undefined {
  if (toolName !== PUSH_NOTIFICATION_TOOL_NAME) {
    return undefined;
  }

  const inputMessage = nonEmptyString(asRecordOrNull(rawInput)?.message);
  const candidates = resultRecordCandidates(rawResult, rawContent);
  for (const parsed of [parseJsonCandidate(rawResult), parseJsonCandidate(rawContent)]) {
    candidates.push(...resultRecordCandidates(parsed, undefined));
  }

  for (const candidate of candidates) {
    const isStructuredPushOutput =
      "message" in candidate ||
      "pushSent" in candidate ||
      "localSent" in candidate ||
      "disabledReason" in candidate ||
      "idleSec" in candidate ||
      "hasFocus" in candidate ||
      "sentAt" in candidate;
    if (!isStructuredPushOutput) {
      continue;
    }

    const lines: string[] = [];
    const outputMessage = nonEmptyString(candidate.message);
    if (outputMessage && outputMessage !== inputMessage) {
      lines.push(`Result: ${outputMessage}`);
    }
    pushBooleanField(lines, "Push sent", candidate.pushSent);
    pushBooleanField(lines, "Local sent", candidate.localSent);
    const disabledReason = pushNotificationDisabledReason(candidate.disabledReason);
    if (disabledReason) {
      lines.push(`Disabled reason: ${disabledReason}`);
    }
    if (typeof candidate.idleSec === "number" && Number.isFinite(candidate.idleSec)) {
      lines.push(`Idle time: ${formatDurationSeconds(candidate.idleSec)}`);
    }
    pushBooleanField(lines, "App focused", candidate.hasFocus);
    const sentAt = formatIsoTimestamp(candidate.sentAt);
    if (sentAt) {
      lines.push(`Sent at: ${sentAt}`);
    }
    return lines.join("\n");
  }

  return undefined;
}

function compactJson(value: unknown): string | undefined {
  if (value === undefined || value === null) {
    return undefined;
  }
  if (typeof value === "string") {
    return value.trim() ? value : undefined;
  }
  if (Array.isArray(value) && value.length === 0) {
    return undefined;
  }
  const record = asRecordOrNull(value);
  if (record && Object.keys(record).length === 0) {
    return undefined;
  }
  try {
    return JSON.stringify(value);
  } catch {
    return undefined;
  }
}

function compactParsedJsonString(value: string): string | undefined {
  const trimmed = value.trim();
  if (!trimmed) {
    return undefined;
  }
  try {
    return JSON.stringify(JSON.parse(trimmed));
  } catch {
    return trimmed;
  }
}

function remoteTriggerResultFields(
  toolName: string,
  rawResult: unknown,
  rawContent: unknown,
): { output?: string; failed: boolean } | undefined {
  if (toolName !== REMOTE_TRIGGER_TOOL_NAME) {
    return undefined;
  }

  const candidates = resultRecordCandidates(rawResult, rawContent);
  for (const parsed of [parseJsonCandidate(rawResult), parseJsonCandidate(rawContent)]) {
    candidates.push(...resultRecordCandidates(parsed, undefined));
  }

  for (const candidate of candidates) {
    if (typeof candidate.status !== "number" || typeof candidate.json !== "string") {
      continue;
    }

    const lines = [`Status: ${candidate.status}`];
    const summary = nonEmptyString(candidate.summary);
    if (summary) {
      lines.push(`Summary: ${summary}`);
    } else {
      const response = compactParsedJsonString(candidate.json);
      if (response) {
        lines.push(`Response: ${response}`);
      }
    }

    return { output: lines.join("\n"), failed: candidate.status >= 400 };
  }

  return undefined;
}

function hasConcreteReplField(record: Record<string, unknown>): boolean {
  return (
    "code" in record ||
    "error" in record ||
    "stdout" in record ||
    "stderr" in record ||
    "registeredTools" in record ||
    "images" in record ||
    "documents" in record
  );
}

function isReplWrapperRecord(record: Record<string, unknown>): boolean {
  const nestedResult = asRecordOrNull(record.result);
  return Boolean(nestedResult && hasConcreteReplField(nestedResult));
}

function isReplOutputRecord(record: Record<string, unknown>): boolean {
  if (isReplWrapperRecord(record)) {
    return false;
  }
  return hasConcreteReplField(record) || "result" in record;
}

function replResultFields(
  toolName: string,
  rawResult: unknown,
  rawContent: unknown,
): { output?: string; failed: boolean } | undefined {
  if (toolName !== REPL_TOOL_NAME) {
    return undefined;
  }

  const candidates = resultRecordCandidates(rawResult, rawContent);
  for (const parsed of [parseJsonCandidate(rawResult), parseJsonCandidate(rawContent)]) {
    candidates.push(...resultRecordCandidates(parsed, undefined));
  }

  for (const candidate of candidates) {
    if (!isReplOutputRecord(candidate)) {
      continue;
    }

    const lines: string[] = [];
    let failed = false;
    if ("error" in candidate) {
      failed = true;
      const error = compactJson(candidate.error);
      if (error) {
        lines.push(`Error: ${error}`);
      }
    }

    const stdout = typeof candidate.stdout === "string" ? candidate.stdout : "";
    if (stdout) {
      lines.push(`Stdout: ${stdout}`);
    }
    const stderr = typeof candidate.stderr === "string" ? candidate.stderr : "";
    if (stderr) {
      lines.push(`Stderr: ${stderr}`);
    }
    const result = compactJson(candidate.result);
    if (result) {
      lines.push(`Result: ${result}`);
    }

    if (Array.isArray(candidate.registeredTools)) {
      const registeredTools = candidate.registeredTools.filter(
        (tool): tool is string => typeof tool === "string" && tool.trim().length > 0,
      );
      if (registeredTools.length > 0) {
        lines.push(`Registered tools: ${registeredTools.join(", ")}`);
      }
    }
    if (Array.isArray(candidate.images) && candidate.images.length > 0) {
      lines.push(`Images: ${candidate.images.length}`);
    }
    if (Array.isArray(candidate.documents) && candidate.documents.length > 0) {
      lines.push(`Documents: ${candidate.documents.length}`);
    }

    return { output: lines.length > 0 ? lines.join("\n") : undefined, failed };
  }

  return undefined;
}

type BackgroundLaunchResult = {
  output?: string;
  taskId?: string;
  failed: boolean;
  keepRunning: boolean;
};

function collectResultCandidates(rawResult: unknown, rawContent: unknown): Record<string, unknown>[] {
  const candidates = resultRecordCandidates(rawResult, rawContent);
  for (const parsed of [parseJsonCandidate(rawResult), parseJsonCandidate(rawContent)]) {
    candidates.push(...resultRecordCandidates(parsed, undefined));
  }
  return candidates;
}

function monitorResultFields(
  toolName: string,
  rawResult: unknown,
  rawContent: unknown,
): BackgroundLaunchResult | undefined {
  if (toolName !== MONITOR_TOOL_NAME) {
    return undefined;
  }

  for (const candidate of collectResultCandidates(rawResult, rawContent)) {
    const taskId = nonEmptyString(candidate.taskId);
    const timeoutMs =
      typeof candidate.timeoutMs === "number" && Number.isFinite(candidate.timeoutMs)
        ? Math.max(0, Math.trunc(candidate.timeoutMs))
        : undefined;
    const persistent =
      typeof candidate.persistent === "boolean"
        ? candidate.persistent
        : timeoutMs === 0
          ? true
          : timeoutMs !== undefined
            ? false
            : undefined;
    const isStructuredMonitorOutput =
      taskId !== undefined || timeoutMs !== undefined || persistent !== undefined;
    if (!isStructuredMonitorOutput) {
      continue;
    }

    const lines: string[] = [];
    if (taskId) {
      lines.push(`Task ID: ${taskId}`);
    }
    if (persistent !== undefined) {
      lines.push(`Persistent: ${booleanLabel(persistent)}`);
    }
    if (timeoutMs !== undefined && persistent !== true) {
      lines.push(`Timeout: ${formatDurationMilliseconds(timeoutMs)}`);
    }
    return {
      output: lines.length > 0 ? lines.join("\n") : undefined,
      taskId,
      failed: false,
      keepRunning: Boolean(taskId),
    };
  }

  return undefined;
}

function workflowStatusLabel(value: unknown): string | undefined {
  switch (value) {
    case "async_launched":
      return "async launched";
    case "remote_launched":
      return "remote launched";
    default:
      return nonEmptyString(value);
  }
}

function workflowResultFields(
  toolName: string,
  rawResult: unknown,
  rawContent: unknown,
): BackgroundLaunchResult | undefined {
  if (toolName !== WORKFLOW_TOOL_NAME) {
    return undefined;
  }

  for (const candidate of collectResultCandidates(rawResult, rawContent)) {
    const taskId = nonEmptyString(candidate.taskId);
    const status = workflowStatusLabel(candidate.status);
    const error = nonEmptyString(candidate.error);
    const taskType = nonEmptyString(candidate.taskType);
    const workflowName = nonEmptyString(candidate.workflowName);
    const isStructuredWorkflowOutput =
      status !== undefined ||
      taskId !== undefined ||
      taskType !== undefined ||
      workflowName !== undefined ||
      "runId" in candidate ||
      "summary" in candidate ||
      "transcriptDir" in candidate ||
      "scriptPath" in candidate ||
      "sessionUrl" in candidate ||
      "warning" in candidate ||
      "error" in candidate;
    if (!isStructuredWorkflowOutput) {
      continue;
    }

    const lines: string[] = [];
    if (status) {
      lines.push(`Status: ${status}`);
    }
    if (taskId) {
      lines.push(`Task ID: ${taskId}`);
    }
    if (taskType) {
      lines.push(`Task type: ${taskType}`);
    }
    if (workflowName) {
      lines.push(`Workflow name: ${workflowName}`);
    }
    const runId = nonEmptyString(candidate.runId);
    if (runId) {
      lines.push(`Run ID: ${runId}`);
    }
    const summary = nonEmptyString(candidate.summary);
    if (summary) {
      lines.push(`Summary: ${summary}`);
    }
    const transcriptDir = nonEmptyString(candidate.transcriptDir);
    if (transcriptDir) {
      lines.push(`Transcript dir: ${transcriptDir}`);
    }
    const scriptPath = nonEmptyString(candidate.scriptPath);
    if (scriptPath) {
      lines.push(`Script path: ${scriptPath}`);
    }
    const sessionUrl = nonEmptyString(candidate.sessionUrl);
    if (sessionUrl) {
      lines.push(`Session URL: ${sessionUrl}`);
    }
    const warning = nonEmptyString(candidate.warning);
    if (warning) {
      lines.push(`Warning: ${warning}`);
    }
    if (error) {
      lines.push(`Error: ${error}`);
    }

    return {
      output: lines.length > 0 ? lines.join("\n") : undefined,
      taskId,
      failed: Boolean(error),
      keepRunning: Boolean(taskId && !error),
    };
  }

  return undefined;
}

function backgroundLaunchResultFields(
  toolName: string,
  rawResult: unknown,
  rawContent: unknown,
): BackgroundLaunchResult | undefined {
  return (
    monitorResultFields(toolName, rawResult, rawContent) ??
    workflowResultFields(toolName, rawResult, rawContent)
  );
}

export function backgroundToolLaunchTaskIdFromResult(
  toolName: string,
  rawResult: unknown,
  rawContent: unknown,
): string | undefined {
  const result = backgroundLaunchResultFields(toolName, rawResult, rawContent);
  return result?.keepRunning ? result.taskId : undefined;
}

function enterPlanModeStructuredOutputHandled(
  toolName: string,
  rawResult: unknown,
  rawContent: unknown,
): boolean {
  if (toolName !== ENTER_PLAN_MODE_TOOL_NAME) {
    return false;
  }

  const candidates = resultRecordCandidates(rawResult, rawContent);
  for (const parsed of [parseJsonCandidate(rawResult), parseJsonCandidate(rawContent)]) {
    candidates.push(...resultRecordCandidates(parsed, undefined));
  }

  for (const candidate of candidates) {
    if (typeof candidate.message === "string") {
      return true;
    }
  }

  return false;
}

function mcpResourceReadErrorText(
  toolName: string,
  rawResult: unknown,
  rawContent: unknown,
): string | undefined {
  if (!isMcpResourceReadToolName(toolName)) {
    return undefined;
  }

  for (const candidate of collectResultCandidates(rawResult, rawContent)) {
    const error = nonEmptyString(candidate.error);
    if (error) {
      return `Error: ${error}`;
    }
  }

  return undefined;
}

function webFetchResultText(
  toolName: string,
  rawResult: unknown,
  rawContent: unknown,
): string | undefined {
  if (toolName !== "WebFetch") {
    return undefined;
  }

  for (const candidate of collectResultCandidates(rawResult, rawContent)) {
    const isStructuredWebFetchOutput =
      "result" in candidate ||
      "url" in candidate ||
      "code" in candidate ||
      "codeText" in candidate ||
      "bytes" in candidate ||
      "durationMs" in candidate ||
      "artifactRead" in candidate;
    if (!isStructuredWebFetchOutput) {
      continue;
    }

    const result = nonEmptyString(candidate.result);
    if (result) {
      return result;
    }

    const lines: string[] = [];
    const url = nonEmptyString(candidate.url);
    if (url) {
      lines.push(`URL: ${url}`);
    }
    if (typeof candidate.code === "number" && Number.isFinite(candidate.code)) {
      const codeText = nonEmptyString(candidate.codeText);
      lines.push(`Status: ${candidate.code}${codeText ? ` ${codeText}` : ""}`);
    }
    if (typeof candidate.bytes === "number" && Number.isFinite(candidate.bytes)) {
      lines.push(`Bytes: ${Math.max(0, Math.trunc(candidate.bytes))}`);
    }
    if (typeof candidate.durationMs === "number" && Number.isFinite(candidate.durationMs)) {
      lines.push(`Duration: ${Math.max(0, Math.trunc(candidate.durationMs))}ms`);
    }
    return lines.length > 0 ? lines.join("\n") : undefined;
  }

  return undefined;
}

export function buildToolResultFields(
  isError: boolean,
  rawContent: unknown,
  base?: ToolCall,
  rawResult?: unknown,
  _context: TaskTitleContext = {},
): ToolCallUpdateFields {
  const toolName = resolveToolName(base);
  const fields: ToolCallUpdateFields = {
    status: isError ? "failed" : "completed",
  };
  const outputMetadata = extractToolOutputMetadata(toolName, rawResult, rawContent);
  if (outputMetadata) {
    fields.output_metadata = outputMetadata;
  }
  const fileUnchangedText = !isError && toolName === "Read" ? fileUnchangedResultText(rawResult, rawContent) : "";
  if (fileUnchangedText) {
    fields.raw_output = fileUnchangedText;
    fields.content = [{ type: "content", content: { type: "text", text: fileUnchangedText } }];
    return fields;
  }
  const agentTitle = !isError && toolName === "Agent"
    ? agentTitleFromAgentOutput(rawResult, rawContent, base)
    : "";
  if (agentTitle) {
    fields.title = agentTitle;
  }
  const readMcpResourceError = mcpResourceReadErrorText(toolName, rawResult, rawContent);
  if (readMcpResourceError) {
    fields.status = "failed";
    fields.raw_output = readMcpResourceError;
    fields.content = [{ type: "content", content: { type: "text", text: readMcpResourceError } }];
    return fields;
  }
  const readMcpResourceDirOutput = !isError
    ? mcpResourceDirTextFromResult(toolName, rawResult, rawContent)
    : undefined;
  if (readMcpResourceDirOutput !== undefined) {
    fields.raw_output = readMcpResourceDirOutput;
    fields.content = [
      { type: "content", content: { type: "text", text: readMcpResourceDirOutput } },
    ];
    return fields;
  }
  const searchOutput = !isError ? searchResultText(toolName, rawResult, rawContent) : undefined;
  if (searchOutput !== undefined) {
    fields.raw_output = searchOutput;
    fields.content = [{ type: "content", content: { type: "text", text: searchOutput } }];
    return fields;
  }
  const webFetchOutput = !isError ? webFetchResultText(toolName, rawResult, rawContent) : undefined;
  if (webFetchOutput !== undefined) {
    fields.raw_output = webFetchOutput;
    fields.content = [{ type: "content", content: { type: "text", text: webFetchOutput } }];
    return fields;
  }
  const worktreeOutput = !isError
    ? worktreeResultFields(toolName, rawResult, rawContent)
    : undefined;
  if (worktreeOutput) {
    if (worktreeOutput.output) {
      fields.raw_output = worktreeOutput.output;
      fields.content = [
        { type: "content", content: { type: "text", text: worktreeOutput.output } },
      ];
    }
    return fields;
  }
  const cronOutput = !isError
    ? cronResultText(toolName, rawResult, rawContent, base?.raw_input)
    : undefined;
  if (cronOutput !== undefined) {
    fields.raw_output = cronOutput;
    fields.content = [{ type: "content", content: { type: "text", text: cronOutput } }];
    return fields;
  }
  const scheduleWakeupOutput = !isError
    ? scheduleWakeupResultText(toolName, rawResult, rawContent)
    : undefined;
  if (scheduleWakeupOutput !== undefined) {
    fields.raw_output = scheduleWakeupOutput;
    fields.content = [
      { type: "content", content: { type: "text", text: scheduleWakeupOutput } },
    ];
    return fields;
  }
  const pushNotificationOutput = !isError
    ? pushNotificationResultText(toolName, rawResult, rawContent, base?.raw_input)
    : undefined;
  if (pushNotificationOutput !== undefined) {
    fields.raw_output = pushNotificationOutput;
    fields.content = [
      { type: "content", content: { type: "text", text: pushNotificationOutput } },
    ];
    return fields;
  }
  if (
    !isError &&
    enterPlanModeStructuredOutputHandled(toolName, rawResult, rawContent)
  ) {
    return fields;
  }
  const remoteTriggerOutput = remoteTriggerResultFields(toolName, rawResult, rawContent);
  if (remoteTriggerOutput !== undefined) {
    if (remoteTriggerOutput.failed) {
      fields.status = "failed";
    }
    if (remoteTriggerOutput.output) {
      fields.raw_output = remoteTriggerOutput.output;
      fields.content = [
        { type: "content", content: { type: "text", text: remoteTriggerOutput.output } },
      ];
    }
    return fields;
  }
  const replOutput = replResultFields(toolName, rawResult, rawContent);
  if (replOutput !== undefined) {
    if (replOutput.failed) {
      fields.status = "failed";
    }
    if (replOutput.output) {
      fields.raw_output = replOutput.output;
      fields.content = [
        { type: "content", content: { type: "text", text: replOutput.output } },
      ];
    }
    return fields;
  }
  const backgroundLaunchOutput = !isError
    ? backgroundLaunchResultFields(toolName, rawResult, rawContent)
    : undefined;
  if (backgroundLaunchOutput !== undefined) {
    fields.status = backgroundLaunchOutput.failed
      ? "failed"
      : backgroundLaunchOutput.keepRunning
        ? "in_progress"
        : "completed";
    if (backgroundLaunchOutput.output) {
      fields.raw_output = backgroundLaunchOutput.output;
      fields.content = [
        { type: "content", content: { type: "text", text: backgroundLaunchOutput.output } },
      ];
    }
    return fields;
  }
  const shellResultRecord = isShellToolName(toolName)
    ? findShellResultRecord(rawResult, rawContent)
    : undefined;
  const normalizedRawOutput = normalizeToolResultText(rawContent, isError);
  const rawOutput = shellResultRecord
    ? buildShellDisplayOutput(shellResultRecord)
    : normalizedRawOutput || JSON.stringify(rawContent);
  if (rawOutput && !(isTaskToolName(toolName) && !isError)) {
    fields.raw_output = rawOutput;
  }
  if (!isError && isTaskToolName(toolName)) {
    if (toolName === "TaskUpdate" && taskUpdateSucceeded(rawResult, rawContent) === false) {
      fields.status = "failed";
    }
    const taskOutput = taskToolResultText(toolName, rawResult, rawContent, base?.raw_input);
    if (taskOutput) {
      fields.content = [{ type: "content", content: { type: "text", text: taskOutput } }];
      return fields;
    }
    if (toolName === "TaskCreate" || toolName === "TaskUpdate" || toolName === "TaskOutput" || toolName === "TaskStop") {
      return fields;
    }
  }

  if (!isError && toolName === "Write") {
    const structuredDiff = writeDiffFromResult(rawContent);
    if (structuredDiff.length > 0) {
      fields.content = structuredDiff;
      return fields;
    }
    const inputDiff = writeDiffFromInput(base?.raw_input);
    if (inputDiff.length > 0) {
      fields.content = inputDiff;
      return fields;
    }
  }

  if (!isError && toolName === "Edit") {
    const structuredDiff = editDiffFromResult(rawResult, base?.raw_input);
    if (structuredDiff.length > 0) {
      fields.content = structuredDiff;
      return fields;
    }
    if (base?.content.some((entry) => entry.type === "diff")) {
      return fields;
    }
  }

  if (!isError && toolName === READ_MCP_RESOURCE_TOOL_NAME) {
    const structuredResourceContent = mcpResourceContentFromResult(rawResult, rawContent);
    if (structuredResourceContent.length > 0) {
      fields.content = structuredResourceContent;
      return fields;
    }
  }

  if (rawOutput) {
    fields.content = [{ type: "content", content: { type: "text", text: rawOutput } }];
  }
  return fields;
}

export function unwrapToolUseResult(rawResult: unknown): { isError: boolean; content: unknown } {
  if (!rawResult || typeof rawResult !== "object") {
    return { isError: false, content: rawResult };
  }
  const record = rawResult as Record<string, unknown>;
  const isError =
    (typeof record.is_error === "boolean" && record.is_error) ||
    (typeof record.error === "boolean" && record.error);
  if ("content" in record) {
    return { isError: Boolean(isError), content: record.content };
  }
  if ("result" in record) {
    return { isError: Boolean(isError), content: record.result };
  }
  if ("text" in record) {
    return { isError: Boolean(isError), content: record.text };
  }
  return { isError: Boolean(isError), content: rawResult };
}

