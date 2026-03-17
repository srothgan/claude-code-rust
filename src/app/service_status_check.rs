// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::App;
use crate::agent::events::{ClientEvent, ServiceStatusSeverity};
use serde::Deserialize;
use std::time::Duration;

const SERVICE_STATUS_TIMEOUT: Duration = Duration::from_secs(4);
const STATUSPAGE_STATUS_URL: &str = "https://status.claude.com/api/v2/status.json";

#[derive(Debug, Clone, PartialEq, Eq)]
struct ServiceStatusIssue {
    severity: ServiceStatusSeverity,
    message: String,
}

#[derive(Debug, Clone, Deserialize)]
struct StatusPageResponse {
    status: StatusPageStatus,
}

#[derive(Debug, Clone, Deserialize)]
struct StatusPageStatus {
    indicator: String,
    description: String,
}

pub fn start_service_status_check(app: &App) {
    let event_tx = app.event_tx.clone();

    tokio::task::spawn_local(async move {
        let Some(issue) = resolve_service_status_issue().await else {
            return;
        };
        let _ = event_tx
            .send(ClientEvent::ServiceStatus { severity: issue.severity, message: issue.message });
    });
}

async fn resolve_service_status_issue() -> Option<ServiceStatusIssue> {
    let client = reqwest::Client::builder().timeout(SERVICE_STATUS_TIMEOUT).build().ok()?;
    let response = client.get(STATUSPAGE_STATUS_URL).send().await.ok()?;
    if !response.status().is_success() {
        tracing::debug!("service-status request failed with status {}", response.status());
        return None;
    }

    let payload = response.json::<StatusPageResponse>().await.ok()?;
    classify_status_indicator(&payload.status)
}

fn classify_status_indicator(status: &StatusPageStatus) -> Option<ServiceStatusIssue> {
    let indicator = status.indicator.trim().to_ascii_lowercase();
    if indicator == "none" {
        return None;
    }

    let severity = match indicator.as_str() {
        "major" | "critical" => ServiceStatusSeverity::Error,
        _ => ServiceStatusSeverity::Warning,
    };

    let description = status.description.trim();
    let message = if description.is_empty() {
        "Claude Code status indicates a service disruption.".to_owned()
    } else {
        format!("Claude Code status: {description}.")
    };

    Some(ServiceStatusIssue { severity, message })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(indicator: &str, description: &str) -> StatusPageStatus {
        StatusPageStatus { indicator: indicator.to_owned(), description: description.to_owned() }
    }

    #[test]
    fn classify_none_indicator_as_healthy() {
        assert!(classify_status_indicator(&status("none", "All Systems Operational")).is_none());
    }

    #[test]
    fn classify_minor_indicator_as_warning() {
        let issue = classify_status_indicator(&status("minor", "Partial Outage"))
            .expect("expected warning issue");
        assert_eq!(issue.severity, ServiceStatusSeverity::Warning);
    }

    #[test]
    fn classify_major_indicator_as_error() {
        let issue = classify_status_indicator(&status("major", "Major Outage"))
            .expect("expected error issue");
        assert_eq!(issue.severity, ServiceStatusSeverity::Error);
    }

    #[test]
    fn classify_unknown_indicator_as_warning() {
        let issue = classify_status_indicator(&status("maintenance", "Maintenance"))
            .expect("expected warning issue");
        assert_eq!(issue.severity, ServiceStatusSeverity::Warning);
    }

    #[test]
    fn classify_uses_description_for_message() {
        let issue = classify_status_indicator(&status("minor", "Minor Service Outage"))
            .expect("expected warning issue");
        assert_eq!(issue.message, "Claude Code status: Minor Service Outage.");
    }
}
