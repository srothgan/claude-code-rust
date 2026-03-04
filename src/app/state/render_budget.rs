// Claude Code Rust - A native Rust terminal interface for Claude Code
// Copyright (C) 2025  Simon Peter Rothgang
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as
// published by the Free Software Foundation, either version 3 of the
// License, or (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

use super::messages::MessageBlock;
use super::types::{AppStatus, CacheBudgetEnforceStats};
use crate::agent::model;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CacheSlotCandidate {
    pub(super) msg_idx: usize,
    pub(super) block_idx: usize,
    pub(super) bytes: usize,
    pub(super) last_access_tick: u64,
}

impl super::App {
    pub fn enforce_render_cache_budget(&mut self) -> CacheBudgetEnforceStats {
        let mut stats = CacheBudgetEnforceStats::default();
        let is_streaming = matches!(self.status, AppStatus::Thinking | AppStatus::Running);
        let msg_count = self.messages.len();
        let mut evictable = Vec::new();

        for (msg_idx, msg) in self.messages.iter().enumerate() {
            let protect_message_tail = is_streaming && (msg_idx + 1 == msg_count);
            for (block_idx, block) in msg.blocks.iter().enumerate() {
                let (cache, protect_block) = match block {
                    MessageBlock::Text(_, cache, _) => (cache, false),
                    MessageBlock::Welcome(welcome) => (&welcome.cache, false),
                    MessageBlock::ToolCall(tc) => (
                        &tc.cache,
                        matches!(
                            tc.status,
                            model::ToolCallStatus::Pending | model::ToolCallStatus::InProgress
                        ),
                    ),
                };

                let bytes = cache.cached_bytes();
                if bytes == 0 {
                    continue;
                }
                stats.total_before_bytes = stats.total_before_bytes.saturating_add(bytes);

                if !(protect_message_tail || protect_block) {
                    evictable.push(CacheSlotCandidate {
                        msg_idx,
                        block_idx,
                        bytes,
                        last_access_tick: cache.last_access_tick(),
                    });
                }
            }
        }

        if stats.total_before_bytes <= self.render_cache_budget.max_bytes {
            self.render_cache_budget.last_total_bytes = stats.total_before_bytes;
            self.render_cache_budget.last_evicted_bytes = 0;
            stats.total_after_bytes = stats.total_before_bytes;
            return stats;
        }

        evictable.sort_by_key(|slot| (slot.last_access_tick, std::cmp::Reverse(slot.bytes)));
        stats.total_after_bytes = stats.total_before_bytes;

        for slot in evictable {
            if stats.total_after_bytes <= self.render_cache_budget.max_bytes {
                break;
            }
            let removed = self.evict_cache_slot(slot.msg_idx, slot.block_idx);
            if removed == 0 {
                continue;
            }
            stats.total_after_bytes = stats.total_after_bytes.saturating_sub(removed);
            stats.evicted_bytes = stats.evicted_bytes.saturating_add(removed);
            stats.evicted_blocks = stats.evicted_blocks.saturating_add(1);
        }

        self.render_cache_budget.last_total_bytes = stats.total_after_bytes;
        self.render_cache_budget.last_evicted_bytes = stats.evicted_bytes;
        self.render_cache_budget.total_evictions =
            self.render_cache_budget.total_evictions.saturating_add(stats.evicted_blocks);

        stats
    }

    fn evict_cache_slot(&mut self, msg_idx: usize, block_idx: usize) -> usize {
        let Some(msg) = self.messages.get_mut(msg_idx) else {
            return 0;
        };
        let Some(block) = msg.blocks.get_mut(block_idx) else {
            return 0;
        };
        match block {
            MessageBlock::Text(_, cache, _) => cache.evict_cached_render(),
            MessageBlock::Welcome(welcome) => welcome.cache.evict_cached_render(),
            MessageBlock::ToolCall(tc) => tc.cache.evict_cached_render(),
        }
    }
}
