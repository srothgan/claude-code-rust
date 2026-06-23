// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

//! Rendering helpers for SDK `Workflow` tool calls.

use crate::agent::model;
use crate::app::ToolCallInfo;
use crate::ui::diff::strip_outer_code_fence;
use ratatui::text::{Line, Span};

use super::errors::render_failed_tool_text_content;
use super::fields::{self, ToolField};

pub(super) fn is_workflow_tool(tc: &ToolCallInfo) -> bool {
    tc.sdk_tool_name == "Workflow"
}

pub(super) fn has_structured_body(tc: &ToolCallInfo) -> bool {
    tc.raw_input.is_some() || !tc.content.is_empty()
}

pub(super) fn render_tool_content(tc: &ToolCallInfo) -> Option<Vec<Line<'static>>> {
    if !is_workflow_tool(tc) {
        return None;
    }

    let mut lines = render_input_content(tc);
    lines.extend(render_text_content(tc));
    Some(lines)
}

fn render_input_content(tc: &ToolCallInfo) -> Vec<Line<'static>> {
    let input = tc.raw_input.as_ref().and_then(serde_json::Value::as_object);
    let mut workflow_fields = Vec::new();

    if let Some(input) = input {
        if let Some(name) = json_string(input, "name") {
            workflow_fields.push(ToolField::new("Name", name));
        }
        if let Some(script_path) = json_string(input, "scriptPath") {
            workflow_fields.push(ToolField::new("Script path", script_path));
        }
        if input
            .get("script")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|script| !script.trim().is_empty())
        {
            workflow_fields.push(ToolField::new("Script", "inline"));
        }
        if let Some(args) = input.get("args").and_then(compact_json) {
            workflow_fields.push(ToolField::new("Args", args));
        }
        if let Some(resume_run_id) = json_string(input, "resumeFromRunId") {
            workflow_fields.push(ToolField::new("Resume run ID", resume_run_id));
        }
    }

    fields::render_fields(workflow_fields)
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

fn compact_json(value: &serde_json::Value) -> Option<String> {
    if value.is_null() {
        return None;
    }
    if let Some(text) = value.as_str() {
        let trimmed = text.trim();
        return (!trimmed.is_empty()).then(|| trimmed.to_owned());
    }
    if value.as_array().is_some_and(Vec::is_empty) {
        return None;
    }
    if value.as_object().is_some_and(serde_json::Map::is_empty) {
        return None;
    }
    serde_json::to_string(value).ok()
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

        let lines = render_tool_content(&tc).expect("workflow content");

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

        let lines = render_tool_content(&tc).expect("workflow content");

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

        let lines = render_tool_content(&tc).expect("workflow content");

        assert_eq!(
            rendered_line_texts(&lines),
            vec!["Status: remote launched", "Task ID: wf-2", "Error: syntax failed"]
        );
    }
}
