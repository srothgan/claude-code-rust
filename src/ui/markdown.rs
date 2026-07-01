// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use std::panic::{self, AssertUnwindSafe};

pub(super) fn render_markdown_safe(text: &str, bg: Option<Color>) -> Vec<Line<'static>> {
    render_markdown_safe_with(text, bg, render_with_tui_markdown)
}

fn render_markdown_safe_with<F>(text: &str, bg: Option<Color>, renderer: F) -> Vec<Line<'static>>
where
    F: FnOnce(&str, Option<Color>) -> Vec<Line<'static>>,
{
    if let Ok(lines) = panic::catch_unwind(AssertUnwindSafe(|| renderer(text, bg))) {
        lines
    } else {
        tracing::warn!(
            target: crate::logging::targets::APP_RENDER,
            event_name = "markdown_render_failed",
            message = "markdown renderer panicked; falling back to plain text",
            outcome = "fallback",
        );
        plain_text_fallback(text, bg)
    }
}

fn render_with_tui_markdown(text: &str, bg: Option<Color>) -> Vec<Line<'static>> {
    let rendered = tui_markdown::from_str(text);
    let lines = rendered
        .lines
        .into_iter()
        .map(|line| {
            let owned_spans: Vec<Span<'static>> = line
                .spans
                .into_iter()
                .map(|span| {
                    let style =
                        if let Some(bg_color) = bg { span.style.bg(bg_color) } else { span.style };
                    Span::styled(span.content.into_owned(), style)
                })
                .collect();
            let line_style =
                if let Some(bg_color) = bg { line.style.bg(bg_color) } else { line.style };
            Line::from(owned_spans).style(line_style)
        })
        .collect();
    normalize_list_spacing(lines)
}

fn normalize_list_spacing(lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
    let list_lines = rendered_list_line_flags(&lines);
    let mut normalized = Vec::with_capacity(lines.len());

    for (idx, line) in lines.into_iter().enumerate() {
        if line_is_blank(&line) {
            let before_list = list_lines.get(idx + 1).copied().unwrap_or(false);
            let after_list = idx > 0 && list_lines.get(idx - 1).copied().unwrap_or(false);
            if before_list || after_list {
                continue;
            }
        }

        if list_lines[idx] {
            normalized.push(indent_line(line, "  "));
        } else {
            normalized.push(line);
        }
    }

    normalized
}

fn rendered_list_line_flags(lines: &[Line<'_>]) -> Vec<bool> {
    let mut in_fenced_code = false;
    let mut flags = Vec::with_capacity(lines.len());

    for line in lines {
        let text = line_text(line);
        let starts_fence = text.trim_start().starts_with("```");
        let in_code_line = in_fenced_code || starts_fence;
        flags.push(!in_code_line && rendered_line_is_list_item(&text));
        if starts_fence {
            in_fenced_code = !in_fenced_code;
        }
    }

    flags
}

fn rendered_line_is_list_item(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with("- ") || starts_with_ordered_list_marker(trimmed)
}

fn starts_with_ordered_list_marker(text: &str) -> bool {
    let digit_count = text.bytes().take_while(u8::is_ascii_digit).count();
    digit_count > 0 && text[digit_count..].starts_with(". ")
}

fn indent_line(mut line: Line<'static>, indent: &'static str) -> Line<'static> {
    line.spans.insert(0, Span::styled(indent, line.style));
    line
}

fn line_is_blank(line: &Line<'_>) -> bool {
    line.spans.iter().all(|span| span.content.trim().is_empty())
}

fn line_text(line: &Line<'_>) -> String {
    line.spans.iter().map(|span| span.content.as_ref()).collect()
}

fn plain_text_fallback(text: &str, bg: Option<Color>) -> Vec<Line<'static>> {
    let style =
        if let Some(bg_color) = bg { Style::default().bg(bg_color) } else { Style::default() };

    text.split('\n').map(|line| Line::from(Span::styled(line.to_owned(), style))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::catch_unwind;

    fn rendered_text(lines: &[Line<'_>]) -> Vec<String> {
        lines.iter().map(line_text).collect()
    }

    #[test]
    fn render_markdown_safe_handles_common_and_edge_case_inputs_without_panicking() {
        let inputs = [
            "- [ ] one\n- [x] two",
            "- [ ] Move todos below input top line",
            "- [ ]\n- [x]\n- [ ]",
            "- [x] done\n  - [ ] child",
            "1. [ ] numbered checklist marker",
            "[]()[]()[]()",
            "```md\n- [ ] fenced checklist\n```",
            "> - [ ] blockquote checklist\n>\n> text",
            "# Heading\n- [ ] item\n\n| a | b |\n|---|---|\n| x | y |",
            "- [ ] [link](https://example.com) [",
            "- [ ] \u{200d}\u{200d}\u{200d}",
        ];

        for input in inputs {
            let result = catch_unwind(|| render_markdown_safe(input, None));
            assert!(result.is_ok(), "input triggered panic: {input}");
            assert!(!result.unwrap().is_empty(), "input rendered zero lines: {input}");
        }
    }

    #[test]
    fn render_markdown_safe_falls_back_to_plain_text_and_preserves_requested_bg() {
        let lines = render_markdown_safe_with("line1\nline2", Some(Color::Blue), |_text, _bg| {
            panic!("forced renderer panic for fallback path")
        });
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].spans[0].content.as_ref(), "line1");
        assert_eq!(lines[1].spans[0].content.as_ref(), "line2");
        assert_eq!(lines[0].spans[0].style.bg, Some(Color::Blue));
        assert_eq!(lines[1].spans[0].style.bg, Some(Color::Blue));
    }

    #[test]
    fn unordered_lists_use_indentation_instead_of_boundary_blank_lines() {
        let lines = render_markdown_safe("Intro\n\n- One\n- Two\n\nOutro", None);

        assert_eq!(rendered_text(&lines), vec!["Intro", "  - One", "  - Two", "Outro"]);
    }

    #[test]
    fn ordered_lists_use_indentation_instead_of_boundary_blank_lines() {
        let lines = render_markdown_safe("Intro\n\n1. One\n2. Two\n\nOutro", None);

        assert_eq!(rendered_text(&lines), vec!["Intro", "  1. One", "  2. Two", "Outro"]);
    }

    #[test]
    fn nested_lists_keep_relative_indentation() {
        let lines = render_markdown_safe("- Parent\n  - Child", None);

        assert_eq!(rendered_text(&lines), vec!["  - Parent", "      - Child"]);
    }

    #[test]
    fn task_lists_keep_markers_after_indentation() {
        let lines = render_markdown_safe("- [ ] Todo\n- [x] Done", None);

        assert_eq!(rendered_text(&lines), vec!["  - [ ] Todo", "  - [x] Done"]);
    }

    #[test]
    fn fenced_code_keeps_list_like_lines_unchanged() {
        let lines = render_markdown_safe("```md\n- Not a rendered list\n```", None);

        assert_eq!(rendered_text(&lines), vec!["```md", "- Not a rendered list", "```"]);
    }

    #[test]
    fn list_indent_preserves_requested_background() {
        let lines = render_markdown_safe("- One", Some(Color::Blue));

        assert_eq!(rendered_text(&lines), vec!["  - One"]);
        assert_eq!(lines[0].spans[0].style.bg, Some(Color::Blue));
        assert!(lines[0].spans.iter().all(|span| span.style.bg == Some(Color::Blue)));
    }
}
