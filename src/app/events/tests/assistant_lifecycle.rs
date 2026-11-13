// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::agent::client::CommandReceiver;
use crate::agent::wire::BridgeCommand;
use crate::app::HistoryOutputId;
use crate::ui::inline_chat_rows::{
    LiveRowBoundaryKind, serialize_live_rows_with_boundaries_excluding,
};
use pretty_assertions::assert_eq;
use std::collections::BTreeSet;

fn connected_app() -> (App, CommandReceiver) {
    let mut app = App::test_default();
    let (connection, receiver) = crate::agent::client::AgentConnection::test_channel();
    app.session_runtime.conn = Some(Rc::new(connection));
    app.session_runtime.session_id = Some(model::SessionId::new("test-session"));
    app.status = AppStatus::Ready;
    (app, receiver)
}

fn submit(app: &mut App, receiver: &mut CommandReceiver, source: &str) -> String {
    app.input.set_text(source);
    crate::app::input_submit::submit_input(app);
    let mut uuid = None;
    while let Ok(envelope) = receiver.try_recv() {
        match envelope.command {
            BridgeCommand::Prompt { session_id, message_uuid, chunks, inline_pastes } => {
                assert!(uuid.is_none());
                assert_eq!(session_id, "test-session");
                assert!(inline_pastes.is_empty());
                assert_eq!(chunks.len(), 1);
                assert_eq!(chunks[0].kind, "text");
                assert_eq!(chunks[0].value, serde_json::json!(source));
                uuid = Some(message_uuid);
            }
            BridgeCommand::GetContextUsage { session_id } => assert_eq!(session_id, "test-session"),
            other => panic!("unexpected command {other:?}"),
        }
    }
    uuid.expect("submitted prompt must reach the bridge")
}

fn drain_refreshes(receiver: &mut CommandReceiver) {
    while let Ok(envelope) = receiver.try_recv() {
        assert!(
            matches!(envelope.command,BridgeCommand::GetContextUsage { session_id } if session_id=="test-session")
        );
    }
}

fn owner_rows(app: &mut App, owner: usize, width: u16, kind: LiveRowBoundaryKind) -> Vec<String> {
    let rows = serialize_live_rows_with_boundaries_excluding(app, width, &BTreeSet::new(), None);
    rows.segments()
        .iter()
        .filter(|s| s.msg_idx == owner && s.kind == kind)
        .flat_map(|s| rows.segment_rows(s).unwrap())
        .map(ToString::to_string)
        .collect()
}

fn compact_rows(rows: &[String]) -> String {
    rows.join("").chars().filter(|c| !c.is_whitespace()).collect()
}

fn next_draft(app: &mut App) -> (String, String) {
    let paste = "draft \u{754c} details\n".repeat(120);
    app.input.set_text("next [Image #1]\n");
    app.input.insert_paste_block(&paste);
    app.input.image_payloads.push(crate::app::clipboard_image::ImageAttachment {
        data:"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aN1sAAAAASUVORK5CYII=".into(),mime_type:"image/png".into(),
    });
    (format!("next [Image #1]\n{paste}"), paste)
}

fn update(app: &mut App, update: model::SessionUpdate) {
    handle_client_event(app, session_update(update));
}

fn text(value: &str) -> model::SessionUpdate {
    model::SessionUpdate::AgentMessageChunk(model::ContentChunk::new(model::ContentBlock::Text(
        model::TextContent::new(value),
    )))
}

pub(super) fn assert_owner_has_no_rows(app: &mut App, owner: usize) {
    for width in [80, 12, 80] {
        let rendered =
            serialize_live_rows_with_boundaries_excluding(app, width, &BTreeSet::new(), None);
        assert!(rendered.segments().iter().all(|segment| segment.msg_idx != owner));
        assert_eq!(
            rendered.rows().len(),
            rendered
                .segments()
                .iter()
                .map(|segment| segment.end_row - segment.start_row)
                .sum::<usize>(),
            "an invisible owner must not leave unowned padding rows"
        );
    }
}

#[test]
fn accepted_prompt_reserves_one_invisible_owner_until_the_first_visible_event() {
    for visible in ["activity", "text", "tool", "notice"] {
        let (mut app, mut receiver) = connected_app();
        let uuid = submit(&mut app, &mut receiver, "Question with stable identity");
        let owner = app.active_turn_assistant_idx().unwrap();
        let owner_id = app.transcript.messages[owner].id;
        let user =
            app.transcript.messages.iter().find(|m| matches!(m.role, MessageRole::User)).unwrap();
        assert!(
            matches!(&user.blocks[..],[MessageBlock::Text(t)] if t.text=="Question with stable identity" && t.source_message_uuids==[uuid])
        );
        let mut snapshot = crate::agent::settings::SettingsSnapshot::test_value(
            "showMessageTimestamps",
            serde_json::json!(true),
        );
        snapshot.values.extend(
            crate::agent::settings::SettingsSnapshot::test_value(
                "showTurnDuration",
                serde_json::json!(true),
            )
            .values,
        );
        app.config.snapshot = Some(snapshot);
        for _ in 0..2 {
            update(&mut app, model::SessionUpdate::AgentResponseStarted);
        }
        update(&mut app, text(""));
        update(&mut app, text("\n \t\n"));
        update(
            &mut app,
            model::SessionUpdate::MessageMetadata {
                role: "assistant".into(),
                timestamp: "2026-10-15T13:00:00Z".into(),
                source_message_uuid: Some("first-response".into()),
            },
        );
        update(
            &mut app,
            model::SessionUpdate::TurnTiming { duration_ms: 42.0, api_duration_ms: Some(40.0) },
        );
        update(
            &mut app,
            model::SessionUpdate::ToolCall(
                model::ToolCall::new("internal-tool", "Hidden child tool")
                    .status(model::ToolCallStatus::InProgress)
                    .meta(serde_json::json!({"claudeCode":{"parentToolUseId":"parent-agent"}})),
            ),
        );
        assert_eq!(app.active_turn_assistant_idx(), Some(owner));
        assert_owner_has_no_rows(&mut app, owner);
        let (kind, expected) = match visible {
            "activity" => {
                update(
                    &mut app,
                    model::SessionUpdate::AgentActivityUpdate(model::AgentActivityPhase::Thinking),
                );
                (LiveRowBoundaryKind::AssistantThinking, None)
            }
            "text" => {
                update(&mut app, text("Distinct first answer"));
                (LiveRowBoundaryKind::AssistantText, Some("Distinct first answer"))
            }
            "tool" => {
                update(
                    &mut app,
                    model::SessionUpdate::ToolCall(
                        model::ToolCall::new("visible-tool", "Visible tool sentinel")
                            .status(model::ToolCallStatus::InProgress),
                    ),
                );
                assert_eq!(app.lookup_tool_call("visible-tool").unwrap().0, owner);
                (LiveRowBoundaryKind::AssistantTool, Some("Visible tool sentinel"))
            }
            _ => {
                crate::app::events::push_submission_feedback(
                    &mut app,
                    SystemSeverity::Warning,
                    "Visible notice sentinel",
                );
                (LiveRowBoundaryKind::AssistantNotice, Some("Visible notice sentinel"))
            }
        };
        let rendered = owner_rows(&mut app, owner, 80, kind);
        if let Some(expected) = expected {
            assert_eq!(rendered.iter().filter(|r| r.contains(expected)).count(), 1);
        } else {
            assert!(rendered.iter().any(|r| !r.trim().is_empty()));
            let activity = app.activity_presentation(std::time::Instant::now()).unwrap();
            assert!(activity.thinking);
            assert_eq!(activity.heading, crate::app::activity::ActivityHeading::Assistant(owner));
        }
        assert_eq!(app.transcript.messages[owner].id, owner_id);
        assert!(receiver.try_recv().is_err());
    }
}

#[test]
fn a_complete_live_reply_without_a_stream_start_cannot_extend_committed_history() {
    for (running, results, expected_text) in [
        (false, vec![text("Local "), text("command result")], Some("Local command result")),
        (
            false,
            vec![model::SessionUpdate::ToolCall(
                model::ToolCall::new("new-tool", "New tool result")
                    .status(model::ToolCallStatus::Completed)
                    .content(vec!["Distinct completed tool content".into()]),
            )],
            None,
        ),
        (
            true,
            vec![
                text("New "),
                text("live reply"),
                model::SessionUpdate::ToolCall(
                    model::ToolCall::new("new-tool", "New tool result")
                        .status(model::ToolCallStatus::InProgress),
                ),
            ],
            Some("New live reply"),
        ),
    ] {
        let (mut app, mut receiver) = connected_app();
        submit(&mut app, &mut receiver, "Previous question");
        update(&mut app, text("Finished "));
        update(&mut app, text("answer"));
        let previous = app.active_turn_assistant_idx().unwrap();
        let previous_id = app.transcript.messages[previous].id;
        handle_client_event(&mut app, turn_complete(None));
        let history =
            serialize_live_rows_with_boundaries_excluding(&mut app, 80, &BTreeSet::new(), None);
        let committed = history.segments().iter().flat_map(|s| s.ids.iter().cloned()).collect();
        if running {
            update(
                &mut app,
                model::SessionUpdate::RuntimeSessionStateUpdate(
                    model::RuntimeSessionState::Running,
                ),
            );
        }
        for result in results {
            update(&mut app, result);
        }
        let owner = app.active_turn_assistant_idx().unwrap();
        assert_ne!(app.transcript.messages[owner].id, previous_id);
        let remaining =
            serialize_live_rows_with_boundaries_excluding(&mut app, 80, &committed, None);
        assert!(remaining.segments().iter().all(|s| s.msg_idx != previous));
        assert!(remaining.segments().iter().any(|s| s.msg_idx == owner));
        if let Some(expected) = expected_text {
            assert_eq!(first_block_text(&app.transcript.messages[owner]), expected);
            assert!(remaining.rows().iter().any(|r| r.to_string().contains(expected)));
        }
        if running || expected_text.is_none() {
            assert_eq!(app.lookup_tool_call("new-tool").unwrap().0, owner);
            let tool_region = remaining
                .segments()
                .iter()
                .find(|s| s.msg_idx == owner && s.kind == LiveRowBoundaryKind::AssistantTool)
                .unwrap();
            let tool_rows = remaining.segment_rows(tool_region).unwrap();
            assert!(tool_rows.iter().any(|r| r.to_string().contains("New tool result")));
            if !running {
                assert_eq!(
                    app.tool_call("new-tool").unwrap().content,
                    [model::ToolCallContent::from("Distinct completed tool content")]
                );
                assert!(
                    tool_rows
                        .iter()
                        .any(|r| r.to_string().contains("Distinct completed tool content"))
                );
            }
        }
        assert_eq!(app.transcript.messages[previous].id, previous_id);
        assert_eq!(first_block_text(&app.transcript.messages[previous]), "Finished answer");
        assert_eq!(app.transcript.messages[previous].blocks.len(), 1);
        assert_eq!(app.status, if running { AppStatus::Running } else { AppStatus::Ready });
        assert_eq!(app.turn.activity.is_some(), running);
        handle_client_event(&mut app, turn_complete(None));
        assert_eq!(app.status, AppStatus::Ready);
        assert_eq!(first_block_text(&app.transcript.messages[previous]), "Finished answer");
        while let Ok(envelope) = receiver.try_recv() {
            assert!(
                matches!(envelope.command,BridgeCommand::GetContextUsage{ref session_id} if session_id=="test-session"),
                "completion may refresh usage, but must not submit another turn"
            );
        }
    }
}

#[test]
fn autonomous_start_after_a_running_signal_never_reopens_the_finished_response() {
    let (mut app, mut receiver) = connected_app();
    submit(&mut app, &mut receiver, "Previous question");
    update(&mut app, text("Finished answer"));
    let previous = app.active_turn_assistant_idx().unwrap();
    let previous_id = app.transcript.messages[previous].id;
    handle_client_event(&mut app, turn_complete(None));
    update(
        &mut app,
        model::SessionUpdate::RuntimeSessionStateUpdate(model::RuntimeSessionState::Running),
    );
    assert_eq!(app.active_turn_assistant_idx(), None);
    update(&mut app, model::SessionUpdate::AgentResponseStarted);
    let owner = app.active_turn_assistant_idx().expect("autonomous response owns fresh output");
    assert_ne!(app.transcript.messages[owner].id, previous_id);
    assert_owner_has_no_rows(&mut app, owner);
    update(&mut app, model::SessionUpdate::AgentResponseStarted);
    update(&mut app, text("Background answer"));
    assert_eq!(app.active_turn_assistant_idx(), Some(owner));
    let MessageBlock::Text(old) = &app.transcript.messages[previous].blocks[0] else {
        panic!("previous answer");
    };
    assert_eq!(old.text, "Finished answer");
    let MessageBlock::Text(new) = &app.transcript.messages[owner].blocks[0] else {
        panic!("background answer");
    };
    assert_eq!(new.text, "Background answer");
}

#[test]
fn empty_response_exit_removes_its_owner_without_consuming_the_next_draft() {
    for (outcome, queued) in [
        ("complete", false),
        ("cancel", false),
        ("cancel-error", false),
        ("cancel", true),
        ("cancel-error", true),
    ] {
        let (mut app, mut receiver) = connected_app();
        submit(&mut app, &mut receiver, "Prior question");
        update(&mut app, text("Prior preserved answer"));
        let previous = app.active_turn_assistant_idx().unwrap();
        let previous_id = app.transcript.messages[previous].id;
        handle_client_event(&mut app, turn_complete(None));
        submit(&mut app, &mut receiver, "Empty admitted question");
        let owner = app.active_turn_assistant_idx().unwrap();
        let owner_id = app.transcript.messages[owner].id;
        let next = queued.then(|| submit(&mut app, &mut receiver, "Queued continuation source"));
        next_draft(&mut app);
        let draft = app.input.snapshot();
        if outcome != "complete" {
            crate::app::input_submit::request_cancel(&mut app).unwrap();
            assert!(
                matches!(receiver.try_recv().unwrap().command,BridgeCommand::CancelTurn { session_id } if session_id=="test-session")
            );
        }
        assert_owner_has_no_rows(&mut app, owner);
        if outcome == "cancel-error" {
            fail_turn(&mut app, TurnErrorClass::Other, "cancelled operation");
        } else {
            handle_client_event(&mut app, turn_complete(None));
        }
        assert!(app.transcript.messages.iter().all(|m| m.id != owner_id));
        assert!(app.active_turn_assistant_idx().is_none());
        assert_eq!(app.transcript.messages[previous].id, previous_id);
        assert_eq!(first_block_text(&app.transcript.messages[previous]), "Prior preserved answer");
        assert_eq!(app.input.snapshot(), draft);
        let hints = app
            .transcript
            .messages
            .iter()
            .filter(|m| matches!(m.role, MessageRole::System(Some(SystemSeverity::Info))))
            .filter(|m| {
                m.blocks
                    .iter()
                    .any(|b| matches!(b,MessageBlock::Text(t) if t.text.contains("interrupted")))
            })
            .count();
        assert_eq!(hints, usize::from(outcome != "complete" && !queued));
        let rows =
            serialize_live_rows_with_boundaries_excluding(&mut app, 80, &BTreeSet::new(), None);
        assert!(
            !rows
                .segments()
                .iter()
                .any(|s| s.ids.contains(&HistoryOutputId::AssistantLabel(owner_id)))
        );
        drain_refreshes(&mut receiver);
        if let Some(uuid) = next {
            handle_client_event(
                &mut app,
                ClientEvent::UserMessageStarted {
                    session_id: "test-session".into(),
                    message_uuid: uuid.clone(),
                    source: crate::agent::types::UserMessageStartSource::StreamEvent,
                },
            );
            assert!(app.transcript.messages.iter().any(|m|m.blocks.iter().any(|b|matches!(b,MessageBlock::Text(t) if t.text=="Queued continuation source" && t.source_message_uuids==[uuid.clone()]))));
        }
        update(&mut app, text("Later independently owned answer"));
        let later = app.active_turn_assistant_idx().unwrap();
        assert_ne!(app.transcript.messages[later].id, owner_id);
        assert_eq!(
            first_block_text(&app.transcript.messages[later]),
            "Later independently owned answer"
        );
        assert_eq!(app.input.snapshot(), draft);
        assert!(receiver.try_recv().is_err());
    }
}

fn fail_turn(app: &mut App, class: TurnErrorClass, message: &str) {
    handle_client_event(
        app,
        ClientEvent::TurnErrorClassified {
            session_id: "test-session".into(),
            message: message.into(),
            class,
            queued_turn_count: Some(0),
            api_error_status: None,
            terminal_reason: Some(crate::agent::types::TerminalReason::ApiError),
        },
    );
}

#[test]
fn repeated_rate_limit_failures_render_in_their_submitted_replies() {
    for allowance_update in [false, true] {
        let (mut app, mut receiver) = connected_app();
        let mut replies = Vec::new();
        for cause in [
            "First limit: resets 13:30Z",
            "Second limit: resets 14:45Z",
            "Third limit: resets tomorrow",
        ] {
            let uuid = submit(&mut app, &mut receiver, "same prompt");
            let owner = app.active_turn_assistant_idx().unwrap();
            if allowance_update {
                update(&mut app,model::SessionUpdate::RateLimitUpdate(serde_json::from_value(serde_json::json!({"status":"rejected","rate_limit_type":"five_hour","resets_at":1_792_409_400.0})).unwrap()));
                assert_owner_has_no_rows(&mut app, owner);
            }
            app.input.set_text("editable next draft");
            let draft = app.input.snapshot();
            fail_turn(&mut app, TurnErrorClass::PlanLimit, cause);
            assert_eq!(app.status, AppStatus::Ready);
            assert!(app.composer_access().can_edit());
            assert!(app.composer_access().can_submit());
            replies.push((owner, app.transcript.messages[owner].id, cause));
            assert!(app.transcript.messages.iter().any(|m|m.blocks.iter().any(|b|matches!(b,MessageBlock::Text(t) if t.text=="same prompt" && t.source_message_uuids==[uuid.clone()]))));
            for &(index, id, expected) in &replies {
                assert_eq!(app.transcript.messages[index].id, id);
                for width in [40, 100] {
                    let rendered = compact_rows(&owner_rows(
                        &mut app,
                        index,
                        width,
                        LiveRowBoundaryKind::AssistantNotice,
                    ));
                    assert!(rendered.contains(&compact_rows(&[expected.into()])));
                    assert!(rendered.contains("retry"));
                    for &(_, _, other) in &replies {
                        if other != expected {
                            assert!(!rendered.contains(&compact_rows(&[other.into()])));
                        }
                    }
                }
            }
            assert_eq!(app.input.snapshot(), draft);
            drain_refreshes(&mut receiver);
        }
    }
}

fn assert_timeout_preserves_its_tool_and_cannot_be_unlocked_by_liveness() {
    let (mut app, mut receiver) = connected_app();
    submit(&mut app, &mut receiver, "Earlier prompt");
    update(&mut app, text("Earlier independent answer"));
    handle_client_event(&mut app, turn_complete(None));
    let prior =
        serialize_live_rows_with_boundaries_excluding(&mut app, 100, &BTreeSet::new(), None);
    let prior_id = app.transcript.messages[1].id;
    submit(&mut app, &mut receiver, "Failing prompt");
    let owner = app.active_turn_assistant_idx().unwrap();
    let owner_id = app.transcript.messages[owner].id;
    update(&mut app, text("Exact partial source"));
    update(
        &mut app,
        model::SessionUpdate::ToolCall(
            model::ToolCall::new("timeout-tool", "Timeout tool sentinel")
                .status(model::ToolCallStatus::InProgress)
                .content(vec!["Tool partial source".into()]),
        ),
    );
    app.input.set_text("Unsent recovery draft");
    handle_client_event(
        &mut app,
        ClientEvent::TurnError {
            session_id: "test-session".into(),
            message: "timeout fixture cause".into(),
            queued_turn_count: None,
            api_error_status: None,
            terminal_reason: None,
        },
    );
    assert_eq!(app.status, AppStatus::Error);
    assert_eq!(app.transcript.messages[owner].id, owner_id);
    assert_eq!(app.lookup_tool_call("timeout-tool").unwrap().0, owner);
    let [MessageBlock::Text(partial), MessageBlock::ToolCall(tool), MessageBlock::Notice(notice)] =
        app.transcript.messages[owner].blocks.as_slice()
    else {
        panic!("partial source then failed tool then its failure")
    };
    assert_eq!(partial.text, "Exact partial source");
    assert_eq!(tool.id, "timeout-tool");
    assert_eq!(tool.status, model::ToolCallStatus::Failed);
    assert_eq!(tool.content, vec![model::ToolCallContent::from("Tool partial source")]);
    assert!(notice.text.text.contains("timeout fixture cause"));
    let rendered =
        serialize_live_rows_with_boundaries_excluding(&mut app, 100, &BTreeSet::new(), None);
    let error = rendered
        .segments()
        .iter()
        .find(|s| s.msg_idx == owner && s.kind == LiveRowBoundaryKind::AssistantNotice)
        .unwrap();
    let tool = rendered
        .segments()
        .iter()
        .find(|s| s.msg_idx == owner && s.kind == LiveRowBoundaryKind::AssistantTool)
        .unwrap();
    assert!(tool.end_row <= error.start_row);
    assert!(
        rendered
            .segment_rows(error)
            .unwrap()
            .iter()
            .any(|row| row.to_string().contains("timeout fixture cause"))
    );
    let old: Vec<_> = rendered
        .segments()
        .iter()
        .filter(|s| s.msg_idx < owner - 1)
        .flat_map(|s| rendered.segment_rows(s).unwrap().iter().cloned())
        .collect();
    assert_eq!(old, prior.rows());
    assert_eq!(app.transcript.messages[1].id, prior_id);
    update(
        &mut app,
        model::SessionUpdate::RuntimeSessionStateUpdate(model::RuntimeSessionState::Running),
    );
    assert_eq!(app.status, AppStatus::Error);
    crate::app::input_submit::submit_input(&mut app);
    assert_eq!(app.input.text(), "Unsent recovery draft");
    assert!(receiver.try_recv().is_err(), "ordinary timeout cannot fabricate a retry Prompt");
}

fn assert_failure_regions(
    app: &mut App,
    previous: usize,
    owner: usize,
    class: TurnErrorClass,
    guidance: &str,
    partial: bool,
) {
    for width in [40, 100] {
        let rendered =
            compact_rows(&owner_rows(app, owner, width, LiveRowBoundaryKind::AssistantNotice));
        assert!(rendered.contains(guidance), "{class:?}: {rendered}");
        if class != TurnErrorClass::AuthRequired {
            assert!(rendered.contains("Distinctsuppliedfailurecause"));
        }
        let earlier =
            compact_rows(&owner_rows(app, previous, width, LiveRowBoundaryKind::AssistantNotice));
        assert!(earlier.contains("Earlierindependentwarning"));
        assert!(!earlier.contains("Distinctsuppliedfailurecause"));
        assert!(!rendered.contains("Earlierindependentwarning"));
        if partial {
            assert_eq!(first_block_text(&app.transcript.messages[owner]), "Exact partial answer");
            let rows =
                serialize_live_rows_with_boundaries_excluding(app, width, &BTreeSet::new(), None);
            let answer = rows
                .segments()
                .iter()
                .find(|s| s.msg_idx == owner && s.kind == LiveRowBoundaryKind::AssistantText)
                .unwrap();
            let notice = rows
                .segments()
                .iter()
                .find(|s| s.msg_idx == owner && s.kind == LiveRowBoundaryKind::AssistantNotice)
                .unwrap();
            assert!(answer.end_row <= notice.start_row);
            assert!(
                compact_rows(
                    &rows
                        .segment_rows(answer)
                        .unwrap()
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                )
                .contains("Exactpartialanswer")
            );
        }
    }
}

#[test]
fn turn_failures_preserve_owned_output_draft_and_class_specific_recovery() {
    assert_timeout_preserves_its_tool_and_cannot_be_unlocked_by_liveness();
    for (class, guidance) in [
        (TurnErrorClass::PlanLimit, "retry"),
        (TurnErrorClass::AuthRequired, "Authenticationrequired"),
        (TurnErrorClass::AccountAccess, "accountororganization"),
        (TurnErrorClass::ModelUnavailable, "/model"),
        (TurnErrorClass::TransientService, "retry"),
        (TurnErrorClass::Internal, "Inputdisabled"),
        (TurnErrorClass::Other, "Inputdisabled"),
    ] {
        for partial in [false, true] {
            let (mut app, mut receiver) = connected_app();
            submit(&mut app, &mut receiver, "Prior independent question");
            update(&mut app, text("Prior source sentinel"));
            crate::app::events::push_submission_feedback(
                &mut app,
                SystemSeverity::Warning,
                "Earlier independent warning",
            );
            let previous = app.active_turn_assistant_idx().unwrap();
            let previous_id = app.transcript.messages[previous].id;
            handle_client_event(&mut app, turn_complete(None));
            submit(&mut app, &mut receiver, "Failing question");
            let owner = app.active_turn_assistant_idx().unwrap();
            if partial {
                update(&mut app, text("Exact partial answer"));
            }
            let (expanded, paste) = next_draft(&mut app);
            let draft = app.input.snapshot();
            let images = app.input.images();
            fail_turn(&mut app, class, "Distinct supplied failure cause");
            assert_eq!(app.input.snapshot(), draft);
            assert_eq!(app.input.images(), images);
            assert!(app.active_turn_assistant_idx().is_none());
            assert_eq!(app.transcript.messages[previous].id, previous_id);
            assert_eq!(
                first_block_text(&app.transcript.messages[previous]),
                "Prior source sentinel"
            );
            assert_failure_regions(&mut app, previous, owner, class, guidance, partial);
            drain_refreshes(&mut receiver);
            crate::app::input_submit::submit_input(&mut app);
            if class == TurnErrorClass::PlanLimit {
                let BridgeCommand::Prompt { session_id, chunks, inline_pastes, .. } =
                    receiver.try_recv().expect("explicit retry").command
                else {
                    panic!("prompt")
                };
                assert_eq!(session_id, "test-session");
                assert_eq!(inline_pastes, [paste]);
                assert_eq!(
                    chunks.iter().find(|c| c.kind == "text").unwrap().value,
                    serde_json::json!(expanded)
                );
                assert_eq!(
                    chunks.iter().find(|c| c.kind == "image").unwrap().value["data"],
                    images[0].data
                );
                assert!(app.input.is_empty());
                drain_refreshes(&mut receiver);
            } else {
                assert!(receiver.try_recv().is_err());
                assert_eq!(app.input.snapshot(), draft);
                assert!(!app.composer_access().can_submit());
                if class == TurnErrorClass::AuthRequired {
                    assert!(app.shutdown_requested());
                    assert!(matches!(app.exit_error, Some(crate::error::AppError::AuthRequired)));
                } else {
                    assert_eq!(app.status, AppStatus::Error);
                    assert!(!app.shutdown_requested());
                }
            }
        }
    }
}

// Consume the named reply while leaving neighboring transcript sources available.
fn take_owner_text_rows(
    app: &mut App,
    owner: usize,
    excluded: &mut BTreeSet<HistoryOutputId>,
    width: u16,
    stable_only: bool,
) -> Vec<String> {
    let rendered = serialize_live_rows_with_boundaries_excluding(app, width, excluded, None);
    let mut rows = Vec::new();
    for segment in rendered.segments().iter().filter(|segment| {
        segment.msg_idx == owner && segment.kind == LiveRowBoundaryKind::AssistantText
    }) {
        if stable_only && segment.end_row > rendered.stable_row_count() {
            continue;
        }
        rows.extend(
            rendered
                .segment_rows(segment)
                .unwrap()
                .iter()
                .map(|row| row.to_string().trim_end().to_owned()),
        );
        excluded.extend(segment.ids.iter().cloned());
    }
    rows
}

fn assert_table_cells(rows: &[String], expected: &[(String, String)]) {
    // Read displayed columns, joining only wrapped continuation lines within each cell.
    // Expectations are authored from fixture meaning, never a Markdown parser/render.
    let mut cells: Vec<(String, String)> = Vec::new();
    for row in rows.iter().filter(|row| row.contains('\u{2502}')) {
        let columns = row.split('\u{2502}').collect::<Vec<_>>();
        assert_eq!(columns.len(), 4, "two displayed cells, including escaped literal pipes: {row}");
        let left = columns[1].trim();
        let right = columns[2].trim();
        if left.is_empty() {
            let previous = cells.last_mut().expect("wrapped cell follows its logical row");
            if !right.is_empty() {
                previous.1.push(' ');
                previous.1.push_str(right);
            }
        } else {
            cells.push((left.to_owned(), right.to_owned()));
        }
    }
    assert_eq!(
        cells, expected,
        "header, every cell pair and logical row order in this owner's table"
    );
}

type TableFixture = (&'static str, String, Vec<(String, String)>, bool);

fn table_fixtures() -> [TableFixture; 4] {
    use std::fmt::Write as _;

    let mut long_table = "Name | Value\n--- | ---\n".to_owned();
    let mut long_cells = vec![("Name".to_owned(), "Value".to_owned())];
    let mut n = 0;
    while long_table.len() <= crate::app::default_cache_split_policy().hard_limit_bytes {
        writeln!(
            long_table,
            "entry_{n} | value_{n} with enough detail to exceed the streaming cache budget"
        )
        .unwrap();
        long_cells.push((
            format!("entry_{n}"),
            format!("value_{n} with enough detail to exceed the streaming cache budget"),
        ));
        n += 1;
    }
    let pairs = |values: &[(&str, &str)]| {
        values
            .iter()
            .map(|(left, right)| ((*left).to_owned(), (*right).to_owned()))
            .collect::<Vec<_>>()
    };
    [
        (
            "Introduction.\n\n",
            "\u{540d}\u{524d} | Value\n- | -\nred | one\nblue | two\n".to_owned(),
            pairs(&[("\u{540d}\u{524d}", "Value"), ("red", "one"), ("blue", "two")]),
            false,
        ),
        (
            "Introduction.\n\n",
            "> Name | Value\n> --- | ---\n> red | `one\\|two`\n> blue | **three**\n".to_owned(),
            pairs(&[("Name", "Value"), ("red", "one|two"), ("blue", "three")]),
            false,
        ),
        ("Introduction.\n\n", long_table, long_cells, false),
        (
            "## Context usage\n\n",
            "| Category | Tokens |\n| --- | ---: |\n| System | 100 |".to_owned(),
            pairs(&[("Category", "Tokens"), ("System", "100")]),
            true,
        ),
    ]
}

fn assert_context_table_source(app: &App, owner: usize) {
    for block in &app.transcript.messages[owner].blocks {
        let MessageBlock::Text(block) = block else { panic!("context source block") };
        assert!(
            block.has_source_message_uuid("context-assistant-1"),
            "context UUID survives heading/table splits"
        );
    }
    let canonical = app.transcript.messages[owner]
        .blocks
        .iter()
        .map(|block| {
            let MessageBlock::Text(block) = block else { panic!("context source block") };
            block.text.as_str()
        })
        .collect::<String>();
    assert_eq!(
        canonical,
        "## Context usage\n\n| Category | Tokens |\n| --- | ---: |\n| System | 100 |"
    );
}

fn assert_table_neighbor_is_unconsumed(
    app: &mut App,
    neighbor: usize,
    excluded: &BTreeSet<HistoryOutputId>,
) {
    let remaining = serialize_live_rows_with_boundaries_excluding(app, 36, excluded, None);
    let rows = remaining
        .segments()
        .iter()
        .filter(|segment| {
            segment.msg_idx == neighbor && segment.kind == LiveRowBoundaryKind::AssistantText
        })
        .flat_map(|segment| remaining.segment_rows(segment).unwrap())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    assert_table_cells(
        &rows,
        &[
            ("Neighbor".to_owned(), "Other".to_owned()),
            ("decoy".to_owned(), "untouched".to_owned()),
        ],
    );
}

#[test]
fn tables_survive_sdk_chunks_cache_splits_and_scrollback_commits() {
    for (intro, table, expected_cells, context) in table_fixtures() {
        let ending = "\n\nFinal paragraph.";
        let source = format!("{intro}{table}{ending}");
        let mut whole = connected_app().0;
        update(&mut whole, model::SessionUpdate::AgentResponseStarted);
        update(&mut whole, text(&source));
        let whole_owner = whole.active_turn_assistant_idx().unwrap();
        handle_client_event(&mut whole, turn_complete(None));
        let whole_rows =
            take_owner_text_rows(&mut whole, whole_owner, &mut BTreeSet::new(), 36, false);
        assert_table_cells(&whole_rows, &expected_cells);

        // One-character chunks bisect syntax; larger chunks cross paragraph/cache boundaries.
        let chars = format!("{intro}{table}").chars().collect::<Vec<_>>();
        let sizes =
            if table.len() < 512 { vec![1, 17, chars.len()] } else { vec![113, 509, chars.len()] };
        for chunk_size in sizes {
            let (mut app, mut receiver) = connected_app();
            submit(&mut app, &mut receiver, "Neighbor question");
            update(
                &mut app,
                text(
                    "Neighbor heading.\n\n| Neighbor | Other |\n| --- | --- |\n| decoy | untouched |",
                ),
            );
            let neighbor = app.active_turn_assistant_idx().unwrap();
            let neighbor_id = app.transcript.messages[neighbor].id;
            handle_client_event(&mut app, turn_complete(None));
            submit(&mut app, &mut receiver, "Show the requested table");
            let owner = app.active_turn_assistant_idx().unwrap();
            let owner_id = app.transcript.messages[owner].id;
            let mut excluded = BTreeSet::new();
            let mut committed = Vec::new();
            for chunk in chars.chunks(chunk_size) {
                let value = chunk.iter().collect::<String>();
                let event = if context {
                    model::SessionUpdate::AgentMessageChunk(
                        model::ContentChunk::new(model::ContentBlock::Text(
                            model::TextContent::new(value),
                        ))
                        .source_message_uuid(Some("context-assistant-1".to_owned())),
                    )
                } else {
                    text(&value)
                };
                update(&mut app, event);
                committed.extend(take_owner_text_rows(&mut app, owner, &mut excluded, 36, true));
                assert!(
                    committed.iter().all(|row| !row.contains('\u{2502}')),
                    "an unfinished table cannot enter final history"
                );
            }
            assert_eq!(app.active_turn_assistant_idx(), Some(owner));
            assert_eq!(app.transcript.messages[owner].id, owner_id);
            let heading = if context { "Context usage" } else { "Introduction." };
            assert_eq!(
                committed.iter().filter(|row| row.contains(heading)).count(),
                1,
                "completed heading/prose progresses once while the table remains mutable"
            );
            let live = serialize_live_rows_with_boundaries_excluding(&mut app, 36, &excluded, None);
            let live_rows = live
                .segments()
                .iter()
                .filter(|segment| {
                    segment.msg_idx == owner && segment.kind == LiveRowBoundaryKind::AssistantText
                })
                .flat_map(|segment| live.segment_rows(segment).unwrap())
                .map(ToString::to_string)
                .collect::<Vec<_>>();
            assert_table_cells(&live_rows, &expected_cells);
            if context {
                assert_context_table_source(&app, owner);
            }
            update(&mut app, text(ending));
            committed.extend(take_owner_text_rows(&mut app, owner, &mut excluded, 36, true));
            assert_table_cells(&committed, &expected_cells);
            handle_client_event(&mut app, turn_complete(None));
            committed.extend(take_owner_text_rows(&mut app, owner, &mut excluded, 36, true));
            assert_table_cells(&committed, &expected_cells); // Completion must not duplicate a consumed table.
            assert_eq!(committed.iter().filter(|row| row.contains(heading)).count(), 1);
            assert_eq!(committed.iter().filter(|row| row.contains("Final paragraph.")).count(), 1);
            assert_eq!(
                committed, whole_rows,
                "secondary chunk invariance, chunk size {chunk_size}"
            );
            assert!(
                take_owner_text_rows(&mut app, owner, &mut excluded, 36, false).is_empty(),
                "consumed owner text is excluded"
            );
            assert_table_neighbor_is_unconsumed(&mut app, neighbor, &excluded);
            assert_eq!(app.transcript.messages[neighbor].id, neighbor_id);
            let copied = crate::app::copy::choices(&app);
            assert_eq!(
                copied.first().expect("completed response copy choice").text,
                source,
                "full-response copy is the exact requested Markdown"
            );
        }
    }
}

#[test]
fn list_rendering_survives_sdk_chunks_cache_splits_and_scrollback_commits() {
    for long in [false, true] {
        let continuation = if long {
            "Long continuation with useful detail. ".repeat(130)
        } else {
            "Parent continuation.".into()
        };
        let marker = if long { "." } else { ")" };
        let list = format!(
            "9{marker} Parent item.\n\n   {continuation}\n\n   - Child with \u{65e5}\u{672c}\u{8a9e}\u{1f600}.\n\n     Child continuation.\n10{marker} Next item."
        );
        let opening = format!("Introduction.\n\n{list}");
        let ending = "\n\nFinal paragraph.";
        let source = format!("{opening}{ending}");
        let chars = opening.chars().collect::<Vec<_>>();
        for width in [36, 70] {
            let (mut whole, mut receiver) = connected_app();
            submit(&mut whole, &mut receiver, "List question");
            update(&mut whole, text(&source));
            let whole_owner = whole.active_turn_assistant_idx().unwrap();
            handle_client_event(&mut whole, turn_complete(None));
            let whole_rows =
                take_owner_text_rows(&mut whole, whole_owner, &mut BTreeSet::new(), width, true);
            assert_list_content(&whole_rows, long);
            for size in if long { vec![17, 113, chars.len()] } else { vec![1, 17, chars.len()] } {
                let (mut app, mut receiver) = connected_app();
                submit(&mut app, &mut receiver, "Neighbor question");
                update(&mut app, text("Neighbor unrelated source"));
                let neighbor = app.active_turn_assistant_idx().unwrap();
                handle_client_event(&mut app, turn_complete(None));
                submit(&mut app, &mut receiver, "Requested list");
                let owner = app.active_turn_assistant_idx().unwrap();
                let mut excluded = BTreeSet::new();
                let mut committed = Vec::new();
                let mut streamed_source = String::new();
                for chunk in chars.chunks(size) {
                    let piece = chunk.iter().collect::<String>();
                    streamed_source.push_str(&piece);
                    update(&mut app, text(&piece));
                    let visible =
                        take_owner_text_rows(&mut app, owner, &mut excluded.clone(), width, false);
                    let visible_text = visible.join("\n");
                    for (source_token, rendered_token) in [
                        (format!("9{marker} Parent item."), "9. Parent item."),
                        ("Child with".into(), "Child with"),
                        ("Child continuation.".into(), "Child continuation."),
                        (format!("10{marker} Next item."), "10. Next item."),
                    ] {
                        if streamed_source.contains(&source_token) {
                            assert!(
                                visible_text.contains(rendered_token),
                                "received list content must remain visible before commitment: {rendered_token}"
                            );
                        }
                    }
                    committed.extend(take_owner_text_rows(
                        &mut app,
                        owner,
                        &mut excluded,
                        width,
                        true,
                    ));
                    assert!(
                        !committed.iter().any(|r| r.contains("Parent item")
                            || r.contains("Child")
                            || r.contains("Next item")),
                        "unfinished list stays mutable"
                    );
                }
                assert_eq!(committed.iter().filter(|r| r.contains("Introduction.")).count(), 1);
                update(&mut app, text(ending));
                handle_client_event(&mut app, turn_complete(None));
                committed.extend(take_owner_text_rows(&mut app, owner, &mut excluded, width, true));
                assert_list_content(&committed, long);
                assert_eq!(committed, whole_rows, "secondary chunk invariance");
                assert_eq!(crate::app::copy::choices(&app)[0].text, source);
                assert!(
                    take_owner_text_rows(&mut app, owner, &mut excluded, width, false).is_empty()
                );
                assert_eq!(
                    first_block_text(&app.transcript.messages[neighbor]),
                    "Neighbor unrelated source"
                );
                let remaining =
                    serialize_live_rows_with_boundaries_excluding(&mut app, width, &excluded, None);
                assert!(
                    remaining
                        .segments()
                        .iter()
                        .any(|s| s.msg_idx == neighbor
                            && s.kind == LiveRowBoundaryKind::AssistantText)
                );
            }
        }
    }
}

fn assert_list_content(rows: &[String], long: bool) {
    let locate = |needle: &str| {
        rows.iter()
            .position(|r| r.contains(needle))
            .unwrap_or_else(|| panic!("missing {needle}: {rows:?}"))
    };
    let intro = locate("Introduction.");
    let parent = locate("9. Parent item.");
    let child = locate("Child with");
    let continuation = locate("Child continuation.");
    let next = locate("10. Next item.");
    let end = locate("Final paragraph.");
    assert!(
        intro < parent
            && parent < child
            && child < continuation
            && continuation < next
            && next < end
    );
    let parent_column = rows[parent].find("9.").unwrap();
    let child_column = rows[child].find("Child").unwrap();
    assert!(child_column > parent_column);
    assert_eq!(
        rows[continuation].find("Child").unwrap(),
        child_column,
        "nested continuation keeps its hanging indent"
    );
    assert_eq!(
        rows[next].find("10.").unwrap(),
        parent_column,
        "number-width change does not change nesting"
    );
    let joined = compact_rows(rows);
    assert_eq!(joined.matches("Introduction.").count(), 1);
    assert_eq!(joined.matches("9.Parentitem.").count(), 1);
    assert_eq!(joined.matches("10.Nextitem.").count(), 1);
    assert!(joined.contains("Childwith\u{65e5}\u{672c}\u{8a9e}\u{1f600}."));
    assert_eq!(joined.matches("Childcontinuation.").count(), 1);
    assert_eq!(joined.matches("Finalparagraph.").count(), 1);
    if long {
        assert_eq!(joined.matches("Longcontinuationwithusefuldetail.").count(), 130);
    } else {
        assert_eq!(joined.matches("Parentcontinuation.").count(), 1);
    }
}
