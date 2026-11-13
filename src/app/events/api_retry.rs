// SPDX-License-Identifier: Apache-2.0

use super::notices::upsert_turn_notice;
use crate::agent::model::ApiRetryError;
use crate::app::{App, NoticeDedupKey, SystemSeverity};

pub(super) fn handle_api_retry_update(
    app: &mut App,
    attempt: u64,
    max_retries: u64,
    retry_delay_ms: f64,
    error_status: Option<u16>,
    error: ApiRetryError,
) {
    let message =
        format_api_retry_message(attempt, max_retries, retry_delay_ms, error_status, error);
    upsert_turn_notice(app, NoticeDedupKey::ApiRetry, SystemSeverity::Warning, &message);
}

fn format_api_retry_message(
    attempt: u64,
    max_retries: u64,
    retry_delay_ms: f64,
    error_status: Option<u16>,
    error: ApiRetryError,
) -> String {
    let error_label = api_retry_error_label(error);
    let status = error_status.map_or_else(String::new, |status| format!(" HTTP {status}"));
    let delay = format_retry_delay(retry_delay_ms);
    format!("API retry {attempt}/{max_retries} after {error_label}{status}, retrying in {delay}")
}

fn api_retry_error_label(error: ApiRetryError) -> &'static str {
    match error {
        ApiRetryError::AuthenticationFailed => "authentication failure",
        ApiRetryError::AccountOnHold => "account on hold",
        ApiRetryError::OauthOrgNotAllowed => "account access not allowed",
        ApiRetryError::BillingError => "billing issue",
        ApiRetryError::RateLimit => "rate limit",
        ApiRetryError::Overloaded => "service overloaded",
        ApiRetryError::InvalidRequest => "invalid request",
        ApiRetryError::ModelNotFound => "model unavailable",
        ApiRetryError::ServerError => "server error",
        ApiRetryError::VerificationRequired => "verification required",
        ApiRetryError::CloudCredentialError => "cloud provider credentials",
        ApiRetryError::MaxOutputTokens => "max output tokens",
        ApiRetryError::Unknown => "connection error",
    }
}

fn format_retry_delay(retry_delay_ms: f64) -> String {
    if retry_delay_ms >= 1000.0 {
        let tenths = (retry_delay_ms / 100.0).ceil();
        format!("{:.1}s", tenths / 10.0)
    } else {
        format!("{:.0}ms", retry_delay_ms.ceil())
    }
}

#[cfg(test)]
mod tests {
    use super::format_retry_delay;

    #[test]
    fn formats_fractional_delay_at_display_boundary() {
        assert_eq!(format_retry_delay(549.888_169_845_942_6), "550ms");
        for (delay, expected) in
            [(0.0, "0ms"), (999.9, "1000ms"), (1000.0, "1.0s"), (1000.1, "1.1s"), (1250.5, "1.3s")]
        {
            assert_eq!(format_retry_delay(delay), expected);
        }
    }
}
