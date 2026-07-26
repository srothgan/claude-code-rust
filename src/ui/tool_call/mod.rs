// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang
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
mod remote_trigger;
mod repl;
mod schedule_wakeup;
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
    extract_tool_use_error_message, failed_tool_text_summary, looks_like_internal_error,
    render_tool_use_error_content, summarize_internal_error,
};

#[cfg(test)]
use standard::{cap_write_diff_lines, content_summary};

pub(super) const TOOL_MAX_RENDER_LINES: usize = 10;
pub(super) const TOOL_BODY_MAX_LINES: usize = TOOL_MAX_RENDER_LINES - 1;

/// Spinner frames as `&'static str` for use in `status_icon` return type.
const SPINNER_STRS: &[&str] = &[
    "\u{280B}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283C}", "\u{2834}", "\u{2826}", "\u{2827}",
    "\u{2807}", "\u{280F}",
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ToolCallRenderContext<'a> {
    pub current_mode_id: Option<&'a str>,
}

pub fn status_icon(status: model::ToolCallStatus, spinner_frame: usize) -> (&'static str, Color) {
    match status {
        model::ToolCallStatus::Pending => ("\u{25CB}", theme::RUST_ORANGE),
        model::ToolCallStatus::InProgress => {
            let s = SPINNER_STRS[spinner_frame % SPINNER_STRS.len()];
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

/// Render a tool call with caching. Only re-renders when cache is stale.
///
/// The title is rendered live and the body is cached independently. Tool content
/// is capped by policy, while pending permission/question controls remain visible.
pub fn render_tool_call_cached(
    tc: &mut ToolCallInfo,
    render_context: ToolCallRenderContext<'_>,
    width: u16,
    spinner_frame: usize,
    out: &mut Vec<Line<'static>>,
) {
    let title = standard::render_tool_call_title(tc, render_context, width, spinner_frame);
    out.push(title);

    if !standard::tool_call_has_body(tc) {
        return;
    }

    let body_depends_on_width = standard::tool_call_body_depends_on_width(tc);

    // Expanded body: use cache if valid, otherwise render and cache.
    let cached_body =
        if body_depends_on_width { tc.cache.get_for_width(width) } else { tc.cache.get() };
    if let Some(cached_body) = cached_body {
        crate::perf::mark_with("tc::cache_hit_body", "lines", cached_body.len());
        out.extend_from_slice(cached_body);
    } else {
        crate::perf::mark("tc::cache_miss_body");
        let _t = crate::perf::start("tc::render_body");
        let body = standard::render_tool_call_body(tc, width);
        if body_depends_on_width {
            tc.cache.store_for_width(body, width);
        } else {
            tc.cache.store(body);
        }
        let stored =
            if body_depends_on_width { tc.cache.get_for_width(width) } else { tc.cache.get() };
        if let Some(stored) = stored {
            out.extend_from_slice(stored);
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

    if matches!(tc.sdk_tool_name.as_str(), "Agent" | "Task") {
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
        if let Some(models) = agent_model_route_badge(tc) {
            badges.push(Span::styled(
                format!("  [models: {}]", models.join(" -> ")),
                Style::default().fg(Color::LightBlue).add_modifier(Modifier::BOLD),
            ));
        } else if let Some(model) = agent_display_model_badge(tc) {
            badges.push(Span::styled(
                format!("  [model: {model}]"),
                Style::default().fg(Color::LightBlue).add_modifier(Modifier::BOLD),
            ));
        }
    }

    if tc.task_is_backgrounded() {
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

fn agent_model_route_badge(tc: &ToolCallInfo) -> Option<&[String]> {
    tc.output_metadata
        .as_ref()
        .and_then(|metadata| metadata.agent.as_ref())
        .and_then(|agent| agent.models_used.as_deref())
        .filter(|models| models.len() >= 2)
}

fn agent_display_model_badge(tc: &ToolCallInfo) -> Option<String> {
    tc.output_metadata
        .as_ref()
        .and_then(|metadata| metadata.agent.as_ref())
        .and_then(|agent| agent.resolved_model.as_deref())
        .map(str::trim)
        .filter(|resolved_model| !resolved_model.is_empty())
        .or_else(|| tool_raw_input_string(tc, "model"))
        .map(str::to_owned)
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
    use crate::app::{BlockCache, ToolCallInfo};
    use ratatui::text::Line;

    pub(super) fn tool_call(
        id: &str,
        title: &str,
        sdk_tool_name: &str,
        raw_input: serde_json::Value,
        content: Option<&str>,
        status: model::ToolCallStatus,
    ) -> ToolCallInfo {
        let mut tc = ToolCallInfo {
            id: id.to_owned(),
            source_message_uuids: Vec::new(),
            title: title.to_owned(),
            sdk_tool_name: sdk_tool_name.to_owned(),
            raw_input: Some(raw_input),
            raw_input_bytes: 0,
            locations: Vec::new(),
            output_metadata: None,
            task_metadata: None,
            status,
            content: Vec::new(),
            hidden: false,
            terminal_id: None,
            terminal_command: None,
            terminal_output: None,
            terminal_output_len: 0,
            cache: BlockCache::default(),
            pending_permission: None,
            pending_question: None,
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
}

#[cfg(test)]
mod tests;
