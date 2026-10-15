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
    while let Ok(envelope) = receiver.try_recv() {
        if let BridgeCommand::Prompt { message_uuid, chunks, .. } = envelope.command {
            assert_eq!(chunks[0].value.as_str(), Some(source));
            return message_uuid;
        }
    }
    panic!("submitted prompt must reach the bridge");
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
        let rendered = serialize_live_rows_with_boundaries_excluding(app, width, &BTreeSet::new());
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
    for (first, expected_kind, expected_text) in [
        (
            model::SessionUpdate::AgentActivityUpdate(model::AgentActivityPhase::Thinking),
            LiveRowBoundaryKind::AssistantThinking,
            "Thinking…",
        ),
        (text("First text"), LiveRowBoundaryKind::AssistantText, "First text"),
        (
            model::SessionUpdate::ToolCall(
                model::ToolCall::new("first-tool", "First tool")
                    .status(model::ToolCallStatus::InProgress),
            ),
            LiveRowBoundaryKind::AssistantTool,
            "First tool",
        ),
    ] {
        let (mut app, mut receiver) = connected_app();
        submit(&mut app, &mut receiver, "Question");
        let owner = app.active_turn_assistant_idx().unwrap();
        let owner_id = app.transcript.messages[owner].id;
        assert_eq!(app.status, AppStatus::Running);
        assert!(app.turn.activity.is_some());
        assert!(app.transcript.messages[owner].blocks.is_empty());
        update(&mut app, model::SessionUpdate::AgentResponseStarted);
        update(&mut app, model::SessionUpdate::AgentResponseStarted);
        update(&mut app, text(""));
        assert_eq!(app.active_turn_assistant_idx(), Some(owner));
        assert_owner_has_no_rows(&mut app, owner);

        update(&mut app, first);
        let rendered =
            serialize_live_rows_with_boundaries_excluding(&mut app, 80, &BTreeSet::new());
        let content = rendered
            .segments()
            .iter()
            .find(|segment| segment.msg_idx == owner && segment.kind == expected_kind)
            .expect("first output belongs to the reserved response");
        assert!(content.ids.contains(&HistoryOutputId::AssistantLabel(owner_id)));
        assert!(rendered.segment_rows(content).unwrap().iter().any(|row| {
            row.to_string().contains(expected_text) && row.to_string().contains('●')
        }));
        assert_eq!(app.transcript.messages[owner].id, owner_id);
        assert_eq!(app.transcript.messages.len(), 2);
    }
}

#[test]
fn a_complete_live_reply_without_a_stream_start_cannot_extend_committed_history() {
    for result in [
        text("Local command result"),
        model::SessionUpdate::ToolCall(
            model::ToolCall::new("new-tool", "New tool result")
                .status(model::ToolCallStatus::Completed),
        ),
    ] {
        let (mut app, mut receiver) = connected_app();
        submit(&mut app, &mut receiver, "Previous question");
        update(&mut app, text("Finished answer"));
        let previous = app.active_turn_assistant_idx().unwrap();
        let previous_id = app.transcript.messages[previous].id;
        handle_client_event(&mut app, turn_complete(None));
        let history = serialize_live_rows_with_boundaries_excluding(&mut app, 80, &BTreeSet::new());
        let committed =
            history.segments().iter().flat_map(|segment| segment.ids.iter().cloned()).collect();
        update(&mut app, result);
        let owner = app.active_turn_assistant_idx().unwrap();
        assert_ne!(app.transcript.messages[owner].id, previous_id);
        let remaining = serialize_live_rows_with_boundaries_excluding(&mut app, 80, &committed);
        assert!(remaining.segments().iter().any(|segment| segment.msg_idx == owner));
        let MessageBlock::Text(old) = &app.transcript.messages[previous].blocks[0] else {
            panic!("previous answer");
        };
        assert_eq!(old.text, "Finished answer");
        assert_eq!(app.transcript.messages[previous].blocks.len(), 1);
        assert_eq!(app.status, AppStatus::Ready, "complete output must not invent a running turn");
        assert!(app.turn.activity.is_none());
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
fn metadata_whitespace_and_hidden_tools_do_not_create_an_empty_message_frame() {
    let (mut app, mut receiver) = connected_app();
    submit(&mut app, &mut receiver, "Question");
    let owner = app.active_turn_assistant_idx().unwrap();
    app.config.snapshot = Some(crate::agent::settings::SettingsSnapshot::test_value(
        "showMessageTimestamps",
        serde_json::json!(true),
    ));
    app.config.snapshot.as_mut().unwrap().values.extend(
        crate::agent::settings::SettingsSnapshot::test_value(
            "showTurnDuration",
            serde_json::json!(true),
        )
        .values,
    );
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
    update(&mut app, text("\n \t\n"));
    let mut hidden = model::ToolCall::new("internal-tool", "Hidden child tool")
        .status(model::ToolCallStatus::InProgress);
    hidden.meta = Some(serde_json::json!({"claudeCode": {"parentToolUseId": "parent-agent"}}));
    update(&mut app, model::SessionUpdate::ToolCall(hidden));
    assert_owner_has_no_rows(&mut app, owner);

    crate::app::events::push_submission_feedback(
        &mut app,
        SystemSeverity::Warning,
        "Visible notice",
    );
    let rendered = serialize_live_rows_with_boundaries_excluding(&mut app, 80, &BTreeSet::new());
    let notice = rendered
        .segments()
        .iter()
        .find(|segment| {
            segment.msg_idx == owner && segment.kind == LiveRowBoundaryKind::AssistantNotice
        })
        .unwrap();
    assert!(rendered.segment_rows(notice).unwrap().iter().any(|row| {
        row.to_string().contains("Visible notice") && row.to_string().contains('●')
    }));
    assert!(!app.transcript.messages[owner].timing.timestamp.unwrap().observed);
}

#[test]
fn failed_admission_preserves_the_draft_without_creating_user_or_assistant_messages() {
    let (mut app, receiver) = connected_app();
    drop(receiver);
    app.input.set_text("Keep this exact draft\n\nwith paragraphs");
    let draft = app.input.snapshot();
    crate::app::input_submit::submit_input(&mut app);
    assert_eq!(app.input.snapshot(), draft);
    assert!(
        app.transcript
            .messages
            .iter()
            .all(|message| matches!(message.role, MessageRole::System(_)))
    );
    assert_eq!(app.active_turn_assistant_idx(), None);
    assert!(app.turn.activity.is_none());
}

#[test]
fn queued_start_changes_owner_only_once_and_preserves_the_previous_response() {
    let (mut app, mut receiver) = connected_app();
    submit(&mut app, &mut receiver, "First question");
    update(&mut app, text("First answer"));
    let first_owner = app.active_turn_assistant_idx().unwrap();
    let first_id = app.transcript.messages[first_owner].id;
    let queued = submit(&mut app, &mut receiver, "Second question");
    assert_eq!(app.active_turn_assistant_idx(), Some(first_owner));
    assert_eq!(app.transcript.messages.len(), 2);
    for uuid in ["unknown", queued.as_str(), queued.as_str()] {
        handle_client_event(
            &mut app,
            ClientEvent::UserMessageStarted {
                session_id: "test-session".into(),
                message_uuid: uuid.into(),
                source: crate::agent::types::UserMessageStartSource::StreamEvent,
            },
        );
    }
    let second_owner = app.active_turn_assistant_idx().unwrap();
    assert_ne!(app.transcript.messages[second_owner].id, first_id);
    assert_eq!(app.transcript.messages.len(), 4);
    assert_owner_has_no_rows(&mut app, second_owner);
    update(&mut app, text("Second answer"));
    let MessageBlock::Text(first) = &app.transcript.messages[first_owner].blocks[0] else {
        panic!("first answer");
    };
    assert_eq!(first.text, "First answer");
    let MessageBlock::Text(second) = &app.transcript.messages[second_owner].blocks[0] else {
        panic!("second answer");
    };
    assert_eq!(second.text, "Second answer");
}

#[test]
fn empty_response_exit_removes_its_owner_without_consuming_the_next_draft() {
    for outcome in ["complete", "error", "cancel"] {
        let (mut app, mut receiver) = connected_app();
        submit(&mut app, &mut receiver, "Question");
        let owner = app.active_turn_assistant_idx().unwrap();
        let owner_id = app.transcript.messages[owner].id;
        app.input.set_text("Next unsent draft");
        let draft = app.input.snapshot();
        if outcome == "cancel" {
            crate::app::input_submit::request_cancel(&mut app).unwrap();
        }
        assert_owner_has_no_rows(&mut app, owner);
        let event = if outcome == "error" {
            ClientEvent::TurnError {
                session_id: "test-session".into(),
                message: "Fixture failure".into(),
                queued_turn_count: Some(0),
                api_error_status: None,
                terminal_reason: None,
            }
        } else {
            turn_complete(None)
        };
        handle_client_event(&mut app, event);
        assert!(app.transcript.messages.iter().all(|message| message.id != owner_id));
        assert_eq!(app.active_turn_assistant_idx(), None);
        assert!(app.turn.activity.is_none());
        assert_eq!(app.input.snapshot(), draft);
        if outcome == "error" {
            assert!(app.transcript.messages.iter().any(|message| {
                matches!(message.role, MessageRole::System(_))
                    && matches!(&message.blocks[0], MessageBlock::Text(block) if block.text.contains("Fixture failure"))
            }));
        }
    }
}
