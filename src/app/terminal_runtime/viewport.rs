// SPDX-License-Identifier: Apache-2.0

//! One viewport policy for live following, explicit reading, and reflow.

use crate::app::HistoryOutputId;
use crate::ui::inline_chat_rows::{LiveRowBoundaryKind, SerializedLiveRows};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct ChatViewport {
    reading: bool,
    anchor: Option<RowAnchor>,
    pending_scroll: isize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RowAnchor {
    id: HistoryOutputId,
    text_offset: usize,
    empty_rows: usize,
}

impl ChatViewport {
    pub(crate) const fn is_reading(&self) -> bool {
        self.reading
    }

    pub(crate) fn pause(&mut self) {
        self.reading = true;
    }

    pub(crate) fn resume(&mut self) {
        self.reading = false;
        self.anchor = None;
        self.pending_scroll = 0;
    }

    pub(crate) fn scroll(&mut self, rows: isize) {
        self.pause();
        self.pending_scroll = self.pending_scroll.saturating_add(rows);
    }

    pub(crate) fn start(&mut self, rows: &SerializedLiveRows, height: usize) -> usize {
        let last_start = rows.rows().len().saturating_sub(height);
        let start = self
            .anchor
            .as_ref()
            .and_then(|anchor| anchor.row(rows))
            // Retention can remove the anchor: stay with the oldest available history.
            .unwrap_or_else(|| if self.anchor.is_some() { 0 } else { last_start });
        let start = start.saturating_add_signed(self.pending_scroll).min(last_start);
        self.pending_scroll = 0;
        start
    }

    pub(crate) fn record(&mut self, rows: &SerializedLiveRows, start: usize) {
        self.anchor =
            rows.segments().iter().find(|segment| segment.end_row > start).and_then(|segment| {
                let label =
                    segment.ids.iter().find(|id| matches!(id, HistoryOutputId::AssistantLabel(_)));
                let id = if start == segment.start_row {
                    label.or_else(|| segment.ids.first())
                } else {
                    segment.ids.first()
                };
                id.map(|id| {
                    let (text_offset, empty_rows) = semantic_rows(rows, id)
                        .take_while(|(index, _, _)| *index < start)
                        .fold((0, 0), |(offset, blanks), (_, row, kind)| {
                            let length = content_len(row, kind);
                            (offset + length, if length == 0 { blanks + 1 } else { 0 })
                        });
                    RowAnchor { id: id.clone(), text_offset, empty_rows }
                })
            });
    }
}

// A yielding producer can give multiple segments the same semantic identity.
// Both recording and restoration must traverse that identity's complete content.
fn semantic_rows<'a>(
    rows: &'a SerializedLiveRows,
    id: &'a HistoryOutputId,
) -> impl Iterator<Item = (usize, &'a ratatui::text::Line<'static>, LiveRowBoundaryKind)> {
    rows.segments().iter().filter(move |segment| segment.ids.contains(id)).flat_map(
        move |segment| {
            // A labelled assistant starts with one presentation row.
            // Keep that row addressable without counting its timestamp as body content.
            let has_label =
                segment.ids.iter().any(|id| matches!(id, HistoryOutputId::AssistantLabel(_)));
            let is_label = matches!(id, HistoryOutputId::AssistantLabel(_));
            let range = segment.start_row..segment.end_row;
            let count = if is_label { 1 } else { range.len() };
            range
                .skip(usize::from(has_label && !is_label))
                .take(count)
                .map(move |index| (index, &rows.rows()[index], segment.kind))
        },
    )
}

impl RowAnchor {
    fn row(&self, rows: &SerializedLiveRows) -> Option<usize> {
        let mut offset = 0;
        let mut empty_rows = self.empty_rows;
        let mut last = None;
        for (index, row, kind) in semantic_rows(rows, &self.id) {
            last = Some(index);
            if offset == self.text_offset {
                if empty_rows == 0 {
                    return Some(index);
                }
                empty_rows -= 1;
                continue;
            }
            offset += content_len(row, kind);
            if offset > self.text_offset {
                return Some(index);
            }
        }
        last
    }
}

// Soft wrapping changes whitespace/indentation but keeps the rendered content
// stream. Anchor within that stream rather than within its physical rows.
fn content_len(row: &ratatui::text::Line<'_>, kind: LiveRowBoundaryKind) -> usize {
    use unicode_width::UnicodeWidthChar;
    // Role markers can appear when retention removes the preceding assistant.
    // Their gutter is presentation, not part of the semantic source position.
    // The serializer owns which segments use that gutter.
    let inset = if kind.uses_assistant_gutter() {
        let layout = crate::ui::message_layout::MessageLayout::new(
            u16::try_from(row.width()).unwrap_or(u16::MAX),
        );
        usize::from(layout.horizontal_padding + layout.marker_width)
    } else {
        0
    };
    let mut column = 0;
    row.spans
        .iter()
        .flat_map(|span| span.content.chars())
        .filter(|ch| {
            let in_content = column >= inset;
            column += ch.width().unwrap_or(0);
            in_content && !ch.is_whitespace()
        })
        .count()
}
