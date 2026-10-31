// SPDX-License-Identifier: Apache-2.0
// These independent literal fixtures require exact values; no approximate calculation is compared.
#![allow(clippy::float_cmp)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::test_support::{CliFixture, ProbeGuard, Server};
use super::*;
use serde_json::json;
use std::time::UNIX_EPOCH;

fn cached(value: f64) -> UsageSnapshot {
    UsageSnapshot {
        source: UsageSourceKind::Sdk,
        fetched_at: UNIX_EPOCH,
        subscription_type: Some("fixture".into()),
        five_hour: Some(UsageWindow {
            label: "5-hour".into(),
            utilization: value,
            resets_at: Some(UNIX_EPOCH + Duration::from_secs(86_400)),
            reset_description: None,
        }),
        seven_day: None,
        seven_day_oauth_apps: None,
        seven_day_opus: None,
        seven_day_sonnet: None,
        model_scoped: vec![],
        extra_usage: None,
        session: None,
        activity: None,
    }
}

#[test]
fn reset_presentation_uses_exact_time_boundaries_and_description_precedence() {
    let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    for (seconds, expected) in [
        (0, "< 1 minute"),
        (59, "< 1 minute"),
        (60, "1m"),
        (3599, "59m"),
        (3600, "1h"),
        (3660, "1h 1m"),
        (86_399, "23h 59m"),
        (86_400, "1d 0h"),
        (388_800, "4d 12h"),
        (392_399, "4d 12h"),
    ] {
        let window = UsageWindow {
            label: "5-hour".into(),
            utilization: 1.0,
            resets_at: Some(now + Duration::from_secs(seconds)),
            reset_description: Some("must not override timestamp".into()),
        };
        assert_eq!(format_window_reset_at(&window, now), Some(format!("resets in {expected}")));
    }
    let mut window = cached(1.0).five_hour.unwrap();
    window.resets_at = Some(now - Duration::from_secs(1));
    assert_eq!(format_window_reset_at(&window, now).as_deref(), Some("resets in < 1 minute"));
    window.resets_at = None;
    for (raw, expected) in [
        (None, None),
        (Some(" \t "), None),
        (Some("  Resets tomorrow 日本語  "), Some("Resets tomorrow 日本語")),
    ] {
        window.reset_description = raw.map(str::to_owned);
        assert_eq!(format_window_reset_at(&window, now).as_deref(), expected);
    }
}

#[test]
fn visible_windows_preserve_each_value_reset_and_model_priority_without_inventing_rows() {
    let mut snapshot = cached(11.0);
    snapshot.five_hour = None;
    assert!(visible_windows(&snapshot).is_empty());
    let window = |label: &str, value, seconds| UsageWindow {
        label: label.into(),
        utilization: value,
        resets_at: Some(UNIX_EPOCH + Duration::from_secs(seconds)),
        reset_description: None,
    };
    snapshot.five_hour = Some(window("5-hour", 11.0, 1));
    snapshot.seven_day = Some(window("7-day", 22.0, 2));
    snapshot.seven_day_oauth_apps = Some(window("7-day OAuth apps", 33.0, 3));
    snapshot.seven_day_sonnet = Some(window("7-day Sonnet", 44.0, 4));
    snapshot.seven_day_opus = Some(window("7-day Opus", 55.0, 5));
    snapshot.model_scoped =
        vec![window("7-day Model B", 66.0, 6), window("7-day Model A", 77.0, 7)];
    let actual = visible_windows(&snapshot);
    let expected = [
        ("5-hour", 11.0, 1),
        ("7-day", 22.0, 2),
        ("7-day OAuth apps", 33.0, 3),
        ("7-day Sonnet", 44.0, 4),
        ("7-day Opus", 55.0, 5),
        ("7-day Model B", 66.0, 6),
        ("7-day Model A", 77.0, 7),
    ];
    for (label, value, seconds) in expected {
        let matches = actual.iter().filter(|w| w.label == label).collect::<Vec<_>>();
        assert_eq!(matches.len(), 1, "{label} appears once");
        assert_eq!(matches[0].utilization, value);
        assert_eq!(matches[0].resets_at, Some(UNIX_EPOCH + Duration::from_secs(seconds)));
    }
    assert_eq!(
        actual
            .iter()
            .filter(|w| w.label.contains("Model"))
            .map(|w| w.label.as_str())
            .collect::<Vec<_>>(),
        ["7-day Model B", "7-day Model A"]
    );
    snapshot.seven_day = None;
    snapshot.seven_day_sonnet = None;
    snapshot.model_scoped.clear();
    let actual = visible_windows(&snapshot);
    assert_eq!(
        actual.iter().map(|w| w.label.as_str()).collect::<std::collections::BTreeSet<_>>(),
        ["5-hour", "7-day OAuth apps", "7-day Opus"].into_iter().collect()
    );
}

#[test]
fn sdk_conversion_keeps_account_session_and_activity_associations_across_availability() {
    let base = json!({"subscription_type":"max","five_hour":{"utilization":-5,"resets_at":"1970-01-02T00:00:00Z"},"seven_day":{"utilization":13},"seven_day_oauth_apps":{"utilization":24},"seven_day_opus":{"utilization":35},"seven_day_sonnet":{"utilization":146},"model_scoped":[{"display_name":"Model B","utilization":61},{"display_name":"Model A","utilization":72}],"extra_usage":{"monthly_limit":2000,"used_credits":1240,"utilization":123,"currency":"EUR"},"session":{"total_cost_usd":0.125,"total_api_duration_ms":900,"total_duration_ms":1500,"total_lines_added":12.5,"total_lines_removed":3,"model_count":2},"activity_day":{"request_count":4,"session_count":2,"behaviors":[{"key":"future_behavior","pct":125,"count":3}],"agents":[{"name":"agent-day","pct":11}],"skills":[{"name":"skill-day","pct":22}],"plugins":[{"name":"plugin-day","pct":33}],"mcp_servers":[{"name":"mcp-day","pct":44}]},"activity_week":{"request_count":8,"session_count":3,"behaviors":[{"key":"workflow","pct":37,"count":5}],"agents":[{"name":"agent-week","pct":55}]}});
    for available in [None, Some(true), Some(false)] {
        let mut raw = base.clone();
        if let Some(value) = available {
            raw["rate_limits_available"] = json!(value);
        }
        let snapshot = map_structured_sdk_snapshot(serde_json::from_value(raw).unwrap());
        assert_eq!(snapshot.source, UsageSourceKind::Sdk);
        assert_eq!(snapshot.subscription_type.as_deref(), Some("max"));
        assert_eq!(
            snapshot.session,
            Some(SessionUsageSummary {
                total_cost_usd: Some(0.125),
                total_api_duration_ms: Some(900.0),
                total_duration_ms: Some(1500.0),
                total_lines_added: Some(12.5),
                total_lines_removed: Some(3.0),
                model_count: Some(2)
            })
        );
        let activity = snapshot.activity.as_ref().unwrap();
        let day = activity.day.as_ref().unwrap();
        let week = activity.week.as_ref().unwrap();
        assert_eq!((day.request_count, day.session_count), (4, 2));
        assert_eq!((week.request_count, week.session_count), (8, 3));
        assert_eq!(
            day.behaviors,
            [UsageBehaviorAttribution { key: "future_behavior".into(), pct: 100.0, count: 3 }]
        );
        assert_eq!(
            week.behaviors,
            [UsageBehaviorAttribution { key: "workflow".into(), pct: 37.0, count: 5 }]
        );
        for (items, name, pct) in [
            (&day.agents, "agent-day", 11.0),
            (&day.skills, "skill-day", 22.0),
            (&day.plugins, "plugin-day", 33.0),
            (&day.mcp_servers, "mcp-day", 44.0),
            (&week.agents, "agent-week", 55.0),
        ] {
            assert_eq!(items.as_slice(), [UsageNamedAttribution { name: name.into(), pct }]);
        }
        if available == Some(false) {
            assert!(visible_windows(&snapshot).is_empty());
            assert!(snapshot.extra_usage.is_none());
        } else {
            for (window, label, value) in [
                (&snapshot.five_hour, "5-hour", 0.0),
                (&snapshot.seven_day, "7-day", 13.0),
                (&snapshot.seven_day_oauth_apps, "7-day OAuth apps", 24.0),
                (&snapshot.seven_day_opus, "7-day Opus", 35.0),
                (&snapshot.seven_day_sonnet, "7-day Sonnet", 100.0),
            ] {
                let window = window.as_ref().unwrap();
                assert_eq!((&*window.label, window.utilization), (label, value));
            }
            assert_eq!(
                snapshot.five_hour.as_ref().unwrap().resets_at,
                Some(UNIX_EPOCH + Duration::from_secs(86_400))
            );
            assert_eq!(
                snapshot
                    .model_scoped
                    .iter()
                    .map(|w| (w.label.as_str(), w.utilization))
                    .collect::<Vec<_>>(),
                [("7-day Model B", 61.0), ("7-day Model A", 72.0)]
            );
            assert_eq!(
                snapshot.extra_usage,
                Some(ExtraUsage {
                    monthly_limit: None,
                    used_credits: None,
                    utilization: Some(100.0),
                    currency: Some("EUR".into())
                })
            );
        }
    }
    let mut invalid = base;
    invalid["session"]["total_lines_added"] = json!(-1);
    invalid["session"]["total_lines_removed"] = json!(null);
    let session =
        map_structured_sdk_snapshot(serde_json::from_value(invalid).unwrap()).session.unwrap();
    assert_eq!(session.total_lines_added, None);
    assert_eq!(session.total_lines_removed, None);
}

async fn complete(app: &mut App, expected_epoch: u64) {
    loop {
        let event = tokio::time::timeout(Duration::from_secs(12), app.event_rx.recv())
            .await
            .unwrap()
            .unwrap();
        let terminal = match &event {
            ClientEvent::UsageRefreshStarted { epoch } => {
                assert_eq!(*epoch, expected_epoch);
                false
            }
            ClientEvent::UsageSnapshotReceived { epoch, .. }
            | ClientEvent::UsageRefreshFailed { epoch, .. } => {
                assert_eq!(*epoch, expected_epoch);
                true
            }
            _ => panic!("unexpected non-usage event"),
        };
        crate::app::handle_client_event(app, event);
        if terminal {
            return;
        }
    }
}

#[tokio::test(flavor = "current_thread")]
#[allow(
    clippy::too_many_lines,
    reason = "Keep the source-policy table beside its real refresh and retry workflow"
)]
async fn host_usage_fallback_preserves_source_policy_cached_data_and_retry() {
    tokio::task::LocalSet::new()
        .run_until(Box::pin(async {
            for (
                origin,
                credential,
                status,
                body,
                mode,
                cli_fails,
                expected_source,
                expected_error,
            ) in [
                (
                    "direct",
                    "valid",
                    200,
                    r#"{"five_hour":{"utilization":38}}"#,
                    "reply",
                    false,
                    UsageSourceKind::Oauth,
                    None,
                ),
                ("direct", "missing", 200, "{}", "reply", false, UsageSourceKind::Cli, None),
                ("closed", "expired", 200, "{}", "reply", false, UsageSourceKind::Cli, None),
                ("direct", "valid", 401, "denied", "reply", false, UsageSourceKind::Cli, None),
                (
                    "direct",
                    "valid",
                    500,
                    "server unavailable",
                    "reply",
                    false,
                    UsageSourceKind::Oauth,
                    Some("500"),
                ),
                (
                    "direct",
                    "valid",
                    200,
                    "bad JSON",
                    "reply",
                    false,
                    UsageSourceKind::Oauth,
                    Some("decode"),
                ),
                (
                    "direct",
                    "valid",
                    200,
                    "",
                    "disconnect",
                    false,
                    UsageSourceKind::Oauth,
                    Some("network"),
                ),
                (
                    "direct",
                    "missing",
                    200,
                    "{}",
                    "reply",
                    true,
                    UsageSourceKind::Cli,
                    Some("exit code 7"),
                ),
                (
                    "sdk",
                    "valid",
                    401,
                    "denied",
                    "reply",
                    true,
                    UsageSourceKind::Cli,
                    Some("SDK fixture refusal"),
                ),
            ] {
                let fixture = CliFixture::new();
                if cli_fails {
                    fixture.configure("fail", "fixture refusal", "");
                }
                let mut server =
                    Server::new(status, body, mode, Some(fixture.project.join("attempts")));
                let mut oauth = server.probe();
                if credential == "missing" {
                    oauth.credentials = None;
                } else if credential == "expired" {
                    oauth.credentials.as_mut().unwrap().expires_at = Some(UNIX_EPOCH);
                }
                let _guard = ProbeGuard::install(HostProbe { oauth, cli: fixture.probe() });
                let (mut app, commands) = crate::app::test_support::app_with_connection();
                let mut commands = Some(commands);
                if origin == "direct" {
                    app.session_runtime.conn = None;
                }
                if origin == "closed" {
                    drop(commands.take());
                }
                app.cwd_raw = fixture.project.to_str().unwrap().into();
                app.session_runtime.session_scope_epoch = 7;
                app.usage.snapshot = Some(cached(9.0));
                request_refresh(&mut app);
                if origin == "sdk" {
                    assert_eq!(
                        commands.as_mut().unwrap().try_recv().unwrap().command,
                        crate::agent::wire::BridgeCommand::GetUsage {
                            session_id: "session-1".into()
                        }
                    );
                    assert!(app.event_rx.try_recv().is_err());
                    crate::app::handle_client_event(
                        &mut app,
                        ClientEvent::StructuredUsageReceived {
                            session_id: "session-1".into(),
                            snapshot: None,
                            error: Some("SDK fixture refusal".into()),
                        },
                    );
                }
                complete(&mut app, 7).await;
                assert!(!app.usage.in_flight);
                let journal = std::fs::read_to_string(fixture.project.join("attempts")).unwrap();
                let expected = if credential != "valid" {
                    "cli\n"
                } else if expected_source == UsageSourceKind::Cli {
                    "oauth\ncli\n"
                } else {
                    "oauth\n"
                };
                assert_eq!(journal, expected);
                assert_eq!(app.usage.last_attempted_source, Some(expected_source));
                if expected_source == UsageSourceKind::Cli {
                    fixture.assert_invocation();
                }
                if let Some(cause) = expected_error {
                    let error = app.usage.last_error.as_ref().unwrap();
                    assert!(error.contains(cause), "{error}");
                    if expected_source == UsageSourceKind::Cli {
                        assert!(error.contains("OAuth") && error.contains("CLI"));
                        assert!(error.contains("exit code 7") && error.contains("fixture refusal"));
                        assert!(error.contains(if credential == "missing" {
                            "No Claude OAuth credentials"
                        } else {
                            "rejected"
                        }));
                    }
                    assert_eq!(app.usage.snapshot, Some(cached(9.0)));
                    app.session_runtime.conn = None;
                    let mut retry = Server::new(
                        200,
                        r#"{"five_hour":{"utilization":64}}"#,
                        "reply",
                        Some(fixture.project.join("attempts")),
                    );
                    let _retry = ProbeGuard::install(HostProbe {
                        oauth: retry.probe(),
                        cli: fixture.probe(),
                    });
                    request_refresh(&mut app);
                    complete(&mut app, 7).await;
                    assert_eq!(
                        app.usage
                            .snapshot
                            .as_ref()
                            .unwrap()
                            .five_hour
                            .as_ref()
                            .unwrap()
                            .utilization,
                        64.0
                    );
                    assert!(app.usage.last_error.is_none());
                    assert!(!app.usage.in_flight);
                    retry.finish().await;
                } else {
                    let snapshot = app.usage.snapshot.as_ref().unwrap();
                    assert_eq!(snapshot.source, expected_source);
                    assert_eq!(
                        snapshot.five_hour.as_ref().unwrap().utilization,
                        if expected_source == UsageSourceKind::Oauth { 38.0 } else { 23.0 }
                    );
                    assert!(app.usage.last_error.is_none());
                }
                server.finish().await;
            }
        }))
        .await;
}

#[test]
fn stale_usage_events_preserve_current_data_error_and_request_until_matching_completion() {
    let (mut app, _) = crate::app::test_support::app_with_connection();
    app.session_runtime.session_scope_epoch = 8;
    app.usage.snapshot = Some(cached(71.0));
    app.usage.last_error = Some("current failure".into());
    app.usage.last_attempted_source = Some(UsageSourceKind::Oauth);
    app.usage.in_flight = true;
    for event in [
        ClientEvent::UsageRefreshStarted { epoch: 7 },
        ClientEvent::UsageRefreshFailed {
            epoch: 7,
            message: "obsolete failure".into(),
            source: UsageSourceKind::Cli,
        },
        ClientEvent::UsageSnapshotReceived { epoch: 7, snapshot: cached(12.0) },
    ] {
        crate::app::handle_client_event(&mut app, event);
        assert_eq!(app.usage.snapshot, Some(cached(71.0)));
        assert_eq!(app.usage.last_error.as_deref(), Some("current failure"));
        assert_eq!(app.usage.last_attempted_source, Some(UsageSourceKind::Oauth));
        assert!(app.usage.in_flight);
    }
    crate::app::handle_client_event(
        &mut app,
        ClientEvent::UsageSnapshotReceived { epoch: 8, snapshot: cached(83.0) },
    );
    assert_eq!(app.usage.snapshot, Some(cached(83.0)));
    assert_eq!(app.usage.last_attempted_source, Some(UsageSourceKind::Sdk));
    assert!(app.usage.last_error.is_none());
    assert!(!app.usage.in_flight);
}

#[tokio::test(flavor = "current_thread")]
async fn held_host_refresh_carries_original_epoch_and_cwd_and_empty_failure_stays_empty() {
    tokio::task::LocalSet::new()
        .run_until(Box::pin(async {
            let fixture = CliFixture::new();
            let newer_project = tempfile::tempdir().unwrap();
            let mut server =
                Server::new(401, "denied", "gated", Some(fixture.project.join("attempts")));
            let _guard =
                ProbeGuard::install(HostProbe { oauth: server.probe(), cli: fixture.probe() });
            let (mut app, _) = crate::app::test_support::app_with_connection();
            app.session_runtime.conn = None;
            app.session_runtime.session_scope_epoch = 7;
            app.cwd_raw = fixture.project.to_str().unwrap().into();
            request_refresh(&mut app);
            let request = tokio::time::timeout(Duration::from_secs(10), &mut server.request)
                .await
                .unwrap()
                .unwrap();
            assert!(request.starts_with("GET /api/oauth/usage "));
            app.session_runtime.session_scope_epoch = 8;
            app.cwd_raw = newer_project.path().to_str().unwrap().into();
            app.usage.snapshot = Some(cached(71.0));
            app.usage.last_error = Some("current failure".into());
            app.usage.in_flight = true;
            server.release();
            complete(&mut app, 7).await;
            fixture.assert_invocation();
            assert_eq!(
                std::fs::read_to_string(fixture.project.join("attempts")).unwrap(),
                "oauth\ncli\n"
            );
            assert_eq!(std::fs::read_dir(newer_project.path()).unwrap().count(), 0);
            assert_eq!(app.usage.snapshot, Some(cached(71.0)));
            assert_eq!(app.usage.last_error.as_deref(), Some("current failure"));
            assert!(app.usage.in_flight);
            server.finish().await;

            let mut failure = Server::new(503, "fixture outage", "reply", None);
            let _failure =
                ProbeGuard::install(HostProbe { oauth: failure.probe(), cli: fixture.probe() });
            let (mut empty, _) = crate::app::test_support::app_with_connection();
            empty.session_runtime.conn = None;
            request_refresh(&mut empty);
            let epoch = empty.session_runtime.session_scope_epoch;
            complete(&mut empty, epoch).await;
            assert!(empty.usage.snapshot.is_none());
            assert!(empty.usage.last_error.as_deref().unwrap().contains("503"));
            assert!(!empty.usage.in_flight);
            failure.finish().await;
        }))
        .await;
}
