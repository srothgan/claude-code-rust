// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

//! Rendering helpers for SDK worktree context tools.

use crate::agent::model;
use crate::app::ToolCallInfo;
use crate::ui::diff::strip_outer_code_fence;
use ratatui::text::{Line, Span};

use super::errors::render_failed_tool_text_content;
use super::fields::{self, ToolField};

pub(super) fn is_worktree_tool(tc: &ToolCallInfo) -> bool {
    matches!(tc.sdk_tool_name.as_str(), "EnterWorktree" | "ExitWorktree")
}

pub(super) fn has_structured_body(tc: &ToolCallInfo) -> bool {
    match tc.sdk_tool_name.as_str() {
        "EnterWorktree" => input_string(tc, "path").is_some() || !tc.content.is_empty(),
        "ExitWorktree" => {
            input_string(tc, "action").is_some()
                || input_bool(tc, "discard_changes") == Some(true)
                || !tc.content.is_empty()
        }
        _ => false,
    }
}

pub(super) fn render_tool_content(tc: &ToolCallInfo) -> Option<Vec<Line<'static>>> {
    let mut lines = match tc.sdk_tool_name.as_str() {
        "EnterWorktree" => render_enter_content(tc),
        "ExitWorktree" => render_exit_content(tc),
        _ => return None,
    };
    lines.extend(render_text_content(tc));
    Some(lines)
}

fn render_enter_content(tc: &ToolCallInfo) -> Vec<Line<'static>> {
    let fields = input_string(tc, "path").map(|path| ToolField::new("Path", path));
    fields::render_fields(fields)
}

fn render_exit_content(tc: &ToolCallInfo) -> Vec<Line<'static>> {
    let mut fields = Vec::new();
    if let Some(action) = input_string(tc, "action") {
        fields.push(ToolField::new("Action", action));
    }
    if input_bool(tc, "discard_changes") == Some(true) {
        fields.push(ToolField::new("Discard changes", "yes"));
    }
    fields::render_fields(fields)
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
    if let Some((label, value)) = line.split_once(':') {
        let label = match label {
            "Action" => "Action",
            "Branch" => "Branch",
            "Discard changes" => "Discard changes",
            "Path" => "Path",
            _ => return Line::from(Span::raw(line.to_owned())),
        };
        return fields::render_field(label, value.trim_start());
    }
    Line::from(Span::raw(line.to_owned()))
}

fn input_string<'a>(tc: &'a ToolCallInfo, key: &str) -> Option<&'a str> {
    tc.raw_input
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .and_then(|input| input.get(key))
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
}

fn input_bool(tc: &ToolCallInfo, key: &str) -> Option<bool> {
    tc.raw_input
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .and_then(|input| input.get(key))
        .and_then(serde_json::Value::as_bool)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{BlockCache, TerminalSnapshotMode};
    use crate::ui::theme;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn worktree_tool_call(
        sdk_tool_name: &str,
        raw_input: serde_json::Value,
        content: Option<&str>,
    ) -> ToolCallInfo {
        let mut tc = ToolCallInfo {
            id: "tc-worktree".to_owned(),
            source_message_uuids: Vec::new(),
            title: "tc-worktree".to_owned(),
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
    fn enter_input_body_renders_path_field() {
        let tc = worktree_tool_call(
            "EnterWorktree",
            json!({ "path": "C:\\repo\\.worktrees\\feature" }),
            None,
        );

        let lines = render_tool_content(&tc).expect("worktree content");

        assert_eq!(rendered_line_texts(&lines), vec!["Path: C:\\repo\\.worktrees\\feature"]);
        assert_eq!(lines[0].spans[0].style.fg, Some(theme::DIM));
    }

    #[test]
    fn exit_input_body_renders_action_and_discard_fields() {
        let tc = worktree_tool_call(
            "ExitWorktree",
            json!({ "action": "remove", "discard_changes": true }),
            None,
        );

        let lines = render_tool_content(&tc).expect("worktree content");

        assert_eq!(rendered_line_texts(&lines), vec!["Action: remove", "Discard changes: yes"]);
        assert_eq!(lines[0].spans[0].style.fg, Some(theme::DIM));
        assert_eq!(lines[1].spans[0].style.fg, Some(theme::DIM));
    }

    #[test]
    fn output_body_reuses_field_formatting() {
        let tc = worktree_tool_call("EnterWorktree", json!({}), Some("Branch: feature"));

        let lines = render_tool_content(&tc).expect("worktree content");

        assert_eq!(rendered_line_texts(&lines), vec!["Branch: feature"]);
        assert_eq!(lines[0].spans[0].style.fg, Some(theme::DIM));
    }
}
