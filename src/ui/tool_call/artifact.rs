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
        if let Some(prompt) = typed::json_string(input, "prompt") {
            artifact_fields.push(ToolField::new("Prompt", prompt));
        }
        if let Some(out_dir) = typed::json_string(input, "out_dir") {
            artifact_fields.push(ToolField::new("Output directory", out_dir));
        }
        if let Some(asset_id) = typed::json_string(input, "asset_id") {
            artifact_fields.push(ToolField::new("Asset ID", asset_id));
        }
        if let Some(after) = typed::json_string(input, "after") {
            artifact_fields.push(ToolField::new("After", after));
        }
        if let Some(capabilities) =
            input.get("capabilities").and_then(typed::non_empty_compact_json)
        {
            artifact_fields.push(ToolField::new("Capabilities", capabilities));
        }
        if let Some(contract) = typed::json_string(input, "contract") {
            artifact_fields.push(ToolField::new("Contract", contract));
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
                "prompt",
                "out_dir",
                "asset_id",
                "after",
                "capabilities",
                "contract",
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

fn render_output_fields(object: &Map<String, Value>) -> Vec<ToolField<'_>> {
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
    if let Some(artifact_id) = typed::json_string(object, "artifact_id") {
        artifact_fields.push(ToolField::new("Artifact ID", artifact_id));
    }
    if let Some(mcp_dropped) = typed::json_string(object, "mcpDropped") {
        artifact_fields.push(ToolField::new("MCP dropped", mcp_dropped));
    }
    if let Some(capabilities) = object.get("capabilities").and_then(typed::non_empty_compact_json) {
        artifact_fields.push(ToolField::new("Capabilities", capabilities));
    }
    if let Some(stored) = object.get("stored").and_then(Value::as_object) {
        artifact_fields.extend(render_stored_output_fields(stored));
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
    if let Some(audience) = typed::json_string(object, "audience") {
        artifact_fields.push(ToolField::new("Audience", audience));
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
            "artifact_id",
            "mcpDropped",
            "capabilities",
            "stored",
            "warnings",
            "contract",
            "updated",
            "audience",
            "liveSubscription",
            "read",
            "artifactRead",
            "asset_upload",
            "asset_list",
            "asset_read",
            "asset_delete",
            "watch",
            "unwatch",
            "watches",
            "filter_url",
            "arms",
        ],
    ) {
        artifact_fields.push(ToolField::new("Additional output", additional));
    }

    artifact_fields
}

fn render_stored_output_fields(stored: &Map<String, Value>) -> Vec<ToolField<'_>> {
    let mut fields = Vec::new();
    if let Some(contract) = typed::json_string(stored, "contract") {
        fields.push(ToolField::new("Stored contract", contract));
    }
    if let Some(capabilities) = stored.get("capabilities").and_then(typed::non_empty_compact_json) {
        fields.push(ToolField::new("Stored capabilities", capabilities));
    }
    if let Some(preferred_contract) = typed::json_string(stored, "preferredContract") {
        fields.push(ToolField::new("Preferred contract", preferred_contract));
    }
    if let Some(carried) = typed::json_bool(stored, "carried") {
        fields.push(ToolField::new("Stored state carried", typed::bool_label(carried)));
    }
    if let Some(read) = typed::json_string(stored, "read") {
        fields.push(ToolField::new("Stored read state", read));
    }
    if let Some(additional) = additional_json(
        stored,
        &["contract", "preferredContract", "capabilities", "carried", "read"],
    ) {
        fields.push(ToolField::new("Additional stored output", additional));
    }
    fields
}

fn render_output_object(object: &Map<String, Value>) -> Vec<Line<'static>> {
    let mut lines = fields::render_fields(render_output_fields(object));
    if let Some(read) = object.get("read") {
        lines.extend(render_read_output(read, object.get("artifactRead")));
    } else if let Some(artifact_read) = object.get("artifactRead") {
        lines.extend(render_artifact_read_metadata(artifact_read));
    }
    if let Some(asset_upload) = object.get("asset_upload") {
        lines.extend(render_asset_upload_output(asset_upload));
    }
    if let Some(asset_list) = object.get("asset_list") {
        lines.extend(render_asset_list_output(asset_list));
    }
    if let Some(asset_read) = object.get("asset_read") {
        lines.extend(render_asset_read_output(asset_read));
    }
    if let Some(asset_delete) = object.get("asset_delete") {
        lines.extend(render_asset_delete_output(asset_delete));
    }
    if let Some(watch) = object.get("watch") {
        lines.extend(render_watch_output(watch));
    }
    if let Some(unwatch) = object.get("unwatch") {
        lines.extend(render_unwatch_output(unwatch));
    }
    if object.get("watches").is_some() || object.get("arms").is_some() {
        lines.extend(render_watch_status_output(object));
    }
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
            let favicon = typed::json_string(artifact, "favicon")
                .map(|value| format!(" — icon {value}"))
                .unwrap_or_default();
            let updated = typed::json_string(artifact, "updatedAt")
                .map(|timestamp| format!(" — updated {timestamp}"))
                .unwrap_or_default();
            let additional =
                additional_json(artifact, &["title", "url", "favicon", "updatedAt", "rel"])
                    .map(|value| format!(" — additional {value}"))
                    .unwrap_or_default();
            lines.push(fields::render_dynamic_field(
                format!("Artifact {}", index + 1),
                format!(
                    "{}{title} — {url}{favicon}{updated}{additional}",
                    relation.unwrap_or_default()
                ),
            ));
        }
    }
    lines
}

fn render_watch_output(value: &Value) -> Vec<Line<'static>> {
    let Some(watch) = value.as_object() else {
        return compact_value_field("Watch", value);
    };
    let mut values = Vec::new();
    for (key, label) in [
        ("url", "Watch URL"),
        ("outcome", "Watch outcome"),
        ("reason", "Watch reason"),
        ("durable_skip_reason", "Durable skip reason"),
        ("task_id", "Watch task ID"),
        ("rail", "Watch rail"),
        ("trigger_id", "Watch trigger ID"),
        ("durable_since", "Durable since"),
        ("detail", "Watch detail"),
        ("note", "Watch note"),
    ] {
        if let Some(value) = typed::json_string(watch, key) {
            values.push(ToolField::new(label, value));
        }
    }
    if let Some(watching) = typed::json_bool(watch, "watching") {
        values.push(ToolField::new("Watching", typed::bool_label(watching)));
    }
    for (key, label) in [
        ("since", "Watch since"),
        ("token_expires_at", "Token expires at"),
        ("status", "Watch status"),
    ] {
        if let Some(value) = typed::json_i64(watch, key) {
            values.push(ToolField::new(label, value.to_string()));
        }
    }
    if let Some(events) = typed::json_string_array(watch.get("events")) {
        values.push(ToolField::new("Watch events", events.join(", ")));
    }
    if let Some(additional) = additional_json(
        watch,
        &[
            "url",
            "watching",
            "outcome",
            "reason",
            "durable_skip_reason",
            "task_id",
            "since",
            "token_expires_at",
            "rail",
            "trigger_id",
            "durable_since",
            "status",
            "detail",
            "note",
            "events",
        ],
    ) {
        values.push(ToolField::new("Additional watch output", additional));
    }
    fields::render_fields(values)
}

fn render_unwatch_output(value: &Value) -> Vec<Line<'static>> {
    let Some(unwatch) = value.as_object() else {
        return compact_value_field("Unwatch", value);
    };
    let mut values = Vec::new();
    if let Some(url) = typed::json_string(unwatch, "url") {
        values.push(ToolField::new("Unwatch URL", url));
    }
    if let Some(was_watching) = typed::json_bool(unwatch, "was_watching") {
        values.push(ToolField::new("Was watching", typed::bool_label(was_watching)));
    }
    if let Some(additional) = additional_json(unwatch, &["url", "was_watching"]) {
        values.push(ToolField::new("Additional unwatch output", additional));
    }
    fields::render_fields(values)
}

fn render_watch_status_output(object: &Map<String, Value>) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if let Some(filter_url) = typed::json_string(object, "filter_url") {
        lines.push(fields::render_field("Watch filter", filter_url));
    }
    if let Some(watches) = object.get("watches").and_then(Value::as_array) {
        lines.push(fields::render_field("Watches", watches.len().to_string()));
        for (index, watch) in watches.iter().enumerate() {
            lines.extend(compact_value_field(format!("Watcher {}", index + 1), watch));
        }
    }
    if let Some(arms) = object.get("arms").and_then(Value::as_array) {
        lines.push(fields::render_field("Watch arms", arms.len().to_string()));
        for (index, arm) in arms.iter().enumerate() {
            let Some(arm) = arm.as_object() else {
                lines.extend(compact_value_field(format!("Watch arm {}", index + 1), arm));
                continue;
            };
            let url = typed::json_string(arm, "url").unwrap_or("<no URL>");
            let state = typed::json_string(arm, "state").unwrap_or("unknown");
            let server_message = typed::json_string(arm, "server_message")
                .map(|value| format!("; server message {value}"))
                .unwrap_or_default();
            let detail = typed::json_string(arm, "detail")
                .map(|value| format!("; detail {value}"))
                .unwrap_or_default();
            let additional = additional_json(
                arm,
                &[
                    "url",
                    "state",
                    "server_message",
                    "detail",
                    "rail",
                    "reconnect",
                    "failures",
                    "max_failures",
                    "next_in_s",
                    "last_failure",
                    "reason",
                    "at",
                ],
            )
            .map(|value| format!("; additional {value}"))
            .unwrap_or_default();
            lines.push(fields::render_dynamic_field(
                format!("Watch arm {}", index + 1),
                format!("{url} — {state}{server_message}{detail}{additional}"),
            ));
        }
    }
    lines
}

fn render_read_output(read: &Value, artifact_read: Option<&Value>) -> Vec<Line<'static>> {
    let Some(read) = read.as_object() else {
        return compact_value_field("Read", read);
    };
    let mut values = Vec::new();
    if let Some(url) = typed::json_string(read, "url") {
        values.push(ToolField::new("Read URL", url));
    }
    if let Some(bytes) = typed::json_i64(read, "bytes") {
        values.push(ToolField::new("Bytes", bytes.to_string()));
    }
    if let Some(code) = typed::json_i64(read, "code") {
        let status = typed::json_string(read, "codeText")
            .map_or_else(|| code.to_string(), |text| format!("{code} {text}"));
        values.push(ToolField::new("Status", status));
    } else if let Some(code_text) = typed::json_string(read, "codeText") {
        values.push(ToolField::new("Status", code_text));
    }
    if let Some(duration_ms) = typed::json_i64(read, "durationMs") {
        values.push(ToolField::new("Duration", format!("{duration_ms} ms")));
    }
    let mut lines = fields::render_fields(values);
    if let Some(artifact_read) = artifact_read {
        lines.extend(render_artifact_read_metadata(artifact_read));
    }
    if let Some(result) = typed::json_string(read, "result") {
        lines.extend(fields::render_multiline_field("Result", result));
    }
    if let Some(additional) =
        additional_json(read, &["url", "bytes", "code", "codeText", "result", "durationMs"])
    {
        lines.push(fields::render_field("Additional read output", additional));
    }
    lines
}

fn render_artifact_read_metadata(value: &Value) -> Vec<Line<'static>> {
    let Some(metadata) = value.as_object() else {
        return compact_value_field("Artifact read", value);
    };
    let mut values = Vec::new();
    if let Some(slug) = typed::json_string(metadata, "slug") {
        values.push(ToolField::new("Artifact slug", slug));
    }
    if let Some(ver) = typed::json_string(metadata, "ver") {
        values.push(ToolField::new("Artifact version", ver));
    }
    if let Some(seeded) = typed::json_bool(metadata, "seeded") {
        values.push(ToolField::new("Seeded", typed::bool_label(seeded)));
    }
    if let Some(additional) = additional_json(metadata, &["slug", "ver", "seeded"]) {
        values.push(ToolField::new("Additional artifact metadata", additional));
    }
    fields::render_fields(values)
}

fn render_asset_upload_output(value: &Value) -> Vec<Line<'static>> {
    let Some(asset) = value.as_object() else {
        return compact_value_field("Asset upload", value);
    };
    render_asset_fields(
        asset,
        "Uploaded asset",
        &["id", "url", "file_name", "content_type", "size_bytes", "sha256"],
    )
}

fn render_asset_read_output(value: &Value) -> Vec<Line<'static>> {
    let Some(asset) = value.as_object() else {
        return compact_value_field("Asset read", value);
    };
    render_asset_fields(
        asset,
        "Read asset",
        &["id", "path", "content_type", "size_bytes", "sha256"],
    )
}

fn render_asset_fields(
    asset: &Map<String, Value>,
    id_label: &'static str,
    handled: &[&str],
) -> Vec<Line<'static>> {
    let mut values = Vec::new();
    if let Some(id) = typed::json_string(asset, "id") {
        values.push(ToolField::new(id_label, id));
    }
    if let Some(url) = typed::json_string(asset, "url") {
        values.push(ToolField::new("Asset URL", url));
    }
    if let Some(path) = typed::json_string(asset, "path") {
        values.push(ToolField::new("Saved path", path));
    }
    if let Some(file_name) = typed::json_string(asset, "file_name") {
        values.push(ToolField::new("File name", file_name));
    }
    if let Some(content_type) = typed::json_string(asset, "content_type") {
        values.push(ToolField::new("Content type", content_type));
    }
    if let Some(size_bytes) = typed::json_i64(asset, "size_bytes") {
        values.push(ToolField::new("Size", format!("{size_bytes} bytes")));
    }
    if let Some(sha256) = typed::json_string(asset, "sha256") {
        values.push(ToolField::new("SHA-256", sha256));
    }
    if let Some(additional) = additional_json(asset, handled) {
        values.push(ToolField::new("Additional asset output", additional));
    }
    fields::render_fields(values)
}

fn render_asset_list_output(value: &Value) -> Vec<Line<'static>> {
    let Some(list) = value.as_object() else {
        return compact_value_field("Asset list", value);
    };
    let mut values = Vec::new();
    if let Some(url) = typed::json_string(list, "url") {
        values.push(ToolField::new("Artifact URL", url));
    }
    if let Some(assets) = list.get("assets").and_then(Value::as_array) {
        values.push(ToolField::new("Assets", assets.len().to_string()));
    }
    if let Some(next) = typed::json_string(list, "next") {
        values.push(ToolField::new("Next", next));
    }
    let mut lines = fields::render_fields(values);
    if let Some(assets) = list.get("assets").and_then(Value::as_array) {
        for (index, asset) in assets.iter().enumerate() {
            let Some(asset) = asset.as_object() else {
                lines.extend(compact_value_field(format!("Asset {}", index + 1), asset));
                continue;
            };
            let id = typed::json_string(asset, "id").unwrap_or("<unknown ID>");
            let url = typed::json_string(asset, "url").unwrap_or("<no URL>");
            let content_type = typed::json_string(asset, "content_type").unwrap_or("unknown type");
            let size = typed::json_i64(asset, "size_bytes")
                .map_or_else(|| "unknown size".to_owned(), |bytes| format!("{bytes} bytes"));
            let created = typed::json_string(asset, "created_at")
                .map(|value| format!("; created {value}"))
                .unwrap_or_default();
            let sha = typed::json_string(asset, "sha256")
                .map(|value| format!("; SHA-256 {value}"))
                .unwrap_or_default();
            let additional = additional_json(
                asset,
                &["id", "url", "content_type", "size_bytes", "sha256", "created_at"],
            )
            .map(|value| format!("; additional {value}"))
            .unwrap_or_default();
            lines.push(fields::render_dynamic_field(
                format!("Asset {}", index + 1),
                format!("{id} — {url} — {content_type}, {size}{created}{sha}{additional}"),
            ));
        }
    }
    if let Some(usage) = list.get("usage") {
        lines.extend(render_asset_usage(usage));
    }
    if let Some(additional) = additional_json(list, &["url", "assets", "usage", "next"]) {
        lines.push(fields::render_field("Additional asset-list output", additional));
    }
    lines
}

fn render_asset_usage(value: &Value) -> Vec<Line<'static>> {
    let Some(usage) = value.as_object() else {
        return compact_value_field("Asset usage", value);
    };
    let files = typed::json_i64(usage, "files");
    let max_files = typed::json_i64(usage, "max_files");
    let bytes = typed::json_i64(usage, "bytes");
    let max_bytes = typed::json_i64(usage, "max_bytes");
    let file_usage = match (files, max_files) {
        (Some(files), Some(max)) => Some(format!("{files} / {max} files")),
        (Some(files), None) => Some(format!("{files} files")),
        _ => None,
    };
    let byte_usage = match (bytes, max_bytes) {
        (Some(bytes), Some(max)) => Some(format!("{bytes} / {max} bytes")),
        (Some(bytes), None) => Some(format!("{bytes} bytes")),
        _ => None,
    };
    let mut values = Vec::new();
    let summary = [file_usage, byte_usage].into_iter().flatten().collect::<Vec<_>>().join("; ");
    if !summary.is_empty() {
        values.push(ToolField::new("Asset usage", summary));
    }
    if let Some(additional) = additional_json(usage, &["files", "bytes", "max_files", "max_bytes"])
    {
        values.push(ToolField::new("Additional asset usage", additional));
    }
    fields::render_fields(values)
}

fn render_asset_delete_output(value: &Value) -> Vec<Line<'static>> {
    let Some(asset) = value.as_object() else {
        return compact_value_field("Asset delete", value);
    };
    let mut values = Vec::new();
    if let Some(id) = typed::json_string(asset, "id") {
        values.push(ToolField::new("Deleted asset", id));
    }
    if let Some(deleted) = typed::json_bool(asset, "deleted") {
        values.push(ToolField::new("Deleted", typed::bool_label(deleted)));
    }
    if let Some(additional) = additional_json(asset, &["id", "deleted"]) {
        values.push(ToolField::new("Additional asset output", additional));
    }
    fields::render_fields(values)
}

fn compact_value_field(label: impl Into<String>, value: &Value) -> Vec<Line<'static>> {
    typed::non_empty_compact_json(value)
        .map(|value| vec![fields::render_dynamic_field(label, value)])
        .unwrap_or_default()
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
                r#"{"title":"Dashboard","url":"https://artifact.local/dashboard","path":"C:/work/dashboard.html","version":"v3","artifact_id":"artifact-42","capabilities":{"storage":true},"stored":{"contract":"artifact-v1","preferredContract":"artifact-v2","capabilities":{"persist":true},"carried":true,"read":"v3","futureStored":4},"warnings":["legacy contract"],"contract":"artifact-v2","updated":true,"audience":"workspace","liveSubscription":"subscription-42","futureOutput":{"revision":4}}"#,
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
                "Artifact ID: artifact-42",
                "Capabilities: {\"storage\":true}",
                "Stored contract: artifact-v1",
                "Stored capabilities: {\"persist\":true}",
                "Preferred contract: artifact-v2",
                "Stored state carried: yes",
                "Stored read state: v3",
                "Additional stored output: {\"futureStored\":4}",
                "Warnings: legacy contract",
                "Contract: artifact-v2",
                "Updated: yes",
                "Audience: workspace",
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
                r#"{"scope":"all","truncated":false,"artifacts":[{"rel":"mine","title":"Dashboard","url":"https://artifact.local/dashboard","favicon":"📊","updatedAt":"2026-07-18T10:00:00Z","futureItem":true},{"rel":"shared","title":"Roadmap","url":"https://artifact.local/roadmap"}]}"#,
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
                "Artifact 1: (mine) Dashboard — https://artifact.local/dashboard — icon 📊 — updated 2026-07-18T10:00:00Z — additional {\"futureItem\":true}",
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

    #[test]
    fn renders_read_input_and_multiline_output_with_artifact_metadata() {
        let tc = artifact_tool_call(
            json!({
                "action": "read",
                "url": "https://artifact.local/dashboard",
                "prompt": "Summarize the metrics",
                "out_dir": "C:/work/assets",
                "asset_id": "0123456789abcdef0123456789abcdef",
                "after": "page-2",
                "capabilities": {"storage": true},
                "contract": "latest"
            }),
            Some(
                r#"{"read":{"url":"https://artifact.local/dashboard","bytes":128,"code":200,"codeText":"OK","result":"First line\nSecond line","durationMs":42,"futureRead":true},"artifactRead":{"slug":"dashboard","ver":"v3","seeded":false,"futureMeta":"kept"},"futureOutput":{"revision":4}}"#,
            ),
        );

        assert_eq!(
            rendered_line_texts(&render_tool_content(&tc)),
            vec![
                "Action: read",
                "URL: https://artifact.local/dashboard",
                "Prompt: Summarize the metrics",
                "Output directory: C:/work/assets",
                "Asset ID: 0123456789abcdef0123456789abcdef",
                "After: page-2",
                "Capabilities: {\"storage\":true}",
                "Contract: latest",
                "Additional output: {\"futureOutput\":{\"revision\":4}}",
                "Read URL: https://artifact.local/dashboard",
                "Bytes: 128",
                "Status: 200 OK",
                "Duration: 42 ms",
                "Artifact slug: dashboard",
                "Artifact version: v3",
                "Seeded: no",
                "Additional artifact metadata: {\"futureMeta\":\"kept\"}",
                "Result: First line",
                "        Second line",
                "Additional read output: {\"futureRead\":true}",
            ]
        );
    }

    #[test]
    fn renders_uploaded_asset_details() {
        let tc = artifact_tool_call(
            json!({"action": "upload_asset", "url": "https://artifact.local/dashboard", "file_path": "C:/work/chart.png"}),
            Some(
                r#"{"asset_upload":{"id":"asset-1","url":"https://artifact.local/assets/1","size_bytes":2048,"content_type":"image/png","sha256":"abc123","file_name":"chart.png","future":true}}"#,
            ),
        );

        assert_eq!(
            rendered_line_texts(&render_tool_content(&tc)),
            vec![
                "Action: upload_asset",
                "File path: C:/work/chart.png",
                "URL: https://artifact.local/dashboard",
                "Uploaded asset: asset-1",
                "Asset URL: https://artifact.local/assets/1",
                "File name: chart.png",
                "Content type: image/png",
                "Size: 2048 bytes",
                "SHA-256: abc123",
                "Additional asset output: {\"future\":true}",
            ]
        );
    }

    #[test]
    fn renders_paginated_asset_list_and_usage() {
        let tc = artifact_tool_call(
            json!({"action": "list_assets", "url": "https://artifact.local/dashboard", "after": "page-1"}),
            Some(
                r#"{"asset_list":{"url":"https://artifact.local/dashboard","assets":[{"id":"asset-1","url":"https://artifact.local/assets/1","content_type":"image/png","size_bytes":2048,"sha256":"abc123","created_at":"2026-08-22T08:00:00Z","future":true}],"usage":{"files":1,"bytes":2048,"max_files":1000,"max_bytes":1048576,"futureUsage":true},"next":"page-2","futureList":true}}"#,
            ),
        );

        assert_eq!(
            rendered_line_texts(&render_tool_content(&tc)),
            vec![
                "Action: list_assets",
                "URL: https://artifact.local/dashboard",
                "After: page-1",
                "Artifact URL: https://artifact.local/dashboard",
                "Assets: 1",
                "Next: page-2",
                "Asset 1: asset-1 — https://artifact.local/assets/1 — image/png, 2048 bytes; created 2026-08-22T08:00:00Z; SHA-256 abc123; additional {\"future\":true}",
                "Asset usage: 1 / 1000 files; 2048 / 1048576 bytes",
                "Additional asset usage: {\"futureUsage\":true}",
                "Additional asset-list output: {\"futureList\":true}",
            ]
        );
    }

    #[test]
    fn renders_read_and_deleted_asset_results() {
        let read = artifact_tool_call(
            json!({"action": "read_asset", "url": "https://artifact.local/dashboard", "asset_id": "asset-1", "out_dir": "C:/work/downloads"}),
            Some(
                r#"{"asset_read":{"id":"asset-1","path":"C:/work/downloads/asset-1.png","size_bytes":2048,"content_type":"image/png","sha256":"abc123"}}"#,
            ),
        );
        assert_eq!(
            rendered_line_texts(&render_tool_content(&read)),
            vec![
                "Action: read_asset",
                "URL: https://artifact.local/dashboard",
                "Output directory: C:/work/downloads",
                "Asset ID: asset-1",
                "Read asset: asset-1",
                "Saved path: C:/work/downloads/asset-1.png",
                "Content type: image/png",
                "Size: 2048 bytes",
                "SHA-256: abc123",
            ]
        );

        let delete = artifact_tool_call(
            json!({"action": "delete_asset", "url": "https://artifact.local/dashboard", "asset_id": "asset-1"}),
            Some(r#"{"asset_delete":{"id":"asset-1","deleted":true}}"#),
        );
        assert_eq!(
            rendered_line_texts(&render_tool_content(&delete)),
            vec![
                "Action: delete_asset",
                "URL: https://artifact.local/dashboard",
                "Asset ID: asset-1",
                "Deleted asset: asset-1",
                "Deleted: yes",
            ]
        );
    }

    #[test]
    fn keeps_watch_status_on_forward_compatible_fallback() {
        let tc = artifact_tool_call(
            json!({"action": "watch", "url": "https://artifact.local/dashboard"}),
            Some(
                r#"{"watch":{"url":"https://artifact.local/dashboard","watching":false,"outcome":"unsupported_here","detail":"runtime unavailable","futureWatch":true}}"#,
            ),
        );

        assert_eq!(
            rendered_line_texts(&render_tool_content(&tc)),
            vec![
                "Action: watch",
                "URL: https://artifact.local/dashboard",
                "Watch URL: https://artifact.local/dashboard",
                "Watch outcome: unsupported_here",
                "Watch detail: runtime unavailable",
                "Watching: no",
                "Additional watch output: {\"futureWatch\":true}",
            ]
        );
    }

    #[test]
    fn renders_watcher_status_and_server_details() {
        let tc = artifact_tool_call(
            json!({"action": "status"}),
            Some(
                r#"{"filter_url":"https://artifact.local/dashboard","watches":[{"url":"https://artifact.local/dashboard","rail":"durable_wake","trigger_id":"trigger-1","since":"2026-09-02","events":["published"],"futureWatcher":true}],"arms":[{"url":"https://artifact.local/dashboard","state":"retrying","server_message":"try later","detail":"connection reset","futureArm":true}]}"#,
            ),
        );
        let rendered = rendered_line_texts(&render_tool_content(&tc));

        assert!(rendered.contains(&"Watch filter: https://artifact.local/dashboard".to_owned()));
        assert!(rendered.iter().any(|line| line.contains("futureWatcher")));
        assert!(rendered.iter().any(|line| line.contains("server message try later")));
        assert!(rendered.iter().any(|line| line.contains("detail connection reset")));
        assert!(rendered.iter().any(|line| line.contains("futureArm")));
    }
}
