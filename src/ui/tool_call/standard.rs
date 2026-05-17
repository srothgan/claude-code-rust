// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

//! Rendering for tool-call titles, standard bodies, and compact content summaries.

use crate::agent::model;
use crate::app::todos::parse_todos_if_present;
use crate::app::{TodoItem, TodoStatus, ToolCallInfo};
use crate::ui::diff::{is_markdown_file, lang_from_title, render_diff, strip_outer_code_fence};
use crate::ui::highlight;
use crate::ui::markdown;
use crate::ui::theme;
use crate::ui::wrap::wrap_lines_to_physical_rows;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::errors::{
    debug_failed_tool_render, failed_execute_first_line, failed_tool_text_summary,
    render_failed_tool_text_content,
};
use super::interactions::{render_permission_lines, render_question_lines};
use super::{
    TOOL_BODY_MAX_LINES, ToolCallRenderContext, execute, markdown_inline_spans, status_icon,
    tool_display_title, tool_output_badge_spans, truncate_spans_to_width,
};

pub(super) const WRITE_DIFF_MAX_LINES: usize = TOOL_BODY_MAX_LINES;
const DEFAULT_TEXT_SUMMARY_LIMIT: usize = 60;
const IN_PROGRESS_SUBAGENT_TEXT_SUMMARY_LIMIT: usize = 180;
const READ_BODY_HEAD_LINES: usize = 3;
const READ_BODY_TAIL_LINES: usize = 4;
const DIFF_BODY_INDENT: &str = "  ";
const DIFF_BODY_INDENT_WIDTH: u16 = 2;
const STANDARD_BODY_PREFIX_WIDTH: u16 = 5;
const EXECUTE_BODY_INDENT: &str = "      ";
const EXECUTE_BODY_INDENT_WIDTH: u16 = 6;
const TODO_OMISSION_MARKER: &str = "...";

/// Render just the title line for a tool call (the line containing the spinner icon).
/// Used for in-progress tool calls where only the spinner changes each frame.
pub(super) fn render_tool_call_title(
    tc: &ToolCallInfo,
    render_context: ToolCallRenderContext<'_>,
    width: u16,
    spinner_frame: usize,
) -> Line<'static> {
    let (icon, icon_color) = status_icon(tc.status, spinner_frame);
    let (kind_icon, kind_name) = theme::tool_name_label(&tc.sdk_tool_name);

    let mut title_spans = vec![
        Span::styled(format!("  {icon} "), Style::default().fg(icon_color)),
        Span::styled(
            format!("{kind_icon} "),
            Style::default().fg(ratatui::style::Color::White).add_modifier(Modifier::BOLD),
        ),
    ];
    if tc.is_execute_tool() {
        title_spans.push(Span::styled(
            format!("{kind_name} "),
            Style::default().fg(ratatui::style::Color::White).add_modifier(Modifier::BOLD),
        ));
    }

    let display_title = tool_display_title(tc, render_context);
    title_spans.extend(markdown_inline_spans(display_title.as_ref()));
    title_spans.extend(tool_output_badge_spans(tc));

    Line::from(truncate_spans_to_width(title_spans, usize::from(width)))
}

/// Render the body lines (everything after the title) for a tool call.
/// Used for in-progress tool calls where the body is cached separately from the title.
pub(super) fn render_tool_call_body(tc: &ToolCallInfo, width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    render_standard_body(tc, width, &mut lines);
    lines
}

#[must_use]
pub(super) fn tool_call_body_depends_on_width(tc: &ToolCallInfo) -> bool {
    tool_call_has_body(tc)
}

#[must_use]
pub(super) fn tool_call_has_body(tc: &ToolCallInfo) -> bool {
    if is_todo_write_tool(tc) {
        return todo_write_todos(tc).is_some_and(|todos| !todos.is_empty())
            || tc.pending_permission.is_some()
            || tc.pending_question.is_some();
    }

    !tc.content.is_empty()
        || tc.pending_permission.is_some()
        || tc.pending_question.is_some()
        || (tc.is_execute_tool()
            && (tc.terminal_command.is_some()
                || tc.terminal_output.is_some()
                || matches!(tc.status, model::ToolCallStatus::InProgress)))
}

/// Render the body (everything after the title line) of a tool call.
fn render_standard_body(tc: &ToolCallInfo, width: u16, lines: &mut Vec<Line<'static>>) {
    let pipe_style = Style::default().fg(theme::DIM);

    if !tool_call_has_body(tc) {
        return;
    }

    let is_execute = tc.is_execute_tool();
    let prefix_width =
        if is_execute { EXECUTE_BODY_INDENT_WIDTH } else { STANDARD_BODY_PREFIX_WIDTH };
    let content_width = width.saturating_sub(prefix_width);
    let mut content_lines = render_tool_content(tc, content_width);
    let protected_source_lines = protected_content_source_lines(tc, &content_lines);
    content_lines = cap_tool_content_lines(
        content_lines,
        content_width,
        "source lines hidden",
        protected_source_lines,
    );

    if let Some(ref perm) = tc.pending_permission {
        content_lines.extend(render_permission_lines(tc, perm));
    }
    if let Some(ref question) = tc.pending_question {
        content_lines.extend(render_question_lines(question));
    }

    let last_idx = content_lines.len().saturating_sub(1);
    for (i, content_line) in content_lines.into_iter().enumerate() {
        let prefix = if is_execute {
            EXECUTE_BODY_INDENT
        } else if i == last_idx {
            "  \u{2514}\u{2500} " // corner
        } else {
            "  \u{2502}  " // pipe
        };
        let mut spans = vec![Span::styled(prefix.to_owned(), pipe_style)];
        spans.extend(truncate_spans_to_width(content_line.spans, usize::from(content_width)));
        lines.push(Line::from(spans));
    }
}

/// One-line summary for tools that should not render expanded content.
pub(super) fn content_summary(tc: &ToolCallInfo) -> String {
    // For Execute tool calls, show last non-empty line of terminal output
    if tc.terminal_id.is_some() {
        if let Some(ref output) = tc.terminal_output {
            let stripped_output = highlight::strip_ansi(output);
            if matches!(tc.status, model::ToolCallStatus::Failed | model::ToolCallStatus::Killed)
                && let Some(first_line) = failed_execute_first_line(&stripped_output)
            {
                return if first_line.chars().count() > 80 {
                    let truncated: String = first_line.chars().take(77).collect();
                    format!("{truncated}...")
                } else {
                    first_line
                };
            }
            let last = stripped_output.lines().rev().find(|l| !l.trim().is_empty());
            if let Some(line) = last {
                return if line.chars().count() > 80 {
                    let truncated: String = line.chars().take(77).collect();
                    format!("{truncated}...")
                } else {
                    line.to_owned()
                };
            }
        }
        return if matches!(tc.status, model::ToolCallStatus::InProgress) {
            "running...".to_owned()
        } else {
            String::new()
        };
    }

    for content in &tc.content {
        match content {
            model::ToolCallContent::Diff(diff) => {
                let name = diff.path.file_name().map_or_else(
                    || diff.path.to_string_lossy().into_owned(),
                    |f| f.to_string_lossy().into_owned(),
                );
                return name;
            }
            model::ToolCallContent::McpResource(resource) => {
                if let Some(path) = &resource.blob_saved_to {
                    return path.file_name().map_or_else(
                        || path.to_string_lossy().into_owned(),
                        |f| f.to_string_lossy().into_owned(),
                    );
                }
                if let Some(text) = resource.text.as_deref() {
                    if let Some(summary) = failed_tool_text_summary(tc.status, text) {
                        return truncate_summary_line(&summary, DEFAULT_TEXT_SUMMARY_LIMIT);
                    }
                    let first = text.lines().find(|line| !line.trim().is_empty()).unwrap_or("");
                    return truncate_summary_line(first, DEFAULT_TEXT_SUMMARY_LIMIT);
                }
                return resource.uri.clone();
            }
            model::ToolCallContent::Content(c) => {
                if let model::ContentBlock::Text(text) = &c.content {
                    let stripped = strip_outer_code_fence(&text.text);
                    if let Some(summary) = failed_tool_text_summary(tc.status, &stripped) {
                        return truncate_summary_line(&summary, text_summary_limit(tc));
                    }
                    let first = stripped.lines().next().unwrap_or("");
                    return truncate_summary_line(first, text_summary_limit(tc));
                }
            }
            model::ToolCallContent::Terminal(_) => {}
        }
    }
    String::new()
}

fn text_summary_limit(tc: &ToolCallInfo) -> usize {
    if matches!(tc.status, model::ToolCallStatus::InProgress)
        && matches!(tc.sdk_tool_name.as_str(), "Agent" | "Task")
    {
        IN_PROGRESS_SUBAGENT_TEXT_SUMMARY_LIMIT
    } else {
        DEFAULT_TEXT_SUMMARY_LIMIT
    }
}

fn truncate_summary_line(line: &str, max_chars: usize) -> String {
    if line.chars().count() > max_chars {
        let truncated: String = line.chars().take(max_chars.saturating_sub(3)).collect();
        format!("{truncated}...")
    } else {
        line.to_owned()
    }
}

/// Render the full content of a tool call as lines.
fn render_tool_content(tc: &ToolCallInfo, width: u16) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();

    if is_todo_write_tool(tc) {
        return todo_write_todos(tc)
            .map(|todos| render_todo_write_content(&todos))
            .unwrap_or_default();
    }

    if tc.is_execute_tool() {
        lines.extend(execute::render_execute_content(tc));
        debug_failed_tool_render(tc);
        return lines;
    }

    if tool_body_uses_summary_only(tc) {
        let summary = content_summary(tc);
        return if summary.is_empty() {
            Vec::new()
        } else {
            vec![Line::from(Span::styled(summary, Style::default().fg(theme::DIM)))]
        };
    }

    for content in &tc.content {
        match content {
            model::ToolCallContent::Diff(diff) => {
                if is_plan_file_path(&diff.path) {
                    lines.extend(render_plan_content(&diff.new_text));
                } else {
                    let raw = render_diff(diff, width.saturating_sub(DIFF_BODY_INDENT_WIDTH));
                    let raw = cap_write_diff_lines(raw);
                    lines.extend(indent_rendered_lines(raw, DIFF_BODY_INDENT));
                }
            }
            model::ToolCallContent::McpResource(resource) => {
                lines.extend(render_mcp_resource_content(tc, resource));
            }
            model::ToolCallContent::Content(c) => {
                if let model::ContentBlock::Text(text) = &c.content {
                    render_text_content(tc, &text.text, &mut lines);
                }
            }
            model::ToolCallContent::Terminal(_) => {}
        }
    }

    debug_failed_tool_render(tc);
    if is_read_tool(tc) {
        return cap_read_content_lines(lines);
    }
    lines
}

fn is_todo_write_tool(tc: &ToolCallInfo) -> bool {
    tc.sdk_tool_name == "TodoWrite"
}

fn todo_write_todos(tc: &ToolCallInfo) -> Option<Vec<TodoItem>> {
    if !is_todo_write_tool(tc) {
        return None;
    }
    tc.raw_input.as_ref().and_then(parse_todos_if_present)
}

fn render_todo_write_content(todos: &[TodoItem]) -> Vec<Line<'static>> {
    if todos.is_empty() {
        return Vec::new();
    }

    let window = todo_visible_window(todos);
    let mut lines = Vec::with_capacity(TOOL_BODY_MAX_LINES.min(todos.len()));
    if window.hidden_above {
        lines.push(todo_omission_line());
    }
    lines.extend(todos[window.start..window.end].iter().map(render_todo_line));
    if window.hidden_below {
        lines.push(todo_omission_line());
    }
    lines
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TodoWindow {
    start: usize,
    end: usize,
    hidden_above: bool,
    hidden_below: bool,
}

fn todo_visible_window(todos: &[TodoItem]) -> TodoWindow {
    let total = todos.len();
    if total <= TOOL_BODY_MAX_LINES {
        return TodoWindow { start: 0, end: total, hidden_above: false, hidden_below: false };
    }

    let anchor = todos
        .iter()
        .position(|todo| todo.status != TodoStatus::Completed)
        .unwrap_or(total.saturating_sub(1));
    let mut hidden_above = anchor > 0;
    let mut hidden_below = anchor + 1 < total;

    loop {
        let marker_rows = usize::from(hidden_above) + usize::from(hidden_below);
        let item_budget = TOOL_BODY_MAX_LINES.saturating_sub(marker_rows).max(1).min(total);
        let mut start = anchor.saturating_sub(item_budget / 2);
        start = start.min(total.saturating_sub(item_budget));
        let end = start.saturating_add(item_budget).min(total);
        let next_hidden_above = start > 0;
        let next_hidden_below = end < total;

        if next_hidden_above == hidden_above && next_hidden_below == hidden_below {
            return TodoWindow { start, end, hidden_above, hidden_below };
        }
        hidden_above = next_hidden_above;
        hidden_below = next_hidden_below;
    }
}

fn render_todo_line(todo: &TodoItem) -> Line<'static> {
    let (marker, marker_style, text, text_style) = match todo.status {
        TodoStatus::Completed => (
            "\u{25a0}",
            Style::default().fg(theme::DIM),
            todo.content.as_str(),
            Style::default().fg(theme::DIM).add_modifier(Modifier::DIM),
        ),
        TodoStatus::InProgress => (
            "\u{25a3}",
            Style::default().fg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD),
            if todo.active_form.is_empty() {
                todo.content.as_str()
            } else {
                todo.active_form.as_str()
            },
            Style::default().fg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD),
        ),
        TodoStatus::Pending => (
            "\u{25a1}",
            Style::default().fg(theme::DIM),
            todo.content.as_str(),
            Style::default().fg(Color::Gray),
        ),
    };

    Line::from(vec![
        Span::styled(marker.to_owned(), marker_style),
        Span::raw(" "),
        Span::styled(text.to_owned(), text_style),
    ])
}

fn todo_omission_line() -> Line<'static> {
    Line::from(Span::styled(
        TODO_OMISSION_MARKER,
        Style::default().fg(theme::DIM).add_modifier(Modifier::ITALIC),
    ))
}

fn tool_body_uses_summary_only(tc: &ToolCallInfo) -> bool {
    tc.is_exit_plan_mode_tool()
        || matches!(tc.sdk_tool_name.as_str(), "Agent" | "Task" | "WebSearch" | "WebFetch")
}

fn is_read_tool(tc: &ToolCallInfo) -> bool {
    tc.sdk_tool_name.eq_ignore_ascii_case("read")
}

fn protected_content_source_lines(tc: &ToolCallInfo, lines: &[Line<'static>]) -> usize {
    let mut count = if tc.is_execute_tool() && tc.terminal_command.is_some() {
        1
    } else if is_read_tool(tc) {
        READ_BODY_HEAD_LINES
    } else {
        leading_diff_metadata_line_count(lines)
    }
    .min(lines.len());

    while lines.get(count).is_some_and(line_text_is_existing_omission_marker) {
        count += 1;
    }
    count
}

fn cap_read_content_lines(lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
    let visible_content_lines = READ_BODY_HEAD_LINES.saturating_add(READ_BODY_TAIL_LINES);
    let capped_line_count = visible_content_lines.saturating_add(1);
    if lines.len() <= capped_line_count {
        return lines;
    }

    let omitted = lines.len().saturating_sub(visible_content_lines);
    let tail_start = lines.len().saturating_sub(READ_BODY_TAIL_LINES);
    let mut out = Vec::with_capacity(capped_line_count);
    out.extend(lines.iter().take(READ_BODY_HEAD_LINES).cloned());
    out.push(Line::from(Span::styled(
        format!("... {omitted} lines hidden ..."),
        Style::default().fg(theme::DIM).add_modifier(Modifier::ITALIC),
    )));
    out.extend(lines.into_iter().skip(tail_start));
    out
}

fn render_plan_content(text: &str) -> Vec<Line<'static>> {
    let md_source = strip_outer_code_fence(text);
    markdown::render_markdown_safe(&md_source, None)
        .into_iter()
        .map(|line| {
            let owned: Vec<Span<'static>> = line
                .spans
                .into_iter()
                .map(|span| Span::styled(span.content.into_owned(), span.style))
                .collect();
            Line::from(owned)
        })
        .collect()
}

fn render_text_content(tc: &ToolCallInfo, text: &str, lines: &mut Vec<Line<'static>>) {
    let stripped = strip_outer_code_fence(text);
    if let Some(failed_lines) = render_failed_tool_text_content(tc.status, &stripped) {
        lines.extend(failed_lines);
        return;
    }
    let md_source = if is_markdown_file(&tc.title) {
        stripped
    } else {
        let lang = lang_from_title(&tc.title);
        lines.extend(highlight::highlight_code(
            &stripped,
            (!lang.is_empty()).then_some(lang.as_str()),
        ));
        return;
    };
    for line in markdown::render_markdown_safe(&md_source, None) {
        let owned: Vec<Span<'static>> =
            line.spans.into_iter().map(|s| Span::styled(s.content.into_owned(), s.style)).collect();
        lines.push(Line::from(owned));
    }
}

fn render_mcp_resource_content(
    tc: &ToolCallInfo,
    resource: &model::McpResource,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if let Some(text) = resource.text.as_deref() {
        render_text_content(tc, text, &mut lines);
    }
    if let Some(blob_saved_to) = &resource.blob_saved_to {
        let saved_path = blob_saved_to.to_string_lossy().into_owned();
        let text_mentions_path =
            resource.text.as_deref().is_some_and(|text| text.contains(saved_path.as_str()));
        if !text_mentions_path {
            lines.push(Line::from(vec![
                Span::styled(
                    "Saved to: ",
                    Style::default().fg(theme::DIM).add_modifier(Modifier::BOLD),
                ),
                Span::styled(saved_path, Style::default().fg(theme::DIM)),
            ]));
        }
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(resource.uri.clone(), Style::default().fg(theme::DIM))));
    }
    lines
}

fn indent_rendered_lines(lines: Vec<Line<'static>>, indent: &str) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .map(|line| {
            let mut spans = vec![Span::raw(indent.to_owned())];
            spans.extend(line.spans);
            Line::from(spans)
        })
        .collect()
}

/// Returns `true` for paths inside `.claude/plans/` (cross-platform).
/// These files render as markdown plan content instead of unified diffs.
fn is_plan_file_path(path: &std::path::Path) -> bool {
    path.components()
        .zip(path.components().skip(1))
        .any(|(a, b)| a.as_os_str() == ".claude" && b.as_os_str() == "plans")
}

pub(super) fn cap_write_diff_lines(lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
    if lines.len() <= WRITE_DIFF_MAX_LINES {
        return lines;
    }
    let protected = leading_diff_metadata_line_count(&lines);
    let tail = WRITE_DIFF_MAX_LINES.saturating_sub(protected + 1);
    let omitted = lines.len().saturating_sub(protected + tail);

    let mut out = Vec::with_capacity(WRITE_DIFF_MAX_LINES);
    out.extend(lines.iter().take(protected).cloned());
    out.push(Line::from(Span::styled(
        format!("... {omitted} diff lines omitted ..."),
        Style::default().fg(theme::DIM).add_modifier(Modifier::ITALIC),
    )));
    out.extend(lines.into_iter().skip(protected + omitted));
    out
}

fn leading_diff_metadata_line_count(lines: &[Line<'static>]) -> usize {
    let mut count = 0;
    if lines.get(count).is_some_and(line_text_starts_with_repository_label) {
        count += 1;
    }
    if lines.get(count).is_some_and(line_text_starts_with_diff_count_header) {
        count += 1;
    }
    count
}

fn line_text_starts_with_repository_label(line: &Line<'static>) -> bool {
    line_plain_text(line).trim_start().starts_with('[')
}

fn line_text_starts_with_diff_count_header(line: &Line<'static>) -> bool {
    line_plain_text(line).trim_start().starts_with('(')
}

#[derive(Clone)]
struct WrappedContentRow {
    source_index: usize,
    row: Line<'static>,
}

fn cap_tool_content_lines(
    lines: Vec<Line<'static>>,
    width: u16,
    omitted_label: &str,
    protected_source_lines: usize,
) -> Vec<Line<'static>> {
    let source_line_count = lines.len();
    let wrapped = wrap_content_lines_with_source(lines, width);
    if wrapped.len() <= TOOL_BODY_MAX_LINES {
        return wrapped.into_iter().map(|wrapped| wrapped.row).collect();
    }

    let protected_source_lines = protected_source_lines.min(source_line_count);
    let protected_rows =
        wrapped.iter().take_while(|row| row.source_index < protected_source_lines).count();
    if protected_rows >= TOOL_BODY_MAX_LINES {
        let visible = TOOL_BODY_MAX_LINES.saturating_sub(1);
        let omitted_rows = wrapped.len().saturating_sub(visible);
        let mut out = Vec::with_capacity(TOOL_BODY_MAX_LINES);
        out.extend(wrapped.iter().take(visible).map(|wrapped| wrapped.row.clone()));
        if omitted_rows > 0 {
            out.push(Line::from(Span::styled(
                format!("... {omitted_rows} wrapped rows hidden ..."),
                Style::default().fg(theme::DIM).add_modifier(Modifier::ITALIC),
            )));
        }
        return out;
    }

    let tail = TOOL_BODY_MAX_LINES.saturating_sub(protected_rows + 1);
    let tail_start = wrapped.len().saturating_sub(tail).max(protected_rows);
    let omitted_rows = tail_start.saturating_sub(protected_rows);
    let first_tail_source =
        wrapped.get(tail_start).map_or(source_line_count, |row| row.source_index);
    let omitted_source_lines = first_tail_source.saturating_sub(protected_source_lines);
    let omitted = if omitted_source_lines > 0 {
        format!("... {omitted_source_lines} {omitted_label} ...")
    } else {
        format!("... {omitted_rows} wrapped rows hidden ...")
    };

    let mut out = Vec::with_capacity(TOOL_BODY_MAX_LINES);
    out.extend(wrapped.iter().take(protected_rows).map(|wrapped| wrapped.row.clone()));
    out.push(Line::from(Span::styled(
        omitted,
        Style::default().fg(theme::DIM).add_modifier(Modifier::ITALIC),
    )));
    out.extend(wrapped.into_iter().skip(tail_start).map(|wrapped| wrapped.row));
    out
}

fn wrap_content_lines_with_source(lines: Vec<Line<'static>>, width: u16) -> Vec<WrappedContentRow> {
    let mut rows = Vec::new();
    for (source_index, line) in lines.into_iter().enumerate() {
        rows.extend(
            wrap_lines_to_physical_rows(&[line], width)
                .into_iter()
                .map(|row| WrappedContentRow { source_index, row }),
        );
    }
    rows
}

fn line_text_is_existing_omission_marker(line: &Line<'static>) -> bool {
    let text = line_plain_text(line);
    text.contains("lines hidden") || text.contains("lines omitted")
}

fn line_plain_text(line: &Line<'static>) -> String {
    line.spans.iter().flat_map(|span| span.content.chars()).collect()
}
