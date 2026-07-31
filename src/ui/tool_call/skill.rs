// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

//! Rendering helpers for SDK `Skill` tool calls.

use crate::agent::model;
use crate::app::ToolCallInfo;
use crate::ui::diff::strip_outer_code_fence;
use ratatui::text::Line;

use super::typed;

pub(super) fn is_skill_tool(tc: &ToolCallInfo) -> bool {
    tc.sdk_tool_name == "Skill"
}

pub(super) fn has_structured_body(tc: &ToolCallInfo) -> bool {
    invocation_text(tc).is_some() || typed::text_blocks(tc).any(|text| has_visible_output(tc, text))
}

pub(super) fn render_tool_content(tc: &ToolCallInfo) -> Vec<Line<'static>> {
    let mut lines = invocation_text(tc).map_or_else(Vec::new, render_plain_lines);
    lines.extend(typed::render_processed_text_blocks(tc, |text| {
        if is_redundant_launch_message(tc, text) { Vec::new() } else { render_plain_lines(text) }
    }));
    lines
}

fn invocation_text(tc: &ToolCallInfo) -> Option<&str> {
    typed::input_string(tc, "args").map(str::trim).filter(|args| !args.is_empty())
}

fn render_plain_lines(text: &str) -> Vec<Line<'static>> {
    typed::non_empty_trimmed_lines(text.trim()).map(|line| Line::from(line.to_owned())).collect()
}

fn has_visible_output(tc: &ToolCallInfo, text: &str) -> bool {
    let stripped = strip_outer_code_fence(text);
    !stripped.trim().is_empty() && !is_redundant_launch_message(tc, &stripped)
}

fn is_redundant_launch_message(tc: &ToolCallInfo, text: &str) -> bool {
    if !matches!(tc.status, model::ToolCallStatus::Completed) {
        return false;
    }
    let Some(skill_name) = typed::input_string(tc, "skill").map(str::trim) else {
        return false;
    };
    text.trim() == format!("Launching skill: {skill_name}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::tool_call::test_support::{rendered_line_texts, tool_call};
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn skill_tool_call(
        raw_input: serde_json::Value,
        content: Option<&str>,
        status: model::ToolCallStatus,
    ) -> ToolCallInfo {
        tool_call("tc-skill", "Skill", "Skill: review", raw_input, content, status)
    }

    #[test]
    fn successful_inline_skill_has_no_redundant_body() {
        let tc = skill_tool_call(
            json!({ "skill": "frontend-design:frontend-design" }),
            Some("Launching skill: frontend-design:frontend-design"),
            model::ToolCallStatus::Completed,
        );

        assert!(!has_structured_body(&tc));
        assert!(render_tool_content(&tc).is_empty());
    }

    #[test]
    fn invocation_text_renders_without_an_internal_field_label() {
        let tc = skill_tool_call(
            json!({
                "skill": "code-review",
                "args": "high src/foo.ts\nfocus on error handling"
            }),
            None,
            model::ToolCallStatus::InProgress,
        );

        assert!(has_structured_body(&tc));
        assert_eq!(
            rendered_line_texts(&render_tool_content(&tc)),
            vec!["high src/foo.ts", "focus on error handling"]
        );
    }

    #[test]
    fn unknown_and_failed_outputs_are_preserved() {
        let completed = skill_tool_call(
            json!({ "skill": "review" }),
            Some("Future structured skill result"),
            model::ToolCallStatus::Completed,
        );
        let failed = skill_tool_call(
            json!({ "skill": "review" }),
            Some("Launching skill: review"),
            model::ToolCallStatus::Failed,
        );

        assert_eq!(
            rendered_line_texts(&render_tool_content(&completed)),
            vec!["Future structured skill result"]
        );
        assert_eq!(
            rendered_line_texts(&render_tool_content(&failed)),
            vec!["Launching skill: review"]
        );
    }
}
