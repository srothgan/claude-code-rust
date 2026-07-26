// SPDX-License-Identifier: Apache-2.0

use super::super::{App, session};
use crate::agent::events::ClientEvent;

pub(super) fn handle(app: &mut App, event: ClientEvent) {
    match event {
        ClientEvent::Connected {
            session_id,
            cwd,
            current_model,
            available_models,
            mode,
            fast_mode_state,
            fast_mode_disabled_reason,
            history_updates,
        } => {
            session::handle_connected_client_event(
                app,
                session::ConnectedEventData {
                    session_id,
                    cwd,
                    current_model,
                    available_models,
                    mode,
                    fast_mode_state,
                    fast_mode_disabled_reason,
                    history_updates,
                },
            );
            refresh_session_snapshots(app);
        }
        ClientEvent::SessionReplaced {
            session_id,
            cwd,
            current_model,
            available_models,
            mode,
            fast_mode_state,
            fast_mode_disabled_reason,
            history_updates,
            restored_input,
        } => {
            session::handle_session_replaced_event(
                app,
                session::SessionReplacedEventData {
                    session_id,
                    cwd,
                    current_model,
                    available_models,
                    mode,
                    fast_mode_state,
                    fast_mode_disabled_reason,
                    history_updates,
                    restored_input,
                },
            );
            refresh_session_snapshots(app);
        }
        ClientEvent::SessionsListed { sessions } => {
            session::handle_sessions_listed_event(app, sessions);
        }
        ClientEvent::AuthRequired { method_name, method_description } => {
            session::handle_auth_required_event(app, method_name, method_description);
        }
        ClientEvent::ConnectionFailed(message) => {
            session::handle_connection_failed_event(app, &message);
        }
        ClientEvent::UpdateAvailable { latest_version, current_version } => {
            session::handle_update_available_event(app, &latest_version, &current_version);
        }
        ClientEvent::ServiceStatus { severity, message } => {
            session::handle_service_status_event(app, severity, &message);
        }
        ClientEvent::AuthCompleted { conn } => {
            session::handle_auth_completed_event(app, &conn);
        }
        ClientEvent::LogoutCompleted => {
            session::handle_logout_completed_event(app);
        }
        ClientEvent::StatusSnapshotReceived { session_id, account } => {
            apply_status_snapshot(app, &session_id, account);
        }
        ClientEvent::ContextUsageReceived { session_id: _, percentage } => {
            crate::app::session_runtime::apply_context_usage_snapshot(app, percentage);
        }
        ClientEvent::RewindTargetsReceived { session_id, targets } => {
            app.sdk_inventory.rewind_targets = targets;
            app.sdk_inventory.rewind_targets_session_id =
                Some(crate::agent::model::SessionId::new(session_id));
            app.sdk_inventory.rewind_targets_in_flight = false;
            crate::app::slash::sync_with_cursor(app);
        }
        ClientEvent::RewindResultReceived { result } => {
            session::handle_rewind_result_event(app, &result);
        }
        ClientEvent::FatalError(error) => session::handle_fatal_error_event(app, error),
        _ => unreachable!("client event family routed a non-session event to the session handler"),
    }
}

fn refresh_session_snapshots(app: &mut App) {
    crate::app::config::refresh_mcp_snapshot(app);
    crate::app::session_runtime::request_status_snapshot_refresh(app);
    crate::app::session_runtime::request_context_usage_refresh(app);
}

fn apply_status_snapshot(
    app: &mut App,
    session_id: &str,
    account: crate::agent::model::AccountInfo,
) {
    let has_email = account.email.as_deref().is_some_and(|email| !email.trim().is_empty());
    let has_organization = account.organization.is_some();
    let subscription_type = account.subscription_type.clone();
    let token_source = account.token_source.clone();
    let api_key_source = account.api_key_source.clone();
    let api_provider = account.api_provider.clone();
    app.session_runtime.account_info = Some(account);
    app.sync_welcome_snapshot();
    app.request_active_surface_repaint();
    tracing::info!(
        target: crate::logging::targets::APP_AUTH,
        event_name = "status_snapshot_applied",
        message = "status snapshot applied",
        outcome = "success",
        session_id,
        has_email,
        has_organization,
        subscription_type = ?subscription_type,
        token_source = ?token_source,
        api_key_source = ?api_key_source,
        api_provider = ?api_provider,
    );
}
