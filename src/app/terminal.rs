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

use super::{App, MessageBlock, TerminalSnapshotMode, ToolCallInfo};

enum TerminalUpdatePayload {
    Append { bytes: Vec<u8>, current_len: usize },
    Replace { bytes: Vec<u8>, current_len: usize },
}

fn apply_terminal_payload(tc: &mut ToolCallInfo, payload: TerminalUpdatePayload) -> bool {
    match payload {
        TerminalUpdatePayload::Append { bytes, current_len } => {
            if bytes.is_empty() {
                return false;
            }
            let delta = String::from_utf8_lossy(&bytes);
            crate::perf::mark_with("terminal_delta_bytes", "bytes", bytes.len());
            let output = tc.terminal_output.get_or_insert_with(String::new);
            output.push_str(&delta);
            tc.terminal_bytes_seen = current_len;
            tc.terminal_output_len = current_len;
            tc.terminal_snapshot_mode = TerminalSnapshotMode::AppendOnly;
            true
        }
        TerminalUpdatePayload::Replace { bytes, current_len } => {
            crate::perf::mark("terminal_full_snapshot_fallbacks");
            let snapshot = String::from_utf8_lossy(&bytes).to_string();
            let changed = tc.terminal_output.as_deref() != Some(snapshot.as_str());
            if changed {
                tc.terminal_output = Some(snapshot);
            }
            tc.terminal_bytes_seen = current_len;
            tc.terminal_output_len = current_len;
            tc.terminal_snapshot_mode = TerminalSnapshotMode::AppendOnly;
            changed
        }
    }
}

/// Snapshot terminal output buffers into `ToolCallInfo` for rendering.
/// Called each frame so in-progress Execute tool calls show live output.
///
/// Uses append-only deltas when possible, with full-snapshot fallback when
/// invariants are broken (truncate/reset/replace mode).
pub(super) fn update_terminal_outputs(app: &mut App) -> bool {
    let _t = app.perf.as_ref().map(|p| p.start("terminal::update"));
    let terminals = app.terminals.borrow();
    if terminals.is_empty() {
        return false;
    }

    let mut changed = false;
    let mut dirty_from: Option<usize> = None;

    // Use the indexed terminal tool calls instead of scanning all messages/blocks.
    for &(ref tid, mi, bi) in &app.terminal_tool_calls {
        let Some(terminal) = terminals.get(tid.as_str()) else {
            continue;
        };
        let Some(MessageBlock::ToolCall(tc)) =
            app.messages.get_mut(mi).and_then(|m| m.blocks.get_mut(bi))
        else {
            continue;
        };
        let tc = tc.as_mut();

        // Copy only the required bytes under lock, then decode outside the
        // critical section to avoid blocking output writers.
        let payload = {
            let Ok(buf) = terminal.output_buffer.lock() else {
                continue;
            };
            let current_len = buf.len();
            let force_replace =
                matches!(tc.terminal_snapshot_mode, TerminalSnapshotMode::ReplaceSnapshot);
            if !force_replace && current_len == tc.terminal_bytes_seen {
                continue;
            }
            if !force_replace && current_len > tc.terminal_bytes_seen {
                TerminalUpdatePayload::Append {
                    bytes: buf[tc.terminal_bytes_seen..].to_vec(),
                    current_len,
                }
            } else {
                TerminalUpdatePayload::Replace { bytes: buf.clone(), current_len }
            }
        };
        if apply_terminal_payload(tc, payload) {
            tc.mark_tool_call_layout_dirty();
            dirty_from = Some(dirty_from.map_or(mi, |oldest| oldest.min(mi)));
            changed = true;
        }
    }

    drop(terminals);

    if let Some(mi) = dirty_from {
        app.mark_message_layout_dirty(mi);
    }

    changed
}
