// SPDX-License-Identifier: Apache-2.0

//! SDK result sections share one timestamp formatter and preserve per-item boundaries.

use crate::agent::model::{ToolResultPresentation, ToolResultValue};
use crate::app::presentation::ClockFormat;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

pub(super) fn render(
    result: &ToolResultPresentation,
    clock: ClockFormat<'_>,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for (index, section) in result.sections.iter().enumerate() {
        if index > 0 {
            lines.push(Line::from(Span::styled(
                "─".repeat(16),
                Style::default().fg(crate::ui::theme::DIM),
            )));
        }
        for field in &section.fields {
            let value = match &field.value {
                ToolResultValue::Text(text) => text.clone(),
                ToolResultValue::Timestamp(timestamp) => {
                    let Some(value) = clock.timestamp(timestamp) else { continue };
                    value
                }
            };
            let mut values = value.lines();
            lines.push(super::fields::render_dynamic_field(
                field.label.clone(),
                values.next().unwrap_or_default().to_owned(),
            ));
            lines.extend(values.map(|line| Line::from(line.to_owned())));
        }
        if let Some(text) = &section.text {
            lines.extend(text.lines().map(|line| Line::from(line.to_owned())));
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(result: &ToolResultPresentation, clock: ClockFormat<'_>) -> Vec<String> {
        render(result, clock).into_iter().map(|row| row.to_string()).collect()
    }

    fn item_region<'a>(rows: &'a [String], label: &str, id: &str) -> &'a [String] {
        let identity = format!("{label}: {id}");
        let matches: Vec<_> =
            rows.iter().enumerate().filter(|(_, row)| **row == identity).collect();
        assert_eq!(matches.len(), 1, "each item must have its own unique identity: {id}");
        let start = matches[0].0;
        let prefix = format!("{label}:");
        let end = rows[start + 1..]
            .iter()
            .position(|row| row.starts_with(&prefix))
            .map_or(rows.len(), |next| start + 1 + next);
        &rows[start..end]
    }

    fn assert_field(rows: &[String], label: &str, expected: &[&str]) {
        let prefix = format!("{label}: ");
        let values: Vec<_> = rows.iter().filter_map(|row| row.strip_prefix(&prefix)).collect();
        assert_eq!(values, expected, "field {label} must belong to this item");
    }

    #[test]
    fn each_saved_job_keeps_its_own_scheduler_instant_across_the_repeated_dst_hour() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../tests/fixtures/tool-result-times.json"))
                .expect("SDK result fixtures");
        let result =
            serde_json::from_value(fixture[1]["presentation"].clone()).expect("cron sections");
        let rendered =
            rows(&result, ClockFormat { format: "%F %T.%3f %z", zone: Some("Europe/Berlin") });
        for (id, prompt, next_run) in [
            ("job-a", "First job", Some("2026-10-25 02:30:11.321 +0200")),
            ("job-b", "Second job", Some("2026-10-25 02:30:23.456 +0100")),
            ("job-old", "Old job", None),
        ] {
            let region = item_region(&rendered, "Schedule ID", id);
            assert_field(region, "Prompt", &[prompt]);
            assert_field(region, "Next run", &next_run.into_iter().collect::<Vec<_>>());
        }
    }

    #[test]
    fn a_partial_drain_labels_the_drain_and_each_delivery_without_interpreting_payloads() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../tests/fixtures/tool-result-times.json"))
                .expect("SDK result fixtures");
        let result =
            serde_json::from_value(fixture[2]["presentation"].clone()).expect("queue sections");
        let rendered =
            rows(&result, ClockFormat { format: "%F %T.%3f %z", zone: Some("Europe/Berlin") });
        let first_delivery = rendered
            .iter()
            .position(|row| row.starts_with("Notification ID:"))
            .expect("delivery region");
        let drain = &rendered[..first_delivery];
        assert_field(drain, "Remaining", &["4"]);
        assert_field(drain, "Read at", &["2026-03-29 03:10:12.345 +0200"]);
        for (id, origin, queued, arrived, payload, other_payload) in [
            (
                "delivery-a",
                "github_webhook",
                "2026-03-29 01:59:30.000 +0100",
                "2026-03-29 03:00:01.250 +0200",
                "<user-tag>\nKeep this **literal**\n</user-tag>",
                "Second payload",
            ),
            (
                "delivery-b",
                "future_origin",
                "2026-03-29 03:05:00.000 +0200",
                "2026-03-29 03:09:02.500 +0200",
                "Second payload",
                "Keep this **literal**",
            ),
        ] {
            let region = item_region(&rendered, "Notification ID", id);
            assert_field(region, "Origin", &[origin]);
            assert_field(region, "Queued at", &[queued]);
            assert_field(region, "Arrived at", &[arrived]);
            assert_field(region, "Remaining", &[]);
            assert_field(region, "Read at", &[]);
            let literal: Vec<_> = payload.lines().collect();
            assert!(
                region.windows(literal.len()).any(|rows| rows == literal.as_slice()),
                "literal payload missing from {id}: {region:?}"
            );
            assert!(
                !region.iter().any(|row| row == other_payload),
                "another delivery's payload leaked into {id}"
            );
        }
    }

    #[test]
    fn invalid_optional_times_stay_unknown_and_never_hide_the_delivery() {
        let result: ToolResultPresentation = serde_json::from_value(serde_json::json!({
            "sections": [{"fields": [
                {"label":"Notification ID", "value":{"type":"text","value":"older-delivery"}},
                {"label":"Arrived at", "value":{"type":"timestamp","value":"not-a-time"}},
                {"label":"Read at", "value":{"type":"timestamp","value":"2026-02-30T01:02:03Z"}}
            ], "text":"Saved payload"}]
        }))
        .expect("result");
        for format in ["12-hour", "24-hour", "24-hour-utc", "%H:%M:%S"] {
            assert_eq!(
                rows(&result, ClockFormat { format, zone: Some("Europe/Berlin") }),
                ["Notification ID: older-delivery", "Saved payload"]
            );
        }
    }
}
