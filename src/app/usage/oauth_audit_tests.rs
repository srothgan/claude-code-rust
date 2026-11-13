// SPDX-License-Identifier: Apache-2.0
// These independent literal fixtures require exact values; no approximate calculation is compared.
#![allow(clippy::float_cmp)]

use super::super::test_support::Server;
use super::*;
use serde_json::json;

#[test]
fn usage_reset_inputs_resolve_to_independently_known_instants() {
    for (raw, seconds, nanos) in [
        ("1970-01-02T00:00:00Z", 86_400, 0),
        ("1970-01-02T01:30:00+01:30", 86_400, 0),
        ("1970-01-01T20:00:00-04:00", 86_400, 0),
        ("1970-01-02T00:00:00.125Z", 86_400, 125_000_000),
        ("2024-02-29T00:00:00Z", 1_709_164_800, 0),
        ("1735128000", 1_735_128_000, 0),
        ("1735128000123", 1_735_128_000, 123_000_000),
    ] {
        let expected = Some(UNIX_EPOCH + Duration::new(seconds, nanos));
        assert_eq!(parse_timestamp(raw), expected, "{raw}");
        let snapshot = decode_usage_payload(
            json!({"five_hour":{"utilization":1,"resets_at":raw}}).to_string().as_bytes(),
        )
        .unwrap();
        assert_eq!(snapshot.five_hour.unwrap().resets_at, expected);
    }
    for raw in [json!(1_735_128_000_i64), json!(1_735_128_000_123_i64)] {
        let nanos = if raw == json!(1_735_128_000_i64) { 0 } else { 123_000_000 };
        assert_eq!(
            parse_timestamp_value(&raw),
            Some(UNIX_EPOCH + Duration::new(1_735_128_000, nanos))
        );
    }
}

#[test]
fn oauth_windows_keep_distinct_values_resets_and_optional_availability() {
    let snapshot=decode_usage_payload(br#"{"five_hour":{"utilization":12.5,"resets_at":"1970-01-02T00:00:00Z"},"seven_day":{"utilization":23,"resets_at":1735128000123},"seven_day_oauth_apps":{"utilization":34,"resets_at":"invalid"},"seven_day_opus":{"utilization":45},"seven_day_sonnet":{"utilization":56,"resets_at":"1970-01-03T00:00:00Z"},"unknown_field":{"future":true}}"#).unwrap();
    for (window, label, value, reset) in [
        (snapshot.five_hour, "5-hour", 12.5, Some(UNIX_EPOCH + Duration::from_secs(86_400))),
        (
            snapshot.seven_day,
            "7-day",
            23.0,
            Some(UNIX_EPOCH + Duration::new(1_735_128_000, 123_000_000)),
        ),
        (snapshot.seven_day_oauth_apps, "7-day OAuth apps", 34.0, None),
        (snapshot.seven_day_opus, "7-day Opus", 45.0, None),
        (
            snapshot.seven_day_sonnet,
            "7-day Sonnet",
            56.0,
            Some(UNIX_EPOCH + Duration::from_secs(172_800)),
        ),
    ] {
        let window = window.unwrap();
        assert_eq!(window.label, label);
        assert_eq!(window.utilization, value);
        assert_eq!(window.resets_at, reset);
        assert_eq!(window.reset_description, None);
    }
    assert_eq!(snapshot.source, UsageSourceKind::Oauth);
    for (raw, expected) in [(-12.0, 0.0), (0.0, 0.0), (112.0, 100.0)] {
        let snapshot=decode_usage_payload(json!({"five_hour":{"utilization":raw},"seven_day":{"resets_at":"1970-01-02T00:00:00Z"}}).to_string().as_bytes()).unwrap();
        assert_eq!(snapshot.five_hour.unwrap().utilization, expected);
        assert!(
            snapshot.seven_day.is_none()
                && snapshot.seven_day_oauth_apps.is_none()
                && snapshot.seven_day_opus.is_none()
                && snapshot.seven_day_sonnet.is_none()
        );
        assert!(snapshot.model_scoped.is_empty() && snapshot.extra_usage.is_none());
    }
    for body in
        ["not-json", "{}", r#"{"five_hour":{}}"#, r#"{"five_hour":{"utilization":"wrong-type"}}"#]
    {
        assert!(matches!(decode_usage_payload(body.as_bytes()), Err(OauthFetchError::Failed(_))));
    }
}

#[test]
fn oauth_credits_distinguish_disabled_missing_zero_and_known_major_units() {
    for (extra, expected) in [
        (
            json!({"is_enabled":true,"monthly_limit":2000,"used_credits":1240,"utilization":62,"currency":"USD"}),
            Some((Some(20.0), Some(12.4), Some(62.0), Some("USD"))),
        ),
        (
            json!({"is_enabled":false,"monthly_limit":2000,"used_credits":1240,"utilization":62,"currency":"USD"}),
            None,
        ),
        (json!(null), None),
        (
            json!({"is_enabled":true,"used_credits":0,"utilization":120}),
            Some((None, Some(0.0), Some(100.0), None)),
        ),
        (json!({"monthly_limit":0,"utilization":-2}), Some((Some(0.0), None, Some(0.0), None))),
    ] {
        let snapshot = decode_usage_payload(
            json!({"five_hour":{"utilization":1},"extra_usage":extra}).to_string().as_bytes(),
        )
        .unwrap();
        assert_eq!(
            snapshot.extra_usage.as_ref().map(|e| (
                e.monthly_limit,
                e.used_credits,
                e.utilization,
                e.currency.as_deref()
            )),
            expected
        );
    }
}

#[tokio::test]
async fn credential_eligibility_suppresses_requests_until_expiry_is_unknown_or_future() {
    let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    for (expiry, present, eligible) in [
        (None, false, false),
        (Some(now - Duration::from_secs(1)), true, false),
        (Some(now), true, false),
        (Some(now + Duration::from_secs(1)), true, true),
        (None, true, true),
    ] {
        let mut server = Server::new(200, r#"{"five_hour":{"utilization":27}}"#, "reply", None);
        let mut probe = server.probe();
        probe.credentials = present.then(|| auth::ClaudeOAuthCredentials {
            access_token: "synthetic-usage-token".into(),
            expires_at: expiry,
        });
        let result = probe.fetch_snapshot_at(now).await;
        if eligible {
            assert_eq!(result.unwrap().five_hour.unwrap().utilization, 27.0);
            let request = (&mut server.request).await.unwrap().to_ascii_lowercase();
            assert!(request.contains("authorization: bearer synthetic-usage-token\r\n"));
        } else {
            assert!(matches!(result, Err(OauthFetchError::Unavailable(_))));
            assert!(matches!(
                server.request.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ));
        }
        server.finish().await;
    }
}

#[tokio::test]
async fn authenticated_http_results_keep_status_decode_and_transport_failures_distinct() {
    for (status, body, mode, kind, cause) in [
        (200, r#"{"five_hour":{"utilization":31}}"#, "reply", "success", ""),
        (401, "denied", "reply", "unauthorized", "rejected"),
        (403, "denied", "reply", "unauthorized", "rejected"),
        (503, "maintenance", "reply", "failed", "503"),
        (200, "invalid JSON", "reply", "failed", "decode"),
        (200, r#"{"seven_day":{"utilization":4}}"#, "reply", "failed", "current session"),
        (200, "{}", "short-body", "failed", "read"),
        (200, "", "disconnect", "failed", "network"),
    ] {
        let mut server = Server::new(status, body, mode, None);
        let result = server.probe().fetch_snapshot().await;
        let request = (&mut server.request).await.unwrap();
        let headers = request.to_ascii_lowercase();
        assert!(request.starts_with("GET /api/oauth/usage HTTP/1.1\r\n"));
        for header in [
            "authorization: bearer synthetic-usage-token\r\n",
            "anthropic-beta: oauth-2025-04-20\r\n",
            "accept: application/json\r\n",
            "content-type: application/json\r\n",
        ] {
            assert!(headers.contains(header), "{header}");
        }
        match (kind, result) {
            ("success", Ok(snapshot)) => {
                assert_eq!(snapshot.source, UsageSourceKind::Oauth);
                assert_eq!(snapshot.five_hour.unwrap().utilization, 31.0);
            }
            ("unauthorized", Err(OauthFetchError::Unauthorized(message)))
            | ("failed", Err(OauthFetchError::Failed(message))) => {
                assert!(message.contains(cause), "{message}");
            }
            (_, other) => panic!("unexpected {kind}: {other:?}"),
        }
        server.finish().await;
    }
    let body = "é日本語 ".repeat(120);
    let mut server = Server::new(502, &body, "reply", None);
    let error = server.probe().fetch_snapshot().await.unwrap_err();
    let OauthFetchError::Failed(message) = error else {
        panic!("ordinary HTTP error");
    };
    assert!(message.contains("502") && message.contains("é日本語"));
    assert!(!message.contains(&body));
    server.finish().await;
}

#[tokio::test]
async fn observed_http_request_times_out_and_releases_its_connection() {
    let mut server = Server::new(200, "", "stall", None);
    let mut probe = server.probe();
    probe.timeout = Duration::from_secs(2);
    let future = probe.fetch_snapshot();
    tokio::pin!(future);
    tokio::select! { request=&mut server.request=>{assert!(request.unwrap().starts_with("GET /api/oauth/usage "));}, result=&mut future=>panic!("must observe a stalled request: {result:?}") }
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(3)).await;
    let result = future.await;
    tokio::time::resume();
    assert!(
        matches!(result,Err(OauthFetchError::Failed(ref message)) if message.contains("network"))
    );
    assert!(server.finish().await, "peer must observe EOF after request cancellation");
}

#[test]
fn invalid_calendar_and_clock_values_never_become_reset_instants() {
    for raw in [
        "2025-02-29T12:00:00Z",
        "2024-04-31T12:00:00Z",
        "2024-01-02T24:00:00Z",
        "2024-01-02T12:60:00Z",
        "2024-01-02T12:00:60Z",
        "2024-01-02T12:00:00+01:60",
        "2024-01-02T12:00:00+24:00",
        "2024-01-02T12:00:00Zgarbage",
        "2024-01-02-extraT12:00:00Z",
    ] {
        assert_eq!(parse_timestamp(raw), None, "{raw}");
        let snapshot = decode_usage_payload(
            serde_json::json!({"five_hour":{"utilization":19,"resets_at":raw}})
                .to_string()
                .as_bytes(),
        )
        .unwrap();
        let window = snapshot.five_hour.unwrap();
        assert_eq!(window.utilization, 19.0);
        assert_eq!(window.resets_at, None, "{raw}");
    }
}
