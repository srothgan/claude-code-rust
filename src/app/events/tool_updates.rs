// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::super::{App, AppStatus, InvalidationLevel, MessageBlock, ToolCallInfo, ToolCallScope};
use super::tool_calls::{
    current_session_id, has_in_progress_tool_calls, json_value_size, log_terminal_spawned,
    sdk_tool_name_from_meta, should_jump_on_large_write, tool_scope_name,
};
use crate::agent::model;
use crate::app::todos::{parse_todos_if_present, set_todos};
use std::time::Instant;

pub(super) fn handle_tool_call_update_session(app: &mut App, tcu: &model::ToolCallUpdate) {
    let id_str = tcu.tool_call_id.clone();
    let Some((mi, bi)) = app.lookup_tool_call(&id_str) else {
        tracing::warn!(
            target: crate::logging::targets::APP_TOOL,
            event_name = "tool_call_update_missing",
            message = "tool call update dropped because tool call was not found",
            outcome = "dropped",
            session_id = %current_session_id(app),
            tool_call_id = %id_str,
            tool_status = ?tcu.fields.status,
        );
        return;
    };
    let tool_scope = app.tool_call_scope(&id_str);
    let previous_status = app.messages.get(mi).and_then(|message| message.blocks.get(bi)).and_then(
        |block| match block {
            MessageBlock::ToolCall(tc) => Some(tc.status),
            _ => None,
        },
    );
    let previous_terminal_id =
        app.messages.get(mi).and_then(|message| message.blocks.get(bi)).and_then(
            |block| match block {
                MessageBlock::ToolCall(tc) => tc.terminal_id.clone(),
                _ => None,
            },
        );
    apply_tool_scope_status_update(app, &id_str, tool_scope, tcu.fields.status);

    let update_outcome = apply_tool_call_update_to_indexed_block(app, mi, bi, &id_str, tcu);
    if let Some(mi) = update_outcome.layout_dirty_idx {
        app.recompute_message_retained_bytes(mi);
        app.invalidate_layout(InvalidationLevel::MessageChanged(mi));
    }
    log_tool_call_update_applied(app, &id_str, tcu, tool_scope, previous_status, &update_outcome);
    log_command_update_applied(app, &id_str, previous_status, previous_terminal_id.as_deref());
    if let Some(todos) = update_outcome.pending_todos {
        set_todos(app, todos);
    }
    if matches!(app.status, AppStatus::Running) && !has_in_progress_tool_calls(app) {
        app.status = AppStatus::Thinking;
    }
}

fn apply_tool_scope_status_update(
    app: &mut App,
    id_str: &str,
    tool_scope: Option<ToolCallScope>,
    status: Option<model::ToolCallStatus>,
) {
    let Some(status) = status else {
        return;
    };
    match tool_scope {
        Some(ToolCallScope::Subagent) => match status {
            model::ToolCallStatus::Pending | model::ToolCallStatus::InProgress => {
                app.mark_subagent_tool_started(id_str);
            }
            model::ToolCallStatus::Completed | model::ToolCallStatus::Failed => {
                app.mark_subagent_tool_finished(id_str, Instant::now());
            }
        },
        Some(ToolCallScope::Task) => match status {
            model::ToolCallStatus::Pending | model::ToolCallStatus::InProgress => {
                app.refresh_subagent_idle_since(Instant::now());
            }
            model::ToolCallStatus::Completed | model::ToolCallStatus::Failed => {
                app.remove_active_task(id_str);
                app.refresh_subagent_idle_since(Instant::now());
            }
        },
        Some(ToolCallScope::MainAgent) | None => {}
    }
}

struct ToolCallUpdateApplyOutcome {
    changed: bool,
    layout_dirty_idx: Option<usize>,
    pending_todos: Option<Vec<super::super::TodoItem>>,
}

fn apply_tool_call_update_to_indexed_block(
    app: &mut App,
    mi: usize,
    bi: usize,
    id_str: &str,
    tcu: &model::ToolCallUpdate,
) -> ToolCallUpdateApplyOutcome {
    let mut out =
        ToolCallUpdateApplyOutcome { changed: false, layout_dirty_idx: None, pending_todos: None };
    let terminals = std::rc::Rc::clone(&app.terminals);
    let session_id = current_session_id(app);
    let mut terminal_subscription: Option<String> = None;
    let mut detach_terminal = false;

    if let Some(MessageBlock::ToolCall(tc)) =
        app.messages.get_mut(mi).and_then(|m| m.blocks.get_mut(bi))
    {
        let tc = tc.as_mut();
        let mut changed = false;
        changed |= apply_tool_call_status_update(tc, tcu.fields.status);
        changed |= apply_tool_call_title_update(tc, tcu.fields.title.as_deref(), &app.cwd_raw);
        changed |= apply_tool_call_content_update(
            tc,
            tcu.fields.content.as_deref(),
            &terminals,
            &mut terminal_subscription,
        );
        changed |= apply_tool_call_raw_input_update(tc, tcu.fields.raw_input.as_ref());
        changed |= apply_tool_call_output_metadata_update(tc, tcu.fields.output_metadata.as_ref());
        changed |= apply_tool_call_raw_output_update(tc, tcu.fields.raw_output.as_ref());
        changed |= apply_tool_call_name_update(tc, tcu.meta.as_ref());
        out.pending_todos = extract_todo_updates_from_tool_call_update(
            id_str,
            &session_id,
            tc,
            tcu.fields.raw_input.as_ref(),
        );
        detach_terminal = detach_terminal_if_final(tc);

        if changed {
            out.changed = true;
            if should_jump_on_large_write(tc) {
                app.viewport.engage_auto_scroll();
            }
            tc.mark_tool_call_layout_dirty();
            app.sync_render_cache_slot(mi, bi);
            out.layout_dirty_idx = Some(mi);
        } else {
            crate::perf::mark("tool_update_noop_skips");
        }
    }

    if detach_terminal {
        app.untrack_terminal_tool_call(mi, bi);
    } else if let Some(terminal_id) = terminal_subscription {
        app.sync_terminal_tool_call(terminal_id, mi, bi);
    }

    out
}

fn apply_tool_call_status_update(
    tc: &mut ToolCallInfo,
    status: Option<model::ToolCallStatus>,
) -> bool {
    if let Some(status) = status
        && tc.status != status
    {
        tc.status = status;
        return true;
    }
    false
}

fn apply_tool_call_title_update(tc: &mut ToolCallInfo, title: Option<&str>, cwd_raw: &str) -> bool {
    let Some(title) = title else {
        return false;
    };
    let shortened = super::tool_calls::shorten_tool_title(title, cwd_raw);
    if tc.title == shortened {
        return false;
    }
    tc.title = shortened;
    true
}

fn apply_tool_call_content_update(
    tc: &mut ToolCallInfo,
    content: Option<&[model::ToolCallContent]>,
    terminals: &crate::agent::events::TerminalMap,
    terminal_subscription: &mut Option<String>,
) -> bool {
    let Some(content) = content else {
        return false;
    };
    let mut changed = false;
    for cb in content {
        if let model::ToolCallContent::Terminal(t) = cb {
            let tid = t.terminal_id.clone();
            if let Some(terminal) = terminals.borrow().get(&tid)
                && tc.terminal_command.as_deref() != Some(terminal.command.as_str())
            {
                tc.terminal_command = Some(terminal.command.clone());
                changed = true;
            }
            if tc.terminal_id.as_deref() != Some(tid.as_str()) {
                tc.terminal_id = Some(tid.clone());
                changed = true;
            }
            *terminal_subscription = Some(tid);
        }
    }
    if tc.content != content {
        tc.content = content.to_vec();
        changed = true;
    }
    changed
}

fn apply_tool_call_raw_input_update(
    tc: &mut ToolCallInfo,
    raw_input: Option<&serde_json::Value>,
) -> bool {
    let Some(raw_input) = raw_input else {
        return false;
    };
    tc.set_raw_input(Some(raw_input.clone()))
}

fn apply_tool_call_output_metadata_update(
    tc: &mut ToolCallInfo,
    output_metadata: Option<&model::ToolOutputMetadata>,
) -> bool {
    let Some(output_metadata) = output_metadata else {
        return false;
    };
    if tc.output_metadata.as_ref() == Some(output_metadata) {
        return false;
    }
    tc.output_metadata = Some(output_metadata.clone());
    true
}

fn apply_tool_call_raw_output_update(
    tc: &mut ToolCallInfo,
    raw_output: Option<&serde_json::Value>,
) -> bool {
    if !tc.is_execute_tool() {
        return false;
    }
    let Some(raw_output) = raw_output else {
        return false;
    };
    let Some(output) = raw_output_to_terminal_text(raw_output) else {
        return false;
    };
    if tc.terminal_output.as_deref() == Some(output.as_str()) {
        return false;
    }
    tc.terminal_output_len = output.len();
    tc.terminal_bytes_seen = output.len();
    tc.terminal_output = Some(output);
    tc.terminal_snapshot_mode = crate::app::TerminalSnapshotMode::ReplaceSnapshot;
    true
}

fn apply_tool_call_name_update(tc: &mut ToolCallInfo, meta: Option<&serde_json::Value>) -> bool {
    let Some(name) = sdk_tool_name_from_meta(meta) else {
        return false;
    };
    if name.trim().is_empty() || tc.sdk_tool_name == name {
        return false;
    }
    name.clone_into(&mut tc.sdk_tool_name);
    true
}

fn detach_terminal_if_final(tc: &mut ToolCallInfo) -> bool {
    if !tc.is_execute_tool()
        || matches!(tc.status, model::ToolCallStatus::Pending | model::ToolCallStatus::InProgress)
        || tc.terminal_id.is_none()
    {
        return false;
    }

    tc.terminal_id = None;
    true
}

fn extract_todo_updates_from_tool_call_update(
    id_str: &str,
    session_id: &str,
    tc: &ToolCallInfo,
    raw_input: Option<&serde_json::Value>,
) -> Option<Vec<super::super::TodoItem>> {
    if tc.sdk_tool_name != "TodoWrite" {
        return None;
    }
    let raw_input = raw_input?;
    if let Some(todos) = parse_todos_if_present(raw_input) {
        tracing::info!(
            target: crate::logging::targets::APP_TOOL,
            event_name = "tool_plan_synchronized",
            message = "todo plan synchronized from tool update",
            outcome = "success",
            session_id = %session_id,
            tool_call_id = %id_str,
            count = todos.len(),
            size_bytes = json_value_size(Some(raw_input)).unwrap_or_default(),
            tool_name = "TodoWrite",
            todo_count = todos.len(),
        );
        return Some(todos);
    }
    tracing::debug!(
        target: crate::logging::targets::APP_TOOL,
        event_name = "tool_plan_sync_skipped",
        message = "todo plan sync skipped for tool update",
        outcome = "skipped",
        session_id = %session_id,
        tool_call_id = %id_str,
        size_bytes = json_value_size(Some(raw_input)).unwrap_or_default(),
        tool_name = "TodoWrite",
    );
    None
}

pub(super) fn raw_output_to_terminal_text(raw_output: &serde_json::Value) -> Option<String> {
    match raw_output {
        serde_json::Value::Null => None,
        serde_json::Value::String(s) => (!s.is_empty()).then(|| s.clone()),
        serde_json::Value::Array(items) => {
            let chunks: Vec<&str> = items.iter().filter_map(extract_text_field).collect();
            if chunks.is_empty() {
                serde_json::to_string_pretty(raw_output).ok().filter(|s| !s.is_empty())
            } else {
                Some(chunks.join("\n"))
            }
        }
        value => extract_text_field(value)
            .map(str::to_owned)
            .or_else(|| serde_json::to_string_pretty(value).ok().filter(|s| !s.is_empty())),
    }
}

fn extract_text_field(value: &serde_json::Value) -> Option<&str> {
    value.get("text").and_then(serde_json::Value::as_str)
}

fn log_tool_call_update_applied(
    app: &App,
    id_str: &str,
    tcu: &model::ToolCallUpdate,
    tool_scope: Option<ToolCallScope>,
    previous_status: Option<model::ToolCallStatus>,
    update_outcome: &ToolCallUpdateApplyOutcome,
) {
    if !update_outcome.changed {
        return;
    }

    let Some(tc) = app
        .lookup_tool_call(id_str)
        .and_then(|(mi, bi)| app.messages.get(mi).and_then(|message| message.blocks.get(bi)))
        .and_then(|block| match block {
            MessageBlock::ToolCall(tc) => Some(tc.as_ref()),
            _ => None,
        })
    else {
        return;
    };

    let session_id = current_session_id(app);
    let log_spec = tool_update_log_spec(tc, tcu, previous_status);
    let scope_name = tool_scope.map_or("unknown", tool_scope_name);
    let raw_output_chars = tcu.fields.raw_output.as_ref().and_then(|value| match value {
        serde_json::Value::String(text) => Some(text.chars().count()),
        _ => serde_json::to_string(value).ok().map(|text| text.chars().count()),
    });
    let content_block_count = tcu.fields.content.as_ref().map_or(tc.content.len(), Vec::len);
    let raw_input_bytes = json_value_size(tcu.fields.raw_input.as_ref()).unwrap_or_default();
    let location_count = tcu.fields.locations.as_ref().map_or(0, Vec::len);

    match log_spec.level {
        ToolUpdateLogLevel::Info => tracing::info!(
            target: crate::logging::targets::APP_TOOL,
            event_name = log_spec.event_name,
            message = log_spec.message,
            outcome = log_spec.outcome,
            session_id = %session_id,
            tool_call_id = %id_str,
            tool_name = %tc.sdk_tool_name,
            tool_title = %tc.title,
            tool_scope = scope_name,
            previous_status = ?previous_status,
            tool_status = ?tc.status,
            content_block_count,
            raw_output_chars = raw_output_chars.unwrap_or_default(),
            has_output_metadata = tc.output_metadata.is_some(),
        ),
        ToolUpdateLogLevel::Warn => tracing::warn!(
            target: crate::logging::targets::APP_TOOL,
            event_name = log_spec.event_name,
            message = log_spec.message,
            outcome = log_spec.outcome,
            session_id = %session_id,
            tool_call_id = %id_str,
            tool_name = %tc.sdk_tool_name,
            tool_title = %tc.title,
            tool_scope = scope_name,
            previous_status = ?previous_status,
            tool_status = ?tc.status,
            content_block_count,
            raw_output_chars = raw_output_chars.unwrap_or_default(),
            has_output_metadata = tc.output_metadata.is_some(),
        ),
        ToolUpdateLogLevel::Debug => tracing::debug!(
            target: crate::logging::targets::APP_TOOL,
            event_name = log_spec.event_name,
            message = log_spec.message,
            outcome = log_spec.outcome,
            session_id = %session_id,
            tool_call_id = %id_str,
            tool_name = %tc.sdk_tool_name,
            tool_title = %tc.title,
            tool_scope = scope_name,
            previous_status = ?previous_status,
            tool_status = ?tc.status,
            content_block_count,
            raw_output_chars = raw_output_chars.unwrap_or_default(),
            has_output_metadata = tc.output_metadata.is_some(),
            title_changed = tcu.fields.title.is_some(),
            status_changed = tcu.fields.status != previous_status,
            raw_input_bytes,
            location_count,
        ),
    }
}

#[derive(Clone, Copy)]
enum ToolUpdateLogLevel {
    Info,
    Warn,
    Debug,
}

#[derive(Clone, Copy)]
struct ToolUpdateLogSpec {
    level: ToolUpdateLogLevel,
    event_name: &'static str,
    message: &'static str,
    outcome: &'static str,
}

fn tool_update_log_spec(
    tc: &ToolCallInfo,
    tcu: &model::ToolCallUpdate,
    previous_status: Option<model::ToolCallStatus>,
) -> ToolUpdateLogSpec {
    match tc.status {
        model::ToolCallStatus::Completed => ToolUpdateLogSpec {
            level: if entered_final_status(previous_status, tc.status) {
                ToolUpdateLogLevel::Info
            } else {
                ToolUpdateLogLevel::Debug
            },
            event_name: if entered_final_status(previous_status, tc.status) {
                "tool_call_completed"
            } else {
                "tool_call_updated"
            },
            message: if entered_final_status(previous_status, tc.status) {
                "tool call completed"
            } else {
                "tool call updated after completion"
            },
            outcome: "success",
        },
        model::ToolCallStatus::Failed => {
            if !entered_final_status(previous_status, tc.status) {
                return ToolUpdateLogSpec {
                    level: ToolUpdateLogLevel::Debug,
                    event_name: "tool_call_updated",
                    message: "tool call updated after failure",
                    outcome: "failure",
                };
            }
            if let Some(raw_output) = tcu.fields.raw_output.as_ref() {
                let text = match raw_output {
                    serde_json::Value::String(text) => text.to_ascii_lowercase(),
                    value => serde_json::to_string(value).unwrap_or_default().to_ascii_lowercase(),
                };
                if text.contains("permission denied")
                    || text.contains("cancelled by user")
                    || text.contains("plan rejected")
                    || text.contains("question cancelled")
                {
                    return ToolUpdateLogSpec {
                        level: ToolUpdateLogLevel::Info,
                        event_name: "tool_call_refused",
                        message: "tool call refused",
                        outcome: "cancelled",
                    };
                }
                if text.contains("timed out") || text.contains("timeout") {
                    return ToolUpdateLogSpec {
                        level: ToolUpdateLogLevel::Warn,
                        event_name: "tool_call_timeout",
                        message: "tool call timed out",
                        outcome: "timeout",
                    };
                }
            }
            ToolUpdateLogSpec {
                level: ToolUpdateLogLevel::Warn,
                event_name: "tool_call_failed",
                message: "tool call failed",
                outcome: "failure",
            }
        }
        model::ToolCallStatus::Pending | model::ToolCallStatus::InProgress => ToolUpdateLogSpec {
            level: ToolUpdateLogLevel::Debug,
            event_name: "tool_call_updated",
            message: "tool call updated",
            outcome: "success",
        },
    }
}

fn entered_final_status(
    previous_status: Option<model::ToolCallStatus>,
    current_status: model::ToolCallStatus,
) -> bool {
    matches!(current_status, model::ToolCallStatus::Completed | model::ToolCallStatus::Failed)
        && !matches!(previous_status, Some(status) if status == current_status)
}

fn log_command_update_applied(
    app: &App,
    id_str: &str,
    previous_status: Option<model::ToolCallStatus>,
    previous_terminal_id: Option<&str>,
) {
    let Some(tc) = app
        .lookup_tool_call(id_str)
        .and_then(|(mi, bi)| app.messages.get(mi).and_then(|message| message.blocks.get(bi)))
        .and_then(|block| match block {
            MessageBlock::ToolCall(tc) => Some(tc.as_ref()),
            _ => None,
        })
    else {
        return;
    };

    if !tc.is_execute_tool() {
        return;
    }

    if previous_terminal_id.is_none() && tc.terminal_id.is_some() {
        log_terminal_spawned(app, tc, "update");
    }

    let transitioned_to_final =
        matches!(
            previous_status,
            Some(model::ToolCallStatus::Pending | model::ToolCallStatus::InProgress)
        ) && matches!(tc.status, model::ToolCallStatus::Completed | model::ToolCallStatus::Failed);
    if !transitioned_to_final {
        return;
    }

    let failure_kind = command_failure_kind(tc);
    match tc.status {
        model::ToolCallStatus::Completed => tracing::info!(
            target: crate::logging::targets::APP_COMMAND,
            event_name = "command_completed",
            message = "command execution completed",
            outcome = "success",
            session_id = %current_session_id(app),
            tool_call_id = %tc.id,
            terminal_id = %tc.terminal_id.as_deref().unwrap_or(""),
            tool_name = %tc.sdk_tool_name,
            terminal_output_bytes = u64::try_from(tc.terminal_output_len).unwrap_or_default(),
            has_terminal = tc.terminal_id.is_some(),
            assistant_auto_backgrounded = tc.assistant_auto_backgrounded(),
            token_saver_active = tc.token_saver_active(),
        ),
        model::ToolCallStatus::Failed => tracing::warn!(
            target: crate::logging::targets::APP_COMMAND,
            event_name = "command_failed",
            message = "command execution failed",
            outcome = "failure",
            session_id = %current_session_id(app),
            tool_call_id = %tc.id,
            terminal_id = %tc.terminal_id.as_deref().unwrap_or(""),
            tool_name = %tc.sdk_tool_name,
            error_kind = failure_kind,
            terminal_output_bytes = u64::try_from(tc.terminal_output_len).unwrap_or_default(),
            has_terminal = tc.terminal_id.is_some(),
            assistant_auto_backgrounded = tc.assistant_auto_backgrounded(),
            token_saver_active = tc.token_saver_active(),
        ),
        model::ToolCallStatus::Pending | model::ToolCallStatus::InProgress => {}
    }
}

fn command_failure_kind(tc: &ToolCallInfo) -> &'static str {
    let text = tc.terminal_output.as_deref().unwrap_or("").to_ascii_lowercase();
    if text.contains("permission denied")
        || text.contains("cancelled by user")
        || text.contains("plan rejected")
        || text.contains("question cancelled")
    {
        return "refused";
    }
    if text.contains("timed out") || text.contains("timeout") {
        return "timeout";
    }
    "command_error"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{
        App, BlockCache, ChatMessage, MessageBlock, MessageRole, TerminalSnapshotMode,
    };

    fn make_bash_tool_call(
        id: &str,
        status: model::ToolCallStatus,
        terminal_id: Option<&str>,
    ) -> ToolCallInfo {
        ToolCallInfo {
            id: id.to_owned(),
            title: format!("tool {id}"),
            sdk_tool_name: "Bash".to_owned(),
            raw_input: None,
            raw_input_bytes: 0,
            output_metadata: None,
            status,
            content: Vec::new(),
            hidden: false,
            terminal_id: terminal_id.map(str::to_owned),
            terminal_command: Some("echo test".to_owned()),
            terminal_output: None,
            terminal_output_len: 0,
            terminal_bytes_seen: 0,
            terminal_snapshot_mode: TerminalSnapshotMode::AppendOnly,
            render_epoch: 0,
            layout_epoch: 0,
            last_measured_width: 0,
            last_measured_height: 0,
            last_measured_layout_epoch: 0,
            last_measured_layout_generation: 0,
            cache: BlockCache::default(),
            pending_permission: None,
            pending_question: None,
        }
    }

    fn terminal_content(terminal_id: &str) -> Vec<model::ToolCallContent> {
        vec![model::ToolCallContent::Terminal(model::TerminalToolCallContent::new(terminal_id))]
    }

    #[test]
    fn completed_execute_update_detaches_terminal_subscription() {
        let mut app = App::test_default();
        let tool_id = "tool-1";
        app.messages.push(ChatMessage::new(
            MessageRole::Assistant,
            vec![MessageBlock::ToolCall(Box::new(make_bash_tool_call(
                tool_id,
                model::ToolCallStatus::InProgress,
                Some("term-1"),
            )))],
            None,
        ));
        app.index_tool_call(tool_id.to_owned(), 0, 0);
        app.sync_terminal_tool_call("term-1".to_owned(), 0, 0);

        let update = model::ToolCallUpdate::new(
            tool_id,
            model::ToolCallUpdateFields::new()
                .status(model::ToolCallStatus::Completed)
                .raw_output(serde_json::Value::String("done".to_owned())),
        );

        handle_tool_call_update_session(&mut app, &update);

        let MessageBlock::ToolCall(tc) = &app.messages[0].blocks[0] else {
            panic!("expected tool call block");
        };
        assert_eq!(tc.status, model::ToolCallStatus::Completed);
        assert_eq!(tc.terminal_id, None);
        assert_eq!(tc.terminal_output.as_deref(), Some("done"));
        assert!(app.terminal_tool_calls.is_empty());
        assert!(app.terminal_tool_call_membership.is_empty());
    }

    #[test]
    fn repeated_terminal_updates_do_not_duplicate_subscription() {
        let mut app = App::test_default();
        let tool_id = "tool-1";
        app.messages.push(ChatMessage::new(
            MessageRole::Assistant,
            vec![MessageBlock::ToolCall(Box::new(make_bash_tool_call(
                tool_id,
                model::ToolCallStatus::InProgress,
                None,
            )))],
            None,
        ));
        app.index_tool_call(tool_id.to_owned(), 0, 0);

        let update = model::ToolCallUpdate::new(
            tool_id,
            model::ToolCallUpdateFields::new().content(terminal_content("term-1")),
        );

        handle_tool_call_update_session(&mut app, &update);
        handle_tool_call_update_session(&mut app, &update);

        assert_eq!(app.terminal_tool_calls.len(), 1);
        assert_eq!(app.terminal_tool_call_membership.len(), 1);
        assert_eq!(app.terminal_tool_calls[0].terminal_id, "term-1");
    }

    #[test]
    fn terminal_update_replaces_stale_subscription_for_same_tool_call() {
        let mut app = App::test_default();
        let tool_id = "tool-1";
        app.messages.push(ChatMessage::new(
            MessageRole::Assistant,
            vec![MessageBlock::ToolCall(Box::new(make_bash_tool_call(
                tool_id,
                model::ToolCallStatus::InProgress,
                Some("term-1"),
            )))],
            None,
        ));
        app.index_tool_call(tool_id.to_owned(), 0, 0);
        app.sync_terminal_tool_call("term-1".to_owned(), 0, 0);

        let update = model::ToolCallUpdate::new(
            tool_id,
            model::ToolCallUpdateFields::new().content(terminal_content("term-2")),
        );

        handle_tool_call_update_session(&mut app, &update);

        assert_eq!(app.terminal_tool_calls.len(), 1);
        assert_eq!(app.terminal_tool_call_membership.len(), 1);
        assert_eq!(app.terminal_tool_calls[0].terminal_id, "term-2");
        let MessageBlock::ToolCall(tc) = &app.messages[0].blocks[0] else {
            panic!("expected tool call block");
        };
        assert_eq!(tc.terminal_id.as_deref(), Some("term-2"));
    }

    #[test]
    fn repeated_completed_status_update_does_not_log_a_second_completion() {
        let tc = make_bash_tool_call("tool-1", model::ToolCallStatus::Completed, None);
        let update = model::ToolCallUpdate::new("tool-1", model::ToolCallUpdateFields::new());

        let spec = tool_update_log_spec(&tc, &update, Some(model::ToolCallStatus::Completed));

        assert!(matches!(spec.level, ToolUpdateLogLevel::Debug));
        assert_eq!(spec.event_name, "tool_call_updated");
        assert_eq!(spec.outcome, "success");
    }

    #[test]
    fn first_completed_status_update_logs_completion() {
        let tc = make_bash_tool_call("tool-1", model::ToolCallStatus::Completed, None);
        let update = model::ToolCallUpdate::new(
            "tool-1",
            model::ToolCallUpdateFields::new().status(model::ToolCallStatus::Completed),
        );

        let spec = tool_update_log_spec(&tc, &update, Some(model::ToolCallStatus::InProgress));

        assert!(matches!(spec.level, ToolUpdateLogLevel::Info));
        assert_eq!(spec.event_name, "tool_call_completed");
        assert_eq!(spec.outcome, "success");
    }
}
