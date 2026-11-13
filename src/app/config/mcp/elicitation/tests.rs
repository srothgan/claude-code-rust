// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::agent::{client::AgentConnection, wire::BridgeCommand};
use serde_json::json;

#[path = "audit_tests.rs"]
mod audit;

fn request(id: &str) -> types::ElicitationRequest {
    types::ElicitationRequest {
        request_id: id.into(),
        server_name: "forms".into(),
        message: format!("Request {id}"),
        mode: types::ElicitationMode::Form,
        url: None,
        elicitation_id: None,
        field_order: vec!["name".into(), "flag".into()],
        requested_schema: Some(
            json!({"type":"object","properties":{"name":{"type":"string","minLength":2},"flag":{"type":"boolean"}},"required":["name","flag"]}),
        ),
    }
}
fn app() -> App {
    let mut app = App::test_default();
    app.session_runtime.session_id = Some(model::SessionId::new("session-1"));
    app
}
fn event(app: &mut App, event: ClientEvent) {
    crate::app::events::handle_client_event(app, event);
}
fn settled(app: &mut App, id: &str) {
    event(
        app,
        ClientEvent::McpElicitationSettled {
            session_id: "session-1".into(),
            request_id: id.into(),
            session_closing: false,
        },
    );
}

#[test]
fn a_late_teardown_for_a_settled_request_cannot_clear_a_new_queue() {
    let mut app = app();
    present_mcp_elicitation_request(&mut app, request("old"));
    settled(&mut app, "old");
    present_mcp_elicitation_request(&mut app, request("new"));
    event(
        &mut app,
        ClientEvent::McpElicitationSettled {
            session_id: "session-1".into(),
            request_id: "old".into(),
            session_closing: true,
        },
    );
    assert_eq!(app.mcp.elicitations.active().unwrap().request.request_id, "new");
}
