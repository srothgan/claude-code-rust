// SPDX-License-Identifier: Apache-2.0
use std::collections::BTreeSet;

use ratatui::text::Line;

use crate::app::HistoryOutputId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SerializedLiveRows {
    rows: Vec<Line<'static>>,
    segments: Vec<LiveRowSegment>,
}

impl SerializedLiveRows {
    pub(crate) fn new(rows: Vec<Line<'static>>, segments: Vec<LiveRowSegment>) -> Self {
        Self { rows, segments }
    }

    #[cfg(test)]
    pub(crate) fn from_parts_for_test(
        rows: Vec<Line<'static>>,
        segments: Vec<LiveRowSegment>,
    ) -> Self {
        Self { rows, segments }
    }

    pub(crate) fn rows(&self) -> &[Line<'static>] {
        &self.rows
    }

    pub(crate) fn segments(&self) -> &[LiveRowSegment] {
        &self.segments
    }

    pub(crate) fn segment_rows(&self, segment: &LiveRowSegment) -> Option<&[Line<'static>]> {
        if segment.start_row > segment.end_row || segment.end_row > self.rows.len() {
            return None;
        }
        Some(&self.rows[segment.start_row..segment.end_row])
    }

    pub(crate) fn rows_excluding_ids(
        &self,
        excluded_ids: &BTreeSet<HistoryOutputId>,
    ) -> Vec<Line<'static>> {
        let mut rows = Vec::new();
        for segment in &self.segments {
            if segment.ids.iter().all(|id| excluded_ids.contains(id)) {
                continue;
            }
            let Some(segment_rows) = self.segment_rows(segment) else {
                continue;
            };
            rows.extend(segment_rows.iter().cloned());
        }
        rows
    }

    pub(crate) fn stable_row_count(&self) -> usize {
        self.segments
            .iter()
            .find(|segment| !segment.commit_ready)
            .map_or(self.rows.len(), |segment| segment.start_row.min(self.rows.len()))
    }

    pub(crate) fn first_mutable_boundary_kind(&self) -> Option<LiveRowBoundaryKind> {
        self.segments.iter().find(|segment| !segment.commit_ready).map(|segment| segment.kind)
    }

    pub(crate) fn first_mutable_boundary_start(&self) -> Option<usize> {
        self.segments
            .iter()
            .find(|segment| !segment.commit_ready)
            .map(|segment| segment.start_row.min(self.rows.len()))
    }

    pub(crate) fn first_mutable_boundary_msg_idx(&self) -> Option<usize> {
        self.segments.iter().find(|segment| !segment.commit_ready).map(|segment| segment.msg_idx)
    }

    pub(crate) fn first_mutable_boundary_block_idx(&self) -> Option<usize> {
        self.segments
            .iter()
            .find(|segment| !segment.commit_ready)
            .and_then(|segment| segment.block_idx)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiveRowSegment {
    pub(crate) ids: Vec<HistoryOutputId>,
    pub(crate) msg_idx: usize,
    pub(crate) block_idx: Option<usize>,
    pub(crate) kind: LiveRowBoundaryKind,
    pub(crate) start_row: usize,
    pub(crate) end_row: usize,
    pub(crate) commit_ready: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiveRowBoundary {
    pub(crate) ids: Vec<HistoryOutputId>,
    pub(crate) msg_idx: usize,
    pub(crate) block_idx: Option<usize>,
    pub(crate) kind: LiveRowBoundaryKind,
    pub(crate) start_row: usize,
    pub(crate) commit_ready: bool,
}

impl LiveRowBoundary {
    pub(crate) fn shifted(mut self, offset: usize) -> Self {
        self.start_row = self.start_row.saturating_add(offset);
        self
    }

    fn into_segment(self, end_row: usize) -> Option<LiveRowSegment> {
        (self.start_row < end_row).then_some(LiveRowSegment {
            ids: self.ids,
            msg_idx: self.msg_idx,
            block_idx: self.block_idx,
            kind: self.kind,
            start_row: self.start_row,
            end_row,
            commit_ready: self.commit_ready,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LiveRowBoundaryKind {
    Message,
    AssistantLabel,
    AssistantText,
    AssistantNotice,
    AssistantTool,
    AssistantIndicator,
}

pub(crate) fn ids_are_excluded(
    ids: &[HistoryOutputId],
    excluded_ids: &BTreeSet<HistoryOutputId>,
) -> bool {
    !ids.is_empty() && ids.iter().all(|id| excluded_ids.contains(id))
}

pub(crate) fn live_boundaries_to_segments(
    mut boundaries: Vec<LiveRowBoundary>,
    row_count: usize,
) -> Vec<LiveRowSegment> {
    boundaries.sort_by_key(|boundary| boundary.start_row);
    let mut segments = Vec::with_capacity(boundaries.len());
    for idx in 0..boundaries.len() {
        let end_row =
            boundaries.get(idx + 1).map_or(row_count, |next| next.start_row).min(row_count);
        if let Some(segment) = boundaries[idx].clone().into_segment(end_row) {
            segments.push(segment);
        }
    }
    segments
}
