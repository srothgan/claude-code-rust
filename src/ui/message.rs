// SPDX-License-Identifier: Apache-2.0

#[cfg(test)]
use crate::app::ChatMessage;
use crate::app::{BlockCache, TextBlock};
use crate::ui::tool_call;
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
        Self {
            tool_render_context: tool_call::ToolCallRenderContext {
                current_mode_id,
                clock: crate::app::presentation::ClockFormat::default(),
                control: None,
            },
            width,
        }
    }

    pub(crate) fn with_control(
        current_mode_id: Option<&'a str>,
        width: u16,
        control: &'a super::layout_control::LayoutControl<'a>,
    ) -> Self {
        Self {
            tool_render_context: tool_call::ToolCallRenderContext {
                current_mode_id,
                clock: crate::app::presentation::ClockFormat::default(),
                control: Some(control),
            },
            width,
        }
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
    control: &super::layout_control::LayoutControl<'_>,
) {
    // Reuse only lines laid out at this width.
    // Tables and code resolve width-dependent geometry before final row wrapping,
    // so a fresh cache from another width is not safe to reuse.
    if let Some(cached_lines) = cache.get_for_width(width) {
        out.extend_from_slice(cached_lines);
        return;
    }

    let fresh =
        super::markdown::render_markdown_with_control(text, width, bg, preserve_newlines, control);
    if control.cancelled() {
        return;
    }

    // Physical rows, and therefore their height, belong to the final layout pass.
    cache.store_for_width(fresh, width);
    if let Some(stored) = cache.get_for_width(width) {
        out.extend_from_slice(stored);
    }
}

pub(super) fn render_text_block_cached(
    block: &mut TextBlock,
    width: u16,
    bg: Option<Color>,
    preserve_newlines: bool,
    out: &mut Vec<Line<'static>>,
    control: &super::layout_control::LayoutControl<'_>,
) {
    render_text_cached(&block.text, &mut block.cache, width, bg, preserve_newlines, out, control);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{ChatMessage, MessageBlock, MessageRole, TextBlock};
    use pretty_assertions::assert_eq;

    fn make_text_message(role: MessageRole, text: &str) -> ChatMessage {
        ChatMessage::new(role, vec![MessageBlock::Text(TextBlock::from_complete(text))], None)
    }

    fn render_lines_to_strings(lines: &[Line<'static>]) -> Vec<String> {
        lines.iter().map(|line| line.to_string().trim_end().to_owned()).collect()
    }

    #[test]
    fn user_message_preserves_source_structure_through_cached_resizes() {
        let source = "first line\nsecond line\n\n```text\n  indented\nplain\n```\n\nfinal";
        let mut msg = make_text_message(MessageRole::User, source);
        for width in [80, 80, 28, 80] {
            let mut lines = Vec::new();
            render_message(&mut msg, width, &mut lines);
            let rows = lines.iter().map(ToString::to_string).collect::<Vec<_>>();
            let text = rows.join("\n");
            for expected in ["first line", "second line", "  indented", "plain", "final"] {
                assert!(text.contains(expected), "missing {expected:?} at width {width}: {text}");
            }
            let row_of = |needle: &str| {
                rows.iter()
                    .position(|row| row.contains(needle))
                    .unwrap_or_else(|| panic!("missing row {needle:?} at width {width}: {text}"))
            };
            let first = row_of("first line");
            let second = row_of("second line");
            let code = row_of("indented");
            let plain = row_of("plain");
            let final_row = row_of("final");
            assert!(first < second, "explicit source line break was lost: {rows:?}");
            assert!(second + 1 < code, "paragraph boundary before code was lost: {rows:?}");
            assert!(code < plain && plain < final_row, "source order changed: {rows:?}");
            assert!(text.contains("  indented"), "code indentation must survive");
            let MessageBlock::Text(block) = &msg.blocks[0] else {
                panic!("expected user text");
            };
            assert_eq!(block.text, source);
        }
    }

    #[test]
    fn markdown_table_rerenders_when_width_changes_in_both_directions() {
        let table = "| Key | Val |\n| --- | --- |\n| A | amberpinewood |\n| B | cobaltstream |\n";
        let mut msg = make_text_message(MessageRole::User, table);
        let mut wide_lines = Vec::new();
        render_message(&mut msg, 40, &mut wide_lines);
        let wide = render_lines_to_strings(&wide_lines);
        let mut narrow_lines = Vec::new();
        render_message(&mut msg, 20, &mut narrow_lines);
        let narrow = render_lines_to_strings(&narrow_lines);
        assert_ne!(narrow, wide, "resize must reflow the table");
        for (rows, width) in [(&wide, 40_u16), (&narrow, 20_u16)] {
            let joined = rows.join("\n");
            let compact = joined.chars().filter(|ch| ch.is_alphanumeric()).collect::<String>();
            assert!(
                compact.contains("KeyValAamberpinewoodBcobaltstream"),
                "table lost or reordered a header, row key, or value at width {width}: {joined}"
            );
            assert!(
                rows.iter().all(|row| {
                    unicode_width::UnicodeWidthStr::width(row.as_str()) <= usize::from(width)
                }),
                "a rendered row exceeded terminal width {width}: {rows:?}"
            );
        }
        let mut wide_again_lines = Vec::new();
        render_message(&mut msg, 40, &mut wide_again_lines);
        assert_eq!(
            render_lines_to_strings(&wide_again_lines),
            wide,
            "growing must restore wide layout"
        );
    }
}
