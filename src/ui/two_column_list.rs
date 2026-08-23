// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use super::wrap::{StyledChunk, join_column_lines, wrap_styled_chunks, wrapped_line_count};
use ratatui::style::Style;
use ratatui::text::Line;

#[derive(Clone, Debug)]
pub(crate) struct TwoColumnItem {
    pub left: String,
    pub right: String,
    pub left_style: Style,
    pub right_style: Style,
}

#[must_use]
pub(crate) fn visible_item_count(
    items: &[TwoColumnItem],
    start: usize,
    available_lines: usize,
    left_width: usize,
    right_width: usize,
    spacer_rows: usize,
) -> usize {
    let mut used = 0usize;
    let mut count = 0usize;

    for item in items.iter().skip(start) {
        let item_height = wrapped_item_height(item, left_width, right_width).max(1);
        let spacer = if count == 0 { 0 } else { spacer_rows };
        if used + spacer + item_height > available_lines {
            break;
        }

        used += spacer + item_height;
        count += 1;
    }

    count.max(1)
}

#[must_use]
pub(crate) fn wrapped_item_height(
    item: &TwoColumnItem,
    left_width: usize,
    right_width: usize,
) -> usize {
    wrapped_line_count(&item.left, left_width)
        .max(wrapped_line_count(&item.right, right_width))
        .max(1)
}

#[must_use]
pub(crate) fn render_lines(
    items: &[TwoColumnItem],
    left_width: usize,
    right_width: usize,
    gap: usize,
    spacer_rows: usize,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    for (idx, item) in items.iter().enumerate() {
        if idx > 0 {
            for _ in 0..spacer_rows {
                lines.push(Line::default());
            }
        }

        let left_lines = wrap_styled_chunks(
            &[StyledChunk { text: item.left.clone(), style: item.left_style }],
            left_width,
        );
        let right_lines = wrap_styled_chunks(
            &[StyledChunk { text: item.right.clone(), style: item.right_style }],
            right_width,
        );
        lines.extend(join_column_lines(left_lines, right_lines, left_width, gap));
    }

    if lines.is_empty() {
        lines.push(Line::default());
    }
    lines
}
