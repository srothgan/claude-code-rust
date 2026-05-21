// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

//! Tool-call rendering: entry points, caching, and shared helpers.
//!
//! Submodules handle specific rendering concerns:
//! - [`standard`] -- non-Execute tool calls (Read, Write, Glob, etc.)
//! - [`execute`] -- Execute/Bash content rendering
//! - [`interactions`] -- inline permissions, questions, and plan approvals
//! - [`errors`] -- error rendering and tool-use error extraction

mod errors;
mod execute;
mod interactions;
mod standard;

use std::borrow::Cow;

use crate::agent::model;
use crate::app::ToolCallInfo;
use crate::ui::markdown;
use crate::ui::theme;
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

    if tc.assistant_auto_backgrounded() {
        badges.push(Span::styled(
            "  [assistant backgrounded]",
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ));
    }

    if tc.task_is_backgrounded() {
        badges.push(Span::styled(
            "  [backgrounded]",
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ));
    }

    if tc.verification_nudge_needed() {
        badges.push(Span::styled(
            "  [verification needed]",
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ));
    }

    badges
}

fn tool_display_title<'a>(
    tc: &'a ToolCallInfo,
    render_context: ToolCallRenderContext<'_>,
) -> Cow<'a, str> {
    if render_context.current_mode_id == Some("plan") {
        match tc.sdk_tool_name.as_str() {
            "Write" => return Cow::Borrowed("Create Plan"),
            "Edit" | "MultiEdit" => return Cow::Borrowed("Update Plan"),
            _ => {}
        }
    }

    Cow::Borrowed(&tc.title)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::BlockCache;
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use std::fmt::Write as _;

    fn test_tool_call(
        id: &str,
        sdk_tool_name: &str,
        status: model::ToolCallStatus,
    ) -> ToolCallInfo {
        ToolCallInfo {
            id: id.to_owned(),
            title: id.to_owned(),
            sdk_tool_name: sdk_tool_name.to_owned(),
            raw_input: None,
            raw_input_bytes: 0,
            output_metadata: None,
            task_metadata: None,
            status,
            content: Vec::new(),
            hidden: false,
            terminal_id: None,
            terminal_command: None,
            terminal_output: None,
            terminal_output_len: 0,
            terminal_bytes_seen: 0,
            terminal_snapshot_mode: crate::app::TerminalSnapshotMode::AppendOnly,
            cache: BlockCache::default(),
            pending_permission: None,
            pending_question: None,
        }
    }

    fn todo_write_tool_call(raw_input: serde_json::Value) -> ToolCallInfo {
        let mut tc = test_tool_call("tc-todo", "TodoWrite", model::ToolCallStatus::Completed);
        tc.raw_input = Some(raw_input);
        tc
    }

    fn rendered_line_texts(lines: &[Line<'static>]) -> Vec<String> {
        lines
            .iter()
            .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
            .collect()
    }

    fn rendered_line_texts_trimmed(lines: &[Line<'static>]) -> Vec<String> {
        rendered_line_texts(lines).into_iter().map(|line| line.trim_end().to_owned()).collect()
    }

    // status_icon

    #[test]
    fn status_icon_pending() {
        let (icon, color) = status_icon(model::ToolCallStatus::Pending, 0);
        assert!(!icon.is_empty());
        assert_eq!(color, theme::RUST_ORANGE);
    }

    #[test]
    fn status_icon_in_progress() {
        let (icon, color) = status_icon(model::ToolCallStatus::InProgress, 3);
        assert!(!icon.is_empty());
        assert_eq!(color, theme::RUST_ORANGE);
    }

    #[test]
    fn status_icon_completed() {
        let (icon, color) = status_icon(model::ToolCallStatus::Completed, 0);
        assert_eq!(icon, theme::ICON_COMPLETED);
        assert_eq!(color, theme::RUST_ORANGE);
    }

    #[test]
    fn status_icon_failed() {
        let (icon, color) = status_icon(model::ToolCallStatus::Failed, 0);
        assert_eq!(icon, theme::ICON_FAILED);
        assert_eq!(color, theme::STATUS_ERROR);
    }

    #[test]
    fn status_icon_killed() {
        let (icon, color) = status_icon(model::ToolCallStatus::Killed, 0);
        assert_eq!(icon, theme::ICON_FAILED);
        assert_eq!(color, theme::STATUS_ERROR);
    }

    #[test]
    fn status_icon_spinner_wraps() {
        let (icon_a, _) = status_icon(model::ToolCallStatus::InProgress, 0);
        let (icon_b, _) = status_icon(model::ToolCallStatus::InProgress, SPINNER_STRS.len());
        assert_eq!(icon_a, icon_b);
    }

    #[test]
    fn status_icon_all_spinner_frames_valid() {
        for i in 0..SPINNER_STRS.len() {
            let (icon, _) = status_icon(model::ToolCallStatus::InProgress, i);
            assert!(!icon.is_empty());
        }
    }

    /// Spinner frames are all distinct.
    #[test]
    fn status_icon_spinner_frames_distinct() {
        let frames: Vec<&str> = (0..SPINNER_STRS.len())
            .map(|i| status_icon(model::ToolCallStatus::InProgress, i).0)
            .collect();
        for i in 0..frames.len() {
            for j in (i + 1)..frames.len() {
                assert_ne!(frames[i], frames[j], "frames {i} and {j} are identical");
            }
        }
    }

    /// Large spinner frame number wraps correctly.
    #[test]
    fn status_icon_spinner_large_frame() {
        let (icon, _) = status_icon(model::ToolCallStatus::Pending, 999_999);
        assert!(!icon.is_empty());
    }

    #[test]
    fn truncate_spans_adds_ellipsis_when_needed() {
        let spans = vec![Span::raw("abcdefghijklmnopqrstuvwxyz")];
        let out = truncate_spans_to_width(spans, 8);
        let rendered: String = out.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(rendered, "abcdefg\u{2026}");
        assert!(spans_width(&out) <= 8);
    }

    #[test]
    fn markdown_inline_spans_removes_markdown_syntax() {
        let spans = markdown_inline_spans("**Allow** _once_");
        let rendered: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(rendered.contains("Allow"));
        assert!(rendered.contains("once"));
        assert!(!rendered.contains('*'));
        assert!(!rendered.contains('_'));
    }

    #[test]
    fn render_tool_call_title_shows_backgrounded_badge() {
        let mut tc = test_tool_call("tc-bg", "Agent", model::ToolCallStatus::InProgress);
        tc.task_metadata = Some(model::TaskMetadata::new().backgrounded(Some(true)));

        let line = standard::render_tool_call_title(&tc, ToolCallRenderContext::default(), 80, 0);
        let rendered: String = line.spans.iter().map(|s| s.content.as_ref()).collect();

        assert!(rendered.contains("[backgrounded]"));
    }

    #[test]
    fn tool_display_title_uses_plan_aliases() {
        let write = test_tool_call("tc-plan-write", "Write", model::ToolCallStatus::Completed);
        let edit = test_tool_call("tc-plan-edit", "Edit", model::ToolCallStatus::Completed);
        let read = test_tool_call("tc-plan-read", "Read", model::ToolCallStatus::Completed);
        let plan = ToolCallRenderContext { current_mode_id: Some("plan") };

        assert_eq!(tool_display_title(&write, plan), "Create Plan");
        assert_eq!(tool_display_title(&edit, plan), "Update Plan");
        assert_eq!(tool_display_title(&read, plan), "tc-plan-read");
    }

    #[test]
    fn standard_title_uses_plan_alias_for_write() {
        let tc = test_tool_call("Write notes/plan.md", "Write", model::ToolCallStatus::Completed);

        let rendered = standard::render_tool_call_title(
            &tc,
            ToolCallRenderContext { current_mode_id: Some("plan") },
            80,
            0,
        );
        let text: String = rendered.spans.iter().map(|span| span.content.as_ref()).collect();

        assert!(text.contains("Create Plan"));
        assert!(!text.contains("Write notes/plan.md"));
    }

    #[test]
    fn bash_title_does_not_wrap_for_long_title() {
        let tc = ToolCallInfo {
            id: "tc-1".into(),
            title: "echo very long command title with markdown **bold** and path /a/b/c/d/e/f"
                .into(),
            sdk_tool_name: "Bash".into(),
            raw_input: None,
            raw_input_bytes: 0,
            output_metadata: None,
            task_metadata: None,
            status: model::ToolCallStatus::Pending,
            content: Vec::new(),
            hidden: false,
            terminal_id: None,
            terminal_command: None,
            terminal_output: None,
            terminal_output_len: 0,
            terminal_bytes_seen: 0,
            terminal_snapshot_mode: crate::app::TerminalSnapshotMode::AppendOnly,
            cache: BlockCache::default(),
            pending_permission: None,
            pending_question: None,
        };

        let top = standard::render_tool_call_title(&tc, ToolCallRenderContext::default(), 40, 0);
        assert!(spans_width(&top.spans) <= 40);
    }

    #[test]
    fn bash_body_uses_plain_indent_without_box_borders() {
        let mut tc = test_tool_call("tc-bash-indent", "Bash", model::ToolCallStatus::Completed);
        tc.terminal_id = Some("term-indent".to_owned());
        tc.terminal_command = Some("echo hi".to_owned());
        tc.terminal_output = Some("hi".to_owned());

        let mut rendered = Vec::new();
        render_tool_call_cached(&mut tc, ToolCallRenderContext::default(), 80, 0, &mut rendered);
        let rendered_text: Vec<String> = rendered
            .iter()
            .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
            .collect();

        assert_eq!(rendered_text.len(), 3);
        assert!(rendered_text[1].starts_with("      $ echo hi"));
        assert!(rendered_text[2].starts_with("      hi"));
        assert!(
            rendered_text
                .iter()
                .all(|line| !line.contains('\u{256D}') && !line.contains('\u{2570}'))
        );
        assert!(rendered_text.iter().all(|line| !line.starts_with("  \u{2502}")));
    }

    #[test]
    fn read_body_caps_long_wrapped_line_by_physical_rows() {
        let mut tc = test_tool_call("tc-read-wrap", "Read", model::ToolCallStatus::Completed);
        tc.title = "output.txt".to_owned();
        let long_line = (0..80).map(|idx| format!("word{idx}")).collect::<Vec<_>>().join(" ");
        tc.content = vec![model::ToolCallContent::from(long_line)];

        let body = standard::render_tool_call_body(&tc, 24);
        let rendered = rendered_line_texts_trimmed(&body);

        assert_eq!(body.len(), TOOL_BODY_MAX_LINES);
        assert!(rendered.iter().any(|line| line.contains("wrapped")));
    }

    #[test]
    fn body_cap_reports_hidden_source_lines_when_full_lines_are_omitted() {
        let mut tc =
            test_tool_call("tc-source-line-count", "CustomTool", model::ToolCallStatus::Completed);
        tc.title = "output.txt".to_owned();
        let text = (0..20).map(|idx| format!("line {idx}")).collect::<Vec<_>>().join("\n");
        tc.content = vec![model::ToolCallContent::from(text)];

        let body = standard::render_tool_call_body(&tc, 80);
        let rendered = rendered_line_texts_trimmed(&body);

        assert_eq!(body.len(), TOOL_BODY_MAX_LINES);
        assert!(rendered[0].contains("12 source lines hidden"));
    }

    #[test]
    fn wrapped_content_cap_keeps_permission_rows_visible() {
        let mut tc = test_tool_call(
            "tc-permission-after-cap",
            "CustomTool",
            model::ToolCallStatus::InProgress,
        );
        tc.title = "output.txt".to_owned();
        let long_line = (0..80).map(|idx| format!("word{idx}")).collect::<Vec<_>>().join(" ");
        tc.content = vec![model::ToolCallContent::from(long_line)];

        let (response_tx, _response_rx) = tokio::sync::oneshot::channel();
        tc.pending_permission = Some(crate::app::InlinePermission {
            options: vec![
                model::PermissionOption::new(
                    "allow",
                    "Allow",
                    model::PermissionOptionKind::AllowOnce,
                ),
                model::PermissionOption::new(
                    "deny",
                    "Deny",
                    model::PermissionOptionKind::RejectOnce,
                ),
            ],
            display: None,
            response_tx,
            selected_index: 0,
            focused: true,
        });

        let body = standard::render_tool_call_body(&tc, 24);
        let rendered = rendered_line_texts_trimmed(&body);

        assert!(rendered.iter().any(|line| line.contains("wrapped")));
        assert!(rendered.iter().any(|line| line.contains("Allow")));
    }

    #[test]
    fn cached_tool_body_rerenders_after_width_change() {
        let mut tc = test_tool_call("tc-cache-width", "Bash", model::ToolCallStatus::Completed);
        tc.terminal_id = Some("term-cache-width".to_owned());
        tc.terminal_command = Some("echo wrapped".to_owned());
        tc.terminal_output =
            Some("alpha beta gamma delta epsilon zeta eta theta iota kappa lambda".to_owned());

        let mut wide = Vec::new();
        render_tool_call_cached(&mut tc, ToolCallRenderContext::default(), 100, 0, &mut wide);

        let mut narrow = Vec::new();
        render_tool_call_cached(&mut tc, ToolCallRenderContext::default(), 24, 0, &mut narrow);

        assert!(
            narrow.len() > wide.len(),
            "narrow render should rebuild cached body at the new width"
        );
    }

    #[test]
    fn bash_title_renders_assistant_backgrounded_badge() {
        let mut tc = test_tool_call("tc-bash-bg", "Bash", model::ToolCallStatus::Completed);
        tc.output_metadata =
            Some(model::ToolOutputMetadata::new().bash(Some(
                model::BashOutputMetadata::new().assistant_auto_backgrounded(Some(true)),
            )));

        let rendered =
            standard::render_tool_call_title(&tc, ToolCallRenderContext::default(), 100, 0);
        let text: String = rendered.spans.iter().map(|span| span.content.as_ref()).collect();
        assert!(text.contains("[assistant backgrounded]"));
    }

    #[test]
    fn bash_title_preserves_command_title_in_plan_mode() {
        let mut tc = test_tool_call("echo hi", "Bash", model::ToolCallStatus::Completed);
        tc.terminal_command = Some("echo hi".to_owned());

        let rendered = standard::render_tool_call_title(
            &tc,
            ToolCallRenderContext { current_mode_id: Some("plan") },
            80,
            0,
        );
        let text: String = rendered.spans.iter().map(|span| span.content.as_ref()).collect();

        assert!(text.contains("Bash"));
        assert!(text.contains("echo hi"));
        assert!(!text.contains("Create Plan"));
        assert!(!text.contains("Update Plan"));
    }

    #[test]
    fn todo_write_title_renders_verification_badge() {
        let mut tc = test_tool_call("tc-todo", "TodoWrite", model::ToolCallStatus::Completed);
        tc.output_metadata = Some(model::ToolOutputMetadata::new().todo_write(Some(
            model::TodoWriteOutputMetadata::new().verification_nudge_needed(Some(true)),
        )));

        let rendered =
            standard::render_tool_call_title(&tc, ToolCallRenderContext::default(), 80, 0);
        let text: String = rendered.spans.iter().map(|span| span.content.as_ref()).collect();
        assert!(text.contains("[verification needed]"));
    }

    #[test]
    fn todo_write_body_renders_todos_from_raw_input_and_ignores_reported_content() {
        let mut tc = todo_write_tool_call(json!({
            "todos": [
                {"content": "Raw task", "status": "pending", "activeForm": "Raw active"}
            ]
        }));
        tc.content = vec![model::ToolCallContent::from("reported output should not render")];

        let body = standard::render_tool_call_body(&tc, 80);
        let rendered = rendered_line_texts(&body);

        assert!(rendered.iter().any(|line| line.contains("Raw task")));
        assert!(!rendered.iter().any(|line| line.contains("reported output")));
    }

    #[test]
    fn todo_write_without_todos_array_does_not_render_reported_content() {
        let mut tc = todo_write_tool_call(json!({}));
        tc.content = vec![model::ToolCallContent::from("reported output should not render")];

        assert!(!standard::tool_call_has_body(&tc));
        assert!(standard::render_tool_call_body(&tc, 80).is_empty());
    }

    #[test]
    fn todo_write_body_uses_block_markers_and_active_form() {
        let tc = todo_write_tool_call(json!({
            "todos": [
                {"content": "Done task", "status": "completed", "activeForm": "Done active"},
                {"content": "Doing task", "status": "in_progress", "activeForm": "Doing active"},
                {"content": "Pending task", "status": "pending", "activeForm": "Pending active"}
            ]
        }));

        let body = standard::render_tool_call_body(&tc, 80);
        let rendered = rendered_line_texts(&body);

        assert!(rendered.iter().any(|line| line.contains("\u{25a0} Done task")));
        assert!(rendered.iter().any(|line| line.contains("\u{25a3} Doing active")));
        assert!(rendered.iter().any(|line| line.contains("\u{25a1} Pending task")));
        assert!(!rendered.iter().any(|line| line.contains("Doing task")));

        let done_line = body
            .iter()
            .find(|line| line.spans.iter().any(|span| span.content.as_ref() == "\u{25a0}"))
            .expect("done marker line");
        let done_marker = done_line
            .spans
            .iter()
            .find(|span| span.content.as_ref() == "\u{25a0}")
            .expect("done marker span");
        assert_eq!(done_marker.style.fg, Some(theme::DIM));

        let current_line = body
            .iter()
            .find(|line| line.spans.iter().any(|span| span.content.as_ref() == "\u{25a3}"))
            .expect("current marker line");
        let current_text = current_line
            .spans
            .iter()
            .find(|span| span.content.as_ref() == "Doing active")
            .expect("current text span");
        assert_eq!(current_text.style.fg, Some(theme::RUST_ORANGE));
    }

    #[test]
    fn todo_write_overflow_anchors_first_unfinished_todo() {
        let tc = todo_write_tool_call(json!({
            "todos": (0..12)
                .map(|idx| {
                    let status = if idx == 0 { "pending" } else { "completed" };
                    json!({"content": format!("Task {idx}"), "status": status})
                })
                .collect::<Vec<_>>()
        }));

        let rendered = rendered_line_texts(&standard::render_tool_call_body(&tc, 80));

        assert_eq!(rendered.len(), TOOL_BODY_MAX_LINES);
        assert!(rendered.first().is_some_and(|line| line.contains("Task 0")));
        assert!(rendered.last().is_some_and(|line| line.contains("...")));
        assert!(!rendered.iter().any(|line| line.contains("hidden")));
    }

    #[test]
    fn todo_write_overflow_centers_middle_unfinished_todo() {
        let tc = todo_write_tool_call(json!({
            "todos": (0..12)
                .map(|idx| {
                    let status = if idx == 6 { "pending" } else { "completed" };
                    json!({"content": format!("Task {idx}"), "status": status})
                })
                .collect::<Vec<_>>()
        }));

        let rendered = rendered_line_texts(&standard::render_tool_call_body(&tc, 80));

        assert_eq!(rendered.len(), TOOL_BODY_MAX_LINES);
        assert!(rendered.first().is_some_and(|line| line.contains("...")));
        assert!(rendered.last().is_some_and(|line| line.contains("...")));
        assert!(rendered.iter().any(|line| line.contains("Task 6")));
        assert!(!rendered.iter().any(|line| line.contains("Task 2")));
        assert!(!rendered.iter().any(|line| line.contains("Task 10")));
        assert!(!rendered.iter().any(|line| line.contains("hidden")));
    }

    #[test]
    fn todo_write_overflow_anchors_last_unfinished_todo() {
        let tc = todo_write_tool_call(json!({
            "todos": (0..12)
                .map(|idx| {
                    let status = if idx == 11 { "pending" } else { "completed" };
                    json!({"content": format!("Task {idx}"), "status": status})
                })
                .collect::<Vec<_>>()
        }));

        let rendered = rendered_line_texts(&standard::render_tool_call_body(&tc, 80));

        assert_eq!(rendered.len(), TOOL_BODY_MAX_LINES);
        assert!(rendered.first().is_some_and(|line| line.contains("...")));
        assert!(rendered.last().is_some_and(|line| line.contains("Task 11")));
        assert!(!rendered.iter().any(|line| line.contains("hidden")));
    }

    #[test]
    fn mcp_resource_body_renders_saved_path_hint_when_text_omits_it() {
        let mut tc =
            test_tool_call("tc-mcp-resource", "ReadMcpResource", model::ToolCallStatus::Completed);
        tc.content = vec![model::ToolCallContent::McpResource(
            model::McpResource::new("file://manual.pdf")
                .mime_type(Some("application/pdf".to_owned()))
                .text(Some("Binary resource downloaded successfully.".to_owned()))
                .blob_saved_to(Some("C:\\tmp\\manual.pdf".to_owned())),
        )];

        let body = standard::render_tool_call_body(&tc, 80);
        let rendered: Vec<String> = body
            .iter()
            .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
            .collect();

        assert!(
            rendered.iter().any(|line| line.contains("Binary resource downloaded successfully."))
        );
        assert!(rendered.iter().any(|line| line.contains("Saved to: C:\\tmp\\manual.pdf")));
    }

    #[test]
    fn mcp_resource_body_avoids_duplicate_saved_path_hint_when_text_already_mentions_it() {
        let mut tc = test_tool_call(
            "tc-mcp-resource-dupe",
            "ReadMcpResource",
            model::ToolCallStatus::Completed,
        );
        tc.content = vec![model::ToolCallContent::McpResource(
            model::McpResource::new("file://manual.pdf")
                .mime_type(Some("application/pdf".to_owned()))
                .text(Some(
                    "[Resource from docs at file://manual.pdf] Saved to C:\\tmp\\manual.pdf"
                        .to_owned(),
                ))
                .blob_saved_to(Some("C:\\tmp\\manual.pdf".to_owned())),
        )];

        let body = standard::render_tool_call_body(&tc, 80);
        let rendered: Vec<String> = body
            .iter()
            .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
            .collect();

        assert_eq!(
            rendered.iter().filter(|line| line.contains("Saved to: C:\\tmp\\manual.pdf")).count(),
            0
        );
    }

    #[test]
    fn read_tool_renders_head_hidden_marker_and_tail() {
        let mut tc = test_tool_call("tc-read-body", "Read", model::ToolCallStatus::Completed);
        tc.content = vec![model::ToolCallContent::from(
            (0..12).map(|idx| format!("line {idx}")).collect::<Vec<_>>().join("\n"),
        )];

        let body = standard::render_tool_call_body(&tc, 80);
        let rendered_text: Vec<String> = body
            .iter()
            .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
            .collect();

        assert_eq!(rendered_text.len(), 8);
        for visible in ["line 0", "line 1", "line 2", "line 8", "line 9", "line 10", "line 11"] {
            assert!(rendered_text.iter().any(|line| line.contains(visible)));
        }
        assert!(rendered_text.iter().any(|line| line.contains("5 lines hidden")));
        for hidden in ["line 3", "line 4", "line 5", "line 6", "line 7"] {
            assert!(!rendered_text.iter().any(|line| line.contains(hidden)));
        }
    }

    #[test]
    fn compact_tools_render_only_summary_line() {
        for sdk_tool_name in ["Agent", "Task", "WebSearch", "WebFetch", "ExitPlanMode"] {
            let mut tc = test_tool_call(
                &format!("tc-{sdk_tool_name}"),
                sdk_tool_name,
                model::ToolCallStatus::Completed,
            );
            tc.content = vec![model::ToolCallContent::from("first line\nsecond line".to_owned())];

            let body = standard::render_tool_call_body(&tc, 80);
            let rendered: Vec<String> = body
                .iter()
                .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
                .collect();

            assert_eq!(rendered.len(), 1);
            assert!(rendered[0].contains("first line"));
            assert!(!rendered[0].contains("second line"));
        }
    }

    #[test]
    fn diff_tool_renders_without_expand_hint() {
        let mut tc = test_tool_call("tc-diff", "Write", model::ToolCallStatus::Completed);
        tc.content = vec![model::ToolCallContent::Diff(
            model::Diff::new("src/main.rs", "new".to_owned()).old_text(Some("old".to_owned())),
        )];

        let mut rendered = Vec::new();
        render_tool_call_cached(&mut tc, ToolCallRenderContext::default(), 80, 0, &mut rendered);
        let text: Vec<String> = rendered
            .iter()
            .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
            .collect();

        assert!(!text.iter().any(|line| line.contains("expand")));
        assert!(text.iter().any(|line| line.contains("(+1, -1)")));
        assert!(text.iter().any(|line| line.contains("+  new")));
        assert!(text.len() > 2);
    }

    #[test]
    fn diff_tool_body_adds_nested_indent_inside_tool_prefix() {
        let mut tc = test_tool_call("tc-diff-indent", "Edit", model::ToolCallStatus::Completed);
        tc.content = vec![model::ToolCallContent::Diff(
            model::Diff::new("src/main.rs", "new".to_owned())
                .old_text(Some("old".to_owned()))
                .repository(Some("acme/project".to_owned())),
        )];

        let body = standard::render_tool_call_body(&tc, 80);
        let rendered: Vec<String> = body
            .iter()
            .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
            .collect();

        assert!(rendered.iter().any(|line| line.starts_with("  │    [acme/project]")));
        assert!(rendered.iter().any(|line| line.starts_with("  │    (+1, -1)")));
        assert!(rendered.iter().any(|line| {
            (line.starts_with("  │   ") || line.starts_with("  └─   ")) && line.contains("+  new")
        }));
    }

    #[test]
    fn diff_tool_body_preserves_source_code_indentation() {
        let mut tc =
            test_tool_call("tc-diff-code-indent", "Edit", model::ToolCallStatus::Completed);
        tc.content = vec![model::ToolCallContent::Diff(model::Diff::new(
            "src/main.rs",
            "fn main() {\n    if true {\n        return;\n    }\n}\n".to_owned(),
        ))];

        let body = standard::render_tool_call_body(&tc, 80);
        let rendered: Vec<String> = body
            .iter()
            .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
            .collect();

        assert!(rendered.iter().any(|line| line.contains("+      if true {")));
        assert!(rendered.iter().any(|line| line.contains("+          return;")));
    }

    #[test]
    fn diff_tool_body_preserves_nested_indent_for_wrapped_continuations() {
        let mut tc = test_tool_call("tc-diff-wrap", "Edit", model::ToolCallStatus::Completed);
        tc.content = vec![model::ToolCallContent::Diff(model::Diff::new(
            "src/main.rs",
            "        This is a long added line that should wrap onto another visual line.\n"
                .to_owned(),
        ))];

        let body = standard::render_tool_call_body(&tc, 28);
        let rendered: Vec<String> = body
            .iter()
            .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
            .collect();

        assert!(rendered.iter().any(|line| line.contains("diff lines")));
        assert!(
            rendered.iter().any(|line| line.starts_with("  │                  "))
                || rendered.iter().any(|line| line.starts_with("  └─                  "))
        );
        assert!(rendered.iter().any(|line| line.contains("another")));
        assert!(rendered.iter().any(|line| line.contains("line.")));
    }

    #[test]
    fn write_diff_cap_keeps_omission_marker_nested_indented() {
        let new_text = (0..120).fold(String::new(), |mut text, idx| {
            let _ = writeln!(&mut text, "line {idx}");
            text
        });
        let mut tc = test_tool_call("tc-diff-cap", "Write", model::ToolCallStatus::Completed);
        tc.content = vec![model::ToolCallContent::Diff(model::Diff::new("src/main.rs", new_text))];

        let body = standard::render_tool_call_body(&tc, 80);
        let rendered: Vec<String> = body
            .iter()
            .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
            .collect();

        assert!(
            rendered.iter().any(|line| line.starts_with("  │    (+120)"))
                || rendered.iter().any(|line| line.starts_with("  └─   (+120)"))
        );
        assert!(
            rendered
                .iter()
                .any(|line| line.starts_with("  │    ... ") && line.contains("diff lines omitted"))
                || rendered
                    .iter()
                    .any(|line| line.starts_with("  └─    ... ")
                        && line.contains("diff lines omitted"))
        );
    }

    #[test]
    fn plan_files_render_markdown_instead_of_diff() {
        let mut tc = test_tool_call(
            "Write .claude/plans/launch.md",
            "Write",
            model::ToolCallStatus::Completed,
        );
        tc.content = vec![model::ToolCallContent::Diff(
            model::Diff::new(
                ".claude/plans/launch.md",
                "# Launch Plan\n\n- Ship aliases\n- Render plan markdown\n".to_owned(),
            )
            .old_text(Some("# Old Plan\n".to_owned())),
        )];

        let body = standard::render_tool_call_body(&tc, 80);
        let rendered: Vec<String> = body
            .iter()
            .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
            .collect();

        assert!(rendered.iter().any(|line| line.contains("Launch Plan")));
        assert!(rendered.iter().any(|line| line.contains("Render plan markdown")));
        assert!(!rendered.iter().any(|line| line.contains("@@")));
        assert!(!rendered.iter().any(|line| line.starts_with("+ ")));
    }

    #[test]
    fn plan_file_markdown_body_is_not_capped_by_tool_height() {
        let mut tc = test_tool_call(
            "Write .claude/plans/long.md",
            "Write",
            model::ToolCallStatus::Completed,
        );
        let plan_text = (0..24).map(|idx| format!("- Step {idx}")).collect::<Vec<_>>().join("\n");
        tc.content = vec![model::ToolCallContent::Diff(model::Diff::new(
            ".claude/plans/long.md",
            plan_text,
        ))];

        let body = standard::render_tool_call_body(&tc, 80);
        let rendered = rendered_line_texts_trimmed(&body);

        assert!(body.len() > TOOL_BODY_MAX_LINES);
        assert!(rendered.iter().any(|line| line.contains("Step 23")));
        assert!(!rendered.iter().any(|line| line.contains("hidden")));
        assert!(!rendered.iter().any(|line| line.contains("omitted")));
    }

    #[test]
    fn non_plan_write_diff_body_stays_capped_by_tool_height() {
        let mut tc =
            test_tool_call("Write notes/long.md", "Write", model::ToolCallStatus::Completed);
        let new_text = (0..80).map(|idx| format!("line {idx}")).collect::<Vec<_>>().join("\n");
        tc.content =
            vec![model::ToolCallContent::Diff(model::Diff::new("notes/long.md", new_text))];

        let body = standard::render_tool_call_body(&tc, 80);
        let rendered = rendered_line_texts_trimmed(&body);

        assert_eq!(body.len(), TOOL_BODY_MAX_LINES);
        assert!(rendered.iter().any(|line| line.contains("diff lines omitted")));
    }

    #[test]
    fn internal_error_detection_accepts_xml_payload() {
        let payload =
            "<error><code>-32603</code><message>Adapter process crashed</message></error>";
        assert!(looks_like_internal_error(payload));
    }

    #[test]
    fn internal_error_detection_rejects_plain_bash_failure() {
        let payload = "bash: unknown_command: command not found";
        assert!(!looks_like_internal_error(payload));
    }

    #[test]
    fn summarize_internal_error_prefers_xml_message() {
        let payload =
            "<error><code>-32603</code><message>Adapter process crashed</message></error>";
        assert_eq!(summarize_internal_error(payload), "Adapter process crashed");
    }

    #[test]
    fn summarize_internal_error_reads_json_rpc_message() {
        let payload = r#"{"jsonrpc":"2.0","error":{"code":-32603,"message":"internal rpc fault"}}"#;
        assert_eq!(summarize_internal_error(payload), "internal rpc fault");
    }

    #[test]
    fn extract_tool_use_error_message_reads_inner_text() {
        let payload = "<tool_use_error>Sibling tool call errored</tool_use_error>";
        assert_eq!(
            extract_tool_use_error_message(payload).as_deref(),
            Some("Sibling tool call errored")
        );
    }

    #[test]
    fn failed_tool_text_summary_reads_common_xml_error_wrappers() {
        let failed = model::ToolCallStatus::Failed;
        assert_eq!(
            failed_tool_text_summary(
                failed,
                "<tool_use_error>Sibling tool call errored</tool_use_error>"
            )
            .as_deref(),
            Some("Sibling tool call errored")
        );
        assert_eq!(
            failed_tool_text_summary(
                failed,
                "<error><code>-32603</code><message>Adapter process crashed</message></error>"
            )
            .as_deref(),
            Some("Adapter process crashed")
        );
        assert_eq!(
            failed_tool_text_summary(failed, "<fault>Remote call failed</fault>").as_deref(),
            Some("Remote call failed")
        );
        assert_eq!(
            failed_tool_text_summary(failed, "<custom_error>Wrapped failure</custom_error>")
                .as_deref(),
            Some("Wrapped failure")
        );
        assert_eq!(
            failed_tool_text_summary(
                model::ToolCallStatus::Completed,
                "<message>Successful XML output</message>",
            ),
            None
        );
        assert_eq!(failed_tool_text_summary(failed, "<message>missing close"), None);
    }

    #[test]
    fn render_tool_use_error_content_shows_only_inner_text_lines() {
        let lines = render_tool_use_error_content("Line A\nLine B");
        let rendered: Vec<String> = lines
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert_eq!(rendered.len(), 2);
        assert!(rendered.iter().any(|line| line == "Line A"));
        assert!(rendered.iter().any(|line| line == "Line B"));
    }

    #[test]
    fn content_summary_only_extracts_tool_use_error_for_failed_execute() {
        let tc = ToolCallInfo {
            id: "tc-1".into(),
            title: "Bash".into(),
            sdk_tool_name: "Bash".into(),
            raw_input: None,
            raw_input_bytes: 0,
            output_metadata: None,
            task_metadata: None,
            status: model::ToolCallStatus::Completed,
            content: Vec::new(),
            hidden: false,
            terminal_id: Some("term-1".into()),
            terminal_command: Some("echo done".into()),
            terminal_output: Some("<tool_use_error>bad</tool_use_error>\ndone".into()),
            terminal_output_len: 0,
            terminal_bytes_seen: 0,
            terminal_snapshot_mode: crate::app::TerminalSnapshotMode::AppendOnly,
            cache: BlockCache::default(),
            pending_permission: None,
            pending_question: None,
        };
        assert_eq!(content_summary(&tc), "done");
    }

    #[test]
    fn content_summary_extracts_tool_use_error_for_failed_execute() {
        let tc = ToolCallInfo {
            id: "tc-1".into(),
            title: "Bash".into(),
            sdk_tool_name: "Bash".into(),
            raw_input: None,
            raw_input_bytes: 0,
            output_metadata: None,
            task_metadata: None,
            status: model::ToolCallStatus::Failed,
            content: Vec::new(),
            hidden: false,
            terminal_id: Some("term-1".into()),
            terminal_command: Some("echo done".into()),
            terminal_output: Some("<tool_use_error>bad</tool_use_error>\ndone".into()),
            terminal_output_len: 0,
            terminal_bytes_seen: 0,
            terminal_snapshot_mode: crate::app::TerminalSnapshotMode::AppendOnly,
            cache: BlockCache::default(),
            pending_permission: None,
            pending_question: None,
        };
        assert_eq!(content_summary(&tc), "bad");
    }

    #[test]
    fn content_summary_extracts_xml_message_for_failed_text_tool() {
        let mut tc = test_tool_call("tc-web", "WebFetch", model::ToolCallStatus::Failed);
        tc.content = vec![model::ToolCallContent::from(
            "<error><message>Fetch failed</message></error>".to_owned(),
        )];

        assert_eq!(content_summary(&tc), "Fetch failed");
    }

    #[test]
    fn content_summary_uses_first_terminal_line_for_failed_execute() {
        let tc = ToolCallInfo {
            id: "tc-2".into(),
            title: "Bash".into(),
            sdk_tool_name: "Bash".into(),
            raw_input: None,
            raw_input_bytes: 0,
            output_metadata: None,
            task_metadata: None,
            status: model::ToolCallStatus::Failed,
            content: Vec::new(),
            hidden: false,
            terminal_id: Some("term-2".into()),
            terminal_command: Some("cd path with spaces".into()),
            terminal_output: Some(
                "Exit code 1\n/usr/bin/bash: line 1: cd: too many arguments\nmore detail".into(),
            ),
            terminal_output_len: 0,
            terminal_bytes_seen: 0,
            terminal_snapshot_mode: crate::app::TerminalSnapshotMode::AppendOnly,
            cache: BlockCache::default(),
            pending_permission: None,
            pending_question: None,
        };
        assert_eq!(content_summary(&tc), "Exit code 1");
    }

    #[test]
    fn content_summary_uses_higher_limit_for_in_progress_agent() {
        let mut tc = test_tool_call("tc-agent", "Agent", model::ToolCallStatus::InProgress);
        let long_line = "a".repeat(150);
        tc.content = vec![model::ToolCallContent::from(long_line.clone())];

        assert_eq!(content_summary(&tc), long_line);
    }

    #[test]
    fn content_summary_keeps_normal_limit_for_completed_agent() {
        let mut tc = test_tool_call("tc-agent-done", "Agent", model::ToolCallStatus::Completed);
        let long_line = "a".repeat(150);
        tc.content = vec![model::ToolCallContent::from(long_line)];

        let summary = content_summary(&tc);
        assert_eq!(summary.chars().count(), 60);
        assert!(summary.ends_with("..."));
    }

    #[test]
    fn render_execute_content_keeps_tail_output() {
        let tc = ToolCallInfo {
            id: "tc-3".into(),
            title: "Bash".into(),
            sdk_tool_name: "Bash".into(),
            raw_input: None,
            raw_input_bytes: 0,
            output_metadata: None,
            task_metadata: None,
            status: model::ToolCallStatus::Failed,
            content: Vec::new(),
            hidden: false,
            terminal_id: Some("term-3".into()),
            terminal_command: Some("cd path with spaces".into()),
            terminal_output: Some(
                (0..30).map(|idx| format!("line {idx}")).collect::<Vec<_>>().join("\n"),
            ),
            terminal_output_len: 0,
            terminal_bytes_seen: 0,
            terminal_snapshot_mode: crate::app::TerminalSnapshotMode::AppendOnly,
            cache: BlockCache::default(),
            pending_permission: None,
            pending_question: None,
        };

        let lines = execute::render_execute_content(&tc);
        let rendered: Vec<String> = lines
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert_eq!(rendered.len(), super::TOOL_BODY_MAX_LINES);
        assert!(rendered[0].contains("$ cd path with spaces"));
        assert!(rendered[1].contains("lines hidden"));
        assert!(!rendered.iter().any(|line| line == "line 0"));
        assert!(rendered.iter().any(|line| line == "line 23"));
        assert_eq!(rendered.last().map(String::as_str), Some("line 29"));
    }

    #[test]
    fn render_execute_content_extracts_tool_use_error() {
        let mut tc = test_tool_call("tc-xml", "Bash", model::ToolCallStatus::Failed);
        tc.terminal_id = Some("term-xml".into());
        tc.terminal_command = Some("cd path with spaces".into());
        tc.terminal_output = Some(
            "<tool_use_error>Cancelled: parallel tool call Bash(cd path) errored</tool_use_error>\nraw fallback"
                .into(),
        );

        let lines = execute::render_execute_content(&tc);
        let rendered: Vec<String> = lines
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();

        assert!(rendered[0].contains("$ cd path with spaces"));
        assert!(
            rendered
                .iter()
                .any(|line| line == "Cancelled: parallel tool call Bash(cd path) errored")
        );
        assert!(!rendered.iter().any(|line| line.contains("<tool_use_error>")));
        assert!(!rendered.iter().any(|line| line.contains("</tool_use_error>")));
        assert!(!rendered.iter().any(|line| line.contains("raw fallback")));
    }

    #[test]
    fn render_execute_content_extracts_xml_message_error() {
        let mut tc = test_tool_call("tc-xml-message", "Bash", model::ToolCallStatus::Failed);
        tc.terminal_id = Some("term-xml".into());
        tc.terminal_output =
            Some("<error><message>Adapter process crashed</message></error>".into());

        let lines = execute::render_execute_content(&tc);
        let rendered: Vec<String> = lines
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();

        assert!(rendered.iter().any(|line| line == "Adapter process crashed"));
        assert!(!rendered.iter().any(|line| line.contains("<message>")));
        assert!(!rendered.iter().any(|line| line.contains("<error>")));
    }

    #[test]
    fn failed_text_tool_body_extracts_xml_error_message() {
        let mut tc = test_tool_call("tc-read-error", "Read", model::ToolCallStatus::Failed);
        tc.content = vec![model::ToolCallContent::from(
            "<error><message>Read failed</message></error>".to_owned(),
        )];

        let body = standard::render_tool_call_body(&tc, 80);
        let rendered: Vec<String> = body
            .iter()
            .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
            .collect();

        assert!(rendered.iter().any(|line| line.contains("Read failed")));
        assert!(!rendered.iter().any(|line| line.contains("<message>")));
        assert!(!rendered.iter().any(|line| line.contains("<error>")));
    }

    #[test]
    fn successful_text_tool_body_preserves_xml_like_output() {
        let mut tc = test_tool_call("tc-read-xml", "Read", model::ToolCallStatus::Completed);
        tc.content =
            vec![model::ToolCallContent::from("<message>not an error</message>".to_owned())];

        let body = standard::render_tool_call_body(&tc, 80);
        let rendered: Vec<String> = body
            .iter()
            .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
            .collect();

        assert!(rendered.iter().any(|line| line.contains("<message>not an error</message>")));
    }

    #[test]
    fn diff_tool_body_preserves_xml_like_source() {
        let mut tc = test_tool_call("tc-diff-xml", "Write", model::ToolCallStatus::Failed);
        tc.content = vec![model::ToolCallContent::Diff(model::Diff::new(
            "src/main.xml",
            "<message>source text</message>".to_owned(),
        ))];

        let body = standard::render_tool_call_body(&tc, 80);
        let rendered: Vec<String> = body
            .iter()
            .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
            .collect();

        assert!(rendered.iter().any(|line| line.contains("<message>source text</message>")));
    }

    #[test]
    fn write_diff_cap_keeps_tail_with_omission_marker() {
        use standard::WRITE_DIFF_MAX_LINES;

        let lines: Vec<Line<'static>> =
            (0..120).map(|idx| Line::from(format!("line {idx}"))).collect();
        let capped = cap_write_diff_lines(lines);
        let rendered: Vec<String> = capped
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();

        assert_eq!(rendered.len(), WRITE_DIFF_MAX_LINES);
        assert!(rendered[0].contains("diff lines omitted"));
        assert!(!rendered.iter().any(|line| line == "line 0"));
        assert!(rendered.iter().any(|line| line == "line 112"));
        assert_eq!(rendered.last().map(String::as_str), Some("line 119"));
    }

    #[test]
    fn write_diff_cap_preserves_diff_count_header() {
        use standard::WRITE_DIFF_MAX_LINES;

        let mut lines: Vec<Line<'static>> = vec![Line::from("(+120)")];
        lines.extend((0..120).map(|idx| Line::from(format!("line {idx}"))));

        let capped = cap_write_diff_lines(lines);
        let rendered: Vec<String> = capped
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();

        assert_eq!(rendered.len(), WRITE_DIFF_MAX_LINES);
        assert_eq!(rendered[0], "(+120)");
        assert!(rendered[1].contains("diff lines omitted"));
        assert!(!rendered.iter().any(|line| line == "line 0"));
        assert!(rendered.iter().any(|line| line == "line 113"));
        assert_eq!(rendered.last().map(String::as_str), Some("line 119"));
    }
}
