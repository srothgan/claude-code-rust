// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

//! Rendering helpers for SDK `ScheduleWakeup` tool calls.

use crate::agent::model;
use crate::app::ToolCallInfo;
use crate::ui::diff::strip_outer_code_fence;
use ratatui::text::{Line, Span};

use super::errors::render_failed_tool_text_content;
use super::fields::{self, ToolField};

pub(super) fn is_schedule_wakeup_tool(tc: &ToolCallInfo) -> bool {
    tc.sdk_tool_name == "ScheduleWakeup"
}

pub(super) fn has_structured_body(tc: &ToolCallInfo) -> bool {
    tc.raw_input.is_some() || !tc.content.is_empty()
}

pub(super) fn render_tool_content(tc: &ToolCallInfo) -> Option<Vec<Line<'static>>> {
    if !is_schedule_wakeup_tool(tc) {
        return None;
    }

    let include_requested_delay = !has_completed_output(tc);
    let mut lines = render_input_content(tc, include_requested_delay);
    lines.extend(render_text_content(tc));
    Some(lines)
}

fn render_input_content(tc: &ToolCallInfo, include_requested_delay: bool) -> Vec<Line<'static>> {
    let input = tc.raw_input.as_ref().and_then(serde_json::Value::as_object);
    let mut wakeup_fields = Vec::new();

    if let Some(input) = input {
        if include_requested_delay && let Some(delay_seconds) = json_i64(input, "delaySeconds") {
            wakeup_fields
                .push(ToolField::new("Requested delay", format_duration_seconds(delay_seconds)));
        }
        if let Some(reason) = json_string(input, "reason") {
            wakeup_fields.push(ToolField::new("Reason", reason));
        }
        if let Some(prompt) = json_string(input, "prompt") {
            wakeup_fields.push(ToolField::new("Prompt", prompt));
        }
    }

    fields::render_fields(wakeup_fields)
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
        return fields::render_field(label, field_value(label, value.trim_start()));
    }
    Line::from(Span::raw(line.to_owned()))
}

fn has_completed_output(tc: &ToolCallInfo) -> bool {
    matches!(tc.status, model::ToolCallStatus::Completed)
        && tc.content.iter().any(|content| {
            let model::ToolCallContent::Content(content) = content else {
                return false;
            };
            let model::ContentBlock::Text(text) = &content.content else {
                return false;
            };
            let stripped = strip_outer_code_fence(&text.text);
            stripped.lines().any(is_output_field_line)
        })
}

fn is_output_field_line(line: &str) -> bool {
    let Some((label, _)) = line.split_once(':') else {
        return false;
    };
    matches!(
        label.trim(),
        "Scheduled for" | "Actual delay" | "Clamped" | "clampedDelaySeconds" | "wasClamped"
    )
}

fn field_label(label: &str) -> Option<&'static str> {
    match label {
        "Scheduled for" => Some("Scheduled for"),
        "Actual delay" | "clampedDelaySeconds" => Some("Actual delay"),
        "Clamped" | "wasClamped" => Some("Clamped"),
        "Reason" => Some("Reason"),
        "Prompt" => Some("Prompt"),
        _ => None,
    }
}

fn field_value(label: &str, value: &str) -> String {
    match label {
        "Actual delay" => {
            value.parse::<i64>().map_or_else(|_| value.to_owned(), format_duration_seconds)
        }
        "Clamped" => bool_text_label(value).unwrap_or(value).to_owned(),
        _ => value.to_owned(),
    }
}

fn format_duration_seconds(seconds: i64) -> String {
    let seconds = u64::try_from(seconds.max(0)).unwrap_or_default();
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let remaining_seconds = seconds % 60;
    let mut parts = Vec::new();

    if hours > 0 {
        parts.push(format!("{hours}h"));
    }
    if minutes > 0 {
        parts.push(format!("{minutes}m"));
    }
    if remaining_seconds > 0 || parts.is_empty() {
        parts.push(format!("{remaining_seconds}s"));
    }

    parts.join(" ")
}

fn bool_text_label(value: &str) -> Option<&'static str> {
    match value {
        "true" | "yes" => Some("yes"),
        "false" | "no" => Some("no"),
        _ => None,
    }
}

fn json_string<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Option<&'a str> {
    object.get(key).and_then(serde_json::Value::as_str).filter(|value| !value.is_empty())
}

fn json_i64(object: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<i64> {
    object.get(key).and_then(serde_json::Value::as_i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{BlockCache, TerminalSnapshotMode};
    use crate::ui::theme;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn schedule_wakeup_tool_call(
        raw_input: serde_json::Value,
        content: Option<&str>,
        status: model::ToolCallStatus,
    ) -> ToolCallInfo {
        let mut tc = ToolCallInfo {
            id: "tc-schedule-wakeup".to_owned(),
            source_message_uuids: Vec::new(),
            title: "ScheduleWakeup".to_owned(),
            sdk_tool_name: "ScheduleWakeup".to_owned(),
            raw_input: Some(raw_input),
            raw_input_bytes: 0,
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
    fn input_body_renders_requested_delay_reason_and_prompt() {
        let tc = schedule_wakeup_tool_call(
            json!({
                "delaySeconds": 90,
                "reason": "Poll again after the remote job warms up",
                "prompt": "/loop watch deployment"
            }),
            None,
            model::ToolCallStatus::InProgress,
        );

        let lines = render_tool_content(&tc).expect("schedule wakeup content");

        assert_eq!(
            rendered_line_texts(&lines),
            vec![
                "Requested delay: 1m 30s",
                "Reason: Poll again after the remote job warms up",
                "Prompt: /loop watch deployment",
            ]
        );
        assert_eq!(lines[0].spans[0].style.fg, Some(theme::DIM));
    }

    #[test]
    fn completed_body_renders_input_context_and_output_fields() {
        let tc = schedule_wakeup_tool_call(
            json!({
                "delaySeconds": 30,
                "reason": "Retry after runtime clamp",
                "prompt": "/loop keep checking"
            }),
            Some("Scheduled for: 2026-06-05 14:00:00 local\nActual delay: 1m\nClamped: yes"),
            model::ToolCallStatus::Completed,
        );

        let lines = render_tool_content(&tc).expect("schedule wakeup content");

        assert_eq!(
            rendered_line_texts(&lines),
            vec![
                "Reason: Retry after runtime clamp",
                "Prompt: /loop keep checking",
                "Scheduled for: 2026-06-05 14:00:00 local",
                "Actual delay: 1m",
                "Clamped: yes",
            ]
        );
        assert!(!rendered_line_texts(&lines).iter().any(|line| line.contains("Requested delay")));
    }

    #[test]
    fn failed_body_keeps_requested_delay_before_error_text() {
        let tc = schedule_wakeup_tool_call(
            json!({
                "delaySeconds": 3600,
                "reason": "Wake up later",
                "prompt": "/loop continue"
            }),
            Some("Could not schedule wakeup"),
            model::ToolCallStatus::Failed,
        );

        let rendered = rendered_line_texts(&render_tool_content(&tc).expect("schedule wakeup"));

        assert!(rendered.iter().any(|line| line.contains("Requested delay: 1h")));
        assert!(rendered.iter().any(|line| line.contains("Could not schedule wakeup")));
    }
}
