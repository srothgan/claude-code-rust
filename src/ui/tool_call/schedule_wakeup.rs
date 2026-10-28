// SPDX-License-Identifier: Apache-2.0

//! Rendering helpers for SDK `ScheduleWakeup` tool calls.

use crate::agent::model;
use crate::app::ToolCallInfo;
use crate::ui::diff::strip_outer_code_fence;
use ratatui::text::Line;

use super::fields::{self, ToolField};
use super::typed;

pub(super) fn is_schedule_wakeup_tool(tc: &ToolCallInfo) -> bool {
    tc.sdk_tool_name == "ScheduleWakeup"
}

pub(super) fn has_structured_body(tc: &ToolCallInfo) -> bool {
    tc.raw_input.is_some() || !tc.content.is_empty()
}

pub(super) fn render_tool_content(tc: &ToolCallInfo) -> Vec<Line<'static>> {
    let include_requested_delay = !has_completed_output(tc);
    let mut lines = render_input_content(tc, include_requested_delay);
    lines.extend(typed::render_stripped_text_blocks(tc, render_text_line));
    lines
}

pub(super) fn render_input_content(
    tc: &ToolCallInfo,
    include_requested_delay: bool,
) -> Vec<Line<'static>> {
    let input = typed::input_object(tc);
    let mut wakeup_fields = Vec::new();

    if let Some(input) = input {
        if typed::json_bool(input, "stop") == Some(true) {
            wakeup_fields.push(ToolField::new("Stop", "yes"));
            return fields::render_fields(wakeup_fields);
        }
        if include_requested_delay
            && let Some(delay_seconds) = typed::json_i64(input, "delaySeconds")
        {
            wakeup_fields.push(ToolField::new(
                "Requested delay",
                typed::format_duration_seconds(delay_seconds),
            ));
        }
        if let Some(reason) = typed::json_string(input, "reason") {
            wakeup_fields.push(ToolField::new("Reason", reason));
        }
        if let Some(prompt) = typed::json_string(input, "prompt") {
            wakeup_fields.push(ToolField::new("Prompt", prompt));
        }
        if let Some(noop) = typed::json_bool(input, "noop") {
            wakeup_fields.push(ToolField::new("No changes", typed::bool_label(noop)));
        }
    }

    fields::render_fields(wakeup_fields)
}

fn render_text_line(line: &str) -> Line<'static> {
    typed::render_colon_field_line(line, field_label, field_value)
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
        "Stop" | "stop" => Some("Stop"),
        "No changes" | "noop" => Some("No changes"),
        _ => None,
    }
}

fn field_value(label: &str, value: &str) -> String {
    match label {
        "Actual delay" => {
            value.parse::<i64>().map_or_else(|_| value.to_owned(), typed::format_duration_seconds)
        }
        "Clamped" | "Stop" | "No changes" => {
            typed::bool_text_label(value).unwrap_or(value).to_owned()
        }
        _ => value.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::tool_call::test_support::{rendered_line_texts, tool_call};
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn schedule_wakeup_tool_call(
        raw_input: serde_json::Value,
        content: Option<&str>,
        status: model::ToolCallStatus,
    ) -> ToolCallInfo {
        tool_call(
            "tc-schedule-wakeup",
            "ScheduleWakeup",
            "ScheduleWakeup",
            raw_input,
            content,
            status,
        )
    }

    #[test]
    fn scheduling_cards_distinguish_requested_accepted_failed_and_unknown_results() {
        use crate::ui::tool_call::test_support::card_body;
        for (status, delay, output, expected_delay) in [
            (model::ToolCallStatus::InProgress, 90, None, "Requested delay: 1m 30s"),
            (
                model::ToolCallStatus::Completed,
                30,
                Some("Scheduled for: 2026-06-05 14:00:00 local\nActual delay: 1m\nClamped: yes"),
                "Actual delay: 1m",
            ),
            (model::ToolCallStatus::Failed, 3600, Some("WAKEUP-REFUSED-41"), "Requested delay: 1h"),
            (
                model::ToolCallStatus::Completed,
                90,
                Some("Unrecognized scheduler receipt"),
                "Requested delay: 1m 30s",
            ),
        ] {
            let mut tc = schedule_wakeup_tool_call(
                json!({
                    "delaySeconds":delay,"reason":"Deployment 41 status","prompt":"/loop poll deployment-41"
                }),
                output,
                status,
            );
            if delay == 30 {
                crate::ui::tool_call::test_support::set_result_fields(
                    &mut tc,
                    &[
                        ("Scheduled for", "2026-06-05 14:00:00 local"),
                        ("Actual delay", "1m"),
                        ("Clamped", "yes"),
                    ],
                );
            }
            let rows = card_body(&mut tc, 120);
            assert!(rows.contains(&expected_delay.to_owned()), "{status:?}: {rows:?}");
            assert!(rows.contains(&"Reason: Deployment 41 status".to_owned()));
            assert!(rows.contains(&"Prompt: /loop poll deployment-41".to_owned()));
            if delay == 30 {
                assert!(rows.contains(&"Scheduled for: 2026-06-05 14:00:00 local".to_owned()));
                assert!(rows.contains(&"Clamped: yes".to_owned()));
                assert!(!rows.iter().any(|row| row.contains("Requested delay")));
            } else {
                assert!(!rows.iter().any(
                    |row| row.starts_with("Scheduled for:") || row.starts_with("Actual delay:")
                ));
                if let Some(output) = output {
                    assert!(rows.contains(&output.to_owned()));
                }
            }
        }
    }

    #[test]
    fn input_body_renders_noop_changed_and_stop_states() {
        for (noop, expected) in [(true, "yes"), (false, "no")] {
            let tc = schedule_wakeup_tool_call(
                json!({
                    "delaySeconds": 60,
                    "reason": "Check again",
                    "prompt": "/loop continue",
                    "noop": noop
                }),
                None,
                model::ToolCallStatus::InProgress,
            );
            assert!(
                rendered_line_texts(&render_tool_content(&tc))
                    .contains(&format!("No changes: {expected}"))
            );
        }

        let stopped = schedule_wakeup_tool_call(
            json!({
                "stop": true,
                "delaySeconds": 60,
                "reason": "ignored",
                "prompt": "ignored",
                "noop": true
            }),
            None,
            model::ToolCallStatus::InProgress,
        );
        assert_eq!(rendered_line_texts(&render_tool_content(&stopped)), vec!["Stop: yes"]);
    }

    #[test]
    fn confirmed_wakeup_stops_display_actual_counts_without_claiming_a_new_schedule() {
        use crate::ui::tool_call::test_support::{card_body, confirmed_receipt};
        for (id, count) in [
            ("wakeup-stop-empty", Some("0")),
            ("wakeup-stop-many", Some("3")),
            ("wakeup-stop-unknown-count", None),
        ] {
            let mut tc = confirmed_receipt(id);
            let rows = card_body(&mut tc, 100);
            assert!(rows.contains(&"Stopped: yes".to_owned()), "{id}: {rows:?}");
            let counts: Vec<_> =
                rows.iter().filter(|row| row.starts_with("Cancelled wakeups:")).collect();
            assert_eq!(
                counts,
                count.map(|count| format!("Cancelled wakeups: {count}")).iter().collect::<Vec<_>>()
            );
            assert!(!rows.iter().any(|row| row.contains("Scheduled for")
                || row.contains("Actual delay")
                || row.contains("Cron")));
        }
        let mut failed = schedule_wakeup_tool_call(
            json!({"stop":true}),
            Some("STOP-REFUSED-9"),
            model::ToolCallStatus::Failed,
        );
        let rows = card_body(&mut failed, 100);
        assert!(rows.contains(&"Stop: yes".to_owned()));
        assert!(rows.contains(&"STOP-REFUSED-9".to_owned()));
        assert!(
            !rows
                .iter()
                .any(|row| row.starts_with("Stopped:") || row.starts_with("Cancelled wakeups:"))
        );
    }
}
