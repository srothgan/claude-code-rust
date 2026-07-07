// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

//! Rendering helpers for SDK `RemoteTrigger` tool calls.

use crate::app::ToolCallInfo;
use ratatui::text::Line;

use super::fields::{self, ToolField};
use super::typed;

pub(super) fn is_remote_trigger_tool(tc: &ToolCallInfo) -> bool {
    tc.sdk_tool_name == "RemoteTrigger"
}

pub(super) fn has_structured_body(tc: &ToolCallInfo) -> bool {
    input_has_body(tc) || !tc.content.is_empty()
}

pub(super) fn render_tool_content(tc: &ToolCallInfo) -> Vec<Line<'static>> {
    let mut lines = render_input_content(tc);
    lines.extend(typed::render_stripped_text_blocks(tc, render_text_line));
    lines
}

fn input_has_body(tc: &ToolCallInfo) -> bool {
    let input = typed::input_object(tc);
    input.is_some_and(|input| {
        typed::json_string(input, "action").is_some()
            || typed::json_string(input, "trigger_id").is_some()
            || input.contains_key("body")
    })
}

fn render_input_content(tc: &ToolCallInfo) -> Vec<Line<'static>> {
    let input = typed::input_object(tc);
    let mut remote_fields = Vec::new();

    if let Some(input) = input {
        if let Some(action) = typed::json_string(input, "action") {
            remote_fields.push(ToolField::new("Action", action));
        }
        if let Some(trigger_id) = typed::json_string(input, "trigger_id") {
            remote_fields.push(ToolField::new("Trigger ID", trigger_id));
        }
        if let Some(body) = input.get("body").and_then(typed::compact_json) {
            remote_fields.push(ToolField::new("Body", body));
        }
    }

    fields::render_fields(remote_fields)
}

fn render_text_line(line: &str) -> Line<'static> {
    typed::render_colon_field_line(line, field_label, |_, value| value.to_owned())
}

fn field_label(label: &str) -> Option<&'static str> {
    match label {
        "status" | "Status" => Some("Status"),
        "summary" | "Summary" => Some("Summary"),
        "json" | "Response" => Some("Response"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::model;
    use crate::app::{BlockCache, TerminalSnapshotMode};
    use crate::ui::theme;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn remote_trigger_tool_call(
        raw_input: serde_json::Value,
        content: Option<&str>,
        status: model::ToolCallStatus,
    ) -> ToolCallInfo {
        let mut tc = ToolCallInfo {
            id: "tc-remote-trigger".to_owned(),
            source_message_uuids: Vec::new(),
            title: "RemoteTrigger".to_owned(),
            sdk_tool_name: "RemoteTrigger".to_owned(),
            raw_input: Some(raw_input),
            raw_input_bytes: 0,
            locations: Vec::new(),
            output_metadata: None,
            task_metadata: None,
            status,
            content: Vec::new(),
            hidden: false,
            terminal_id: None,
            terminal_command: None,
            terminal_output: None,
            terminal_output_len: 0,
            terminal_bytes_seen: 0,
            terminal_snapshot_mode: TerminalSnapshotMode::AppendOnly,
            cache: BlockCache::default(),
            pending_permission: None,
            pending_question: None,
        };
        if let Some(content) = content {
            tc.content = vec![model::ToolCallContent::from(content)];
        }
        tc
    }

    fn rendered_line_texts(lines: &[Line<'static>]) -> Vec<String> {
        lines
            .iter()
            .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn input_body_renders_action_trigger_id_and_body() {
        let tc = remote_trigger_tool_call(
            json!({
                "action": "run",
                "trigger_id": "deploy-prod",
                "body": { "branch": "main", "retry": false }
            }),
            None,
            model::ToolCallStatus::InProgress,
        );

        let lines = render_tool_content(&tc);

        assert_eq!(
            rendered_line_texts(&lines),
            vec![
                "Action: run",
                "Trigger ID: deploy-prod",
                "Body: {\"branch\":\"main\",\"retry\":false}",
            ]
        );
        assert_eq!(lines[0].spans[0].style.fg, Some(theme::DIM));
    }

    #[test]
    fn completed_output_renders_status_and_summary() {
        let tc = remote_trigger_tool_call(
            json!({}),
            Some("Status: 200\nSummary: Trigger completed"),
            model::ToolCallStatus::Completed,
        );

        let lines = render_tool_content(&tc);

        assert_eq!(rendered_line_texts(&lines), vec!["Status: 200", "Summary: Trigger completed"]);
    }

    #[test]
    fn fallback_output_renders_response() {
        let tc = remote_trigger_tool_call(
            json!({}),
            Some("Response: {\"ok\":true}"),
            model::ToolCallStatus::Completed,
        );

        let lines = render_tool_content(&tc);

        assert_eq!(rendered_line_texts(&lines), vec!["Response: {\"ok\":true}"]);
    }

    #[test]
    fn failed_structured_status_still_renders_typed_rows() {
        let tc = remote_trigger_tool_call(
            json!({}),
            Some("Status: 500\nResponse: {\"error\":\"boom\"}"),
            model::ToolCallStatus::Failed,
        );

        let lines = render_tool_content(&tc);

        assert_eq!(
            rendered_line_texts(&lines),
            vec!["Status: 500", "Response: {\"error\":\"boom\"}"]
        );
    }

    #[test]
    fn failed_error_payload_uses_failed_text_fallback() {
        let tc = remote_trigger_tool_call(
            json!({ "action": "run" }),
            Some("<tool_use_error>Remote trigger unavailable</tool_use_error>"),
            model::ToolCallStatus::Failed,
        );

        let lines = render_tool_content(&tc);
        let rendered = rendered_line_texts(&lines);

        assert_eq!(rendered, vec!["Action: run", "Remote trigger unavailable"]);
        assert_eq!(lines[1].spans[0].style.fg, Some(theme::STATUS_ERROR));
    }
}
