// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use std::collections::{BTreeSet, HashMap, HashSet};

use super::messages::ChatMessage;
use super::render_budget;
use super::tool_tracking::TerminalToolCallRef;

#[derive(Default)]
pub struct Transcript {
    pub messages: Vec<ChatMessage>,
    /// Cached approximate retained bytes for each message, parallel to `messages`.
    pub(super) message_retained_bytes: Vec<usize>,
    /// Rolling total of `message_retained_bytes`.
    pub(super) retained_history_bytes: usize,
    /// O(1) lookup: `tool_call_id` -> `(message_index, block_index)`.
    pub(super) tool_call_index: HashMap<String, (usize, usize)>,
    /// Indexed terminal tool calls for per-frame terminal snapshot updates.
    /// Avoids O(n*m) scan of all messages/blocks every frame.
    pub(super) terminal_tool_calls: Vec<TerminalToolCallRef>,
    /// Membership index for `terminal_tool_calls`, used to avoid linear duplicate checks.
    pub(super) terminal_tool_call_membership: HashSet<TerminalToolCallRef>,
    /// Cached render-cache slot metadata parallel to `messages[*].blocks[*]`.
    pub(super) render_cache_slots: Vec<Vec<render_budget::RenderCacheSlotState>>,
    /// Rolling total of cached render bytes across blocks and message-level caches.
    pub(super) render_cache_total_bytes: usize,
    /// Rolling total of cached render bytes currently excluded from the budget.
    pub(super) render_cache_protected_bytes: usize,
    /// Evictable cached blocks ordered by LRU and size tie-breaker.
    pub(super) render_cache_evictable: BTreeSet<render_budget::RenderCacheEvictionKey>,
    /// Last message index currently protected as the streaming tail, if any.
    pub(super) render_cache_tail_msg_idx: Option<usize>,
}

impl Transcript {
    #[must_use]
    pub fn new(messages: Vec<ChatMessage>) -> Self {
        Self { messages, ..Self::default() }
    }
}
