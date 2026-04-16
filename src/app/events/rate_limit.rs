// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::super::{App, NoticeDedupKey, NoticeStage, RateLimitIncidentKey, SystemSeverity};
use crate::agent::model;
use std::time::Duration;

const EXTRA_USAGE_REQUIRED_MESSAGE: &str = "Extra usage is required for 1M context. Use /extra-usage to enable it, /model to switch models, or /1m-context disable to turn off 1M context for this folder.";

fn format_rate_limit_type(raw: &str) -> &str {
    match raw {
        "five_hour" => "5-hour",
        "daily" => "daily",
        "minute" => "per-minute",
        "seven_day" => "7-day",
        "seven_day_opus" => "7-day Opus",
        "seven_day_sonnet" => "7-day Sonnet",
        "overage" => "overage",
        other => other,
    }
}

/// Format an epoch timestamp as a countdown and UTC wall-clock: "4h 23m at 14:30 UTC".
fn format_resets_at(epoch_secs: f64) -> String {
    use std::time::{Duration, UNIX_EPOCH};

    let now = std::time::SystemTime::now();

    let countdown = match (UNIX_EPOCH + Duration::from_secs_f64(epoch_secs)).duration_since(now) {
        Ok(d) => {
            let total_secs = d.as_secs();
            if total_secs < 60 {
                "< 1 minute".to_owned()
            } else {
                let hours = total_secs / 3600;
                let minutes = (total_secs % 3600) / 60;
                if hours > 0 { format!("{hours}h {minutes}m") } else { format!("{minutes}m") }
            }
        }
        Err(_) => "now".to_owned(),
    };

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let epoch_u64 = epoch_secs.max(0.0) as u64;
    let h = (epoch_u64 % 86400) / 3600;
    let m = (epoch_u64 % 3600) / 60;

    format!("{countdown} at {h:02}:{m:02} UTC")
}

fn has_primary_rate_limit_context(update: &model::RateLimitUpdate) -> bool {
    update.utilization.is_some() || update.rate_limit_type.is_some() || update.resets_at.is_some()
}

fn is_org_level_disabled_extra_usage_case(update: &model::RateLimitUpdate) -> bool {
    matches!(update.status, model::RateLimitStatus::Rejected)
        && !has_primary_rate_limit_context(update)
        && update.is_using_overage == Some(false)
        && update.overage_disabled_reason.as_deref() == Some("org_level_disabled")
}

pub(super) fn format_rate_limit_summary(update: &model::RateLimitUpdate) -> String {
    if is_org_level_disabled_extra_usage_case(update) {
        return EXTRA_USAGE_REQUIRED_MESSAGE.to_owned();
    }

    let is_rejected = matches!(update.status, model::RateLimitStatus::Rejected);

    // Intro
    let intro = if is_rejected { "Rate limit reached" } else { "Approaching rate limit" };

    // "you've used 91% of your 5-hour rate limit"
    let usage_part = match (update.utilization, &update.rate_limit_type) {
        (Some(util), Some(rlt)) => {
            format!(
                "you've used {:.0}% of your {} rate limit",
                util * 100.0,
                format_rate_limit_type(rlt),
            )
        }
        (Some(util), None) => format!("you've used {:.0}% of your rate limit", util * 100.0),
        (None, Some(rlt)) => {
            format!("you've hit your {} rate limit", format_rate_limit_type(rlt))
        }
        (None, None) => "you've hit your rate limit".to_owned(),
    };

    let mut message = format!("{intro}, {usage_part}.");

    // Overage hint
    if is_rejected {
        // Rejected: state if overage is in use
        if update.is_using_overage == Some(true) {
            message.push_str(" You are using your overage allowance.");
        }
    } else {
        // Warning: hint that overage is available
        if update.is_using_overage == Some(false) || update.overage_status.is_some() {
            message.push_str(" You can continue using your overage allowance.");
        }
    }

    // Resets in X at HH:MM
    if let Some(resets_at) = update.resets_at {
        use std::fmt::Write;
        let _ = write!(message, " Resets in {}.", format_resets_at(resets_at));
    }

    message
}

pub(super) fn rate_limit_notice_key(update: &model::RateLimitUpdate) -> NoticeDedupKey {
    NoticeDedupKey::RateLimit(RateLimitIncidentKey {
        rate_limit_type: update.rate_limit_type.clone(),
        resets_at_bucket: update.resets_at.and_then(reset_bucket_from_epoch_secs),
    })
}

fn reset_bucket_from_epoch_secs(value: f64) -> Option<u64> {
    if !value.is_finite() {
        return None;
    }
    Some(Duration::from_secs_f64(value.max(0.0)).as_secs())
}

pub(super) fn handle_rate_limit_update(app: &mut App, update: &model::RateLimitUpdate) {
    app.last_rate_limit_update = Some(update.clone());
    tracing::debug!(
        target: crate::logging::targets::APP_SESSION,
        event_name = "rate_limit_update_applied",
        message = "rate limit update applied",
        outcome = "success",
        status = ?update.status,
        utilization = update.utilization,
        rate_limit_type = update.rate_limit_type.as_deref().unwrap_or(""),
        resets_at = update.resets_at.unwrap_or_default(),
        overage_status = ?update.overage_status,
        overage_resets_at = update.overage_resets_at.unwrap_or_default(),
        overage_disabled_reason = update.overage_disabled_reason.as_deref().unwrap_or(""),
        is_using_overage = ?update.is_using_overage,
        surpassed_threshold = update.surpassed_threshold.unwrap_or_default(),
    );

    match update.status {
        model::RateLimitStatus::Allowed => {}
        model::RateLimitStatus::AllowedWarning => {
            let summary = format_rate_limit_summary(update);
            super::notices::upsert_turn_notice(
                app,
                rate_limit_notice_key(update),
                NoticeStage::Warning,
                SystemSeverity::Warning,
                &summary,
            );
        }
        model::RateLimitStatus::Rejected => {
            let summary = format_rate_limit_summary(update);
            super::notices::upsert_turn_notice(
                app,
                rate_limit_notice_key(update),
                NoticeStage::Rejected,
                SystemSeverity::Error,
                &summary,
            );
        }
    }
}

pub(super) fn handle_compaction_boundary_update(
    app: &mut App,
    boundary: model::CompactionBoundary,
) {
    app.is_compacting = true;
    if matches!(boundary.trigger, model::CompactionTrigger::Manual) {
        app.pending_compact_clear = true;
    }
    app.session_usage.last_compaction_trigger = Some(boundary.trigger);
    app.session_usage.last_compaction_pre_tokens = Some(boundary.pre_tokens);
    tracing::debug!(
        "CompactionBoundary: trigger={:?} pre_tokens={}",
        boundary.trigger,
        boundary.pre_tokens
    );
}

#[cfg(test)]
mod tests {
    use super::format_rate_limit_summary;
    use crate::agent::model::{RateLimitStatus, RateLimitUpdate};

    #[test]
    fn org_level_disabled_without_primary_context_uses_extra_usage_message() {
        let update = RateLimitUpdate {
            status: RateLimitStatus::Rejected,
            resets_at: None,
            utilization: None,
            rate_limit_type: None,
            overage_status: None,
            overage_resets_at: None,
            overage_disabled_reason: Some("org_level_disabled".to_owned()),
            is_using_overage: Some(false),
            surpassed_threshold: None,
        };

        assert_eq!(
            format_rate_limit_summary(&update),
            "Extra usage is required for 1M context. Use /extra-usage to enable it, /model to switch models, or /1m-context disable to turn off 1M context for this folder."
        );
    }

    #[test]
    fn org_level_disabled_with_primary_context_keeps_normal_rate_limit_message() {
        let update = RateLimitUpdate {
            status: RateLimitStatus::Rejected,
            resets_at: Some(1_741_280_000.0),
            utilization: None,
            rate_limit_type: Some("five_hour".to_owned()),
            overage_status: None,
            overage_resets_at: None,
            overage_disabled_reason: Some("org_level_disabled".to_owned()),
            is_using_overage: Some(false),
            surpassed_threshold: None,
        };

        let summary = format_rate_limit_summary(&update);
        assert!(summary.contains("Rate limit reached"));
        assert!(summary.contains("5-hour rate limit"));
        assert!(!summary.contains("Extra usage is required for 1M context"));
    }
}
