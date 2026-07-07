// SPDX-License-Identifier: Apache-2.0

//! Rendering helpers for SDK `Artifact` tool calls.

use crate::app::ToolCallInfo;
use ratatui::text::Line;

use super::fields::{self, ToolField};
use super::typed;

pub(super) fn is_artifact_tool(tc: &ToolCallInfo) -> bool {
    tc.sdk_tool_name == "Artifact"
}

pub(super) fn has_structured_body(tc: &ToolCallInfo) -> bool {
    tc.raw_input.is_some() || !tc.content.is_empty()
}

pub(super) fn render_tool_content(tc: &ToolCallInfo) -> Vec<Line<'static>> {
    let mut lines = render_input_content(tc);
    lines.extend(typed::render_json_or_text_blocks(tc, render_output_object));
    lines
}

fn render_input_content(tc: &ToolCallInfo) -> Vec<Line<'static>> {
    let input = typed::input_object(tc);
    let mut artifact_fields = Vec::new();

    if let Some(input) = input {
        if let Some(label) = typed::json_string(input, "label") {
            artifact_fields.push(ToolField::new("Label", label));
        }
        if let Some(description) = typed::json_string(input, "description") {
            artifact_fields.push(ToolField::new("Description", description));
        }
        if let Some(file_path) = typed::json_string(input, "file_path") {
            artifact_fields.push(ToolField::new("File path", file_path));
        }
        if let Some(url) = typed::json_string(input, "url") {
            artifact_fields.push(ToolField::new("URL", url));
        }
    }

    fields::render_fields(artifact_fields)
}

fn render_output_object(object: &serde_json::Map<String, serde_json::Value>) -> Vec<Line<'static>> {
    let mut artifact_fields = Vec::new();
    if let Some(title) = typed::json_string(object, "title") {
        artifact_fields.push(ToolField::new("Title", title));
    }
    if let Some(url) = typed::json_string(object, "url") {
        artifact_fields.push(ToolField::new("URL", url));
    }
    if let Some(path) = typed::json_string(object, "path") {
        artifact_fields.push(ToolField::new("Path", path));
    }
    if let Some(version) = typed::json_string(object, "version") {
        artifact_fields.push(ToolField::new("Version", version));
    }
    if let Some(mcp_dropped) = typed::json_string(object, "mcpDropped") {
        artifact_fields.push(ToolField::new("MCP dropped", mcp_dropped));
    }
    fields::render_fields(artifact_fields)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::model;
    use crate::app::{BlockCache, TerminalSnapshotMode};
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn artifact_tool_call(raw_input: serde_json::Value, content: Option<&str>) -> ToolCallInfo {
        let mut tc = ToolCallInfo {
            id: "tc-artifact".to_owned(),
            source_message_uuids: Vec::new(),
            title: "Artifact: dashboard".to_owned(),
            sdk_tool_name: "Artifact".to_owned(),
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

    #[test]
    fn renders_input_and_output_fields() {
        let tc = artifact_tool_call(
            json!({
                "file_path": "C:/work/dashboard.html",
                "label": "dashboard",
                "description": "Interactive dashboard",
                "url": "https://artifact.local/old"
            }),
            Some(
                r#"{"url":"https://artifact.local/new","path":"C:/work/dashboard.html","title":"Dashboard","version":"v2"}"#,
            ),
        );

        let lines = render_tool_content(&tc);

        assert_eq!(
            rendered_line_texts(&lines),
            vec![
                "Label: dashboard",
                "Description: Interactive dashboard",
                "File path: C:/work/dashboard.html",
                "URL: https://artifact.local/old",
                "Title: Dashboard",
                "URL: https://artifact.local/new",
                "Path: C:/work/dashboard.html",
                "Version: v2",
            ]
        );
    }
}
