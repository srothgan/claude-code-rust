// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use super::App;
use super::settings;
use crate::Cli;
use reqwest::header::{ACCEPT, HeaderMap, HeaderValue, USER_AGENT};
use serde::Deserialize;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{Instrument as _, info_span};

const UPDATE_CHECK_DISABLE_ENV: &str = "CLAUDE_RUST_NO_UPDATE_CHECK";
const UPDATE_CHECK_TTL_SECS: u64 = 24 * 60 * 60;
const UPDATE_CHECK_TIMEOUT: Duration = Duration::from_secs(4);
const GITHUB_LATEST_RELEASE_API_URL: &str =
    "https://api.github.com/repos/srothgan/claude-code-rust/releases/latest";
const GITHUB_API_ACCEPT_VALUE: &str = "application/vnd.github+json";
const GITHUB_API_VERSION_VALUE: &str = "2022-11-28";
const GITHUB_USER_AGENT_VALUE: &str = "claude-code-rust-update-check";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct SimpleVersion {
    major: u64,
    minor: u64,
    patch: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct GithubLatestRelease {
    tag_name: String,
    html_url: Option<String>,
}

#[derive(Debug, Clone)]
struct LatestRelease {
    latest_version: String,
    release_url: String,
}

pub fn start_update_check(app: &App, cli: &Cli) {
    if update_check_disabled(cli.no_update_check) {
        tracing::debug!(
            target: crate::logging::targets::APP_UPDATE,
            event_name = "update_check_skipped",
            message = "update check skipped",
            outcome = "skipped",
            reason = "disabled_by_flag_or_env",
        );
        return;
    }

    let current_version = env!("CARGO_PKG_VERSION").to_owned();
    let settings_path = app.global_settings_path.clone();
    let settings_snapshot = app.global_settings.clone();
    tracing::info!(
        target: crate::logging::targets::APP_UPDATE,
        event_name = "update_check_started",
        message = "update check started",
        outcome = "start",
        current_version = %current_version,
    );

    let update_check_span = info_span!(
        target: crate::logging::targets::APP_UPDATE,
        "update_check",
        current_version = %current_version,
    );

    tokio::task::spawn_local(
        async move {
            let Some((mut global_settings, release)) =
                resolve_latest_release(settings_snapshot).await
            else {
                return;
            };

            settings::record_update_check_result(
                &mut global_settings,
                &current_version,
                &release.latest_version,
                &release.release_url,
                unix_now_secs().unwrap_or(0),
            );
            if let Some(path) = settings_path.as_ref()
                && let Err(err) = settings::save_global_settings(path, &global_settings)
            {
                tracing::warn!(
                    target: crate::logging::targets::APP_UPDATE,
                    event_name = "update_settings_write_failed",
                    message = "failed to write update check result",
                    outcome = "failure",
                    settings_path = %path.display(),
                    error_message = %err,
                );
            }
        }
        .instrument(update_check_span),
    );
}

pub(crate) fn update_check_disabled(no_update_check_flag: bool) -> bool {
    if no_update_check_flag {
        return true;
    }
    std::env::var(UPDATE_CHECK_DISABLE_ENV)
        .ok()
        .is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
}

async fn resolve_latest_release(
    settings: settings::AppSettings,
) -> Option<(settings::AppSettings, LatestRelease)> {
    let now = unix_now_secs()?;

    if let Some(result) = settings.updates.last_result.as_ref()
        && now.saturating_sub(result.checked_at_unix_secs) <= UPDATE_CHECK_TTL_SECS
        && is_valid_version(&result.latest_version)
    {
        tracing::debug!(
            target: crate::logging::targets::APP_UPDATE,
            event_name = "update_check_cache_hit",
            message = "update check cache hit",
            outcome = "success",
            latest_version = %result.latest_version,
        );
        return None;
    }

    let release = fetch_latest_release().await?;
    Some((settings, release))
}

pub(crate) fn unix_now_secs() -> Option<u64> {
    SystemTime::now().duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs())
}

async fn fetch_latest_release() -> Option<LatestRelease> {
    let client = reqwest::Client::builder().timeout(UPDATE_CHECK_TIMEOUT).build().ok()?;

    let response = client
        .get(GITHUB_LATEST_RELEASE_API_URL)
        .headers(github_api_headers())
        .send()
        .await
        .ok()?;

    if !response.status().is_success() {
        tracing::warn!(
            target: crate::logging::targets::APP_UPDATE,
            event_name = "update_check_failed",
            message = "update check request failed",
            outcome = "failure",
            status = %response.status(),
            url = GITHUB_LATEST_RELEASE_API_URL,
        );
        return None;
    }

    let release = response.json::<GithubLatestRelease>().await.ok()?;
    let latest_version = normalize_version_string(&release.tag_name)?;
    let release_url = release
        .html_url
        .filter(|url| !url.trim().is_empty())
        .or_else(|| settings::release_url_for_version(&latest_version))?;
    Some(LatestRelease { latest_version, release_url })
}

fn github_api_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static(GITHUB_API_ACCEPT_VALUE));
    headers.insert("X-GitHub-Api-Version", HeaderValue::from_static(GITHUB_API_VERSION_VALUE));
    headers.insert(USER_AGENT, HeaderValue::from_static(GITHUB_USER_AGENT_VALUE));
    headers
}

pub(crate) fn normalize_version_string(raw: &str) -> Option<String> {
    parse_simple_version(raw).map(|v| format!("{}.{}.{}", v.major, v.minor, v.patch))
}

pub(crate) fn parse_simple_version(raw: &str) -> Option<SimpleVersion> {
    let trimmed = raw.trim();
    let without_prefix = trimmed.strip_prefix('v').unwrap_or(trimmed);
    let core = without_prefix.split_once('-').map_or(without_prefix, |(c, _)| c);

    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(SimpleVersion { major, minor, patch })
}

pub(crate) fn is_valid_version(version: &str) -> bool {
    parse_simple_version(version).is_some()
}

pub(crate) fn is_newer_version(candidate: &str, current: &str) -> bool {
    let Some(candidate) = parse_simple_version(candidate) else {
        return false;
    };
    let Some(current) = parse_simple_version(current) else {
        return false;
    };
    candidate > current
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_version_accepts_v_prefix() {
        assert_eq!(
            parse_simple_version("v1.2.3"),
            Some(SimpleVersion { major: 1, minor: 2, patch: 3 })
        );
    }

    #[test]
    fn parse_simple_version_rejects_invalid_shapes() {
        assert_eq!(parse_simple_version("1.2"), None);
        assert_eq!(parse_simple_version("1.2.3.4"), None);
        assert_eq!(parse_simple_version("v1.two.3"), None);
    }

    #[test]
    fn parse_simple_version_ignores_prerelease_suffix() {
        assert_eq!(
            parse_simple_version("v2.4.6-rc1"),
            Some(SimpleVersion { major: 2, minor: 4, patch: 6 })
        );
    }

    #[test]
    fn normalize_version_string_accepts_release_tag() {
        assert_eq!(normalize_version_string("v0.10.0").as_deref(), Some("0.10.0"));
    }

    #[test]
    fn github_release_payload_parses_tag_name() {
        let payload = r#"{"tag_name":"v0.11.0"}"#;
        let parsed = serde_json::from_str::<GithubLatestRelease>(payload).ok();
        assert_eq!(parsed.map(|r| r.tag_name), Some("v0.11.0".to_owned()));
    }

    #[test]
    fn update_check_disabled_prefers_flag() {
        assert!(update_check_disabled(true));
    }

    #[test]
    fn is_newer_version_compares_semver_triplets() {
        assert!(is_newer_version("0.3.0", "0.2.9"));
        assert!(!is_newer_version("0.2.9", "0.3.0"));
        assert!(!is_newer_version("bad", "0.3.0"));
    }
}
