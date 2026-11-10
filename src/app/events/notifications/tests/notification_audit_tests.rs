// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::agent::wire::BridgeCommand;
use crate::app::{MessageBlock, SystemSeverity};

fn notices(app: &App) -> Vec<(String, SystemSeverity)> {
    app.transcript
        .messages
        .iter()
        .flat_map(|message| &message.blocks)
        .filter_map(|block| match block {
            MessageBlock::Notice(notice) => Some((notice.text.text.clone(), notice.severity)),
            _ => None,
        })
        .collect()
}
fn named_notice(id: &str, text: &str, priority: &str) -> Value {
    let mut value = notice(id);
    value["text"] = json!(text);
    value["priority"] = json!(priority);
    value
}
fn assert_delivery(body: &str) {
    let sent = deliveries();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].body, body);
    assert!(sent[0].ring_bell && !sent[0].send_desktop && sent[0].escape_sequence.is_none());
}

#[test]
fn replay_rebuilds_distinct_notice_content_and_severity_without_reviving_tool_alerts() {
    let mut app = app();
    app.notifications.on_focus_lost();
    let historical = named_notice("history", "HISTORICAL_INFO", "normal");
    let history = vec![
        model::SessionUpdate::NotificationUpdate {
            notification: serde_json::from_value(historical.clone()).unwrap(),
            replay: false,
        },
        model::SessionUpdate::ToolCall(
            model::ToolCall::new("terminal-tool", "Old notice")
                .status(model::ToolCallStatus::Completed),
        ),
        model::SessionUpdate::ToolCall(
            model::ToolCall::new("updated-tool", "Updated notice")
                .status(model::ToolCallStatus::InProgress),
        ),
        model::SessionUpdate::ToolCallUpdate(model::ToolCallUpdate::new(
            "updated-tool",
            model::ToolCallUpdateFields::new().status(model::ToolCallStatus::Completed),
        )),
    ];
    crate::app::events::session_reset::load_resume_history(&mut app, &history);
    assert_eq!(notices(&app), [("HISTORICAL_INFO".into(), SystemSeverity::Info)]);
    for id in ["terminal-tool", "updated-tool"] {
        sdk(&mut app, &proactive(id), false);
    }
    sdk(&mut app, &proactive("explicit-replay"), true);
    sdk(&mut app, &proactive("explicit-replay"), false);
    sdk(&mut app, &historical, false);
    for (id, text, priority) in
        [("live-high", "LIVE_HIGH", "high"), ("live-immediate", "LIVE_IMMEDIATE", "immediate")]
    {
        let payload = named_notice(id, text, priority);
        sdk(&mut app, &payload, false);
        sdk(&mut app, &payload, false);
    }
    assert_eq!(
        notices(&app),
        [
            ("HISTORICAL_INFO".into(), SystemSeverity::Info),
            ("LIVE_HIGH".into(), SystemSeverity::Warning),
            ("LIVE_IMMEDIATE".into(), SystemSeverity::Warning)
        ]
    );
    assert!(deliveries().is_empty());
    crate::app::events::session_reset::load_resume_history(&mut app, &history);
    assert_eq!(notices(&app), [("HISTORICAL_INFO".into(), SystemSeverity::Info)]);
    assert!(deliveries().is_empty());
    sdk(&mut app, &named_notice("later", "LATER_INFO", "low"), false);
    assert_eq!(
        notices(&app),
        [
            ("HISTORICAL_INFO".into(), SystemSeverity::Info),
            ("LATER_INFO".into(), SystemSeverity::Info)
        ]
    );
    let before = notices(&app);
    let mut payload = proactive("fresh-tool");
    payload["text"] = json!("FRESH_TOOL_BODY");
    sdk(&mut app, &payload, false);
    assert_delivery("FRESH_TOOL_BODY");
    assert_eq!(notices(&app), before);
}

enum Response {
    Permission(tokio::sync::oneshot::Receiver<model::RequestPermissionResponse>),
    Question(tokio::sync::oneshot::Receiver<model::RequestQuestionResponse>),
}
impl Response {
    fn delivered(&mut self) -> bool {
        match self {
            Self::Permission(rx) => rx.try_recv().is_ok(),
            Self::Question(rx) => rx.try_recv().is_ok(),
        }
    }
}
fn request(app: &mut App, id: &str, question: bool, session: &str) -> Response {
    if question {
        let (tx, rx) = tokio::sync::oneshot::channel();
        crate::app::handle_client_event(
            app,
            ClientEvent::QuestionRequest {
                session_id: session.into(),
                request: model::RequestQuestionRequest::new(
                    session,
                    model::ToolCallUpdate::new(id, model::ToolCallUpdateFields::new()),
                    model::QuestionPrompt::new(
                        "PRIVATE_REQUEST_BODY",
                        "Choice",
                        false,
                        vec![model::QuestionOption::new("yes", "Yes")],
                    ),
                    0,
                    1,
                ),
                response_tx: tx,
            },
        );
        Response::Question(rx)
    } else {
        let (tx, rx) = tokio::sync::oneshot::channel();
        crate::app::handle_client_event(
            app,
            ClientEvent::PermissionRequest {
                session_id: session.into(),
                request: model::RequestPermissionRequest::new(
                    session,
                    model::ToolCallUpdate::new(id, model::ToolCallUpdateFields::new()),
                    vec![model::PermissionOption::new(
                        "allow",
                        "Allow",
                        model::PermissionOptionKind::AllowOnce,
                    )],
                    None,
                ),
                response_tx: tx,
            },
        );
        Response::Permission(rx)
    }
}

#[test]
fn accepted_interactions_remain_usable_with_independent_notification_preferences() {
    for question in [false, true] {
        for enabled in [None, Some(false), Some(true)] {
            let logs = notification_logs(|| {
                let mut app = app();
                app.notifications.on_focus_lost();
                if let Some(value) = enabled {
                    set(&mut app, "notifications.actionsRequired", json!(value));
                    set(&mut app, "notifications.turnComplete", json!(false));
                } else {
                    app.config.snapshot = None;
                }
                let _unknown = request(&mut app, "unknown", question, "session-1");
                let _stale = request(&mut app, "stale", question, "old-session");
                assert!(deliveries().is_empty());
                for id in ["first-action", "later-action"] {
                    update(
                        &mut app,
                        model::SessionUpdate::ToolCall(model::ToolCall::new(
                            id,
                            if question { "AskUserQuestion" } else { "Read" },
                        )),
                    );
                    let mut response = request(&mut app, id, question, "session-1");
                    let _duplicate = request(&mut app, id, question, "session-1");
                    if enabled == Some(true) {
                        assert_delivery(if question {
                            "Question required -- waiting for your input"
                        } else {
                            "Permission required -- waiting for your approval"
                        });
                    } else {
                        assert!(deliveries().is_empty());
                    }
                    crate::app::test_support::press(&mut app, crossterm::event::KeyCode::Enter);
                    assert!(
                        response.delivered(),
                        "suppressed notifications must not disable the accepted interaction"
                    );
                    assert!(deliveries().is_empty());
                }
            });
            for id in ["first-action", "later-action"] {
                let record = logs
                    .iter()
                    .find(|r| {
                        r["event_name"] == "notification_delivery_decided"
                            && r["span"]["interaction_id"] == id
                    })
                    .unwrap();
                assert_eq!(record["span"]["session_id"], "session-1");
                assert_eq!(
                    record["category"],
                    if question { "QuestionRequired" } else { "PermissionRequired" }
                );
                assert_eq!(
                    record["outcome"],
                    if enabled == Some(true) { "dispatch" } else { "suppressed" }
                );
            }
            assert!(!serde_json::to_string(&logs).unwrap().contains("PRIVATE_REQUEST_BODY"));
        }
    }
}

fn attach(app: &mut App) -> crate::agent::client::CommandReceiver {
    let (connection, commands) = crate::agent::client::AgentConnection::test_channel();
    app.session_runtime.conn = Some(std::rc::Rc::new(connection));
    commands
}
fn admit(app: &mut App, commands: &mut crate::agent::client::CommandReceiver, text: &str) {
    app.input.set_text(text);
    crate::app::test_support::press(app, crossterm::event::KeyCode::Enter);
    assert!(
        std::iter::from_fn(|| commands.try_recv().ok())
            .any(|e| matches!(e.command, BridgeCommand::Prompt { .. }))
    );
    update(
        app,
        model::SessionUpdate::AgentMessageChunk(model::ContentChunk::new(
            model::ContentBlock::Text(model::TextContent::new(text)),
        )),
    );
}
fn complete(app: &mut App, reason: Option<crate::agent::types::TerminalReason>) {
    crate::app::handle_client_event(
        app,
        ClientEvent::TurnComplete {
            session_id: "session-1".into(),
            queued_turn_count: Some(0),
            terminal_reason: reason,
        },
    );
}

#[test]
fn completion_delivery_follows_admitted_turn_outcomes_and_recovers_after_suppression() {
    use crate::agent::types::TerminalReason as R;
    for outcome in ["success", "disabled", "cancel", "aborted-stream", "aborted-tools", "failure"] {
        let mut app = app();
        let mut commands = attach(&mut app);
        app.notifications.on_focus_lost();
        set(&mut app, "notifications.actionsRequired", json!(outcome == "disabled"));
        set(&mut app, "notifications.turnComplete", json!(outcome != "disabled"));
        admit(&mut app, &mut commands, "RECEIVED_ORIGINAL");
        if outcome == "cancel" {
            crate::app::input_submit::request_cancel(&mut app).unwrap();
            assert!(
                std::iter::from_fn(|| commands.try_recv().ok())
                    .any(|e| matches!(e.command, BridgeCommand::CancelTurn { .. }))
            );
        }
        if outcome == "failure" {
            crate::app::handle_client_event(
                &mut app,
                ClientEvent::TurnErrorClassified {
                    session_id: "session-1".into(),
                    message: "Usage limit".into(),
                    class: crate::agent::error_handling::TurnErrorClass::PlanLimit,
                    queued_turn_count: Some(0),
                    api_error_status: Some(429),
                    terminal_reason: Some(R::ApiError),
                },
            );
        } else {
            let reason = match outcome {
                "aborted-stream" => Some(R::AbortedStreaming),
                "aborted-tools" => Some(R::AbortedTools),
                _ => Some(R::Completed),
            };
            complete(&mut app, reason);
            complete(&mut app, reason);
        }
        if outcome == "success" {
            assert_delivery("Turn complete");
        } else {
            assert!(deliveries().is_empty());
        }
        set(&mut app, "notifications.turnComplete", json!(true));
        admit(&mut app, &mut commands, "RECOVERED_WORK");
        complete(&mut app, Some(R::Completed));
        assert_delivery("Turn complete");
        assert!(
            app.transcript
                .messages
                .iter()
                .flat_map(|m| &m.blocks)
                .any(|b| matches!(b,MessageBlock::Text(t) if t.text=="RECEIVED_ORIGINAL"))
        );
    }
}

#[tokio::test]
async fn notification_identity_survives_real_session_replacement_and_separates_origins() {
    let root = tempfile::tempdir().unwrap();
    let mut app = app();
    let _commands = attach(&mut app);
    app.settings_home_override = Some(root.path().to_path_buf());
    app.cwd_raw = root.path().to_string_lossy().into_owned();
    app.notifications.on_focus_lost();
    for (session, text) in [("session-1", "FIRST"), ("session-2", "SECOND")] {
        if session == "session-2" {
            let cwd = app.cwd_raw.clone();
            crate::app::handle_client_event(
                &mut app,
                ClientEvent::SessionReplaced {
                    session_id: session.into(),
                    cwd,
                    current_model: model::CurrentModel::new("fixture", "Fixture", "Fixture"),
                    available_models: vec![],
                    mode: None,
                    fast_mode_state: model::FastModeState::Off,
                    fast_mode_disabled_reason: None,
                    ultracode: None,
                    history_updates: vec![],
                    restored_input: None,
                },
            );
            set(&mut app, "preferredNotifChannel", json!("terminal_bell"));
        }
        let mut notice = named_notice("shared-id", text, "high");
        notice["session_id"] = json!(session);
        let mut tool = proactive("shared-id");
        tool["session_id"] = json!(session);
        tool["text"] = json!(format!("{text}_TOOL"));
        sdk_session(&mut app, session, &notice, false);
        sdk_session(&mut app, session, &tool, false);
        assert_delivery(&format!("{text}_TOOL"));
        assert!(notices(&app).contains(&(text.into(), SystemSeverity::Warning)));
        let before = notices(&app);
        sdk_session(&mut app, session, &notice, false);
        sdk_session(&mut app, session, &tool, false);
        if session == "session-2" {
            sdk_session(&mut app, "session-1", &proactive("late-old"), false);
            sdk_session(
                &mut app,
                session,
                &named_notice("late-payload", "OLD_PAYLOAD", "high"),
                false,
            );
        }
        assert_eq!(notices(&app), before);
        assert!(deliveries().is_empty());
    }
}
