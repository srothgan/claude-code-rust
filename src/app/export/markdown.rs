// SPDX-License-Identifier: Apache-2.0

//! Markdown rendering of a session's complete history for `/export`.
//!
//! The input is the bridge's resume translation of the saved transcript, so this
//! module owns only presentation: message text is kept as written but cannot
//! change the file's structure (see `contain_message`), tool calls get a heading
//! with a compact input summary, and results and diffs go into fenced blocks that
//! are truncated with a note. Saved notices and notifications become quoted notes,
//! and what is left out is named in the header and in closing notes. Terminal
//! control sequences never reach the file.

use crate::agent::model::{
    ContentBlock, SessionUpdate, SystemNoticeSeverity, ToolCall, ToolCallContent, ToolCallStatus,
    ToolCallUpdateFields,
};
use crate::agent::notifications::SdkNotification;
use crate::ui::terminal_output::strip_ansi;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;

/// Longest tool result kept in the file, in characters and in lines.
const MAX_RESULT_CHARS: usize = 4_000;
const MAX_RESULT_LINES: usize = 60;
const MAX_DIFF_LINES: usize = 200;
const MAX_SUMMARY_CHARS: usize = 100;
/// Tool input fields that best name what a call does, in priority order.
const SUMMARY_KEYS: [&str; 8] =
    ["command", "file_path", "path", "pattern", "url", "query", "description", "prompt"];

pub(super) struct Header<'a> {
    pub title: Option<&'a str>,
    pub session_id: &'a str,
    pub cwd: &'a str,
    pub model: Option<&'a str>,
    pub exported_at: &'a str,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    User,
    Assistant,
}

/// What the export leaves out by design, stated in the header so a reader never has to guess.
const NOT_INCLUDED: &str = "thinking blocks, images inside tool results, background task state, \
     timing and other session metadata";

pub(super) fn render(
    header: &Header<'_>,
    updates: &[SessionUpdate],
    omitted_updates: u64,
    unmapped_updates: u64,
) -> String {
    let tools = final_tool_states(updates);
    let mut out = String::new();
    write_header(&mut out, header);
    let mut role = None;
    for update in updates {
        match update {
            SessionUpdate::UserMessageChunk(chunk) => {
                write_text(&mut out, &mut role, Role::User, &chunk.content);
            }
            SessionUpdate::AgentMessageChunk(chunk) => {
                write_text(&mut out, &mut role, Role::Assistant, &chunk.content);
            }
            SessionUpdate::ToolCall(call) => {
                enter_role(&mut out, &mut role, Role::Assistant);
                if let Some(call) = tools.get(call.tool_call_id.as_str()) {
                    write_tool(&mut out, call);
                }
            }
            SessionUpdate::SystemNoticeUpdate { severity, message } => {
                let label = match severity {
                    SystemNoticeSeverity::Info => "Notice",
                    SystemNoticeSeverity::Warning => "Warning",
                    SystemNoticeSeverity::Error => "Error",
                };
                write_note(&mut out, label, message);
            }
            SessionUpdate::NotificationUpdate { notification, .. } => {
                let (SdkNotification::SdkNotice { text, .. }
                | SdkNotification::ModelTool { text, .. }) = notification;
                write_note(&mut out, "Notification", text);
            }
            SessionUpdate::ExternalMessageUpdate(message) => {
                let label = format!("Message from {}", clean_line(&message.origin.kind));
                write_note(&mut out, &label, &message.content);
            }
            _ => {}
        }
    }
    write_footer(&mut out, omitted_updates, unmapped_updates);
    out
}

fn write_footer(out: &mut String, omitted_updates: u64, unmapped_updates: u64) {
    if unmapped_updates == 0 && omitted_updates == 0 {
        return;
    }
    out.push_str("---\n\n");
    if unmapped_updates > 0 {
        let _ = writeln!(
            out,
            "_{unmapped_updates} saved history entries could not be shown by this app and are not included._\n"
        );
    }
    if omitted_updates > 0 {
        let _ = writeln!(
            out,
            "_Export truncated: {omitted_updates} later history entries were left out because the session is very large._"
        );
    }
}

/// Each tool call as the session ended: its call merged with every later update.
fn final_tool_states(updates: &[SessionUpdate]) -> HashMap<&str, ToolCall> {
    let mut tools: HashMap<&str, ToolCall> = HashMap::new();
    for update in updates {
        match update {
            SessionUpdate::ToolCall(call) => {
                tools.insert(call.tool_call_id.as_str(), call.clone());
            }
            SessionUpdate::ToolCallUpdate(update) => {
                if let Some(call) = tools.get_mut(update.tool_call_id.as_str()) {
                    apply_fields(call, &update.fields);
                }
            }
            _ => {}
        }
    }
    tools
}

fn apply_fields(call: &mut ToolCall, fields: &ToolCallUpdateFields) {
    if let Some(title) = &fields.title {
        call.title.clone_from(title);
    }
    if let Some(status) = fields.status {
        call.status = status;
    }
    if let Some(content) = &fields.content {
        call.content.clone_from(content);
    }
    if let Some(raw_input) = &fields.raw_input {
        call.raw_input = Some(raw_input.clone());
    }
    if let Some(raw_output) = &fields.raw_output {
        call.raw_output = Some(raw_output.clone());
    }
}

fn write_header(out: &mut String, header: &Header<'_>) {
    let title = header.title.map(clean_line).filter(|title| !title.is_empty());
    let _ = writeln!(out, "# {}\n", title.as_deref().unwrap_or("Conversation"));
    let _ = writeln!(out, "- Session: {}", clean_line(header.session_id));
    let _ = writeln!(out, "- Directory: {}", clean_line(header.cwd));
    if let Some(model) = header.model {
        let _ = writeln!(out, "- Model: {}", clean_line(model));
    }
    let _ = writeln!(out, "- Exported: {}", clean_line(header.exported_at));
    let _ = writeln!(out, "- Not included: {NOT_INCLUDED}\n");
}

fn enter_role(out: &mut String, current: &mut Option<Role>, role: Role) {
    if *current == Some(role) {
        return;
    }
    let heading = match role {
        Role::User => "User",
        Role::Assistant => "Assistant",
    };
    let _ = writeln!(out, "## {heading}\n");
    *current = Some(role);
}

fn write_text(out: &mut String, current: &mut Option<Role>, role: Role, block: &ContentBlock) {
    let text = match block {
        ContentBlock::Text(text) => contain_message(&clean(&text.text)),
        ContentBlock::Image(_) => "[image]".to_owned(),
    };
    let text = text.trim_end_matches('\n');
    if text.trim().is_empty() {
        return;
    }
    enter_role(out, current, role);
    out.push_str(text);
    out.push_str("\n\n");
}

/// A saved notice or notification, quoted so it reads as a note and not as speech.
fn write_note(out: &mut String, label: &str, text: &str) {
    let text = contain_message(&clean(text));
    let text = text.trim_end_matches('\n');
    if text.trim().is_empty() {
        return;
    }
    for (index, line) in text.split('\n').enumerate() {
        if index == 0 {
            let _ = write!(out, "> **{label}:**");
        } else {
            out.push('>');
        }
        if !line.is_empty() {
            out.push(' ');
            out.push_str(line);
        }
        out.push('\n');
    }
    out.push('\n');
}

/// An open code fence: the character and length that must close it.
struct Fence {
    marker: char,
    len: usize,
}

/// The fence a line opens, if any (up to three spaces of indent, three or more
/// backticks or tildes; a backtick fence's info string has no backtick).
fn fence_opener(line: &str) -> Option<Fence> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let marker = trimmed.chars().next().filter(|ch| matches!(ch, '`' | '~'))?;
    let len = trimmed.chars().take_while(|ch| *ch == marker).count();
    if len < 3 || (marker == '`' && trimmed[len..].contains('`')) {
        return None;
    }
    Some(Fence { marker, len })
}

fn closes_fence(line: &str, fence: &Fence) -> bool {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return false;
    }
    let run = trimmed.chars().take_while(|ch| *ch == fence.marker).count();
    run >= fence.len && trimmed[run..].trim().is_empty()
}

/// A raw HTML block (or comment) that a Markdown parser keeps open until its end
/// marker, however many lines later: `CommonMark` HTML block types 1-5.
struct RawBlock {
    ends: &'static [&'static str],
    /// Closing tags match without regard to case.
    ignore_case: bool,
}

const COMMENT: RawBlock = RawBlock { ends: &["-->"], ignore_case: false };
const RAW_TEXT: RawBlock =
    RawBlock { ends: &["</pre>", "</script>", "</style>", "</textarea>"], ignore_case: true };
const PROCESSING: RawBlock = RawBlock { ends: &["?>"], ignore_case: false };
const DECLARATION: RawBlock = RawBlock { ends: &[">"], ignore_case: false };
const CDATA: RawBlock = RawBlock { ends: &["]]>"], ignore_case: false };

/// The raw block a line opens at its start (up to three spaces of indent), and how
/// many bytes of the opener cannot count toward the end marker (`<?>` is not closed).
fn block_start(line: &str) -> Option<(&'static RawBlock, usize)> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("<![cdata[") {
        return Some((&CDATA, 9));
    }
    if lower.starts_with("<?") {
        return Some((&PROCESSING, 2));
    }
    if trimmed
        .strip_prefix("<!")
        .is_some_and(|rest| rest.starts_with(|ch: char| ch.is_ascii_alphabetic()))
    {
        return Some((&DECLARATION, 2));
    }
    let tag = lower.strip_prefix('<')?;
    let name_end = tag.find(|ch: char| !ch.is_ascii_alphanumeric()).unwrap_or(tag.len());
    let ends_name = tag[name_end..].starts_with([' ', '\t', '>']) || name_end == tag.len();
    (ends_name && matches!(&tag[..name_end], "pre" | "script" | "style" | "textarea"))
        .then_some((&RAW_TEXT, 0))
}

/// Offset just past the first end marker of `block` in `text`.
fn block_end(block: &RawBlock, text: &str) -> Option<usize> {
    let haystack = if block.ignore_case { text.to_ascii_lowercase() } else { text.to_owned() };
    block.ends.iter().filter_map(|end| haystack.find(end).map(|at| at + end.len())).min()
}

/// Offset just past the first `<!--` outside an inline code span.
fn comment_start(text: &str) -> Option<usize> {
    let mut at = 0;
    while at < text.len() {
        let rest = &text[at..];
        if rest.starts_with("<!--") {
            return Some(at + 4);
        }
        if rest.starts_with('`') {
            let run = rest.chars().take_while(|ch| *ch == '`').count();
            let ticks = &rest[..run];
            let span_end = rest[run..]
                .match_indices(ticks)
                .find(|(index, _)| {
                    !rest[run + index + run..].starts_with('`')
                        && !rest[..run + index].ends_with('`')
                })
                .map(|(index, _)| run + index + run);
            at += span_end.unwrap_or(run);
            continue;
        }
        at += rest.chars().next().map_or(1, char::len_utf8);
    }
    None
}

/// The raw block still open after this line, given the one open before it.
fn raw_block_after(line: &str, mut open: Option<&'static RawBlock>) -> Option<&'static RawBlock> {
    let mut rest = line;
    if open.is_none()
        && let Some((block, skip)) = block_start(line)
    {
        open = Some(block);
        rest = &line[skip..];
    }
    loop {
        if let Some(block) = open {
            let Some(end) = block_end(block, rest) else { return open };
            rest = &rest[end..];
            open = None;
        } else {
            let Some(start) = comment_start(rest) else { return open };
            rest = &rest[start..];
            open = Some(&COMMENT);
        }
    }
}

/// A heading the export itself writes: `# `..`### ` followed by a role or a tool label.
fn forges_export_heading(line: &str) -> bool {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return false;
    }
    let hashes = trimmed.chars().take_while(|ch| *ch == '#').count();
    let Some(rest) = trimmed[hashes..].strip_prefix([' ', '\t']) else { return false };
    let title = rest.trim().trim_end_matches('#').trim();
    (1..=3).contains(&hashes)
        && (title == "User" || title == "Assistant" || title.starts_with("Tool:"))
}

/// Message text that cannot change the file's structure: a fence, HTML comment or raw HTML block
/// the message leaves open is closed at its end, and a line that would read as one
/// of the export's own role or tool headings gets its first `#` escaped.
fn contain_message(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut fence: Option<Fence> = None;
    let mut raw: Option<&'static RawBlock> = None;
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        if let Some(open) = &fence {
            if closes_fence(line, open) {
                fence = None;
            }
            out.push_str(line);
            continue;
        }
        if raw.is_none() {
            if let Some(open) = fence_opener(line) {
                fence = Some(open);
                out.push_str(line);
                continue;
            }
            if forges_export_heading(line)
                && let Some(position) = line.find('#')
            {
                out.push_str(&line[..position]);
                out.push('\\');
                out.push_str(&line[position..]);
                raw = raw_block_after(line, raw);
                continue;
            }
        }
        raw = raw_block_after(line, raw);
        out.push_str(line);
    }
    if let Some(open) = fence {
        out.push('\n');
        out.push_str(&open.marker.to_string().repeat(open.len));
    }
    if let Some(block) = raw {
        out.push('\n');
        out.push_str(block.ends[0]);
    }
    out
}

fn write_tool(out: &mut String, call: &ToolCall) {
    let name = clean_line(tool_name(call));
    let name = name.as_str();
    match input_summary(call.raw_input.as_ref()) {
        Some(summary) => {
            let _ = writeln!(out, "### Tool: {name} - {summary}\n");
        }
        None => {
            let _ = writeln!(out, "### Tool: {name}\n");
        }
    }

    let mut wrote_diff = false;
    for content in &call.content {
        if let ToolCallContent::Diff(diff) = content {
            write_diff(out, &diff.path, diff.old_text.as_deref(), &diff.new_text);
            wrote_diff = true;
        }
    }
    if !wrote_diff && let Some((path, old, new)) = edit_from_input(name, call.raw_input.as_ref()) {
        write_diff(out, Path::new(&path), old.as_deref(), &new);
    }

    let result = result_text(call);
    if !result.trim().is_empty() {
        let label = if call.status == ToolCallStatus::Failed { "Error" } else { "Result" };
        let _ = writeln!(out, "{label}:\n");
        write_fenced(out, "", &result, MAX_RESULT_LINES, MAX_RESULT_CHARS);
    }

    let images = call
        .content
        .iter()
        .filter(|content| {
            matches!(content, ToolCallContent::Content(content)
                if matches!(content.content, ContentBlock::Image(_)))
        })
        .count();
    if images > 0 {
        let noun = if images == 1 { "image" } else { "images" };
        let _ = writeln!(out, "_Not included: {images} {noun} in this result._\n");
    }
}

fn tool_name(call: &ToolCall) -> &str {
    call.meta
        .as_ref()
        .and_then(|meta| meta.get("claudeCode"))
        .and_then(|code| code.get("toolName"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or(&call.title)
}

fn input_summary(input: Option<&serde_json::Value>) -> Option<String> {
    let input = input?.as_object()?;
    let value = SUMMARY_KEYS
        .iter()
        .filter_map(|key| input.get(*key)?.as_str())
        .map(|value| value.split_whitespace().collect::<Vec<_>>().join(" "))
        .find(|value| !value.is_empty())?;
    let value = clean_line(&value);
    if value.chars().count() <= MAX_SUMMARY_CHARS {
        return Some(value);
    }
    let kept: String = value.chars().take(MAX_SUMMARY_CHARS).collect();
    Some(format!("{kept}..."))
}

/// An Edit or Write call whose result no longer carries its diff.
fn edit_from_input(
    name: &str,
    input: Option<&serde_json::Value>,
) -> Option<(String, Option<String>, String)> {
    let input = input?.as_object()?;
    let path = input.get("file_path")?.as_str()?.to_owned();
    match name {
        "Edit" => Some((
            path,
            Some(input.get("old_string")?.as_str()?.to_owned()),
            input.get("new_string")?.as_str()?.to_owned(),
        )),
        "Write" => Some((path, None, input.get("content")?.as_str()?.to_owned())),
        _ => None,
    }
}

fn result_text(call: &ToolCall) -> String {
    let texts: Vec<&str> = call
        .content
        .iter()
        .filter_map(|content| match content {
            ToolCallContent::Content(content) => match &content.content {
                ContentBlock::Text(text) => Some(text.text.as_str()),
                ContentBlock::Image(_) => None,
            },
            ToolCallContent::McpResource(resource) => resource.text.as_deref(),
            _ => None,
        })
        .collect();
    if !texts.is_empty() {
        return clean(&texts.join("\n"));
    }
    match &call.raw_output {
        Some(serde_json::Value::String(text)) => clean(text),
        Some(serde_json::Value::Null) | None => String::new(),
        Some(other) => clean(&serde_json::to_string_pretty(other).unwrap_or_default()),
    }
}

fn write_diff(out: &mut String, path: &Path, old: Option<&str>, new: &str) {
    let path = clean_line(&path.display().to_string());
    let old_label = if old.is_some() { path.as_str() } else { "/dev/null" };
    let text_diff = similar::TextDiff::from_lines(old.unwrap_or(""), new);
    let mut unified = text_diff.unified_diff();
    unified.context_radius(3).header(old_label, &path);
    write_fenced(out, "diff", &clean(&unified.to_string()), MAX_DIFF_LINES, usize::MAX);
}

/// Write `body` as a fenced block no longer than the limits, with a note when cut.
fn write_fenced(out: &mut String, language: &str, body: &str, max_lines: usize, max_chars: usize) {
    let (shown, cut) = truncate(body.trim_end_matches('\n'), max_lines, max_chars);
    let ticks = longest_backtick_run(shown).max(2) + 1;
    let fence = "`".repeat(ticks);
    let _ = writeln!(out, "{fence}{language}\n{shown}\n{fence}\n");
    if cut {
        let total = body.chars().count();
        let kept = shown.chars().count();
        let _ = writeln!(
            out,
            "_Output truncated: the first {kept} of {total} characters are shown._\n"
        );
    }
}

/// The longest prefix within both limits, and whether anything was cut.
fn truncate(text: &str, max_lines: usize, max_chars: usize) -> (&str, bool) {
    let mut lines = 1;
    for (chars, (index, ch)) in text.char_indices().enumerate() {
        if chars >= max_chars {
            return (&text[..index], true);
        }
        if ch == '\n' {
            lines += 1;
            if lines > max_lines {
                return (&text[..index], true);
            }
        }
    }
    (text, false)
}

fn longest_backtick_run(text: &str) -> usize {
    text.split(|ch| ch != '`').map(str::len).max().unwrap_or(0)
}

/// Remove terminal control sequences and stray control characters, keeping text as written.
///
/// Sequences are stripped one line at a time: `strip_ansi` runs an operating-system
/// command until BEL or ESC, so a stray `ESC ]` would otherwise swallow every later
/// line before truncation could say anything. A real sequence never spans a line.
fn clean(text: &str) -> String {
    text.replace("\r\n", "\n")
        .split('\n')
        .map(strip_ansi)
        .collect::<Vec<_>>()
        .join("\n")
        .chars()
        .filter(|ch| !ch.is_control() || matches!(ch, '\n' | '\t'))
        .collect()
}

/// A single-line form for headings and header fields.
fn clean_line(text: &str) -> String {
    clean(text).split_whitespace().collect::<Vec<_>>().join(" ")
}
