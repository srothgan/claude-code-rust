// SPDX-License-Identifier: Apache-2.0

//! Rendering helpers for SDK task-state tools.

use crate::agent::model;
use crate::app::ToolCallInfo;
use crate::ui::theme;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::fields::{self, ToolField};
use super::typed::{self, compact_json, json_bool, json_i64, json_string, json_string_array};

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

pub(super) fn render_tool_content(tc: &ToolCallInfo) -> Vec<Line<'static>> {
    let mut lines = match tc.sdk_tool_name.as_str() {
        "TaskCreate" => render_create_content(tc),
        "TaskUpdate" => render_update_content(tc),
        "TaskGet" => render_get_content(tc),
        "TaskList" => Vec::new(),
        "TaskOutput" => render_output_content(tc),
        "TaskStop" => render_stop_content(tc),
        _ => return Vec::new(),
    };

    let mut output_lines = render_text_content(tc);
    if update_deletes_task(tc)
        && matches!(tc.status, model::ToolCallStatus::Completed)
        && output_lines.is_empty()
    {
        output_lines.push(deleted_task_line("Deleted task"));
    }

    lines.extend(output_lines);
    lines
}

fn render_create_content(tc: &ToolCallInfo) -> Vec<Line<'static>> {
    let input = typed::input_object(tc);
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
    let input = typed::input_object(tc);
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
    let input = typed::input_object(tc);
    let mut task_fields = Vec::new();
    if let Some(task_id) = input.and_then(|input| json_string(input, "taskId")) {
        task_fields.push(ToolField::new("Task ID", task_id));
    }
    fields::render_fields(task_fields)
}

fn render_output_content(tc: &ToolCallInfo) -> Vec<Line<'static>> {
    let input = typed::input_object(tc);
    let mut task_fields = Vec::new();
    if let Some(input) = input {
        if let Some(task_id) = json_string(input, "task_id") {
            task_fields.push(ToolField::new("Task ID", task_id));
        }
        if let Some(block) = json_bool(input, "block") {
            task_fields.push(ToolField::new("Block", typed::bool_label(block)));
        }
        if let Some(timeout) = json_i64(input, "timeout") {
            task_fields.push(ToolField::new("Timeout", format!("{timeout}ms")));
        }
    }
    fields::render_fields(task_fields)
}

fn render_stop_content(tc: &ToolCallInfo) -> Vec<Line<'static>> {
    let input = typed::input_object(tc);
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
    typed::render_processed_text_blocks(tc, |stripped| {
        typed::non_empty_trimmed_lines(stripped).map(|line| render_text_line(tc, line)).collect()
    })
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
    use crate::ui::tool_call::test_support::{rendered_line_texts, tool_call};
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn task_tool_call(
        sdk_tool_name: &str,
        raw_input: serde_json::Value,
        content: Option<&str>,
    ) -> ToolCallInfo {
        tool_call(
            "tc-task",
            "tc-task",
            sdk_tool_name,
            raw_input,
            content,
            model::ToolCallStatus::Completed,
        )
    }

    fn task_tool_call_title(tc: &ToolCallInfo) -> Line<'static> {
        super::super::standard::render_tool_call_title(
            tc,
            super::super::ToolCallRenderContext::default(),
            80,
            crate::ui::SpinnerState::Animated(0),
        )
    }

    #[test]
    fn task_state_indicators_remain_distinct_from_the_invocations_outcome() {
        let mut indicators = Vec::new();
        for (name, input, title) in [
            ("TaskCreate", json!({"subject":"Alpha"}), "Task alpha"),
            ("TaskUpdate", json!({"taskId":"beta","status":"in_progress"}), "Task beta"),
            ("TaskUpdate", json!({"taskId":"gamma","status":"completed"}), "Task gamma"),
            ("TaskUpdate", json!({"taskId":"delta","status":"running"}), "Task delta"),
        ] {
            let mut tc = task_tool_call(name, input, None);
            tc.title = title.into();
            let line = task_tool_call_title(&tc);
            let text = rendered_line_texts(&[line])[0].clone();
            assert!(text.contains(title));
            let before_title = text.split_once(title).expect("associated title").0;
            let indicator =
                before_title.split_whitespace().last().expect("visible task state").to_owned();
            indicators.push(indicator.clone());
            tc.status = model::ToolCallStatus::Pending;
            let pending = rendered_line_texts(&[task_tool_call_title(&tc)])[0].clone();
            assert!(
                pending.split_once(title).unwrap().0.contains(&indicator),
                "requested task state survives a different invocation state"
            );
            assert_ne!(pending, text, "pending and completed invocations remain distinguishable");
        }
        assert_ne!(indicators[0], indicators[1]);
        assert_ne!(indicators[1], indicators[2]);
        assert_ne!(indicators[0], indicators[2]);
        assert_eq!(indicators[1], indicators[3]);
        let mut deletion = task_tool_call(
            "TaskUpdate",
            json!({"taskId":"delete-target","status":"deleted"}),
            None,
        );
        deletion.title = "Delete target record".into();
        let success = task_card(&mut deletion, 80);
        assert!(rendered_line_texts(&success)[0].contains("Delete target record"));
        assert!(
            rendered_line_texts(&success)[1..].iter().any(|row| row.contains("Deleted task")),
            "successful empty result has a meaningful fallback"
        );
        for status in [model::ToolCallStatus::Failed, model::ToolCallStatus::Killed] {
            let mut failed = task_tool_call(
                "TaskUpdate",
                json!({"taskId":"delete-target","status":"deleted"}),
                None,
            );
            failed.title = "Delete target record".into();
            failed.status = status;
            let failure = task_card(&mut failed, 80);
            assert_ne!(failure[0], success[0], "invocation failure is visible");
            assert!(
                !rendered_line_texts(&failure)[1..].iter().any(|row| row.contains("Deleted task"))
            );
        }
    }

    fn task_card(tc: &mut ToolCallInfo, width: u16) -> Vec<Line<'static>> {
        let mut lines = vec![];
        super::super::render_tool_call_cached(
            tc,
            super::super::ToolCallRenderContext::default(),
            width,
            crate::ui::SpinnerState::Static,
            &mut lines,
        );
        lines
    }

    #[test]
    fn task_cards_keep_structured_input_and_normalized_output_in_their_own_rows() {
        let cases = [
            (
                "TaskCreate",
                json!({"subject":"Alpha","description":"Independent create description","metadata":{"phase":"phase-sentinel"}}),
                None,
                vec![
                    ("Description", "Independent create description"),
                    ("Metadata", "phase-sentinel"),
                ],
            ),
            (
                "TaskOutput",
                json!({"task_id":"output-target","block":true,"timeout":1700}),
                Some("Retrieval status: not ready\nTask type: local bash\nStatus: running"),
                vec![
                    ("Task ID", "output-target"),
                    ("Block", "yes"),
                    ("Timeout", "1700ms"),
                    ("Retrieval status", "not ready"),
                    ("Task type", "local bash"),
                    ("Status", "In Progress"),
                ],
            ),
            (
                "TaskOutput",
                json!({"task_id":"instant-target","block":false,"timeout":0}),
                Some("Status: completed"),
                vec![
                    ("Task ID", "instant-target"),
                    ("Block", "no"),
                    ("Timeout", "0ms"),
                    ("Status", "Completed"),
                ],
            ),
            (
                "TaskStop",
                json!({"task_id":"stop-target","shell_id":"ignored-shell"}),
                Some(
                    "Message: Process stopped\nTask ID: stop-target\nTask type: bash\nCommand: echo 'exact command'",
                ),
                vec![
                    ("Task ID", "stop-target"),
                    ("Message", "Process stopped"),
                    ("Task type", "bash"),
                    ("Command", "echo 'exact command'"),
                ],
            ),
            (
                "TaskStop",
                json!({"shell_id":"shell-target"}),
                Some("Message: Shell stopped"),
                vec![("Shell ID", "shell-target"), ("Message", "Shell stopped")],
            ),
            (
                "TaskGet",
                json!({"taskId":"missing-target"}),
                Some("Task not found"),
                vec![("Task ID", "missing-target")],
            ),
        ];
        for (name, input, output, expected) in cases {
            assert_task_fields(name, input, output, &expected);
        }
    }

    fn assert_task_fields(
        name: &str,
        input: serde_json::Value,
        output: Option<&str>,
        expected: &[(&str, &str)],
    ) {
        let mut tc = task_tool_call(name, input, output);
        tc.title = format!("{name} selected invocation");
        let rows = rendered_line_texts(&task_card(&mut tc, 100));
        assert!(rows[0].contains(&format!("{name} selected invocation")));
        let body = &rows[1..];
        for &(label, value) in expected {
            assert!(
                body.iter().any(|row| row.contains(&format!("{label}:")) && row.contains(value)),
                "{name} {label}: {body:?}"
            );
        }
        if name == "TaskStop" && output.is_some_and(|text| text.contains("Task ID")) {
            let ids: Vec<_> = body
                .iter()
                .enumerate()
                .filter(|(_, row)| row.contains("Task ID:") && row.contains("stop-target"))
                .map(|(i, _)| i)
                .collect();
            let message = body.iter().position(|row| row.contains("Message:")).unwrap();
            assert!(
                ids.iter().any(|i| *i < message) && ids.iter().any(|i| *i > message),
                "input and result each retain their target"
            );
            assert!(!body.iter().any(|row| row.contains("ignored-shell")));
        }
        if name == "TaskGet" {
            assert!(
                body.iter().any(|row| row.contains("Task not found")),
                "normalized not-found outcome"
            );
        }
        if name == "TaskCreate" {
            assert!(
                !body.iter().any(|row| row.contains("Task not found") || row.contains("Message:"))
            );
            let metadata = body
                .iter()
                .find_map(|row| row.split_once("Metadata:").map(|(_, json)| json.trim()))
                .unwrap();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(metadata).unwrap(),
                json!({"phase":"phase-sentinel"})
            );
        }
    }

    #[test]
    fn normalized_task_lists_preserve_state_omissions_and_clipped_source_tail() {
        let mut tc = task_tool_call(
            "TaskList",
            json!({}),
            Some("...\n\u{25a1} Pending alpha\n\u{25a3} Active beta\n\u{25a0} Finished gamma\n..."),
        );
        let wide = task_card(&mut tc, 90);
        let rows = rendered_line_texts(&wide);
        let pending = rows.iter().position(|row| row.contains("Pending alpha")).unwrap();
        let active = rows.iter().position(|row| row.contains("Active beta")).unwrap();
        let complete = rows.iter().position(|row| row.contains("Finished gamma")).unwrap();
        assert!(pending < active && active < complete);
        assert!(
            rows[pending].contains('\u{25a1}')
                && rows[active].contains('\u{25a3}')
                && rows[complete].contains('\u{25a0}')
        );
        assert!(rows[1..pending].iter().any(|row| row.contains("...")));
        assert!(rows[complete + 1..].iter().any(|row| row.contains("...")));
        let long = format!(
            "...\n\u{25a1} {}\n\u{25a3} {}\n\u{25a0} FINAL result\n...",
            "wide \u{754c} entry ".repeat(12),
            "other \u{754c} entry ".repeat(12)
        );
        let mut tc = task_tool_call("TaskList", json!({}), Some(&long));
        let narrow = task_card(&mut tc, 24);
        assert!(
            narrow.iter().all(|line| line.width() <= 24),
            "physical columns stay within the card"
        );
        let rows = rendered_line_texts(&narrow);
        assert!(
            rows.iter().any(|row| row.contains("source lines")),
            "additional physical clipping is disclosed: {rows:?}"
        );
        assert!(
            rows.iter().any(|row| row.contains("FINAL result")),
            "source tail remains accessible: {rows:?}"
        );
        assert!(rows.last().unwrap().contains("..."));
        assert!(
            rows.iter().any(|row| row.contains('\u{754c}')),
            "wide glyphs are actually visible in the clipped card"
        );
        let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 30, 12));
        for y in 0..12 {
            buffer[(25, y)].set_symbol("|");
        }
        ratatui::widgets::Widget::render(
            ratatui::widgets::Paragraph::new(narrow),
            ratatui::layout::Rect::new(0, 0, 24, 11),
            &mut buffer,
        );
        assert!((0..12).all(|y| buffer[(25, y)].symbol() == "|"), "neighbor columns are untouched");
    }
}
