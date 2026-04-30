// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::notices::upsert_turn_notice;
use crate::agent::model::ApiRetryError;
use crate::app::{App, NoticeDedupKey, NoticeStage, SystemSeverity};

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
    upsert_turn_notice(
        app,
        NoticeDedupKey::ApiRetry,
        NoticeStage::Warning,
        SystemSeverity::Warning,
        &message,
    );
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
        ApiRetryError::AuthenticationFailed => "authentication_failed",
        ApiRetryError::BillingError => "billing_error",
        ApiRetryError::RateLimit => "rate_limit",
        ApiRetryError::InvalidRequest => "invalid_request",
        ApiRetryError::ServerError => "server_error",
        ApiRetryError::MaxOutputTokens => "max_output_tokens",
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
    use super::{format_api_retry_message, format_retry_delay};
    use crate::agent::model::ApiRetryError;

    #[test]
    fn formats_api_retry_http_status() {
        assert_eq!(
            format_api_retry_message(2, 4, 1500.0, Some(529), ApiRetryError::ServerError),
            "API retry 2/4 after server_error HTTP 529, retrying in 1.5s",
        );
    }

    #[test]
    fn formats_api_retry_without_http_response() {
        assert_eq!(
            format_api_retry_message(1, 4, 250.0, None, ApiRetryError::Unknown),
            "API retry 1/4 after connection error, retrying in 250ms",
        );
    }

    #[test]
    fn formats_second_delay_with_one_decimal() {
        assert_eq!(format_retry_delay(1000.0), "1.0s");
    }

    #[test]
    fn formats_fractional_delay_at_display_boundary() {
        assert_eq!(format_retry_delay(549.888_169_845_942_6), "550ms");
    }
}
