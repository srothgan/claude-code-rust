// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

//! Rendering helpers for SDK worktree context tools.

use crate::app::ToolCallInfo;
use ratatui::text::Line;

use super::fields::{self, ToolField};
use super::typed;

pub(super) fn is_worktree_tool(tc: &ToolCallInfo) -> bool {
    matches!(tc.sdk_tool_name.as_str(), "EnterWorktree" | "ExitWorktree")
}

pub(super) fn has_structured_body(tc: &ToolCallInfo) -> bool {
    match tc.sdk_tool_name.as_str() {
        "EnterWorktree" => typed::input_string(tc, "path").is_some() || !tc.content.is_empty(),
        "ExitWorktree" => {
            typed::input_string(tc, "action").is_some()
                || typed::input_bool(tc, "discard_changes") == Some(true)
                || !tc.content.is_empty()
        }
        _ => false,
    }
}

pub(super) fn render_tool_content(tc: &ToolCallInfo) -> Vec<Line<'static>> {
    let mut lines = match tc.sdk_tool_name.as_str() {
        "EnterWorktree" => render_enter_content(tc),
        "ExitWorktree" => render_exit_content(tc),
        _ => return Vec::new(),
    };
    lines.extend(typed::render_stripped_text_blocks(tc, render_text_line));
    lines
}

fn render_enter_content(tc: &ToolCallInfo) -> Vec<Line<'static>> {
    let fields = typed::input_string(tc, "path").map(|path| ToolField::new("Path", path));
    fields::render_fields(fields)
}

fn render_exit_content(tc: &ToolCallInfo) -> Vec<Line<'static>> {
    let mut fields = Vec::new();
    if let Some(action) = typed::input_string(tc, "action") {
        fields.push(ToolField::new("Action", action));
    }
    if typed::input_bool(tc, "discard_changes") == Some(true) {
        fields.push(ToolField::new("Discard changes", "yes"));
    }
    fields::render_fields(fields)
}

fn render_text_line(line: &str) -> Line<'static> {
    typed::render_colon_field_line(line, field_label, |_, value| value.to_owned())
}

fn field_label(label: &str) -> Option<&'static str> {
    match label {
        "Action" => Some("Action"),
        "Branch" => Some("Branch"),
        "Discard changes" => Some("Discard changes"),
        "Path" => Some("Path"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::model;
    use crate::ui::theme;
    use crate::ui::tool_call::test_support::{rendered_line_texts, tool_call};
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn worktree_tool_call(
        sdk_tool_name: &str,
        raw_input: serde_json::Value,
        content: Option<&str>,
    ) -> ToolCallInfo {
        tool_call(
            "tc-worktree",
            "tc-worktree",
            sdk_tool_name,
            raw_input,
            content,
            model::ToolCallStatus::Completed,
        )
    }

    #[test]
    fn enter_input_body_renders_path_field() {
        let tc = worktree_tool_call(
            "EnterWorktree",
            json!({ "path": "C:\\repo\\.worktrees\\feature" }),
            None,
        );

        let lines = render_tool_content(&tc);

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

        let lines = render_tool_content(&tc);

        assert_eq!(rendered_line_texts(&lines), vec!["Action: remove", "Discard changes: yes"]);
        assert_eq!(lines[0].spans[0].style.fg, Some(theme::DIM));
        assert_eq!(lines[1].spans[0].style.fg, Some(theme::DIM));
    }

    #[test]
    fn output_body_reuses_field_formatting() {
        let tc = worktree_tool_call("EnterWorktree", json!({}), Some("Branch: feature"));

        let lines = render_tool_content(&tc);

        assert_eq!(rendered_line_texts(&lines), vec!["Branch: feature"]);
        assert_eq!(lines[0].spans[0].style.fg, Some(theme::DIM));
    }
}
