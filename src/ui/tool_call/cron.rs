// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

//! Rendering helpers for SDK cron schedule-control tools.

use crate::agent::model;
use crate::app::ToolCallInfo;
use crate::ui::theme;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use super::fields::{self, ToolField};
use super::typed;

const CRON_LIST_DIVIDER: &str = "__cron_list_job_divider__";

pub(super) fn is_cron_tool(tc: &ToolCallInfo) -> bool {
    matches!(tc.sdk_tool_name.as_str(), "CronCreate" | "CronDelete" | "CronList")
}

pub(super) fn has_structured_body(tc: &ToolCallInfo) -> bool {
    match tc.sdk_tool_name.as_str() {
        "CronCreate" => tc.raw_input.is_some() || !tc.content.is_empty(),
        "CronDelete" => typed::input_string(tc, "id").is_some() || !tc.content.is_empty(),
        "CronList" => !tc.content.is_empty(),
        _ => false,
    }
}

pub(super) fn render_tool_content(tc: &ToolCallInfo) -> Vec<Line<'static>> {
    let text_lines = typed::render_stripped_text_blocks(tc, render_text_line);
    let has_completed_output =
        matches!(tc.status, model::ToolCallStatus::Completed) && !text_lines.is_empty();
    let mut lines = match tc.sdk_tool_name.as_str() {
        "CronCreate" => render_create_content(tc, !has_completed_output),
        "CronDelete" if has_completed_output => Vec::new(),
        "CronDelete" => render_delete_content(tc),
        "CronList" => Vec::new(),
        _ => return Vec::new(),
    };
    lines.extend(text_lines);
    lines
}

fn render_create_content(tc: &ToolCallInfo, include_cron: bool) -> Vec<Line<'static>> {
    let input = typed::input_object(tc);
    let mut cron_fields = Vec::new();

    if let Some(input) = input {
        if include_cron && let Some(cron) = typed::json_string(input, "cron") {
            cron_fields.push(ToolField::new("Cron", cron));
        }
        if let Some(prompt) = typed::json_string(input, "prompt") {
            cron_fields.push(ToolField::new("Prompt", prompt));
        }
        let recurring = input.get("recurring").and_then(serde_json::Value::as_bool).unwrap_or(true);
        cron_fields.push(ToolField::new("Recurring", typed::bool_label(recurring)));
        if input.get("durable").and_then(serde_json::Value::as_bool) == Some(true) {
            cron_fields.push(ToolField::new("Durable", "yes"));
        }
    }

    fields::render_fields(cron_fields)
}

fn render_delete_content(tc: &ToolCallInfo) -> Vec<Line<'static>> {
    let fields =
        typed::input_string(tc, "id").map(|schedule_id| ToolField::new("Schedule ID", schedule_id));
    fields::render_fields(fields)
}

fn render_text_line(line: &str) -> Line<'static> {
    if line.trim() == CRON_LIST_DIVIDER {
        return Line::from(Span::styled("\u{2500}".repeat(16), Style::default().fg(theme::DIM)));
    }
    if let Some((label, value)) = line.split_once(':')
        && let Some(label) = field_label(label.trim())
    {
        return fields::render_field(label, field_value(label, value.trim_start()));
    }
    Line::from(Span::raw(line.to_owned()))
}

fn field_label(label: &str) -> Option<&'static str> {
    match label {
        "id" | "Schedule ID" => Some("Schedule ID"),
        "cron" | "Cron" => Some("Cron"),
        "humanSchedule" | "Schedule" => Some("Schedule"),
        "prompt" | "Prompt" => Some("Prompt"),
        "recurring" | "Recurring" => Some("Recurring"),
        "durable" | "Durable" => Some("Durable"),
        "Jobs" => Some("Jobs"),
        _ => None,
    }
}

fn field_value(label: &str, value: &str) -> String {
    if matches!(label, "Recurring" | "Durable") {
        typed::bool_text_label(value).unwrap_or(value).to_owned()
    } else {
        value.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{BlockCache, TerminalSnapshotMode};
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn cron_tool_call(
        sdk_tool_name: &str,
        raw_input: serde_json::Value,
        content: Option<&str>,
    ) -> ToolCallInfo {
        let mut tc = ToolCallInfo {
            id: "tc-cron".to_owned(),
            source_message_uuids: Vec::new(),
            title: "tc-cron".to_owned(),
            sdk_tool_name: sdk_tool_name.to_owned(),
            raw_input: Some(raw_input),
            raw_input_bytes: 0,
            locations: Vec::new(),
            output_metadata: None,
            task_metadata: None,
            status: model::ToolCallStatus::Completed,
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
    fn create_input_body_renders_labels_and_defaults() {
        let tc = cron_tool_call(
            "CronCreate",
            json!({
                "cron": "30 9 * * 1",
                "prompt": "Send weekly status",
                "durable": true
            }),
            None,
        );

        let rendered = rendered_line_texts(&super::super::standard::render_tool_call_body(&tc, 80));
        let rendered: Vec<String> =
            rendered.into_iter().map(|line| line.trim_end().to_owned()).collect();

        assert_eq!(
            rendered,
            vec![
                "  \u{2502}  Cron: 30 9 * * 1",
                "  \u{2502}  Prompt: Send weekly status",
                "  \u{2502}  Recurring: yes",
                "  \u{2514}\u{2500} Durable: yes",
            ]
        );
    }

    #[test]
    fn completed_create_omits_raw_cron_when_schedule_output_exists() {
        let tc = cron_tool_call(
            "CronCreate",
            json!({
                "cron": "7 * * * *",
                "prompt": "Send hourly tick"
            }),
            Some(
                "Schedule ID: schedule-1\nSchedule: Every hour at minute 07\nRecurring: yes\nDurable: no",
            ),
        );

        let rendered = rendered_line_texts(&render_tool_content(&tc));

        assert_eq!(
            rendered,
            vec![
                "Prompt: Send hourly tick",
                "Recurring: yes",
                "Schedule ID: schedule-1",
                "Schedule: Every hour at minute 07",
                "Recurring: yes",
                "Durable: no",
            ]
        );
    }

    #[test]
    fn completed_delete_output_does_not_duplicate_schedule_id() {
        let tc = cron_tool_call(
            "CronDelete",
            json!({ "id": "schedule-1" }),
            Some("Schedule ID: schedule-1"),
        );

        let rendered = rendered_line_texts(&render_tool_content(&tc));

        assert_eq!(rendered, vec!["Schedule ID: schedule-1"]);
    }

    #[test]
    fn list_input_has_no_body() {
        let tc = cron_tool_call("CronList", json!({}), None);

        let rendered = rendered_line_texts(&super::super::standard::render_tool_call_body(&tc, 80));

        assert!(rendered.is_empty());
    }

    #[test]
    fn output_body_formats_colon_fields_and_maps_labels() {
        let tc = cron_tool_call(
            "CronList",
            json!({}),
            Some(
                "id: schedule-1\nhumanSchedule: every weekday at 09:30\nrecurring: false\ndurable: true",
            ),
        );

        let lines = render_tool_content(&tc);

        assert_eq!(
            rendered_line_texts(&lines),
            vec![
                "Schedule ID: schedule-1",
                "Schedule: every weekday at 09:30",
                "Recurring: no",
                "Durable: yes",
            ]
        );
    }

    #[test]
    fn list_output_renders_job_divider_without_label_corruption() {
        let content = format!(
            "Schedule ID: schedule-1\nSchedule: Every hour at minute 07\nPrompt: first\n{CRON_LIST_DIVIDER}\nSchedule ID: schedule-2\nCron: 0 9 1 * 1\nPrompt: second"
        );
        let tc = cron_tool_call("CronList", json!({}), Some(&content));

        let lines = render_tool_content(&tc);
        let rendered = rendered_line_texts(&lines);
        let divider = "\u{2500}".repeat(16);

        assert_eq!(
            rendered,
            vec![
                "Schedule ID: schedule-1",
                "Schedule: Every hour at minute 07",
                "Prompt: first",
                divider.as_str(),
                "Schedule ID: schedule-2",
                "Cron: 0 9 1 * 1",
                "Prompt: second",
            ]
        );
        assert_eq!(lines[3].spans[0].style.fg, Some(crate::ui::theme::DIM));
    }

    #[test]
    fn long_prompt_body_stays_bounded_after_wrapping() {
        let long_prompt = (0..120).map(|idx| format!("word{idx}")).collect::<Vec<_>>().join(" ");
        let content = format!(
            "Schedule ID: schedule-1\nCron: * * * * *\nSchedule: every minute\nPrompt: {long_prompt}\nRecurring: yes"
        );
        let tc = cron_tool_call("CronList", json!({}), Some(&content));

        let body = super::super::standard::render_tool_call_body(&tc, 40);
        let rendered = rendered_line_texts(&body);

        assert_eq!(body.len(), super::super::TOOL_BODY_MAX_LINES);
        assert!(rendered.iter().any(|line| line.contains("hidden")));
    }

    #[test]
    fn long_multi_job_prompt_body_stays_bounded_after_wrapping() {
        let long_prompt = (0..120).map(|idx| format!("word{idx}")).collect::<Vec<_>>().join(" ");
        let content = format!(
            "Schedule ID: schedule-1\nSchedule: Every hour at minute 07\nPrompt: {long_prompt}\n{CRON_LIST_DIVIDER}\nSchedule ID: schedule-2\nSchedule: Every day at 09:30\nPrompt: short"
        );
        let tc = cron_tool_call("CronList", json!({}), Some(&content));

        let body = super::super::standard::render_tool_call_body(&tc, 40);
        let rendered = rendered_line_texts(&body);

        assert_eq!(body.len(), super::super::TOOL_BODY_MAX_LINES);
        assert!(rendered.iter().any(|line| line.contains("hidden")));
    }
}
