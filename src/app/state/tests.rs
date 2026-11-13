// SPDX-License-Identifier: Apache-2.0
use super::types::{ComposerAccess, ComposerBlockReason};
use super::*;
use crate::agent::model;
use pretty_assertions::assert_eq;
use ratatui::text::Line;
use std::time::Instant;

fn make_test_app() -> App {
    App::test_default()
}

fn assistant_text_block(text: &str) -> MessageBlock {
    MessageBlock::Text(TextBlock::from_complete(text))
}

fn user_text_message(text: &str) -> ChatMessage {
    ChatMessage::new(MessageRole::User, vec![assistant_text_block(text)], None)
}

fn system_text_message(text: &str) -> ChatMessage {
    ChatMessage::new(
        MessageRole::System(Some(SystemSeverity::Info)),
        vec![assistant_text_block(text)],
        None,
    )
}

fn assistant_tool_message(id: &str, status: model::ToolCallStatus) -> ChatMessage {
    let mut tool_call = crate::app::test_support::tool_call_info(id, status);
    tool_call.title = format!("tool {id}");
    tool_call.terminal_output = Some("x".repeat(1024));
    tool_call.terminal_output_len = 1024;
    ChatMessage::new(
        MessageRole::Assistant,
        vec![MessageBlock::ToolCall(Box::new(tool_call))],
        None,
    )
}

fn assistant_bash_tool_message(
    id: &str,
    status: model::ToolCallStatus,
    terminal_id: &str,
) -> ChatMessage {
    let mut tool_call = crate::app::test_support::tool_call_info(id, status);
    tool_call.title = format!("tool {id}");
    tool_call.sdk_tool_name = "Bash".to_owned();
    tool_call.kind = model::ToolKind::Execute;
    tool_call.terminal_id = Some(terminal_id.to_owned());
    tool_call.terminal_command = Some("echo hi".to_owned());
    tool_call.terminal_output = Some("x".repeat(1024));
    tool_call.terminal_output_len = 1024;
    ChatMessage::new(
        MessageRole::Assistant,
        vec![MessageBlock::ToolCall(Box::new(tool_call))],
        None,
    )
}

#[test]
fn composer_access_is_the_authoritative_status_policy() {
    let mut app = App::test_default();
    for (status, expected) in [
        (AppStatus::Connecting, ComposerAccess::DraftOnly),
        (AppStatus::CommandPending, ComposerAccess::Blocked(ComposerBlockReason::CommandPending)),
        (AppStatus::Ready, ComposerAccess::Active),
        (AppStatus::Thinking, ComposerAccess::Active),
        (AppStatus::Running, ComposerAccess::Active),
        (AppStatus::Error, ComposerAccess::Blocked(ComposerBlockReason::Error)),
    ] {
        app.status = status;
        assert_eq!(app.composer_access(), expected);
    }

    app.status = AppStatus::Ready;
    app.request_shutdown();
    assert_eq!(app.composer_access(), ComposerAccess::Blocked(ComposerBlockReason::Shutdown));
}

#[test]
fn clear_session_runtime_identity_resets_active_state_and_preserves_resumable_session_id() {
    let mut app = App::test_default();
    app.session_runtime.activate_session(crate::agent::model::SessionId::new("session-1"));
    app.session_runtime.conversation_id = Some("conversation-1".to_owned());
    app.session_runtime.current_model = Some(
        crate::agent::model::CurrentModel::new("sonnet", "Claude Sonnet", "Claude Sonnet")
            .authoritative(true),
    );
    app.session_runtime.mode = Some(crate::app::ModeState {
        current_mode_id: "plan".to_owned(),
        current_mode_name: "Plan".to_owned(),
        available_modes: Vec::new(),
    });
    app.session_runtime.session_usage.context_usage_percent = Some(62);
    app.session_runtime.session_usage.context_usage_in_flight = true;
    app.session_runtime.session_usage.context_usage_refresh_pending = true;
    app.session_runtime.session_usage.context_usage_last_requested_at = Some(Instant::now());

    app.clear_session_runtime_identity();

    assert!(app.session_runtime.session_id.is_none());
    assert!(app.session_runtime.conversation_id.is_none());
    assert_eq!(
        app.session_runtime.resumable_session_id().map(crate::agent::model::SessionId::as_str),
        Some("session-1")
    );
    assert!(app.session_runtime.current_model.is_none());
    assert!(app.session_runtime.mode.is_none());
    assert_eq!(app.session_runtime.session_usage, SessionUsageState::default());
}

#[test]
fn history_retention_counts_canonical_text_allocation_once() {
    let baseline = user_text_message("");
    let baseline_bytes = App::measure_message_bytes(&baseline);
    let mut source = String::with_capacity(4096);
    source.push_str("hello\n\nhow are you");
    let capacity = source.capacity();
    let message =
        ChatMessage::new(MessageRole::User, vec![MessageBlock::Text(TextBlock::new(source))], None);
    assert_eq!(App::measure_message_bytes(&message) - baseline_bytes, capacity);
    let mut app = make_test_app();
    app.push_message_tracked(message);
    assert_eq!(app.transcript.retained_history_bytes, baseline_bytes + capacity);
}

#[test]
fn detached_calls_survive_turn_exits_but_finish_when_the_session_ends() {
    let mut app = make_test_app();
    let mut tool =
        crate::app::test_support::tool_call_info("web-1", model::ToolCallStatus::Detached);
    tool.sdk_tool_name = "WebFetch".to_owned();
    tool.kind = model::ToolKind::Fetch;
    app.transcript.messages.push(ChatMessage::new(
        MessageRole::Assistant,
        vec![MessageBlock::ToolCall(Box::new(tool))],
        None,
    ));
    app.index_tool_call("web-1".to_owned(), 0, 0);
    app.sync_tool_call_history(0, 0);
    for status in [model::ToolCallStatus::Completed, model::ToolCallStatus::Failed] {
        app.finalize_turn_runtime_artifacts(status);
        let MessageBlock::ToolCall(tool) = &app.transcript.messages[0].blocks[0] else {
            panic!("expected tool call")
        };
        assert_eq!(tool.status, model::ToolCallStatus::Detached);
    }
    app.finalize_session_runtime_artifacts(model::ToolCallStatus::Failed);
    let MessageBlock::ToolCall(tool) = &app.transcript.messages[0].blocks[0] else {
        panic!("expected tool call")
    };
    assert_eq!(tool.status, model::ToolCallStatus::Failed);
    let results: Vec<_> = app
        .transcript
        .messages
        .iter()
        .flat_map(|message| &message.blocks)
        .filter_map(|block| match block {
            MessageBlock::ToolResult { tool, .. } => Some(tool),
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, "web-1");
    assert_eq!(results[0].status, model::ToolCallStatus::Failed);
}

mod retention_audit;
