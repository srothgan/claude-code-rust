// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::app::MessageBlock;
use serde_json::{Value, json};
use std::time::UNIX_EPOCH;

fn delivered(payload: Value, now: u64) -> String {
    let update = serde_json::from_value(payload).expect("warning fixture");
    let mut app = App::test_default();
    handle_rate_limit_update_at(&mut app, &update, UNIX_EPOCH + Duration::from_secs(now));
    let [message] = app.transcript.messages.as_slice() else { panic!("delivered warning") };
    let [MessageBlock::Notice(notice)] = message.blocks.as_slice() else { panic!("notice lane") };
    assert_eq!(notice.severity, SystemSeverity::Warning);
    notice.text.text.clone()
}

#[test]
fn allowance_guidance_obeys_independent_availability_and_current_use_observations() {
    for (fields, expected, forbidden) in [
        (json!({}), None, vec!["Extra usage", "are using", "can continue"]),
        (
            json!({"overage_enabled":false}),
            Some("Extra usage is unavailable"),
            vec!["are using", "can continue"],
        ),
        (
            json!({"overage_enabled":true}),
            Some("Extra usage is enabled"),
            vec!["are using", "can continue", "unavailable"],
        ),
        (
            json!({"overage_enabled":true,"overage_status":"allowed"}),
            Some("can continue using your overage allowance"),
            vec!["are using", "unavailable"],
        ),
        (
            json!({"overage_enabled":true,"overage_status":"allowed_warning"}),
            Some("can continue using your overage allowance"),
            vec!["are using", "unavailable"],
        ),
        (
            json!({"is_using_overage":true}),
            Some("are using your overage allowance"),
            vec!["can continue", "unavailable"],
        ),
        (
            json!({"overage_enabled":false,"is_using_overage":true}),
            Some("Extra usage is unavailable"),
            vec!["are using", "can continue"],
        ),
        (
            json!({"overage_enabled":true,"overage_status":"rejected","is_using_overage":true}),
            Some("Extra usage is unavailable"),
            vec!["are using", "can continue"],
        ),
        (
            json!({"overage_enabled":true,"overage_status":"allowed","overage_disabled_reason":"out_of_credits","is_using_overage":true}),
            Some("Extra usage is unavailable"),
            vec!["are using", "can continue"],
        ),
    ] {
        let mut payload = fields;
        payload["status"] = json!("allowed_warning");
        let text = delivered(payload, 0);
        if let Some(expected) = expected {
            assert!(text.contains(expected), "{text}");
        }
        for forbidden in forbidden {
            assert!(!text.contains(forbidden), "{text}");
        }
    }
}

#[test]
fn delivered_warning_associates_usage_window_and_shared_scope() {
    for (scope, intro) in [
        ("personal", "Approaching rate limit"),
        ("group_pool", "Approaching shared group rate limit"),
    ] {
        let text = delivered(
            json!({"status":"allowed_warning","utilization":0.92,
            "rate_limit_type":"seven_day_overage_included","limit_scope":scope}),
            0,
        );
        assert!(text.starts_with(intro));
        assert!(text.contains("92% of your 7-day overage-included rate limit"));
        assert!(!text.contains("seven_day_overage_included"));
        if scope == "personal" {
            assert!(!text.contains("shared group"));
        }
    }
    let unknown =
        delivered(json!({"status":"allowed_warning","rate_limit_type":"rolling_quota"}), 0);
    assert!(unknown.contains("rolling_quota"));
    assert!(!unknown.contains('%'));
    let absent = delivered(json!({"status":"allowed_warning"}), 0);
    assert!(!absent.contains('%'));
    for window in ["rolling_quota", "5-hour", "7-day", "daily", "shared group"] {
        assert!(!absent.contains(window));
    }
}

#[test]
fn reset_notice_uses_the_limit_reset_and_a_deterministic_clock() {
    // Literal epoch values on 1970-01-01/02 keep UTC expectations independent of local time.
    for (now, reset, expected) in [
        (36_000, Some(51_780), Some("Resets in 4h 23m at 14:23 UTC")),
        (36_000, Some(36_059), Some("Resets in < 1 minute at 10:00 UTC")),
        (36_000, Some(35_940), Some("Resets in now at 09:59 UTC")),
        (86_340, Some(86_460), Some("Resets in 2m at 00:01 UTC")),
        (36_000, None, None),
    ] {
        let text = delivered(
            json!({"status":"allowed_warning","resets_at":reset,"overage_resets_at":72_000}),
            now,
        );
        if let Some(expected) = expected {
            assert!(text.contains(expected), "{text}");
        } else {
            assert!(!text.contains("Resets"));
        }
        assert!(!text.contains("20:00 UTC"));
    }
}
