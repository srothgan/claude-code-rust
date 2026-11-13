// SPDX-License-Identifier: Apache-2.0
use crate::agent::model;
use std::rc::Rc;

#[tokio::test]
async fn subagent_stop_dispatch_preserves_request_and_run_identity() {
    let (connection, _commands) = crate::agent::client::AgentConnection::test_channel();
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let mut connected = true;
    super::event_dispatch::handle_bridge_event(
        &tx,
        &connection,
        &mut connected,
        false,
        crate::agent::wire::EventEnvelope {
            request_id: Some("stop-A".to_owned()),
            event: crate::agent::wire::BridgeEvent::SubagentStopResult {
                session_id: "session".to_owned(),
                task_id: "worker".to_owned(),
                run_id: Some("A".to_owned()),
                error: None,
            },
        },
    )
    .await;
    let crate::agent::events::ClientEvent::SubagentStopResult {
        session_id,
        task_id,
        run_id,
        request_id,
        error,
    } = rx.try_recv().expect("stop response")
    else {
        panic!("expected stop result")
    };
    assert_eq!(
        (session_id.as_str(), task_id.as_str(), run_id.as_deref(), request_id.as_deref()),
        ("session", "worker", Some("A"), Some("stop-A"))
    );
    assert!(error.is_none());
}

#[tokio::test]
async fn turn_error_dispatch_preserves_classification_authority_and_fallback() {
    use crate::agent::error_handling::TurnErrorClass;

    for (payload, session, cause, class, guidance, retry_allowed) in [
        (
            r#"{"event":"turn_error","session_id":"account-session","message":"Please log in to inspect this resource","error_kind":"account_access"}"#,
            "account-session",
            "Please log in to inspect this resource",
            Some(TurnErrorClass::AccountAccess),
            "current account or organization",
            false,
        ),
        (
            r#"{"event":"turn_error","session_id":"explicit-other-session","message":"Quota exhausted for this request","error_kind":"other"}"#,
            "explicit-other-session",
            "Quota exhausted for this request",
            Some(TurnErrorClass::Other),
            "Turn failed",
            false,
        ),
        (
            r#"{"event":"turn_error","session_id":"unknown-tag-session","message":"Quota exhausted until the next allowance window","error_kind":"future_error_kind"}"#,
            "unknown-tag-session",
            "Quota exhausted until the next allowance window",
            None,
            "retry here",
            true,
        ),
        (
            r#"{"event":"turn_error","session_id":"absent-tag-session","message":"Rate limit reached for the current allowance"}"#,
            "absent-tag-session",
            "Rate limit reached for the current allowance",
            None,
            "retry here",
            true,
        ),
    ] {
        assert_dispatched_turn_error(payload, session, cause, class, guidance, retry_allowed).await;
    }
}

async fn assert_dispatched_turn_error(
    payload: &str,
    session: &str,
    cause: &str,
    class: Option<crate::agent::error_handling::TurnErrorClass>,
    guidance: &str,
    retry_allowed: bool,
) {
    use crate::agent::events::ClientEvent;
    use crate::app::{
        App, AppStatus, ChatMessage, MessageBlock, MessageRole, SystemSeverity, TextBlock,
    };

    let (connection, mut commands) = crate::agent::client::AgentConnection::test_channel();
    let mut app = App::test_default();
    app.session_runtime.conn = Some(Rc::new(connection));
    app.session_runtime.session_id = Some(model::SessionId::new(session));
    app.status = AppStatus::Running;
    app.input.set_text("next draft");
    for text in ["unrelated completed answer", "active partial answer"] {
        app.push_message_tracked(ChatMessage::new(
            MessageRole::Assistant,
            vec![MessageBlock::Text(TextBlock::from_complete(text))],
            None,
        ));
    }
    app.bind_active_turn_assistant(1);
    let prior_id = app.transcript.messages[0].id;
    let active_id = app.transcript.messages[1].id;
    let mut connected = true;
    let envelope = serde_json::from_str(payload).expect("literal turn-error envelope");
    super::event_dispatch::handle_bridge_event(
        &app.event_tx,
        app.session_runtime.conn.as_ref().expect("live connection"),
        &mut connected,
        false,
        envelope,
    )
    .await;
    let event = app.event_rx.try_recv().expect("dispatched turn error");
    let (actual_session, actual_cause, actual_class) = match &event {
        ClientEvent::TurnErrorClassified { session_id, message, class, .. } => {
            (session_id, message, Some(*class))
        }
        ClientEvent::TurnError { session_id, message, .. } => (session_id, message, None),
        _ => panic!("expected dispatched turn error"),
    };
    assert_eq!(actual_session, session);
    assert_eq!(actual_cause, cause);
    assert_eq!(actual_class, class);

    crate::app::events::handle_client_event(&mut app, event);

    assert_eq!(
        app.session_runtime.session_id.as_ref().map(model::SessionId::as_str),
        Some(session)
    );
    assert_eq!(app.status, if retry_allowed { AppStatus::Ready } else { AppStatus::Error });
    assert!(!app.shutdown_requested(), "{session}: must not request authentication shutdown");
    assert_eq!(app.exit_error, None);
    assert_eq!(app.active_turn_assistant_idx(), None);
    let prior = app.transcript.messages.iter().find(|message| message.id == prior_id).unwrap();
    let [MessageBlock::Text(prior_text)] = prior.blocks.as_slice() else {
        panic!("unrelated answer must not acquire this turn's error");
    };
    assert_eq!(prior_text.text, "unrelated completed answer");
    let active = app.transcript.messages.iter().find(|message| message.id == active_id).unwrap();
    let [MessageBlock::Text(active_text), MessageBlock::Notice(notice)] = active.blocks.as_slice()
    else {
        panic!("error notice must belong to the seeded active answer");
    };
    assert_eq!(active_text.text, "active partial answer");
    assert_eq!(notice.severity, SystemSeverity::Error);
    assert!(notice.text.text.contains(cause), "{}", notice.text.text);
    assert!(notice.text.text.contains(guidance), "{}", notice.text.text);
    assert!(!notice.text.text.contains("unrelated completed answer"));
    assert_eq!(notice.text.text.contains("Input disabled"), !retry_allowed);
    assert_turn_error_input_admission(&mut app, &mut commands, session, retry_allowed);
}

fn assert_turn_error_input_admission(
    app: &mut crate::app::App,
    commands: &mut crate::agent::client::CommandReceiver,
    session: &str,
    retry_allowed: bool,
) {
    use crate::agent::wire::BridgeCommand;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

    while let Ok(envelope) = commands.try_recv() {
        assert!(!matches!(envelope.command, BridgeCommand::Prompt { .. }), "no automatic retry");
    }
    assert_eq!(app.input.text(), "next draft");
    for code in [KeyCode::End, KeyCode::Char('!'), KeyCode::Enter] {
        crate::app::events::handle_terminal_event(
            app,
            Event::Key(KeyEvent::new(code, KeyModifiers::NONE)),
        );
    }
    crate::app::finalize_deferred_submit(app);
    let mut prompts = Vec::new();
    while let Ok(envelope) = commands.try_recv() {
        if let BridgeCommand::Prompt { session_id, chunks, .. } = envelope.command {
            prompts.push((session_id, chunks));
        }
    }
    if retry_allowed {
        let [(actual_session, chunks)] = prompts.as_slice() else {
            panic!("one manual retry must reach the bridge");
        };
        assert_eq!(actual_session, session);
        let text = chunks.iter().find(|chunk| chunk.kind == "text").expect("retry text chunk");
        assert_eq!(text.value, serde_json::json!("next draft!"));
        assert!(app.input.is_empty());
    } else {
        assert!(prompts.is_empty(), "locked errors must not admit a prompt");
        assert_eq!(app.input.text(), "next draft");
    }
}
