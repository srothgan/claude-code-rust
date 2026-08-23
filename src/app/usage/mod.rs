// SPDX-License-Identifier: Apache-2.0
mod cli;
mod oauth;

use crate::agent::events::ClientEvent;
use crate::agent::types::StructuredUsageSnapshot;
use crate::app::{
    App, ExtraUsage, SessionUsageSummary, UsageActivitySummary, UsageActivityWindow,
    UsageBehaviorAttribution, UsageNamedAttribution, UsageSnapshot, UsageSourceKind,
    UsageSourceMode, UsageWindow,
};
use std::time::{Duration, SystemTime};

const USAGE_REFRESH_TTL: Duration = Duration::from_secs(30);

struct UsageRefreshFailure {
    source: UsageSourceKind,
    message: String,
}

pub(crate) fn request_refresh_if_needed(app: &mut App) {
    if app.usage.in_flight {
        return;
    }
    if app.usage.snapshot.as_ref().is_some_and(snapshot_is_fresh) {
        return;
    }
    request_refresh(app);
}

pub(crate) fn request_refresh(app: &mut App) {
    if app.usage.in_flight || tokio::runtime::Handle::try_current().is_err() {
        return;
    }

    apply_refresh_started(app);

    if app.usage.active_source == UsageSourceMode::Auto
        && let (Some(connection), Some(session_id)) =
            (app.session_runtime.conn.as_ref(), app.session_runtime.session_id.as_ref())
        && connection.get_usage(session_id.as_str().to_owned()).is_ok()
    {
        return;
    }

    spawn_host_refresh(app, app.usage.active_source, None);
}

fn spawn_host_refresh(app: &App, source_mode: UsageSourceMode, sdk_error: Option<String>) {
    let event_tx = app.event_tx.clone();
    let epoch = app.session_runtime.session_scope_epoch;
    let cwd_raw = app.cwd_raw.clone();

    tokio::task::spawn_local(async move {
        let _ = event_tx.send(ClientEvent::UsageRefreshStarted { epoch }).await;
        match refresh_snapshot(source_mode, cwd_raw).await {
            Ok(snapshot) => {
                let _ = event_tx.send(ClientEvent::UsageSnapshotReceived { epoch, snapshot }).await;
            }
            Err(error) => {
                let message = sdk_error.map_or(error.message.clone(), |sdk_error| {
                    format!(
                        "Structured SDK usage unavailable ({sdk_error}). Account usage fallback failed: {}",
                        error.message
                    )
                });
                let _ = event_tx
                    .send(ClientEvent::UsageRefreshFailed { epoch, message, source: error.source })
                    .await;
            }
        }
    });
}

pub(crate) fn apply_structured_sdk_result(
    app: &mut App,
    snapshot: Option<StructuredUsageSnapshot>,
    error: Option<String>,
) {
    if let Some(snapshot) = snapshot {
        apply_refresh_success(app, map_structured_sdk_snapshot(snapshot));
        return;
    }

    let message = error.unwrap_or_else(|| "structured SDK usage returned no snapshot".to_owned());
    if tokio::runtime::Handle::try_current().is_ok() {
        spawn_host_refresh(app, UsageSourceMode::Auto, Some(message));
    } else {
        apply_refresh_failure(app, message.clone(), UsageSourceKind::Sdk);
    }
}

fn map_structured_sdk_snapshot(snapshot: StructuredUsageSnapshot) -> UsageSnapshot {
    let rate_limits_available = snapshot.rate_limits_available != Some(false);
    let map_window = |window: Option<crate::agent::types::StructuredUsageWindow>, label: &str| {
        rate_limits_available.then_some(window).flatten().map(|window| UsageWindow {
            label: label.to_owned(),
            utilization: window.utilization.clamp(0.0, 100.0),
            resets_at: window.resets_at.as_deref().and_then(oauth::parse_timestamp),
            reset_description: None,
        })
    };
    let extra_usage =
        rate_limits_available.then_some(snapshot.extra_usage).flatten().map(|extra| {
            ExtraUsage {
                // The experimental SDK declaration does not specify the denomination of these two raw fields. Do not present guessed monetary values.
                monthly_limit: None,
                used_credits: None,
                utilization: extra.utilization.map(|value| value.clamp(0.0, 100.0)),
                currency: extra.currency,
            }
        });
    let model_scoped = if rate_limits_available {
        snapshot
            .model_scoped
            .into_iter()
            .map(|window| UsageWindow {
                label: format!("7-day {}", window.display_name),
                utilization: window.utilization.clamp(0.0, 100.0),
                resets_at: window.resets_at.as_deref().and_then(oauth::parse_timestamp),
                reset_description: None,
            })
            .collect()
    } else {
        Vec::new()
    };
    let session = snapshot.session.map(|session| SessionUsageSummary {
        total_cost_usd: session.total_cost_usd,
        total_api_duration_ms: session.total_api_duration_ms,
        total_duration_ms: session.total_duration_ms,
        total_lines_added: nonnegative_number(session.total_lines_added),
        total_lines_removed: nonnegative_number(session.total_lines_removed),
        model_count: session.model_count,
    });
    let activity = UsageActivitySummary {
        day: snapshot.activity_day.map(map_activity_window),
        week: snapshot.activity_week.map(map_activity_window),
    };
    let activity = (activity.day.is_some() || activity.week.is_some()).then_some(activity);

    UsageSnapshot {
        source: UsageSourceKind::Sdk,
        fetched_at: SystemTime::now(),
        subscription_type: snapshot.subscription_type,
        five_hour: map_window(snapshot.five_hour, "5-hour"),
        seven_day: map_window(snapshot.seven_day, "7-day"),
        seven_day_oauth_apps: map_window(snapshot.seven_day_oauth_apps, "7-day OAuth apps"),
        seven_day_opus: map_window(snapshot.seven_day_opus, "7-day Opus"),
        seven_day_sonnet: map_window(snapshot.seven_day_sonnet, "7-day Sonnet"),
        model_scoped,
        extra_usage,
        session,
        activity,
    }
}

fn map_activity_window(
    window: crate::agent::types::StructuredActivityWindow,
) -> UsageActivityWindow {
    UsageActivityWindow {
        request_count: window.request_count,
        session_count: window.session_count,
        behaviors: window
            .behaviors
            .into_iter()
            .map(|item| UsageBehaviorAttribution {
                key: item.key,
                pct: item.pct.clamp(0.0, 100.0),
                count: item.count,
            })
            .collect(),
        agents: map_named_attributions(window.agents),
        skills: map_named_attributions(window.skills),
        plugins: map_named_attributions(window.plugins),
        mcp_servers: map_named_attributions(window.mcp_servers),
    }
}

fn map_named_attributions(
    items: Vec<crate::agent::types::StructuredNamedAttribution>,
) -> Vec<UsageNamedAttribution> {
    items
        .into_iter()
        .map(|item| UsageNamedAttribution { name: item.name, pct: item.pct.clamp(0.0, 100.0) })
        .collect()
}

fn nonnegative_number(value: Option<f64>) -> Option<f64> {
    let value = value?;
    (value.is_finite() && value >= 0.0).then_some(value)
}

pub(crate) fn apply_refresh_started(app: &mut App) {
    app.usage.in_flight = true;
    app.usage.last_error = None;
    app.usage.last_attempted_source = None;
}

pub(crate) fn apply_refresh_success(app: &mut App, snapshot: UsageSnapshot) {
    app.usage.last_attempted_source = Some(snapshot.source);
    app.usage.snapshot = Some(snapshot);
    app.usage.in_flight = false;
    app.usage.last_error = None;
}

pub(crate) fn apply_refresh_failure(app: &mut App, message: String, source: UsageSourceKind) {
    app.usage.in_flight = false;
    app.usage.last_error = Some(message);
    app.usage.last_attempted_source = Some(source);
}

pub(crate) fn reset_for_session_change(app: &mut App) {
    app.usage.snapshot = None;
    app.usage.in_flight = false;
    app.usage.last_error = None;
    app.usage.last_attempted_source = None;
}

pub(crate) fn visible_windows(snapshot: &UsageSnapshot) -> Vec<&UsageWindow> {
    let mut windows = Vec::new();
    if let Some(window) = snapshot.five_hour.as_ref() {
        windows.push(window);
    }
    if let Some(window) = snapshot.seven_day.as_ref() {
        windows.push(window);
    }
    if let Some(window) = snapshot.seven_day_oauth_apps.as_ref() {
        windows.push(window);
    }
    if let Some(window) = snapshot.seven_day_sonnet.as_ref() {
        windows.push(window);
    }
    if let Some(window) = snapshot.seven_day_opus.as_ref() {
        windows.push(window);
    }
    windows.extend(snapshot.model_scoped.iter());
    windows
}

pub(crate) fn format_window_reset(window: &UsageWindow) -> Option<String> {
    if let Some(resets_at) = window.resets_at {
        return Some(format!("resets in {}", format_remaining_until(resets_at)));
    }

    let description = window.reset_description.as_deref()?.trim();
    if description.is_empty() { None } else { Some(description.to_owned()) }
}

fn snapshot_is_fresh(snapshot: &UsageSnapshot) -> bool {
    snapshot.fetched_at.elapsed().is_ok_and(|age| age < USAGE_REFRESH_TTL)
}

fn format_remaining_until(target: SystemTime) -> String {
    let Ok(remaining) = target.duration_since(SystemTime::now()) else {
        return "< 1 minute".to_owned();
    };

    if remaining < Duration::from_secs(60) {
        return "< 1 minute".to_owned();
    }

    let total_minutes = remaining.as_secs() / 60;
    let days = total_minutes / (24 * 60);
    let hours = (total_minutes % (24 * 60)) / 60;
    let minutes = total_minutes % 60;

    if days > 0 {
        return format!("{days}d {hours}h");
    }
    if hours > 0 {
        if minutes == 0 {
            return format!("{hours}h");
        }
        return format!("{hours}h {minutes}m");
    }
    format!("{minutes}m")
}

async fn refresh_snapshot(
    source_mode: UsageSourceMode,
    cwd_raw: String,
) -> Result<UsageSnapshot, UsageRefreshFailure> {
    match source_mode {
        UsageSourceMode::Oauth => oauth::fetch_snapshot().await.map_err(|error| {
            UsageRefreshFailure { source: UsageSourceKind::Oauth, message: error.into_message() }
        }),
        UsageSourceMode::Cli => cli::fetch_snapshot(cwd_raw)
            .await
            .map_err(|message| UsageRefreshFailure { source: UsageSourceKind::Cli, message }),
        UsageSourceMode::Auto => refresh_snapshot_auto(cwd_raw).await,
    }
}

async fn refresh_snapshot_auto(cwd_raw: String) -> Result<UsageSnapshot, UsageRefreshFailure> {
    match oauth::fetch_snapshot().await {
        Ok(snapshot) => Ok(snapshot),
        Err(error) if error.should_fallback_to_cli() => {
            let oauth_message = error.into_message();
            cli::fetch_snapshot(cwd_raw).await.map_err(|message| UsageRefreshFailure {
                source: UsageSourceKind::Cli,
                message: format!(
                    "OAuth unavailable ({oauth_message}). CLI fallback failed: {message}"
                ),
            })
        }
        Err(error) => Err(UsageRefreshFailure {
            source: UsageSourceKind::Oauth,
            message: error.into_message(),
        }),
    }
}

#[cfg(test)]
mod tests;
