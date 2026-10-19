// SPDX-License-Identifier: Apache-2.0

use ratatui::style::Style;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    text::{Line, Span, Text},
    widgets::{Paragraph, Widget, Wrap},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Debug)]
pub(crate) struct StyledChunk {
    pub text: String,
    pub style: Style,
}

#[derive(Clone)]
struct StyledToken {
    text: String,
    style: Style,
    width: usize,
}

enum WrapToken {
    Text(StyledToken),
    Space(StyledToken),
    Newline,
}

#[must_use]
pub(crate) fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

#[must_use]
pub(crate) fn line_display_width(line: &Line<'_>) -> usize {
    line.spans.iter().map(|span| display_width(span.content.as_ref())).sum()
}

#[must_use]
pub(crate) fn wrap_plain(text: &str, width: usize) -> Vec<String> {
    wrap_styled_chunks(&[StyledChunk { text: text.to_owned(), style: Style::default() }], width)
        .into_iter()
        .map(|line| line.spans.into_iter().map(|span| span.content.into_owned()).collect())
        .collect()
}

#[must_use]
pub(crate) fn wrapped_line_count(text: &str, width: usize) -> usize {
    wrap_plain(text, width).len().max(1)
}

#[must_use]
pub(crate) fn wrap_lines_to_physical_rows(
    lines: &[Line<'static>],
    width: u16,
) -> Vec<Line<'static>> {
    wrap_lines_with_control(lines, width, &super::layout_control::LayoutControl::uninterrupted())
}

pub(super) fn wrap_lines_with_control(
    lines: &[Line<'static>],
    width: u16,
    control: &super::layout_control::LayoutControl<'_>,
) -> Vec<Line<'static>> {
    let mut rows = Vec::new();
    for line in lines {
        if control.cancelled() {
            return Vec::new();
        }
        let layout = WrappedLineLayout::new(line.clone(), width);
        let height = layout.height();
        rows.extend(layout.render(0..height, control));
        if control.cancelled() {
            return Vec::new();
        }
    }
    rows
}

/// Measure wrapping without rasterizing hidden rows. Ratatui remains the owner of
/// word boundaries, alignment and grapheme width for both full and windowed layout.
pub(super) struct WrappedLineLayout {
    paragraph: Paragraph<'static>,
    width: u16,
    height: u16,
}

impl WrappedLineLayout {
    pub(super) fn new(line: Line<'static>, width: u16) -> Self {
        let paragraph = Paragraph::new(Text::from(line)).wrap(Wrap { trim: false });
        let height = u16::try_from(paragraph.line_count(width).max(1)).unwrap_or(u16::MAX);
        Self { paragraph, width, height }
    }

    pub(super) fn height(&self) -> usize {
        usize::from(self.height)
    }

    pub(super) fn render(
        self,
        rows: std::ops::Range<usize>,
        control: &super::layout_control::LayoutControl<'_>,
    ) -> Vec<Line<'static>> {
        let start = rows.start.min(self.height());
        let end = rows.end.min(self.height());
        if start >= end || control.cancelled() {
            return Vec::new();
        }
        if self.width == 0 {
            return vec![Line::default(); end - start];
        }
        // Both bounds are limited to the measured u16 height above.
        let height = u16::try_from(end - start).unwrap_or(self.height);
        let offset = u16::try_from(start).unwrap_or(self.height);
        let area = Rect::new(0, 0, self.width, height);
        let mut buffer = Buffer::empty(area);
        self.paragraph.scroll((offset, 0)).render(area, &mut buffer);
        let mut rendered = Vec::with_capacity(usize::from(height));
        for row in 0..height {
            if control.cancelled() {
                return Vec::new();
            }
            rendered.push(buffer_row_to_line(&buffer, area, row));
        }
        rendered
    }
}

/// Splits a line at grapheme boundaries into rows of at most `width` cells, keeping every
/// character, including leading and trailing whitespace, without padding rows.
#[must_use]
pub(crate) fn split_line_at_width(line: Line<'static>, width: usize) -> Vec<Line<'static>> {
    if width == 0 || line_display_width(&line) <= width {
        return vec![line];
    }
    let style = line.style;
    let mut rows = Vec::new();
    let mut spans = Vec::new();
    let mut row_width = 0;
    for span in line.spans {
        let mut text = String::new();
        for grapheme in span.content.graphemes(true) {
            let grapheme_width = display_width(grapheme);
            if row_width > 0 && row_width + grapheme_width > width {
                push_styled_text(&mut spans, &std::mem::take(&mut text), span.style);
                rows.push(Line::from(std::mem::take(&mut spans)).style(style));
                row_width = 0;
            }
            text.push_str(grapheme);
            row_width += grapheme_width;
        }
        push_styled_text(&mut spans, &text, span.style);
    }
    rows.push(Line::from(spans).style(style));
    rows
}

#[must_use]
pub(crate) fn wrap_styled_chunks(chunks: &[StyledChunk], width: usize) -> Vec<Line<'static>> {
    if width == 0 || chunks.is_empty() {
        return vec![Line::default()];
    }

    let tokens = tokenize_chunks(chunks);
    let mut lines = Vec::new();
    let mut spans = Vec::new();
    let mut line_width = 0usize;
    let mut pending_spaces = Vec::<StyledToken>::new();

    for token in tokens {
        match token {
            WrapToken::Newline => {
                finish_wrapped_line(&mut lines, &mut spans, &mut line_width);
                pending_spaces.clear();
            }
            WrapToken::Space(space) => {
                if line_width > 0 {
                    pending_spaces.push(space);
                }
            }
            WrapToken::Text(text) => {
                let pending_width: usize = pending_spaces.iter().map(|space| space.width).sum();
                if line_width > 0 && line_width + pending_width + text.width > width {
                    finish_wrapped_line(&mut lines, &mut spans, &mut line_width);
                    pending_spaces.clear();
                }

                if line_width > 0 {
                    for space in pending_spaces.drain(..) {
                        push_styled_text(&mut spans, &space.text, space.style);
                        line_width += space.width;
                    }
                }

                if text.width <= width.saturating_sub(line_width) {
                    push_styled_text(&mut spans, &text.text, text.style);
                    line_width += text.width;
                    continue;
                }

                wrap_long_token(&text, width, &mut lines, &mut spans, &mut line_width);
            }
        }
    }

    finish_wrapped_line(&mut lines, &mut spans, &mut line_width);
    if lines.is_empty() {
        lines.push(Line::default());
    }
    lines
}

#[must_use]
pub(crate) fn wrap_styled_chunks_with_hanging_prefix(
    prefix: &[StyledChunk],
    body: &[StyledChunk],
    width: usize,
    line_style: Style,
) -> Vec<Line<'static>> {
    if width == 0 {
        return vec![Line::default()];
    }

    let prefix_width = prefix.iter().map(|chunk| display_width(&chunk.text)).sum::<usize>();
    if prefix_width == 0 {
        return wrap_styled_chunks(body, width)
            .into_iter()
            .map(|line| line.style(line_style))
            .collect();
    }
    if prefix_width >= width {
        let mut combined = Vec::with_capacity(prefix.len() + body.len());
        combined.extend_from_slice(prefix);
        combined.extend_from_slice(body);
        return wrap_styled_chunks(&combined, width)
            .into_iter()
            .map(|line| line.style(line_style))
            .collect();
    }

    let mut body_rows = wrap_styled_chunks(body, width - prefix_width).into_iter();
    let mut first_spans = prefix
        .iter()
        .map(|chunk| Span::styled(chunk.text.clone(), chunk.style))
        .collect::<Vec<_>>();
    if let Some(first_body) = body_rows.next() {
        first_spans.extend(first_body.spans);
    }

    let mut rows = vec![Line::from(first_spans).style(line_style)];
    let continuation_prefix = " ".repeat(prefix_width);
    rows.extend(body_rows.map(|body_row| {
        let mut spans = vec![Span::styled(continuation_prefix.clone(), line_style)];
        spans.extend(body_row.spans);
        Line::from(spans).style(line_style)
    }));
    rows
}

#[must_use]
pub(crate) fn pad_line_to_width(
    mut line: Line<'static>,
    width: usize,
    padding_style: Style,
) -> Line<'static> {
    let padding = width.saturating_sub(line_display_width(&line));
    if padding > 0 {
        line.spans.push(Span::styled(" ".repeat(padding), padding_style));
    }
    line
}

#[must_use]
pub(crate) fn blank_line(width: usize, style: Style) -> Line<'static> {
    Line::from(Span::styled(" ".repeat(width), style))
}

#[must_use]
pub(crate) fn join_column_lines(
    mut left_lines: Vec<Line<'static>>,
    mut right_lines: Vec<Line<'static>>,
    left_width: usize,
    gap: usize,
) -> Vec<Line<'static>> {
    let row_height = left_lines.len().max(right_lines.len()).max(1);
    while left_lines.len() < row_height {
        left_lines.push(blank_line(left_width, Style::default()));
    }
    while right_lines.len() < row_height {
        right_lines.push(Line::default());
    }

    let mut lines = Vec::with_capacity(row_height);
    for idx in 0..row_height {
        let mut line =
            pad_line_to_width(std::mem::take(&mut left_lines[idx]), left_width, Style::default());
        if gap > 0 {
            line.spans.push(Span::styled(" ".repeat(gap), Style::default()));
        }
        line.spans.extend(std::mem::take(&mut right_lines[idx].spans));
        lines.push(line);
    }

    lines
}

fn tokenize_chunks(chunks: &[StyledChunk]) -> Vec<WrapToken> {
    let mut tokens = Vec::new();

    for chunk in chunks {
        let mut current = String::new();
        let mut is_space = None;

        let flush_current = |tokens: &mut Vec<WrapToken>,
                             current: &mut String,
                             is_space: &mut Option<bool>,
                             style: Style| {
            if current.is_empty() {
                return;
            }
            let text = std::mem::take(current);
            let width = display_width(text.as_str());
            let token = StyledToken { text, style, width };
            if is_space.unwrap_or(false) {
                tokens.push(WrapToken::Space(token));
            } else {
                tokens.push(WrapToken::Text(token));
            }
        };

        for grapheme in UnicodeSegmentation::graphemes(chunk.text.as_str(), true) {
            if grapheme == "\n" {
                flush_current(&mut tokens, &mut current, &mut is_space, chunk.style);
                is_space = None;
                tokens.push(WrapToken::Newline);
                continue;
            }

            let grapheme_is_space =
                grapheme.chars().all(char::is_whitespace) && grapheme.chars().all(|ch| ch != '\n');
            if is_space.is_some_and(|value| value != grapheme_is_space) {
                flush_current(&mut tokens, &mut current, &mut is_space, chunk.style);
            }

            is_space = Some(grapheme_is_space);
            current.push_str(grapheme);
        }

        flush_current(&mut tokens, &mut current, &mut is_space, chunk.style);
    }

    tokens
}

fn wrap_long_token(
    token: &StyledToken,
    width: usize,
    lines: &mut Vec<Line<'static>>,
    spans: &mut Vec<Span<'static>>,
    line_width: &mut usize,
) {
    let mut segment = String::new();
    let mut segment_width = 0usize;

    for grapheme in UnicodeSegmentation::graphemes(token.text.as_str(), true) {
        let grapheme_width = display_width(grapheme);
        if *line_width > 0 && *line_width + segment_width + grapheme_width > width {
            if !segment.is_empty() {
                push_styled_text(spans, &segment, token.style);
                *line_width += segment_width;
                segment.clear();
                segment_width = 0;
            }
            finish_wrapped_line(lines, spans, line_width);
        }

        if segment_width + grapheme_width > width && !segment.is_empty() {
            push_styled_text(spans, &segment, token.style);
            *line_width += segment_width;
            segment.clear();
            segment_width = 0;
            finish_wrapped_line(lines, spans, line_width);
        }

        segment.push_str(grapheme);
        segment_width += grapheme_width;
    }

    if !segment.is_empty() {
        push_styled_text(spans, &segment, token.style);
        *line_width += segment_width;
    }
}

fn finish_wrapped_line(
    lines: &mut Vec<Line<'static>>,
    spans: &mut Vec<Span<'static>>,
    line_width: &mut usize,
) {
    lines.push(Line::from(std::mem::take(spans)));
    *line_width = 0;
}

fn push_styled_text(spans: &mut Vec<Span<'static>>, text: &str, style: Style) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = spans.last_mut()
        && last.style == style
    {
        last.content.to_mut().push_str(text);
        return;
    }
    spans.push(Span::styled(text.to_owned(), style));
}

fn buffer_row_to_line(buf: &Buffer, area: Rect, row: u16) -> Line<'static> {
    let y = area.y.saturating_add(row);
    let mut spans = Vec::new();
    let mut current_style = None;
    let mut current_text = String::new();
    let mut continuation_cells = 0;

    for x in 0..area.width {
        if continuation_cells > 0 {
            continuation_cells -= 1;
            continue;
        }
        let Some(cell) = buf.cell((area.x.saturating_add(x), y)) else {
            continue;
        };
        let symbol = cell.symbol();
        if symbol.is_empty() {
            continue;
        }
        // A wide grapheme already accounts for its trailing buffer cells.
        continuation_cells = UnicodeWidthStr::width(symbol).saturating_sub(1);
        let style = cell.style();
        match current_style {
            Some(existing) if existing == style => current_text.push_str(symbol),
            Some(existing) => {
                spans.push(Span::styled(std::mem::take(&mut current_text), existing));
                current_text.push_str(symbol);
                current_style = Some(style);
            }
            None => {
                current_text.push_str(symbol);
                current_style = Some(style);
            }
        }
    }

    if let Some(style) = current_style {
        spans.push(Span::styled(current_text, style));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    fn line_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|span| span.content.as_ref()).collect()
    }
    use super::*;
    use pretty_assertions::assert_eq;
    use ratatui::style::Modifier;

    #[test]
    fn windowed_rows_match_the_selected_paragraph_cells() {
        let fixtures = [
            Line::from("word boundaries  and trailing spaces   ".repeat(20)),
            Line::from(vec![
                Span::raw("  日本語👩‍💻e\u{301} ".repeat(20)),
                Span::styled("colored tail", Style::new().fg(ratatui::style::Color::Red)),
            ])
            .style(Style::new().bg(ratatui::style::Color::Blue)),
            Line::from("right aligned words ".repeat(20)).right_aligned(),
            Line::from("centered words ".repeat(20)).centered(),
            Line::from("unbroken".repeat(100)),
            Line::default(),
        ];
        for line in fixtures {
            for width in [1, 4, 13, 60] {
                let paragraph = Paragraph::new(line.clone()).wrap(Wrap { trim: false });
                let height = u16::try_from(paragraph.line_count(width).max(1)).unwrap();
                let area = Rect::new(0, 0, width, height);
                let mut expected = Buffer::empty(area);
                paragraph.render(area, &mut expected);
                for start in [0, height / 2, height.saturating_sub(3)] {
                    let end = height.min(start + 3);
                    let layout = WrappedLineLayout::new(line.clone(), width);
                    let rows = layout.render(
                        usize::from(start)..usize::from(end),
                        &super::super::layout_control::LayoutControl::uninterrupted(),
                    );
                    let viewport = Rect::new(0, 0, width, end - start);
                    let mut actual = Buffer::empty(viewport);
                    Paragraph::new(rows).render(viewport, &mut actual);
                    for y in 0..viewport.height {
                        for x in 0..width {
                            assert_eq!(
                                actual.cell((x, y)),
                                expected.cell((x, start + y)),
                                "width={width}, start={start}, cell=({x}, {y})"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn windowed_rows_handle_zero_width_and_empty_or_cancelled_windows() {
        use super::super::layout_control::LayoutControl;
        let control = LayoutControl::uninterrupted();
        let layout = WrappedLineLayout::new(Line::from("text"), 0);
        assert_eq!(layout.height(), 1);
        assert_eq!(layout.render(0..1, &control), vec![Line::default()]);
        let layout = WrappedLineLayout::new(Line::from("text"), 2);
        assert!(layout.render(2..2, &control).is_empty());
        let layout = WrappedLineLayout::new(Line::from("text"), 2);
        control.cancel();
        assert!(layout.render(0..2, &control).is_empty());
    }

    #[test]
    fn physical_rows_match_paragraph_cells_across_logical_lines() {
        let lines = vec![
            Line::from(vec![
                Span::raw("  alpha "),
                Span::styled("日本語👩‍💻beta", Style::new().add_modifier(Modifier::BOLD)),
            ]),
            Line::default(),
            Line::from("right aligned words").right_aligned(),
            Line::from(" trailing spaces  ").style(Style::new().bg(ratatui::style::Color::Blue)),
        ];
        for width in [1, 4, 13, 60] {
            let paragraph = Paragraph::new(lines.clone()).wrap(Wrap { trim: false });
            let height = u16::try_from(paragraph.line_count(width)).unwrap();
            let area = Rect::new(0, 0, width, height);
            let mut expected = Buffer::empty(area);
            paragraph.render(area, &mut expected);
            let mut actual = Buffer::empty(area);
            Paragraph::new(wrap_lines_to_physical_rows(&lines, width)).render(area, &mut actual);
            assert_eq!(actual, expected, "width={width}");
        }
    }

    #[test]
    fn split_line_at_width_keeps_whitespace_styles_and_wide_graphemes_unpadded() {
        let bold = Style::default().add_modifier(Modifier::BOLD);
        let line = Line::from(vec![Span::raw("  ab"), Span::styled("你好 x", bold)]);

        let rows = split_line_at_width(line, 5);

        assert_eq!(rows.iter().map(line_text).collect::<Vec<_>>(), ["  ab", "你好 ", "x"]);
        assert_eq!(rows[1].spans[0].style, bold);
        assert_eq!(rows[2].spans[0].style, bold);
    }

    #[test]
    fn wrap_plain_preserves_explicit_newlines() {
        assert_eq!(wrap_plain("alpha\nbeta", 16), vec!["alpha".to_owned(), "beta".to_owned()]);
    }

    #[test]
    fn wrap_plain_handles_cjk_width() {
        assert_eq!(wrap_plain("你好 世界", 4), vec!["你好".to_owned(), "世界".to_owned()]);
    }

    #[test]
    fn wrap_plain_wraps_long_emoji_graphemes() {
        assert_eq!(wrap_plain("👩‍💻👩‍💻👩‍💻", 4), vec!["👩‍💻👩‍💻".to_owned(), "👩‍💻".to_owned()]);
    }

    #[test]
    fn physical_rows_preserve_wide_graphemes_without_adding_placeholder_spaces() {
        let style = Style::default().add_modifier(Modifier::ITALIC);
        let rows =
            wrap_lines_to_physical_rows(&[Line::from(Span::styled("日本語 😀 end", style))], 16);
        assert_eq!(rows.len(), 1);
        assert_eq!(line_text(&rows[0]).trim_end(), "日本語 😀 end");
        assert_eq!(rows[0].width(), 16);
        assert!(rows[0].spans[0].style.add_modifier.contains(Modifier::ITALIC));
        let wrapped = wrap_lines_to_physical_rows(&[Line::from("日本語😀日本語")], 6);
        assert!(wrapped.iter().all(|row| row.width() <= 6));
        assert_eq!(
            wrapped.iter().map(|row| line_text(row).trim_end().to_owned()).collect::<String>(),
            "日本語😀日本語"
        );
    }

    #[test]
    fn wrap_styled_chunks_preserves_styles() {
        let lines = wrap_styled_chunks(
            &[StyledChunk {
                text: "bold text".to_owned(),
                style: Style::default().add_modifier(Modifier::BOLD),
            }],
            32,
        );
        assert!(lines[0].spans[0].style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn hanging_prefix_wraps_continuations_below_the_body() {
        let rows = wrap_styled_chunks_with_hanging_prefix(
            &[StyledChunk { text: "Tips: ".to_owned(), style: Style::default() }],
            &[StyledChunk { text: "alpha beta gamma delta".to_owned(), style: Style::default() }],
            17,
            Style::default(),
        );

        assert_eq!(
            rows.into_iter().map(|line| line_text(&line)).collect::<Vec<_>>(),
            vec!["Tips: alpha beta", "      gamma delta"]
        );
    }

    #[test]
    fn joined_columns_preserve_the_left_offset_on_continuation_rows() {
        let rows = join_column_lines(
            vec![Line::from("XX")],
            vec![Line::from("one"), Line::from("two")],
            2,
            1,
        );

        assert_eq!(
            rows.into_iter().map(|line| line_text(&line)).collect::<Vec<_>>(),
            vec!["XX one", "   two"]
        );
    }
}
