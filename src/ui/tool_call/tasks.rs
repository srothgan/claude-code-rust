// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

//! Rendering helpers for SDK task-state tools.

use crate::agent::model;
use crate::app::ToolCallInfo;
use crate::ui::diff::strip_outer_code_fence;
use crate::ui::theme;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::errors::render_failed_tool_text_content;

const TASK_OMISSION_MARKER: &str = "...";

pub(super) fn is_state_tool(tc: &ToolCallInfo) -> bool {
    matches!(tc.sdk_tool_name.as_str(), "TaskCreate" | "TaskUpdate" | "TaskGet" | "TaskList")
}

pub(super) fn has_structured_body(tc: &ToolCallInfo) -> bool {
    match tc.sdk_tool_name.as_str() {
        "TaskCreate" | "TaskUpdate" | "TaskGet" => tc.raw_input.is_some() || !tc.content.is_empty(),
        "TaskList" => !tc.content.is_empty(),
        _ => false,
    }
}

pub(super) fn update_deletes_task(tc: &ToolCallInfo) -> bool {
    if tc.sdk_tool_name != "TaskUpdate" {
        return false;
    }
    tc.raw_input.as_ref().and_then(|input| input.get("status")).and_then(serde_json::Value::as_str)
        == Some("deleted")
}

pub(super) fn render_tool_content(tc: &ToolCallInfo) -> Option<Vec<Line<'static>>> {
    let mut lines = match tc.sdk_tool_name.as_str() {
        "TaskCreate" => render_create_content(tc),
        "TaskUpdate" => render_update_content(tc),
        "TaskGet" => render_get_content(tc),
        "TaskList" => Vec::new(),
        _ => return None,
    };

    let mut output_lines = render_text_content(tc);
    if update_deletes_task(tc)
        && matches!(tc.status, model::ToolCallStatus::Completed)
        && output_lines.is_empty()
    {
        output_lines.push(deleted_task_line("Deleted task"));
    }

    lines.extend(output_lines);
    Some(lines)
}

fn render_create_content(tc: &ToolCallInfo) -> Vec<Line<'static>> {
    let input = tc.raw_input.as_ref().and_then(serde_json::Value::as_object);
    let mut lines = Vec::new();

    if let Some(description) = input.and_then(|input| json_string(input, "description")) {
        lines.push(field_line("Description", description));
    }
    if let Some(metadata) = input.and_then(|input| input.get("metadata")).and_then(compact_json) {
        lines.push(field_line("Metadata", &metadata));
    }
    lines
}

fn render_update_content(tc: &ToolCallInfo) -> Vec<Line<'static>> {
    let input = tc.raw_input.as_ref().and_then(serde_json::Value::as_object);
    let mut lines = Vec::new();

    if let Some(input) = input {
        for (key, label) in [("description", "Description"), ("owner", "Owner")] {
            if let Some(value) = json_string(input, key) {
                lines.push(field_line(label, value));
            }
        }
        if let Some(blocks) = json_string_array(input.get("addBlocks")) {
            lines.push(field_line("Blocks", &blocks.join(", ")));
        }
        if let Some(blocked_by) = json_string_array(input.get("addBlockedBy")) {
            lines.push(field_line("Blocked by", &blocked_by.join(", ")));
        }
        if let Some(metadata) = input.get("metadata").and_then(compact_json) {
            lines.push(field_line("Metadata", &metadata));
        }
    }
    lines
}

fn render_get_content(tc: &ToolCallInfo) -> Vec<Line<'static>> {
    let input = tc.raw_input.as_ref().and_then(serde_json::Value::as_object);
    let mut lines = Vec::new();
    if let Some(task_id) = input.and_then(|input| json_string(input, "taskId")) {
        lines.push(field_line("Task ID", task_id));
    }
    lines
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
    if line == TASK_OMISSION_MARKER {
        return omission_line();
    }
    if let Some(subject) = line.strip_prefix("\u{25a0} ") {
        return render_task_line(subject, "", model::TaskStatus::Completed);
    }
    if let Some(subject) = line.strip_prefix("\u{25a3} ") {
        return render_task_line(subject, "", model::TaskStatus::InProgress);
    }
    if let Some(subject) = line.strip_prefix("\u{25a1} ") {
        return render_task_line(subject, "", model::TaskStatus::Pending);
    }
    if line.starts_with("Deleted task") {
        return deleted_task_line(line);
    }
    if matches!(line, "Task not found" | "No tasks") {
        return Line::from(Span::styled(line.to_owned(), Style::default().fg(theme::DIM)));
    }
    if let Some((label, value)) = line.split_once(':')
        && matches!(label, "Task ID" | "Fields" | "Status" | "Blocked by" | "Activity")
    {
        let value = value.trim_start();
        if label == "Status" {
            return field_line(label, &status_label(value));
        }
        return field_line(label, value);
    }
    Line::from(Span::raw(line.to_owned()))
}

fn status_label(status: &str) -> String {
    match status {
        "pending" => "Pending".to_owned(),
        "running" | "in_progress" => "In Progress".to_owned(),
        "completed" => "Completed".to_owned(),
        "deleted" => "Deleted".to_owned(),
        other => other.to_owned(),
    }
}

fn json_string<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Option<&'a str> {
    object.get(key).and_then(serde_json::Value::as_str).filter(|value| !value.is_empty())
}

fn json_string_array(value: Option<&serde_json::Value>) -> Option<Vec<String>> {
    Some(
        value?
            .as_array()?
            .iter()
            .filter_map(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .collect(),
    )
}

fn compact_json(value: &serde_json::Value) -> Option<String> {
    serde_json::to_string(value).ok().filter(|value| !value.is_empty())
}

fn field_line(label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label}: "), Style::default().fg(theme::DIM)),
        Span::raw(value.to_owned()),
    ])
}

fn deleted_task_line(text: &str) -> Line<'static> {
    Line::from(Span::styled(
        text.to_owned(),
        Style::default().fg(theme::STATUS_ERROR).add_modifier(Modifier::BOLD),
    ))
}

fn render_task_line(subject: &str, active_form: &str, status: model::TaskStatus) -> Line<'static> {
    let (marker, marker_style, text, text_style) = match status {
        model::TaskStatus::Completed => (
            "\u{25a0}",
            Style::default().fg(theme::DIM),
            subject,
            Style::default().fg(theme::DIM).add_modifier(Modifier::DIM),
        ),
        model::TaskStatus::InProgress => (
            "\u{25a3}",
            Style::default().fg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD),
            if active_form.is_empty() { subject } else { active_form },
            Style::default().fg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD),
        ),
        model::TaskStatus::Pending => {
            ("\u{25a1}", Style::default().fg(theme::DIM), subject, Style::default().fg(Color::Gray))
        }
    };

    Line::from(vec![
        Span::styled(marker.to_owned(), marker_style),
        Span::raw(" "),
        Span::styled(text.to_owned(), text_style),
    ])
}

fn omission_line() -> Line<'static> {
    Line::from(Span::styled(
        TASK_OMISSION_MARKER,
        Style::default().fg(theme::DIM).add_modifier(Modifier::ITALIC),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{BlockCache, TerminalSnapshotMode};
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn task_tool_call(
        sdk_tool_name: &str,
        raw_input: serde_json::Value,
        content: Option<&str>,
    ) -> ToolCallInfo {
        let mut tc = ToolCallInfo {
            id: "tc-task".to_owned(),
            title: "tc-task".to_owned(),
            sdk_tool_name: sdk_tool_name.to_owned(),
            raw_input: Some(raw_input),
            raw_input_bytes: 0,
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
    fn create_body_renders_structured_fields() {
        let tc = task_tool_call(
            "TaskCreate",
            json!({
                "subject": "Run checks",
                "description": "Validate the branch",
                "activeForm": "Running checks",
                "metadata": { "phase": "6A" }
            }),
            None,
        );

        let rendered = rendered_line_texts(&super::super::standard::render_tool_call_body(&tc, 80));

        assert!(!rendered.iter().any(|line| line.contains("Subject:")));
        assert!(rendered.iter().any(|line| line.contains("Description: Validate the branch")));
        assert!(!rendered.iter().any(|line| line.contains("Active:")));
        assert!(rendered.iter().any(|line| line.contains(r#"Metadata: {"phase":"6A"}"#)));
    }

    #[test]
    fn update_body_renders_activity_without_redundant_status() {
        let tc = task_tool_call(
            "TaskUpdate",
            json!({ "taskId": "task-1", "status": "in_progress" }),
            Some("Activity: Scaffolding Next.js app"),
        );

        let rendered = rendered_line_texts(&super::super::standard::render_tool_call_body(&tc, 80));

        assert!(!rendered.iter().any(|line| line.contains("Status:")));
        assert!(rendered.iter().any(|line| line.contains("Activity: Scaffolding Next.js app")));
        assert!(!rendered.iter().any(|line| line.contains("Task ID: task-1")));
    }

    #[test]
    fn deleted_update_uses_red_x_without_failed_status() {
        let tc = task_tool_call(
            "TaskUpdate",
            json!({ "taskId": "task-1", "status": "deleted" }),
            Some("Deleted task: task-1"),
        );

        assert_eq!(tc.status, model::ToolCallStatus::Completed);
        let title = super::super::standard::render_tool_call_title(
            &tc,
            super::super::ToolCallRenderContext::default(),
            80,
            0,
        );
        assert!(title.spans.first().is_some_and(|span| span.content.contains(theme::ICON_FAILED)));
        assert_eq!(title.spans.first().and_then(|span| span.style.fg), Some(theme::STATUS_ERROR));

        let rendered = rendered_line_texts(&super::super::standard::render_tool_call_body(&tc, 80));
        assert!(rendered.iter().any(|line| line.contains("Deleted task")));
    }

    #[test]
    fn get_null_renders_not_found() {
        let tc =
            task_tool_call("TaskGet", json!({ "taskId": "task-missing" }), Some("Task not found"));

        let rendered = rendered_line_texts(&super::super::standard::render_tool_call_body(&tc, 80));

        assert!(rendered.iter().any(|line| line.contains("Task not found")));
    }

    #[test]
    fn long_list_is_windowed() {
        let tc = task_tool_call(
            "TaskList",
            json!({}),
            Some(
                "...\n\u{25a0} Task 4\n\u{25a0} Task 5\n\u{25a3} Task 6\n\u{25a0} Task 7\n\u{25a0} Task 8\n\u{25a0} Task 9\n\u{25a0} Task 10\n...",
            ),
        );

        let rendered = rendered_line_texts(&super::super::standard::render_tool_call_body(&tc, 80));

        assert_eq!(rendered.len(), super::super::TOOL_BODY_MAX_LINES);
        assert!(rendered.iter().any(|line| line.contains("Task 6")));
        assert!(rendered.first().is_some_and(|line| line.contains("...")));
        assert!(rendered.last().is_some_and(|line| line.contains("...")));
    }
}
