// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use std::collections::HashSet;

use super::super::{App, MessageBlock, MessageRole};
use crate::agent::model;

pub(super) fn handle_transcript_retraction(
    app: &mut App,
    retraction: &model::TranscriptRetraction,
) {
    let message_uuids: HashSet<&str> = retraction
        .message_uuids
        .iter()
        .map(String::as_str)
        .filter(|uuid| !uuid.trim().is_empty())
        .collect();
    if message_uuids.is_empty() {
        return;
    }

    let mut removed_blocks = 0usize;
    let mut removed_messages = 0usize;
    let mut changed = false;
    let mut rebuild_indices = false;

    for msg_idx in (0..app.transcript.messages.len()).rev() {
        let (removed, empty_assistant) = match app.transcript.messages.get_mut(msg_idx) {
            Some(message) => {
                let before_len = message.blocks.len();
                message.blocks.retain(|block| {
                    let retract = block_matches_retraction(block, &message_uuids);
                    if retract && matches!(block, MessageBlock::ToolCall(_)) {
                        rebuild_indices = true;
                    }
                    !retract
                });
                (
                    before_len.saturating_sub(message.blocks.len()),
                    message.blocks.is_empty() && matches!(message.role, MessageRole::Assistant),
                )
            }
            None => continue,
        };
        if removed == 0 {
            continue;
        }

        changed = true;
        removed_blocks = removed_blocks.saturating_add(removed);
        if empty_assistant {
            if app.remove_message_tracked(msg_idx).is_some() {
                removed_messages = removed_messages.saturating_add(1);
            }
        } else {
            app.sync_after_message_blocks_changed(msg_idx);
        }
    }

    if rebuild_indices {
        app.rebuild_tool_indices_and_terminal_refs();
    }

    tracing::info!(
        target: crate::logging::targets::APP_SESSION,
        event_name = "transcript_retraction_applied",
        message = "transcript retraction applied",
        outcome = if changed { "success" } else { "noop" },
        reason = ?retraction.reason,
        requested_uuid_count = message_uuids.len(),
        removed_block_count = removed_blocks,
        removed_message_count = removed_messages,
        request_id = retraction.request_id.as_deref().unwrap_or(""),
        original_model = retraction.original_model.as_deref().unwrap_or(""),
        fallback_model = retraction.fallback_model.as_deref().unwrap_or(""),
    );
}

fn block_matches_retraction(block: &MessageBlock, message_uuids: &HashSet<&str>) -> bool {
    match block {
        MessageBlock::Text(block) => {
            block.source_message_uuids.iter().any(|uuid| message_uuids.contains(uuid.as_str()))
        }
        MessageBlock::ToolCall(tool_call) => {
            tool_call.source_message_uuids.iter().any(|uuid| message_uuids.contains(uuid.as_str()))
        }
        MessageBlock::Notice(_)
        | MessageBlock::Welcome(_)
        | MessageBlock::ImageAttachment(_)
        | MessageBlock::UserDialog(_) => false,
    }
}
