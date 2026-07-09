// SPDX-License-Identifier: Apache-2.0
// =====
// TESTS: 40
// =====

use super::*;
use crate::agent::error_handling::TurnErrorClass;
use crate::agent::events::ClientEvent;
use crate::agent::events::ServiceStatusSeverity;
use crate::agent::events::TerminalProcess;
use crate::app::keymap::{
    KeyAction, KeyBinding, KeyBindingSource, KeyContext, KeySpec, ResolvedKeymap, TerminalAction,
};
use crate::app::slash::{SlashCandidate, SlashContext, SlashState};
use crate::app::{
    BlockCache, CancelOrigin, ChatRebuildKind, ComposerRenderState, FocusOwner, FocusTarget,
    FullscreenView, InlinePermission, InlineQuestion, LiveRegionRenderState, ReleaseReason,
    SurfaceMode, TerminalLifecycleState, TextBlockSpacing, ToolCallInfo, ToolCallScope,
    UsageSnapshot, UsageSourceKind, mention,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use pretty_assertions::assert_eq;
use std::fmt::Write as _;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

// Helper: build a minimal ToolCallInfo with given id + status

fn tool_call(id: &str, status: model::ToolCallStatus) -> ToolCallInfo {
    ToolCallInfo {
        id: id.into(),
        source_message_uuids: Vec::new(),
        title: id.into(),
        sdk_tool_name: "Read".into(),
        raw_input: None,
        raw_input_bytes: 0,
        locations: Vec::new(),
        output_metadata: None,
        task_metadata: None,
        status,
        content: vec![],
        hidden: false,
        terminal_id: None,
        terminal_command: None,
        terminal_output: None,
        terminal_output_len: 0,
        terminal_bytes_seen: 0,
        terminal_snapshot_mode: crate::app::TerminalSnapshotMode::AppendOnly,
        cache: BlockCache::default(),
        pending_permission: None,
        pending_question: None,
    }
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

#[test]
fn update_install_command_is_documented_npm_command() {
    assert_eq!(session::update_install_command(), "npm install -g claude-code-rust");
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
        MessageBlock::Notice(block) => {
            BlockSnapshot::Notice { severity: block.severity, text: block.text.text.clone() }
        }
        MessageBlock::ToolCall(tool_call) => BlockSnapshot::ToolCall {
            id: tool_call.id.clone(),
            title: tool_call.title.clone(),
            status: tool_call.status,
            hidden: tool_call.hidden,
        },
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
        width: 90,
        hint_rows: 1,
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
    let mut app = App::test_default();
    app.transcript.messages.push(assistant_msg(vec![
        source_text("stale", "old-assistant"),
        source_text("keep", "new-assistant"),
    ]));

    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate(transcript_retraction(
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
    let mut app = App::test_default();
    let mut stale_tool = tool_call("tool-old", model::ToolCallStatus::Completed);
    stale_tool.source_message_uuids = vec!["assistant-tool".to_owned(), "user-result".to_owned()];
    stale_tool.sdk_tool_name = "Bash".to_owned();
    stale_tool.terminal_id = Some("term-old".to_owned());
    app.transcript.messages.push(assistant_msg(vec![
        MessageBlock::ToolCall(Box::new(stale_tool)),
        source_text("replacement", "assistant-new"),
    ]));
    app.index_tool_call("tool-old".to_owned(), 0, 0);
    app.sync_terminal_tool_call("term-old".to_owned(), 0, 0);

    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate(transcript_retraction(
            vec!["user-result"],
            model::TranscriptRetractionReason::ModelFallback,
        )),
    );

    assert!(app.lookup_tool_call("tool-old").is_none());
    assert!(app.terminal_tool_calls().is_empty());
    assert_eq!(app.transcript.messages.len(), 1);
    assert_eq!(app.transcript.messages[0].blocks.len(), 1);
    let MessageBlock::Text(block) = &app.transcript.messages[0].blocks[0] else {
        panic!("expected replacement text");
    };
    assert_eq!(block.text, "replacement");
}

#[test]
fn transcript_retraction_then_replacement_leaves_canonical_assistant_content() {
    let mut app = App::test_default();
    app.transcript.messages.push(assistant_msg(vec![source_text("stale", "assistant-old")]));

    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate(transcript_retraction(
            vec!["assistant-old"],
            model::TranscriptRetractionReason::AssistantSupersedes,
        )),
    );
    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::AgentMessageChunk(
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
        Some(MessageBlock::ToolCall(_)) => panic!("expected text-like block, found tool call"),
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

fn is_update_notice_message(msg: &ChatMessage) -> bool {
    matches!(msg.role, MessageRole::System(Some(SystemSeverity::Warning)))
        && first_block_text(msg).contains(session::update_install_command())
}

// shorten_tool_title

#[test]
fn shorten_unix_path() {
    let result =
        tool_calls::shorten_tool_title("Read /home/user/project/src/main.rs", "/home/user/project");
    assert_eq!(result, "Read src/main.rs");
}

#[test]
fn register_tool_call_scope_treats_agent_as_subagent_root() {
    let mut app = make_test_app();
    let scope = tool_calls::register_tool_call_scope(&mut app, "tool-agent", "Agent", None);
    assert_eq!(scope, ToolCallScope::SubagentRoot);
}

#[test]
fn register_tool_call_scope_treats_task_as_subagent_root() {
    let mut app = make_test_app();
    let scope = tool_calls::register_tool_call_scope(&mut app, "tool-task", "Task", None);
    assert_eq!(scope, ToolCallScope::SubagentRoot);
}

#[test]
fn register_tool_call_scope_uses_explicit_parent_for_subagent_child() {
    let mut app = make_test_app();
    let scope =
        tool_calls::register_tool_call_scope(&mut app, "tool-child", "Bash", Some("tool-parent"));
    assert_eq!(
        scope,
        ToolCallScope::SubagentChild { parent_tool_use_id: "tool-parent".to_owned() }
    );
}

/// Regression: when a Task was cancelled mid-turn, `active_task_ids` was never cleared
/// because `finalize_in_progress_tool_calls` doesn't call `remove_active_task` and
/// `clear_tool_scope_tracking` (called on `TurnComplete`) did not clear `active_task_ids`.
/// The leaked ID caused main-agent tools on the next turn to be classified as Subagent,
/// which eventually caused main-agent tools to inherit the wrong scope.
#[test]
fn turn_complete_after_cancelled_task_leaves_no_stale_active_task_ids() {
    let mut app = make_test_app();

    // Simulate a Task tool call arriving as InProgress (no Completed update will follow)
    let task_tc = model::ToolCall::new("task-1", "Research")
        .kind(model::ToolKind::Think)
        .status(model::ToolCallStatus::InProgress)
        .meta(serde_json::json!({"claudeCode": {"toolName": "Task"}}));
    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::ToolCall(task_tc)),
    );
    assert!(app.turn.active_task_ids.contains("task-1"), "task must be tracked while InProgress");

    // User cancels then TurnComplete finalizes the turn
    handle_client_event(&mut app, ClientEvent::TurnCancelled);
    handle_client_event(&mut app, ClientEvent::TurnComplete { terminal_reason: None });

    // Stale task ID must be gone after turn boundary
    assert!(app.turn.active_task_ids.is_empty(), "stale task id must not survive TurnComplete");

    // Next turn: a normal main-agent Glob must get MainAgent scope, not Subagent
    let glob_tc = model::ToolCall::new("glob-1", "Glob **/*.rs")
        .kind(model::ToolKind::Search)
        .status(model::ToolCallStatus::InProgress)
        .meta(serde_json::json!({"claudeCode": {"toolName": "Glob"}}));
    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::ToolCall(glob_tc)),
    );
    assert_eq!(
        app.tool_call_scope("glob-1"),
        Some(ToolCallScope::MainAgent),
        "main-agent tool must not be misclassified as Subagent after stale task is cleared"
    );
}

#[test]
fn shorten_windows_path() {
    let result = tool_calls::shorten_tool_title(
        "Read C:\\Users\\me\\project\\src\\main.rs",
        "C:\\Users\\me\\project",
    );
    assert_eq!(result, "Read src/main.rs");
}

#[test]
fn shorten_no_match_returns_original() {
    let result = tool_calls::shorten_tool_title("Read /other/path/file.rs", "/home/user/project");
    assert_eq!(result, "Read /other/path/file.rs");
}

// shorten_tool_title

#[test]
fn shorten_empty_cwd() {
    let result = tool_calls::shorten_tool_title("Read /some/path/file.rs", "");
    assert_eq!(result, "Read /some/path/file.rs");
}

#[test]
fn shorten_cwd_with_trailing_slash() {
    let result =
        tool_calls::shorten_tool_title("Read /home/user/project/file.rs", "/home/user/project/");
    assert_eq!(result, "Read file.rs");
}

#[test]
fn shorten_title_is_just_path() {
    let result = tool_calls::shorten_tool_title("/home/user/project/file.rs", "/home/user/project");
    assert_eq!(result, "file.rs");
}

#[test]
fn shorten_mixed_separators() {
    let result = tool_calls::shorten_tool_title(
        "Read C:/Users/me/project/src/lib.rs",
        "C:\\Users\\me\\project",
    );
    assert_eq!(result, "Read src/lib.rs");
}

#[test]
fn shorten_empty_title() {
    assert_eq!(tool_calls::shorten_tool_title("", "/some/cwd"), "");
}

#[test]
fn shorten_title_no_path_at_all() {
    assert_eq!(tool_calls::shorten_tool_title("Read", "/home/user"), "Read");
    assert_eq!(tool_calls::shorten_tool_title("Write something", "/proj"), "Write something");
}

#[test]
fn shorten_title_equals_cwd_exactly() {
    // Title IS the cwd path - after stripping, nothing left
    let result = tool_calls::shorten_tool_title("/home/user/project", "/home/user/project");
    // The cwd+/ won't match because title doesn't have trailing content after cwd
    // cwd_norm = "/home/user/project/", title doesn't contain that
    assert_eq!(result, "/home/user/project");
}

// shorten_tool_title

#[test]
fn shorten_partial_match_no_false_positive() {
    let result = tool_calls::shorten_tool_title("Read /home/username/file.rs", "/home/user");
    assert_eq!(result, "Read /home/username/file.rs");
}

#[test]
fn shorten_deeply_nested_path() {
    let cwd = "/a/b/c/d/e/f/g";
    let title = "Read /a/b/c/d/e/f/g/h/i/j.rs";
    let result = tool_calls::shorten_tool_title(title, cwd);
    assert_eq!(result, "Read h/i/j.rs");
}

#[test]
fn shorten_cwd_appears_multiple_times() {
    let result = tool_calls::shorten_tool_title("Diff /proj/a.rs /proj/b.rs", "/proj");
    assert_eq!(result, "Diff a.rs b.rs");
}

/// Spaces in path (real Windows path with spaces).
#[test]
fn shorten_spaces_in_path() {
    let result = tool_calls::shorten_tool_title(
        "Read C:\\Users\\Simon Peter Rothgang\\Desktop\\project\\src\\main.rs",
        "C:\\Users\\Simon Peter Rothgang\\Desktop\\project",
    );
    assert_eq!(result, "Read src/main.rs");
}

/// Unicode characters in path components.
#[test]
fn shorten_unicode_in_path() {
    let result = tool_calls::shorten_tool_title(
        "Read /home/\u{00FC}ser/\u{30D7}\u{30ED}\u{30B8}\u{30A7}\u{30AF}\u{30C8}/src/lib.rs",
        "/home/\u{00FC}ser/\u{30D7}\u{30ED}\u{30B8}\u{30A7}\u{30AF}\u{30C8}",
    );
    assert_eq!(result, "Read src/lib.rs");
}

/// Root as cwd (Unix).
#[test]
fn shorten_cwd_is_root_unix() {
    // cwd = "/" => with_sep = "/", so "/foo/bar.rs".contains("/") => replaces
    let result = tool_calls::shorten_tool_title("Read /foo/bar.rs", "/");
    // "/" is first path component = "" (empty), heuristic check uses "" which is in everything
    // After normalization: cwd = "/", with_sep = "/", title contains "/" => replaces ALL "/"
    assert_eq!(result, "Read foobar.rs");
}

/// Root as cwd (Windows).
#[test]
fn shorten_cwd_is_drive_root_windows() {
    let result = tool_calls::shorten_tool_title("Read C:\\src\\main.rs", "C:\\");
    assert_eq!(result, "Read src/main.rs");
}

/// Very long path (stress test).
#[test]
fn shorten_very_long_path() {
    let segments: String = (0..50).fold(String::new(), |mut s, i| {
        use std::fmt::Write;
        write!(s, "/seg{i}").unwrap();
        s
    });
    let cwd = segments.clone();
    let title = format!("Read {segments}/deep/file.rs");
    let result = tool_calls::shorten_tool_title(&title, &cwd);
    assert_eq!(result, "Read deep/file.rs");
}

/// Case sensitivity: paths are case-sensitive.
#[test]
fn shorten_case_sensitive() {
    let result =
        tool_calls::shorten_tool_title("Read /Home/User/Project/file.rs", "/home/user/project");
    // Different case, so the first-component heuristic "home" matches "Home"?
    // No: cwd_start = "home", title doesn't contain "home" (has "Home") => early return
    assert_eq!(result, "Read /Home/User/Project/file.rs");
}

/// Cwd that is a prefix at directory boundary but not at cwd boundary.
#[test]
fn shorten_cwd_prefix_boundary() {
    // cwd="/pro" should NOT strip from "/project/file.rs"
    let result = tool_calls::shorten_tool_title("Read /project/file.rs", "/pro");
    // cwd_start = "pro", title contains "pro" (in "project") => proceeds to normalize
    // with_sep = "/pro/", title_norm = "Read /project/file.rs", doesn't contain "/pro/"
    assert_eq!(result, "Read /project/file.rs");
}

#[test]
fn split_index_prefers_double_newline() {
    let text = "first\n\nsecond";
    let split_at = streaming::find_text_block_split_index(text);
    assert_eq!(split_at, Some("first\n\n".len()));
}

#[test]
fn split_index_soft_limit_prefers_newline() {
    use super::super::default_cache_split_policy;
    let prefix = "a".repeat(default_cache_split_policy().soft_limit_bytes - 1);
    let text = format!("{prefix}\n{}", "b".repeat(32));
    let split_at = streaming::find_text_block_split_index(&text).expect("expected split index");
    assert_eq!(&text[..split_at], format!("{prefix}\n"));
}

#[test]
fn split_index_hard_limit_uses_sentence_when_needed() {
    use super::super::default_cache_split_policy;
    let prefix = "a".repeat(default_cache_split_policy().hard_limit_bytes + 32);
    let text = format!("{prefix}. tail");
    let split_at = streaming::find_text_block_split_index(&text).expect("expected split index");
    assert_eq!(&text[..split_at], format!("{prefix}."));
}

#[test]
fn split_index_ignores_double_newline_inside_code_fence() {
    let text = "```\nline1\n\nline2\n```";
    assert!(streaming::find_text_block_split_index(text).is_none());
}

#[test]
fn agent_message_chunk_splits_into_frozen_text_blocks() {
    let mut app = make_test_app();
    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::AgentMessageChunk(
            model::ContentChunk::new(model::ContentBlock::Text(model::TextContent::new(
                "p1\n\np2\n\np3",
            ))),
        )),
    );

    assert_eq!(app.transcript.messages.len(), 1);
    let Some(last) = app.transcript.messages.last() else {
        panic!("missing assistant message");
    };
    assert!(matches!(last.role, MessageRole::Assistant));
    assert_eq!(last.blocks.len(), 3);
    let Some(MessageBlock::Text(b1)) = last.blocks.first() else {
        panic!("expected first text block");
    };
    let Some(MessageBlock::Text(b2)) = last.blocks.get(1) else {
        panic!("expected second text block");
    };
    let Some(MessageBlock::Text(b3)) = last.blocks.get(2) else {
        panic!("expected third text block");
    };
    assert_eq!(b1.text, "p1\n\n");
    assert_eq!(b2.text, "p2\n\n");
    assert_eq!(b3.text, "p3");
    assert_eq!(b1.trailing_spacing, TextBlockSpacing::ParagraphBreak);
    assert_eq!(b2.trailing_spacing, TextBlockSpacing::ParagraphBreak);
    assert_eq!(b3.trailing_spacing, TextBlockSpacing::None);
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
        ClientEvent::SessionUpdate(model::SessionUpdate::AgentMessageChunk(
            model::ContentChunk::new(model::ContentBlock::Text(model::TextContent::new(&table))),
        )),
    );

    assert!(matches!(app.status, AppStatus::Running));
    let serialized = crate::ui::inline_chat_rows::serialize_live_rows_with_boundaries_excluding(
        &mut app,
        180,
        &std::collections::BTreeSet::new(),
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

// has_in_progress_tool_calls

fn make_test_app() -> App {
    App::test_default()
}

fn test_current_model(model_name: &str) -> model::CurrentModel {
    model::CurrentModel::new(model_name, model_name, model_name).authoritative(true)
}

fn canonical_messages_contain_text(app: &App, expected: &str) -> bool {
    app.transcript.messages.iter().any(|message| {
        message.blocks.iter().any(|block| match block {
            MessageBlock::Text(text) => text.text == expected,
            MessageBlock::Notice(notice) => notice.text.text == expected,
            MessageBlock::ToolCall(_)
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
        history_updates: Vec::new(),
    }
}

fn app_with_bridge_connection()
-> (App, tokio::sync::mpsc::UnboundedReceiver<crate::agent::wire::CommandEnvelope>) {
    let mut app = make_test_app();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    app.session_runtime.conn = Some(Rc::new(crate::agent::client::AgentConnection::new(tx)));
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
    handle_client_event(&mut app, ClientEvent::SessionUpdate(model::SessionUpdate::ToolCall(tc)));

    let fields = model::ToolCallUpdateFields::new()
        .status(model::ToolCallStatus::Completed)
        .raw_output(serde_json::json!("line 1\nline 2"));
    let update = model::ToolCallUpdate::new("tc-exec", fields);
    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::ToolCallUpdate(update)),
    );

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
    handle_client_event(&mut app, ClientEvent::SessionUpdate(model::SessionUpdate::ToolCall(tc)));

    let fields = model::ToolCallUpdateFields::new()
        .raw_input(serde_json::json!({ "command": "Get-ChildItem" }));
    let update = model::ToolCallUpdate::new("tc-pwsh", fields);
    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::ToolCallUpdate(update)),
    );

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
fn late_tool_update_for_removed_tool_does_not_corrupt_active_task_set() {
    let mut app = make_test_app();
    app.transcript.messages.push(assistant_msg(vec![MessageBlock::ToolCall(Box::new(tool_call(
        "tool-stale",
        model::ToolCallStatus::Completed,
    )))]));
    app.index_tool_call("tool-stale".into(), 0, 0);
    app.register_tool_call_scope(
        "tool-stale".into(),
        ToolCallScope::SubagentChild { parent_tool_use_id: "task-1".to_owned() },
    );

    let removed = app.remove_message_tracked(0);
    assert!(removed.is_some());
    assert_eq!(app.tool_call_scope("tool-stale"), None);

    let update = model::ToolCallUpdate::new(
        "tool-stale",
        model::ToolCallUpdateFields::new().status(model::ToolCallStatus::InProgress),
    );
    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::ToolCallUpdate(update)),
    );

    assert!(app.turn.active_task_ids.is_empty());
}

#[test]
fn repeated_tool_call_updates_existing_execute_snapshot_state() {
    let mut app = make_test_app();
    app.terminals.borrow_mut().insert(
        "term-2".to_owned(),
        TerminalProcess {
            child: None,
            output_buffer: Arc::new(Mutex::new(Vec::new())),
            command: "echo second".to_owned(),
        },
    );

    let first = model::ToolCall::new("tc-dup", "Terminal")
        .kind(model::ToolKind::Execute)
        .status(model::ToolCallStatus::InProgress)
        .content(vec![model::ToolCallContent::Terminal(model::TerminalToolCallContent::new(
            "term-1",
        ))])
        .raw_output(serde_json::json!("first"));
    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::ToolCall(first)),
    );

    let second = model::ToolCall::new("tc-dup", "Terminal")
        .kind(model::ToolKind::Execute)
        .status(model::ToolCallStatus::InProgress)
        .content(vec![model::ToolCallContent::Terminal(model::TerminalToolCallContent::new(
            "term-2",
        ))])
        .raw_output(serde_json::json!("second"));
    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::ToolCall(second)),
    );

    let (mi, bi) = app.lookup_tool_call("tc-dup").expect("tool call not indexed");
    let MessageBlock::ToolCall(tc) = &app.transcript.messages[mi].blocks[bi] else {
        panic!("expected tool call block");
    };
    assert_eq!(tc.terminal_output.as_deref(), Some("second"));
    assert_eq!(tc.terminal_id.as_deref(), Some("term-2"));
    assert_eq!(tc.terminal_command.as_deref(), Some("echo second"));
    assert!(app.terminal_tool_calls().iter().any(|entry| entry.terminal_id == "term-2"
        && entry.msg_idx == mi
        && entry.block_idx == bi));
    assert!(app.terminal_tool_calls().iter().all(|entry| entry.terminal_id != "term-1"));
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
    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::ToolCall(todo_call)),
    );

    let update = model::ToolCallUpdate::new(
        "tc-todo-update",
        model::ToolCallUpdateFields::new().raw_input(serde_json::json!({})),
    );
    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::ToolCallUpdate(update)),
    );

    assert_eq!(app.sdk_inventory.tasks.len(), 1);
    assert_eq!(app.sdk_inventory.tasks[0].task_id, "task-1");
    assert_eq!(app.sdk_inventory.tasks[0].subject, "Existing task");
    assert_eq!(app.sdk_inventory.tasks[0].status, model::TaskStatus::InProgress);
}

#[test]
fn has_in_progress_empty_messages() {
    let app = make_test_app();
    assert!(!tool_calls::has_in_progress_tool_calls(&app));
}

#[test]
fn has_in_progress_no_tool_calls() {
    let mut app = make_test_app();
    app.transcript
        .messages
        .push(assistant_msg(vec![MessageBlock::Text(TextBlock::from_complete("hello"))]));
    assert!(!tool_calls::has_in_progress_tool_calls(&app));
}

#[test]
fn has_in_progress_with_pending_tool() {
    let mut app = make_test_app();
    app.transcript.messages.push(assistant_msg(vec![MessageBlock::ToolCall(Box::new(tool_call(
        "tc1",
        model::ToolCallStatus::Pending,
    )))]));
    app.bind_active_turn_assistant_to_tail();
    assert!(tool_calls::has_in_progress_tool_calls(&app));
}

#[test]
fn has_in_progress_with_in_progress_tool() {
    let mut app = make_test_app();
    app.transcript.messages.push(assistant_msg(vec![MessageBlock::ToolCall(Box::new(tool_call(
        "tc1",
        model::ToolCallStatus::InProgress,
    )))]));
    app.bind_active_turn_assistant_to_tail();
    assert!(tool_calls::has_in_progress_tool_calls(&app));
}

#[test]
fn has_in_progress_all_completed() {
    let mut app = make_test_app();
    app.transcript.messages.push(assistant_msg(vec![MessageBlock::ToolCall(Box::new(tool_call(
        "tc1",
        model::ToolCallStatus::Completed,
    )))]));
    assert!(!tool_calls::has_in_progress_tool_calls(&app));
}

#[test]
fn has_in_progress_all_failed() {
    let mut app = make_test_app();
    app.transcript.messages.push(assistant_msg(vec![MessageBlock::ToolCall(Box::new(tool_call(
        "tc1",
        model::ToolCallStatus::Failed,
    )))]));
    assert!(!tool_calls::has_in_progress_tool_calls(&app));
}

// has_in_progress_tool_calls

#[test]
fn has_in_progress_user_message_last() {
    let mut app = make_test_app();
    app.transcript.messages.push(user_msg("hi"));
    assert!(!tool_calls::has_in_progress_tool_calls(&app));
}

/// Without an explicit owner, in-progress tools do not count even if the last assistant has them.
#[test]
fn has_in_progress_requires_explicit_owner() {
    let mut app = make_test_app();
    app.transcript.messages.push(assistant_msg(vec![MessageBlock::ToolCall(Box::new(tool_call(
        "tc1",
        model::ToolCallStatus::InProgress,
    )))]));
    app.transcript.messages.push(user_msg("thanks"));
    assert!(!tool_calls::has_in_progress_tool_calls(&app));
}

/// The owned assistant decides the result even when another assistant trails later.
#[test]
fn has_in_progress_uses_owned_assistant_not_latest_assistant() {
    let mut app = make_test_app();
    app.transcript.messages.push(assistant_msg(vec![MessageBlock::ToolCall(Box::new(tool_call(
        "tc1",
        model::ToolCallStatus::InProgress,
    )))]));
    app.transcript.messages.push(user_msg("ok"));
    app.transcript.messages.push(assistant_msg(vec![MessageBlock::ToolCall(Box::new(tool_call(
        "tc2",
        model::ToolCallStatus::Completed,
    )))]));
    app.bind_active_turn_assistant(0);
    assert!(tool_calls::has_in_progress_tool_calls(&app));
}

#[test]
fn has_in_progress_mixed_completed_and_pending() {
    let mut app = make_test_app();
    app.transcript.messages.push(assistant_msg(vec![
        MessageBlock::ToolCall(Box::new(tool_call("tc1", model::ToolCallStatus::Completed))),
        MessageBlock::ToolCall(Box::new(tool_call("tc2", model::ToolCallStatus::InProgress))),
    ]));
    app.bind_active_turn_assistant_to_tail();
    assert!(tool_calls::has_in_progress_tool_calls(&app));
}

/// Text blocks mixed with tool calls - text blocks are correctly skipped.
#[test]
fn has_in_progress_text_and_tools_mixed() {
    let mut app = make_test_app();
    app.transcript.messages.push(assistant_msg(vec![
        MessageBlock::Text(TextBlock::from_complete("thinking...")),
        MessageBlock::ToolCall(Box::new(tool_call("tc1", model::ToolCallStatus::Completed))),
        MessageBlock::Text(TextBlock::from_complete("done")),
    ]));
    assert!(!tool_calls::has_in_progress_tool_calls(&app));
}

/// Stress: 100 completed tool calls + 1 pending at the end.
#[test]
fn has_in_progress_stress_100_tools_one_pending() {
    let mut app = make_test_app();
    let mut blocks: Vec<MessageBlock> = (0..100)
        .map(|i| {
            MessageBlock::ToolCall(Box::new(tool_call(
                &format!("tc{i}"),
                model::ToolCallStatus::Completed,
            )))
        })
        .collect();
    blocks.push(MessageBlock::ToolCall(Box::new(tool_call(
        "tc_pending",
        model::ToolCallStatus::Pending,
    ))));
    app.transcript.messages.push(assistant_msg(blocks));
    app.bind_active_turn_assistant_to_tail();
    assert!(tool_calls::has_in_progress_tool_calls(&app));
}

/// Stress: 100 completed tool calls, none pending.
#[test]
fn has_in_progress_stress_100_tools_all_done() {
    let mut app = make_test_app();
    let blocks: Vec<MessageBlock> = (0..100)
        .map(|i| {
            MessageBlock::ToolCall(Box::new(tool_call(
                &format!("tc{i}"),
                model::ToolCallStatus::Completed,
            )))
        })
        .collect();
    app.transcript.messages.push(assistant_msg(blocks));
    assert!(!tool_calls::has_in_progress_tool_calls(&app));
}

/// Mix of Failed and Completed - neither counts as in-progress.
#[test]
fn has_in_progress_failed_and_completed_mix() {
    let mut app = make_test_app();
    app.transcript.messages.push(assistant_msg(vec![
        MessageBlock::ToolCall(Box::new(tool_call("tc1", model::ToolCallStatus::Completed))),
        MessageBlock::ToolCall(Box::new(tool_call("tc2", model::ToolCallStatus::Failed))),
        MessageBlock::ToolCall(Box::new(tool_call("tc3", model::ToolCallStatus::Completed))),
    ]));
    assert!(!tool_calls::has_in_progress_tool_calls(&app));
}

/// Empty assistant message (no blocks at all).
#[test]
fn has_in_progress_empty_assistant_blocks() {
    let mut app = make_test_app();
    app.transcript.messages.push(assistant_msg(vec![]));
    assert!(!tool_calls::has_in_progress_tool_calls(&app));
}

// make_test_app - verify defaults

#[test]
fn test_app_defaults() {
    let app = make_test_app();
    assert!(app.transcript.messages.is_empty());
    assert_eq!(app.surface_mode, SurfaceMode::Chat);
    assert_eq!(app.surface_mode, SurfaceMode::Chat);
    assert_eq!(app.terminal_lifecycle, TerminalLifecycleState::Running(SurfaceMode::Chat));
    assert!(!app.surface_dirty.fullscreen.redraw);
    assert!(!app.surface_dirty.terminal_mode);
    assert!(!app.should_quit);
    assert!(app.session_runtime.session_id.is_none());
    assert_eq!(app.files_accessed, 0);
    assert!(app.turn.pending_interaction_ids.is_empty());
    assert_eq!(app.surface_dirty.chat.rebuild, ChatRebuildKind::None);
    assert!(app.sdk_inventory.tasks.is_empty());
    assert!(app.mention.is_none());
    assert!(!app.turn.cancelled_pending_hint);
    assert!(matches!(app.status, AppStatus::Ready));
}

#[test]
fn resize_marks_chat_surface_dirty_when_running_chat() {
    let mut app = make_test_app();
    app.surface_dirty = crate::app::SurfaceDirtyState::default();
    app.terminal_lifecycle = TerminalLifecycleState::Running(SurfaceMode::Chat);
    seed_resize_measurements(&mut app);

    handle_terminal_event(&mut app, Event::Resize(120, 40));

    assert!(!app.surface_dirty.fullscreen.redraw);
    assert_eq!(
        app.surface_dirty.chat.rebuild,
        ChatRebuildKind::PurgeReplay(crate::app::ChatPurgeReplayOptions::resize())
    );
    assert!(app.surface_dirty.chat.repaint);
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
fn resize_marks_fullscreen_surface_dirty_when_running_fullscreen() {
    let mut app = make_test_app();
    app.surface_dirty = crate::app::SurfaceDirtyState::default();
    app.terminal_lifecycle =
        TerminalLifecycleState::Running(SurfaceMode::Fullscreen(FullscreenView::Config));
    seed_resize_measurements(&mut app);

    handle_terminal_event(&mut app, Event::Resize(120, 40));

    assert!(app.surface_dirty.fullscreen.redraw);
    assert_eq!(app.surface_dirty.chat.rebuild, ChatRebuildKind::None);
    assert!(!app.surface_dirty.chat.repaint);
    assert!(app.chat_render.resize_purge_replay_on_chat_return);
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
    let mut app = make_test_app();

    handle_client_event(&mut app, ClientEvent::TurnCancelled);
    assert!(app.turn.cancelled_pending_hint);

    handle_client_event(&mut app, ClientEvent::TurnComplete { terminal_reason: None });

    assert!(!app.turn.cancelled_pending_hint);
    let last = app.transcript.messages.last().expect("expected interruption hint message");
    assert!(matches!(last.role, MessageRole::System(Some(SystemSeverity::Info))));
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Conversation interrupted. Tell the model how to proceed.");
}

#[test]
fn connected_updates_welcome_session_id_while_pristine() {
    let mut app = make_test_app();
    app.transcript.messages.push(ChatMessage::welcome(
        env!("CARGO_PKG_VERSION"),
        "-",
        "/test",
        "-",
    ));
    let Some(MessageBlock::Welcome(welcome)) = app.transcript.messages[0].blocks.first_mut() else {
        panic!("expected welcome block");
    };
    welcome.tip_seed = 7;

    handle_client_event(&mut app, connected_event("claude-updated"));

    let Some(first) = app.transcript.messages.first() else {
        panic!("missing welcome message");
    };
    let Some(MessageBlock::Welcome(welcome)) = first.blocks.first() else {
        panic!("expected welcome block");
    };
    assert_eq!(welcome.session_id, "test-session");
    assert_eq!(welcome.tip_seed, 7);
}

#[test]
fn connected_session_preserves_inline_viewport_for_startup_welcome_transition() {
    let mut app = make_test_app();
    app.transcript.messages.push(ChatMessage::welcome(
        env!("CARGO_PKG_VERSION"),
        "-",
        "/test",
        "-",
    ));
    app.surface_dirty.chat.rebuild = ChatRebuildKind::None;
    app.surface_dirty.chat.repaint = false;

    handle_client_event(&mut app, connected_event("claude-updated"));

    assert_eq!(app.surface_dirty.chat.rebuild, ChatRebuildKind::None);
    assert!(app.surface_dirty.chat.repaint);
}

#[test]
fn connected_keeps_subscription_placeholder_until_status_snapshot_arrives() {
    let mut app = make_test_app();
    app.transcript.messages.push(ChatMessage::welcome(
        env!("CARGO_PKG_VERSION"),
        "old",
        "/test",
        "old",
    ));

    handle_client_event(&mut app, connected_event("opus"));

    let Some(first) = app.transcript.messages.first() else {
        panic!("missing welcome message");
    };
    let Some(MessageBlock::Welcome(welcome)) = first.blocks.first() else {
        panic!("expected welcome block");
    };
    assert_eq!(welcome.subscription, "-");
}

#[test]
fn connected_requests_mcp_snapshot_even_outside_mcp_tab() {
    let (mut app, mut rx) = app_with_bridge_connection();
    app.config.active_tab = crate::app::config::ConfigTab::Status;
    app.mcp.servers.push(crate::agent::model::McpServerStatus {
        name: "supabase".into(),
        status: crate::agent::model::McpServerConnectionStatus::Connected,
        server_info: None,
        error: None,
        config: None,
        scope: None,
        tools: Vec::new(),
    });
    app.mcp.removed_config_servers.insert(
        crate::app::state::types::RemovedMcpServerKey::new(
            "user".to_owned(),
            "supabase".to_owned(),
        ),
        crate::app::state::types::RemovedMcpServerGuard {
            expected_source: crate::agent::types::McpSnapshotSource::ReloadPlugins,
        },
    );

    handle_client_event(&mut app, connected_event("claude-updated"));

    let envelope = rx.try_recv().expect("mcp snapshot command");
    assert_eq!(
        envelope.command,
        crate::agent::wire::BridgeCommand::GetMcpSnapshot { session_id: "test-session".to_owned() }
    );
    assert!(app.mcp.in_flight);
    assert!(app.mcp.servers.is_empty());
    assert!(app.mcp.removed_config_servers.is_empty());
}

#[test]
fn connected_updates_cwd_and_clears_resuming_marker() {
    let mut app = make_test_app();
    app.transcript.messages.push(ChatMessage::welcome(
        env!("CARGO_PKG_VERSION"),
        "-",
        "/test",
        "-",
    ));
    app.resuming_session_id = Some("resume-123".into());

    handle_client_event(
        &mut app,
        ClientEvent::Connected {
            session_id: model::SessionId::new("session-cwd"),
            cwd: "/changed".into(),
            current_model: test_current_model("claude-updated"),
            available_models: Vec::new(),
            mode: None,
            history_updates: Vec::new(),
        },
    );

    assert_eq!(app.cwd_raw, "/changed");
    assert_eq!(app.cwd, "/changed");
    assert!(app.resuming_session_id.is_none());
    let Some(first) = app.transcript.messages.first() else {
        panic!("missing welcome message");
    };
    let Some(MessageBlock::Welcome(welcome)) = first.blocks.first() else {
        panic!("expected welcome block");
    };
    assert_eq!(welcome.cwd, "/changed");
}

#[test]
fn connected_reconciles_trust_for_new_cwd() {
    let mut app = make_test_app();
    app.trust.status = crate::app::trust::TrustStatus::Trusted;
    app.config.committed_preferences_document = serde_json::json!({
        "projects": {}
    });

    handle_client_event(
        &mut app,
        ClientEvent::Connected {
            session_id: model::SessionId::new("session-trust"),
            cwd: "/untrusted".into(),
            current_model: test_current_model("claude-updated"),
            available_models: Vec::new(),
            mode: None,
            history_updates: Vec::new(),
        },
    );

    assert_eq!(app.trust.status, crate::app::trust::TrustStatus::Untrusted);
    assert_eq!(
        app.trust.project_key,
        crate::app::trust::store::normalize_project_key(std::path::Path::new("/untrusted"))
    );
}

#[test]
fn connected_updates_welcome_once_even_after_chat_started() {
    let mut app = make_test_app();
    app.transcript.messages.push(ChatMessage::welcome(
        env!("CARGO_PKG_VERSION"),
        "-",
        "/test",
        "-",
    ));
    let Some(MessageBlock::Welcome(welcome)) = app.transcript.messages[0].blocks.first_mut() else {
        panic!("expected welcome block");
    };
    welcome.tip_seed = 11;
    app.transcript.messages.push(user_msg("hello"));

    handle_client_event(&mut app, connected_event("claude-updated"));

    let Some(first) = app.transcript.messages.first() else {
        panic!("missing first message");
    };
    let Some(MessageBlock::Welcome(welcome)) = first.blocks.first() else {
        panic!("expected welcome block");
    };
    assert_eq!(welcome.session_id, "test-session");
    assert_eq!(welcome.tip_seed, 11);
}

#[test]
fn current_model_update_does_not_mutate_welcome_snapshot_after_settings_reconcile() {
    let mut app = make_test_app();
    app.session_runtime.session_id = Some(model::SessionId::new("session-1"));
    app.session_runtime.current_model = Some(test_current_model("opus"));
    app.transcript.messages =
        vec![ChatMessage::welcome(env!("CARGO_PKG_VERSION"), "-", "/test", "session-1")];
    crate::app::config::store::set_model(&mut app.config.committed_settings_document, Some("opus"));

    crate::app::config::store::set_model(
        &mut app.config.committed_settings_document,
        Some("haiku"),
    );
    app.reconcile_runtime_from_persisted_settings_change();

    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::CurrentModelUpdate(
            model::CurrentModelUpdate::new(test_current_model("claude-opus-4-7")),
        )),
    );

    let Some(MessageBlock::Welcome(welcome)) = app.transcript.messages[0].blocks.first() else {
        panic!("expected welcome block");
    };
    assert_eq!(welcome.session_id, "session-1");
    assert_eq!(welcome.subscription, "-");
}

#[test]
fn connected_resets_session_scoped_view_data() {
    let mut app = make_test_app();
    app.transcript.messages.push(user_msg("hello"));
    app.status = AppStatus::Running;
    app.files_accessed = 9;
    app.usage.snapshot = Some(UsageSnapshot {
        source: UsageSourceKind::Oauth,
        fetched_at: std::time::SystemTime::now(),
        five_hour: None,
        seven_day: None,
        seven_day_opus: None,
        seven_day_sonnet: None,
        extra_usage: None,
    });
    app.session_runtime.account_info = Some(crate::agent::model::AccountInfo {
        email: Some("old@example.com".into()),
        organization: None,
        subscription_type: None,
        token_source: None,
        api_key_source: None,
        api_provider: None,
    });
    app.plugins.installed.push(installed_plugin_entry("old-plugin"));
    app.plugins.last_inventory_refresh_at = Some(Instant::now());
    app.config.pending_session_title_change =
        Some(crate::app::config::PendingSessionTitleChangeState {
            session_id: "old-session".into(),
            kind: crate::app::config::PendingSessionTitleChangeKind::Generate,
        });

    handle_client_event(&mut app, connected_event("claude-updated"));

    assert!(matches!(app.status, AppStatus::Ready));
    assert_eq!(app.transcript.messages.len(), 1);
    assert!(matches!(app.transcript.messages[0].role, MessageRole::Welcome));
    assert_eq!(app.files_accessed, 0);
    assert!(app.usage.snapshot.is_none());
    assert!(app.session_runtime.account_info.is_none());
    assert!(app.plugins.installed.is_empty());
    assert!(app.plugins.last_inventory_refresh_at.is_none());
    assert!(app.config.pending_session_title_change.is_none());
}

#[test]
fn current_model_update_leaves_existing_welcome_snapshot_unchanged() {
    let mut app = make_test_app();
    app.session_runtime.current_model = Some(test_current_model("opus"));
    app.transcript.messages.push(ChatMessage::welcome(
        env!("CARGO_PKG_VERSION"),
        "-",
        "/test",
        "-",
    ));
    app.transcript.messages.push(user_msg("hello"));

    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::CurrentModelUpdate(
            model::CurrentModelUpdate::new(test_current_model("claude-opus-4-7")),
        )),
    );

    let Some(first) = app.transcript.messages.first() else {
        panic!("missing first message");
    };
    let Some(MessageBlock::Welcome(welcome)) = first.blocks.first() else {
        panic!("expected welcome block");
    };
    assert_eq!(welcome.session_id, "-");

    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::CurrentModelUpdate(
            model::CurrentModelUpdate::new(test_current_model("claude-sonnet-4-5")),
        )),
    );

    let Some(first) = app.transcript.messages.first() else {
        panic!("missing first message");
    };
    let Some(MessageBlock::Welcome(welcome)) = first.blocks.first() else {
        panic!("expected welcome block");
    };
    assert_eq!(welcome.session_id, "-");
}

#[test]
fn auth_required_sets_hint_without_prefilling_login_command() {
    let mut app = make_test_app();
    app.input.set_text("keep me");

    handle_client_event(
        &mut app,
        ClientEvent::AuthRequired {
            method_name: "oauth".into(),
            method_description: "Open browser".into(),
        },
    );

    assert!(matches!(app.status, AppStatus::Ready));
    assert_eq!(app.input.text(), "keep me");
    let Some(hint) = &app.session_runtime.login_hint else {
        panic!("expected login hint");
    };
    assert_eq!(hint.method_name, "oauth");
    assert_eq!(hint.method_description, "Open browser");
}

#[test]
fn update_available_pushes_warning_system_message_with_versions_and_install_command() {
    let mut app = make_test_app();
    assert!(app.update_notice.is_none());

    handle_client_event(
        &mut app,
        ClientEvent::UpdateAvailable {
            latest_version: "0.3.0".into(),
            current_version: "0.2.0".into(),
        },
    );

    assert_eq!(app.transcript.messages.len(), 1);
    assert!(matches!(
        app.transcript.messages[0].role,
        MessageRole::System(Some(SystemSeverity::Warning))
    ));
    assert_eq!(
        first_block_text(&app.transcript.messages[0]),
        format!(
            "Update available: current v0.2.0, latest v0.3.0. Upgrade to latest version via {}.",
            session::update_install_command()
        )
    );
    let Some(update_notice) = app.update_notice.as_ref() else {
        panic!("expected update notice state");
    };
    assert_eq!(update_notice.current_version, "0.2.0");
    assert_eq!(update_notice.latest_version, "0.3.0");
    assert_eq!(
        update_notice.emitted_session_scope_epoch,
        Some(app.session_runtime.session_scope_epoch)
    );
}

#[test]
fn service_status_warning_pushes_system_warning_without_locking_input() {
    let mut app = make_test_app();

    handle_client_event(
        &mut app,
        ClientEvent::ServiceStatus {
            severity: ServiceStatusSeverity::Warning,
            message: "Claude Code status: Partial Outage (indicator: minor).".into(),
        },
    );

    assert!(matches!(app.status, AppStatus::Ready));
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected system message");
    };
    assert!(matches!(last.role, MessageRole::System(Some(SystemSeverity::Warning))));
}

#[test]
fn service_status_warning_survives_status_snapshot_welcome_update() {
    const CONNECTIVITY_MESSAGE: &str =
        "Claude Code could not connect to the internet. Please check your connection.";
    let mut app = make_test_app();

    handle_client_event(&mut app, connected_event("opus"));
    handle_client_event(
        &mut app,
        ClientEvent::ServiceStatus {
            severity: ServiceStatusSeverity::Warning,
            message: CONNECTIVITY_MESSAGE.into(),
        },
    );
    handle_client_event(
        &mut app,
        ClientEvent::StatusSnapshotReceived {
            session_id: "test-session".into(),
            account: crate::agent::model::AccountInfo {
                email: None,
                organization: None,
                subscription_type: Some("Claude Pro".into()),
                token_source: None,
                api_key_source: None,
                api_provider: None,
            },
        },
    );

    let warning_count = app
        .transcript
        .messages
        .iter()
        .filter(|message| {
            matches!(message.role, MessageRole::System(Some(SystemSeverity::Warning)))
        })
        .filter(|message| first_block_text(message) == CONNECTIVITY_MESSAGE)
        .count();
    assert_eq!(warning_count, 1);
}

#[test]
fn service_status_error_pushes_system_error_without_locking_input() {
    let mut app = make_test_app();
    app.input.set_text("draft stays");

    handle_client_event(
        &mut app,
        ClientEvent::ServiceStatus {
            severity: ServiceStatusSeverity::Error,
            message: "Claude Code status: Major Outage (indicator: major).".into(),
        },
    );

    assert!(matches!(app.status, AppStatus::Ready));
    assert_eq!(app.input.text(), "draft stays");
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected system message");
    };
    assert!(matches!(last.role, MessageRole::System(Some(SystemSeverity::Error))));
}

#[test]
fn session_replaced_resets_chat_and_transient_state() {
    let mut app = make_test_app();
    app.transcript.messages.push(ChatMessage::welcome(
        env!("CARGO_PKG_VERSION"),
        "-",
        "/test",
        "-",
    ));
    let Some(MessageBlock::Welcome(welcome)) = app.transcript.messages[0].blocks.first_mut() else {
        panic!("expected welcome block");
    };
    welcome.tip_seed = 5;
    app.transcript.messages.push(user_msg("hello"));
    app.transcript
        .messages
        .push(assistant_msg(vec![MessageBlock::Text(TextBlock::from_complete("world"))]));
    app.status = AppStatus::Running;
    app.files_accessed = 9;
    app.turn.pending_interaction_ids.push("perm-1".into());
    app.sdk_inventory.tasks.push(task_item("task-1", "Task", model::TaskStatus::InProgress));
    app.mention = Some(mention::MentionState::new(0, 0, String::new(), Vec::new()));
    app.mcp.servers.push(crate::agent::model::McpServerStatus {
        name: "supabase".into(),
        status: crate::agent::model::McpServerConnectionStatus::Connected,
        server_info: None,
        error: None,
        config: None,
        scope: None,
        tools: Vec::new(),
    });

    handle_client_event(
        &mut app,
        ClientEvent::SessionReplaced {
            session_id: model::SessionId::new("replacement"),
            cwd: "/replacement".into(),
            current_model: test_current_model("new-model"),
            available_models: Vec::new(),
            mode: None,
            history_updates: Vec::new(),
            restored_input: None,
        },
    );

    assert!(matches!(app.status, AppStatus::Ready));
    assert_eq!(
        app.session_runtime.session_id.as_ref().map(ToString::to_string).as_deref(),
        Some("replacement")
    );
    assert_eq!(
        app.session_runtime.current_model.as_ref().map(|model| model.resolved_id.as_str()),
        Some("new-model")
    );
    assert_eq!(app.transcript.messages.len(), 1);
    assert!(matches!(app.transcript.messages[0].role, MessageRole::Welcome));
    assert_eq!(app.files_accessed, 0);
    assert!(app.turn.pending_interaction_ids.is_empty());
    assert!(app.sdk_inventory.tasks.is_empty());
    assert!(app.mention.is_none());
    assert!(app.mcp.servers.is_empty());
    assert!(app.mcp.removed_config_servers.is_empty());
    assert_eq!(app.cwd_raw, "/replacement");
    assert_eq!(app.cwd, "/replacement");
    let Some(MessageBlock::Welcome(welcome)) = app.transcript.messages[0].blocks.first() else {
        panic!("expected welcome block");
    };
    assert_eq!(welcome.cwd, "/replacement");
    assert_ne!(welcome.tip_seed, 5);
    assert_eq!(
        app.surface_dirty.chat.rebuild,
        ChatRebuildKind::PurgeReplay(crate::app::ChatPurgeReplayOptions::session_replacement())
    );
}

#[test]
fn session_replaced_requests_mcp_snapshot_even_outside_mcp_tab() {
    let (mut app, mut rx) = app_with_bridge_connection();
    app.config.active_tab = crate::app::config::ConfigTab::Status;
    app.mcp.servers.push(crate::agent::model::McpServerStatus {
        name: "supabase".into(),
        status: crate::agent::model::McpServerConnectionStatus::Connected,
        server_info: None,
        error: None,
        config: None,
        scope: None,
        tools: Vec::new(),
    });

    handle_client_event(
        &mut app,
        ClientEvent::SessionReplaced {
            session_id: model::SessionId::new("replacement"),
            cwd: "/replacement".into(),
            current_model: test_current_model("new-model"),
            available_models: Vec::new(),
            mode: None,
            history_updates: Vec::new(),
            restored_input: None,
        },
    );

    let envelope = rx.try_recv().expect("mcp snapshot command");
    assert_eq!(
        envelope.command,
        crate::agent::wire::BridgeCommand::GetMcpSnapshot { session_id: "replacement".to_owned() }
    );
    assert!(app.mcp.in_flight);
    assert!(app.mcp.servers.is_empty());
}

#[test]
fn connected_requests_status_snapshot_on_connect() {
    let (mut app, mut rx) = app_with_bridge_connection();

    handle_client_event(&mut app, connected_event("claude-updated"));

    let mcp = rx.try_recv().expect("mcp snapshot command");
    assert_eq!(
        mcp.command,
        crate::agent::wire::BridgeCommand::GetMcpSnapshot { session_id: "test-session".to_owned() }
    );
    let status = rx.try_recv().expect("status snapshot command");
    assert_eq!(
        status.command,
        crate::agent::wire::BridgeCommand::GetStatusSnapshot {
            session_id: "test-session".to_owned(),
        }
    );
}

#[tokio::test(flavor = "current_thread")]
async fn connected_requests_usage_refresh_when_usage_tab_is_open() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut app = make_test_app();
            app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::Config);
            app.config.active_tab = crate::app::ConfigTab::Usage;

            handle_client_event(&mut app, connected_event("claude-updated"));

            assert!(app.usage.in_flight);
        })
        .await;
}

#[test]
fn stale_status_snapshot_for_old_session_is_ignored() {
    let mut app = make_test_app();
    app.session_runtime.session_id = Some(model::SessionId::new("current-session"));

    handle_client_event(
        &mut app,
        ClientEvent::StatusSnapshotReceived {
            session_id: "old-session".into(),
            account: crate::agent::model::AccountInfo {
                email: Some("old@example.com".into()),
                organization: None,
                subscription_type: None,
                token_source: None,
                api_key_source: None,
                api_provider: None,
            },
        },
    );

    assert!(app.session_runtime.account_info.is_none());
}

#[test]
fn status_snapshot_updates_welcome_subscription() {
    let mut app = make_test_app();
    app.transcript.messages.push(ChatMessage::welcome(
        env!("CARGO_PKG_VERSION"),
        "-",
        "/test",
        "session-1",
    ));
    app.session_runtime.session_id = Some(model::SessionId::new("session-1"));

    handle_client_event(
        &mut app,
        ClientEvent::StatusSnapshotReceived {
            session_id: "session-1".into(),
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

    let Some(MessageBlock::Welcome(welcome)) = app.transcript.messages[0].blocks.first() else {
        panic!("expected welcome block");
    };
    assert_eq!(welcome.subscription, "Claude Max");
    assert!(session_overview_has_welcome(&app));
}

#[test]
fn status_snapshot_does_not_commit_welcome_when_session_overview_is_suppressed() {
    let mut app = make_test_app();
    app.show_session_overview = false;
    app.transcript.messages.push(ChatMessage::welcome(
        env!("CARGO_PKG_VERSION"),
        "-",
        "/test",
        "session-1",
    ));
    app.session_runtime.session_id = Some(model::SessionId::new("session-1"));

    handle_client_event(
        &mut app,
        ClientEvent::StatusSnapshotReceived {
            session_id: "session-1".into(),
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

    let Some(MessageBlock::Welcome(welcome)) = app.transcript.messages[0].blocks.first() else {
        panic!("expected welcome block");
    };
    assert_eq!(welcome.subscription, "Claude Max");
    assert!(!session_overview_has_welcome(&app));
}

#[test]
fn stale_mcp_snapshot_for_old_session_is_ignored() {
    let mut app = make_test_app();
    app.session_runtime.session_id = Some(model::SessionId::new("current-session"));
    app.mcp.servers.push(crate::agent::model::McpServerStatus {
        name: "current".into(),
        status: crate::agent::model::McpServerConnectionStatus::Connected,
        server_info: None,
        error: None,
        config: None,
        scope: None,
        tools: Vec::new(),
    });

    handle_client_event(
        &mut app,
        ClientEvent::McpSnapshotReceived {
            session_id: "old-session".into(),
            servers: vec![crate::agent::model::McpServerStatus {
                name: "stale".into(),
                status: crate::agent::model::McpServerConnectionStatus::Connected,
                server_info: None,
                error: None,
                config: None,
                scope: None,
                tools: Vec::new(),
            }],
            source: Some(crate::agent::types::McpSnapshotSource::McpStatus),
            error: None,
        },
    );

    assert_eq!(app.mcp.servers.len(), 1);
    assert_eq!(app.mcp.servers[0].name, "current");
}

#[test]
fn removed_config_mcp_server_is_filtered_from_current_session_snapshot() {
    let mut app = make_test_app();
    app.session_runtime.session_id = Some(model::SessionId::new("current-session"));
    app.mcp.removed_config_servers.insert(
        crate::app::state::types::RemovedMcpServerKey::new("user".to_owned(), "notion".to_owned()),
        crate::app::state::types::RemovedMcpServerGuard {
            expected_source: crate::agent::types::McpSnapshotSource::ReloadPlugins,
        },
    );

    handle_client_event(
        &mut app,
        ClientEvent::McpSnapshotReceived {
            session_id: "current-session".into(),
            servers: vec![
                crate::agent::model::McpServerStatus {
                    name: "notion".into(),
                    status: crate::agent::model::McpServerConnectionStatus::Connected,
                    server_info: None,
                    error: None,
                    config: None,
                    scope: Some("user".into()),
                    tools: Vec::new(),
                },
                crate::agent::model::McpServerStatus {
                    name: "fff".into(),
                    status: crate::agent::model::McpServerConnectionStatus::Connected,
                    server_info: None,
                    error: None,
                    config: None,
                    scope: Some("user".into()),
                    tools: Vec::new(),
                },
            ],
            source: Some(crate::agent::types::McpSnapshotSource::McpStatus),
            error: None,
        },
    );

    assert_eq!(app.mcp.servers.len(), 1);
    assert_eq!(app.mcp.servers[0].name, "fff");
    assert_eq!(app.mcp.removed_config_servers.len(), 1);
}

#[test]
fn removed_config_mcp_guard_clears_after_matching_source_snapshot_proves_absence() {
    let mut app = make_test_app();
    app.session_runtime.session_id = Some(model::SessionId::new("current-session"));
    app.mcp.removed_config_servers.insert(
        crate::app::state::types::RemovedMcpServerKey::new("user".to_owned(), "notion".to_owned()),
        crate::app::state::types::RemovedMcpServerGuard {
            expected_source: crate::agent::types::McpSnapshotSource::ReloadPlugins,
        },
    );

    handle_client_event(
        &mut app,
        ClientEvent::McpSnapshotReceived {
            session_id: "current-session".into(),
            servers: vec![crate::agent::model::McpServerStatus {
                name: "fff".into(),
                status: crate::agent::model::McpServerConnectionStatus::Connected,
                server_info: None,
                error: None,
                config: None,
                scope: Some("user".into()),
                tools: Vec::new(),
            }],
            source: Some(crate::agent::types::McpSnapshotSource::ReloadPlugins),
            error: None,
        },
    );

    assert_eq!(app.mcp.servers.len(), 1);
    assert_eq!(app.mcp.servers[0].name, "fff");
    assert!(app.mcp.removed_config_servers.is_empty());
}

#[test]
fn removed_config_mcp_guard_stays_after_matching_source_snapshot_error() {
    let mut app = make_test_app();
    app.session_runtime.session_id = Some(model::SessionId::new("current-session"));
    app.mcp.removed_config_servers.insert(
        crate::app::state::types::RemovedMcpServerKey::new("user".to_owned(), "notion".to_owned()),
        crate::app::state::types::RemovedMcpServerGuard {
            expected_source: crate::agent::types::McpSnapshotSource::ReloadPlugins,
        },
    );

    handle_client_event(
        &mut app,
        ClientEvent::McpSnapshotReceived {
            session_id: "current-session".into(),
            servers: vec![crate::agent::model::McpServerStatus {
                name: "fff".into(),
                status: crate::agent::model::McpServerConnectionStatus::Connected,
                server_info: None,
                error: None,
                config: None,
                scope: Some("user".into()),
                tools: Vec::new(),
            }],
            source: Some(crate::agent::types::McpSnapshotSource::ReloadPlugins),
            error: Some("reload failed".to_owned()),
        },
    );

    assert_eq!(app.mcp.servers.len(), 1);
    assert_eq!(app.mcp.servers[0].name, "fff");
    assert_eq!(app.mcp.removed_config_servers.len(), 1);
}

#[test]
fn stale_usage_refresh_result_for_old_epoch_is_ignored() {
    let mut app = make_test_app();
    app.session_runtime.session_scope_epoch = 5;

    handle_client_event(
        &mut app,
        ClientEvent::UsageSnapshotReceived {
            epoch: 4,
            snapshot: UsageSnapshot {
                source: UsageSourceKind::Oauth,
                fetched_at: std::time::SystemTime::now(),
                five_hour: None,
                seven_day: None,
                seven_day_opus: None,
                seven_day_sonnet: None,
                extra_usage: None,
            },
        },
    );

    assert!(app.usage.snapshot.is_none());
}

#[test]
fn stale_plugin_inventory_result_for_old_cwd_is_ignored() {
    let mut app = make_test_app();
    app.cwd_raw = "/current".into();

    handle_client_event(
        &mut app,
        ClientEvent::PluginsInventoryUpdated {
            cwd_raw: "/old".into(),
            snapshot: crate::app::plugins::PluginsInventorySnapshot {
                installed: vec![installed_plugin_entry("stale-plugin")],
                marketplace: Vec::new(),
                marketplaces: Vec::new(),
            },
            claude_path: std::path::PathBuf::from("claude"),
        },
    );

    assert!(app.plugins.installed.is_empty());
}

#[test]
fn slash_command_error_while_resuming_returns_ready_and_clears_marker() {
    let mut app = make_test_app();
    app.status = AppStatus::CommandPending;
    app.resuming_session_id = Some("resume-123".into());

    handle_client_event(&mut app, ClientEvent::SlashCommandError("resume failed".into()));

    assert!(matches!(app.status, AppStatus::Ready));
    assert!(app.resuming_session_id.is_none());
}

#[test]
fn slash_command_error_clears_rewind_target_loading_state() {
    let mut app = make_test_app();
    app.sdk_inventory.rewind_targets_in_flight = true;
    app.sdk_inventory.rewind_targets_session_id = Some(model::SessionId::new("session-1"));
    app.sdk_inventory.rewind_targets = vec![model::RewindTarget {
        uuid: "user-1".into(),
        first_text: "hello".into(),
        input_text: "hello".into(),
        index: 0,
        previous_assistant_uuid: None,
    }];

    handle_client_event(
        &mut app,
        ClientEvent::SlashCommandError("failed to load rewind targets".into()),
    );

    assert!(!app.sdk_inventory.rewind_targets_in_flight);
    assert!(app.sdk_inventory.rewind_targets_session_id.is_none());
}

#[test]
fn slash_command_error_during_running_turn_does_not_stop_turn_status() {
    let mut app = make_test_app();
    app.status = AppStatus::Running;
    app.turn.pending_command_label = Some("Switching mode...".into());
    app.turn.pending_command_ack = Some(PendingCommandAck::CurrentMode);

    handle_client_event(&mut app, ClientEvent::SlashCommandError("failed to set mode".into()));

    assert!(matches!(app.status, AppStatus::Running));
    assert!(app.turn.pending_command_label.is_none());
    assert!(app.turn.pending_command_ack.is_none());
}

#[test]
fn slash_command_error_during_active_turn_inserts_inline_notice() {
    let mut app = make_test_app();
    app.status = AppStatus::Running;
    app.transcript.messages.push(assistant_msg(vec![MessageBlock::Text(
        TextBlock::from_complete("streaming answer"),
    )]));
    app.bind_active_turn_assistant(0);
    app.turn.pending_command_label = Some("Switching mode...".into());
    app.turn.pending_command_ack = Some(PendingCommandAck::CurrentMode);

    handle_client_event(
        &mut app,
        ClientEvent::SlashCommandError("failed to set mode to auto".into()),
    );

    assert!(matches!(app.status, AppStatus::Running));
    assert!(app.turn.pending_command_label.is_none());
    assert!(app.turn.pending_command_ack.is_none());
    assert_eq!(app.transcript.messages.len(), 1);
    assert!(app.turn.notice_refs.is_empty());
    let [MessageBlock::Text(text), MessageBlock::Notice(notice)] =
        app.transcript.messages[0].blocks.as_slice()
    else {
        panic!("expected assistant text followed by inline notice");
    };
    assert_eq!(text.text, "streaming answer");
    assert_eq!(notice.severity, SystemSeverity::Error);
    assert_eq!(notice.text.text, "failed to set mode to auto");
    assert!(notice.dedup_key.is_none());
}

#[test]
fn slash_command_error_without_active_turn_inserts_standalone_notice() {
    let mut app = make_test_app();
    app.status = AppStatus::CommandPending;
    app.turn.pending_command_label = Some("Switching mode...".into());
    app.turn.pending_command_ack = Some(PendingCommandAck::CurrentMode);

    handle_client_event(
        &mut app,
        ClientEvent::SlashCommandError("failed to set mode to auto".into()),
    );

    assert!(matches!(app.status, AppStatus::Ready));
    assert!(app.turn.pending_command_label.is_none());
    assert!(app.turn.pending_command_ack.is_none());
    let Some(ChatMessage {
        role: MessageRole::System(Some(SystemSeverity::Error)), blocks, ..
    }) = app.transcript.messages.last()
    else {
        panic!("expected standalone system notice");
    };
    let [MessageBlock::Notice(notice)] = blocks.as_slice() else {
        panic!("expected standalone notice block");
    };
    assert_eq!(notice.severity, SystemSeverity::Error);
    assert_eq!(notice.text.text, "failed to set mode to auto");
    assert!(notice.dedup_key.is_none());
}

#[test]
fn slash_command_error_during_thinking_turn_does_not_stop_turn_status() {
    let mut app = make_test_app();
    app.status = AppStatus::Thinking;
    app.turn.pending_command_label = Some("Switching model...".into());
    app.turn.pending_command_ack = Some(PendingCommandAck::CurrentModel);

    handle_client_event(&mut app, ClientEvent::SlashCommandError("failed to set model".into()));

    assert!(matches!(app.status, AppStatus::Thinking));
    assert!(app.turn.pending_command_label.is_none());
    assert!(app.turn.pending_command_ack.is_none());
}

#[test]
fn terminal_release_event_marks_child_process_lifecycle_without_redraw() {
    let mut app = make_test_app();
    app.request_chat_repaint();

    handle_client_event(
        &mut app,
        ClientEvent::TerminalReleasedToChild { reason: ReleaseReason::AuthFlow },
    );

    assert_eq!(
        app.terminal_lifecycle,
        TerminalLifecycleState::ReleasedToChild(ReleaseReason::AuthFlow)
    );
    assert!(!app.surface_dirty.chat.repaint);
}

#[test]
fn terminal_events_are_ignored_while_released_to_child_except_resize() {
    let mut app = make_test_app();
    app.terminal_lifecycle = TerminalLifecycleState::ReleasedToChild(ReleaseReason::AuthFlow);

    handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)),
    );

    assert_eq!(app.input.text(), "");
}

#[test]
fn terminal_return_event_restores_chat_lifecycle_and_rebuilds_chat() {
    let mut app = make_test_app();
    app.terminal_lifecycle = TerminalLifecycleState::ReleasedToChild(ReleaseReason::AuthFlow);
    app.chat_render.live_region.anchor_valid = true;
    app.surface_dirty.chat.repaint = false;

    handle_client_event(
        &mut app,
        ClientEvent::TerminalReturnedFromChild { reason: ReleaseReason::AuthFlow },
    );

    assert_eq!(app.terminal_lifecycle, TerminalLifecycleState::Running(SurfaceMode::Chat));
    assert!(app.surface_dirty.terminal_mode);
    assert!(!app.chat_render.live_region.anchor_valid);
    assert_eq!(app.surface_dirty.chat.rebuild, ChatRebuildKind::VisibleScreen);
    assert!(app.surface_dirty.chat.repaint);
}

#[test]
fn sessions_listed_completes_pending_session_rename() {
    let mut app = make_test_app();
    app.config.pending_session_title_change =
        Some(crate::app::config::PendingSessionTitleChangeState {
            session_id: "session-1".to_owned(),
            kind: crate::app::config::PendingSessionTitleChangeKind::Rename {
                requested_title: Some("Renamed session".to_owned()),
            },
        });

    handle_client_event(
        &mut app,
        ClientEvent::SessionsListed {
            sessions: vec![crate::agent::types::SessionListEntry {
                session_id: "session-1".to_owned(),
                summary: "Renamed session".to_owned(),
                last_modified_ms: 1,
                file_size_bytes: 2,
                cwd: Some("/test".to_owned()),
                git_branch: None,
                custom_title: Some("Renamed session".to_owned()),
                first_prompt: Some("prompt".to_owned()),
            }],
        },
    );

    assert!(app.config.pending_session_title_change.is_none());
    assert_eq!(app.config.status_message.as_deref(), Some("Renamed session to Renamed session"));
    assert!(app.config.last_error.is_none());
    assert_eq!(app.recent_sessions.len(), 1);
}

#[test]
fn slash_command_error_for_pending_session_rename_stays_in_config_feedback() {
    let mut app = make_test_app();
    app.config.pending_session_title_change =
        Some(crate::app::config::PendingSessionTitleChangeState {
            session_id: "session-1".to_owned(),
            kind: crate::app::config::PendingSessionTitleChangeKind::Rename {
                requested_title: Some("Renamed session".to_owned()),
            },
        });

    handle_client_event(
        &mut app,
        ClientEvent::SlashCommandError("failed to rename session: boom".into()),
    );

    assert!(app.config.pending_session_title_change.is_none());
    assert_eq!(app.config.last_error.as_deref(), Some("failed to rename session: boom"));
    assert!(app.config.status_message.is_none());
    assert!(app.transcript.messages.is_empty());
}

#[test]
fn mcp_operation_error_stays_in_mcp_feedback_and_out_of_chat() {
    let mut app = make_test_app();
    app.config.active_tab = crate::app::config::ConfigTab::Mcp;
    app.config.status_message = Some("Starting MCP auth for claude.ai Google Calendar...".into());
    app.mcp.in_flight = true;

    handle_client_event(
        &mut app,
        ClientEvent::McpOperationError {
            error: crate::agent::types::McpOperationError {
                server_name: Some("claude.ai Google Calendar".into()),
                operation: "authenticate".into(),
                message: "Server type \"claudeai-proxy\" does not support OAuth authentication"
                    .into(),
            },
        },
    );

    assert_eq!(
        app.mcp.last_error.as_deref(),
        Some(
            "Failed to authenticate MCP server claude.ai Google Calendar: Server type \"claudeai-proxy\" does not support OAuth authentication"
        )
    );
    assert_eq!(app.config.last_error, app.mcp.last_error);
    assert!(app.config.status_message.is_none());
    assert!(!app.mcp.in_flight);
    assert!(app.transcript.messages.is_empty());
}

#[test]
fn sessions_listed_completes_pending_session_title_generation() {
    let mut app = make_test_app();
    app.config.pending_session_title_change =
        Some(crate::app::config::PendingSessionTitleChangeState {
            session_id: "session-1".to_owned(),
            kind: crate::app::config::PendingSessionTitleChangeKind::Generate,
        });

    handle_client_event(
        &mut app,
        ClientEvent::SessionsListed {
            sessions: vec![crate::agent::types::SessionListEntry {
                session_id: "session-1".to_owned(),
                summary: "Generated session".to_owned(),
                last_modified_ms: 1,
                file_size_bytes: 2,
                cwd: Some("/test".to_owned()),
                git_branch: None,
                custom_title: Some("Generated session".to_owned()),
                first_prompt: Some("prompt".to_owned()),
            }],
        },
    );

    assert!(app.config.pending_session_title_change.is_none());
    assert_eq!(app.config.status_message.as_deref(), Some("Generated session title"));
    assert!(app.config.last_error.is_none());
}

#[test]
fn startup_picker_waits_for_connected_after_sessions_listed() {
    let mut app = make_test_app();
    app.startup = crate::app::state::StartupState::new(None, None, true);
    app.startup.request_connection();
    assert!(app.startup.mark_connection_started());

    handle_client_event(
        &mut app,
        ClientEvent::SessionsListed {
            sessions: vec![listed_session("session-1", "First Session")],
        },
    );

    assert_eq!(app.surface_mode, SurfaceMode::Chat);
    assert!(app.startup.recent_sessions_loaded());
    assert!(!app.startup.session_picker_resolved());

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    app.session_runtime.conn = Some(Rc::new(crate::agent::client::AgentConnection::new(tx)));
    handle_client_event(&mut app, connected_event("claude-updated"));

    assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::SessionPicker));
    assert!(app.startup.session_picker_resolved());
}

#[test]
fn startup_picker_empty_list_stays_in_chat_with_info_message() {
    let mut app = make_test_app();
    app.startup = crate::app::state::StartupState::new(None, None, true);
    app.startup.request_connection();
    assert!(app.startup.mark_connection_started());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    app.session_runtime.conn = Some(Rc::new(crate::agent::client::AgentConnection::new(tx)));

    handle_client_event(&mut app, connected_event("claude-updated"));
    assert_eq!(app.surface_mode, SurfaceMode::Chat);
    assert!(!app.startup.session_picker_resolved());

    handle_client_event(&mut app, ClientEvent::SessionsListed { sessions: Vec::new() });

    assert_eq!(app.surface_mode, SurfaceMode::Chat);
    assert!(app.startup.session_picker_resolved());
    let last = app.transcript.messages.last().expect("info message");
    let text = match last.blocks.first().expect("text block") {
        MessageBlock::Text(block) => block.text.as_str(),
        _ => panic!("expected text block"),
    };
    assert!(text.contains("No recent sessions found for this directory"));
}

#[test]
fn sessions_listed_refresh_preserves_picker_selection_by_session_id() {
    let mut app = make_test_app();
    app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::SessionPicker);
    app.recent_sessions = vec![
        crate::app::RecentSessionInfo {
            session_id: "session-1".to_owned(),
            summary: "First".to_owned(),
            last_modified_ms: 1,
            file_size_bytes: 1,
            cwd: Some("/test".to_owned()),
            git_branch: Some("main".to_owned()),
            custom_title: Some("First".to_owned()),
            first_prompt: Some("prompt one".to_owned()),
        },
        crate::app::RecentSessionInfo {
            session_id: "session-2".to_owned(),
            summary: "Second".to_owned(),
            last_modified_ms: 2,
            file_size_bytes: 1,
            cwd: Some("/test".to_owned()),
            git_branch: Some("main".to_owned()),
            custom_title: Some("Second".to_owned()),
            first_prompt: Some("prompt two".to_owned()),
        },
    ];
    app.session_picker.selected = 1;
    app.session_picker.scroll_offset = 1;

    handle_client_event(
        &mut app,
        ClientEvent::SessionsListed {
            sessions: vec![
                listed_session("session-2", "Second"),
                listed_session("session-3", "Third"),
            ],
        },
    );

    assert_eq!(app.session_picker.selected, 0);
    assert_eq!(app.recent_sessions[app.session_picker.selected].session_id, "session-2");
    assert_eq!(app.session_picker.scroll_offset, 0);
}

#[test]
fn current_model_update_updates_state_and_clears_pending_when_expected() {
    let mut app = make_test_app();
    app.status = AppStatus::CommandPending;
    app.turn.pending_command_label = Some("Switching model...".into());
    app.turn.pending_command_ack = Some(PendingCommandAck::CurrentModel);
    app.session_runtime.current_model = Some(test_current_model("old-model"));

    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::CurrentModelUpdate(
            model::CurrentModelUpdate::new(test_current_model("sonnet")),
        )),
    );

    assert!(matches!(app.status, AppStatus::Ready));
    assert_eq!(
        app.session_runtime.current_model.as_ref().map(|model| model.resolved_id.as_str()),
        Some("sonnet")
    );
    assert!(app.turn.pending_command_label.is_none());
    assert!(app.turn.pending_command_ack.is_none());
}

#[test]
fn non_matching_config_option_update_keeps_pending() {
    let mut app = make_test_app();
    app.status = AppStatus::CommandPending;
    app.turn.pending_command_label = Some("Switching model...".into());
    app.turn.pending_command_ack =
        Some(PendingCommandAck::ConfigOption { option_id: "model".to_owned() });

    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::ConfigOptionUpdate(
            model::ConfigOptionUpdate {
                option_id: "max_thinking_tokens".to_owned(),
                value: serde_json::json!(2048),
            },
        )),
    );

    assert!(matches!(app.status, AppStatus::CommandPending));
    assert_eq!(
        app.session_runtime.config_options.get("max_thinking_tokens"),
        Some(&serde_json::json!(2048))
    );
    assert_eq!(app.turn.pending_command_label.as_deref(), Some("Switching model..."));
    assert!(matches!(
        app.turn.pending_command_ack.as_ref(),
        Some(PendingCommandAck::ConfigOption { option_id }) if option_id == "model"
    ));
}

#[test]
fn resume_does_not_add_confirmation_system_message() {
    let mut app = make_test_app();
    app.resuming_session_id = Some("requested-123".into());

    handle_client_event(
        &mut app,
        ClientEvent::SessionReplaced {
            session_id: model::SessionId::new("active-456"),
            cwd: "/replacement".into(),
            current_model: test_current_model("new-model"),
            available_models: Vec::new(),
            mode: None,
            history_updates: Vec::new(),
            restored_input: None,
        },
    );

    assert_eq!(app.transcript.messages.len(), 1);
    assert!(matches!(app.transcript.messages[0].role, MessageRole::Welcome));
    assert!(app.resuming_session_id.is_none());
    assert!(matches!(app.status, AppStatus::Ready));
}

#[test]
fn resume_history_renders_user_message_chunks() {
    let mut app = make_test_app();
    let history_updates = vec![
        model::SessionUpdate::UserMessageChunk(model::ContentChunk::new(
            model::ContentBlock::Text(model::TextContent::new("first user line")),
        )),
        model::SessionUpdate::AgentMessageChunk(model::ContentChunk::new(
            model::ContentBlock::Text(model::TextContent::new("assistant reply")),
        )),
    ];

    handle_client_event(
        &mut app,
        ClientEvent::SessionReplaced {
            session_id: model::SessionId::new("active-456"),
            cwd: "/replacement".into(),
            current_model: test_current_model("new-model"),
            available_models: Vec::new(),
            mode: None,
            history_updates,
            restored_input: None,
        },
    );

    assert_eq!(app.transcript.messages.len(), 3);
    assert!(matches!(app.transcript.messages[0].role, MessageRole::Welcome));
    assert!(matches!(app.transcript.messages[1].role, MessageRole::User));
    assert!(matches!(app.transcript.messages[2].role, MessageRole::Assistant));

    let Some(MessageBlock::Text(user_text)) = app.transcript.messages[1].blocks.first() else {
        panic!("expected user text block");
    };
    assert_eq!(user_text.text, "first user line");
    assert!(canonical_messages_contain_text(&app, "first user line"));
    assert!(canonical_messages_contain_text(&app, "assistant reply"));
    assert!(!session_overview_has_welcome(&app));
    assert!(matches!(app.status, AppStatus::Ready));
    assert_eq!(app.turn.pending_cancel_origin, None);
    assert!(!app.turn.pending_auto_submit_after_cancel);

    handle_client_event(
        &mut app,
        ClientEvent::StatusSnapshotReceived {
            session_id: "active-456".into(),
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
fn session_replaced_restores_input_after_loading_history() {
    let mut app = make_test_app();
    app.turn.pending_command_label = Some("Rewinding conversation...".to_owned());
    app.status = AppStatus::CommandPending;
    let history_updates = vec![model::SessionUpdate::AgentMessageChunk(model::ContentChunk::new(
        model::ContentBlock::Text(model::TextContent::new("assistant reply")),
    ))];

    handle_client_event(
        &mut app,
        ClientEvent::SessionReplaced {
            session_id: model::SessionId::new("rewound"),
            cwd: "/replacement".into(),
            current_model: test_current_model("new-model"),
            available_models: Vec::new(),
            mode: None,
            history_updates,
            restored_input: Some("selected prompt".to_owned()),
        },
    );

    assert_eq!(app.input.text(), "selected prompt");
    assert!(app.turn.pending_command_label.is_none());
    assert!(matches!(app.status, AppStatus::Ready));
    assert!(canonical_messages_contain_text(&app, "assistant reply"));
}

#[test]
fn startup_resume_history_renders_from_canonical_messages() {
    let mut app = make_test_app();
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
            history_updates,
        },
    );

    assert!(canonical_messages_contain_text(&app, "startup user line"));
    assert!(canonical_messages_contain_text(&app, "startup assistant reply"));
    assert!(live_rows_contain_text(&mut app, "startup user line"));
    assert!(live_rows_contain_text(&mut app, "startup assistant reply"));
    assert!(!session_overview_has_welcome(&app));
    assert!(matches!(app.status, AppStatus::Ready));
    assert_eq!(app.turn.pending_cancel_origin, None);
    assert!(!app.turn.pending_auto_submit_after_cancel);

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
    assert_eq!(app.turn.pending_cancel_origin, None);
    assert!(!app.turn.pending_auto_submit_after_cancel);
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
fn resume_history_clears_tool_scope_tracking_after_loading() {
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
            history_updates: vec![model::SessionUpdate::ToolCall(task_tool)],
            restored_input: None,
        },
    );

    assert!(app.turn.active_task_ids.is_empty());
    assert_eq!(app.tool_call_scope("resume-task"), None);
}

#[test]
fn turn_complete_without_cancel_does_not_render_interrupted_hint() {
    let mut app = make_test_app();
    handle_client_event(&mut app, ClientEvent::TurnComplete { terminal_reason: None });
    assert!(app.transcript.messages.is_empty());
}

#[test]
fn turn_complete_keeps_history_and_adds_compaction_success_after_manual_boundary() {
    let mut app = make_test_app();
    app.session_runtime.session_id = Some(model::SessionId::new("session-x"));
    app.transcript.messages.push(user_msg("/compact"));
    app.transcript
        .messages
        .push(assistant_msg(vec![MessageBlock::Text(TextBlock::from_complete("compacted"))]));
    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::CompactionBoundary(
            model::CompactionBoundary {
                trigger: model::CompactionTrigger::Manual,
                pre_tokens: 123_456,
            },
        )),
    );
    assert!(app.turn.pending_compact_clear);

    handle_client_event(&mut app, ClientEvent::TurnComplete { terminal_reason: None });

    assert!(!app.turn.pending_compact_clear);
    assert_eq!(app.transcript.messages.len(), 3);
    let Some(ChatMessage { role: MessageRole::System(Some(SystemSeverity::Info)), blocks, .. }) =
        app.transcript.messages.last()
    else {
        panic!("expected compaction success system message");
    };
    let Some(MessageBlock::Text(block)) = blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Session successfully compacted.");
    assert_eq!(
        app.session_runtime.session_id.as_ref().map(ToString::to_string).as_deref(),
        Some("session-x")
    );
}

#[test]
fn first_agent_chunk_clears_unconfirmed_compacting_without_success_message() {
    let mut app = make_test_app();
    app.turn.is_compacting = true;

    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::AgentMessageChunk(
            model::ContentChunk::new(model::ContentBlock::Text(model::TextContent::new(
                "regular answer",
            ))),
        )),
    );

    assert!(!app.turn.is_compacting);
    assert!(!app.turn.pending_compact_clear);
    assert!(app.transcript.messages.iter().all(|message| {
        !matches!(
            message,
            ChatMessage { role: MessageRole::System(Some(SystemSeverity::Info)), .. }
        )
    }));
}

#[test]
fn session_status_idle_does_not_emit_compaction_success_without_boundary() {
    let mut app = make_test_app();
    app.turn.is_compacting = true;

    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::SessionStatusUpdate(
            model::SessionStatus::Idle,
        )),
    );

    assert!(!app.turn.is_compacting);
    assert!(!app.turn.pending_compact_clear);
    assert!(app.transcript.messages.is_empty());
}

#[test]
fn turn_error_keeps_history_when_compact_pending() {
    let mut app = make_test_app();
    app.turn.pending_compact_clear = true;
    app.transcript.messages.push(user_msg("/compact"));

    handle_client_event(
        &mut app,
        ClientEvent::TurnError {
            message: "adapter failed".into(),
            api_error_status: None,
            terminal_reason: None,
        },
    );

    assert!(!app.turn.pending_compact_clear);
    assert!(matches!(app.status, AppStatus::Error));
    assert_eq!(app.transcript.messages.len(), 3);
    assert!(matches!(app.transcript.messages[0].role, MessageRole::User));
    let Some(ChatMessage { role: MessageRole::System(Some(SystemSeverity::Info)), blocks, .. }) =
        app.transcript.messages.get(1)
    else {
        panic!("expected compaction success system message");
    };
    let Some(MessageBlock::Text(block)) = blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Session successfully compacted.");
    let Some(ChatMessage { role: MessageRole::System(_), blocks, .. }) =
        app.transcript.messages.last()
    else {
        panic!("expected system error message");
    };
    let Some(MessageBlock::Text(block)) = blocks.first() else {
        panic!("expected text block");
    };
    assert!(block.text.contains("Turn failed: adapter failed"));
    assert!(block.text.contains("Press Ctrl+Q to quit and try again"));
}

#[test]
fn turn_cancel_keeps_manual_compaction_success_pending_until_exit() {
    let mut app = make_test_app();
    app.turn.pending_compact_clear = true;
    app.turn.is_compacting = true;

    handle_client_event(&mut app, ClientEvent::TurnCancelled);

    assert!(app.turn.pending_compact_clear);
    assert!(app.turn.is_compacting);
}

#[test]
fn turn_error_after_cancel_keeps_compaction_success_before_interrupted_hint() {
    let mut app = make_test_app();
    app.transcript.messages.push(user_msg("/compact"));
    app.turn.pending_compact_clear = true;
    app.turn.is_compacting = true;

    handle_client_event(&mut app, ClientEvent::TurnCancelled);
    handle_client_event(
        &mut app,
        ClientEvent::TurnError {
            message: "cancelled".into(),
            api_error_status: None,
            terminal_reason: None,
        },
    );

    assert_eq!(app.transcript.messages.len(), 3);
    assert!(matches!(
        app.transcript.messages[1].role,
        MessageRole::System(Some(SystemSeverity::Info))
    ));
    let Some(MessageBlock::Text(block)) = app.transcript.messages[1].blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Session successfully compacted.");
    let Some(MessageBlock::Text(block)) = app.transcript.messages[2].blocks.first() else {
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
            message: "HTTP 429 Too Many Requests: max turns exceeded".into(),
            api_error_status: None,
            terminal_reason: None,
        },
    );

    assert!(matches!(app.status, AppStatus::Error));
    let Some(ChatMessage { role: MessageRole::System(_), blocks, .. }) =
        app.transcript.messages.last()
    else {
        panic!("expected system error message");
    };
    assert!(matches!(blocks.first(), Some(MessageBlock::Notice(_))));
    let text = first_block_text(app.transcript.messages.last().expect("expected message"));
    assert!(text.contains("Turn blocked by account or plan limits"));
    assert!(text.contains("Next steps:"));
    assert!(text.contains("Check quota/billing"));
}

#[test]
fn classified_turn_error_plan_limit_uses_guidance_without_text_matching() {
    let mut app = make_test_app();

    handle_client_event(
        &mut app,
        ClientEvent::TurnErrorClassified {
            message: "turn failed".into(),
            class: TurnErrorClass::PlanLimit,
            api_error_status: None,
            terminal_reason: None,
        },
    );

    assert!(matches!(app.status, AppStatus::Error));
    let Some(ChatMessage { role: MessageRole::System(_), blocks, .. }) =
        app.transcript.messages.last()
    else {
        panic!("expected system error message");
    };
    assert!(matches!(blocks.first(), Some(MessageBlock::Notice(_))));
    let text = first_block_text(app.transcript.messages.last().expect("expected message"));
    assert!(text.contains("Turn blocked by account or plan limits"));
    assert!(text.contains("Next steps:"));
}

#[test]
fn classified_turn_error_auth_required_sets_exit_error_and_quits() {
    let mut app = make_test_app();

    handle_client_event(
        &mut app,
        ClientEvent::TurnErrorClassified {
            message: "auth required".into(),
            class: TurnErrorClass::AuthRequired,
            api_error_status: None,
            terminal_reason: None,
        },
    );

    assert!(matches!(app.status, AppStatus::Error));
    assert!(app.should_quit);
    assert_eq!(app.exit_error, Some(crate::error::AppError::AuthRequired));
}

#[test]
fn classified_turn_error_model_unavailable_suggests_model_switch() {
    let mut app = make_test_app();

    handle_client_event(
        &mut app,
        ClientEvent::TurnErrorClassified {
            message: "model_not_found".into(),
            class: TurnErrorClass::ModelUnavailable,
            api_error_status: Some(404),
            terminal_reason: None,
        },
    );

    assert!(matches!(app.status, AppStatus::Error));
    assert!(!app.should_quit);
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
            message: "oauth_org_not_allowed".into(),
            class: TurnErrorClass::AccountAccess,
            api_error_status: Some(403),
            terminal_reason: None,
        },
    );

    assert!(matches!(app.status, AppStatus::Error));
    assert!(!app.should_quit);
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
            message: "overloaded".into(),
            class: TurnErrorClass::TransientService,
            api_error_status: Some(529),
            terminal_reason: None,
        },
    );

    assert!(matches!(app.status, AppStatus::Error));
    assert!(!app.should_quit);
    let text = first_block_text(app.transcript.messages.last().expect("expected message"));
    assert!(text.contains("temporarily overloaded or unavailable"));
    assert!(text.contains("retry"));
}

#[test]
fn turn_error_clears_tool_scope_tracking() {
    let mut app = make_test_app();
    app.transcript.messages.push(assistant_msg(vec![MessageBlock::ToolCall(Box::new(tool_call(
        "task-1",
        model::ToolCallStatus::InProgress,
    )))]));
    app.register_tool_call_scope("task-1".into(), ToolCallScope::SubagentRoot);
    app.insert_active_task("task-1".into());

    handle_client_event(
        &mut app,
        ClientEvent::TurnError {
            message: "boom".into(),
            api_error_status: None,
            terminal_reason: None,
        },
    );

    assert!(app.turn.active_task_ids.is_empty());
    assert_eq!(app.tool_call_scope("task-1"), None);
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
    app.register_tool_call_scope("task-1".into(), ToolCallScope::SubagentRoot);
    app.insert_active_task("task-1".into());
    app.turn.pending_interaction_ids.push("task-1".into());
    app.claim_focus_target(FocusTarget::Permission);

    handle_client_event(
        &mut app,
        ClientEvent::AuthRequired {
            method_name: "oauth".into(),
            method_description: "Open browser".into(),
        },
    );

    assert_eq!(app.active_turn_assistant_idx(), None);
    assert!(app.turn.active_task_ids.is_empty());
    assert!(app.turn.pending_interaction_ids.is_empty());
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
    assert!(app.should_quit);
    assert_eq!(app.exit_error, Some(crate::error::AppError::ConnectionFailed));
}

#[test]
fn connection_failed_clears_active_turn_runtime_tracking() {
    let mut app = make_test_app();
    app.status = AppStatus::Running;
    app.transcript.messages.push(assistant_msg(vec![MessageBlock::ToolCall(Box::new(tool_call(
        "task-1",
        model::ToolCallStatus::InProgress,
    )))]));
    app.bind_active_turn_assistant(0);
    app.register_tool_call_scope("task-1".into(), ToolCallScope::SubagentRoot);
    app.insert_active_task("task-1".into());

    handle_client_event(&mut app, ClientEvent::ConnectionFailed("bridge down".into()));

    assert_eq!(app.active_turn_assistant_idx(), None);
    assert!(app.turn.active_task_ids.is_empty());
    let Some(MessageBlock::ToolCall(tc)) = app.transcript.messages[0].blocks.first() else {
        panic!("expected tool call block");
    };
    assert_eq!(tc.status, model::ToolCallStatus::Failed);
}

#[test]
fn fatal_event_clears_active_turn_runtime_tracking() {
    let mut app = make_test_app();
    app.status = AppStatus::Running;
    app.transcript.messages.push(assistant_msg(vec![MessageBlock::ToolCall(Box::new(tool_call(
        "task-1",
        model::ToolCallStatus::InProgress,
    )))]));
    app.bind_active_turn_assistant(0);
    app.register_tool_call_scope("task-1".into(), ToolCallScope::SubagentRoot);
    app.insert_active_task("task-1".into());

    handle_client_event(
        &mut app,
        ClientEvent::FatalError(crate::error::AppError::ConnectionFailed),
    );

    assert_eq!(app.active_turn_assistant_idx(), None);
    assert!(app.turn.active_task_ids.is_empty());
    let Some(MessageBlock::ToolCall(tc)) = app.transcript.messages[0].blocks.first() else {
        panic!("expected tool call block");
    };
    assert_eq!(tc.status, model::ToolCallStatus::Failed);
}

#[test]
fn compaction_boundary_enables_compacting_and_records_boundary() {
    let mut app = make_test_app();
    assert!(!app.turn.is_compacting);

    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::CompactionBoundary(
            model::CompactionBoundary {
                trigger: model::CompactionTrigger::Manual,
                pre_tokens: 123_456,
            },
        )),
    );

    assert!(app.turn.is_compacting);
    assert!(app.turn.pending_compact_clear);
    assert_eq!(
        app.session_runtime.session_usage.last_compaction_trigger,
        Some(model::CompactionTrigger::Manual)
    );
    assert_eq!(app.session_runtime.session_usage.last_compaction_pre_tokens, Some(123_456));
}

#[test]
fn auto_compaction_boundary_sets_compacting_without_manual_success_pending() {
    let mut app = make_test_app();
    assert!(!app.turn.is_compacting);

    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::CompactionBoundary(
            model::CompactionBoundary {
                trigger: model::CompactionTrigger::Auto,
                pre_tokens: 234_567,
            },
        )),
    );

    assert!(app.turn.is_compacting);
    assert!(!app.turn.pending_compact_clear);
    assert_eq!(
        app.session_runtime.session_usage.last_compaction_trigger,
        Some(model::CompactionTrigger::Auto)
    );
    assert_eq!(app.session_runtime.session_usage.last_compaction_pre_tokens, Some(234_567));
}

#[test]
fn fast_mode_update_sets_state() {
    let mut app = make_test_app();
    assert_eq!(app.session_runtime.fast_mode_state, model::FastModeState::Off);

    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::FastModeUpdate(
            model::FastModeState::Cooldown,
        )),
    );

    assert_eq!(app.session_runtime.fast_mode_state, model::FastModeState::Cooldown);
}

#[test]
fn rate_limit_notices_dedup_and_upgrade_in_place() {
    let mut app = make_test_app();

    let warning_update = model::RateLimitUpdate {
        status: model::RateLimitStatus::AllowedWarning,
        error_code: None,
        resets_at: Some(123.0),
        utilization: Some(0.92),
        rate_limit_type: Some("five_hour".to_owned()),
        overage_status: None,
        overage_resets_at: None,
        overage_disabled_reason: None,
        is_using_overage: None,
        surpassed_threshold: None,
        can_user_purchase_credits: None,
        has_chargeable_saved_payment_method: None,
    };

    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::RateLimitUpdate(warning_update.clone())),
    );
    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::RateLimitUpdate(warning_update.clone())),
    );

    assert_eq!(app.transcript.messages.len(), 1);
    assert!(matches!(
        app.transcript.messages[0].role,
        MessageRole::System(Some(SystemSeverity::Warning))
    ));
    assert!(matches!(app.transcript.messages[0].blocks.first(), Some(MessageBlock::Notice(_))));

    let rejected_update =
        model::RateLimitUpdate { status: model::RateLimitStatus::Rejected, ..warning_update };
    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::RateLimitUpdate(rejected_update.clone())),
    );
    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::RateLimitUpdate(rejected_update)),
    );

    assert_eq!(app.transcript.messages.len(), 1);
    assert!(matches!(
        app.transcript.messages[0].role,
        MessageRole::System(Some(SystemSeverity::Error))
    ));
    assert!(first_block_text(&app.transcript.messages[0]).contains("Rate limit reached"));
}

#[test]
fn plan_limit_turn_error_upgrades_inline_notice_in_active_assistant() {
    let mut app = make_test_app();
    app.status = AppStatus::Thinking;
    app.transcript.messages.push(user_msg("hello"));
    app.transcript.messages.push(assistant_msg(vec![MessageBlock::Text(
        TextBlock::from_complete("partial response"),
    )]));
    app.bind_active_turn_assistant(1);

    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::RateLimitUpdate(model::RateLimitUpdate {
            status: model::RateLimitStatus::AllowedWarning,
            error_code: None,
            resets_at: Some(1_741_280_000.0),
            utilization: Some(0.95),
            rate_limit_type: Some("five_hour".to_owned()),
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
            message: "HTTP 429 Too Many Requests".to_owned(),
            class: TurnErrorClass::PlanLimit,
            api_error_status: None,
            terminal_reason: None,
        },
    );

    assert!(matches!(app.status, AppStatus::Error));
    assert_eq!(app.transcript.messages.len(), 2);
    assert_eq!(app.transcript.messages[1].blocks.len(), 2);
    let Some(MessageBlock::Notice(block)) = app.transcript.messages[1].blocks.get(1) else {
        panic!("expected inline notice block");
    };
    assert_eq!(block.severity, SystemSeverity::Warning);
    assert!(block.text.text.contains("Approaching rate limit"));
    assert!(block.text.text.contains("Turn blocked by account or plan limits"));
    assert!(app.turn.notice_refs.is_empty());
}

#[test]
fn different_rate_limit_incident_in_later_turn_keeps_older_notice() {
    let mut app = make_test_app();
    app.session_runtime.last_rate_limit_update = Some(model::RateLimitUpdate {
        status: model::RateLimitStatus::AllowedWarning,
        error_code: None,
        resets_at: Some(1_741_280_000.0),
        utilization: Some(0.95),
        rate_limit_type: Some("five_hour".to_owned()),
        overage_status: None,
        overage_resets_at: None,
        overage_disabled_reason: None,
        is_using_overage: None,
        surpassed_threshold: None,
        can_user_purchase_credits: None,
        has_chargeable_saved_payment_method: None,
    });
    app.status = AppStatus::Thinking;
    app.transcript.messages.push(user_msg("first"));
    app.transcript.messages.push(assistant_msg(vec![]));
    app.bind_active_turn_assistant(1);

    handle_client_event(
        &mut app,
        ClientEvent::TurnErrorClassified {
            message: "HTTP 429 Too Many Requests".to_owned(),
            class: TurnErrorClass::PlanLimit,
            api_error_status: None,
            terminal_reason: None,
        },
    );
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
        ClientEvent::SessionUpdate(model::SessionUpdate::RateLimitUpdate(model::RateLimitUpdate {
            status: model::RateLimitStatus::Rejected,
            error_code: None,
            resets_at: Some(1_741_290_000.0),
            utilization: None,
            rate_limit_type: Some("daily".to_owned()),
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
fn turn_notice_tracking_clears_on_turn_complete_and_session_reset() {
    let mut app = make_test_app();
    app.status = AppStatus::Thinking;
    app.transcript.messages.push(user_msg("hello"));
    app.transcript.messages.push(assistant_msg(vec![]));
    app.bind_active_turn_assistant(1);

    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::RateLimitUpdate(model::RateLimitUpdate {
            status: model::RateLimitStatus::AllowedWarning,
            error_code: None,
            resets_at: Some(123.0),
            utilization: Some(0.91),
            rate_limit_type: Some("five_hour".to_owned()),
            overage_status: None,
            overage_resets_at: None,
            overage_disabled_reason: None,
            is_using_overage: None,
            surpassed_threshold: None,
            can_user_purchase_credits: None,
            has_chargeable_saved_payment_method: None,
        })),
    );

    assert_eq!(app.turn.notice_refs.len(), 1);
    handle_client_event(&mut app, ClientEvent::TurnComplete { terminal_reason: None });
    assert!(app.turn.notice_refs.is_empty());

    app.status = AppStatus::Thinking;
    app.transcript.messages.push(user_msg("again"));
    app.transcript.messages.push(assistant_msg(vec![]));
    app.bind_active_turn_assistant(app.transcript.messages.len() - 1);
    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::RateLimitUpdate(model::RateLimitUpdate {
            status: model::RateLimitStatus::AllowedWarning,
            error_code: None,
            resets_at: Some(456.0),
            utilization: Some(0.92),
            rate_limit_type: Some("daily".to_owned()),
            overage_status: None,
            overage_resets_at: None,
            overage_disabled_reason: None,
            is_using_overage: None,
            surpassed_threshold: None,
            can_user_purchase_credits: None,
            has_chargeable_saved_payment_method: None,
        })),
    );
    assert_eq!(app.turn.notice_refs.len(), 1);

    handle_client_event(
        &mut app,
        ClientEvent::Connected {
            session_id: model::SessionId::new("new-session"),
            cwd: "/test".into(),
            current_model: test_current_model("claude"),
            available_models: Vec::new(),
            mode: None,
            history_updates: Vec::new(),
        },
    );
    assert!(app.turn.notice_refs.is_empty());
}

#[test]
fn turn_error_after_cancel_shows_interrupted_hint_instead_of_error_block() {
    let mut app = make_test_app();
    app.transcript.messages.push(user_msg("build app"));

    handle_client_event(&mut app, ClientEvent::TurnCancelled);
    assert!(app.turn.cancelled_pending_hint);

    handle_client_event(
        &mut app,
        ClientEvent::TurnError {
            message: "Error: Request was aborted.\n    at stack line".into(),
            api_error_status: None,
            terminal_reason: None,
        },
    );

    assert!(!app.turn.cancelled_pending_hint);
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

    handle_client_event(&mut app, ClientEvent::TurnCancelled);

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

    handle_client_event(&mut app, ClientEvent::TurnComplete { terminal_reason: None });

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
    app.paste.pending_text = "clipboard".to_owned();

    handle_normal_key(&mut app, KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));

    assert_eq!(app.input.text(), "");
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

#[test]
fn ctrl_backspace_and_delete_use_word_operations() {
    let mut app = make_test_app();
    app.input.set_text("hello world");

    handle_normal_key(&mut app, KeyEvent::new(KeyCode::Backspace, WORD_NAV_MOD));
    assert_eq!(app.input.text(), "hello ");

    app.input.move_home();
    handle_normal_key(&mut app, KeyEvent::new(KeyCode::Delete, WORD_NAV_MOD));
    assert_eq!(app.input.text(), " ");
}

#[test]
fn configured_undo_and_redo_restore_textarea_history() {
    let mut app = make_test_app();
    app.input.set_text("hello world");

    handle_normal_key(&mut app, KeyEvent::new(KeyCode::Backspace, WORD_NAV_MOD));
    assert_eq!(app.input.text(), "hello ");

    #[cfg(target_os = "macos")]
    handle_normal_key(&mut app, KeyEvent::new(KeyCode::Char('z'), CMD_MOD));
    #[cfg(target_os = "windows")]
    handle_normal_key(&mut app, KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL));
    #[cfg(all(unix, not(target_os = "macos")))]
    handle_normal_key(&mut app, KeyEvent::new(KeyCode::Char('_'), KeyModifiers::CONTROL));
    assert_eq!(app.input.text(), "hello world");

    #[cfg(target_os = "macos")]
    handle_normal_key(&mut app, KeyEvent::new(KeyCode::Char('z'), CMD_MOD | KeyModifiers::SHIFT));
    #[cfg(not(target_os = "macos"))]
    handle_normal_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL | KeyModifiers::SHIFT),
    );
    assert_eq!(app.input.text(), "hello ");
}

#[test]
fn ctrl_y_yanks_the_last_killed_text() {
    let mut app = make_test_app();
    app.input.set_text("hello world");
    app.input.move_home();

    handle_normal_key(&mut app, KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
    assert_eq!(app.input.text(), "");

    handle_normal_key(&mut app, KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL));
    assert_eq!(app.input.text(), "hello world");
}

#[test]
fn ctrl_left_right_move_by_word() {
    let mut app = make_test_app();
    app.input.set_text("hello world");
    app.input.move_home();

    handle_normal_key(&mut app, KeyEvent::new(KeyCode::Right, WORD_NAV_MOD));
    assert!(app.input.cursor_col() > 0);

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

    assert_eq!(app.turn.pending_interaction_ids, vec!["perm-b", "perm-a"]);
}

#[test]
fn permission_focus_allows_typing_for_non_permission_keys() {
    let mut app = make_test_app();
    app.turn.pending_interaction_ids.push("perm-1".into());
    app.claim_focus_target(FocusTarget::Permission);

    handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE)),
    );

    assert_eq!(app.input.text(), "h");
    assert_eq!(app.focus_owner(), FocusOwner::Input);
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
    assert_eq!(app.turn.pending_interaction_ids, vec![tool_id]);
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
    assert_eq!(app.turn.pending_interaction_ids, vec![tool_id]);
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
    assert!(app.turn.pending_interaction_ids.is_empty());
    assert!(bridge_rx.try_recv().is_ok());
    assert!(response_rx.try_recv().is_err());
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
    app.turn.pending_interaction_ids.insert(0, "stale-id".into());

    handle_terminal_event(&mut app, Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));

    let response = response_rx.try_recv().expect("permission response");
    assert!(matches!(response.outcome, model::RequestPermissionOutcome::Selected(_)));
    assert!(app.turn.pending_interaction_ids.is_empty());
}

#[test]
fn permission_focus_tab_returns_focus_to_input() {
    let mut app = make_test_app();
    let _response_rx = attach_pending_permission(
        &mut app,
        "perm-1",
        vec![
            model::PermissionOption::new("allow", "Allow", model::PermissionOptionKind::AllowOnce),
            model::PermissionOption::new("deny", "Deny", model::PermissionOptionKind::RejectOnce),
        ],
        true,
    );

    handle_terminal_event(&mut app, Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));

    assert_eq!(app.focus_owner(), FocusOwner::Input);
}

#[test]
fn update_notice_is_not_duplicated_within_same_session_epoch() {
    let mut app = make_test_app();
    app.update_notice = Some(crate::app::UpdateNoticeState {
        current_version: "0.11.1".into(),
        latest_version: "0.11.2".into(),
        emitted_session_scope_epoch: None,
    });

    session::ensure_update_notice_message(&mut app);
    session::ensure_update_notice_message(&mut app);

    assert_eq!(
        app.transcript.messages.iter().filter(|msg| is_update_notice_message(msg)).count(),
        1
    );
    assert_eq!(
        app.update_notice.as_ref().and_then(|notice| notice.emitted_session_scope_epoch),
        Some(app.session_runtime.session_scope_epoch)
    );
}

#[test]
fn update_notice_is_re_emitted_after_epoch_change() {
    let mut app = make_test_app();
    app.update_notice = Some(crate::app::UpdateNoticeState {
        current_version: "0.11.1".into(),
        latest_version: "0.11.2".into(),
        emitted_session_scope_epoch: None,
    });

    session::ensure_update_notice_message(&mut app);
    app.bump_session_scope_epoch();
    session::ensure_update_notice_message(&mut app);

    assert_eq!(
        app.transcript.messages.iter().filter(|msg| is_update_notice_message(msg)).count(),
        2
    );
    assert_eq!(
        app.update_notice.as_ref().and_then(|notice| notice.emitted_session_scope_epoch),
        Some(app.session_runtime.session_scope_epoch)
    );
}

#[test]
fn update_available_persists_across_connected_session_reset() {
    let mut app = make_test_app();

    handle_client_event(
        &mut app,
        ClientEvent::UpdateAvailable {
            latest_version: "0.11.2".into(),
            current_version: "0.11.1".into(),
        },
    );
    handle_client_event(&mut app, connected_event("claude-updated"));

    assert_eq!(
        app.transcript.messages.iter().filter(|msg| is_update_notice_message(msg)).count(),
        1
    );
    assert!(matches!(
        app.transcript.messages.first().map(|msg| &msg.role),
        Some(MessageRole::Welcome)
    ));
    let notice = app
        .transcript
        .messages
        .iter()
        .find(|msg| is_update_notice_message(msg))
        .expect("expected update notice message after connect");
    assert_eq!(
        first_block_text(notice),
        format!(
            "Update available: current v0.11.1, latest v0.11.2. Upgrade to latest version via {}.",
            session::update_install_command()
        )
    );
    assert_eq!(
        app.update_notice
            .as_ref()
            .and_then(|update_notice| update_notice.emitted_session_scope_epoch),
        Some(app.session_runtime.session_scope_epoch)
    );
}

#[test]
fn update_available_persists_across_session_replaced_reset() {
    let mut app = make_test_app();

    handle_client_event(
        &mut app,
        ClientEvent::UpdateAvailable {
            latest_version: "0.11.2".into(),
            current_version: "0.11.1".into(),
        },
    );
    handle_client_event(
        &mut app,
        ClientEvent::SessionReplaced {
            session_id: model::SessionId::new("replacement"),
            cwd: "/replacement".into(),
            current_model: test_current_model("new-model"),
            available_models: Vec::new(),
            mode: None,
            history_updates: Vec::new(),
            restored_input: None,
        },
    );

    assert_eq!(
        app.transcript.messages.iter().filter(|msg| is_update_notice_message(msg)).count(),
        1
    );
    assert!(matches!(
        app.transcript.messages.first().map(|msg| &msg.role),
        Some(MessageRole::Welcome)
    ));
    let notice = app
        .transcript
        .messages
        .iter()
        .find(|msg| is_update_notice_message(msg))
        .expect("expected update notice message after replacement");
    assert_eq!(
        first_block_text(notice),
        format!(
            "Update available: current v0.11.1, latest v0.11.2. Upgrade to latest version via {}.",
            session::update_install_command()
        )
    );
    assert_eq!(
        app.update_notice
            .as_ref()
            .and_then(|update_notice| update_notice.emitted_session_scope_epoch),
        Some(app.session_runtime.session_scope_epoch)
    );
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
    app.turn.pending_interaction_ids.push(tool_id.into());
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
    app.turn.pending_interaction_ids.push(tool_id.into());
    if focused {
        app.claim_focus_target(FocusTarget::Permission);
    }
    response_rx
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

#[test]
fn permission_ctrl_y_does_not_resolve_pending_permission() {
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

    handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL)),
    );

    assert!(matches!(
        response_rx.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    assert_eq!(app.turn.pending_interaction_ids, vec!["perm-1"]);
}

#[test]
fn permission_ctrl_a_does_not_resolve_pending_permission() {
    let mut app = make_test_app();
    let mut response_rx = attach_pending_permission(
        &mut app,
        "perm-1",
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
        ],
        true,
    );

    handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL)),
    );

    assert!(matches!(
        response_rx.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    assert_eq!(app.turn.pending_interaction_ids, vec!["perm-1"]);
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

    app.slash = Some(SlashState {
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
    assert_eq!(app.turn.pending_interaction_ids, vec!["perm-1"]);
}

#[test]
fn plan_approval_raw_ctrl_y_does_not_resolve_permission() {
    let mut app = make_test_app();
    app.input.set_text("seed");
    let mut response_rx = attach_pending_permission(
        &mut app,
        "perm-1",
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
        ],
        true,
    );

    handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('\u{19}'), KeyModifiers::NONE)),
    );

    assert!(matches!(
        response_rx.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    assert_eq!(app.input.text(), "seed");
    assert_eq!(app.turn.pending_interaction_ids, vec!["perm-1"]);
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
    assert!(app.turn.pending_interaction_ids.is_empty());
    assert_eq!(app.turn.pending_cancel_origin, None);

    handle_terminal_event(&mut app, Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));

    assert_eq!(app.turn.pending_cancel_origin, Some(CancelOrigin::Manual));
    let envelope = rx.try_recv().expect("second Esc should send turn cancel");
    assert!(matches!(
        envelope.command,
        crate::agent::wire::BridgeCommand::CancelTurn { session_id }
            if session_id == "session-1"
    ));
}

#[test]
fn connecting_state_blocks_input_shortcuts_and_tab() {
    let mut app = make_test_app();
    app.status = AppStatus::Connecting;
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
fn ctrl_c_quits() {
    let mut app = make_test_app();

    handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
    );

    assert!(app.should_quit);
}

#[test]
fn ctrl_c_clears_local_draft_before_quitting() {
    let mut app = make_test_app();
    app.input.set_text("draft");
    app.pending_submit = Some(app.input.snapshot());
    app.paste.pending_text = "queued paste".to_owned();
    app.pending_images.push(crate::app::clipboard_image::ImageAttachment {
        data: "image-data".to_owned(),
        mime_type: "image/png".to_owned(),
    });

    handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
    );

    assert!(!app.should_quit);
    assert!(app.input.is_empty());
    assert!(app.pending_submit.is_none());
    assert!(app.paste.pending_text.is_empty());
    assert!(app.pending_images.is_empty());

    handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
    );

    assert!(app.should_quit);
}

#[test]
fn ctrl_q_quits() {
    let mut app = make_test_app();

    handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL)),
    );

    assert!(app.should_quit);
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
}

#[test]
fn connecting_state_ctrl_q_quits() {
    let mut app = make_test_app();
    app.status = AppStatus::Connecting;

    handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL)),
    );

    assert!(app.should_quit);
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
fn error_state_ctrl_q_quits() {
    let mut app = make_test_app();
    app.status = AppStatus::Error;

    handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL)),
    );

    assert!(app.should_quit);
}

#[test]
fn error_state_ctrl_c_quits() {
    let mut app = make_test_app();
    app.status = AppStatus::Error;

    handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
    );

    assert!(app.should_quit);
}

#[test]
fn error_state_blocks_paste_events() {
    let mut app = make_test_app();
    app.status = AppStatus::Error;

    handle_terminal_event(&mut app, Event::Paste("blocked".into()));

    assert!(app.paste.pending_text.is_empty());
    assert!(app.input.is_empty());
}

#[test]
fn mention_owner_releases_back_to_input() {
    let mut app = make_test_app();
    app.slash = Some(SlashState {
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
    app.claim_focus_target(FocusTarget::Mention);

    handle_terminal_event(&mut app, Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));

    assert!(app.mention.is_none());
    assert_eq!(app.focus_owner(), FocusOwner::Input);
}

#[test]
fn settings_view_routes_space_to_settings_handler_not_chat_input() {
    let mut app = make_test_app();
    let dir = tempfile::tempdir().expect("tempdir");
    app.settings_home_override = Some(dir.path().to_path_buf());
    app.cwd_raw = dir.path().to_string_lossy().to_string();
    crate::app::config::open(&mut app).expect("open settings");
    app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::Config);
    app.config.selected_setting_index = crate::app::config::setting_specs()
        .iter()
        .position(|spec| spec.id == crate::app::config::SettingId::FastMode)
        .expect("fast mode setting row");
    app.input.set_text("seed");

    handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE)),
    );

    assert_eq!(app.input.text(), "seed");
    assert!(app.pending_submit.is_none());
    assert!(app.config.fast_mode_effective());
    assert!(app.config.last_error.is_none());
}

#[test]
fn settings_view_routes_enter_to_close_not_chat_submit() {
    let mut app = make_test_app();
    let dir = tempfile::tempdir().expect("tempdir");
    app.settings_home_override = Some(dir.path().to_path_buf());
    app.cwd_raw = dir.path().to_string_lossy().to_string();
    crate::app::config::open(&mut app).expect("open settings");
    app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::Config);
    app.input.set_text("seed");

    handle_terminal_event(&mut app, Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));

    assert_eq!(app.surface_mode, SurfaceMode::Chat);
    assert_eq!(app.input.text(), "seed");
    assert!(app.pending_submit.is_none());
}

#[test]
fn settings_view_ignores_paste_events() {
    let mut app = make_test_app();
    app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::Config);

    handle_terminal_event(&mut app, Event::Paste("blocked".into()));

    assert!(app.paste.pending_text.is_empty());
    assert!(app.input.is_empty());
}

#[test]
fn clipboard_paste_shortcut_dispatches_on_release() {
    let key = crossterm::event::KeyEvent {
        code: KeyCode::Char('v'),
        modifiers: KeyModifiers::CONTROL,
        kind: KeyEventKind::Release,
        state: crossterm::event::KeyEventState::NONE,
    };
    assert!(should_dispatch_key_event(key));
}

#[test]
fn non_paste_shortcut_release_is_ignored() {
    let key = crossterm::event::KeyEvent {
        code: KeyCode::Char('q'),
        modifiers: KeyModifiers::CONTROL,
        kind: KeyEventKind::Release,
        state: crossterm::event::KeyEventState::NONE,
    };
    assert!(!should_dispatch_key_event(key));
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
    app.config.preferences_path = Some(path);
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
    assert!(app.startup.connection_requested());
}

#[test]
fn trusted_view_ignores_paste_events() {
    let mut app = make_test_app();
    app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::Trusted);

    handle_terminal_event(&mut app, Event::Paste("blocked".into()));

    assert!(app.paste.pending_text.is_empty());
    assert!(app.input.is_empty());
}

#[test]
fn session_picker_ignores_paste_events() {
    let mut app = make_test_app();
    app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::SessionPicker);

    handle_terminal_event(&mut app, Event::Paste("blocked".into()));

    assert!(app.paste.pending_text.is_empty());
    assert!(app.input.is_empty());
}

#[test]
fn buffered_paste_char_does_not_request_redraw() {
    let mut app = make_test_app();
    let now = Instant::now();

    assert_eq!(
        app.paste.burst.on_char('a', now),
        super::super::paste_burst::CharAction::Passthrough('a')
    );
    assert_eq!(
        app.paste.burst.on_char('b', now + Duration::from_millis(1)),
        super::super::paste_burst::CharAction::Consumed
    );
    assert_eq!(
        app.paste.burst.on_char('c', now + Duration::from_millis(2)),
        super::super::paste_burst::CharAction::RetroCapture(1)
    );

    app.surface_dirty.chat.repaint = false;
    handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE)),
    );

    assert!(!app.surface_dirty.chat.repaint);
    assert!(app.input.is_empty());
}

#[test]
fn api_retry_updates_single_warning_notice() {
    let mut app = make_test_app();
    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::ApiRetryUpdate {
            attempt: 1,
            max_retries: 4,
            retry_delay_ms: 1000.0,
            error_status: None,
            error: model::ApiRetryError::Unknown,
        }),
    );
    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::ApiRetryUpdate {
            attempt: 2,
            max_retries: 4,
            retry_delay_ms: 1500.0,
            error_status: Some(529),
            error: model::ApiRetryError::ServerError,
        }),
    );

    assert_eq!(app.transcript.messages.len(), 1);
    assert_eq!(app.turn.notice_refs.len(), 1);
    let MessageBlock::Notice(notice) = &app.transcript.messages[0].blocks[0] else {
        panic!("expected API retry notice");
    };
    assert_eq!(notice.severity, SystemSeverity::Warning);
    assert_eq!(notice.text.text, "API retry 2/4 after server error HTTP 529, retrying in 1.5s",);
}

#[test]
fn system_notice_update_uses_notice_lane() {
    let mut app = make_test_app();
    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::SystemNoticeUpdate {
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
fn available_commands_update_replaces_previous_commands() {
    let mut app = make_test_app();
    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::AvailableCommandsUpdate(
            model::AvailableCommandsUpdate::new(vec![model::AvailableCommand::new(
                "/old",
                "Old command",
            )]),
        )),
    );
    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::AvailableCommandsUpdate(
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
        ClientEvent::SessionUpdate(model::SessionUpdate::RuntimeSessionStateUpdate(
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
        ClientEvent::SessionUpdate(model::SessionUpdate::RuntimeSessionStateUpdate(
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
        ClientEvent::SessionUpdate(model::SessionUpdate::SettingsParseError {
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
fn internal_error_detection_accepts_xml_payload() {
    use crate::agent::error_handling::looks_like_internal_error;
    let payload = "<error><code>-32603</code><message>Adapter process crashed</message></error>";
    assert!(looks_like_internal_error(payload));
}

#[test]
fn internal_error_detection_rejects_plain_bash_failure() {
    use crate::agent::error_handling::looks_like_internal_error;
    let payload = "bash: unknown_command: command not found";
    assert!(!looks_like_internal_error(payload));
}

#[test]
fn summarize_internal_error_prefers_xml_message() {
    use crate::agent::error_handling::summarize_internal_error;
    let payload = "<error><code>-32603</code><message>Adapter process crashed</message></error>";
    assert_eq!(summarize_internal_error(payload), "Adapter process crashed");
}

#[test]
fn summarize_internal_error_reads_json_rpc_message() {
    use crate::agent::error_handling::summarize_internal_error;
    let payload = r#"{"jsonrpc":"2.0","error":{"code":-32603,"message":"internal rpc fault"}}"#;
    assert_eq!(summarize_internal_error(payload), "internal rpc fault");
}

#[test]
fn internal_error_detection_accepts_permission_zod_payload() {
    use crate::agent::error_handling::looks_like_internal_error;
    let payload = "Tool permission request failed: ZodError: [{\"message\":\"Invalid input\"}]";
    assert!(looks_like_internal_error(payload));
}

#[test]
fn summarize_internal_error_prefers_permission_failure_summary() {
    use crate::agent::error_handling::summarize_internal_error;
    let payload = "Tool permission request failed: ZodError: [{\"message\":\"Invalid input: expected record, received undefined\"}]";
    assert_eq!(
        summarize_internal_error(payload),
        "Tool permission request failed: Invalid input: expected record, received undefined"
    );
}
