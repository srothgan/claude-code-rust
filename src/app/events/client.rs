// SPDX-License-Identifier: Apache-2.0

mod host;
mod mcp;
mod session;
mod turn;

use super::App;
use crate::agent::events::ClientEvent;

#[derive(Clone, Copy)]
enum ClientEventFamily {
    Turn,
    Mcp,
    Session,
    Host,
}

pub fn handle_client_event(app: &mut App, event: ClientEvent) {
    if is_stale_session_event(app, &event) {
        return;
    }

    app.request_active_surface_repaint();
    match client_event_family(&event) {
        ClientEventFamily::Turn => turn::handle(app, event),
        ClientEventFamily::Mcp => mcp::handle(app, event),
        ClientEventFamily::Session => session::handle(app, event),
        ClientEventFamily::Host => host::handle(app, event),
    }
}

fn is_stale_session_event(app: &App, event: &ClientEvent) -> bool {
    let Some(event_session_id) = event.scoped_session_id() else {
        return false;
    };
    if app.session_runtime.session_id.as_ref().map(crate::agent::model::SessionId::as_str)
        == Some(event_session_id)
    {
        return false;
    }

    tracing::debug!(
        target: crate::logging::targets::APP_SESSION,
        event_name = "stale_client_event_dropped",
        message = "client event dropped for a stale session",
        outcome = "dropped",
        session_id = %event_session_id,
        active_session_id = app
            .session_runtime
            .session_id
            .as_ref()
            .map_or("<none>", crate::agent::model::SessionId::as_str),
    );
    true
}

fn client_event_family(event: &ClientEvent) -> ClientEventFamily {
    match event {
        ClientEvent::SessionUpdate { .. }
        | ClientEvent::PermissionRequest { .. }
        | ClientEvent::QuestionRequest { .. }
        | ClientEvent::UserDialogRequest { .. }
        | ClientEvent::UserMessageQueued { .. }
        | ClientEvent::UserMessageStarted { .. }
        | ClientEvent::UserMessageRejected { .. }
        | ClientEvent::TurnInterruptReceipt { .. }
        | ClientEvent::TurnComplete { .. }
        | ClientEvent::TurnError { .. }
        | ClientEvent::TurnErrorClassified { .. }
        | ClientEvent::SlashCommandError { .. } => ClientEventFamily::Turn,
        ClientEvent::McpElicitationRequest { .. }
        | ClientEvent::McpElicitationCompleted { .. }
        | ClientEvent::McpElicitationResponseQueued { .. }
        | ClientEvent::McpAuthRedirect { .. }
        | ClientEvent::McpOperationError { .. }
        | ClientEvent::McpSetServersResult { .. }
        | ClientEvent::McpConfigRemoveSucceeded { .. }
        | ClientEvent::McpConfigRemoveFailed { .. }
        | ClientEvent::McpSnapshotReceived { .. } => ClientEventFamily::Mcp,
        ClientEvent::Connected { .. }
        | ClientEvent::ConnectionFailed(_)
        | ClientEvent::AuthRequired { .. }
        | ClientEvent::SessionResumeFailed { .. }
        | ClientEvent::SessionReplaced { .. }
        | ClientEvent::SessionsListed { .. }
        | ClientEvent::UpdateAvailable { .. }
        | ClientEvent::ServiceStatus { .. }
        | ClientEvent::AuthCompleted { .. }
        | ClientEvent::LogoutCompleted
        | ClientEvent::StatusSnapshotReceived { .. }
        | ClientEvent::ContextUsageReceived { .. }
        | ClientEvent::RewindTargetsReceived { .. }
        | ClientEvent::RewindResultReceived { .. }
        | ClientEvent::FatalError(_) => ClientEventFamily::Session,
        ClientEvent::TerminalReleasedToChild { .. }
        | ClientEvent::TerminalReturnedFromChild { .. }
        | ClientEvent::RuntimeReloadCompleted { .. }
        | ClientEvent::RuntimeReloadFailed { .. }
        | ClientEvent::StructuredUsageReceived { .. }
        | ClientEvent::UsageRefreshStarted { .. }
        | ClientEvent::UsageSnapshotReceived { .. }
        | ClientEvent::UsageRefreshFailed { .. }
        | ClientEvent::PluginsInventoryUpdated { .. }
        | ClientEvent::PluginsInventoryRefreshFailed { .. }
        | ClientEvent::PluginsCliActionSucceeded { .. }
        | ClientEvent::PluginsCliActionFailed { .. } => ClientEventFamily::Host,
    }
}
