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
    App, AppStatus, BlockCache, ChatMessage, InvalidationLevel, MessageBlock, MessageRole,
    ToolCallInfo, ToolCallScope,
};
use super::tool_updates::raw_output_to_terminal_text;
use crate::agent::model;
use crate::app::todos::{parse_todos_if_present, set_todos};
use std::time::Instant;

pub(super) fn handle_tool_call(app: &mut App, tc: model::ToolCall) {
    log_tool_call_received(&tc);
    let id_str = tc.tool_call_id.clone();
    let sdk_tool_name = resolve_sdk_tool_name(tc.kind, tc.meta.as_ref());
    let scope = register_tool_call_scope(app, &id_str, &sdk_tool_name);
    maybe_apply_todo_write_from_tool_call(app, &id_str, &sdk_tool_name, tc.raw_input.as_ref());
    update_subagent_scope_state(app, scope, tc.status, &id_str);

    let tool_info = build_tool_info_from_tool_call(app, tc, sdk_tool_name);
    if should_jump_on_large_write(&tool_info) {
        app.viewport.engage_auto_scroll();
    }
    upsert_tool_call_into_assistant_message(app, tool_info);

    app.status = AppStatus::Running;
    app.files_accessed += 1;
}

fn log_tool_call_received(tc: &model::ToolCall) {
    let id_str = tc.tool_call_id.clone();
    let title = tc.title.clone();
    let kind = tc.kind;
    tracing::debug!(
        "ToolCall: id={id_str} title={title} kind={kind:?} status={:?} content_blocks={} has_raw_output={}",
        tc.status,
        tc.content.len(),
        tc.raw_output.is_some()
    );
}

pub(super) fn register_tool_call_scope(
    app: &mut App,
    id: &str,
    sdk_tool_name: &str,
) -> ToolCallScope {
    // TODO: When the bridge exposes an explicit Task/Agent <-> child-tool relation,
    // redesign subagent rendering so the parent Task/Agent summary block becomes
    // the primary visible surface and child agent tools are not rendered directly.
    let is_task = matches!(sdk_tool_name, "Task" | "Agent");
    let scope = if is_task {
        ToolCallScope::Task
    } else if app.active_task_ids.is_empty() {
        ToolCallScope::MainAgent
    } else {
        ToolCallScope::Subagent
    };
    app.register_tool_call_scope(id.to_owned(), scope);
    if is_task {
        app.insert_active_task(id.to_owned());
    }
    scope
}

fn maybe_apply_todo_write_from_tool_call(
    app: &mut App,
    id: &str,
    sdk_tool_name: &str,
    raw_input: Option<&serde_json::Value>,
) {
    if sdk_tool_name != "TodoWrite" {
        return;
    }
    tracing::info!("TodoWrite ToolCall detected: id={id}, raw_input={raw_input:?}");
    if let Some(raw_input) = raw_input {
        if let Some(todos) = parse_todos_if_present(raw_input) {
            tracing::info!("Parsed {} todos from ToolCall raw_input", todos.len());
            set_todos(app, todos);
        } else {
            tracing::debug!(
                "TodoWrite ToolCall raw_input has no todos array yet; preserving existing todos"
            );
        }
    } else {
        tracing::warn!("TodoWrite ToolCall has no raw_input");
    }
}

pub(super) fn update_subagent_scope_state(
    app: &mut App,
    scope: ToolCallScope,
    status: model::ToolCallStatus,
    id: &str,
) {
    match (scope, status) {
        (
            ToolCallScope::Subagent,
            model::ToolCallStatus::InProgress | model::ToolCallStatus::Pending,
        ) => app.mark_subagent_tool_started(id),
        (
            ToolCallScope::Subagent,
            model::ToolCallStatus::Completed | model::ToolCallStatus::Failed,
        ) => app.mark_subagent_tool_finished(id, Instant::now()),
        _ => app.refresh_subagent_idle_since(Instant::now()),
    }
}

fn build_tool_info_from_tool_call(
    app: &App,
    tc: model::ToolCall,
    sdk_tool_name: String,
) -> ToolCallInfo {
    let initial_execute_output = if super::super::is_execute_tool_name(&sdk_tool_name) {
        tc.raw_output.as_ref().and_then(raw_output_to_terminal_text)
    } else {
        None
    };

    let mut tool_info = ToolCallInfo {
        id: tc.tool_call_id,
        title: shorten_tool_title(&tc.title, &app.cwd_raw),
        sdk_tool_name,
        raw_input: tc.raw_input,
        status: tc.status,
        content: tc.content,
        collapsed: app.tools_collapsed,
        hidden: false,
        terminal_id: None,
        terminal_command: None,
        terminal_output: None,
        terminal_output_len: 0,
        terminal_bytes_seen: 0,
        terminal_snapshot_mode: crate::app::TerminalSnapshotMode::AppendOnly,
        render_epoch: 0,
        layout_epoch: 0,
        last_measured_width: 0,
        last_measured_height: 0,
        last_measured_layout_epoch: 0,
        last_measured_layout_generation: 0,
        cache: BlockCache::default(),
        pending_permission: None,
    };
    if let Some(output) = initial_execute_output {
        tool_info.terminal_output_len = output.len();
        tool_info.terminal_bytes_seen = output.len();
        tool_info.terminal_output = Some(output);
        tool_info.terminal_snapshot_mode = crate::app::TerminalSnapshotMode::ReplaceSnapshot;
    }
    tool_info
}

pub(super) fn upsert_tool_call_into_assistant_message(app: &mut App, tool_info: ToolCallInfo) {
    let msg_idx = app.messages.len().saturating_sub(1);
    let existing_pos = app.lookup_tool_call(&tool_info.id);
    let is_assistant =
        app.messages.last().is_some_and(|m| matches!(m.role, MessageRole::Assistant));

    if is_assistant {
        if let Some((mi, bi)) = existing_pos {
            update_existing_tool_call(app, mi, bi, &tool_info);
        } else if let Some(last) = app.messages.last_mut() {
            let block_idx = last.blocks.len();
            let tc_id = tool_info.id.clone();
            last.blocks.push(MessageBlock::ToolCall(Box::new(tool_info)));
            app.index_tool_call(tc_id, msg_idx, block_idx);
        }
    } else {
        let tc_id = tool_info.id.clone();
        let new_idx = app.messages.len();
        app.messages.push(ChatMessage {
            role: MessageRole::Assistant,
            blocks: vec![MessageBlock::ToolCall(Box::new(tool_info))],
            usage: None,
        });
        app.index_tool_call(tc_id, new_idx, 0);
    }
}

fn update_existing_tool_call(app: &mut App, mi: usize, bi: usize, tool_info: &ToolCallInfo) {
    let mut layout_dirty = false;
    if let Some(MessageBlock::ToolCall(existing)) =
        app.messages.get_mut(mi).and_then(|m| m.blocks.get_mut(bi))
    {
        let existing = existing.as_mut();
        let mut changed = false;
        changed |= sync_if_changed(&mut existing.title, &tool_info.title);
        changed |= sync_if_changed(&mut existing.status, &tool_info.status);
        changed |= sync_if_changed(&mut existing.content, &tool_info.content);
        changed |= sync_if_changed(&mut existing.sdk_tool_name, &tool_info.sdk_tool_name);
        changed |= sync_if_changed(&mut existing.raw_input, &tool_info.raw_input);
        if changed {
            existing.mark_tool_call_layout_dirty();
            layout_dirty = true;
        } else {
            crate::perf::mark("tool_update_noop_skips");
        }
    }
    if layout_dirty {
        app.invalidate_layout(InvalidationLevel::Single(mi));
    }
}

pub(super) fn sync_if_changed<T: PartialEq + Clone>(dst: &mut T, src: &T) -> bool {
    if dst == src {
        return false;
    }
    dst.clone_from(src);
    true
}

pub(super) fn sdk_tool_name_from_meta(meta: Option<&serde_json::Value>) -> Option<&str> {
    meta.and_then(|m| m.get("claudeCode")).and_then(|v| v.get("toolName")).and_then(|v| v.as_str())
}

fn fallback_sdk_tool_name(kind: model::ToolKind) -> &'static str {
    match kind {
        model::ToolKind::Read => "Read",
        model::ToolKind::Edit => "Edit",
        model::ToolKind::Delete => "Delete",
        model::ToolKind::Move => "Move",
        model::ToolKind::Search => "Search",
        model::ToolKind::Execute => "Bash",
        model::ToolKind::Think => "Think",
        model::ToolKind::Fetch => "Fetch",
        model::ToolKind::SwitchMode => "ExitPlanMode",
        model::ToolKind::Other => "Tool",
    }
}

pub(super) fn resolve_sdk_tool_name(
    kind: model::ToolKind,
    meta: Option<&serde_json::Value>,
) -> String {
    if let Some(name) = sdk_tool_name_from_meta(meta).filter(|name| !name.trim().is_empty()) {
        name.to_owned()
    } else {
        let fallback = fallback_sdk_tool_name(kind);
        if matches!(kind, model::ToolKind::Think) {
            tracing::warn!(
                "ToolKind::Think tool arrived with no meta.claudeCode.toolName -- \
                 Task/Agent scope detection may be incorrect; falling back to '{fallback}'"
            );
        }
        fallback.to_owned()
    }
}

/// Shorten absolute paths in tool titles to relative paths based on cwd.
/// e.g. "Read C:\\Users\\me\\project\\src\\main.rs" -> "Read src/main.rs"
/// Handles both `/` and `\\` separators on all platforms since the bridge adapter
/// may use either regardless of the host OS.
pub(super) fn shorten_tool_title(title: &str, cwd_raw: &str) -> String {
    if cwd_raw.is_empty() {
        return title.to_owned();
    }

    // Quick check: if title doesn't contain any part of cwd, skip normalization
    // Use the first path component of cwd as a heuristic
    let cwd_start = cwd_raw.split(['/', '\\']).find(|s| !s.is_empty()).unwrap_or(cwd_raw);
    if !title.contains(cwd_start) {
        return title.to_owned();
    }

    // Normalize both to forward slashes for matching
    let cwd_norm = cwd_raw.replace('\\', "/");
    let title_norm = title.replace('\\', "/");

    // Ensure cwd ends with slash so we strip the separator too
    let with_sep = if cwd_norm.ends_with('/') { cwd_norm } else { format!("{cwd_norm}/") };

    if title_norm.contains(&with_sep) {
        return title_norm.replace(&with_sep, "");
    }
    title_norm
}

pub(super) const WRITE_DIFF_JUMP_THRESHOLD_LINES: usize = 40;

pub(super) fn should_jump_on_large_write(tc: &ToolCallInfo) -> bool {
    if tc.sdk_tool_name != "Write" {
        return false;
    }
    tc.content.iter().any(|c| match c {
        model::ToolCallContent::Diff(diff) => {
            let new_lines = diff.new_text.lines().count();
            let old_lines = diff.old_text.as_deref().map_or(0, |t| t.lines().count());
            new_lines.max(old_lines) >= WRITE_DIFF_JUMP_THRESHOLD_LINES
        }
        _ => false,
    })
}

/// Check if any tool call in the current assistant message is still in-progress.
pub(super) fn has_in_progress_tool_calls(app: &App) -> bool {
    if let Some(last) = app.messages.last()
        && matches!(last.role, MessageRole::Assistant)
    {
        return last.blocks.iter().any(|block| {
            matches!(
                block,
                MessageBlock::ToolCall(tc)
                    if matches!(tc.status, model::ToolCallStatus::InProgress | model::ToolCallStatus::Pending)
            )
        });
    }
    false
}
