// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

mod document;
mod table;

use crate::failure;
use crate::ui::theme;
use pulldown_cmark::{Event, Options, Parser, Tag};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// App styles for Markdown, shared by the document layout and `tui-markdown`.
#[derive(Clone, Copy)]
struct MarkdownStyle;

impl tui_markdown::StyleSheet for MarkdownStyle {
    fn heading(&self, level: u8) -> Style {
        let style = Style::new().add_modifier(Modifier::BOLD);
        if level == 1 { style.add_modifier(Modifier::UNDERLINED) } else { style }
    }

    fn heading_marker(&self, _level: u8) -> &'static str {
        ""
    }
}

impl MarkdownStyle {
    fn code_label() -> Style {
        Style::new().fg(theme::DIM).add_modifier(Modifier::ITALIC)
    }
}

pub(super) fn render_markdown_safe(text: &str, bg: Option<Color>) -> Vec<Line<'static>> {
    render_markdown_at_width(text, u16::MAX, bg, false)
}

/// Lays out `text` from one parse. `preserve_newlines` renders soft line breaks as hard breaks.
pub(super) fn render_markdown_at_width(
    text: &str,
    width: u16,
    bg: Option<Color>,
    preserve_newlines: bool,
) -> Vec<Line<'static>> {
    render_markdown_safe_with(text, bg, |text, bg| {
        document::render(text, width, bg, preserve_newlines)
    })
}

fn parser_options() -> Options {
    // Match the dialect used by tui-markdown for blocks it renders.
    Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_HEADING_ATTRIBUTES
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
        | Options::ENABLE_SUPERSCRIPT
        | Options::ENABLE_SUBSCRIPT
        | Options::ENABLE_MATH
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_DEFINITION_LIST
        | Options::ENABLE_GFM
}

fn list_ranges(source: &str) -> impl Iterator<Item = std::ops::Range<usize>> + '_ {
    Parser::new_ext(source, parser_options())
        .into_offset_iter()
        .filter_map(|(event, range)| matches!(event, Event::Start(Tag::List(_))).then_some(range))
}

/// A boundary inside one parsed list cannot be frozen or rendered independently.
pub(super) fn list_crosses_boundary(source: &str, boundary: usize) -> bool {
    list_ranges(source).any(|range| range.start < boundary && range.end > boundary)
}

fn render_markdown_safe_with<F>(text: &str, bg: Option<Color>, renderer: F) -> Vec<Line<'static>>
where
    F: FnOnce(&str, Option<Color>) -> Vec<Line<'static>>,
{
    if let Ok(lines) = failure::catch_expected_panic(|| renderer(text, bg)) {
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
    let rendered =
        tui_markdown::from_str_with_options(text, &tui_markdown::Options::new(MarkdownStyle));
    rendered
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
        .collect()
}

fn plain_text_fallback(text: &str, bg: Option<Color>) -> Vec<Line<'static>> {
    let style =
        if let Some(bg_color) = bg { Style::default().bg(bg_color) } else { Style::default() };

    text.split('\n').map(|line| Line::from(Span::styled(line.to_owned(), style))).collect()
}

#[cfg(test)]
mod tests {
    fn line_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|span| span.content.as_ref()).collect()
    }
    use super::*;
    use std::panic::catch_unwind;

    fn rendered_text(lines: &[Line<'_>]) -> Vec<String> {
        lines.iter().map(line_text).collect()
    }

    fn document_rows(source: &str, width: u16) -> Vec<String> {
        let lines = render_markdown_at_width(source, width, None, false);
        crate::ui::wrap::wrap_lines_to_physical_rows(&lines, width)
            .iter()
            .map(|line| line_text(line).trim_end().to_owned())
            .collect()
    }

    #[test]
    fn list_continuation_paragraphs_and_nested_items_retain_their_parent() {
        let rows = document_rows(
            "3. First paragraph.\n\n   Second paragraph.\n\n   - Child item.\n\n     Child continuation.\n\n   Parent again.\n4. Next item.",
            60,
        );
        for (text, column) in [
            ("Second paragraph.", 5),
            ("- Child item.", 5),
            ("Child continuation.", 7),
            ("Parent again.", 5),
        ] {
            let row = rows.iter().find(|row| row.trim() == text).expect("list content survives");
            assert_eq!(row.find(text), Some(column), "wrong parent alignment: {rows:?}");
        }
        let first = rows.iter().position(|row| row.contains("First paragraph.")).unwrap();
        let second = rows.iter().position(|row| row.contains("Second paragraph.")).unwrap();
        assert!(
            rows[first + 1..second].iter().any(String::is_empty),
            "separate item paragraphs must stay separate: {rows:?}"
        );
        assert!(rows.iter().any(|row| row.trim() == "4. Next item."));
    }

    #[test]
    fn ordered_and_task_item_wrapping_use_the_actual_body_column() {
        for (source, prefix, column) in [
            ("9) alpha beta gamma delta epsilon\n1) zeta eta theta iota kappa", "9.", 5),
            ("10. alpha beta gamma delta epsilon", "10.", 6),
            ("- [x] alpha beta gamma delta epsilon", "- [x]", 8),
        ] {
            let rows = document_rows(source, 22);
            let first =
                rows.iter().position(|row| row.trim_start().starts_with(prefix)).expect("marker");
            let continuation = &rows[first + 1];
            assert!(!continuation.trim().is_empty());
            assert_eq!(continuation.len() - continuation.trim_start().len(), column, "{rows:?}");
            assert!(rows.iter().all(|row| crate::ui::wrap::display_width(row) <= 22));
            if source.starts_with("9)") {
                assert!(
                    rows.iter().any(|row| row.trim_start().starts_with("10. zeta")),
                    "number sequentially from the declared start: {rows:?}"
                );
            }
        }
    }

    #[test]
    fn code_and_escaped_markers_do_not_acquire_list_indentation() {
        for source in [
            "~~~text\n- literal marker\n~~~",
            "    - indented code",
            "\\- escaped marker",
            "3a ordinary text",
        ] {
            let rows = document_rows(source, 60);
            let content = rows
                .iter()
                .find(|row| {
                    row.contains("marker") || row.contains("ordinary") || row.contains("indented")
                })
                .expect("literal content");
            assert!(
                !content.starts_with("  "),
                "non-list source acquired list padding: {source:?}: {rows:?}"
            );
        }
    }

    #[test]
    fn nested_list_wrapping_preserves_inline_styles_links_and_unicode() {
        let lines = render_markdown_at_width(
            "1. Parent\n   - **Bold** and *italic* [docs](https://example.com) 日本語😀 words words words",
            28,
            Some(Color::Blue),
            false,
        );
        let text = lines.iter().map(line_text).collect::<Vec<_>>();
        let content = text.join(" ");
        for required in ["Bold", "italic", "https://example.com", "日本語😀"] {
            assert!(content.contains(required), "lost inline content: {text:?}");
        }
        for (word, modifier) in
            [("Bold", ratatui::style::Modifier::BOLD), ("italic", ratatui::style::Modifier::ITALIC)]
        {
            assert!(
                lines.iter().flat_map(|line| &line.spans).any(|span| span.content.contains(word)
                    && span.style.add_modifier.contains(modifier)),
                "lost {word} formatting"
            );
        }
        assert!(lines.iter().all(|line| crate::ui::wrap::line_display_width(line) <= 28));
        assert!(
            lines
                .iter()
                .flat_map(|line| &line.spans)
                .all(|span| span.style.bg == Some(Color::Blue))
        );
        let child = text.iter().position(|row| row.contains("- Bold")).expect("nested item");
        for row in &text[child + 1..] {
            assert_eq!(
                row.find(|ch: char| !ch.is_whitespace()),
                Some(7),
                "child continuation lost its parent: {text:?}"
            );
        }
    }

    #[test]
    fn table_and_code_inside_a_list_keep_the_parent_column() {
        let rows = document_rows(
            "1. Parent\n\n   | Name | Value |\n   | --- | --- |\n   | Alpha | Beta |\n\n   ~~~text\n   - literal code\n       indented code\n   ~~~\n\n   Tail paragraph.",
            60,
        );
        for word in ["Name", "Alpha", "- literal code", "Tail paragraph."] {
            let row = rows.iter().find(|row| row.contains(word)).expect("nested content survives");
            assert!(row.starts_with("     "), "content escaped its parent: {rows:?}");
        }
        let code = rows.iter().find(|row| row.contains("- literal code")).unwrap();
        assert_eq!(code.find('-'), Some(5), "code marker must stay literal");
        let indented = rows.iter().find(|row| row.contains("indented code")).unwrap();
        assert_eq!(
            indented.find("indented code"),
            Some(9),
            "code indentation must survive inside its parent list"
        );
    }

    #[test]
    fn quoted_lists_keep_one_quote_prefix_on_wrapped_and_continued_rows() {
        let rows = document_rows(
            "> Introduction\n> continued\n>\n> 1. Parent item with enough words to wrap onto another row\n>\n>    Continuation paragraph\n>\n>    - Child item\n>\n> End quote",
            30,
        );
        for row in rows.iter().filter(|row| !row.trim().is_empty()) {
            assert!(row.starts_with("> "), "quote ownership lost: {rows:?}");
            assert_eq!(row.matches('>').count(), 1, "quote prefixes duplicated: {rows:?}");
        }
        assert!(rows.iter().any(|row| row.contains("Child item")));
        assert!(rows.iter().any(|row| row.contains("End quote")));
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
    fn unordered_lists_preserve_surrounding_paragraph_separators() {
        let lines = render_markdown_safe("Intro\n\n- One\n- Two\n\nOutro", None);

        assert_eq!(rendered_text(&lines), vec!["Intro", "", "  - One", "  - Two", "", "Outro"]);
    }

    #[test]
    fn ordered_lists_preserve_surrounding_paragraph_separators() {
        let lines = render_markdown_safe("Intro\n\n1. One\n2. Two\n\nOutro", None);

        assert_eq!(rendered_text(&lines), vec!["Intro", "", "  1. One", "  2. Two", "", "Outro"]);
    }

    #[test]
    fn nested_lists_keep_relative_indentation() {
        let lines = render_markdown_safe("- Parent\n  - Child", None);

        assert_eq!(rendered_text(&lines), vec!["  - Parent", "    - Child"]);
    }

    #[test]
    fn task_lists_keep_markers_after_indentation() {
        let lines = render_markdown_safe("- [ ] Todo\n- [x] Done", None);

        assert_eq!(rendered_text(&lines), vec!["  - [ ] Todo", "  - [x] Done"]);
    }

    #[test]
    fn fenced_code_keeps_list_like_lines_unchanged() {
        let lines = render_markdown_safe("```md\n- Not a rendered list\n```", None);

        assert_eq!(rendered_text(&lines), vec!["md", "- Not a rendered list"]);
    }

    #[test]
    fn headings_render_bold_without_markers() {
        let lines = render_markdown_safe("# Title\n\nBody\n\n## Section", None);
        let text = rendered_text(&lines);

        for heading in ["Title", "Section"] {
            let line = lines.iter().find(|line| line_text(line) == heading).expect("heading row");
            assert!(
                line.spans.iter().all(|span| line
                    .style
                    .patch(span.style)
                    .add_modifier
                    .contains(Modifier::BOLD)),
                "{heading} lost heading style: {text:?}"
            );
        }
        assert!(!text.iter().any(|line| line.contains('#')), "{text:?}");
    }

    #[test]
    fn fenced_code_renders_hash_lines_verbatim_under_a_language_label() {
        let lines = render_markdown_safe(
            "Intro\n\n```bash\n#!/usr/bin/env bash\n# install deps\n  npm ci\n```\n\nAfter",
            None,
        );

        assert_eq!(
            rendered_text(&lines),
            vec![
                "Intro",
                "",
                "bash",
                "#!/usr/bin/env bash",
                "# install deps",
                "  npm ci",
                "",
                "After"
            ]
        );
        let label = lines[2].spans.iter().find(|span| span.content == "bash").expect("label span");
        assert_eq!(label.style, MarkdownStyle::code_label());
    }

    #[test]
    fn unlabelled_and_indented_code_use_a_generic_label() {
        for source in ["```\nplain\n```", "    plain"] {
            let lines = render_markdown_safe(source, None);
            assert_eq!(rendered_text(&lines), vec!["code", "plain"], "{source:?}");
        }
    }

    #[test]
    fn quoted_code_wraps_inside_the_quote_column() {
        let code = "let value = compute_something_long(alpha, beta);";
        let rows = document_rows(&format!("> ```rust\n> {code}\n> ```"), 24);

        assert_eq!(rows[0], "> rust");
        let body = &rows[1..];
        assert!(body.len() > 1, "code must wrap at the available width: {rows:?}");
        for row in body {
            assert!(row.starts_with("> "), "wrapped code escaped its quote: {rows:?}");
            assert!(crate::ui::wrap::display_width(row) <= 24);
        }
        let joined = body.iter().map(|row| &row[2..]).collect::<String>();
        assert_eq!(joined.replace(' ', ""), code.replace(' ', ""));
    }

    #[test]
    fn preserved_newlines_become_line_breaks_outside_code() {
        let lines = render_markdown_at_width(
            "first line\nsecond line\n\n- item one\n  continued\n\n```sh\necho a\necho b\n```",
            80,
            None,
            true,
        );
        let text = rendered_text(&lines);

        for row in
            ["first line", "second line", "  - item one", "    continued", "echo a", "echo b"]
        {
            assert!(text.iter().any(|line| line == row), "missing {row:?}: {text:?}");
        }
        assert!(text.iter().all(|line| !line.ends_with(' ')), "trailing spaces: {text:?}");
    }

    #[test]
    fn list_indent_preserves_requested_background() {
        let lines = render_markdown_safe("- One", Some(Color::Blue));

        assert_eq!(rendered_text(&lines), vec!["  - One"]);
        assert_eq!(lines[0].spans[0].style.bg, Some(Color::Blue));
        assert!(lines[0].spans.iter().all(|span| span.style.bg == Some(Color::Blue)));
    }
}
