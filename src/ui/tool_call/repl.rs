// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

//! Rendering helpers for SDK `REPL` tool calls.

use crate::app::ToolCallInfo;
use ratatui::text::{Line, Span};

use super::fields::{self, ToolField};
use super::typed;

pub(super) fn is_repl_tool(tc: &ToolCallInfo) -> bool {
    tc.sdk_tool_name == "REPL"
}

pub(super) fn has_structured_body(tc: &ToolCallInfo) -> bool {
    input_has_body(tc) || !tc.content.is_empty()
}

pub(super) fn render_tool_content(tc: &ToolCallInfo) -> Vec<Line<'static>> {
    let mut lines = render_input_content(tc);
    lines.extend(typed::render_processed_text_blocks(tc, render_output_fields));
    lines
}

fn input_has_body(tc: &ToolCallInfo) -> bool {
    let input = typed::input_object(tc);
    input.is_some_and(|input| {
        typed::json_string(input, "description").is_some()
            || typed::json_i64(input, "timeout").is_some()
    })
}

fn render_input_content(tc: &ToolCallInfo) -> Vec<Line<'static>> {
    let input = typed::input_object(tc);
    let mut repl_fields = Vec::new();

    if let Some(input) = input {
        if let Some(description) = typed::json_string(input, "description") {
            repl_fields.push(ToolField::new("Description", description));
        }
        if let Some(timeout_ms) = typed::json_i64(input, "timeout") {
            repl_fields.push(ToolField::new("Timeout", format_duration_millis(timeout_ms)));
        }
    }

    fields::render_fields(repl_fields)
}

fn render_output_fields(text: &str) -> Vec<Line<'static>> {
    let (fields, raw_lines) = parse_output_fields(text);
    let mut lines = Vec::new();
    for (label, value) in fields {
        lines.extend(fields::render_multiline_field(label, value));
    }
    lines.extend(raw_lines.into_iter().map(Line::from));
    lines
}

fn parse_output_fields(text: &str) -> (Vec<(&'static str, String)>, Vec<Span<'static>>) {
    let mut fields = Vec::new();
    let mut raw_lines = Vec::new();
    let mut current: Option<(&'static str, String)> = None;

    for raw_line in text.lines() {
        let line = raw_line.trim_end();
        if line.trim().is_empty() {
            if let Some((_, value)) = current.as_mut() {
                value.push('\n');
            }
            continue;
        }

        if let Some((raw_label, value)) = line.split_once(':')
            && let Some(label) = field_label(raw_label.trim())
        {
            flush_field(&mut current, &mut fields);
            current = Some((label, field_value(label, value.trim_start())));
            continue;
        }

        if let Some((_, value)) = current.as_mut() {
            if !value.is_empty() {
                value.push('\n');
            }
            value.push_str(line);
        } else {
            raw_lines.push(Span::raw(line.to_owned()));
        }
    }

    flush_field(&mut current, &mut fields);
    (fields, raw_lines)
}

fn flush_field(
    current: &mut Option<(&'static str, String)>,
    fields: &mut Vec<(&'static str, String)>,
) {
    if let Some(field) = current.take() {
        fields.push(field);
    }
}

fn field_label(label: &str) -> Option<&'static str> {
    match label {
        "Error" | "error" => Some("Error"),
        "Stdout" | "stdout" => Some("Stdout"),
        "Stderr" | "stderr" => Some("Stderr"),
        "Result" | "result" => Some("Result"),
        "Registered tools" | "registeredTools" => Some("Registered tools"),
        "Images" | "images" => Some("Images"),
        "Documents" | "documents" => Some("Documents"),
        "Images omitted" | "imagesOmitted" => Some("Images omitted"),
        "Failed image page" | "imagePagesFailed" => Some("Failed image page"),
        "Failed image pages omitted" | "imagePagesFailedOmitted" => {
            Some("Failed image pages omitted")
        }
        "Documents omitted" | "documentsOmitted" => Some("Documents omitted"),
        _ => None,
    }
}

fn field_value(label: &str, value: &str) -> String {
    match label {
        "Registered tools" => registered_tools_value(value),
        "Images"
        | "Documents"
        | "Images omitted"
        | "Failed image pages omitted"
        | "Documents omitted" => media_count_value(value),
        _ => value.to_owned(),
    }
}

fn registered_tools_value(value: &str) -> String {
    let trimmed = value.trim();
    if let Ok(serde_json::Value::Array(values)) = serde_json::from_str::<serde_json::Value>(trimmed)
    {
        let tools = values
            .iter()
            .filter_map(serde_json::Value::as_str)
            .filter(|tool| !tool.is_empty())
            .collect::<Vec<_>>();
        if !tools.is_empty() {
            return tools.join(", ");
        }
    }
    trimmed.to_owned()
}

fn media_count_value(value: &str) -> String {
    let trimmed = value.trim();
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if let Some(values) = json.as_array() {
            return values.len().to_string();
        }
        if let Some(count) = json.as_u64() {
            return count.to_string();
        }
    }
    trimmed.to_owned()
}

fn format_duration_millis(milliseconds: i64) -> String {
    let milliseconds = u64::try_from(milliseconds.max(0)).unwrap_or_default();
    let hours = milliseconds / 3_600_000;
    let minutes = (milliseconds % 3_600_000) / 60_000;
    let seconds = (milliseconds % 60_000) / 1_000;
    let remaining_millis = milliseconds % 1_000;
    let mut parts = Vec::new();

    if hours > 0 {
        parts.push(format!("{hours}h"));
    }
    if minutes > 0 {
        parts.push(format!("{minutes}m"));
    }
    if seconds > 0 {
        parts.push(format!("{seconds}s"));
    }
    if remaining_millis > 0 || parts.is_empty() {
        parts.push(format!("{remaining_millis}ms"));
    }

    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::model;
    use crate::ui::tool_call::test_support::{rendered_line_texts, tool_call};
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn repl_tool_call(
        raw_input: serde_json::Value,
        content: Option<&str>,
        status: model::ToolCallStatus,
    ) -> ToolCallInfo {
        tool_call("tc-repl", "REPL", "REPL", raw_input, content, status)
    }

    #[test]
    fn input_body_renders_description_and_timeout() {
        let tc = repl_tool_call(
            json!({
                "code": "await main()",
                "description": "Run main function",
                "timeout": 45_000
            }),
            None,
            model::ToolCallStatus::InProgress,
        );

        let lines = render_tool_content(&tc);

        assert_eq!(
            rendered_line_texts(&lines),
            vec!["Description: Run main function", "Timeout: 45s"]
        );
    }

    #[test]
    fn code_only_input_has_no_body() {
        let tc = repl_tool_call(
            json!({ "code": "await main()" }),
            None,
            model::ToolCallStatus::InProgress,
        );

        assert!(!has_structured_body(&tc));
    }

    #[test]
    fn output_body_renders_field_rows_in_order() {
        let tc = repl_tool_call(
            json!({}),
            Some(
                "Error: boom\nStdout: out\nStderr: err\nResult: {\"ok\":true}\nRegistered tools: fetch, parse\nImages: 2\nDocuments: 1",
            ),
            model::ToolCallStatus::Failed,
        );

        let lines = render_tool_content(&tc);

        assert_eq!(
            rendered_line_texts(&lines),
            vec![
                "Error: boom",
                "Stdout: out",
                "Stderr: err",
                "Result: {\"ok\":true}",
                "Registered tools: fetch, parse",
                "Images: 2",
                "Documents: 1",
            ]
        );
    }

    #[test]
    fn raw_replay_labels_are_mapped_and_media_payloads_are_counted() {
        let tc = repl_tool_call(
            json!({}),
            Some(
                "stdout: out\nstderr: err\nresult: {\"ok\":true}\nregisteredTools: [\"fetch\",\"parse\"]\nimages: [{\"base64\":\"secret-image\",\"mediaType\":\"image/png\"}]\ndocuments: [{\"base64\":\"secret-document\"},{\"base64\":\"secret-document-2\"}]",
            ),
            model::ToolCallStatus::Completed,
        );

        let lines = render_tool_content(&tc);
        let rendered = rendered_line_texts(&lines);

        assert_eq!(
            rendered,
            vec![
                "Stdout: out",
                "Stderr: err",
                "Result: {\"ok\":true}",
                "Registered tools: fetch, parse",
                "Images: 1",
                "Documents: 2",
            ]
        );
        assert!(!rendered.iter().any(|line| line.contains("secret-image")));
        assert!(!rendered.iter().any(|line| line.contains("secret-document")));
    }

    #[test]
    fn long_result_body_stays_bounded_after_wrapping() {
        let long_result = (0..120).map(|idx| format!("word{idx}")).collect::<Vec<_>>().join(" ");
        let tc = repl_tool_call(
            json!({}),
            Some(&format!("Result: {long_result}")),
            model::ToolCallStatus::Completed,
        );

        let body = super::super::standard::render_tool_call_body(&tc, 40);
        let rendered = rendered_line_texts(&body);

        assert_eq!(body.len(), super::super::TOOL_BODY_MAX_LINES);
        assert!(rendered.iter().any(|line| line.contains("hidden")));
    }

    #[test]
    fn omission_and_failed_page_fields_render_as_structured_rows() {
        let tc = repl_tool_call(
            json!({}),
            Some(
                "Images omitted: 3\nFailed image page: 4 (file manual.pdf: render failed)\nFailed image pages omitted: 2\nDocuments omitted: 1",
            ),
            model::ToolCallStatus::Completed,
        );

        assert_eq!(
            rendered_line_texts(&render_tool_content(&tc)),
            vec![
                "Images omitted: 3",
                "Failed image page: 4 (file manual.pdf: render failed)",
                "Failed image pages omitted: 2",
                "Documents omitted: 1",
            ]
        );
    }
}
