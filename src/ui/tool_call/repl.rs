// SPDX-License-Identifier: Apache-2.0

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
    fn repl_cards_associate_partial_channels_failures_and_retained_result_tails() {
        use crate::ui::tool_call::test_support::card_body;
        let mut failed = repl_tool_call(
            json!({"code":"private_requested_code()"}),
            Some("Error: EXECUTION-REFUSED\nStdout: partial-out-17\nStderr: partial-err-93"),
            model::ToolCallStatus::Failed,
        );
        let rows = card_body(&mut failed, 120);
        for expected in
            ["Error: EXECUTION-REFUSED", "Stdout: partial-out-17", "Stderr: partial-err-93"]
        {
            assert!(rows.contains(&expected.to_owned()), "{rows:?}");
        }
        assert!(
            !rows
                .iter()
                .any(|row| row.contains("private_requested_code") || row.starts_with("Result:"))
        );
        let mut partial = repl_tool_call(
            json!({}),
            Some(
                "Result: {\"answer\":17}\nRegistered tools: fetch-events, decode-record\nImages: 2\nDocuments: 1\nImages omitted: 3\nFailed image page: 4 (file manual.pdf: render failed)\nFailed image pages omitted: 2\nDocuments omitted: 1",
            ),
            model::ToolCallStatus::Completed,
        );
        let rows = card_body(&mut partial, 120);
        for expected in [
            "Result: {\"answer\":17}",
            "Registered tools: fetch-events, decode-record",
            "Images: 2",
            "Documents: 1",
            "Images omitted: 3",
            "Failed image page: 4 (file manual.pdf: render failed)",
            "Failed image pages omitted: 2",
            "Documents omitted: 1",
        ] {
            assert!(rows.contains(&expected.to_owned()), "{rows:?}");
        }
        let content = format!("Result: {} FINAL-RESULT-TAIL", "omitted-middle ".repeat(120));
        let mut long = repl_tool_call(json!({}), Some(&content), model::ToolCallStatus::Completed);
        crate::ui::tool_call::test_support::set_result_fields(
            &mut long,
            &[("Result", content.strip_prefix("Result: ").expect("result fixture"))],
        );
        let rows = card_body(&mut long, 40);
        assert!(rows.iter().any(|row| row.contains("FINAL-RESULT-TAIL")), "{rows:?}");
        assert!(rows.iter().any(|row| row.contains("hidden")), "{rows:?}");
        assert!(!rows.iter().any(|row| row.len() > 40));
        assert_eq!(long.content[0], model::ToolCallContent::from(content.as_str()));
    }

    #[test]
    fn confirmed_repl_channels_keep_label_looking_payloads_literal_in_the_normal_card() {
        use crate::ui::tool_call::test_support::{card_body, confirmed_receipt};
        for labels in [vec!["Stdout", "Images", "Documents"], vec!["Stderr", "Result"]] {
            let mut tc = confirmed_receipt("repl-literal-channels");
            let original = tc.content.clone();
            let metadata = tc.output_metadata.as_mut().expect("wire metadata");
            let presentation = metadata.presentation.as_mut().expect("wire sections");
            // Compact independently framed channel cases fit without clipping; the canonical receipt remains intact.
            presentation.sections[0].fields.retain(|field| labels.contains(&field.label.as_str()));
            let rows = card_body(&mut tc, 120);
            if labels.contains(&"Stdout") {
                let start = rows
                    .iter()
                    .position(|row| row == "Stdout: first stdout")
                    .expect("stdout label");
                assert_eq!(
                    &rows[start + 1..start + 5],
                    ["Images: [1,2]", "Result: literal stdout value", "", "last stdout"]
                );
                assert!(rows.contains(&"Images: 2".to_owned()));
                assert!(rows.contains(&"Documents: 1".to_owned()));
                assert!(!rows.iter().any(|row| row.contains("stderr only")));
            } else {
                let start =
                    rows.iter().position(|row| row == "Stderr: stderr only").expect("stderr label");
                assert_eq!(rows[start + 1], "Documents: [fake]");
                assert!(rows.contains(&"Result: {\"actual\":false,\"count\":9}".to_owned()));
                assert!(!rows.iter().any(|row| row.contains("first stdout")));
            }
            assert!(
                !rows
                    .iter()
                    .any(|row| row.contains("PRIVATE-IMAGE") || row.contains("PRIVATE-DOCUMENT"))
            );
            assert_eq!(tc.content, original);
        }
    }
}
