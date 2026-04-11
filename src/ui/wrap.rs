// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use ratatui::style::Style;
use ratatui::text::{Line, Span};
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
pub(crate) fn truncate_to_width(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if display_width(text) <= width {
        return text.to_owned();
    }

    let mut out = String::new();
    let mut used = 0usize;
    for grapheme in UnicodeSegmentation::graphemes(text, true) {
        let grapheme_width = display_width(grapheme);
        if used + grapheme_width > width {
            break;
        }
        out.push_str(grapheme);
        used += grapheme_width;
    }
    out
}

#[must_use]
pub(crate) fn take_prefix_by_width(text: &str, width: usize) -> (String, String) {
    if width == 0 || text.is_empty() {
        return (String::new(), text.to_owned());
    }

    let mut used = 0usize;
    let mut split_at = 0usize;
    for (idx, grapheme) in UnicodeSegmentation::grapheme_indices(text, true) {
        let grapheme_width = display_width(grapheme);
        if used + grapheme_width > width {
            break;
        }
        used += grapheme_width;
        split_at = idx + grapheme.len();
    }

    if split_at == 0 {
        return (String::new(), text.to_owned());
    }

    (text[..split_at].to_owned(), text[split_at..].to_owned())
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

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use ratatui::style::Modifier;

    #[test]
    fn take_prefix_by_width_handles_grapheme_clusters() {
        let text = "a👩‍💻b";
        let (chunk, rest) = take_prefix_by_width(text, 3);
        assert_eq!(chunk, "a👩‍💻");
        assert_eq!(rest, "b");
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
}
