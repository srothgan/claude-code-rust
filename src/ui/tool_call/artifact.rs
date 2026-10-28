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
        if let Some(icon) = typed::json_string(input, "icon") {
            artifact_fields.push(ToolField::new("Icon", icon));
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
        if let Some(pin) = typed::json_bool(input, "pin") {
            artifact_fields.push(ToolField::new("Pin after publish", typed::bool_label(pin)));
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
                "icon",
                "url",
                "prompt",
                "out_dir",
                "asset_id",
                "after",
                "capabilities",
                "contract",
                "pin",
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
    if let Some(pinned) = typed::json_bool(object, "pinned") {
        artifact_fields.push(ToolField::new("Pinned", typed::bool_label(pinned)));
    }
    if let Some(pins_enabled) = typed::json_bool(object, "pins_enabled") {
        artifact_fields.push(ToolField::new("Pinning enabled", typed::bool_label(pins_enabled)));
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
    render_publish_state_fields(object, &mut artifact_fields);
    if let Some(additional) = additional_json(
        object,
        &[
            "artifacts",
            "truncated",
            "scope",
            "title",
            "url",
            "pinned",
            "pins_enabled",
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
            "publishesRemaining",
            "publishesResetAt",
            "seq",
            "unchanged",
            "written",
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

fn render_publish_state_fields<'a>(
    object: &'a Map<String, Value>,
    artifact_fields: &mut Vec<ToolField<'a>>,
) {
    if let Some(remaining) = typed::json_i64(object, "publishesRemaining") {
        artifact_fields.push(ToolField::new("Publishes remaining", remaining.to_string()));
    }
    if let Some(reset_at) = typed::json_i64(object, "publishesResetAt") {
        artifact_fields.push(ToolField::new("Publish limit resets at", reset_at.to_string()));
    }
    if let Some(sequence) = typed::json_i64(object, "seq") {
        artifact_fields.push(ToolField::new("Publish sequence", sequence.to_string()));
    }
    if let Some(unchanged) = typed::json_bool(object, "unchanged") {
        artifact_fields.push(ToolField::new("Unchanged", typed::bool_label(unchanged)));
    }
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
    if let Some(written) = object.get("written") {
        lines.extend(render_written_output(written));
    }
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
            let pinned = typed::json_bool(artifact, "pinned")
                .map(|value| format!(" — pinned {}", typed::bool_label(value)))
                .unwrap_or_default();
            let additional = additional_json(
                artifact,
                &["title", "url", "favicon", "updatedAt", "rel", "pinned"],
            )
            .map(|value| format!(" — additional {value}"))
            .unwrap_or_default();
            lines.push(fields::render_dynamic_field(
                format!("Artifact {}", index + 1),
                format!(
                    "{}{title} — {url}{favicon}{updated}{pinned}{additional}",
                    relation.unwrap_or_default()
                ),
            ));
        }
    }
    lines
}

fn render_written_output(value: &Value) -> Vec<Line<'static>> {
    let Some(written) = value.as_object() else {
        return compact_value_field("Written artifact", value);
    };
    let mut values = Vec::new();
    if let Some(url) = typed::json_string(written, "url") {
        values.push(ToolField::new("Written artifact", url));
    }
    if let Some(additional) = additional_json(written, &["url"]) {
        values.push(ToolField::new("Additional written output", additional));
    }
    fields::render_fields(values)
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
            // Only exclude fields rendered above; diagnostics and future values
            // must remain attached to this arm in the fallback projection.
            let additional = additional_json(arm, &["url", "state", "server_message", "detail"])
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
    if let Some(title) = typed::json_string(read, "title") {
        values.push(ToolField::new("Stored title", title));
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
    if let Some(additional) = additional_json(
        read,
        &["url", "title", "bytes", "code", "codeText", "result", "durationMs"],
    ) {
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
        &["id", "path", "content_type", "size_bytes", "sha256", "cowritten", "foreign"],
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
    if let Some(cowritten) = typed::json_bool(asset, "cowritten") {
        values.push(ToolField::new("Co-written", typed::bool_label(cowritten)));
    }
    if let Some(foreign) = typed::json_bool(asset, "foreign") {
        values.push(ToolField::new("Foreign shared asset", typed::bool_label(foreign)));
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
    if let Some(cowritten) = typed::json_bool(list, "cowritten") {
        values.push(ToolField::new("Co-written artifact", typed::bool_label(cowritten)));
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
    if let Some(additional) =
        additional_json(list, &["url", "assets", "usage", "next", "cowritten"])
    {
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
    use crate::ui::tool_call::test_support::{card_body, rendered_line_texts, tool_call};
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
    fn publish_results_preserve_identity_capabilities_and_unknown_value_ownership() {
        let mut compact = artifact_tool_call(
            json!({"file_path":"C:/work/request.html","label":"requested-label","description":"Requested description","url":"https://artifact.local/old"}),
            Some(
                r#"{"title":"Returned dashboard","url":"https://artifact.local/new","path":"C:/work/stored.html","version":"v2"}"#,
            ),
        );
        let rows = card_body(&mut compact, 120);
        assert_fields(
            &rows,
            &[
                ("File path", "C:/work/request.html"),
                ("Label", "requested-label"),
                ("Description", "Requested description"),
                ("URL", "https://artifact.local/old"),
                ("Title", "Returned dashboard"),
                ("URL", "https://artifact.local/new"),
                ("Path", "C:/work/stored.html"),
                ("Version", "v2"),
            ],
        );

        let rich = artifact_tool_call(
            json!({"action":"publish","file_path":"C:/work/page.html","title":"Requested title","favicon":"chart","icon":"map","pin":true,"force":true,"futureInput":{"enabled":false}}),
            Some(
                r#"{"url":"https://artifact.local/published","path":"C:/work/page.html","title":"Published title","version":"v3","artifact_id":"artifact-42","pinned":false,"pins_enabled":true,"capabilities":{"storage":false},"stored":{"contract":"artifact-v1","preferredContract":"artifact-v2","capabilities":{"persist":true},"carried":false,"read":"v3","futureStored":{"revision":4}},"warnings":["legacy contract"],"contract":"artifact-v2","updated":false,"audience":"workspace","liveSubscription":"subscription-42","publishesRemaining":0,"publishesResetAt":1234,"seq":7,"unchanged":true,"futureOutput":{"revision":8}}"#,
            ),
        );
        let rows = rendered_line_texts(&render_tool_content(&rich));
        assert_fields(
            &rows,
            &[
                ("Title", "Requested title"),
                ("Title", "Published title"),
                ("Favicon", "chart"),
                ("Icon", "map"),
                ("Artifact ID", "artifact-42"),
                ("Version", "v3"),
                ("Pinned", "no"),
                ("Pinning enabled", "yes"),
                ("Stored contract", "artifact-v1"),
                ("Preferred contract", "artifact-v2"),
                ("Stored state carried", "no"),
                ("Stored read state", "v3"),
                ("Warnings", "legacy contract"),
                ("Contract", "artifact-v2"),
                ("Updated", "no"),
                ("Audience", "workspace"),
                ("Live subscription", "subscription-42"),
                ("Publishes remaining", "0"),
                ("Publish limit resets at", "1234"),
                ("Publish sequence", "7"),
                ("Unchanged", "yes"),
            ],
        );
        assert_json_field(&rows, "Capabilities", &json!({"storage":false}));
        assert_json_field(&rows, "Stored capabilities", &json!({"persist":true}));
        assert_json_field(&rows, "Additional input", &json!({"futureInput":{"enabled":false}}));
        assert_json_field(
            &rows,
            "Additional stored output",
            &json!({"futureStored":{"revision":4}}),
        );
        assert_json_field(&rows, "Additional output", &json!({"futureOutput":{"revision":8}}));
        let carried = artifact_tool_call(
            json!({"action":"publish","file_path":"C:/work/page.html"}),
            Some(
                r#"{"url":"https://artifact.local/carried","path":"page.html","stored":{"contract":"artifact-v1","carried":true}}"#,
            ),
        );
        assert_fields(
            &rendered_line_texts(&render_tool_content(&carried)),
            &[("Stored state carried", "yes")],
        );

        for flag in [None, Some(false), Some(true)] {
            let mut input = json!({"action":"publish","file_path":"C:/work/page.html","capabilities":{"storage":true},"contract":"latest"});
            if let Some(flag) = flag {
                input["pin"] = json!(flag);
                input["force"] = json!(flag);
            }
            let mut tc = artifact_tool_call(
                input,
                Some(r#"{"url":"https://artifact.local/published","path":"C:/work/page.html"}"#),
            );
            let rows = card_body(&mut tc, 120);
            for label in ["Pin after publish", "Overwrite conflicts"] {
                let value = rows.iter().find_map(|row| row.strip_prefix(&format!("{label}: ")));
                assert_eq!(value, flag.map(|flag| if flag { "yes" } else { "no" }));
            }
            assert_fields(&rows, &[("URL", "https://artifact.local/published")]);
            assert_json_field(&rows, "Capabilities", &json!({"storage":true}));
            assert_fields(&rows, &[("Contract", "latest")]);
            assert!(
                !rows.iter().any(|row| row.starts_with("Pinned:") || row.starts_with("Unchanged:"))
            );
        }
    }

    #[test]
    fn artifact_list_keeps_each_items_identity_and_metadata_together() {
        let mut tc = artifact_tool_call(
            json!({"action":"list","scope":"all","limit":10}),
            Some(
                r#"{"scope":"all","truncated":false,"artifacts":[{"rel":"mine","title":"Dashboard","url":"https://artifact.local/dashboard","favicon":"chart","updatedAt":"2026-07-18T10:00:00Z","pinned":true,"futureItem":{"revision":4}},{"rel":"shared","title":"Roadmap","url":"https://artifact.local/roadmap","favicon":"map","updatedAt":"2026-07-19T11:00:00Z","pinned":false,"futureItem":{"revision":9}}]}"#,
            ),
        );
        let rows = card_body(&mut tc, 240);
        assert_fields(
            &rows,
            &[("Scope", "all"), ("Limit", "10"), ("Truncated", "no"), ("Artifacts", "2")],
        );
        for (url, own, peer, future) in [
            (
                "https://artifact.local/dashboard",
                vec!["Dashboard", "mine", "chart", "2026-07-18T10:00:00Z", "pinned yes"],
                "Roadmap",
                json!({"futureItem":{"revision":4}}),
            ),
            (
                "https://artifact.local/roadmap",
                vec!["Roadmap", "shared", "map", "2026-07-19T11:00:00Z", "pinned no"],
                "Dashboard",
                json!({"futureItem":{"revision":9}}),
            ),
        ] {
            let row = rows.iter().find(|row| row.contains(url)).expect("visible artifact item");
            for value in own {
                assert!(row.contains(value), "{row}");
            }
            assert!(!row.contains(peer), "{row}");
            assert_eq!(json_suffix(row), future);
        }
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
    fn read_result_keeps_literal_content_and_nested_metadata_with_its_source() {
        let tc = artifact_tool_call(
            json!({"action":"read","url":"https://artifact.local/requested","prompt":"Summarize metrics","futureInput":{"owner":"request"}}),
            Some(
                r#"{"read":{"url":"https://artifact.local/returned","title":"Stored dashboard","bytes":128,"code":200,"codeText":"OK","result":"First literal line\n\n  Second literal line","durationMs":42,"futureRead":{"owner":"read"}},"artifactRead":{"slug":"dashboard","ver":"v3","seeded":false,"futureMeta":{"owner":"metadata"}},"futureOutput":{"owner":"root"}}"#,
            ),
        );
        let lines = render_tool_content(&tc);
        let rows = rendered_line_texts(&lines);
        assert_fields(
            &rows,
            &[
                ("URL", "https://artifact.local/requested"),
                ("Prompt", "Summarize metrics"),
                ("Read URL", "https://artifact.local/returned"),
                ("Stored title", "Stored dashboard"),
                ("Bytes", "128"),
                ("Status", "200 OK"),
                ("Duration", "42 ms"),
                ("Artifact slug", "dashboard"),
                ("Artifact version", "v3"),
                ("Seeded", "no"),
            ],
        );
        for (label, expected) in [
            ("Additional input", json!({"futureInput":{"owner":"request"}})),
            ("Additional read output", json!({"futureRead":{"owner":"read"}})),
            ("Additional artifact metadata", json!({"futureMeta":{"owner":"metadata"}})),
            ("Additional output", json!({"futureOutput":{"owner":"root"}})),
        ] {
            assert_json_field(&rows, label, &expected);
        }
        let start = rows.iter().position(|row| row.starts_with("Result: ")).unwrap();
        let values: Vec<String> = lines[start..start + 3]
            .iter()
            .map(|line| line.spans.iter().skip(1).map(|span| span.content.as_ref()).collect())
            .collect();
        assert_eq!(values, vec!["First literal line", "", "  Second literal line"]);

        let mut compact = artifact_tool_call(
            json!({"action":"read","url":"https://artifact.local/visible"}),
            Some(
                r#"{"read":{"url":"https://artifact.local/visible","bytes":19,"code":200,"codeText":"OK","result":"literal A\nliteral B","durationMs":7}}"#,
            ),
        );
        let body = card_body(&mut compact, 120);
        assert_fields(
            &body,
            &[
                ("Read URL", "https://artifact.local/visible"),
                ("Status", "200 OK"),
                ("Result", "literal A"),
            ],
        );
        assert!(body.iter().any(|row| row.trim_start() == "literal B"), "{body:?}");
    }

    #[test]
    fn paginated_assets_keep_item_metadata_cursor_and_usage_values_distinct() {
        let tc = artifact_tool_call(
            json!({"action":"list_assets","url":"https://artifact.local/dashboard","after":"opaque-before"}),
            Some(
                r#"{"asset_list":{"url":"https://artifact.local/dashboard","assets":[{"id":"11111111111111111111111111111111","url":"https://artifact.local/assets/one","content_type":"image/png","size_bytes":2048,"sha256":"abc123","created_at":"2026-08-22T08:00:00Z","future":{"owner":"first"}},{"id":"22222222222222222222222222222222","url":"https://artifact.local/assets/two","content_type":"application/pdf","size_bytes":4096,"sha256":"def456","created_at":"2026-08-23T09:00:00Z","future":{"owner":"second"}}],"usage":{"files":2,"bytes":6144,"max_files":17,"max_bytes":8192,"futureUsage":{"quota":"workspace"}},"next":"opaque-after","cowritten":false,"futureList":{"generation":5}}}"#,
            ),
        );
        let rows = rendered_line_texts(&render_tool_content(&tc));
        assert_fields(
            &rows,
            &[
                ("After", "opaque-before"),
                ("Next", "opaque-after"),
                ("Artifact URL", "https://artifact.local/dashboard"),
                ("Assets", "2"),
                ("Co-written artifact", "no"),
            ],
        );
        for (url, expected, peer, future) in [
            (
                "https://artifact.local/assets/one",
                vec![
                    "11111111111111111111111111111111",
                    "image/png",
                    "2048 bytes",
                    "abc123",
                    "2026-08-22T08:00:00Z",
                ],
                "def456",
                json!({"future":{"owner":"first"}}),
            ),
            (
                "https://artifact.local/assets/two",
                vec![
                    "22222222222222222222222222222222",
                    "application/pdf",
                    "4096 bytes",
                    "def456",
                    "2026-08-23T09:00:00Z",
                ],
                "abc123",
                json!({"future":{"owner":"second"}}),
            ),
        ] {
            let row = rows.iter().find(|row| row.contains(url)).unwrap();
            for value in expected {
                assert!(row.contains(value), "{row}");
            }
            assert!(!row.contains(peer));
            assert_eq!(json_suffix(row), future);
        }
        let usage = rows.iter().find(|row| row.starts_with("Asset usage:")).unwrap();
        assert_usage(usage, ["2", "17", "files"], ["6144", "8192", "bytes"]);
        assert_json_field(
            &rows,
            "Additional asset usage",
            &json!({"futureUsage":{"quota":"workspace"}}),
        );
        assert_json_field(
            &rows,
            "Additional asset-list output",
            &json!({"futureList":{"generation":5}}),
        );
        let mut empty = artifact_tool_call(
            json!({"action":"list_assets","url":"https://artifact.local/empty"}),
            Some(
                r#"{"asset_list":{"url":"https://artifact.local/empty","assets":[],"usage":{"files":0,"bytes":0,"max_files":17,"max_bytes":8192}}}"#,
            ),
        );
        let body = card_body(&mut empty, 120);
        assert_fields(&body, &[("Assets", "0")]);
        let usage = body.iter().find(|row| row.starts_with("Asset usage:")).unwrap();
        assert_usage(usage, ["0", "17", "files"], ["0", "8192", "bytes"]);
        assert!(!body.iter().any(|row| row.starts_with("Next:") || row.contains("assets/one")));
    }

    #[test]
    fn asset_operations_preserve_ids_locations_payload_metadata_and_ownership() {
        for (input, receipt, fields) in [
            (
                json!({"action":"upload_asset","url":"https://artifact.local/upload-target","file_path":"C:/work/chart.png"}),
                json!({"asset_upload":{"id":"11111111111111111111111111111111","url":"https://artifact.local/assets/one","file_name":"chart.png","content_type":"image/png","size_bytes":2048,"sha256":"abc123","future":{"operation":"upload"}}}),
                vec![
                    ("Uploaded asset", "11111111111111111111111111111111"),
                    ("Asset URL", "https://artifact.local/assets/one"),
                    ("File name", "chart.png"),
                    ("Content type", "image/png"),
                    ("Size", "2048 bytes"),
                    ("SHA-256", "abc123"),
                    ("File path", "C:/work/chart.png"),
                ],
            ),
            (
                json!({"action":"read_asset","url":"https://artifact.local/read-target","asset_id":"22222222222222222222222222222222","out_dir":"C:/work/downloads"}),
                json!({"written":{"url":"https://artifact.local/read-target"},"asset_read":{"id":"22222222222222222222222222222222","path":"C:/work/downloads/second.pdf","content_type":"application/pdf","size_bytes":4096,"sha256":"def456","cowritten":true,"foreign":true,"future":{"operation":"read"}}}),
                vec![
                    ("Read asset", "22222222222222222222222222222222"),
                    ("Written artifact", "https://artifact.local/read-target"),
                    ("Saved path", "C:/work/downloads/second.pdf"),
                    ("Output directory", "C:/work/downloads"),
                    ("Content type", "application/pdf"),
                    ("Size", "4096 bytes"),
                    ("SHA-256", "def456"),
                    ("Co-written", "yes"),
                    ("Foreign shared asset", "yes"),
                ],
            ),
        ] {
            let tc = artifact_tool_call(input, Some(&receipt.to_string()));
            let rows = rendered_line_texts(&render_tool_content(&tc));
            assert_fields(&rows, &fields);
            let operation = if receipt.get("asset_upload").is_some() { "upload" } else { "read" };
            assert_json_field(
                &rows,
                "Additional asset output",
                &json!({"future":{"operation":operation}}),
            );
        }
        for (action, result, identity, location) in [
            (
                "upload_asset",
                json!({"asset_upload":{"id":"11111111111111111111111111111111","url":"https://artifact.local/assets/one","size_bytes":2048,"content_type":"image/png"}}),
                ("Uploaded asset", "11111111111111111111111111111111"),
                ("Asset URL", "https://artifact.local/assets/one"),
            ),
            (
                "read_asset",
                json!({"asset_read":{"id":"22222222222222222222222222222222","path":"C:/downloads/second.pdf","size_bytes":4096,"content_type":"application/pdf","cowritten":false,"foreign":false}}),
                ("Read asset", "22222222222222222222222222222222"),
                ("Saved path", "C:/downloads/second.pdf"),
            ),
        ] {
            let mut input = json!({"action":action,"url":"https://artifact.local/target"});
            if action == "upload_asset" {
                input["file_path"] = json!("C:/work/chart.png");
            } else {
                input["asset_id"] = json!("22222222222222222222222222222222");
            }
            let mut tc = artifact_tool_call(input, Some(&result.to_string()));
            let body = card_body(&mut tc, 160);
            assert_fields(&body, &[identity, location]);
            if action == "read_asset" {
                assert_fields(&body, &[("Co-written", "no"), ("Foreign shared asset", "no")]);
            }
        }
        for deleted in [true, false] {
            let result =
                json!({"asset_delete":{"id":"33333333333333333333333333333333","deleted":deleted}});
            let mut tc = artifact_tool_call(
                json!({"action":"delete_asset","url":"https://artifact.local/delete-target","asset_id":"33333333333333333333333333333333"}),
                Some(&result.to_string()),
            );
            let body = card_body(&mut tc, 160);
            assert_fields(
                &body,
                &[
                    ("URL", "https://artifact.local/delete-target"),
                    ("Deleted asset", "33333333333333333333333333333333"),
                    ("Deleted", if deleted { "yes" } else { "no" }),
                ],
            );
            assert!(
                !body.iter().any(|row| row == if deleted { "Deleted: no" } else { "Deleted: yes" })
            );
        }
    }

    #[test]
    fn watch_status_keeps_targets_states_and_unknown_values_associated() {
        let tc = artifact_tool_call(
            json!({"action":"status","url":"https://artifact.local/a"}),
            Some(
                r#"{"filter_url":"https://artifact.local/a","watches":[{"url":"https://artifact.local/a","rail":"durable_wake","trigger_id":"trigger-a","since":"2026-09-02","events":["published"],"futureWatcher":{"owner":"alpha"}},{"url":"https://artifact.local/b","rail":"durable_wake","trigger_id":"trigger-b","since":"2026-09-03","events":["updated"],"futureWatcher":{"owner":"beta"}}],"arms":[{"url":"https://artifact.local/a","state":"retrying","server_message":"try later","detail":"connection reset","futureArm":{"owner":"alpha"}},{"url":"https://artifact.local/b","state":"stopped","server_message":"stop accepted","detail":"closed by user","futureArm":{"owner":"beta"}}]}"#,
            ),
        );
        let rows = rendered_line_texts(&render_tool_content(&tc));
        assert_fields(&rows, &[("Watch filter", "https://artifact.local/a")]);
        let watchers: Vec<Value> = rows
            .iter()
            .filter(|row| row.starts_with("Watcher "))
            .map(|row| serde_json::from_str(row.split_once(": ").unwrap().1).unwrap())
            .collect();
        for expected in [
            json!({"url":"https://artifact.local/a","rail":"durable_wake","trigger_id":"trigger-a","since":"2026-09-02","events":["published"],"futureWatcher":{"owner":"alpha"}}),
            json!({"url":"https://artifact.local/b","rail":"durable_wake","trigger_id":"trigger-b","since":"2026-09-03","events":["updated"],"futureWatcher":{"owner":"beta"}}),
        ] {
            let actual = watchers
                .iter()
                .find(|watcher| watcher["url"] == expected["url"])
                .expect("watcher target");
            assert_eq!(actual, &expected);
        }
        for (url, expected, peer, extra) in [
            (
                "https://artifact.local/a",
                vec!["retrying", "try later", "connection reset"],
                "stop accepted",
                json!({"futureArm":{"owner":"alpha"}}),
            ),
            (
                "https://artifact.local/b",
                vec!["stopped", "stop accepted", "closed by user"],
                "try later",
                json!({"futureArm":{"owner":"beta"}}),
            ),
        ] {
            let row =
                rows.iter().find(|row| row.starts_with("Watch arm ") && row.contains(url)).unwrap();
            for value in expected {
                assert!(row.contains(value), "{row}");
            }
            assert!(!row.contains(peer));
            assert_eq!(json_suffix(row), extra);
        }
        let mut unsupported = artifact_tool_call(
            json!({"action":"watch","url":"https://artifact.local/unsupported"}),
            Some(
                r#"{"watch":{"url":"https://artifact.local/unsupported","watching":false,"outcome":"unsupported_here","detail":"runtime unavailable","futureWatch":{"retryable":false}}}"#,
            ),
        );
        let body = card_body(&mut unsupported, 160);
        assert_fields(
            &body,
            &[
                ("Watch URL", "https://artifact.local/unsupported"),
                ("Watching", "no"),
                ("Watch outcome", "unsupported_here"),
                ("Watch detail", "runtime unavailable"),
            ],
        );
        assert_json_field(
            &body,
            "Additional watch output",
            &json!({"futureWatch":{"retryable":false}}),
        );
        assert!(!body.iter().any(|row| row == "Watching: yes"));
    }

    #[test]
    fn watch_arm_failure_and_retry_diagnostics_survive_in_the_correct_card_region() {
        for (arms, expected) in [
            (
                json!([
                    {"url":"https://artifact.local/failed","state":"failed","reason":"token expired","last_failure":"HTTP 401"},
                    {"url":"https://artifact.local/retry","state":"retrying","failures":2,"max_failures":7,"next_in_s":15,"last_failure":"connection reset"}
                ]),
                vec![
                    (
                        "https://artifact.local/failed",
                        "failed",
                        json!({"reason":"token expired","last_failure":"HTTP 401"}),
                    ),
                    (
                        "https://artifact.local/retry",
                        "retrying",
                        json!({"failures":2,"max_failures":7,"next_in_s":15,"last_failure":"connection reset"}),
                    ),
                ],
            ),
            (
                json!([
                    {"url":"https://artifact.local/zero","state":"retrying","failures":0,"max_failures":5,"next_in_s":0,"reconnect":false,"rail":"live","at":1234},
                    {"url":"https://artifact.local/unknown","state":"pending"}
                ]),
                vec![
                    (
                        "https://artifact.local/zero",
                        "retrying",
                        json!({"failures":0,"max_failures":5,"next_in_s":0,"reconnect":false,"rail":"live","at":1234}),
                    ),
                    ("https://artifact.local/unknown", "pending", json!({})),
                ],
            ),
        ] {
            let result = json!({"watches":[],"arms":arms});
            let mut tc = artifact_tool_call(json!({"action":"status"}), Some(&result.to_string()));
            let original = tc.content.clone();
            let body = card_body(&mut tc, 240);
            for (url, state, diagnostics) in expected {
                let row = body
                    .iter()
                    .find(|row| row.starts_with("Watch arm ") && row.contains(url))
                    .expect("target arm visible");
                assert!(row.contains(state), "{row}");
                if diagnostics == json!({}) {
                    assert!(
                        !row.contains("additional"),
                        "missing diagnostics must stay unknown: {row}"
                    );
                } else {
                    assert_eq!(json_suffix(row), diagnostics);
                }
            }
            assert_eq!(tc.content, original);
        }
    }

    fn assert_fields(rows: &[String], fields: &[(&str, &str)]) {
        for (label, value) in fields {
            assert!(
                rows.iter().any(|row| row == &format!("{label}: {value}")),
                "missing {label}: {value} in {rows:?}"
            );
        }
    }

    fn assert_json_field(rows: &[String], label: &str, expected: &Value) {
        let raw = rows
            .iter()
            .find_map(|row| row.strip_prefix(&format!("{label}: ")))
            .expect("JSON field");
        assert_eq!(&serde_json::from_str::<Value>(raw).unwrap(), expected);
    }

    fn assert_usage(row: &str, files: [&str; 3], bytes: [&str; 3]) {
        let tokens: Vec<_> = row
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|part| part.parse::<u64>().is_ok() || matches!(*part, "files" | "bytes"))
            .collect();
        assert!(tokens.windows(3).any(|window| window == files), "{row}");
        assert!(tokens.windows(3).any(|window| window == bytes), "{row}");
    }

    fn json_suffix(row: &str) -> Value {
        let start = row.find('{').expect("additional values must be visible");
        serde_json::from_str(&row[start..]).expect("literal additional JSON values")
    }
}
