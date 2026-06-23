// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

//! Rendering helpers for SDK `RemoteTrigger` tool calls.

use crate::agent::model;
use crate::app::ToolCallInfo;
use crate::ui::diff::strip_outer_code_fence;
use ratatui::text::{Line, Span};

use super::errors::render_failed_tool_text_content;
use super::fields::{self, ToolField};

pub(super) fn is_remote_trigger_tool(tc: &ToolCallInfo) -> bool {
    tc.sdk_tool_name == "RemoteTrigger"
}

pub(super) fn has_structured_body(tc: &ToolCallInfo) -> bool {
    input_has_body(tc) || !tc.content.is_empty()
}

pub(super) fn render_tool_content(tc: &ToolCallInfo) -> Option<Vec<Line<'static>>> {
    if !is_remote_trigger_tool(tc) {
        return None;
    }

    let mut lines = render_input_content(tc);
    lines.extend(render_text_content(tc));
    Some(lines)
}

fn input_has_body(tc: &ToolCallInfo) -> bool {
    let input = tc.raw_input.as_ref().and_then(serde_json::Value::as_object);
    input.is_some_and(|input| {
        json_string(input, "action").is_some()
            || json_string(input, "trigger_id").is_some()
            || input.contains_key("body")
    })
}

fn render_input_content(tc: &ToolCallInfo) -> Vec<Line<'static>> {
    let input = tc.raw_input.as_ref().and_then(serde_json::Value::as_object);
    let mut remote_fields = Vec::new();

    if let Some(input) = input {
        if let Some(action) = json_string(input, "action") {
            remote_fields.push(ToolField::new("Action", action));
        }
        if let Some(trigger_id) = json_string(input, "trigger_id") {
            remote_fields.push(ToolField::new("Trigger ID", trigger_id));
        }
        if let Some(body) = input.get("body").and_then(compact_json) {
            remote_fields.push(ToolField::new("Body", body));
        }
    }

    fields::render_fields(remote_fields)
}

fn render_text_content(tc: &ToolCallInfo) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for content in &tc.content {
        let model::ToolCallContent::Content(content) = content else {
            continue;
        };
        let model::ContentBlock::Text(text) = &content.content else {
            continue;
        };
        render_text_block(tc, &text.text, &mut lines);
    }
    lines
}

fn render_text_block(tc: &ToolCallInfo, text: &str, lines: &mut Vec<Line<'static>>) {
    let stripped = strip_outer_code_fence(text);
    if let Some(failed_lines) = render_failed_tool_text_content(tc.status, &stripped) {
        lines.extend(failed_lines);
        return;
    }
    lines.extend(
        stripped
            .lines()
            .map(str::trim_end)
            .filter(|line| !line.trim().is_empty())
            .map(render_text_line),
    );
}

fn render_text_line(line: &str) -> Line<'static> {
    if let Some((label, value)) = line.split_once(':')
        && let Some(label) = field_label(label.trim())
    {
        return fields::render_field(label, value.trim_start().to_owned());
    }
    Line::from(Span::raw(line.to_owned()))
}

fn field_label(label: &str) -> Option<&'static str> {
    match label {
        "status" | "Status" => Some("Status"),
        "summary" | "Summary" => Some("Summary"),
        "json" | "Response" => Some("Response"),
        _ => None,
    }
}

fn compact_json(value: &serde_json::Value) -> Option<String> {
    serde_json::to_string(value).ok()
}

fn json_string<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Option<&'a str> {
    object.get(key).and_then(serde_json::Value::as_str).filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
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

        let lines = render_tool_content(&tc).expect("remote trigger content");

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

        let lines = render_tool_content(&tc).expect("remote trigger content");

        assert_eq!(rendered_line_texts(&lines), vec!["Status: 200", "Summary: Trigger completed"]);
    }

    #[test]
    fn fallback_output_renders_response() {
        let tc = remote_trigger_tool_call(
            json!({}),
            Some("Response: {\"ok\":true}"),
            model::ToolCallStatus::Completed,
        );

        let lines = render_tool_content(&tc).expect("remote trigger content");

        assert_eq!(rendered_line_texts(&lines), vec!["Response: {\"ok\":true}"]);
    }

    #[test]
    fn failed_structured_status_still_renders_typed_rows() {
        let tc = remote_trigger_tool_call(
            json!({}),
            Some("Status: 500\nResponse: {\"error\":\"boom\"}"),
            model::ToolCallStatus::Failed,
        );

        let lines = render_tool_content(&tc).expect("remote trigger content");

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

        let lines = render_tool_content(&tc).expect("remote trigger content");
        let rendered = rendered_line_texts(&lines);

        assert_eq!(rendered, vec!["Action: run", "Remote trigger unavailable"]);
        assert_eq!(lines[1].spans[0].style.fg, Some(theme::STATUS_ERROR));
    }
}
