// SPDX-License-Identifier: Apache-2.0

//! Rendering helpers for SDK `Artifact` tool calls.

use crate::app::ToolCallInfo;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::{Map, Value};

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
        if let Some(action) = typed::json_string(input, "action") {
            artifact_fields.push(ToolField::new("Action", action));
        }
        if let Some(scope) = typed::json_string(input, "scope") {
            artifact_fields.push(ToolField::new("Scope", scope));
        }
        if let Some(limit) = typed::json_i64(input, "limit") {
            artifact_fields.push(ToolField::new("Limit", limit.to_string()));
        }
        if let Some(title) = typed::json_string(input, "title") {
            artifact_fields.push(ToolField::new("Title", title));
        }
        if let Some(label) = typed::json_string(input, "label") {
            artifact_fields.push(ToolField::new("Label", label));
        }
        if let Some(description) = typed::json_string(input, "description") {
            artifact_fields.push(ToolField::new("Description", description));
        }
        if let Some(file_path) = typed::json_string(input, "file_path") {
            artifact_fields.push(ToolField::new("File path", file_path));
        }
        if let Some(favicon) = typed::json_string(input, "favicon") {
            artifact_fields.push(ToolField::new("Favicon", favicon));
        }
        if let Some(url) = typed::json_string(input, "url") {
            artifact_fields.push(ToolField::new("URL", url));
        }
        if let Some(additional) = additional_json(
            input,
            &[
                "action",
                "scope",
                "limit",
                "title",
                "label",
                "description",
                "file_path",
                "favicon",
                "url",
                "force",
            ],
        ) {
            artifact_fields.push(ToolField::new("Additional input", additional));
        }
    }

    let mut lines = fields::render_fields(artifact_fields);
    if let Some(force) = input.and_then(|input| typed::json_bool(input, "force")) {
        let style = if force {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        lines.push(Line::from(vec![
            Span::styled("Overwrite conflicts: ", Style::default().fg(crate::ui::theme::DIM)),
            Span::styled(typed::bool_label(force), style),
        ]));
    }
    lines
}

fn render_output_object(object: &Map<String, Value>) -> Vec<Line<'static>> {
    let mut artifact_fields = Vec::new();
    if let Some(scope) = typed::json_string(object, "scope") {
        artifact_fields.push(ToolField::new("Scope", scope));
    }
    if let Some(truncated) = typed::json_bool(object, "truncated") {
        artifact_fields.push(ToolField::new("Truncated", typed::bool_label(truncated)));
    }
    if let Some(artifacts) = object.get("artifacts").and_then(Value::as_array) {
        artifact_fields.push(ToolField::new("Artifacts", artifacts.len().to_string()));
    }
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
    if let Some(capabilities) = object.get("capabilities").and_then(typed::non_empty_compact_json) {
        artifact_fields.push(ToolField::new("Capabilities", capabilities));
    }
    if let Some(stored) = object.get("stored").and_then(Value::as_object) {
        if let Some(contract) = typed::json_string(stored, "contract") {
            artifact_fields.push(ToolField::new("Stored contract", contract));
        }
        if let Some(capabilities) =
            stored.get("capabilities").and_then(typed::non_empty_compact_json)
        {
            artifact_fields.push(ToolField::new("Stored capabilities", capabilities));
        }
    }
    if let Some(warnings) = typed::json_string_array(object.get("warnings")) {
        artifact_fields.push(ToolField::new("Warnings", warnings.join("; ")));
    }
    if let Some(contract) = typed::json_string(object, "contract") {
        artifact_fields.push(ToolField::new("Contract", contract));
    }
    if let Some(updated) = typed::json_bool(object, "updated") {
        artifact_fields.push(ToolField::new("Updated", typed::bool_label(updated)));
    }
    if let Some(live_subscription) = typed::json_string(object, "liveSubscription") {
        artifact_fields.push(ToolField::new("Live subscription", live_subscription));
    }
    if let Some(additional) = additional_json(
        object,
        &[
            "artifacts",
            "truncated",
            "scope",
            "title",
            "url",
            "path",
            "version",
            "mcpDropped",
            "capabilities",
            "stored",
            "warnings",
            "contract",
            "updated",
            "liveSubscription",
        ],
    ) {
        artifact_fields.push(ToolField::new("Additional output", additional));
    }

    let mut lines = fields::render_fields(artifact_fields);
    if let Some(artifacts) = object.get("artifacts").and_then(Value::as_array) {
        for (index, artifact) in artifacts.iter().enumerate() {
            let Some(artifact) = artifact.as_object() else {
                if let Some(value) = typed::non_empty_compact_json(artifact) {
                    lines.push(fields::render_dynamic_field(
                        format!("Artifact {}", index + 1),
                        value,
                    ));
                }
                continue;
            };
            let title = typed::json_string(artifact, "title").unwrap_or("<untitled>");
            let url = typed::json_string(artifact, "url").unwrap_or("<no URL>");
            let relation = typed::json_string(artifact, "rel").map(|rel| format!("({rel}) "));
            let updated = typed::json_string(artifact, "updatedAt")
                .map(|timestamp| format!(" — updated {timestamp}"))
                .unwrap_or_default();
            lines.push(fields::render_dynamic_field(
                format!("Artifact {}", index + 1),
                format!("{}{title} — {url}{updated}", relation.unwrap_or_default()),
            ));
        }
    }
    lines
}

fn additional_json(object: &Map<String, Value>, handled: &[&str]) -> Option<String> {
    let additional = object
        .iter()
        .filter(|(key, _)| !handled.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<Map<_, _>>();
    (!additional.is_empty()).then(|| typed::compact_json(&Value::Object(additional))).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::model;
    use crate::ui::tool_call::test_support::{rendered_line_texts, tool_call};
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn artifact_tool_call(raw_input: serde_json::Value, content: Option<&str>) -> ToolCallInfo {
        tool_call(
            "tc-artifact",
            "Artifact: dashboard",
            "Artifact",
            raw_input,
            content,
            model::ToolCallStatus::Completed,
        )
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

    #[test]
    fn renders_publish_contract_without_dropping_unknown_fields() {
        let tc = artifact_tool_call(
            json!({
                "action": "publish",
                "title": "Dashboard page",
                "favicon": "chart",
                "force": true,
                "futureInput": {"enabled": true}
            }),
            Some(
                r#"{"title":"Dashboard","url":"https://artifact.local/dashboard","path":"C:/work/dashboard.html","version":"v3","capabilities":{"storage":true},"stored":{"contract":"artifact-v1","capabilities":{"persist":true}},"warnings":["legacy contract"],"contract":"artifact-v2","updated":true,"liveSubscription":"subscription-42","futureOutput":{"revision":4}}"#,
            ),
        );

        let lines = render_tool_content(&tc);

        assert_eq!(
            rendered_line_texts(&lines),
            vec![
                "Action: publish",
                "Title: Dashboard page",
                "Favicon: chart",
                "Additional input: {\"futureInput\":{\"enabled\":true}}",
                "Overwrite conflicts: yes",
                "Title: Dashboard",
                "URL: https://artifact.local/dashboard",
                "Path: C:/work/dashboard.html",
                "Version: v3",
                "Capabilities: {\"storage\":true}",
                "Stored contract: artifact-v1",
                "Stored capabilities: {\"persist\":true}",
                "Warnings: legacy contract",
                "Contract: artifact-v2",
                "Updated: yes",
                "Live subscription: subscription-42",
                "Additional output: {\"futureOutput\":{\"revision\":4}}",
            ]
        );
    }

    #[test]
    fn renders_list_scope_and_each_artifact() {
        let tc = artifact_tool_call(
            json!({"action": "list", "scope": "all", "limit": 10}),
            Some(
                r#"{"scope":"all","truncated":false,"artifacts":[{"rel":"mine","title":"Dashboard","url":"https://artifact.local/dashboard","updatedAt":"2026-07-18T10:00:00Z"},{"rel":"shared","title":"Roadmap","url":"https://artifact.local/roadmap"}]}"#,
            ),
        );

        let lines = render_tool_content(&tc);

        assert_eq!(
            rendered_line_texts(&lines),
            vec![
                "Action: list",
                "Scope: all",
                "Limit: 10",
                "Scope: all",
                "Truncated: no",
                "Artifacts: 2",
                "Artifact 1: (mine) Dashboard — https://artifact.local/dashboard — updated 2026-07-18T10:00:00Z",
                "Artifact 2: (shared) Roadmap — https://artifact.local/roadmap",
            ]
        );
    }

    #[test]
    fn preserves_native_human_readable_output() {
        let tc = artifact_tool_call(
            json!({"action": "list", "scope": "all"}),
            Some(
                "Found 1 artifact(s) (scope: all):\n- (mine) Dashboard — https://artifact.local/dashboard",
            ),
        );

        let lines = render_tool_content(&tc);

        assert_eq!(
            rendered_line_texts(&lines),
            vec![
                "Action: list",
                "Scope: all",
                "Found 1 artifact(s) (scope: all):",
                "- (mine) Dashboard — https://artifact.local/dashboard",
            ]
        );
    }
}
