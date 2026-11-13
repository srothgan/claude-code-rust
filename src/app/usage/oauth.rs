// SPDX-License-Identifier: Apache-2.0
use crate::app::auth;
use crate::app::{ExtraUsage, UsageSnapshot, UsageSourceKind, UsageWindow};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue, USER_AGENT};
use serde::Deserialize;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const OAUTH_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const OAUTH_BETA_HEADER: &str = "oauth-2025-04-20";
const OAUTH_TIMEOUT: Duration = Duration::from_secs(8);
const FALLBACK_USER_AGENT: &str = "claude-code/unknown";

#[cfg(test)]
#[path = "oauth_audit_tests.rs"]
mod audit_tests;

#[derive(Debug)]
pub(super) enum OauthFetchError {
    Unavailable(String),
    Unauthorized(String),
    Failed(String),
}

impl OauthFetchError {
    pub(super) fn should_fallback_to_cli(&self) -> bool {
        matches!(self, Self::Unavailable(_) | Self::Unauthorized(_))
    }

    pub(super) fn into_message(self) -> String {
        match self {
            Self::Unavailable(message) | Self::Unauthorized(message) | Self::Failed(message) => {
                message
            }
        }
    }
}

#[derive(Debug, Deserialize)]
struct OAuthUsagePayload {
    five_hour: Option<OAuthUsageWindowPayload>,
    seven_day: Option<OAuthUsageWindowPayload>,
    seven_day_oauth_apps: Option<OAuthUsageWindowPayload>,
    seven_day_opus: Option<OAuthUsageWindowPayload>,
    seven_day_sonnet: Option<OAuthUsageWindowPayload>,
    iguana_necktie: Option<OAuthUsageWindowPayload>,
    extra_usage: Option<OAuthExtraUsagePayload>,
}

#[derive(Debug, Deserialize)]
struct OAuthUsageWindowPayload {
    utilization: Option<f64>,
    resets_at: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct OAuthExtraUsagePayload {
    is_enabled: Option<bool>,
    monthly_limit: Option<f64>,
    used_credits: Option<f64>,
    utilization: Option<f64>,
    currency: Option<String>,
}

#[derive(Clone)]
pub(super) struct Probe {
    pub(super) credentials: Option<auth::ClaudeOAuthCredentials>,
    pub(super) endpoint: String,
    pub(super) timeout: Duration,
}

impl Probe {
    pub(super) fn system() -> Self {
        Self {
            credentials: auth::load_oauth_credentials(),
            endpoint: OAUTH_USAGE_URL.into(),
            timeout: OAUTH_TIMEOUT,
        }
    }

    pub(super) async fn fetch_snapshot(&self) -> Result<UsageSnapshot, OauthFetchError> {
        self.fetch_snapshot_at(SystemTime::now()).await
    }

    async fn fetch_snapshot_at(&self, now: SystemTime) -> Result<UsageSnapshot, OauthFetchError> {
        let credentials = self.credentials.as_ref().ok_or_else(|| {
            OauthFetchError::Unavailable(
                "No Claude OAuth credentials found. Run /login to authenticate.".into(),
            )
        })?;
        if credentials.expires_at.is_some_and(|expires_at| expires_at <= now) {
            return Err(OauthFetchError::Unavailable(
                "Claude OAuth credentials expired. Run /login to refresh them.".to_owned(),
            ));
        }

        let client = reqwest::Client::builder()
            .timeout(self.timeout)
            .default_headers(oauth_headers(&credentials.access_token)?)
            .build()
            .map_err(|error| {
                OauthFetchError::Failed(format!("Failed to create OAuth client: {error}"))
            })?;

        let response = client.get(&self.endpoint).send().await.map_err(|error| {
            OauthFetchError::Failed(format!("Claude OAuth network error: {error}"))
        })?;

        let status = response.status();
        let body = response.bytes().await.map_err(|error| {
            OauthFetchError::Failed(format!("Failed to read Claude OAuth usage response: {error}"))
        })?;

        match status.as_u16() {
        200 => decode_usage_payload(&body),
        401 | 403 => Err(OauthFetchError::Unauthorized(
            "Claude OAuth usage request was rejected. Run /login to refresh Claude credentials."
                .to_owned(),
        )),
        _ => Err(OauthFetchError::Failed(format!(
            "Claude OAuth usage request failed with HTTP {}{}",
            status.as_u16(),
            truncated_body_suffix(&body),
        ))),
    }
    }
}

fn oauth_headers(access_token: &str) -> Result<HeaderMap, OauthFetchError> {
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert("anthropic-beta", HeaderValue::from_static(OAUTH_BETA_HEADER));
    headers.insert(USER_AGENT, HeaderValue::from_static(FALLBACK_USER_AGENT));
    let token = HeaderValue::from_str(&format!("Bearer {access_token}")).map_err(|error| {
        OauthFetchError::Failed(format!("Invalid OAuth bearer token header: {error}"))
    })?;
    headers.insert(AUTHORIZATION, token);
    Ok(headers)
}

fn decode_usage_payload(body: &[u8]) -> Result<UsageSnapshot, OauthFetchError> {
    let payload = serde_json::from_slice::<OAuthUsagePayload>(body).map_err(|error| {
        OauthFetchError::Failed(format!("Failed to decode Claude OAuth usage response: {error}"))
    })?;

    let five_hour = map_window(payload.five_hour, "5-hour");
    if five_hour.is_none() {
        return Err(OauthFetchError::Failed(
            "Claude OAuth usage response did not include the current session window.".to_owned(),
        ));
    }

    let _ = payload.iguana_necktie;

    Ok(UsageSnapshot {
        source: UsageSourceKind::Oauth,
        fetched_at: SystemTime::now(),
        subscription_type: None,
        five_hour,
        seven_day: map_window(payload.seven_day, "7-day"),
        seven_day_oauth_apps: map_window(payload.seven_day_oauth_apps, "7-day OAuth apps"),
        seven_day_opus: map_window(payload.seven_day_opus, "7-day Opus"),
        seven_day_sonnet: map_window(payload.seven_day_sonnet, "7-day Sonnet"),
        model_scoped: Vec::new(),
        extra_usage: map_extra_usage(payload.extra_usage),
        session: None,
        activity: None,
    })
}

fn map_window(payload: Option<OAuthUsageWindowPayload>, label: &str) -> Option<UsageWindow> {
    let payload = payload?;
    let utilization = payload.utilization?;
    Some(UsageWindow {
        label: label.to_owned(),
        utilization: utilization.clamp(0.0, 100.0),
        resets_at: payload.resets_at.as_ref().and_then(parse_timestamp_value),
        reset_description: None,
    })
}

fn map_extra_usage(payload: Option<OAuthExtraUsagePayload>) -> Option<ExtraUsage> {
    let payload = payload?;
    if payload.is_enabled == Some(false) {
        return None;
    }

    Some(ExtraUsage {
        monthly_limit: payload.monthly_limit.map(|value| value / 100.0),
        used_credits: payload.used_credits.map(|value| value / 100.0),
        utilization: payload.utilization.map(|value| value.clamp(0.0, 100.0)),
        currency: payload.currency,
    })
}

fn truncated_body_suffix(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body).trim().replace('\n', " ");
    if text.is_empty() {
        return String::new();
    }

    let shortened = if text.chars().count() > 200 {
        let mut out = text.chars().take(200).collect::<String>();
        out.push_str("...");
        out
    } else {
        text
    };
    format!(": {shortened}")
}

fn parse_timestamp_value(value: &serde_json::Value) -> Option<SystemTime> {
    if let Some(raw) = value.as_str() {
        parse_timestamp(raw)
    } else {
        crate::epoch_time::parse_value(value)
    }
}

pub(super) fn parse_timestamp(raw: &str) -> Option<SystemTime> {
    parse_iso8601_timestamp(raw).or_else(|| crate::epoch_time::parse_str(raw))
}

fn parse_iso8601_timestamp(raw: &str) -> Option<SystemTime> {
    let parsed = chrono::DateTime::parse_from_rfc3339(raw.trim()).ok()?;
    let seconds = u64::try_from(parsed.timestamp()).ok()?;
    let nanos = parsed.timestamp_subsec_nanos();
    // Source reset instants use ordinary seconds, not leap-second normalization.
    if nanos >= 1_000_000_000 {
        return None;
    }
    UNIX_EPOCH.checked_add(Duration::new(seconds, nanos))
}
