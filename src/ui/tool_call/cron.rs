// SPDX-License-Identifier: Apache-2.0

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
    use crate::ui::tool_call::test_support::{
        card_body, rendered_line_texts, set_result_fields, tool_call,
    };
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn cron_tool_call(
        sdk_tool_name: &str,
        raw_input: serde_json::Value,
        content: Option<&str>,
    ) -> ToolCallInfo {
        tool_call(
            "tc-cron",
            "tc-cron",
            sdk_tool_name,
            raw_input,
            content,
            model::ToolCallStatus::Completed,
        )
    }

    #[test]
    fn create_input_body_renders_labels_and_defaults() {
        for (input, recurring) in [
            (json!({"cron":"30 9 * * 1","prompt":"Send weekly status","durable":true}), "yes"),
            (
                json!({"cron":"30 9 * * 1","prompt":"Send weekly status","recurring":false,"durable":true}),
                "no",
            ),
        ] {
            let mut tc = cron_tool_call("CronCreate", input, None);
            tc.status = model::ToolCallStatus::InProgress;
            let body = card_body(&mut tc, 100);
            for expected in [
                "Cron: 30 9 * * 1",
                "Prompt: Send weekly status",
                "Durable: yes",
                &format!("Recurring: {recurring}"),
            ] {
                assert!(body.iter().any(|row| row == expected), "{body:?}");
            }
            assert!(!body.iter().any(|row| row.starts_with("Schedule ID:")));
        }
    }

    #[test]
    fn completed_create_uses_confirmed_schedule_and_recurrence_without_request_defaults() {
        for (requested, returned, prompt) in [
            (
                json!({"cron":"7 * * * *","prompt":"Send hourly tick","durable":true}),
                "no",
                "Send hourly tick",
            ),
            (
                json!({"cron":"7 * * * *","prompt":"Repeat accepted tick","recurring":false,"durable":true}),
                "yes",
                "Repeat accepted tick",
            ),
        ] {
            let receipt = format!(
                "Schedule ID: accepted-job-71\nSchedule: Every hour at minute 07\nPrompt: {prompt}\nRecurring: {returned}\nDurable: no"
            );
            let mut tc = cron_tool_call("CronCreate", requested, Some(&receipt));
            set_result_fields(
                &mut tc,
                &[
                    ("Schedule ID", "accepted-job-71"),
                    ("Schedule", "Every hour at minute 07"),
                    ("Prompt", prompt),
                    ("Recurring", returned),
                    ("Durable", "no"),
                ],
            );
            let body = card_body(&mut tc, 100);
            for expected in [
                "Schedule ID: accepted-job-71",
                "Schedule: Every hour at minute 07",
                &format!("Prompt: {prompt}"),
                &format!("Recurring: {returned}"),
                "Durable: no",
            ] {
                assert_eq!(
                    body.iter().filter(|row| row.as_str() == expected).count(),
                    1,
                    "{body:?}"
                );
            }
            assert_eq!(body.iter().filter(|row| row.starts_with("Recurring:")).count(), 1);
            assert!(!body.iter().any(|row| row.starts_with("Cron:") || row == "Durable: yes"));
        }
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
    fn list_results_preserve_job_association_and_truthful_bounded_previews() {
        for structured in [true, false] {
            let mut tc = two_job_list("FIRST_JOB_PROMPT");
            if !structured {
                tc.output_metadata = None;
            }
            let body = card_body(&mut tc, 100);
            assert_job_regions(&body);
        }

        let mut empty = cron_tool_call("CronList", json!({}), Some("Jobs: none"));
        set_result_fields(&mut empty, &[("Jobs", "none")]);
        let body = card_body(&mut empty, 100);
        assert!(body.iter().any(|row| row == "Jobs: none"));
        assert!(!body.iter().any(|row| row.starts_with("Schedule ID:")));
        let mut pending = cron_tool_call("CronList", json!({}), None);
        pending.status = model::ToolCallStatus::InProgress;
        assert!(!card_body(&mut pending, 100).iter().any(|row| row == "Jobs: none"));

        let long_prompt = format!("FIRST_JOB_PROMPT {} FIRST_JOB_TAIL", "detail ".repeat(100));
        let mut tc = two_job_list(&long_prompt);
        let canonical = tc.content.clone();
        let result = tc.output_metadata.as_ref().unwrap().presentation.as_ref().unwrap();
        let full = rendered_line_texts(&super::super::result_presentation::render(
            result,
            crate::app::presentation::ClockFormat::default(),
        ));
        assert_job_regions(&full);
        let body = card_body(&mut tc, 60);
        assert!(body.iter().any(|row| row == "Schedule ID: job-alpha"), "{body:?}");
        assert!(body.iter().any(|row| row == "Schedule: Every hour at minute 07"));
        assert!(body.iter().any(|row| row.contains("FIRST_JOB_PROMPT")));
        assert!(body.iter().any(|row| row.contains("hidden")), "{body:?}");
        assert!(
            !body.iter().any(|row| row.contains("SECOND_JOB_PROMPT") || row.contains("job-beta"))
        );
        assert_eq!(tc.content, canonical);

        // Saved text uses the existing tail policy rather than protected metadata.
        let mut saved = two_job_list(&long_prompt);
        saved.output_metadata = None;
        let body = card_body(&mut saved, 60);
        assert!(body.iter().any(|row| row.contains("FIRST_JOB_TAIL")), "{body:?}");
        assert!(body.iter().any(|row| row.contains("hidden")));
        let second = body.iter().position(|row| row == "Schedule ID: job-beta").unwrap();
        assert!(body[second..].iter().any(|row| row == "Schedule: Every day at 09:30"));
        assert!(body[second..].iter().any(|row| row == "Prompt: SECOND_JOB_PROMPT"));
        assert!(!body[second..].iter().any(|row| row.contains("FIRST_JOB")));
    }

    fn two_job_list(first_prompt: &str) -> ToolCallInfo {
        // Literal saved divider is independent of the renderer's constant.
        let receipt = format!(
            "Schedule ID: job-alpha\nSchedule: Every hour at minute 07\nPrompt: {first_prompt}\n__cron_list_job_divider__\nSchedule ID: job-beta\nSchedule: Every day at 09:30\nPrompt: SECOND_JOB_PROMPT"
        );
        let mut tc = cron_tool_call("CronList", json!({}), Some(&receipt));
        tc.output_metadata = Some(
            serde_json::from_value(json!({"presentation":{"sections":[
                {"fields":[
                    {"label":"Schedule ID","value":{"type":"text","value":"job-alpha"}},
                    {"label":"Schedule","value":{"type":"text","value":"Every hour at minute 07"}},
                    {"label":"Prompt","value":{"type":"text","value":first_prompt}}
                ]},
                {"fields":[
                    {"label":"Schedule ID","value":{"type":"text","value":"job-beta"}},
                    {"label":"Schedule","value":{"type":"text","value":"Every day at 09:30"}},
                    {"label":"Prompt","value":{"type":"text","value":"SECOND_JOB_PROMPT"}}
                ]}
            ]}}))
            .unwrap(),
        );
        tc
    }

    fn assert_job_regions(rows: &[String]) {
        for (id, schedule, prompt, other) in [
            ("job-alpha", "Every hour at minute 07", "FIRST_JOB_PROMPT", "SECOND_JOB_PROMPT"),
            ("job-beta", "Every day at 09:30", "SECOND_JOB_PROMPT", "FIRST_JOB_PROMPT"),
        ] {
            let start = rows.iter().position(|row| row == &format!("Schedule ID: {id}")).unwrap();
            let end = rows[start + 1..]
                .iter()
                .position(|row| row.starts_with("Schedule ID:"))
                .map_or(rows.len(), |offset| start + 1 + offset);
            let region = &rows[start..end];
            assert!(region.iter().any(|row| row == &format!("Schedule: {schedule}")), "{region:?}");
            assert!(region.iter().any(|row| row.contains(prompt)), "{region:?}");
            assert!(!region.iter().any(|row| row.contains(other)), "{region:?}");
        }
    }
}
