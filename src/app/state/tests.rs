// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::agent::model;
use crate::app::focus::{FocusOwner, FocusTarget};
use crate::app::slash::{SlashCandidate, SlashContext, SlashState};
use pretty_assertions::assert_eq;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use std::time::Instant;

// BlockCache

#[test]
fn cache_lifecycle_covers_default_store_invalidate_and_restore() {
    let mut cache = BlockCache::default();
    assert!(cache.get().is_none());

    cache.store(vec![Line::from("old")]);
    assert_eq!(cache.get().unwrap().len(), 1);

    cache.invalidate();
    cache.invalidate();
    cache.invalidate();
    assert!(cache.get().is_none());

    cache.store(vec![Line::from("new")]);
    let lines = cache.get().unwrap();
    assert_eq!(lines.len(), 1);
    let span_content: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(span_content, "new");
}

#[test]
fn cache_store_empty_lines() {
    let mut cache = BlockCache::default();
    cache.store(Vec::new());
    let lines = cache.get().unwrap();
    assert!(lines.is_empty());
}

/// Store twice without invalidating - second store overwrites first.
#[test]
fn cache_store_overwrite_without_invalidate() {
    let mut cache = BlockCache::default();
    cache.store(vec![Line::from("first")]);
    cache.store(vec![Line::from("second"), Line::from("line2")]);
    let lines = cache.get().unwrap();
    assert_eq!(lines.len(), 2);
    let content: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(content, "second");
}

/// `get()` called twice returns consistent data.
#[test]
fn cache_get_twice_consistent() {
    let mut cache = BlockCache::default();
    cache.store(vec![Line::from("stable")]);
    let first = cache.get().unwrap().len();
    let second = cache.get().unwrap().len();
    assert_eq!(first, second);
}

// BlockCache

#[test]
fn cache_store_many_lines() {
    let mut cache = BlockCache::default();
    let lines: Vec<Line<'static>> =
        (0..1000).map(|i| Line::from(Span::raw(format!("line {i}")))).collect();
    cache.store(lines);
    assert_eq!(cache.get().unwrap().len(), 1000);
}

#[test]
fn cache_store_splits_into_kb_segments() {
    let mut cache = BlockCache::default();
    let long = "x".repeat(800);
    let lines: Vec<Line<'static>> = (0..12).map(|_| Line::from(long.clone())).collect();
    cache.store(lines);
    assert!(cache.segment_count() > 1);
    assert!(cache.cached_bytes() > 0);
}

#[test]
fn cache_invalidate_without_store() {
    let mut cache = BlockCache::default();
    cache.invalidate();
    assert!(cache.get().is_none());
}

#[test]
fn cache_rapid_store_invalidate_cycle() {
    let mut cache = BlockCache::default();
    for i in 0..50 {
        cache.store(vec![Line::from(format!("v{i}"))]);
        assert!(cache.get().is_some());
        cache.invalidate();
        assert!(cache.get().is_none());
    }
    cache.store(vec![Line::from("final")]);
    assert!(cache.get().is_some());
}

/// Store styled lines with multiple spans per line.
#[test]
fn cache_store_styled_lines() {
    let mut cache = BlockCache::default();
    let line = Line::from(vec![
        Span::styled("bold", Style::default().fg(Color::Red)),
        Span::raw(" normal "),
        Span::styled("blue", Style::default().fg(Color::Blue)),
    ]);
    cache.store(vec![line]);
    let lines = cache.get().unwrap();
    assert_eq!(lines[0].spans.len(), 3);
}

/// Version counter after many invalidations - verify it doesn't
/// accidentally wrap to 0 (which would make stale data appear fresh).
/// With u64, 10K invalidations is nowhere near overflow.
#[test]
fn cache_version_no_false_fresh_after_many_invalidations() {
    let mut cache = BlockCache::default();
    cache.store(vec![Line::from("data")]);
    for _ in 0..10_000 {
        cache.invalidate();
    }
    // Cache was invalidated 10K times without re-storing - must be stale
    assert!(cache.get().is_none());
}

/// Invalidate, store, invalidate, store - alternating pattern.
#[test]
fn cache_alternating_invalidate_store() {
    let mut cache = BlockCache::default();
    for i in 0..100 {
        cache.invalidate();
        assert!(cache.get().is_none(), "stale after invalidate at iter {i}");
        cache.store(vec![Line::from(format!("v{i}"))]);
        assert!(cache.get().is_some(), "fresh after store at iter {i}");
    }
}

// BlockCache height

#[test]
fn cache_height_default_returns_none() {
    let cache = BlockCache::default();
    assert!(cache.height_at(80).is_none());
}

#[test]
fn cache_set_height_then_height_at() {
    let mut cache = BlockCache::default();
    cache.store(vec![Line::from("hello")]);
    cache.set_height(1, 80);
    assert_eq!(cache.height_at(80), Some(1));
    assert!(cache.get().is_some());
}

#[test]
fn cache_height_at_wrong_width_returns_none() {
    let mut cache = BlockCache::default();
    cache.store(vec![Line::from("hello")]);
    cache.set_height(1, 80);
    assert!(cache.height_at(120).is_none());
}

#[test]
fn cache_height_invalidated_returns_none() {
    let mut cache = BlockCache::default();
    cache.store(vec![Line::from("hello")]);
    cache.set_height(1, 80);
    cache.invalidate();
    assert!(cache.height_at(80).is_none());
}

#[test]
fn clear_session_runtime_identity_resets_session_usage() {
    let mut app = App::test_default();
    app.session_runtime.session_id = Some(crate::agent::model::SessionId::new("session-1"));
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
    app.session_runtime.session_usage.last_compaction_pre_tokens = Some(123_456);

    app.clear_session_runtime_identity();

    assert!(app.session_runtime.session_id.is_none());
    assert!(app.session_runtime.current_model.is_none());
    assert!(app.session_runtime.mode.is_none());
    assert_eq!(app.session_runtime.session_usage, SessionUsageState::default());
}

#[test]
fn test_default_initializes_chat_render_state() {
    let app = App::test_default();

    assert_eq!(app.chat_render, ChatRenderState::default());
}

#[test]
fn cache_store_without_height_has_no_height() {
    let mut cache = BlockCache::default();
    cache.store(vec![Line::from("hello")]);
    // store() without height leaves wrapped_width at 0
    assert!(cache.height_at(80).is_none());
}

#[test]
fn cache_store_and_set_height_overwrite() {
    let mut cache = BlockCache::default();
    cache.store(vec![Line::from("old")]);
    cache.set_height(1, 80);
    cache.invalidate();
    cache.store(vec![Line::from("new long line")]);
    cache.set_height(3, 120);
    assert_eq!(cache.height_at(120), Some(3));
    assert!(cache.height_at(80).is_none());
}

// BlockCache set_height (separate from store)

#[test]
fn cache_set_height_after_store() {
    let mut cache = BlockCache::default();
    cache.store(vec![Line::from("hello")]);
    assert!(cache.height_at(80).is_none()); // no height yet
    cache.set_height(1, 80);
    assert_eq!(cache.height_at(80), Some(1));
    assert!(cache.get().is_some()); // lines still valid
}

#[test]
fn cache_set_height_update_width() {
    let mut cache = BlockCache::default();
    cache.store(vec![Line::from("hello world")]);
    cache.set_height(1, 80);
    assert_eq!(cache.height_at(80), Some(1));
    // Re-measure at new width
    cache.set_height(2, 40);
    assert_eq!(cache.height_at(40), Some(2));
    assert!(cache.height_at(80).is_none()); // old width no longer valid
}

#[test]
fn cache_set_height_invalidate_clears_height() {
    let mut cache = BlockCache::default();
    cache.store(vec![Line::from("data")]);
    cache.set_height(3, 80);
    cache.invalidate();
    assert!(cache.height_at(80).is_none()); // version mismatch
}

#[test]
fn cache_set_height_on_invalidated_cache_returns_none() {
    let mut cache = BlockCache::default();
    cache.store(vec![Line::from("data")]);
    cache.invalidate(); // version != 0
    cache.set_height(5, 80);
    // height_at returns None because cache is stale (version != 0)
    assert!(cache.height_at(80).is_none());
}

#[test]
fn cache_get_updates_last_access_tick() {
    let mut cache = BlockCache::default();
    cache.store(vec![Line::from("tick")]);
    let before = cache.last_access_tick();
    let _ = cache.get();
    let after = cache.last_access_tick();
    assert!(after > before);
}

// App tool_call_index

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

fn user_text_image_message(text: &str, image_count: usize) -> ChatMessage {
    ChatMessage::new(
        MessageRole::User,
        vec![
            assistant_text_block(text),
            MessageBlock::ImageAttachment(ImageAttachmentBlock::new(image_count)),
        ],
        None,
    )
}

fn set_account_subscription(app: &mut App, subscription: &str) {
    app.session_runtime.account_info = Some(crate::agent::model::AccountInfo {
        subscription_type: Some(subscription.to_owned()),
        ..Default::default()
    });
}

#[test]
fn push_message_tracked_appends_user_message_and_requests_repaint() {
    let mut app = make_test_app();
    let _ = app.surface_dirty.chat.take_repaint();

    app.push_message_tracked(user_text_message("hello"));

    assert_eq!(app.transcript.messages.len(), 1);
    assert!(matches!(app.transcript.messages[0].role, MessageRole::User));
    let MessageBlock::Text(text) = &app.transcript.messages[0].blocks[0] else {
        panic!("expected text block");
    };
    assert_eq!(text.text, "hello");
    assert!(app.surface_dirty.chat.repaint);
}

#[test]
fn push_message_tracked_preserves_message_order() {
    let mut app = make_test_app();

    app.push_message_tracked(user_text_message("first"));
    app.push_message_tracked(system_text_message("second"));

    assert_eq!(app.transcript.messages.len(), 2);
    assert!(matches!(app.transcript.messages[0].role, MessageRole::User));
    assert!(matches!(
        app.transcript.messages[1].role,
        MessageRole::System(Some(SystemSeverity::Info))
    ));
}

#[test]
fn sync_welcome_snapshot_updates_canonical_welcome_message() {
    let mut app = make_test_app();
    app.ensure_welcome_message();

    app.session_runtime.session_id = Some(crate::agent::model::SessionId::new("session-1"));
    set_account_subscription(&mut app, "Pro");

    app.sync_welcome_snapshot();
    app.sync_welcome_snapshot();

    let MessageBlock::Welcome(welcome) = &app.transcript.messages[0].blocks[0] else {
        panic!("expected welcome block");
    };
    assert_eq!(welcome.subscription, "Pro");
    assert_eq!(welcome.session_id, "session-1");
}

#[test]
fn sync_welcome_snapshot_updates_existing_canonical_welcome_in_place() {
    let mut app = make_test_app();
    app.ensure_welcome_message();
    app.session_runtime.session_id = Some(crate::agent::model::SessionId::new("session-1"));
    set_account_subscription(&mut app, "Pro");
    app.sync_welcome_snapshot();

    set_account_subscription(&mut app, "Claude Max");
    app.sync_welcome_snapshot();

    let MessageBlock::Welcome(welcome) = &app.transcript.messages[0].blocks[0] else {
        panic!("expected welcome block");
    };
    assert_eq!(app.transcript.messages.len(), 1);
    assert_eq!(welcome.subscription, "Claude Max");
}

#[test]
fn push_message_tracked_preserves_user_image_attachment_block() {
    let mut app = make_test_app();

    app.push_message_tracked(user_text_image_message("see attached", 2));

    assert_eq!(app.transcript.messages.len(), 1);
    assert!(matches!(app.transcript.messages[0].role, MessageRole::User));
    let MessageBlock::Text(text) = &app.transcript.messages[0].blocks[0] else {
        panic!("expected text block");
    };
    assert_eq!(text.text, "see attached");
    let MessageBlock::ImageAttachment(image) = &app.transcript.messages[0].blocks[1] else {
        panic!("expected image attachment block");
    };
    assert_eq!(image.count, 2);
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

fn assistant_tool_message_with_pending_permission(id: &str) -> ChatMessage {
    let (tx, _rx) = tokio::sync::oneshot::channel();
    let mut tool_call =
        crate::app::test_support::tool_call_info(id, model::ToolCallStatus::Completed);
    tool_call.title = format!("tool {id}");
    tool_call.terminal_output = Some("x".repeat(1024));
    tool_call.terminal_output_len = 1024;
    tool_call.pending_permission = Some(InlinePermission {
        options: vec![model::PermissionOption::new(
            "allow-once",
            "Allow once",
            model::PermissionOptionKind::AllowOnce,
        )],
        display: None,
        subagent_context: None,
        response_tx: tx,
        selected_index: 0,
        focused: false,
    });
    ChatMessage::new(
        MessageRole::Assistant,
        vec![MessageBlock::ToolCall(Box::new(tool_call))],
        None,
    )
}

fn pending_user_dialog_message(request_id: &str) -> ChatMessage {
    let (tx, _rx) = tokio::sync::oneshot::channel();
    ChatMessage::new(
        MessageRole::System(Some(SystemSeverity::Warning)),
        vec![MessageBlock::UserDialog(UserDialogBlock::new(
            model::RequestUserDialogRequest::new(
                "session-1",
                request_id,
                "refusal_fallback_prompt",
                model::RefusalFallbackPayload {
                    original_model: "claude-opus-4-8".to_owned(),
                    fallback_model: "claude-sonnet-4-6".to_owned(),
                    ..Default::default()
                },
                vec![
                    model::UserDialogOption::new("retry_fallback", "Switch to claude-sonnet-4-6"),
                    model::UserDialogOption::new(
                        "edit_prompt",
                        "Edit prompt and retry with claude-opus-4-8",
                    ),
                ],
            ),
            tx,
        ))],
        None,
    )
}

#[test]
fn enforce_render_cache_budget_evicts_lru_block() {
    let mut app = make_test_app();
    app.transcript.messages = vec![
        ChatMessage::new(MessageRole::Assistant, vec![assistant_text_block("a")], None),
        ChatMessage::new(MessageRole::Assistant, vec![assistant_text_block("b")], None),
    ];

    let bytes_a = if let MessageBlock::Text(block) = &mut app.transcript.messages[0].blocks[0] {
        block.cache.store(vec![Line::from("x".repeat(2200))]);
        block.cache.cached_bytes()
    } else {
        0
    };
    let bytes_b = if let MessageBlock::Text(block) = &mut app.transcript.messages[1].blocks[0] {
        block.cache.store(vec![Line::from("y".repeat(2200))]);
        let _ = block.cache.get();
        block.cache.cached_bytes()
    } else {
        0
    };

    app.render_cache_budget.max_bytes = bytes_b;
    let stats = app.enforce_render_cache_budget();
    assert!(stats.evicted_blocks >= 1);
    assert!(stats.evicted_bytes >= bytes_a);
    assert!(stats.total_after_bytes <= app.render_cache_budget.max_bytes);
    assert_eq!(stats.protected_bytes, 0);

    if let MessageBlock::Text(block) = &app.transcript.messages[0].blocks[0] {
        assert_eq!(block.cache.cached_bytes(), 0);
    } else {
        panic!("expected text block");
    }
    if let MessageBlock::Text(block) = &app.transcript.messages[1].blocks[0] {
        assert_eq!(block.cache.cached_bytes(), bytes_b);
    } else {
        panic!("expected text block");
    }
}

#[test]
fn enforce_render_cache_budget_protects_streaming_tail_message() {
    let mut app = make_test_app();
    app.status = AppStatus::Thinking;
    app.transcript.messages = vec![ChatMessage::new(
        MessageRole::Assistant,
        vec![assistant_text_block("streaming tail")],
        None,
    )];

    let before = if let MessageBlock::Text(block) = &mut app.transcript.messages[0].blocks[0] {
        block.cache.store(vec![Line::from("z".repeat(4096))]);
        block.cache.cached_bytes()
    } else {
        0
    };
    app.render_cache_budget.max_bytes = 64;
    let stats = app.enforce_render_cache_budget();
    assert_eq!(stats.evicted_blocks, 0);
    assert_eq!(stats.evicted_bytes, 0);
    assert_eq!(stats.protected_bytes, before);

    if let MessageBlock::Text(block) = &app.transcript.messages[0].blocks[0] {
        assert_eq!(block.cache.cached_bytes(), before);
    } else {
        panic!("expected text block");
    }
}

#[test]
fn enforce_render_cache_budget_excludes_protected_from_budget() {
    let mut app = make_test_app();
    app.status = AppStatus::Running;
    app.transcript.messages = vec![
        ChatMessage::new(MessageRole::Assistant, vec![assistant_text_block("old message")], None),
        ChatMessage::new(
            MessageRole::Assistant,
            vec![assistant_text_block("streaming tail")],
            None,
        ),
    ];

    let bytes_a = if let MessageBlock::Text(block) = &mut app.transcript.messages[0].blocks[0] {
        block.cache.store(vec![Line::from("x".repeat(2200))]);
        block.cache.cached_bytes()
    } else {
        0
    };
    let bytes_b = if let MessageBlock::Text(block) = &mut app.transcript.messages[1].blocks[0] {
        block.cache.store(vec![Line::from("y".repeat(5000))]);
        block.cache.cached_bytes()
    } else {
        0
    };

    // Budget fits old message alone but not old + tail combined.
    app.render_cache_budget.max_bytes = bytes_a + 100;
    assert!(bytes_a + bytes_b > app.render_cache_budget.max_bytes);

    let stats = app.enforce_render_cache_budget();

    // Protected bytes should be the streaming tail.
    assert_eq!(stats.protected_bytes, bytes_b);
    // No eviction: budgeted bytes (bytes_a) are under max_bytes.
    assert_eq!(stats.evicted_blocks, 0);
    assert_eq!(stats.evicted_bytes, 0);
    // Old message cache intact.
    if let MessageBlock::Text(block) = &app.transcript.messages[0].blocks[0] {
        assert_eq!(block.cache.cached_bytes(), bytes_a);
    } else {
        panic!("expected text block");
    }
}

#[test]
fn enforce_render_cache_budget_protects_active_streaming_owner_not_physical_tail() {
    let mut app = make_test_app();
    app.status = AppStatus::Running;
    app.transcript.messages = vec![
        ChatMessage::new(MessageRole::Assistant, vec![assistant_text_block("old message")], None),
        ChatMessage::new(
            MessageRole::Assistant,
            vec![assistant_text_block("active streaming owner")],
            None,
        ),
        ChatMessage::new(
            MessageRole::System(Some(SystemSeverity::Info)),
            vec![assistant_text_block("late trailing system row")],
            None,
        ),
    ];
    app.bind_active_turn_assistant(1);

    if let MessageBlock::Text(block) = &mut app.transcript.messages[0].blocks[0] {
        block.cache.store(vec![Line::from("x".repeat(2000))]);
    }
    let protected_bytes =
        if let MessageBlock::Text(block) = &mut app.transcript.messages[1].blocks[0] {
            block.cache.store(vec![Line::from("y".repeat(4000))]);
            block.cache.cached_bytes()
        } else {
            0
        };
    if let MessageBlock::Text(block) = &mut app.transcript.messages[2].blocks[0] {
        block.cache.store(vec![Line::from("z".repeat(5000))]);
    }

    app.render_cache_budget.max_bytes = 64;
    let stats = app.enforce_render_cache_budget();

    assert_eq!(stats.protected_bytes, protected_bytes);
}

#[test]
fn enforce_render_cache_budget_evicts_when_budgeted_over_limit() {
    let mut app = make_test_app();
    app.status = AppStatus::Running;
    app.transcript.messages = vec![
        ChatMessage::new(MessageRole::Assistant, vec![assistant_text_block("old-a")], None),
        ChatMessage::new(MessageRole::Assistant, vec![assistant_text_block("old-b")], None),
        ChatMessage::new(MessageRole::Assistant, vec![assistant_text_block("streaming")], None),
    ];

    // Populate caches: messages 0 and 1 evictable, message 2 protected.
    if let MessageBlock::Text(block) = &mut app.transcript.messages[0].blocks[0] {
        block.cache.store(vec![Line::from("x".repeat(3000))]);
    }
    let bytes_b = if let MessageBlock::Text(block) = &mut app.transcript.messages[1].blocks[0] {
        block.cache.store(vec![Line::from("y".repeat(3000))]);
        let _ = block.cache.get(); // touch to make more recently accessed
        block.cache.cached_bytes()
    } else {
        0
    };
    let bytes_c = if let MessageBlock::Text(block) = &mut app.transcript.messages[2].blocks[0] {
        block.cache.store(vec![Line::from("z".repeat(5000))]);
        block.cache.cached_bytes()
    } else {
        0
    };

    // Budget fits message B but not A+B (excludes C as protected).
    app.render_cache_budget.max_bytes = bytes_b + 100;

    let stats = app.enforce_render_cache_budget();

    assert_eq!(stats.protected_bytes, bytes_c);
    assert!(stats.evicted_blocks >= 1); // message A evicted (older access)
    // Message B should survive (more recent access).
    if let MessageBlock::Text(block) = &app.transcript.messages[1].blocks[0] {
        assert_eq!(block.cache.cached_bytes(), bytes_b);
    } else {
        panic!("expected text block");
    }
}

#[test]
fn enforce_render_cache_budget_protected_bytes_zero_when_not_streaming() {
    let mut app = make_test_app();
    app.status = AppStatus::Ready;
    app.transcript.messages =
        vec![ChatMessage::new(MessageRole::Assistant, vec![assistant_text_block("done")], None)];

    if let MessageBlock::Text(block) = &mut app.transcript.messages[0].blocks[0] {
        block.cache.store(vec![Line::from("x".repeat(2000))]);
    }
    app.render_cache_budget.max_bytes = usize::MAX;

    let stats = app.enforce_render_cache_budget();
    assert_eq!(stats.protected_bytes, 0);
}

#[test]
fn enforce_history_retention_noop_under_budget() {
    let mut app = make_test_app();
    app.transcript.messages = vec![
        ChatMessage::welcome(env!("CARGO_PKG_VERSION"), "-", "/cwd", "-"),
        user_text_message("small message"),
        user_text_message("another message"),
    ];
    app.history_retention.max_bytes = usize::MAX / 4;

    let stats = app.enforce_history_retention();
    assert_eq!(stats.dropped_messages, 0);
    assert_eq!(stats.total_dropped_messages, 0);
    assert!(!app.transcript.messages.iter().any(App::is_history_hidden_marker_message));
}

#[test]
fn enforce_history_retention_drops_oldest_and_adds_marker() {
    let mut app = make_test_app();
    app.transcript.messages = vec![
        ChatMessage::welcome(env!("CARGO_PKG_VERSION"), "-", "/cwd", "-"),
        user_text_message("first old message"),
        user_text_message("second old message"),
        user_text_message("third old message"),
    ];
    app.history_retention.max_bytes = 1;

    let stats = app.enforce_history_retention();
    assert_eq!(stats.dropped_messages, 3);
    assert!(matches!(app.transcript.messages[0].role, MessageRole::Welcome));
    assert!(app.transcript.messages.iter().any(App::is_history_hidden_marker_message));
    assert_eq!(app.transcript.messages.len(), 2);
}

#[test]
fn enforce_history_retention_preserves_in_progress_tool_message() {
    let mut app = make_test_app();
    app.transcript.messages = vec![
        ChatMessage::welcome(env!("CARGO_PKG_VERSION"), "-", "/cwd", "-"),
        user_text_message("droppable"),
        assistant_tool_message("tool-keep", model::ToolCallStatus::InProgress),
    ];
    app.history_retention.max_bytes = 1;

    let stats = app.enforce_history_retention();
    assert_eq!(stats.dropped_messages, 1);
    assert!(app.transcript.messages.iter().any(|msg| {
        msg.blocks.iter().any(|block| {
            matches!(
                block,
                MessageBlock::ToolCall(tc) if tc.id == "tool-keep"
                    && matches!(tc.status, model::ToolCallStatus::InProgress)
            )
        })
    }));
}

#[test]
fn enforce_history_retention_preserves_pending_tool_message() {
    let mut app = make_test_app();
    app.transcript.messages = vec![
        ChatMessage::welcome(env!("CARGO_PKG_VERSION"), "-", "/cwd", "-"),
        user_text_message("droppable"),
        assistant_tool_message("tool-pending", model::ToolCallStatus::Pending),
    ];
    app.history_retention.max_bytes = 1;

    let stats = app.enforce_history_retention();
    assert_eq!(stats.dropped_messages, 1);
    assert!(app.transcript.messages.iter().any(|msg| {
        msg.blocks
            .iter()
            .any(|block| matches!(block, MessageBlock::ToolCall(tc) if tc.id == "tool-pending"))
    }));
}

#[test]
fn enforce_history_retention_preserves_permission_tool_message() {
    let mut app = make_test_app();
    app.transcript.messages = vec![
        ChatMessage::welcome(env!("CARGO_PKG_VERSION"), "-", "/cwd", "-"),
        user_text_message("droppable"),
        assistant_tool_message_with_pending_permission("tool-perm"),
    ];
    app.history_retention.max_bytes = 1;

    let stats = app.enforce_history_retention();
    assert_eq!(stats.dropped_messages, 1);
    assert!(app.transcript.messages.iter().any(|msg| {
        msg.blocks
            .iter()
            .any(|block| matches!(block, MessageBlock::ToolCall(tc) if tc.id == "tool-perm"))
    }));
}

#[test]
fn enforce_history_retention_preserves_and_rebuilds_pending_user_dialog() {
    let mut app = make_test_app();
    app.transcript.messages = vec![
        ChatMessage::welcome(env!("CARGO_PKG_VERSION"), "-", "/cwd", "-"),
        user_text_message("droppable"),
        pending_user_dialog_message("dialog-1"),
    ];
    app.index_tool_call("dialog-1".to_owned(), 99, 99);
    app.turn.pending_interaction_ids = vec!["stale-dialog".to_owned(), "dialog-1".to_owned()];
    app.history_retention.max_bytes = 1;

    let stats = app.enforce_history_retention();

    assert_eq!(stats.dropped_messages, 1);
    assert_eq!(app.lookup_tool_call("dialog-1"), Some((2, 0)));
    assert_eq!(app.turn.pending_interaction_ids, vec!["dialog-1".to_owned()]);
    let Some((msg_idx, block_idx)) = app.lookup_tool_call("dialog-1") else {
        panic!("expected rebuilt dialog index");
    };
    let Some(MessageBlock::UserDialog(dialog)) =
        app.transcript.messages.get(msg_idx).and_then(|msg| msg.blocks.get(block_idx))
    else {
        panic!("expected user dialog block");
    };
    assert!(dialog.focused);
    assert!(!dialog.answered);
}

#[test]
fn enforce_history_retention_rebuilds_tool_index_after_prune() {
    let mut app = make_test_app();
    app.transcript.messages = vec![
        ChatMessage::welcome(env!("CARGO_PKG_VERSION"), "-", "/cwd", "-"),
        user_text_message("drop this"),
        assistant_bash_tool_message("tool-idx", model::ToolCallStatus::InProgress, "term-1"),
    ];
    app.index_tool_call("tool-idx".to_owned(), 99, 99);
    app.history_retention.max_bytes = 1;

    let _ = app.enforce_history_retention();
    assert_eq!(app.lookup_tool_call("tool-idx"), Some((2, 0)));
}

#[test]
fn enforce_history_retention_preserves_active_turn_assistant_message() {
    let mut app = make_test_app();
    app.status = AppStatus::Thinking;
    app.transcript.messages = vec![
        ChatMessage::welcome(env!("CARGO_PKG_VERSION"), "-", "/cwd", "-"),
        user_text_message("drop this"),
        ChatMessage::new(MessageRole::Assistant, Vec::new(), None),
    ];
    app.bind_active_turn_assistant(2);
    app.history_retention.max_bytes = 1;

    let stats = app.enforce_history_retention();

    assert_eq!(stats.dropped_messages, 1);
    assert_eq!(app.active_turn_assistant_idx(), Some(2));
    assert!(matches!(app.transcript.messages[2].role, MessageRole::Assistant));
}

#[test]
fn enforce_history_retention_remaps_active_turn_assistant_after_prune() {
    let mut app = make_test_app();
    app.status = AppStatus::Thinking;
    app.transcript.messages = vec![
        user_text_message("drop this"),
        ChatMessage::new(
            MessageRole::Assistant,
            vec![assistant_text_block("streaming reply")],
            None,
        ),
    ];
    app.bind_active_turn_assistant(1);
    app.history_retention.max_bytes = App::measure_message_bytes(&app.transcript.messages[1]);

    let stats = app.enforce_history_retention();

    assert_eq!(stats.dropped_messages, 1);
    assert_eq!(app.active_turn_assistant_idx(), Some(1));
    assert!(App::is_history_hidden_marker_message(&app.transcript.messages[0]));
    assert!(matches!(app.transcript.messages[1].role, MessageRole::Assistant));
}

#[test]
fn enforce_history_retention_keeps_single_marker_on_repeat() {
    let mut app = make_test_app();
    app.transcript.messages = vec![
        ChatMessage::welcome(env!("CARGO_PKG_VERSION"), "-", "/cwd", "-"),
        user_text_message("drop me"),
    ];
    app.history_retention.max_bytes = 1;

    let first = app.enforce_history_retention();
    let second = app.enforce_history_retention();
    let marker_count = app
        .transcript
        .messages
        .iter()
        .filter(|msg| App::is_history_hidden_marker_message(msg))
        .count();

    assert_eq!(first.dropped_messages, 1);
    assert_eq!(second.dropped_messages, 0);
    assert_eq!(marker_count, 1);
}

#[allow(clippy::cast_precision_loss)]
#[test]
fn lookup_missing_returns_none() {
    let app = make_test_app();
    assert!(app.lookup_tool_call("nonexistent").is_none());
}

#[test]
fn index_and_lookup() {
    let mut app = make_test_app();
    app.index_tool_call("tc-123".into(), 2, 5);
    assert_eq!(app.lookup_tool_call("tc-123"), Some((2, 5)));
}

// App tool_call_index

/// Index same ID twice - second write overwrites first.
#[test]
fn index_overwrite_existing() {
    let mut app = make_test_app();
    app.index_tool_call("tc-1".into(), 0, 0);
    app.index_tool_call("tc-1".into(), 5, 10);
    assert_eq!(app.lookup_tool_call("tc-1"), Some((5, 10)));
}

/// Empty string as tool call ID.
#[test]
fn index_empty_string_id() {
    let mut app = make_test_app();
    app.index_tool_call(String::new(), 1, 2);
    assert_eq!(app.lookup_tool_call(""), Some((1, 2)));
}

/// Stress: 1000 tool calls indexed and looked up.
#[test]
fn index_stress_1000_entries() {
    let mut app = make_test_app();
    for i in 0..1000 {
        app.index_tool_call(format!("tc-{i}"), i, i * 2);
    }
    // Spot check first, middle, last
    assert_eq!(app.lookup_tool_call("tc-0"), Some((0, 0)));
    assert_eq!(app.lookup_tool_call("tc-500"), Some((500, 1000)));
    assert_eq!(app.lookup_tool_call("tc-999"), Some((999, 1998)));
    // Non-existent still returns None
    assert!(app.lookup_tool_call("tc-1000").is_none());
}

/// Unicode in tool call ID.
#[test]
fn index_unicode_id() {
    let mut app = make_test_app();
    app.index_tool_call("\u{1F600}-tool".into(), 3, 7);
    assert_eq!(app.lookup_tool_call("\u{1F600}-tool"), Some((3, 7)));
}

// active_task_ids

#[test]
fn active_task_insert_remove() {
    let mut app = make_test_app();
    app.insert_active_task("task-1".into());
    assert!(app.turn.active_task_ids.contains("task-1"));
    app.remove_active_task("task-1");
    assert!(!app.turn.active_task_ids.contains("task-1"));
}

#[test]
fn remove_nonexistent_task_is_noop() {
    let mut app = make_test_app();
    app.remove_active_task("does-not-exist");
    assert!(app.turn.active_task_ids.is_empty());
}

// active_task_ids

/// Insert same ID twice - set deduplicates; one remove clears it.
#[test]
fn active_task_insert_duplicate() {
    let mut app = make_test_app();
    app.insert_active_task("task-1".into());
    app.insert_active_task("task-1".into());
    assert_eq!(app.turn.active_task_ids.len(), 1);
    app.remove_active_task("task-1");
    assert!(app.turn.active_task_ids.is_empty());
}

/// Insert many tasks, remove in different order.
#[test]
fn active_task_insert_many_remove_out_of_order() {
    let mut app = make_test_app();
    for i in 0..100 {
        app.insert_active_task(format!("task-{i}"));
    }
    assert_eq!(app.turn.active_task_ids.len(), 100);
    // Remove in reverse order
    for i in (0..100).rev() {
        app.remove_active_task(&format!("task-{i}"));
    }
    assert!(app.turn.active_task_ids.is_empty());
}

/// Mixed insert/remove interleaving.
#[test]
fn active_task_interleaved_insert_remove() {
    let mut app = make_test_app();
    app.insert_active_task("a".into());
    app.insert_active_task("b".into());
    app.remove_active_task("a");
    app.insert_active_task("c".into());
    assert!(!app.turn.active_task_ids.contains("a"));
    assert!(app.turn.active_task_ids.contains("b"));
    assert!(app.turn.active_task_ids.contains("c"));
    assert_eq!(app.turn.active_task_ids.len(), 2);
}

/// Remove from empty set multiple times - no panic.
#[test]
fn active_task_remove_from_empty_repeatedly() {
    let mut app = make_test_app();
    for i in 0..100 {
        app.remove_active_task(&format!("ghost-{i}"));
    }
    assert!(app.turn.active_task_ids.is_empty());
}

/// `clear_tool_scope_tracking` must also clear `active_task_ids`.
/// Regression test: before the fix, a leaked task ID from a cancelled turn
/// caused main-agent tools on the next turn to be misclassified as Subagent scope.
#[test]
fn clear_tool_scope_tracking_also_clears_active_task_ids() {
    let mut app = make_test_app();
    app.insert_active_task("task-leaked".into());
    assert!(!app.turn.active_task_ids.is_empty());
    app.clear_tool_scope_tracking();
    assert!(app.turn.active_task_ids.is_empty(), "active_task_ids must be cleared at turn end");
}

#[test]
fn finalize_in_progress_tool_calls_preserves_terminal_metadata() {
    let mut app = make_test_app();
    app.transcript.messages.push(assistant_bash_tool_message(
        "bash-1",
        model::ToolCallStatus::InProgress,
        "term-1",
    ));
    app.index_tool_call("bash-1".to_owned(), 0, 0);

    let changed = app.finalize_in_progress_tool_calls(model::ToolCallStatus::Completed);

    assert_eq!(changed, 1);
    let MessageBlock::ToolCall(tc) = &app.transcript.messages[0].blocks[0] else {
        panic!("expected tool call");
    };
    assert_eq!(tc.status, model::ToolCallStatus::Completed);
    assert_eq!(tc.terminal_id.as_deref(), Some("term-1"));
}

#[test]
fn remove_message_tracked_tail_removes_orphaned_tool_indices() {
    let mut app = make_test_app();
    app.transcript.messages.push(user_text_message("before"));
    app.transcript
        .messages
        .push(assistant_tool_message("tool-1", model::ToolCallStatus::Completed));
    app.index_tool_call("tool-1".to_owned(), 1, 0);

    let removed = app.remove_message_tracked(1);

    assert!(removed.is_some());
    assert!(app.lookup_tool_call("tool-1").is_none());
}

#[test]
fn remove_message_tracked_prunes_tool_scope_entries() {
    let mut app = make_test_app();
    app.transcript
        .messages
        .push(assistant_tool_message("tool-1", model::ToolCallStatus::Completed));
    app.index_tool_call("tool-1".to_owned(), 0, 0);
    app.register_tool_call_scope(
        "tool-1".to_owned(),
        ToolCallScope::SubagentChild { parent_tool_use_id: "task-1".to_owned() },
    );

    let removed = app.remove_message_tracked(0);

    assert!(removed.is_some());
    assert_eq!(app.tool_call_scope("tool-1"), None);
}

#[test]
fn clear_messages_tracked_clears_tool_tracking() {
    let mut app = make_test_app();
    app.transcript.messages.push(assistant_bash_tool_message(
        "bash-1",
        model::ToolCallStatus::InProgress,
        "term-1",
    ));
    app.index_tool_call("bash-1".to_owned(), 0, 0);
    app.turn.pending_interaction_ids.push("bash-1".into());

    app.clear_messages_tracked();

    assert!(app.transcript.messages.is_empty());
    assert!(app.transcript.tool_call_index.is_empty());
    assert!(app.turn.pending_interaction_ids.is_empty());
}

#[test]
fn rebuild_tool_indices_includes_completed_tools() {
    let mut app = make_test_app();
    app.transcript.messages.push(assistant_bash_tool_message(
        "bash-1",
        model::ToolCallStatus::Completed,
        "term-1",
    ));
    app.index_tool_call("bash-1".to_owned(), 0, 0);

    app.rebuild_tool_indices();

    assert_eq!(app.lookup_tool_call("bash-1"), Some((0, 0)));
}

// IncrementalMarkdown

/// Simple render function for tests: wraps each line in a `Line`.
fn test_render(src: &str) -> Vec<Line<'static>> {
    src.lines().map(|l| Line::from(l.to_owned())).collect()
}

fn test_render_key() -> super::messages::MarkdownRenderKey {
    super::messages::MarkdownRenderKey { width: 80, bg: None, preserve_newlines: false }
}

#[test]
fn incr_default_empty() {
    let incr = IncrementalMarkdown::default();
    assert!(incr.full_text().is_empty());
}

#[test]
fn incr_from_complete() {
    let incr = IncrementalMarkdown::from_complete("hello world");
    assert_eq!(incr.full_text(), "hello world");
}

#[test]
fn incr_append_single_chunk() {
    let mut incr = IncrementalMarkdown::default();
    incr.append("hello");
    assert_eq!(incr.full_text(), "hello");
}

#[test]
fn incr_append_accumulates_chunks() {
    let mut incr = IncrementalMarkdown::default();
    incr.append("line1");
    incr.append("\nline2");
    incr.append("\nline3");
    assert_eq!(incr.full_text(), "line1\nline2\nline3");
}

#[test]
fn incr_append_preserves_paragraph_delimiters() {
    let mut incr = IncrementalMarkdown::default();
    incr.append("para1\n\npara2");
    assert_eq!(incr.full_text(), "para1\n\npara2");
}

#[test]
fn incr_full_text_reconstruction() {
    let mut incr = IncrementalMarkdown::default();
    incr.append("p1\n\np2\n\np3");
    assert_eq!(incr.full_text(), "p1\n\np2\n\np3");
}

#[test]
fn incr_lines_renders_all() {
    let mut incr = IncrementalMarkdown::default();
    incr.append("line1\n\nline2\n\nline3");
    let lines = incr.lines(test_render_key(), &test_render);
    // test_render maps each source line to one output line
    assert_eq!(lines.len(), 5);
}

#[test]
fn incr_ensure_rendered_preserves_text() {
    let mut incr = IncrementalMarkdown::default();
    incr.append("p1\n\np2\n\ntail");
    incr.ensure_rendered(test_render_key(), &test_render);
    assert_eq!(incr.full_text(), "p1\n\np2\n\ntail");
}

#[test]
fn incr_invalidate_renders_preserves_text() {
    let mut incr = IncrementalMarkdown::default();
    incr.append("p1\n\np2\n\ntail");
    incr.invalidate_renders();
    assert_eq!(incr.full_text(), "p1\n\np2\n\ntail");
}

#[test]
fn incr_reuses_rendered_prefix_chunks() {
    use std::cell::Cell;

    let calls = Cell::new(0usize);
    let render = |src: &str| -> Vec<Line<'static>> {
        calls.set(calls.get() + 1);
        test_render(src)
    };

    let mut incr = IncrementalMarkdown::default();
    incr.append("p1\n\np2");
    let _ = incr.lines(test_render_key(), &render);
    assert_eq!(calls.get(), 2);

    incr.append(" tail");
    let _ = incr.lines(test_render_key(), &render);
    assert_eq!(calls.get(), 3);
}

#[test]
fn incr_does_not_split_inside_fenced_code_blocks() {
    let calls = std::cell::Cell::new(0usize);
    let render = |src: &str| -> Vec<Line<'static>> {
        calls.set(calls.get() + 1);
        test_render(src)
    };

    let mut incr = IncrementalMarkdown::default();
    incr.append("```rust\nfn main() {\n\nprintln!(\"hi\");\n}\n```\n\nafter");
    let _ = incr.lines(test_render_key(), &render);

    assert_eq!(calls.get(), 2);
}

#[test]
fn incr_streaming_simulation() {
    // Simulate a realistic streaming scenario
    let mut incr = IncrementalMarkdown::default();
    let chunks = ["Here is ", "some text.\n", "\nNext para", "graph here.\n\n", "Final."];
    for chunk in chunks {
        incr.append(chunk);
    }
    assert_eq!(incr.full_text(), "Here is some text.\n\nNext paragraph here.\n\nFinal.");
}

fn focus_test_app_with_available_targets() -> App {
    let mut app = make_test_app();
    app.turn.pending_interaction_ids.push("perm-1".into());
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
    app
}

#[test]
fn focus_owner_respects_target_priority_and_release_order() {
    let mut app = focus_test_app_with_available_targets();

    assert_eq!(app.focus_owner(), FocusOwner::Input);

    app.claim_focus_target(FocusTarget::Permission);
    assert_eq!(app.focus_owner(), FocusOwner::Permission);

    app.claim_focus_target(FocusTarget::Mention);
    assert_eq!(app.focus_owner(), FocusOwner::Mention);

    app.release_focus_target(FocusTarget::Mention);
    assert_eq!(app.focus_owner(), FocusOwner::Permission);

    app.release_focus_target(FocusTarget::Permission);
    assert_eq!(app.focus_owner(), FocusOwner::Input);
}

#[test]
fn focus_owner_falls_back_to_input_when_claimed_target_is_unavailable() {
    let mut app = make_test_app();
    app.claim_focus_target(FocusTarget::Permission);
    assert_eq!(app.focus_owner(), FocusOwner::Input);
}

// --- InvalidationLevel tests ---
