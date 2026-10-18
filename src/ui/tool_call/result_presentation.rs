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

    #[test]
    fn each_saved_job_keeps_its_own_scheduler_instant_across_the_repeated_dst_hour() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../tests/fixtures/tool-result-times.json"))
                .expect("SDK result fixtures");
        let result =
            serde_json::from_value(fixture[1]["presentation"].clone()).expect("cron sections");
        let rendered =
            rows(&result, ClockFormat { format: "%F %T.%3f %z", zone: Some("Europe/Berlin") });
        let sections: Vec<_> = rendered.split(|line| line == &"\u{2500}".repeat(16)).collect();
        assert!(sections[0].contains(&"Schedule ID: job-a".to_owned()));
        assert!(sections[0].contains(&"Next run: 2026-10-25 02:30:11.321 +0200".to_owned()));
        assert!(!sections[0].iter().any(|line| line.contains("23.456")));
        assert!(sections[1].contains(&"Schedule ID: job-b".to_owned()));
        assert!(sections[1].contains(&"Next run: 2026-10-25 02:30:23.456 +0100".to_owned()));
        assert!(sections[2].contains(&"Schedule ID: job-old".to_owned()));
        assert!(!sections[2].iter().any(|line| line.starts_with("Next run:")));
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
        let sections: Vec<_> = rendered.split(|line| line == &"\u{2500}".repeat(16)).collect();
        assert_eq!(sections[0], ["Remaining: 4", "Read at: 2026-03-29 03:10:12.345 +0200"]);
        assert!(sections[1].contains(&"Notification ID: delivery-a".to_owned()));
        assert!(sections[1].contains(&"Queued at: 2026-03-29 01:59:30.000 +0100".to_owned()));
        assert!(sections[1].contains(&"Arrived at: 2026-03-29 03:00:01.250 +0200".to_owned()));
        assert!(sections[1].ends_with(&[
            "<user-tag>".to_owned(),
            "Keep this **literal**".to_owned(),
            "</user-tag>".to_owned()
        ]));
        assert!(sections[2].contains(&"Notification ID: delivery-b".to_owned()));
        assert!(sections[2].contains(&"Origin: future_origin".to_owned()));
        assert!(sections[2].contains(&"Arrived at: 2026-03-29 03:09:02.500 +0200".to_owned()));
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
