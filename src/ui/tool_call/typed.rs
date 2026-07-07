// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

//! Shared typed SDK tool-call rendering contracts and helpers.

use crate::agent::model;
use crate::app::ToolCallInfo;
use crate::ui::diff::strip_outer_code_fence;
use ratatui::text::{Line, Span};
use serde_json::{Map, Value};

use super::errors::render_failed_tool_text_content;
use super::{
    artifact, cron, fields, monitor, projects, push_notification, remote_trigger, repl,
    schedule_wakeup, tasks, workflow, worktree,
};

pub(super) struct TypedToolRenderer {
    pub(super) name: &'static str,
    pub(super) matches: fn(&ToolCallInfo) -> bool,
    pub(super) has_structured_body: fn(&ToolCallInfo) -> bool,
    pub(super) render: fn(&ToolCallInfo) -> Vec<Line<'static>>,
}

const RENDERERS: &[TypedToolRenderer] = &[
    TypedToolRenderer {
        name: "task-state",
        matches: tasks::is_state_tool,
        has_structured_body: tasks::has_structured_body,
        render: tasks::render_tool_content,
    },
    TypedToolRenderer {
        name: "worktree",
        matches: worktree::is_worktree_tool,
        has_structured_body: worktree::has_structured_body,
        render: worktree::render_tool_content,
    },
    TypedToolRenderer {
        name: "cron",
        matches: cron::is_cron_tool,
        has_structured_body: cron::has_structured_body,
        render: cron::render_tool_content,
    },
    TypedToolRenderer {
        name: "schedule-wakeup",
        matches: schedule_wakeup::is_schedule_wakeup_tool,
        has_structured_body: schedule_wakeup::has_structured_body,
        render: schedule_wakeup::render_tool_content,
    },
    TypedToolRenderer {
        name: "push-notification",
        matches: push_notification::is_push_notification_tool,
        has_structured_body: push_notification::has_structured_body,
        render: push_notification::render_tool_content,
    },
    TypedToolRenderer {
        name: "remote-trigger",
        matches: remote_trigger::is_remote_trigger_tool,
        has_structured_body: remote_trigger::has_structured_body,
        render: remote_trigger::render_tool_content,
    },
    TypedToolRenderer {
        name: "repl",
        matches: repl::is_repl_tool,
        has_structured_body: repl::has_structured_body,
        render: repl::render_tool_content,
    },
    TypedToolRenderer {
        name: "monitor",
        matches: monitor::is_monitor_tool,
        has_structured_body: monitor::has_structured_body,
        render: monitor::render_tool_content,
    },
    TypedToolRenderer {
        name: "workflow",
        matches: workflow::is_workflow_tool,
        has_structured_body: workflow::has_structured_body,
        render: workflow::render_tool_content,
    },
    TypedToolRenderer {
        name: "projects",
        matches: projects::is_projects_tool,
        has_structured_body: projects::has_structured_body,
        render: projects::render_tool_content,
    },
    TypedToolRenderer {
        name: "artifact",
        matches: artifact::is_artifact_tool,
        has_structured_body: artifact::has_structured_body,
        render: artifact::render_tool_content,
    },
];

pub(super) fn renderer_for(tc: &ToolCallInfo) -> Option<&'static TypedToolRenderer> {
    RENDERERS.iter().find(|renderer| {
        debug_assert!(!renderer.name.is_empty());
        (renderer.matches)(tc)
    })
}

pub(super) fn input_object(tc: &ToolCallInfo) -> Option<&Map<String, Value>> {
    tc.raw_input.as_ref().and_then(Value::as_object)
}

pub(super) fn input_string<'a>(tc: &'a ToolCallInfo, key: &str) -> Option<&'a str> {
    input_object(tc).and_then(|input| json_string(input, key))
}

pub(super) fn input_bool(tc: &ToolCallInfo, key: &str) -> Option<bool> {
    input_object(tc).and_then(|input| json_bool(input, key))
}

pub(super) fn json_string<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    object.get(key).and_then(Value::as_str).filter(|value| !value.is_empty())
}

pub(super) fn json_bool(object: &Map<String, Value>, key: &str) -> Option<bool> {
    object.get(key).and_then(Value::as_bool)
}

pub(super) fn json_i64(object: &Map<String, Value>, key: &str) -> Option<i64> {
    object.get(key).and_then(Value::as_i64)
}

pub(super) fn compact_json(value: &Value) -> Option<String> {
    serde_json::to_string(value).ok()
}

pub(super) fn non_empty_compact_json(value: &Value) -> Option<String> {
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
    if value.as_object().is_some_and(Map::is_empty) {
        return None;
    }
    compact_json(value)
}

pub(super) fn json_array_len(object: &Map<String, Value>, key: &str) -> Option<usize> {
    object.get(key).and_then(Value::as_array).map(Vec::len)
}

pub(super) fn json_string_array(value: Option<&Value>) -> Option<Vec<String>> {
    let values = value?
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    (!values.is_empty()).then_some(values)
}

pub(super) fn text_blocks(tc: &ToolCallInfo) -> impl Iterator<Item = &str> {
    tc.content.iter().filter_map(|content| {
        let model::ToolCallContent::Content(content) = content else {
            return None;
        };
        let model::ContentBlock::Text(text) = &content.content else {
            return None;
        };
        Some(text.text.as_str())
    })
}

pub(super) fn render_stripped_text_blocks(
    tc: &ToolCallInfo,
    render_line: fn(&str) -> Line<'static>,
) -> Vec<Line<'static>> {
    render_processed_text_blocks(tc, |stripped| {
        non_empty_trimmed_lines(stripped).map(render_line).collect()
    })
}

pub(super) fn render_processed_text_blocks<F>(
    tc: &ToolCallInfo,
    render_stripped: F,
) -> Vec<Line<'static>>
where
    F: Fn(&str) -> Vec<Line<'static>>,
{
    let mut lines = Vec::new();
    for text in text_blocks(tc) {
        let stripped = strip_outer_code_fence(text);
        if let Some(failed_lines) = render_failed_tool_text_content(tc.status, &stripped) {
            lines.extend(failed_lines);
            continue;
        }
        lines.extend(render_stripped(&stripped));
    }
    lines
}

pub(super) fn render_json_or_text_blocks(
    tc: &ToolCallInfo,
    render_object: fn(&Map<String, Value>) -> Vec<Line<'static>>,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for text in text_blocks(tc) {
        let stripped = strip_outer_code_fence(text);
        if let Some(failed_lines) = render_failed_tool_text_content(tc.status, &stripped) {
            lines.extend(failed_lines);
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(&stripped)
            && let Some(object) = value.as_object()
        {
            lines.extend(render_object(object));
            continue;
        }
        lines.extend(non_empty_trimmed_lines(&stripped).map(|line| Line::from(line.to_owned())));
    }
    lines
}

pub(super) fn non_empty_trimmed_lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines().map(str::trim_end).filter(|line| !line.trim().is_empty())
}

pub(super) fn render_colon_field_line(
    line: &str,
    field_label: fn(&str) -> Option<&'static str>,
    field_value: fn(&str, &str) -> String,
) -> Line<'static> {
    if let Some((label, value)) = line.split_once(':')
        && let Some(label) = field_label(label.trim())
    {
        return fields::render_field(label, field_value(label, value.trim_start()));
    }
    Line::from(Span::raw(line.to_owned()))
}

pub(super) fn bool_label(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

pub(super) fn bool_text_label(value: &str) -> Option<&'static str> {
    match value {
        "true" | "yes" => Some("yes"),
        "false" | "no" => Some("no"),
        _ => None,
    }
}

pub(super) fn format_duration_seconds(seconds: i64) -> String {
    let seconds = u64::try_from(seconds.max(0)).unwrap_or_default();
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let remaining_seconds = seconds % 60;
    let mut parts = Vec::new();

    if hours > 0 {
        parts.push(format!("{hours}h"));
    }
    if minutes > 0 {
        parts.push(format!("{minutes}m"));
    }
    if remaining_seconds > 0 || parts.is_empty() {
        parts.push(format!("{remaining_seconds}s"));
    }

    parts.join(" ")
}
