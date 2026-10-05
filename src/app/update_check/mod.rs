// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use super::App;
use super::settings;
use crate::Cli;
use crate::agent::events::ClientEvent;
use crate::install_method::{InstallMethod, detect_install_method};
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
    start_update_check_at(
        app,
        update_check_disabled(cli.no_update_check),
        GITHUB_LATEST_RELEASE_API_URL,
    );
}

fn start_update_check_at(app: &App, disabled: bool, endpoint: &str) {
    if disabled {
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
    let settings_snapshot = app.global_settings.clone();
    let event_tx = app.event_tx.clone();
    let endpoint = endpoint.to_owned();
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
                resolve_latest_release(settings_snapshot, &endpoint).await
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
            if let Some(result) = global_settings.updates.last_result {
                let _ = event_tx.send(ClientEvent::UpdateCheckCompleted { result }).await;
            }
        }
        .instrument(update_check_span),
    );
}

pub(crate) fn apply_check_result(app: &mut App, result: &settings::UpdateCheckResult) {
    tracing::info!(
        target: crate::logging::targets::APP_UPDATE,
        event_name = "update_check_completed",
        latest_version = %result.latest_version,
        outcome = "success",
    );
    settings::record_update_check_result(
        &mut app.global_settings,
        &result.current_version,
        &result.latest_version,
        &result.release_url,
        result.checked_at_unix_secs,
    );
    if let Some(path) = app.global_settings_path.as_ref()
        && let Err(error) = settings::save_global_settings(path, &app.global_settings)
    {
        tracing::warn!(
            target: crate::logging::targets::APP_UPDATE,
            event_name = "update_settings_write_failed",
            outcome = "failure",
            error_message = %error,
        );
    }
}

/// Decide only after terminal and bridge cleanup; read current saved consent.
pub fn automatic_update_action(app: &App, cli: &Cli) -> Option<super::PostExitAction> {
    automatic_update_action_with(app, cli, detect_install_method)
}

fn automatic_update_action_with(
    app: &App,
    cli: &Cli,
    install_method: impl FnOnce() -> InstallMethod,
) -> Option<super::PostExitAction> {
    if app.shutdown_forced() || update_check_disabled(cli.no_update_check) {
        return None;
    }
    let path = app.global_settings_path.as_deref()?;
    let settings = match settings::load_from_path(path) {
        Ok(settings) => settings,
        Err(error) => {
            tracing::warn!(target: crate::logging::targets::APP_UPDATE,
                event_name = "automatic_update_skipped", reason = "settings_unavailable", error_message = %error);
            return None;
        }
    };
    if !settings.updates.auto_install {
        return None;
    }
    let result = settings.updates.last_result.as_ref()?;
    if !is_newer_version(&result.latest_version, env!("CARGO_PKG_VERSION")) {
        return None;
    }
    let method = install_method();
    if method == InstallMethod::Unknown {
        tracing::warn!(target: crate::logging::targets::APP_UPDATE,
            event_name = "automatic_update_skipped", reason = "unknown_install_method", latest_version = %result.latest_version);
        return None;
    }
    tracing::info!(target: crate::logging::targets::APP_UPDATE,
        event_name = "automatic_update_selected", latest_version = %result.latest_version, install_method = method.label());
    Some(super::PostExitAction::InstallUpdate {
        latest_version: result.latest_version.clone(),
        method,
    })
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
    endpoint: &str,
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

    let release = fetch_latest_release(endpoint).await?;
    Some((settings, release))
}

pub(crate) fn unix_now_secs() -> Option<u64> {
    SystemTime::now().duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs())
}

async fn fetch_latest_release(endpoint: &str) -> Option<LatestRelease> {
    let client = reqwest::Client::builder().timeout(UPDATE_CHECK_TIMEOUT).build().ok()?;

    let response = client.get(endpoint).headers(github_api_headers()).send().await.ok()?;

    if !response.status().is_success() {
        tracing::warn!(
            target: crate::logging::targets::APP_UPDATE,
            event_name = "update_check_failed",
            message = "update check request failed",
            outcome = "failure",
            status = %response.status(),
            url = endpoint,
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
mod tests;
