// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::types::{
    AssistantCommittedUnit, AssistantFormattingState, AssistantTranscriptEntry,
    CommittedAssistantKind, CommittedNoticeUnit, CommittedTextUnit, CommittedToolUnit,
    LiveAssistantUnit, TranscriptEntry,
};
use crate::app::TextBlockSpacing;

pub(crate) fn serialize_assistant_prefix(
    units: &[LiveAssistantUnit],
    formatting: &AssistantFormattingState,
) -> Vec<TranscriptEntry> {
    let committed_units = committed_units_from_live(units);
    let mut entries = Vec::with_capacity(committed_units.len());
    let mut previous_kind = formatting.previous_committed_kind;
    let mut header_printed = formatting.header_printed;

    for committed in committed_units {
        let committed_kind = committed.kind();
        let leading_blank_lines = match (previous_kind, committed_kind) {
            (None, _)
            | (Some(CommittedAssistantKind::TextLike), CommittedAssistantKind::TextLike)
            | (Some(CommittedAssistantKind::Tool), CommittedAssistantKind::Tool) => 0,
            (Some(CommittedAssistantKind::TextLike), CommittedAssistantKind::Tool)
            | (Some(CommittedAssistantKind::Tool), CommittedAssistantKind::TextLike) => 1,
        };
        let entry = AssistantTranscriptEntry { leading_blank_lines, unit: committed };

        if header_printed {
            entries.push(TranscriptEntry::AssistantContinue(entry));
        } else {
            entries.push(TranscriptEntry::AssistantOpen(entry));
            header_printed = true;
        }

        previous_kind = Some(committed_kind);
    }

    entries
}

fn committed_units_from_live(units: &[LiveAssistantUnit]) -> Vec<AssistantCommittedUnit> {
    let mut committed_units = Vec::with_capacity(units.len());
    for unit in units {
        match unit {
            LiveAssistantUnit::StableText(text) => {
                if let Some(AssistantCommittedUnit::Text(previous)) = committed_units.last_mut() {
                    append_text_run(
                        &mut previous.text,
                        previous.trailing_spacing,
                        text.text.as_str(),
                    );
                    previous.trailing_spacing = text.trailing_spacing;
                } else {
                    committed_units.push(AssistantCommittedUnit::Text(CommittedTextUnit {
                        text: text.text.clone(),
                        trailing_spacing: text.trailing_spacing,
                    }));
                }
            }
            LiveAssistantUnit::Notice(_)
            | LiveAssistantUnit::Tool(_)
            | LiveAssistantUnit::MutableTextTail(_) => {
                committed_units.push(committed_unit_from_live(unit));
            }
        }
    }
    committed_units
}

fn append_text_run(existing: &mut String, existing_spacing: TextBlockSpacing, text: &str) {
    if existing.is_empty() || text.is_empty() {
        existing.push_str(text);
        return;
    }

    if !text.starts_with('\n') {
        match existing_spacing {
            TextBlockSpacing::None if !existing.ends_with('\n') => existing.push('\n'),
            TextBlockSpacing::ParagraphBreak if !existing.ends_with("\n\n") => {
                if existing.ends_with('\n') {
                    existing.push('\n');
                } else {
                    existing.push_str("\n\n");
                }
            }
            TextBlockSpacing::None | TextBlockSpacing::ParagraphBreak => {}
        }
    }

    existing.push_str(text);
}

fn committed_unit_from_live(unit: &LiveAssistantUnit) -> AssistantCommittedUnit {
    match unit {
        LiveAssistantUnit::StableText(text) => AssistantCommittedUnit::Text(CommittedTextUnit {
            text: text.text.clone(),
            trailing_spacing: text.trailing_spacing,
        }),
        LiveAssistantUnit::Notice(notice) => AssistantCommittedUnit::Notice(CommittedNoticeUnit {
            severity: notice.severity,
            text: notice.text.clone(),
            trailing_spacing: notice.trailing_spacing,
        }),
        LiveAssistantUnit::Tool(tool) => {
            AssistantCommittedUnit::Tool(Box::new(CommittedToolUnit {
                snapshot: tool.snapshot.clone(),
            }))
        }
        LiveAssistantUnit::MutableTextTail(_) => {
            debug_assert!(false, "mutable text tails must not be serialized");
            AssistantCommittedUnit::Text(CommittedTextUnit {
                text: String::new(),
                trailing_spacing: TextBlockSpacing::None,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::serialize_assistant_prefix;
    use crate::agent::model;
    use crate::app::handoff::types::{
        AssistantCommittedUnit, AssistantFormattingState, CommittedAssistantKind,
        LiveAssistantUnit, LiveNoticeUnit, LiveToolUnit, LiveUnitId, NoticeMutability,
        StableTextUnit, TerminalMutationState, ToolTranscriptSnapshot, TranscriptEntry,
    };
    use crate::app::{SystemSeverity, TextBlockSpacing};

    fn stable_text(text: &str, spacing: TextBlockSpacing) -> LiveAssistantUnit {
        LiveAssistantUnit::StableText(StableTextUnit {
            id: LiveUnitId(1),
            text: text.to_owned(),
            trailing_spacing: spacing,
        })
    }

    fn stable_notice(text: &str) -> LiveAssistantUnit {
        LiveAssistantUnit::Notice(LiveNoticeUnit {
            id: LiveUnitId(2),
            dedup_key: None,
            severity: SystemSeverity::Info,
            text: text.to_owned(),
            trailing_spacing: TextBlockSpacing::ParagraphBreak,
            mutability: NoticeMutability::Final,
        })
    }

    fn stable_tool(id: &str) -> LiveAssistantUnit {
        LiveAssistantUnit::Tool(LiveToolUnit {
            id: LiveUnitId(3),
            snapshot: ToolTranscriptSnapshot {
                tool_call_id: id.to_owned(),
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
        })
    }

    #[test]
    fn first_committed_assistant_unit_emits_assistant_open() {
        let entries = serialize_assistant_prefix(
            &[stable_text("hello", TextBlockSpacing::None)],
            &AssistantFormattingState::default(),
        );

        assert!(matches!(entries[0], TranscriptEntry::AssistantOpen(_)));
    }

    #[test]
    fn later_committed_assistant_unit_emits_assistant_continue() {
        let entries = serialize_assistant_prefix(
            &[stable_text("hello", TextBlockSpacing::None)],
            &AssistantFormattingState {
                header_printed: true,
                previous_committed_kind: Some(CommittedAssistantKind::TextLike),
            },
        );

        assert!(matches!(entries[0], TranscriptEntry::AssistantContinue(_)));
    }

    #[test]
    fn assistant_header_is_not_duplicated_across_batches() {
        let entries = serialize_assistant_prefix(
            &[
                stable_text("hello", TextBlockSpacing::None),
                stable_text("world", TextBlockSpacing::None),
            ],
            &AssistantFormattingState {
                header_printed: true,
                previous_committed_kind: Some(CommittedAssistantKind::TextLike),
            },
        );

        assert!(entries.iter().all(|entry| matches!(entry, TranscriptEntry::AssistantContinue(_))));
    }

    #[test]
    fn adjacent_stable_text_units_serialize_as_one_text_run() {
        let entries = serialize_assistant_prefix(
            &[
                stable_text("line 1\n\n", TextBlockSpacing::ParagraphBreak),
                stable_text("line 2", TextBlockSpacing::None),
            ],
            &AssistantFormattingState::default(),
        );

        assert_eq!(entries.len(), 1);
        let TranscriptEntry::AssistantOpen(entry) = &entries[0] else {
            panic!("expected assistant open");
        };
        let AssistantCommittedUnit::Text(text) = &entry.unit else {
            panic!("expected text unit");
        };
        assert_eq!(text.text, "line 1\n\nline 2");
        assert_eq!(text.trailing_spacing, TextBlockSpacing::None);
    }

    #[test]
    fn adjacent_text_units_without_source_newline_keep_visual_boundary() {
        let entries = serialize_assistant_prefix(
            &[
                stable_text("before", TextBlockSpacing::None),
                stable_text("after", TextBlockSpacing::None),
            ],
            &AssistantFormattingState::default(),
        );

        assert_eq!(entries.len(), 1);
        let TranscriptEntry::AssistantOpen(entry) = &entries[0] else {
            panic!("expected assistant open");
        };
        let AssistantCommittedUnit::Text(text) = &entry.unit else {
            panic!("expected text unit");
        };
        assert_eq!(text.text, "before\nafter");
    }

    #[test]
    fn text_like_to_tool_gets_one_leading_blank_line() {
        let entries = serialize_assistant_prefix(
            &[stable_text("hello", TextBlockSpacing::None), stable_tool("tool-1")],
            &AssistantFormattingState::default(),
        );

        let TranscriptEntry::AssistantContinue(tool_entry) = &entries[1] else {
            panic!("expected assistant continue");
        };
        assert_eq!(tool_entry.leading_blank_lines, 1);
    }

    #[test]
    fn tool_to_text_like_gets_one_leading_blank_line() {
        let entries = serialize_assistant_prefix(
            &[stable_tool("tool-1"), stable_text("after", TextBlockSpacing::None)],
            &AssistantFormattingState::default(),
        );

        let TranscriptEntry::AssistantContinue(text_entry) = &entries[1] else {
            panic!("expected assistant continue");
        };
        assert_eq!(text_entry.leading_blank_lines, 1);
    }

    #[test]
    fn tool_to_tool_gets_no_synthetic_blank_line() {
        let entries = serialize_assistant_prefix(
            &[stable_tool("tool-1"), stable_tool("tool-2")],
            &AssistantFormattingState::default(),
        );

        let TranscriptEntry::AssistantContinue(tool_entry) = &entries[1] else {
            panic!("expected assistant continue");
        };
        assert_eq!(tool_entry.leading_blank_lines, 0);
    }

    #[test]
    fn paragraph_break_survives_serialization() {
        let entries = serialize_assistant_prefix(
            &[stable_notice("note")],
            &AssistantFormattingState::default(),
        );

        let TranscriptEntry::AssistantOpen(entry) = &entries[0] else {
            panic!("expected assistant open");
        };
        let AssistantCommittedUnit::Notice(notice) = &entry.unit else {
            panic!("expected notice unit");
        };
        assert_eq!(notice.trailing_spacing, TextBlockSpacing::ParagraphBreak);
    }
}
