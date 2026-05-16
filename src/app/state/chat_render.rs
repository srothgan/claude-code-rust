// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ChatRenderState {
    pub terminal_width: u16,
    pub terminal_height: u16,
    pub line_wrap_disabled: bool,
    pub thinking_verb: Option<&'static str>,
    pub resize_purge_replay_after_turn: bool,
    pub composer: ComposerRenderState,
    pub live_region: LiveRegionRenderState,
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

    pub fn mark_resize_purge_replay_during_turn(&mut self) {
        self.resize_purge_replay_after_turn = true;
    }

    pub fn take_resize_purge_replay_after_turn(&mut self) -> bool {
        let replay_needed = self.resize_purge_replay_after_turn;
        self.resize_purge_replay_after_turn = false;
        replay_needed
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

#[cfg(test)]
mod tests {
    use super::{ChatRenderState, ComposerRenderState};

    #[test]
    fn clear_measurements_preserves_terminal_size_and_invalidates_live_rows() {
        let mut state = ChatRenderState {
            terminal_width: 120,
            terminal_height: 40,
            line_wrap_disabled: true,
            thinking_verb: Some("Pondering"),
            resize_purge_replay_after_turn: true,
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
        };

        state.clear_measurements();

        assert_eq!(state.terminal_width, 120);
        assert_eq!(state.terminal_height, 40);
        assert!(state.line_wrap_disabled);
        assert_eq!(state.thinking_verb, Some("Pondering"));
        assert!(state.resize_purge_replay_after_turn);
        assert_eq!(state.composer, ComposerRenderState::default());
        assert_eq!(state.live_region.total_rows, 0);
        assert_eq!(state.live_region.hidden_rows_above, 0);
        assert_eq!(state.live_region.viewport_height, 0);
        assert_eq!(state.live_region.last_rendered_rows, 0);
        assert!(state.live_region.anchor_valid);
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
    fn resize_purge_replay_after_turn_flag_is_drained() {
        let mut state = ChatRenderState::default();

        state.mark_resize_purge_replay_during_turn();

        assert!(state.take_resize_purge_replay_after_turn());
        assert!(!state.take_resize_purge_replay_after_turn());
    }
}
