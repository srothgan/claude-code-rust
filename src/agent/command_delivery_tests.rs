// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::agent::types::{
    ElicitationAction, PermissionOutcome, QuestionOutcome, UserDialogOutcome,
};
use serde_json::json;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::task::{Context, Poll, Waker};

fn pending<F: Future>(future: Pin<&mut F>) {
    require_pending(future).unwrap();
}

fn require_pending<F: Future>(future: Pin<&mut F>) -> anyhow::Result<()> {
    anyhow::ensure!(
        future.poll(&mut Context::from_waker(Waker::noop())).is_pending(),
        "operation completed before its gate was released"
    );
    Ok(())
}

fn submit_lane(connection: &AgentConnection, control: bool, id: &str) -> anyhow::Result<()> {
    if control {
        connection.cancel(id.into())
    } else {
        connection.set_model(id.into(), "chosen-model".into())
    }
}

fn assert_lane(envelope: &CommandEnvelope, control: bool, expected_id: &str) {
    assert!(envelope.request_id.is_none());
    match &envelope.command {
        BridgeCommand::CancelTurn { session_id } if control => assert_eq!(session_id, expected_id),
        BridgeCommand::SetModel { session_id, model } if !control => {
            assert_eq!(session_id, expected_id);
            assert_eq!(model, "chosen-model");
        }
        other => panic!("wrong accepted command: {other:?}"),
    }
}

#[tokio::test]
async fn lane_budgets_are_shared_bounded_and_retained_until_inflight_work_finishes() {
    for (control, capacity) in
        [(false, REGULAR_COMMAND_QUEUE_CAPACITY), (true, CONTROL_COMMAND_QUEUE_CAPACITY)]
    {
        let (connection, mut receiver) = command_channel();
        let clone = connection.clone();
        for index in 0..capacity {
            submit_lane(&connection, control, &format!("accepted-{index}")).unwrap();
        }
        assert!(submit_lane(&clone, control, "overflow").is_err());
        submit_lane(&clone, !control, "other-lane").unwrap();
        let in_flight = receiver.recv().await.unwrap();
        assert!(
            submit_lane(&clone, control, "premature-reuse").is_err(),
            "dequeue must retain the budget until the write finishes"
        );
        assert_lane(&in_flight.envelope, control, "accepted-0");
        drop(in_flight);
        submit_lane(&clone, control, "reused-slot").unwrap();
        for index in 1..capacity {
            assert_lane(
                &receiver.recv_envelope().await.unwrap(),
                control,
                &format!("accepted-{index}"),
            );
        }
        assert_lane(&receiver.recv_envelope().await.unwrap(), !control, "other-lane");
        assert_lane(&receiver.recv_envelope().await.unwrap(), control, "reused-slot");
        assert!(receiver.try_recv().is_err(), "rejected commands must never arrive");
        submit_lane(&connection, control, "next-cycle").unwrap();
        assert_lane(&receiver.recv_envelope().await.unwrap(), control, "next-cycle");
    }
}

async fn respond(connection: &AgentConnection, case: &str) -> anyhow::Result<()> {
    match case {
        "permission" => {
            connection
                .respond_to_permission(
                    "answer-session".into(),
                    "permission-tool".into(),
                    PermissionOutcome::Selected { option_id: "allow_once".into() },
                )
                .await
        }
        "question" => {
            connection
                .respond_to_question(
                    "answer-session".into(),
                    "question-tool".into(),
                    QuestionOutcome::Answered {
                        selected_option_ids: vec!["chosen-option".into()],
                        annotation: None,
                    },
                )
                .await
        }
        "dialog" => {
            connection
                .respond_to_user_dialog(
                    "answer-session".into(),
                    "dialog-callback".into(),
                    UserDialogOutcome::Selected { option_id: "retry_fallback".into() },
                )
                .await
        }
        "accept-empty" => {
            connection
                .respond_to_elicitation(
                    "answer-session".into(),
                    "elicitation-callback".into(),
                    ElicitationAction::Accept,
                    None,
                )
                .await
        }
        "decline" => {
            connection
                .respond_to_elicitation(
                    "answer-session".into(),
                    "elicitation-callback".into(),
                    ElicitationAction::Decline,
                    None,
                )
                .await
        }
        "accept-form" => {
            connection
                .respond_to_elicitation(
                    "answer-session".into(),
                    "elicitation-callback".into(),
                    ElicitationAction::Accept,
                    Some(json!({"approved":false,"limit":3})),
                )
                .await
        }
        _ => panic!("unknown case"),
    }
}

fn saturate_control(connection: &AgentConnection) {
    for index in 0..CONTROL_COMMAND_QUEUE_CAPACITY {
        connection.cancel(format!("queued-{index}")).unwrap();
    }
}

#[tokio::test]
async fn interactive_responses_wait_for_capacity_and_arrive_once_with_their_payloads() {
    for (case, expected) in [
        (
            "permission",
            json!({"command":"permission_response","session_id":"answer-session","tool_call_id":"permission-tool","outcome":{"outcome":"selected","option_id":"allow_once"}}),
        ),
        (
            "question",
            json!({"command":"question_response","session_id":"answer-session","tool_call_id":"question-tool","outcome":{"outcome":"answered","selected_option_ids":["chosen-option"],"annotation":null}}),
        ),
        (
            "dialog",
            json!({"command":"user_dialog_response","session_id":"answer-session","request_id":"dialog-callback","outcome":{"outcome":"selected","option_id":"retry_fallback"}}),
        ),
        (
            "accept-empty",
            json!({"command":"elicitation_response","session_id":"answer-session","elicitation_request_id":"elicitation-callback","action":"accept"}),
        ),
        (
            "decline",
            json!({"command":"elicitation_response","session_id":"answer-session","elicitation_request_id":"elicitation-callback","action":"decline"}),
        ),
        (
            "accept-form",
            json!({"command":"elicitation_response","session_id":"answer-session","elicitation_request_id":"elicitation-callback","action":"accept","content":{"approved":false,"limit":3}}),
        ),
    ] {
        let (connection, mut receiver) = command_channel();
        saturate_control(&connection);
        let mut response = Box::pin(respond(&connection, case));
        pending(response.as_mut());
        let in_flight = receiver.recv().await.unwrap();
        pending(response.as_mut());
        assert_lane(&in_flight.envelope, true, "queued-0");
        drop(in_flight);
        tokio::time::timeout(Duration::from_secs(2), response).await.unwrap().unwrap();
        for index in 1..CONTROL_COMMAND_QUEUE_CAPACITY {
            assert_lane(&receiver.recv_envelope().await.unwrap(), true, &format!("queued-{index}"));
        }
        assert_eq!(serde_json::to_value(receiver.try_recv().unwrap()).unwrap(), expected, "{case}");
        assert!(receiver.try_recv().is_err());
    }
}

#[tokio::test]
async fn cancelled_waiters_release_admission_and_closed_receivers_settle_waiters() {
    let (connection, mut receiver) = command_channel();
    saturate_control(&connection);
    let mut abandoned = Box::pin(respond(&connection, "decline"));
    pending(abandoned.as_mut());
    drop(abandoned);
    for index in 0..CONTROL_COMMAND_QUEUE_CAPACITY {
        assert_lane(&receiver.recv_envelope().await.unwrap(), true, &format!("queued-{index}"));
    }
    tokio::time::timeout(Duration::from_secs(2), respond(&connection, "accept-empty"))
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        receiver.try_recv().unwrap().command,
        BridgeCommand::ElicitationResponse { action: ElicitationAction::Accept, content: None, .. }
    ));
    assert!(receiver.try_recv().is_err(), "cancelled decline must not be admitted later");
    saturate_control(&connection);
    let mut waiting = Box::pin(respond(&connection, "permission"));
    pending(waiting.as_mut());
    drop(receiver);
    assert!(tokio::time::timeout(Duration::from_secs(2), waiting).await.unwrap().is_err());
}

#[derive(Default)]
struct SinkState {
    pending_bytes: Vec<u8>,
    visible: Vec<u8>,
    allow_write: bool,
    allow_flush: bool,
    fail_write: bool,
    waker: Option<Waker>,
}

struct ControlledSink(Arc<Mutex<SinkState>>);

impl AsyncWrite for ControlledSink {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let mut state = self.0.lock().unwrap();
        if !state.allow_write {
            state.waker = Some(cx.waker().clone());
            return Poll::Pending;
        }
        if state.fail_write {
            return Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
        }
        let count = bytes.len().min(3);
        state.pending_bytes.extend_from_slice(&bytes[..count]);
        Poll::Ready(Ok(count))
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let mut state = self.0.lock().unwrap();
        if !state.allow_flush {
            state.waker = Some(cx.waker().clone());
            return Poll::Pending;
        }
        let bytes = std::mem::take(&mut state.pending_bytes);
        state.visible.extend(bytes);
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.poll_flush(cx)
    }
}

fn release(state: &Arc<Mutex<SinkState>>, fail: bool) {
    let mut state = state.lock().unwrap();
    state.allow_write = true;
    state.allow_flush = true;
    state.fail_write = fail;
    if let Some(waker) = state.waker.take() {
        waker.wake();
    }
}

#[tokio::test]
async fn envelopes_survive_short_writes_and_become_visible_only_after_flush() {
    let state = Arc::new(Mutex::new(SinkState { allow_write: true, ..Default::default() }));
    let mut writer = ControlledSink(state.clone());
    for (envelope, expected) in [
        (
            CommandEnvelope {
                request_id: Some("rename-operation".into()),
                command: BridgeCommand::RenameSession {
                    session_id: "source-session".into(),
                    title: "Quoted \"雪\"\nnext line".into(),
                },
            },
            json!({"request_id":"rename-operation","command":"rename_session","session_id":"source-session","title":"Quoted \"雪\"\nnext line"}),
        ),
        (
            CommandEnvelope {
                request_id: Some("cancel-operation".into()),
                command: BridgeCommand::CancelTurn { session_id: "other-session".into() },
            },
            json!({"request_id":"cancel-operation","command":"cancel_turn","session_id":"other-session"}),
        ),
    ] {
        state.lock().unwrap().allow_flush = false;
        let before = state.lock().unwrap().visible.clone();
        let mut write = Box::pin(write_command_envelope(&mut writer, &envelope));
        pending(write.as_mut());
        assert_eq!(state.lock().unwrap().visible, before);
        release(&state, false);
        tokio::time::timeout(Duration::from_secs(2), write).await.unwrap().unwrap();
        let output = state.lock().unwrap().visible.clone();
        let appended = &output[before.len()..];
        let (delimiter, record) = appended.split_last().unwrap();
        assert_eq!(*delimiter, b'\n');
        assert!(!record.contains(&b'\n'), "one physical NDJSON delimiter");
        assert_eq!(serde_json::from_slice::<serde_json::Value>(appended).unwrap(), expected);
    }
}

#[tokio::test]
async fn acknowledged_sends_require_flush_and_writer_failure_settles_queued_waiters() {
    let directory = tempfile::tempdir().unwrap();
    let script = directory.path().join("ack-peer.cjs");
    std::fs::write(&script, "setInterval(() => {}, 1000);\n").unwrap();
    let mut bridge =
        BridgeClient::spawn(&BridgeLauncher { runtime_path: "node".into(), script_path: script })
            .unwrap();
    bridge.stop_writer().await;
    let result = acknowledgement_scenarios(&mut bridge).await;
    bridge.shutdown_and_wait(Duration::from_millis(30)).await.unwrap();
    result.unwrap();
}

async fn acknowledgement_scenarios(bridge: &mut BridgeClient) -> anyhow::Result<()> {
    for fail in [false, true] {
        let (connection, receiver) = command_channel();
        bridge.command_tx = connection.command_tx;
        let state = Arc::new(Mutex::new(SinkState { allow_write: !fail, ..Default::default() }));
        let mut writer = Box::pin(run_command_writer(ControlledSink(state.clone()), receiver));
        let mut first = Box::pin(bridge.send(CommandEnvelope {
            request_id: Some("first-ack".into()),
            command: BridgeCommand::CancelTurn { session_id: "first-session".into() },
        }));
        require_pending(first.as_mut())?;
        require_pending(writer.as_mut())?;
        require_pending(first.as_mut())?;
        let mut queued = Box::pin(bridge.send(CommandEnvelope {
            request_id: Some("queued-ack".into()),
            command: BridgeCommand::CancelTurn { session_id: "queued-session".into() },
        }));
        require_pending(queued.as_mut())?;
        anyhow::ensure!(state.lock().unwrap().visible.is_empty(), "acknowledgement before flush");
        release(&state, fail);
        if fail {
            anyhow::ensure!(
                tokio::time::timeout(Duration::from_secs(2), writer).await?.is_err(),
                "failing write succeeded"
            );
        } else {
            require_pending(writer.as_mut())?;
        }
        let first_result = tokio::time::timeout(Duration::from_secs(2), first).await?;
        let queued_result = tokio::time::timeout(Duration::from_secs(2), queued).await?;
        if fail {
            anyhow::ensure!(
                first_result.is_err() && queued_result.is_err(),
                "failed/queued command was falsely acknowledged"
            );
            anyhow::ensure!(
                state.lock().unwrap().visible.is_empty(),
                "failed write became visible"
            );
        } else {
            first_result?;
            queued_result?;
            let bytes = state.lock().unwrap().visible.clone();
            let records: Vec<serde_json::Value> = bytes
                .split(|b| *b == b'\n')
                .filter(|line| !line.is_empty())
                .map(serde_json::from_slice)
                .collect::<Result<_, _>>()?;
            anyhow::ensure!(
                records
                    == vec![
                        json!({"request_id":"first-ack","command":"cancel_turn","session_id":"first-session"}),
                        json!({"request_id":"queued-ack","command":"cancel_turn","session_id":"queued-session"})
                    ],
                "wrong acknowledged records"
            );
        }
    }
    Ok(())
}
