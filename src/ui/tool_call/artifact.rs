// SPDX-License-Identifier: Apache-2.0

//! Rendering helpers for SDK `Artifact` tool calls.

use crate::agent::model;
use crate::app::ToolCallInfo;
use crate::ui::diff::strip_outer_code_fence;
use ratatui::text::Line;

use super::errors::render_failed_tool_text_content;
use super::fields::{self, ToolField};

pub(super) fn is_artifact_tool(tc: &ToolCallInfo) -> bool {
    tc.sdk_tool_name == "Artifact"
}

pub(super) fn has_structured_body(tc: &ToolCallInfo) -> bool {
    tc.raw_input.is_some() || !tc.content.is_empty()
}

pub(super) fn render_tool_content(tc: &ToolCallInfo) -> Option<Vec<Line<'static>>> {
    if !is_artifact_tool(tc) {
        return None;
    }

    let mut lines = render_input_content(tc);
    lines.extend(render_text_content(tc));
    Some(lines)
}

fn render_input_content(tc: &ToolCallInfo) -> Vec<Line<'static>> {
    let input = tc.raw_input.as_ref().and_then(serde_json::Value::as_object);
    let mut artifact_fields = Vec::new();

    if let Some(input) = input {
        if let Some(label) = json_string(input, "label") {
            artifact_fields.push(ToolField::new("Label", label));
        }
        if let Some(file_path) = json_string(input, "file_path") {
            artifact_fields.push(ToolField::new("File path", file_path));
        }
        if let Some(url) = json_string(input, "url") {
            artifact_fields.push(ToolField::new("URL", url));
        }
    }

    fields::render_fields(artifact_fields)
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
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&stripped)
        && let Some(object) = value.as_object()
    {
        lines.extend(render_output_object(object));
        return;
    }
    lines.extend(
        stripped
            .lines()
            .map(str::trim_end)
            .filter(|line| !line.trim().is_empty())
            .map(|line| Line::from(line.to_owned())),
    );
}

fn render_output_object(object: &serde_json::Map<String, serde_json::Value>) -> Vec<Line<'static>> {
    let mut artifact_fields = Vec::new();
    if let Some(title) = json_string(object, "title") {
        artifact_fields.push(ToolField::new("Title", title));
    }
    if let Some(url) = json_string(object, "url") {
        artifact_fields.push(ToolField::new("URL", url));
    }
    if let Some(path) = json_string(object, "path") {
        artifact_fields.push(ToolField::new("Path", path));
    }
    if let Some(version) = json_string(object, "version") {
        artifact_fields.push(ToolField::new("Version", version));
    }
    if let Some(mcp_dropped) = json_string(object, "mcpDropped") {
        artifact_fields.push(ToolField::new("MCP dropped", mcp_dropped));
    }
    fields::render_fields(artifact_fields)
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
                "url": "https://artifact.local/old"
            }),
            Some(
                r#"{"url":"https://artifact.local/new","path":"C:/work/dashboard.html","title":"Dashboard","version":"v2"}"#,
            ),
        );

        let lines = render_tool_content(&tc).expect("artifact content");

        assert_eq!(
            rendered_line_texts(&lines),
            vec![
                "Label: dashboard",
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
