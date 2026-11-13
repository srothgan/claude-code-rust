// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::agent::{client::CommandReceiver, events::ClientEvent, model, wire::BridgeCommand};
use crate::app::test_support::{
    app_with_connection, deliver_update, permission_options, press, request_permission,
};
use crossterm::event::KeyCode;

fn render(app: &mut App, width: u16, excluded: &BTreeSet<HistoryOutputId>) -> SerializedLiveRows {
    serialize_live_rows_with_boundaries_excluding(app, width, excluded, None)
}

fn target_rows(rows: &SerializedLiveRows, id: &HistoryOutputId) -> Vec<Line<'static>> {
    let selected: Vec<_> = rows.segments().iter().filter(|s| s.ids.contains(id)).collect();
    assert!(!selected.is_empty(), "missing source region {id:?}");
    selected.into_iter().flat_map(|s| rows.segment_rows(s).unwrap().iter().cloned()).collect()
}

fn visible(rows: &[Line<'_>]) -> String {
    rows.iter().map(|l| l.to_string().trim_end().to_owned()).collect::<Vec<_>>().join("\n")
}

fn assert_emphasis(rows: &[Line<'_>], needle: &str, bold: bool) {
    let row =
        rows.iter().find(|r| r.to_string().contains(needle)).expect("intended emphasis content");
    let source = row.to_string();
    let start = source.find(needle).unwrap();
    let end = start + needle.len();
    let mut offset = 0;
    let mut covered = 0;
    for span in &row.spans {
        let span_end = offset + span.content.len();
        let overlap = span_end.min(end).saturating_sub(offset.max(start));
        if overlap > 0 {
            covered += overlap;
            assert_eq!(
                row.style.patch(span.style).add_modifier.contains(Modifier::BOLD),
                bold,
                "style must belong to {needle}"
            );
        }
        offset = span_end;
    }
    assert_eq!(covered, needle.len());
}

fn assert_closed_emphasis(rows: &SerializedLiveRows, id: &HistoryOutputId) {
    let own = target_rows(rows, id);
    let contents: Vec<_> = own
        .iter()
        .map(ToString::to_string)
        .map(|s| s.trim().trim_start_matches('●').trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    assert_eq!(contents.join(" "), "BEFORE BOLD CORE AFTER");
    for word in ["BOLD", "CORE"] {
        assert_emphasis(&own, word, true);
    }
    for word in ["BEFORE", "AFTER"] {
        assert_emphasis(&own, word, false);
    }
}

#[test]
fn inline_emphasis_handoff_releases_only_closed_context_and_keeps_the_independent_tail() {
    let (mut app, _commands) = app_with_connection();
    let first = TextBlock::from_complete("BEFORE **BOLD");
    let first_id = HistoryOutputId::Block(first.id);
    let message = ChatMessage::new(MessageRole::Assistant, vec![MessageBlock::Text(first)], None);
    let label = HistoryOutputId::AssistantLabel(message.id);
    app.push_message_tracked(message);
    app.bind_active_turn_assistant(0);
    app.status = AppStatus::Running;
    for width in [32, 80] {
        let open = render(&mut app, width, &BTreeSet::new());
        assert!(visible(&target_rows(&open, &first_id)).contains("BEFORE **BOLD"));
        assert!(
            open.segments().iter().filter(|s| s.ids.contains(&first_id)).all(|s| !s.commit_ready)
        );
    }
    let suffix = TextBlock::from_complete(" CORE** AFTER");
    let suffix_id = HistoryOutputId::Block(suffix.id);
    app.transcript.messages[0].blocks.push(MessageBlock::Text(suffix));
    for width in [32, 80] {
        let closed = render(&mut app, width, &BTreeSet::new());
        for id in [&first_id, &suffix_id] {
            assert_closed_emphasis(&closed, id);
            assert!(
                closed.segments().iter().filter(|s| s.ids.contains(id)).all(|s| !s.commit_ready)
            );
        }
    }
    let tail = TextBlock::from_complete("TAIL_ORCHID");
    let tail_id = HistoryOutputId::Block(tail.id);
    app.transcript.messages[0].blocks.push(MessageBlock::Text(tail));
    for width in [32, 80] {
        let released = render(&mut app, width, &BTreeSet::new());
        for id in [&first_id, &suffix_id] {
            assert_closed_emphasis(&released, id);
            assert!(
                released
                    .segments()
                    .iter()
                    .filter(|s| s.ids.contains(id))
                    .all(|s| s.commit_ready && s.end_row <= released.stable_row_count())
            );
        }
        assert!(
            released
                .segments()
                .iter()
                .filter(|s| s.ids.contains(&tail_id))
                .all(|s| !s.commit_ready)
        );
        let excluded = BTreeSet::from([first_id.clone(), suffix_id.clone(), label.clone()]);
        let live = render(&mut app, width, &excluded);
        let own = target_rows(&live, &tail_id);
        assert!(visible(&own).contains("TAIL_ORCHID"));
        assert_emphasis(&own, "TAIL_ORCHID", false);
        assert!(!visible(live.rows()).contains("BEFORE"));
    }
    complete(&mut app);
    let final_rows = render(&mut app, 32, &BTreeSet::new());
    assert_eq!(final_rows.stable_row_count(), final_rows.rows().len());
    assert_closed_emphasis(&final_rows, &first_id);
    assert!(visible(&target_rows(&final_rows, &tail_id)).contains("TAIL_ORCHID"));
}

fn start_turn(app: &mut App, commands: &mut CommandReceiver, question: &str) -> usize {
    app.input.set_text(question);
    press(app, KeyCode::Enter);
    let mut prompts = 0;
    while let Ok(envelope) = commands.try_recv() {
        if let BridgeCommand::Prompt { session_id, chunks, .. } = envelope.command {
            assert_eq!(session_id, "session-1");
            assert_eq!(chunks[0].value, serde_json::json!(question));
            prompts += 1;
        }
    }
    assert_eq!(prompts, 1, "real submission must reach the bridge");
    app.active_turn_assistant_idx().expect("admitted response owner")
}

fn phase(app: &mut App, phase: model::AgentActivityPhase) {
    deliver_update(app, model::SessionUpdate::AgentActivityUpdate(phase));
}

fn chunk(app: &mut App, text: &str) {
    deliver_update(
        app,
        model::SessionUpdate::AgentMessageChunk(model::ContentChunk::new(
            model::ContentBlock::Text(model::TextContent::new(text)),
        )),
    );
}

fn complete(app: &mut App) {
    crate::app::handle_client_event(
        app,
        ClientEvent::TurnComplete {
            session_id: "session-1".into(),
            queued_turn_count: Some(0),
            terminal_reason: None,
        },
    );
}

#[track_caller]
fn assert_thinking(app: &mut App, owner: usize, expected: bool) {
    for width in [32, 80] {
        let rows = render(app, width, &BTreeSet::new());
        let own = rows
            .segments()
            .iter()
            .filter(|s| s.msg_idx == owner)
            .flat_map(|s| rows.segment_rows(s).unwrap().iter().cloned())
            .collect::<Vec<_>>();
        assert_eq!(visible(&own).contains("Thinking…"), expected, "owner rows: {}", visible(&own));
        if expected {
            assert_eq!(
                visible(&own).matches('●').count(),
                1,
                "thinking belongs to the current response"
            );
        }
        let thinking: Vec<_> = rows
            .segments()
            .iter()
            .filter(|s| s.msg_idx == owner && s.kind == LiveRowBoundaryKind::AssistantThinking)
            .collect();
        assert_eq!(!thinking.is_empty(), expected, "visibility follows delivered lifecycle events");
        for segment in thinking {
            assert!(!segment.commit_ready);
            assert!(segment.start_row >= rows.stable_row_count());
            assert!(visible(rows.segment_rows(segment).unwrap()).contains("Thinking…"));
        }
        assert!(!visible(&rows.rows()[..rows.stable_row_count()]).contains("Thinking…"));
    }
}

fn wait_and_compact(app: &mut App, owner: usize) {
    let request = model::RequestPermissionRequest::new(
        "session-1",
        model::ToolCallUpdate::new("tool-target", model::ToolCallUpdateFields::new()),
        permission_options("target"),
        None,
    );
    let mut response = request_permission(app, request);
    phase(app, model::AgentActivityPhase::Thinking);
    assert_thinking(app, owner, false);
    press(app, KeyCode::Enter);
    assert!(matches!(
        response.try_recv().expect("real permission response").outcome,
        model::RequestPermissionOutcome::Selected(_)
    ));
    deliver_update(
        app,
        model::SessionUpdate::ToolCallUpdate(model::ToolCallUpdate::new(
            "tool-target",
            model::ToolCallUpdateFields::new().status(model::ToolCallStatus::Completed),
        )),
    );
    phase(app, model::AgentActivityPhase::Thinking);
    assert_thinking(app, owner, true);
    deliver_update(app, model::SessionUpdate::CompactionUpdate(model::CompactionUpdate::Started));
    assert_thinking(app, owner, false);
    deliver_update(
        app,
        model::SessionUpdate::CompactionUpdate(model::CompactionUpdate::Finished {
            result: model::CompactionResult::Success,
            error_code: None,
            error: None,
        }),
    );
    phase(app, model::AgentActivityPhase::Working);
}

#[test]
fn thinking_follows_real_work_wait_compaction_completion_and_cancel_events() {
    for tips in [false, true] {
        let (mut app, mut commands) = app_with_connection();
        app.config.snapshot = Some(crate::agent::settings::SettingsSnapshot::test_value(
            "spinnerTipsEnabled",
            serde_json::json!(tips),
        ));
        let owner = start_turn(&mut app, &mut commands, "THINKING_QUESTION");
        let owner_id = app.transcript.messages[owner].id;
        for (event, expected) in [
            (model::AgentActivityPhase::Thinking, true),
            (model::AgentActivityPhase::Working, false),
            (model::AgentActivityPhase::Thinking, true),
        ] {
            phase(&mut app, event);
            assert_thinking(&mut app, owner, expected);
        }
        phase(&mut app, model::AgentActivityPhase::Working);
        assert_thinking(&mut app, owner, false);
        deliver_update(
            &mut app,
            model::SessionUpdate::ToolCall(
                model::ToolCall::new("tool-target", "TOOL_ORCHID")
                    .status(model::ToolCallStatus::InProgress)
                    .meta(serde_json::json!({"claudeCode":{"toolName":"Write"}})),
            ),
        );
        let tool = render(&mut app, 80, &BTreeSet::new());
        assert!(
            visible(&target_rows(&tool, &HistoryOutputId::ToolCall("tool-target".into())))
                .contains("TOOL_ORCHID")
        );
        assert_thinking(&mut app, owner, false);
        wait_and_compact(&mut app, owner);
        chunk(&mut app, "REAL_RESPONSE_ORCHID");
        assert_thinking(&mut app, owner, false);
        let rows = render(&mut app, 80, &BTreeSet::new());
        let own: Vec<_> = rows
            .segments()
            .iter()
            .filter(|s| s.msg_idx == owner)
            .flat_map(|s| rows.segment_rows(s).unwrap().iter().cloned())
            .collect();
        assert_eq!(
            visible(&own).matches('●').count(),
            1,
            "one speaker marker belongs to this response"
        );
        assert!(visible(&own).contains("REAL_RESPONSE_ORCHID"));
        let excluded = BTreeSet::from([HistoryOutputId::AssistantLabel(owner_id)]);
        let remaining = render(&mut app, 32, &excluded);
        let own: Vec<_> = remaining
            .segments()
            .iter()
            .filter(|s| s.msg_idx == owner)
            .flat_map(|s| remaining.segment_rows(s).unwrap().iter().cloned())
            .collect();
        assert!(!visible(&own).contains('●'));
        assert!(visible(&own).contains("REAL_RESPONSE_ORCHID"));
        complete(&mut app);
        let settled = visible(render(&mut app, 80, &BTreeSet::new()).rows());
        phase(&mut app, model::AgentActivityPhase::Thinking);
        assert_thinking(&mut app, owner, false);
        assert_eq!(visible(render(&mut app, 80, &BTreeSet::new()).rows()), settled);
        assert!(crate::ui::activity_rows::build_activity_rows(&app, 80, Instant::now()).is_empty());
        let next = start_turn(&mut app, &mut commands, "CANCEL_QUESTION");
        phase(&mut app, model::AgentActivityPhase::Thinking);
        assert_thinking(&mut app, next, true);
        press(&mut app, KeyCode::Esc);
        let mut cancelled = false;
        while let Ok(envelope) = commands.try_recv() {
            if let BridgeCommand::CancelTurn { session_id } = envelope.command {
                assert_eq!(session_id, "session-1");
                cancelled = true;
            }
        }
        assert!(cancelled, "suppression must follow an actual queued cancel command");
        assert_thinking(&mut app, next, false);
        complete(&mut app);
    }
}

#[test]
fn later_system_notice_cannot_steal_stream_chunks_or_advance_past_an_active_owner() {
    let (mut app, mut commands) = app_with_connection();
    let owner = start_turn(&mut app, &mut commands, "NOTICE_QUESTION");
    let owner_id = app.transcript.messages[owner].id;
    chunk(&mut app, "PREFIX_ORCHID");
    let notice_message = ChatMessage::new(
        MessageRole::System(Some(SystemSeverity::Warning)),
        vec![MessageBlock::Notice(NoticeBlock::new(
            SystemSeverity::Warning,
            "MID_TURN_NOTICE".into(),
        ))],
        None,
    );
    let notice = notice_message.id;
    app.push_message_tracked(notice_message);
    chunk(&mut app, " SUFFIX_JASMINE");
    assert_eq!(app.active_turn_assistant_idx(), Some(owner));
    assert_eq!(
        app.transcript.messages.iter().filter(|m| matches!(m.role, MessageRole::Assistant)).count(),
        1
    );
    assert_eq!(app.transcript.messages[owner].id, owner_id);
    for width in [40, 80] {
        let rows = render(&mut app, width, &BTreeSet::new());
        let assistant = rows
            .segments()
            .iter()
            .filter(|s| s.msg_idx == owner && s.kind == LiveRowBoundaryKind::AssistantText)
            .collect::<Vec<_>>();
        assert!(!assistant.is_empty());
        let text = assistant
            .iter()
            .flat_map(|s| rows.segment_rows(s).unwrap().iter().cloned())
            .collect::<Vec<_>>();
        assert!(visible(&text).contains("PREFIX_ORCHID SUFFIX_JASMINE"));
        let notice_id = HistoryOutputId::Message(notice);
        assert!(visible(&target_rows(&rows, &notice_id)).contains("MID_TURN_NOTICE"));
        let notice_segments =
            rows.segments().iter().filter(|s| s.ids.contains(&notice_id)).collect::<Vec<_>>();
        assert!(assistant.iter().all(|s| !s.commit_ready));
        assert!(notice_segments.iter().all(|n| assistant.iter().all(|a| a.end_row <= n.start_row)
            && n.start_row >= rows.stable_row_count()));
        assert!(!visible(&rows.rows()[..rows.stable_row_count()]).contains("MID_TURN_NOTICE"));
    }
    complete(&mut app);
    let completed = render(&mut app, 80, &BTreeSet::new());
    assert_eq!(completed.stable_row_count(), completed.rows().len());
    assert!(
        visible(&target_rows(&completed, &HistoryOutputId::Message(notice)))
            .contains("MID_TURN_NOTICE")
    );
}
