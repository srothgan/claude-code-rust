// SPDX-License-Identifier: Apache-2.0

use super::super::{App, NoticeDedupKey, RateLimitIncidentKey, SystemSeverity};
use crate::agent::model;
use std::time::Duration;

fn format_rate_limit_type(raw: &str) -> &str {
    match raw {
        "five_hour" => "5-hour",
        "daily" => "daily",
        "minute" => "per-minute",
        "seven_day" => "7-day",
        "seven_day_opus" => "7-day Opus",
        "seven_day_sonnet" => "7-day Sonnet",
        "seven_day_overage_included" => "7-day overage-included",
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

fn format_rate_limit_warning(update: &model::RateLimitUpdate) -> String {
    let intro = if update.limit_scope.as_deref() == Some("group_pool") {
        "Approaching shared group rate limit"
    } else {
        "Approaching rate limit"
    };

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

    // Enablement, availability and current use are separate SDK observations.
    if update.overage_enabled == Some(false)
        || update.overage_disabled_reason.is_some()
        || update.overage_status == Some(model::RateLimitStatus::Rejected)
    {
        message.push_str(" Extra usage is unavailable.");
    } else if update.is_using_overage == Some(true) {
        message.push_str(" You are using your overage allowance.");
    } else if update.overage_enabled == Some(true) {
        if matches!(
            update.overage_status,
            Some(model::RateLimitStatus::Allowed | model::RateLimitStatus::AllowedWarning)
        ) {
            message.push_str(" You can continue using your overage allowance.");
        } else {
            message.push_str(" Extra usage is enabled.");
        }
    }

    // Resets in X at HH:MM
    if let Some(resets_at) = update.resets_at {
        use std::fmt::Write;
        let _ = write!(message, " Resets in {}.", format_resets_at(resets_at));
    }

    message
}

fn rate_limit_notice_key(update: &model::RateLimitUpdate) -> NoticeDedupKey {
    NoticeDedupKey::RateLimit(RateLimitIncidentKey {
        rate_limit_type: update.rate_limit_type.clone(),
        limit_scope: update.limit_scope.clone(),
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
    tracing::debug!(
        target: crate::logging::targets::APP_SESSION,
        event_name = "rate_limit_update_applied",
        message = "rate limit update applied",
        outcome = "success",
        status = ?update.status,
        utilization = update.utilization,
        rate_limit_type = update.rate_limit_type.as_deref().unwrap_or(""),
        limit_scope = update.limit_scope.as_deref().unwrap_or(""),
        resets_at = update.resets_at.unwrap_or_default(),
        overage_enabled = ?update.overage_enabled,
        overage_status = ?update.overage_status,
        overage_resets_at = update.overage_resets_at.unwrap_or_default(),
        overage_disabled_reason = update.overage_disabled_reason.as_deref().unwrap_or(""),
        is_using_overage = ?update.is_using_overage,
        surpassed_threshold = update.surpassed_threshold.unwrap_or_default(),
    );

    match update.status {
        // Rejections are reported by the turn result, which carries its own error text.
        model::RateLimitStatus::Allowed | model::RateLimitStatus::Rejected => {}
        model::RateLimitStatus::AllowedWarning => {
            let summary = format_rate_limit_warning(update);
            super::notices::upsert_turn_notice(
                app,
                rate_limit_notice_key(update),
                SystemSeverity::Warning,
                &summary,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::format_rate_limit_warning;
    use crate::agent::model::{RateLimitStatus, RateLimitUpdate};

    #[test]
    fn overage_warning_requires_enabled_and_available_allowance() {
        for enabled in [None, Some(false), Some(true)] {
            for status in [
                None,
                Some(RateLimitStatus::Allowed),
                Some(RateLimitStatus::AllowedWarning),
                Some(RateLimitStatus::Rejected),
            ] {
                for disabled_reason in [None, Some("out_of_credits")] {
                    let update: RateLimitUpdate = serde_json::from_value(serde_json::json!({
                        "status": "allowed_warning", "overage_enabled": enabled,
                        "overage_status": status, "overage_disabled_reason": disabled_reason,
                        "is_using_overage": false
                    }))
                    .expect("warning fixture");
                    let summary = format_rate_limit_warning(&update);
                    let available = enabled == Some(true)
                        && matches!(
                            status,
                            Some(RateLimitStatus::Allowed | RateLimitStatus::AllowedWarning)
                        )
                        && disabled_reason.is_none();
                    assert_eq!(
                        summary.contains("can continue"),
                        available,
                        "{update:?}: {summary}"
                    );
                }
            }
        }
    }

    #[test]
    fn current_use_does_not_override_an_unavailable_allowance() {
        for disabled in [false, true] {
            let update: RateLimitUpdate = serde_json::from_value(serde_json::json!({
                "status": "allowed_warning", "overage_enabled": !disabled,
                "is_using_overage": true
            }))
            .expect("warning fixture");
            let summary = format_rate_limit_warning(&update);
            assert_eq!(summary.contains("are using"), !disabled);
            assert!(!summary.contains("can continue"));
        }
    }

    #[test]
    fn seven_day_overage_included_rate_limit_type_renders_readable_label() {
        let update = RateLimitUpdate {
            status: RateLimitStatus::AllowedWarning,
            error_code: None,
            resets_at: None,
            utilization: Some(0.92),
            rate_limit_type: Some("seven_day_overage_included".to_owned()),
            limit_scope: None,
            overage_enabled: None,
            overage_status: None,
            overage_resets_at: None,
            overage_disabled_reason: None,
            is_using_overage: None,
            surpassed_threshold: None,
            can_user_purchase_credits: None,
            has_chargeable_saved_payment_method: None,
        };

        let summary = format_rate_limit_warning(&update);
        assert!(summary.contains("7-day overage-included rate limit"));
        assert!(!summary.contains("seven_day_overage_included"));
    }
}
