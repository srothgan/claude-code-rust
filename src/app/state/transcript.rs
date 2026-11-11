// SPDX-License-Identifier: Apache-2.0

use std::collections::HashMap;

use super::messages::{ChatMessage, MessageBlock, MessageRole};

#[derive(Default)]
pub struct Transcript {
    pub messages: Vec<ChatMessage>,
    /// Changes to existing message positions invalidate an in-progress traversal.
    topology_revision: u64,
    /// Cached approximate retained bytes for each message, parallel to `messages`.
    pub(super) message_retained_bytes: Vec<usize>,
    /// Rolling total of `message_retained_bytes`.
    pub(super) retained_history_bytes: usize,
    /// O(1) lookup: `tool_call_id` -> `(message_index, block_index)`.
    pub(super) tool_call_index: HashMap<String, (usize, usize)>,
}

impl Transcript {
    pub(crate) const fn topology_revision(&self) -> u64 {
        self.topology_revision
    }

    pub(super) fn mark_topology_changed(&mut self) {
        self.topology_revision = self.topology_revision.wrapping_add(1);
    }

    #[must_use]
    pub fn new(messages: Vec<ChatMessage>) -> Self {
        Self { messages, ..Self::default() }
    }

    #[must_use]
    pub(crate) fn latest_user_text(&self) -> Option<&str> {
        self.messages.iter().rev().find_map(|message| {
            if !matches!(message.role, MessageRole::User) {
                return None;
            }
            message.blocks.iter().rev().find_map(|block| {
                if let MessageBlock::Text(text) = block { Some(text.text.as_str()) } else { None }
            })
        })
    }
}
