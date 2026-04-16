// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use crate::app::App;

pub(crate) enum RuntimeReloadRequestOutcome {
    Requested,
    Unavailable,
    Failed,
}

pub(crate) fn request_runtime_reload(app: &mut App) -> RuntimeReloadRequestOutcome {
    let Some(conn) = app.conn.as_ref() else {
        return RuntimeReloadRequestOutcome::Unavailable;
    };
    let Some(ref sid) = app.session_id else {
        return RuntimeReloadRequestOutcome::Unavailable;
    };
    let session_id = sid.to_string();
    match conn.reload_plugins(session_id.clone()) {
        Ok(()) => {
            tracing::debug!(
                target: crate::logging::targets::APP_SESSION,
                event_name = "runtime_reload_requested",
                message = "session runtime plugin reload requested",
                outcome = "start",
                session_id = %session_id,
            );
            RuntimeReloadRequestOutcome::Requested
        }
        Err(error) => {
            tracing::warn!(
                target: crate::logging::targets::APP_SESSION,
                event_name = "runtime_reload_request_failed",
                message = "failed to request session runtime plugin reload",
                outcome = "failure",
                session_id = %session_id,
                error_message = %error,
            );
            RuntimeReloadRequestOutcome::Failed
        }
    }
}

pub(crate) fn request_context_usage_refresh(app: &mut App) {
    if app.session_usage.context_usage_in_flight {
        app.session_usage.context_usage_refresh_pending = true;
        return;
    }

    let Some(conn) = app.conn.as_ref() else {
        clear_context_usage_refresh_state(app);
        return;
    };
    let Some(ref sid) = app.session_id else {
        clear_context_usage_refresh_state(app);
        return;
    };

    let session_id = sid.to_string();
    app.session_usage.context_usage_in_flight = true;
    app.session_usage.context_usage_refresh_pending = false;
    match conn.get_context_usage(session_id.clone()) {
        Ok(()) => tracing::debug!(
            target: crate::logging::targets::APP_SESSION,
            event_name = "context_usage_requested",
            message = "session context usage requested",
            outcome = "start",
            session_id = %session_id,
        ),
        Err(error) => {
            app.session_usage.context_usage_in_flight = false;
            tracing::warn!(
                target: crate::logging::targets::APP_SESSION,
                event_name = "context_usage_request_failed",
                message = "failed to request session context usage",
                outcome = "failure",
                session_id = %session_id,
                error_message = %error,
            );
        }
    }
}

pub(crate) fn request_status_snapshot_refresh(app: &mut App) {
    let Some(conn) = app.conn.as_ref() else {
        return;
    };
    let Some(ref sid) = app.session_id else {
        return;
    };

    let session_id = sid.to_string();
    match conn.get_status_snapshot(session_id.clone()) {
        Ok(()) => tracing::debug!(
            target: crate::logging::targets::APP_AUTH,
            event_name = "status_snapshot_requested",
            message = "session status snapshot requested",
            outcome = "start",
            session_id = %session_id,
        ),
        Err(error) => tracing::warn!(
            target: crate::logging::targets::APP_AUTH,
            event_name = "status_snapshot_request_failed",
            message = "failed to request session status snapshot",
            outcome = "failure",
            session_id = %session_id,
            error_message = %error,
        ),
    }
}

pub(crate) fn apply_context_usage_snapshot(app: &mut App, percentage: Option<u8>) {
    app.session_usage.context_usage_percent = percentage;
    app.session_usage.context_usage_in_flight = false;
    let refresh_pending = std::mem::take(&mut app.session_usage.context_usage_refresh_pending);
    if refresh_pending {
        request_context_usage_refresh(app);
    }
}

fn clear_context_usage_refresh_state(app: &mut App) {
    app.session_usage.context_usage_in_flight = false;
    app.session_usage.context_usage_refresh_pending = false;
}

#[cfg(test)]
mod tests {
    use super::{
        RuntimeReloadRequestOutcome, apply_context_usage_snapshot, request_context_usage_refresh,
        request_runtime_reload, request_status_snapshot_refresh,
    };
    use crate::agent::model;
    use crate::agent::wire::BridgeCommand;
    use crate::app::App;

    fn app_with_connection()
    -> (App, tokio::sync::mpsc::UnboundedReceiver<crate::agent::wire::CommandEnvelope>) {
        let mut app = App::test_default();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        app.conn = Some(std::rc::Rc::new(crate::agent::client::AgentConnection::new(tx)));
        app.session_id = Some(model::SessionId::new("session-1"));
        (app, rx)
    }

    #[test]
    fn request_runtime_reload_sends_bridge_command() {
        let (mut app, mut rx) = app_with_connection();

        assert!(matches!(request_runtime_reload(&mut app), RuntimeReloadRequestOutcome::Requested));

        let envelope = rx.try_recv().expect("reload command");
        assert!(matches!(
            envelope.command,
            BridgeCommand::ReloadPlugins { session_id } if session_id == "session-1"
        ));
    }

    #[test]
    fn request_runtime_reload_reports_unavailable_without_session_connection() {
        let mut app = App::test_default();

        assert!(matches!(
            request_runtime_reload(&mut app),
            RuntimeReloadRequestOutcome::Unavailable
        ));
    }

    #[test]
    fn request_context_usage_refresh_coalesces_in_flight_requests() {
        let (mut app, mut rx) = app_with_connection();

        request_context_usage_refresh(&mut app);
        request_context_usage_refresh(&mut app);

        assert!(app.session_usage.context_usage_in_flight);
        assert!(app.session_usage.context_usage_refresh_pending);
        let envelope = rx.try_recv().expect("context usage command");
        assert!(matches!(
            envelope.command,
            BridgeCommand::GetContextUsage { session_id } if session_id == "session-1"
        ));
        assert!(rx.try_recv().is_err(), "coalesced refresh should not send twice");
    }

    #[test]
    fn apply_context_usage_snapshot_replays_pending_refresh() {
        let (mut app, mut rx) = app_with_connection();
        request_context_usage_refresh(&mut app);
        request_context_usage_refresh(&mut app);
        let _ = rx.try_recv().expect("initial context usage command");

        apply_context_usage_snapshot(&mut app, Some(62));

        assert_eq!(app.session_usage.context_usage_percent, Some(62));
        assert!(app.session_usage.context_usage_in_flight);
        assert!(!app.session_usage.context_usage_refresh_pending);
        let envelope = rx.try_recv().expect("replayed context usage command");
        assert!(matches!(
            envelope.command,
            BridgeCommand::GetContextUsage { session_id } if session_id == "session-1"
        ));
    }

    #[test]
    fn request_status_snapshot_refresh_sends_bridge_command() {
        let (mut app, mut rx) = app_with_connection();

        request_status_snapshot_refresh(&mut app);

        let envelope = rx.try_recv().expect("status snapshot command");
        assert!(matches!(
            envelope.command,
            BridgeCommand::GetStatusSnapshot { session_id } if session_id == "session-1"
        ));
    }
}
