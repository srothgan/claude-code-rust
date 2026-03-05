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

use super::super::{
    App, AppStatus, BlockCache, ChatMessage, IncrementalMarkdown, MessageBlock, MessageRole,
    default_cache_split_policy, find_text_split_index,
};
use crate::agent::model;

pub(super) fn handle_agent_message_chunk(app: &mut App, chunk: model::ContentChunk) {
    let model::ContentBlock::Text(text) = chunk.content else {
        return;
    };

    app.status = AppStatus::Running;
    if text.text.is_empty() {
        return;
    }
    if let Some(last) = app.messages.last_mut()
        && matches!(last.role, MessageRole::Assistant)
    {
        append_agent_stream_text(&mut last.blocks, &text.text);
        return;
    }

    let mut blocks = Vec::new();
    append_agent_stream_text(&mut blocks, &text.text);
    app.messages.push(ChatMessage { role: MessageRole::Assistant, blocks, usage: None });
}

pub(super) fn append_agent_stream_text(blocks: &mut Vec<MessageBlock>, chunk: &str) {
    if chunk.is_empty() {
        return;
    }
    if let Some(MessageBlock::Text(text, cache, incr)) = blocks.last_mut() {
        text.push_str(chunk);
        incr.append(chunk);
        cache.invalidate();
    } else {
        blocks.push(new_text_block(chunk.to_owned()));
    }

    let split_count = split_tail_text_block(blocks);
    if split_count > 0 {
        crate::perf::mark_with("text_block_split_count", "count", split_count);
    }

    if let Some(MessageBlock::Text(text, _, _)) = blocks.last() {
        crate::perf::mark_with("text_block_active_tail_bytes", "bytes", text.len());
    }
    let text_block_count = blocks.iter().filter(|b| matches!(b, MessageBlock::Text(..))).count();
    crate::perf::mark_with("text_block_frozen_count", "count", text_block_count.saturating_sub(1));
}

fn new_text_block(text: String) -> MessageBlock {
    let incr = IncrementalMarkdown::from_complete(&text);
    MessageBlock::Text(text, BlockCache::default(), incr)
}

fn split_tail_text_block(blocks: &mut Vec<MessageBlock>) -> usize {
    let mut split_count = 0usize;
    loop {
        let Some(tail_idx) = blocks.len().checked_sub(1) else {
            break;
        };
        let Some(split_at) = blocks.get(tail_idx).and_then(|block| {
            if let MessageBlock::Text(text, _, _) = block {
                find_text_block_split_index(text)
            } else {
                None
            }
        }) else {
            break;
        };

        let (completed, remainder) = match blocks.get(tail_idx) {
            Some(MessageBlock::Text(text, _, _)) => {
                (text[..split_at].to_owned(), text[split_at..].to_owned())
            }
            _ => break,
        };

        if completed.is_empty() || remainder.is_empty() {
            break;
        }

        blocks[tail_idx] = new_text_block(remainder);
        blocks.insert(tail_idx, new_text_block(completed));
        split_count += 1;
    }
    split_count
}

pub(super) fn find_text_block_split_index(text: &str) -> Option<usize> {
    find_text_split_index(text, *default_cache_split_policy())
}
