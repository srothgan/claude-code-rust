// SPDX-License-Identifier: Apache-2.0
use super::super::{App, MessageBlock, ToolCallInfo};
use super::tool_calls::{
    current_session_id, json_value_size, log_terminal_spawned, parent_tool_use_id_from_meta,
    sdk_tool_name_from_meta, tool_scope_name,
};
use crate::agent::model;

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

    let (previous_status, previous_terminal_id) = app
        .tool_call_at(mi, bi)
        .map_or((None, None), |tc| (Some(tc.status), tc.terminal_id.clone()));

    let changed = apply_tool_call_update_to_indexed_block(app, mi, bi, tcu);
    let history_changed = app.sync_tool_call_history(mi, bi);
    let task_display_changed = crate::app::tasks::refresh_task_tool_display_at(app, mi, bi);
    if changed || history_changed || task_display_changed {
        app.finish_block_change(mi, bi);
    }
    let Some(tc) = app.tool_call_at(mi, bi) else {
        return;
    };
    if changed {
        log_tool_call_update_applied(app, tc, tcu, previous_status);
    }
    log_command_update_applied(app, tc, previous_status, previous_terminal_id.as_deref());
}

fn apply_tool_call_update_to_indexed_block(
    app: &mut App,
    mi: usize,
    bi: usize,
    tcu: &model::ToolCallUpdate,
) -> bool {
    let Some(MessageBlock::ToolCall(tc)) =
        app.transcript.messages.get_mut(mi).and_then(|m| m.blocks.get_mut(bi))
    else {
        return false;
    };
    let tc = tc.as_mut();
    let mut changed = false;
    changed |= tc.add_source_message_uuid(tcu.source_message_uuid.as_deref());
    changed |= apply_tool_call_status_update(tc, tcu.fields.status);
    changed |= apply_tool_call_title_update(tc, tcu.fields.title.as_deref(), &app.cwd_raw);
    changed |= apply_tool_call_content_update(tc, tcu.fields.content.as_deref());
    changed |= apply_tool_call_raw_input_update(tc, tcu.fields.raw_input.as_ref());
    changed |= apply_tool_call_locations_update(tc, tcu.fields.locations.as_ref());
    changed |= apply_tool_call_output_metadata_update(tc, tcu.fields.output_metadata.as_ref());
    changed |= apply_tool_call_task_metadata_update(tc, tcu.fields.task_metadata.as_ref());
    if let Some(kind) = tcu.fields.kind {
        changed |= super::tool_calls::sync_if_changed(&mut tc.kind, &kind);
    }
    changed |= apply_tool_call_raw_output_update(tc, tcu.fields.raw_output.as_ref());
    changed |= apply_tool_call_name_update(tc, tcu.meta.as_ref());
    changed |= apply_tool_call_parent_update(tc, tcu.meta.as_ref());
    if changed {
        tc.invalidate_render_cache();
    }
    changed
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
) -> bool {
    let Some(content) = content else {
        return false;
    };
    let mut changed = false;
    for cb in content {
        if let model::ToolCallContent::Terminal(t) = cb
            && tc.terminal_id.as_deref() != Some(t.terminal_id.as_str())
        {
            tc.terminal_id = Some(t.terminal_id.clone());
            changed = true;
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

fn apply_tool_call_locations_update(
    tc: &mut ToolCallInfo,
    locations: Option<&Vec<model::ToolCallLocation>>,
) -> bool {
    let Some(locations) = locations else {
        return false;
    };
    tc.set_locations(locations.clone())
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

fn apply_tool_call_task_metadata_update(
    tc: &mut ToolCallInfo,
    task_metadata: Option<&model::TaskMetadata>,
) -> bool {
    let Some(task_metadata) = task_metadata else {
        return false;
    };
    if tc.task_metadata.as_ref() == Some(task_metadata) {
        return false;
    }
    tc.task_metadata = Some(task_metadata.clone());
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
    tc.terminal_output = Some(output);
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

fn apply_tool_call_parent_update(tc: &mut ToolCallInfo, meta: Option<&serde_json::Value>) -> bool {
    let Some(parent) = parent_tool_use_id_from_meta(meta) else {
        return false;
    };
    super::tool_calls::sync_if_changed(&mut tc.parent_tool_use_id, &Some(parent.to_owned()))
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
    tc: &ToolCallInfo,
    tcu: &model::ToolCallUpdate,
    previous_status: Option<model::ToolCallStatus>,
) {
    let session_id = current_session_id(app);
    let log_spec = tool_update_log_spec(tc, tcu, previous_status);
    let scope_name = tool_scope_name(&tc.scope());
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
            tool_call_id = %tc.id,
            tool_name = %tc.sdk_tool_name,
            tool_title = %tc.title,
            tool_scope = scope_name,
            previous_status = ?previous_status,
            tool_status = ?tc.status,
            content_block_count,
            raw_output_chars = raw_output_chars.unwrap_or_default(),
            has_output_metadata = tc.output_metadata.is_some(),
            has_task_metadata = tc.task_metadata.is_some(),
        ),
        ToolUpdateLogLevel::Warn => tracing::warn!(
            target: crate::logging::targets::APP_TOOL,
            event_name = log_spec.event_name,
            message = log_spec.message,
            outcome = log_spec.outcome,
            session_id = %session_id,
            tool_call_id = %tc.id,
            tool_name = %tc.sdk_tool_name,
            tool_title = %tc.title,
            tool_scope = scope_name,
            previous_status = ?previous_status,
            tool_status = ?tc.status,
            content_block_count,
            raw_output_chars = raw_output_chars.unwrap_or_default(),
            has_output_metadata = tc.output_metadata.is_some(),
            has_task_metadata = tc.task_metadata.is_some(),
        ),
        ToolUpdateLogLevel::Debug => tracing::debug!(
            target: crate::logging::targets::APP_TOOL,
            event_name = log_spec.event_name,
            message = log_spec.message,
            outcome = log_spec.outcome,
            session_id = %session_id,
            tool_call_id = %tc.id,
            tool_name = %tc.sdk_tool_name,
            tool_title = %tc.title,
            tool_scope = scope_name,
            previous_status = ?previous_status,
            tool_status = ?tc.status,
            content_block_count,
            raw_output_chars = raw_output_chars.unwrap_or_default(),
            has_output_metadata = tc.output_metadata.is_some(),
            has_task_metadata = tc.task_metadata.is_some(),
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
        model::ToolCallStatus::Failed | model::ToolCallStatus::Killed => {
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
                event_name: if matches!(tc.status, model::ToolCallStatus::Killed) {
                    "tool_call_killed"
                } else {
                    "tool_call_failed"
                },
                message: if matches!(tc.status, model::ToolCallStatus::Killed) {
                    "tool call killed"
                } else {
                    "tool call failed"
                },
                outcome: "failure",
            }
        }
        model::ToolCallStatus::Pending
        | model::ToolCallStatus::InProgress
        | model::ToolCallStatus::Detached => ToolUpdateLogSpec {
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
    matches!(
        current_status,
        model::ToolCallStatus::Completed
            | model::ToolCallStatus::Failed
            | model::ToolCallStatus::Killed
    ) && !matches!(previous_status, Some(status) if status == current_status)
}

fn log_command_update_applied(
    app: &App,
    tc: &ToolCallInfo,
    previous_status: Option<model::ToolCallStatus>,
    previous_terminal_id: Option<&str>,
) {
    if !tc.is_execute_tool() {
        return;
    }

    if previous_terminal_id.is_none() && tc.terminal_id.is_some() {
        log_terminal_spawned(app, tc, "update");
    }

    let transitioned_to_final = matches!(
        previous_status,
        Some(
            model::ToolCallStatus::Pending
                | model::ToolCallStatus::InProgress
                | model::ToolCallStatus::Detached
        )
    ) && matches!(
        tc.status,
        model::ToolCallStatus::Completed
            | model::ToolCallStatus::Failed
            | model::ToolCallStatus::Killed
    );
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
        ),
        model::ToolCallStatus::Failed | model::ToolCallStatus::Killed => tracing::warn!(
            target: crate::logging::targets::APP_COMMAND,
            event_name = if matches!(tc.status, model::ToolCallStatus::Killed) {
                "command_killed"
            } else {
                "command_failed"
            },
            message = if matches!(tc.status, model::ToolCallStatus::Killed) {
                "command execution killed"
            } else {
                "command execution failed"
            },
            outcome = "failure",
            session_id = %current_session_id(app),
            tool_call_id = %tc.id,
            terminal_id = %tc.terminal_id.as_deref().unwrap_or(""),
            tool_name = %tc.sdk_tool_name,
            error_kind = failure_kind,
            terminal_output_bytes = u64::try_from(tc.terminal_output_len).unwrap_or_default(),
            has_terminal = tc.terminal_id.is_some(),
            assistant_auto_backgrounded = tc.assistant_auto_backgrounded(),
        ),
        model::ToolCallStatus::Pending
        | model::ToolCallStatus::InProgress
        | model::ToolCallStatus::Detached => {}
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
    use crate::app::{App, ChatMessage, MessageBlock, MessageRole, ToolCallScope};

    fn make_bash_tool_call(
        id: &str,
        status: model::ToolCallStatus,
        terminal_id: Option<&str>,
    ) -> ToolCallInfo {
        ToolCallInfo {
            title: format!("tool {id}"),
            terminal_id: terminal_id.map(str::to_owned),
            terminal_command: Some("echo test".to_owned()),
            ..crate::app::test_support::named_tool_call_info(id, "Bash", status)
        }
    }

    fn terminal_content(terminal_id: &str) -> Vec<model::ToolCallContent> {
        vec![model::ToolCallContent::Terminal(model::TerminalToolCallContent::new(terminal_id))]
    }

    fn make_task_tool_call(id: &str, status: model::ToolCallStatus) -> ToolCallInfo {
        ToolCallInfo {
            title: format!("task {id}"),
            ..crate::app::test_support::named_tool_call_info(id, "Agent", status)
        }
    }

    #[test]
    fn completed_execute_update_preserves_terminal_metadata() {
        let mut app = App::test_default();
        let tool_id = "tool-1";
        app.transcript.messages.push(ChatMessage::new(
            MessageRole::Assistant,
            vec![MessageBlock::ToolCall(Box::new(make_bash_tool_call(
                tool_id,
                model::ToolCallStatus::InProgress,
                Some("term-1"),
            )))],
            None,
        ));
        app.index_tool_call(tool_id.to_owned(), 0, 0);

        let update = model::ToolCallUpdate::new(
            tool_id,
            model::ToolCallUpdateFields::new()
                .status(model::ToolCallStatus::Completed)
                .raw_output(serde_json::Value::String("done".to_owned())),
        );

        handle_tool_call_update_session(&mut app, &update);

        let MessageBlock::ToolCall(tc) = &app.transcript.messages[0].blocks[0] else {
            panic!("expected tool call block");
        };
        assert_eq!(tc.status, model::ToolCallStatus::Completed);
        assert_eq!(tc.terminal_id.as_deref(), Some("term-1"));
        assert_eq!(tc.terminal_output.as_deref(), Some("done"));
    }

    #[test]
    fn repeated_terminal_updates_are_idempotent() {
        let mut app = App::test_default();
        let tool_id = "tool-1";
        app.transcript.messages.push(ChatMessage::new(
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

        let MessageBlock::ToolCall(tc) = &app.transcript.messages[0].blocks[0] else {
            panic!("expected tool call block");
        };
        assert_eq!(tc.terminal_id.as_deref(), Some("term-1"));
        assert_eq!(tc.content.len(), 1);
    }

    #[test]
    fn terminal_update_replaces_stale_terminal_id_for_same_tool_call() {
        let mut app = App::test_default();
        let tool_id = "tool-1";
        app.transcript.messages.push(ChatMessage::new(
            MessageRole::Assistant,
            vec![MessageBlock::ToolCall(Box::new(make_bash_tool_call(
                tool_id,
                model::ToolCallStatus::InProgress,
                Some("term-1"),
            )))],
            None,
        ));
        app.index_tool_call(tool_id.to_owned(), 0, 0);

        let update = model::ToolCallUpdate::new(
            tool_id,
            model::ToolCallUpdateFields::new().content(terminal_content("term-2")),
        );

        handle_tool_call_update_session(&mut app, &update);

        let MessageBlock::ToolCall(tc) = &app.transcript.messages[0].blocks[0] else {
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

    #[test]
    fn deferred_web_output_preserves_launch_and_appends_linked_result_after_turn_completion() {
        let mut app = App::test_default();
        let mut tool = make_task_tool_call("web-1", model::ToolCallStatus::InProgress);
        tool.sdk_tool_name = "WebFetch".to_owned();
        tool.kind = model::ToolKind::Fetch;
        app.transcript.messages.push(ChatMessage::new(
            MessageRole::Assistant,
            vec![MessageBlock::ToolCall(Box::new(tool))],
            None,
        ));
        app.index_tool_call("web-1".to_owned(), 0, 0);
        handle_tool_call_update_session(
            &mut app,
            &model::ToolCallUpdate::new(
                "web-1",
                model::ToolCallUpdateFields::new().status(model::ToolCallStatus::Detached),
            ),
        );
        app.finalize_turn_runtime_artifacts(model::ToolCallStatus::Completed);
        let MessageBlock::ToolCall(tool) = &app.transcript.messages[0].blocks[0] else {
            panic!("expected web call")
        };
        assert_eq!(tool.status, model::ToolCallStatus::Detached);
        handle_tool_call_update_session(
            &mut app,
            &model::ToolCallUpdate::new(
                "web-1",
                model::ToolCallUpdateFields::new()
                    .status(model::ToolCallStatus::Completed)
                    .content(vec![model::ToolCallContent::from("Actual web output")]),
            ),
        );
        let MessageBlock::ToolCall(tool) = &app.transcript.messages[0].blocks[0] else {
            panic!("expected web call")
        };
        assert_eq!(tool.status, model::ToolCallStatus::Completed);
        assert_eq!(tool.content, vec![model::ToolCallContent::from("Actual web output")]);
        assert_eq!(tool.display().status, model::ToolCallStatus::Detached);
        assert_eq!(app.transcript.messages.len(), 2);
        let MessageBlock::ToolResult { tool: result, .. } = &app.transcript.messages[1].blocks[0]
        else {
            panic!("expected linked result")
        };
        assert_eq!(result.id, "web-1");
        assert_eq!(result.status, model::ToolCallStatus::Completed);
        assert_eq!(result.content, vec![model::ToolCallContent::from("Actual web output")]);
        assert_eq!(app.tool_call_index_len(), 1);
    }

    #[test]
    fn task_metadata_update_is_applied_to_tool_call() {
        let mut app = App::test_default();
        let tool_id = "task-1";
        app.transcript.messages.push(ChatMessage::new(
            MessageRole::Assistant,
            vec![MessageBlock::ToolCall(Box::new(make_task_tool_call(
                tool_id,
                model::ToolCallStatus::InProgress,
            )))],
            None,
        ));
        app.index_tool_call(tool_id.to_owned(), 0, 0);

        let update = model::ToolCallUpdate::new(
            tool_id,
            model::ToolCallUpdateFields::new().task_metadata(
                model::TaskMetadata::new()
                    .error(Some("Task paused".to_owned()))
                    .backgrounded(Some(true)),
            ),
        );

        handle_tool_call_update_session(&mut app, &update);

        let MessageBlock::ToolCall(tc) = &app.transcript.messages[0].blocks[0] else {
            panic!("expected tool call block");
        };
        assert_eq!(
            tc.task_metadata,
            Some(
                model::TaskMetadata::new()
                    .error(Some("Task paused".to_owned()))
                    .backgrounded(Some(true)),
            )
        );
    }

    #[test]
    fn present_task_metadata_replaces_the_projection_and_absence_leaves_it_alone() {
        let mut tc = make_task_tool_call("agent", model::ToolCallStatus::InProgress);
        tc.task_metadata = Some(
            model::TaskMetadata::new()
                .error(Some("old failure".to_owned()))
                .terminal_status(Some("failed".to_owned()))
                .end_time(Some(1234))
                .subagent_retry(Some(model::SubagentRetryUpdate::Waiting {
                    agent_id: None,
                    attempt: 1,
                    max_retries: 3,
                    retry_delay_ms: 500,
                    error_status: None,
                    error_category: None,
                })),
        );
        let original = tc.task_metadata.clone();
        let mut app = App::test_default();
        app.transcript.messages.push(ChatMessage::new(
            MessageRole::Assistant,
            vec![MessageBlock::ToolCall(Box::new(tc))],
            None,
        ));
        app.index_tool_call("agent".to_owned(), 0, 0);
        handle_tool_call_update_session(
            &mut app,
            &model::ToolCallUpdate::new(
                "agent",
                model::ToolCallUpdateFields::new().status(model::ToolCallStatus::Completed),
            ),
        );
        let MessageBlock::ToolCall(tc) = &app.transcript.messages[0].blocks[0] else {
            panic!("tool call")
        };
        assert_eq!(
            tc.task_metadata, original,
            "absent metadata is not a metadata update, even with a terminal status"
        );
        let replacement = model::TaskMetadata::new().backgrounded(Some(true));
        handle_tool_call_update_session(
            &mut app,
            &model::ToolCallUpdate::new(
                "agent",
                model::ToolCallUpdateFields::new().task_metadata(replacement.clone()),
            ),
        );
        let MessageBlock::ToolCall(tc) = &app.transcript.messages[0].blocks[0] else {
            panic!("tool call")
        };
        assert_eq!(tc.task_metadata, Some(replacement));
    }

    #[test]
    fn killed_task_update_applies_terminal_status() {
        let mut app = App::test_default();
        let tool_id = "task-1";
        app.transcript.messages.push(ChatMessage::new(
            MessageRole::Assistant,
            vec![MessageBlock::ToolCall(Box::new(make_task_tool_call(
                tool_id,
                model::ToolCallStatus::InProgress,
            )))],
            None,
        ));
        app.index_tool_call(tool_id.to_owned(), 0, 0);
        crate::app::test_support::set_tool_scope(&mut app, tool_id, ToolCallScope::SubagentRoot);

        let update = model::ToolCallUpdate::new(
            tool_id,
            model::ToolCallUpdateFields::new().status(model::ToolCallStatus::Killed),
        );

        handle_tool_call_update_session(&mut app, &update);

        let MessageBlock::ToolCall(tc) = &app.transcript.messages[0].blocks[0] else {
            panic!("expected tool call block");
        };
        assert_eq!(tc.status, model::ToolCallStatus::Killed);
    }

    #[test]
    fn completed_tool_update_mutates_canonical_tool_block() {
        let mut app = App::test_default();
        let tool_id = "tool-1";
        app.transcript.messages.push(ChatMessage::new(
            MessageRole::Assistant,
            vec![MessageBlock::ToolCall(Box::new(make_bash_tool_call(
                tool_id,
                model::ToolCallStatus::InProgress,
                Some("term-1"),
            )))],
            None,
        ));
        app.bind_active_turn_assistant(0);
        app.index_tool_call(tool_id.to_owned(), 0, 0);

        let update = model::ToolCallUpdate::new(
            tool_id,
            model::ToolCallUpdateFields::new()
                .status(model::ToolCallStatus::Completed)
                .raw_output(serde_json::Value::String("done".to_owned())),
        );

        handle_tool_call_update_session(&mut app, &update);

        assert_eq!(app.active_turn_assistant_idx(), Some(0));
        let MessageBlock::ToolCall(tc) = &app.transcript.messages[0].blocks[0] else {
            panic!("expected tool call block");
        };
        assert_eq!(tc.status, model::ToolCallStatus::Completed);
        assert!(tc.terminal_output.as_deref().is_some_and(|output| output.contains("done")));
    }
}
