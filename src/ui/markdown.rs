// SPDX-License-Identifier: Apache-2.0

mod document;

pub(super) use document::DocumentCursor;

use crate::failure;
use crate::ui::theme;
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
    render_markdown_with_control(
        text,
        width,
        bg,
        preserve_newlines,
        &super::layout_control::LayoutControl::uninterrupted(),
    )
}

pub(super) fn render_markdown_with_control(
    text: &str,
    width: u16,
    bg: Option<Color>,
    preserve_newlines: bool,
    control: &super::layout_control::LayoutControl<'_>,
) -> Vec<Line<'static>> {
    render_markdown_safe_with(text, bg, |text, bg| {
        document::render(text, width, bg, preserve_newlines, control)
    })
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

fn render_with_tui_markdown(
    text: &str,
    events: Vec<pulldown_cmark::Event<'_>>,
    width: u16,
    bg: Option<Color>,
) -> Vec<Line<'static>> {
    render_markdown_safe_with(text, bg, |_text, bg| {
        let rendered = tui_markdown::from_events_with_options(
            events,
            &tui_markdown::Options::new(MarkdownStyle).table_width(width),
        );
        rendered
            .lines
            .into_iter()
            .map(|line| {
                let owned_spans: Vec<Span<'static>> = line
                    .spans
                    .into_iter()
                    .map(|span| {
                        let style = if let Some(bg_color) = bg {
                            span.style.bg(bg_color)
                        } else {
                            span.style
                        };
                        Span::styled(span.content.into_owned(), style)
                    })
                    .collect();
                let line_style =
                    if let Some(bg_color) = bg { line.style.bg(bg_color) } else { line.style };
                Line::from(owned_spans).style(line_style)
            })
            .collect()
    })
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
    fn code_and_escaped_markers_do_not_acquire_list_indentation() {
        for (source, expected) in [
            ("~~~text\n  - literal marker\n~~~", "  - literal marker"),
            ("    - indented code", "- indented code"),
            ("\\- escaped marker", "- escaped marker"),
            ("3a ordinary text", "3a ordinary text"),
        ] {
            assert!(document_rows(source, 60).iter().any(|r| r == expected), "{source}");
        }
        let rows = document_rows("- Real item", 60);
        let item = rows.iter().find(|r| r.contains("Real item")).unwrap();
        assert_ne!(item, "Real item");
        assert!(!item.contains("\\-"));
    }

    #[test]
    fn nested_list_wrapping_preserves_inline_styles_links_and_unicode() {
        for bg in [None, Some(Color::Blue)] {
            let lines = render_markdown_at_width(
                "1. Parent\n   - **Bold** and *italic* [docs](https://example.test/a) \u{65e5}\u{672c}\u{8a9e}\u{1f600} one two three four",
                28,
                bg,
                false,
            );
            let physical = crate::ui::wrap::wrap_lines_to_physical_rows(&lines, 28);
            let rows = rendered_text(&physical);
            let parent = rows.iter().position(|r| r.contains("Parent")).unwrap();
            let child = rows.iter().position(|r| r.contains("Bold")).unwrap();
            assert!(child > parent);
            assert!(rows[child].find("Bold").unwrap() > rows[parent].find("1.").unwrap());
            let joined = rows[child..].join("");
            let start = joined.find("Bold").unwrap();
            assert_eq!(
                joined[start..].chars().filter(|c| !c.is_whitespace()).collect::<String>(),
                "Boldanditalicdocs(https://example.test/a)\u{65e5}\u{672c}\u{8a9e}\u{1f600}onetwothreefour"
            );
            assert!(physical.len() > child + 1);
            for (word, modifier) in [("Bold", Modifier::BOLD), ("italic", Modifier::ITALIC)] {
                let row = physical.iter().find(|l| line_text(l).contains(word)).unwrap();
                let chars = styled_chars(row);
                let text = chars.iter().map(|(c, _)| *c).collect::<String>();
                let offset = text[..text.find(word).unwrap()].chars().count();
                assert!(
                    chars[offset..offset + word.len()]
                        .iter()
                        .all(|(_, s)| s.add_modifier.contains(modifier))
                );
            }
            for line in &physical {
                assert!(line.width() <= 28);
                // The physical wrapper pads unused cells; only the list's occupied region owns this background.
                let occupied = line_text(line).trim_end().chars().count();
                for (_, style) in styled_chars(line).into_iter().take(occupied) {
                    if let Some(bg) = bg {
                        assert_eq!(style.bg, Some(bg), "background on {:?}", line_text(line));
                    } else {
                        assert_ne!(style.bg, Some(Color::Blue));
                    }
                }
            }
        }
    }

    #[test]
    fn table_and_code_inside_a_list_preserve_cells_and_literal_code_lines() {
        let rows = document_rows(
            "1. Parent\n\n   | Name | Value |\n   | --- | --- |\n   | Alpha | Bravo |\n   | Gamma | Delta |\n\n   ~~~text\n   - literal code\n       indented code\n   ~~~\n\n   Tail paragraph.",
            70,
        );
        let header = rows.iter().position(|r| r.contains("Name") && r.contains("Value")).unwrap();
        let column = |row: &str, word: &str| {
            unicode_width::UnicodeWidthStr::width(&row[..row.find(word).unwrap()])
        };
        for (left, right) in [("Alpha", "Bravo"), ("Gamma", "Delta")] {
            let i = rows.iter().position(|r| r.contains(left)).unwrap();
            assert!(i > header);
            assert_eq!(column(&rows[i], left), column(&rows[header], "Name"));
            assert_eq!(column(&rows[i], right), column(&rows[header], "Value"));
            assert!(column(&rows[i], left) < column(&rows[i], right));
        }
        let code = rows.iter().position(|r| r.trim_start() == "- literal code").unwrap();
        let indent = rows[code].len() - rows[code].trim_start().len();
        assert_eq!(rows[code + 1], format!("{}    indented code", " ".repeat(indent)));
        assert!(rows[code + 2..].iter().any(|r| r.trim() == "Tail paragraph."));
    }

    #[test]
    fn checklist_state_and_malformed_input_remain_readable() {
        let rows = document_rows("- [ ] Pending\n- [x] Complete\n  - [ ] Child", 70);
        let pending = rows.iter().find(|r| r.contains("Pending")).unwrap();
        let complete = rows.iter().find(|r| r.contains("Complete")).unwrap();
        assert_ne!(pending.split("Pending").next(), complete.split("Complete").next());
        let child = rows.iter().find(|r| r.contains("Child")).unwrap();
        assert!(child.find("Child").unwrap() > pending.find("Pending").unwrap());
        assert!(
            document_rows("```md\n- [ ] literal\n```", 70).iter().any(|r| r == "- [ ] literal")
        );
        let malformed = document_rows("Unfinished [label](broken", 70);
        assert!(malformed.iter().any(|r| r == "Unfinished [label](broken"));
        let quoted =
            document_rows("> Report\n>\n> | Owner | Status |\n> |---|---|\n> | Ada | Ready |", 70);
        let row = quoted.iter().find(|r| r.contains("Ada")).unwrap();
        assert!(row.contains("Ready"));
        assert!(
            quoted.iter().position(|r| r.contains("Report")).unwrap()
                < quoted.iter().position(|r| r.contains("Ada")).unwrap()
        );
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
    fn lists_preserve_paragraph_gaps_item_order_and_numbering() {
        for (items, ordered) in [("- One\n- Two", false), ("1. One\n2. Two", true)] {
            let rows =
                rendered_text(&render_markdown_safe(&format!("Intro\n\n{items}\n\nOutro"), None));
            let first = rows.iter().position(|r| r.ends_with("One")).unwrap();
            let second = rows.iter().position(|r| r.ends_with("Two")).unwrap();
            assert_eq!(rows[..first], ["Intro", ""]);
            assert_eq!(second, first + 1);
            assert_eq!(rows[second + 1..], ["", "Outro"]);
            if ordered {
                assert!(rows[first].starts_with("1."));
                assert!(rows[second].starts_with("2."));
            } else {
                assert!(!rows[first].trim_start().starts_with("One"));
                assert_eq!(rows[first].strip_suffix("One"), rows[second].strip_suffix("Two"));
            }
        }
    }

    #[test]
    fn text_after_a_list_returns_to_the_normal_column_without_a_blank_line() {
        let lines = render_markdown_at_width("Intro\n1. One\n2. Two\nOutro", 40, None, true);
        let rows = rendered_text(&lines);
        let last_item = rows.iter().position(|row| row == "2. Two").expect("last list item");
        assert_eq!(rows[last_item + 1], "Outro", "trailing text acquired list indentation");
        assert_eq!(rows.first().map(String::as_str), Some("Intro"));
    }

    #[test]
    fn headings_preserve_order_and_differ_from_body_without_source_markers() {
        let lines = render_markdown_safe("# Title\n\nBody\n\n## Section", None);
        let rows = rendered_text(&lines);
        let body = rows.iter().position(|r| r == "Body").unwrap();
        let body_style = styled_chars(&lines[body])[0].1;
        for (heading, before) in [("Title", true), ("Section", false)] {
            let i = rows.iter().position(|r| r == heading).unwrap();
            assert_eq!(i < body, before);
            assert!(styled_chars(&lines[i]).iter().any(|(_, style)| *style != body_style));
        }
        assert!(!rows.iter().any(|r| r.contains('#')));
    }

    #[test]
    fn code_kinds_preserve_literal_body_language_and_separators() {
        for (source, expected, language) in [
            (
                "```bash\n# heading\n- list\n- [ ] checklist\n  alpha  beta\n```",
                vec!["# heading", "- list", "- [ ] checklist", "  alpha  beta"],
                Some("bash"),
            ),
            ("```\nplain  source\n```", vec!["plain  source"], None),
            ("    indented  source", vec!["indented  source"], None),
        ] {
            let rows =
                rendered_text(&render_markdown_safe(&format!("Intro\n\n{source}\n\nAfter"), None));
            let first = rows.iter().position(|r| r == expected[0]).unwrap();
            assert!(first > 1);
            assert_eq!(rows[first..first + expected.len()], expected);
            assert!(!rows[first - 1].trim().is_empty());
            if let Some(language) = language {
                assert_eq!(rows[first - 1], language);
            } else {
                assert_ne!(rows[first - 1], expected[0]);
            }
            assert_eq!(rows[..first - 1], ["Intro", ""]);
            assert_eq!(rows[first + expected.len()..], ["", "After"]);
        }
    }

    #[test]
    fn quoted_code_wraps_inside_the_quote_column() {
        let code = "    let  value = \"\u{754c}  alpha beta gamma\";  END";
        for width in [24, 40] {
            let lines = render_markdown_at_width(
                &format!("> ```rust\n> {code}\n> ```"),
                width,
                None,
                false,
            );
            let rows = rendered_text(&lines);
            let label = rows.iter().position(|r| r.ends_with("rust")).unwrap();
            let prefix = rows[label].strip_suffix("rust").unwrap();
            assert!(!prefix.trim().is_empty());
            let body = &rows[label + 1..];
            assert!(body.len() > 1);
            let recovered = body
                .iter()
                .map(|r| r.strip_prefix(prefix).expect("quoted continuation"))
                .collect::<String>();
            assert_eq!(recovered, code);
            assert!(lines.iter().all(|l| l.width() <= usize::from(width)));
        }
    }

    #[test]
    fn preserved_newlines_become_line_breaks_outside_code() {
        for ending in ["\n", "\r\n"] {
            let source="first line\nsecond line\n\n- item one\n  continued\n\n```sh\necho  a  \necho b\n```".replace('\n',ending);
            for preserve in [true, false] {
                let rows = rendered_text(&render_markdown_at_width(&source, 80, None, preserve));
                let first = rows.iter().position(|r| r.contains("first line")).unwrap();
                if preserve {
                    assert_eq!(&rows[first..first + 2], ["first line", "second line"]);
                } else {
                    assert_eq!(rows[first], "first line second line");
                }
                let item = rows.iter().position(|r| r.contains("item one")).unwrap();
                assert!(item > first);
                if preserve {
                    assert_eq!(rows[item + 1].trim_start(), "continued");
                } else {
                    assert!(rows[item].contains("continued"));
                }
                let code = rows.iter().position(|r| r == "echo  a  ").unwrap();
                assert_eq!(rows[code + 1], "echo b");
                assert!(code > item);
            }
        }
    }

    fn styled_chars(line: &Line<'_>) -> Vec<(char, Style)> {
        line.spans
            .iter()
            .flat_map(|span| span.content.chars().map(|ch| (ch, line.style.patch(span.style))))
            .collect()
    }
    #[test]
    fn reference_links_resolve_across_code_in_synchronous_and_cursor_layout() {
        for with_code in [false, true] {
            let code = if with_code { "\n\n```text\n[Literal][alpha]\n```" } else { "" };
            let source = format!(
                "[Alpha][alpha]{code}\n\n[Beta][beta]\n\n[alpha]: https://alpha.test/a\n[beta]: https://beta.test/b\n"
            );
            let control = crate::ui::layout_control::LayoutControl::uninterrupted();
            let synchronous = render_markdown_at_width(&source, 100, None, false);
            let mut cursor = DocumentCursor::new(&source, &control);
            let mut resumed = Vec::new();
            while let Some(lines) = cursor.next(&source, 100, None, false, &control) {
                resumed.extend(lines);
            }
            for lines in [synchronous, resumed] {
                let rows = rendered_text(&lines);
                let alpha = rows
                    .iter()
                    .position(|r| r.contains("Alpha") && !r.contains("Literal"))
                    .unwrap();
                let beta = rows.iter().position(|r| r.contains("Beta")).unwrap();
                assert!(alpha < beta);
                assert_eq!(rows[alpha], "Alpha (https://alpha.test/a)");
                assert_eq!(rows[beta], "Beta (https://beta.test/b)");
                if with_code {
                    let literal = rows.iter().position(|r| r == "[Literal][alpha]").unwrap();
                    assert!(alpha < literal && literal < beta);
                    assert_eq!(rows[literal - 1], "text");
                }
            }
        }
    }
}
