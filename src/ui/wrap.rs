// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

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
    if lines.is_empty() {
        return Vec::new();
    }
    if width == 0 {
        return vec![Line::default(); lines.len()];
    }

    let text = Text::from(lines.to_vec());
    let height = Paragraph::new(text.clone()).wrap(Wrap { trim: false }).line_count(width).max(1);
    let area = Rect::new(0, 0, width, u16::try_from(height).unwrap_or(u16::MAX));
    let mut buffer = Buffer::empty(area);
    Paragraph::new(text).wrap(Wrap { trim: false }).render(area, &mut buffer);

    (0..area.height).map(|row| buffer_row_to_line(&buffer, area, row)).collect()
}

#[must_use]
pub(crate) fn wrap_markdown_lines_to_physical_rows(
    lines: &[Line<'static>],
    width: u16,
) -> Vec<Line<'static>> {
    if lines.is_empty() {
        return Vec::new();
    }

    let mut rows = Vec::new();
    let mut in_fenced_code = false;
    for line in lines {
        let text = line_text(line);
        let starts_fence =
            text.trim_start().starts_with("```") || text.trim_start().starts_with("~~~");
        let in_code_line = in_fenced_code || starts_fence;
        rows.extend(wrap_markdown_line_to_physical_rows(line, width, in_code_line));
        if starts_fence {
            in_fenced_code = !in_fenced_code;
        }
    }
    rows
}

fn wrap_markdown_line_to_physical_rows(
    line: &Line<'static>,
    width: u16,
    in_fenced_code: bool,
) -> Vec<Line<'static>> {
    if in_fenced_code {
        return wrap_lines_to_physical_rows(std::slice::from_ref(line), width);
    }

    let Some(body_start) = markdown_list_body_start(&line_text(line)) else {
        return wrap_lines_to_physical_rows(std::slice::from_ref(line), width);
    };
    if width == 0 {
        return vec![Line::default()];
    }

    let (marker, body) = split_line_chunks_at_byte(line, body_start);
    let marker_width = marker.iter().map(|chunk| display_width(&chunk.text)).sum::<usize>();
    let width = usize::from(width);
    if marker_width == 0 || marker_width >= width {
        return wrap_lines_to_physical_rows(
            std::slice::from_ref(line),
            u16::try_from(width).unwrap_or(u16::MAX),
        );
    }

    let body_rows = wrap_styled_chunks(&body, width.saturating_sub(marker_width));
    let mut rows = Vec::with_capacity(body_rows.len().max(1));
    let mut body_rows = body_rows.into_iter();

    let mut first_spans =
        marker.into_iter().map(|chunk| Span::styled(chunk.text, chunk.style)).collect::<Vec<_>>();
    if let Some(first_body) = body_rows.next() {
        first_spans.extend(first_body.spans);
    }
    rows.push(Line::from(first_spans).style(line.style));

    let continuation_prefix = " ".repeat(marker_width);
    for body_row in body_rows {
        let mut spans = vec![Span::styled(continuation_prefix.clone(), line.style)];
        spans.extend(body_row.spans);
        rows.push(Line::from(spans).style(line.style));
    }
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

fn split_line_chunks_at_byte(
    line: &Line<'static>,
    split_at: usize,
) -> (Vec<StyledChunk>, Vec<StyledChunk>) {
    let mut left = Vec::new();
    let mut right = Vec::new();
    let mut seen = 0usize;

    for span in &line.spans {
        let text = span.content.as_ref();
        let span_start = seen;
        let span_end = seen + text.len();
        if span_end <= split_at {
            left.push(StyledChunk { text: text.to_owned(), style: span.style });
        } else if span_start >= split_at {
            right.push(StyledChunk { text: text.to_owned(), style: span.style });
        } else {
            let local_split = split_at - span_start;
            let (prefix, suffix) = text.split_at(local_split);
            if !prefix.is_empty() {
                left.push(StyledChunk { text: prefix.to_owned(), style: span.style });
            }
            if !suffix.is_empty() {
                right.push(StyledChunk { text: suffix.to_owned(), style: span.style });
            }
        }
        seen = span_end;
    }

    (left, right)
}

fn markdown_list_body_start(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut idx = bytes.iter().take_while(|byte| **byte == b' ').count();
    if idx >= bytes.len() {
        return None;
    }

    if matches!(bytes[idx], b'-' | b'*' | b'+')
        && bytes.get(idx + 1).is_some_and(|byte| *byte == b' ')
    {
        idx += 2;
        if task_list_marker_len(&bytes[idx..]).is_some() {
            idx += 4;
        }
        return Some(idx);
    }

    let digit_start = idx;
    while bytes.get(idx).is_some_and(u8::is_ascii_digit) {
        idx += 1;
    }
    if idx > digit_start
        && bytes.get(idx).is_some_and(|byte| *byte == b'.')
        && bytes.get(idx + 1).is_some_and(|byte| *byte == b' ')
    {
        return Some(idx + 2);
    }

    None
}

fn task_list_marker_len(bytes: &[u8]) -> Option<usize> {
    (bytes.len() >= 4
        && bytes[0] == b'['
        && matches!(bytes[1], b' ' | b'x' | b'X')
        && bytes[2] == b']'
        && bytes[3] == b' ')
        .then_some(4)
}

fn line_text(line: &Line<'_>) -> String {
    line.spans.iter().map(|span| span.content.as_ref()).collect()
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

    for x in 0..area.width {
        let Some(cell) = buf.cell((area.x.saturating_add(x), y)) else {
            continue;
        };
        let symbol = cell.symbol();
        if symbol.is_empty() {
            continue;
        }
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
    use super::*;
    use pretty_assertions::assert_eq;
    use ratatui::style::Modifier;

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
    fn markdown_ordered_list_wraps_with_hanging_indent() {
        let rows = wrap_markdown_lines_to_physical_rows(
            &[Line::from("  3. Keymap survey - mapping the keymap system")],
            32,
        );

        assert_eq!(
            rows.into_iter()
                .map(|line| line
                    .spans
                    .into_iter()
                    .map(|span| span.content.into_owned())
                    .collect::<String>())
                .collect::<Vec<_>>(),
            vec!["  3. Keymap survey - mapping the", "     keymap system"]
        );
    }

    #[test]
    fn markdown_task_list_wraps_under_task_marker() {
        let rows = wrap_markdown_lines_to_physical_rows(
            &[Line::from("  - [x] completed task with extra words")],
            24,
        );

        let text = rows
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.into_owned())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect::<Vec<_>>();

        assert_eq!(text, vec!["  - [x] completed task", "        with extra words"]);
    }

    #[test]
    fn markdown_list_wrapping_skips_fenced_code_lines() {
        let rows = wrap_markdown_lines_to_physical_rows(
            &[
                Line::from("```md"),
                Line::from("- code line that wraps without hanging indent"),
                Line::from("```"),
            ],
            18,
        );

        let text = rows
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.into_owned())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect::<Vec<_>>();

        assert_eq!(
            text,
            vec!["```md", "- code line that", "wraps without", "hanging indent", "```"]
        );
    }
}
