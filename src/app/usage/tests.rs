// SPDX-License-Identifier: Apache-2.0
use super::*;

#[test]
fn sdk_behavior_keys_keep_their_own_attribution_during_conversion() {
    let raw: serde_json::Value =
        serde_json::from_str(include_str!("../../../tests/fixtures/sdk-usage-attribution.json"))
            .expect("SDK fixture");
    let event: crate::agent::wire::BridgeEvent = serde_json::from_value(serde_json::json!({
        "event":"usage_snapshot", "session_id":"usage-session", "snapshot":{
            "rate_limits_available":false,
            "activity_day":raw["behaviors"]["day"], "activity_week":raw["behaviors"]["week"]
        }
    }))
    .expect("usage wire event");
    let crate::agent::wire::BridgeEvent::UsageSnapshot { snapshot: Some(snapshot), .. } = event
    else {
        panic!("usage snapshot")
    };
    let mut app = App::test_default();
    super::apply_structured_sdk_result(&mut app, Some(snapshot), None);
    let activity = app
        .usage
        .snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.activity.as_ref())
        .expect("activity");
    for (window, requests, sessions, expected) in [
        (
            activity.day.as_ref().expect("day"),
            20,
            2,
            vec![
                ("max_effort", 71.0, 7),
                ("workflow", 62.0, 6),
                ("always_on_context", 53.0, 5),
                ("large_tool_results", 44.0, 4),
                ("future_behavior", 35.0, 3),
                ("sixth_behavior", 26.0, 2),
            ],
        ),
        (
            activity.week.as_ref().expect("week"),
            100,
            9,
            vec![
                ("max_effort", 81.0, 17),
                ("workflow", 72.0, 16),
                ("always_on_context", 63.0, 15),
                ("large_tool_results", 54.0, 14),
                ("future_behavior", 45.0, 13),
                ("sixth_behavior", 36.0, 12),
            ],
        ),
    ] {
        assert_eq!(window.request_count, requests);
        assert_eq!(window.session_count, sessions);
        let actual: Vec<_> =
            window.behaviors.iter().map(|item| (item.key.as_str(), item.pct, item.count)).collect();
        assert_eq!(actual, expected);
        assert!(window.behaviors.iter().map(|item| item.pct).sum::<f64>() > 100.0);
    }
}
