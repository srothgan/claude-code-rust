// SPDX-License-Identifier: Apache-2.0
//! Tool-call rendering: entry points, caching, and shared helpers.
//!
//! Submodules handle specific rendering concerns:
//! - [`standard`] -- non-Execute tool calls (Read, Write, Glob, etc.)
//! - [`execute`] -- Execute/Bash content rendering
//! - [`interactions`] -- inline permissions, questions, and plan approvals
//! - [`errors`] -- error rendering and tool-use error extraction

mod artifact;
mod cron;
mod errors;
mod execute;
mod fields;
mod interactions;
mod monitor;
mod projects;
mod push_notification;
mod read;
mod remote_trigger;
mod repl;
mod result_presentation;
mod schedule_wakeup;
mod skill;
mod standard;
mod tasks;
mod typed;
mod workflow;
mod worktree;

use std::borrow::Cow;

use crate::agent::model;
use crate::app::ToolCallInfo;
use crate::ui::markdown;
use crate::ui::theme;
use crate::ui::tool_display;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

// Re-export submodule items used by tests.
#[cfg(test)]
use errors::{
    extract_tool_use_error_message, looks_like_internal_error, render_failed_tool_text_content,
    render_tool_use_error_content, summarize_internal_error,
};

#[cfg(test)]
use standard::cap_write_diff_lines;

pub(super) const TOOL_MAX_RENDER_LINES: usize = 10;
pub(super) const TOOL_BODY_MAX_LINES: usize = TOOL_MAX_RENDER_LINES - 1;

#[derive(Clone, Copy, Default)]
pub struct ToolCallRenderContext<'a> {
    pub clock: crate::app::presentation::ClockFormat<'a>,
    pub current_mode_id: Option<&'a str>,
    pub control: Option<&'a crate::ui::layout_control::LayoutControl<'a>>,
}

pub fn status_icon(
    status: model::ToolCallStatus,
    spinner: crate::ui::SpinnerState,
) -> (&'static str, Color) {
    match status {
        model::ToolCallStatus::Pending => ("\u{25CB}", theme::RUST_ORANGE),
        model::ToolCallStatus::Detached => (theme::ICON_DETACHED, Color::LightBlue),
        model::ToolCallStatus::InProgress => {
            let s = spinner.icon();
            (s, theme::RUST_ORANGE)
        }
        model::ToolCallStatus::Completed => (theme::ICON_COMPLETED, theme::RUST_ORANGE),
        model::ToolCallStatus::Failed | model::ToolCallStatus::Killed => {
            (theme::ICON_FAILED, theme::STATUS_ERROR)
        }
    }
}

// ---------------------------------------------------------------------------
// Public entry points (delegating to submodules)
// ---------------------------------------------------------------------------

/// One active tool traversal. Title and body can be delivered separately, while
/// a large file preview keeps its syntax state across scheduling turns.
#[derive(Default)]
pub(crate) struct ToolRowsCursor {
    source_revision: Option<u64>,
    body: standard::ToolBodyCursor,
    finished: bool,
}

impl ToolRowsCursor {
    pub(crate) fn is_finished(&self) -> bool {
        self.finished
    }

    pub(crate) fn next(
        &mut self,
        tc: &mut ToolCallInfo,
        context: ToolCallRenderContext<'_>,
        width: u16,
        spinner: crate::ui::SpinnerState,
    ) -> Vec<Line<'static>> {
        let uninterrupted = crate::ui::layout_control::LayoutControl::uninterrupted();
        let control = context.control.unwrap_or(&uninterrupted);
        if self.finished || control.cancelled() {
            return Vec::new();
        }
        if self.source_revision.is_some_and(|revision| revision != tc.cache.source_revision()) {
            control.cancel();
            return Vec::new();
        }
        if self.source_revision.is_none() {
            self.source_revision = Some(tc.cache.source_revision());
            self.finished = !standard::tool_call_has_body(tc);
            return vec![standard::render_tool_call_title(tc, context, width, spinner)];
        }
        if let Some(cached) = tc.cache.get_for_width(width) {
            self.finished = true;
            return cached.clone();
        }
        let body = if let Some(result) =
            tc.output_metadata.as_ref().and_then(|metadata| metadata.presentation.as_deref())
        {
            std::task::Poll::Ready(standard::render_result_presentation(
                tc,
                result,
                context.clock,
                width,
                control,
            ))
        } else if control.can_yield() {
            self.body.poll(tc, width, control)
        } else {
            std::task::Poll::Ready(self.body.render(tc, width, control))
        };
        let std::task::Poll::Ready(body) = body else {
            return Vec::new();
        };
        if control.cancelled() {
            return Vec::new();
        }
        self.finished = true;
        tc.cache.store_for_width(body.clone(), width);
        body
    }
}

/// Synchronous consumers drain the same cursor used by incremental replay.
pub fn render_tool_call_cached(
    tc: &mut ToolCallInfo,
    context: ToolCallRenderContext<'_>,
    width: u16,
    spinner: crate::ui::SpinnerState,
    out: &mut Vec<Line<'static>>,
) {
    let mut cursor = ToolRowsCursor::default();
    while !cursor.is_finished() {
        out.extend(cursor.next(tc, context, width, spinner));
        if context.control.is_some_and(crate::ui::layout_control::LayoutControl::cancelled) {
            break;
        }
    }
}

// ---------------------------------------------------------------------------
// Shared helpers (used by multiple submodules)
// ---------------------------------------------------------------------------

fn markdown_inline_spans(input: &str) -> Vec<Span<'static>> {
    markdown::render_markdown_safe(input, None).into_iter().next().map_or_else(Vec::new, |line| {
        line.spans.into_iter().map(|s| Span::styled(s.content.into_owned(), s.style)).collect()
    })
}

fn spans_width(spans: &[Span<'static>]) -> usize {
    spans.iter().map(|s| UnicodeWidthStr::width(s.content.as_ref())).sum()
}

fn truncate_spans_to_width(spans: Vec<Span<'static>>, max_width: usize) -> Vec<Span<'static>> {
    if max_width == 0 {
        return Vec::new();
    }
    if spans_width(&spans) <= max_width {
        return spans;
    }

    let keep_width = max_width.saturating_sub(1);
    let mut used = 0usize;
    let mut out: Vec<Span<'static>> = Vec::new();

    for span in spans {
        if used >= keep_width {
            break;
        }
        let mut chunk = String::new();
        for ch in span.content.chars() {
            let w = UnicodeWidthChar::width(ch).unwrap_or(0);
            if used + w > keep_width {
                break;
            }
            chunk.push(ch);
            used += w;
        }
        if !chunk.is_empty() {
            out.push(Span::styled(chunk, span.style));
        }
    }
    out.push(Span::styled("\u{2026}", Style::default().fg(theme::DIM)));
    out
}

fn tool_output_badge_spans(tc: &ToolCallInfo) -> Vec<Span<'static>> {
    let mut badges = Vec::new();

    if tc.output_metadata.as_ref().is_some_and(|metadata| metadata.structured_content_omitted) {
        badges.push(Span::styled("  [structured output omitted]", Style::default().fg(theme::DIM)));
    }

    if let Some(timeout_ms) = tc.timed_out_after_ms() {
        badges.push(Span::styled(
            format!("  [auto-backgrounded after {} ms]", format_u64_grouped(timeout_ms)),
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ));
    } else if tc.assistant_auto_backgrounded() {
        badges.push(Span::styled(
            "  [assistant backgrounded]",
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ));
    }

    if tc.background_ends_with_final_response() {
        badges.push(Span::styled("  [ends with final response]", Style::default().fg(theme::DIM)));
    }

    if tc.task_metadata.as_ref().and_then(|metadata| metadata.terminal_reason.as_deref())
        == Some("worker_restart")
    {
        badges.push(Span::styled(
            "  [worker restarted]",
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ));
    }

    if model::is_agent_tool_name(&tc.sdk_tool_name) {
        if let Some(depth) = tc.task_spawn_depth() {
            badges.push(Span::styled(
                format!("  [depth: {depth}]"),
                Style::default().fg(Color::LightBlue).add_modifier(Modifier::BOLD),
            ));
        }
        if let Some(retry) = subagent_retry_badge(tc) {
            badges.push(Span::styled(
                format!("  [{retry}]"),
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ));
        }
        if let Some(agent_type) = agent_requested_type_badge(tc) {
            badges.push(Span::styled(
                format!("  [type: {agent_type}]"),
                Style::default().fg(Color::LightBlue).add_modifier(Modifier::BOLD),
            ));
        }
        if let Some((model, _)) = agent_model_choice(Some(tc), None, None) {
            let label = if model.contains(" -> ") { "models" } else { "model" };
            badges.push(Span::styled(
                format!("  [{label}: {model}]"),
                Style::default().fg(Color::LightBlue).add_modifier(Modifier::BOLD),
            ));
        }
    }

    if tc.task_is_backgrounded() || tc.skill_is_backgrounded() {
        badges.push(Span::styled(
            "  [backgrounded]",
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ));
    }

    badges
}

fn subagent_retry_badge(tc: &ToolCallInfo) -> Option<String> {
    if !matches!(tc.status, model::ToolCallStatus::InProgress) {
        return None;
    }
    let model::SubagentRetryUpdate::Waiting { attempt, max_retries, retry_delay_ms, .. } =
        tc.task_metadata.as_ref()?.subagent_retry.as_ref()?
    else {
        return None;
    };
    Some(format!("retry {attempt}/{max_retries} in {}", format_retry_delay(*retry_delay_ms)))
}

fn format_retry_delay(delay_ms: u64) -> String {
    if delay_ms < 1_000 {
        return format!("{delay_ms}ms");
    }
    let seconds = delay_ms / 1_000;
    let milliseconds = delay_ms % 1_000;
    if milliseconds == 0 {
        return format!("{seconds}s");
    }
    let fractional = format!("{milliseconds:03}").trim_end_matches('0').to_owned();
    format!("{seconds}.{fractional}s")
}

fn format_u64_grouped(value: u64) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    grouped
}

fn tool_raw_input_string<'a>(tc: &'a ToolCallInfo, key: &str) -> Option<&'a str> {
    tc.raw_input
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .and_then(|input| input.get(key))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn agent_requested_type_badge(tc: &ToolCallInfo) -> Option<String> {
    let name = tool_raw_input_string(tc, "name")?;
    let subagent_type = tool_raw_input_string(tc, "subagent_type")?;
    let expected_title = format!("{}: {name}", tc.sdk_tool_name);
    if name == subagent_type || tc.title.trim() != expected_title {
        return None;
    }
    Some(subagent_type.to_owned())
}

pub(crate) fn agent_model_choice(
    tc: Option<&ToolCallInfo>,
    kind: Option<&str>,
    default: Option<&str>,
) -> Option<(String, &'static str)> {
    let metadata = tc.and_then(|tc| tc.output_metadata.as_ref()?.agent.as_ref());
    if let Some(models) =
        metadata.and_then(|meta| meta.models_used.as_ref()).filter(|models| models.len() >= 2)
    {
        return Some((models.join(" -> "), "observed"));
    }
    if let Some(model) = metadata
        .and_then(|meta| meta.resolved_model.as_deref())
        .map(str::trim)
        .filter(|model| !model.is_empty())
    {
        return Some((model.to_owned(), "observed"));
    }
    if kind.or_else(|| tc.and_then(|tc| tool_raw_input_string(tc, "subagent_type"))) == Some("fork")
    {
        return Some(("parent model".to_owned(), "inherited"));
    }
    if let Some(model) = tc.and_then(|tc| tool_raw_input_string(tc, "model")) {
        return Some((model.to_owned(), "requested"));
    }
    default.map(|model| (model.to_owned(), "agent default"))
}

fn tool_display_title<'a>(
    tc: &'a ToolCallInfo,
    render_context: ToolCallRenderContext<'_>,
) -> Cow<'a, str> {
    if tc.is_ask_question_tool() {
        return ask_user_question_display_title(tc);
    }

    if render_context.current_mode_id == Some("plan") {
        match tc.sdk_tool_name.as_str() {
            "Write" => return Cow::Borrowed("Create Plan"),
            "Edit" | "MultiEdit" => return Cow::Borrowed("Update Plan"),
            _ => {}
        }
    }

    tool_display::tool_title(&tc.sdk_tool_name, &tc.title)
}

fn ask_user_question_display_title(tc: &ToolCallInfo) -> Cow<'static, str> {
    if let Some(question) = &tc.pending_question {
        return if question.total_questions > 1 {
            Cow::Owned(format!("Questions ({})", question.total_questions))
        } else {
            Cow::Borrowed("Question")
        };
    }

    let result_count = tc
        .raw_input
        .as_ref()
        .and_then(|input| input.get("question_results"))
        .and_then(serde_json::Value::as_array)
        .map_or(0, Vec::len);

    match result_count {
        0 => Cow::Borrowed("Question"),
        1 => Cow::Borrowed("Answered question"),
        count => Cow::Owned(format!("Answered questions ({count})")),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(super) mod test_support {
    use crate::agent::model;
    use crate::app::ToolCallInfo;
    use ratatui::text::Line;

    pub(crate) fn interaction_app(tools: Vec<ToolCallInfo>) -> crate::app::App {
        use crate::app::{App, AppStatus, ChatMessage, MessageBlock, MessageRole};
        let mut app = App::test_default();
        app.status = AppStatus::Running;
        app.session_runtime.session_id = Some("interaction-render".into());
        let ids: Vec<_> = tools.iter().map(|tool| tool.id.clone()).collect();
        app.push_message_tracked(ChatMessage::new(
            MessageRole::Assistant,
            tools.into_iter().map(|tool| MessageBlock::ToolCall(Box::new(tool))).collect(),
            None,
        ));
        for (index, id) in ids.into_iter().enumerate() {
            app.index_tool_call(id, 0, index);
        }
        app.bind_active_turn_assistant(0);
        app
    }

    pub(crate) fn attach_question(
        app: &mut crate::app::App,
        id: &str,
        prompt: model::QuestionPrompt,
    ) -> tokio::sync::oneshot::Receiver<model::RequestQuestionResponse> {
        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        crate::app::handle_client_event(
            app,
            crate::agent::events::ClientEvent::QuestionRequest {
                session_id: "interaction-render".into(),
                request: model::RequestQuestionRequest::new(
                    "interaction-render",
                    model::ToolCallUpdate::new(id, model::ToolCallUpdateFields::new()),
                    prompt,
                    0,
                    1,
                ),
                response_tx,
            },
        );
        response_rx
    }

    pub(crate) fn attach_permission(
        app: &mut crate::app::App,
        id: &str,
        options: Vec<model::PermissionOption>,
        display: Option<model::PermissionDisplay>,
    ) -> tokio::sync::oneshot::Receiver<model::RequestPermissionResponse> {
        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        crate::app::handle_client_event(
            app,
            crate::agent::events::ClientEvent::PermissionRequest {
                session_id: "interaction-render".into(),
                request: model::RequestPermissionRequest::new(
                    "interaction-render",
                    model::ToolCallUpdate::new(id, model::ToolCallUpdateFields::new()),
                    options,
                    display,
                ),
                response_tx,
            },
        );
        response_rx
    }

    pub(crate) fn press(app: &mut crate::app::App, code: crossterm::event::KeyCode) {
        use crossterm::event::{Event, KeyEvent, KeyModifiers};
        crate::app::handle_terminal_event(app, Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
    }

    pub(crate) fn interaction_card(
        app: &mut crate::app::App,
        id: &str,
        width: u16,
    ) -> Vec<Line<'static>> {
        let (mi, bi) = app.lookup_tool_call(id).expect("target tool");
        let tool = app.tool_call_at_mut(mi, bi).expect("target card");
        let mut rows = Vec::new();
        super::render_tool_call_cached(
            tool,
            super::ToolCallRenderContext::default(),
            width,
            crate::ui::SpinnerState::Static,
            &mut rows,
        );
        rows
    }

    pub(crate) fn assert_focused_option(rows: &[Line<'_>], labels: [&str; 3], focused: usize) {
        let styles: Vec<_> = labels
            .iter()
            .map(|label| {
                let mut styles = Vec::new();
                for row in rows {
                    let text = row.to_string();
                    for (position, _) in text.match_indices(label) {
                        let mut offset = 0;
                        let span = row
                            .spans
                            .iter()
                            .find(|span| {
                                offset += span.content.len();
                                position < offset
                            })
                            .expect("rendered label style");
                        styles.push(span.style);
                    }
                }
                assert_eq!(styles.len(), 1, "option must appear once in its card: {label}");
                styles[0]
            })
            .collect();
        let inactive = styles[(focused + 1) % labels.len()];
        for (index, style) in styles.iter().enumerate() {
            assert_eq!(
                *style == inactive,
                index != focused,
                "focus must distinguish {}",
                labels[index]
            );
        }
    }

    pub(super) fn tool_call(
        id: &str,
        title: &str,
        sdk_tool_name: &str,
        raw_input: serde_json::Value,
        content: Option<&str>,
        status: model::ToolCallStatus,
    ) -> ToolCallInfo {
        let mut tc = ToolCallInfo {
            title: title.to_owned(),
            raw_input: Some(raw_input),
            ..crate::app::test_support::named_tool_call_info(id, sdk_tool_name, status)
        };
        if let Some(content) = content {
            tc.content = vec![model::ToolCallContent::from(content)];
        }
        tc
    }

    pub(super) fn rendered_line_texts(lines: &[Line<'static>]) -> Vec<String> {
        lines
            .iter()
            .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
            .collect()
    }

    pub(super) fn card_body(tc: &mut ToolCallInfo, width: u16) -> Vec<String> {
        let mut lines = Vec::new();
        super::render_tool_call_cached(
            tc,
            super::ToolCallRenderContext::default(),
            width,
            crate::ui::SpinnerState::Static,
            &mut lines,
        );
        lines
            .into_iter()
            .skip(1)
            .map(|line| {
                let text: String =
                    line.spans.iter().skip(1).map(|span| span.content.as_ref()).collect();
                text.trim_end().to_owned()
            })
            .collect()
    }

    pub(super) fn set_result_fields(tc: &mut ToolCallInfo, fields: &[(&str, &str)]) {
        let fields: Vec<_> = fields.iter().map(|(label,value)| {
            serde_json::json!({"label":label,"value":{"type":"text","value":value}})
        }).collect();
        tc.output_metadata = Some(
            serde_json::from_value(serde_json::json!({
                "presentation":{"sections":[{"fields":fields}]}
            }))
            .expect("existing wire presentation contract"),
        );
    }

    pub(super) fn confirmed_receipt(id: &str) -> ToolCallInfo {
        let fixtures: Vec<serde_json::Value> = serde_json::from_str(include_str!(
            "../../../tests/fixtures/sdk-confirmed-tool-results.json"
        ))
        .expect("independent SDK receipt fixtures");
        let row = fixtures.iter().find(|row| row["id"] == id).expect("named receipt");
        let mut tc = tool_call(
            id,
            id,
            row["tool"].as_str().expect("tool name"),
            row["input"].clone(),
            row["text"].as_str(),
            model::ToolCallStatus::Completed,
        );
        let fields: Vec<_> = row["fields"]
            .as_array()
            .expect("receipt fields")
            .iter()
            .map(|field| {
                (field[0].as_str().expect("field label"), field[1].as_str().expect("literal value"))
            })
            .collect();
        set_result_fields(&mut tc, &fields);
        tc
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod title_audit_tests;
