// SPDX-License-Identifier: Apache-2.0

//! Rendering helpers for SDK `Workflow` tool calls.

use crate::app::ToolCallInfo;
use ratatui::text::Line;

use super::fields::{self, ToolField};
use super::typed;

pub(super) fn is_workflow_tool(tc: &ToolCallInfo) -> bool {
    tc.sdk_tool_name == "Workflow"
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
    let mut workflow_fields = Vec::new();

    if let Some(input) = input {
        if let Some(name) = typed::json_string(input, "name") {
            workflow_fields.push(ToolField::new("Name", name));
        }
        if let Some(script_path) = typed::json_string(input, "scriptPath") {
            workflow_fields.push(ToolField::new("Script path", script_path));
        }
        if input
            .get("script")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|script| !script.trim().is_empty())
        {
            workflow_fields.push(ToolField::new("Script", "inline"));
        }
        if let Some(args) = input.get("args").and_then(typed::non_empty_compact_json) {
            workflow_fields.push(ToolField::new("Args", args));
        }
        if let Some(resume_run_id) = typed::json_string(input, "resumeFromRunId") {
            workflow_fields.push(ToolField::new("Resume run ID", resume_run_id));
        }
    }

    fields::render_fields(workflow_fields)
}

fn render_text_line(line: &str) -> Line<'static> {
    typed::render_colon_field_line(line, field_label, field_value)
}

fn field_label(label: &str) -> Option<&'static str> {
    match label {
        "status" | "Status" => Some("Status"),
        "taskId" | "Task ID" => Some("Task ID"),
        "taskType" | "Task type" => Some("Task type"),
        "workflowName" | "Workflow name" => Some("Workflow name"),
        "runId" | "Run ID" => Some("Run ID"),
        "summary" | "Summary" => Some("Summary"),
        "transcriptDir" | "Transcript dir" => Some("Transcript dir"),
        "scriptPath" | "Script path" => Some("Script path"),
        "sessionUrl" | "Session URL" => Some("Session URL"),
        "warning" | "Warning" => Some("Warning"),
        "error" | "Error" => Some("Error"),
        _ => None,
    }
}

fn field_value(label: &str, value: &str) -> String {
    match label {
        "Status" => workflow_status_label(value).unwrap_or(value).to_owned(),
        _ => value.to_owned(),
    }
}

fn workflow_status_label(value: &str) -> Option<&'static str> {
    match value {
        "async_launched" | "async launched" => Some("async launched"),
        "remote_launched" | "remote launched" => Some("remote launched"),
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

    fn workflow_tool_call(
        raw_input: serde_json::Value,
        content: Option<&str>,
        status: model::ToolCallStatus,
    ) -> ToolCallInfo {
        tool_call("tc-workflow", "Workflow", "Workflow", raw_input, content, status)
    }

    #[test]
    fn input_body_renders_workflow_fields_without_script_body() {
        let cases = [
            (
                json!({"name":"release-review","args":{"topic":"literal: review","retry":false}}),
                vec!["Name: release-review"],
                vec!["Script:", "Script path:", "Resume run ID:"],
            ),
            (
                json!({"script":"export const meta = { name: 'private-source-sentinel', description: 'check', phases: [] };","args":{"topic":"literal: review","retry":false}}),
                vec!["Script: inline"],
                vec!["private-source-sentinel", "Name:", "Script path:"],
            ),
            (
                json!({"scriptPath":"C:/work space/check.js","resumeFromRunId":"prior-run-42","args":{"topic":"literal: review","retry":false}}),
                vec!["Script path: C:/work space/check.js", "Resume run ID: prior-run-42"],
                vec!["Script: inline", "Name:"],
            ),
        ];
        for (input, expected, absent) in cases {
            let mut tc = workflow_tool_call(input, None, model::ToolCallStatus::InProgress);
            let body = card_body(&mut tc, 120);
            for value in expected {
                assert!(body.iter().any(|line| line == value), "{body:?}");
            }
            for value in absent {
                assert!(!body.iter().any(|line| line.contains(value)), "{body:?}");
            }
            let args = body
                .iter()
                .find_map(|line| line.strip_prefix("Args: "))
                .expect("literal arguments");
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(args).unwrap(),
                json!({"topic":"literal: review","retry":false})
            );
        }
    }

    #[test]
    fn output_body_renders_status_task_and_optional_fields() {
        for (receipt, status, expected, absent) in [
            (
                "Status: async launched\nTask ID: local-task-17\nTask type: local_workflow\nWorkflow name: release-review\nRun ID: local-run-83\nTranscript dir: C:/session/transcripts\nScript path: C:/session/check.js\nWarning: branch diverged",
                model::ToolCallStatus::Detached,
                vec![
                    "Task ID: local-task-17",
                    "Run ID: local-run-83",
                    "Transcript dir: C:/session/transcripts",
                    "Script path: C:/session/check.js",
                    "Warning: branch diverged",
                ],
                vec!["Session URL:", "tc-workflow"],
            ),
            (
                "Status: remote launched\nTask ID: remote-task-29\nTask type: remote_workflow\nSession URL: https://claude.ai/session/remote-61\nSummary: remote review started",
                model::ToolCallStatus::Detached,
                vec![
                    "Task ID: remote-task-29",
                    "Session URL: https://claude.ai/session/remote-61",
                    "Summary: remote review started",
                ],
                vec!["Run ID:", "local-task-17", "tc-workflow"],
            ),
            (
                "Error: syntax failed at check.js:12",
                model::ToolCallStatus::Failed,
                vec!["Error: syntax failed at check.js:12"],
                vec!["Task ID:", "Run ID:", "Session URL:"],
            ),
        ] {
            let mut tc = workflow_tool_call(json!({}), Some(receipt), status);
            let body = card_body(&mut tc, 120);
            for value in expected {
                assert!(body.iter().any(|line| line == value), "{body:?}");
            }
            for value in absent {
                assert!(!body.iter().any(|line| line.contains(value)), "{body:?}");
            }
        }
    }
}
