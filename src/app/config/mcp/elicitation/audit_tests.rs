// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::agent::client::CommandReceiver;
use crate::agent::types::ElicitationAction::{self, Accept, Cancel, Decline};
use std::{cell::RefCell, rc::Rc, time::Duration};

fn deliver(app: &mut App, request: types::ElicitationRequest) {
    let session_id = app.session_runtime.session_id.as_ref().unwrap().to_string();
    event(app, ClientEvent::McpElicitationRequest { session_id, request });
}

fn press(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
    crate::app::handle_terminal_event(
        app,
        crossterm::event::Event::Key(KeyEvent::new(code, modifiers)),
    );
}

fn paste(app: &mut App, text: &str) {
    crate::app::handle_terminal_event(app, crossterm::event::Event::Paste(text.into()));
}

fn content(app: &App) -> serde_json::Value {
    serde_json::Value::Object(
        app.mcp.elicitations.active().unwrap().form.as_ref().ok().unwrap().content().unwrap(),
    )
}

fn focus_name(app: &App) -> &str {
    let entry = app.mcp.elicitations.active().unwrap();
    &entry.form.as_ref().ok().unwrap().fields[entry.focus].name
}

fn connect(app: &mut App) -> CommandReceiver {
    let (conn, commands) = AgentConnection::test_channel();
    app.session_runtime.conn = Some(Rc::new(conn));
    commands
}

async fn response(
    app: &mut App,
    commands: &mut CommandReceiver,
    session: &str,
    request: &str,
    action: ElicitationAction,
    expected: Option<serde_json::Value>,
) {
    let command = tokio::time::timeout(Duration::from_secs(3), commands.recv_envelope())
        .await
        .expect("response deadline")
        .unwrap()
        .command;
    match command {
        BridgeCommand::ElicitationResponse {
            session_id,
            elicitation_request_id,
            action: actual,
            content,
        } => {
            assert_eq!(session_id, session);
            assert_eq!(elicitation_request_id, request);
            assert_eq!(actual, action);
            assert_eq!(content, expected);
        }
        other => panic!("expected response, got {other:?}"),
    }
    let queued = tokio::time::timeout(Duration::from_secs(3), app.event_rx.recv())
        .await
        .expect("admission deadline")
        .unwrap();
    assert!(
        matches!(&queued, ClientEvent::McpElicitationResponseQueued { session_id, request_id } if session_id == session && request_id == request)
    );
    event(app, queued);
    assert_eq!(app.mcp.elicitations.active().unwrap().request.request_id, request);
    assert!(app.mcp.elicitations.active().unwrap().sending);
    assert!(
        commands.try_recv().is_err(),
        "admission must neither duplicate the answer nor refresh the snapshot"
    );
}

fn two_texts(id: &str) -> types::ElicitationRequest {
    let mut req = request(id);
    req.field_order = vec!["leading".into(), "answer".into()];
    req.requested_schema = Some(json!({"type":"object","properties":{
        "leading":{"type":"string","default":"first"},
        "answer":{"type":"string","title":"Destination","minLength":2}
    },"required":["leading","answer"]}));
    req
}

#[test]
fn queued_requests_preserve_active_drafts_and_settled_ids_never_reopen() {
    let mut app = app();
    deliver(&mut app, request("active"));
    paste(&mut app, "kept-active-answer");
    press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    let mut removed = two_texts("removed-queued");
    removed.message = "Discarded queued request".into();
    deliver(&mut app, removed.clone());
    deliver(&mut app, request("active"));
    assert_eq!(focus_name(&app), "flag");
    assert_eq!(content(&app), json!({"name":"kept-active-answer","flag":false}));
    settled(&mut app, "removed-queued");
    deliver(&mut app, removed);
    assert!(app.mcp.elicitations.get_mut("removed-queued").is_none());
    let mut next = two_texts("next");
    next.requested_schema.as_mut().unwrap()["properties"]["answer"]["default"] =
        json!("next-default");
    deliver(&mut app, next);
    assert_eq!(focus_name(&app), "flag");
    assert_eq!(content(&app), json!({"name":"kept-active-answer","flag":false}));
    settled(&mut app, "active");
    assert_eq!(app.mcp.elicitations.active().unwrap().request.request_id, "next");
    assert_eq!(focus_name(&app), "leading");
    assert_eq!(content(&app), json!({"leading":"first","answer":"next-default"}));
    for old in ["active", "removed-queued"] {
        settled(&mut app, old);
        event(
            &mut app,
            ClientEvent::McpElicitationResponseQueued {
                session_id: "session-1".into(),
                request_id: old.into(),
            },
        );
        event(
            &mut app,
            ClientEvent::McpElicitationResponseFailed {
                session_id: "session-1".into(),
                request_id: old.into(),
            },
        );
        deliver(&mut app, request(old));
    }
    assert_eq!(app.mcp.elicitations.active().unwrap().request.request_id, "next");
    assert_eq!(content(&app), json!({"leading":"first","answer":"next-default"}));
    assert!(app.mcp.elicitations.active().unwrap().error.is_none());
    settled(&mut app, "next");
    assert!(app.mcp.elicitations.is_empty());
    assert!(app.config.overlay.is_none());
    deliver(&mut app, request("unrelated-overlay"));
    let other = ConfigOverlayState::McpDetails(crate::app::config::McpDetailsOverlayState {
        server_name: "other-server".into(),
        selected_index: 0,
    });
    app.config.replace_overlay(other.clone());
    settled(&mut app, "unrelated-overlay");
    assert_eq!(app.config.overlay, Some(other));
}

#[tokio::test(flavor = "current_thread")]
async fn session_replacement_discards_old_forms_and_filters_late_events() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let scratch = tempfile::tempdir().unwrap();
            let mut app = app();
            deliver(&mut app, request("same-id"));
            paste(&mut app, "current-private-draft");
            press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
            for stale in stale_session_events() {
                let active = app.mcp.elicitations.active_mut().unwrap();
                active.sending = true;
                active.error = Some("retained local error".into());
                event(&mut app, stale);
                let active = app.mcp.elicitations.active().unwrap();
                assert!(active.sending);
                assert_eq!(active.error.as_deref(), Some("retained local error"));
                assert_eq!(focus_name(&app), "flag");
                assert_eq!(content(&app), json!({"name":"current-private-draft","flag":false}));
                assert!(app.mcp.elicitations.get_mut("stale-only").is_none());
            }
            event(
                &mut app,
                ClientEvent::McpElicitationResponseFailed {
                    session_id: "session-1".into(),
                    request_id: "same-id".into(),
                },
            );
            assert!(!app.mcp.elicitations.active().unwrap().sending);
            assert!(
                app.mcp.elicitations.active().unwrap().error.as_ref().unwrap().contains("Retry")
            );
            event(
                &mut app,
                ClientEvent::SessionReplaced {
                    session_id: model::SessionId::new("replacement-session"),
                    cwd: scratch.path().to_string_lossy().into_owned(),
                    current_model: model::CurrentModel::new("model", "model", "model"),
                    available_models: vec![],
                    mode: None,
                    fast_mode_state: model::FastModeState::Off,
                    fast_mode_disabled_reason: None,
                    ultracode: None,
                    history_updates: vec![],
                    restored_input: None,
                },
            );
            assert!(app.mcp.elicitations.is_empty());
            assert!(app.config.overlay.is_none());
            for late in [
                ClientEvent::McpElicitationRequest {
                    session_id: "session-1".into(),
                    request: request("late"),
                },
                ClientEvent::McpElicitationSettled {
                    session_id: "session-1".into(),
                    request_id: "same-id".into(),
                    session_closing: true,
                },
                ClientEvent::McpElicitationResponseFailed {
                    session_id: "session-1".into(),
                    request_id: "same-id".into(),
                },
            ] {
                event(&mut app, late);
            }
            let mut commands = connect(&mut app);
            deliver(&mut app, request("same-id"));
            paste(&mut app, "replacement-answer");
            event(
                &mut app,
                ClientEvent::McpElicitationResponseFailed {
                    session_id: "session-1".into(),
                    request_id: "same-id".into(),
                },
            );
            event(
                &mut app,
                ClientEvent::McpElicitationSettled {
                    session_id: "session-1".into(),
                    request_id: "same-id".into(),
                    session_closing: true,
                },
            );
            assert!(app.mcp.elicitations.active().unwrap().error.is_none());
            assert_eq!(content(&app), json!({"name":"replacement-answer","flag":false}));
            press(&mut app, KeyCode::Enter, KeyModifiers::CONTROL);
            response(
                &mut app,
                &mut commands,
                "replacement-session",
                "same-id",
                Accept,
                Some(json!({"name":"replacement-answer","flag":false})),
            )
            .await;
            drop(app); // Stops the scratch-directory index/watch resources before cleanup.
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn validation_connection_retry_and_admission_preserve_the_exact_response() {
    tokio::task::LocalSet::new().run_until(async {
        let mut app = app();
        deliver(&mut app, two_texts("typed-request"));
        press(&mut app, KeyCode::Enter, KeyModifiers::CONTROL);
        assert_eq!(focus_name(&app), "answer");
        assert!(app.mcp.elicitations.active().unwrap().error.as_ref().unwrap().contains("Destination"));
        paste(&mut app, "private-valid");
        press(&mut app, KeyCode::Enter, KeyModifiers::CONTROL);
        assert!(!app.mcp.elicitations.active().unwrap().sending);
        assert!(app.mcp.elicitations.active().unwrap().error.as_ref().unwrap().contains("connection"));
        let mut commands = connect(&mut app);
        let expected = json!({"leading":"first","answer":"private-valid"});
            // Repeat the invalid-later-field path with a live transport; it must
            // retain the earlier answer and never start an admission task.
            for _ in 0..13 { press(&mut app, KeyCode::Backspace, KeyModifiers::NONE); }
            press(&mut app, KeyCode::Up, KeyModifiers::NONE);
            press(&mut app, KeyCode::Enter, KeyModifiers::CONTROL);
            assert_eq!(focus_name(&app), "answer");
            assert!(!app.mcp.elicitations.active().unwrap().sending);
            assert!(commands.try_recv().is_err());
            paste(&mut app, "private-valid");
        for attempt in 0..2 {
            press(&mut app, KeyCode::Enter, KeyModifiers::CONTROL);
            press(&mut app, KeyCode::Enter, KeyModifiers::CONTROL);
            press(&mut app, KeyCode::Char('x'), KeyModifiers::NONE);
            paste(&mut app, "must-not-edit");
            response(&mut app, &mut commands, "session-1", "typed-request", Accept, Some(expected.clone())).await;
            assert_eq!(content(&app), expected);
            if attempt == 0 {
                event(&mut app, ClientEvent::McpElicitationResponseFailed { session_id:"session-1".into(), request_id:"typed-request".into() });
                assert_eq!(focus_name(&app), "answer");
                assert_eq!(content(&app), expected);
                assert!(app.mcp.elicitations.active().unwrap().error.as_ref().unwrap().contains("Retry"));
            }
        }
        settled(&mut app, "typed-request");
        assert!(matches!(commands.try_recv().unwrap().command, BridgeCommand::GetMcpSnapshot { session_id } if session_id == "session-1"));
        assert!(app.config.overlay.is_none());
        settled(&mut app, "typed-request");
        assert!(commands.try_recv().is_err());
    }).await;
}

#[tokio::test(flavor = "current_thread")]
async fn unsupported_accept_is_refused_and_decline_waits_for_matching_sdk_settlement() {
    tokio::task::LocalSet::new().run_until(async {
        let mut app = app();
        let mut commands = connect(&mut app);
        let mut unsupported = request("declined-request");
        unsupported.requested_schema = Some(json!({"type":"object","properties":{"nested":{"type":"object"}}}));
        deliver(&mut app, unsupported.clone());
        press(&mut app, KeyCode::Enter, KeyModifiers::CONTROL);
        assert!(!app.mcp.elicitations.active().unwrap().sending);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        response(&mut app, &mut commands, "session-1", "declined-request", Decline, None).await;
        deliver(&mut app, request("queued"));
        for id in ["unknown", "queued", "queued"] { settled(&mut app, id); }
        assert_eq!(app.mcp.elicitations.active().unwrap().request.request_id, "declined-request");
        assert!(commands.try_recv().is_err());
        settled(&mut app, "declined-request");
        assert!(matches!(commands.try_recv().unwrap().command, BridgeCommand::GetMcpSnapshot { session_id } if session_id == "session-1"));
        settled(&mut app, "declined-request");
        assert!(commands.try_recv().is_err());
        unsupported.request_id = "cancelled-unsupported".into();
        deliver(&mut app, unsupported);
        press(&mut app, KeyCode::Down, KeyModifiers::NONE);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        response(&mut app, &mut commands, "session-1", "cancelled-unsupported", Cancel, None).await;
    }).await;
}

fn url_request(id: &str, url: Option<&str>) -> types::ElicitationRequest {
    let mut req = request(id);
    req.mode = types::ElicitationMode::Url;
    req.url = url.map(str::to_owned);
    req.elicitation_id = Some(format!("sdk-elicitation-{id}"));
    req
}

#[test]
fn browser_activation_is_exactly_once_and_teardown_never_promotes_urls() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let journal = calls.clone();
    let _browser = super::super::super::auth::browser_test_support::install(move |url| {
        journal.borrow_mut().push(url.to_owned());
        if url.ends_with("failure") {
            Err("Browser unavailable; open the URL manually.".into())
        } else {
            Ok(())
        }
    });
    let mut app = app();
    let mut commands = connect(&mut app);
    deliver(&mut app, request("form"));
    deliver(&mut app, url_request("discard", Some("https://forms.example/discard")));
    let survivor = url_request("survivor", Some("https://forms.example/survivor"));
    deliver(&mut app, survivor.clone());
    assert!(calls.borrow().is_empty());
    settled(&mut app, "discard");
    assert!(calls.borrow().is_empty());
    settled(&mut app, "form");
    assert_eq!(&*calls.borrow(), &["https://forms.example/survivor"]);
    assert!(app.mcp.elicitations.active().unwrap().browser_opened);
    deliver(&mut app, survivor);
    settled(&mut app, "form");
    assert_eq!(&*calls.borrow(), &["https://forms.example/survivor"]);
    while commands.try_recv().is_ok() {}
    settled(&mut app, "survivor");
    deliver(&mut app, url_request("missing", None));
    assert!(
        app.mcp.elicitations.active().unwrap().browser_open_error.as_ref().unwrap().contains("URL")
    );
    settled(&mut app, "missing");
    deliver(&mut app, url_request("failed", Some("https://forms.example/failure")));
    let entry = app.mcp.elicitations.active().unwrap();
    assert!(!entry.browser_opened);
    assert_eq!(entry.request.url.as_deref(), Some("https://forms.example/failure"));
    assert!(entry.browser_open_error.as_ref().unwrap().contains("manually"));
    assert_eq!(
        &*calls.borrow(),
        &["https://forms.example/survivor", "https://forms.example/failure"]
    );
    for close_queued in [false, true] {
        let mut app = super::app();
        let mut commands = connect(&mut app);
        deliver(&mut app, request("closing-form"));
        deliver(&mut app, url_request("closing-url", Some("https://forms.example/never")));
        event(
            &mut app,
            ClientEvent::McpElicitationSettled {
                session_id: "session-1".into(),
                request_id: if close_queued { "closing-url" } else { "closing-form" }.into(),
                session_closing: true,
            },
        );
        assert!(app.mcp.elicitations.is_empty());
        assert!(app.config.overlay.is_none());
        assert!(commands.try_recv().is_err());
    }
    assert_eq!(
        &*calls.borrow(),
        &["https://forms.example/survivor", "https://forms.example/failure"]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn closed_transport_retains_private_drafts_and_reconnected_cancel_sends_no_content() {
    tokio::task::LocalSet::new().run_until(Box::pin(async {
        let mut app = app();
        drop(connect(&mut app));
        deliver(&mut app, request("private"));
        paste(&mut app, "private-answer-739");
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        let mut queued = two_texts("unrelated-queued");
        queued.requested_schema.as_mut().unwrap()["properties"]["answer"]["default"] = json!("queued-private-628");
        deliver(&mut app, queued);
        press(&mut app, KeyCode::Enter, KeyModifiers::CONTROL);
        let failure = tokio::time::timeout(Duration::from_secs(3), app.event_rx.recv()).await.unwrap().unwrap();
        assert!(matches!(&failure, ClientEvent::McpElicitationResponseFailed { session_id, request_id } if session_id == "session-1" && request_id == "private"));
        event(&mut app, failure);
        assert!(!app.mcp.elicitations.active().unwrap().sending);
        assert_eq!(focus_name(&app), "flag");
        assert_eq!(content(&app), json!({"name":"private-answer-739","flag":false}));
        assert!(app.mcp.elicitations.active().unwrap().error.as_ref().unwrap().contains("Retry"));
        press(&mut app, KeyCode::Char(' '), KeyModifiers::NONE);
        assert_eq!(content(&app), json!({"name":"private-answer-739","flag":true}));
        let mut commands = connect(&mut app);
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        response(&mut app, &mut commands, "session-1", "private", Cancel, None).await;
        settled(&mut app, "private");
        assert_eq!(app.mcp.elicitations.active().unwrap().request.request_id, "unrelated-queued");
        assert_eq!(content(&app), json!({"leading":"first","answer":"queued-private-628"}));
    })).await;
}

#[tokio::test(flavor = "current_thread")]
async fn both_shift_tab_encodings_edit_only_the_previous_field() {
    tokio::task::LocalSet::new()
        .run_until(async {
            for code in [KeyCode::Tab, KeyCode::BackTab] {
                let mut app = app();
                let mut commands = connect(&mut app);
                deliver(&mut app, two_texts("backwards"));
                press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
                assert_eq!(focus_name(&app), "answer");
                paste(&mut app, "second-unchanged");
                press(&mut app, code, KeyModifiers::SHIFT);
                assert_eq!(focus_name(&app), "leading");
                press(&mut app, KeyCode::End, KeyModifiers::NONE);
                paste(&mut app, "-edited");
                press(&mut app, KeyCode::Enter, KeyModifiers::CONTROL);
                response(
                    &mut app,
                    &mut commands,
                    "session-1",
                    "backwards",
                    Accept,
                    Some(json!({"leading":"first-edited","answer":"second-unchanged"})),
                )
                .await;
            }
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn optional_answers_omit_restore_and_reinclude_without_losing_the_draft() {
    tokio::task::LocalSet::new().run_until(async {
        for scenario in ["omit", "restore", "invalid-omitted", "invalid-restored", "edit", "paste"] {
            let mut app = app();
            let mut commands = connect(&mut app);
            let mut req = request(scenario);
            req.field_order = vec!["flag".into(),"zero".into(),"note".into()];
            req.requested_schema = Some(json!({"type":"object","properties":{
                "flag":{"type":"boolean"},"zero":{"type":"integer","default":0},"note":{"type":"string","title":"Optional note","minLength":2}
            },"required":["flag","zero"]}));
            deliver(&mut app, req);
            press(&mut app, KeyCode::Char('o'), KeyModifiers::CONTROL);
            press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
            press(&mut app, KeyCode::Char('o'), KeyModifiers::CONTROL);
            press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
            paste(&mut app, if scenario.starts_with("invalid") {"x"} else {"saved-note"});
            press(&mut app, KeyCode::Char('o'), KeyModifiers::CONTROL);
            let mut expected = json!({"flag":false,"zero":0});
            match scenario {
                "restore" => {
                    press(&mut app, KeyCode::Char('o'), KeyModifiers::CONTROL);
                    expected["note"] = json!("saved-note");
                }
                "invalid-restored" => {
                    press(&mut app, KeyCode::Char('o'), KeyModifiers::CONTROL);
                    press(&mut app, KeyCode::Enter, KeyModifiers::CONTROL);
                    assert!(!app.mcp.elicitations.active().unwrap().sending);
                    assert_eq!(focus_name(&app), "note");
                    assert!(app.mcp.elicitations.active().unwrap().error.as_ref().unwrap().contains("Optional note"));
                    paste(&mut app, "y");
                    expected["note"] = json!("xy");
                }
                "edit" => {
                    press(&mut app, KeyCode::Char('!'), KeyModifiers::NONE);
                    expected["note"] = json!("saved-note!");
                }
                "paste" => {
                    paste(&mut app, "-pasted");
                    expected["note"] = json!("saved-note-pasted");
                }
                _ => {}
            }
            press(&mut app, KeyCode::Enter, KeyModifiers::CONTROL);
            response(&mut app, &mut commands, "session-1", scenario, Accept, Some(expected)).await;
        }
    }).await;
}

#[tokio::test(flavor = "current_thread")]
async fn url_actions_target_the_host_request_and_never_include_form_content() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let calls = Rc::new(RefCell::new(Vec::new()));
            let journal = calls.clone();
            let _browser = super::super::super::auth::browser_test_support::install(move |url| {
                journal.borrow_mut().push(url.to_owned());
                Err("Browser unavailable; open manually".into())
            });
            for (id, action, down) in
                [("url-accept", Accept, 0), ("url-decline", Decline, 1), ("url-cancel", Cancel, 2)]
            {
                let mut app = app();
                let mut commands = connect(&mut app);
                deliver(&mut app, url_request(id, Some("https://forms.example/manual")));
                let entry = app.mcp.elicitations.active().unwrap();
                assert_eq!(entry.field_count(), 0);
                assert_eq!(entry.request.url.as_deref(), Some("https://forms.example/manual"));
                assert!(entry.browser_open_error.as_ref().unwrap().contains("manually"));
                for _ in 0..down {
                    press(&mut app, KeyCode::Down, KeyModifiers::NONE);
                }
                press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
                response(&mut app, &mut commands, "session-1", id, action, None).await;
                settled(&mut app, "wrong-target");
                assert_eq!(app.mcp.elicitations.active().unwrap().request.request_id, id);
                settled(&mut app, id);
                assert!(app.config.overlay.is_none());
            }
            assert_eq!(
                &*calls.borrow(),
                &[
                    "https://forms.example/manual",
                    "https://forms.example/manual",
                    "https://forms.example/manual"
                ]
            );
        })
        .await;
}

fn stale_session_events() -> [ClientEvent; 5] {
    [
        ClientEvent::McpElicitationRequest {
            session_id: "old-session".into(),
            request: request("same-id"),
        },
        ClientEvent::McpElicitationRequest {
            session_id: "old-session".into(),
            request: request("stale-only"),
        },
        ClientEvent::McpElicitationResponseQueued {
            session_id: "old-session".into(),
            request_id: "same-id".into(),
        },
        ClientEvent::McpElicitationResponseFailed {
            session_id: "old-session".into(),
            request_id: "same-id".into(),
        },
        ClientEvent::McpElicitationSettled {
            session_id: "old-session".into(),
            request_id: "same-id".into(),
            session_closing: true,
        },
    ]
}
