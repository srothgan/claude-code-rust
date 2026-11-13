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
        Some("project_memory_read") => {
            render_project_memory_read_fields(object, &mut project_fields);
        }
        _ => {}
    }

    let mut lines = fields::render_fields(project_fields);
    if typed::json_string(object, "method") == Some("project_search") {
        lines.extend(render_project_search_hits(object));
    }
    if typed::json_string(object, "method") == Some("project_read")
        && let Some(content) = typed::json_string(object, "content")
    {
        lines.extend(fields::render_multiline_field("Content", content));
    }
    if typed::json_string(object, "method") == Some("project_memory_read")
        && let Some(content) = typed::json_string(object, "content")
    {
        lines.extend(fields::render_multiline_field("Content", bounded_memory_content(content)));
    }
    if typed::json_string(object, "method") == Some("project_memory_list") {
        lines.extend(render_project_memory_list(object));
    }
    lines
}

fn render_project_memory_read_fields<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    project_fields: &mut Vec<ToolField<'a>>,
) {
    if let Some(path) = typed::json_string(object, "path") {
        project_fields.push(ToolField::new("Path", path.to_owned()));
    }
    if let Some(local_file) = typed::json_string(object, "local_file") {
        project_fields.push(ToolField::new("Local file", local_file.to_owned()));
    }
    if let Some(size) = typed::json_i64(object, "size_bytes") {
        project_fields.push(ToolField::new("Size", format!("{size} bytes")));
    }
    if let Some(updated_at) = typed::json_string(object, "updated_at") {
        project_fields.push(ToolField::new("Updated", updated_at.to_owned()));
    }
    if let Some(truncated) = typed::json_bool(object, "truncated") {
        project_fields.push(ToolField::new("Truncated", typed::bool_label(truncated)));
    }
    if let Some(additional) = additional_memory_json(
        object,
        &[
            "method",
            "notice",
            "path",
            "local_file",
            "size_bytes",
            "updated_at",
            "truncated",
            "content",
        ],
    ) {
        project_fields.push(ToolField::new("Additional memory output", additional));
    }
}

fn render_project_memory_list(
    object: &serde_json::Map<String, serde_json::Value>,
) -> Vec<Line<'static>> {
    const MAX_FILES: usize = 128;
    let mut lines = Vec::new();
    if let Some(truncated) = typed::json_bool(object, "truncated") {
        lines.push(fields::render_field("List truncated", typed::bool_label(truncated)));
    }
    let files = object.get("files").and_then(serde_json::Value::as_array);
    if let Some(files) = files {
        lines.push(fields::render_field("Memory files", files.len().to_string()));
        for (index, file) in files.iter().take(MAX_FILES).enumerate() {
            let Some(file) = file.as_object() else {
                if let Some(value) = typed::non_empty_compact_json(file) {
                    lines.push(fields::render_dynamic_field(
                        format!("Memory file {}", index + 1),
                        value,
                    ));
                }
                continue;
            };
            let path = typed::json_string(file, "path").unwrap_or("<unknown path>");
            let size = typed::json_i64(file, "size_bytes")
                .map(|value| format!("; {value} bytes"))
                .unwrap_or_default();
            let updated = typed::json_string(file, "updated_at").map_or_else(
                || "; updated unknown".to_owned(),
                |value| format!("; updated {value}"),
            );
            let truncated = typed::json_bool(file, "truncated")
                .filter(|value| *value)
                .map(|_| "; truncated".to_owned())
                .unwrap_or_default();
            let additional =
                additional_memory_json(file, &["path", "size_bytes", "updated_at", "truncated"])
                    .map(|value| format!("; additional {value}"))
                    .unwrap_or_default();
            lines.push(fields::render_dynamic_field(
                format!("Memory file {}", index + 1),
                format!("{path}{size}{updated}{truncated}{additional}"),
            ));
        }
        if files.len() > MAX_FILES {
            lines.push(Line::from(format!(
                "… [{} memory files not shown]",
                files.len() - MAX_FILES
            )));
        }
    }
    if let Some(additional) =
        additional_memory_json(object, &["method", "notice", "files", "truncated"])
    {
        lines.push(fields::render_field("Additional memory-list output", additional));
    }
    lines
}

fn bounded_memory_content(content: &str) -> String {
    const MAX_CHARS: usize = 16_384;
    const MAX_LINES: usize = 200;
    let mut lines = 1;
    for (chars, (byte, ch)) in content.char_indices().enumerate() {
        if chars == MAX_CHARS || (ch == '\n' && lines == MAX_LINES && byte + 1 < content.len()) {
            return format!("{}\n… [content truncated for display]", &content[..byte]);
        }
        if ch == '\n' {
            lines += 1;
        }
    }
    content.to_owned()
}

fn additional_memory_json(
    object: &serde_json::Map<String, serde_json::Value>,
    handled: &[&str],
) -> Option<String> {
    let additional = object
        .iter()
        .filter(|(key, _)| !handled.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<serde_json::Map<_, _>>();
    (!additional.is_empty())
        .then(|| typed::compact_json(&serde_json::Value::Object(additional)))
        .flatten()
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
}

fn render_project_search_hits(
    object: &serde_json::Map<String, serde_json::Value>,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if let Some(hits) = object.get("hits").and_then(serde_json::Value::as_array) {
        for hit in hits.iter().filter_map(serde_json::Value::as_object) {
            let parts: Vec<_> = ["name", "doc_uuid", "text"]
                .into_iter()
                .filter_map(|key| typed::json_string(hit, key))
                .collect();
            if !parts.is_empty() {
                lines.extend(fields::render_multiline_field("Hit", parts.join("; ")));
            }
        }
    }
    lines
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
    use crate::ui::tool_call::test_support::{card_body, rendered_line_texts, tool_call};
    use pretty_assertions::assert_eq;
    use serde_json::{Value, json};

    fn projects_tool_call(input: Value, result: &Value) -> ToolCallInfo {
        tool_call(
            "tc-projects",
            "Projects",
            "Projects",
            input,
            Some(&result.to_string()),
            model::ToolCallStatus::Completed,
        )
    }

    fn require_row(rows: &[String], expected: &str) {
        assert!(rows.iter().any(|row| row == expected), "missing {expected:?}: {rows:?}");
    }

    #[test]
    fn renders_search_input_and_output_fields() {
        let mut tc = projects_tool_call(
            json!({"method":"project_search","query":"rendering","n":2}),
            &json!({"method":"project_search","rag":true,"hits":[
                {"name":"layout-notes","doc_uuid":"doc-layout-7","text":"clip only the margin"},
                {"name":"input-notes","doc_uuid":"doc-input-4","text":"retain typed accents"}
            ],"docs":["docs/layout.md","docs/input.md"]}),
        );
        let body = card_body(&mut tc, 120);
        require_row(&body, "Query: rendering");
        require_row(&body, "Limit: 2");
        require_row(&body, "RAG: yes");
        for (name, id, snippet, other) in [
            ("layout-notes", "doc-layout-7", "clip only the margin", "doc-input-4"),
            ("input-notes", "doc-input-4", "retain typed accents", "doc-layout-7"),
        ] {
            let row = body.iter().find(|row| row.contains(name)).expect("visible hit identity");
            assert!(row.contains(id) && row.contains(snippet), "{row}");
            assert!(!row.contains(other), "hit association: {row}");
        }
        require_row(&body, "Docs: docs/layout.md, docs/input.md");
        let mut empty = projects_tool_call(
            json!({"method":"project_search","query":"missing"}),
            &json!({"method":"project_search","rag":false,"hits":[],"docs":[],"notice":"No indexed documents"}),
        );
        let body = card_body(&mut empty, 120);
        require_row(&body, "RAG: no");
        require_row(&body, "Hits: 0");
        require_row(&body, "Notice: No indexed documents");
        assert!(!body.iter().any(|row| row.contains("doc-layout") || row.contains("Hit 1:")));
    }

    #[test]
    fn renders_write_input_and_output_fields() {
        for (input, path, id, replaced) in [
            (
                json!({"method":"project_write","path":"notes.md","local_path":"C:/upload/notes.md"}),
                "claude/notes.md",
                "doc-upload-1",
                false,
            ),
            (
                json!({"method":"project_write","path":"guide.md","content":"PRIVATE-UPLOAD-BODY"}),
                "claude/guide.md",
                "doc-inline-2",
                true,
            ),
        ] {
            let result =
                json!({"method":"project_write","path":path,"doc_uuid":id,"replaced":replaced});
            let mut tc = projects_tool_call(input.clone(), &result);
            let body = card_body(&mut tc, 120);
            require_row(&body, &format!("Path: {path}"));
            require_row(&body, &format!("Doc UUID: {id}"));
            require_row(&body, if replaced { "Replaced: yes" } else { "Replaced: no" });
            if input.get("local_path").is_some() {
                require_row(&body, "Local path: C:/upload/notes.md");
            }
            assert!(!body.iter().any(|row| row.contains("PRIVATE-UPLOAD-BODY")));
        }
    }

    #[test]
    fn renders_project_memory_list_and_read_metadata() {
        let result = json!({"method":"project_memory_list","truncated":true,"files":[
            {"path":"memory/first.md","size_bytes":42,"updated_at":"2026-09-13T10:00:00Z","truncated":true,"future":{"owner":"alpha","enabled":false}},
            {"path":"memory/second.md","size_bytes":73,"updated_at":null,"truncated":false,"future":["beta",2]}
        ]});
        let mut tc = projects_tool_call(json!({"method":"project_memory_list"}), &result);
        let body = card_body(&mut tc, 240);
        require_row(&body, "List truncated: yes");
        for (path, size, updated, truncated, future) in [
            (
                "memory/first.md",
                "42 bytes",
                "2026-09-13T10:00:00Z",
                true,
                json!({"future":{"owner":"alpha","enabled":false}}),
            ),
            (
                "memory/second.md",
                "73 bytes",
                "updated unknown",
                false,
                json!({"future":["beta",2]}),
            ),
        ] {
            let row = body.iter().find(|row| row.contains(path)).expect("memory item");
            assert!(row.contains(size) && row.contains(updated), "{row}");
            assert_eq!(row.contains("; truncated"), truncated);
            let extra = row.split_once("; additional ").expect("unknown metadata value").1;
            assert_eq!(serde_json::from_str::<Value>(extra).unwrap(), future);
        }
        let result = json!({"method":"project_memory_read","path":"memory/returned.md","content":"first\n\n  indented \u{03b2}","size_bytes":20,"updated_at":null,"truncated":false,"future":{"revision":7}});
        let mut read = projects_tool_call(
            json!({"method":"project_memory_read","path":"returned.md"}),
            &result,
        );
        let projection = render_tool_content(&read);
        assert_eq!(content_rows(&projection), vec!["first", "", "  indented \u{03b2}"]);
        let projected = rendered_line_texts(&projection);
        require_row(&projected, "Path: memory/returned.md");
        require_row(&projected, "Truncated: no");
        assert!(!projected.iter().any(|row| row.starts_with("Updated:")));
        let extra = projected
            .iter()
            .find_map(|row| row.strip_prefix("Additional memory output: "))
            .unwrap();
        assert_eq!(serde_json::from_str::<Value>(extra).unwrap(), json!({"future":{"revision":7}}));
        let body = card_body(&mut read, 120);
        assert!(body.iter().any(|row| row.contains("indented \u{03b2}")), "{body:?}");
    }

    // Inspect value spans in the local projection; continuation indentation is presentation.
    fn content_rows(lines: &[Line<'static>]) -> Vec<String> {
        let start = lines
            .iter()
            .position(|line| line.spans.first().is_some_and(|span| span.content == "Content: "))
            .expect("content field");
        lines[start..]
            .iter()
            .map(|line| line.spans.iter().skip(1).map(|span| span.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn memory_list_preview_reports_local_omission_without_changing_received_records() {
        for count in [127, 128, 129] {
            let files: Vec<_> = (0..count).map(|i| json!({"path":format!("memory/entry-{i:03}-\u{03b2}.md"),"size_bytes":i+10,"updated_at":null,"truncated":false})).collect();
            let result = json!({"method":"project_memory_list","files":files,"truncated":false});
            let mut tc = projects_tool_call(json!({"method":"project_memory_list"}), &result);
            let original = tc.content.clone();
            let rows = rendered_line_texts(&render_tool_content(&tc));
            let entries: Vec<_> =
                rows.iter().filter(|row| row.starts_with("Memory file ")).collect();
            assert_eq!(entries.len(), count.min(128));
            for (i, row) in entries.iter().enumerate() {
                assert!(row.contains(&format!("memory/entry-{i:03}-\u{03b2}.md")), "{row}");
            }
            require_row(&rows, "List truncated: no");
            assert_eq!(rows.iter().any(|row| row.contains("not shown")), count > 128, "{rows:?}");
            let body = card_body(&mut tc, 120);
            assert!(body.iter().any(|row| row.contains("hidden")), "normal card capping: {body:?}");
            if count > 128 {
                assert!(
                    body.iter().any(|row| row.contains("not shown")),
                    "local omission: {body:?}"
                );
            }
            assert_eq!(tc.content, original);
        }
    }

    #[test]
    fn memory_read_preview_preserves_literal_prefix_and_reports_line_or_unicode_clipping() {
        let mut cases = vec![(
            "\nfirst\n\n  second \u{03b2}".to_owned(),
            vec!["", "first", "", "  second \u{03b2}"]
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>(),
            false,
        )];
        for count in [199, 200, 201] {
            let rows: Vec<_> = (0..count).map(|i| format!("line-{i:03}-\u{03b2}")).collect();
            cases.push((rows.join("\n"), rows[..count.min(200)].to_vec(), count > 200));
        }
        for count in [16_383, 16_384, 16_385] {
            let source = format!("\u{03b1}\n{}", "🦀".repeat(count - 2));
            cases.push((
                source,
                vec!["\u{03b1}".to_owned(), "🦀".repeat(count.min(16_384) - 2)],
                count > 16_384,
            ));
        }
        for (source, expected, omitted) in cases {
            let result = json!({"method":"project_memory_read","path":"memory/long.md","content":source,"size_bytes":source.len(),"updated_at":null,"truncated":false});
            let mut tc = projects_tool_call(
                json!({"method":"project_memory_read","path":"long.md"}),
                &result,
            );
            let original = tc.content.clone();
            let projection = render_tool_content(&tc);
            let mut actual = content_rows(&projection);
            if omitted {
                assert!(actual.pop().unwrap().contains("truncated for display"));
            }
            assert_eq!(actual, expected);
            require_row(&rendered_line_texts(&projection), "Truncated: no");
            let body = card_body(&mut tc, 100);
            if omitted {
                assert!(body.iter().any(|row| row.contains("truncated for display")), "{body:?}");
            }
            assert_eq!(tc.content, original);
        }
    }

    #[test]
    fn read_and_delete_receipts_preserve_returned_path_content_and_outcome() {
        for result in [
            json!({"method":"project_read","path":"claude/text.md","created_at":null,"content":"literal first\n\n  literal second"}),
            json!({"method":"project_read","path":"claude/image.png","created_at":null,"file_kind":"image","local_file":"C:/downloads/image.png"}),
            json!({"method":"project_delete","path":"claude/removed.md","deleted":true,"notice":"Removed requested document"}),
            json!({"method":"project_delete","path":"claude/missing.md","deleted":false,"notice":"Already absent"}),
        ] {
            let mut tc = projects_tool_call(
                json!({"method":result["method"],"path":"bare-request"}),
                &result,
            );
            let body = card_body(&mut tc, 120);
            require_row(&body, &format!("Path: {}", result["path"].as_str().unwrap()));
            if result.get("content").is_some() {
                assert_eq!(
                    content_rows(&render_tool_content(&tc)),
                    vec!["literal first", "", "  literal second"]
                );
                assert!(body.iter().any(|row| row.contains("literal second")), "{body:?}");
            } else if result.get("local_file").is_some() {
                require_row(&body, "Local file: C:/downloads/image.png");
                assert!(!body.iter().any(|row| row.contains("literal first")));
            } else {
                require_row(
                    &body,
                    if result["deleted"] == true { "Deleted: yes" } else { "Deleted: no" },
                );
                require_row(&body, &format!("Notice: {}", result["notice"].as_str().unwrap()));
                assert!(!body.iter().any(|row| row.contains("Local file:")));
            }
        }
    }
}
