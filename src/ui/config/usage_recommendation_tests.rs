// SPDX-License-Identifier: Apache-2.0

use super::*;
use ratatui::{Terminal, backend::TestBackend};

fn render_usage_buffer(app: &mut App, width: u16, height: u16) -> Buffer {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| {
            super::render(frame, frame.area(), app);
        })
        .expect("draw");
    terminal.backend().buffer().clone()
}

fn buffer_rows(buffer: &Buffer) -> Vec<String> {
    buffer
        .content
        .chunks(usize::from(buffer.area.width))
        .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect::<String>())
        .collect()
}

fn render_usage_rows(app: &mut App, width: u16, height: u16) -> Vec<String> {
    buffer_rows(&render_usage_buffer(app, width, height))
}

fn recommendation_app() -> App {
    let raw: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/sdk-usage-recommendations.json"
    ))
    .expect("SDK contract fixture");
    let event: crate::agent::wire::BridgeEvent = serde_json::from_value(serde_json::json!({
        "event":"usage_snapshot", "session_id":"usage-session", "snapshot":{
            "five_hour":raw["rate_limits"]["five_hour"],
            "session":raw["session"],
            "activity_day":raw["behaviors"]["day"],
            "activity_week":raw["behaviors"]["week"]
        }
    }))
    .expect("wire response");
    let crate::agent::wire::BridgeEvent::UsageSnapshot { snapshot, error, .. } = event else {
        panic!("usage event");
    };
    let mut app = App::test_default();
    app.config.active_tab = crate::app::ConfigTab::Usage;
    crate::app::usage::apply_structured_sdk_result(&mut app, snapshot, error);
    app
}

#[test]
fn activity_labels_and_recommendations_have_visible_hierarchy() {
    for width in [55, 100] {
        let mut app = recommendation_app();
        let activity = app.usage.snapshot.as_mut().unwrap().activity.as_mut().unwrap();
        activity.day.as_mut().unwrap().behaviors = vec![crate::app::UsageBehaviorAttribution {
            key: "long_context".to_owned(),
            pct: 25.0,
            count: 5,
        }];
        activity.day.as_mut().unwrap().recommendations.as_mut().unwrap()[0].detail = Some(
            "Review /effort before your next task.\nUse a lower effort for routine work."
                .to_owned(),
        );
        let buffer = render_usage_buffer(&mut app, width, 80);
        let rows = buffer_rows(&buffer);
        let position = |text: &str| {
            let y = rows.iter().position(|row| row.contains(text)).expect(text);
            let x = rows[y].find(text).unwrap();
            // String offsets include the multibyte card border; buffer columns do not.
            let x = rows[y][..x].chars().count();
            (u16::try_from(x).unwrap(), u16::try_from(y).unwrap())
        };
        for label in [
            "Last 24 hours",
            "Last 7 days",
            "Activity",
            "Behaviors",
            "Recommendations",
            "Lower effort for routine work",
        ] {
            let (x, y) = position(label);
            for column in x..x + u16::try_from(label.len()).unwrap() {
                assert!(buffer[(column, y)].modifier.contains(Modifier::BOLD), "{label}");
            }
        }
        let (activity_x, activity_y) = position("Activity");
        let (count_x, count_y) = position("20 requests");
        let (behavior_x, behavior_y) = position("Behaviors");
        let (value_x, value_y) = position("long context");
        assert_eq!(activity_y, count_y);
        assert_eq!(behavior_y, value_y);
        assert_eq!(activity_x, behavior_x);
        assert_eq!(count_x, value_x, "field values share a column");
        assert!(value_x >= behavior_x + u16::try_from("Behaviors".len()).unwrap() + 2);
        assert!(!buffer[(value_x, value_y)].modifier.contains(Modifier::BOLD));
        for label in ["Recommendations", "Last 7 days", "Review the Alpha plugin"] {
            let (_, y) = position(label);
            assert!(
                rows[usize::from(y - 1)].trim_matches(['\u{2502}', ' ']).is_empty(),
                "space before {label}"
            );
        }
        let (title_x, title_y) = position("Lower effort for routine work");
        let (detail_x, detail_y) = position("Review /effort before your next task.");
        assert_eq!(detail_y, title_y + 1);
        assert_eq!(detail_x, title_x, "detail aligns with its recommendation title");
        assert_eq!(buffer[(detail_x, detail_y)].fg, theme::DIM);
        assert!(!buffer[(detail_x, detail_y)].modifier.contains(Modifier::BOLD));
        let (continuation_x, continuation_y) = position("Use a lower effort for routine work.");
        assert_eq!(continuation_y, detail_y + 1);
        assert_eq!(continuation_x, detail_x);
        assert_eq!(buffer[(continuation_x, continuation_y)].fg, theme::DIM);
        assert_eq!(rows.iter().filter(|row| row.contains("Recommendation percentages")).count(), 1);
        assert!(
            rows.iter().any(|row| row.contains("claude.ai.")),
            "wrapped footer remains visible"
        );
    }
}

#[test]
fn recommendations_keep_their_period_values_order_and_optional_details() {
    let mut app = recommendation_app();
    let rows = render_usage_rows(&mut app, 100, 50);
    let day = rows.iter().position(|row| row.contains("Last 24 hours")).expect("day");
    let week = rows.iter().position(|row| row.contains("Last 7 days")).expect("week");
    let end =
        rows.iter().position(|row| row.contains("Local transcript scan")).expect("disclaimer");
    let day_rows = &rows[day..week];
    let week_rows = &rows[week..end];
    let effort = day_rows
        .iter()
        .position(|row| row.contains("Lower effort for routine work (71%)"))
        .expect("effort recommendation");
    assert!(day_rows[effort + 1].contains("Review /effort before your next task."));
    let alpha = day_rows
        .iter()
        .position(|row| row.contains("Review the Alpha plugin (53%)"))
        .expect("first plugin");
    let beta = day_rows
        .iter()
        .position(|row| row.contains("Review the Beta plugin (35%)"))
        .expect("second plugin sharing key");
    assert!(effort < alpha && alpha < beta, "preserve SDK ordering and repeated source keys");
    assert!(week_rows.iter().any(|row| row.contains("Review the weekly workflow (81%)")));
    assert!(!week_rows.iter().any(|row| row.contains("Lower effort")));
    assert!(!day_rows.iter().any(|row| row.contains("weekly workflow")));
    assert!(
        rows[week..].iter().any(|row| row.contains("local usage affected, not estimated savings"))
    );
    assert!(!rows.iter().any(|row| row.contains("FIX-MUST-NOT-BE-SUBMITTED")));
}

#[test]
fn usage_navigation_reaches_recommendations_and_recovers_after_resize_or_refresh() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    for width in [55, 100] {
        let mut app = recommendation_app();
        let start = render_usage_rows(&mut app, width, 12);
        assert!(start.iter().any(|row| row.contains("5-hour")));
        assert!(!start.iter().any(|row| row.contains("weekly workflow")));
        crate::app::config::handle_key(&mut app, KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        let end = render_usage_rows(&mut app, width, 12);
        assert!(end.iter().any(|row| row.contains("Review the weekly workflow (81%)")), "{end:?}");
        assert!(!end.iter().any(|row| row.contains("5-hour")));
        crate::app::config::handle_key(&mut app, KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        assert_eq!(render_usage_rows(&mut app, width, 12), start);
        for _ in 0..10 {
            crate::app::config::handle_key(
                &mut app,
                KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
            );
        }
        assert_eq!(render_usage_rows(&mut app, width, 12), end);
        let expanded = render_usage_rows(&mut app, width, 80);
        assert!(expanded.iter().any(|row| row.contains("5-hour")));
        assert!(expanded.iter().any(|row| row.contains("Review the weekly workflow (81%)")));
        render_usage_rows(&mut app, width, 12);
        crate::app::config::handle_key(&mut app, KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        let replacement =
            serde_json::from_value(serde_json::json!({"five_hour":{"utilization":7}}))
                .expect("replacement snapshot");
        crate::app::usage::apply_structured_sdk_result(&mut app, Some(replacement), None);
        let refreshed = render_usage_rows(&mut app, width, 12);
        assert!(refreshed.iter().any(|row| row.contains("5-hour") && row.contains("7% used")));
        assert!(!refreshed.iter().any(|row| row.contains("weekly workflow")));
    }
}

#[test]
fn usage_distinguishes_unavailable_recommendations_from_an_empty_result() {
    let mut app = recommendation_app();
    let snapshot = serde_json::from_value(serde_json::json!({
        "activity_day":{"request_count":2,"session_count":1},
        "activity_week":{"request_count":3,"session_count":1,"recommendations":[]}
    }))
    .expect("availability snapshot");
    crate::app::usage::apply_structured_sdk_result(&mut app, Some(snapshot), None);
    let rows = render_usage_rows(&mut app, 100, 24);
    let day = rows.iter().position(|row| row.contains("Last 24 hours")).expect("day");
    let week = rows.iter().position(|row| row.contains("Last 7 days")).expect("week");
    assert!(rows[day..week].iter().any(|row| row.contains("Recommendations unavailable")));
    assert!(rows[week..].iter().any(|row| row.contains("No recommendations")));
}
