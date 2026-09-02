// SPDX-License-Identifier: Apache-2.0
use claude_code_rust::agent::events::ClientEvent;
use claude_code_rust::agent::model;
use claude_code_rust::app::App;
use tokio::sync::oneshot;

/// Build a minimal `App` for in-process integration-style testing.
/// This exercises app state and event handling directly, without a real bridge or TUI boundary.
pub fn test_app() -> App {
    let mut app = App::test_default();
    app.session_runtime.session_id = Some(model::SessionId::new("test-session"));
    app
}

/// Send a client event through the app's in-process event handling pipeline.
pub fn send_client_event(app: &mut App, event: ClientEvent) {
    claude_code_rust::app::handle_client_event(app, event);
}

pub fn session_update(update: model::SessionUpdate) -> ClientEvent {
    ClientEvent::SessionUpdate { session_id: "test-session".to_owned(), update }
}

pub fn turn_complete() -> ClientEvent {
    ClientEvent::TurnComplete {
        session_id: "test-session".to_owned(),
        queued_turn_count: None,
        terminal_reason: None,
    }
}

pub fn permission_request(
    request: model::RequestPermissionRequest,
    response_tx: oneshot::Sender<model::RequestPermissionResponse>,
) -> ClientEvent {
    ClientEvent::PermissionRequest { session_id: "test-session".to_owned(), request, response_tx }
}
