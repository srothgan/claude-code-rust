// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

#[cfg(test)]
use crate::app::ChatMessage;
use crate::app::{BlockCache, IncrementalMarkdown, MarkdownRenderKey, TextBlock};
use crate::ui::tool_call;
use ratatui::style::Color;
use ratatui::text::{Line, Text};
use ratatui::widgets::{Paragraph, Wrap};

#[cfg(test)]
use super::message_rows::build_user_system_message_rows;

/// Frame counter for animated status and tool indicators.
#[derive(Clone, Copy)]
pub struct SpinnerState {
    pub frame: usize,
}

#[derive(Clone, Copy)]
pub(crate) struct MessageRenderContext<'a> {
    pub(crate) tool_render_context: tool_call::ToolCallRenderContext<'a>,
    pub(crate) width: u16,
}

impl<'a> MessageRenderContext<'a> {
    pub(crate) fn new(current_mode_id: Option<&'a str>, width: u16) -> Self {
        Self { tool_render_context: tool_call::ToolCallRenderContext { current_mode_id }, width }
    }
}

#[cfg(test)]
pub(crate) fn render_message(msg: &mut ChatMessage, width: u16, out: &mut Vec<Line<'static>>) {
    let render_context = MessageRenderContext::new(None, width);
    render_message_rows(msg, render_context, out);
}

#[cfg(test)]
fn render_message_rows(
    msg: &mut ChatMessage,
    render_context: MessageRenderContext<'_>,
    out: &mut Vec<Line<'static>>,
) {
    let rows = build_user_system_message_rows(msg, render_context);
    for segment in rows.segments {
        match segment {
            super::message_rows::MessageRowSegment::Blank => out.push(Line::default()),
            super::message_rows::MessageRowSegment::Lines { lines } => out.extend(lines),
        }
    }
}

/// Preprocess markdown that `tui_markdown` doesn't handle well.
/// Headings (`# Title`) become `**Title**` (bold) with a blank line before.
/// Handles variations: `#Title`, `#  Title`, `  ## Title  `, etc.
/// Links are left as-is -- `tui_markdown` handles `[title](url)` natively.
fn preprocess_markdown(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            // Strip all leading '#' characters
            let after_hashes = trimmed.trim_start_matches('#');
            // Extract heading content (trim spaces between # and text, and trailing)
            let content = after_hashes.trim();
            if !content.is_empty() {
                // Blank line before heading for visual separation
                if !result.is_empty() && !result.ends_with("\n\n") {
                    result.push('\n');
                }
                result.push_str("**");
                result.push_str(content);
                result.push_str("**\n");
                continue;
            }
        }
        result.push_str(line);
        result.push('\n');
    }
    if !text.ends_with('\n') {
        result.pop();
    }
    result
}

/// Render a text block with caching. Uses paragraph-level incremental markdown
/// during streaming to avoid re-parsing the entire text every frame.
///
/// Cache hierarchy:
/// 1. `BlockCache` (full block) -- hit for completed messages (no changes).
/// 2. `IncrementalMarkdown` (per-paragraph) -- only tail paragraph re-parsed during streaming.
pub(super) fn render_text_cached(
    text: &str,
    cache: &mut BlockCache,
    incr: &mut IncrementalMarkdown,
    width: u16,
    bg: Option<Color>,
    preserve_newlines: bool,
    out: &mut Vec<Line<'static>>,
) {
    // Fast path only when the cached lines were measured at this width.
    // Markdown tables produce width-dependent logical lines before paragraph
    // wrapping, so a fresh cache from another width is not safe to reuse.
    if cache.height_at(width).is_some()
        && let Some(cached_lines) = cache.get()
    {
        crate::perf::mark_with("msg::cache_hit", "lines", cached_lines.len());
        out.extend_from_slice(cached_lines);
        return;
    }
    crate::perf::mark("msg::cache_miss");

    let _t = crate::perf::start("msg::render_text");

    // Build a render function that handles preprocessing + tui_markdown
    let render_fn = |src: &str| -> Vec<Line<'static>> {
        let mut preprocessed = preprocess_markdown(src);
        if preserve_newlines {
            preprocessed = force_markdown_line_breaks(&preprocessed);
        }
        super::document_table::render_markdown_with_tables(&preprocessed, width, bg)
    };
    let render_key = MarkdownRenderKey { width, bg, preserve_newlines };

    // Ensure any previously invalidated paragraph caches are re-rendered
    let _ = text;
    incr.ensure_rendered(render_key, &render_fn);

    // Render: cached paragraphs + fresh tail
    let fresh = incr.lines(render_key, &render_fn);

    // Store in the full block cache with wrapped height.
    // For streaming messages this will be invalidated on the next chunk,
    // but for completed messages it persists.
    let h = {
        let _t = crate::perf::start_with("msg::wrap_height", "lines", fresh.len());
        Paragraph::new(Text::from(fresh.clone())).wrap(Wrap { trim: false }).line_count(width)
    };
    cache.store(fresh);
    cache.set_height(h, width);
    if let Some(stored) = cache.get() {
        out.extend_from_slice(stored);
    }
}

pub(super) fn render_text_block_cached(
    block: &mut TextBlock,
    width: u16,
    bg: Option<Color>,
    preserve_newlines: bool,
    out: &mut Vec<Line<'static>>,
) {
    render_text_cached(
        &block.text,
        &mut block.cache,
        &mut block.markdown,
        width,
        bg,
        preserve_newlines,
        out,
    );
}

/// Convert single line breaks into hard breaks so user-entered newlines persist.
fn force_markdown_line_breaks(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = String::with_capacity(text.len());
    for (i, line) in lines.iter().enumerate() {
        if !line.is_empty() {
            out.push_str(line);
            out.push_str("  ");
        }
        if i + 1 < lines.len() || text.ends_with('\n') {
            out.push('\n');
        }
    }
    if text.ends_with('\n') {
        // preserve trailing newline
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{ChatMessage, MessageBlock, MessageRole, SystemSeverity, TextBlock};
    use pretty_assertions::assert_eq;

    // preprocess_markdown

    #[test]
    fn preprocess_h1_heading() {
        let result = preprocess_markdown("# Hello");
        assert!(result.contains("**Hello**"));
        assert!(!result.contains('#'));
    }

    #[test]
    fn preprocess_h3_heading() {
        let result = preprocess_markdown("### Deeply Nested");
        assert!(result.contains("**Deeply Nested**"));
    }

    #[test]
    fn preprocess_non_heading_passthrough() {
        let input = "Just normal text\nwith multiple lines";
        let result = preprocess_markdown(input);
        assert_eq!(result, input);
    }

    #[test]
    fn preprocess_mixed_headings_and_text() {
        let input = "# Title\nSome text\n## Subtitle\nMore text";
        let result = preprocess_markdown(input);
        assert!(result.contains("**Title**"));
        assert!(result.contains("Some text"));
        assert!(result.contains("**Subtitle**"));
        assert!(result.contains("More text"));
    }

    #[test]
    fn preprocess_heading_no_space() {
        let result = preprocess_markdown("#Title");
        assert!(result.contains("**Title**"));
    }

    #[test]
    fn preprocess_heading_extra_spaces() {
        let result = preprocess_markdown("#   Spaced Out   ");
        assert!(result.contains("**Spaced Out**"));
    }

    #[test]
    fn preprocess_indented_heading() {
        let result = preprocess_markdown("  ## Indented");
        assert!(result.contains("**Indented**"));
    }

    #[test]
    fn preprocess_empty_heading() {
        let result = preprocess_markdown("# ");
        assert_eq!(result, "# ");
    }

    #[test]
    fn preprocess_empty_string() {
        assert_eq!(preprocess_markdown(""), "");
    }

    #[test]
    fn preprocess_preserves_trailing_newline() {
        let result = preprocess_markdown("hello\n");
        assert!(result.ends_with('\n'));
    }

    #[test]
    fn preprocess_no_trailing_newline() {
        let result = preprocess_markdown("hello");
        assert!(!result.ends_with('\n'));
    }

    #[test]
    fn preprocess_blank_line_before_heading() {
        let input = "text\n\n# Heading";
        let result = preprocess_markdown(input);
        assert!(!result.contains("\n\n\n"));
        assert!(result.contains("**Heading**"));
    }

    #[test]
    fn preprocess_consecutive_headings() {
        let input = "# First\n# Second";
        let result = preprocess_markdown(input);
        assert!(result.contains("**First**"));
        assert!(result.contains("**Second**"));
    }

    #[test]
    fn preprocess_hash_in_code_not_heading() {
        let result = preprocess_markdown("# actual heading");
        assert!(result.contains("**actual heading**"));
    }

    /// H6 heading (6 `#` chars).
    #[test]
    fn preprocess_h6_heading() {
        let result = preprocess_markdown("###### Deep H6");
        assert!(result.contains("**Deep H6**"));
        assert!(!result.contains('#'));
    }

    /// Heading with markdown formatting inside.
    #[test]
    fn preprocess_heading_with_bold_inside() {
        let result = preprocess_markdown("# **bold** and *italic*");
        assert!(result.contains("****bold** and *italic***"));
    }

    /// Heading at end of file with no trailing newline.
    #[test]
    fn preprocess_heading_at_eof_no_newline() {
        let result = preprocess_markdown("text\n# Final");
        assert!(result.contains("**Final**"));
        assert!(!result.ends_with('\n'));
    }

    /// Only hashes with no text: `###` - content after stripping is empty, passthrough.
    #[test]
    fn preprocess_only_hashes() {
        let result = preprocess_markdown("###");
        assert_eq!(result, "###");
    }

    /// Very long heading.
    #[test]
    fn preprocess_very_long_heading() {
        let long_text = "A".repeat(1000);
        let input = format!("# {long_text}");
        let result = preprocess_markdown(&input);
        assert!(result.starts_with("**"));
        assert!(result.contains(&long_text));
    }

    /// Unicode emoji in heading.
    #[test]
    fn preprocess_unicode_heading() {
        let result = preprocess_markdown("# \u{1F680} Launch \u{4F60}\u{597D}");
        assert!(result.contains("**\u{1F680} Launch \u{4F60}\u{597D}**"));
    }

    /// Quoted heading: `> # Heading` - starts with `>` not `#`, so passthrough.
    #[test]
    fn preprocess_blockquote_heading_passthrough() {
        let result = preprocess_markdown("> # Quoted heading");
        // Line starts with `>`, not `#`, so trimmed starts with `>` not `#`
        assert!(!result.contains("**"));
        assert!(result.contains("> # Quoted heading"));
    }

    /// All heading levels in sequence.
    #[test]
    fn preprocess_all_heading_levels() {
        let input = "# H1\n## H2\n### H3\n#### H4\n##### H5\n###### H6";
        let result = preprocess_markdown(input);
        for label in ["H1", "H2", "H3", "H4", "H5", "H6"] {
            assert!(result.contains(&format!("**{label}**")), "missing {label}");
        }
    }

    // force_markdown_line_breaks

    #[test]
    fn force_breaks_adds_trailing_spaces() {
        let result = force_markdown_line_breaks("line1\nline2");
        assert!(result.contains("line1  \n"));
        assert!(result.contains("line2  "));
    }

    #[test]
    fn force_breaks_preserves_trailing_newline() {
        let result = force_markdown_line_breaks("hello\n");
        assert!(result.ends_with('\n'));
    }

    #[test]
    fn force_breaks_empty_lines_no_trailing_spaces() {
        let result = force_markdown_line_breaks("a\n\nb");
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].ends_with("  "));
        assert_eq!(lines[1], "");
        assert!(lines[2].ends_with("  "));
    }

    #[test]
    fn force_breaks_single_line_no_trailing_newline() {
        let result = force_markdown_line_breaks("hello");
        assert_eq!(result, "hello  ");
    }

    #[test]
    fn force_breaks_many_consecutive_empty_lines() {
        let result = force_markdown_line_breaks("a\n\n\nb");
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 4);
    }

    /// Empty input.
    #[test]
    fn force_breaks_empty_input() {
        let result = force_markdown_line_breaks("");
        assert_eq!(result, "");
    }

    /// Only empty lines.
    #[test]
    fn force_breaks_only_empty_lines() {
        let result = force_markdown_line_breaks("\n\n\n");
        let lines: Vec<&str> = result.lines().collect();
        // All lines are empty, so no trailing spaces added
        for line in &lines {
            assert!(line.is_empty(), "empty line got content: {line:?}");
        }
    }

    /// Line already ending with two spaces - gets two more.
    #[test]
    fn force_breaks_already_has_trailing_spaces() {
        let result = force_markdown_line_breaks("hello  \nworld");
        // "hello  " + "  " = "hello    "
        assert!(result.starts_with("hello    "));
    }

    /// Single newline (no content).
    #[test]
    fn force_breaks_single_newline() {
        let result = force_markdown_line_breaks("\n");
        // One empty line, should stay empty with trailing newline
        assert_eq!(result, "\n");
    }

    fn make_text_message(role: MessageRole, text: &str) -> ChatMessage {
        ChatMessage::new(role, vec![MessageBlock::Text(TextBlock::from_complete(text))], None)
    }

    fn render_lines_to_strings(lines: &[Line<'static>]) -> Vec<String> {
        lines
            .iter()
            .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn markdown_table_rerenders_when_width_changes_in_both_directions() {
        let table = concat!(
            "| Name | Description |\n",
            "| --- | --- |\n",
            "| foo | long wrapped value |\n",
        );
        let mut msg = make_text_message(MessageRole::User, table);

        let mut wide_lines = Vec::new();
        render_message(&mut msg, 40, &mut wide_lines);
        let wide_rendered = render_lines_to_strings(&wide_lines);
        assert!(wide_rendered.iter().any(|line| line.contains("Name")));
        assert!(wide_rendered.iter().any(|line| line.contains('─')));
        assert!(!wide_rendered.iter().any(|line| line.contains("Name:")));

        let mut narrow_lines = Vec::new();
        render_message(&mut msg, 12, &mut narrow_lines);
        let narrow_rendered = render_lines_to_strings(&narrow_lines);
        assert!(narrow_rendered.iter().any(|line| line.contains("Name:")));
        assert!(narrow_rendered.iter().any(|line| line.contains("Description")));
        assert!(!narrow_rendered.iter().any(|line| line.contains('─')));

        let mut wide_again_lines = Vec::new();
        render_message(&mut msg, 40, &mut wide_again_lines);
        let wide_again_rendered = render_lines_to_strings(&wide_again_lines);
        assert!(wide_again_rendered.iter().any(|line| line.contains("Name")));
        assert!(wide_again_rendered.iter().any(|line| line.contains('─')));
        assert!(!wide_again_rendered.iter().any(|line| line.contains("Name:")));
    }

    #[test]
    fn system_warning_severity_renders_warning_label() {
        let mut msg = make_text_message(
            MessageRole::System(Some(SystemSeverity::Warning)),
            "Rate limit warning",
        );
        let mut lines = Vec::new();
        render_message(&mut msg, 120, &mut lines);
        let rendered = render_lines_to_strings(&lines);

        assert!(rendered.iter().any(|line| line.contains("Warning")));
        assert!(rendered.iter().any(|line| line.contains("Rate limit warning")));
    }
}
