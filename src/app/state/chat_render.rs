// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use crate::app::handoff::types::TranscriptEntry;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ChatRenderState {
    pub terminal_width: u16,
    pub terminal_height: u16,
    pub line_wrap_disabled: bool,
    pub composer: ComposerRenderState,
    pub live_region: LiveRegionRenderState,
    pub transcript: TranscriptRenderState,
}

impl ChatRenderState {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn set_terminal_size(&mut self, width: u16, height: u16) {
        self.terminal_width = width;
        self.terminal_height = height;
    }

    pub fn clear_measurements(&mut self) {
        self.composer = ComposerRenderState::default();
        self.live_region.total_rows = 0;
        self.live_region.hidden_rows_above = 0;
        self.live_region.viewport_height = 0;
        self.live_region.last_rendered_rows = 0;
    }

    pub fn invalidate_live_anchor(&mut self) {
        self.live_region.anchor_valid = false;
        self.live_region.total_rows = 0;
        self.live_region.hidden_rows_above = 0;
        self.live_region.viewport_height = 0;
        self.live_region.last_rendered_rows = 0;
    }

    pub fn reset_committed_output(&mut self) {
        self.transcript.pending_entries.clear();
        self.transcript.history_in_sync = false;
    }

    pub(crate) fn queue_pending_transcript_entries(
        &mut self,
        entries: impl IntoIterator<Item = TranscriptEntry>,
    ) {
        self.transcript.pending_entries.extend(entries);
    }

    pub(crate) fn take_pending_transcript_entries(&mut self) -> Vec<TranscriptEntry> {
        std::mem::take(&mut self.transcript.pending_entries)
    }

    pub(crate) fn mark_terminal_history_synced(&mut self) {
        self.transcript.history_in_sync = true;
    }

    pub(crate) fn terminal_history_is_synced(&self) -> bool {
        self.transcript.history_in_sync
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ComposerRenderState {
    pub width: u16,
    pub hint_rows: u16,
    pub editor_rows: u16,
    pub footer_rows: u16,
    pub total_rows: u16,
    pub caret_row: u16,
    pub caret_col: u16,
    pub last_rendered_rows: u16,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct LiveRegionRenderState {
    pub anchor_valid: bool,
    pub total_rows: u16,
    pub hidden_rows_above: u16,
    pub viewport_height: u16,
    pub last_rendered_rows: u16,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TranscriptRenderState {
    pub(crate) pending_entries: Vec<TranscriptEntry>,
    pub history_in_sync: bool,
}

#[cfg(test)]
mod tests {
    use super::{ChatRenderState, ComposerRenderState, TranscriptRenderState};
    use crate::app::handoff::types::{SystemTranscriptEntry, TranscriptEntry};

    #[test]
    fn clear_measurements_preserves_terminal_size_and_invalidates_live_rows() {
        let mut state = ChatRenderState {
            terminal_width: 120,
            terminal_height: 40,
            line_wrap_disabled: true,
            composer: ComposerRenderState {
                width: 120,
                hint_rows: 1,
                editor_rows: 2,
                footer_rows: 2,
                total_rows: 5,
                caret_row: 1,
                caret_col: 3,
                last_rendered_rows: 5,
            },
            live_region: super::LiveRegionRenderState {
                anchor_valid: true,
                total_rows: 9,
                hidden_rows_above: 2,
                viewport_height: 7,
                last_rendered_rows: 7,
            },
            transcript: super::TranscriptRenderState {
                pending_entries: vec![TranscriptEntry::System(SystemTranscriptEntry {
                    severity: None,
                    text: "queued".to_owned(),
                })],
                history_in_sync: true,
            },
        };

        state.clear_measurements();

        assert_eq!(state.terminal_width, 120);
        assert_eq!(state.terminal_height, 40);
        assert!(state.line_wrap_disabled);
        assert_eq!(state.composer, ComposerRenderState::default());
        assert_eq!(state.live_region.total_rows, 0);
        assert_eq!(state.live_region.hidden_rows_above, 0);
        assert_eq!(state.live_region.viewport_height, 0);
        assert_eq!(state.live_region.last_rendered_rows, 0);
        assert!(state.live_region.anchor_valid);
        assert_eq!(
            state.transcript,
            TranscriptRenderState {
                pending_entries: vec![TranscriptEntry::System(SystemTranscriptEntry {
                    severity: None,
                    text: "queued".to_owned(),
                })],
                history_in_sync: true,
            }
        );
    }

    #[test]
    fn invalidate_live_anchor_clears_anchor_and_rows() {
        let mut state = ChatRenderState::default();
        state.live_region.anchor_valid = true;
        state.live_region.total_rows = 12;
        state.live_region.hidden_rows_above = 3;
        state.live_region.viewport_height = 9;
        state.live_region.last_rendered_rows = 9;

        state.invalidate_live_anchor();

        assert!(!state.live_region.anchor_valid);
        assert_eq!(state.live_region.total_rows, 0);
        assert_eq!(state.live_region.hidden_rows_above, 0);
        assert_eq!(state.live_region.viewport_height, 0);
        assert_eq!(state.live_region.last_rendered_rows, 0);
    }

    #[test]
    fn reset_committed_output_clears_pending_entries_and_unsyncs_history() {
        let mut state = ChatRenderState::default();
        state.queue_pending_transcript_entries([TranscriptEntry::System(SystemTranscriptEntry {
            severity: None,
            text: "queued".to_owned(),
        })]);
        state.mark_terminal_history_synced();

        state.reset_committed_output();

        assert_eq!(state.transcript.pending_entries, Vec::<TranscriptEntry>::new());
        assert!(!state.transcript.history_in_sync);
    }

    #[test]
    fn take_pending_entries_drains_queue() {
        let mut state = ChatRenderState::default();
        state.queue_pending_transcript_entries([TranscriptEntry::System(SystemTranscriptEntry {
            severity: None,
            text: "queued".to_owned(),
        })]);

        let drained = state.take_pending_transcript_entries();

        assert_eq!(
            drained,
            vec![TranscriptEntry::System(SystemTranscriptEntry {
                severity: None,
                text: "queued".to_owned(),
            })]
        );
        assert!(state.transcript.pending_entries.is_empty());
    }
}
