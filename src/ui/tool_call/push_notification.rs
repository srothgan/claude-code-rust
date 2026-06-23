// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

//! Rendering helpers for SDK `PushNotification` tool calls.

use crate::agent::model;
use crate::app::ToolCallInfo;
use crate::ui::diff::strip_outer_code_fence;
use ratatui::text::{Line, Span};

use super::errors::render_failed_tool_text_content;
use super::fields::{self, ToolField};

pub(super) fn is_push_notification_tool(tc: &ToolCallInfo) -> bool {
    tc.sdk_tool_name == "PushNotification"
}

pub(super) fn has_structured_body(tc: &ToolCallInfo) -> bool {
    tc.raw_input.is_some() || !tc.content.is_empty()
}

pub(super) fn render_tool_content(tc: &ToolCallInfo) -> Option<Vec<Line<'static>>> {
    if !is_push_notification_tool(tc) {
        return None;
    }

    let mut lines = render_input_content(tc);
    lines.extend(render_text_content(tc));
    Some(lines)
}

fn render_input_content(tc: &ToolCallInfo) -> Vec<Line<'static>> {
    let message = tc
        .raw_input
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .and_then(|input| json_string(input, "message"));
    fields::render_fields(message.map(|message| ToolField::new("Message", message)))
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

fn field_label(label: &str) -> Option<&'static str> {
    match label {
        "message" | "Result" => Some("Result"),
        "pushSent" | "Push sent" => Some("Push sent"),
        "localSent" | "Local sent" => Some("Local sent"),
        "disabledReason" | "Disabled reason" => Some("Disabled reason"),
        "idleSec" | "Idle time" => Some("Idle time"),
        "hasFocus" | "App focused" => Some("App focused"),
        "sentAt" | "Sent at" => Some("Sent at"),
        _ => None,
    }
}

fn field_value(label: &str, value: &str) -> String {
    match label {
        "Push sent" | "Local sent" | "App focused" => {
            bool_text_label(value).unwrap_or(value).to_owned()
        }
        "Disabled reason" => disabled_reason_label(value).unwrap_or(value).to_owned(),
        "Idle time" => {
            value.parse::<i64>().map_or_else(|_| value.to_owned(), format_duration_seconds)
        }
        _ => value.to_owned(),
    }
}

fn disabled_reason_label(value: &str) -> Option<&'static str> {
    match value {
        "config_off" | "notifications disabled" => Some("notifications disabled"),
        "user_present" | "user present" => Some("user present"),
        "no_transport" | "no notification transport" => Some("no notification transport"),
        _ => None,
    }
}

fn bool_text_label(value: &str) -> Option<&'static str> {
    match value {
        "true" | "yes" => Some("yes"),
        "false" | "no" => Some("no"),
        _ => None,
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

    fn push_notification_tool_call(
        raw_input: serde_json::Value,
        content: Option<&str>,
        status: model::ToolCallStatus,
    ) -> ToolCallInfo {
        let mut tc = ToolCallInfo {
            id: "tc-push-notification".to_owned(),
            source_message_uuids: Vec::new(),
            title: "PushNotification".to_owned(),
            sdk_tool_name: "PushNotification".to_owned(),
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
    fn input_body_renders_message_field() {
        let tc = push_notification_tool_call(
            json!({ "message": "Build finished", "status": "proactive" }),
            None,
            model::ToolCallStatus::InProgress,
        );

        let lines = render_tool_content(&tc).expect("push notification content");

        assert_eq!(rendered_line_texts(&lines), vec!["Message: Build finished"]);
        assert_eq!(lines[0].spans[0].style.fg, Some(theme::DIM));
    }

    #[test]
    fn completed_body_renders_delivery_fields_in_order() {
        let tc = push_notification_tool_call(
            json!({ "message": "Build finished", "status": "proactive" }),
            Some(
                "Result: Notification queued\nPush sent: no\nLocal sent: yes\nDisabled reason: notifications disabled\nIdle time: 1m 30s\nApp focused: no\nSent at: 2026-06-05 14:00:00 local",
            ),
            model::ToolCallStatus::Completed,
        );

        let lines = render_tool_content(&tc).expect("push notification content");

        assert_eq!(
            rendered_line_texts(&lines),
            vec![
                "Message: Build finished",
                "Result: Notification queued",
                "Push sent: no",
                "Local sent: yes",
                "Disabled reason: notifications disabled",
                "Idle time: 1m 30s",
                "App focused: no",
                "Sent at: 2026-06-05 14:00:00 local",
            ]
        );
    }

    #[test]
    fn raw_output_labels_are_mapped_to_human_values() {
        let tc = push_notification_tool_call(
            json!({ "message": "Deployment changed", "status": "proactive" }),
            Some(
                "message: Delivered\npushSent: true\nlocalSent: false\ndisabledReason: no_transport\nidleSec: 90\nhasFocus: true\nsentAt: raw timestamp",
            ),
            model::ToolCallStatus::Completed,
        );

        let lines = render_tool_content(&tc).expect("push notification content");

        assert_eq!(
            rendered_line_texts(&lines),
            vec![
                "Message: Deployment changed",
                "Result: Delivered",
                "Push sent: yes",
                "Local sent: no",
                "Disabled reason: no notification transport",
                "Idle time: 1m 30s",
                "App focused: yes",
                "Sent at: raw timestamp",
            ]
        );
    }

    #[test]
    fn long_message_body_stays_bounded_after_wrapping() {
        let long_message = (0..120).map(|idx| format!("word{idx}")).collect::<Vec<_>>().join(" ");
        let tc = push_notification_tool_call(
            json!({ "message": long_message, "status": "proactive" }),
            Some("Push sent: true"),
            model::ToolCallStatus::Completed,
        );

        let body = super::super::standard::render_tool_call_body(&tc, 40);
        let rendered = rendered_line_texts(&body);

        assert_eq!(body.len(), super::super::TOOL_BODY_MAX_LINES);
        assert!(rendered.iter().any(|line| line.contains("hidden")));
    }
}
