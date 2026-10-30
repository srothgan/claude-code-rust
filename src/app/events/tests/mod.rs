// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::agent::error_handling::TurnErrorClass;
use crate::agent::events::ClientEvent;
use crate::agent::events::ServiceStatusSeverity;
use crate::app::keymap::{
    KeyAction, KeyBinding, KeyBindingSource, KeyContext, KeySpec, ResolvedKeymap, TerminalAction,
};
use crate::app::slash::{SlashCandidate, SlashContext, SlashState};
use crate::app::{
    ChatRebuildKind, ComposerRenderState, FocusOwner, FocusTarget, FullscreenView,
    InlinePermission, InlineQuestion, LiveRegionRenderState, ReleaseReason, SurfaceMode,
    TerminalLifecycleState, TextBlockSpacing, ToolCallInfo, ToolCallScope, UsageSnapshot,
    UsageSourceKind, mention,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use pretty_assertions::assert_eq;
use std::rc::Rc;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

mod assistant_lifecycle;
mod client_events;
mod clipboard_paste;
mod notice_audit;
mod session_controls;
mod slash_input;

fn session_update(update: model::SessionUpdate) -> ClientEvent {
    ClientEvent::SessionUpdate { session_id: "test-session".to_owned(), update }
}

fn turn_complete(terminal_reason: Option<crate::agent::types::TerminalReason>) -> ClientEvent {
    ClientEvent::TurnComplete {
        session_id: "test-session".to_owned(),
        queued_turn_count: None,
        terminal_reason,
    }
}

#[test]
fn autonomous_response_owns_fresh_text_and_preserves_an_existing_live_owner() {
    let mut app = make_test_app();
    app.status = AppStatus::Ready;
    app.push_message_tracked(assistant_msg(vec![MessageBlock::Text(TextBlock::from_complete(
        "Already completed and inserted.",
    ))]));
    let finished_id = app.transcript.messages[0].id;
    handle_client_event(&mut app, session_update(model::SessionUpdate::AgentResponseStarted));
    assert_eq!(app.status, AppStatus::Running);
    assert_eq!(app.active_turn_assistant_idx(), Some(1));
    let active_id = app.transcript.messages[1].id;
    assert!(app.turn.activity.is_some());
    assistant_lifecycle::assert_owner_has_no_rows(&mut app, 1);
    for text in ["The", " background task finished completely."] {
        handle_client_event(
            &mut app,
            session_update(model::SessionUpdate::AgentMessageChunk(model::ContentChunk::new(
                model::ContentBlock::Text(model::TextContent::new(text)),
            ))),
        );
        // Further main response starts inside a live turn must keep its owner.
        handle_client_event(&mut app, session_update(model::SessionUpdate::AgentResponseStarted));
        assert_eq!(app.transcript.messages.len(), 2);
        assert_eq!(app.transcript.messages[1].id, active_id);
    }
    assert_eq!(app.transcript.messages[0].id, finished_id);
    let MessageBlock::Text(finished) = &app.transcript.messages[0].blocks[0] else {
        panic!("finished text");
    };
    assert_eq!(finished.text, "Already completed and inserted.");
    let MessageBlock::Text(reply) = &app.transcript.messages[1].blocks[0] else {
        panic!("completion reply");
    };
    assert_eq!(reply.text, "The background task finished completely.");
    handle_client_event(&mut app, turn_complete(None));
    assert_eq!(app.status, AppStatus::Ready);
    assert_eq!(app.active_turn_assistant_idx(), None);
    for phase in [model::AgentActivityPhase::Working, model::AgentActivityPhase::Thinking] {
        handle_client_event(
            &mut app,
            session_update(model::SessionUpdate::AgentActivityUpdate(phase)),
        );
    }
    assert_eq!(app.status, AppStatus::Ready);
    assert_eq!(app.active_turn_assistant_idx(), None);
    assert!(app.turn.activity.is_none());
    handle_client_event(
        &mut app,
        ClientEvent::TurnError {
            session_id: "test-session".to_owned(),
            message: "Fixture service unavailable".to_owned(),
            queued_turn_count: None,
            api_error_status: Some(503),
            terminal_reason: None,
        },
    );
    assert_eq!(app.status, AppStatus::Error);
    handle_client_event(&mut app, session_update(model::SessionUpdate::AgentResponseStarted));
    assert_eq!(app.status, AppStatus::Running);
    assert_eq!(app.active_turn_assistant_idx(), Some(app.transcript.messages.len() - 1));
}

fn slash_command_error(message: String) -> ClientEvent {
    ClientEvent::SlashCommandError { session_id: None, message }
}

fn tool_call(id: &str, status: model::ToolCallStatus) -> ToolCallInfo {
    crate::app::test_support::tool_call_info(id, status)
}

fn installed_plugin_entry(id: &str) -> crate::app::plugins::InstalledPluginEntry {
    crate::app::plugins::InstalledPluginEntry {
        id: id.to_owned(),
        version: None,
        scope: "user".to_owned(),
        enabled: true,
        installed_at: None,
        last_updated: None,
        project_path: None,
        mcp_server_names: Vec::new(),
    }
}

fn task_item(id: &str, subject: &str, status: model::TaskStatus) -> model::TaskItem {
    model::TaskItem {
        run_id: None,
        parent_task_id: None,
        task_id: id.to_owned(),
        subject: subject.to_owned(),
        description: None,
        active_form: None,
        status,
        owner: None,
        blocks: Vec::new(),
        blocked_by: Vec::new(),
        metadata: None,
        source_tool_call_id: None,
        source_tool_call_run_id: None,
    }
}

fn assistant_msg(blocks: Vec<MessageBlock>) -> ChatMessage {
    ChatMessage::new(MessageRole::Assistant, blocks, None)
}

fn append_tool_call_block(app: &mut App, tool_id: &str) -> (usize, usize) {
    app.transcript.messages.push(assistant_msg(vec![MessageBlock::ToolCall(Box::new(tool_call(
        tool_id,
        model::ToolCallStatus::InProgress,
    )))]));
    let msg_idx = app.transcript.messages.len().saturating_sub(1);
    app.index_tool_call(tool_id.into(), msg_idx, 0);
    (msg_idx, 0)
}

fn user_msg(text: &str) -> ChatMessage {
    ChatMessage::new(
        MessageRole::User,
        vec![MessageBlock::Text(TextBlock::from_complete(text))],
        None,
    )
}

fn source_text(text: &str, source_message_uuid: &str) -> MessageBlock {
    MessageBlock::Text(
        TextBlock::from_complete(text).with_source_message_uuid(Some(source_message_uuid)),
    )
}

fn transcript_retraction(
    message_uuids: Vec<&str>,
    reason: model::TranscriptRetractionReason,
) -> model::SessionUpdate {
    model::SessionUpdate::TranscriptRetraction(model::TranscriptRetraction {
        message_uuids: message_uuids.into_iter().map(str::to_owned).collect(),
        reason,
        request_id: None,
        trigger: None,
        direction: None,
        original_model: None,
        fallback_model: None,
        scope: None,
        api_refusal_category: None,
        api_refusal_explanation: None,
        content: None,
    })
}

#[derive(Debug, PartialEq, Eq)]
struct MessageSnapshot {
    role: MessageRole,
    blocks: Vec<BlockSnapshot>,
}

#[derive(Debug, PartialEq, Eq)]
enum BlockSnapshot {
    Text {
        text: String,
        trailing_spacing: TextBlockSpacing,
    },
    BtwExchange {
        question: String,
        answer: String,
    },
    Notice {
        severity: SystemSeverity,
        text: String,
    },
    ToolCall {
        id: String,
        title: String,
        status: model::ToolCallStatus,
        hidden: bool,
    },
    Welcome {
        version: String,
        subscription: String,
        cwd: String,
        session_id: String,
        tip_seed: u64,
    },
    ImageAttachment {
        count: usize,
    },
    UserDialog {
        request_id: String,
    },
}

fn message_snapshots(app: &App) -> Vec<MessageSnapshot> {
    app.transcript
        .messages
        .iter()
        .map(|message| MessageSnapshot {
            role: message.role.clone(),
            blocks: message.blocks.iter().map(block_snapshot).collect(),
        })
        .collect()
}

fn block_snapshot(block: &MessageBlock) -> BlockSnapshot {
    match block {
        MessageBlock::Text(block) => BlockSnapshot::Text {
            text: block.text.clone(),
            trailing_spacing: block.trailing_spacing,
        },
        MessageBlock::BtwExchange(block) => BlockSnapshot::BtwExchange {
            question: block.question.clone(),
            answer: block.answer.clone(),
        },
        MessageBlock::Notice(block) => {
            BlockSnapshot::Notice { severity: block.severity, text: block.text.text.clone() }
        }
        MessageBlock::ToolCall(tool_call) | MessageBlock::ToolResult { tool: tool_call, .. } => {
            BlockSnapshot::ToolCall {
                id: tool_call.id.clone(),
                title: tool_call.title.clone(),
                status: tool_call.status,
                hidden: tool_call.is_subagent_child(),
            }
        }
        MessageBlock::Welcome(block) => BlockSnapshot::Welcome {
            version: block.version.clone(),
            subscription: block.subscription.clone(),
            cwd: block.cwd.clone(),
            session_id: block.session_id.clone(),
            tip_seed: block.tip_seed,
        },
        MessageBlock::ImageAttachment(block) => {
            BlockSnapshot::ImageAttachment { count: block.count }
        }
        MessageBlock::UserDialog(block) => {
            BlockSnapshot::UserDialog { request_id: block.request_id.clone() }
        }
    }
}

fn seed_resize_measurements(app: &mut App) {
    app.chat_render.terminal_width = 90;
    app.chat_render.terminal_height = 30;
    app.chat_render.composer = ComposerRenderState {
        activity_rows: 0,
        width: 90,
        hint_rows: 1,
        btw_rows: 0,
        editor_rows: 2,
        footer_rows: 1,
        total_rows: 4,
        caret_row: 1,
        caret_col: 3,
        last_rendered_rows: 4,
    };
    app.chat_render.live_region = LiveRegionRenderState {
        anchor_valid: true,
        total_rows: 12,
        hidden_rows_above: 3,
        viewport_height: 9,
        last_rendered_rows: 7,
    };
}

fn assert_resize_measurements_cleared(app: &App, width: u16, height: u16) {
    assert_eq!(app.chat_render.terminal_width, width);
    assert_eq!(app.chat_render.terminal_height, height);
    assert_eq!(app.chat_render.composer, ComposerRenderState::default());
    assert!(!app.chat_render.live_region.anchor_valid);
    assert_eq!(app.chat_render.live_region.total_rows, 0);
    assert_eq!(app.chat_render.live_region.hidden_rows_above, 0);
    assert_eq!(app.chat_render.live_region.viewport_height, 0);
    assert_eq!(app.chat_render.live_region.last_rendered_rows, 0);
}

fn assert_seed_resize_measurements_preserved(app: &App) {
    assert_eq!(app.chat_render.terminal_width, 90);
    assert_eq!(app.chat_render.terminal_height, 30);
    assert_eq!(app.chat_render.composer.width, 90);
    assert_eq!(app.chat_render.composer.total_rows, 4);
    assert!(app.chat_render.live_region.anchor_valid);
    assert_eq!(app.chat_render.live_region.total_rows, 12);
    assert_eq!(app.chat_render.live_region.hidden_rows_above, 3);
    assert_eq!(app.chat_render.live_region.viewport_height, 9);
    assert_eq!(app.chat_render.live_region.last_rendered_rows, 7);
}

#[test]
fn transcript_retraction_removes_matching_text_blocks_only() {
    let mut app = make_test_app();
    app.transcript.messages.push(assistant_msg(vec![
        source_text("stale", "old-assistant"),
        source_text("keep", "new-assistant"),
    ]));

    handle_client_event(
        &mut app,
        session_update(transcript_retraction(
            vec!["old-assistant", "old-assistant", "unknown"],
            model::TranscriptRetractionReason::ModelRefusalFallback,
        )),
    );

    assert_eq!(app.transcript.messages.len(), 1);
    assert_eq!(app.transcript.messages[0].blocks.len(), 1);
    let MessageBlock::Text(block) = &app.transcript.messages[0].blocks[0] else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "keep");
    assert!(block.has_source_message_uuid("new-assistant"));
}

#[test]
fn transcript_retraction_removes_tool_blocks_and_rebuilds_indices() {
    let mut app = make_test_app();
    let mut stale_tool = tool_call("tool-old", model::ToolCallStatus::Completed);
    stale_tool.source_message_uuids = vec!["assistant-tool".to_owned(), "user-result".to_owned()];
    stale_tool.sdk_tool_name = "Bash".to_owned();
    stale_tool.kind = model::ToolKind::Execute;
    stale_tool.terminal_id = Some("term-old".to_owned());
    app.transcript.messages.push(assistant_msg(vec![
        MessageBlock::ToolCall(Box::new(stale_tool)),
        source_text("replacement", "assistant-new"),
    ]));
    app.index_tool_call("tool-old".to_owned(), 0, 0);

    handle_client_event(
        &mut app,
        session_update(transcript_retraction(
            vec!["user-result"],
            model::TranscriptRetractionReason::ModelFallback,
        )),
    );

    assert!(app.lookup_tool_call("tool-old").is_none());
    assert_eq!(app.transcript.messages.len(), 1);
    assert_eq!(app.transcript.messages[0].blocks.len(), 1);
    let MessageBlock::Text(block) = &app.transcript.messages[0].blocks[0] else {
        panic!("expected replacement text");
    };
    assert_eq!(block.text, "replacement");
}

#[test]
fn transcript_retraction_then_replacement_leaves_canonical_assistant_content() {
    let mut app = make_test_app();
    app.transcript.messages.push(assistant_msg(vec![source_text("stale", "assistant-old")]));

    handle_client_event(
        &mut app,
        session_update(transcript_retraction(
            vec!["assistant-old"],
            model::TranscriptRetractionReason::AssistantSupersedes,
        )),
    );
    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::AgentMessageChunk(
            model::ContentChunk::new(model::ContentBlock::Text(model::TextContent::new(
                "replacement",
            )))
            .source_message_uuid(Some("assistant-new".to_owned())),
        )),
    );

    assert_eq!(app.transcript.messages.len(), 1);
    let MessageBlock::Text(block) = &app.transcript.messages[0].blocks[0] else {
        panic!("expected replacement text");
    };
    assert_eq!(block.text, "replacement");
    assert!(block.has_source_message_uuid("assistant-new"));
}

fn first_block_text(msg: &ChatMessage) -> &str {
    match msg.blocks.first() {
        Some(MessageBlock::Text(block)) => &block.text,
        Some(MessageBlock::Notice(block)) => &block.text.text,
        Some(MessageBlock::BtwExchange(_)) => {
            panic!("expected text-like block, found BTW exchange")
        }
        Some(MessageBlock::ToolCall(_) | MessageBlock::ToolResult { .. }) => {
            panic!("expected text-like block, found tool call")
        }
        Some(MessageBlock::Welcome(_)) => panic!("expected text-like block, found welcome"),
        Some(MessageBlock::ImageAttachment(_)) => {
            panic!("expected text-like block, found image attachment")
        }
        Some(MessageBlock::UserDialog(_)) => {
            panic!("expected text-like block, found user dialog")
        }
        None => panic!("expected message block"),
    }
}

#[test]
fn tool_metadata_updates_share_scope_with_live_and_historical_presentation() {
    for name in ["Task", "Agent"] {
        let mut app = make_test_app();
        handle_client_event(
            &mut app,
            session_update(model::SessionUpdate::ToolCall(
                model::ToolCall::new("tool", "Research")
                    .kind(model::ToolKind::Think)
                    .meta(serde_json::json!({"claudeCode": {"toolName": "Unknown"}})),
            )),
        );
        assert_eq!(app.tool_call_scope("tool"), Some(ToolCallScope::MainAgent));
        handle_client_event(
            &mut app,
            session_update(model::SessionUpdate::ToolCallUpdate(
                model::ToolCallUpdate::new("tool", model::ToolCallUpdateFields::new())
                    .meta(serde_json::json!({"claudeCode": {"toolName": name}})),
            )),
        );
        assert_eq!(app.tool_call_scope("tool"), Some(ToolCallScope::SubagentRoot));
        assert!(app.tool_call("tool").unwrap().is_subagent_root_tool());
        for parent in ["parent-1", "parent-2"] {
            handle_client_event(
                &mut app,
                session_update(model::SessionUpdate::ToolCallUpdate(
                    model::ToolCallUpdate::new(
                        "tool",
                        model::ToolCallUpdateFields::new().status(model::ToolCallStatus::Detached),
                    )
                    .meta(serde_json::json!({"claudeCode": {"parentToolUseId": parent}})),
                )),
            );
            let tool = app.tool_call("tool").unwrap();
            assert_eq!(
                app.tool_call_scope("tool"),
                Some(ToolCallScope::SubagentChild { parent_tool_use_id: parent.to_owned() })
            );
            assert!(!tool.is_subagent_root_tool());
            assert!(tool.hidden_unless_focused_interaction());
            assert_eq!(tool.display().parent_tool_use_id.as_deref(), Some("parent-1"));
            assert_eq!(tool.display().sdk_tool_name, name);
        }
        handle_client_event(
            &mut app,
            session_update(model::SessionUpdate::ToolCallUpdate(model::ToolCallUpdate::new(
                "tool",
                model::ToolCallUpdateFields::new().status(model::ToolCallStatus::Completed),
            ))),
        );
        assert_eq!(app.tool_call("tool").unwrap().parent_tool_use_id.as_deref(), Some("parent-2"));
        let result = app
            .transcript
            .messages
            .iter()
            .flat_map(|message| &message.blocks)
            .find_map(|block| match block {
                MessageBlock::ToolResult { tool, .. } if tool.id == "tool" => Some(tool),
                _ => None,
            })
            .expect("detached completion in transcript");
        assert_eq!(result.parent_tool_use_id.as_deref(), Some("parent-2"));
        assert!(!result.is_subagent_root_tool());
    }
}

#[test]
fn execute_kind_owns_output_handling_across_initial_and_partial_events() {
    for (name, kind, expected) in [
        ("CustomExecutor", model::ToolKind::Execute, Some("initial")),
        ("Bash", model::ToolKind::Other, None),
        ("bash", model::ToolKind::Other, None),
    ] {
        let mut app = make_test_app();
        handle_client_event(
            &mut app,
            session_update(model::SessionUpdate::ToolCall(
                model::ToolCall::new("tool", "Command")
                    .kind(kind)
                    .raw_output(serde_json::json!("initial"))
                    .meta(serde_json::json!({"claudeCode": {"toolName": name}})),
            )),
        );
        assert_eq!(app.tool_call("tool").unwrap().terminal_output.as_deref(), expected);
        handle_client_event(
            &mut app,
            session_update(model::SessionUpdate::ToolCallUpdate(model::ToolCallUpdate::new(
                "tool",
                model::ToolCallUpdateFields::new()
                    .kind(model::ToolKind::Execute)
                    .raw_output(serde_json::json!("updated")),
            ))),
        );
        assert_eq!(app.tool_call("tool").unwrap().terminal_output.as_deref(), Some("updated"));
        handle_client_event(
            &mut app,
            session_update(model::SessionUpdate::ToolCallUpdate(
                model::ToolCallUpdate::new(
                    "tool",
                    model::ToolCallUpdateFields::new()
                        .status(model::ToolCallStatus::Detached)
                        .raw_output(serde_json::json!("final")),
                )
                .meta(serde_json::json!({"claudeCode": {"toolName": "RenamedExecutor"}})),
            )),
        );
        let tool = app.tool_call("tool").unwrap();
        assert_eq!(tool.terminal_output.as_deref(), Some("final"));
        assert!(tool.is_execute_tool());
        assert!(tool.display().is_execute_tool());
        assert_eq!(tool.display().terminal_output.as_deref(), Some("final"));
    }
}

/// A cancelled subagent must not change the scope of the next foreground tool.
#[test]
fn turn_complete_after_cancelled_task_preserves_next_tool_scope() {
    let mut app = make_test_app();

    // Simulate a Task tool call arriving as InProgress (no Completed update will follow)
    let task_tc = model::ToolCall::new("task-1", "Research")
        .kind(model::ToolKind::Think)
        .status(model::ToolCallStatus::InProgress)
        .meta(serde_json::json!({"claudeCode": {"toolName": "Task"}}));
    handle_client_event(&mut app, session_update(model::SessionUpdate::ToolCall(task_tc)));

    // User cancels then TurnComplete finalizes the turn
    handle_local_cancel_enqueued(&mut app);
    handle_client_event(&mut app, turn_complete(None));

    // Next turn: a normal main-agent Glob must get MainAgent scope, not Subagent
    let glob_tc = model::ToolCall::new("glob-1", "Glob **/*.rs")
        .kind(model::ToolKind::Search)
        .status(model::ToolCallStatus::InProgress)
        .meta(serde_json::json!({"claudeCode": {"toolName": "Glob"}}));
    handle_client_event(&mut app, session_update(model::SessionUpdate::ToolCall(glob_tc)));
    assert_eq!(
        app.tool_call_scope("glob-1"),
        Some(ToolCallScope::MainAgent),
        "main-agent tool must not be misclassified as Subagent after stale task is cleared"
    );
}

#[test]
fn streaming_long_markdown_table_does_not_leave_raw_pipe_row_tail() {
    let mut rows = String::new();
    for idx in 0..70 {
        let _ = writeln!(
            rows,
            "| Slow startup {idx} | Startup output lands after a long delay once context is large. | https://example.com/issues/{idx}/very/long/reference |"
        );
    }
    let table = format!("| Hassle | What users report | Refs |\n| --- | --- | --- |\n{rows}");
    assert!(table.len() > crate::app::DEFAULT_CACHE_SPLIT_SOFT_LIMIT_BYTES);
    let mut app = make_test_app();
    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::RuntimeSessionStateUpdate(
            model::RuntimeSessionState::Running,
        )),
    );

    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::AgentMessageChunk(model::ContentChunk::new(
            model::ContentBlock::Text(model::TextContent::new(&table)),
        ))),
    );

    assert!(matches!(app.status, AppStatus::Running));
    let serialized = crate::ui::inline_chat_rows::serialize_live_rows_with_boundaries_excluding(
        &mut app,
        180,
        &std::collections::BTreeSet::new(),
        None,
    );
    let committed_ids = serialized
        .segments()
        .iter()
        .filter(|segment| segment.commit_ready)
        .flat_map(|segment| segment.ids.iter().cloned())
        .collect::<std::collections::BTreeSet<_>>();
    let remaining = crate::ui::inline_chat_rows::serialize_live_rows_with_boundaries_excluding(
        &mut app,
        180,
        &committed_ids,
        None,
    );
    let remaining_text = remaining
        .rows()
        .iter()
        .map(|row| row.spans.iter().map(|span| span.content.as_ref()).collect::<String>())
        .collect::<Vec<_>>();

    assert!(
        remaining_text.iter().any(|line| line.contains("Slow startup")),
        "expected table rows to remain visible: {remaining_text:?}"
    );
    assert!(
        !remaining_text.iter().any(|line| line.trim_start().starts_with("| Slow startup |")),
        "live tail must not render raw Markdown table rows: {remaining_text:?}"
    );
}

fn make_test_app() -> App {
    let mut app = App::test_default();
    app.session_runtime.session_id = Some(model::SessionId::new("test-session"));
    app
}

fn test_current_model(model_name: &str) -> model::CurrentModel {
    model::CurrentModel::new(model_name, model_name, model_name).authoritative(true)
}

fn canonical_messages_contain_text(app: &App, expected: &str) -> bool {
    app.transcript.messages.iter().any(|message| {
        message.blocks.iter().any(|block| match block {
            MessageBlock::Text(text) => text.text == expected,
            MessageBlock::Notice(notice) => notice.text.text == expected,
            MessageBlock::BtwExchange(exchange) => {
                exchange.question == expected || exchange.answer == expected
            }
            MessageBlock::ToolCall(_)
            | MessageBlock::ToolResult { .. }
            | MessageBlock::Welcome(_)
            | MessageBlock::ImageAttachment(_)
            | MessageBlock::UserDialog(_) => false,
        })
    })
}

fn live_rows_contain_text(app: &mut App, expected: &str) -> bool {
    crate::ui::inline_chat_rows::serialize_live_rows_with_boundaries_excluding(
        app,
        120,
        &std::collections::BTreeSet::new(),
        None,
    )
    .rows()
    .iter()
    .any(|row| row.spans.iter().any(|span| span.content.as_ref().contains(expected)))
}

fn session_overview_has_welcome(app: &App) -> bool {
    app.show_session_overview
        && app
            .transcript
            .messages
            .iter()
            .any(|message| matches!(message.role, MessageRole::Welcome))
}

fn connected_event(model_name: &str) -> ClientEvent {
    ClientEvent::Connected {
        session_id: model::SessionId::new("test-session"),
        cwd: "/test".into(),
        current_model: test_current_model(model_name),
        available_models: Vec::new(),
        mode: None,
        fast_mode_state: model::FastModeState::Off,
        fast_mode_disabled_reason: None,
        ultracode: None,
        history_updates: Vec::new(),
    }
}

fn app_with_bridge_connection() -> (App, crate::agent::client::CommandReceiver) {
    let mut app = make_test_app();
    let (connection, rx) = crate::agent::client::AgentConnection::test_channel();
    app.session_runtime.conn = Some(Rc::new(connection));
    (app, rx)
}

fn listed_session(id: &str, title: &str) -> crate::agent::types::SessionListEntry {
    crate::agent::types::SessionListEntry {
        session_id: id.to_owned(),
        summary: title.to_owned(),
        last_modified_ms: 1,
        file_size_bytes: 2,
        cwd: Some("/test".to_owned()),
        git_branch: Some("main".to_owned()),
        custom_title: Some(title.to_owned()),
        first_prompt: Some(format!("prompt {title}")),
    }
}

#[test]
fn raw_output_string_maps_to_terminal_text() {
    let raw = serde_json::json!("hello\nworld");
    assert_eq!(tool_updates::raw_output_to_terminal_text(&raw).as_deref(), Some("hello\nworld"));
}

#[test]
fn raw_output_text_array_maps_to_terminal_text() {
    let raw = serde_json::json!([
        {"type": "text", "text": "first"},
        {"type": "text", "text": "second"}
    ]);
    assert_eq!(tool_updates::raw_output_to_terminal_text(&raw).as_deref(), Some("first\nsecond"));
}

#[test]
fn execute_tool_update_uses_raw_output_fallback() {
    let mut app = make_test_app();
    let tc = model::ToolCall::new("tc-exec", "Terminal")
        .kind(model::ToolKind::Execute)
        .status(model::ToolCallStatus::InProgress);
    handle_client_event(&mut app, session_update(model::SessionUpdate::ToolCall(tc)));

    let fields = model::ToolCallUpdateFields::new()
        .status(model::ToolCallStatus::Completed)
        .raw_output(serde_json::json!("line 1\nline 2"));
    let update = model::ToolCallUpdate::new("tc-exec", fields);
    handle_client_event(&mut app, session_update(model::SessionUpdate::ToolCallUpdate(update)));

    let Some((mi, bi)) = app.lookup_tool_call("tc-exec") else {
        panic!("tool call not indexed");
    };
    let Some(MessageBlock::ToolCall(tc)) =
        app.transcript.messages.get(mi).and_then(|m| m.blocks.get(bi))
    else {
        panic!("tool call block missing");
    };
    assert_eq!(tc.terminal_output.as_deref(), Some("line 1\nline 2"));
}

#[test]
fn powershell_raw_input_update_does_not_populate_terminal_command() {
    let mut app = make_test_app();
    let tc = model::ToolCall::new("tc-pwsh", "Terminal")
        .kind(model::ToolKind::Execute)
        .status(model::ToolCallStatus::InProgress)
        .meta(serde_json::json!({"claudeCode": {"toolName": "PowerShell"}}));
    handle_client_event(&mut app, session_update(model::SessionUpdate::ToolCall(tc)));

    let fields = model::ToolCallUpdateFields::new()
        .raw_input(serde_json::json!({ "command": "Get-ChildItem" }));
    let update = model::ToolCallUpdate::new("tc-pwsh", fields);
    handle_client_event(&mut app, session_update(model::SessionUpdate::ToolCallUpdate(update)));

    let Some((mi, bi)) = app.lookup_tool_call("tc-pwsh") else {
        panic!("tool call not indexed");
    };
    let Some(MessageBlock::ToolCall(tc)) =
        app.transcript.messages.get(mi).and_then(|m| m.blocks.get(bi))
    else {
        panic!("tool call block missing");
    };
    assert_eq!(tc.sdk_tool_name, "PowerShell");
    assert_eq!(tc.terminal_command, None);
    assert_eq!(tc.raw_input, Some(serde_json::json!({ "command": "Get-ChildItem" })));
}

#[test]
fn late_tool_update_for_removed_tool_does_not_restore_scope_or_transcript() {
    let mut app = make_test_app();
    app.transcript.messages.push(assistant_msg(vec![MessageBlock::ToolCall(Box::new(tool_call(
        "tool-stale",
        model::ToolCallStatus::Completed,
    )))]));
    app.index_tool_call("tool-stale".into(), 0, 0);
    crate::app::test_support::set_tool_scope(
        &mut app,
        "tool-stale",
        ToolCallScope::SubagentChild { parent_tool_use_id: "task-1".to_owned() },
    );

    let removed = app.remove_message_tracked(0);
    assert!(removed.is_some());
    assert_eq!(app.tool_call_scope("tool-stale"), None);

    let update = model::ToolCallUpdate::new(
        "tool-stale",
        model::ToolCallUpdateFields::new().status(model::ToolCallStatus::InProgress),
    )
    .meta(serde_json::json!({"claudeCode": {"parentToolUseId": "task-1"}}));
    handle_client_event(&mut app, session_update(model::SessionUpdate::ToolCallUpdate(update)));
    assert!(app.tool_call_scope("tool-stale").is_none());
    assert!(!app.has_tool_call("tool-stale"));
    assert!(app.transcript.messages.is_empty());
}

#[test]
fn repeated_tool_call_updates_existing_execute_snapshot_state() {
    let mut app = make_test_app();
    let first = model::ToolCall::new("tc-dup", "Terminal")
        .kind(model::ToolKind::Execute)
        .status(model::ToolCallStatus::InProgress)
        .content(vec![model::ToolCallContent::Terminal(model::TerminalToolCallContent::new(
            "term-1",
        ))])
        .raw_output(serde_json::json!("first"));
    handle_client_event(&mut app, session_update(model::SessionUpdate::ToolCall(first)));

    let second = model::ToolCall::new("tc-dup", "Terminal")
        .kind(model::ToolKind::Execute)
        .status(model::ToolCallStatus::InProgress)
        .content(vec![model::ToolCallContent::Terminal(model::TerminalToolCallContent::new(
            "term-2",
        ))])
        .raw_output(serde_json::json!("second"));
    handle_client_event(&mut app, session_update(model::SessionUpdate::ToolCall(second)));

    let (mi, bi) = app.lookup_tool_call("tc-dup").expect("tool call not indexed");
    let MessageBlock::ToolCall(tc) = &app.transcript.messages[mi].blocks[bi] else {
        panic!("expected tool call block");
    };
    assert_eq!(tc.terminal_output.as_deref(), Some("second"));
    assert_eq!(tc.terminal_id.as_deref(), Some("term-2"));
    assert_eq!(tc.terminal_command, None);
}

#[test]
fn todowrite_tool_call_does_not_mutate_task_state() {
    let mut app = make_test_app();
    app.sdk_inventory.tasks.push(task_item(
        "task-1",
        "Existing task",
        model::TaskStatus::InProgress,
    ));
    let todo_call = model::ToolCall::new("tc-todo-update", "TodoWrite")
        .kind(model::ToolKind::Other)
        .raw_input(serde_json::json!({
            "todos": [{"content": "Task A", "status": "in_progress"}]
        }))
        .meta(serde_json::json!({"claudeCode": {"toolName": "TodoWrite"}}));
    handle_client_event(&mut app, session_update(model::SessionUpdate::ToolCall(todo_call)));

    let update = model::ToolCallUpdate::new(
        "tc-todo-update",
        model::ToolCallUpdateFields::new().raw_input(serde_json::json!({})),
    );
    handle_client_event(&mut app, session_update(model::SessionUpdate::ToolCallUpdate(update)));

    assert_eq!(app.sdk_inventory.tasks.len(), 1);
    assert_eq!(app.sdk_inventory.tasks[0].task_id, "task-1");
    assert_eq!(app.sdk_inventory.tasks[0].subject, "Existing task");
    assert_eq!(app.sdk_inventory.tasks[0].status, model::TaskStatus::InProgress);
}

#[test]
fn resize_marks_chat_surface_dirty_when_running_chat() {
    let mut app = make_test_app();
    app.surface_dirty = crate::app::SurfaceDirtyState::default();
    app.terminal_lifecycle = TerminalLifecycleState::Running(SurfaceMode::Chat);
    app.session_runtime.prompt_suggestion = Some("Write focused tests".to_owned());
    seed_resize_measurements(&mut app);

    handle_terminal_event(&mut app, Event::Resize(120, 40));

    assert!(!app.surface_dirty.fullscreen.redraw);
    assert_eq!(
        app.surface_dirty.chat.rebuild,
        ChatRebuildKind::PurgeReplay(crate::app::ChatPurgeReplayOptions::resize())
    );
    assert!(app.surface_dirty.chat.repaint);
    assert_eq!(app.session_runtime.prompt_suggestion.as_deref(), Some("Write focused tests"));
    assert_resize_measurements_cleared(&app, 120, 40);
}

#[test]
fn same_size_resize_does_not_request_chat_purge() {
    let mut app = make_test_app();
    app.surface_dirty = crate::app::SurfaceDirtyState::default();
    app.terminal_lifecycle = TerminalLifecycleState::Running(SurfaceMode::Chat);
    seed_resize_measurements(&mut app);

    handle_terminal_event(&mut app, Event::Resize(90, 30));

    assert!(!app.surface_dirty.fullscreen.redraw);
    assert_eq!(app.surface_dirty.chat.rebuild, ChatRebuildKind::None);
    assert!(!app.surface_dirty.chat.repaint);
    assert_seed_resize_measurements_preserved(&app);
}

#[test]
fn reconciling_an_unreported_size_requests_the_same_chat_purge_as_a_resize_event() {
    let mut app = make_test_app();
    app.surface_dirty = crate::app::SurfaceDirtyState::default();
    app.terminal_lifecycle = TerminalLifecycleState::Running(SurfaceMode::Chat);
    seed_resize_measurements(&mut app);

    reconcile_terminal_size(&mut app, 90, 55);

    assert_eq!(
        app.surface_dirty.chat.rebuild,
        ChatRebuildKind::PurgeReplay(crate::app::ChatPurgeReplayOptions::resize())
    );
    assert!(app.surface_dirty.chat.repaint);
    assert_resize_measurements_cleared(&app, 90, 55);
}

#[test]
fn reconciling_the_recorded_size_changes_nothing() {
    let mut app = make_test_app();
    app.surface_dirty = crate::app::SurfaceDirtyState::default();
    app.terminal_lifecycle = TerminalLifecycleState::Running(SurfaceMode::Chat);
    seed_resize_measurements(&mut app);

    reconcile_terminal_size(&mut app, 90, 30);

    assert_eq!(app.surface_dirty.chat.rebuild, ChatRebuildKind::None);
    assert!(!app.surface_dirty.chat.repaint);
    assert_seed_resize_measurements_preserved(&app);
}

#[test]
fn resize_marks_fullscreen_surface_dirty_when_running_fullscreen() {
    let mut app = make_test_app();
    app.surface_dirty = crate::app::SurfaceDirtyState::default();
    app.terminal_lifecycle =
        TerminalLifecycleState::Running(SurfaceMode::Fullscreen(FullscreenView::Config));
    app.session_runtime.prompt_suggestion = Some("Write focused tests".to_owned());
    seed_resize_measurements(&mut app);

    handle_terminal_event(&mut app, Event::Resize(120, 40));

    assert!(app.surface_dirty.fullscreen.redraw);
    assert_eq!(app.surface_dirty.chat.rebuild, ChatRebuildKind::None);
    assert!(!app.surface_dirty.chat.repaint);
    assert!(app.chat_render.resize_purge_replay_on_chat_return);
    assert_eq!(app.session_runtime.prompt_suggestion.as_deref(), Some("Write focused tests"));
    assert_resize_measurements_cleared(&app, 120, 40);
}

#[test]
fn same_size_resize_while_fullscreen_does_not_defer_chat_purge() {
    let mut app = make_test_app();
    app.surface_dirty = crate::app::SurfaceDirtyState::default();
    app.terminal_lifecycle =
        TerminalLifecycleState::Running(SurfaceMode::Fullscreen(FullscreenView::Config));
    seed_resize_measurements(&mut app);

    handle_terminal_event(&mut app, Event::Resize(90, 30));

    assert!(!app.surface_dirty.fullscreen.redraw);
    assert_eq!(app.surface_dirty.chat.rebuild, ChatRebuildKind::None);
    assert!(!app.surface_dirty.chat.repaint);
    assert!(!app.chat_render.resize_purge_replay_on_chat_return);
    assert_seed_resize_measurements_preserved(&app);
}

#[test]
fn resize_while_released_to_child_stores_size_without_drawing_hidden_chat() {
    let mut app = make_test_app();
    app.surface_dirty = crate::app::SurfaceDirtyState::default();
    app.terminal_lifecycle = TerminalLifecycleState::ReleasedToChild(ReleaseReason::AuthFlow);
    seed_resize_measurements(&mut app);

    handle_terminal_event(&mut app, Event::Resize(120, 40));

    assert!(!app.surface_dirty.fullscreen.redraw);
    assert_eq!(app.surface_dirty.chat.rebuild, ChatRebuildKind::None);
    assert!(!app.surface_dirty.chat.repaint);
    assert_resize_measurements_cleared(&app, 120, 40);
}

#[test]
fn resize_does_not_mutate_messages() {
    let mut app = make_test_app();
    app.terminal_lifecycle = TerminalLifecycleState::Running(SurfaceMode::Chat);
    app.transcript.messages.push(user_msg("hello"));
    app.transcript.messages.push(assistant_msg(vec![
        MessageBlock::Text(TextBlock::from_complete("answer")),
        MessageBlock::ToolCall(Box::new(tool_call("tc-resize", model::ToolCallStatus::InProgress))),
    ]));
    let before = message_snapshots(&app);

    handle_terminal_event(&mut app, Event::Resize(120, 40));

    assert_eq!(message_snapshots(&app), before);
}

#[test]
fn resize_does_not_clear_active_assistant_ownership() {
    let mut app = make_test_app();
    app.terminal_lifecycle = TerminalLifecycleState::Running(SurfaceMode::Chat);
    app.transcript.messages.push(user_msg("hello"));
    app.transcript.messages.push(assistant_msg(vec![MessageBlock::Text(
        TextBlock::from_complete("streaming answer"),
    )]));
    app.bind_active_turn_assistant(1);

    handle_terminal_event(&mut app, Event::Resize(120, 40));

    assert_eq!(app.active_turn_assistant_idx(), Some(1));
}

#[test]
fn resize_during_active_turn_marks_final_purge_replay_needed() {
    let mut app = make_test_app();
    app.surface_dirty = crate::app::SurfaceDirtyState::default();
    app.terminal_lifecycle = TerminalLifecycleState::Running(SurfaceMode::Chat);
    app.status = AppStatus::Running;
    seed_resize_measurements(&mut app);

    handle_terminal_event(&mut app, Event::Resize(120, 40));

    assert_eq!(
        app.surface_dirty.chat.rebuild,
        ChatRebuildKind::PurgeReplay(crate::app::ChatPurgeReplayOptions::resize())
    );
    assert!(app.chat_render.resize_purge_replay_after_turn);
}

#[test]
fn turn_complete_after_cancel_renders_interrupted_hint() {
    for cancelled in [true, false] {
        let mut app = make_test_app();
        app.status = AppStatus::Running;
        app.transcript.messages.push(user_msg("build the feature"));
        app.transcript.messages.push(assistant_msg(vec![MessageBlock::Text(
            TextBlock::from_complete("partial answer"),
        )]));
        app.bind_active_turn_assistant(1);
        let history = message_snapshots(&app);
        if cancelled {
            handle_local_cancel_enqueued(&mut app);
            assert!(app.turn.cancel_requested);
        }

        handle_client_event(&mut app, turn_complete(None));

        assert!(!app.turn.cancel_requested);
        assert_eq!(&message_snapshots(&app)[..history.len()], history);
        let appended = &app.transcript.messages[history.len()..];
        if cancelled {
            let [notice] = appended else {
                panic!("cancelled response must append exactly one interruption notice");
            };
            assert!(matches!(notice.role, MessageRole::System(Some(SystemSeverity::Info))));
            assert!(!first_block_text(notice).trim().is_empty());
        } else {
            assert!(appended.is_empty(), "normal completion must not append an interruption");
        }
    }
}

#[test]
fn startup_resume_history_renders_from_canonical_messages() {
    let mut app = make_test_app();
    let history_updates = vec![
        model::SessionUpdate::UserMessageChunk(model::ContentChunk::new(
            model::ContentBlock::Text(model::TextContent::new("startup user line\n\n")),
        )),
        model::SessionUpdate::UserMessageChunk(model::ContentChunk::new(
            model::ContentBlock::Text(model::TextContent::new("second paragraph")),
        )),
        model::SessionUpdate::AgentMessageChunk(model::ContentChunk::new(
            model::ContentBlock::Text(model::TextContent::new("startup assistant reply")),
        )),
    ];

    handle_client_event(
        &mut app,
        ClientEvent::Connected {
            session_id: model::SessionId::new("startup-resume"),
            cwd: "/resumed".into(),
            current_model: test_current_model("new-model"),
            available_models: Vec::new(),
            mode: None,
            fast_mode_state: model::FastModeState::Off,
            fast_mode_disabled_reason: None,
            ultracode: None,
            history_updates,
        },
    );

    assert!(canonical_messages_contain_text(&app, "startup user line\n\nsecond paragraph"));
    assert!(canonical_messages_contain_text(&app, "startup assistant reply"));
    assert!(live_rows_contain_text(&mut app, "startup user line"));
    assert!(live_rows_contain_text(&mut app, "startup assistant reply"));
    let user = app
        .transcript
        .messages
        .iter()
        .find(|msg| matches!(msg.role, MessageRole::User))
        .expect("restored user");
    let MessageBlock::Text(block) = &user.blocks[0] else {
        panic!("expected restored user text");
    };
    assert_eq!(block.text, "startup user line\n\nsecond paragraph");
    assert!(!session_overview_has_welcome(&app));
    assert!(matches!(app.status, AppStatus::Ready));
    assert!(!app.turn.cancel_requested);

    handle_client_event(
        &mut app,
        ClientEvent::StatusSnapshotReceived {
            session_id: "startup-resume".into(),
            account: crate::agent::model::AccountInfo {
                email: None,
                organization: None,
                subscription_type: Some("Claude Max".into()),
                token_source: None,
                api_key_source: None,
                api_provider: None,
            },
        },
    );

    assert!(!session_overview_has_welcome(&app));
}

#[test]
fn resume_history_preserves_user_message_boundaries_and_joins_only_same_source_chunks() {
    for startup in [true, false] {
        let mut app = make_test_app();
        let history_updates = [
            ("rename", "/rename "),
            ("rename", "review-444"),
            ("color", "/color blue"),
            ("prompt", "Explain <example>literal XML</example>"),
        ]
        .into_iter()
        .map(|(uuid, text)| {
            model::SessionUpdate::UserMessageChunk(
                model::ContentChunk::new(model::ContentBlock::Text(model::TextContent::new(text)))
                    .source_message_uuid(Some(uuid.to_owned())),
            )
        })
        .collect();
        let event = if startup {
            ClientEvent::Connected {
                session_id: model::SessionId::new("resumed"),
                cwd: "/resumed".into(),
                current_model: test_current_model("new-model"),
                available_models: Vec::new(),
                mode: None,
                fast_mode_state: model::FastModeState::Off,
                fast_mode_disabled_reason: None,
                ultracode: None,
                history_updates,
            }
        } else {
            ClientEvent::SessionReplaced {
                session_id: model::SessionId::new("resumed"),
                cwd: "/resumed".into(),
                current_model: test_current_model("new-model"),
                available_models: Vec::new(),
                mode: None,
                fast_mode_state: model::FastModeState::Off,
                fast_mode_disabled_reason: None,
                ultracode: None,
                history_updates,
                restored_input: None,
            }
        };
        handle_client_event(&mut app, event);
        let users = app
            .transcript
            .messages
            .iter()
            .filter(|message| matches!(message.role, MessageRole::User))
            .map(|message| match message.blocks.as_slice() {
                [MessageBlock::Text(text)] => {
                    (text.source_message_uuids.clone(), text.text.clone())
                }
                _ => panic!("expected one complete user text block"),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            users,
            vec![
                (vec!["rename".to_owned()], "/rename review-444".to_owned()),
                (vec!["color".to_owned()], "/color blue".to_owned()),
                (vec!["prompt".to_owned()], "Explain <example>literal XML</example>".to_owned()),
            ]
        );
        let rows = crate::ui::inline_chat_rows::serialize_live_rows_with_boundaries_excluding(
            &mut app,
            80,
            &std::collections::BTreeSet::new(),
            None,
        );
        let expected =
            ["/rename review-444", "/color blue", "Explain <example>literal XML</example>"];
        for (message_idx, message) in app.transcript.messages.iter().enumerate() {
            if !matches!(message.role, MessageRole::User) {
                continue;
            }
            let message_text = first_block_text(message);
            let regions = rows
                .segments()
                .iter()
                .filter(|region| region.msg_idx == message_idx)
                .collect::<Vec<_>>();
            assert!(!regions.is_empty(), "user message must have its own rendered region");
            let region_rows = regions
                .iter()
                .flat_map(|region| rows.segment_rows(region).expect("valid message region"))
                .map(ToString::to_string)
                .collect::<Vec<_>>();
            assert!(
                region_rows.iter().any(|row| row.contains(message_text)),
                "user text missing from its own region, startup={startup}"
            );
            for other in expected.iter().filter(|other| **other != message_text) {
                assert!(
                    region_rows.iter().all(|row| !row.contains(other)),
                    "distinct user messages must not share a rendered region"
                );
            }
        }
    }
}

#[test]
fn startup_resume_history_allows_immediate_prompt_submit() {
    let (mut app, mut rx) = app_with_bridge_connection();
    let history_updates = vec![
        model::SessionUpdate::UserMessageChunk(model::ContentChunk::new(
            model::ContentBlock::Text(model::TextContent::new("startup user line")),
        )),
        model::SessionUpdate::AgentMessageChunk(model::ContentChunk::new(
            model::ContentBlock::Text(model::TextContent::new("startup assistant reply")),
        )),
    ];

    handle_client_event(
        &mut app,
        ClientEvent::Connected {
            session_id: model::SessionId::new("startup-resume"),
            cwd: "/resumed".into(),
            current_model: test_current_model("new-model"),
            available_models: Vec::new(),
            mode: None,
            fast_mode_state: model::FastModeState::Off,
            fast_mode_disabled_reason: None,
            ultracode: None,
            history_updates,
        },
    );
    while rx.try_recv().is_ok() {}

    app.input.set_text("next prompt");
    handle_normal_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    super::super::finalize_deferred_submit(&mut app);

    let envelope = rx.try_recv().expect("prompt command should be sent");
    assert!(matches!(
        envelope.command,
        crate::agent::wire::BridgeCommand::Prompt { session_id, .. }
            if session_id == "startup-resume"
    ));
    assert!(!app.turn.cancel_requested);
    assert!(rx.try_recv().is_err(), "resume submit should not send cancel");
}

#[test]
fn resume_history_preserves_turn_order_between_user_and_assistant_messages() {
    let mut app = make_test_app();
    let history_updates = vec![
        model::SessionUpdate::UserMessageChunk(model::ContentChunk::new(
            model::ContentBlock::Text(model::TextContent::new("first user")),
        )),
        model::SessionUpdate::AgentMessageChunk(model::ContentChunk::new(
            model::ContentBlock::Text(model::TextContent::new("first assistant")),
        )),
        model::SessionUpdate::UserMessageChunk(model::ContentChunk::new(
            model::ContentBlock::Text(model::TextContent::new("second user")),
        )),
        model::SessionUpdate::AgentMessageChunk(model::ContentChunk::new(
            model::ContentBlock::Text(model::TextContent::new("second assistant")),
        )),
    ];

    handle_client_event(
        &mut app,
        ClientEvent::SessionReplaced {
            session_id: model::SessionId::new("active-457"),
            cwd: "/replacement".into(),
            current_model: test_current_model("new-model"),
            available_models: Vec::new(),
            mode: None,
            fast_mode_state: model::FastModeState::Off,
            fast_mode_disabled_reason: None,
            ultracode: None,
            history_updates,
            restored_input: None,
        },
    );

    let rendered: Vec<(MessageRole, String)> = app
        .transcript
        .messages
        .iter()
        .filter_map(|message| {
            let text = message.blocks.iter().find_map(|block| match block {
                MessageBlock::Text(block) => Some(block.text.clone()),
                _ => None,
            })?;
            Some((message.role.clone(), text))
        })
        .collect();

    assert_eq!(
        rendered,
        vec![
            (MessageRole::User, "first user".to_owned()),
            (MessageRole::Assistant, "first assistant".to_owned()),
            (MessageRole::User, "second user".to_owned()),
            (MessageRole::Assistant, "second assistant".to_owned()),
        ]
    );
}

#[test]
fn resume_history_forces_open_tool_calls_to_failed() {
    let mut app = make_test_app();
    let open_tool = model::ToolCall::new("resume-open", "Execute command")
        .kind(model::ToolKind::Execute)
        .status(model::ToolCallStatus::InProgress);

    handle_client_event(
        &mut app,
        ClientEvent::SessionReplaced {
            session_id: model::SessionId::new("active-789"),
            cwd: "/replacement".into(),
            current_model: test_current_model("new-model"),
            available_models: Vec::new(),
            mode: None,
            fast_mode_state: model::FastModeState::Off,
            fast_mode_disabled_reason: None,
            ultracode: None,
            history_updates: vec![model::SessionUpdate::ToolCall(open_tool)],
            restored_input: None,
        },
    );

    let Some((mi, bi)) = app.lookup_tool_call("resume-open") else {
        panic!("missing tool call index");
    };
    let Some(MessageBlock::ToolCall(tc)) =
        app.transcript.messages.get(mi).and_then(|m| m.blocks.get(bi))
    else {
        panic!("expected tool call block");
    };
    assert_eq!(tc.status, model::ToolCallStatus::Failed);
}

#[test]
fn resume_history_clears_active_turn_owner_after_loading() {
    let mut app = make_test_app();

    handle_client_event(
        &mut app,
        ClientEvent::SessionReplaced {
            session_id: model::SessionId::new("active-790"),
            cwd: "/replacement".into(),
            current_model: test_current_model("new-model"),
            available_models: Vec::new(),
            mode: None,
            fast_mode_state: model::FastModeState::Off,
            fast_mode_disabled_reason: None,
            ultracode: None,
            history_updates: vec![model::SessionUpdate::AgentMessageChunk(
                model::ContentChunk::new(model::ContentBlock::Text(model::TextContent::new(
                    "assistant reply",
                ))),
            )],
            restored_input: None,
        },
    );

    assert_eq!(app.active_turn_assistant_idx(), None);
}

#[test]
fn resume_history_clears_foreground_tracking_and_retains_tool_relationships() {
    let mut app = make_test_app();
    let task_tool = model::ToolCall::new("resume-task", "Run subagent")
        .kind(model::ToolKind::Think)
        .status(model::ToolCallStatus::InProgress)
        .meta(serde_json::json!({"claudeCode": {"toolName": "Task"}}));

    handle_client_event(
        &mut app,
        ClientEvent::SessionReplaced {
            session_id: model::SessionId::new("active-791"),
            cwd: "/replacement".into(),
            current_model: test_current_model("new-model"),
            available_models: Vec::new(),
            mode: None,
            fast_mode_state: model::FastModeState::Off,
            fast_mode_disabled_reason: None,
            ultracode: None,
            history_updates: vec![model::SessionUpdate::ToolCall(task_tool)],
            restored_input: None,
        },
    );

    assert_eq!(app.tool_call_scope("resume-task"), Some(ToolCallScope::SubagentRoot));
    let (mi, bi) = app.lookup_tool_call("resume-task").expect("resumed foreground tool");
    let MessageBlock::ToolCall(tool) = &app.transcript.messages[mi].blocks[bi] else {
        panic!("resumed tool call");
    };
    assert_eq!(tool.status, model::ToolCallStatus::Failed);
}

#[test]
fn turn_complete_without_cancel_does_not_render_interrupted_hint() {
    let mut app = make_test_app();
    handle_client_event(&mut app, turn_complete(None));
    assert!(app.transcript.messages.is_empty());
}

#[test]
fn explicit_manual_compaction_success_keeps_history_and_emits_once() {
    let mut app = make_test_app();
    app.turn.compaction.begin_manual();
    app.transcript.messages.push(user_msg("/compact"));
    app.transcript
        .messages
        .push(assistant_msg(vec![MessageBlock::Text(TextBlock::from_complete("compacted"))]));
    let history = message_snapshots(&app);
    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::CompactionUpdate(model::CompactionUpdate::Boundary(
            model::CompactionBoundary {
                trigger: model::CompactionTrigger::Manual,
                pre_tokens: 123_456,
                post_tokens: Some(20_000),
                duration_ms: Some(1_500),
            },
        ))),
    );
    assert!(app.turn.compaction.is_active());

    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::CompactionUpdate(model::CompactionUpdate::Finished {
            result: model::CompactionResult::Success,
            error_code: None,
            error: None,
        })),
    );

    handle_client_event(&mut app, turn_complete(None));

    assert!(!app.turn.compaction.is_active());
    assert_eq!(&message_snapshots(&app)[..history.len()], history);
    let [notice] = &app.transcript.messages[history.len()..] else {
        panic!("manual compaction must append exactly one success notice");
    };
    assert!(matches!(notice.role, MessageRole::System(Some(SystemSeverity::Info))));
    assert!(first_block_text(notice).to_lowercase().contains("compact"));
    assert_eq!(
        app.session_runtime.session_id.as_ref().map(ToString::to_string).as_deref(),
        Some("test-session")
    );
}

#[test]
fn late_boundary_after_explicit_success_does_not_reopen_or_duplicate_manual_notice() {
    let mut app = make_test_app();
    app.turn.compaction.begin_manual();

    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::CompactionUpdate(model::CompactionUpdate::Started)),
    );
    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::CompactionUpdate(model::CompactionUpdate::Finished {
            result: model::CompactionResult::Success,
            error_code: None,
            error: None,
        })),
    );
    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::SessionStatusUpdate(model::SessionStatus::Idle)),
    );
    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::CompactionUpdate(model::CompactionUpdate::Boundary(
            model::CompactionBoundary {
                trigger: model::CompactionTrigger::Manual,
                pre_tokens: 47_024,
                post_tokens: Some(8_049),
                duration_ms: Some(92_408),
            },
        ))),
    );

    assert!(!app.turn.compaction.is_active());

    handle_client_event(&mut app, turn_complete(None));

    let success_notices = app
        .transcript
        .messages
        .iter()
        .filter(|message| {
            matches!(message.role, MessageRole::System(Some(SystemSeverity::Info)))
                && message.blocks.iter().any(|block| {
                    matches!(
                        block,
                        MessageBlock::Text(text) if text.text == "Session successfully compacted."
                    )
                })
        })
        .count();
    assert_eq!(success_notices, 1);
}

#[test]
fn first_agent_chunk_clears_unconfirmed_compacting_without_success_message() {
    let mut app = make_test_app();
    app.turn.compaction.begin();

    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::AgentMessageChunk(model::ContentChunk::new(
            model::ContentBlock::Text(model::TextContent::new("regular answer")),
        ))),
    );

    assert!(!app.turn.compaction.is_active());
    assert!(app.transcript.messages.iter().all(|message| {
        !matches!(
            message,
            ChatMessage { role: MessageRole::System(Some(SystemSeverity::Info)), .. }
        )
    }));
}

#[test]
fn generic_session_status_does_not_mutate_compaction() {
    let mut app = make_test_app();
    app.turn.compaction.begin();

    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::SessionStatusUpdate(model::SessionStatus::Idle)),
    );

    assert!(app.turn.compaction.is_active());
    assert!(app.transcript.messages.is_empty());
}

#[test]
fn turn_error_clears_manual_compaction_without_success_message() {
    let mut app = make_test_app();
    app.turn.compaction.begin_manual();
    app.transcript.messages.push(user_msg("/compact"));

    handle_client_event(
        &mut app,
        ClientEvent::TurnError {
            session_id: "test-session".to_owned(),
            message: "adapter failed".into(),
            queued_turn_count: None,
            api_error_status: None,
            terminal_reason: None,
        },
    );

    assert!(!app.turn.compaction.is_active());
    assert!(matches!(app.status, AppStatus::Error));
    assert_eq!(app.transcript.messages.len(), 2);
    assert!(matches!(app.transcript.messages[0].role, MessageRole::User));
    let Some(ChatMessage { role: MessageRole::System(_), blocks, .. }) =
        app.transcript.messages.last()
    else {
        panic!("expected system error message");
    };
    let Some(MessageBlock::Notice(block)) = blocks.first() else {
        panic!("expected failure notice");
    };
    assert!(block.text.text.contains("Turn failed: adapter failed"));
    assert!(block.text.text.contains("Press Ctrl+Q to quit and try again"));
}

fn turn_error_log_levels(cancelled: bool) -> Vec<(String, String)> {
    let mut app = make_test_app();
    app.transcript.messages.push(user_msg("hello"));
    if cancelled {
        handle_local_cancel_enqueued(&mut app);
    }
    let records = crate::logging::test_capture::capture(|| {
        handle_client_event(
            &mut app,
            ClientEvent::TurnError {
                session_id: "test-session".to_owned(),
                message: "turn failed".into(),
                queued_turn_count: None,
                api_error_status: None,
                terminal_reason: None,
            },
        );
    });
    records
        .iter()
        .filter(|record| {
            matches!(
                record["event_name"].as_str(),
                Some("turn_error_suppressed" | "turn_error_received")
            )
        })
        .map(|record| {
            (
                record["event_name"].as_str().unwrap_or_default().to_owned(),
                record["level"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

// The default log keeps warnings and errors only, so an ordinary user cancellation must be
// below warn while a real turn error stays an error.
#[test]
fn cancelled_turn_error_is_not_a_warning_but_a_real_one_is_an_error() {
    assert_eq!(
        turn_error_log_levels(true),
        vec![("turn_error_suppressed".to_owned(), "DEBUG".to_owned())]
    );
    assert_eq!(
        turn_error_log_levels(false),
        vec![("turn_error_received".to_owned(), "ERROR".to_owned())]
    );
}

#[test]
fn turn_cancel_keeps_manual_compaction_active_until_exit() {
    let mut app = make_test_app();
    app.turn.compaction.begin_manual();

    handle_local_cancel_enqueued(&mut app);

    assert!(app.turn.compaction.is_active());
}

#[test]
fn turn_error_after_cancel_clears_compaction_without_success() {
    let mut app = make_test_app();
    app.transcript.messages.push(user_msg("/compact"));
    app.turn.compaction.begin_manual();

    handle_local_cancel_enqueued(&mut app);
    handle_client_event(
        &mut app,
        ClientEvent::TurnError {
            session_id: "test-session".to_owned(),
            message: "cancelled".into(),
            queued_turn_count: None,
            api_error_status: None,
            terminal_reason: None,
        },
    );

    assert!(!app.turn.compaction.is_active());
    assert_eq!(app.transcript.messages.len(), 2);
    let Some(MessageBlock::Text(block)) = app.transcript.messages[1].blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Conversation interrupted. Tell the model how to proceed.");
}

#[test]
fn turn_error_plan_limit_shows_next_steps_guidance() {
    let mut app = make_test_app();

    handle_client_event(
        &mut app,
        ClientEvent::TurnError {
            session_id: "test-session".to_owned(),
            message: "HTTP 429 Too Many Requests: max turns exceeded".into(),
            queued_turn_count: None,
            api_error_status: None,
            terminal_reason: None,
        },
    );

    assert!(matches!(app.status, AppStatus::Ready));
    let Some(ChatMessage { role: MessageRole::System(_), blocks, .. }) =
        app.transcript.messages.last()
    else {
        panic!("expected system error message");
    };
    assert!(matches!(blocks.first(), Some(MessageBlock::Notice(_))));
    let text = first_block_text(app.transcript.messages.last().expect("expected message"));
    assert!(text.contains("Turn blocked by account or plan limits"));
    assert!(text.contains("retry here"));
    assert!(!text.contains("Input disabled"));
}

#[test]
fn classified_turn_error_plan_limit_uses_guidance_without_text_matching() {
    let mut app = make_test_app();

    handle_client_event(
        &mut app,
        ClientEvent::TurnErrorClassified {
            session_id: "test-session".to_owned(),
            message: "turn failed".into(),
            class: TurnErrorClass::PlanLimit,
            queued_turn_count: None,
            api_error_status: None,
            terminal_reason: None,
        },
    );

    assert!(matches!(app.status, AppStatus::Ready));
    let Some(ChatMessage { role: MessageRole::System(_), blocks, .. }) =
        app.transcript.messages.last()
    else {
        panic!("expected system error message");
    };
    assert!(matches!(blocks.first(), Some(MessageBlock::Notice(_))));
    let text = first_block_text(app.transcript.messages.last().expect("expected message"));
    assert!(text.contains("Turn blocked by account or plan limits"));
    assert!(text.contains("retry here"));
    assert!(!text.contains("Input disabled"));
}

#[test]
fn classified_turn_error_auth_required_sets_exit_error_and_quits() {
    let mut app = make_test_app();

    handle_client_event(
        &mut app,
        ClientEvent::TurnErrorClassified {
            session_id: "test-session".to_owned(),
            message: "auth required".into(),
            class: TurnErrorClass::AuthRequired,
            queued_turn_count: None,
            api_error_status: None,
            terminal_reason: None,
        },
    );

    assert!(matches!(app.status, AppStatus::Error));
    assert!(app.shutdown_requested());
    assert_eq!(app.exit_error, Some(crate::error::AppError::AuthRequired));
}

#[test]
fn classified_turn_error_model_unavailable_suggests_model_switch() {
    let mut app = make_test_app();

    handle_client_event(
        &mut app,
        ClientEvent::TurnErrorClassified {
            session_id: "test-session".to_owned(),
            message: "model_not_found".into(),
            class: TurnErrorClass::ModelUnavailable,
            queued_turn_count: None,
            api_error_status: Some(404),
            terminal_reason: None,
        },
    );

    assert!(matches!(app.status, AppStatus::Error));
    assert!(!app.shutdown_requested());
    assert_eq!(app.exit_error, None);
    let text = first_block_text(app.transcript.messages.last().expect("expected message"));
    assert!(text.contains("The selected model is unavailable"));
    assert!(text.contains("Use /model"));
}

#[test]
fn classified_turn_error_account_access_does_not_quit_for_login() {
    let mut app = make_test_app();

    handle_client_event(
        &mut app,
        ClientEvent::TurnErrorClassified {
            session_id: "test-session".to_owned(),
            message: "oauth_org_not_allowed".into(),
            class: TurnErrorClass::AccountAccess,
            queued_turn_count: None,
            api_error_status: Some(403),
            terminal_reason: None,
        },
    );

    assert!(matches!(app.status, AppStatus::Error));
    assert!(!app.shutdown_requested());
    assert_eq!(app.exit_error, None);
    let text = first_block_text(app.transcript.messages.last().expect("expected message"));
    assert!(text.contains("current account or organization"));
    assert!(text.contains("cannot use the requested resource"));
}

#[test]
fn classified_turn_error_transient_service_suggests_retry() {
    let mut app = make_test_app();

    handle_client_event(
        &mut app,
        ClientEvent::TurnErrorClassified {
            session_id: "test-session".to_owned(),
            message: "overloaded".into(),
            class: TurnErrorClass::TransientService,
            queued_turn_count: None,
            api_error_status: Some(529),
            terminal_reason: None,
        },
    );

    assert!(matches!(app.status, AppStatus::Error));
    assert!(!app.shutdown_requested());
    let text = first_block_text(app.transcript.messages.last().expect("expected message"));
    assert!(text.contains("temporarily overloaded or unavailable"));
    assert!(text.contains("retry"));
}

#[test]
fn turn_error_clears_foreground_tracking_without_misclassifying_the_next_tool() {
    let mut app = make_test_app();
    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::ToolCall(
            model::ToolCall::new("task-1", "Run subagent")
                .status(model::ToolCallStatus::InProgress)
                .meta(serde_json::json!({"claudeCode": {"toolName": "Agent"}})),
        )),
    );

    handle_client_event(
        &mut app,
        ClientEvent::TurnError {
            session_id: "test-session".to_owned(),
            message: "boom".into(),
            queued_turn_count: None,
            api_error_status: None,
            terminal_reason: None,
        },
    );

    assert_eq!(app.tool_call_scope("task-1"), Some(ToolCallScope::SubagentRoot));
    let (mi, bi) = app.lookup_tool_call("task-1").expect("failed foreground tool");
    let MessageBlock::ToolCall(tool) = &app.transcript.messages[mi].blocks[bi] else {
        panic!("foreground tool call");
    };
    assert_eq!(tool.status, model::ToolCallStatus::Failed);
    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::ToolCall(
            model::ToolCall::new("next-bash", "Foreground command")
                .status(model::ToolCallStatus::InProgress)
                .meta(serde_json::json!({"claudeCode": {"toolName": "Bash"}})),
        )),
    );
    assert_eq!(app.tool_call_scope("next-bash"), Some(ToolCallScope::MainAgent));
}

#[test]
fn auth_required_clears_active_turn_runtime_tracking() {
    let mut app = make_test_app();
    app.status = AppStatus::Running;
    app.session_runtime.session_id = Some(model::SessionId::new("session-auth"));
    app.session_runtime.current_model = Some(test_current_model("claude-old"));
    app.session_runtime.mode = Some(crate::app::ModeState {
        current_mode_id: "plan".into(),
        current_mode_name: "Plan".into(),
        available_modes: vec![crate::app::ModeInfo { id: "plan".into(), name: "Plan".into() }],
    });
    app.session_runtime.fast_mode_state = model::FastModeState::On;
    app.transcript.messages.push(assistant_msg(vec![MessageBlock::ToolCall(Box::new(tool_call(
        "task-1",
        model::ToolCallStatus::InProgress,
    )))]));
    app.bind_active_turn_assistant(0);
    app.index_tool_call("task-1".into(), 0, 0);
    crate::app::test_support::set_tool_scope(&mut app, "task-1", ToolCallScope::SubagentRoot);
    app.pending_interaction_ids.push("task-1".into());
    app.claim_focus_target(FocusTarget::Permission);

    handle_client_event(
        &mut app,
        ClientEvent::AuthRequired {
            method_name: "oauth".into(),
            method_description: "Open browser".into(),
        },
    );

    assert_eq!(app.active_turn_assistant_idx(), None);
    assert!(app.pending_interaction_ids.is_empty());
    assert_ne!(app.focus_owner(), FocusOwner::Permission);
    let Some(MessageBlock::ToolCall(tc)) = app.transcript.messages[0].blocks.first() else {
        panic!("expected tool call block");
    };
    assert_eq!(tc.status, model::ToolCallStatus::Failed);
    assert!(app.session_runtime.session_id.is_none());
    assert!(app.session_runtime.current_model.is_none());
    assert!(app.session_runtime.mode.is_none());
    assert_eq!(app.session_runtime.fast_mode_state, model::FastModeState::Off);
}

#[test]
fn logout_completed_clears_session_runtime_identity_caches() {
    let mut app = make_test_app();
    app.session_runtime.session_id = Some(model::SessionId::new("session-x"));
    app.session_runtime.current_model = Some(test_current_model("claude-old"));
    app.session_runtime.mode = Some(crate::app::ModeState {
        current_mode_id: "plan".into(),
        current_mode_name: "Plan".into(),
        available_modes: vec![crate::app::ModeInfo { id: "plan".into(), name: "Plan".into() }],
    });
    app.session_runtime.fast_mode_state = model::FastModeState::On;

    handle_client_event(&mut app, ClientEvent::LogoutCompleted);

    assert!(app.session_runtime.session_id.is_none());
    assert!(app.session_runtime.current_model.is_none());
    assert!(app.session_runtime.mode.is_none());
    assert_eq!(app.session_runtime.fast_mode_state, model::FastModeState::Off);
}

#[test]
fn fatal_event_sets_exit_error_and_quits() {
    let mut app = make_test_app();

    handle_client_event(
        &mut app,
        ClientEvent::FatalError(crate::error::AppError::ConnectionFailed),
    );

    assert!(matches!(app.status, AppStatus::Error));
    assert!(app.shutdown_requested());
    assert_eq!(app.exit_error, Some(crate::error::AppError::ConnectionFailed));
}

#[test]
fn terminal_events_clear_active_turn_runtime_tracking() {
    for event in [
        ClientEvent::ConnectionFailed("bridge down".into()),
        ClientEvent::FatalError(crate::error::AppError::ConnectionFailed),
    ] {
        let mut app = make_test_app();
        app.status = AppStatus::Running;
        app.transcript.messages.push(assistant_msg(vec![MessageBlock::ToolCall(Box::new(
            tool_call("task-1", model::ToolCallStatus::InProgress),
        ))]));
        app.bind_active_turn_assistant(0);
        app.index_tool_call("task-1".into(), 0, 0);
        crate::app::test_support::set_tool_scope(&mut app, "task-1", ToolCallScope::SubagentRoot);

        handle_client_event(&mut app, event);

        assert_eq!(app.active_turn_assistant_idx(), None);
        let Some(MessageBlock::ToolCall(tc)) = app.transcript.messages[0].blocks.first() else {
            panic!("expected tool call block");
        };
        assert_eq!(tc.status, model::ToolCallStatus::Failed);
    }
}

#[test]
fn compaction_boundaries_keep_manual_and_auto_compaction_active() {
    for trigger in [model::CompactionTrigger::Manual, model::CompactionTrigger::Auto] {
        let mut app = make_test_app();
        match trigger {
            model::CompactionTrigger::Manual => app.turn.compaction.begin_manual(),
            model::CompactionTrigger::Auto => handle_client_event(
                &mut app,
                session_update(model::SessionUpdate::CompactionUpdate(
                    model::CompactionUpdate::Started,
                )),
            ),
        }
        assert!(app.turn.compaction.is_active());
        handle_client_event(
            &mut app,
            session_update(model::SessionUpdate::CompactionUpdate(
                model::CompactionUpdate::Boundary(model::CompactionBoundary {
                    trigger,
                    pre_tokens: 123_456,
                    post_tokens: Some(20_000),
                    duration_ms: Some(1_500),
                }),
            )),
        );
        assert!(
            app.turn.compaction.is_active(),
            "boundary must not admit a prompt mid-compaction: {trigger:?}"
        );
        assert!(app.transcript.messages.is_empty(), "boundary is not a completion notice");
    }
}

#[test]
fn duplicate_compaction_updates_share_one_idempotent_lifecycle() {
    let mut app = make_test_app();
    app.status = AppStatus::Ready;

    for _ in 0..2 {
        handle_client_event(
            &mut app,
            session_update(model::SessionUpdate::CompactionUpdate(
                model::CompactionUpdate::Started,
            )),
        );
    }
    assert!(app.turn.compaction.is_active());
    assert_eq!(app.status, AppStatus::Ready);
    assert_eq!(app.active_turn_assistant_idx(), None);
    assert!(app.transcript.messages.is_empty());

    for _ in 0..2 {
        handle_client_event(
            &mut app,
            session_update(model::SessionUpdate::CompactionUpdate(
                model::CompactionUpdate::Finished {
                    result: model::CompactionResult::Success,
                    error_code: None,
                    error: None,
                },
            )),
        );
    }

    assert!(!app.turn.compaction.is_active());
    assert!(app.transcript.messages.is_empty());
    assert_eq!(app.status, AppStatus::Ready);
    assert_eq!(app.active_turn_assistant_idx(), None);
    assert!(app.turn.activity.is_none());
}

#[test]
fn failed_compaction_clears_state_and_surfaces_sdk_error() {
    let mut app = make_test_app();
    app.turn.compaction.begin();

    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::CompactionUpdate(model::CompactionUpdate::Finished {
            result: model::CompactionResult::Failed,
            error_code: Some(model::CompactionFailureCode::Unknown),
            error: Some("Prompt is too long".to_owned()),
        })),
    );

    assert!(!app.turn.compaction.is_active());
    let Some(MessageBlock::Text(block)) =
        app.transcript.messages.last().and_then(|message| message.blocks.first())
    else {
        panic!("expected compaction failure message");
    };
    assert_eq!(block.text, "Context compaction failed: Prompt is too long");
}

#[test]
fn too_few_groups_compaction_failure_is_actionable() {
    let mut app = make_test_app();
    app.turn.compaction.begin();

    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::CompactionUpdate(model::CompactionUpdate::Finished {
            result: model::CompactionResult::Failed,
            error_code: Some(model::CompactionFailureCode::TooFewGroups),
            error: Some("too_few_groups".to_owned()),
        })),
    );

    let Some(MessageBlock::Text(block)) =
        app.transcript.messages.last().and_then(|message| message.blocks.first())
    else {
        panic!("expected compaction failure message");
    };
    assert!(!block.text.contains("too_few_groups"));
    assert!(block.text.to_lowercase().contains("start a new session"));
}

#[test]
fn fast_mode_disabled_reason_updates_clears_and_notifies_once_when_requested() {
    let mut app = make_test_app();
    app.config.snapshot = Some(crate::agent::settings::SettingsSnapshot::test_value(
        "fastMode",
        serde_json::json!(true),
    ));

    let update = model::SessionUpdate::FastModeUpdate {
        state: model::FastModeState::Off,
        disabled_reason: Some("model_not_allowed".to_owned()),
    };
    handle_client_event(&mut app, session_update(update.clone()));

    assert_eq!(app.session_runtime.fast_mode_disabled_reason.as_deref(), Some("model_not_allowed"));
    assert_eq!(
        first_block_text(app.transcript.messages.last().expect("fast mode notice")),
        "Fast mode is unavailable for the current model."
    );
    assert!(matches!(
        app.transcript.messages.last().map(|message| &message.role),
        Some(MessageRole::System(Some(SystemSeverity::Warning)))
    ));

    let notice_count = app.transcript.messages.len();
    handle_client_event(&mut app, session_update(update));
    assert_eq!(app.transcript.messages.len(), notice_count);

    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::FastModeUpdate {
            state: model::FastModeState::Off,
            disabled_reason: None,
        }),
    );
    assert!(app.session_runtime.fast_mode_disabled_reason.is_none());
    assert_eq!(app.transcript.messages.len(), notice_count);
}

#[test]
fn fast_mode_disabled_reason_does_not_warn_when_unrequested_or_cooling_down() {
    let mut unrequested = make_test_app();
    handle_client_event(
        &mut unrequested,
        session_update(model::SessionUpdate::FastModeUpdate {
            state: model::FastModeState::Off,
            disabled_reason: Some("free".to_owned()),
        }),
    );
    assert!(unrequested.transcript.messages.is_empty());

    let mut cooling_down = make_test_app();
    cooling_down.config.snapshot = Some(crate::agent::settings::SettingsSnapshot::test_value(
        "fastMode",
        serde_json::json!(true),
    ));
    handle_client_event(
        &mut cooling_down,
        session_update(model::SessionUpdate::FastModeUpdate {
            state: model::FastModeState::Cooldown,
            disabled_reason: Some("network_error".to_owned()),
        }),
    );
    assert!(cooling_down.transcript.messages.is_empty());
}

#[test]
fn rate_limit_warnings_do_not_duplicate_or_turn_into_failures() {
    let mut app = make_test_app();

    let warning_update = model::RateLimitUpdate {
        status: model::RateLimitStatus::AllowedWarning,
        error_code: None,
        resets_at: Some(123.0),
        utilization: Some(0.92),
        rate_limit_type: Some("five_hour".to_owned()),
        limit_scope: None,
        overage_enabled: Some(true),
        overage_status: Some(model::RateLimitStatus::Allowed),
        overage_resets_at: None,
        overage_disabled_reason: None,
        is_using_overage: None,
        surpassed_threshold: None,
        can_user_purchase_credits: None,
        has_chargeable_saved_payment_method: None,
    };

    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::RateLimitUpdate(warning_update.clone())),
    );
    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::RateLimitUpdate(warning_update.clone())),
    );

    assert_eq!(app.transcript.messages.len(), 1);
    assert!(matches!(
        app.transcript.messages[0].role,
        MessageRole::System(Some(SystemSeverity::Warning))
    ));
    assert!(matches!(app.transcript.messages[0].blocks.first(), Some(MessageBlock::Notice(_))));

    let mut allowed = warning_update.clone();
    allowed.status = model::RateLimitStatus::Allowed;
    handle_client_event(&mut app, session_update(model::SessionUpdate::RateLimitUpdate(allowed)));

    let rejected_update = model::RateLimitUpdate {
        status: model::RateLimitStatus::Rejected,
        overage_status: Some(model::RateLimitStatus::Rejected),
        ..warning_update
    };
    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::RateLimitUpdate(rejected_update.clone())),
    );
    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::RateLimitUpdate(rejected_update)),
    );

    assert_eq!(app.transcript.messages.len(), 1);
    assert!(matches!(
        app.transcript.messages[0].role,
        MessageRole::System(Some(SystemSeverity::Warning))
    ));
    assert!(first_block_text(&app.transcript.messages[0]).contains("Approaching rate limit"));
}

#[test]
fn plan_limit_failure_preserves_the_warning_and_reports_its_own_error() {
    let mut app = make_test_app();
    app.status = AppStatus::Thinking;
    app.transcript.messages.push(user_msg("hello"));
    app.transcript.messages.push(assistant_msg(vec![MessageBlock::Text(
        TextBlock::from_complete("partial response"),
    )]));
    app.bind_active_turn_assistant(1);

    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::RateLimitUpdate(model::RateLimitUpdate {
            status: model::RateLimitStatus::AllowedWarning,
            error_code: None,
            resets_at: Some(1_741_280_000.0),
            utilization: Some(0.95),
            rate_limit_type: Some("five_hour".to_owned()),
            limit_scope: None,
            overage_enabled: None,
            overage_status: None,
            overage_resets_at: None,
            overage_disabled_reason: None,
            is_using_overage: None,
            surpassed_threshold: None,
            can_user_purchase_credits: None,
            has_chargeable_saved_payment_method: None,
        })),
    );
    assert_eq!(app.transcript.messages.len(), 2);
    assert_eq!(app.transcript.messages[1].blocks.len(), 2);
    assert!(matches!(app.transcript.messages[1].blocks[1], MessageBlock::Notice(_)));
    assert_eq!(app.turn.notice_refs.len(), 1);

    handle_client_event(
        &mut app,
        ClientEvent::TurnErrorClassified {
            session_id: "test-session".to_owned(),
            message: "HTTP 429 Too Many Requests".to_owned(),
            class: TurnErrorClass::PlanLimit,
            queued_turn_count: None,
            api_error_status: None,
            terminal_reason: None,
        },
    );

    assert!(matches!(app.status, AppStatus::Ready));
    let [MessageBlock::Text(partial), MessageBlock::Notice(warning), MessageBlock::Notice(failure)] =
        app.transcript.messages[1].blocks.as_slice()
    else {
        panic!("partial reply, allowance warning, and terminal failure belong to this response");
    };
    assert_eq!(partial.text, "partial response");
    assert_eq!(warning.severity, SystemSeverity::Warning);
    assert!(warning.text.text.contains("Approaching rate limit"));
    assert_eq!(failure.severity, SystemSeverity::Error);
    assert!(failure.text.text.contains("HTTP 429 Too Many Requests"));
    assert!(!failure.text.text.contains("Approaching rate limit"));
    assert!(!failure.text.text.contains("5-hour"), "the error must not inherit allowance context");
    assert!(app.turn.notice_refs.is_empty());
}

#[test]
fn allowance_warnings_in_later_turns_keep_older_notices() {
    let mut app = make_test_app();
    let warning = model::RateLimitUpdate {
        status: model::RateLimitStatus::AllowedWarning,
        error_code: None,
        resets_at: Some(1_741_280_000.0),
        utilization: Some(0.95),
        rate_limit_type: Some("five_hour".to_owned()),
        limit_scope: None,
        overage_enabled: None,
        overage_status: None,
        overage_resets_at: None,
        overage_disabled_reason: None,
        is_using_overage: None,
        surpassed_threshold: None,
        can_user_purchase_credits: None,
        has_chargeable_saved_payment_method: None,
    };
    app.status = AppStatus::Thinking;
    app.transcript.messages.push(user_msg("first"));
    app.transcript.messages.push(assistant_msg(vec![]));
    app.bind_active_turn_assistant(1);

    handle_client_event(&mut app, session_update(model::SessionUpdate::RateLimitUpdate(warning)));
    handle_client_event(&mut app, turn_complete(None));
    assert_eq!(app.transcript.messages.len(), 2);
    let first_notice_text = match app.transcript.messages[1].blocks.as_slice() {
        [MessageBlock::Notice(block)] => block.text.text.clone(),
        _ => panic!("expected first turn notice"),
    };
    assert!(first_notice_text.contains("Approaching rate limit"));

    app.status = AppStatus::Thinking;
    app.transcript.messages.push(user_msg("second"));
    app.transcript.messages.push(assistant_msg(vec![]));
    app.bind_active_turn_assistant(3);
    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::RateLimitUpdate(model::RateLimitUpdate {
            status: model::RateLimitStatus::AllowedWarning,
            error_code: None,
            resets_at: Some(1_741_290_000.0),
            utilization: None,
            rate_limit_type: Some("daily".to_owned()),
            limit_scope: None,
            overage_enabled: None,
            overage_status: None,
            overage_resets_at: None,
            overage_disabled_reason: None,
            is_using_overage: None,
            surpassed_threshold: None,
            can_user_purchase_credits: None,
            has_chargeable_saved_payment_method: None,
        })),
    );

    assert_eq!(app.transcript.messages.len(), 4);
    let Some(MessageBlock::Notice(first_notice)) = app.transcript.messages[1].blocks.first() else {
        panic!("expected first turn notice");
    };
    assert_eq!(first_notice.text.text, first_notice_text);
    let Some(MessageBlock::Notice(second_notice)) = app.transcript.messages[3].blocks.first()
    else {
        panic!("expected second turn notice");
    };
    assert!(second_notice.text.text.contains("daily rate limit"));
    assert_ne!(second_notice.text.text, first_notice_text);
}

#[test]
fn notices_after_turn_completion_or_session_reset_append_distinct_blocks() {
    for lifecycle in [
        turn_complete(None),
        connected_event("claude"),
        ClientEvent::SessionReplaced {
            session_id: model::SessionId::new("replacement"),
            cwd: "/replacement".into(),
            current_model: test_current_model("new-model"),
            available_models: Vec::new(),
            mode: None,
            fast_mode_state: model::FastModeState::Off,
            fast_mode_disabled_reason: None,
            ultracode: None,
            history_updates: Vec::new(),
            restored_input: None,
        },
    ] {
        let mut app = make_test_app();
        app.status = AppStatus::Thinking;
        app.transcript.messages.push(user_msg("hello"));
        // Standalone notices can arrive before an assistant response starts.
        handle_client_event(
            &mut app,
            session_update(model::SessionUpdate::RateLimitUpdate(model::RateLimitUpdate {
                status: model::RateLimitStatus::AllowedWarning,
                error_code: None,
                resets_at: Some(123.0),
                utilization: Some(0.91),
                rate_limit_type: Some("five_hour".to_owned()),
                limit_scope: None,
                overage_enabled: None,
                overage_status: None,
                overage_resets_at: None,
                overage_disabled_reason: None,
                is_using_overage: None,
                surpassed_threshold: None,
                can_user_purchase_credits: None,
                has_chargeable_saved_payment_method: None,
            })),
        );
        let old_notice_id = app.transcript.messages.last().expect("first notice").id;
        assert!(first_block_text(app.transcript.messages.last().unwrap()).contains("5-hour"));
        let old_history = message_snapshots(&app);
        let completing_turn = matches!(lifecycle, ClientEvent::TurnComplete { .. });
        handle_client_event(&mut app, lifecycle);
        if completing_turn {
            assert_eq!(message_snapshots(&app), old_history);
        } else {
            assert!(app.transcript.messages.iter().all(|message| message.id != old_notice_id));
        }
        let prefix = message_snapshots(&app);
        let active_session =
            app.session_runtime.session_id.as_ref().expect("active session").to_string();
        handle_client_event(
            &mut app,
            ClientEvent::SessionUpdate {
                session_id: active_session,
                update: model::SessionUpdate::RateLimitUpdate(model::RateLimitUpdate {
                    status: model::RateLimitStatus::AllowedWarning,
                    error_code: None,
                    resets_at: Some(123.0),
                    utilization: Some(0.92),
                    rate_limit_type: Some("five_hour".to_owned()),
                    limit_scope: None,
                    overage_enabled: None,
                    overage_status: None,
                    overage_resets_at: None,
                    overage_disabled_reason: None,
                    is_using_overage: None,
                    surpassed_threshold: None,
                    can_user_purchase_credits: None,
                    has_chargeable_saved_payment_method: None,
                }),
            },
        );
        assert_eq!(
            &message_snapshots(&app)[..prefix.len()],
            prefix,
            "a later warning must not overwrite an older transcript block"
        );
        let [notice] = &app.transcript.messages[prefix.len()..] else {
            panic!("the later lifecycle must append exactly one new notice");
        };
        assert_ne!(notice.id, old_notice_id);
        let [MessageBlock::Notice(warning)] = notice.blocks.as_slice() else {
            panic!("expected a distinct new warning block");
        };
        assert_eq!(warning.severity, SystemSeverity::Warning);
        assert!(warning.text.text.contains("5-hour rate limit"));
        assert!(warning.text.text.contains("92%"));
    }
}

#[test]
fn turn_error_after_cancel_shows_interrupted_hint_instead_of_error_block() {
    let mut app = make_test_app();
    app.transcript.messages.push(user_msg("build app"));

    handle_local_cancel_enqueued(&mut app);
    assert!(app.turn.cancel_requested);

    handle_client_event(
        &mut app,
        ClientEvent::TurnError {
            session_id: "test-session".to_owned(),
            message: "Error: Request was aborted.\n    at stack line".into(),
            queued_turn_count: None,
            api_error_status: None,
            terminal_reason: None,
        },
    );

    assert!(!app.turn.cancel_requested);
    assert!(matches!(app.status, AppStatus::Ready));

    let Some(last) = app.transcript.messages.last() else {
        panic!("expected interruption hint message");
    };
    assert!(matches!(last.role, MessageRole::System(Some(SystemSeverity::Info))));
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Conversation interrupted. Tell the model how to proceed.");
}

#[test]
fn turn_cancel_marks_active_tools_failed() {
    let mut app = make_test_app();
    app.transcript.messages.push(assistant_msg(vec![
        MessageBlock::ToolCall(Box::new(tool_call("tc1", model::ToolCallStatus::InProgress))),
        MessageBlock::ToolCall(Box::new(tool_call("tc2", model::ToolCallStatus::Pending))),
        MessageBlock::ToolCall(Box::new(tool_call("tc3", model::ToolCallStatus::Completed))),
    ]));

    handle_local_cancel_enqueued(&mut app);

    let Some(last) = app.transcript.messages.last() else {
        panic!("missing assistant message");
    };
    let statuses: Vec<model::ToolCallStatus> = last
        .blocks
        .iter()
        .filter_map(|b| match b {
            MessageBlock::ToolCall(tc) => Some(tc.status),
            _ => None,
        })
        .collect();
    assert_eq!(
        statuses,
        vec![
            model::ToolCallStatus::Failed,
            model::ToolCallStatus::Failed,
            model::ToolCallStatus::Completed
        ]
    );
}

#[test]
fn turn_complete_marks_lingering_tools_completed() {
    let mut app = make_test_app();
    app.transcript.messages.push(assistant_msg(vec![
        MessageBlock::ToolCall(Box::new(tool_call("tc1", model::ToolCallStatus::InProgress))),
        MessageBlock::ToolCall(Box::new(tool_call("tc2", model::ToolCallStatus::Pending))),
    ]));

    handle_client_event(&mut app, turn_complete(None));

    let Some(last) = app.transcript.messages.last() else {
        panic!("missing assistant message");
    };
    let statuses: Vec<model::ToolCallStatus> = last
        .blocks
        .iter()
        .filter_map(|b| match b {
            MessageBlock::ToolCall(tc) => Some(tc.status),
            _ => None,
        })
        .collect();
    assert_eq!(statuses, vec![model::ToolCallStatus::Completed, model::ToolCallStatus::Completed]);
}

#[test]
fn ctrl_v_not_inserted_by_chat_key_handlers() {
    for handler in
        [handle_normal_key as fn(&mut App, KeyEvent), handle_mention_key as fn(&mut App, KeyEvent)]
    {
        let mut app = make_test_app();
        handler(&mut app, KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL));
        assert_eq!(app.input.text(), "");
    }
}

#[test]
fn pending_paste_payload_blocks_overlapping_key_text_insertion() {
    let mut app = make_test_app();
    handle_terminal_event(&mut app, Event::Paste("clipboard".into()));
    handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)),
    );

    assert_eq!(app.input.text(), "");
    assert_eq!(app.paste.pending_text, "clipboard");
}

#[test]
fn altgr_at_inserts_char_and_activates_mention() {
    let mut app = make_test_app();
    handle_normal_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('@'), KeyModifiers::CONTROL | KeyModifiers::ALT),
    );

    assert_eq!(app.input.text(), "@");
    assert!(app.mention.is_some());
}

fn completion_key(app: &mut App, code: KeyCode) {
    handle_terminal_event(app, Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
}

fn assert_completion_menu(app: &App, candidates: &[&str], selected: Option<&str>) -> String {
    let rows = crate::ui::input_rows::build_composer_hint_rows(app);
    let mut visible = Vec::new();
    let mut selected_names = Vec::new();
    for row in &rows {
        let text = row.to_string();
        let matches = candidates.iter().filter(|name| text.contains(**name)).collect::<Vec<_>>();
        let [name] = matches.as_slice() else {
            panic!("menu candidate region must show exactly one expected identity: {text:?}");
        };
        visible.push(**name);
        if row.spans.iter().any(|span| span.content.trim() == ">") {
            selected_names.push(**name);
        }
    }
    visible.sort_unstable();
    let mut expected = candidates.to_vec();
    expected.sort_unstable();
    assert_eq!(visible, expected, "rendered completion candidates");
    let [selected_name] = selected_names.as_slice() else {
        panic!("exactly one visible candidate must carry the selection control");
    };
    if let Some(expected) = selected {
        assert_eq!(*selected_name, expected, "selection belongs to the intended candidate row");
    }
    (*selected_name).to_owned()
}

fn await_file_completion(app: &mut App, query: &str, candidates: &[&str]) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut expected = candidates.to_vec();
    expected.sort_unstable();
    loop {
        crate::app::file_index::drain_events(app);
        if let Some(menu) = &app.mention {
            let mut actual =
                menu.candidates.iter().map(|item| item.rel_path.as_str()).collect::<Vec<_>>();
            actual.sort_unstable();
            if menu.query == query && actual == expected {
                return;
            }
        }
        assert!(
            Instant::now() < deadline,
            "no correlated file-search result for {query:?}: {expected:?}"
        );
        std::thread::yield_now();
    }
}

#[test]
fn file_completion_keys_follow_the_current_token_and_preserve_the_suffix() {
    let mut app = make_test_app();
    let fixture = tempfile::tempdir().expect("fixture index root");
    app.cwd_raw = fixture.path().to_string_lossy().into_owned();
    app.file_index.root = Some(fixture.path().to_path_buf());
    app.file_index.respect_gitignore = app.config.respect_gitignore_effective();
    app.file_index.status = crate::app::file_index::FileIndexStatus::Complete;
    for path in ["review-a.rs", "review-b.rs", "refactor.rs", "other.txt"] {
        app.file_index.entries.insert(
            path.to_owned(),
            crate::app::file_index::FileCandidate {
                rel_path: path.to_owned(),
                rel_path_lower: path.to_owned(),
                basename_lower: path.to_owned(),
                depth: 0,
            },
        );
    }
    app.input.set_text("open @re keep suffix");
    app.input.set_cursor(0, 5);
    assert!(crate::ui::input_rows::build_composer_hint_rows(&app).is_empty());

    for _ in 0..3 {
        completion_key(&mut app, KeyCode::Right);
    }
    await_file_completion(&mut app, "re", &["review-a.rs", "review-b.rs", "refactor.rs"]);
    assert_completion_menu(&app, &["review-a.rs", "review-b.rs", "refactor.rs"], None);
    completion_key(&mut app, KeyCode::Char('v'));
    await_file_completion(&mut app, "rev", &["review-a.rs", "review-b.rs"]);
    assert_completion_menu(&app, &["review-a.rs", "review-b.rs"], None);
    assert_eq!(app.input.text(), "open @rev keep suffix");

    completion_key(&mut app, KeyCode::End);
    assert!(crate::ui::input_rows::build_composer_hint_rows(&app).is_empty());
    completion_key(&mut app, KeyCode::Home);
    for _ in 0..9 {
        completion_key(&mut app, KeyCode::Right);
    }
    await_file_completion(&mut app, "rev", &["review-a.rs", "review-b.rs"]);
    assert_completion_menu(&app, &["review-a.rs", "review-b.rs"], None);
    completion_key(&mut app, KeyCode::Esc);
    completion_key(&mut app, KeyCode::F(12));
    assert!(crate::ui::input_rows::build_composer_hint_rows(&app).is_empty());
    assert_eq!(app.input.text(), "open @rev keep suffix");

    completion_key(&mut app, KeyCode::Left);
    completion_key(&mut app, KeyCode::Right);
    await_file_completion(&mut app, "rev", &["review-a.rs", "review-b.rs"]);
    let initial = assert_completion_menu(&app, &["review-a.rs", "review-b.rs"], None);
    let target = if initial == "review-a.rs" { "review-b.rs" } else { "review-a.rs" };
    completion_key(&mut app, KeyCode::Down);
    assert_completion_menu(&app, &["review-a.rs", "review-b.rs"], Some(target));
    completion_key(&mut app, KeyCode::Enter);
    assert_eq!(
        app.input.text(),
        if target == "review-b.rs" {
            "open @'review-b.rs' keep suffix"
        } else {
            "open @'review-a.rs' keep suffix"
        }
    );
    assert!(app.pending_submit.is_none(), "completion must not submit the draft");
}

#[test]
fn agent_completion_keys_follow_the_current_token_and_preserve_the_suffix() {
    let mut app = make_test_app();
    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::AvailableAgentsUpdate(
            model::AvailableAgentsUpdate::new(vec![
                model::AvailableAgent::new("review-a", "Audit A"),
                model::AvailableAgent::new("review-b", "Audit B"),
                model::AvailableAgent::new("refactor", "Cleanup"),
                model::AvailableAgent::new("other", "Misc"),
            ]),
        )),
    );
    app.input.set_text("ask &re keep suffix");
    app.input.set_cursor(0, 4);
    assert!(crate::ui::input_rows::build_composer_hint_rows(&app).is_empty());

    for _ in 0..3 {
        completion_key(&mut app, KeyCode::Right);
    }
    assert_completion_menu(&app, &["&review-a", "&review-b", "&refactor"], None);
    completion_key(&mut app, KeyCode::Char('v'));
    assert_completion_menu(&app, &["&review-a", "&review-b"], None);
    assert_eq!(app.input.text(), "ask &rev keep suffix");

    completion_key(&mut app, KeyCode::End);
    assert!(crate::ui::input_rows::build_composer_hint_rows(&app).is_empty());
    completion_key(&mut app, KeyCode::Home);
    for _ in 0..8 {
        completion_key(&mut app, KeyCode::Right);
    }
    assert_completion_menu(&app, &["&review-a", "&review-b"], None);
    completion_key(&mut app, KeyCode::Esc);
    completion_key(&mut app, KeyCode::F(12));
    assert!(crate::ui::input_rows::build_composer_hint_rows(&app).is_empty());
    assert_eq!(app.input.text(), "ask &rev keep suffix");

    completion_key(&mut app, KeyCode::Left);
    completion_key(&mut app, KeyCode::Right);
    let initial = assert_completion_menu(&app, &["&review-a", "&review-b"], None);
    let target = if initial == "&review-a" { "&review-b" } else { "&review-a" };
    completion_key(&mut app, KeyCode::Down);
    assert_completion_menu(&app, &["&review-a", "&review-b"], Some(target));
    completion_key(&mut app, KeyCode::Enter);
    assert_eq!(
        app.input.text(),
        if target == "&review-b" {
            "ask &review-b keep suffix"
        } else {
            "ask &review-a keep suffix"
        }
    );
    assert!(app.pending_submit.is_none(), "completion must not submit the draft");
}

#[test]
fn ctrl_backspace_and_delete_use_word_operations() {
    let mut app = make_test_app();
    app.input.set_text("hello world");

    handle_normal_key(&mut app, KeyEvent::new(KeyCode::Backspace, WORD_NAV_MOD));
    assert_eq!(app.input.text(), "hello ");

    app.input.textarea_move_home();
    handle_normal_key(&mut app, KeyEvent::new(KeyCode::Delete, WORD_NAV_MOD));
    assert_eq!(app.input.text(), " ");
}

#[test]
fn configured_undo_and_redo_restore_textarea_history() {
    #[cfg(target_os = "macos")]
    let aliases = [
        ('z', KeyModifiers::SUPER, 'z', KeyModifiers::SUPER | KeyModifiers::SHIFT),
        ('z', KeyModifiers::SUPER, 'y', KeyModifiers::SUPER),
    ];
    #[cfg(target_os = "windows")]
    let aliases = [('z', KeyModifiers::CONTROL, 'z', KeyModifiers::CONTROL | KeyModifiers::SHIFT)];
    #[cfg(all(unix, not(target_os = "macos")))]
    let aliases = [
        ('_', KeyModifiers::CONTROL, 'z', KeyModifiers::CONTROL | KeyModifiers::SHIFT),
        ('/', KeyModifiers::CONTROL, 'z', KeyModifiers::CONTROL | KeyModifiers::SHIFT),
    ];
    for (undo, undo_modifiers, redo, redo_modifiers) in aliases {
        let mut app = make_test_app();
        app.status = AppStatus::Ready;
        app.input.set_text("hello world");
        handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Backspace, WORD_NAV_MOD)),
        );
        assert_eq!(app.input.text(), "hello ");
        let outcome = handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char(undo), undo_modifiers)),
        );
        assert_eq!(app.input.text(), "hello world");
        assert_eq!(outcome.runtime_command, None, "history keys must not suspend the process");
        let outcome = handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char(redo), redo_modifiers)),
        );
        assert_eq!(app.input.text(), "hello ");
        assert_eq!(outcome.runtime_command, None);
    }
}

#[test]
fn ctrl_y_yanks_the_last_killed_text() {
    let mut app = make_test_app();
    app.input.set_text("hello world");
    app.input.textarea_move_home();

    handle_normal_key(&mut app, KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
    assert_eq!(app.input.text(), "");

    handle_normal_key(&mut app, KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL));
    assert_eq!(app.input.text(), "hello world");
}

#[test]
fn ctrl_left_right_move_by_word() {
    let mut app = make_test_app();
    app.input.set_text("hello world");
    app.input.textarea_move_home();

    handle_normal_key(&mut app, KeyEvent::new(KeyCode::Right, WORD_NAV_MOD));
    assert_eq!(app.input.cursor_col(), 6, "word-right lands at the start of world");

    handle_normal_key(&mut app, KeyEvent::new(KeyCode::Left, WORD_NAV_MOD));
    assert_eq!(app.input.cursor_col(), 0);
}

#[test]
fn permission_owner_handles_up_down_for_pending_interactions() {
    let mut app = make_test_app();
    let _rx_a = attach_pending_permission(
        &mut app,
        "perm-a",
        vec![
            model::PermissionOption::new("allow", "Allow", model::PermissionOptionKind::AllowOnce),
            model::PermissionOption::new("deny", "Deny", model::PermissionOptionKind::RejectOnce),
        ],
        true,
    );
    let _rx_b = attach_pending_permission(
        &mut app,
        "perm-b",
        vec![
            model::PermissionOption::new("allow", "Allow", model::PermissionOptionKind::AllowOnce),
            model::PermissionOption::new("deny", "Deny", model::PermissionOptionKind::RejectOnce),
        ],
        false,
    );
    app.claim_focus_target(FocusTarget::Permission);

    handle_terminal_event(&mut app, Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)));

    assert_eq!(app.pending_interaction_ids, vec!["perm-b", "perm-a"]);
}

#[test]
fn permission_request_with_existing_draft_does_not_claim_focus() {
    let mut app = make_test_app();
    let tool_id = "perm-draft";
    append_tool_call_block(&mut app, tool_id);
    app.input.set_text("draft in progress");

    let (response_tx, _response_rx) = oneshot::channel();
    turn::handle_permission_request_event(
        &mut app,
        model::RequestPermissionRequest::new(
            "session-1",
            model::ToolCallUpdate::new(tool_id, model::ToolCallUpdateFields::new()),
            vec![
                model::PermissionOption::new(
                    "allow",
                    "Allow",
                    model::PermissionOptionKind::AllowOnce,
                ),
                model::PermissionOption::new(
                    "deny",
                    "Deny",
                    model::PermissionOptionKind::RejectOnce,
                ),
            ],
            None,
        ),
        response_tx,
    );

    assert_eq!(app.focus_owner(), FocusOwner::Input);
    assert_eq!(app.pending_interaction_ids, vec![tool_id]);
    assert_eq!(permission_focus_state(&app, tool_id), Some(false));
}

#[test]
fn question_request_with_existing_draft_does_not_claim_focus() {
    let mut app = make_test_app();
    let tool_id = "question-draft";
    append_tool_call_block(&mut app, tool_id);
    app.input.set_text("draft in progress");

    let (response_tx, _response_rx) = oneshot::channel();
    turn::handle_question_request_event(
        &mut app,
        model::RequestQuestionRequest::new(
            "session-1",
            model::ToolCallUpdate::new(tool_id, model::ToolCallUpdateFields::new()),
            model::QuestionPrompt::new(
                "Choose one",
                "Question",
                false,
                vec![
                    model::QuestionOption::new("yes", "Yes"),
                    model::QuestionOption::new("no", "No"),
                ],
            ),
            0,
            1,
        ),
        response_tx,
    );

    assert_eq!(app.focus_owner(), FocusOwner::Input);
    assert_eq!(app.pending_interaction_ids, vec![tool_id]);
    assert_eq!(question_focus_state(&app, tool_id), Some(false));
}

#[test]
fn enter_submits_draft_when_permission_arrives_mid_compose() {
    let (mut app, mut bridge_rx) = app_with_bridge_connection();
    let tool_id = "perm-submit";
    append_tool_call_block(&mut app, tool_id);
    app.session_runtime.session_id = Some(model::SessionId::new("session-1"));
    app.input.set_text("ship the fix");

    let (response_tx, mut response_rx) = oneshot::channel();
    turn::handle_permission_request_event(
        &mut app,
        model::RequestPermissionRequest::new(
            "session-1",
            model::ToolCallUpdate::new(tool_id, model::ToolCallUpdateFields::new()),
            vec![
                model::PermissionOption::new(
                    "allow",
                    "Allow",
                    model::PermissionOptionKind::AllowOnce,
                ),
                model::PermissionOption::new(
                    "deny",
                    "Deny",
                    model::PermissionOptionKind::RejectOnce,
                ),
            ],
            None,
        ),
        response_tx,
    );

    handle_terminal_event(&mut app, Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));

    assert!(app.pending_submit.is_some());
    assert!(matches!(
        response_rx.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));

    super::super::finalize_deferred_submit(&mut app);

    assert!(app.pending_submit.is_none());
    assert!(app.pending_interaction_ids.is_empty());
    let crate::agent::wire::BridgeCommand::Prompt { session_id, chunks, .. } =
        bridge_rx.try_recv().expect("draft prompt dispatched").command
    else {
        panic!("Enter must dispatch the draft as a prompt");
    };
    assert_eq!(session_id, "session-1");
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].kind, "text");
    assert_eq!(chunks[0].value, serde_json::json!("ship the fix"));
    assert!(
        matches!(response_rx.try_recv(), Err(tokio::sync::oneshot::error::TryRecvError::Closed)),
        "starting a fresh turn drops the old permission without answering it"
    );
    let tool = app.tool_call(tool_id).expect("original permission tool retained");
    assert!(tool.pending_permission.is_none());
    assert_eq!(tool.status, model::ToolCallStatus::Failed);
}

#[test]
fn tab_toggles_focus_between_input_and_pending_permission() {
    let mut app = make_test_app();
    let _response_rx = attach_pending_permission(
        &mut app,
        "perm-tab",
        vec![
            model::PermissionOption::new("allow", "Allow", model::PermissionOptionKind::AllowOnce),
            model::PermissionOption::new("deny", "Deny", model::PermissionOptionKind::RejectOnce),
        ],
        false,
    );
    app.input.set_text("keep drafting");
    app.release_focus_target(FocusTarget::Permission);

    handle_terminal_event(&mut app, Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
    assert_eq!(app.focus_owner(), FocusOwner::Permission);
    assert_eq!(permission_focus_state(&app, "perm-tab"), Some(true));

    handle_terminal_event(&mut app, Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
    assert_eq!(app.focus_owner(), FocusOwner::Input);
    assert_eq!(permission_focus_state(&app, "perm-tab"), Some(false));
}

#[test]
fn typing_reclaims_input_from_auto_focused_permission() {
    let mut app = make_test_app();
    let _response_rx = attach_pending_permission(
        &mut app,
        "perm-auto",
        vec![
            model::PermissionOption::new("allow", "Allow", model::PermissionOptionKind::AllowOnce),
            model::PermissionOption::new("deny", "Deny", model::PermissionOptionKind::RejectOnce),
        ],
        true,
    );

    handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE)),
    );

    assert_eq!(app.focus_owner(), FocusOwner::Input);
    assert_eq!(app.input.text(), "h");
    assert_eq!(permission_focus_state(&app, "perm-auto"), Some(false));
}

#[test]
fn tab_focuses_question_and_enter_confirms_only_after_explicit_handoff() {
    let (mut app, _bridge_rx) = app_with_bridge_connection();
    let mut response_rx = attach_pending_question(
        &mut app,
        "question-tab",
        model::QuestionPrompt::new(
            "Choose one",
            "Question",
            false,
            vec![model::QuestionOption::new("yes", "Yes"), model::QuestionOption::new("no", "No")],
        ),
        false,
    );
    app.input.set_text("draft answer");

    handle_terminal_event(&mut app, Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
    assert!(app.pending_submit.is_some());
    assert!(matches!(
        response_rx.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));

    handle_terminal_event(&mut app, Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
    assert_eq!(app.focus_owner(), FocusOwner::Permission);
    assert_eq!(question_focus_state(&app, "question-tab"), Some(true));

    handle_terminal_event(&mut app, Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
    let response = response_rx.try_recv().expect("question should be answered after Tab focus");
    assert!(matches!(response.outcome, model::RequestQuestionOutcome::Answered(_)));
}

#[test]
fn typing_reclaims_input_from_auto_focused_question() {
    let mut app = make_test_app();
    let _response_rx = attach_pending_question(
        &mut app,
        "question-auto",
        model::QuestionPrompt::new(
            "Choose one",
            "Question",
            false,
            vec![model::QuestionOption::new("yes", "Yes"), model::QuestionOption::new("no", "No")],
        ),
        true,
    );

    handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE)),
    );

    assert_eq!(app.focus_owner(), FocusOwner::Input);
    assert_eq!(app.input.text(), "n");
    assert_eq!(question_focus_state(&app, "question-auto"), Some(false));
}

#[test]
fn space_toggles_focused_question_without_reclaiming_input() {
    let mut app = make_test_app();
    let _response_rx = attach_pending_question(
        &mut app,
        "question-space",
        model::QuestionPrompt::new(
            "Choose drinks",
            "Drinks",
            true,
            vec![
                model::QuestionOption::new("coffee", "Coffee"),
                model::QuestionOption::new("tea", "Tea"),
            ],
        ),
        true,
    );

    handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE)),
    );

    assert_eq!(app.focus_owner(), FocusOwner::Permission);
    assert_eq!(app.input.text(), "");
    let (mi, bi) = app.lookup_tool_call("question-space").expect("question tool call");
    let MessageBlock::ToolCall(tc) =
        app.transcript.messages.get(mi).unwrap().blocks.get(bi).unwrap()
    else {
        panic!("expected tool call block");
    };
    let question = tc.pending_question.as_ref().expect("pending question");
    assert!(question.selected_option_indices.contains(&0));
}

#[test]
fn stale_inline_interaction_queue_head_is_pruned_before_enter_response() {
    let mut app = make_test_app();
    let mut response_rx = attach_pending_permission(
        &mut app,
        "perm-1",
        vec![
            model::PermissionOption::new("allow", "Allow", model::PermissionOptionKind::AllowOnce),
            model::PermissionOption::new("deny", "Deny", model::PermissionOptionKind::RejectOnce),
        ],
        false,
    );
    app.pending_interaction_ids.insert(0, "stale-id".into());

    handle_terminal_event(&mut app, Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));

    let response = response_rx.try_recv().expect("permission response");
    assert!(matches!(response.outcome, model::RequestPermissionOutcome::Selected(_)));
    assert!(app.pending_interaction_ids.is_empty());
}

#[test]
fn update_result_persists_across_session_resets_without_notice() {
    for event in [
        connected_event("claude-updated"),
        ClientEvent::SessionReplaced {
            session_id: model::SessionId::new("replacement"),
            cwd: "/replacement".into(),
            current_model: test_current_model("new-model"),
            available_models: Vec::new(),
            mode: None,
            fast_mode_state: model::FastModeState::Off,
            fast_mode_disabled_reason: None,
            ultracode: None,
            history_updates: Vec::new(),
            restored_input: None,
        },
    ] {
        let mut app = make_test_app();

        handle_client_event(
            &mut app,
            ClientEvent::UpdateCheckCompleted {
                result: crate::app::settings::UpdateCheckResult {
                    latest_version: "0.11.2".into(),
                    current_version: "0.11.1".into(),
                    release_url: crate::app::settings::release_url_for_version("0.11.2")
                        .expect("release URL"),
                    checked_at_unix_secs: 10,
                    source: "github_release".into(),
                },
            },
        );
        handle_client_event(&mut app, event);

        assert!(matches!(
            app.transcript.messages.first().map(|msg| &msg.role),
            Some(MessageRole::Welcome)
        ));
        assert_eq!(app.transcript.messages.len(), 1);
        let last_result = app.global_settings.updates.last_result.as_ref().expect("update result");
        assert_eq!(last_result.current_version, "0.11.1");
        assert_eq!(last_result.latest_version, "0.11.2");
        assert_eq!(last_result.checked_at_unix_secs, 10);
        assert_eq!(last_result.source, "github_release");
    }
}

fn attach_pending_permission(
    app: &mut App,
    tool_id: &str,
    options: Vec<model::PermissionOption>,
    focused: bool,
) -> oneshot::Receiver<model::RequestPermissionResponse> {
    let (response_tx, response_rx) = oneshot::channel();
    let mut tc = tool_call(tool_id, model::ToolCallStatus::InProgress);
    tc.pending_permission = Some(InlinePermission {
        options,
        display: None,
        subagent_context: None,
        response_tx,
        selected_index: 0,
        focused,
    });
    app.transcript.messages.push(assistant_msg(vec![MessageBlock::ToolCall(Box::new(tc))]));
    let msg_idx = app.transcript.messages.len().saturating_sub(1);
    app.index_tool_call(tool_id.into(), msg_idx, 0);
    app.pending_interaction_ids.push(tool_id.into());
    app.claim_focus_target(FocusTarget::Permission);
    response_rx
}

fn attach_pending_question(
    app: &mut App,
    tool_id: &str,
    prompt: model::QuestionPrompt,
    focused: bool,
) -> oneshot::Receiver<model::RequestQuestionResponse> {
    let (response_tx, response_rx) = oneshot::channel();
    let mut tc = tool_call(tool_id, model::ToolCallStatus::InProgress);
    tc.pending_question = Some(InlineQuestion {
        idle_timeout: None,
        last_activity: std::time::Instant::now(),
        prompt,
        response_tx,
        focused_option_index: 0,
        selected_option_indices: std::collections::BTreeSet::new(),
        notes: String::new(),
        notes_cursor: 0,
        editing_notes: false,
        focused,
        question_index: 0,
        total_questions: 1,
    });
    app.transcript.messages.push(assistant_msg(vec![MessageBlock::ToolCall(Box::new(tc))]));
    let msg_idx = app.transcript.messages.len().saturating_sub(1);
    app.index_tool_call(tool_id.into(), msg_idx, 0);
    app.pending_interaction_ids.push(tool_id.into());
    if focused {
        app.claim_focus_target(FocusTarget::Permission);
    }
    response_rx
}

fn show_config_slash_menu(app: &mut App) {
    app.slash.show(SlashState {
        trigger_row: 0,
        trigger_col: 0,
        query: String::new(),
        context: SlashContext::CommandName,
        candidates: vec![SlashCandidate {
            insert_value: "/config".into(),
            primary: "/config".into(),
            secondary: Some("Open settings".into()),
        }],
        placeholder: None,
        dialog: crate::app::dialog::DialogState::default(),
    });
}

fn permission_focus_state(app: &App, tool_id: &str) -> Option<bool> {
    let (mi, bi) = app.lookup_tool_call(tool_id)?;
    let MessageBlock::ToolCall(tc) = app.transcript.messages.get(mi)?.blocks.get(bi)? else {
        return None;
    };
    tc.pending_permission.as_ref().map(|permission| permission.focused)
}

fn question_focus_state(app: &App, tool_id: &str) -> Option<bool> {
    let (mi, bi) = app.lookup_tool_call(tool_id)?;
    let MessageBlock::ToolCall(tc) = app.transcript.messages.get(mi)?.blocks.get(bi)? else {
        return None;
    };
    tc.pending_question.as_ref().map(|question| question.focused)
}

fn control_key_permission_options(plan: bool) -> Vec<model::PermissionOption> {
    if plan {
        vec![
            model::PermissionOption::new(
                "plan-approve",
                "Approve",
                model::PermissionOptionKind::PlanApprove,
            ),
            model::PermissionOption::new(
                "plan-reject",
                "Reject",
                model::PermissionOptionKind::PlanReject,
            ),
        ]
    } else {
        vec![
            model::PermissionOption::new(
                "allow-once",
                "Allow once",
                model::PermissionOptionKind::AllowOnce,
            ),
            model::PermissionOption::new(
                "allow-always",
                "Allow always",
                model::PermissionOptionKind::AllowAlways,
            ),
            model::PermissionOption::new("deny", "Deny", model::PermissionOptionKind::RejectOnce),
        ]
    }
}

#[test]
fn control_keys_do_not_answer_pending_permissions() {
    for (key, plan) in [
        (KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE), false),
        (KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE), false),
        (KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE), false),
        (KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL), false),
        (KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL), false),
        (KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL), false),
        (KeyEvent::new(KeyCode::Char('\u{19}'), KeyModifiers::NONE), true),
    ] {
        let mut app = make_test_app();
        app.status = AppStatus::Running;
        let (connection, mut commands) = crate::agent::client::AgentConnection::test_channel();
        app.session_runtime.conn = Some(std::rc::Rc::new(connection));
        let tool_id = "permission-control";
        append_tool_call_block(&mut app, tool_id);
        let options = control_key_permission_options(plan);
        let (response_tx, mut response_rx) = oneshot::channel();
        handle_client_event(
            &mut app,
            ClientEvent::PermissionRequest {
                session_id: "test-session".into(),
                request: model::RequestPermissionRequest::new(
                    "test-session",
                    model::ToolCallUpdate::new(tool_id, model::ToolCallUpdateFields::new()),
                    options,
                    None,
                ),
                response_tx,
            },
        );
        assert_eq!(app.focus_owner(), FocusOwner::Permission);
        // Keep the existing raw-control draft while the actual permission owns focus.
        if plan {
            app.input.set_text("seed");
        }
        let draft = app.input.text();
        let (mi, bi) = app.lookup_tool_call(tool_id).expect("indexed permission tool");
        handle_terminal_event(&mut app, Event::Key(key));
        assert!(
            matches!(response_rx.try_recv(), Err(tokio::sync::oneshot::error::TryRecvError::Empty)),
            "{key:?} must leave the response channel pending"
        );
        assert_eq!(app.pending_interaction_ids, vec![tool_id]);
        let MessageBlock::ToolCall(tool) = &app.transcript.messages[mi].blocks[bi] else {
            panic!("permission must stay on its original tool");
        };
        assert_eq!(tool.id, tool_id);
        assert!(tool.pending_permission.is_some());
        assert_eq!(tool.status, model::ToolCallStatus::InProgress);
        if plan {
            assert_eq!(app.input.text(), draft);
        }
        // A real answer must still reach this request; a closed/dead channel cannot pass.
        if app.focus_owner() != FocusOwner::Permission {
            handle_terminal_event(
                &mut app,
                Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            );
        }
        assert_eq!(permission_focus_state(&app, tool_id), Some(true));
        handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        );
        let response = response_rx.try_recv().expect("Esc must reject the same live request");
        let model::RequestPermissionOutcome::Selected(selected) = response.outcome else {
            panic!("expected explicit rejection");
        };
        assert_eq!(selected.option_id, if plan { "plan-reject" } else { "deny" });
        assert!(app.pending_interaction_ids.is_empty());
        assert!(app.pending_submit.is_none());
        assert!(commands.try_recv().is_err());
    }
}

#[test]
fn permission_ctrl_n_does_not_bypass_mention_focus() {
    let mut app = make_test_app();
    let mut response_rx = attach_pending_permission(
        &mut app,
        "perm-1",
        vec![
            model::PermissionOption::new("allow", "Allow", model::PermissionOptionKind::AllowOnce),
            model::PermissionOption::new("deny", "Deny", model::PermissionOptionKind::RejectOnce),
        ],
        true,
    );

    show_config_slash_menu(&mut app);
    app.claim_focus_target(FocusTarget::Mention);
    assert_eq!(app.focus_owner(), FocusOwner::Mention);

    handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL)),
    );

    assert!(matches!(
        response_rx.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    assert_eq!(app.pending_interaction_ids, vec!["perm-1"]);
}

#[test]
fn second_esc_after_permission_rejection_requests_turn_cancel() {
    let (mut app, mut rx) = app_with_bridge_connection();
    app.status = AppStatus::Running;
    app.session_runtime.session_id = Some(model::SessionId::new("session-1"));
    let mut response_rx = attach_pending_permission(
        &mut app,
        "perm-1",
        vec![
            model::PermissionOption::new("allow", "Allow", model::PermissionOptionKind::AllowOnce),
            model::PermissionOption::new("deny", "Deny", model::PermissionOptionKind::RejectOnce),
        ],
        true,
    );

    handle_terminal_event(&mut app, Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));

    let response = response_rx.try_recv().expect("first Esc should answer permission");
    let model::RequestPermissionOutcome::Selected(selected) = response.outcome else {
        panic!("expected selected permission response");
    };
    assert_eq!(selected.option_id.clone(), "deny");
    assert!(app.pending_interaction_ids.is_empty());
    assert!(!app.turn.cancel_requested);

    handle_terminal_event(&mut app, Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));

    assert!(app.turn.cancel_requested);
    let envelope = rx.try_recv().expect("second Esc should send turn cancel");
    assert!(matches!(
        envelope.command,
        crate::agent::wire::BridgeCommand::CancelTurn { session_id }
            if session_id == "session-1"
    ));
}

#[test]
fn connecting_state_allows_drafting_but_blocks_submission() {
    let mut app = make_test_app();
    app.status = AppStatus::Connecting;
    app.input.set_text("seed");

    handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)),
    );
    handle_terminal_event(&mut app, Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
    handle_terminal_event(&mut app, Event::Paste(" pasted".into()));
    crate::app::input_submit::submit_input(&mut app);

    assert_eq!(app.input.text(), "seedx");
    assert_eq!(app.paste.pending_text, " pasted");
    assert!(app.pending_submit.is_none());
}

#[test]
fn quit_keys_request_shutdown_in_supported_statuses() {
    for (status, key) in [
        (AppStatus::Ready, 'c'),
        (AppStatus::Ready, 'q'),
        (AppStatus::Connecting, 'q'),
        (AppStatus::Error, 'q'),
        (AppStatus::Error, 'c'),
    ] {
        let mut app = make_test_app();
        app.status = status;
        handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::CONTROL)),
        );
        assert!(app.shutdown_requested(), "{status:?}, Ctrl+{key}");
    }
}

#[test]
fn fullscreen_ctrl_c_returns_to_chat_without_stopping_active_turn() {
    let mut app = make_test_app();
    app.status = AppStatus::Running;
    app.session_runtime.prompt_suggestion = Some("Write focused tests".to_owned());
    app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::Config);
    app.terminal_lifecycle =
        TerminalLifecycleState::Running(SurfaceMode::Fullscreen(FullscreenView::Config));

    handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
    );

    assert_eq!(app.surface_mode, SurfaceMode::Chat);
    assert_eq!(app.status, AppStatus::Running);
    assert_eq!(app.session_runtime.prompt_suggestion.as_deref(), Some("Write focused tests"));
    assert!(!app.shutdown_requested());
    assert!(!app.turn.cancel_requested);
}

#[test]
fn repeated_ctrl_c_escalates_requested_shutdown() {
    let mut app = make_test_app();
    app.request_shutdown();

    handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
    );

    assert!(app.shutdown_forced());
}

#[test]
fn ctrl_c_clears_local_draft_before_quitting() {
    let mut app = make_test_app();
    app.input.set_text("draft");
    app.pending_submit = Some(app.input.snapshot());
    app.paste.pending_text = "queued paste".to_owned();
    app.input.image_payloads.push(crate::app::clipboard_image::ImageAttachment {
        data: "image-data".to_owned(),
        mime_type: "image/png".to_owned(),
    });

    handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
    );

    assert!(!app.shutdown_requested());
    assert!(app.input.is_empty());
    assert!(app.pending_submit.is_none());
    assert!(app.paste.pending_text.is_empty());
    assert!(app.input.image_payloads.is_empty());

    handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
    );

    assert!(app.shutdown_requested());
}

#[test]
fn terminal_event_outcome_carries_runtime_command() {
    let mut app = make_test_app();
    app.keymap = ResolvedKeymap::from_bindings([KeyBinding::new(
        KeyContext::Global,
        KeySpec::char('s', KeyModifiers::CONTROL),
        KeyAction::Terminal(TerminalAction::Suspend),
        KeyBindingSource::Config,
    )])
    .expect("custom test keymap should validate");

    let outcome = handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)),
    );

    assert!(outcome.changed);
    assert_eq!(outcome.runtime_command, Some(crate::app::keys::RuntimeCommand::SuspendProcess));
    #[cfg(unix)]
    {
        let mut app = make_test_app();
        app.status = AppStatus::Ready;
        app.input.set_text("edited draft");
        handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)),
        );
        assert_eq!(app.input.text(), "edited draf");
        let outcome = handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL)),
        );
        assert_eq!(outcome.runtime_command, Some(crate::app::keys::RuntimeCommand::SuspendProcess));
        assert_eq!(app.input.text(), "edited draf");
    }
}

#[test]
fn error_state_blocks_input_shortcuts() {
    let mut app = make_test_app();
    app.status = AppStatus::Error;
    app.input.set_text("seed");
    app.pending_submit = None;

    for key in [
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('@'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
    ] {
        handle_terminal_event(&mut app, Event::Key(key));
    }

    assert_eq!(app.input.text(), "seed");
    assert!(app.pending_submit.is_none());
}

#[test]
fn blocked_surfaces_ignore_paste_without_changing_the_draft() {
    for (status, surface, blocked) in [
        (AppStatus::Error, SurfaceMode::Chat, true),
        (AppStatus::Ready, SurfaceMode::Fullscreen(FullscreenView::Config), true),
        (AppStatus::Ready, SurfaceMode::Fullscreen(FullscreenView::Trusted), true),
        (AppStatus::Ready, SurfaceMode::Chat, false),
    ] {
        let mut app = make_test_app();
        app.status = status;
        app.surface_mode = surface;
        app.input.set_text("unfinished draft");
        handle_terminal_event(&mut app, Event::Paste("pasted payload".into()));
        assert_eq!(app.input.text(), "unfinished draft");
        assert_eq!(
            app.paste.pending_text,
            if blocked { "" } else { "pasted payload" },
            "{status:?}, {surface:?}"
        );
        assert!(app.pending_submit.is_none());
    }
}

#[test]
fn mention_owner_releases_back_to_input() {
    let mut app = make_test_app();
    show_config_slash_menu(&mut app);
    app.claim_focus_target(FocusTarget::Mention);

    handle_terminal_event(&mut app, Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));

    assert!(app.mention.is_none());
    assert_eq!(app.focus_owner(), FocusOwner::Input);
}

#[test]
fn config_status_routes_enter_to_close_not_chat_submit() {
    let mut app = make_test_app();
    let dir = tempfile::tempdir().expect("tempdir");
    app.settings_home_override = Some(dir.path().to_path_buf());
    app.cwd_raw = dir.path().to_string_lossy().to_string();
    crate::app::config::open(&mut app).expect("open settings");
    app.config.active_tab = crate::app::config::ConfigTab::Status;
    app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::Config);
    app.input.set_text("seed");

    handle_terminal_event(&mut app, Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));

    assert_eq!(app.surface_mode, SurfaceMode::Chat);
    assert_eq!(app.input.text(), "seed");
    assert!(app.pending_submit.is_none());
}

#[test]
#[cfg(windows)]
fn conpty_release_only_accent_survives_input_and_prompt_dispatch() {
    let mut app = make_test_app();
    let (connection, mut commands) = crate::agent::client::AgentConnection::test_channel();
    app.session_runtime.conn = Some(Rc::new(connection));
    app.status = AppStatus::Ready;
    for (ch, kind) in [
        ('e', KeyEventKind::Press),
        ('e', KeyEventKind::Release),
        ('\u{301}', KeyEventKind::Release),
    ] {
        handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new_with_kind(KeyCode::Char(ch), KeyModifiers::NONE, kind)),
        );
    }
    if let Some(action) = app.paste.burst.tick(Instant::now() + Duration::from_millis(500)) {
        crate::app::apply_paste_burst_flush(&mut app, action);
    }
    assert_eq!(app.input.text(), "e\u{301}");

    crate::app::input_submit::submit_input(&mut app);

    let crate::agent::wire::BridgeCommand::Prompt { chunks, .. } =
        commands.try_recv().expect("prompt admitted").command
    else {
        panic!("expected prompt");
    };
    assert_eq!(chunks[0].value.as_str(), Some("e\u{301}"));
    assert!(app.input.is_empty());
}

#[test]
fn non_paste_shortcut_release_is_ignored() {
    let mut app = make_test_app();
    let quit =
        |kind| Event::Key(KeyEvent::new_with_kind(KeyCode::Char('q'), KeyModifiers::CONTROL, kind));

    let release = handle_terminal_event(&mut app, quit(KeyEventKind::Release));

    assert_eq!(release, TerminalEventOutcome::ignored());
    assert!(!app.shutdown_requested());
    // The same shortcut acts on its press, so the release above was dropped by kind.
    handle_terminal_event(&mut app, quit(KeyEventKind::Press));
    assert!(app.shutdown_requested());
}

#[test]
fn trusted_view_accept_key_does_not_edit_chat_input() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(".claude.json");
    std::fs::write(&path, "{\n  \"projects\": {}\n}\n").expect("write");

    let mut app = make_test_app();
    app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::Trusted);
    app.input.set_text("seed");
    app.cwd_raw = dir.path().join("project").to_string_lossy().to_string();
    app.trust.preferences_path = Some(path);
    app.trust.status = crate::app::trust::TrustStatus::Untrusted;
    app.trust.project_key =
        crate::app::trust::store::normalize_project_key(std::path::Path::new(&app.cwd_raw));

    handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE)),
    );

    assert_eq!(app.surface_mode, SurfaceMode::Chat);
    assert_eq!(app.input.text(), "seed");
    assert!(app.paste.pending_text.is_empty());
    assert!(app.startup.mark_connection_started());
}

#[test]
fn session_picker_paste_edits_search_without_changing_the_chat_draft() {
    let mut app = make_test_app();
    app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::SessionPicker);

    app.input.set_text("unfinished question");
    handle_terminal_event(&mut app, Event::Paste("session-id".into()));

    assert!(app.paste.pending_text.is_empty());
    assert_eq!(app.input.text(), "unfinished question");
    assert_eq!(app.session_picker.query, "session-id");
}

#[test]
fn api_retry_updates_single_warning_notice() {
    let (mut app, mut commands) = app_with_bridge_connection();
    app.push_message_tracked(ChatMessage::new(
        MessageRole::System(Some(SystemSeverity::Info)),
        vec![MessageBlock::Text(TextBlock::from_complete("unrelated history notice"))],
        None,
    ));
    app.transcript.messages.push(user_msg("retry this request"));
    let history = message_snapshots(&app);
    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::ApiRetryUpdate {
            attempt: 1,
            max_retries: 4,
            retry_delay_ms: 1000.0,
            error_status: None,
            error: model::ApiRetryError::Unknown,
        }),
    );
    let notice_id = app.transcript.messages.last().expect("first retry notice").id;
    assert!(first_block_text(app.transcript.messages.last().unwrap()).contains("1/4"));
    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::ApiRetryUpdate {
            attempt: 2,
            max_retries: 4,
            retry_delay_ms: 1500.0,
            error_status: Some(529),
            error: model::ApiRetryError::ServerError,
        }),
    );

    assert_eq!(&message_snapshots(&app)[..history.len()], history);
    let [message] = &app.transcript.messages[history.len()..] else {
        panic!("retries must share one transcript notice");
    };
    assert_eq!(message.id, notice_id, "retry warning must update in place");
    let [MessageBlock::Notice(notice)] = message.blocks.as_slice() else {
        panic!("expected API retry notice");
    };
    assert_eq!(notice.severity, SystemSeverity::Warning);
    assert!(notice.text.text.contains("2/4"));
    assert!(notice.text.text.contains("HTTP 529"));
    assert!(notice.text.text.contains("1.5s"));
    assert!(!notice.text.text.contains("1/4"));
    for (error, label) in [
        (model::ApiRetryError::ModelNotFound, "model unavailable"),
        (model::ApiRetryError::OauthOrgNotAllowed, "account access not allowed"),
        (model::ApiRetryError::AccountOnHold, "account on hold"),
        (model::ApiRetryError::Overloaded, "service overloaded"),
        (model::ApiRetryError::Unknown, "connection error"),
    ] {
        handle_client_event(
            &mut app,
            session_update(model::SessionUpdate::ApiRetryUpdate {
                attempt: 3,
                max_retries: 4,
                retry_delay_ms: 250.0,
                error_status: None,
                error,
            }),
        );
        assert_eq!(&message_snapshots(&app)[..history.len()], history);
        let [message] = &app.transcript.messages[history.len()..] else {
            panic!("one retry notice")
        };
        assert_eq!(message.id, notice_id);
        let [MessageBlock::Notice(notice)] = message.blocks.as_slice() else {
            panic!("retry notice")
        };
        assert_eq!(notice.severity, SystemSeverity::Warning);
        assert!(notice.text.text.contains(label));
        assert!(notice.text.text.contains("3/4"));
        assert!(notice.text.text.contains("250ms"));
        assert!(!notice.text.text.contains("HTTP"));
        assert!(!notice.text.text.contains("2/4"));
        assert_ne!(app.status, AppStatus::Error);
        assert!(commands.try_recv().is_err(), "retry must not submit another prompt");
    }
}

#[test]
fn system_notice_update_uses_notice_lane() {
    let mut app = make_test_app();
    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::SystemNoticeUpdate {
            severity: model::SystemNoticeSeverity::Warning,
            message: "Plugin install failed.".to_owned(),
        }),
    );

    assert_eq!(app.transcript.messages.len(), 1);
    let MessageBlock::Notice(notice) = &app.transcript.messages[0].blocks[0] else {
        panic!("expected system notice");
    };
    assert_eq!(notice.severity, SystemSeverity::Warning);
    assert_eq!(notice.text.text, "Plugin install failed.");
}

#[test]
fn external_claude_message_is_rendered_as_non_human_input_with_provenance() {
    let mut app = make_test_app();
    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::ExternalMessageUpdate(model::ExternalMessageUpdate {
            content: "Please inspect the build.".to_owned(),
            source_message_uuid: Some("message-peer".to_owned()),
            origin: model::MessageOrigin {
                kind: "peer".to_owned(),
                subkind: None,
                from: Some("reported-route".to_owned()),
                name: Some("Build session".to_owned()),
                from_session: Some("session-peer".to_owned()),
                sender_task_id: None,
                verified_peer_pid: Some(4242),
                from_mode: Some("bypass".to_owned()),
            },
        })),
    );

    assert_eq!(app.transcript.messages.len(), 1);
    let MessageBlock::Text(block) = &app.transcript.messages[0].blocks[0] else {
        panic!("expected external message text block");
    };
    assert!(block.text.contains("Message from another Claude session"));
    assert!(block.text.contains("sender-reported name: Build session"));
    assert!(block.text.contains("session session-peer"));
    assert!(block.text.contains("verified connecting PID 4242"));
    assert!(block.text.contains("sender permission class: bypass"));
    assert!(block.text.contains("Please inspect the build."));
}

#[test]
fn available_commands_update_replaces_previous_commands() {
    let mut app = make_test_app();
    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::AvailableCommandsUpdate(
            model::AvailableCommandsUpdate::new(vec![model::AvailableCommand::new(
                "/old",
                "Old command",
            )]),
        )),
    );
    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::AvailableCommandsUpdate(
            model::AvailableCommandsUpdate::new(vec![
                model::AvailableCommand::new("/new", "New command").input_hint("<arg>"),
            ]),
        )),
    );

    assert_eq!(
        app.sdk_inventory.available_commands,
        vec![model::AvailableCommand::new("/new", "New command").input_hint("<arg>")]
    );
}

#[test]
fn conversation_reset_mounts_a_fresh_transcript_without_dropping_session_inventory() {
    let mut app = make_test_app();
    app.transcript.messages.push(user_msg("old conversation"));
    app.recent_sessions = vec![
        crate::app::RecentSessionInfo {
            session_id: "test-session".to_owned(),
            summary: "Old conversation title".to_owned(),
            last_modified_ms: 1,
            file_size_bytes: 2,
            cwd: Some("/test".to_owned()),
            git_branch: Some("main".to_owned()),
            custom_title: Some("Old conversation title".to_owned()),
            first_prompt: Some("prompt Old conversation title".to_owned()),
        },
        crate::app::RecentSessionInfo {
            session_id: "other-session".to_owned(),
            summary: "Other conversation title".to_owned(),
            last_modified_ms: 1,
            file_size_bytes: 2,
            cwd: Some("/test".to_owned()),
            git_branch: Some("main".to_owned()),
            custom_title: Some("Other conversation title".to_owned()),
            first_prompt: Some("prompt Other conversation title".to_owned()),
        },
    ];
    app.sdk_inventory.available_commands = vec![model::AvailableCommand::new("/remote", "Remote")];
    app.sdk_inventory.available_agents = vec![model::AvailableAgent::new("reviewer", "Reviews")];
    app.config.pending_session_title_change =
        Some(crate::app::config::PendingSessionTitleChangeState {
            session_id: "test-session".to_owned(),
            kind: crate::app::config::PendingSessionTitleChangeKind::Generate,
        });
    app.session_runtime.session_title = Some("Old conversation name".to_owned());

    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::ConversationReset {
            new_conversation_id: "conversation-2".to_owned(),
            trigger: Some("future-trigger".to_owned()),
            timestamp: Some("2026-10-01T12:00:00Z".to_owned()),
            user_message_uuid: None,
        }),
    );

    assert_eq!(app.transcript.messages.len(), 1);
    assert!(!app.transcript.messages.iter().any(|message| {
        message.blocks.iter().any(|block| {
            matches!(block, MessageBlock::Text(text) if text.text.contains("old conversation"))
        })
    }));
    assert_eq!(app.sdk_inventory.available_commands.len(), 1);
    assert_eq!(app.sdk_inventory.available_agents.len(), 1);
    assert_eq!(app.session_runtime.conversation_id.as_deref(), Some("conversation-2"));
    let active_session = app
        .recent_sessions
        .iter()
        .find(|session| session.session_id == "test-session")
        .expect("active session cache");
    assert!(active_session.custom_title.is_none());
    assert!(active_session.summary.is_empty());
    assert!(active_session.first_prompt.is_none());
    let other_session = app
        .recent_sessions
        .iter()
        .find(|session| session.session_id == "other-session")
        .expect("unrelated session cache");
    assert_eq!(other_session.custom_title.as_deref(), Some("Other conversation title"));
    assert_eq!(other_session.summary, "Other conversation title");
    assert_eq!(other_session.first_prompt.as_deref(), Some("prompt Other conversation title"));
    assert!(app.config.pending_session_title_change.is_none());
    // A reset drops the shown name with the conversation; the bridge sends
    // the title the session reports after it.
    assert_eq!(app.session_runtime.session_title, None);
    assert_eq!(app.status, AppStatus::Ready);
}

#[test]
fn prompt_suggestion_tab_accepts_empty_input() {
    let mut app = make_test_app();
    app.session_runtime.prompt_suggestion = Some("Write focused tests".to_owned());

    handle_normal_key(&mut app, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));

    assert_eq!(app.input.text(), "Write focused tests");
    assert!(app.session_runtime.prompt_suggestion.is_none());
}

#[test]
fn runtime_session_state_updates_status_with_guards() {
    let mut app = make_test_app();
    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::RuntimeSessionStateUpdate(
            model::RuntimeSessionState::Running,
        )),
    );
    assert_eq!(
        app.session_runtime.runtime_session_state,
        Some(model::RuntimeSessionState::Running)
    );
    assert!(matches!(app.status, AppStatus::Running));

    app.status = AppStatus::Error;
    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::RuntimeSessionStateUpdate(
            model::RuntimeSessionState::Idle,
        )),
    );
    assert!(matches!(app.status, AppStatus::Error));
}

#[test]
fn settings_parse_error_surfaces_system_error_message() {
    let mut app = make_test_app();
    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::SettingsParseError {
            file: Some("C:/work/.claude/settings.json".to_owned()),
            path: "permissions.allow".to_owned(),
            message: "Expected array".to_owned(),
        }),
    );

    assert_eq!(app.transcript.messages.len(), 1);
    assert!(matches!(
        app.transcript.messages[0].role,
        MessageRole::System(Some(SystemSeverity::Error))
    ));
    let MessageBlock::Text(text) = &app.transcript.messages[0].blocks[0] else {
        panic!("expected settings parse error text");
    };
    assert_eq!(
        text.text,
        "Settings parse error in C:/work/.claude/settings.json at permissions.allow: Expected array",
    );
}

#[test]
fn settings_parse_error_is_recorded_in_the_default_log() {
    let mut app = make_test_app();
    let records = crate::logging::test_capture::capture(|| {
        handle_client_event(
            &mut app,
            session_update(model::SessionUpdate::SettingsParseError {
                file: Some("settings.json".to_owned()),
                path: "permissions.allow".to_owned(),
                message: "Expected array".to_owned(),
            }),
        );
    });

    let record = records
        .iter()
        .find(|record| record["event_name"] == "settings_parse_error")
        .expect("settings parse error record");
    assert_eq!(record["level"], "WARN");
    assert_eq!(
        record["error_message"],
        "Settings parse error in settings.json at permissions.allow: Expected array"
    );
}

#[test]
fn clearing_the_composer_discards_held_input_and_allows_a_fresh_prompt() {
    use crate::agent::wire::BridgeCommand;
    use crate::app::test_support::{EventClock, press};
    for (prefix, typed) in [("old draft ", "ab"), ("old draft ", "abc"), ("", "ab"), ("", "abc")] {
        let clock = EventClock::new();
        let (mut app, mut commands) = app_with_bridge_connection();
        app.status = AppStatus::Ready;
        app.input.set_text(prefix);
        for ch in typed.chars() {
            press(&mut app, KeyCode::Char(ch));
        }
        handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        );
        assert!(!app.shutdown_requested());
        assert!(app.input.is_empty());
        if let Some(action) = app.paste.burst.tick(clock.at(1)) {
            crate::app::apply_paste_burst_flush(&mut app, action);
        }
        crate::app::finalize_pending_paste_event(&mut app);
        assert!(app.input.is_empty(), "cleared text must not return on the next tick");
        assert!(commands.try_recv().is_err());
        for (i, ch) in "fresh".chars().enumerate() {
            clock.at_millis(1500 + u64::try_from(i).unwrap() * 300);
            press(&mut app, KeyCode::Char(ch));
        }
        clock.at(4);
        press(&mut app, KeyCode::Enter);
        let BridgeCommand::Prompt { session_id, chunks, .. } =
            commands.try_recv().expect("fresh Prompt").command
        else {
            panic!("expected Prompt")
        };
        assert_eq!(session_id, "test-session");
        assert_eq!(
            chunks.iter().filter(|c| c.kind == "text").map(|c| c.value.clone()).collect::<Vec<_>>(),
            [serde_json::json!("fresh")]
        );
        while let Ok(envelope) = commands.try_recv() {
            assert!(!matches!(envelope.command, BridgeCommand::Prompt { .. }));
        }
    }
}

fn select_slash_for_render(app: &mut App, target: &str) {
    let limit = app.slash.visible().expect("slash menu").candidates.len();
    for _ in 0..limit {
        let menu = app.slash.visible().expect("slash menu");
        if menu.candidates[menu.dialog.selected].insert_value == target {
            return;
        }
        completion_key(app, KeyCode::Down);
    }
    panic!("missing slash target {target}");
}

fn assert_selected_slash_row(app: &App, target: &str) {
    let rows = crate::ui::input_rows::build_composer_hint_rows(app);
    let selected =
        rows.iter().filter(|r| r.spans.iter().any(|s| s.content.trim() == ">")).collect::<Vec<_>>();
    let [row] = selected.as_slice() else { panic!("one visible selected candidate required") };
    assert!(row.to_string().contains(target), "selection must belong to {target}: {row:?}");
}

#[test]
fn slash_completion_rows_follow_scrolling_and_deliberate_selection() {
    for (source, target, expected) in
        [("/help", "/a-help", "/a-help "), ("/effort high", "xhigh", "/effort xhigh ")]
    {
        let mut app = make_test_app();
        handle_client_event(
            &mut app,
            session_update(model::SessionUpdate::AvailableCommandsUpdate(
                model::AvailableCommandsUpdate::new(vec![model::AvailableCommand::new(
                    "a-help",
                    "Competing plugin",
                )]),
            )),
        );
        handle_client_event(
            &mut app,
            session_update(model::SessionUpdate::CurrentModelUpdate(
                model::CurrentModelUpdate::new(
                    model::CurrentModel::new("opus", "Opus", "Opus")
                        .supports_effort(true)
                        .supported_effort_levels(vec![
                            model::EffortLevel::High,
                            model::EffortLevel::XHigh,
                        ]),
                ),
            )),
        );
        app.input.set_text(source);
        app.input.set_cursor(0, 0);
        completion_key(&mut app, KeyCode::End);
        select_slash_for_render(&mut app, target);
        assert_selected_slash_row(&app, target);
        completion_key(&mut app, KeyCode::Enter);
        assert_eq!(app.pending_submit.as_ref().expect("handoff").lines.join("\n"), expected);
    }
    for target in ["/aaa-74", "/new-session", "/docs"] {
        let mut app = make_test_app();
        handle_client_event(
            &mut app,
            session_update(model::SessionUpdate::AvailableCommandsUpdate(
                model::AvailableCommandsUpdate::new(
                    (0..75)
                        .map(|i| {
                            model::AvailableCommand::new(format!("aaa-{i:02}"), "Plugin command")
                        })
                        .collect(),
                ),
            )),
        );
        app.input.set_text("/");
        app.input.set_cursor(0, 0);
        completion_key(&mut app, KeyCode::Right);
        assert!(
            !crate::ui::input_rows::build_composer_hint_rows(&app)
                .iter()
                .any(|r| r.to_string().contains(target))
        );
        select_slash_for_render(&mut app, target);
        assert_selected_slash_row(&app, target);
        assert!(
            !crate::ui::input_rows::build_composer_hint_rows(&app)
                .iter()
                .any(|r| r.to_string().contains("/aaa-00"))
        );
        completion_key(&mut app, if target == "/docs" { KeyCode::Enter } else { KeyCode::Tab });
        assert_eq!(app.input.text(), format!("{target} "));
        assert!(app.pending_submit.is_none());
        if target == "/docs" {
            assert!(
                crate::ui::input_rows::build_composer_hint_rows(&app)
                    .iter()
                    .any(|r| r.to_string().contains("commands"))
            );
        }
    }
}

#[test]
fn opaque_sdk_argument_help_renders_the_active_commands_hint() {
    for (input, expected, other) in [
        ("/clear ", "[conversation label]", "<deployment target>"),
        ("/reset ", "[conversation label]", "<deployment target>"),
        ("/ship staging", "<deployment target>", "[conversation label]"),
    ] {
        let mut app = make_test_app();
        handle_client_event(
            &mut app,
            session_update(model::SessionUpdate::AvailableCommandsUpdate(
                model::AvailableCommandsUpdate::new(vec![
                    model::AvailableCommand::new("clear", "Clear")
                        .aliases(vec!["reset".into()])
                        .input_hint("[conversation label]"),
                    model::AvailableCommand::new("deploy", "Deploy")
                        .aliases(vec!["ship".into()])
                        .input_hint("<deployment target>"),
                ]),
            )),
        );
        app.input.set_text(input);
        app.input.set_cursor(0, 0);
        completion_key(&mut app, KeyCode::End);
        assert!(crate::ui::input_rows::build_composer_hint_rows(&app).is_empty());
        for _ in 0..2 {
            completion_key(&mut app, KeyCode::Tab);
            let rows = crate::ui::input_rows::build_composer_hint_rows(&app);
            assert!(rows.iter().any(|r| r.to_string().contains(expected)));
            assert!(rows.iter().all(|r| !r.to_string().contains(other)));
            assert!(
                rows.iter().all(|r| r.spans.iter().all(|s| s.content.trim() != ">")),
                "opaque hint is not a selectable value"
            );
            assert_eq!(app.input.text(), input);
            assert!(app.pending_submit.is_none());
        }
        completion_key(&mut app, KeyCode::Enter);
        assert_eq!(app.pending_submit.as_ref().expect("handoff").lines.join("\n"), input);
    }
}

fn await_correlated_file_search(app: &mut App, query: &str, expected: &[&str]) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let event = app
            .file_index_event_rx
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("correlated matcher result");
        let crate::app::file_index::FileIndexEvent::MatchResult(result) = event else {
            panic!("the completed fixture index only emits matcher results");
        };
        // A matcher can answer before loading its initial index. The normal
        // result consumer schedules a follow-up for that incomplete answer.
        let settled = result.query == query
            && result.generation == app.file_index.generation
            && result.index_version == app.file_index.index_version
            && result.status == crate::app::file_index::FileIndexStatus::Complete;
        // Use the same consumer as file_index::drain_events, including its
        // sequence check. Requeueing could drain a newer result unseen here.
        if mention::apply_match_result(app, result) && settled {
            break;
        }
    }
    let menu = app.mention.as_ref().expect("active mention");
    assert_eq!(menu.query, query);
    let actual = menu
        .candidates
        .iter()
        .map(|c| c.rel_path.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(actual, expected.iter().copied().collect());
}

#[test]
fn spaced_file_completion_distinguishes_literal_paths_from_selected_matches() {
    for matching in [false, true] {
        let clock = crate::app::test_support::EventClock::new();
        let (mut app, mut commands) = app_with_bridge_connection();
        let fixture = tempfile::tempdir().expect("fixture root");
        app.cwd_raw = fixture.path().to_string_lossy().into_owned();
        app.file_index.root = Some(fixture.path().to_path_buf());
        app.file_index.respect_gitignore = app.config.respect_gitignore_effective();
        app.file_index.status = crate::app::file_index::FileIndexStatus::Complete;
        for path in ["docs/manual path.md", "docs/manual peer.md", "other.txt"] {
            app.file_index.entries.insert(
                path.into(),
                crate::app::file_index::FileCandidate {
                    rel_path: path.into(),
                    rel_path_lower: path.into(),
                    basename_lower: path.rsplit('/').next().unwrap().into(),
                    depth: usize::from(path.contains('/')),
                },
            );
        }
        app.input.set_text("@");
        app.input.set_cursor(0, 0);
        completion_key(&mut app, KeyCode::Right);
        completion_key(&mut app, KeyCode::Char(' '));
        assert_eq!(app.input.text(), "@ ");
        assert!(app.pending_submit.is_none());
        assert!(commands.try_recv().is_err());
        handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        );
        app.input.set_text("open @docs/manual keep suffix");
        app.input.set_cursor(0, 16);
        completion_key(&mut app, KeyCode::Right);
        await_correlated_file_search(
            &mut app,
            "docs/manual",
            &["docs/manual path.md", "docs/manual peer.md"],
        );
        clock.at(1);
        completion_key(&mut app, KeyCode::Char(' '));
        assert_eq!(app.input.text(), "open @docs/manual  keep suffix");
        assert!(app.pending_submit.is_none());
        for (i, ch) in (if matching { "p" } else { "missing" }).chars().enumerate() {
            clock.at_millis(1500 + u64::try_from(i).unwrap() * 300);
            completion_key(&mut app, KeyCode::Char(ch));
        }
        let (query, expected) = if matching {
            ("docs/manual p", &["docs/manual path.md", "docs/manual peer.md"][..])
        } else {
            ("docs/manual missing", &[][..])
        };
        await_correlated_file_search(&mut app, query, expected);
        if matching {
            for _ in 0..2 {
                let menu = app.mention.as_ref().unwrap();
                if menu.candidates[menu.dialog.selected].rel_path == "docs/manual peer.md" {
                    break;
                }
                completion_key(&mut app, KeyCode::Down);
            }
            assert_completion_menu(&app, expected, Some("docs/manual peer.md"));
        }
        completion_key(&mut app, KeyCode::Tab);
        assert_eq!(
            app.input.text(),
            if matching {
                "open @'docs/manual peer.md' keep suffix"
            } else {
                "open @'docs/manual missing' keep suffix"
            }
        );
        assert_eq!(app.input.cursor(), (0, 27));
        assert!(app.pending_submit.is_none());
        assert!(commands.try_recv().is_err());
        clock.at(5);
        completion_key(&mut app, KeyCode::Right);
        completion_key(&mut app, KeyCode::Char('!'));
        assert_eq!(
            app.input.text(),
            if matching {
                "open @'docs/manual peer.md' !keep suffix"
            } else {
                "open @'docs/manual missing' !keep suffix"
            }
        );
        assert!(app.pending_submit.is_none());
        assert!(commands.try_recv().is_err());
    }
}
