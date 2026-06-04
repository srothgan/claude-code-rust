use crate::ui::theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthChar;

pub(super) fn text_input_line(draft: &str, cursor: usize, placeholder: &str) -> Line<'static> {
    let cursor_style =
        Style::default().fg(Color::Black).bg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD);
    let text_style = Style::default().fg(Color::White);
    let placeholder_style = Style::default().fg(theme::DIM);

    if draft.is_empty() {
        return Line::from(vec![
            Span::styled(" ".to_owned(), cursor_style),
            Span::styled(placeholder.to_owned(), placeholder_style),
        ]);
    }

    let cursor = cursor.min(draft.chars().count());
    let chars = draft.chars().collect::<Vec<_>>();
    let prefix = chars[..cursor].iter().collect::<String>();
    let mut spans = Vec::new();

    if !prefix.is_empty() {
        spans.push(Span::styled(prefix, text_style));
    }

    if cursor < chars.len() {
        spans.push(Span::styled(chars[cursor].to_string(), cursor_style));
        let suffix = chars[cursor + 1..].iter().collect::<String>();
        if !suffix.is_empty() {
            spans.push(Span::styled(suffix, text_style));
        }
    } else {
        spans.push(Span::styled(" ".to_owned(), cursor_style));
    }

    Line::from(spans)
}

pub(super) fn render_text_input_field(
    frame: &mut Frame,
    area: Rect,
    draft: &str,
    cursor: usize,
    placeholder: &str,
) {
    let content_width = area.width.saturating_sub(2);
    let content = text_input_line_for_width(draft, cursor, placeholder, content_width);
    let mut spans = Vec::with_capacity(content.spans.len().saturating_add(2));
    spans.push(Span::styled(" ", Style::default().bg(theme::USER_MSG_BG)));
    spans.extend(content.spans);
    spans.push(Span::styled(" ", Style::default().bg(theme::USER_MSG_BG)));
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(theme::USER_MSG_BG)),
        area,
    );
}

fn text_input_line_for_width(
    draft: &str,
    cursor: usize,
    placeholder: &str,
    width: u16,
) -> Line<'static> {
    if width == 0 {
        return Line::from(Vec::<Span<'static>>::new());
    }
    if draft.is_empty() {
        return text_input_line(draft, cursor, placeholder);
    }

    let cursor_style =
        Style::default().fg(Color::Black).bg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD);
    let text_style = Style::default().fg(Color::White);
    let overflow_style = Style::default().fg(theme::DIM);
    let chars = draft.chars().collect::<Vec<_>>();
    let cursor = cursor.min(chars.len());
    let width = usize::from(width);
    let cursor_width = chars.get(cursor).map_or(1, |ch| char_width(*ch));

    let mut start = cursor;
    let mut used = cursor_width;
    while start > 0 {
        let previous_width = char_width(chars[start - 1]);
        if used + previous_width > width {
            break;
        }
        used += previous_width;
        start -= 1;
    }
    let left_overflow = start > 0;
    if left_overflow && width > 1 {
        start = cursor;
        used = cursor_width + 1;
        while start > 0 {
            let previous_width = char_width(chars[start - 1]);
            if used + previous_width > width {
                break;
            }
            used += previous_width;
            start -= 1;
        }
    }

    let right_indicator_width = usize::from(width > 1);
    let mut end = cursor + usize::from(cursor < chars.len());
    let mut visible_width = line_slice_width(&chars[start..end]) + usize::from(left_overflow);
    while end < chars.len() {
        let next_width = char_width(chars[end]);
        let will_overflow_right = end + 1 < chars.len();
        let reserved = usize::from(will_overflow_right) * right_indicator_width;
        if visible_width + next_width + reserved > width {
            break;
        }
        visible_width += next_width;
        end += 1;
    }
    let right_overflow = end < chars.len();

    let mut spans = Vec::new();
    if left_overflow {
        spans.push(Span::styled("<".to_owned(), overflow_style));
    }
    let prefix = chars[start..cursor].iter().collect::<String>();
    if !prefix.is_empty() {
        spans.push(Span::styled(prefix, text_style));
    }
    if cursor < chars.len() {
        spans.push(Span::styled(chars[cursor].to_string(), cursor_style));
        if cursor + 1 < end {
            spans.push(Span::styled(chars[cursor + 1..end].iter().collect::<String>(), text_style));
        }
    } else {
        spans.push(Span::styled(" ".to_owned(), cursor_style));
    }
    if right_overflow {
        spans.push(Span::styled(">".to_owned(), overflow_style));
    }

    Line::from(spans)
}

fn char_width(ch: char) -> usize {
    UnicodeWidthChar::width(ch).unwrap_or(0).max(1)
}

fn line_slice_width(chars: &[char]) -> usize {
    chars.iter().map(|ch| char_width(*ch)).sum()
}

pub(super) fn add_marketplace_example_lines() -> Vec<Line<'static>> {
    let dim = Style::default().fg(theme::DIM);
    vec![
        Line::from(Span::styled("Examples:", dim.add_modifier(Modifier::BOLD))),
        Line::from(Span::styled("  - owner/repo (GitHub)", dim)),
        Line::from(Span::styled("  - git@github.com:owner/repo.git (SSH)", dim)),
        Line::from(Span::styled("  - https://example.com/marketplace.json", dim)),
        Line::from(Span::styled("  - ./path/to/marketplace", dim)),
    ]
}
