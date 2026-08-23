// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::app::UsageSourceKind;

#[test]
fn formats_day_scale_reset() {
    let target = SystemTime::now() + Duration::from_secs(4 * 24 * 60 * 60 + 12 * 60 * 60);
    let formatted = format_window_reset(&UsageWindow {
        label: "7-day".to_owned(),
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
        label: "7-day".to_owned(),
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
        subscription_type: None,
        five_hour: Some(UsageWindow {
            label: "5-hour".to_owned(),
            utilization: 10.0,
            resets_at: None,
            reset_description: None,
        }),
        seven_day: None,
        seven_day_oauth_apps: None,
        seven_day_opus: Some(UsageWindow {
            label: "7-day Opus".to_owned(),
            utilization: 30.0,
            resets_at: None,
            reset_description: None,
        }),
        seven_day_sonnet: None,
        model_scoped: Vec::new(),
        extra_usage: None,
        session: None,
        activity: None,
    };

    let labels = visible_windows(&snapshot)
        .into_iter()
        .map(|window| window.label.as_str())
        .collect::<Vec<_>>();
    assert_eq!(labels, vec!["5-hour", "7-day Opus"]);
}

#[test]
fn maps_structured_sdk_usage_without_conflating_session_and_account_totals() {
    let snapshot = map_structured_sdk_snapshot(StructuredUsageSnapshot {
        subscription_type: Some("max".to_owned()),
        rate_limits_available: Some(true),
        five_hour: Some(crate::agent::types::StructuredUsageWindow {
            utilization: 42.0,
            resets_at: Some("2026-08-12T10:00:00Z".to_owned()),
        }),
        seven_day: None,
        seven_day_oauth_apps: None,
        seven_day_opus: None,
        seven_day_sonnet: None,
        model_scoped: vec![crate::agent::types::StructuredModelUsageWindow {
            display_name: "Fable".to_owned(),
            utilization: 33.0,
            resets_at: None,
        }],
        extra_usage: Some(crate::agent::types::StructuredExtraUsage {
            monthly_limit: Some(2_000.0),
            used_credits: Some(375.0),
            utilization: Some(18.75),
            currency: Some("USD".to_owned()),
        }),
        session: Some(crate::agent::types::StructuredSessionUsage {
            total_cost_usd: Some(0.125),
            total_api_duration_ms: Some(1_500.0),
            total_duration_ms: Some(2_000.0),
            total_lines_added: Some(12.0),
            total_lines_removed: Some(3.0),
            model_count: Some(2),
        }),
        activity_day: Some(crate::agent::types::StructuredActivityWindow {
            request_count: 4,
            session_count: 2,
            behaviors: vec![crate::agent::types::StructuredBehaviorAttribution {
                key: "long_context".to_owned(),
                pct: 25.0,
                count: 1,
            }],
            agents: vec![crate::agent::types::StructuredNamedAttribution {
                name: "Explore".to_owned(),
                pct: 40.0,
            }],
            skills: Vec::new(),
            plugins: Vec::new(),
            mcp_servers: Vec::new(),
        }),
        activity_week: None,
    });

    assert_eq!(snapshot.source, UsageSourceKind::Sdk);
    assert_eq!(snapshot.subscription_type.as_deref(), Some("max"));
    assert_eq!(snapshot.five_hour.as_ref().map(|window| window.utilization), Some(42.0));
    assert_eq!(snapshot.extra_usage.as_ref().and_then(|extra| extra.monthly_limit), None);
    assert_eq!(snapshot.extra_usage.as_ref().and_then(|extra| extra.used_credits), None);
    assert_eq!(snapshot.extra_usage.as_ref().and_then(|extra| extra.utilization), Some(18.75));
    assert_eq!(
        snapshot.model_scoped.first().map(|window| window.label.as_str()),
        Some("7-day Fable")
    );
    assert_eq!(snapshot.session.as_ref().and_then(|session| session.total_lines_added), Some(12.0));
    assert_eq!(
        snapshot
            .activity
            .as_ref()
            .and_then(|activity| activity.day.as_ref())
            .map(|day| day.request_count),
        Some(4)
    );
    assert_eq!(
        snapshot
            .activity
            .as_ref()
            .and_then(|activity| activity.day.as_ref())
            .and_then(|day| day.behaviors.first())
            .map(|behavior| behavior.key.as_str()),
        Some("long_context")
    );
}

#[test]
fn suppresses_plan_windows_when_sdk_reports_rate_limits_unavailable() {
    let snapshot = map_structured_sdk_snapshot(StructuredUsageSnapshot {
        subscription_type: None,
        rate_limits_available: Some(false),
        five_hour: Some(crate::agent::types::StructuredUsageWindow {
            utilization: 42.0,
            resets_at: None,
        }),
        seven_day: None,
        seven_day_oauth_apps: None,
        seven_day_opus: None,
        seven_day_sonnet: None,
        model_scoped: Vec::new(),
        extra_usage: None,
        session: None,
        activity_day: None,
        activity_week: None,
    });

    assert!(snapshot.five_hour.is_none());
}
