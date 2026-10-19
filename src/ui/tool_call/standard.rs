// SPDX-License-Identifier: Apache-2.0

//! Rendering for tool-call titles and standard bodies.

use crate::agent::model;
use crate::app::ToolCallInfo;
use crate::ui::diff::{is_markdown_file, lang_from_title, render_diff, strip_outer_code_fence};
use crate::ui::highlight;
use crate::ui::layout_control::LayoutControl;
use crate::ui::markdown;
use crate::ui::theme;
use crate::ui::tool_display;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;
use std::path::Path;
use std::task::Poll;

use super::errors::{debug_failed_tool_render, render_failed_tool_text_content};
use super::interactions::{render_permission_lines, render_question_lines};
use super::{
    TOOL_BODY_MAX_LINES, ToolCallRenderContext, execute, markdown_inline_spans, read, status_icon,
    tasks, tool_display_title, tool_output_badge_spans, truncate_spans_to_width, typed,
};

pub(super) const WRITE_DIFF_MAX_LINES: usize = TOOL_BODY_MAX_LINES;
const DIFF_BODY_INDENT: &str = "  ";
const DIFF_BODY_INDENT_WIDTH: u16 = 2;
const STANDARD_BODY_PREFIX_WIDTH: u16 = 5;
const EXECUTE_BODY_INDENT: &str = "      ";
const EXECUTE_BODY_INDENT_WIDTH: u16 = 6;
const ASK_USER_QUESTION_RESULT_TEXT_INDENT: &str = "      ";
const VIEWED_IMAGE_RESULT_PREFIX: &str = "Viewed Image ";

/// Render just the title line for a tool call (the line containing the spinner icon).
/// Used for in-progress tool calls where only the spinner changes each frame.
pub(super) fn render_tool_call_title(
    tc: &ToolCallInfo,
    render_context: ToolCallRenderContext<'_>,
    width: u16,
    spinner: crate::ui::SpinnerState,
) -> Line<'static> {
    let (icon, icon_color) = status_icon(tc.status, spinner);
    let kind_label = tool_display::tool_name_label(&tc.sdk_tool_name);
    let kind_icon_span = if let Some((task_marker, task_marker_style)) = tasks::title_marker(tc) {
        Span::styled(format!("{task_marker} "), task_marker_style)
    } else {
        Span::styled(
            format!("{} ", kind_label.icon),
            Style::default().fg(ratatui::style::Color::White).add_modifier(Modifier::BOLD),
        )
    };

    let mut title_spans =
        vec![Span::styled(format!("{icon} "), Style::default().fg(icon_color)), kind_icon_span];
    if tc.is_execute_tool() {
        title_spans.push(Span::styled(
            format!("{} ", kind_label.label.as_ref()),
            Style::default().fg(ratatui::style::Color::White).add_modifier(Modifier::BOLD),
        ));
    }

    if let Some(prefix) = hidden_subagent_permission_title_prefix(tc) {
        title_spans.push(prefix);
    }

    let display_title = tool_display_title(tc, render_context);
    // Titles have one source line. Shell text is literal command syntax, not Markdown.
    // Keep the full title/command in the model; only this display projection is narrowed.
    let title_line = display_title.lines().next().unwrap_or_default();
    let literal = tc.is_execute_tool();
    title_spans.extend(tc.cache.inline_spans(title_line, literal, |source| {
        if literal { vec![Span::raw(source.to_owned())] } else { markdown_inline_spans(source) }
    }));
    title_spans.extend(tool_output_badge_spans(tc));

    Line::from(truncate_spans_to_width(title_spans, usize::from(width)))
}

fn hidden_subagent_permission_title_prefix(tc: &ToolCallInfo) -> Option<Span<'static>> {
    let has_subagent_permission = tc
        .pending_permission
        .as_ref()
        .is_some_and(|permission| permission.subagent_context.is_some());
    (tc.is_subagent_child() && has_subagent_permission).then(|| {
        Span::styled("Subagent · ", Style::default().fg(theme::DIM).add_modifier(Modifier::BOLD))
    })
}

/// Render the body lines (everything after the title) for a tool call.
/// Used for in-progress tool calls where the body is cached separately from the title.
#[derive(Default)]
pub(super) struct ToolBodyCursor {
    content: ToolContentCursor,
}

impl ToolBodyCursor {
    pub(super) fn render(
        &mut self,
        tc: &ToolCallInfo,
        width: u16,
        control: &LayoutControl<'_>,
    ) -> Vec<Line<'static>> {
        loop {
            if let Poll::Ready(lines) = self.poll(tc, width, control) {
                return lines;
            }
        }
    }

    pub(super) fn poll(
        &mut self,
        tc: &ToolCallInfo,
        width: u16,
        control: &LayoutControl<'_>,
    ) -> Poll<Vec<Line<'static>>> {
        if control.cancelled() {
            return Poll::Ready(Vec::new());
        }
        let prefix = if tc.is_execute_tool() {
            EXECUTE_BODY_INDENT_WIDTH
        } else {
            STANDARD_BODY_PREFIX_WIDTH
        };
        let Poll::Ready(content) = self.content.poll(tc, width.saturating_sub(prefix), control)
        else {
            return Poll::Pending;
        };
        let mut lines = Vec::new();
        finish_standard_body(tc, width, content, &mut lines, control);
        Poll::Ready(lines)
    }
}

pub(super) fn render_result_presentation(
    tc: &ToolCallInfo,
    result: &model::ToolResultPresentation,
    clock: crate::app::presentation::ClockFormat<'_>,
    width: u16,
    control: &LayoutControl<'_>,
) -> Vec<Line<'static>> {
    let content = super::result_presentation::render(result, clock);
    let mut lines = Vec::new();
    finish_standard_body(tc, width, content, &mut lines, control);
    lines
}

#[must_use]
pub(super) fn tool_call_has_body(tc: &ToolCallInfo) -> bool {
    if tc.output_metadata.as_ref().is_some_and(|metadata| metadata.presentation.is_some()) {
        return true;
    }
    if let Some(renderer) = typed::renderer_for(tc) {
        return (renderer.has_structured_body)(tc)
            || tc.pending_permission.is_some()
            || tc.pending_question.is_some()
            || tc.output_was_staged()
            || tc.non_execution_metadata().is_some();
    }

    !tc.content.is_empty()
        || tc.pending_permission.is_some()
        || tc.pending_question.is_some()
        || tc.output_was_staged()
        || tc.non_execution_metadata().is_some()
        || renders_structured_ask_user_question_result(tc)
        || (tc.is_execute_tool()
            && (tc.terminal_output.is_some()
                || matches!(tc.status, model::ToolCallStatus::InProgress)))
}

/// Render the body (everything after the title line) of a tool call.
fn finish_standard_body(
    tc: &ToolCallInfo,
    width: u16,
    mut content_lines: Vec<Line<'static>>,
    lines: &mut Vec<Line<'static>>,
    control: &LayoutControl<'_>,
) {
    let pipe_style = Style::default().fg(theme::DIM);

    let is_execute = tc.is_execute_tool();
    let prefix_width =
        if is_execute { EXECUTE_BODY_INDENT_WIDTH } else { STANDARD_BODY_PREFIX_WIDTH };
    let content_width = width.saturating_sub(prefix_width);
    if tc.output_was_staged() {
        content_lines.push(Line::from(vec![
            Span::styled("Staged for review: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("file unchanged"),
        ]));
    }
    content_lines.extend(render_non_execution_metadata(tc));
    content_lines = match tool_content_height_policy(tc) {
        ToolContentHeightPolicy::Preformatted => content_lines,
        ToolContentHeightPolicy::Bounded => {
            let protected_source_lines = protected_content_source_lines(tc, &content_lines);
            cap_tool_content_lines(
                content_lines,
                content_width,
                "source lines hidden",
                protected_source_lines,
                control,
            )
        }
        ToolContentHeightPolicy::Unbounded => {
            crate::ui::wrap::wrap_lines_with_control(&content_lines, content_width, control)
        }
    };

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

fn render_non_execution_metadata(tc: &ToolCallInfo) -> Vec<Line<'static>> {
    let Some(metadata) = tc.non_execution_metadata() else {
        return Vec::new();
    };
    let reason = match metadata.kind.as_str() {
        "user-rejected" => "rejected by the user".to_owned(),
        "permission-rule" => "blocked by a permission rule".to_owned(),
        "automode-unavailable" => "automatic mode was unavailable".to_owned(),
        "automode-parsing-error" => "automatic mode could not parse the request".to_owned(),
        "automode-blocked" => "automatic mode blocked the request".to_owned(),
        "cancelled" => "cancelled".to_owned(),
        "interrupted" => "interrupted".to_owned(),
        other => other.to_owned(),
    };
    let mut lines = vec![Line::from(vec![
        Span::styled("Not executed: ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(reason),
    ])];
    if let Some(feedback) = metadata.user_feedback.as_deref() {
        lines.push(Line::from(vec![
            Span::styled("Feedback: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(feedback.to_owned()),
        ]));
    }
    lines
}

#[derive(Default)]
struct ToolContentCursor {
    next_content: usize,
    lines: Vec<Line<'static>>,
    read: Option<read::ReadCursor>,
}

impl ToolContentCursor {
    fn poll(
        &mut self,
        tc: &ToolCallInfo,
        width: u16,
        control: &LayoutControl<'_>,
    ) -> Poll<Vec<Line<'static>>> {
        if hides_pending_ask_user_question_transcript(tc) || control.cancelled() {
            return Poll::Ready(Vec::new());
        }
        if let Some(renderer) = typed::renderer_for(tc) {
            return Poll::Ready((renderer.render)(tc));
        }
        if tc.is_execute_tool() {
            return Poll::Ready(render_execute_tool_content(tc, control));
        }
        if let Some(lines) = render_completed_ask_user_question_content(tc) {
            return Poll::Ready(lines);
        }
        while let Some(content) = tc.content.get(self.next_content) {
            if control.cancelled() {
                return Poll::Ready(Vec::new());
            }
            match content {
                model::ToolCallContent::Diff(diff) => {
                    if is_plan_file_path(&diff.path) {
                        self.lines.extend(render_plan_content(&diff.new_text, width, control));
                    } else {
                        let raw = render_diff(
                            diff,
                            width.saturating_sub(DIFF_BODY_INDENT_WIDTH),
                            control,
                        );
                        self.lines.extend(indent_rendered_lines(
                            cap_write_diff_lines(raw),
                            DIFF_BODY_INDENT,
                        ));
                    }
                }
                model::ToolCallContent::McpResource(resource) => {
                    self.lines.extend(render_mcp_resource_content(tc, resource, width, control));
                }
                model::ToolCallContent::ResourceLink(link) => {
                    self.lines.extend(render_mcp_resource_link_content(link));
                }
                model::ToolCallContent::Content(c) => {
                    if let model::ContentBlock::Text(text) = &c.content
                        && render_text_content(
                            tc,
                            &text.text,
                            width,
                            &mut self.lines,
                            &mut self.read,
                            control,
                        )
                        .is_pending()
                    {
                        return Poll::Pending;
                    }
                }
                model::ToolCallContent::Terminal(_) => {}
            }
            self.read = None;
            self.next_content += 1;
            if self.next_content < tc.content.len() && control.yield_due() {
                return Poll::Pending;
            }
        }
        debug_failed_tool_render(tc);
        Poll::Ready(std::mem::take(&mut self.lines))
    }
}

fn render_execute_tool_content(
    tc: &ToolCallInfo,
    control: &LayoutControl<'_>,
) -> Vec<Line<'static>> {
    let lines = execute::render_execute_content(tc, control);
    debug_failed_tool_render(tc);
    lines
}

fn render_completed_ask_user_question_content(tc: &ToolCallInfo) -> Option<Vec<Line<'static>>> {
    if tc.is_ask_question_tool()
        && tc.pending_question.is_none()
        && matches!(tc.status, model::ToolCallStatus::Completed)
    {
        render_ask_user_question_result(tc)
    } else {
        None
    }
}

fn hides_pending_ask_user_question_transcript(tc: &ToolCallInfo) -> bool {
    tc.is_ask_question_tool() && tc.pending_question.is_some()
}

fn render_ask_user_question_result(tc: &ToolCallInfo) -> Option<Vec<Line<'static>>> {
    let results = tc.raw_input.as_ref()?.get("question_results")?.as_array()?;
    if results.is_empty() {
        return None;
    }

    let mut lines = Vec::new();
    for (idx, result) in results.iter().enumerate() {
        if idx > 0 {
            lines.push(Line::default());
        }
        render_ask_user_question_result_entry(result, &mut lines);
    }

    (!lines.is_empty()).then_some(lines)
}

fn render_ask_user_question_result_entry(result: &Value, lines: &mut Vec<Line<'static>>) {
    let header = json_str(result, "header").unwrap_or("Question");
    let question = json_str(result, "question");
    let title = ask_user_question_result_title(
        header,
        json_usize(result, "question_index"),
        json_usize(result, "total_questions"),
    );

    lines.push(Line::from(vec![
        Span::styled("? ", Style::default().fg(theme::RUST_ORANGE)),
        Span::styled(title, Style::default().add_modifier(Modifier::BOLD)),
    ]));

    if let Some(question) = question {
        for row in question.lines() {
            lines.push(Line::from(Span::styled(
                format!("  {row}"),
                Style::default().fg(theme::DIM),
            )));
        }
    }

    let selected_options: &[Value] =
        result.get("selected_options").and_then(Value::as_array).map_or(&[], Vec::as_slice);
    if !selected_options.is_empty() {
        lines.push(Line::default());
        for option in selected_options {
            render_selected_question_option(option, lines);
        }
    }

    render_question_annotation_section(result, "preview", "Preview", lines);
    render_question_annotation_section(result, "notes", "Notes", lines);
}

fn ask_user_question_result_title(
    header: &str,
    question_index: Option<usize>,
    total_questions: Option<usize>,
) -> String {
    match (question_index, total_questions) {
        (Some(index), Some(total)) if total > 1 => format!("{header} ({}/{total})", index + 1),
        _ => header.to_owned(),
    }
}

fn render_selected_question_option(option: &Value, lines: &mut Vec<Line<'static>>) {
    let Some(label) = json_str(option, "label") else {
        return;
    };
    let mut spans = vec![
        Span::styled("  [x] ", Style::default().fg(theme::DIM)),
        Span::styled(label.to_owned(), Style::default().add_modifier(Modifier::BOLD)),
    ];

    if let Some(description) = json_str(option, "description").map(single_line_text) {
        spans.push(Span::styled(" - ", Style::default().fg(theme::DIM)));
        spans.push(Span::styled(description, Style::default().fg(theme::DIM)));
    }
    lines.push(Line::from(spans));
}

fn render_question_annotation_section(
    result: &Value,
    key: &str,
    title: &str,
    lines: &mut Vec<Line<'static>>,
) {
    let Some(text) = result.get("annotation").and_then(|annotation| json_str(annotation, key))
    else {
        return;
    };

    lines.push(Line::default());
    if key == "notes" {
        lines.push(Line::from(vec![
            Span::styled(
                format!("{ASK_USER_QUESTION_RESULT_TEXT_INDENT}{title}: "),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(single_line_text(text), Style::default().fg(theme::DIM)),
        ]));
    } else {
        lines.push(Line::from(Span::styled(
            format!("{ASK_USER_QUESTION_RESULT_TEXT_INDENT}{title}"),
            Style::default().add_modifier(Modifier::BOLD),
        )));
        for row in text.lines() {
            lines.push(Line::from(Span::styled(
                format!("{ASK_USER_QUESTION_RESULT_TEXT_INDENT}  {row}"),
                Style::default().fg(theme::DIM),
            )));
        }
    }
}

fn single_line_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn json_str<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str().map(str::trim).filter(|text| !text.is_empty())
}

fn json_usize(value: &Value, key: &str) -> Option<usize> {
    value.get(key)?.as_u64().and_then(|number| usize::try_from(number).ok())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ToolContentHeightPolicy {
    /// Read source owns its gutter, wrapping, omission count, and row budget.
    Preformatted,
    Bounded,
    Unbounded,
}

fn tool_content_height_policy(tc: &ToolCallInfo) -> ToolContentHeightPolicy {
    if renders_only_plan_file_content(tc) || renders_structured_ask_user_question_result(tc) {
        ToolContentHeightPolicy::Unbounded
    } else if is_read_tool(tc)
        && !matches!(tc.status, model::ToolCallStatus::Failed | model::ToolCallStatus::Killed)
    {
        ToolContentHeightPolicy::Preformatted
    } else {
        ToolContentHeightPolicy::Bounded
    }
}

fn renders_structured_ask_user_question_result(tc: &ToolCallInfo) -> bool {
    tc.is_ask_question_tool()
        && tc.pending_question.is_none()
        && matches!(tc.status, model::ToolCallStatus::Completed)
        && tc
            .raw_input
            .as_ref()
            .and_then(|input| input.get("question_results"))
            .and_then(Value::as_array)
            .is_some_and(|results| !results.is_empty())
}

fn renders_only_plan_file_content(tc: &ToolCallInfo) -> bool {
    let mut saw_plan_file = false;

    for content in &tc.content {
        match content {
            model::ToolCallContent::Diff(diff) if is_plan_file_path(&diff.path) => {
                saw_plan_file = true;
            }
            model::ToolCallContent::Terminal(_) => {}
            model::ToolCallContent::Diff(_)
            | model::ToolCallContent::McpResource(_)
            | model::ToolCallContent::ResourceLink(_)
            | model::ToolCallContent::Content(_) => return false,
        }
    }

    saw_plan_file
}

fn is_read_tool(tc: &ToolCallInfo) -> bool {
    tc.sdk_tool_name.eq_ignore_ascii_case("read")
}

fn protected_content_source_lines(tc: &ToolCallInfo, lines: &[Line<'static>]) -> usize {
    if tc.output_metadata.as_ref().is_some_and(|metadata| metadata.presentation.is_some()) {
        // Structured records begin with identity and timestamps. Keep that prefix
        // within the existing preview budget rather than showing an unlabelled tail.
        return lines.len().min(TOOL_BODY_MAX_LINES.saturating_sub(1));
    }
    let mut count =
        if matches!(tc.content.first(), Some(model::ToolCallContent::ResourceLink(_))) {
            2
        } else {
            leading_diff_metadata_line_count(lines)
        }
        .min(lines.len());

    while lines.get(count).is_some_and(line_text_is_existing_omission_marker) {
        count += 1;
    }
    count
}

fn render_plan_content(text: &str, width: u16, control: &LayoutControl<'_>) -> Vec<Line<'static>> {
    let md_source = strip_outer_code_fence(text);
    markdown::render_markdown_with_control(&md_source, width, None, false, control)
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

fn render_text_content(
    tc: &ToolCallInfo,
    text: &str,
    width: u16,
    lines: &mut Vec<Line<'static>>,
    read_cursor: &mut Option<read::ReadCursor>,
    control: &LayoutControl<'_>,
) -> Poll<()> {
    if is_read_tool(tc) {
        let read = tc.read_output_metadata();
        if read.is_none() {
            let stripped = strip_outer_code_fence(text);
            if let Some(failed_lines) = render_failed_tool_text_content(tc.status, &stripped) {
                lines.extend(failed_lines);
                return Poll::Ready(());
            }
            if let Some(viewed_image) = render_viewed_image_content(tc, &stripped) {
                lines.push(viewed_image);
                return Poll::Ready(());
            }
        }
        let title_lang = lang_from_title(&tc.title);
        let source = read::ReadSource {
            text,
            start_line: read.map(|read| read.start_line),
            path: read.map(|read| read.file_path.as_path()).or_else(|| read_syntax_path(tc)),
            language_fallback: (!title_lang.is_empty()).then_some(title_lang.as_str()),
        };
        let cursor = read_cursor.get_or_insert_with(|| read::ReadCursor::new(&source, width));
        let Poll::Ready(rendered) = cursor.poll(&source, control) else {
            return Poll::Pending;
        };
        lines.extend(rendered);
    } else {
        render_generic_text_content(tc, text, width, lines, control);
    }
    Poll::Ready(())
}

fn render_viewed_image_content(tc: &ToolCallInfo, text: &str) -> Option<Line<'static>> {
    let file_name = text.strip_prefix(VIEWED_IMAGE_RESULT_PREFIX)?.trim();
    if file_name.is_empty() || file_name.contains(['\r', '\n']) {
        return None;
    }

    let read_path = read_syntax_path(tc)?.as_os_str().to_str()?.trim_end_matches(['/', '\\']);
    let expected_file_name = read_path.rsplit(['/', '\\']).next()?;
    if file_name != expected_file_name {
        return None;
    }

    Some(Line::from(vec![
        Span::styled(
            "Viewed image",
            Style::default().fg(theme::DIM).add_modifier(Modifier::ITALIC),
        ),
        Span::raw(" "),
        Span::styled(file_name.to_owned(), Style::default().fg(ratatui::style::Color::White)),
    ]))
}

fn render_generic_text_content(
    tc: &ToolCallInfo,
    text: &str,
    width: u16,
    lines: &mut Vec<Line<'static>>,
    control: &LayoutControl<'_>,
) {
    let stripped = strip_outer_code_fence(text);
    if let Some(failed_lines) = render_failed_tool_text_content(tc.status, &stripped) {
        lines.extend(failed_lines);
        return;
    }
    let md_source = if is_markdown_file(&tc.title) {
        stripped
    } else {
        let lang = lang_from_title(&tc.title);
        lines.extend(highlight::highlight_code_with_control(
            &stripped,
            (!lang.is_empty()).then_some(lang.as_str()),
            control,
        ));
        return;
    };
    render_markdown_content(&md_source, width, lines, control);
}

fn render_mcp_resource_text_content(
    tc: &ToolCallInfo,
    text: &str,
    width: u16,
    lines: &mut Vec<Line<'static>>,
    control: &LayoutControl<'_>,
) {
    let stripped = strip_outer_code_fence(text);
    if let Some(failed_lines) = render_failed_tool_text_content(tc.status, &stripped) {
        lines.extend(failed_lines);
        return;
    }
    if is_markdown_file(&tc.title) {
        render_markdown_content(&stripped, width, lines, control);
    } else {
        lines.extend(highlight::plain_text_lines(&stripped));
    }
}

fn render_markdown_content(
    text: &str,
    width: u16,
    lines: &mut Vec<Line<'static>>,
    control: &LayoutControl<'_>,
) {
    for line in markdown::render_markdown_with_control(text, width, None, false, control) {
        let owned: Vec<Span<'static>> =
            line.spans.into_iter().map(|s| Span::styled(s.content.into_owned(), s.style)).collect();
        lines.push(Line::from(owned));
    }
}

fn read_syntax_path(tc: &ToolCallInfo) -> Option<&Path> {
    tc.locations
        .first()
        .map(|location| location.path.as_path())
        .or_else(|| raw_input_file_path(tc).map(Path::new))
}

fn raw_input_file_path(tc: &ToolCallInfo) -> Option<&str> {
    tc.raw_input
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .and_then(|input| input.get("file_path"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty())
}

fn render_mcp_resource_content(
    tc: &ToolCallInfo,
    resource: &model::McpResource,
    width: u16,
    control: &LayoutControl<'_>,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if let Some(text) = resource.text.as_deref() {
        render_mcp_resource_text_content(tc, text, width, &mut lines, control);
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

fn render_mcp_resource_link_content(link: &model::McpResourceLink) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(vec![
        Span::styled("Resource: ", Style::default().fg(theme::DIM).add_modifier(Modifier::BOLD)),
        Span::raw(link.title.as_deref().unwrap_or(&link.name).to_owned()),
    ])];
    lines.push(Line::from(vec![
        Span::styled("URI: ", Style::default().fg(theme::DIM).add_modifier(Modifier::BOLD)),
        Span::raw(link.uri.clone()),
    ]));
    if let Some(description) = &link.description {
        lines.push(Line::from(vec![
            Span::styled(
                "Description: ",
                Style::default().fg(theme::DIM).add_modifier(Modifier::BOLD),
            ),
            Span::raw(description.clone()),
        ]));
    }
    if let Some(mime_type) = &link.mime_type {
        lines.push(Line::from(vec![
            Span::styled("Type: ", Style::default().fg(theme::DIM).add_modifier(Modifier::BOLD)),
            Span::raw(mime_type.clone()),
        ]));
    }
    if let Some(size) = link.size {
        lines.push(Line::from(vec![
            Span::styled("Size: ", Style::default().fg(theme::DIM).add_modifier(Modifier::BOLD)),
            Span::raw(format!("{size} bytes")),
        ]));
    }
    if let Some(annotations) = &link.annotations
        && let Ok(value) = serde_json::to_string(annotations)
    {
        lines.push(Line::from(vec![
            Span::styled(
                "Annotations: ",
                Style::default().fg(theme::DIM).add_modifier(Modifier::BOLD),
            ),
            Span::raw(value),
        ]));
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

fn cap_tool_content_lines(
    lines: Vec<Line<'static>>,
    width: u16,
    omitted_label: &str,
    protected_source_lines: usize,
    control: &LayoutControl<'_>,
) -> Vec<Line<'static>> {
    let source_line_count = lines.len();
    let protected_source_lines = protected_source_lines.min(source_line_count);
    let mut head = lines;
    let rest = head.split_off(protected_source_lines);
    let head = measure_content_lines(head, width, control);
    let protected_rows = content_height(&head);

    // Every logical line produces at least one row. When the source already
    // exceeds the cap, only the protected prefix and the visible tail need layout.
    // Hidden middle lines are counted by source identity, not wrapped into cells.
    if source_line_count > TOOL_BODY_MAX_LINES && protected_rows < TOOL_BODY_MAX_LINES {
        let tail_budget = TOOL_BODY_MAX_LINES - protected_rows - 1;
        let mut tail = std::collections::VecDeque::new();
        let mut first_tail = source_line_count;
        for (index, line) in rest.into_iter().enumerate().rev() {
            if tail.len() >= tail_budget || control.cancelled() {
                break;
            }
            let layout = crate::ui::wrap::WrappedLineLayout::new(line, width);
            let end = layout.height();
            let start = end.saturating_sub(tail_budget - tail.len());
            for row in layout.render(start..end, control).into_iter().rev() {
                tail.push_front(row);
                first_tail = protected_source_lines + index;
            }
        }
        let omitted = first_tail.saturating_sub(protected_source_lines);
        let (_, mut out) = render_content_window(head, 0..protected_rows, control);
        out.push(Line::from(Span::styled(
            format!("... {omitted} {omitted_label} ..."),
            Style::default().fg(theme::DIM).add_modifier(Modifier::ITALIC),
        )));
        out.extend(tail);
        return out;
    }
    let rest = measure_content_lines(rest, width, control);
    let remaining_rows = content_height(&rest);
    let total_rows = protected_rows + remaining_rows;
    if total_rows <= TOOL_BODY_MAX_LINES {
        return render_content_window(
            head.into_iter().chain(rest).collect(),
            0..total_rows,
            control,
        )
        .1;
    }

    if protected_rows >= TOOL_BODY_MAX_LINES {
        let visible = TOOL_BODY_MAX_LINES.saturating_sub(1);
        let omitted_rows = total_rows - visible;
        let (_, mut out) = render_content_window(head, 0..visible, control);
        out.push(Line::from(Span::styled(
            format!("... {omitted_rows} wrapped rows hidden ..."),
            Style::default().fg(theme::DIM).add_modifier(Modifier::ITALIC),
        )));
        return out;
    }

    let tail = TOOL_BODY_MAX_LINES.saturating_sub(protected_rows + 1);
    let omitted_rows = remaining_rows.saturating_sub(tail);
    let (first_tail_source, tail_rows) =
        render_content_window(rest, omitted_rows..remaining_rows, control);
    let omitted_source_lines =
        first_tail_source.unwrap_or(source_line_count - protected_source_lines);
    let omitted = if omitted_source_lines > 0 {
        format!("... {omitted_source_lines} {omitted_label} ...")
    } else {
        format!("... {omitted_rows} wrapped rows hidden ...")
    };

    let (_, mut out) = render_content_window(head, 0..protected_rows, control);
    out.push(Line::from(Span::styled(
        omitted,
        Style::default().fg(theme::DIM).add_modifier(Modifier::ITALIC),
    )));
    out.extend(tail_rows);
    out
}

fn measure_content_lines(
    lines: Vec<Line<'static>>,
    width: u16,
    control: &LayoutControl<'_>,
) -> Vec<crate::ui::wrap::WrappedLineLayout> {
    let mut layouts = Vec::new();
    for line in lines {
        if control.cancelled() {
            return Vec::new();
        }
        layouts.push(crate::ui::wrap::WrappedLineLayout::new(line, width));
    }
    layouts
}

fn content_height(lines: &[crate::ui::wrap::WrappedLineLayout]) -> usize {
    lines.iter().map(crate::ui::wrap::WrappedLineLayout::height).sum()
}

/// Materialize only the selected physical rows, retaining their source association
/// for omission counts. Hidden rows never become terminal cells or owned spans.
fn render_content_window(
    lines: Vec<crate::ui::wrap::WrappedLineLayout>,
    window: std::ops::Range<usize>,
    control: &LayoutControl<'_>,
) -> (Option<usize>, Vec<Line<'static>>) {
    let mut rows = Vec::new();
    let mut first_source = None;
    let mut offset = 0;
    for (source_index, line) in lines.into_iter().enumerate() {
        if offset >= window.end || control.cancelled() {
            break;
        }
        let height = line.height();
        let start = window.start.saturating_sub(offset).min(height);
        let end = window.end.saturating_sub(offset).min(height);
        if start < end {
            first_source.get_or_insert(source_index);
            rows.extend(line.render(start..end, control));
        }
        offset += height;
    }
    (first_source, rows)
}

fn line_text_is_existing_omission_marker(line: &Line<'static>) -> bool {
    let text = line_plain_text(line);
    text.contains("lines hidden") || text.contains("lines omitted")
}

fn line_plain_text(line: &Line<'static>) -> String {
    line.spans.iter().flat_map(|span| span.content.chars()).collect()
}

#[cfg(test)]
mod replay_tests {
    use super::*;

    #[test]
    fn capped_long_line_materializes_only_the_visible_tail() {
        let source = format!("{}TAIL", "x".repeat(29_996));
        let checks = std::cell::Cell::new(0);
        let cancel_unbounded_materialization = || {
            checks.set(checks.get() + 1);
            checks.get() > 40
        };
        let control = LayoutControl::new(&cancel_unbounded_materialization);
        let rows = cap_tool_content_lines(
            vec![Line::from(Span::styled(source, Style::default().fg(theme::RUST_ORANGE)))],
            20,
            "source lines hidden",
            0,
            &control,
        );
        assert!(!control.cancelled(), "hidden physical rows must not be materialized");
        assert_eq!(rows[0].to_string(), "... 1492 wrapped rows hidden ...");
        assert_eq!(rows.last().unwrap().to_string(), format!("{}TAIL", "x".repeat(16)));
        assert_eq!(rows.len(), TOOL_BODY_MAX_LINES);
        assert!(rows[1..].iter().all(|row| {
            row.width() == 20
                && row.spans.iter().all(|span| span.style.fg == Some(theme::RUST_ORANGE))
        }));
    }

    #[test]
    fn capped_content_preserves_protected_rows_and_source_omission_counts() {
        let control = LayoutControl::uninterrupted();
        let rows = cap_tool_content_lines(
            vec![Line::from("HEADER"), Line::from("hidden"), Line::from("tail".repeat(40))],
            20,
            "source lines hidden",
            1,
            &control,
        );
        assert_eq!(rows[0].to_string().trim_end(), "HEADER");
        assert_eq!(rows[1].to_string(), "... 1 source lines hidden ...");
        assert_eq!(
            rows[2..].iter().map(Line::to_string).collect::<Vec<_>>(),
            vec!["tail".repeat(5); 7]
        );

        // A protected prefix that fills the preview keeps its beginning, not its tail.
        let rows = cap_tool_content_lines(
            vec![Line::from("HEAD".repeat(50)), Line::from("body".repeat(10))],
            20,
            "source lines hidden",
            1,
            &control,
        );
        assert_eq!(
            rows[..8].iter().map(Line::to_string).collect::<Vec<_>>(),
            vec!["HEAD".repeat(5); 8]
        );
        assert_eq!(rows[8].to_string(), "... 4 wrapped rows hidden ...");
    }

    #[test]
    fn capped_output_does_not_lay_out_the_hidden_middle() {
        let mut lines = vec![Line::from("HEADER")];
        lines.extend((0..1000).map(|index| Line::from(format!("source {index}"))));
        let checks = std::cell::Cell::new(0);
        let cancel_after_bounded_work = || {
            checks.set(checks.get() + 1);
            checks.get() > 100
        };
        let control = LayoutControl::new(&cancel_after_bounded_work);
        let rows = cap_tool_content_lines(lines, 40, "source lines hidden", 1, &control);
        assert!(!control.cancelled(), "hidden middle lines must not consume layout work");
        let text =
            rows.iter().map(|line| line.to_string().trim_end().to_owned()).collect::<Vec<_>>();
        assert_eq!(text[0], "HEADER");
        assert_eq!(text[1], "... 993 source lines hidden ...");
        assert_eq!(
            &text[2..],
            &(993..1000).map(|index| format!("source {index}")).collect::<Vec<_>>()
        );
    }
}
