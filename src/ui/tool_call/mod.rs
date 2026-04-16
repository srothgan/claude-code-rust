// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

//! Tool-call rendering: entry points, caching, and shared helpers.
//!
//! Submodules handle specific rendering concerns:
//! - [`standard`] -- non-Execute tool calls (Read, Write, Glob, etc.)
//! - [`execute`] -- Execute/Bash two-layer bordered rendering
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
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

// Re-export submodule items used by tests.
#[cfg(test)]
use errors::{
    extract_tool_use_error_message, looks_like_internal_error, render_tool_use_error_content,
    summarize_internal_error,
};

#[cfg(test)]
use standard::{cap_write_diff_lines, content_summary};

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
/// For Execute/Bash tool calls, the cache stores **content only** (command, output,
/// permissions) without border decoration. Borders are applied at render time using
/// the current width, so they always fill the terminal correctly after resize.
/// Height for Execute = `content_lines + 2` (title border + bottom border).
///
/// For other tool calls, the title is rendered live and the expanded body is cached
/// independently, so session collapse preference can change without invalidating
/// every completed tool-call cache.
pub fn render_tool_call_cached_with_tools_collapsed(
    tc: &mut ToolCallInfo,
    render_context: ToolCallRenderContext<'_>,
    width: u16,
    spinner_frame: usize,
    tools_collapsed: bool,
    out: &mut Vec<Line<'static>>,
) {
    let is_execute = tc.is_execute_tool();

    // Execute/Bash: two-layer rendering (cache content, apply borders at render time)
    if is_execute {
        if tc.cache.get().is_none() {
            crate::perf::mark("tc::cache_miss_execute");
            let _t = crate::perf::start("tc::render_exec");
            let content = execute::render_execute_content(tc);
            tc.cache.store(content);
        } else {
            crate::perf::mark("tc::cache_hit_execute");
        }
        if let Some(content) = tc.cache.get() {
            let bordered = execute::render_execute_with_borders(
                tc,
                render_context,
                content,
                width,
                spinner_frame,
            );
            out.extend(bordered);
        }
        return;
    }

    let title = standard::render_tool_call_title(tc, render_context, width, spinner_frame);
    out.push(title);

    let has_body = !(tc.content.is_empty()
        && tc.pending_permission.is_none()
        && tc.pending_question.is_none());
    if !has_body {
        return;
    }

    if standard::tool_call_effectively_collapsed(tc, tools_collapsed) {
        standard::render_collapsed_tool_call_summary(tc, out);
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

/// Ensure tool call caches are up-to-date and return visual wrapped height at `width`.
/// Returns `(height, lines_wrapped_for_measurement)`.
pub fn measure_tool_call_height_cached_with_tools_collapsed(
    tc: &mut ToolCallInfo,
    render_context: ToolCallRenderContext<'_>,
    width: u16,
    spinner_frame: usize,
    layout_generation: u64,
    tools_collapsed: bool,
) -> (usize, usize) {
    if tc.cache_measurement_key_matches(width, layout_generation) {
        crate::perf::mark("tc_measure_fast_path_hits");
        return (tc.last_measured_height, 0);
    }
    crate::perf::mark("tc_measure_recompute_count");

    let is_execute = tc.is_execute_tool();
    if is_execute {
        if tc.cache.get().is_none() {
            let content = execute::render_execute_content(tc);
            tc.cache.store(content);
        }
        if let Some(content) = tc.cache.get() {
            let bordered = execute::render_execute_with_borders(
                tc,
                render_context,
                content,
                width,
                spinner_frame,
            );
            let h = Paragraph::new(Text::from(bordered.clone()))
                .wrap(Wrap { trim: false })
                .line_count(width);
            tc.cache.set_height(h, width);
            tc.record_measured_height(width, h, layout_generation);
            return (h, bordered.len());
        }
        tc.record_measured_height(width, 0, layout_generation);
        return (0, 0);
    }

    let title = standard::render_tool_call_title(tc, render_context, width, spinner_frame);
    let title_h =
        Paragraph::new(Text::from(vec![title])).wrap(Wrap { trim: false }).line_count(width);
    let has_body = !(tc.content.is_empty()
        && tc.pending_permission.is_none()
        && tc.pending_question.is_none());

    if !has_body {
        tc.record_measured_height(width, title_h, layout_generation);
        return (title_h, 1);
    }

    if standard::tool_call_effectively_collapsed(tc, tools_collapsed) {
        let mut summary = Vec::new();
        standard::render_collapsed_tool_call_summary(tc, &mut summary);
        let summary_h = Paragraph::new(Text::from(summary.clone()))
            .wrap(Wrap { trim: false })
            .line_count(width);
        let total = title_h + summary_h;
        tc.record_measured_height(width, total, layout_generation);
        return (total, 1 + summary.len());
    }

    let body_depends_on_width = standard::tool_call_body_depends_on_width(tc);
    let cached_body =
        if body_depends_on_width { tc.cache.get_for_width(width) } else { tc.cache.get() };
    if cached_body.is_some() {
        if let Some(body_h) = tc.cache.height_at(width) {
            let total = title_h + body_h;
            tc.record_measured_height(width, total, layout_generation);
            return (total, 1);
        }
        if let Some(body_h) = tc.cache.measure_and_set_height(width) {
            let total = title_h + body_h;
            tc.record_measured_height(width, total, layout_generation);
            let cached_len = if body_depends_on_width {
                tc.cache.get_for_width(width).map_or(1, |body| body.len() + 1)
            } else {
                tc.cache.get().map_or(1, |body| body.len() + 1)
            };
            return (total, cached_len);
        }
    }

    let body = standard::render_tool_call_body(tc, width);
    let body_h =
        Paragraph::new(Text::from(body.clone())).wrap(Wrap { trim: false }).line_count(width);
    if body_depends_on_width {
        tc.cache.store_for_width(body, width);
    } else {
        tc.cache.store(body);
    }
    tc.cache.set_height(body_h, width);
    let total = title_h + body_h;
    tc.record_measured_height(width, total, layout_generation);
    let cached_len = if body_depends_on_width {
        tc.cache.get_for_width(width).map_or(1, |body| body.len() + 1)
    } else {
        tc.cache.get().map_or(1, |body| body.len() + 1)
    };
    (total, cached_len)
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
            render_epoch: 0,
            layout_epoch: 0,
            last_measured_width: 0,
            last_measured_height: 0,
            last_measured_layout_epoch: 0,
            last_measured_layout_generation: 0,
            cache: BlockCache::default(),
            pending_permission: None,
            pending_question: None,
        }
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
    fn execute_top_border_does_not_wrap_for_long_title() {
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
            render_epoch: 0,
            layout_epoch: 0,
            last_measured_width: 0,
            last_measured_height: 0,
            last_measured_layout_epoch: 0,
            last_measured_layout_generation: 0,
            cache: BlockCache::default(),
            pending_permission: None,
            pending_question: None,
        };

        let rendered =
            execute::render_execute_with_borders(&tc, ToolCallRenderContext::default(), &[], 80, 0);
        let top = rendered.first().expect("top border line");
        assert!(spans_width(&top.spans) <= 80);
    }

    #[test]
    fn execute_title_renders_assistant_backgrounded_badge() {
        let mut tc = test_tool_call("tc-bash-bg", "Bash", model::ToolCallStatus::Completed);
        tc.output_metadata =
            Some(model::ToolOutputMetadata::new().bash(Some(
                model::BashOutputMetadata::new().assistant_auto_backgrounded(Some(true)),
            )));

        let rendered = execute::render_execute_with_borders(
            &tc,
            ToolCallRenderContext::default(),
            &[],
            100,
            0,
        );
        let top = rendered.first().expect("top border line");
        let text: String = top.spans.iter().map(|span| span.content.as_ref()).collect();
        assert!(text.contains("[assistant backgrounded]"));
    }

    #[test]
    fn execute_title_preserves_bash_title_in_plan_mode() {
        let mut tc = test_tool_call("echo hi", "Bash", model::ToolCallStatus::Completed);
        tc.terminal_command = Some("echo hi".to_owned());

        let rendered = execute::render_execute_with_borders(
            &tc,
            ToolCallRenderContext { current_mode_id: Some("plan") },
            &[],
            80,
            0,
        );
        let top = rendered.first().expect("top border line");
        let text: String = top.spans.iter().map(|span| span.content.as_ref()).collect();

        assert!(text.contains("Bash"));
        assert!(text.contains("echo hi"));
    }

    #[test]
    fn execute_measure_fast_path_keeps_height_stable_across_repeated_measurement() {
        let mut tc = test_tool_call("tc-fast", "Bash", model::ToolCallStatus::InProgress);
        tc.terminal_command = Some("echo hi".to_owned());
        tc.terminal_output = Some("hello\nworld".to_owned());

        let (h1, lines1) = measure_tool_call_height_cached_with_tools_collapsed(
            &mut tc,
            ToolCallRenderContext::default(),
            80,
            0,
            1,
            false,
        );
        assert!(h1 > 0);
        assert!(lines1 > 0);

        let (h2, lines2) = measure_tool_call_height_cached_with_tools_collapsed(
            &mut tc,
            ToolCallRenderContext::default(),
            80,
            4,
            1,
            false,
        );
        assert_eq!(h2, h1);
        assert!(lines2 <= lines1);
    }

    #[test]
    fn execute_measure_recomputes_on_layout_generation_change() {
        let mut tc = test_tool_call("tc-layout-gen", "Bash", model::ToolCallStatus::InProgress);
        tc.terminal_command = Some("echo hi".to_owned());
        tc.terminal_output = Some("hello".to_owned());

        let (_, first_lines) = measure_tool_call_height_cached_with_tools_collapsed(
            &mut tc,
            ToolCallRenderContext::default(),
            80,
            0,
            1,
            false,
        );
        assert!(first_lines > 0);
        let (_, second_lines) = measure_tool_call_height_cached_with_tools_collapsed(
            &mut tc,
            ToolCallRenderContext::default(),
            80,
            0,
            2,
            false,
        );
        assert!(second_lines > 0);
    }

    #[test]
    fn layout_dirty_invalidates_measure_fast_path() {
        let mut tc = test_tool_call("tc-dirty", "Read", model::ToolCallStatus::Completed);
        tc.content = vec![model::ToolCallContent::from("one line")];

        let (first_height, first_lines) = measure_tool_call_height_cached_with_tools_collapsed(
            &mut tc,
            ToolCallRenderContext::default(),
            80,
            0,
            1,
            false,
        );
        assert!(first_lines > 0);
        let (cached_height, fast_lines) = measure_tool_call_height_cached_with_tools_collapsed(
            &mut tc,
            ToolCallRenderContext::default(),
            80,
            0,
            1,
            false,
        );
        assert_eq!(cached_height, first_height);
        assert!(fast_lines <= first_lines);

        tc.mark_tool_call_layout_dirty();
        let (recomputed_height, recompute_lines) =
            measure_tool_call_height_cached_with_tools_collapsed(
                &mut tc,
                ToolCallRenderContext::default(),
                80,
                0,
                1,
                false,
            );
        assert_eq!(recomputed_height, first_height);
        assert!(recompute_lines > 0);
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
    fn completed_non_execute_collapse_changes_visible_body_without_hiding_the_title() {
        let mut tc = test_tool_call("tc-collapse", "Read", model::ToolCallStatus::Completed);
        tc.content = vec![model::ToolCallContent::from("alpha\nbeta".to_owned())];

        let mut expanded = Vec::new();
        render_tool_call_cached_with_tools_collapsed(
            &mut tc,
            ToolCallRenderContext::default(),
            80,
            0,
            false,
            &mut expanded,
        );
        let expanded_text: Vec<String> = expanded
            .iter()
            .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
            .collect();
        assert!(expanded_text.iter().any(|line| line.contains("alpha")));
        assert!(expanded_text.first().is_some_and(|line| line.contains("tc-collapse")));

        let mut collapsed = Vec::new();
        render_tool_call_cached_with_tools_collapsed(
            &mut tc,
            ToolCallRenderContext::default(),
            80,
            0,
            true,
            &mut collapsed,
        );
        let collapsed_text: Vec<String> = collapsed
            .iter()
            .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
            .collect();
        assert_eq!(collapsed_text.first(), expanded_text.first());
        assert!(collapsed_text.iter().any(|line| line.contains("ctrl+o to expand")));
        assert!(!collapsed_text.iter().any(|line| line.contains("beta")));
        assert!(collapsed_text.len() < expanded_text.len());
    }

    #[test]
    fn completed_non_execute_measurement_changes_with_session_collapse_preference() {
        let mut tc =
            test_tool_call("tc-measure-collapse", "Read", model::ToolCallStatus::Completed);
        tc.content = vec![model::ToolCallContent::from("alpha\nbeta\ngamma\ndelta".to_owned())];

        let (expanded_h, _) = measure_tool_call_height_cached_with_tools_collapsed(
            &mut tc,
            ToolCallRenderContext::default(),
            24,
            0,
            1,
            false,
        );
        let (collapsed_h, _) = measure_tool_call_height_cached_with_tools_collapsed(
            &mut tc,
            ToolCallRenderContext::default(),
            24,
            0,
            2,
            true,
        );

        assert!(collapsed_h < expanded_h);
    }

    #[test]
    fn diff_tool_stays_expanded_when_session_prefers_collapsed() {
        let mut tc = test_tool_call("tc-diff", "Write", model::ToolCallStatus::Completed);
        tc.content = vec![model::ToolCallContent::Diff(
            model::Diff::new("src/main.rs", "new".to_owned()).old_text(Some("old".to_owned())),
        )];

        let mut rendered = Vec::new();
        render_tool_call_cached_with_tools_collapsed(
            &mut tc,
            ToolCallRenderContext::default(),
            80,
            0,
            true,
            &mut rendered,
        );
        let text: Vec<String> = rendered
            .iter()
            .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
            .collect();

        assert!(!text.iter().any(|line| line.contains("expand")));
        assert!(text.iter().any(|line| line.contains("lines ")));
        assert!(text.iter().any(|line| line.contains("+  new")));
        assert!(text.len() > 2);
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
            render_epoch: 0,
            layout_epoch: 0,
            last_measured_width: 0,
            last_measured_height: 0,
            last_measured_layout_epoch: 0,
            last_measured_layout_generation: 0,
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
            render_epoch: 0,
            layout_epoch: 0,
            last_measured_width: 0,
            last_measured_height: 0,
            last_measured_layout_epoch: 0,
            last_measured_layout_generation: 0,
            cache: BlockCache::default(),
            pending_permission: None,
            pending_question: None,
        };
        assert_eq!(content_summary(&tc), "bad");
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
            render_epoch: 0,
            layout_epoch: 0,
            last_measured_width: 0,
            last_measured_height: 0,
            last_measured_layout_epoch: 0,
            last_measured_layout_generation: 0,
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
    fn render_execute_content_failed_surfaces_summary_before_full_output() {
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
                "Exit code 1\n/usr/bin/bash: line 1: cd: too many arguments\nmore detail".into(),
            ),
            terminal_output_len: 0,
            terminal_bytes_seen: 0,
            terminal_snapshot_mode: crate::app::TerminalSnapshotMode::AppendOnly,
            render_epoch: 0,
            layout_epoch: 0,
            last_measured_width: 0,
            last_measured_height: 0,
            last_measured_layout_epoch: 0,
            last_measured_layout_generation: 0,
            cache: BlockCache::default(),
            pending_permission: None,
            pending_question: None,
        };

        let lines = execute::render_execute_content(&tc);
        let rendered: Vec<String> = lines
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(rendered.iter().any(|line| line.contains("Exit code 1")));
        assert!(!rendered.iter().any(|line| line.contains("more detail")));
    }

    #[test]
    fn write_diff_cap_keeps_head_and_tail_with_omission_marker() {
        use standard::WRITE_DIFF_HEAD_LINES;
        use standard::WRITE_DIFF_MAX_LINES;

        let lines: Vec<Line<'static>> =
            (0..120).map(|idx| Line::from(format!("line {idx}"))).collect();
        let capped = cap_write_diff_lines(lines);
        let rendered: Vec<String> = capped
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();

        assert_eq!(rendered.len(), WRITE_DIFF_MAX_LINES);
        assert_eq!(rendered[0], "line 0");
        assert_eq!(rendered[WRITE_DIFF_HEAD_LINES - 1], "line 9");
        assert!(rendered.iter().any(|line| line.contains("diff lines omitted")));
        assert!(rendered.iter().any(|line| line == "line 83"));
        assert_eq!(rendered.last().map(String::as_str), Some("line 119"));
    }
}
