// SPDX-License-Identifier: Apache-2.0

use super::ToolCallInfo;
use super::prelude::*;

impl App {
    pub(crate) fn tool_call_at(&self, mi: usize, bi: usize) -> Option<&ToolCallInfo> {
        match self.transcript.messages.get(mi)?.blocks.get(bi)? {
            MessageBlock::ToolCall(tool) => Some(tool),
            _ => None,
        }
    }

    pub(crate) fn tool_call_at_mut(&mut self, mi: usize, bi: usize) -> Option<&mut ToolCallInfo> {
        match self.transcript.messages.get_mut(mi)?.blocks.get_mut(bi)? {
            MessageBlock::ToolCall(tool) => Some(tool),
            _ => None,
        }
    }

    pub(crate) fn tool_call(&self, id: &str) -> Option<&ToolCallInfo> {
        let (mi, bi) = self.lookup_tool_call(id)?;
        self.tool_call_at(mi, bi)
    }

    /// Return `Some` when the mutation changed the tool, carrying any extracted value.
    pub(crate) fn mutate_tool_call<T>(
        &mut self,
        id: &str,
        mutate: impl FnOnce(&mut ToolCallInfo) -> Option<T>,
    ) -> Option<T> {
        let (mi, bi) = self.lookup_tool_call(id)?;
        let tool = self.tool_call_at_mut(mi, bi)?;
        let result = mutate(tool)?;
        tool.invalidate_render_cache();
        self.finish_block_change(mi, bi);
        Some(result)
    }

    /// Complete a content change after the block's own render cache is invalidated.
    pub(crate) fn finish_block_change(&mut self, mi: usize, bi: usize) {
        self.sync_render_cache_slot(mi, bi);
        self.recompute_message_retained_bytes(mi);
        self.invalidate_layout(InvalidationLevel::MessageChanged(mi));
    }

    /// Returns whether the tool block changed; the caller syncs its cache slot,
    /// retained bytes, and layout once together with its own changes.
    pub(crate) fn sync_tool_call_history(&mut self, mi: usize, bi: usize) -> bool {
        let Some(tool) = self.tool_call_at_mut(mi, bi) else {
            return false;
        };
        let was_frozen = tool.has_frozen_launch();
        let result = tool.sync_history();
        let froze = !was_frozen && tool.has_frozen_launch();
        let Some((id, tool)) = result else {
            return froze;
        };
        let block = MessageBlock::ToolResult { id, tool };
        if let Some(owner) = self.active_turn_assistant_idx() {
            self.transcript.messages[owner].blocks.push(block);
            self.sync_after_message_blocks_changed(owner);
        } else {
            self.push_message_tracked(ChatMessage::new(MessageRole::Assistant, vec![block], None));
        }
        true
    }

    pub(crate) fn indexed_tool_calls(
        &self,
    ) -> impl Iterator<Item = ((usize, usize), &ToolCallInfo)> {
        self.transcript
            .tool_call_index
            .values()
            .filter_map(|&(mi, bi)| self.tool_call_at(mi, bi).map(|tool| ((mi, bi), tool)))
    }

    #[must_use]
    pub fn tool_call_scope(&self, id: &str) -> Option<ToolCallScope> {
        self.tool_call(id).map(ToolCallInfo::scope)
    }

    /// Look up the (`message_index`, `block_index`) for a tool call ID.
    #[must_use]
    pub fn lookup_tool_call(&self, id: &str) -> Option<(usize, usize)> {
        self.transcript.tool_call_index.get(id).copied()
    }

    /// Register a tool call's position in the message/block arrays.
    pub fn index_tool_call(&mut self, id: String, msg_idx: usize, block_idx: usize) {
        self.transcript.tool_call_index.insert(id, (msg_idx, block_idx));
    }

    pub fn clear_tool_call_index(&mut self) {
        self.transcript.tool_call_index.clear();
    }

    #[must_use]
    pub fn has_tool_call(&self, id: &str) -> bool {
        self.lookup_tool_call(id).is_some()
    }

    #[must_use]
    pub fn tool_call_index_len(&self) -> usize {
        self.transcript.tool_call_index.len()
    }

    pub(crate) fn sync_after_message_blocks_changed(&mut self, msg_idx: usize) {
        self.note_render_cache_structure_changed();
        self.sync_render_cache_message(msg_idx);
        self.recompute_message_retained_bytes(msg_idx);
        self.invalidate_layout(InvalidationLevel::MessageChanged(msg_idx));
    }

    /// Force-finish any lingering in-progress tool calls.
    /// Returns the number of tool calls that were transitioned.
    pub fn finalize_in_progress_tool_calls(&mut self, new_status: model::ToolCallStatus) -> usize {
        self.finalize_tool_calls(new_status, false)
    }

    fn tool_belongs_to_background_execution<'a>(&'a self, mut id: &'a str) -> bool {
        for _ in 0..=self.transcript.tool_call_index.len() {
            let Some(tool) = self.tool_call(id) else {
                return false;
            };
            if tool.status == model::ToolCallStatus::Detached {
                return true;
            }
            let Some(parent) = tool.parent_tool_use_id.as_deref() else {
                return false;
            };
            id = parent;
        }
        false
    }

    fn background_tool_ids(&self) -> std::collections::HashSet<String> {
        self.transcript
            .tool_call_index
            .keys()
            .filter(|id| self.tool_belongs_to_background_execution(id))
            .cloned()
            .collect()
    }

    fn finalize_tool_calls(
        &mut self,
        new_status: model::ToolCallStatus,
        include_detached: bool,
    ) -> usize {
        let mut changed = 0usize;
        let mut cleared_interaction = false;
        let mut changed_message_indices = Vec::new();
        let mut changed_slots = Vec::new();
        let background_tools: std::collections::HashSet<String> = if include_detached {
            std::collections::HashSet::new()
        } else {
            self.background_tool_ids()
        };

        for (msg_idx, msg) in self.transcript.messages.iter_mut().enumerate() {
            for (block_idx, block) in msg.blocks.iter_mut().enumerate() {
                if let MessageBlock::ToolCall(tc) = block {
                    let tc = tc.as_mut();
                    if !tc.status.is_terminal() && !background_tools.contains(&tc.id) {
                        tc.status = new_status;
                        tc.invalidate_render_cache();
                        changed_slots.push((msg_idx, block_idx));
                        if tc.pending_permission.take().is_some() {
                            cleared_interaction = true;
                        }
                        if tc.pending_question.take().is_some() {
                            cleared_interaction = true;
                        }
                        if changed_message_indices.last().copied() != Some(msg_idx) {
                            changed_message_indices.push(msg_idx);
                        }
                        changed += 1;
                    }
                }
            }
        }

        for (msg_idx, block_idx) in changed_slots {
            self.sync_tool_call_history(msg_idx, block_idx);
            self.sync_render_cache_slot(msg_idx, block_idx);
        }

        for msg_idx in changed_message_indices.iter().copied() {
            self.recompute_message_retained_bytes(msg_idx);
        }

        if changed > 0 || cleared_interaction {
            self.invalidate_message_set(changed_message_indices.iter().copied());
            self.pending_interaction_ids.retain(|id| background_tools.contains(id));
            crate::app::inline_interactions::normalize_pending_interaction_queue(self);
        }

        changed
    }

    /// Clear any inline permission/question UI still attached to tool calls.
    /// Returns the number of tool call blocks that changed.
    pub fn clear_inline_tool_interactions(&mut self) -> usize {
        self.clear_inline_tool_interactions_excluding(&std::collections::HashSet::new())
    }

    fn clear_inline_tool_interactions_excluding(
        &mut self,
        preserved_tools: &std::collections::HashSet<String>,
    ) -> usize {
        let mut changed = 0usize;
        let mut changed_message_indices = Vec::new();
        let mut changed_slots = Vec::new();

        for (msg_idx, msg) in self.transcript.messages.iter_mut().enumerate() {
            for (block_idx, block) in msg.blocks.iter_mut().enumerate() {
                let MessageBlock::ToolCall(tc) = block else {
                    continue;
                };
                let tc = tc.as_mut();
                if preserved_tools.contains(&tc.id) {
                    continue;
                }
                let mut block_changed = false;
                if tc.pending_permission.take().is_some() {
                    block_changed = true;
                }
                if tc.pending_question.take().is_some() {
                    block_changed = true;
                }
                if !block_changed {
                    continue;
                }
                tc.invalidate_render_cache();
                changed_slots.push((msg_idx, block_idx));
                if changed_message_indices.last().copied() != Some(msg_idx) {
                    changed_message_indices.push(msg_idx);
                }
                changed += 1;
            }
        }

        for (msg_idx, block_idx) in changed_slots {
            self.sync_render_cache_slot(msg_idx, block_idx);
        }

        for msg_idx in changed_message_indices.iter().copied() {
            self.recompute_message_retained_bytes(msg_idx);
        }

        if changed > 0 {
            self.invalidate_message_set(changed_message_indices.iter().copied());
        }

        if changed > 0 || !self.pending_interaction_ids.is_empty() {
            self.pending_interaction_ids.retain(|id| preserved_tools.contains(id));
            crate::app::inline_interactions::normalize_pending_interaction_queue(self);
        }

        changed
    }

    /// Clear runtime-only turn tracking while preserving the message history itself.
    pub fn finalize_turn_runtime_artifacts(&mut self, new_status: model::ToolCallStatus) {
        self.finalize_runtime_artifacts(new_status, false);
    }

    pub(crate) fn finalize_session_runtime_artifacts(&mut self, new_status: model::ToolCallStatus) {
        self.finalize_runtime_artifacts(new_status, true);
    }

    fn finalize_runtime_artifacts(
        &mut self,
        new_status: model::ToolCallStatus,
        include_detached: bool,
    ) {
        let preserved_tools = if include_detached {
            std::collections::HashSet::new()
        } else {
            self.background_tool_ids()
        };
        let _ = self.finalize_tool_calls(new_status, include_detached);
        let _ = self.clear_inline_tool_interactions_excluding(&preserved_tools);
    }
}
