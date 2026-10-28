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
    use crate::app::{App, ChatMessage, MessageBlock, MessageRole};

    fn terminal_content(terminal_id: &str) -> Vec<model::ToolCallContent> {
        vec![model::ToolCallContent::Terminal(model::TerminalToolCallContent::new(terminal_id))]
    }

    fn make_task_tool_call(id: &str, status: model::ToolCallStatus) -> ToolCallInfo {
        ToolCallInfo {
            title: format!("task {id}"),
            ..crate::app::test_support::named_tool_call_info(id, "Agent", status)
        }
    }

    fn event_tool(app: &mut App, id: &str, name: &str, title: &str, body: &str) {
        crate::app::test_support::deliver_update(app,model::SessionUpdate::ToolCall(model::ToolCall::new(id,title)
            .kind(if name=="Bash" {model::ToolKind::Execute} else {model::ToolKind::Fetch})
            .status(model::ToolCallStatus::InProgress).content(vec![body.into()])
            .raw_input(serde_json::json!({"command":title,"url":"https://example.test/evidence","prompt":"Fetch evidence"}))
            .meta(serde_json::json!({"claudeCode":{"toolName":name}}))));
    }
    fn event_patch(app: &mut App, id: &str, fields: model::ToolCallUpdateFields) {
        crate::app::test_support::deliver_update(
            app,
            model::SessionUpdate::ToolCallUpdate(model::ToolCallUpdate::new(id, fields)),
        );
    }
    fn transcript(
        app: &mut App,
        excluded: &std::collections::BTreeSet<crate::app::HistoryOutputId>,
    ) -> crate::ui::inline_chat_rows::SerializedLiveRows {
        crate::ui::inline_chat_rows::serialize_live_rows_with_boundaries_excluding(
            app, 100, excluded, None,
        )
    }
    fn rendered_card(app: &mut App, id: &str) -> Vec<String> {
        let rendered = transcript(app, &std::collections::BTreeSet::new());
        let target = crate::app::HistoryOutputId::ToolCall(id.into());
        rendered
            .segments()
            .iter()
            .filter(|segment| segment.ids.contains(&target))
            .flat_map(|segment| rendered.segment_rows(segment).unwrap())
            .map(ToString::to_string)
            .collect()
    }
    fn event_text(app: &mut App, value: &str) {
        crate::app::test_support::deliver_update(
            app,
            model::SessionUpdate::AgentMessageChunk(model::ContentChunk::new(
                model::ContentBlock::Text(model::TextContent::new(value)),
            )),
        );
    }
    fn event_complete(app: &mut App) {
        crate::app::handle_client_event(
            app,
            crate::agent::events::ClientEvent::TurnComplete {
                session_id: "session-1".into(),
                queued_turn_count: None,
                terminal_reason: None,
            },
        );
    }

    #[test]
    fn execute_patches_preserve_canonical_output_terminal_association_and_neighbor_cards() {
        let (mut app, _commands) = crate::app::test_support::app_with_connection();
        event_text(&mut app, "Earlier reply sentinel");
        event_complete(&mut app);
        let history_id = app.transcript.messages[0].id;
        for (id, title, terminal, output) in [
            ("alpha", "echo alpha", "term-alpha", "Alpha original"),
            ("beta", "echo beta", "term-beta", "Beta unchanged"),
        ] {
            event_tool(&mut app, id, "Bash", title, "Initial command body");
            event_patch(
                &mut app,
                id,
                model::ToolCallUpdateFields::new()
                    .content(terminal_content(terminal))
                    .raw_output(serde_json::json!(output)),
            );
        }
        let neighbor = rendered_card(&mut app, "beta");
        let _ = rendered_card(&mut app, "alpha");
        let output = "alpha first\nindependent \u{754c} result\nalpha last";
        event_patch(
            &mut app,
            "alpha",
            model::ToolCallUpdateFields::new()
                .status(model::ToolCallStatus::Completed)
                .raw_output(serde_json::json!(output)),
        );
        let tc = app.tool_call("alpha").unwrap();
        assert_eq!(tc.status, model::ToolCallStatus::Completed);
        assert_eq!(tc.terminal_id.as_deref(), Some("term-alpha"));
        assert_eq!(tc.terminal_output.as_deref(), Some(output));
        let completed = rendered_card(&mut app, "alpha");
        for line in output.lines() {
            assert_eq!(completed.iter().filter(|row| row.contains(line)).count(), 1);
        }
        event_patch(
            &mut app,
            "alpha",
            model::ToolCallUpdateFields::new().status(model::ToolCallStatus::Completed),
        );
        assert_eq!(rendered_card(&mut app, "alpha"), completed);
        for _ in 0..2 {
            event_patch(
                &mut app,
                "alpha",
                model::ToolCallUpdateFields::new().content(terminal_content("term-replacement")),
            );
            assert_eq!(
                app.tool_call("alpha").unwrap().terminal_id.as_deref(),
                Some("term-replacement")
            );
            assert_eq!(app.tool_call("alpha").unwrap().terminal_output.as_deref(), Some(output));
            assert_eq!(rendered_card(&mut app, "alpha"), completed);
            assert_eq!(rendered_card(&mut app, "beta"), neighbor);
        }
        assert_eq!(app.transcript.messages[0].id, history_id);
        let MessageBlock::Text(old) = &app.transcript.messages[0].blocks[0] else {
            panic!("prior reply")
        };
        assert_eq!(old.text, "Earlier reply sentinel");
    }

    #[test]
    fn completion_diagnostics_are_emitted_once_for_the_correct_tool_while_late_content_still_arrives()
     {
        let (mut app, _commands) = crate::app::test_support::app_with_connection();
        let records = crate::logging::test_capture::capture(|| {
            event_tool(&mut app, "logged", "Bash", "echo logged", "Original body");
            event_tool(&mut app, "neighbor", "Bash", "echo neighbor", "Neighbor body");
            event_patch(
                &mut app,
                "logged",
                model::ToolCallUpdateFields::new()
                    .status(model::ToolCallStatus::Completed)
                    .raw_output(serde_json::json!("First output")),
            );
            let _ = rendered_card(&mut app, "logged");
            event_patch(
                &mut app,
                "logged",
                model::ToolCallUpdateFields::new()
                    .status(model::ToolCallStatus::Completed)
                    .raw_output(serde_json::json!("First output")),
            );
            event_patch(
                &mut app,
                "logged",
                model::ToolCallUpdateFields::new()
                    .status(model::ToolCallStatus::Completed)
                    .raw_output(serde_json::json!("Late complete output")),
            );
        });
        let completions: Vec<_> = records
            .iter()
            .filter(|r| {
                r["target"] == crate::logging::targets::APP_TOOL
                    && r["event_name"] == "tool_call_completed"
            })
            .collect();
        assert_eq!(
            completions.len(),
            1,
            "one completion event, independent of command diagnostics: {records:?}"
        );
        let record = completions[0];
        assert_eq!(record["session_id"], "session-1");
        assert_eq!(record["tool_call_id"], "logged");
        assert_eq!(record["outcome"], "success");
        assert!(records.iter().any(|r| r["target"] == crate::logging::targets::APP_COMMAND
            && r["event_name"] == "command_completed"));
        assert_eq!(
            app.tool_call("logged").unwrap().terminal_output.as_deref(),
            Some("Late complete output")
        );
        assert!(rendered_card(&mut app, "logged").join("\n").contains("Late complete output"));
        assert_eq!(app.tool_call("neighbor").unwrap().status, model::ToolCallStatus::InProgress);
    }

    #[test]
    fn detached_web_results_preserve_committed_launches_and_join_the_reply_active_at_arrival() {
        for active_next_reply in [false, true] {
            let (mut app, _commands) = crate::app::test_support::app_with_connection();
            event_text(&mut app, "Launch reply text");
            event_tool(
                &mut app,
                "web",
                "WebFetch",
                "Fetch launch sentinel",
                "Launch payload sentinel",
            );
            event_patch(
                &mut app,
                "web",
                model::ToolCallUpdateFields::new().status(model::ToolCallStatus::Detached),
            );
            let launch = rendered_card(&mut app, "web");
            let canonical = app.lookup_tool_call("web").unwrap();
            let launch_id = app.transcript.messages[canonical.0].id;
            event_complete(&mut app);
            assert_eq!(rendered_card(&mut app, "web"), launch);
            let history = transcript(&mut app, &std::collections::BTreeSet::new());
            let committed = history.segments().iter().flat_map(|s| s.ids.iter().cloned()).collect();
            if active_next_reply {
                crate::app::test_support::deliver_update(
                    &mut app,
                    model::SessionUpdate::AgentResponseStarted,
                );
                event_text(&mut app, "New reply before late result");
            }
            let next_owner = app.active_turn_assistant_idx();
            for _ in 0..2 {
                event_patch(
                    &mut app,
                    "web",
                    model::ToolCallUpdateFields::new()
                        .status(model::ToolCallStatus::Completed)
                        .content(vec!["Independent final web result".into()]),
                );
            }
            assert_eq!(app.lookup_tool_call("web"), Some(canonical));
            assert_eq!(app.transcript.messages[canonical.0].id, launch_id);
            assert_eq!(rendered_card(&mut app, "web"), launch);
            let results: Vec<_> = app
                .transcript
                .messages
                .iter()
                .enumerate()
                .flat_map(|(mi, msg)| {
                    msg.blocks.iter().enumerate().filter_map(move |(bi, block)| match block {
                        MessageBlock::ToolResult { tool, .. } if tool.id == "web" => {
                            Some((mi, bi, tool.as_ref()))
                        }
                        _ => None,
                    })
                })
                .collect();
            let [(owner, block, result)] = results.as_slice() else {
                panic!("exactly one linked web result")
            };
            let (owner, block) = (*owner, *block);
            assert_eq!(
                result.content,
                vec![model::ToolCallContent::from("Independent final web result")]
            );
            assert_eq!(result.status, model::ToolCallStatus::Completed);
            assert_ne!(owner, canonical.0);
            if active_next_reply {
                assert_eq!(Some(owner), next_owner);
                event_text(&mut app, "New reply after late result");
                assert_eq!(app.active_turn_assistant_idx(), Some(owner));
            }
            let remaining = transcript(&mut app, &committed);
            assert!(remaining.segments().iter().all(|s| s.msg_idx != canonical.0));
            let result_segment = remaining
                .segments()
                .iter()
                .find(|s| s.msg_idx == owner && s.block_idx == Some(block))
                .expect("result survives excluding launch history");
            assert!(
                remaining
                    .segment_rows(result_segment)
                    .unwrap()
                    .iter()
                    .any(|line| line.to_string().contains("Independent final web result"))
            );
            if active_next_reply {
                let rendered =
                    remaining.rows().iter().map(ToString::to_string).collect::<Vec<_>>().join("\n");
                assert!(
                    rendered.find("New reply before late result").unwrap()
                        < rendered.find("Independent final web result").unwrap()
                );
                assert!(
                    rendered.find("Independent final web result").unwrap()
                        < rendered.find("New reply after late result").unwrap()
                );
            }
        }
    }

    fn assert_first_task_projection() {
        let (mut app, _commands) = crate::app::test_support::app_with_connection();
        crate::app::test_support::deliver_update(
            &mut app,
            model::SessionUpdate::ToolCall(
                model::ToolCall::new("first-agent", "Agent launch")
                    .meta(serde_json::json!({"claudeCode":{"toolName":"Agent"}})),
            ),
        );
        let projection =
            model::TaskMetadata::new().error(Some("Task paused".into())).backgrounded(Some(true));
        event_patch(
            &mut app,
            "first-agent",
            model::ToolCallUpdateFields::new().task_metadata(projection.clone()),
        );
        assert_eq!(app.tool_call("first-agent").unwrap().task_metadata, Some(projection));
    }

    #[test]
    fn present_task_metadata_replaces_the_projection_and_absence_leaves_it_alone() {
        assert_first_task_projection();
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
}
