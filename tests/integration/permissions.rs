// SPDX-License-Identifier: Apache-2.0
// Permission grant/deny flow integration tests.
// Validates that PermissionRequest events are correctly attached to tool calls,
// that the pending_interaction_ids queue is maintained, and that responses
// are sent through the oneshot channel.

use claude_code_rust::agent::model;
use claude_code_rust::app::{AppStatus, MessageBlock};
use pretty_assertions::assert_eq;
use tokio::sync::oneshot;

use crate::helpers::{
    permission_request, send_client_event, session_update, test_app, turn_complete,
};

/// Helper: create a tool call, send it, then send a permission request for it.
/// Returns the oneshot receiver so the test can verify the response.
fn setup_permission(
    app: &mut claude_code_rust::app::App,
    tool_id: &str,
    options: Vec<model::PermissionOption>,
) -> oneshot::Receiver<model::RequestPermissionResponse> {
    // First create the tool call so it exists in the index
    let id = tool_id.to_owned();
    let tc = model::ToolCall::new(id, "Write file").status(model::ToolCallStatus::InProgress);
    send_client_event(app, session_update(model::SessionUpdate::ToolCall(tc)));

    let (response_tx, response_rx) = oneshot::channel();
    let tool_call_update =
        model::ToolCallUpdate::new(tool_id.to_owned(), model::ToolCallUpdateFields::new());
    let request =
        model::RequestPermissionRequest::new("test-session", tool_call_update, options, None);
    send_client_event(app, permission_request(request, response_tx));
    response_rx
}

fn allow_deny_options() -> Vec<model::PermissionOption> {
    vec![
        model::PermissionOption::new("allow", "Allow", model::PermissionOptionKind::AllowOnce),
        model::PermissionOption::new("deny", "Deny", model::PermissionOptionKind::RejectOnce),
    ]
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

// --- Permission for unknown tool call auto-rejects ---

#[tokio::test]
async fn permission_for_unknown_tool_call_auto_rejects() {
    let mut app = test_app();

    let (response_tx, mut response_rx) = oneshot::channel();
    let tool_call_update =
        model::ToolCallUpdate::new("nonexistent", model::ToolCallUpdateFields::new());
    let options = allow_deny_options();
    let request =
        model::RequestPermissionRequest::new("test-session", tool_call_update, options, None);
    send_client_event(&mut app, permission_request(request, response_tx));

    // Should NOT be in pending queue
    assert!(app.pending_interaction_ids.is_empty());

    // The response should have been sent (auto-reject with last option = "deny")
    let response = response_rx.try_recv();
    assert!(response.is_ok(), "auto-reject should send response immediately");
    let resp = response.unwrap();
    if let model::RequestPermissionOutcome::Selected(selected) = resp.outcome {
        assert_eq!(selected.option_id.clone(), "deny", "auto-reject should pick last option");
    } else {
        panic!("expected Selected outcome from auto-reject");
    }
}

#[tokio::test]
async fn duplicate_permission_preserves_original_and_neighbor_response_ownership() {
    use claude_code_rust::app::handle_terminal_event;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    let options = |prefix: &str| {
        vec![
            model::PermissionOption::new(
                format!("{prefix}-allow"),
                format!("{prefix} allow"),
                model::PermissionOptionKind::AllowOnce,
            ),
            model::PermissionOption::new(
                format!("{prefix}-deny"),
                format!("{prefix} deny"),
                model::PermissionOptionKind::RejectOnce,
            ),
        ]
    };
    let mut app = test_app();
    let mut original = setup_permission(&mut app, "original", options("original"));
    let mut neighbor = setup_permission(&mut app, "neighbor", options("neighbor"));
    let (response_tx, mut duplicate) = oneshot::channel();
    let request = model::RequestPermissionRequest::new(
        "test-session",
        model::ToolCallUpdate::new("original", model::ToolCallUpdateFields::new()),
        options("duplicate"),
        Some(model::PermissionDisplay::new().title(Some("Duplicate metadata".into()))),
    );
    send_client_event(&mut app, permission_request(request, response_tx));
    assert_eq!(
        duplicate.try_recv().expect("duplicate rejection").outcome,
        model::RequestPermissionOutcome::Selected(model::SelectedPermissionOutcome::new(
            "duplicate-deny"
        ))
    );
    assert!(matches!(original.try_recv(), Err(oneshot::error::TryRecvError::Empty)));
    assert!(matches!(neighbor.try_recv(), Err(oneshot::error::TryRecvError::Empty)));
    let (mi, bi) = app.lookup_tool_call("original").expect("original");
    let MessageBlock::ToolCall(tool) = &app.transcript.messages[mi].blocks[bi] else {
        panic!("tool");
    };
    let pending = tool.pending_permission.as_ref().expect("original permission");
    assert_eq!(pending.options, options("original"));
    assert!(pending.display.is_none(), "duplicate metadata must not replace the original");
    for key in [KeyCode::Right, KeyCode::Enter] {
        handle_terminal_event(&mut app, Event::Key(KeyEvent::new(key, KeyModifiers::NONE)));
    }
    assert_eq!(
        original.try_recv().expect("original response").outcome,
        model::RequestPermissionOutcome::Selected(model::SelectedPermissionOutcome::new(
            "original-deny"
        ))
    );
    assert!(matches!(neighbor.try_recv(), Err(oneshot::error::TryRecvError::Empty)));
    handle_terminal_event(&mut app, Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
    assert_eq!(
        neighbor.try_recv().expect("neighbor response").outcome,
        model::RequestPermissionOutcome::Selected(model::SelectedPermissionOutcome::new(
            "neighbor-allow"
        ))
    );
}

// --- Confirmed session inventory across completed turns ---

#[tokio::test]
async fn confirmed_task_and_mode_inventory_survives_completion_and_accepts_next_turn_updates() {
    use claude_code_rust::app::{ModeInfo, ModeState};
    let mut app = test_app();
    let original = vec![
        task_item("alpha", "Alpha confirmed", model::TaskStatus::Pending),
        task_item("beta", "Beta confirmed", model::TaskStatus::InProgress),
    ];
    let mode = ModeState {
        current_mode_id: "plan".into(),
        current_mode_name: "Confirmed planning".into(),
        available_modes: vec![
            ModeInfo { id: "plan".into(), name: "Confirmed planning".into() },
            ModeInfo { id: "acceptEdits".into(), name: "Editing choice".into() },
        ],
    };
    send_client_event(
        &mut app,
        session_update(model::SessionUpdate::TaskStateUpdate(
            model::TaskStateUpdate::new(model::TaskUpdateSource::List)
                .tasks(original.clone())
                .complete_snapshot(true),
        )),
    );
    send_client_event(
        &mut app,
        session_update(model::SessionUpdate::ModeStateUpdate(mode.clone())),
    );
    send_client_event(&mut app, session_update(model::SessionUpdate::AgentResponseStarted));
    send_client_event(
        &mut app,
        session_update(model::SessionUpdate::AgentMessageChunk(model::ContentChunk::new(
            model::ContentBlock::Text(model::TextContent::new("First turn answer")),
        ))),
    );
    assert_ne!(app.status, AppStatus::Ready);
    send_client_event(&mut app, turn_complete());
    assert_eq!(app.status, AppStatus::Ready);
    assert_eq!(app.sdk_inventory.tasks, original);
    assert_mode(&app, &mode);
    send_client_event(&mut app, session_update(model::SessionUpdate::AgentResponseStarted));
    let changed = task_item("alpha", "Alpha completed", model::TaskStatus::Completed);
    send_client_event(
        &mut app,
        session_update(model::SessionUpdate::TaskStateUpdate(
            model::TaskStateUpdate::new(model::TaskUpdateSource::Update)
                .tasks(vec![changed.clone()]),
        )),
    );
    let replacement = ModeState {
        current_mode_id: "acceptEdits".into(),
        current_mode_name: "Confirmed editing".into(),
        available_modes: vec![ModeInfo {
            id: "acceptEdits".into(),
            name: "Confirmed editing".into(),
        }],
    };
    send_client_event(
        &mut app,
        session_update(model::SessionUpdate::ModeStateUpdate(replacement.clone())),
    );
    assert_eq!(app.sdk_inventory.tasks, vec![changed, original[1].clone()]);
    assert_mode(&app, &replacement);
    send_client_event(&mut app, turn_complete());
    assert_mode(&app, &replacement);
    assert_eq!(app.status, AppStatus::Ready);
}

fn assert_mode(app: &claude_code_rust::app::App, expected: &claude_code_rust::app::ModeState) {
    assert_eq!(
        app.session_runtime.mode.as_ref().map(|mode| (
            &mode.current_mode_id,
            &mode.current_mode_name,
            mode.available_modes.iter().map(|m| (&m.id, &m.name)).collect::<Vec<_>>()
        )),
        Some((
            &expected.current_mode_id,
            &expected.current_mode_name,
            expected.available_modes.iter().map(|m| (&m.id, &m.name)).collect::<Vec<_>>()
        ))
    );
}
