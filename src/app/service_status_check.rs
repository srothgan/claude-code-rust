// SPDX-License-Identifier: Apache-2.0

use super::App;
use crate::agent::events::{ClientEvent, ServiceStatusSeverity};
use serde::Deserialize;
use std::time::Duration;
use tracing::{Instrument as _, info_span};

const SERVICE_STATUS_TIMEOUT: Duration = Duration::from_secs(4);
const STATUSPAGE_SUMMARY_URL: &str = "https://status.claude.com/api/v2/summary.json";
const SERVICE_STATUS_CONNECTIVITY_MESSAGE: &str =
    "Claude Code could not connect to the internet. Please check your connection.";

/// Component names we care about. "Claude Code" is the primary component;
/// "Claude API" is included because Claude Code depends on it.
const RELEVANT_COMPONENTS: &[&str] = &["Claude Code", "Claude API (api.anthropic.com)"];

#[derive(Debug, Clone, PartialEq, Eq)]
struct ServiceStatusIssue {
    severity: ServiceStatusSeverity,
    message: String,
}

#[derive(Debug, Clone, Deserialize)]
struct SummaryResponse {
    components: Vec<Component>,
    incidents: Vec<Incident>,
}

#[derive(Debug, Clone, Deserialize)]
struct Component {
    name: String,
    status: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Incident {
    name: String,
    components: Vec<IncidentComponent>,
}

#[derive(Debug, Clone, Deserialize)]
struct IncidentComponent {
    name: String,
}

pub fn start_service_status_check(app: &App) {
    let event_tx = app.event_tx.clone();
    tracing::info!(
        target: crate::logging::targets::APP_NETWORK,
        event_name = "service_check_started",
        message = "service status check started",
        outcome = "start",
        url = STATUSPAGE_SUMMARY_URL,
    );

    let service_status_span = info_span!(
        target: crate::logging::targets::APP_NETWORK,
        "service_status_check",
        url = STATUSPAGE_SUMMARY_URL,
    );

    tokio::task::spawn_local(
        run_service_status_check(
            event_tx,
            STATUSPAGE_SUMMARY_URL,
            SERVICE_STATUS_TIMEOUT,
            reqwest::Client::builder(),
        )
        .instrument(service_status_span),
    );
}

async fn run_service_status_check(
    event_tx: tokio::sync::mpsc::Sender<ClientEvent>,
    endpoint: &str,
    timeout: Duration,
    client_builder: reqwest::ClientBuilder,
) {
    let Some(issue) = resolve_service_status_issue(endpoint, timeout, client_builder).await else {
        return;
    };
    tracing::info!(
        target: crate::logging::targets::APP_NETWORK,
        event_name = "service_issue_detected",
        message = "service status issue detected",
        outcome = "success",
        severity = ?issue.severity,
    );
    let _ = event_tx
        .send(ClientEvent::ServiceStatus { severity: issue.severity, message: issue.message })
        .await;
}

async fn resolve_service_status_issue(
    endpoint: &str,
    timeout: Duration,
    client_builder: reqwest::ClientBuilder,
) -> Option<ServiceStatusIssue> {
    let client = match client_builder.timeout(timeout).build() {
        Ok(client) => client,
        Err(error) => {
            tracing::warn!(
                target: crate::logging::targets::APP_NETWORK,
                event_name = "service_check_failed",
                message = "service status client creation failed",
                outcome = "failure",
                error_message = %error,
                url = endpoint,
            );
            return Some(connectivity_issue());
        }
    };
    let response = match client.get(endpoint).send().await {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(
                target: crate::logging::targets::APP_NETWORK,
                event_name = "service_check_failed",
                message = "service status request failed",
                outcome = "failure",
                error_message = %error,
                url = endpoint,
            );
            return Some(connectivity_issue());
        }
    };
    if !response.status().is_success() {
        tracing::warn!(
            target: crate::logging::targets::APP_NETWORK,
            event_name = "service_check_failed",
            message = "service status request failed",
            outcome = "failure",
            status = %response.status(),
            url = endpoint,
        );
        return None;
    }

    let payload = response.json::<SummaryResponse>().await.ok()?;
    classify_summary(&payload)
}

fn connectivity_issue() -> ServiceStatusIssue {
    ServiceStatusIssue {
        severity: ServiceStatusSeverity::Warning,
        message: SERVICE_STATUS_CONNECTIVITY_MESSAGE.to_owned(),
    }
}

fn is_relevant_component(name: &str) -> bool {
    RELEVANT_COMPONENTS.contains(&name)
}

fn classify_component_status(status: &str) -> Option<ServiceStatusSeverity> {
    match status {
        "operational" | "under_maintenance" => None,
        "major_outage" => Some(ServiceStatusSeverity::Error),
        // degraded_performance, partial_outage, or unknown
        _ => Some(ServiceStatusSeverity::Warning),
    }
}

fn classify_summary(summary: &SummaryResponse) -> Option<ServiceStatusIssue> {
    // Check if any relevant component is degraded
    let worst_severity = summary
        .components
        .iter()
        .filter(|c| is_relevant_component(&c.name))
        .filter_map(|c| classify_component_status(&c.status))
        .max_by_key(|s| match s {
            ServiceStatusSeverity::Warning => 0,
            ServiceStatusSeverity::Error => 1,
        });

    let severity = worst_severity?;

    // Find incidents that affect our relevant components for a better message
    let relevant_incident = summary
        .incidents
        .iter()
        .find(|incident| incident.components.iter().any(|c| is_relevant_component(&c.name)));

    let message = if let Some(incident) = relevant_incident {
        format!("Claude Code status: {}.", incident.name.trim())
    } else {
        "Claude Code status indicates a service disruption.".to_owned()
    };

    Some(ServiceStatusIssue { severity, message })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn component(name: &str, status: &str) -> Component {
        Component { name: name.to_owned(), status: status.to_owned() }
    }

    fn incident(name: &str, component_names: &[&str]) -> Incident {
        Incident {
            name: name.to_owned(),
            components: component_names
                .iter()
                .map(|n| IncidentComponent { name: (*n).to_owned() })
                .collect(),
        }
    }

    fn summary(components: Vec<Component>, incidents: Vec<Incident>) -> SummaryResponse {
        SummaryResponse { components, incidents }
    }

    #[test]
    fn relevant_components_determine_the_worst_advisory_severity() {
        use ServiceStatusSeverity::{Error, Warning};
        let cases = [
            (
                vec![
                    ("Claude Code", "operational"),
                    ("Claude API (api.anthropic.com)", "operational"),
                ],
                None,
            ),
            (
                vec![
                    ("Claude Code", "operational"),
                    ("Claude API (api.anthropic.com)", "under_maintenance"),
                ],
                None,
            ),
            (
                vec![
                    ("Claude Code", "under_maintenance"),
                    ("Claude API (api.anthropic.com)", "operational"),
                ],
                None,
            ),
            (vec![("claude.ai", "major_outage"), ("Claude Code", "operational")], None),
            (vec![("Claude Code", "degraded_performance")], Some(Warning)),
            (vec![("Claude Code", "major_outage")], Some(Error)),
            (vec![("Claude API (api.anthropic.com)", "partial_outage")], Some(Warning)),
            (vec![("Claude API (api.anthropic.com)", "future_status")], Some(Warning)),
            (
                vec![
                    ("Claude Code", "partial_outage"),
                    ("Claude API (api.anthropic.com)", "degraded_performance"),
                ],
                Some(Warning),
            ),
        ];
        for (components, expected) in cases {
            let payload = summary(
                components.iter().map(|(name, status)| component(name, status)).collect(),
                vec![],
            );
            assert_eq!(
                classify_summary(&payload).map(|issue| issue.severity),
                expected,
                "{components:?}"
            );
        }
        for major in ["Claude Code", "Claude API (api.anthropic.com)"] {
            let other = if major == "Claude Code" {
                "Claude API (api.anthropic.com)"
            } else {
                "Claude Code"
            };
            for reversed in [false, true] {
                let mut components = vec![
                    component(major, "major_outage"),
                    component(other, "degraded_performance"),
                ];
                if reversed {
                    components.reverse();
                }
                assert_eq!(
                    classify_summary(&summary(components, vec![])).expect("outage").severity,
                    Error
                );
            }
        }
    }

    #[test]
    fn advisory_uses_a_relevant_cause_or_useful_generic_fallback() {
        let unrelated = incident("UNRELATED web login failure", &["claude.ai"]);
        let relevant =
            incident("DISTINCT API inference interruption", &["Claude API (api.anthropic.com)"]);
        for (incidents, expected_cause) in [
            (vec![unrelated.clone(), relevant], Some("DISTINCT API inference interruption")),
            (vec![unrelated], None),
            (vec![], None),
        ] {
            let issue = classify_summary(&summary(
                vec![component("Claude Code", "partial_outage")],
                incidents,
            ))
            .expect("advisory");
            assert!(!issue.message.contains("UNRELATED"));
            if let Some(cause) = expected_cause {
                assert!(issue.message.contains(cause), "{}", issue.message);
            } else {
                assert!(issue.message.contains("Claude"));
                assert!(issue.message.to_lowercase().contains("disrupt"), "{}", issue.message);
            }
        }
    }

    async fn read_http_request(stream: &mut tokio::net::TcpStream) {
        use tokio::io::AsyncReadExt as _;
        let mut request = Vec::new();
        let mut buffer = [0; 1024];
        while !request.windows(4).any(|part| part == b"\r\n\r\n") {
            let count = stream.read(&mut buffer).await.expect("read local request");
            assert!(count > 0, "request closed before headers");
            request.extend_from_slice(&buffer[..count]);
            assert!(request.len() < 8192, "bounded request headers");
        }
    }

    async fn completed_worker(endpoint: &str) -> tokio::sync::mpsc::Receiver<ClientEvent> {
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        tokio::time::timeout(
            Duration::from_secs(10),
            run_service_status_check(
                tx,
                endpoint,
                SERVICE_STATUS_TIMEOUT,
                reqwest::Client::builder().no_proxy(),
            ),
        )
        .await
        .expect("status worker completed");
        rx
    }

    async fn assert_connectivity_warning(mut rx: tokio::sync::mpsc::Receiver<ClientEvent>) {
        let Some(ClientEvent::ServiceStatus { severity, message }) = rx.recv().await else {
            panic!("expected status advisory");
        };
        assert_eq!(severity, ServiceStatusSeverity::Warning);
        assert!(message.to_lowercase().contains("connect"), "{message}");
        assert!(rx.recv().await.is_none(), "one advisory per completed request");
    }

    #[tokio::test]
    async fn refused_status_connection_delivers_one_informative_warning() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let endpoint = format!("http://{}/summary", listener.local_addr().expect("address"));
        drop(listener);
        assert_connectivity_warning(completed_worker(&endpoint).await).await;
    }

    #[tokio::test]
    async fn held_status_request_reaches_the_configured_deadline() {
        use tokio::sync::oneshot;
        tokio::task::LocalSet::new()
            .run_until(async {
                let listener =
                    tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("listener");
                let endpoint =
                    format!("http://{}/summary", listener.local_addr().expect("address"));
                let (accepted_tx, accepted_rx) = oneshot::channel();
                let (release_tx, release_rx) = oneshot::channel();
                let server = tokio::spawn(async move {
                    let (mut stream, _) = listener.accept().await.expect("accept");
                    read_http_request(&mut stream).await;
                    accepted_tx.send(()).expect("request observed");
                    release_rx.await.expect("release held connection");
                    drop(stream);
                });
                let (tx, rx) = tokio::sync::mpsc::channel(4);
                let worker = tokio::task::spawn_local(async move {
                    run_service_status_check(
                        tx,
                        &endpoint,
                        SERVICE_STATUS_TIMEOUT,
                        reqwest::Client::builder().no_proxy(),
                    )
                    .await;
                });
                tokio::time::timeout(Duration::from_secs(10), accepted_rx)
                    .await
                    .expect("request reached server")
                    .expect("accepted");
                tokio::time::pause();
                assert!(!worker.is_finished(), "held request must still be pending");
                tokio::time::advance(SERVICE_STATUS_TIMEOUT).await;
                tokio::time::timeout(Duration::from_millis(10), worker)
                    .await
                    .expect("production deadline terminated request")
                    .expect("worker task");
                assert_connectivity_warning(rx).await;
                release_tx.send(()).expect("release server");
                server.await.expect("server task");
            })
            .await;
    }

    #[tokio::test]
    async fn completed_http_worker_admits_only_usable_successful_responses() {
        use tokio::io::AsyncWriteExt as _;
        const OUTAGE: &str = r#"{"components":[{"name":"Claude Code","status":"major_outage"}],"incidents":[{"name":"Fixture inference outage","components":[{"name":"Claude Code"}]}]}"#;
        const HEALTHY: &str =
            r#"{"components":[{"name":"Claude Code","status":"operational"}],"incidents":[]}"#;
        for (status, body, delivers) in [
            (200, OUTAGE, true),
            (200, HEALTHY, false),
            (503, OUTAGE, false),
            (200, "{broken", false),
            (200, r#"{"components":[]}"#, false),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("listener");
            let endpoint = format!("http://{}/summary", listener.local_addr().expect("address"));
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.expect("accept");
                read_http_request(&mut stream).await;
                let response = format!(
                    "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.expect("response");
            });
            let mut events = completed_worker(&endpoint).await;
            server.await.expect("server task");
            if delivers {
                let Some(ClientEvent::ServiceStatus { severity, message }) = events.recv().await
                else {
                    panic!("expected outage event");
                };
                assert_eq!(severity, ServiceStatusSeverity::Error);
                assert!(message.contains("Fixture inference outage"));
            }
            assert!(events.recv().await.is_none(), "completed HTTP {status}: {body}");
        }
    }
}
