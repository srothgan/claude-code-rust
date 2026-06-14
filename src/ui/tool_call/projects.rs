// SPDX-License-Identifier: Apache-2.0

//! Rendering helpers for SDK `Projects` tool calls.

use crate::agent::model;
use crate::app::ToolCallInfo;
use crate::ui::diff::strip_outer_code_fence;
use ratatui::text::Line;

use super::errors::render_failed_tool_text_content;
use super::fields::{self, ToolField};

pub(super) fn is_projects_tool(tc: &ToolCallInfo) -> bool {
    tc.sdk_tool_name == "Projects"
}

pub(super) fn has_structured_body(tc: &ToolCallInfo) -> bool {
    tc.raw_input.is_some() || !tc.content.is_empty()
}

pub(super) fn render_tool_content(tc: &ToolCallInfo) -> Option<Vec<Line<'static>>> {
    if !is_projects_tool(tc) {
        return None;
    }

    let mut lines = render_input_content(tc);
    lines.extend(render_text_content(tc));
    Some(lines)
}

fn render_input_content(tc: &ToolCallInfo) -> Vec<Line<'static>> {
    let input = tc.raw_input.as_ref().and_then(serde_json::Value::as_object);
    let mut project_fields = Vec::new();

    if let Some(input) = input {
        if let Some(method) = json_string(input, "method") {
            project_fields.push(ToolField::new("Method", project_method_label(method)));
        }
        if let Some(path) = json_string(input, "path") {
            project_fields.push(ToolField::new("Path", path));
        }
        if let Some(query) = json_string(input, "query") {
            project_fields.push(ToolField::new("Query", query));
        }
        if let Some(local_path) = json_string(input, "local_path") {
            project_fields.push(ToolField::new("Local path", local_path));
        }
        if let Some(n) = json_i64(input, "n") {
            project_fields.push(ToolField::new("Limit", n.to_string()));
        }
        if let Some(force) = json_bool(input, "force") {
            project_fields.push(ToolField::new("Force", boolean_label(force)));
        }
    }

    fields::render_fields(project_fields)
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
    let mut project_fields = Vec::new();
    if let Some(method) = json_string(object, "method") {
        project_fields.push(ToolField::new("Method", project_method_label(method)));
    }
    if let Some(notice) = json_string(object, "notice") {
        project_fields.push(ToolField::new("Notice", notice));
    }

    match json_string(object, "method") {
        Some("project_info") => render_project_info_fields(object, &mut project_fields),
        Some("project_read") => render_project_read_fields(object, &mut project_fields),
        Some("project_search") => render_project_search_fields(object, &mut project_fields),
        Some("project_write") => render_project_write_fields(object, &mut project_fields),
        Some("project_delete") => render_project_delete_fields(object, &mut project_fields),
        _ => {}
    }

    fields::render_fields(project_fields)
}

fn render_project_info_fields<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    project_fields: &mut Vec<ToolField<'a>>,
) {
    if let Some(name) = json_string(object, "name") {
        project_fields.push(ToolField::new("Name", name.to_owned()));
    }
    if let Some(description) = json_string(object, "description") {
        project_fields.push(ToolField::new("Description", description.to_owned()));
    }
    if let Some(docs) = json_array_len(object, "docs") {
        project_fields.push(ToolField::new("Docs", docs.to_string()));
    }
    if let Some(files) = json_array_len(object, "files") {
        project_fields.push(ToolField::new("Files", files.to_string()));
    }
    if let Some(knowledge) = object.get("knowledge").and_then(compact_json) {
        project_fields.push(ToolField::new("Knowledge", knowledge));
    }
}

fn render_project_read_fields<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    project_fields: &mut Vec<ToolField<'a>>,
) {
    if let Some(path) = json_string(object, "path") {
        project_fields.push(ToolField::new("Path", path.to_owned()));
    }
    if let Some(file_kind) = json_string(object, "file_kind") {
        project_fields.push(ToolField::new("File kind", file_kind.to_owned()));
    }
    if let Some(local_file) = json_string(object, "local_file") {
        project_fields.push(ToolField::new("Local file", local_file.to_owned()));
    }
    if let Some(content) = json_string(object, "content") {
        project_fields.push(ToolField::new("Content", content.to_owned()));
    }
}

fn render_project_search_fields<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    project_fields: &mut Vec<ToolField<'a>>,
) {
    if let Some(rag) = json_bool(object, "rag") {
        project_fields.push(ToolField::new("RAG", boolean_label(rag)));
    }
    if let Some(hits) = json_array_len(object, "hits") {
        project_fields.push(ToolField::new("Hits", hits.to_string()));
    }
    if let Some(docs) = json_string_array(object.get("docs")) {
        project_fields.push(ToolField::new("Docs", docs.join(", ")));
    }
}

fn render_project_write_fields<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    project_fields: &mut Vec<ToolField<'a>>,
) {
    if let Some(path) = json_string(object, "path") {
        project_fields.push(ToolField::new("Path", path.to_owned()));
    }
    if let Some(doc_uuid) = json_string(object, "doc_uuid") {
        project_fields.push(ToolField::new("Doc UUID", doc_uuid.to_owned()));
    }
    if let Some(replaced) = json_bool(object, "replaced") {
        project_fields.push(ToolField::new("Replaced", boolean_label(replaced)));
    }
}

fn render_project_delete_fields<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    project_fields: &mut Vec<ToolField<'a>>,
) {
    if let Some(path) = json_string(object, "path") {
        project_fields.push(ToolField::new("Path", path.to_owned()));
    }
    if let Some(deleted) = json_bool(object, "deleted") {
        project_fields.push(ToolField::new("Deleted", boolean_label(deleted)));
    }
}

fn project_method_label(method: &str) -> String {
    method.strip_prefix("project_").unwrap_or(method).replace('_', " ")
}

fn boolean_label(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn compact_json(value: &serde_json::Value) -> Option<String> {
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
    if value.as_object().is_some_and(serde_json::Map::is_empty) {
        return None;
    }
    serde_json::to_string(value).ok()
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

fn json_array_len(object: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<usize> {
    object.get(key).and_then(serde_json::Value::as_array).map(Vec::len)
}

fn json_string_array(value: Option<&serde_json::Value>) -> Option<Vec<String>> {
    let values = value?
        .as_array()?
        .iter()
        .filter_map(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    (!values.is_empty()).then_some(values)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{BlockCache, TerminalSnapshotMode};
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn projects_tool_call(raw_input: serde_json::Value, content: Option<&str>) -> ToolCallInfo {
        let mut tc = ToolCallInfo {
            id: "tc-projects".to_owned(),
            source_message_uuids: Vec::new(),
            title: "Projects: search rendering".to_owned(),
            sdk_tool_name: "Projects".to_owned(),
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
    fn renders_search_input_and_output_fields() {
        let tc = projects_tool_call(
            json!({
                "method": "project_search",
                "query": "rendering",
                "n": 2
            }),
            Some(
                r#"{"method":"project_search","rag":true,"hits":[{"name":"notes"}],"docs":["docs/rendering.md"]}"#,
            ),
        );

        let lines = render_tool_content(&tc).expect("projects content");

        assert_eq!(
            rendered_line_texts(&lines),
            vec![
                "Method: search",
                "Query: rendering",
                "Limit: 2",
                "Method: search",
                "RAG: yes",
                "Hits: 1",
                "Docs: docs/rendering.md",
            ]
        );
    }

    #[test]
    fn renders_write_input_and_output_fields() {
        let tc = projects_tool_call(
            json!({
                "method": "project_write",
                "path": "claude/migration.md",
                "local_path": "tmp/migration.md",
                "force": true
            }),
            Some(
                r#"{"method":"project_write","path":"claude/migration.md","doc_uuid":"doc-1","replaced":false}"#,
            ),
        );

        let lines = render_tool_content(&tc).expect("projects content");

        assert_eq!(
            rendered_line_texts(&lines),
            vec![
                "Method: write",
                "Path: claude/migration.md",
                "Local path: tmp/migration.md",
                "Force: yes",
                "Method: write",
                "Path: claude/migration.md",
                "Doc UUID: doc-1",
                "Replaced: no",
            ]
        );
    }
}
