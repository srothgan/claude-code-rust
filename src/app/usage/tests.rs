use super::*;
use crate::app::UsageSourceKind;

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
