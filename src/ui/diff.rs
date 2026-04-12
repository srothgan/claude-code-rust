// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use crate::agent::model;
use crate::ui::theme;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use similar::TextDiff;

/// Render a diff with proper unified-style output using the `similar` crate.
/// The model `Diff` struct provides `old_text`/`new_text` -- we compute the actual
/// line-level changes and show only changed lines with context.
pub fn render_diff(diff: &model::Diff) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();

    // File path header
    let name = diff.path.file_name().map_or_else(
        || diff.path.to_string_lossy().into_owned(),
        |f| f.to_string_lossy().into_owned(),
    );
    let mut header_spans =
        vec![Span::styled(name, Style::default().fg(Color::White).add_modifier(Modifier::BOLD))];
    if let Some(repository) = diff.repository.as_deref() {
        header_spans
            .push(Span::styled(format!("  [{repository}]"), Style::default().fg(theme::DIM)));
    }
    lines.push(Line::from(header_spans));

    let old = diff.old_text.as_deref().unwrap_or("");
    let new = &diff.new_text;
    let text_diff = TextDiff::from_lines(old, new);

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
                header.to_owned(),
                Style::default().fg(Color::Cyan),
            )));
        }

        for change in hunk.iter_changes() {
            let value = change.as_str().unwrap_or("").trim_end_matches('\n');
            let (prefix, style) = match change.tag() {
                similar::ChangeTag::Delete => ("-", Style::default().fg(Color::Red)),
                similar::ChangeTag::Insert => ("+", Style::default().fg(Color::Green)),
                similar::ChangeTag::Equal => (" ", Style::default().fg(theme::DIM)),
            };
            lines.push(Line::from(Span::styled(format!("{prefix} {value}"), style)));
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
        );
        let header: String = lines[0].spans.iter().map(|span| span.content.as_ref()).collect();
        assert!(header.contains("main.rs"));
        assert!(header.contains("[acme/project]"));
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
