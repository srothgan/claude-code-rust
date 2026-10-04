// SPDX-License-Identifier: Apache-2.0
use super::theme;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Paragraph, Wrap},
};

pub(super) const fn selection_marker(selected: bool) -> &'static str {
    if selected { "›" } else { " " }
}

pub(super) fn selection_style(selected: bool) -> Style {
    if selected { Style::default().bg(theme::USER_MSG_BG) } else { Style::default() }
}

pub(super) fn title_style() -> Style {
    Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
}

pub(super) fn accent_style() -> Style {
    Style::default().fg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD)
}

pub(super) fn cursor_style() -> Style {
    title_style().fg(Color::Black).bg(theme::RUST_ORANGE)
}

pub(super) fn input_style() -> Style {
    selection_style(true).fg(Color::White)
}

pub(super) fn help_style() -> Style {
    Style::default().fg(theme::RUST_ORANGE)
}

pub(super) fn marker_span(selected: bool) -> Span<'static> {
    Span::styled(selection_marker(selected), help_style())
}

pub(super) fn overlay_line_style(selected: bool, focused: bool) -> Style {
    if selected && focused {
        accent_style()
    } else if selected {
        title_style()
    } else {
        Style::default().fg(Color::White)
    }
}

pub(super) fn selection_line(mut line: Line<'static>, selected: bool) -> Line<'static> {
    line.spans.insert(0, Span::raw(" "));
    line.spans.insert(0, marker_span(selected));
    line.style = line.style.patch(selection_style(selected));
    line
}

pub(super) fn position_counter(selected: usize, total: usize) -> Span<'static> {
    Span::styled(
        format!("{}/{}", if total == 0 { 0 } else { selected.min(total - 1) + 1 }, total),
        Style::default().fg(theme::DIM),
    )
}

pub(super) fn render_details(frame: &mut Frame, area: Rect, lines: Vec<Line<'static>>) {
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

pub(super) fn render_selection_blocks(
    frame: &mut Frame,
    area: Rect,
    blocks: Vec<Vec<Line<'static>>>,
    selected: usize,
    focused: bool,
) {
    let mut lines = Vec::new();
    let mut offset = 0usize;
    let mut scroll = 0;
    for (index, block) in blocks.into_iter().enumerate() {
        let block = block
            .into_iter()
            .enumerate()
            .map(|(row, mut line)| {
                let active = focused && index == selected;
                if row == 0 {
                    selection_line(line, active)
                } else {
                    line.spans.insert(0, Span::raw("  "));
                    line.style = line.style.patch(selection_style(active));
                    line
                }
            })
            .collect::<Vec<_>>();
        let height = usize::from(wrapped_height(block.clone(), area.width));
        if index == selected {
            // Keep the title visible when one entry exceeds the viewport height.
            scroll = selected_scroll(offset, height.min(usize::from(area.height)), area.height);
        }
        lines.extend(block);
        lines.push(Line::default());
        offset = offset.saturating_add(height).saturating_add(1);
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).scroll((scroll, 0)), area);
}

pub(super) fn render_message(frame: &mut Frame, area: Rect, title: &str, body: &str) {
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(title, title_style()),
            Line::default(),
            Line::styled(body, Style::default().fg(theme::DIM)),
        ])
        .wrap(Wrap { trim: false }),
        area,
    );
}

pub(super) fn section_heading(title: &str) -> Line<'static> {
    Line::styled(title.to_owned(), accent_style())
}

pub(super) fn detail_kv(key: &str, value: &str, value_color: Color) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{key}: "), Style::default().fg(theme::DIM)),
        Span::styled(value.to_owned(), Style::default().fg(value_color)),
    ])
}

pub(super) fn badge_span(label: &str, fg: Color, bg: Color) -> Span<'static> {
    Span::styled(format!(" {label} "), Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum TabStyle {
    Primary,
    Secondary,
}

pub(super) fn tab_line(
    labels: &[String],
    active: usize,
    width: u16,
    style: TabStyle,
) -> Line<'static> {
    let secondary = style == TabStyle::Secondary;
    let separator = if secondary { "  " } else { " | " };
    let label = |index: usize| {
        if secondary { format!(" {} ", labels[index]) } else { labels[index].clone() }
    };
    let mut start = 0;
    while start < active
        && (start..=active).map(|i| Span::raw(label(i)).width()).sum::<usize>()
            + (active - start) * separator.len()
            > usize::from(width)
    {
        start += 1;
    }
    let mut spans = Vec::new();
    for index in start..labels.len() {
        if index > start {
            spans.push(Span::styled(separator, Style::default().fg(theme::DIM)));
        }
        let style = if index == active {
            if secondary { cursor_style() } else { accent_style() }
        } else if secondary {
            title_style()
        } else {
            Style::default().fg(Color::White)
        };
        spans.push(Span::styled(label(index), style));
    }
    Line::from(spans)
}

pub(super) fn hint_text(hints: Vec<&str>, width: u16) -> String {
    hint_text_with_escape(hints, width, "Esc close")
}

pub(super) fn hint_text_with_escape(mut hints: Vec<&str>, width: u16, escape: &str) -> String {
    loop {
        let help = hints.iter().copied().chain([escape]).collect::<Vec<_>>().join(" | ");
        if wrapped_height(help.as_str(), width) <= 3 || hints.is_empty() {
            return help;
        }
        hints.pop();
    }
}

pub(super) fn selected_scroll(
    selected_start: usize,
    selected_height: usize,
    viewport_height: u16,
) -> u16 {
    if viewport_height == 0 {
        return 0;
    }
    u16::try_from(
        selected_start.saturating_add(selected_height).saturating_sub(usize::from(viewport_height)),
    )
    .unwrap_or(u16::MAX)
}

pub(super) fn wrapped_height<'a>(text: impl Into<Text<'a>>, width: u16) -> u16 {
    u16::try_from(Paragraph::new(text).wrap(Wrap { trim: false }).line_count(width.max(1)))
        .unwrap_or(u16::MAX)
        .max(1)
}
