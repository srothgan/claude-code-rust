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
use super::fields::{self, ToolField};

const TASK_OMISSION_MARKER: &str = "...";
const TASK_MARKER_PENDING: &str = "\u{25a1}";
const TASK_MARKER_IN_PROGRESS: &str = "\u{25a3}";
const TASK_MARKER_COMPLETED: &str = "\u{25a0}";

pub(super) fn is_state_tool(tc: &ToolCallInfo) -> bool {
    matches!(
        tc.sdk_tool_name.as_str(),
        "TaskCreate" | "TaskUpdate" | "TaskGet" | "TaskList" | "TaskOutput" | "TaskStop"
    )
}

pub(super) fn has_structured_body(tc: &ToolCallInfo) -> bool {
    match tc.sdk_tool_name.as_str() {
        "TaskCreate" | "TaskUpdate" | "TaskGet" | "TaskOutput" | "TaskStop" => {
            tc.raw_input.is_some() || !tc.content.is_empty()
        }
        "TaskList" => !tc.content.is_empty(),
        _ => false,
    }
}

pub(super) fn update_deletes_task(tc: &ToolCallInfo) -> bool {
    if tc.sdk_tool_name != "TaskUpdate" {
        return false;
    }
    task_update_status(tc) == Some("deleted")
}

pub(super) fn title_marker(tc: &ToolCallInfo) -> Option<(&'static str, Style)> {
    match tc.sdk_tool_name.as_str() {
        "TaskCreate" | "TaskGet" | "TaskList" | "TaskOutput" | "TaskStop" => {
            Some(task_marker(model::TaskStatus::Pending))
        }
        "TaskUpdate" => Some(match task_update_status(tc) {
            Some("running" | "in_progress") => task_marker(model::TaskStatus::InProgress),
            Some("completed") => task_marker(model::TaskStatus::Completed),
            _ => task_marker(model::TaskStatus::Pending),
        }),
        _ => None,
    }
}

pub(super) fn render_tool_content(tc: &ToolCallInfo) -> Option<Vec<Line<'static>>> {
    let mut lines = match tc.sdk_tool_name.as_str() {
        "TaskCreate" => render_create_content(tc),
        "TaskUpdate" => render_update_content(tc),
        "TaskGet" => render_get_content(tc),
        "TaskList" => Vec::new(),
        "TaskOutput" => render_output_content(tc),
        "TaskStop" => render_stop_content(tc),
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
    let mut task_fields = Vec::new();

    if let Some(description) = input.and_then(|input| json_string(input, "description")) {
        task_fields.push(ToolField::new("Description", description));
    }
    if let Some(metadata) = input.and_then(|input| input.get("metadata")).and_then(compact_json) {
        task_fields.push(ToolField::new("Metadata", metadata));
    }
    fields::render_fields(task_fields)
}

fn render_update_content(tc: &ToolCallInfo) -> Vec<Line<'static>> {
    let input = tc.raw_input.as_ref().and_then(serde_json::Value::as_object);
    let mut task_fields = Vec::new();

    if let Some(input) = input {
        for (key, label) in [("description", "Description"), ("owner", "Owner")] {
            if let Some(value) = json_string(input, key) {
                task_fields.push(ToolField::new(label, value));
            }
        }
        if let Some(blocks) = json_string_array(input.get("addBlocks")) {
            task_fields.push(ToolField::new("Blocks", blocks.join(", ")));
        }
        if let Some(blocked_by) = json_string_array(input.get("addBlockedBy")) {
            task_fields.push(ToolField::new("Blocked by", blocked_by.join(", ")));
        }
        if let Some(metadata) = input.get("metadata").and_then(compact_json) {
            task_fields.push(ToolField::new("Metadata", metadata));
        }
    }
    fields::render_fields(task_fields)
}

fn render_get_content(tc: &ToolCallInfo) -> Vec<Line<'static>> {
    let input = tc.raw_input.as_ref().and_then(serde_json::Value::as_object);
    let mut task_fields = Vec::new();
    if let Some(task_id) = input.and_then(|input| json_string(input, "taskId")) {
        task_fields.push(ToolField::new("Task ID", task_id));
    }
    fields::render_fields(task_fields)
}

fn render_output_content(tc: &ToolCallInfo) -> Vec<Line<'static>> {
    let input = tc.raw_input.as_ref().and_then(serde_json::Value::as_object);
    let mut task_fields = Vec::new();
    if let Some(input) = input {
        if let Some(task_id) = json_string(input, "task_id") {
            task_fields.push(ToolField::new("Task ID", task_id));
        }
        if let Some(block) = json_bool(input, "block") {
            task_fields.push(ToolField::new("Block", boolean_label(block)));
        }
        if let Some(timeout) = json_i64(input, "timeout") {
            task_fields.push(ToolField::new("Timeout", format!("{timeout}ms")));
        }
    }
    fields::render_fields(task_fields)
}

fn render_stop_content(tc: &ToolCallInfo) -> Vec<Line<'static>> {
    let input = tc.raw_input.as_ref().and_then(serde_json::Value::as_object);
    let mut task_fields = Vec::new();
    if let Some(input) = input {
        if let Some(task_id) = json_string(input, "task_id") {
            task_fields.push(ToolField::new("Task ID", task_id));
        } else if let Some(shell_id) = json_string(input, "shell_id") {
            task_fields.push(ToolField::new("Shell ID", shell_id));
        }
    }
    fields::render_fields(task_fields)
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
            .map(|line| render_text_line(tc, line)),
    );
}

fn render_text_line(tc: &ToolCallInfo, line: &str) -> Line<'static> {
    if line == TASK_OMISSION_MARKER {
        return omission_line();
    }
    if let Some(subject) = strip_task_marker_prefix(line, TASK_MARKER_COMPLETED) {
        return render_task_line(subject, "", model::TaskStatus::Completed);
    }
    if let Some(subject) = strip_task_marker_prefix(line, TASK_MARKER_IN_PROGRESS) {
        return render_task_line(subject, "", model::TaskStatus::InProgress);
    }
    if let Some(subject) = strip_task_marker_prefix(line, TASK_MARKER_PENDING) {
        return render_task_line(subject, "", model::TaskStatus::Pending);
    }
    if line.starts_with("Deleted task") {
        return deleted_task_line(line);
    }
    if matches!(line, "Task not found" | "No tasks") {
        return Line::from(Span::styled(line.to_owned(), Style::default().fg(theme::DIM)));
    }
    if let Some((label, value)) = line.split_once(':') {
        let label = match label {
            "Task ID" => "Task ID",
            "Task type" => "Task type",
            "Fields" => "Fields",
            "Status" => "Status",
            "Blocked by" => "Blocked by",
            "Activity" => "Activity",
            "Message" => "Message",
            "Command" => "Command",
            _ if uses_dynamic_field_labels(tc) && is_safe_dynamic_label(label) => {
                return fields::render_dynamic_field(label.trim(), value.trim_start());
            }
            _ => return Line::from(Span::raw(line.to_owned())),
        };
        let value = value.trim_start();
        if label == "Status" {
            return fields::render_field(label, status_label(value));
        }
        return fields::render_field(label, value);
    }
    Line::from(Span::raw(line.to_owned()))
}

fn uses_dynamic_field_labels(tc: &ToolCallInfo) -> bool {
    tc.sdk_tool_name == "TaskOutput"
}

fn is_safe_dynamic_label(label: &str) -> bool {
    let label = label.trim();
    !label.is_empty()
        && label.len() <= 48
        && label.chars().any(char::is_alphabetic)
        && label.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == ' ')
}

fn boolean_label(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
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

fn task_update_status(tc: &ToolCallInfo) -> Option<&str> {
    tc.raw_input.as_ref().and_then(|input| input.get("status")).and_then(serde_json::Value::as_str)
}

fn strip_task_marker_prefix<'a>(line: &'a str, marker: &str) -> Option<&'a str> {
    line.strip_prefix(marker)?.strip_prefix(' ')
}

fn json_string<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Option<&'a str> {
    object.get(key).and_then(serde_json::Value::as_str).filter(|value| !value.is_empty())
}

fn json_bool(object: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<bool> {
    object.get(key).and_then(serde_json::Value::as_bool)
}

fn json_i64(object: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<i64> {
    object.get(key).and_then(serde_json::Value::as_i64)
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

fn deleted_task_line(text: &str) -> Line<'static> {
    Line::from(Span::styled(text.to_owned(), Style::default().fg(theme::DIM)))
}

fn render_task_line(subject: &str, active_form: &str, status: model::TaskStatus) -> Line<'static> {
    let (marker, marker_style) = task_marker(status);
    let (text, text_style) = match status {
        model::TaskStatus::Completed => {
            (subject, Style::default().fg(theme::DIM).add_modifier(Modifier::DIM))
        }
        model::TaskStatus::InProgress => (
            if active_form.is_empty() { subject } else { active_form },
            Style::default().fg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD),
        ),
        model::TaskStatus::Pending => (subject, Style::default().fg(Color::Gray)),
    };

    Line::from(vec![
        Span::styled(marker.to_owned(), marker_style),
        Span::raw(" "),
        Span::styled(text.to_owned(), text_style),
    ])
}

fn task_marker(status: model::TaskStatus) -> (&'static str, Style) {
    match status {
        model::TaskStatus::Completed => (TASK_MARKER_COMPLETED, Style::default().fg(theme::DIM)),
        model::TaskStatus::InProgress => (
            TASK_MARKER_IN_PROGRESS,
            Style::default().fg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD),
        ),
        model::TaskStatus::Pending => (TASK_MARKER_PENDING, Style::default().fg(theme::DIM)),
    }
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
            source_message_uuids: Vec::new(),
            title: "tc-task".to_owned(),
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

    fn task_tool_call_title(tc: &ToolCallInfo) -> Line<'static> {
        super::super::standard::render_tool_call_title(
            tc,
            super::super::ToolCallRenderContext::default(),
            80,
            0,
        )
    }

    fn task_title_marker_span<'a>(line: &'a Line<'static>) -> &'a Span<'static> {
        line.spans.get(1).expect("task title marker span")
    }

    #[test]
    fn create_title_uses_pending_task_marker() {
        let mut tc = task_tool_call(
            "TaskCreate",
            json!({ "subject": "Run checks", "description": "Validate the branch" }),
            None,
        );
        tc.title = "Create task #1: Run checks".to_owned();

        let title = task_tool_call_title(&tc);
        let marker = task_title_marker_span(&title);
        let text: String = title.spans.iter().map(|span| span.content.as_ref()).collect();

        assert_eq!(marker.content.as_ref(), format!("{TASK_MARKER_PENDING} "));
        assert_eq!(marker.style.fg, Some(theme::DIM));
        assert!(!text.contains('\u{25cc}'));
    }

    #[test]
    fn in_progress_update_title_uses_active_task_marker() {
        let mut tc = task_tool_call(
            "TaskUpdate",
            json!({ "taskId": "task-1", "status": "in_progress" }),
            None,
        );
        tc.title = "Start task #1: Run checks".to_owned();

        let title = task_tool_call_title(&tc);
        let marker = task_title_marker_span(&title);

        assert_eq!(marker.content.as_ref(), format!("{TASK_MARKER_IN_PROGRESS} "));
        assert_eq!(marker.style.fg, Some(theme::RUST_ORANGE));
        assert_eq!(marker.style.add_modifier, Modifier::BOLD);
    }

    #[test]
    fn completed_update_title_uses_completed_task_marker() {
        let mut tc = task_tool_call(
            "TaskUpdate",
            json!({ "taskId": "task-1", "status": "completed" }),
            None,
        );
        tc.title = "Complete task #1: Run checks".to_owned();

        let title = task_tool_call_title(&tc);
        let marker = task_title_marker_span(&title);

        assert_eq!(marker.content.as_ref(), format!("{TASK_MARKER_COMPLETED} "));
        assert_eq!(marker.style.fg, Some(theme::DIM));
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
    fn output_body_renders_input_fields_and_output_text() {
        let tc = task_tool_call(
            "TaskOutput",
            json!({ "task_id": "task-1", "block": true, "timeout": 1000 }),
            Some("Retrieval status: not ready\nTask type: local bash\nStatus: running"),
        );

        let rendered = rendered_line_texts(&super::super::standard::render_tool_call_body(&tc, 80));

        assert!(rendered.iter().any(|line| line.contains("Task ID: task-1")));
        assert!(rendered.iter().any(|line| line.contains("Block: yes")));
        assert!(rendered.iter().any(|line| line.contains("Timeout: 1000ms")));
        assert!(rendered.iter().any(|line| line.contains("Retrieval status: not ready")));
        assert!(rendered.iter().any(|line| line.contains("Task type: local bash")));
        assert!(rendered.iter().any(|line| line.contains("Status: In Progress")));
    }

    #[test]
    fn stop_body_renders_input_and_structured_output_fields() {
        let tc = task_tool_call(
            "TaskStop",
            json!({ "task_id": "task-1" }),
            Some("Message: Stopped task\nTask ID: task-1\nTask type: bash\nCommand: npm run watch"),
        );

        let rendered = rendered_line_texts(&super::super::standard::render_tool_call_body(&tc, 80));

        assert!(rendered.iter().any(|line| line.contains("Task ID: task-1")));
        assert!(rendered.iter().any(|line| line.contains("Message: Stopped task")));
        assert!(rendered.iter().any(|line| line.contains("Task type: bash")));
        assert!(rendered.iter().any(|line| line.contains("Command: npm run watch")));
    }

    #[test]
    fn deleted_update_uses_completed_status_icon() {
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
        assert!(
            title.spans.first().is_some_and(|span| span.content.contains(theme::ICON_COMPLETED))
        );
        let marker = task_title_marker_span(&title);
        assert_eq!(marker.content.as_ref(), format!("{TASK_MARKER_PENDING} "));
        assert_eq!(marker.style.fg, Some(theme::DIM));

        let body = super::super::standard::render_tool_call_body(&tc, 80);
        assert!(rendered_line_texts(&body).iter().any(|line| line.contains("Deleted task")));
        let deleted = body
            .iter()
            .find(|line| line.spans.iter().any(|span| span.content.contains("Deleted task")));
        assert_eq!(
            deleted.and_then(|line| line.spans.first()).and_then(|span| span.style.fg),
            Some(theme::DIM)
        );
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
