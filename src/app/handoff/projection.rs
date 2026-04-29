// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::shadow::HandoffShadowState;
use super::types::{AssistantTurnId, TranscriptEntry};
use crate::app::App;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct InlineOutputId(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InlineOutputStatus {
    PendingInsert,
    Inserted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InlineOutputAnchor {
    Message { msg_idx: usize, entry_idx: usize },
    AssistantCommit { msg_idx: usize, turn_id: AssistantTurnId, commit_idx: usize },
    AssistantLive { msg_idx: usize, turn_id: AssistantTurnId },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InlineOutputItemKind {
    Transcript { entry: TranscriptEntry, status: InlineOutputStatus },
    AssistantLive { msg_idx: usize, turn_id: AssistantTurnId },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InlineOutputItem {
    pub id: InlineOutputId,
    pub anchor: InlineOutputAnchor,
    pub kind: InlineOutputItemKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InlineStaticInsertItem {
    pub id: InlineOutputId,
    pub entry: TranscriptEntry,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct InlineStaticInsertPlan {
    pub items: Vec<InlineStaticInsertItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct InlineHistoryReplayPlan {
    pub entries: Vec<TranscriptEntry>,
    pub pending_ids: Vec<InlineOutputId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct InlineOutputState {
    next_id: u64,
    items: Vec<InlineOutputItem>,
}

#[must_use]
pub(crate) fn inline_live_projection(app: &App) -> Vec<InlineOutputItem> {
    app.handoff_shadow.inline_output.live_items().into_iter().cloned().collect()
}

#[must_use]
pub(crate) fn inline_live_projection_after_static_insert(
    app: &App,
    inserted_ids: &[InlineOutputId],
) -> Vec<InlineOutputItem> {
    app.handoff_shadow
        .inline_output
        .items()
        .iter()
        .filter(|item| match item.kind {
            InlineOutputItemKind::Transcript {
                status: InlineOutputStatus::PendingInsert, ..
            } => !inserted_ids.contains(&item.id),
            InlineOutputItemKind::AssistantLive { .. } => true,
            InlineOutputItemKind::Transcript { status: InlineOutputStatus::Inserted, .. } => false,
        })
        .cloned()
        .collect()
}

#[must_use]
pub(crate) fn inline_static_insert_plan(app: &App) -> InlineStaticInsertPlan {
    InlineStaticInsertPlan {
        items: app
            .handoff_shadow
            .inline_output
            .pending_transcript_items()
            .into_iter()
            .filter_map(|item| match &item.kind {
                InlineOutputItemKind::Transcript {
                    entry,
                    status: InlineOutputStatus::PendingInsert,
                } => Some(InlineStaticInsertItem { id: item.id, entry: entry.clone() }),
                _ => None,
            })
            .collect(),
    }
}

#[must_use]
pub(crate) fn inline_history_replay_plan(app: &App) -> InlineHistoryReplayPlan {
    let mut entries = Vec::new();
    let mut pending_ids = Vec::new();

    for item in app.handoff_shadow.inline_output.items() {
        let InlineOutputItemKind::Transcript { entry, status } = &item.kind else {
            continue;
        };

        entries.push(entry.clone());
        if *status == InlineOutputStatus::PendingInsert {
            pending_ids.push(item.id);
        }
    }

    InlineHistoryReplayPlan { entries, pending_ids }
}

pub(crate) fn confirm_static_inserted(shadow: &mut HandoffShadowState, ids: &[InlineOutputId]) {
    shadow.inline_output.confirm_static_inserted(ids);
}

impl InlineOutputState {
    #[must_use]
    pub(crate) fn items(&self) -> &[InlineOutputItem] {
        &self.items
    }

    #[must_use]
    pub(crate) fn pending_transcript_items(&self) -> Vec<&InlineOutputItem> {
        self.items
            .iter()
            .filter(|item| {
                matches!(
                    item.kind,
                    InlineOutputItemKind::Transcript {
                        status: InlineOutputStatus::PendingInsert,
                        ..
                    }
                )
            })
            .collect()
    }

    #[must_use]
    pub(crate) fn live_items(&self) -> Vec<&InlineOutputItem> {
        self.items
            .iter()
            .filter(|item| {
                matches!(
                    item.kind,
                    InlineOutputItemKind::Transcript {
                        status: InlineOutputStatus::PendingInsert,
                        ..
                    } | InlineOutputItemKind::AssistantLive { .. }
                )
            })
            .collect()
    }

    pub(crate) fn record_message_transcript_entries(
        &mut self,
        msg_idx: usize,
        entries: Vec<TranscriptEntry>,
    ) -> Vec<InlineOutputId> {
        entries
            .into_iter()
            .enumerate()
            .map(|(entry_idx, entry)| {
                self.record_transcript_entry(
                    InlineOutputAnchor::Message { msg_idx, entry_idx },
                    entry,
                    None,
                )
                .0
            })
            .collect()
    }

    pub(crate) fn record_assistant_committed_entries(
        &mut self,
        msg_idx: usize,
        turn_id: AssistantTurnId,
        entries: Vec<TranscriptEntry>,
    ) -> Vec<InlineOutputId> {
        let start_commit_idx = self.assistant_commit_count(msg_idx, turn_id);
        let mut insert_before = self.assistant_live_position(msg_idx, turn_id);
        let mut ids = Vec::with_capacity(entries.len());

        for (entry_offset, entry) in entries.into_iter().enumerate() {
            let (id, inserted) = self.record_transcript_entry(
                InlineOutputAnchor::AssistantCommit {
                    msg_idx,
                    turn_id,
                    commit_idx: start_commit_idx.saturating_add(entry_offset),
                },
                entry,
                insert_before,
            );
            if inserted && let Some(position) = insert_before.as_mut() {
                *position = position.saturating_add(1);
            }
            ids.push(id);
        }

        ids
    }

    pub(crate) fn record_assistant_live_slot(
        &mut self,
        msg_idx: usize,
        turn_id: AssistantTurnId,
    ) -> InlineOutputId {
        let anchor = InlineOutputAnchor::AssistantLive { msg_idx, turn_id };
        if let Some(existing) = self.id_for_anchor(&anchor) {
            return existing;
        }

        let id = self.allocate_id();
        self.items.push(InlineOutputItem {
            id,
            anchor,
            kind: InlineOutputItemKind::AssistantLive { msg_idx, turn_id },
        });
        id
    }

    pub(crate) fn remove_assistant_live_slot(&mut self, msg_idx: usize, turn_id: AssistantTurnId) {
        let anchor = InlineOutputAnchor::AssistantLive { msg_idx, turn_id };
        self.items.retain(|item| item.anchor != anchor);
    }

    pub(crate) fn confirm_static_inserted(&mut self, ids: &[InlineOutputId]) {
        for item in &mut self.items {
            if !ids.contains(&item.id) {
                continue;
            }
            if let InlineOutputItemKind::Transcript { status, .. } = &mut item.kind {
                *status = InlineOutputStatus::Inserted;
            }
        }
    }

    fn record_transcript_entry(
        &mut self,
        anchor: InlineOutputAnchor,
        entry: TranscriptEntry,
        insert_before: Option<usize>,
    ) -> (InlineOutputId, bool) {
        if let Some(existing) = self.id_for_anchor(&anchor) {
            return (existing, false);
        }

        let id = self.allocate_id();
        let item = InlineOutputItem {
            id,
            anchor,
            kind: InlineOutputItemKind::Transcript {
                entry,
                status: InlineOutputStatus::PendingInsert,
            },
        };

        match insert_before {
            Some(idx) => self.items.insert(idx, item),
            None => self.items.push(item),
        }
        (id, true)
    }

    fn allocate_id(&mut self) -> InlineOutputId {
        self.next_id = self.next_id.saturating_add(1);
        InlineOutputId(self.next_id)
    }

    fn id_for_anchor(&self, anchor: &InlineOutputAnchor) -> Option<InlineOutputId> {
        self.items.iter().find(|item| item.anchor == *anchor).map(|item| item.id)
    }

    fn assistant_live_position(&self, msg_idx: usize, turn_id: AssistantTurnId) -> Option<usize> {
        self.items
            .iter()
            .position(|item| item.anchor == InlineOutputAnchor::AssistantLive { msg_idx, turn_id })
    }

    fn assistant_commit_count(&self, msg_idx: usize, turn_id: AssistantTurnId) -> usize {
        self.items
            .iter()
            .filter(|item| {
                matches!(
                    item.anchor,
                    InlineOutputAnchor::AssistantCommit {
                        msg_idx: anchor_msg_idx,
                        turn_id: anchor_turn_id,
                        ..
                    } if anchor_msg_idx == msg_idx && anchor_turn_id == turn_id
                )
            })
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::SystemSeverity;
    use crate::app::handoff::types::{
        SystemTranscriptEntry, UserTranscriptBlock, UserTranscriptEntry, WelcomeTranscriptEntry,
    };

    fn system_entry(text: &str) -> TranscriptEntry {
        TranscriptEntry::System(SystemTranscriptEntry {
            severity: Some(SystemSeverity::Info),
            text: text.to_owned(),
        })
    }

    fn user_entry(text: &str) -> TranscriptEntry {
        TranscriptEntry::User(UserTranscriptEntry {
            blocks: vec![UserTranscriptBlock::Text(text.to_owned())],
        })
    }

    fn welcome_entry() -> TranscriptEntry {
        TranscriptEntry::Welcome(WelcomeTranscriptEntry {
            version: "1.2.3".to_owned(),
            subscription: "Pro".to_owned(),
            cwd: "/workspace".to_owned(),
            session_id: "session-1".to_owned(),
            tip_seed: 7,
        })
    }

    #[test]
    fn message_entries_receive_stable_monotonic_ids() {
        let mut state = InlineOutputState::default();

        let first = state.record_message_transcript_entries(0, vec![user_entry("one")]);
        let second = state.record_message_transcript_entries(1, vec![system_entry("two")]);

        assert_eq!(first[0], InlineOutputId(1));
        assert_eq!(second[0], InlineOutputId(2));
        assert_eq!(state.items()[0].id, first[0]);
        assert_eq!(state.items()[1].id, second[0]);
    }

    #[test]
    fn transcript_and_live_items_keep_append_order() {
        let mut state = InlineOutputState::default();
        let turn_id = AssistantTurnId(7);

        state.record_message_transcript_entries(0, vec![user_entry("user")]);
        state.record_assistant_live_slot(1, turn_id);

        assert!(matches!(
            state.items()[0].anchor,
            InlineOutputAnchor::Message { msg_idx: 0, entry_idx: 0 }
        ));
        assert!(matches!(
            state.items()[1].anchor,
            InlineOutputAnchor::AssistantLive { msg_idx: 1, turn_id: AssistantTurnId(7) }
        ));
    }

    #[test]
    fn confirm_static_inserted_marks_only_matching_transcript_items() {
        let mut state = InlineOutputState::default();
        let ids = state
            .record_message_transcript_entries(0, vec![user_entry("one"), system_entry("two")]);

        state.confirm_static_inserted(&[ids[0], InlineOutputId(999)]);

        assert!(matches!(
            state.items()[0].kind,
            InlineOutputItemKind::Transcript { status: InlineOutputStatus::Inserted, .. }
        ));
        assert!(matches!(
            state.items()[1].kind,
            InlineOutputItemKind::Transcript { status: InlineOutputStatus::PendingInsert, .. }
        ));
    }

    #[test]
    fn assistant_commits_insert_before_existing_live_slot() {
        let mut state = InlineOutputState::default();
        let turn_id = AssistantTurnId(3);

        state.record_message_transcript_entries(0, vec![user_entry("user")]);
        let live_id = state.record_assistant_live_slot(1, turn_id);
        let commit_ids = state.record_assistant_committed_entries(
            1,
            turn_id,
            vec![system_entry("first"), system_entry("second")],
        );

        assert_eq!(
            state.items()[0].anchor,
            InlineOutputAnchor::Message { msg_idx: 0, entry_idx: 0 }
        );
        assert_eq!(
            state.items()[1].anchor,
            InlineOutputAnchor::AssistantCommit { msg_idx: 1, turn_id, commit_idx: 0 }
        );
        assert_eq!(
            state.items()[2].anchor,
            InlineOutputAnchor::AssistantCommit { msg_idx: 1, turn_id, commit_idx: 1 }
        );
        assert_eq!(
            state.items()[3].anchor,
            InlineOutputAnchor::AssistantLive { msg_idx: 1, turn_id }
        );
        assert_eq!(state.items()[1].id, commit_ids[0]);
        assert_eq!(state.items()[2].id, commit_ids[1]);
        assert_eq!(state.items()[3].id, live_id);
    }

    #[test]
    fn live_projection_excludes_inserted_transcript_items() {
        let mut state = InlineOutputState::default();
        let turn_id = AssistantTurnId(4);
        let ids = state.record_message_transcript_entries(0, vec![user_entry("inserted")]);
        let live_id = state.record_assistant_live_slot(1, turn_id);

        state.confirm_static_inserted(&ids);

        let live_items = state.live_items();
        assert_eq!(live_items.len(), 1);
        assert_eq!(live_items[0].id, live_id);
    }

    #[test]
    fn inline_live_projection_includes_pending_transcript_roles() {
        let mut app = crate::app::App::test_default();
        app.handoff_shadow.inline_output.record_message_transcript_entries(
            0,
            vec![welcome_entry(), user_entry("user"), system_entry("system")],
        );

        let projection = inline_live_projection(&app);

        assert_eq!(projection.len(), 3);
        assert!(matches!(
            projection[0].kind,
            InlineOutputItemKind::Transcript { entry: TranscriptEntry::Welcome(_), .. }
        ));
        assert!(matches!(
            projection[1].kind,
            InlineOutputItemKind::Transcript { entry: TranscriptEntry::User(_), .. }
        ));
        assert!(matches!(
            projection[2].kind,
            InlineOutputItemKind::Transcript { entry: TranscriptEntry::System(_), .. }
        ));
    }

    #[test]
    fn inline_live_projection_excludes_confirmed_transcript_items() {
        let mut app = crate::app::App::test_default();
        let ids = app
            .handoff_shadow
            .inline_output
            .record_message_transcript_entries(0, vec![user_entry("inserted")]);
        app.handoff_shadow
            .inline_output
            .record_message_transcript_entries(1, vec![system_entry("pending")]);

        app.handoff_shadow.inline_output.confirm_static_inserted(&ids);

        let projection = inline_live_projection(&app);
        assert_eq!(projection.len(), 1);
        assert!(matches!(
            projection[0].kind,
            InlineOutputItemKind::Transcript {
                entry: TranscriptEntry::System(_),
                status: InlineOutputStatus::PendingInsert,
            }
        ));
    }

    #[test]
    fn inline_live_projection_orders_assistant_commit_before_live_slot() {
        let mut app = crate::app::App::test_default();
        let turn_id = AssistantTurnId(9);
        app.handoff_shadow.inline_output.record_assistant_live_slot(0, turn_id);
        app.handoff_shadow.inline_output.record_assistant_committed_entries(
            0,
            turn_id,
            vec![system_entry("committed")],
        );

        let projection = inline_live_projection(&app);

        assert_eq!(projection.len(), 2);
        assert_eq!(
            projection[0].anchor,
            InlineOutputAnchor::AssistantCommit { msg_idx: 0, turn_id, commit_idx: 0 }
        );
        assert_eq!(projection[1].anchor, InlineOutputAnchor::AssistantLive { msg_idx: 0, turn_id });
    }

    #[test]
    fn inline_live_projection_keeps_duplicate_message_anchor_once() {
        let mut app = crate::app::App::test_default();

        app.handoff_shadow
            .inline_output
            .record_message_transcript_entries(0, vec![welcome_entry()]);
        app.handoff_shadow
            .inline_output
            .record_message_transcript_entries(0, vec![welcome_entry()]);

        let projection = inline_live_projection(&app);
        assert_eq!(projection.len(), 1);
        assert_eq!(projection[0].anchor, InlineOutputAnchor::Message { msg_idx: 0, entry_idx: 0 });
    }

    #[test]
    fn inline_static_insert_plan_returns_pending_transcripts_in_timeline_order() {
        let mut app = crate::app::App::test_default();
        let turn_id = AssistantTurnId(11);
        let user_ids = app
            .handoff_shadow
            .inline_output
            .record_message_transcript_entries(0, vec![user_entry("user")]);
        let live_id = app.handoff_shadow.inline_output.record_assistant_live_slot(1, turn_id);
        let commit_ids = app.handoff_shadow.inline_output.record_assistant_committed_entries(
            1,
            turn_id,
            vec![system_entry("commit")],
        );

        let plan = inline_static_insert_plan(&app);

        assert_eq!(plan.items.len(), 2);
        assert_eq!(plan.items[0].id, user_ids[0]);
        assert_eq!(plan.items[0].entry, user_entry("user"));
        assert_eq!(plan.items[1].id, commit_ids[0]);
        assert_eq!(plan.items[1].entry, system_entry("commit"));
        assert!(!plan.items.iter().any(|item| item.id == live_id));
    }

    #[test]
    fn inline_static_insert_plan_excludes_confirmed_transcript_items() {
        let mut app = crate::app::App::test_default();
        let inserted_ids = app
            .handoff_shadow
            .inline_output
            .record_message_transcript_entries(0, vec![user_entry("inserted")]);
        let pending_ids = app
            .handoff_shadow
            .inline_output
            .record_message_transcript_entries(1, vec![system_entry("pending")]);

        confirm_static_inserted(&mut app.handoff_shadow, &inserted_ids);

        let plan = inline_static_insert_plan(&app);
        assert_eq!(plan.items.len(), 1);
        assert_eq!(plan.items[0].id, pending_ids[0]);
        assert_eq!(plan.items[0].entry, system_entry("pending"));
    }

    #[test]
    fn inline_static_insert_plan_keeps_items_pending_until_confirmation() {
        let mut app = crate::app::App::test_default();
        let ids = app
            .handoff_shadow
            .inline_output
            .record_message_transcript_entries(0, vec![user_entry("pending")]);

        let first_plan = inline_static_insert_plan(&app);
        let second_plan = inline_static_insert_plan(&app);

        assert_eq!(first_plan, second_plan);
        assert_eq!(second_plan.items[0].id, ids[0]);
        assert!(matches!(
            app.handoff_shadow.inline_output.items()[0].kind,
            InlineOutputItemKind::Transcript { status: InlineOutputStatus::PendingInsert, .. }
        ));
    }

    #[test]
    fn confirm_static_inserted_top_level_marks_planned_ids_inserted() {
        let mut app = crate::app::App::test_default();
        let ids = app
            .handoff_shadow
            .inline_output
            .record_message_transcript_entries(0, vec![user_entry("one")]);

        confirm_static_inserted(&mut app.handoff_shadow, &ids);

        assert!(inline_static_insert_plan(&app).items.is_empty());
        assert!(matches!(
            app.handoff_shadow.inline_output.items()[0].kind,
            InlineOutputItemKind::Transcript { status: InlineOutputStatus::Inserted, .. }
        ));
    }

    #[test]
    fn inline_history_replay_plan_includes_inserted_and_pending_transcripts_in_timeline_order() {
        let mut app = crate::app::App::test_default();
        let inserted_ids = app
            .handoff_shadow
            .inline_output
            .record_message_transcript_entries(0, vec![user_entry("inserted")]);
        let pending_ids = app
            .handoff_shadow
            .inline_output
            .record_message_transcript_entries(1, vec![system_entry("pending")]);

        confirm_static_inserted(&mut app.handoff_shadow, &inserted_ids);

        let plan = inline_history_replay_plan(&app);
        assert_eq!(plan.entries, vec![user_entry("inserted"), system_entry("pending")]);
        assert_eq!(plan.pending_ids, pending_ids);
    }

    #[test]
    fn inline_history_replay_plan_excludes_assistant_live_items() {
        let mut app = crate::app::App::test_default();
        let turn_id = AssistantTurnId(12);
        app.handoff_shadow
            .inline_output
            .record_message_transcript_entries(0, vec![user_entry("user")]);
        app.handoff_shadow.inline_output.record_assistant_live_slot(1, turn_id);
        app.handoff_shadow.inline_output.record_assistant_committed_entries(
            1,
            turn_id,
            vec![system_entry("commit")],
        );

        let plan = inline_history_replay_plan(&app);

        assert_eq!(plan.entries, vec![user_entry("user"), system_entry("commit")]);
        assert_eq!(plan.pending_ids.len(), 2);
    }

    #[test]
    fn inline_history_replay_plan_has_no_pending_ids_for_inserted_transcripts() {
        let mut app = crate::app::App::test_default();
        let ids = app
            .handoff_shadow
            .inline_output
            .record_message_transcript_entries(0, vec![user_entry("inserted")]);
        confirm_static_inserted(&mut app.handoff_shadow, &ids);

        let plan = inline_history_replay_plan(&app);

        assert_eq!(plan.entries, vec![user_entry("inserted")]);
        assert!(plan.pending_ids.is_empty());
    }

    #[test]
    fn default_state_is_empty_and_reset_drops_items_and_ids() {
        let mut state = InlineOutputState::default();
        state.record_message_transcript_entries(0, vec![user_entry("one")]);

        state = InlineOutputState::default();
        let ids = state.record_message_transcript_entries(0, vec![user_entry("after reset")]);

        assert_eq!(state.items().len(), 1);
        assert_eq!(ids[0], InlineOutputId(1));
    }

    #[test]
    fn duplicate_message_anchor_returns_existing_id_without_new_item() {
        let mut state = InlineOutputState::default();

        let first = state.record_message_transcript_entries(0, vec![user_entry("one")]);
        let second = state.record_message_transcript_entries(0, vec![user_entry("changed")]);

        assert_eq!(first, second);
        assert_eq!(state.items().len(), 1);
        let InlineOutputItemKind::Transcript { entry, .. } = &state.items()[0].kind else {
            panic!("expected transcript item");
        };
        assert_eq!(*entry, user_entry("one"));
    }

    #[test]
    fn duplicate_live_anchor_returns_existing_id_without_new_item() {
        let mut state = InlineOutputState::default();

        let first = state.record_assistant_live_slot(1, AssistantTurnId(8));
        let second = state.record_assistant_live_slot(1, AssistantTurnId(8));

        assert_eq!(first, second);
        assert_eq!(state.items().len(), 1);
    }
}
