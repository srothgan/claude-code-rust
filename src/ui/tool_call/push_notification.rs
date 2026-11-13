// SPDX-License-Identifier: Apache-2.0

//! Rendering helpers for SDK `PushNotification` tool calls.

use crate::app::ToolCallInfo;
use ratatui::text::Line;

use super::fields::{self, ToolField};
use super::typed;

pub(super) fn is_push_notification_tool(tc: &ToolCallInfo) -> bool {
    tc.sdk_tool_name == "PushNotification"
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
    let message = typed::input_object(tc).and_then(|input| typed::json_string(input, "message"));
    fields::render_fields(message.map(|message| ToolField::new("Message", message)))
}

fn render_text_line(line: &str) -> Line<'static> {
    typed::render_colon_field_line(line, field_label, field_value)
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
            typed::bool_text_label(value).unwrap_or(value).to_owned()
        }
        "Disabled reason" => disabled_reason_label(value).unwrap_or(value).to_owned(),
        "Idle time" => {
            value.parse::<i64>().map_or_else(|_| value.to_owned(), typed::format_duration_seconds)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::model;
    use crate::ui::tool_call::test_support::{card_body, tool_call};
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn push_notification_tool_call(
        raw_input: serde_json::Value,
        content: Option<&str>,
        status: model::ToolCallStatus,
    ) -> ToolCallInfo {
        tool_call(
            "tc-push-notification",
            "PushNotification",
            "PushNotification",
            raw_input,
            content,
            status,
        )
    }

    #[test]
    fn pending_message_and_completed_receipts_keep_requested_and_delivered_values_distinct() {
        let input = json!({"message":"Build finished","status":"proactive"});
        let mut pending =
            push_notification_tool_call(input.clone(), None, model::ToolCallStatus::InProgress);
        assert_eq!(card_body(&mut pending, 100), vec!["Message: Build finished"]);

        for (receipt, expected, absent) in [
            (
                "Result: Notification queued\nPush sent: no\nLocal sent: yes\nDisabled reason: notifications disabled\nIdle time: 1m 30s\nApp focused: no\nSent at: 2026-06-05 14:00:00 local",
                vec![
                    "Message: Build finished",
                    "Result: Notification queued",
                    "Push sent: no",
                    "Local sent: yes",
                    "Disabled reason: notifications disabled",
                    "Idle time: 1m 30s",
                    "App focused: no",
                    "Sent at: 2026-06-05 14:00:00 local",
                ],
                vec!["Push sent: yes", "Local sent: no", "Result: Build finished"],
            ),
            (
                "Result: Delivery deferred\nPush sent: no\nLocal sent: no\nDisabled reason: no notification transport\nApp focused: yes",
                vec![
                    "Message: Build finished",
                    "Result: Delivery deferred",
                    "Push sent: no",
                    "Local sent: no",
                    "Disabled reason: no notification transport",
                    "App focused: yes",
                ],
                vec!["Push sent: yes", "Local sent: yes", "Sent at:"],
            ),
            (
                "Result: Remote delivery accepted\nPush sent: yes\nLocal sent: no",
                vec![
                    "Message: Build finished",
                    "Result: Remote delivery accepted",
                    "Push sent: yes",
                    "Local sent: no",
                ],
                vec!["Push sent: no", "Local sent: yes"],
            ),
            (
                "Result: Delivery state unavailable",
                vec!["Message: Build finished", "Result: Delivery state unavailable"],
                vec!["Push sent:", "Local sent:", "Sent at:", "App focused:"],
            ),
        ] {
            let mut tc = push_notification_tool_call(
                input.clone(),
                Some(receipt),
                model::ToolCallStatus::Completed,
            );
            let body = card_body(&mut tc, 100);
            for value in expected {
                assert!(body.iter().any(|row| row == value), "{body:?}");
            }
            for value in absent {
                assert!(!body.iter().any(|row| row.contains(value)), "{body:?}");
            }
        }
    }

    #[test]
    fn long_message_keeps_its_tail_and_actual_delivery_receipt_visible() {
        let message = format!("REQUEST_START {} REQUEST_TAIL_42", "delivery detail ".repeat(100));
        let mut tc = push_notification_tool_call(
            json!({"message":message,"status":"proactive"}),
            Some("Result: Receipt for target 42\nPush sent: no\nLocal sent: yes"),
            model::ToolCallStatus::Completed,
        );
        let original_input = tc.raw_input.clone();
        let original_result = tc.content.clone();
        let body = card_body(&mut tc, 60);
        assert!(body.iter().any(|row| row.contains("hidden")), "{body:?}");
        assert!(body.iter().any(|row| row.contains("REQUEST_TAIL_42")), "{body:?}");
        for receipt in ["Result: Receipt for target 42", "Push sent: no", "Local sent: yes"] {
            assert!(body.iter().any(|row| row == receipt), "{body:?}");
        }
        assert!(!body.iter().any(|row| row.contains("REQUEST_START")
            || row == "Push sent: yes"
            || row == "Local sent: no"));
        assert_eq!(tc.raw_input, original_input);
        assert_eq!(tc.content, original_result);
    }
}
