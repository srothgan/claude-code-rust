// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

//! Rendering helpers for SDK `Monitor` tool calls.

use crate::app::ToolCallInfo;
use ratatui::text::Line;

use super::fields::{self, ToolField};
use super::typed;

pub(super) fn is_monitor_tool(tc: &ToolCallInfo) -> bool {
    tc.sdk_tool_name == "Monitor"
}

pub(super) fn has_structured_body(tc: &ToolCallInfo) -> bool {
    tc.raw_input.is_some() || !tc.content.is_empty()
}

pub(super) fn render_tool_content(tc: &ToolCallInfo) -> Vec<Line<'static>> {
    let mut lines = render_input_content(tc);
    lines.extend(typed::render_stripped_text_blocks(tc, render_text_line));
    lines
}

fn render_input_content(tc: &ToolCallInfo) -> Vec<Line<'static>> {
    let input = typed::input_object(tc);
    let mut monitor_fields = Vec::new();

    if let Some(input) = input {
        if let Some(description) = typed::json_string(input, "description") {
            monitor_fields.push(ToolField::new("Description", description));
        }
        let persistent = typed::json_bool(input, "persistent");
        if let Some(persistent) = persistent {
            monitor_fields.push(ToolField::new("Persistent", bool_label(persistent)));
        }
        if persistent != Some(true)
            && let Some(timeout_ms) = typed::json_i64(input, "timeout_ms")
        {
            monitor_fields.push(ToolField::new("Timeout", format_duration_ms(timeout_ms)));
        }
        if let Some(command) = typed::json_string(input, "command") {
            monitor_fields.push(ToolField::new("Command", command));
        }
    }

    fields::render_fields(monitor_fields)
}

fn render_text_line(line: &str) -> Line<'static> {
    typed::render_colon_field_line(line, field_label, field_value)
}

fn field_label(label: &str) -> Option<&'static str> {
    match label {
        "taskId" | "Task ID" => Some("Task ID"),
        "timeoutMs" | "Timeout" => Some("Timeout"),
        "persistent" | "Persistent" => Some("Persistent"),
        _ => None,
    }
}

fn field_value(label: &str, value: &str) -> String {
    match label {
        "Timeout" => value.parse::<i64>().map_or_else(|_| value.to_owned(), format_duration_ms),
        "Persistent" => typed::bool_text_label(value).unwrap_or(value).to_owned(),
        _ => value.to_owned(),
    }
}

fn format_duration_ms(milliseconds: i64) -> String {
    let milliseconds = u64::try_from(milliseconds.max(0)).unwrap_or_default();
    if milliseconds < 1000 {
        return format!("{milliseconds}ms");
    }
    typed::format_duration_seconds(i64::try_from(milliseconds / 1000).unwrap_or_default())
}

fn bool_label(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::model;
    use crate::app::{BlockCache, TerminalSnapshotMode};
    use crate::ui::theme;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn monitor_tool_call(
        raw_input: serde_json::Value,
        content: Option<&str>,
        status: model::ToolCallStatus,
    ) -> ToolCallInfo {
        let mut tc = ToolCallInfo {
            id: "tc-monitor".to_owned(),
            source_message_uuids: Vec::new(),
            title: "Monitor".to_owned(),
            sdk_tool_name: "Monitor".to_owned(),
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
    fn input_body_renders_monitor_fields() {
        let tc = monitor_tool_call(
            json!({
                "description": "watch deploy logs",
                "timeout_ms": 30000,
                "persistent": false,
                "command": "tail -f deploy.log"
            }),
            None,
            model::ToolCallStatus::InProgress,
        );

        let lines = render_tool_content(&tc);

        assert_eq!(
            rendered_line_texts(&lines),
            vec![
                "Description: watch deploy logs",
                "Persistent: no",
                "Timeout: 30s",
                "Command: tail -f deploy.log",
            ]
        );
        assert_eq!(lines[0].spans[0].style.fg, Some(theme::DIM));
    }

    #[test]
    fn persistent_input_omits_timeout() {
        let tc = monitor_tool_call(
            json!({
                "description": "watch queue",
                "timeout_ms": 30000,
                "persistent": true,
                "command": "watch-queue"
            }),
            None,
            model::ToolCallStatus::InProgress,
        );

        let lines = render_tool_content(&tc);

        assert_eq!(
            rendered_line_texts(&lines),
            vec!["Description: watch queue", "Persistent: yes", "Command: watch-queue",]
        );
    }

    #[test]
    fn output_body_renders_task_id_persistent_and_timeout() {
        let tc = monitor_tool_call(
            json!({}),
            Some("Task ID: mon-1\nPersistent: no\nTimeout: 30s"),
            model::ToolCallStatus::InProgress,
        );

        let lines = render_tool_content(&tc);

        assert_eq!(
            rendered_line_texts(&lines),
            vec!["Task ID: mon-1", "Persistent: no", "Timeout: 30s"]
        );
    }

    #[test]
    fn raw_output_labels_are_normalized_defensively() {
        let tc = monitor_tool_call(
            json!({}),
            Some("taskId: mon-1\npersistent: true\ntimeoutMs: 90000"),
            model::ToolCallStatus::InProgress,
        );

        let lines = render_tool_content(&tc);

        assert_eq!(
            rendered_line_texts(&lines),
            vec!["Task ID: mon-1", "Persistent: yes", "Timeout: 1m 30s"]
        );
    }
}
