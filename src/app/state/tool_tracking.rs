// SPDX-License-Identifier: Apache-2.0

use super::prelude::*;

impl App {
    /// Returns whether the tool block changed; the caller syncs its cache slot,
    /// retained bytes, and layout once together with its own changes.
    pub(crate) fn sync_tool_call_history(&mut self, mi: usize, bi: usize) -> bool {
        let Some(MessageBlock::ToolCall(tool)) =
            self.transcript.messages.get_mut(mi).and_then(|message| message.blocks.get_mut(bi))
        else {
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
    pub fn register_tool_call_scope(&mut self, id: String, scope: ToolCallScope) {
        self.tool_call_scopes.insert(id, scope);
    }

    #[must_use]
    pub fn tool_call_scope(&self, id: &str) -> Option<ToolCallScope> {
        self.tool_call_scopes.get(id).cloned()
    }

    pub fn clear_tool_scope_tracking(&mut self) {
        self.tool_call_scopes.clear();
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
        for _ in 0..=self.tool_call_scopes.len() {
            if self.lookup_tool_call(id).is_some_and(|(mi, bi)| {
                matches!(&self.transcript.messages[mi].blocks[bi], MessageBlock::ToolCall(tool)
                    if tool.status == model::ToolCallStatus::Detached)
            }) {
                return true;
            }
            let Some(ToolCallScope::SubagentChild { parent_tool_use_id }) =
                self.tool_call_scopes.get(id)
            else {
                return false;
            };
            id = parent_tool_use_id;
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
        // Tool scope belongs to the session: detached agents and their descendants
        // can still emit events after the foreground turn ends.
        if include_detached {
            self.clear_tool_scope_tracking();
        }
    }
}
