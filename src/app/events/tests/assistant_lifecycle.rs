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
            serialize_live_rows_with_boundaries_excluding(&mut app, 80, &BTreeSet::new(), None);
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
        let history =
            serialize_live_rows_with_boundaries_excluding(&mut app, 80, &BTreeSet::new(), None);
        let committed =
            history.segments().iter().flat_map(|segment| segment.ids.iter().cloned()).collect();
        update(&mut app, result);
        let owner = app.active_turn_assistant_idx().unwrap();
        assert_ne!(app.transcript.messages[owner].id, previous_id);
        let remaining =
            serialize_live_rows_with_boundaries_excluding(&mut app, 80, &committed, None);
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
    let rendered =
        serialize_live_rows_with_boundaries_excluding(&mut app, 80, &BTreeSet::new(), None);
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
    for outcome in ["complete", "cancel"] {
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
        handle_client_event(&mut app, turn_complete(None));
        assert!(app.transcript.messages.iter().all(|message| message.id != owner_id));
        assert_eq!(app.active_turn_assistant_idx(), None);
        assert!(app.turn.activity.is_none());
        assert_eq!(app.input.snapshot(), draft);
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

fn assert_failure_layout(app: &mut App, owner: usize, width: u16) -> String {
    use ratatui::{
        buffer::Buffer,
        layout::Rect,
        widgets::{Paragraph, Widget},
    };

    let rendered =
        serialize_live_rows_with_boundaries_excluding(app, width, &BTreeSet::new(), None);
    let segments =
        rendered.segments().iter().filter(|segment| segment.msg_idx == owner).collect::<Vec<_>>();
    let rows = segments
        .iter()
        .flat_map(|segment| rendered.segment_rows(segment).unwrap())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    assert!(rows.first().unwrap().trim().is_empty(), "reply has top spacing");
    assert!(rows.last().unwrap().trim().is_empty(), "reply has bottom spacing");
    let content = rows.iter().filter(|row| !row.trim().is_empty()).collect::<Vec<_>>();
    assert!(content[0].starts_with(" \u{25cf} "), "reply owns its assistant marker");
    assert_eq!(content.iter().filter(|row| row.contains('\u{25cf}')).count(), 1);
    for row in &content[1..] {
        assert!(row.starts_with("   "), "continuation aligns with reply text: {row:?}");
    }

    let notices = segments
        .iter()
        .filter(|segment| segment.kind == LiveRowBoundaryKind::AssistantNotice)
        .collect::<Vec<_>>();
    assert_eq!(notices.len(), 1, "one rendered failure in this reply");
    let area = Rect::new(0, 0, width, rendered.rows().len().try_into().unwrap());
    let mut buffer = Buffer::empty(area);
    Paragraph::new(rendered.rows().to_vec()).render(area, &mut buffer);
    let notice = notices[0];
    let mut colored_cells = 0;
    for y in notice.start_row..notice.end_row {
        for x in 3..width {
            let cell = &buffer[(x, u16::try_from(y).unwrap())];
            if !cell.symbol().trim().is_empty() {
                assert_eq!(cell.fg, crate::ui::theme::STATUS_ERROR);
                colored_cells += 1;
            }
        }
    }
    assert!(colored_cells > 0, "failure text must be visible");
    let marker_y = segments[0].start_row + 1;
    assert_eq!(buffer[(1, u16::try_from(marker_y).unwrap())].symbol(), "\u{25cf}");
    assert_eq!(buffer[(1, u16::try_from(marker_y).unwrap())].fg, crate::ui::theme::ROLE_ASSISTANT);
    rendered
        .segment_rows(notice)
        .unwrap()
        .iter()
        .map(ToString::to_string)
        .collect::<String>()
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect()
}

#[test]
fn repeated_rate_limit_failures_render_in_their_submitted_replies() {
    const ERROR: &str = "You've hit your session limit · resets 1:30pm (Europe/Berlin)";
    for send_first_update in [true, false] {
        let (mut app, mut receiver) = connected_app();
        let mut replies = Vec::new();
        for (turn, prompt) in ["hello", "hi", "hi"].into_iter().enumerate() {
            submit(&mut app, &mut receiver, prompt);
            let owner = app.active_turn_assistant_idx().unwrap();
            let owner_id = app.transcript.messages[owner].id;
            if turn == 0 && send_first_update {
                update(
                    &mut app,
                    model::SessionUpdate::RateLimitUpdate(
                        serde_json::from_value(serde_json::json!({
                            "status": "rejected", "rate_limit_type": "five_hour",
                            "resets_at": 1_792_409_400.0,
                        }))
                        .unwrap(),
                    ),
                );
                assert_owner_has_no_rows(&mut app, owner);
            }
            fail_turn(&mut app, TurnErrorClass::PlanLimit, ERROR);
            assert_eq!(app.status, AppStatus::Ready);
            let reply = &app.transcript.messages[owner];
            assert_eq!(reply.id, owner_id, "retain the reserved reply");
            assert!(matches!(reply.role, MessageRole::Assistant));
            let [MessageBlock::Notice(notice)] = reply.blocks.as_slice() else {
                panic!("exactly one notice belongs to this reply");
            };
            assert_eq!(notice.severity, SystemSeverity::Error);
            assert!(notice.text.text.contains(ERROR), "preserve SDK explanation and reset time");
            assert!(
                !notice.text.text.contains("5-hour"),
                "use this result, not earlier allowance data"
            );
            replies.push((owner, owner_id));
            for &(index, id) in &replies {
                assert_eq!(app.transcript.messages[index].id, id);
                for width in [40, 100] {
                    let visible = assert_failure_layout(&mut app, index, width);
                    assert!(
                        visible.contains(
                            &ERROR.chars().filter(|ch| !ch.is_whitespace()).collect::<String>()
                        ),
                        "this reply must render the SDK explanation and reset time"
                    );
                }
            }
        }
        assert_eq!(app.transcript.messages.len(), 6, "three submissions and their three replies");
        for ((owner, _), prompt) in replies.into_iter().zip(["hello", "hi", "hi"]) {
            let user = &app.transcript.messages[owner - 1];
            assert!(matches!(user.role, MessageRole::User));
            assert_eq!(first_block_text(user), prompt);
        }
    }
}

#[test]
fn turn_failures_keep_partial_output_and_use_the_same_notice_renderer() {
    for class in [
        TurnErrorClass::PlanLimit,
        TurnErrorClass::AuthRequired,
        TurnErrorClass::AccountAccess,
        TurnErrorClass::ModelUnavailable,
        TurnErrorClass::TransientService,
        TurnErrorClass::Internal,
        TurnErrorClass::Other,
    ] {
        for partial in [false, true] {
            let (mut app, mut receiver) = connected_app();
            submit(&mut app, &mut receiver, "Question");
            let owner = app.active_turn_assistant_idx().unwrap();
            let owner_id = app.transcript.messages[owner].id;
            if partial {
                update(&mut app, text("Partial answer"));
            }
            app.input.set_text("Next unsent draft");
            let draft = app.input.snapshot();
            fail_turn(&mut app, class, "Fixture failure");
            assert_eq!(app.input.snapshot(), draft);
            assert_eq!(app.active_turn_assistant_idx(), None);
            assert!(app.turn.activity.is_none());
            assert_eq!(app.transcript.messages.len(), 2);
            let reply = &app.transcript.messages[owner];
            assert_eq!(reply.id, owner_id);
            assert!(matches!(reply.role, MessageRole::Assistant));
            if partial {
                assert_eq!(first_block_text(reply), "Partial answer");
            }
            for width in [40, 100] {
                assert_failure_layout(&mut app, owner, width);
                if partial {
                    let rendered = serialize_live_rows_with_boundaries_excluding(
                        &mut app,
                        width,
                        &BTreeSet::new(),
                        None,
                    );
                    let answer = rendered
                        .segments()
                        .iter()
                        .find(|segment| {
                            segment.msg_idx == owner
                                && segment.kind == LiveRowBoundaryKind::AssistantText
                        })
                        .unwrap();
                    let notice = rendered
                        .segments()
                        .iter()
                        .find(|segment| {
                            segment.msg_idx == owner
                                && segment.kind == LiveRowBoundaryKind::AssistantNotice
                        })
                        .unwrap();
                    assert!(answer.end_row <= notice.start_row, "failure follows partial output");
                    assert!(
                        rendered
                            .segment_rows(answer)
                            .unwrap()
                            .iter()
                            .any(|row| row.to_string().contains("Partial answer"))
                    );
                }
            }
        }
    }
}

fn take_assistant_text_rows(
    app: &mut App,
    excluded: &mut BTreeSet<HistoryOutputId>,
    width: u16,
    stable_only: bool,
) -> Vec<String> {
    let rendered = serialize_live_rows_with_boundaries_excluding(app, width, excluded, None);
    let mut rows = Vec::new();
    for segment in rendered.segments() {
        if segment.kind != LiveRowBoundaryKind::AssistantText
            || (stable_only && segment.end_row > rendered.stable_row_count())
        {
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

#[test]
fn tables_survive_sdk_chunks_cache_splits_and_scrollback_commits() {
    use std::fmt::Write as _;

    let long_rows = (0..95).fold(String::new(), |mut rows, n| {
        writeln!(
            rows,
            "entry_{n} | value_{n} with enough detail to exceed the streaming cache budget"
        )
        .unwrap();
        rows
    });
    let fixtures = [
        ("Introduction.\n\n\u{540d}\u{524d} | Value\n- | -\nred | one\nblue | two\n\nFinal paragraph.".to_owned(), "red", "one"),
        ("Introduction.\n\n> Name | Value\n> --- | ---\n> red | `one\\|two`\n> blue | **three**\n\nFinal paragraph.".to_owned(), "red", "one|two"),
        (format!("Introduction.\n\nName | Value\n--- | ---\n{long_rows}\nFinal paragraph."), "entry_94", "value_94"),
    ];
    for (source, name, value) in fixtures {
        let mut whole = connected_app().0;
        update(&mut whole, model::SessionUpdate::AgentResponseStarted);
        update(&mut whole, text(&source));
        handle_client_event(&mut whole, turn_complete(None));
        let expected = take_assistant_text_rows(&mut whole, &mut BTreeSet::new(), 36, false);
        let row = expected.iter().find(|row| row.contains(name)).expect("table data row");
        assert!(row.contains(value), "cells must remain associated on their table row: {row}");
        assert!(!row.contains(&format!("{name} |")), "table delimiters must not render as source");
        assert!(expected.iter().any(|row| row.contains('─')), "table header separator");

        let chars = source.chars().collect::<Vec<_>>();
        let chunk_sizes =
            if chars.len() < 512 { [1, 17, chars.len()] } else { [113, 509, chars.len()] };
        for chunk_size in chunk_sizes {
            let mut app = connected_app().0;
            update(&mut app, model::SessionUpdate::AgentResponseStarted);
            let mut committed = Vec::new();
            let mut excluded = BTreeSet::new();
            for chunk in chars.chunks(chunk_size) {
                update(&mut app, text(&chunk.iter().collect::<String>()));
                committed.extend(take_assistant_text_rows(&mut app, &mut excluded, 36, true));
            }
            assert!(
                committed.iter().any(|row| row.contains("Introduction.")),
                "completed prose should advance into scrollback while the table streams"
            );
            handle_client_event(&mut app, turn_complete(None));
            committed.extend(take_assistant_text_rows(&mut app, &mut excluded, 36, false));
            assert_eq!(committed, expected, "chunk size {chunk_size}");
            assert_eq!(crate::app::copy::choices(&app)[0].text, source);
        }
    }
}

#[test]
fn list_rendering_survives_sdk_chunks_cache_splits_and_scrollback_commits() {
    let long_source = format!(
        "Introduction.\n\n9. Parent item.\n\n   {}\n\n   - Child with 日本語😀.\n10. Next item.\n\nFinal paragraph.",
        "Long continuation with useful detail. ".repeat(130)
    );
    for source in [long_source, "Introduction.\n\n9) Parent item.\n\n   - Child item.\n\n     Child continuation.\n10) Next item.\n\nFinal paragraph.".to_owned()] {
    let chars = source.chars().collect::<Vec<_>>();
    let mut whole = connected_app().0;
    whole.status = AppStatus::Ready;
    update(&mut whole, model::SessionUpdate::AgentResponseStarted);
    update(&mut whole, text(&source));
    handle_client_event(&mut whole, turn_complete(None));
    assert_eq!(
        crate::app::copy::choices(&whole)[0].text,
        source,
        "cache splits must preserve the original Markdown source for copying"
    );
    let expected = take_assistant_text_rows(&mut whole, &mut BTreeSet::new(), 36, false);
    assert!(
        expected.iter().any(|row| row.contains("Introduction.")),
        "fixture must deliver output through the session boundary"
    );

    let chunk_sizes = if chars.len() < 512 { [1, 17, chars.len()] } else { [17, 113, chars.len()] };
    for chunk_size in chunk_sizes {
        let mut app = connected_app().0;
        app.status = AppStatus::Ready;
        update(&mut app, model::SessionUpdate::AgentResponseStarted);
        let mut committed = Vec::new();
        let mut excluded = BTreeSet::new();
        let mut committed_intro_while_running = false;
        for chunk in chars.chunks(chunk_size) {
            update(&mut app, text(&chunk.iter().collect::<String>()));
            committed.extend(take_assistant_text_rows(&mut app, &mut excluded, 36, true));
            committed_intro_while_running |=
                committed.iter().any(|row| row.contains("Introduction."));
        }
        assert!(
            committed_intro_while_running,
            "ordinary completed paragraphs should still reach scrollback during a long list"
        );
        handle_client_event(&mut app, turn_complete(None));
        committed.extend(take_assistant_text_rows(&mut app, &mut excluded, 36, false));
        assert_eq!(
            crate::app::copy::choices(&app)[0].text,
            source,
            "SDK chunk size {chunk_size} must preserve the copied Markdown exactly"
        );
        assert_eq!(
            committed, expected,
            "SDK chunk size {chunk_size} changed the displayed list or duplicated committed rows"
        );
    }
    }
}
