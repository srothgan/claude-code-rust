// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TerminalToolCallRef {
    pub terminal_id: String,
    pub msg_idx: usize,
    pub block_idx: usize,
}

impl TerminalToolCallRef {
    #[must_use]
    pub fn new(terminal_id: String, msg_idx: usize, block_idx: usize) -> Self {
        Self { terminal_id, msg_idx, block_idx }
    }
}

impl App {
    /// Track a Task/Agent tool call as active (in-progress subagent).
    pub fn insert_active_task(&mut self, id: String) {
        self.active_task_ids.insert(id);
    }

    /// Remove a Task/Agent tool call from the active set (completed/failed).
    pub fn remove_active_task(&mut self, id: &str) {
        self.active_task_ids.remove(id);
    }

    pub fn register_tool_call_scope(&mut self, id: String, scope: ToolCallScope) {
        self.tool_call_scopes.insert(id, scope);
    }

    #[must_use]
    pub fn tool_call_scope(&self, id: &str) -> Option<ToolCallScope> {
        self.tool_call_scopes.get(id).cloned()
    }

    #[must_use]
    pub(crate) fn tracked_terminal_id_for_tool(tc: &ToolCallInfo) -> Option<String> {
        (tc.is_execute_tool()
            && matches!(
                tc.status,
                model::ToolCallStatus::Pending | model::ToolCallStatus::InProgress
            ))
        .then(|| tc.terminal_id.clone())
        .flatten()
    }

    pub fn clear_tool_scope_tracking(&mut self) {
        self.tool_call_scopes.clear();
        self.active_task_ids.clear();
    }

    /// Look up the (`message_index`, `block_index`) for a tool call ID.
    #[must_use]
    pub fn lookup_tool_call(&self, id: &str) -> Option<(usize, usize)> {
        self.tool_call_index.get(id).copied()
    }

    /// Register a tool call's position in the message/block arrays.
    pub fn index_tool_call(&mut self, id: String, msg_idx: usize, block_idx: usize) {
        self.tool_call_index.insert(id, (msg_idx, block_idx));
    }

    pub(crate) fn sync_terminal_tool_call(
        &mut self,
        terminal_id: String,
        msg_idx: usize,
        block_idx: usize,
    ) {
        let desired = TerminalToolCallRef::new(terminal_id, msg_idx, block_idx);
        if self.terminal_tool_call_membership.contains(&desired) {
            return;
        }
        self.untrack_terminal_tool_call(msg_idx, block_idx);
        self.terminal_tool_call_membership.insert(desired.clone());
        self.terminal_tool_calls.push(desired);
    }

    pub(crate) fn untrack_terminal_tool_call(&mut self, msg_idx: usize, block_idx: usize) {
        let removed: Vec<_> = self
            .terminal_tool_calls
            .iter()
            .filter(|entry| entry.msg_idx == msg_idx && entry.block_idx == block_idx)
            .cloned()
            .collect();
        if removed.is_empty() {
            return;
        }
        self.terminal_tool_calls
            .retain(|entry| entry.msg_idx != msg_idx || entry.block_idx != block_idx);
        for entry in removed {
            self.terminal_tool_call_membership.remove(&entry);
        }
    }

    pub(crate) fn clear_terminal_tool_call_tracking(&mut self) {
        self.terminal_tool_calls.clear();
        self.terminal_tool_call_membership.clear();
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
        let mut changed = 0usize;
        let mut cleared_interaction = false;
        let mut changed_message_indices = Vec::new();
        let mut changed_slots = Vec::new();
        let mut detached_terminal = false;

        for (msg_idx, msg) in self.messages.iter_mut().enumerate() {
            for (block_idx, block) in msg.blocks.iter_mut().enumerate() {
                if let MessageBlock::ToolCall(tc) = block {
                    let tc = tc.as_mut();
                    if matches!(
                        tc.status,
                        model::ToolCallStatus::InProgress | model::ToolCallStatus::Pending
                    ) {
                        tc.status = new_status;
                        tc.invalidate_render_cache();
                        changed_slots.push((msg_idx, block_idx));
                        if tc.pending_permission.take().is_some() {
                            cleared_interaction = true;
                        }
                        if tc.pending_question.take().is_some() {
                            cleared_interaction = true;
                        }
                        if tc.is_execute_tool() && tc.terminal_id.take().is_some() {
                            detached_terminal = true;
                        }
                        if changed_message_indices.last().copied() != Some(msg_idx) {
                            changed_message_indices.push(msg_idx);
                        }
                        changed += 1;
                    }
                }
            }
        }

        if detached_terminal {
            self.rebuild_tool_indices_and_terminal_refs();
        }

        for (msg_idx, block_idx) in changed_slots {
            self.sync_render_cache_slot(msg_idx, block_idx);
        }

        for msg_idx in changed_message_indices.iter().copied() {
            self.recompute_message_retained_bytes(msg_idx);
        }

        if changed > 0 || cleared_interaction {
            self.invalidate_message_set(changed_message_indices.iter().copied());
            self.pending_interaction_ids.clear();
            self.release_focus_target(FocusTarget::Permission);
        }

        changed
    }

    /// Clear any inline permission/question UI still attached to tool calls.
    /// Returns the number of tool call blocks that changed.
    pub fn clear_inline_tool_interactions(&mut self) -> usize {
        let mut changed = 0usize;
        let mut changed_message_indices = Vec::new();
        let mut changed_slots = Vec::new();

        for (msg_idx, msg) in self.messages.iter_mut().enumerate() {
            for (block_idx, block) in msg.blocks.iter_mut().enumerate() {
                let MessageBlock::ToolCall(tc) = block else {
                    continue;
                };
                let tc = tc.as_mut();
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
            self.pending_interaction_ids.clear();
            self.release_focus_target(FocusTarget::Permission);
        }

        changed
    }

    /// Clear runtime-only turn tracking while preserving the message history itself.
    pub fn finalize_turn_runtime_artifacts(&mut self, new_status: model::ToolCallStatus) {
        let _ = self.finalize_in_progress_tool_calls(new_status);
        let _ = self.clear_inline_tool_interactions();
        self.clear_tool_scope_tracking();
    }
}
