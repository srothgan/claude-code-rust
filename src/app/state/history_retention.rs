// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use crate::agent::model;
use std::collections::HashSet;
use std::mem::size_of;

use super::messages::{
    ChatMessage, IncrementalMarkdown, MessageBlock, MessageRole, TextBlock, WelcomeBlock,
};
use super::tool_call_info::{InlinePermission, InlineQuestion, ToolCallInfo};
use super::types::{HistoryRetentionStats, MessageUsage, RecentSessionInfo};
use super::viewport::InvalidationLevel;

const HISTORY_HIDDEN_MARKER_PREFIX: &str = "Older messages hidden to keep memory bounded";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct HistoryDropCandidate {
    pub(super) msg_idx: usize,
    pub(super) bytes: usize,
    pub(super) approx_rows: usize,
}

impl super::App {
    #[must_use]
    pub(super) fn is_history_hidden_marker_message(msg: &ChatMessage) -> bool {
        if !matches!(msg.role, MessageRole::System(_)) {
            return false;
        }
        let Some(MessageBlock::Text(block)) = msg.blocks.first() else {
            return false;
        };
        block.text.starts_with(HISTORY_HIDDEN_MARKER_PREFIX)
    }

    #[must_use]
    pub(super) fn is_history_protected_message(msg: &ChatMessage) -> bool {
        if matches!(msg.role, MessageRole::Welcome) {
            return true;
        }
        msg.blocks.iter().any(|block| {
            if let MessageBlock::ToolCall(tc) = block {
                tc.pending_permission.is_some()
                    || tc.pending_question.is_some()
                    || matches!(
                        tc.status,
                        model::ToolCallStatus::Pending | model::ToolCallStatus::InProgress
                    )
            } else {
                false
            }
        })
    }

    #[must_use]
    fn measure_tool_content_bytes(content: &model::ToolCallContent) -> usize {
        match content {
            model::ToolCallContent::Content(inner) => match &inner.content {
                model::ContentBlock::Text(text) => text.text.capacity(),
                model::ContentBlock::Image(image) => {
                    image.data.capacity().saturating_add(image.mime_type.capacity())
                }
            },
            model::ToolCallContent::Diff(diff) => diff
                .path
                .capacity()
                .saturating_add(diff.old_text.as_ref().map_or(0, String::capacity))
                .saturating_add(diff.new_text.capacity()),
            model::ToolCallContent::McpResource(resource) => resource
                .uri
                .capacity()
                .saturating_add(resource.mime_type.as_ref().map_or(0, String::capacity))
                .saturating_add(resource.text.as_ref().map_or(0, String::capacity))
                .saturating_add(
                    resource.blob_saved_to.as_ref().map_or(0, std::path::PathBuf::capacity),
                ),
            model::ToolCallContent::Terminal(term) => term.terminal_id.capacity(),
        }
    }

    #[must_use]
    fn estimate_json_value_bytes(value: &serde_json::Value) -> usize {
        serde_json::to_string(value).map_or(0, |json| json.len())
    }

    #[must_use]
    fn measure_tool_call_bytes(tc: &ToolCallInfo) -> usize {
        let mut total = size_of::<ToolCallInfo>()
            .saturating_add(tc.id.capacity())
            .saturating_add(tc.title.capacity())
            .saturating_add(tc.sdk_tool_name.capacity())
            .saturating_add(tc.terminal_id.as_ref().map_or(0, String::capacity))
            .saturating_add(tc.terminal_command.as_ref().map_or(0, String::capacity))
            .saturating_add(tc.terminal_output.as_ref().map_or(0, String::capacity))
            .saturating_add(
                tc.content.capacity().saturating_mul(size_of::<model::ToolCallContent>()),
            );

        if let Some(raw_input) = &tc.raw_input {
            total = total.saturating_add(Self::estimate_json_value_bytes(raw_input));
        }
        for content in &tc.content {
            total = total.saturating_add(Self::measure_tool_content_bytes(content));
        }
        if let Some(permission) = &tc.pending_permission {
            total = total.saturating_add(size_of::<InlinePermission>()).saturating_add(
                permission.options.capacity().saturating_mul(size_of::<model::PermissionOption>()),
            );
            for option in &permission.options {
                total = total
                    .saturating_add(option.option_id.capacity())
                    .saturating_add(option.name.capacity())
                    .saturating_add(option.description.as_ref().map_or(0, String::capacity));
            }
        }
        if let Some(question) = &tc.pending_question {
            total = total
                .saturating_add(size_of::<InlineQuestion>())
                .saturating_add(question.prompt.question.capacity())
                .saturating_add(question.prompt.header.capacity())
                .saturating_add(
                    question
                        .prompt
                        .options
                        .capacity()
                        .saturating_mul(size_of::<model::QuestionOption>()),
                )
                .saturating_add(question.notes.capacity());
            for option in &question.prompt.options {
                total = total
                    .saturating_add(option.option_id.capacity())
                    .saturating_add(option.label.capacity())
                    .saturating_add(option.description.as_ref().map_or(0, String::capacity))
                    .saturating_add(option.preview.as_ref().map_or(0, String::capacity));
            }
        }

        total
    }

    /// Measure the approximate in-memory byte footprint of a single message.
    ///
    /// Uses `String::capacity()` and `std::mem::size_of` for actual heap
    /// allocation sizes rather than content-length heuristics.
    #[must_use]
    pub fn measure_message_bytes(msg: &ChatMessage) -> usize {
        let mut total = size_of::<ChatMessage>()
            .saturating_add(msg.blocks.capacity().saturating_mul(size_of::<MessageBlock>()));
        if msg.usage.is_some() {
            total = total.saturating_add(size_of::<MessageUsage>());
        }

        for block in &msg.blocks {
            match block {
                MessageBlock::Text(block) => {
                    total = total
                        .saturating_add(block.text.capacity())
                        .saturating_add(block.markdown.text_capacity());
                }
                MessageBlock::ToolCall(tc) => {
                    total = total.saturating_add(Self::measure_tool_call_bytes(tc));
                }
                MessageBlock::Welcome(welcome) => {
                    total = total
                        .saturating_add(size_of::<WelcomeBlock>())
                        .saturating_add(welcome.model_name.capacity())
                        .saturating_add(welcome.cwd.capacity())
                        .saturating_add(
                            welcome
                                .recent_sessions
                                .capacity()
                                .saturating_mul(size_of::<RecentSessionInfo>()),
                        );
                    for session in &welcome.recent_sessions {
                        total = total
                            .saturating_add(session.session_id.capacity())
                            .saturating_add(session.summary.capacity())
                            .saturating_add(session.cwd.as_ref().map_or(0, String::capacity))
                            .saturating_add(session.git_branch.as_ref().map_or(0, String::capacity))
                            .saturating_add(
                                session.custom_title.as_ref().map_or(0, String::capacity),
                            )
                            .saturating_add(
                                session.first_prompt.as_ref().map_or(0, String::capacity),
                            );
                    }
                }
            }
        }
        total
    }

    /// Measure the total in-memory byte footprint of all retained messages.
    #[must_use]
    pub fn measure_history_bytes(&self) -> usize {
        self.messages.iter().map(Self::measure_message_bytes).sum()
    }

    pub(super) fn rebuild_tool_indices_and_terminal_refs(&mut self) {
        self.tool_call_index.clear();
        self.terminal_tool_calls.clear();

        let mut pending_permission_ids = Vec::new();
        for (msg_idx, msg) in self.messages.iter_mut().enumerate() {
            for (block_idx, block) in msg.blocks.iter_mut().enumerate() {
                if let MessageBlock::ToolCall(tc) = block {
                    let tc = tc.as_mut();
                    self.tool_call_index.insert(tc.id.clone(), (msg_idx, block_idx));
                    if let Some(terminal_id) = tc.terminal_id.clone() {
                        self.terminal_tool_calls.push((terminal_id, msg_idx, block_idx));
                    }
                    if let Some(permission) = tc.pending_permission.as_mut() {
                        permission.focused = false;
                        pending_permission_ids.push(tc.id.clone());
                    }
                    if let Some(question) = tc.pending_question.as_mut() {
                        question.focused = false;
                        pending_permission_ids.push(tc.id.clone());
                    }
                }
            }
        }

        let permission_set: HashSet<&str> =
            pending_permission_ids.iter().map(String::as_str).collect();
        self.pending_permission_ids.retain(|id| permission_set.contains(id.as_str()));
        for id in pending_permission_ids {
            if !self.pending_permission_ids.iter().any(|existing| existing == &id) {
                self.pending_permission_ids.push(id);
            }
        }

        if let Some(first_id) = self.pending_permission_ids.first().cloned() {
            self.claim_focus_target(super::super::focus::FocusTarget::Permission);
            if let Some((msg_idx, block_idx)) = self.lookup_tool_call(&first_id)
                && let Some(MessageBlock::ToolCall(tc)) =
                    self.messages.get_mut(msg_idx).and_then(|m| m.blocks.get_mut(block_idx))
            {
                if let Some(permission) = tc.pending_permission.as_mut() {
                    permission.focused = true;
                }
                if let Some(question) = tc.pending_question.as_mut() {
                    question.focused = true;
                }
            }
        } else {
            self.release_focus_target(super::super::focus::FocusTarget::Permission);
        }
        self.normalize_focus_stack();
    }

    #[must_use]
    fn format_mib_tenths(bytes: usize) -> String {
        let tenths =
            (u128::try_from(bytes).unwrap_or(u128::MAX).saturating_mul(10) + 524_288) / 1_048_576;
        format!("{}.{}", tenths / 10, tenths % 10)
    }

    #[must_use]
    fn history_hidden_marker_text(
        total_dropped_messages: usize,
        total_dropped_bytes: usize,
    ) -> String {
        format!(
            "{HISTORY_HIDDEN_MARKER_PREFIX} (dropped {total_dropped_messages} messages, {} MiB).",
            Self::format_mib_tenths(total_dropped_bytes)
        )
    }

    fn upsert_history_hidden_marker(&mut self) {
        let marker_idx = self.messages.iter().position(Self::is_history_hidden_marker_message);
        if self.history_retention_stats.total_dropped_messages == 0 {
            if let Some(idx) = marker_idx {
                self.messages.remove(idx);
                self.invalidate_layout(InvalidationLevel::From(idx));
                self.rebuild_tool_indices_and_terminal_refs();
            }
            return;
        }

        let marker_text = Self::history_hidden_marker_text(
            self.history_retention_stats.total_dropped_messages,
            self.history_retention_stats.total_dropped_bytes,
        );

        if let Some(idx) = marker_idx {
            if let Some(MessageBlock::Text(block)) =
                self.messages.get_mut(idx).and_then(|m| m.blocks.get_mut(0))
                && block.text != marker_text
            {
                block.text.clone_from(&marker_text);
                block.markdown = IncrementalMarkdown::from_complete(&marker_text);
                block.cache.invalidate();
                self.invalidate_layout(InvalidationLevel::From(idx));
            }
            return;
        }

        let insert_idx = usize::from(
            self.messages.first().is_some_and(|msg| matches!(msg.role, MessageRole::Welcome)),
        );
        self.messages.insert(
            insert_idx,
            ChatMessage {
                role: MessageRole::System(None),
                blocks: vec![MessageBlock::Text(TextBlock::from_complete(&marker_text))],
                usage: None,
            },
        );
        self.invalidate_layout(InvalidationLevel::From(insert_idx));
        self.rebuild_tool_indices_and_terminal_refs();
    }

    #[allow(clippy::cast_precision_loss)]
    pub fn enforce_history_retention(&mut self) -> HistoryRetentionStats {
        let mut stats = HistoryRetentionStats::default();
        let max_bytes = self.history_retention.max_bytes.max(1);
        stats.total_before_bytes = self.measure_history_bytes();
        stats.total_after_bytes = stats.total_before_bytes;

        if stats.total_before_bytes > max_bytes {
            let mut candidates = Vec::new();
            for (msg_idx, msg) in self.messages.iter().enumerate() {
                if Self::is_history_hidden_marker_message(msg)
                    || Self::is_history_protected_message(msg)
                {
                    continue;
                }
                let bytes = Self::measure_message_bytes(msg);
                if bytes == 0 {
                    continue;
                }
                candidates.push(HistoryDropCandidate {
                    msg_idx,
                    bytes,
                    approx_rows: self.viewport.message_height(msg_idx),
                });
            }

            let mut drop_candidates = Vec::new();
            for candidate in candidates {
                if stats.total_after_bytes <= max_bytes {
                    break;
                }
                stats.total_after_bytes = stats.total_after_bytes.saturating_sub(candidate.bytes);
                stats.dropped_bytes = stats.dropped_bytes.saturating_add(candidate.bytes);
                stats.dropped_messages = stats.dropped_messages.saturating_add(1);
                drop_candidates.push(candidate);
            }

            if !drop_candidates.is_empty() {
                let mut dropped_rows = 0usize;
                let drop_set: HashSet<usize> = drop_candidates
                    .iter()
                    .map(|candidate| {
                        dropped_rows = dropped_rows.saturating_add(candidate.approx_rows);
                        candidate.msg_idx
                    })
                    .collect();

                let mut retained =
                    Vec::with_capacity(self.messages.len().saturating_sub(drop_set.len()));
                for (msg_idx, msg) in self.messages.drain(..).enumerate() {
                    if !drop_set.contains(&msg_idx) {
                        retained.push(msg);
                    }
                }
                self.messages = retained;

                if !self.viewport.auto_scroll && dropped_rows > 0 {
                    self.viewport.scroll_target =
                        self.viewport.scroll_target.saturating_sub(dropped_rows);
                    self.viewport.scroll_offset =
                        self.viewport.scroll_offset.saturating_sub(dropped_rows);
                    let dropped_rows_f = dropped_rows as f32;
                    self.viewport.scroll_pos = if self.viewport.scroll_pos > dropped_rows_f {
                        self.viewport.scroll_pos - dropped_rows_f
                    } else {
                        0.0
                    };
                }
                self.rebuild_tool_indices_and_terminal_refs();
                self.invalidate_layout(InvalidationLevel::From(0));
                self.needs_redraw = true;
            }
        }

        self.history_retention_stats.total_before_bytes = stats.total_before_bytes;
        self.history_retention_stats.total_dropped_messages = self
            .history_retention_stats
            .total_dropped_messages
            .saturating_add(stats.dropped_messages);
        self.history_retention_stats.total_dropped_bytes =
            self.history_retention_stats.total_dropped_bytes.saturating_add(stats.dropped_bytes);

        self.upsert_history_hidden_marker();

        stats.total_after_bytes = self.measure_history_bytes();
        self.history_retention_stats.total_after_bytes = stats.total_after_bytes;
        self.history_retention_stats.dropped_messages = stats.dropped_messages;
        self.history_retention_stats.dropped_bytes = stats.dropped_bytes;

        stats.total_dropped_messages = self.history_retention_stats.total_dropped_messages;
        stats.total_dropped_bytes = self.history_retention_stats.total_dropped_bytes;

        crate::perf::mark_with("history::bytes_before", "bytes", stats.total_before_bytes);
        crate::perf::mark_with("history::bytes_after", "bytes", stats.total_after_bytes);
        crate::perf::mark_with("history::dropped_messages", "count", stats.dropped_messages);
        crate::perf::mark_with("history::dropped_bytes", "bytes", stats.dropped_bytes);
        crate::perf::mark_with("history::total_dropped", "count", stats.total_dropped_messages);

        stats
    }
}
