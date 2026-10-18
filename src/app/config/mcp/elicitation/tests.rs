// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::agent::{client::AgentConnection, wire::BridgeCommand};
use serde_json::json;

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
fn key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
    crate::app::config::handle_key(app, KeyEvent::new(code, modifiers));
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
fn overlapping_forms_keep_focus_answers_and_queue_until_matching_settlement() {
    let mut app = app();
    present_mcp_elicitation_request(&mut app, request("a"));
    assert!(crate::app::config::handle_paste(&mut app, "retained-answer"));
    key(&mut app, KeyCode::Down, KeyModifiers::NONE);
    present_mcp_elicitation_request(&mut app, request("b"));
    present_mcp_elicitation_request(&mut app, request("a"));
    let active = app.mcp.elicitations.active().unwrap();
    assert_eq!(active.request.request_id, "a");
    assert_eq!(active.focus, 1);
    assert_eq!(
        active.form.as_ref().ok().unwrap().content().unwrap()["name"],
        json!("retained-answer")
    );
    assert_eq!(app.mcp.elicitations.len(), 2);
    event(
        &mut app,
        ClientEvent::McpElicitationResponseQueued {
            session_id: "session-1".into(),
            request_id: "a".into(),
        },
    );
    assert_eq!(app.mcp.elicitations.len(), 2);
    settled(&mut app, "b");
    assert_eq!(app.mcp.elicitations.active().unwrap().focus, 1);
    present_mcp_elicitation_request(&mut app, request("c"));
    settled(&mut app, "a");
    assert_eq!(app.mcp.elicitations.active().unwrap().request.request_id, "c");
    settled(&mut app, "a");
    event(
        &mut app,
        ClientEvent::McpElicitationResponseFailed {
            session_id: "session-1".into(),
            request_id: "a".into(),
        },
    );
    event(
        &mut app,
        ClientEvent::McpElicitationResponseQueued {
            session_id: "session-1".into(),
            request_id: "a".into(),
        },
    );
    assert!(app.mcp.elicitations.active().unwrap().error.is_none());
    present_mcp_elicitation_request(&mut app, request("a"));
    assert_eq!(app.mcp.elicitations.len(), 1, "settled duplicate must not reopen");
    settled(&mut app, "c");
    assert!(app.config.overlay.is_none());
}

#[test]
fn stale_sessions_cannot_edit_settle_or_fail_current_requests() {
    let mut app = app();
    present_mcp_elicitation_request(&mut app, request("same-id"));
    for event_value in [
        ClientEvent::McpElicitationRequest {
            session_id: "old-session".into(),
            request: request("old"),
        },
        ClientEvent::McpElicitationSettled {
            session_id: "old-session".into(),
            request_id: "same-id".into(),
            session_closing: false,
        },
        ClientEvent::McpElicitationResponseFailed {
            session_id: "old-session".into(),
            request_id: "same-id".into(),
        },
    ] {
        event(&mut app, event_value);
    }
    assert_eq!(app.mcp.elicitations.len(), 1);
    assert!(app.mcp.elicitations.active().unwrap().error.is_none());
    app.config.invalidate_session(None);
    app.mcp = crate::app::McpState::default();
    assert!(app.mcp.elicitations.is_empty());
    assert!(app.config.overlay.is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn validation_retry_duplicate_submit_and_exact_typed_response_follow_the_real_key_path() {
    tokio::task::LocalSet::new().run_until(async {
        let mut app = app();
        let (conn, mut commands) = AgentConnection::test_channel();
        app.session_runtime.conn = Some(std::rc::Rc::new(conn));
        present_mcp_elicitation_request(&mut app,request("a"));
        key(&mut app,KeyCode::Enter,KeyModifiers::CONTROL);
        assert!(commands.try_recv().is_err());
        assert_eq!(app.mcp.elicitations.active().unwrap().focus,0);
        assert!(app.mcp.elicitations.active().unwrap().error.is_some());
        crate::app::config::handle_paste(&mut app,"valid");
        key(&mut app,KeyCode::Enter,KeyModifiers::CONTROL);
        key(&mut app,KeyCode::Enter,KeyModifiers::CONTROL);
        key(&mut app,KeyCode::Char('x'),KeyModifiers::NONE);
        let command = commands.recv_envelope().await.unwrap().command;
        assert!(matches!(command, BridgeCommand::ElicitationResponse { action:types::ElicitationAction::Accept,content:Some(ref value),.. } if *value == json!({"name":"valid","flag":false})));
        let queued = app.event_rx.recv().await.unwrap(); event(&mut app,queued);
        assert!(commands.try_recv().is_err());
        assert!(app.mcp.elicitations.active().unwrap().sending);
        event(&mut app,ClientEvent::McpElicitationResponseFailed {session_id:"session-1".into(),request_id:"a".into()});
        let entry = app.mcp.elicitations.active().unwrap();
        assert!(!entry.sending);
        assert_eq!(entry.focus,0);
        assert_eq!(entry.form.as_ref().ok().unwrap().content().unwrap()["name"],json!("valid"));
        key(&mut app,KeyCode::Enter,KeyModifiers::CONTROL);
        assert!(matches!(commands.recv_envelope().await.unwrap().command,BridgeCommand::ElicitationResponse {content:Some(ref value),..} if value["name"] == "valid"));
        let queued = app.event_rx.recv().await.unwrap(); event(&mut app,queued);
        settled(&mut app,"a");
        assert!(app.mcp.elicitations.is_empty());
    }).await;
}

#[test]
fn queued_url_requests_never_open_before_activation_and_aborted_queued_urls_never_open() {
    let mut app = app();
    present_mcp_elicitation_request(&mut app, request("a"));
    let mut url = request("url");
    url.mode = types::ElicitationMode::Url;
    url.url = None;
    present_mcp_elicitation_request(&mut app, url);
    let queued = app.mcp.elicitations.get_mut("url").unwrap();
    assert!(!queued.browser_opened);
    assert!(queued.browser_open_error.is_none(), "no activation attempted for queued URL");
    settled(&mut app, "a");
    assert!(app.mcp.elicitations.active().unwrap().browser_open_error.is_some());
    let mut url = request("abort-url");
    url.mode = types::ElicitationMode::Url;
    url.url = None;
    present_mcp_elicitation_request(&mut app, url);
    settled(&mut app, "abort-url");
    assert_eq!(app.mcp.elicitations.active().unwrap().request.request_id, "url");
}

#[tokio::test(flavor = "current_thread")]
async fn failed_transport_retains_draft_and_can_cancel_without_a_form_answer() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut app = app();
            let (conn, commands) = AgentConnection::test_channel();
            drop(commands);
            app.session_runtime.conn = Some(std::rc::Rc::new(conn));
            present_mcp_elicitation_request(&mut app, request("a"));
            crate::app::config::handle_paste(&mut app, "private-answer");
            key(&mut app, KeyCode::Enter, KeyModifiers::CONTROL);
            let failure = app.event_rx.recv().await.unwrap();
            assert!(matches!(failure, ClientEvent::McpElicitationResponseFailed { .. }));
            event(&mut app, failure);
            assert!(!app.mcp.elicitations.active().unwrap().sending);
            let (conn, mut commands) = AgentConnection::test_channel();
            app.session_runtime.conn = Some(std::rc::Rc::new(conn));
            key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
            assert!(matches!(
                commands.recv_envelope().await.unwrap().command,
                BridgeCommand::ElicitationResponse {
                    action: types::ElicitationAction::Cancel,
                    content: None,
                    ..
                }
            ));
            let queued = app.event_rx.recv().await.unwrap();
            event(&mut app, queued);
            assert_eq!(app.mcp.elicitations.len(), 1);
            settled(&mut app, "a");
            assert!(app.config.overlay.is_none());
        })
        .await;
}

#[test]
fn shift_tab_returns_to_the_previous_form_field_for_both_terminal_encodings() {
    let mut app = app();
    present_mcp_elicitation_request(&mut app, request("keys"));
    key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    key(&mut app, KeyCode::Tab, KeyModifiers::SHIFT);
    assert_eq!(app.mcp.elicitations.active().unwrap().focus, 0);
    key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    key(&mut app, KeyCode::BackTab, KeyModifiers::SHIFT);
    assert_eq!(app.mcp.elicitations.active().unwrap().focus, 0);
}

#[test]
fn session_teardown_drops_all_forms_without_activating_the_queued_url() {
    let mut app = app();
    present_mcp_elicitation_request(&mut app, request("a"));
    let mut url = request("b");
    url.mode = types::ElicitationMode::Url;
    present_mcp_elicitation_request(&mut app, url);
    event(
        &mut app,
        ClientEvent::McpElicitationSettled {
            session_id: "session-1".into(),
            request_id: "a".into(),
            session_closing: true,
        },
    );
    assert!(app.mcp.elicitations.is_empty());
    assert!(app.config.overlay.is_none());
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
