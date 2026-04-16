// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use crate::agent::model;
use crate::ui::theme;
use crate::ui::wrap::{StyledChunk, wrap_styled_chunks};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use similar::TextDiff;

/// Render a diff with proper unified-style output using the `similar` crate.
/// The model `Diff` struct provides `old_text`/`new_text` -- we compute the actual
/// line-level changes and show only changed lines with context.
pub fn render_diff(diff: &model::Diff, width: u16) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    if let Some(repository) = diff.repository.as_deref() {
        lines.push(Line::from(Span::styled(
            format!("[{repository}]"),
            Style::default().fg(theme::DIM),
        )));
    }

    let old = diff.old_text.as_deref().unwrap_or("");
    let new = &diff.new_text;
    let text_diff = TextDiff::from_lines(old, new);
    let line_number_width = old.lines().count().max(new.lines().count()).max(1).to_string().len();
    let content_width = usize::from(width).saturating_sub(line_number_width + 5).max(1);

    // Use unified diff with 3 lines of context -- only shows changed hunks
    // instead of the full file content.
    let udiff = text_diff.unified_diff();
    for hunk in udiff.iter_hunks() {
        // Extract the @@ header from the hunk's Display output (first line).
        let hunk_str = hunk.to_string();
        if let Some(header) = hunk_str.lines().next()
            && header.starts_with("@@")
        {
            lines.push(Line::from(Span::styled(
                format_compact_hunk_header(header),
                Style::default().fg(Color::Cyan),
            )));
        }

        for change in hunk.iter_changes() {
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

#[derive(Clone, Copy)]
struct HunkRange {
    start: usize,
    count: usize,
}

fn format_compact_hunk_header(header: &str) -> String {
    parse_unified_hunk_header(header).map_or_else(
        || header.to_owned(),
        |(old_range, new_range)| {
            let mut parts = Vec::with_capacity(2);
            if old_range.count > 0 {
                parts.push(format_range("-", old_range));
            }
            if new_range.count > 0 {
                parts.push(format_range("+", new_range));
            }
            if parts.is_empty() { "lines".to_owned() } else { format!("lines {}", parts.join(" ")) }
        },
    )
}

fn parse_unified_hunk_header(header: &str) -> Option<(HunkRange, HunkRange)> {
    let body = header.strip_prefix("@@ ")?.split(" @@").next()?;
    let mut parts = body.split_whitespace();
    let old_range = parse_prefixed_hunk_range(parts.next()?, '-')?;
    let new_range = parse_prefixed_hunk_range(parts.next()?, '+')?;
    Some((old_range, new_range))
}

fn parse_prefixed_hunk_range(token: &str, prefix: char) -> Option<HunkRange> {
    let raw = token.strip_prefix(prefix)?;
    let (start, count) = raw.split_once(',').map_or((raw, "1"), |(start, count)| (start, count));
    Some(HunkRange { start: start.parse().ok()?, count: count.parse().ok()? })
}

fn format_range(prefix: &str, range: HunkRange) -> String {
    if range.count <= 1 {
        format!("{prefix}{}", range.start)
    } else {
        let end = range.start.saturating_add(range.count.saturating_sub(1));
        format!("{prefix}{}-{end}", range.start)
    }
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
    let content_lines = if value.is_empty() {
        vec![Line::default()]
    } else {
        wrap_styled_chunks(&[StyledChunk { text: value.to_owned(), style }], content_width)
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
            spans.extend(content_line.spans);
            Line::from(spans)
        })
        .collect()
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
    if trimmed.starts_with("```") {
        // Find end of first line (the opening fence, possibly with a language tag)
        if let Some(first_newline) = trimmed.find('\n') {
            let after_opening = &trimmed[first_newline + 1..];
            // Check if it ends with a closing fence
            if let Some(body) = after_opening.strip_suffix("```") {
                return body.trim_end().to_owned();
            }
            // Also handle closing fence followed by newline
            let after_trimmed = after_opening.trim_end();
            if let Some(stripped) = after_trimmed.strip_suffix("```") {
                return stripped.trim_end().to_owned();
            }
        }
    }
    text.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn strip_outer_code_fence_handles_supported_and_passthrough_shapes() {
        let cases = [
            ("```rust\nfn main() {}\n```", "fn main() {}"),
            ("```\nhello world\n```", "hello world"),
            ("```\ncontent\n```  \n", "content"),
            ("```\n```\n", ""),
            ("```python\nline1\nline2\nline3\n```", "line1\nline2\nline3"),
            ("  ```\ncontent\n```", "content"),
            ("just plain text", "just plain text"),
            ("~~~\ncontent\n~~~", "~~~\ncontent\n~~~"),
            ("```rust\nfn main() {}", "```rust\nfn main() {}"),
        ];

        for (input, expected) in cases {
            assert_eq!(strip_outer_code_fence(input), expected, "input: {input:?}");
        }
    }

    #[test]
    fn strip_outer_code_fence_preserves_inner_fences_and_large_blocks() {
        let nested = "```\nsome code\n```\nmore code\n```";
        let nested_result = strip_outer_code_fence(nested);
        assert!(nested_result.contains("some code"));
        assert!(nested_result.contains("more code"));

        let quadruple = "````\ncontent here\n````";
        assert!(strip_outer_code_fence(quadruple).contains("content here"));

        let blank_lines = "```\n\n\n\n```";
        let blank_result = strip_outer_code_fence(blank_lines);
        assert!(blank_result.is_empty() || blank_result.chars().all(|c| c == '\n'));

        let big: String = (0..10_000).fold(String::new(), |mut s, i| {
            use std::fmt::Write;
            writeln!(s, "line {i}").unwrap();
            s
        });
        let input = format!("```\n{big}```");
        let result = strip_outer_code_fence(&input);
        assert!(result.contains("line 0"));
        assert!(result.contains("line 9999"));
    }

    #[test]
    fn render_diff_includes_repository_label() {
        let lines = render_diff(
            &model::Diff::new("src/main.rs", "fn main() {}\n")
                .old_text(Some("fn old() {}\n"))
                .repository(Some("acme/project".to_owned())),
            80,
        );
        let repository_line: String =
            lines[0].spans.iter().map(|span| span.content.as_ref()).collect();
        assert!(repository_line.contains("[acme/project]"));
    }

    #[test]
    fn looks_like_unified_diff_detects_git_style_payload() {
        let raw = "diff --git a/a.rs b/a.rs\nindex 111..222 100644\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-old\n+new\n";
        assert!(looks_like_unified_diff(raw));
    }

    #[test]
    fn render_raw_unified_diff_styles_hunks_and_additions() {
        let raw = "--- a/file.rs\n+++ b/file.rs\n@@ -1 +1 @@\n-old\n+new\n";
        let lines = render_raw_unified_diff(raw);
        assert_eq!(lines[0].spans[0].style.fg, Some(Color::Red));
        assert_eq!(lines[1].spans[0].style.fg, Some(Color::Green));
        assert_eq!(lines[2].spans[0].style.fg, Some(Color::Cyan));
        assert_eq!(lines[4].spans[0].style.fg, Some(Color::Green));
    }

    #[test]
    fn render_diff_adds_line_numbers_and_hanging_indent() {
        let lines = render_diff(
            &model::Diff::new(
                "tmp.md",
                "This is a long added line that should wrap onto another visual line.\n".to_owned(),
            ),
            28,
        );
        let rendered: Vec<String> = lines
            .iter()
            .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
            .collect();

        assert!(rendered.iter().any(|line| line.contains("lines +1")));
        assert!(rendered.iter().any(|line| line.contains("1  +  This is a long")));
        assert!(rendered.iter().any(|line| line.starts_with("      ")));
        assert!(!rendered.iter().any(|line| line == "tmp.md"));
    }

    #[test]
    fn compact_hunk_header_omits_empty_side_and_uses_ranges() {
        assert_eq!(format_compact_hunk_header("@@ -0,0 +1,7 @@"), "lines +1-7");
        assert_eq!(format_compact_hunk_header("@@ -4,3 +4,5 @@"), "lines -4-6 +4-8");
        assert_eq!(format_compact_hunk_header("@@ -8 +8 @@"), "lines -8 +8");
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
}
