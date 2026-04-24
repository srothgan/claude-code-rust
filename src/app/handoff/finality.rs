// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::types::{
    LiveAssistantTurn, LiveAssistantUnit, LiveToolUnit, NoticeMutability, TerminalMutationState,
};
use crate::agent::model::ToolCallStatus;

#[must_use]
pub(crate) fn unit_is_final(unit: &LiveAssistantUnit) -> bool {
    match unit {
        LiveAssistantUnit::StableText(_) => true,
        LiveAssistantUnit::MutableTextTail(_) => false,
        LiveAssistantUnit::Notice(notice) => notice.mutability == NoticeMutability::Final,
        LiveAssistantUnit::Tool(tool) => tool_is_final(tool),
    }
}

#[must_use]
pub(crate) fn committed_prefix_len(turn: &LiveAssistantTurn) -> usize {
    let mut len = 0usize;
    for (idx, unit) in turn.units.iter().enumerate() {
        let has_following_unit = idx + 1 < turn.units.len();
        if !unit_is_committable(unit, turn.sealed, has_following_unit) {
            break;
        }
        len += 1;
    }
    len
}

#[must_use]
fn unit_is_committable(
    unit: &LiveAssistantUnit,
    turn_is_sealed: bool,
    has_following_unit: bool,
) -> bool {
    match unit {
        LiveAssistantUnit::Tool(_) => (turn_is_sealed || has_following_unit) && unit_is_final(unit),
        LiveAssistantUnit::StableText(_)
        | LiveAssistantUnit::MutableTextTail(_)
        | LiveAssistantUnit::Notice(_) => unit_is_final(unit),
    }
}

#[must_use]
fn tool_is_final(tool: &LiveToolUnit) -> bool {
    is_terminal_status(tool.snapshot.status)
        && !tool.pending_permission
        && !tool.pending_question
        && matches!(
            tool.terminal_mutation,
            TerminalMutationState::None | TerminalMutationState::Settled
        )
}

#[must_use]
fn is_terminal_status(status: ToolCallStatus) -> bool {
    matches!(status, ToolCallStatus::Completed | ToolCallStatus::Failed | ToolCallStatus::Killed)
}

#[cfg(test)]
mod tests {
    use super::{committed_prefix_len, unit_is_final};
    use crate::agent::model;
    use crate::app::handoff::types::{
        AssistantTurnId, LiveAssistantTurn, LiveAssistantUnit, LiveNoticeUnit, LiveToolUnit,
        LiveUnitId, MutableTextTailUnit, NoticeMutability, StableTextUnit, TerminalMutationState,
        ToolTranscriptSnapshot,
    };
    use crate::app::{SystemSeverity, TextBlockSpacing};

    fn tool_with(
        status: model::ToolCallStatus,
        terminal_mutation: TerminalMutationState,
        pending_permission: bool,
        pending_question: bool,
    ) -> LiveAssistantUnit {
        LiveAssistantUnit::Tool(LiveToolUnit {
            id: LiveUnitId(1),
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
            pending_permission,
            pending_question,
            terminal_mutation,
        })
    }

    #[test]
    fn stable_text_is_final() {
        let unit = LiveAssistantUnit::StableText(StableTextUnit {
            id: LiveUnitId(1),
            text: "done".to_owned(),
            trailing_spacing: TextBlockSpacing::None,
        });

        assert!(unit_is_final(&unit));
    }

    #[test]
    fn mutable_text_tail_is_not_final() {
        let unit = LiveAssistantUnit::MutableTextTail(MutableTextTailUnit {
            id: LiveUnitId(1),
            text: "live".to_owned(),
        });

        assert!(!unit_is_final(&unit));
    }

    #[test]
    fn upgradeable_notice_is_not_final() {
        let unit = LiveAssistantUnit::Notice(LiveNoticeUnit {
            id: LiveUnitId(1),
            dedup_key: None,
            severity: SystemSeverity::Info,
            text: "notice".to_owned(),
            trailing_spacing: TextBlockSpacing::None,
            mutability: NoticeMutability::Upgradeable,
        });

        assert!(!unit_is_final(&unit));
    }

    #[test]
    fn final_notice_is_final() {
        let unit = LiveAssistantUnit::Notice(LiveNoticeUnit {
            id: LiveUnitId(1),
            dedup_key: None,
            severity: SystemSeverity::Info,
            text: "notice".to_owned(),
            trailing_spacing: TextBlockSpacing::None,
            mutability: NoticeMutability::Final,
        });

        assert!(unit_is_final(&unit));
    }

    #[test]
    fn tool_with_terminal_status_and_streaming_terminal_is_not_final() {
        let unit = tool_with(
            model::ToolCallStatus::Completed,
            TerminalMutationState::Streaming,
            false,
            false,
        );

        assert!(!unit_is_final(&unit));
    }

    #[test]
    fn tool_with_pending_permission_is_not_final() {
        let unit = tool_with(
            model::ToolCallStatus::Completed,
            TerminalMutationState::Settled,
            true,
            false,
        );

        assert!(!unit_is_final(&unit));
    }

    #[test]
    fn tool_with_pending_question_is_not_final() {
        let unit = tool_with(
            model::ToolCallStatus::Completed,
            TerminalMutationState::Settled,
            false,
            true,
        );

        assert!(!unit_is_final(&unit));
    }

    #[test]
    fn tool_with_terminal_status_and_settled_terminal_is_final() {
        let unit = tool_with(
            model::ToolCallStatus::Completed,
            TerminalMutationState::Settled,
            false,
            false,
        );

        assert!(unit_is_final(&unit));
    }

    #[test]
    fn unsealed_turn_keeps_trailing_final_tool_live() {
        let mut turn = LiveAssistantTurn::new(AssistantTurnId(1));
        turn.units.push(tool_with(
            model::ToolCallStatus::Completed,
            TerminalMutationState::Settled,
            false,
            false,
        ));

        assert_eq!(committed_prefix_len(&turn), 0);
    }

    #[test]
    fn unsealed_turn_commits_final_tool_with_following_live_unit() {
        let mut turn = LiveAssistantTurn::new(AssistantTurnId(1));
        turn.units.push(tool_with(
            model::ToolCallStatus::Completed,
            TerminalMutationState::Settled,
            false,
            false,
        ));
        turn.units.push(LiveAssistantUnit::MutableTextTail(MutableTextTailUnit {
            id: LiveUnitId(2),
            text: "after tool".to_owned(),
        }));

        assert_eq!(committed_prefix_len(&turn), 1);
    }

    #[test]
    fn sealed_turn_commits_final_tool_prefix() {
        let mut turn = LiveAssistantTurn::new(AssistantTurnId(1));
        turn.units.push(tool_with(
            model::ToolCallStatus::Completed,
            TerminalMutationState::Settled,
            false,
            false,
        ));
        turn.sealed = true;

        assert_eq!(committed_prefix_len(&turn), 1);
    }
}
