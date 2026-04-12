// =====
// TESTS: 18
// =====
//
// Tool call lifecycle integration tests.
// Validates the full create -> update -> complete flow for tool calls.

use claude_code_rust::agent::events::ClientEvent;
use claude_code_rust::agent::model;
use claude_code_rust::app::{App, AppStatus, MessageBlock, ToolCallInfo};
use pretty_assertions::assert_eq;

use crate::helpers::{send_client_event, test_app};

fn task_meta() -> serde_json::Map<String, serde_json::Value> {
    let mut meta = serde_json::Map::new();
    meta.insert("claudeCode".into(), serde_json::json!({"toolName": "Task"}));
    meta
}

#[allow(clippy::expect_used)]
fn tool_call_block<'a>(app: &'a App, id: &str) -> &'a ToolCallInfo {
    let (message_index, block_index) = app.tool_call_index[id];
    app.messages
        .get(message_index)
        .and_then(|message| message.blocks.get(block_index))
        .and_then(|block| match block {
            MessageBlock::ToolCall(tool_call) => Some(tool_call.as_ref()),
            _ => None,
        })
        .expect("expected ToolCall block")
}

// --- ToolCallUpdate lifecycle ---

#[tokio::test]
async fn tool_call_updates_apply_terminal_statuses_and_title_fields() {
    let mut app = test_app();
    app.status = AppStatus::Running;

    let tc = model::ToolCall::new("tc-update", "Read file")
        .kind(model::ToolKind::Read)
        .status(model::ToolCallStatus::InProgress);
    send_client_event(&mut app, ClientEvent::SessionUpdate(model::SessionUpdate::ToolCall(tc)));

    let fields = model::ToolCallUpdateFields::new()
        .title("Read src/lib.rs".to_owned())
        .status(model::ToolCallStatus::Completed);
    let update = model::ToolCallUpdate::new("tc-update", fields);
    send_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::ToolCallUpdate(update)),
    );

    let tc = model::ToolCall::new("tc-fail", "Write file")
        .kind(model::ToolKind::Edit)
        .status(model::ToolCallStatus::InProgress);
    send_client_event(&mut app, ClientEvent::SessionUpdate(model::SessionUpdate::ToolCall(tc)));

    let fields = model::ToolCallUpdateFields::new().status(model::ToolCallStatus::Failed);
    let update = model::ToolCallUpdate::new("tc-fail", fields);
    send_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::ToolCallUpdate(update)),
    );

    let updated = tool_call_block(&app, "tc-update");
    assert_eq!(updated.title, "Read src/lib.rs");
    assert!(matches!(updated.status, model::ToolCallStatus::Completed));

    let failed = tool_call_block(&app, "tc-fail");
    assert!(matches!(failed.status, model::ToolCallStatus::Failed));
}

// --- All tools terminal -> Thinking ---

#[tokio::test]
async fn terminal_tool_statuses_transition_running_to_thinking_once_all_calls_finish() {
    let mut app = test_app();
    app.status = AppStatus::Running;

    let tc1 = model::ToolCall::new("tc-a", "Read A").status(model::ToolCallStatus::InProgress);
    let tc2 = model::ToolCall::new("tc-b", "Read B").status(model::ToolCallStatus::InProgress);
    send_client_event(&mut app, ClientEvent::SessionUpdate(model::SessionUpdate::ToolCall(tc1)));
    send_client_event(&mut app, ClientEvent::SessionUpdate(model::SessionUpdate::ToolCall(tc2)));

    assert!(matches!(app.status, AppStatus::Running));

    let fields = model::ToolCallUpdateFields::new().status(model::ToolCallStatus::Completed);
    send_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::ToolCallUpdate(
            model::ToolCallUpdate::new("tc-a", fields),
        )),
    );
    assert!(matches!(app.status, AppStatus::Running), "one still in progress");

    let fields = model::ToolCallUpdateFields::new().status(model::ToolCallStatus::Completed);
    send_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::ToolCallUpdate(
            model::ToolCallUpdate::new("tc-b", fields),
        )),
    );
    assert!(matches!(app.status, AppStatus::Thinking), "all-complete should resume thinking");

    let mut mixed_app = test_app();
    mixed_app.status = AppStatus::Running;

    let tc1 = model::ToolCall::new("tc-x", "Op 1").status(model::ToolCallStatus::InProgress);
    let tc2 = model::ToolCall::new("tc-y", "Op 2").status(model::ToolCallStatus::InProgress);
    send_client_event(
        &mut mixed_app,
        ClientEvent::SessionUpdate(model::SessionUpdate::ToolCall(tc1)),
    );
    send_client_event(
        &mut mixed_app,
        ClientEvent::SessionUpdate(model::SessionUpdate::ToolCall(tc2)),
    );

    let f1 = model::ToolCallUpdateFields::new().status(model::ToolCallStatus::Completed);
    let f2 = model::ToolCallUpdateFields::new().status(model::ToolCallStatus::Failed);
    send_client_event(
        &mut mixed_app,
        ClientEvent::SessionUpdate(model::SessionUpdate::ToolCallUpdate(
            model::ToolCallUpdate::new("tc-x", f1),
        )),
    );
    send_client_event(
        &mut mixed_app,
        ClientEvent::SessionUpdate(model::SessionUpdate::ToolCallUpdate(
            model::ToolCallUpdate::new("tc-y", f2),
        )),
    );

    assert!(
        matches!(mixed_app.status, AppStatus::Thinking),
        "mixed terminal outcomes should also resume thinking"
    );
}

// --- Task tool call tracking ---

#[tokio::test]
async fn task_tool_calls_leave_active_set_only_on_terminal_statuses() {
    let mut app = test_app();

    let tc = model::ToolCall::new("task-pend", "Running subtask")
        .kind(model::ToolKind::Think)
        .status(model::ToolCallStatus::InProgress)
        .meta(task_meta());
    send_client_event(&mut app, ClientEvent::SessionUpdate(model::SessionUpdate::ToolCall(tc)));
    assert!(app.active_task_ids.contains("task-pend"), "new Task should be tracked");

    let fields = model::ToolCallUpdateFields::new().status(model::ToolCallStatus::Pending);
    send_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::ToolCallUpdate(
            model::ToolCallUpdate::new("task-pend", fields),
        )),
    );
    assert!(app.active_task_ids.contains("task-pend"), "Pending should stay active");

    let fields = model::ToolCallUpdateFields::new().status(model::ToolCallStatus::Completed);
    send_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::ToolCallUpdate(
            model::ToolCallUpdate::new("task-pend", fields),
        )),
    );
    assert!(!app.active_task_ids.contains("task-pend"), "completed Task should be removed");

    let tc = model::ToolCall::new("task-fail", "Subtask")
        .kind(model::ToolKind::Think)
        .status(model::ToolCallStatus::InProgress)
        .meta(task_meta());
    send_client_event(&mut app, ClientEvent::SessionUpdate(model::SessionUpdate::ToolCall(tc)));
    assert!(app.active_task_ids.contains("task-fail"));

    let fields = model::ToolCallUpdateFields::new().status(model::ToolCallStatus::Failed);
    send_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::ToolCallUpdate(
            model::ToolCallUpdate::new("task-fail", fields),
        )),
    );
    assert!(!app.active_task_ids.contains("task-fail"), "failed Task should also be removed");
}

// --- Collapsed tool calls ---

#[tokio::test]
async fn session_collapse_preference_stays_stable_across_tool_call_lifecycle() {
    let mut app = test_app();
    app.tools_collapsed = true;

    let tc = model::ToolCall::new("tc-col", "Read file").status(model::ToolCallStatus::InProgress);
    send_client_event(&mut app, ClientEvent::SessionUpdate(model::SessionUpdate::ToolCall(tc)));
    assert!(app.tools_collapsed, "session preference should remain collapsed");
    assert!(matches!(tool_call_block(&app, "tc-col").status, model::ToolCallStatus::InProgress));

    let fields = model::ToolCallUpdateFields::new().status(model::ToolCallStatus::InProgress);
    send_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::ToolCallUpdate(
            model::ToolCallUpdate::new("tc-col", fields),
        )),
    );
    assert!(app.tools_collapsed, "in-progress updates should not flip the preference");
    assert!(matches!(tool_call_block(&app, "tc-col").status, model::ToolCallStatus::InProgress));

    let fields = model::ToolCallUpdateFields::new().status(model::ToolCallStatus::Completed);
    send_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::ToolCallUpdate(
            model::ToolCallUpdate::new("tc-col", fields),
        )),
    );
    assert!(app.tools_collapsed, "completed updates should keep the preference");
    assert!(matches!(tool_call_block(&app, "tc-col").status, model::ToolCallStatus::Completed));

    let mut expanded_app = test_app();
    expanded_app.tools_collapsed = false;

    let tc = model::ToolCall::new("tc-exp", "Write file").status(model::ToolCallStatus::InProgress);
    send_client_event(
        &mut expanded_app,
        ClientEvent::SessionUpdate(model::SessionUpdate::ToolCall(tc)),
    );
    assert!(!expanded_app.tools_collapsed, "expanded preference should remain expanded");

    let fields = model::ToolCallUpdateFields::new().status(model::ToolCallStatus::Completed);
    send_client_event(
        &mut expanded_app,
        ClientEvent::SessionUpdate(model::SessionUpdate::ToolCallUpdate(
            model::ToolCallUpdate::new("tc-exp", fields),
        )),
    );
    assert!(!expanded_app.tools_collapsed);
    assert!(matches!(
        tool_call_block(&expanded_app, "tc-exp").status,
        model::ToolCallStatus::Completed
    ));
}

// --- Multiple tool calls indexed correctly ---

#[tokio::test]
async fn multiple_tool_calls_independently_indexed() {
    let mut app = test_app();

    for i in 0..5 {
        let tc = model::ToolCall::new(format!("tc-{i}"), format!("Tool {i}"))
            .status(model::ToolCallStatus::InProgress);
        send_client_event(&mut app, ClientEvent::SessionUpdate(model::SessionUpdate::ToolCall(tc)));
    }

    assert_eq!(app.tool_call_index.len(), 5);
    for i in 0..5 {
        let key = format!("tc-{i}");
        assert!(app.tool_call_index.contains_key(&key), "missing {key}");
    }
}

// --- Edge cases: tool call update propagation ---

#[tokio::test]
async fn tool_call_update_via_meta_sets_sdk_tool_name() {
    let mut app = test_app();

    let tc = model::ToolCall::new("tc-meta", "Some tool").status(model::ToolCallStatus::InProgress);
    send_client_event(&mut app, ClientEvent::SessionUpdate(model::SessionUpdate::ToolCall(tc)));

    // Update arrives with meta setting sdk_tool_name
    let mut meta = serde_json::Map::new();
    meta.insert("claudeCode".into(), serde_json::json!({"toolName": "WebSearch"}));
    let fields = model::ToolCallUpdateFields::new();
    let update = model::ToolCallUpdate::new("tc-meta", fields).meta(meta);
    send_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::ToolCallUpdate(update)),
    );

    let (mi, bi) = app.tool_call_index["tc-meta"];
    if let MessageBlock::ToolCall(tc) = &app.messages[mi].blocks[bi] {
        assert_eq!(tc.sdk_tool_name, "WebSearch");
    } else {
        panic!("expected ToolCall block");
    }
}

#[tokio::test]
async fn todowrite_via_update_raw_input_parses_todos() {
    let mut app = test_app();

    // Create a tool call, initially without TodoWrite meta
    let tc =
        model::ToolCall::new("tc-todo-up", "TodoWrite").status(model::ToolCallStatus::InProgress);
    send_client_event(&mut app, ClientEvent::SessionUpdate(model::SessionUpdate::ToolCall(tc)));

    // Update sets sdk_tool_name + raw_input with todos
    let mut meta = serde_json::Map::new();
    meta.insert("claudeCode".into(), serde_json::json!({"toolName": "TodoWrite"}));
    let raw = serde_json::json!({"todos": [
        {"content": "Step 1", "status": "pending", "activeForm": "Doing step 1"}
    ]});
    let fields = model::ToolCallUpdateFields::new().raw_input(raw);
    let update = model::ToolCallUpdate::new("tc-todo-up", fields).meta(meta);
    send_client_event(
        &mut app,
        ClientEvent::SessionUpdate(model::SessionUpdate::ToolCallUpdate(update)),
    );

    assert_eq!(app.todos.len(), 1);
    assert_eq!(app.todos[0].content, "Step 1");
}

#[tokio::test]
async fn title_shortened_relative_to_cwd() {
    let mut app = test_app();
    app.cwd_raw = "/home/user/project".into();

    let tc = model::ToolCall::new("tc-shorten", "Read /home/user/project/src/main.rs")
        .status(model::ToolCallStatus::InProgress);
    send_client_event(&mut app, ClientEvent::SessionUpdate(model::SessionUpdate::ToolCall(tc)));

    let (mi, bi) = app.tool_call_index["tc-shorten"];
    if let MessageBlock::ToolCall(tc) = &app.messages[mi].blocks[bi] {
        assert_eq!(tc.title, "Read src/main.rs", "absolute path shortened to relative");
    } else {
        panic!("expected ToolCall block");
    }
}
