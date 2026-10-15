// SPDX-License-Identifier: Apache-2.0

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use super::{theme, wrap};

pub(crate) const VERTICAL_PADDING: u16 = 1;
const HORIZONTAL_PADDING: u16 = 1;
const MARKER_WIDTH: u16 = 2;

/// Shared geometry for the composer and transcript. Tiny terminals keep a text cell.
#[derive(Clone, Copy)]
pub(crate) struct MessageLayout {
    pub horizontal_padding: u16,
    pub marker_width: u16,
    pub content_width: u16,
}

impl MessageLayout {
    pub(crate) const fn new(width: u16) -> Self {
        let horizontal_padding = if width >= 5 { HORIZONTAL_PADDING } else { 0 };
        let marker_width = if width >= 3 { MARKER_WIDTH } else { 0 };
        Self {
            horizontal_padding,
            marker_width,
            content_width: width.saturating_sub(horizontal_padding * 2 + marker_width),
        }
    }

    pub(crate) fn row(
        self,
        mut content: Line<'static>,
        marker: Option<(&str, Color)>,
        background: Option<Color>,
    ) -> Line<'static> {
        let style = background.map_or(Style::default(), |color| Style::default().bg(color));
        let mut spans = vec![Span::styled(" ".repeat(usize::from(self.horizontal_padding)), style)];
        if self.marker_width > 0 {
            let (marker, color) = marker.unwrap_or((" ", theme::DIM));
            spans.push(Span::styled(format!("{marker} "), style.fg(color)));
        }
        if let Some(color) = background {
            content.style = content.style.bg(color);
            for span in &mut content.spans {
                span.style = span.style.bg(color);
            }
        }
        spans.extend(content.spans);
        let row = Line::from(spans).style(content.style.patch(style));
        let width = self.horizontal_padding * 2 + self.marker_width + self.content_width;
        wrap::pad_line_to_width(row, usize::from(width), style)
    }
}

pub(crate) fn frame_rows(
    mut content: Vec<Line<'static>>,
    width: u16,
    marker: Option<(&str, Color)>,
    background: Option<Color>,
    padding: (bool, bool),
    full_width_rows: &[std::ops::Range<usize>],
) -> Vec<Line<'static>> {
    if width == 0 {
        return Vec::new();
    }
    if content.is_empty() {
        content.push(Line::default());
    }
    let first = content
        .iter()
        .position(|line| line.spans.iter().any(|span| !span.content.trim().is_empty()))
        .unwrap_or(0);
    let layout = MessageLayout::new(width);
    let blank = || layout.row(Line::default(), None, background);
    let mut rows = Vec::with_capacity(content.len() + 2);
    if padding.0 {
        rows.extend(std::iter::repeat_with(blank).take(usize::from(VERTICAL_PADDING)));
    }
    rows.extend(content.into_iter().enumerate().map(|(index, line)| {
        if full_width_rows.iter().any(|range| range.contains(&index)) {
            line
        } else {
            layout.row(line, if index == first { marker } else { None }, background)
        }
    }));
    if padding.1 {
        rows.extend(std::iter::repeat_with(blank).take(usize::from(VERTICAL_PADDING)));
    }
    rows
}

pub(crate) fn timestamp_row(
    timestamp: &str,
    width: u16,
    background: Option<Color>,
) -> Line<'static> {
    let layout = MessageLayout::new(width);
    let text = wrap::wrap_plain(timestamp, usize::from(layout.content_width))
        .into_iter()
        .next()
        .unwrap_or_default();
    let content = Line::from(Span::styled(text, Style::default().fg(theme::DIM)));
    layout.row(content, None, background)
}
