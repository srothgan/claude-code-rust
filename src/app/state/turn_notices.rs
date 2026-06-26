// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum NoticeStage {
    Warning,
    Rejected,
    PlanLimitTurnError,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnNoticeLocation {
    Inline { msg_idx: usize, block_idx: usize },
    Standalone { msg_idx: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnNoticeRef {
    pub dedup_key: NoticeDedupKey,
    pub stage: NoticeStage,
    pub location: TurnNoticeLocation,
}

impl App {
    pub(crate) fn clear_turn_notice_refs(&mut self) {
        self.turn_notice_refs.clear();
    }

    pub(crate) fn shift_turn_notice_refs_for_insert(&mut self, idx: usize) {
        for notice_ref in &mut self.turn_notice_refs {
            match &mut notice_ref.location {
                TurnNoticeLocation::Inline { msg_idx, .. }
                | TurnNoticeLocation::Standalone { msg_idx }
                    if idx <= *msg_idx =>
                {
                    *msg_idx = msg_idx.saturating_add(1);
                }
                TurnNoticeLocation::Inline { .. } | TurnNoticeLocation::Standalone { .. } => {}
            }
        }
    }

    pub(crate) fn shift_turn_notice_refs_for_remove(&mut self, idx: usize) {
        self.turn_notice_refs.retain_mut(|notice_ref| match &mut notice_ref.location {
            TurnNoticeLocation::Inline { msg_idx, .. }
            | TurnNoticeLocation::Standalone { msg_idx } => match idx.cmp(msg_idx) {
                std::cmp::Ordering::Less => {
                    *msg_idx = msg_idx.saturating_sub(1);
                    true
                }
                std::cmp::Ordering::Equal => false,
                std::cmp::Ordering::Greater => true,
            },
        });
    }

    pub(crate) fn remap_turn_notice_refs_after_message_drop(
        &mut self,
        old_to_new: &[Option<usize>],
    ) {
        self.turn_notice_refs.retain_mut(|notice_ref| match &mut notice_ref.location {
            TurnNoticeLocation::Inline { msg_idx, .. }
            | TurnNoticeLocation::Standalone { msg_idx } => {
                let Some(new_idx) = old_to_new.get(*msg_idx).copied().flatten() else {
                    return false;
                };
                *msg_idx = new_idx;
                true
            }
        });
    }
}
