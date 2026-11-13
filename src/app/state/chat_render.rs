// SPDX-License-Identifier: Apache-2.0

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ChatRenderState {
    pub(crate) viewport: crate::app::terminal_runtime::viewport::ChatViewport,
    pub terminal_width: u16,
    pub terminal_height: u16,
    pub line_wrap_disabled: bool,
    pub resize_purge_replay_after_turn: bool,
    pub resize_purge_replay_on_chat_return: bool,
    pub composer: ComposerRenderState,
    pub live_region: LiveRegionRenderState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalSize {
    pub width: u16,
    pub height: u16,
}

impl TerminalSize {
    pub const fn new(width: u16, height: u16) -> Self {
        Self { width, height }
    }

    const fn is_known(self) -> bool {
        self.width > 0 && self.height > 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalSizeChange {
    Initial { current: TerminalSize },
    Unchanged { current: TerminalSize },
    Changed { previous: TerminalSize, current: TerminalSize },
}

impl TerminalSizeChange {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Initial { .. } => "initial",
            Self::Unchanged { .. } => "unchanged",
            Self::Changed { .. } => "changed",
        }
    }

    pub const fn previous(self) -> Option<TerminalSize> {
        match self {
            Self::Changed { previous, .. } => Some(previous),
            Self::Initial { .. } | Self::Unchanged { .. } => None,
        }
    }

    pub const fn current(self) -> TerminalSize {
        match self {
            Self::Initial { current }
            | Self::Unchanged { current }
            | Self::Changed { current, .. } => current,
        }
    }
}

impl ChatRenderState {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub const fn terminal_size(&self) -> TerminalSize {
        TerminalSize::new(self.terminal_width, self.terminal_height)
    }

    pub fn observe_terminal_size(&mut self, width: u16, height: u16) -> TerminalSizeChange {
        let previous = self.terminal_size();
        let current = TerminalSize::new(width, height);
        self.terminal_width = width;
        self.terminal_height = height;

        if !previous.is_known() {
            TerminalSizeChange::Initial { current }
        } else if previous == current {
            TerminalSizeChange::Unchanged { current }
        } else {
            TerminalSizeChange::Changed { previous, current }
        }
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

    pub fn mark_resize_purge_replay_on_chat_return(&mut self) {
        self.resize_purge_replay_on_chat_return = true;
    }

    pub fn take_resize_purge_replay_on_chat_return(&mut self) -> bool {
        let replay_needed = self.resize_purge_replay_on_chat_return;
        self.resize_purge_replay_on_chat_return = false;
        replay_needed
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ComposerRenderState {
    pub width: u16,
    pub activity_rows: u16,
    pub hint_rows: u16,
    pub btw_rows: u16,
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
