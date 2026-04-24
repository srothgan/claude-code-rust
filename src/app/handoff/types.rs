// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::super::state::{
    NoticeBlock, NoticeDedupKey, SystemSeverity, TextBlockSpacing, ToolCallInfo,
};
use crate::agent::model;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct AssistantTurnId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct LiveUnitId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiveAssistantTurn {
    pub turn_id: AssistantTurnId,
    pub units: Vec<LiveAssistantUnit>,
    pub formatting: AssistantFormattingState,
    pub current_text_tail: Option<LiveUnitId>,
    pub live_indicator: Option<LiveAssistantIndicator>,
    pub sealed: bool,
    next_unit_id: u64,
}

impl LiveAssistantTurn {
    #[must_use]
    pub(crate) fn new(turn_id: AssistantTurnId) -> Self {
        Self {
            turn_id,
            units: Vec::new(),
            formatting: AssistantFormattingState::default(),
            current_text_tail: None,
            live_indicator: None,
            sealed: false,
            next_unit_id: 1,
        }
    }

    pub(crate) fn tool_mut_by_call_id(&mut self, tool_call_id: &str) -> Option<&mut LiveToolUnit> {
        self.units.iter_mut().find_map(|unit| match unit {
            LiveAssistantUnit::Tool(tool) if tool.snapshot.tool_call_id == tool_call_id => {
                Some(tool)
            }
            _ => None,
        })
    }

    pub(crate) fn notice_mut_by_key(
        &mut self,
        key: &NoticeDedupKey,
    ) -> Option<&mut LiveNoticeUnit> {
        self.units.iter_mut().find_map(|unit| match unit {
            LiveAssistantUnit::Notice(notice) if notice.dedup_key.as_ref() == Some(key) => {
                Some(notice)
            }
            _ => None,
        })
    }

    pub(crate) fn set_live_indicator(&mut self, indicator: Option<LiveAssistantIndicator>) {
        self.live_indicator = indicator;
    }

    pub(crate) fn allocate_unit_id(&mut self) -> LiveUnitId {
        let id = LiveUnitId(self.next_unit_id);
        self.next_unit_id = self.next_unit_id.saturating_add(1);
        id
    }

    pub(crate) fn refresh_current_text_tail(&mut self) {
        let mut tail_id = None;
        for unit in &self.units {
            if let LiveAssistantUnit::MutableTextTail(tail) = unit {
                debug_assert!(tail_id.is_none(), "multiple mutable tails are not allowed");
                tail_id = Some(tail.id);
            }
        }
        self.current_text_tail = tail_id;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LiveAssistantUnit {
    StableText(StableTextUnit),
    MutableTextTail(MutableTextTailUnit),
    Notice(LiveNoticeUnit),
    Tool(LiveToolUnit),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LiveAssistantIndicator {
    Thinking,
    Compacting,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TranscriptEntry {
    Welcome(WelcomeTranscriptEntry),
    User(UserTranscriptEntry),
    System(SystemTranscriptEntry),
    AssistantOpen(AssistantTranscriptEntry),
    AssistantContinue(AssistantTranscriptEntry),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AssistantTranscriptEntry {
    pub leading_blank_lines: u8,
    pub unit: AssistantCommittedUnit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AssistantCommittedUnit {
    Text(CommittedTextUnit),
    Notice(CommittedNoticeUnit),
    Tool(Box<CommittedToolUnit>),
}

impl AssistantCommittedUnit {
    #[must_use]
    pub(crate) fn kind(&self) -> CommittedAssistantKind {
        match self {
            Self::Text(_) | Self::Notice(_) => CommittedAssistantKind::TextLike,
            Self::Tool(_) => CommittedAssistantKind::Tool,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StableTextUnit {
    pub id: LiveUnitId,
    pub text: String,
    pub trailing_spacing: TextBlockSpacing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MutableTextTailUnit {
    pub id: LiveUnitId,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiveNoticeUnit {
    pub id: LiveUnitId,
    pub dedup_key: Option<NoticeDedupKey>,
    pub severity: SystemSeverity,
    pub text: String,
    pub trailing_spacing: TextBlockSpacing,
    pub mutability: NoticeMutability,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoticeMutability {
    Upgradeable,
    Final,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiveToolUnit {
    pub id: LiveUnitId,
    pub snapshot: ToolTranscriptSnapshot,
    pub pending_permission: bool,
    pub pending_question: bool,
    pub terminal_mutation: TerminalMutationState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalMutationState {
    None,
    Streaming,
    AwaitingFinalSnapshot,
    Settled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolTranscriptSnapshot {
    pub tool_call_id: String,
    pub title: String,
    pub sdk_tool_name: String,
    pub status: model::ToolCallStatus,
    pub hidden: bool,
    pub raw_input: Option<serde_json::Value>,
    pub output_metadata: Option<model::ToolOutputMetadata>,
    pub task_metadata: Option<model::TaskMetadata>,
    pub content: Vec<model::ToolCallContent>,
    pub terminal_command: Option<String>,
    pub terminal_output: Option<String>,
}

impl ToolTranscriptSnapshot {
    #[must_use]
    pub(crate) fn from_tool_call_info(tc: &ToolCallInfo) -> Self {
        Self {
            tool_call_id: tc.id.clone(),
            title: tc.title.clone(),
            sdk_tool_name: tc.sdk_tool_name.clone(),
            status: tc.status,
            hidden: tc.hidden,
            raw_input: tc.raw_input.clone(),
            output_metadata: tc.output_metadata.clone(),
            task_metadata: tc.task_metadata.clone(),
            content: tc.content.clone(),
            terminal_command: tc.terminal_command.clone(),
            terminal_output: tc.terminal_output.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct AssistantFormattingState {
    pub header_printed: bool,
    pub previous_committed_kind: Option<CommittedAssistantKind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommittedAssistantKind {
    TextLike,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HandoffDecision {
    pub committed_prefix_len: usize,
    pub transcript_entries: Vec<TranscriptEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WelcomeTranscriptEntry {
    pub version: String,
    pub subscription: String,
    pub cwd: String,
    pub session_id: String,
    pub tip_seed: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UserTranscriptEntry {
    pub blocks: Vec<UserTranscriptBlock>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UserTranscriptBlock {
    Text(String),
    ImageAttachment { count: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SystemTranscriptEntry {
    pub severity: Option<SystemSeverity>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommittedTextUnit {
    pub text: String,
    pub trailing_spacing: TextBlockSpacing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommittedNoticeUnit {
    pub severity: SystemSeverity,
    pub text: String,
    pub trailing_spacing: TextBlockSpacing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommittedToolUnit {
    pub snapshot: ToolTranscriptSnapshot,
}

#[must_use]
pub(crate) fn inline_notice_to_live(notice: &NoticeBlock, id: LiveUnitId) -> LiveNoticeUnit {
    LiveNoticeUnit {
        id,
        dedup_key: notice.dedup_key.clone(),
        severity: notice.severity,
        text: notice.text.text.clone(),
        trailing_spacing: notice.text.trailing_spacing,
        mutability: NoticeMutability::Upgradeable,
    }
}
