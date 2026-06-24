// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use ratatui::text::Line;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RenderedHistoryRows {
    pub(super) width: u16,
    rows: Vec<Line<'static>>,
}

impl RenderedHistoryRows {
    pub(super) fn new(width: u16, rows: Vec<Line<'static>>) -> Self {
        Self { width: width.max(1), rows }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub(super) fn len(&self) -> usize {
        self.rows.len()
    }

    pub(super) fn remaining_len(&self, next_row: usize) -> usize {
        self.rows.len().saturating_sub(next_row.min(self.rows.len()))
    }

    pub(super) fn slice(&self, range: std::ops::Range<usize>) -> &[Line<'static>] {
        let start = range.start.min(self.rows.len());
        let end = range.end.min(self.rows.len());
        if start > end { &[] } else { &self.rows[start..end] }
    }
}

#[cfg(test)]
mod tests {
    use super::RenderedHistoryRows;
    use ratatui::text::Line;

    #[test]
    fn slice_returns_empty_for_invalid_range_order() {
        let rows = RenderedHistoryRows::new(80, vec![Line::from("first"), Line::from("second")]);
        let start = 2;
        let end = 1;

        assert!(rows.slice(start..end).is_empty());
    }

    #[test]
    fn slice_clamps_range_end_to_available_rows() {
        let rows = RenderedHistoryRows::new(80, vec![Line::from("first"), Line::from("second")]);

        assert_eq!(rows.slice(1..5).len(), 1);
    }
}
