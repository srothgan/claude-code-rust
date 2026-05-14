// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::block_cache::BlockCache;
use crate::agent::model;

pub struct ToolCallInfo {
    pub id: String,
    pub title: String,
    /// The SDK tool name from `meta.claudeCode.toolName` when available.
    /// Falls back to a derived name when metadata is absent.
    pub sdk_tool_name: String,
    pub raw_input: Option<serde_json::Value>,
    pub raw_input_bytes: usize,
    pub output_metadata: Option<model::ToolOutputMetadata>,
    pub task_metadata: Option<model::TaskMetadata>,
    pub status: model::ToolCallStatus,
    pub content: Vec<model::ToolCallContent>,
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
    pub(crate) fn estimate_json_value_bytes(value: &serde_json::Value) -> usize {
        serde_json::to_string(value).map_or(0, |json| json.len())
    }

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

    #[must_use]
    pub fn assistant_auto_backgrounded(&self) -> bool {
        self.output_metadata
            .as_ref()
            .and_then(|metadata| metadata.bash.as_ref())
            .and_then(|metadata| metadata.assistant_auto_backgrounded)
            .unwrap_or(false)
    }

    #[must_use]
    pub fn verification_nudge_needed(&self) -> bool {
        self.output_metadata
            .as_ref()
            .and_then(|metadata| metadata.todo_write.as_ref())
            .and_then(|metadata| metadata.verification_nudge_needed)
            .unwrap_or(false)
    }

    #[must_use]
    pub fn task_is_backgrounded(&self) -> bool {
        self.task_metadata.as_ref().and_then(|metadata| metadata.is_backgrounded).unwrap_or(false)
    }

    #[must_use]
    pub fn hidden_unless_focused_interaction(&self) -> bool {
        self.hidden
            && !self.pending_permission.as_ref().is_some_and(|permission| permission.focused)
            && !self.pending_question.as_ref().is_some_and(|question| question.focused)
    }

    #[must_use]
    pub fn is_hidden_focused_interaction(&self) -> bool {
        self.hidden
            && (self.pending_permission.as_ref().is_some_and(|permission| permission.focused)
                || self.pending_question.as_ref().is_some_and(|question| question.focused))
    }

    #[must_use]
    pub fn is_subagent_root_tool(&self) -> bool {
        !self.hidden && matches!(self.sdk_tool_name.as_str(), "Task" | "Agent")
    }

    /// Invalidate cached rendered lines for this tool call.
    pub fn invalidate_render_cache(&mut self) {
        crate::perf::mark("tc_invalidations_requested");
        self.cache.invalidate();
        crate::perf::mark("tc_invalidations_applied");
    }

    pub fn set_raw_input(&mut self, raw_input: Option<serde_json::Value>) -> bool {
        if self.raw_input == raw_input {
            return false;
        }
        self.raw_input_bytes = raw_input.as_ref().map_or(0, Self::estimate_json_value_bytes);
        self.raw_input = raw_input;
        true
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
    pub display: Option<model::PermissionDisplay>,
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
