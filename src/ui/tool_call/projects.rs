// SPDX-License-Identifier: Apache-2.0

//! Rendering helpers for SDK `Projects` tool calls.

use crate::app::ToolCallInfo;
use ratatui::text::Line;

use super::fields::{self, ToolField};
use super::typed;

pub(super) fn is_projects_tool(tc: &ToolCallInfo) -> bool {
    tc.sdk_tool_name == "Projects"
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
    let mut project_fields = Vec::new();

    if let Some(input) = input {
        if let Some(method) = typed::json_string(input, "method") {
            project_fields.push(ToolField::new("Method", project_method_label(method)));
        }
        if let Some(path) = typed::json_string(input, "path") {
            project_fields.push(ToolField::new("Path", path));
        }
        if let Some(query) = typed::json_string(input, "query") {
            project_fields.push(ToolField::new("Query", query));
        }
        if let Some(local_path) = typed::json_string(input, "local_path") {
            project_fields.push(ToolField::new("Local path", local_path));
        }
        if let Some(n) = typed::json_i64(input, "n") {
            project_fields.push(ToolField::new("Limit", n.to_string()));
        }
        if let Some(force) = typed::json_bool(input, "force") {
            project_fields.push(ToolField::new("Force", typed::bool_label(force)));
        }
    }

    fields::render_fields(project_fields)
}

fn render_output_object(object: &serde_json::Map<String, serde_json::Value>) -> Vec<Line<'static>> {
    let mut project_fields = Vec::new();
    if let Some(method) = typed::json_string(object, "method") {
        project_fields.push(ToolField::new("Method", project_method_label(method)));
    }
    if let Some(notice) = typed::json_string(object, "notice") {
        project_fields.push(ToolField::new("Notice", notice));
    }

    match typed::json_string(object, "method") {
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
    if let Some(name) = typed::json_string(object, "name") {
        project_fields.push(ToolField::new("Name", name.to_owned()));
    }
    if let Some(description) = typed::json_string(object, "description") {
        project_fields.push(ToolField::new("Description", description.to_owned()));
    }
    if let Some(docs) = typed::json_array_len(object, "docs") {
        project_fields.push(ToolField::new("Docs", docs.to_string()));
    }
    if let Some(files) = typed::json_array_len(object, "files") {
        project_fields.push(ToolField::new("Files", files.to_string()));
    }
    if let Some(knowledge) = object.get("knowledge").and_then(typed::non_empty_compact_json) {
        project_fields.push(ToolField::new("Knowledge", knowledge));
    }
}

fn render_project_read_fields<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    project_fields: &mut Vec<ToolField<'a>>,
) {
    if let Some(path) = typed::json_string(object, "path") {
        project_fields.push(ToolField::new("Path", path.to_owned()));
    }
    if let Some(file_kind) = typed::json_string(object, "file_kind") {
        project_fields.push(ToolField::new("File kind", file_kind.to_owned()));
    }
    if let Some(local_file) = typed::json_string(object, "local_file") {
        project_fields.push(ToolField::new("Local file", local_file.to_owned()));
    }
    if let Some(content) = typed::json_string(object, "content") {
        project_fields.push(ToolField::new("Content", content.to_owned()));
    }
}

fn render_project_search_fields<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    project_fields: &mut Vec<ToolField<'a>>,
) {
    if let Some(rag) = typed::json_bool(object, "rag") {
        project_fields.push(ToolField::new("RAG", typed::bool_label(rag)));
    }
    if let Some(hits) = typed::json_array_len(object, "hits") {
        project_fields.push(ToolField::new("Hits", hits.to_string()));
    }
    if let Some(docs) = typed::json_string_array(object.get("docs")) {
        project_fields.push(ToolField::new("Docs", docs.join(", ")));
    }
}

fn render_project_write_fields<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    project_fields: &mut Vec<ToolField<'a>>,
) {
    if let Some(path) = typed::json_string(object, "path") {
        project_fields.push(ToolField::new("Path", path.to_owned()));
    }
    if let Some(doc_uuid) = typed::json_string(object, "doc_uuid") {
        project_fields.push(ToolField::new("Doc UUID", doc_uuid.to_owned()));
    }
    if let Some(replaced) = typed::json_bool(object, "replaced") {
        project_fields.push(ToolField::new("Replaced", typed::bool_label(replaced)));
    }
}

fn render_project_delete_fields<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    project_fields: &mut Vec<ToolField<'a>>,
) {
    if let Some(path) = typed::json_string(object, "path") {
        project_fields.push(ToolField::new("Path", path.to_owned()));
    }
    if let Some(deleted) = typed::json_bool(object, "deleted") {
        project_fields.push(ToolField::new("Deleted", typed::bool_label(deleted)));
    }
}

fn project_method_label(method: &str) -> String {
    method.strip_prefix("project_").unwrap_or(method).replace('_', " ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::model;
    use crate::ui::tool_call::test_support::{rendered_line_texts, tool_call};
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn projects_tool_call(raw_input: serde_json::Value, content: Option<&str>) -> ToolCallInfo {
        tool_call(
            "tc-projects",
            "Projects: search rendering",
            "Projects",
            raw_input,
            content,
            model::ToolCallStatus::Completed,
        )
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

        let lines = render_tool_content(&tc);

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

        let lines = render_tool_content(&tc);

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
