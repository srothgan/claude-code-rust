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
        if !has_launch_receipt(tc)
            && let Some(timeout_ms) = typed::json_i64(input, "timeout_ms")
        {
            monitor_fields.push(ToolField::new("Timeout", format_duration_ms(timeout_ms)));
        }
        if let Some(command) = typed::json_string(input, "command") {
            monitor_fields.push(ToolField::new("Command", command));
        }
        if let Some(ws) = input.get("ws").and_then(serde_json::Value::as_object) {
            if let Some(url) = typed::json_string(ws, "url") {
                monitor_fields.push(ToolField::new("WebSocket", url));
            }
            if let Some(protocols) = ws.get("protocols").and_then(serde_json::Value::as_array) {
                let protocols: Vec<_> =
                    protocols.iter().filter_map(serde_json::Value::as_str).collect();
                if !protocols.is_empty() {
                    monitor_fields.push(ToolField::new("Protocols", protocols.join(", ")));
                }
            }
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

fn has_launch_receipt(tc: &ToolCallInfo) -> bool {
    typed::text_blocks(tc).any(|text| {
        let mut task = false;
        let mut persistence = false;
        for line in text.lines() {
            let Some((label, value)) = line.split_once(':') else { continue };
            match field_label(label.trim()) {
                Some("Task ID") => task = !value.trim().is_empty(),
                Some("Persistent") => persistence = typed::bool_text_label(value.trim()).is_some(),
                _ => {}
            }
        }
        task && persistence
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::model;
    use crate::ui::tool_call::test_support::tool_call;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn monitor_tool_call(
        raw_input: serde_json::Value,
        content: Option<&str>,
        status: model::ToolCallStatus,
    ) -> ToolCallInfo {
        tool_call("tc-monitor", "Monitor", "Monitor", raw_input, content, status)
    }

    #[test]
    fn command_monitor_card_keeps_the_actual_command_description_and_deadline() {
        use crate::ui::tool_call::test_support::card_body;
        let mut tc = monitor_tool_call(
            json!({"description":"Watch deploy 42","timeout_ms":30000,"command":"tail -f deploy-42.log"}),
            None,
            model::ToolCallStatus::InProgress,
        );
        let rows = card_body(&mut tc, 120);
        for field in
            ["Description: Watch deploy 42", "Timeout: 30s", "Command: tail -f deploy-42.log"]
        {
            assert!(rows.contains(&field.to_owned()), "{rows:?}");
        }
        assert!(
            !rows.iter().any(|row| row.starts_with("Persistent:") || row.starts_with("WebSocket:"))
        );
    }

    #[test]
    fn detached_monitor_cards_keep_native_task_identity_persistence_and_actual_timeout() {
        use crate::ui::tool_call::test_support::card_body;
        for (task, receipt, persistent, timeout) in [
            (
                "native-timed-13",
                "Task ID: native-timed-13\nPersistent: no\nTimeout: 30s",
                "no",
                Some("Timeout: 30s"),
            ),
            ("native-persistent-27", "Task ID: native-persistent-27\nPersistent: yes", "yes", None),
        ] {
            let mut tc = monitor_tool_call(
                json!({"description":"Queue events","command":"watch-queue","timeout_ms":90000}),
                Some(receipt),
                model::ToolCallStatus::Detached,
            );
            let rows = card_body(&mut tc, 120);
            assert!(rows.contains(&format!("Task ID: {task}")), "{rows:?}");
            assert!(!rows.contains(&format!("Task ID: {}", tc.id)));
            assert!(rows.contains(&format!("Persistent: {persistent}")));
            assert!(rows.contains(&"Command: watch-queue".to_owned()));
            let timeouts: Vec<_> =
                rows.iter().filter(|row| row.starts_with("Timeout:")).map(String::as_str).collect();
            assert_eq!(timeouts, timeout.into_iter().collect::<Vec<_>>());
        }
        for (status, output) in [
            (model::ToolCallStatus::Failed, "MONITOR-REFUSED"),
            (model::ToolCallStatus::Completed, "Unrecognized monitor receipt"),
        ] {
            let mut tc = monitor_tool_call(
                json!({"description":"Queue retry","command":"watch-queue","timeout_ms":90000}),
                Some(output),
                status,
            );
            let rows = card_body(&mut tc, 120);
            assert!(rows.contains(&"Timeout: 1m 30s".to_owned()));
            assert!(rows.contains(&output.to_owned()));
            assert!(
                !rows
                    .iter()
                    .any(|row| row.starts_with("Persistent:") || row.starts_with("Task ID:"))
            );
        }
    }

    #[test]
    fn websocket_monitor_cards_show_their_own_target_protocols_and_deadline() {
        use crate::ui::tool_call::test_support::card_body;
        for (target, protocols, other) in [
            (
                "wss://events.example.test/deploy-42",
                Some(vec!["queue.v2", "json.events"]),
                "wss://events.example.test/deploy-99",
            ),
            ("wss://events.example.test/deploy-99", None, "wss://events.example.test/deploy-42"),
        ] {
            let mut ws = json!({"url":target});
            if let Some(protocols) = &protocols {
                ws["protocols"] = json!(protocols);
            }
            let mut tc = monitor_tool_call(
                json!({"description":"Watch queue","timeout_ms":45000,"ws":ws}),
                None,
                model::ToolCallStatus::InProgress,
            );
            let rows = card_body(&mut tc, 120);
            assert!(rows.contains(&format!("WebSocket: {target}")), "{rows:?}");
            assert!(rows.contains(&"Timeout: 45s".to_owned()));
            assert!(rows.contains(&"Description: Watch queue".to_owned()));
            if protocols.is_some() {
                assert!(rows.contains(&"Protocols: queue.v2, json.events".to_owned()));
            } else {
                assert!(!rows.iter().any(|row| row.starts_with("Protocols:")));
            }
            assert!(!rows.iter().any(|row| row.contains(other) || row.starts_with("Command:")));
        }
    }
}
