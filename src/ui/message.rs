// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

#[cfg(test)]
use crate::app::ChatMessage;
use crate::app::{BlockCache, TextBlock};
use crate::ui::tool_call;
use crate::ui::wrap::wrap_lines_to_physical_rows;
use ratatui::style::Color;
use ratatui::text::Line;

#[cfg(test)]
use super::message_rows::build_user_system_message_rows;

/// Presentation of active status and tool indicators, derived from saved preferences.
#[derive(Clone, Copy)]
pub enum SpinnerState {
    Animated(usize),
    Static,
}

impl SpinnerState {
    pub(crate) const FRAMES: &[&str] = &[
        "\u{280B}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283C}", "\u{2834}", "\u{2826}",
        "\u{2827}", "\u{2807}", "\u{280F}",
    ];

    pub(crate) fn for_app(app: &crate::app::App) -> Self {
        if app.config.prefers_reduced_motion_effective() {
            Self::Static
        } else {
            Self::Animated(app.spinner_frame)
        }
    }

    pub(crate) fn icon(self) -> &'static str {
        match self {
            Self::Animated(frame) => Self::FRAMES[frame % Self::FRAMES.len()],
            Self::Static => "\u{25C6}",
        }
    }
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

/// Render the complete canonical source so paragraph separators and Markdown
/// context survive caching. Streaming splits stable prefixes into text blocks
/// at message construction time; each block retains its own render cache.
pub(super) fn render_text_cached(
    text: &str,
    cache: &mut BlockCache,
    width: u16,
    bg: Option<Color>,
    preserve_newlines: bool,
    out: &mut Vec<Line<'static>>,
) {
    // Fast path only when the cached lines were measured at this width.
    // Lists and tables resolve width-dependent geometry before final row wrapping,
    // so a fresh cache from another width is not safe to reuse.
    if cache.height_at(width).is_some()
        && let Some(cached_lines) = cache.get()
    {
        out.extend_from_slice(cached_lines);
        return;
    }

    let fresh = super::markdown::render_markdown_at_width(text, width, bg, preserve_newlines);

    // Store in the full block cache with wrapped height.
    // For streaming messages this will be invalidated on the next chunk,
    // but for completed messages it persists.
    let h = wrap_lines_to_physical_rows(&fresh, width).len();
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
    render_text_cached(&block.text, &mut block.cache, width, bg, preserve_newlines, out);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{ChatMessage, MessageBlock, MessageRole, SystemSeverity, TextBlock};
    use pretty_assertions::assert_eq;

    fn make_text_message(role: MessageRole, text: &str) -> ChatMessage {
        ChatMessage::new(role, vec![MessageBlock::Text(TextBlock::from_complete(text))], None)
    }

    fn render_lines_to_strings(lines: &[Line<'static>]) -> Vec<String> {
        lines.iter().map(|line| line.to_string().trim_end().to_owned()).collect()
    }

    fn user_paragraphs(lines: &[Line<'static>]) -> Vec<String> {
        let content = lines
            .iter()
            .map(|line| line.to_string().chars().skip(3).collect::<String>().trim_end().to_owned())
            .collect::<Vec<_>>();
        content
            .split(String::is_empty)
            .filter(|paragraph| !paragraph.is_empty())
            .map(|paragraph| paragraph.join(" "))
            .collect()
    }

    #[test]
    fn user_paragraph_spacing_survives_cache_reuse_and_resize() {
        for (source, expected) in [
            ("hello\n\nhow are you", vec!["hello", "how are you"]),
            ("hello\nhow are you", vec!["hello how are you"]),
            ("one\n\ntwo\n\nthree", vec!["one", "two", "three"]),
            ("hello\n\n\nhow are you", vec!["hello", "how are you"]),
            ("hello\n \t\nhow are you", vec!["hello", "how are you"]),
            ("こんにちは\n\n😀 hello", vec!["こんにちは", "😀 hello"]),
        ] {
            let mut msg = make_text_message(MessageRole::User, source);
            for width in [80, 80, 16, 80] {
                let mut lines = Vec::new();
                render_message(&mut msg, width, &mut lines);
                assert_eq!(user_paragraphs(&lines), expected, "source={source:?}, width={width}");
                let MessageBlock::Text(block) = &msg.blocks[0] else {
                    panic!("expected user text");
                };
                assert_eq!(block.text, source);
            }
        }
    }

    #[test]
    fn user_paragraph_spacing_survives_source_append_and_cache_invalidation() {
        let mut msg = make_text_message(MessageRole::User, "hello\n\n");
        let mut before = Vec::new();
        render_message(&mut msg, 80, &mut before);
        let MessageBlock::Text(block) = &mut msg.blocks[0] else {
            panic!("expected user text");
        };
        block.text.push_str("how are you");
        block.cache.invalidate();

        let mut after = Vec::new();
        render_message(&mut msg, 80, &mut after);
        assert_eq!(user_paragraphs(&before), ["hello"]);
        assert_eq!(user_paragraphs(&after), ["hello", "how are you"]);
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
        render_message(&mut msg, 16, &mut narrow_lines);
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
    fn user_newlines_persist_and_code_keeps_its_indentation() {
        let source = "first\nsecond\n\n```text\n  indented\nplain\n```";
        let mut msg = make_text_message(MessageRole::User, source);
        let mut lines = Vec::new();
        render_message(&mut msg, 80, &mut lines);
        let content = lines
            .iter()
            .map(|line| line.to_string().chars().skip(3).collect::<String>().trim_end().to_owned())
            .collect::<Vec<_>>();
        let first = content.iter().position(|row| !row.is_empty()).expect("content");
        let last = content.iter().rposition(|row| !row.is_empty()).expect("content");

        assert_eq!(content[first..=last], ["first", "second", "", "text", "  indented", "plain"]);
        let MessageBlock::Text(block) = &msg.blocks[0] else {
            panic!("expected user text");
        };
        assert_eq!(block.text, source);
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
