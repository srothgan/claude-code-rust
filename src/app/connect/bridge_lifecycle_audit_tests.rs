// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::agent::client::BridgeShutdownOutcome;
use crate::agent::wire::SessionLaunchSettings;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::fs;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

struct Fixture(tempfile::TempDir);

#[allow(clippy::unwrap_used, clippy::expect_used)]
impl Fixture {
    fn new(mode: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        fs::write(
            root.path().join("worker.cjs"),
            include_str!("../../../tests/fixtures/bridge-lifecycle-audit.cjs"),
        )
        .unwrap();
        fs::write(root.path().join("options.json"), json!({"mode":mode}).to_string()).unwrap();
        Self(root)
    }

    fn spawn(&self) -> BridgeClient {
        BridgeClient::spawn(&BridgeLauncher {
            runtime_path: "node".into(),
            script_path: self.0.path().join("worker.cjs"),
        })
        .unwrap()
    }

    async fn mark(&self, name: &str) -> Value {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Ok(bytes) = fs::read(self.0.path().join(format!("{name}.json"))) {
                    return serde_json::from_slice(&bytes).unwrap();
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("child checkpoint deadline")
    }

    fn commands(&self) -> Vec<Value> {
        let text = fs::read_to_string(self.0.path().join("commands.ndjson")).unwrap();
        assert!(text.ends_with('\n'), "checkpoint must follow complete protocol records");
        text.lines().map(|line| serde_json::from_str(line).unwrap()).collect()
    }

    fn params(&self, event_tx: mpsc::Sender<ClientEvent>) -> StartConnectionParams {
        StartConnectionParams {
            event_tx,
            cwd_raw: self.0.path().to_str().unwrap().into(),
            bridge_script: None,
            launch: crate::StartupLaunch::NewSession,
            session_launch_settings: SessionLaunchSettings::default(),
        }
    }
}

fn poll_once<F: Future>(future: Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(futures::task::noop_waker_ref()))
}

async fn poll_until_received<F: Future>(mut future: Pin<&mut F>, event: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let records = crate::logging::test_capture::capture(|| {
            assert!(poll_once(future.as_mut()).is_pending());
        });
        if records.iter().any(|record| {
            record["event_name"] == "bridge_event_received" && record["bridge_event"] == event
        }) {
            return;
        }
        assert!(std::time::Instant::now() < deadline, "event must reach the actual dispatcher");
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn initialization_budget_bounds_silence_and_does_not_restart_after_forwarded_events() {
    for forward in [false, true] {
        let fixture = Fixture::new("deadline");
        let mut bridge = fixture.spawn();
        fixture.mark("ready").await;
        let connection = bridge.connection();
        let (tx, mut rx) = mpsc::channel(1);
        let mut connected = false;
        tokio::time::pause();
        {
            let wait = wait_for_bridge_initialized_with_timeout(
                &mut bridge,
                &tx,
                &connection,
                &mut connected,
                false,
                Duration::from_secs(10),
            );
            tokio::pin!(wait);
            assert!(poll_once(wait.as_mut()).is_pending());
            tokio::time::advance(Duration::from_secs(9)).await;
            if forward {
                connection.cancel("emit-auth".into()).unwrap();
                poll_until_received(wait.as_mut(), "auth_required").await;
                assert!(
                    matches!(rx.try_recv(), Ok(ClientEvent::AuthRequired { method_name, method_description }) if method_name == "late-auth" && method_description == "late guidance")
                );
            }
            assert!(poll_once(wait.as_mut()).is_pending());
            tokio::time::advance(Duration::from_millis(1001)).await;
            assert!(matches!(
                poll_once(wait.as_mut()),
                Poll::Ready(Err(InitializationFailure::Transport(AppError::BridgeTimeout)))
            ));
        }
        tokio::time::resume();
        assert!(rx.try_recv().is_err());
        bridge.shutdown_and_wait(Duration::from_secs(2)).await.unwrap();
    }
}

#[tokio::test]
async fn process_side_questions_wait_for_host_finalization_and_keep_the_main_reply_owner() {
    use crate::agent::model;
    use crate::app::{App, AppStatus, MessageBlock};
    let fixture = Fixture::new("side-questions");
    let mut bridge = fixture.spawn();
    let connection = bridge.connection();
    let mut app = App::test_default();
    app.session_runtime.conn = Some(Rc::new(connection.clone()));
    app.session_runtime.activate_session(model::SessionId::new("session-1"));
    app.status = AppStatus::Running;
    let stream = |app: &mut App, text: &str| {
        crate::app::handle_client_event(
            app,
            ClientEvent::SessionUpdate {
                session_id: "session-1".into(),
                update: model::SessionUpdate::AgentMessageChunk(model::ContentChunk::new(
                    model::ContentBlock::Text(model::TextContent::new(text)),
                )),
            },
        );
    };
    let before = "Before the process response.";
    let after = "After the process response.";
    stream(&mut app, before);
    let owner = app
        .transcript
        .messages
        .iter()
        .find(|m| m.blocks.iter().any(|b| matches!(b, MessageBlock::Text(t) if t.text == before)))
        .unwrap()
        .id;
    let questions = ["Why  two spaces?\nAnd Unicode: 日本語?", "Next  question\nwith another line"];
    for question in questions {
        app.input.set_text(&format!("/btw   {question}  "));
        crate::app::input_submit::submit_input(&mut app);
    }
    let (tx, mut rx) = mpsc::channel(4);
    let mut connected = true;
    for (index, expected_event) in ["btw_failed", "btw_result"].into_iter().enumerate() {
        let envelope = tokio::time::timeout(Duration::from_secs(10), bridge.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(envelope.event.event_name(), expected_event);
        // The child acknowledges a later command before the host applies this result.
        // FIFO transport proves a prematurely queued next question would already be journaled.
        let barrier = format!("before-finalization-{index}");
        connection.cancel(barrier.clone()).unwrap();
        fixture.mark(&barrier).await;
        let commands = fixture.commands();
        let sent = commands.iter().filter(|c| c["command"] == "side_question").collect::<Vec<_>>();
        assert_eq!(sent.len(), index + 1, "only host-finalized questions may advance the queue");
        for (i, command) in sent.iter().enumerate() {
            assert_eq!(command["session_id"], "session-1");
            assert_eq!(command["question"], questions[i]);
            assert!(!command["btw_id"].as_str().unwrap().is_empty());
        }
        if index == 1 {
            assert_ne!(sent[0]["btw_id"], sent[1]["btw_id"]);
        }
        handle_bridge_event(&tx, &connection, &mut connected, false, envelope).await;
        crate::app::handle_client_event(&mut app, rx.recv().await.unwrap());
        if index == 0 {
            assert!(
                app.transcript
                    .messages
                    .iter()
                    .all(|m| m.blocks.iter().all(|b| !matches!(b, MessageBlock::BtwExchange(_))))
            );
            assert!(app.btw.has_active());
        }
    }
    stream(&mut app, after);
    let message = app.transcript.messages.iter().find(|m| m.id == owner).unwrap();
    let before_position = message
        .blocks
        .iter()
        .position(|b| matches!(b, MessageBlock::Text(t) if t.text == before))
        .unwrap();
    let card_position = message.blocks.iter().position(|b| matches!(b, MessageBlock::BtwExchange(e) if e.question == questions[1] && e.answer == "answer from child")).unwrap();
    let after_position = message
        .blocks
        .iter()
        .position(|b| matches!(b, MessageBlock::Text(t) if t.text == after))
        .unwrap();
    assert!(before_position < card_position && card_position < after_position);
    assert!(!app.btw.has_active());
    assert!(app.pending_user_messages.is_empty());
    assert!(matches!(app.status, AppStatus::Running));
    bridge.shutdown_and_wait(Duration::from_secs(2)).await.unwrap();
    assert!(!fixture.commands().iter().any(|c| c["command"] == "prompt"));
}

#[tokio::test]
async fn initialization_reports_the_original_structured_failure_once() {
    let fixture = Fixture::new("failure");
    let mut bridge = fixture.spawn();
    let (tx, mut rx) = mpsc::channel(8);
    let params = fixture.params(tx);
    let slot = Rc::new(RefCell::new(None));
    tokio::time::timeout(
        Duration::from_secs(10),
        drive_bridge_connection(&params, &slot, &mut bridge),
    )
    .await
    .unwrap();
    assert!(slot.borrow().is_none());
    let outcome = bridge.shutdown_and_wait(Duration::from_secs(2)).await.unwrap();
    assert!(matches!(outcome, BridgeShutdownOutcome::Graceful(status) if status.success()));
    let ClientEvent::ConnectionFailed(failure) = rx.try_recv().unwrap() else {
        panic!("original failure")
    };
    assert_eq!(failure.message, "Safe SDK startup cause");
    assert_eq!(failure.request_id.as_deref(), Some("init-request-7"));
    let startup = failure.startup_failure.unwrap();
    assert_eq!(startup.reason, "fixture-policy");
    assert_eq!(startup.errors, ["Use the selected organization"]);
    assert!(matches!(rx.try_recv(), Ok(ClientEvent::FatalError(AppError::BridgeSdkFailure))));
    assert!(
        rx.try_recv().is_err(),
        "one failure and one fatal outcome, without generic follow-on errors"
    );
    assert_eq!(
        fixture.commands(),
        [json!({"command":"initialize","cwd":params.cwd_raw}), json!({"command":"shutdown"})]
    );
}

#[tokio::test]
async fn initialization_deadline_includes_backpressured_pre_ready_delivery() {
    let fixture = Fixture::new("pre-ready");
    let mut bridge = fixture.spawn();
    fixture.mark("ready").await;
    let connection = bridge.connection();
    let (tx, mut rx) = mpsc::channel(1);
    tx.send(ClientEvent::LogoutCompleted).await.unwrap();
    let mut connected = false;
    tokio::time::pause();
    {
        let wait = wait_for_bridge_initialized_with_timeout(
            &mut bridge,
            &tx,
            &connection,
            &mut connected,
            false,
            Duration::from_secs(10),
        );
        tokio::pin!(wait);
        poll_until_received(wait.as_mut(), "auth_required").await;
        tokio::time::advance(Duration::from_secs(9)).await;
        assert!(poll_once(wait.as_mut()).is_pending());
        // Tokio timers have millisecond resolution; cross the deadline by one tick.
        tokio::time::advance(Duration::from_millis(1001)).await;
        assert!(
            matches!(
                poll_once(wait.as_mut()),
                Poll::Ready(Err(InitializationFailure::Transport(AppError::BridgeTimeout)))
            ),
            "the production deadline must include a blocked event send"
        );
    }
    tokio::time::resume();
    assert!(matches!(rx.try_recv(), Ok(ClientEvent::LogoutCompleted)));
    assert!(rx.try_recv().is_err());
    bridge.shutdown_and_wait(Duration::from_secs(2)).await.unwrap();
}

#[tokio::test]
async fn shutdown_requires_the_shutdown_command_and_reaps_cooperative_or_unresponsive_children() {
    for mode in ["idle", "unresponsive"] {
        let fixture = Fixture::new(mode);
        let bridge = fixture.spawn();
        assert!(fixture.mark("ready").await["pid"].as_u64().unwrap() > 0);
        let connection = bridge.connection();
        connection.cancel("unrelated-command".into()).unwrap();
        assert_eq!(
            fixture.mark("unrelated-command").await,
            json!({"command":"cancel_turn","session_id":"unrelated-command"})
        );
        let outcome = tokio::time::timeout(
            Duration::from_secs(8),
            bridge.shutdown_and_wait(Duration::from_millis(150)),
        )
        .await
        .unwrap()
        .unwrap();
        match (mode, outcome) {
            ("idle", BridgeShutdownOutcome::Graceful(status)) => assert!(status.success()),
            ("unresponsive", BridgeShutdownOutcome::Forced(_)) => {}
            (_, outcome) => panic!("wrong shutdown outcome: {outcome:?}"),
        }
        assert_eq!(
            fixture.commands(),
            [
                json!({"command":"cancel_turn","session_id":"unrelated-command"}),
                json!({"command":"shutdown"})
            ]
        );
    }
}

#[tokio::test]
async fn shutdown_bounds_a_confirmed_unflushed_stdin_write_and_reaps_the_child() {
    let fixture = Fixture::new("blocked-writer");
    let bridge = fixture.spawn();
    fixture.mark("ready").await;
    bridge.connection().prompt_text("blocked-session".into(), "x".repeat(8 * 1024 * 1024)).unwrap();
    let prefix = fixture.mark("prefix").await;
    assert!(prefix["text"].as_str().unwrap().contains("\"command\":\"prompt\""));
    assert!(prefix["text"].as_str().unwrap().contains("blocked-session"));
    {
        // This real acknowledgement waits behind the partially delivered large command.
        let acknowledgement = bridge.send(CommandEnvelope {
            request_id: None,
            command: BridgeCommand::CancelTurn { session_id: "blocked-session".into() },
        });
        tokio::pin!(acknowledgement);
        assert!(poll_once(acknowledgement.as_mut()).is_pending());
    }
    let outcome = tokio::time::timeout(
        Duration::from_secs(8),
        bridge.shutdown_and_wait(Duration::from_millis(100)),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(matches!(outcome, BridgeShutdownOutcome::Forced(_)));
}

#[tokio::test]
async fn shutdown_cancels_observed_failure_reporting_without_draining_the_full_queue() {
    let root = tempfile::tempdir().unwrap();
    let (tx, mut rx) = mpsc::channel(1);
    tx.send(ClientEvent::LogoutCompleted).await.unwrap();
    let params = StartConnectionParams {
        event_tx: tx,
        cwd_raw: root.path().to_str().unwrap().into(),
        bridge_script: Some(root.path().join("missing.js")),
        launch: crate::StartupLaunch::NewSession,
        session_launch_settings: SessionLaunchSettings::default(),
    };
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let task = run_connection_task(params, Rc::new(RefCell::new(None)), shutdown_rx);
    tokio::pin!(task);
    let records =
        crate::logging::test_capture::capture(|| assert!(poll_once(task.as_mut()).is_pending()));
    assert!(
        records.iter().any(|r| r["event_name"] == "bridge_failure_reported"
            && r["error_category"] == "bridge_spawn")
    );
    shutdown_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), task).await.unwrap();
    assert!(matches!(rx.try_recv(), Ok(ClientEvent::LogoutCompleted)));
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn driver_gates_session_commands_and_publishes_a_usable_connection_only_after_initialized() {
    for resume in [false, true] {
        let fixture = Fixture::new("handshake");
        let mut bridge = fixture.spawn();
        let connection = bridge.connection();
        let (tx, mut rx) = mpsc::channel(8);
        let mut params = fixture.params(tx);
        params.session_launch_settings.name = Some("selected name".into());
        if resume {
            params.launch =
                crate::StartupLaunch::ResumeSession { session_id: "saved-session".into() };
        }
        let slot = Rc::new(RefCell::new(None));
        {
            let drive = drive_bridge_connection(&params, &slot, &mut bridge);
            tokio::pin!(drive);
            let initialize = tokio::select! {
                () = &mut drive => panic!("driver exited before readiness"),
                value = fixture.mark("initialize") => value,
            };
            assert_eq!(initialize, json!({"command":"initialize","cwd":params.cwd_raw}));
            connection.cancel("before-ready".into()).unwrap();
            tokio::select! {
                () = &mut drive => panic!("driver exited before readiness"),
                _ = fixture.mark("before-ready") => {},
            }
            assert!(slot.borrow().is_none());
            assert_eq!(
                fixture.commands(),
                [initialize, json!({"command":"cancel_turn","session_id":"before-ready"})]
            );
            fs::write(fixture.0.path().join("release.json"), b"true").unwrap();
            let event = tokio::select! {
                () = &mut drive => panic!("driver exited instead of serving the session"),
                event = tokio::time::timeout(Duration::from_secs(10), rx.recv()) => event.unwrap().unwrap(),
            };
            assert!(
                matches!(event, ClientEvent::AuthRequired { method_name, method_description } if method_name == "session-live" && method_description == "selected session command received")
            );
            let published = Rc::clone(&slot.borrow().as_ref().unwrap().conn);
            published.cancel("published-live".into()).unwrap();
            tokio::select! {
                () = &mut drive => panic!("published connection stopped"),
                _ = fixture.mark("published-live") => {},
            }
            assert!(rx.try_recv().is_err());
        }
        bridge.shutdown_and_wait(Duration::from_secs(2)).await.unwrap();
        let expected = if resume {
            json!({"command":"resume_session","session_id":"saved-session","launch_settings":{"name":"selected name"}})
        } else {
            json!({"command":"create_session","cwd":params.cwd_raw,"resume":null,"continue_session":false,"launch_settings":{"name":"selected name"}})
        };
        let commands = fixture.commands();
        assert_eq!(
            commands
                .iter()
                .filter(|v| v["command"] == "create_session" || v["command"] == "resume_session")
                .collect::<Vec<_>>(),
            [&expected]
        );
    }
}

#[tokio::test]
async fn driver_reports_eof_before_ready_without_publishing_or_requesting_a_session() {
    let fixture = Fixture::new("eof");
    let mut bridge = fixture.spawn();
    let (tx, mut rx) = mpsc::channel(8);
    let params = fixture.params(tx);
    let slot = Rc::new(RefCell::new(None));
    tokio::time::timeout(
        Duration::from_secs(10),
        drive_bridge_connection(&params, &slot, &mut bridge),
    )
    .await
    .unwrap();
    assert!(!bridge.wait().await.unwrap().success());
    assert!(slot.borrow().is_none());
    assert!(matches!(rx.try_recv(), Ok(ClientEvent::ConnectionFailed(_))));
    assert!(matches!(rx.try_recv(), Ok(ClientEvent::FatalError(AppError::BridgeStdoutClosed))));
    assert!(rx.try_recv().is_err());
    assert_eq!(fixture.commands(), [json!({"command":"initialize","cwd":params.cwd_raw})]);
}

#[tokio::test]
async fn control_delivery_proceeds_while_a_correlated_application_event_waits_for_capacity() {
    let fixture = Fixture::new("idle");
    let bridge = fixture.spawn();
    fixture.mark("ready").await;
    let connection = bridge.connection();
    let (tx, mut rx) = mpsc::channel(1);
    tx.send(ClientEvent::LogoutCompleted).await.unwrap();
    let mut connected = false;
    {
        let dispatch = handle_bridge_event(
            &tx,
            &connection,
            &mut connected,
            false,
            crate::agent::wire::EventEnvelope {
                request_id: None,
                event: BridgeEvent::AuthRequired {
                    method_name: "selected-provider".into(),
                    method_description: "safe login guidance".into(),
                },
            },
        );
        tokio::pin!(dispatch);
        assert!(poll_once(dispatch.as_mut()).is_pending());
        connection.cancel("cancel-session-7".into()).unwrap();
        assert_eq!(
            fixture.mark("cancel-session-7").await,
            json!({"command":"cancel_turn","session_id":"cancel-session-7"})
        );
        assert!(poll_once(dispatch.as_mut()).is_pending());
        assert!(matches!(rx.recv().await, Some(ClientEvent::LogoutCompleted)));
        tokio::time::timeout(Duration::from_secs(1), dispatch).await.unwrap();
        assert!(
            matches!(rx.recv().await, Some(ClientEvent::AuthRequired { method_name, method_description }) if method_name == "selected-provider" && method_description == "safe login guidance")
        );
    }
    bridge.shutdown_and_wait(Duration::from_secs(2)).await.unwrap();
    assert!(rx.try_recv().is_err());
    assert_eq!(
        fixture.commands(),
        [
            json!({"command":"cancel_turn","session_id":"cancel-session-7"}),
            json!({"command":"shutdown"})
        ]
    );
}
