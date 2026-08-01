// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

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
    /// Spinner label shown while a slash command is in flight (`CommandPending`).
    pub pending_command_label: Option<String>,
    /// Ack marker required to clear `CommandPending` for strict completion semantics.
    pub pending_command_ack: Option<PendingCommandAck>,
    /// The single live lifecycle state for manual and automatic context compaction.
    pub compaction: CompactionState,
    /// Tool call IDs with pending inline interactions, ordered by arrival.
    /// The first entry is the focused interaction that receives keyboard input.
    /// Up / Down arrow keys cycle focus through the list.
    pub pending_interaction_ids: Vec<String>,
    /// Set when a cancel notification succeeds; consumed on `TurnComplete`
    /// to render a red interruption hint in chat.
    pub cancelled_pending_hint: bool,
    /// Origin of the in-flight cancellation request, if any.
    pub pending_cancel_origin: Option<CancelOrigin>,
    /// Auto-submit the current input draft once cancellation transitions the app
    /// back to `Ready`.
    pub pending_auto_submit_after_cancel: bool,
    /// Message index that owns the current main-assistant turn indicators.
    pub assistant_message_idx: Option<usize>,
    /// IDs of root Task/Agent tool calls currently `InProgress`.
    /// Use `App::insert_active_task()`, `App::remove_active_task()`.
    pub active_task_ids: HashSet<String>,
    /// Turn-local inline/system notices that may upgrade in place during the active turn.
    pub notice_refs: Vec<TurnNoticeRef>,
}

impl TurnState {
    /// Clear all cancellation bookkeeping (after a cancel resolves or on
    /// session reset). Keeps the three cancel flags from drifting apart.
    pub fn clear_cancel_state(&mut self) {
        self.cancelled_pending_hint = false;
        self.pending_cancel_origin = None;
        self.pending_auto_submit_after_cancel = false;
    }

    /// Clear turn-local tracking that must not survive a completed or failed
    /// turn. Message-index fields are reset here; topology reindexing methods
    /// handle index shifts while a turn is still active.
    pub fn reset_for_turn_exit(&mut self) {
        self.pending_command_label = None;
        self.pending_command_ack = None;
        self.compaction.reset();
        self.pending_interaction_ids.clear();
        self.cancelled_pending_hint = false;
        self.pending_cancel_origin = None;
        self.assistant_message_idx = None;
        self.active_task_ids.clear();
        self.notice_refs.clear();
    }

    /// Clear all state scoped to the active session/turn lifecycle.
    pub fn reset_for_new_session(&mut self) {
        self.reset_for_turn_exit();
        self.pending_auto_submit_after_cancel = false;
    }
}
