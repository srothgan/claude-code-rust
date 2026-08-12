// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use crate::agent::model;
use std::collections::HashSet;
use std::mem::{size_of, size_of_val};

use super::LayoutInvalidation as InvalidationLevel;
use super::messages::{
    ChatMessage, IncrementalMarkdown, MessageBlock, MessageRole, NoticeDedupKey, TextBlock,
    WelcomeBlock,
};
use super::tool_call_info::{InlinePermission, InlineQuestion, ToolCallInfo};
use super::types::{HistoryRetentionStats, MessageUsage};

const HISTORY_HIDDEN_MARKER_PREFIX: &str = "Older messages hidden to keep memory bounded";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct HistoryDropCandidate {
    pub(super) msg_idx: usize,
    pub(super) bytes: usize,
}

impl super::App {
    fn invalidate_tail_transition(
        &mut self,
        previous_tail_after_mutation: Option<usize>,
        new_tail: Option<usize>,
    ) {
        if previous_tail_after_mutation.is_some() || new_tail.is_some() {
            self.invalidate_layout(InvalidationLevel::Global);
        }
    }

    fn sync_after_message_topology_change(&mut self, start_idx: usize) {
        self.rebuild_tool_indices();
        if self.transcript.messages.is_empty() {
            return;
        }
        self.invalidate_layout(InvalidationLevel::MessagesFrom(start_idx));
    }

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
        msg.blocks.iter().any(|block| match block {
            MessageBlock::ToolCall(tc) => {
                tc.pending_permission.is_some()
                    || tc.pending_question.is_some()
                    || matches!(
                        tc.status,
                        model::ToolCallStatus::Pending | model::ToolCallStatus::InProgress
                    )
            }
            MessageBlock::UserDialog(dialog) => !dialog.answered,
            _ => false,
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

        total = total.saturating_add(tc.raw_input_bytes);
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
                MessageBlock::Notice(block) => {
                    total = total
                        .saturating_add(size_of_val(block))
                        .saturating_add(block.text.text.capacity())
                        .saturating_add(block.text.markdown.text_capacity());
                    if let Some(dedup_key) = &block.dedup_key {
                        total = total.saturating_add(size_of_val(dedup_key));
                        total = total.saturating_add(match dedup_key {
                            NoticeDedupKey::RateLimit(incident) => {
                                incident.rate_limit_type.as_ref().map_or(0, String::capacity)
                            }
                            NoticeDedupKey::ApiRetry
                            | NoticeDedupKey::ActiveTurnSubmissionBlocked => 0,
                        });
                    }
                }
                MessageBlock::ToolCall(tc) => {
                    total = total.saturating_add(Self::measure_tool_call_bytes(tc));
                }
                MessageBlock::Welcome(welcome) => {
                    total = total
                        .saturating_add(size_of::<WelcomeBlock>())
                        .saturating_add(welcome.version.capacity())
                        .saturating_add(welcome.subscription.capacity())
                        .saturating_add(welcome.cwd.capacity())
                        .saturating_add(welcome.session_id.capacity());
                }
                MessageBlock::ImageAttachment(_) => {
                    total =
                        total.saturating_add(size_of::<super::messages::ImageAttachmentBlock>());
                }
                MessageBlock::UserDialog(dialog) => {
                    total = total
                        .saturating_add(size_of::<super::messages::UserDialogBlock>())
                        .saturating_add(dialog.request_id.capacity())
                        .saturating_add(dialog.payload.original_model.capacity())
                        .saturating_add(dialog.payload.fallback_model.capacity())
                        .saturating_add(
                            dialog
                                .options
                                .iter()
                                .map(|option| option.option_id.capacity() + option.label.capacity())
                                .sum::<usize>(),
                        );
                }
            }
        }
        total
    }

    /// Measure the total in-memory byte footprint of all retained messages.
    #[must_use]
    pub fn measure_history_bytes(&self) -> usize {
        self.transcript.messages.iter().map(Self::measure_message_bytes).sum()
    }

    pub(crate) fn rebuild_history_retention_accounting(&mut self) {
        self.transcript.message_retained_bytes.clear();
        self.transcript.message_retained_bytes.reserve(self.transcript.messages.len());
        self.transcript.retained_history_bytes = 0;

        for msg in &self.transcript.messages {
            let bytes = Self::measure_message_bytes(msg);
            self.transcript.message_retained_bytes.push(bytes);
            self.transcript.retained_history_bytes =
                self.transcript.retained_history_bytes.saturating_add(bytes);
        }
    }

    pub(crate) fn ensure_history_retention_accounting(&mut self) {
        if self.transcript.message_retained_bytes.len() != self.transcript.messages.len() {
            self.rebuild_history_retention_accounting();
        }
    }

    pub(crate) fn push_message_tracked(&mut self, msg: ChatMessage) {
        let previous_tail = self.transcript.messages.len().checked_sub(1);
        let bytes = Self::measure_message_bytes(&msg);
        self.transcript.messages.push(msg);
        self.transcript.message_retained_bytes.push(bytes);
        self.transcript.retained_history_bytes =
            self.transcript.retained_history_bytes.saturating_add(bytes);
        self.rebuild_render_cache_accounting();
        self.invalidate_tail_transition(
            previous_tail,
            self.transcript.messages.len().checked_sub(1),
        );
        self.request_chat_repaint();
    }

    pub(crate) fn insert_message_tracked(&mut self, idx: usize, msg: ChatMessage) {
        self.ensure_history_retention_accounting();
        let insert_idx = idx.min(self.transcript.messages.len());
        let appended_at_tail = insert_idx == self.transcript.messages.len();
        if !appended_at_tail {
            self.shift_active_turn_assistant_for_insert(insert_idx);
            self.shift_turn_notice_refs_for_insert(insert_idx);
        }
        let bytes = Self::measure_message_bytes(&msg);
        self.transcript.messages.insert(insert_idx, msg);
        self.transcript.message_retained_bytes.insert(insert_idx, bytes);
        self.transcript.retained_history_bytes =
            self.transcript.retained_history_bytes.saturating_add(bytes);
        self.rebuild_render_cache_accounting();
        if appended_at_tail {
            let new_tail = self.transcript.messages.len().checked_sub(1);
            self.invalidate_tail_transition(
                new_tail.and_then(|tail| tail.checked_sub(1)),
                new_tail,
            );
        } else {
            self.sync_after_message_topology_change(insert_idx);
        }
        self.request_chat_repaint();
    }

    pub(crate) fn remove_message_tracked(&mut self, idx: usize) -> Option<ChatMessage> {
        self.ensure_history_retention_accounting();
        let old_len = self.transcript.messages.len();
        if idx >= old_len {
            return None;
        }
        let removed_tail = idx + 1 == old_len;
        self.shift_active_turn_assistant_for_remove(idx);
        self.shift_turn_notice_refs_for_remove(idx);
        let removed = self.transcript.messages.remove(idx);
        let removed_bytes = self.transcript.message_retained_bytes.remove(idx);
        self.transcript.retained_history_bytes =
            self.transcript.retained_history_bytes.saturating_sub(removed_bytes);
        self.rebuild_render_cache_accounting();
        self.rebuild_tool_indices();
        if removed_tail {
            self.invalidate_tail_transition(None, self.transcript.messages.len().checked_sub(1));
        } else if !self.transcript.messages.is_empty() {
            self.invalidate_layout(InvalidationLevel::MessagesFrom(idx));
        }
        self.request_chat_repaint();
        Some(removed)
    }

    pub(crate) fn clear_messages_tracked(&mut self) {
        self.transcript.messages.clear();
        self.transcript.message_retained_bytes.clear();
        self.transcript.retained_history_bytes = 0;
        self.clear_active_turn_assistant();
        self.clear_turn_notice_refs();
        self.rebuild_render_cache_accounting();
        self.rebuild_tool_indices();
        self.request_chat_repaint();
    }

    pub(crate) fn recompute_message_retained_bytes(&mut self, idx: usize) {
        self.ensure_history_retention_accounting();
        let Some(msg) = self.transcript.messages.get(idx) else {
            return;
        };
        let new_bytes = Self::measure_message_bytes(msg);
        let Some(old_bytes) = self.transcript.message_retained_bytes.get_mut(idx) else {
            self.rebuild_history_retention_accounting();
            return;
        };
        self.transcript.retained_history_bytes = self
            .transcript
            .retained_history_bytes
            .saturating_sub(*old_bytes)
            .saturating_add(new_bytes);
        *old_bytes = new_bytes;
    }

    pub(crate) fn rebuild_tool_indices(&mut self) {
        self.transcript.tool_call_index.clear();
        self.turn.active_task_ids.clear();

        let mut pending_interaction_ids = Vec::new();
        for (msg_idx, msg) in self.transcript.messages.iter_mut().enumerate() {
            for (block_idx, block) in msg.blocks.iter_mut().enumerate() {
                match block {
                    MessageBlock::ToolCall(tc) => {
                        let tc = tc.as_mut();
                        self.transcript.tool_call_index.insert(tc.id.clone(), (msg_idx, block_idx));
                        if let Some(permission) = tc.pending_permission.as_mut() {
                            permission.focused = false;
                            pending_interaction_ids.push(tc.id.clone());
                        }
                        if let Some(question) = tc.pending_question.as_mut() {
                            question.focused = false;
                            pending_interaction_ids.push(tc.id.clone());
                        }
                    }
                    MessageBlock::UserDialog(dialog) => {
                        self.transcript
                            .tool_call_index
                            .insert(dialog.request_id.clone(), (msg_idx, block_idx));
                        if !dialog.answered {
                            dialog.focused = false;
                            pending_interaction_ids.push(dialog.request_id.clone());
                        }
                    }
                    _ => {}
                }
            }
        }
        self.rebuild_active_task_tracking_from_tool_scopes();
        self.sync_pending_interaction_focus(pending_interaction_ids);
    }

    fn rebuild_active_task_tracking_from_tool_scopes(&mut self) {
        self.tool_call_scopes.retain(|id, _| self.transcript.tool_call_index.contains_key(id));
        for msg in &self.transcript.messages {
            for block in &msg.blocks {
                let MessageBlock::ToolCall(tc) = block else {
                    continue;
                };
                if !matches!(
                    tc.status,
                    model::ToolCallStatus::Pending | model::ToolCallStatus::InProgress
                ) {
                    continue;
                }
                match self.tool_call_scopes.get(&tc.id) {
                    Some(super::ToolCallScope::SubagentRoot) => {
                        self.turn.active_task_ids.insert(tc.id.clone());
                    }
                    Some(
                        super::ToolCallScope::SubagentChild { .. }
                        | super::ToolCallScope::MainAgent,
                    )
                    | None => {}
                }
            }
        }
    }

    fn sync_pending_interaction_focus(&mut self, pending_interaction_ids: Vec<String>) {
        let interaction_set: HashSet<&str> =
            pending_interaction_ids.iter().map(String::as_str).collect();
        self.turn.pending_interaction_ids.retain(|id| interaction_set.contains(id.as_str()));
        for id in pending_interaction_ids {
            if !self.turn.pending_interaction_ids.iter().any(|existing| existing == &id) {
                self.turn.pending_interaction_ids.push(id);
            }
        }

        if let Some(first_id) = self.turn.pending_interaction_ids.first().cloned() {
            self.claim_focus_target(super::super::focus::FocusTarget::Permission);
            if let Some((msg_idx, block_idx)) = self.lookup_tool_call(&first_id)
                && let Some(block) = self
                    .transcript
                    .messages
                    .get_mut(msg_idx)
                    .and_then(|m| m.blocks.get_mut(block_idx))
            {
                match block {
                    MessageBlock::ToolCall(tc) => {
                        if let Some(permission) = tc.pending_permission.as_mut() {
                            permission.focused = true;
                        }
                        if let Some(question) = tc.pending_question.as_mut() {
                            question.focused = true;
                        }
                    }
                    MessageBlock::UserDialog(dialog) if !dialog.answered => {
                        dialog.focused = true;
                    }
                    _ => {}
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
        self.ensure_history_retention_accounting();
        let marker_idx =
            self.transcript.messages.iter().position(Self::is_history_hidden_marker_message);
        if self.history_retention_stats.total_dropped_messages == 0 {
            if let Some(idx) = marker_idx {
                self.remove_message_tracked(idx);
            }
            return;
        }

        let marker_text = Self::history_hidden_marker_text(
            self.history_retention_stats.total_dropped_messages,
            self.history_retention_stats.total_dropped_bytes,
        );

        if let Some(idx) = marker_idx {
            if let Some(MessageBlock::Text(block)) =
                self.transcript.messages.get_mut(idx).and_then(|m| m.blocks.get_mut(0))
                && block.text != marker_text
            {
                block.text.clone_from(&marker_text);
                block.markdown = IncrementalMarkdown::from_complete(&marker_text);
                block.cache.invalidate();
                self.sync_render_cache_slot(idx, 0);
                self.recompute_message_retained_bytes(idx);
                self.invalidate_layout(InvalidationLevel::MessagesFrom(idx));
            }
            return;
        }

        let insert_idx = usize::from(
            self.transcript
                .messages
                .first()
                .is_some_and(|msg| matches!(msg.role, MessageRole::Welcome)),
        );
        self.insert_message_tracked(
            insert_idx,
            ChatMessage::new(
                MessageRole::System(None),
                vec![MessageBlock::Text(TextBlock::from_complete(&marker_text))],
                None,
            ),
        );
    }

    pub fn enforce_history_retention(&mut self) -> HistoryRetentionStats {
        self.ensure_history_retention_accounting();
        let mut stats = HistoryRetentionStats::default();
        let max_bytes = self.history_retention.max_bytes.max(1);
        let active_turn_owner = self.active_turn_assistant_idx();
        stats.total_before_bytes = self.transcript.retained_history_bytes;
        stats.total_after_bytes = stats.total_before_bytes;

        if stats.total_before_bytes > max_bytes {
            let mut candidates = Vec::new();
            for (msg_idx, msg) in self.transcript.messages.iter().enumerate() {
                if Self::is_history_hidden_marker_message(msg)
                    || Self::is_history_protected_message(msg)
                    || active_turn_owner == Some(msg_idx)
                {
                    continue;
                }
                let bytes =
                    self.transcript.message_retained_bytes.get(msg_idx).copied().unwrap_or(0);
                if bytes == 0 {
                    continue;
                }
                candidates.push(HistoryDropCandidate { msg_idx, bytes });
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
                self.apply_history_retention_drop(&drop_candidates, active_turn_owner);
                self.rebuild_tool_indices();
                self.invalidate_layout(InvalidationLevel::MessagesFrom(0));
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

        stats.total_after_bytes = self.transcript.retained_history_bytes;
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

    fn apply_history_retention_drop(
        &mut self,
        drop_candidates: &[HistoryDropCandidate],
        active_turn_owner: Option<usize>,
    ) {
        let drop_set: HashSet<usize> =
            drop_candidates.iter().map(|candidate| candidate.msg_idx).collect();

        let mut retained =
            Vec::with_capacity(self.transcript.messages.len().saturating_sub(drop_set.len()));
        let mut retained_bytes = Vec::with_capacity(retained.capacity());
        let old_messages = std::mem::take(&mut self.transcript.messages);
        let old_bytes = std::mem::take(&mut self.transcript.message_retained_bytes);
        let mut old_to_new = vec![None; old_messages.len()];
        let mut remapped_active_turn_owner = None;
        self.transcript.retained_history_bytes = 0;
        for (msg_idx, (msg, bytes)) in old_messages.into_iter().zip(old_bytes).enumerate() {
            if !drop_set.contains(&msg_idx) {
                if active_turn_owner == Some(msg_idx) {
                    remapped_active_turn_owner = Some(retained.len());
                }
                old_to_new[msg_idx] = Some(retained.len());
                self.transcript.retained_history_bytes =
                    self.transcript.retained_history_bytes.saturating_add(bytes);
                retained.push(msg);
                retained_bytes.push(bytes);
            }
        }
        self.transcript.messages = retained;
        self.transcript.message_retained_bytes = retained_bytes;
        self.turn.assistant_message_idx = remapped_active_turn_owner;
        self.remap_turn_notice_refs_after_message_drop(&old_to_new);
    }
}
