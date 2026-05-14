// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::finality::committed_prefix_len;
use super::serialize::serialize_assistant_prefix;
use super::stabilizer::seal_mutable_text_tail;
use super::types::{
    HandoffDecision, LiveAssistantTurn, LiveAssistantUnit, NoticeMutability, TranscriptEntry,
};
use crate::agent::model::ToolCallStatus;
use crate::app::TextBlockSpacing;

#[must_use]
pub(crate) fn plan_handoff(turn: &LiveAssistantTurn) -> HandoffDecision {
    let committed_prefix_len = committed_prefix_len(turn);
    let transcript_entries = if committed_prefix_len == 0 {
        Vec::new()
    } else {
        serialize_assistant_prefix(&turn.units[..committed_prefix_len], &turn.formatting)
    };

    HandoffDecision { committed_prefix_len, transcript_entries }
}

pub(crate) fn apply_successful_commit(turn: &mut LiveAssistantTurn, decision: &HandoffDecision) {
    if decision.committed_prefix_len == 0 {
        return;
    }

    let committed_units: Vec<LiveAssistantUnit> =
        turn.units.drain(..decision.committed_prefix_len).collect();
    turn.refresh_current_text_tail();

    if !decision.transcript_entries.is_empty() {
        turn.formatting.header_printed = true;
        turn.formatting.previous_committed_kind =
            last_committed_kind(decision.transcript_entries.as_slice());
    } else if let Some(last_kind) = last_live_kind(committed_units.as_slice()) {
        turn.formatting.header_printed = true;
        turn.formatting.previous_committed_kind = Some(last_kind);
    }
}

pub(crate) fn prepare_for_turn_exit(
    turn: &mut LiveAssistantTurn,
    final_in_progress_status: ToolCallStatus,
) {
    seal_mutable_text_tail(turn, TextBlockSpacing::None);
    turn.remove_hidden_tools();
    for unit in &mut turn.units {
        match unit {
            LiveAssistantUnit::Notice(notice) => {
                if notice.mutability == NoticeMutability::Upgradeable {
                    notice.mutability = NoticeMutability::Final;
                }
            }
            LiveAssistantUnit::Tool(tool) => {
                tool.pending_permission = false;
                tool.pending_question = false;
                tool.terminal_mutation = super::types::TerminalMutationState::Settled;
                if matches!(
                    tool.snapshot.status,
                    ToolCallStatus::Pending | ToolCallStatus::InProgress
                ) {
                    tool.snapshot.status = final_in_progress_status;
                }
            }
            LiveAssistantUnit::StableText(_) | LiveAssistantUnit::MutableTextTail(_) => {}
        }
    }
    turn.live_indicator = None;
    turn.sealed = true;
}

fn last_committed_kind(
    entries: &[TranscriptEntry],
) -> Option<super::types::CommittedAssistantKind> {
    entries.iter().rev().find_map(|entry| match entry {
        TranscriptEntry::AssistantOpen(entry) | TranscriptEntry::AssistantContinue(entry) => {
            Some(entry.unit.kind())
        }
        TranscriptEntry::Welcome(_) | TranscriptEntry::User(_) | TranscriptEntry::System(_) => None,
    })
}

fn last_live_kind(units: &[LiveAssistantUnit]) -> Option<super::types::CommittedAssistantKind> {
    units.iter().rev().find_map(|unit| match unit {
        LiveAssistantUnit::StableText(_) | LiveAssistantUnit::Notice(_) => {
            Some(super::types::CommittedAssistantKind::TextLike)
        }
        LiveAssistantUnit::Tool(_) => Some(super::types::CommittedAssistantKind::Tool),
        LiveAssistantUnit::MutableTextTail(_) => None,
    })
}

#[cfg(test)]
mod tests {
    use super::{apply_successful_commit, plan_handoff, prepare_for_turn_exit};
    use crate::agent::model;
    use crate::app::handoff::stabilizer::{append_text_chunk, insert_tool};
    use crate::app::handoff::types::CommittedAssistantKind;
    use crate::app::handoff::types::{
        AssistantTurnId, LiveAssistantIndicator, LiveAssistantTurn, LiveAssistantUnit,
        LiveToolUnit, LiveUnitId, MutableTextTailUnit, TerminalMutationState,
        ToolTranscriptSnapshot,
    };

    fn empty_turn() -> LiveAssistantTurn {
        LiveAssistantTurn::new(AssistantTurnId(1))
    }

    fn stable_tool(
        status: model::ToolCallStatus,
        terminal_mutation: TerminalMutationState,
    ) -> LiveToolUnit {
        LiveToolUnit {
            id: LiveUnitId(777),
            snapshot: ToolTranscriptSnapshot {
                tool_call_id: "tool-1".to_owned(),
                title: "Tool".to_owned(),
                sdk_tool_name: "Bash".to_owned(),
                status,
                hidden: false,
                raw_input: None,
                output_metadata: None,
                task_metadata: None,
                content: Vec::new(),
                terminal_command: None,
                terminal_output: None,
            },
            pending_permission: false,
            pending_question: false,
            terminal_mutation,
        }
    }

    fn hidden_tool() -> LiveToolUnit {
        let mut tool =
            stable_tool(model::ToolCallStatus::Completed, TerminalMutationState::Settled);
        tool.snapshot.hidden = true;
        tool
    }

    #[test]
    fn pure_text_before_live_tail_does_not_commit_mid_stream() {
        let mut turn = empty_turn();
        append_text_chunk(&mut turn, "first\n\nsecond");
        if let Some(LiveAssistantUnit::MutableTextTail(tail)) = turn.units.last_mut() {
            tail.text = "live".to_owned();
        }

        let decision = plan_handoff(&turn);

        assert_eq!(decision.committed_prefix_len, 0);
        assert!(decision.transcript_entries.is_empty());
    }

    #[test]
    fn final_tool_after_a_live_text_tail_does_not_commit() {
        let mut turn = empty_turn();
        turn.units.push(LiveAssistantUnit::MutableTextTail(MutableTextTailUnit {
            id: LiveUnitId(1),
            text: "live".to_owned(),
        }));
        turn.current_text_tail = Some(LiveUnitId(1));
        insert_tool(
            &mut turn,
            stable_tool(model::ToolCallStatus::Completed, TerminalMutationState::Settled),
        );
        turn.units.insert(
            0,
            LiveAssistantUnit::MutableTextTail(MutableTextTailUnit {
                id: LiveUnitId(2),
                text: "still live".to_owned(),
            }),
        );
        turn.current_text_tail = Some(LiveUnitId(2));

        let decision = plan_handoff(&turn);

        assert_eq!(decision.committed_prefix_len, 0);
        assert!(decision.transcript_entries.is_empty());
    }

    #[test]
    fn final_tool_before_live_text_tail_commits() {
        let mut turn = empty_turn();
        insert_tool(
            &mut turn,
            stable_tool(model::ToolCallStatus::Completed, TerminalMutationState::Settled),
        );
        append_text_chunk(&mut turn, "after tool");

        let decision = plan_handoff(&turn);

        assert_eq!(decision.committed_prefix_len, 1);
        assert_eq!(decision.transcript_entries.len(), 1);
    }

    #[test]
    fn later_final_text_after_a_live_tool_does_not_commit() {
        let mut turn = empty_turn();
        insert_tool(
            &mut turn,
            stable_tool(model::ToolCallStatus::InProgress, TerminalMutationState::Streaming),
        );
        append_text_chunk(&mut turn, "done\n\nlater");

        let decision = plan_handoff(&turn);

        assert_eq!(decision.committed_prefix_len, 0);
        assert!(decision.transcript_entries.is_empty());
    }

    #[test]
    fn stable_suffix_fully_commits_after_prepare_for_turn_exit_completed() {
        let mut turn = empty_turn();
        append_text_chunk(&mut turn, "before");
        insert_tool(
            &mut turn,
            stable_tool(model::ToolCallStatus::InProgress, TerminalMutationState::Streaming),
        );
        append_text_chunk(&mut turn, "after");
        turn.set_live_indicator(Some(LiveAssistantIndicator::Thinking));

        prepare_for_turn_exit(&mut turn, model::ToolCallStatus::Completed);
        let decision = plan_handoff(&turn);

        assert_eq!(decision.committed_prefix_len, 3);
        assert_eq!(decision.transcript_entries.len(), 3);
    }

    #[test]
    fn empty_assistant_turn_produces_no_transcript_entries() {
        let turn = empty_turn();

        let decision = plan_handoff(&turn);

        assert_eq!(decision.committed_prefix_len, 0);
        assert!(decision.transcript_entries.is_empty());
    }

    #[test]
    fn apply_successful_commit_updates_formatting_without_touching_live_indicator() {
        let mut turn = empty_turn();
        append_text_chunk(&mut turn, "before\n\nafter");
        insert_tool(
            &mut turn,
            stable_tool(model::ToolCallStatus::Completed, TerminalMutationState::Settled),
        );
        turn.set_live_indicator(Some(LiveAssistantIndicator::Thinking));

        let decision = plan_handoff(&turn);
        apply_successful_commit(&mut turn, &decision);

        assert!(turn.formatting.header_printed);
        assert_eq!(turn.formatting.previous_committed_kind, Some(CommittedAssistantKind::TextLike));
        assert_eq!(turn.live_indicator, Some(LiveAssistantIndicator::Thinking));
        assert_eq!(turn.units.len(), 1);
        assert!(matches!(turn.units[0], LiveAssistantUnit::Tool(_)));
    }

    #[test]
    fn completion_seals_tail_and_settles_tools() {
        let mut turn = empty_turn();
        append_text_chunk(&mut turn, "live");
        insert_tool(
            &mut turn,
            stable_tool(model::ToolCallStatus::InProgress, TerminalMutationState::Streaming),
        );
        turn.set_live_indicator(Some(LiveAssistantIndicator::Compacting));

        prepare_for_turn_exit(&mut turn, model::ToolCallStatus::Completed);

        assert!(turn.sealed);
        assert!(turn.live_indicator.is_none());
        let LiveAssistantUnit::Tool(tool) = &turn.units[1] else {
            panic!("expected tool");
        };
        assert_eq!(tool.snapshot.status, model::ToolCallStatus::Completed);
        assert_eq!(tool.terminal_mutation, TerminalMutationState::Settled);
    }

    #[test]
    fn completion_drops_hidden_tools_before_commit() {
        let mut turn = empty_turn();
        append_text_chunk(&mut turn, "before");
        insert_tool(&mut turn, hidden_tool());
        append_text_chunk(&mut turn, "after");

        prepare_for_turn_exit(&mut turn, model::ToolCallStatus::Completed);
        let decision = plan_handoff(&turn);

        assert_eq!(turn.units.len(), 2);
        assert!(turn.units.iter().all(|unit| !matches!(
            unit,
            LiveAssistantUnit::Tool(tool) if tool.snapshot.hidden
        )));
        assert_eq!(decision.committed_prefix_len, 2);
        assert_eq!(decision.transcript_entries.len(), 1);
    }

    #[test]
    fn error_or_cancel_exit_preserves_assistant_content_while_clearing_live_indicator() {
        let mut turn = empty_turn();
        append_text_chunk(&mut turn, "hello");
        turn.set_live_indicator(Some(LiveAssistantIndicator::Thinking));

        prepare_for_turn_exit(&mut turn, model::ToolCallStatus::Failed);

        assert!(turn.live_indicator.is_none());
        assert_eq!(turn.units.len(), 1);
        let LiveAssistantUnit::StableText(text) = &turn.units[0] else {
            panic!("expected stable text");
        };
        assert_eq!(text.text, "hello");
    }

    #[test]
    fn empty_assistant_placeholder_remains_empty_and_non_committing_on_exit() {
        let mut turn = empty_turn();
        turn.set_live_indicator(Some(LiveAssistantIndicator::Thinking));

        prepare_for_turn_exit(&mut turn, model::ToolCallStatus::Completed);
        let decision = plan_handoff(&turn);

        assert!(turn.units.is_empty());
        assert!(decision.transcript_entries.is_empty());
        assert_eq!(decision.committed_prefix_len, 0);
    }
}
