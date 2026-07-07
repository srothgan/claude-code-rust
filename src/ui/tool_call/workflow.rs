// Copyright 2025 Simon Peter Rothgang
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
    use crate::app::{BlockCache, TerminalSnapshotMode};
    use crate::ui::theme;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn workflow_tool_call(
        raw_input: serde_json::Value,
        content: Option<&str>,
        status: model::ToolCallStatus,
    ) -> ToolCallInfo {
        let mut tc = ToolCallInfo {
            id: "tc-workflow".to_owned(),
            source_message_uuids: Vec::new(),
            title: "Workflow".to_owned(),
            sdk_tool_name: "Workflow".to_owned(),
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
    fn input_body_renders_workflow_fields_without_script_body() {
        let tc = workflow_tool_call(
            json!({
                "name": "spec",
                "script": "export const meta = { name: 'spec' };",
                "args": { "topic": "rendering", "retry": false },
                "scriptPath": "C:/work/.claude/workflows/spec.js",
                "resumeFromRunId": "run-1"
            }),
            None,
            model::ToolCallStatus::InProgress,
        );

        let lines = render_tool_content(&tc);

        assert_eq!(
            rendered_line_texts(&lines),
            vec![
                "Name: spec",
                "Script path: C:/work/.claude/workflows/spec.js",
                "Script: inline",
                "Args: {\"retry\":false,\"topic\":\"rendering\"}",
                "Resume run ID: run-1",
            ]
        );
        assert_eq!(lines[0].spans[0].style.fg, Some(theme::DIM));
    }

    #[test]
    fn output_body_renders_status_task_and_optional_fields() {
        let tc = workflow_tool_call(
            json!({}),
            Some(
                "Status: async launched\nTask ID: wf-1\nTask type: local_workflow\nWorkflow name: spec\nRun ID: run-1\nSummary: started\nTranscript dir: C:/tmp/transcripts\nScript path: C:/tmp/workflow.js\nSession URL: https://claude.ai/session/1\nWarning: branch diverged",
            ),
            model::ToolCallStatus::InProgress,
        );

        let lines = render_tool_content(&tc);

        assert_eq!(
            rendered_line_texts(&lines),
            vec![
                "Status: async launched",
                "Task ID: wf-1",
                "Task type: local_workflow",
                "Workflow name: spec",
                "Run ID: run-1",
                "Summary: started",
                "Transcript dir: C:/tmp/transcripts",
                "Script path: C:/tmp/workflow.js",
                "Session URL: https://claude.ai/session/1",
                "Warning: branch diverged",
            ]
        );
    }

    #[test]
    fn raw_output_labels_are_normalized_defensively() {
        let tc = workflow_tool_call(
            json!({}),
            Some("status: remote_launched\ntaskId: wf-2\nerror: syntax failed"),
            model::ToolCallStatus::Failed,
        );

        let lines = render_tool_content(&tc);

        assert_eq!(
            rendered_line_texts(&lines),
            vec!["Status: remote launched", "Task ID: wf-2", "Error: syntax failed"]
        );
    }
}
