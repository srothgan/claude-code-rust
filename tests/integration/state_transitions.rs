// SPDX-License-Identifier: Apache-2.0

use claude_code_rust::agent::model;
use claude_code_rust::app::AppStatus;
use pretty_assertions::assert_eq;

use crate::helpers::{send_client_event, session_update, test_app, turn_complete};

#[tokio::test]
async fn observed_activity_owns_phase_without_writing_transcript() {
    let mut app = test_app();
    send_client_event(
        &mut app,
        session_update(model::SessionUpdate::RuntimeSessionStateUpdate(
            model::RuntimeSessionState::Running,
        )),
    );
    assert_eq!(app.status, AppStatus::Running);
    send_client_event(
        &mut app,
        session_update(model::SessionUpdate::AgentActivityUpdate(
            model::AgentActivityPhase::Thinking,
        )),
    );
    assert_eq!(app.status, AppStatus::Thinking);
    assert!(app.transcript.messages.is_empty());
    send_client_event(
        &mut app,
        session_update(model::SessionUpdate::RuntimeSessionStateUpdate(
            model::RuntimeSessionState::Running,
        )),
    );
    let chunk = model::ContentChunk::new(model::ContentBlock::Text(model::TextContent::new(
        "Here is my answer.",
    )));
    send_client_event(&mut app, session_update(model::SessionUpdate::AgentMessageChunk(chunk)));
    assert_eq!(app.status, AppStatus::Thinking, "text and liveness cannot interpret the SDK phase");
    send_client_event(
        &mut app,
        session_update(model::SessionUpdate::AgentActivityUpdate(
            model::AgentActivityPhase::Working,
        )),
    );
    assert_eq!(app.status, AppStatus::Running);
    send_client_event(&mut app, turn_complete());
    assert_eq!(app.status, AppStatus::Ready);
    send_client_event(
        &mut app,
        session_update(model::SessionUpdate::AgentActivityUpdate(
            model::AgentActivityPhase::Thinking,
        )),
    );
    assert_eq!(app.status, AppStatus::Ready, "late phase cannot restart a completed turn");
}
