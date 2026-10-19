// SPDX-License-Identifier: Apache-2.0

use super::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum CompactionState {
    #[default]
    Idle,
    Active(ActiveCompaction),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ActiveCompaction {
    trigger: Option<model::CompactionTrigger>,
    boundary: Option<model::CompactionBoundary>,
}

impl CompactionState {
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Active(_))
    }

    pub(crate) fn begin(&mut self) {
        if matches!(self, Self::Idle) {
            *self = Self::Active(ActiveCompaction::default());
        }
    }

    pub(crate) fn begin_manual(&mut self) {
        if matches!(self, Self::Idle) {
            *self = Self::Active(ActiveCompaction {
                trigger: Some(model::CompactionTrigger::Manual),
                boundary: None,
            });
        }
    }

    pub(crate) fn apply_boundary(&mut self, boundary: model::CompactionBoundary) -> bool {
        let Self::Active(active) = self else {
            return false;
        };
        active.trigger = Some(boundary.trigger);
        active.boundary = Some(boundary);
        true
    }

    pub(crate) fn finish(&mut self) -> Option<ActiveCompaction> {
        match std::mem::take(self) {
            Self::Idle => None,
            Self::Active(active) => Some(active),
        }
    }

    pub(crate) fn reset(&mut self) {
        *self = Self::Idle;
    }
}

impl ActiveCompaction {
    pub(crate) fn trigger(&self) -> Option<model::CompactionTrigger> {
        self.trigger
    }

    pub(crate) fn boundary(&self) -> Option<model::CompactionBoundary> {
        self.boundary
    }
}

/// State scoped to the currently active turn: in-flight command tracking,
/// cancellation bookkeeping, inline interactions, and turn-local notices.
///
/// Everything here resets at turn or session boundaries; grouping it makes
/// those reset contracts explicit instead of scattering them across `App`.
#[allow(clippy::struct_excessive_bools)]
#[derive(Default)]
pub struct TurnState {
    /// Composer status label while a slash command is in flight (`CommandPending`).
    /// `None` marks a silent command whose pending state is not displayed.
    pub pending_command_label: Option<String>,
    /// Stable per-turn activity choices and tip repaint timing.
    pub(crate) activity: Option<crate::app::activity::TurnActivity>,
    /// Ack marker required to clear `CommandPending` for strict completion semantics.
    pub pending_command_ack: Option<PendingCommandAck>,
    /// The single live lifecycle state for manual and automatic context compaction.
    pub compaction: CompactionState,
    /// Whether an explicit cancellation request is awaiting turn exit.
    pub cancel_requested: bool,
    /// Message index that owns the current main-assistant turn output.
    pub assistant_message_idx: Option<usize>,
    /// Turn-local inline/system notices that may update in place during the active turn.
    pub notice_refs: Vec<TurnNoticeRef>,
}

impl TurnState {
    /// Clear cancellation bookkeeping after a cancel resolves or on session reset.
    pub fn clear_cancel_state(&mut self) {
        self.cancel_requested = false;
    }

    /// Clear turn-local tracking that must not survive a completed or failed
    /// turn. Message-index fields are reset here; topology reindexing methods
    /// handle index shifts while a turn is still active.
    pub fn reset_for_turn_exit(&mut self) {
        self.activity = None;
        self.pending_command_label = None;
        self.pending_command_ack = None;
        self.compaction.reset();
        self.cancel_requested = false;
        self.assistant_message_idx = None;
        self.notice_refs.clear();
    }

    /// Clear all state scoped to the active session/turn lifecycle.
    pub fn reset_for_new_session(&mut self) {
        self.reset_for_turn_exit();
    }
}

impl App {
    /// Reserve one output owner. Queue acknowledgements can continue an existing SDK phase.
    pub(crate) fn start_assistant_response(&mut self) {
        self.reserve_assistant_output_owner();
        if !matches!(self.status, AppStatus::Thinking | AppStatus::Running) {
            self.status = AppStatus::Running;
        }
        self.begin_turn_activity(std::time::Instant::now());
        self.enforce_history_retention_tracked();
    }

    #[must_use]
    pub(crate) fn is_agent_turn_active(&self) -> bool {
        matches!(self.status, AppStatus::Thinking | AppStatus::Running)
            || self.turn.compaction.is_active()
            || self.turn.cancel_requested
    }
}
