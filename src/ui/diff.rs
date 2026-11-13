// SPDX-License-Identifier: Apache-2.0

use crate::agent::model;
use crate::ui::theme;
use crate::ui::wrap::{StyledChunk, display_width, wrap_styled_chunks};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use similar::TextDiff;

/// Render a diff with proper unified-style output using the `similar` crate.
/// The model `Diff` struct provides `old_text`/`new_text` -- we compute the actual
/// line-level changes and show only changed lines with context.
pub fn render_diff(
    diff: &model::Diff,
    width: u16,
    control: &super::layout_control::LayoutControl<'_>,
) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    if let Some(repository) = diff.repository.as_deref() {
        lines.push(Line::from(Span::styled(
            format!("[{repository}]"),
            Style::default().fg(theme::DIM),
        )));
    }

    let old = diff.old_text.as_deref().unwrap_or("");
    let new = &diff.new_text;
    if control.cancelled() {
        return Vec::new();
    }
    let text_diff = TextDiff::from_lines(old, new);
    let line_number_width = old.lines().count().max(new.lines().count()).max(1).to_string().len();
    let content_width = usize::from(width).saturating_sub(line_number_width + 5).max(1);

    // Use unified diff with 3 lines of context -- only shows changed hunks
    // instead of the full file content.
    let udiff = text_diff.unified_diff();
    for hunk in udiff.iter_hunks() {
        if control.cancelled() {
            return Vec::new();
        }
        let (added, removed) =
            hunk.iter_changes().fold((0, 0), |(added, removed), change| match change.tag() {
                similar::ChangeTag::Delete => (added, removed + 1),
                similar::ChangeTag::Insert => (added + 1, removed),
                similar::ChangeTag::Equal => (added, removed),
            });
        if let Some(header) = render_diff_count_header(added, removed) {
            lines.push(header);
        }

        for change in hunk.iter_changes() {
            if control.cancelled() {
                return Vec::new();
            }
            let value = change.as_str().unwrap_or("").trim_end_matches('\n');
            let (marker, style, line_number) = match change.tag() {
                similar::ChangeTag::Delete => (
                    "-",
                    Style::default().fg(Color::Red),
                    change.old_index().map(|index| index + 1),
                ),
                similar::ChangeTag::Insert => (
                    "+",
                    Style::default().fg(Color::Green),
                    change.new_index().map(|index| index + 1),
                ),
                similar::ChangeTag::Equal => (
                    " ",
                    Style::default().fg(theme::DIM),
                    change.new_index().map(|index| index + 1),
                ),
            };
            lines.extend(render_wrapped_diff_row(
                line_number,
                line_number_width,
                marker,
                value,
                style,
                content_width,
            ));
        }
    }

    lines
}

pub fn looks_like_unified_diff(text: &str) -> bool {
    let mut saw_hunk = false;
    let mut saw_file_header = false;
    let mut saw_metadata = false;

    for line in text.lines().take(64) {
        if line.starts_with("@@") {
            saw_hunk = true;
        } else if line.starts_with("--- ") || line.starts_with("+++ ") {
            saw_file_header = true;
        } else if line.starts_with("diff --git ")
            || line.starts_with("index ")
            || line.starts_with("new file mode ")
            || line.starts_with("deleted file mode ")
            || line.starts_with("rename from ")
            || line.starts_with("rename to ")
        {
            saw_metadata = true;
        }
    }

    saw_hunk && (saw_file_header || saw_metadata)
}

pub fn render_raw_unified_diff(text: &str) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    for line in text.split('\n') {
        lines.push(render_raw_diff_line(line));
    }

    if lines.is_empty() {
        lines.push(Line::default());
    }

    lines
}

fn render_raw_diff_line(line: &str) -> Line<'static> {
    let style = if line.starts_with("diff --git ")
        || line.starts_with("index ")
        || line.starts_with("new file mode ")
        || line.starts_with("deleted file mode ")
        || line.starts_with("similarity index ")
        || line.starts_with("rename from ")
        || line.starts_with("rename to ")
    {
        Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
    } else if line.starts_with("@@") {
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
    } else if line.starts_with("+++ ") {
        Style::default().fg(Color::Green)
    } else if line.starts_with("--- ") {
        Style::default().fg(Color::Red)
    } else if line.starts_with('+') {
        Style::default().fg(Color::Green)
    } else if line.starts_with('-') {
        Style::default().fg(Color::Red)
    } else if line.starts_with('\\') {
        Style::default().fg(theme::DIM).add_modifier(Modifier::ITALIC)
    } else {
        Style::default().fg(theme::DIM)
    };

    Line::from(Span::styled(line.to_owned(), style))
}

/// Wrap Git's own patch while retaining file line numbers and hunk locations.
pub(crate) fn render_repository_patch(text: &str, width: u16) -> (Vec<Line<'static>>, Vec<usize>) {
    let mut lines = Vec::new();
    let mut hunks = Vec::new();
    let mut position = None;
    let number_width = text
        .lines()
        .filter_map(hunk_position)
        .map(|(old, new)| old.max(new))
        .max()
        .unwrap_or(1)
        .saturating_add(text.lines().count())
        .to_string()
        .len();
    for raw in text.lines() {
        if raw.starts_with("diff ") {
            position = None;
        }
        // The viewer's pane title already identifies the file. Keep its small
        // viewport for changes rather than Git's repeated path/index headers.
        if position.is_none()
            && (raw.starts_with("diff --git ")
                || raw.starts_with("index ")
                || raw.starts_with("--- ")
                || raw.starts_with("+++ "))
        {
            continue;
        }
        if raw.starts_with("@@") {
            hunks.push(lines.len());
            position = hunk_position(raw);
        } else if let Some((old, new)) = &mut position {
            let row = match raw.chars().next() {
                Some('+') => {
                    let row = Some((*new, "+"));
                    *new += 1;
                    row
                }
                Some('-') => {
                    let row = Some((*old, "-"));
                    *old += 1;
                    row
                }
                Some(' ') => {
                    let row = Some((*new, " "));
                    *old += 1;
                    *new += 1;
                    row
                }
                _ => None,
            };
            if let Some((number, marker)) = row {
                let styled = render_raw_diff_line(raw);
                let style = styled.spans[0].style;
                lines.extend(render_wrapped_diff_row(
                    Some(number),
                    number_width,
                    marker,
                    &raw[1..],
                    style,
                    usize::from(width).saturating_sub(number_width + 5).max(1),
                ));
                continue;
            }
        }
        let styled = render_raw_diff_line(raw);
        lines.extend(wrap_styled_chunks(
            &[StyledChunk { text: raw.to_owned(), style: styled.spans[0].style }],
            usize::from(width).max(1),
        ));
    }
    (lines, hunks)
}

fn hunk_position(line: &str) -> Option<(usize, usize)> {
    let mut parts = line.strip_prefix("@@ ")?.split_whitespace();
    let old = parts.next()?.strip_prefix('-')?.split(',').next()?.parse().ok()?;
    let new = parts.next()?.strip_prefix('+')?.split(',').next()?.parse().ok()?;
    Some((old, new))
}

pub(crate) fn render_diff_count_header(added: usize, removed: usize) -> Option<Line<'static>> {
    if added == 0 && removed == 0 {
        return None;
    }

    let punctuation_style = Style::default().fg(theme::DIM);
    let added_style = Style::default().fg(Color::Green);
    let removed_style = Style::default().fg(Color::Red);
    let mut spans = vec![Span::styled("(".to_owned(), punctuation_style)];
    if added > 0 {
        spans.push(Span::styled(format!("+{added}"), added_style));
    }
    if added > 0 && removed > 0 {
        spans.push(Span::styled(", ".to_owned(), punctuation_style));
    }
    if removed > 0 {
        spans.push(Span::styled(format!("-{removed}"), removed_style));
    }
    spans.push(Span::styled(")".to_owned(), punctuation_style));

    Some(Line::from(spans))
}

fn render_wrapped_diff_row(
    line_number: Option<usize>,
    line_number_width: usize,
    marker: &str,
    value: &str,
    style: Style,
    content_width: usize,
) -> Vec<Line<'static>> {
    let number_style = Style::default().fg(theme::DIM);
    let (leading_indent, content) = split_leading_whitespace(value);
    let leading_indent_width = display_width(leading_indent);
    let content_lines = if content.is_empty() {
        vec![Line::default()]
    } else {
        let wrapped_width = content_width.saturating_sub(leading_indent_width).max(1);
        super::wrap::split_line_at_width(
            Line::from(Span::styled(content.to_owned(), style)),
            wrapped_width,
        )
    };

    let line_number_text = line_number.map_or_else(
        || " ".repeat(line_number_width),
        |line_number| format!("{line_number:>line_number_width$}"),
    );
    let continuation_prefix = " ".repeat(line_number_width + 5);

    content_lines
        .into_iter()
        .enumerate()
        .map(|(index, content_line)| {
            let mut spans = if index == 0 {
                vec![
                    Span::styled(line_number_text.clone(), number_style),
                    Span::styled("  ", number_style),
                    Span::styled(marker.to_owned(), style),
                    Span::styled("  ", number_style),
                ]
            } else {
                vec![Span::styled(continuation_prefix.clone(), number_style)]
            };
            if !leading_indent.is_empty() {
                spans.push(Span::styled(leading_indent.to_owned(), style));
            }
            spans.extend(content_line.spans);
            Line::from(spans)
        })
        .collect()
}

fn split_leading_whitespace(text: &str) -> (&str, &str) {
    let split_at = text
        .char_indices()
        .find_map(|(idx, ch)| (!ch.is_whitespace()).then_some(idx))
        .unwrap_or(text.len());
    text.split_at(split_at)
}

/// Check if a tool call title references a markdown file.
#[allow(clippy::case_sensitive_file_extension_comparisons)]
pub fn is_markdown_file(title: &str) -> bool {
    let lower = title.to_lowercase();
    lower.ends_with(".md") || lower.ends_with(".mdx") || lower.ends_with(".markdown")
}

/// Extract a language tag from the file extension in a tool call title.
/// Returns the raw extension (e.g. "rs", "py", "toml") which syntect
/// can resolve to the correct syntax definition. Falls back to empty string.
pub fn lang_from_title(title: &str) -> String {
    // Title may be "src/main.rs" or "Read src/main.rs" - find last path-like token
    title
        .split_whitespace()
        .rev()
        .find_map(|token| {
            let ext = token.rsplit('.').next()?;
            // Ignore if the "extension" is the whole token (no dot found)
            if ext.len() < token.len() { Some(ext.to_lowercase()) } else { None }
        })
        .unwrap_or_default()
}

/// Strip an outer markdown code fence if the text is entirely wrapped in one.
/// The bridge adapter often wraps file contents in ```` ``` ```` fences.
pub fn strip_outer_code_fence(text: &str) -> String {
    let trimmed = text.trim();
    let Some((opening, rest)) = trimmed.split_once('\n') else { return text.to_owned() };
    let fence_len = opening.bytes().take_while(|byte| *byte == b'`').count();
    if fence_len < 3 || opening[fence_len..].contains('`') {
        return text.to_owned();
    }
    let (body, closing) = rest.rsplit_once('\n').unwrap_or(("", rest));
    if closing.len() == fence_len && closing.bytes().all(|byte| byte == b'`') {
        return body.trim_end().to_owned();
    }
    text.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn strip_outer_code_fence_handles_supported_and_passthrough_shapes() {
        for (input, expected) in [
            ("```rust\nfn main() {}\n```", "fn main() {}"),
            ("```\nhello world\n```", "hello world"),
            ("```\ncontent\n```  \n", "content"),
            ("```\n```\n", ""),
            ("```\n  \n```", ""),
            ("```python\nline1\nline2\nline3\n```", "line1\nline2\nline3"),
            ("  ```\ncontent\n```", "content"),
            ("just plain text", "just plain text"),
            ("~~~\ncontent\n~~~", "~~~\ncontent\n~~~"),
            ("```rust\nfn main() {}", "```rust\nfn main() {}"),
            ("```\nsome code\n```\nmore code\n```", "some code\n```\nmore code"),
            (
                "````markdown\nalpha\n```rust\ninner\n```\nomega\n````",
                "alpha\n```rust\ninner\n```\nomega",
            ),
            ("````\ncontent here\n````", "content here"),
            ("````\ncontent\n```", "````\ncontent\n```"),
            ("```\ninline ```", "```\ninline ```"),
        ] {
            assert_eq!(strip_outer_code_fence(input), expected, "input: {input:?}");
        }
    }

    #[test]
    fn terminal_dispatch_distinguishes_patches_from_program_output() {
        let control = crate::ui::layout_control::LayoutControl::uninterrupted();
        for patch in [
            "diff --git a/f b/f\nindex 111..222 100644\n@@ -1 +1 @@\n-old\n+new",
            "--- a/f\n+++ b/f\n@@ -1 +1 @@\n-old\n+new",
        ] {
            let lines = crate::ui::terminal_output::render_terminal_output(patch, &control);
            assert_eq!(plain(&lines).join("\n"), patch);
            assert_row_color(&lines, "-old", Color::Red);
            assert_row_color(&lines, "+new", Color::Green);
        }
        for text in [
            "@@ processing job\n+ordinary",
            "--- report\n+++ totals\n-ordinary",
            "index results\n+ordinary",
            "just output",
        ] {
            let lines = crate::ui::terminal_output::render_terminal_output(
                &format!("\x1b[35m{text}\x1b[0m"),
                &control,
            );
            assert_eq!(plain(&lines).join("\n"), text);
            for line in &lines {
                for span in &line.spans {
                    assert_eq!(span.style.fg, Some(Color::Magenta));
                }
            }
        }
    }

    #[test]
    fn raw_patch_preserves_each_source_row_and_its_change_style() {
        let patch = "diff --git a/f b/f\nindex 111..222 100644\n--- a/f\n+++ b/f\n@@ -1,3 +1,3 @@\n context\n \n-deleted body\n+added body\n\\ No newline at end of file";
        let lines = render_raw_unified_diff(patch);
        assert_eq!(plain(&lines).join("\n"), patch);
        assert_row_color(&lines, "-deleted body", Color::Red);
        assert_row_color(&lines, "+added body", Color::Green);
        let style = |text: &str| {
            lines
                .iter()
                .find(|l| line_text(l) == text)
                .unwrap()
                .spans
                .iter()
                .find(|s| !s.content.is_empty())
                .unwrap()
                .style
        };
        assert_ne!(style(" context"), style("+added body"));
        assert_ne!(style("@@ -1,3 +1,3 @@"), style(" context"));
        assert_ne!(style("\\ No newline at end of file"), style("-deleted body"));
    }

    #[test]
    fn structured_diff_preserves_complete_source_and_wrapped_line_identity() {
        let source = "fn main() {\n    if ready {\n        alpha  beta \u{754c} gamma delta epsilon zeta eta theta END\n    }\n}";
        for width in [32, 55, 100] {
            let lines = render_diff(
                &model::Diff::new("source.rs", source),
                width,
                &crate::ui::layout_control::LayoutControl::uninterrupted(),
            );
            let rows = plain(&lines);
            let starts = numbered_rows(&rows);
            assert_eq!(starts.iter().map(|(_, n, _)| *n).collect::<Vec<_>>(), [1, 2, 3, 4, 5]);
            for (i, expected) in source.lines().enumerate() {
                let (start, _, marker) = starts[i];
                assert_eq!(marker, '+');
                let end = starts.get(i + 1).map_or(rows.len(), |r| r.0);
                assert_wrapped_source(&rows[start..end], expected, '+');
            }
            assert!(lines.iter().all(|l| l.width() <= usize::from(width)));
        }
    }

    #[test]
    fn structured_diff_associates_source_positions_and_counts_with_each_hunk() {
        let old = (1..=130).fold(String::new(), |mut text, n| {
            use std::fmt::Write as _;
            writeln!(text, "source {n}").unwrap();
            text
        });
        let new = old
            .replace("source 12\n", "early replacement\n")
            .replace("source 122\n", "late first\nlate second\n");
        let lines = render_diff(
            &model::Diff::new("f", new).old_text(Some(old)),
            80,
            &crate::ui::layout_control::LayoutControl::uninterrupted(),
        );
        let rows = plain(&lines);
        for (text, number, marker, counts) in [
            ("source 12", 12, '-', ("+1", "-1")),
            ("early replacement", 12, '+', ("+1", "-1")),
            ("source 122", 122, '-', ("+2", "-1")),
            ("late first", 122, '+', ("+2", "-1")),
            ("late second", 123, '+', ("+2", "-1")),
            ("source 123", 124, ' ', ("+2", "-1")),
        ] {
            let index = rows.iter().position(|r| r.ends_with(text)).unwrap();
            assert_number(&rows[index], number, marker);
            let header = rows[..index].iter().rev().find(|r| count_row(r)).unwrap();
            assert!(header.contains(counts.0) && header.contains(counts.1), "{header}");
        }
        for (old, new, added, removed) in [
            ("", "one\ntwo\n", Some("+2"), None),
            ("one\ntwo\nthree\n", "", None, Some("-3")),
            ("old\n", "new\n", Some("+1"), Some("-1")),
            ("same\n", "same\n", None, None),
        ] {
            let lines = render_diff(
                &model::Diff::new("f", new).old_text(Some(old)),
                80,
                &crate::ui::layout_control::LayoutControl::uninterrupted(),
            );
            let headers = plain(&lines).into_iter().filter(|r| count_row(r)).collect::<Vec<_>>();
            if added.is_none() && removed.is_none() {
                assert!(headers.is_empty());
                continue;
            }
            assert_eq!(headers.len(), 1);
            for (expected, sign, color) in [(added, '+', Color::Green), (removed, '-', Color::Red)]
            {
                if let Some(expected) = expected {
                    assert!(headers[0].contains(expected));
                    let header = lines.iter().find(|l| line_text(l) == headers[0]).unwrap();
                    assert!(
                        header
                            .spans
                            .iter()
                            .any(|s| s.content == expected && s.style.fg == Some(color))
                    );
                } else {
                    assert!(!headers[0].contains(sign));
                }
            }
        }
    }

    #[test]
    fn lang_from_title_handles_common_paths_and_edge_cases() {
        let cases = [
            ("src/main.rs", "rs"),
            ("Read foo.py", "py"),
            ("Cargo.toml", "toml"),
            ("Makefile", ""),
            ("", ""),
            ("file.RS", "rs"),
            ("archive.tar.gz", "gz"),
            ("Read some/dir/file.tsx", "tsx"),
            (".gitignore", "gitignore"),
            ("Read a.test.spec.ts", "ts"),
            ("file.", ""),
            ("   ", ""),
            ("Read src\\main.rs", "rs"),
        ];

        for (title, expected) in cases {
            assert_eq!(lang_from_title(title), expected, "title: {title:?}");
        }
    }

    #[test]
    fn is_markdown_file_matches_supported_extensions_only() {
        let supported = [
            "README.md",
            "component.mdx",
            "doc.markdown",
            "README.MD",
            "file.Md",
            "docs/getting-started.md",
            "Read /home/user/notes.md",
            "FILE.MARKDOWN",
        ];
        for path in supported {
            assert!(is_markdown_file(path), "path should be markdown: {path:?}");
        }

        let unsupported = ["main.rs", "style.css", "", "somemdx", "file.md.bak"];
        for path in unsupported {
            assert!(!is_markdown_file(path), "path should not be markdown: {path:?}");
        }
    }
    fn count_row(row: &str) -> bool {
        row.split(|c: char| !c.is_ascii_digit() && c != '+' && c != '-').any(|token| {
            token
                .strip_prefix('+')
                .or_else(|| token.strip_prefix('-'))
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        })
    }
    fn line_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }
    fn plain(lines: &[Line<'_>]) -> Vec<String> {
        lines.iter().map(line_text).collect()
    }
    fn assert_row_color(lines: &[Line<'_>], text: &str, color: Color) {
        let row = lines.iter().find(|l| line_text(l) == text).expect("source row");
        assert!(
            row.spans.iter().filter(|s| !s.content.is_empty()).all(|s| s.style.fg == Some(color))
        );
    }
    fn numbered_rows(rows: &[String]) -> Vec<(usize, usize, char)> {
        rows.iter()
            .enumerate()
            .filter_map(|(i, r)| {
                let t = r.trim_start();
                let n = t.split_whitespace().next()?.parse().ok()?;
                let after = t.trim_start_matches(|c: char| c.is_ascii_digit());
                let marker = match after.trim_start().chars().next() {
                    Some('+') => '+',
                    Some('-') => '-',
                    _ => ' ',
                };
                Some((i, n, marker))
            })
            .collect()
    }
    fn assert_number(row: &str, number: usize, marker: char) {
        let parsed = numbered_rows(&[row.into()]);
        assert_eq!(parsed, [(0, number, marker)], "{row}");
    }
    fn assert_wrapped_source(rows: &[String], source: &str, marker: char) {
        let first = &rows[0];
        let marker_at = first.find(marker).unwrap();
        let body = marker_at + 3;
        let indent = source.len() - source.trim_start().len();
        let mut recovered = first[body..].to_owned();
        for row in &rows[1..] {
            assert!(row[..body + indent].chars().all(char::is_whitespace), "{row}");
            recovered.push_str(&row[body + indent..]);
        }
        assert_eq!(recovered, source);
    }
    #[test]
    fn repository_patch_tracks_literal_positions_and_hunk_anchors_after_wrapping() {
        let long = "    alpha  beta \u{754c} gamma delta epsilon zeta END";
        let patch = format!(
            "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -0,0 +1,2 @@\n+insert one\n+insert two\n@@ -10,2 +11,0 @@\n-delete ten\n-delete eleven\n@@ -20,3 +20,4 @@\n-old twenty\n+new twenty\n+{long}\n \n context twenty two\n@@ -40 +41 @@\n-old forty\n+new forty"
        );
        for width in [32, 100] {
            let (lines, anchors) = render_repository_patch(&patch, width);
            let rows = plain(&lines);
            assert_eq!(
                anchors.iter().map(|i| rows[*i].as_str()).collect::<Vec<_>>(),
                ["@@ -0,0 +1,2 @@", "@@ -10,2 +11,0 @@", "@@ -20,3 +20,4 @@", "@@ -40 +41 @@"]
            );
            for (text, n, marker) in [
                ("insert one", 1, '+'),
                ("insert two", 2, '+'),
                ("delete ten", 10, '-'),
                ("delete eleven", 11, '-'),
                ("old twenty", 20, '-'),
                ("new twenty", 20, '+'),
                ("context twenty two", 23, ' '),
                ("old forty", 40, '-'),
                ("new forty", 41, '+'),
            ] {
                assert_number(rows.iter().find(|r| r.ends_with(text)).unwrap(), n, marker);
            }
            let start = rows.iter().position(|r| r.contains("alpha")).unwrap();
            let end = numbered_rows(&rows)
                .into_iter()
                .find(|(i, n, _)| *i > start && *n == 22)
                .unwrap()
                .0;
            assert_wrapped_source(&rows[start..end], long, '+');
            assert_number(&rows[end], 22, ' ');
            assert!(rows[end].trim_end().ends_with("22"));
            assert!(lines.iter().all(|l| l.width() <= usize::from(width)));
        }
        let (lines, _) = render_repository_patch(
            "@@ -1,2 +1,3 @@\n context\n-old\n\\ No newline at end of file\n+new\n+tail\n\\ No newline at end of file",
            80,
        );
        let rows = plain(&lines);
        assert_number(rows.iter().find(|r| r.ends_with("new")).unwrap(), 2, '+');
        assert_number(rows.iter().find(|r| r.ends_with("tail")).unwrap(), 3, '+');
    }
}
