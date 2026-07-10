// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::app::{ExtraUsage, UsageSourceKind};

#[test]
fn formats_day_scale_reset() {
    let target = SystemTime::now() + Duration::from_secs(4 * 24 * 60 * 60 + 12 * 60 * 60);
    let formatted = format_window_reset(&UsageWindow {
        label: "7-day",
        utilization: 50.0,
        resets_at: Some(target),
        reset_description: None,
    })
    .expect("formatted reset");
    assert!(formatted.starts_with("resets in 4d "));
}

#[test]
fn prefers_reset_description_when_no_timestamp_exists() {
    let window = UsageWindow {
        label: "7-day",
        utilization: 40.0,
        resets_at: None,
        reset_description: Some("Resets Feb 12 at 1:30pm (Asia/Calcutta)".to_owned()),
    };
    assert_eq!(
        format_window_reset(&window),
        Some("Resets Feb 12 at 1:30pm (Asia/Calcutta)".to_owned())
    );
}

#[test]
fn collects_only_present_windows() {
    let snapshot = UsageSnapshot {
        source: UsageSourceKind::Oauth,
        fetched_at: SystemTime::now(),
        five_hour: Some(UsageWindow {
            label: "5-hour",
            utilization: 10.0,
            resets_at: None,
            reset_description: None,
        }),
        seven_day: None,
        seven_day_opus: Some(UsageWindow {
            label: "7-day Opus",
            utilization: 30.0,
            resets_at: None,
            reset_description: None,
        }),
        seven_day_sonnet: None,
        extra_usage: None,
    };

    let labels =
        visible_windows(&snapshot).into_iter().map(|window| window.label).collect::<Vec<_>>();
    assert_eq!(labels, vec!["5-hour", "7-day Opus"]);
}

#[test]
fn formats_limits_summary_as_markdown_table() {
    let snapshot = UsageSnapshot {
        source: UsageSourceKind::Oauth,
        fetched_at: SystemTime::now(),
        five_hour: Some(UsageWindow {
            label: "5-hour",
            utilization: 47.4,
            resets_at: None,
            reset_description: Some("resets in 2h 14m".to_owned()),
        }),
        seven_day: Some(UsageWindow {
            label: "7-day",
            utilization: 62.0,
            resets_at: None,
            reset_description: Some("resets in 4d 11h".to_owned()),
        }),
        seven_day_opus: None,
        seven_day_sonnet: None,
        extra_usage: Some(ExtraUsage {
            monthly_limit: Some(20.0),
            used_credits: Some(12.4),
            utilization: Some(62.0),
            currency: Some("USD".to_owned()),
        }),
    };

    let summary = format_limits_summary(&snapshot);

    assert!(summary.contains("| Window | Used | Reset |"));
    assert!(summary.contains("| 5-hour | 47% | resets in 2h 14m |"));
    assert!(summary.contains("| 7-day | 62% | resets in 4d 11h |"));
    assert!(summary.contains("| Extra credits | Used |"));
    assert!(summary.contains("| USD | 12.40 / 20.00 |"));
}

#[test]
fn limits_summary_omits_absent_optional_sections() {
    let snapshot = UsageSnapshot {
        source: UsageSourceKind::Oauth,
        fetched_at: SystemTime::now(),
        five_hour: Some(UsageWindow {
            label: "5-hour",
            utilization: 10.0,
            resets_at: None,
            reset_description: None,
        }),
        seven_day: None,
        seven_day_opus: None,
        seven_day_sonnet: None,
        extra_usage: None,
    };

    let summary = format_limits_summary(&snapshot);

    assert!(summary.contains("| 5-hour | 10% | unavailable |"));
    assert!(!summary.contains("7-day"));
    assert!(!summary.contains("Extra credits"));
}

#[test]
fn limits_summary_escapes_markdown_table_cells() {
    let snapshot = UsageSnapshot {
        source: UsageSourceKind::Oauth,
        fetched_at: SystemTime::now(),
        five_hour: Some(UsageWindow {
            label: "5-hour",
            utilization: 10.0,
            resets_at: None,
            reset_description: Some("resets | soon\nreally".to_owned()),
        }),
        seven_day: None,
        seven_day_opus: None,
        seven_day_sonnet: None,
        extra_usage: None,
    };

    let summary = format_limits_summary(&snapshot);

    assert!(summary.contains("| 5-hour | 10% | resets \\| soon really |"));
}
