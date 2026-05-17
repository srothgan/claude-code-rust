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
        &self.rows[range]
    }
}
