// SPDX-License-Identifier: Apache-2.0

//! Rendering helpers for SDK `Skill` tool calls.

use crate::app::ToolCallInfo;
use crate::ui::diff::strip_outer_code_fence;
use ratatui::text::Line;

use super::typed;

pub(super) fn is_skill_tool(tc: &ToolCallInfo) -> bool {
    tc.sdk_tool_name == "Skill"
}

pub(super) fn has_structured_body(tc: &ToolCallInfo) -> bool {
    invocation_text(tc).is_some()
        || typed::text_blocks(tc).any(|text| !strip_outer_code_fence(text).trim().is_empty())
}

pub(super) fn render_tool_content(tc: &ToolCallInfo) -> Vec<Line<'static>> {
    let mut lines = invocation_text(tc).map_or_else(Vec::new, render_plain_lines);
    // The bridge omits recognized inline acknowledgements. Any delivered content
    // is a result or fallback, even when it resembles a launch acknowledgement.
    lines.extend(typed::render_processed_text_blocks(tc, render_plain_lines));
    lines
}

fn invocation_text(tc: &ToolCallInfo) -> Option<&str> {
    typed::input_string(tc, "args").map(str::trim).filter(|args| !args.is_empty())
}

fn render_plain_lines(text: &str) -> Vec<Line<'static>> {
    typed::non_empty_trimmed_lines(text.trim()).map(|line| Line::from(line.to_owned())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::model;
    use crate::ui::tool_call::test_support::{card_body, rendered_line_texts, tool_call};
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn skill_tool_call(
        raw_input: serde_json::Value,
        content: Option<&str>,
        status: model::ToolCallStatus,
    ) -> ToolCallInfo {
        tool_call("tc-skill", "Skill: review", "Skill", raw_input, content, status)
    }

    #[test]
    fn recognized_inline_skill_is_compact_but_fallback_and_failures_remain_visible() {
        // The bridge owns classification: recognized inline success has no content.
        // A malformed fork (missing agentId) instead forwards its raw fallback.
        let mut inline =
            skill_tool_call(json!({ "skill": "review" }), None, model::ToolCallStatus::Completed);
        assert!(card_body(&mut inline, 100).is_empty());

        for (skill, output, status) in [
            ("review", "Launching skill: review", model::ToolCallStatus::Completed),
            ("review", "Future structured skill result", model::ToolCallStatus::Completed),
            ("different-skill", "Launching skill: review", model::ToolCallStatus::Completed),
            ("review", "Fork worker found an unsafe path", model::ToolCallStatus::Completed),
            ("review", "Launching skill: review", model::ToolCallStatus::Failed),
        ] {
            let mut tc = skill_tool_call(json!({ "skill": skill }), Some(output), status);
            assert_eq!(card_body(&mut tc, 100), vec![output], "{skill}: {status:?}");
        }
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
}
