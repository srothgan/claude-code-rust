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

use super::block_cache::BlockCache;
use crate::agent::model;

pub struct ToolCallInfo {
    pub id: String,
    pub title: String,
    /// The SDK tool name from `meta.claudeCode.toolName` when available.
    /// Falls back to a derived name when metadata is absent.
    pub sdk_tool_name: String,
    pub raw_input: Option<serde_json::Value>,
    pub status: model::ToolCallStatus,
    pub content: Vec<model::ToolCallContent>,
    pub collapsed: bool,
    /// Hidden tool calls are subagent children - not rendered directly.
    pub hidden: bool,
    /// Terminal ID if this is a Bash-like SDK tool call with a running/completed terminal.
    pub terminal_id: Option<String>,
    /// The shell command that was executed (e.g. "echo hello && ls -la").
    pub terminal_command: Option<String>,
    /// Snapshot of terminal output, updated each frame while `InProgress`.
    pub terminal_output: Option<String>,
    /// Length of terminal buffer at last snapshot - used to skip O(n) re-snapshots
    /// when the buffer hasn't grown.
    pub terminal_output_len: usize,
    /// Number of terminal output bytes consumed for incremental append updates.
    pub terminal_bytes_seen: usize,
    /// Current terminal snapshot ingestion mode.
    pub terminal_snapshot_mode: TerminalSnapshotMode,
    /// Monotonic generation for render-affecting changes.
    pub render_epoch: u64,
    /// Monotonic generation for layout-affecting changes.
    pub layout_epoch: u64,
    /// Last measured width used by tool-call height cache.
    pub last_measured_width: u16,
    /// Last measured visual height in wrapped rows.
    pub last_measured_height: usize,
    /// Layout epoch used for the last measured height.
    pub last_measured_layout_epoch: u64,
    /// Global layout generation used for the last measured height.
    pub last_measured_layout_generation: u64,
    /// Per-block render cache for this tool call.
    pub cache: BlockCache,
    /// Inline permission prompt - rendered inside this tool call block.
    pub pending_permission: Option<InlinePermission>,
    /// Inline question prompt from `AskUserQuestion`.
    pub pending_question: Option<InlineQuestion>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalSnapshotMode {
    AppendOnly,
    ReplaceSnapshot,
}

impl ToolCallInfo {
    #[must_use]
    pub fn is_execute_tool(&self) -> bool {
        is_execute_tool_name(&self.sdk_tool_name)
    }

    #[must_use]
    pub fn is_ask_question_tool(&self) -> bool {
        is_ask_question_tool_name(&self.sdk_tool_name)
    }

    #[must_use]
    pub fn is_exit_plan_mode_tool(&self) -> bool {
        is_exit_plan_mode_tool_name(&self.sdk_tool_name)
    }

    /// Mark render cache for this tool call as stale.
    pub fn mark_tool_call_render_dirty(&mut self) {
        crate::perf::mark("tc_invalidations_requested");
        self.render_epoch = self.render_epoch.wrapping_add(1);
        self.cache.invalidate();
        crate::perf::mark("tc_invalidations_applied");
    }

    /// Mark layout cache for this tool call as stale.
    pub fn mark_tool_call_layout_dirty(&mut self) {
        self.layout_epoch = self.layout_epoch.wrapping_add(1);
        self.last_measured_width = 0;
        self.last_measured_height = 0;
        self.last_measured_layout_epoch = 0;
        self.last_measured_layout_generation = 0;
        self.mark_tool_call_render_dirty();
    }

    #[must_use]
    pub fn cache_measurement_key_matches(&self, width: u16, layout_generation: u64) -> bool {
        self.last_measured_width == width
            && self.last_measured_layout_epoch == self.layout_epoch
            && self.last_measured_layout_generation == layout_generation
    }

    pub fn record_measured_height(&mut self, width: u16, height: usize, layout_generation: u64) {
        self.last_measured_width = width;
        self.last_measured_height = height;
        self.last_measured_layout_epoch = self.layout_epoch;
        self.last_measured_layout_generation = layout_generation;
    }
}

#[must_use]
pub fn is_execute_tool_name(tool_name: &str) -> bool {
    tool_name.eq_ignore_ascii_case("bash")
}

#[must_use]
pub fn is_ask_question_tool_name(tool_name: &str) -> bool {
    tool_name.eq_ignore_ascii_case("askuserquestion")
}

#[must_use]
pub fn is_exit_plan_mode_tool_name(tool_name: &str) -> bool {
    tool_name.eq_ignore_ascii_case("exitplanmode")
}

/// Permission state stored inline on a `ToolCallInfo`, so the permission
/// controls render inside the tool call block (unified edit/permission UX).
pub struct InlinePermission {
    pub options: Vec<model::PermissionOption>,
    pub response_tx: tokio::sync::oneshot::Sender<model::RequestPermissionResponse>,
    pub selected_index: usize,
    /// Whether this permission currently has keyboard focus.
    /// When multiple permissions are pending, only the focused one
    /// shows the selection arrow and accepts Left/Right/Enter input.
    pub focused: bool,
}

pub struct InlineQuestion {
    pub prompt: model::QuestionPrompt,
    pub response_tx: tokio::sync::oneshot::Sender<model::RequestQuestionResponse>,
    pub focused_option_index: usize,
    pub selected_option_indices: std::collections::BTreeSet<usize>,
    pub notes: String,
    pub notes_cursor: usize,
    pub editing_notes: bool,
    pub focused: bool,
    pub question_index: usize,
    pub total_questions: usize,
}
