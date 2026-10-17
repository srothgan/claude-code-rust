import { asRecordOrNull } from "./shared.js";

type TranscriptPresentation =
  | { kind: "conversation" }
  | { kind: "hidden" }
  | { kind: "system_reminder"; text: string }
  | { kind: "task_notification"; notification?: Record<string, unknown> }
  | { kind: "command"; text: string }
  | { kind: "command_output"; text: string; severity: "info" | "error" };

export function messageCandidates(raw: unknown): Record<string, unknown>[] {
  const record = asRecordOrNull(raw);
  if (!record) return [];
  const nested = asRecordOrNull(record.message);
  return nested ? [record, nested] : [record];
}

export function messageText(message: Record<string, unknown>): string {
  const content = message.content;
  if (typeof content === "string") return content;
  return Array.isArray(content)
    ? content.flatMap(block => {
      const record = asRecordOrNull(block);
      return record?.type === "text" && typeof record.text === "string" ? [record.text] : [];
    }).join("")
    : "";
}

// Literal searches keep malformed or very large records linear to scan. Tag bodies
// remain literal: command arguments and task summaries may themselves contain XML.
function tagAt(text: string, offset: number, name: string): { body: string; end: number } | undefined {
  const opening = `<${name}>`;
  if (!text.startsWith(opening, offset)) return undefined;
  const start = offset + opening.length;
  const closing = `</${name}>`;
  const end = text.indexOf(closing, start);
  return end < 0 ? undefined : { body: text.slice(start, end), end: end + closing.length };
}

function tagContent(text: string, name: string): string | undefined {
  const offset = text.indexOf(`<${name}>`);
  return offset < 0 ? undefined : tagAt(text, offset, name)?.body;
}

function taskNotification(raw: unknown): Record<string, unknown> | undefined {
  for (const message of messageCandidates(raw)) {
    const body = tagContent(messageText(message), "task-notification");
    if (!body) continue;
    const field = (name: string) => tagContent(body, name)?.trim();
    const taskId = field("task-id");
    const status = field("status");
    // A background agent the user stopped is saved as "killed" (Claude Code 2.1.300).
    if (taskId && (status === "completed" || status === "failed" || status === "stopped" || status === "killed")) {
      return { subtype: "task_notification", task_id: taskId, status, tool_use_id: field("tool-use-id"), summary: field("summary"), output_file: field("output-file") };
    }
  }
  return undefined;
}

function skipWhitespace(text: string, start: number): number {
  let offset = start;
  while (offset < text.length && /\s/u.test(text[offset])) offset++;
  return offset;
}

function commandText(text: string): string | undefined {
  const commands: string[] = [];
  let offset = 0;
  while (offset < text.length) {
    const name = tagAt(text, offset, "command-name");
    if (!name || !/^\/[^\s<>]+$/u.test(name.body.trim())) return undefined;
    const message = tagAt(text, skipWhitespace(text, name.end), "command-message");
    if (!message) return undefined;
    const args = tagAt(text, skipWhitespace(text, message.end), "command-args");
    if (!args) return undefined;
    const argument = args.body.trim();
    commands.push(`${name.body.trim()}${argument ? ` ${argument}` : ""}`);
    offset = skipWhitespace(text, args.end);
  }
  return commands.length ? commands.join("\n") : undefined;
}

/** Classify SDK transcript records once for history display and fork eligibility.
 * This only projects saved content; it never changes model context or executes commands.
 */
export function transcriptPresentation(raw: unknown): TranscriptPresentation {
  const record = asRecordOrNull(raw);
  if (record?.isCompactSummary === true) return { kind: "hidden" };
  if (record?.type !== "user") return { kind: "conversation" };
  const origin = asRecordOrNull(record.origin);
  if (origin?.kind === "task-notification" && origin.subkind === undefined &&
      (origin.producer === undefined || origin.producer === "session-task")) {
    return { kind: "task_notification", notification: taskNotification(record.message) };
  }
  // Explicitly authored and external messages retain their literal content,
  // including examples that happen to use Claude's internal tag names.
  if (origin) return { kind: "conversation" };
  for (const message of messageCandidates(record.message)) {
    if (Array.isArray(message.content) && message.content.some(block => asRecordOrNull(block)?.type !== "text")) continue;
    const text = messageText(message).trim();
    // The SDK marks a reminder it adds to the context as metadata, which a
    // SessionMessage carries as `is_meta` (getSubagentMessages keeps such
    // records). A prompt is never marked, so a user who writes these tags keeps them.
    const reminder = tagAt(text, 0, "system-reminder");
    if (record.is_meta === true && reminder?.end === text.length) return { kind: "system_reminder", text: reminder.body.trim() };
    const caveat = tagAt(text, 0, "local-command-caveat");
    if (record.isMeta === true && caveat?.end === text.length) return { kind: "hidden" };
    const command = commandText(text);
    if (command !== undefined) return { kind: "command", text: command };
    for (const [tag, severity] of [["local-command-stdout", "info"], ["local-command-stderr", "error"]] as const) {
      const output = tagAt(text, 0, tag);
      if (output?.end === text.length) return { kind: "command_output", text: output.body, severity };
    }
  }
  return { kind: "conversation" };
}
