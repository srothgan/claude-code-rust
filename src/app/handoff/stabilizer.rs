// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::types::{
    LiveAssistantTurn, LiveAssistantUnit, LiveNoticeUnit, LiveToolUnit, MutableTextTailUnit,
    StableTextUnit,
};
use crate::app::TextBlockSpacing;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StableBoundary {
    split_at: usize,
    trailing_spacing: TextBlockSpacing,
}

pub(crate) fn append_text_chunk(turn: &mut LiveAssistantTurn, chunk: &str) {
    if chunk.is_empty() {
        return;
    }

    let tail_id = if let Some(tail_id) = turn.current_text_tail {
        tail_id
    } else {
        let tail_id = turn.allocate_unit_id();
        turn.units.push(LiveAssistantUnit::MutableTextTail(MutableTextTailUnit {
            id: tail_id,
            text: String::new(),
        }));
        turn.current_text_tail = Some(tail_id);
        tail_id
    };

    if let Some(LiveAssistantUnit::MutableTextTail(tail)) = tail_mut_by_id(turn, tail_id) {
        tail.text.push_str(chunk);
    }

    stabilize_mutable_tail(turn);
}

pub(crate) fn insert_notice(turn: &mut LiveAssistantTurn, mut notice: LiveNoticeUnit) {
    seal_mutable_text_tail(turn, TextBlockSpacing::None);
    notice.id = turn.allocate_unit_id();
    turn.units.push(LiveAssistantUnit::Notice(notice));
}

pub(crate) fn insert_tool(turn: &mut LiveAssistantTurn, mut tool: LiveToolUnit) {
    seal_mutable_text_tail(turn, TextBlockSpacing::None);
    tool.id = turn.allocate_unit_id();
    turn.units.push(LiveAssistantUnit::Tool(tool));
}

pub(crate) fn seal_mutable_text_tail(
    turn: &mut LiveAssistantTurn,
    trailing_spacing: TextBlockSpacing,
) {
    let Some(tail_id) = turn.current_text_tail else {
        return;
    };
    let Some(tail_idx) = tail_index_by_id(turn, tail_id) else {
        turn.current_text_tail = None;
        return;
    };

    let tail = match turn.units.remove(tail_idx) {
        LiveAssistantUnit::MutableTextTail(tail) => tail,
        unit => {
            turn.units.insert(tail_idx, unit);
            turn.refresh_current_text_tail();
            return;
        }
    };

    if tail.text.is_empty() {
        turn.refresh_current_text_tail();
        return;
    }

    turn.units.insert(
        tail_idx,
        LiveAssistantUnit::StableText(StableTextUnit {
            id: tail.id,
            text: tail.text,
            trailing_spacing,
        }),
    );
    turn.current_text_tail = None;
}

fn stabilize_mutable_tail(turn: &mut LiveAssistantTurn) {
    loop {
        let Some(tail_id) = turn.current_text_tail else {
            break;
        };
        let Some(tail_idx) = tail_index_by_id(turn, tail_id) else {
            turn.current_text_tail = None;
            break;
        };

        let Some(boundary) = turn.units.get(tail_idx).and_then(|unit| match unit {
            LiveAssistantUnit::MutableTextTail(tail) => find_stable_boundary(&tail.text),
            _ => None,
        }) else {
            break;
        };

        let tail = match turn.units.remove(tail_idx) {
            LiveAssistantUnit::MutableTextTail(tail) => tail,
            unit => {
                turn.units.insert(tail_idx, unit);
                turn.refresh_current_text_tail();
                break;
            }
        };

        let completed = tail.text[..boundary.split_at].to_owned();
        let remainder = tail.text[boundary.split_at..].to_owned();

        turn.units.insert(
            tail_idx,
            LiveAssistantUnit::StableText(StableTextUnit {
                id: tail.id,
                text: completed,
                trailing_spacing: boundary.trailing_spacing,
            }),
        );

        if remainder.is_empty() {
            turn.current_text_tail = None;
            break;
        }

        let new_tail_id = turn.allocate_unit_id();
        turn.units.insert(
            tail_idx + 1,
            LiveAssistantUnit::MutableTextTail(MutableTextTailUnit {
                id: new_tail_id,
                text: remainder,
            }),
        );
        turn.current_text_tail = Some(new_tail_id);
    }
}

fn tail_mut_by_id(
    turn: &mut LiveAssistantTurn,
    tail_id: super::types::LiveUnitId,
) -> Option<&mut LiveAssistantUnit> {
    let tail_idx = tail_index_by_id(turn, tail_id)?;
    turn.units.get_mut(tail_idx)
}

fn tail_index_by_id(turn: &LiveAssistantTurn, tail_id: super::types::LiveUnitId) -> Option<usize> {
    turn.units.iter().position(
        |unit| matches!(unit, LiveAssistantUnit::MutableTextTail(tail) if tail.id == tail_id),
    )
}

fn find_stable_boundary(text: &str) -> Option<StableBoundary> {
    let mut in_fenced_code = false;
    let mut saw_nonblank = false;
    let mut blank_boundary = None;
    let mut offset = 0usize;

    for line in text.split_inclusive('\n') {
        offset += line.len();
        let line_without_newline = line.trim_end_matches(['\r', '\n']);
        let trimmed = line_without_newline.trim();
        let is_blank = trimmed.is_empty();
        let is_fence = line_without_newline.trim_start().starts_with("```");

        if is_fence {
            in_fenced_code = !in_fenced_code;
            saw_nonblank = true;
            if !in_fenced_code && offset < text.len() {
                return Some(StableBoundary {
                    split_at: offset,
                    trailing_spacing: TextBlockSpacing::None,
                });
            }
            continue;
        }

        if !in_fenced_code && is_blank {
            if saw_nonblank {
                blank_boundary = Some(offset);
            }
            continue;
        }

        if !in_fenced_code
            && let Some(split_at) = blank_boundary.take()
            && split_at < text.len()
        {
            return Some(StableBoundary {
                split_at,
                trailing_spacing: TextBlockSpacing::ParagraphBreak,
            });
        }

        if !is_blank {
            saw_nonblank = true;
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::{append_text_chunk, insert_tool};
    use crate::agent::model;
    use crate::app::TextBlockSpacing;
    use crate::app::handoff::types::{
        AssistantTurnId, LiveAssistantTurn, LiveAssistantUnit, LiveToolUnit, LiveUnitId,
        TerminalMutationState, ToolTranscriptSnapshot,
    };

    fn empty_turn() -> LiveAssistantTurn {
        LiveAssistantTurn::new(AssistantTurnId(1))
    }

    fn tool() -> LiveToolUnit {
        LiveToolUnit {
            id: LiveUnitId(999),
            snapshot: ToolTranscriptSnapshot {
                tool_call_id: "tool-1".to_owned(),
                title: "Tool".to_owned(),
                sdk_tool_name: "Bash".to_owned(),
                status: model::ToolCallStatus::Completed,
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
            terminal_mutation: TerminalMutationState::Settled,
        }
    }

    #[test]
    fn paragraph_break_seals_a_stable_text_segment() {
        let mut turn = empty_turn();

        append_text_chunk(&mut turn, "First paragraph.\n\nSecond paragraph");

        assert_eq!(turn.units.len(), 2);
        let LiveAssistantUnit::StableText(stable) = &turn.units[0] else {
            panic!("expected stable text");
        };
        assert_eq!(stable.text, "First paragraph.\n\n");
        assert_eq!(stable.trailing_spacing, TextBlockSpacing::ParagraphBreak);
        let LiveAssistantUnit::MutableTextTail(tail) = &turn.units[1] else {
            panic!("expected mutable tail");
        };
        assert_eq!(tail.text, "Second paragraph");
    }

    #[test]
    fn closed_fenced_code_block_seals_a_stable_text_segment() {
        let mut turn = empty_turn();

        append_text_chunk(&mut turn, "```rust\nfn main() {}\n```\nAfter");

        assert_eq!(turn.units.len(), 2);
        let LiveAssistantUnit::StableText(stable) = &turn.units[0] else {
            panic!("expected stable text");
        };
        assert_eq!(stable.text, "```rust\nfn main() {}\n```\n");
        assert_eq!(stable.trailing_spacing, TextBlockSpacing::None);
        let LiveAssistantUnit::MutableTextTail(tail) = &turn.units[1] else {
            panic!("expected mutable tail");
        };
        assert_eq!(tail.text, "After");
    }

    #[test]
    fn open_fenced_code_block_does_not_seal_early() {
        let mut turn = empty_turn();

        append_text_chunk(&mut turn, "```rust\nfn main() {}\n");

        assert_eq!(turn.units.len(), 1);
        let LiveAssistantUnit::MutableTextTail(tail) = &turn.units[0] else {
            panic!("expected mutable tail");
        };
        assert_eq!(tail.text, "```rust\nfn main() {}\n");
    }

    #[test]
    fn inserting_a_tool_after_a_mutable_text_tail_seals_that_tail() {
        let mut turn = empty_turn();
        append_text_chunk(&mut turn, "before tool");

        insert_tool(&mut turn, tool());

        assert_eq!(turn.units.len(), 2);
        let LiveAssistantUnit::StableText(stable) = &turn.units[0] else {
            panic!("expected stable text");
        };
        assert_eq!(stable.text, "before tool");
        assert_eq!(stable.trailing_spacing, TextBlockSpacing::None);
        assert!(matches!(turn.units[1], LiveAssistantUnit::Tool(_)));
        assert!(turn.current_text_tail.is_none());
    }

    #[test]
    fn later_text_after_a_tool_creates_a_new_mutable_tail() {
        let mut turn = empty_turn();
        insert_tool(&mut turn, tool());

        append_text_chunk(&mut turn, "after tool");

        assert_eq!(turn.units.len(), 2);
        assert!(matches!(turn.units[0], LiveAssistantUnit::Tool(_)));
        let LiveAssistantUnit::MutableTextTail(tail) = &turn.units[1] else {
            panic!("expected mutable tail");
        };
        assert_eq!(tail.text, "after tool");
        assert_eq!(turn.current_text_tail, Some(tail.id));
    }
}
