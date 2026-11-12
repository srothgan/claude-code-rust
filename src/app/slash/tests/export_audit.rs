// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::agent::{
    client::CommandReceiver,
    events::{ClientEvent, HistoryChunk},
    wire::BridgeCommand,
};

fn submit(app: &mut App, text: &str) {
    app.input.set_text(text);
    crate::app::input_submit::submit_input(app);
}

fn fixture(dir: &std::path::Path) -> (App, CommandReceiver) {
    let (mut app, commands) = crate::app::test_support::app_with_connection();
    app.cwd_raw = dir.display().to_string();
    app.settings_home_override = Some(dir.to_path_buf());
    app.session_runtime.session_id = Some("export-session".into());
    (app, commands)
}

async fn request(commands: &mut CommandReceiver) -> String {
    let envelope =
        tokio::time::timeout(std::time::Duration::from_secs(2), commands.recv_envelope())
            .await
            .expect("export dispatch deadline")
            .expect("export command");
    assert_eq!(
        envelope.command,
        BridgeCommand::ExportSessionHistory { session_id: "export-session".into() }
    );
    let id = envelope.request_id.expect("export request identity");
    assert!(!id.is_empty());
    id
}

fn no_command(commands: &mut CommandReceiver) {
    assert!(matches!(commands.try_recv(), Err(tokio::sync::mpsc::error::TryRecvError::Empty)));
}

fn history(session: &str, request: &str, text: Option<&str>, error: Option<&str>) -> ClientEvent {
    ClientEvent::SessionHistoryChunk {
        session_id: session.into(),
        chunk: HistoryChunk {
            request_id: Some(request.into()),
            updates: text
                .map(|text| {
                    vec![model::SessionUpdate::UserMessageChunk(model::ContentChunk::new(
                        model::ContentBlock::Text(model::TextContent::new(text)),
                    ))]
                })
                .unwrap_or_default(),
            last: true,
            omitted_updates: 0,
            unmapped_updates: 0,
            error: error.map(str::to_owned),
        },
    }
}

#[test]
fn disconnected_export_reports_the_cause_and_creates_no_output() {
    let home = tempfile::tempdir().expect("isolated cwd");
    let mut app = App::test_default();
    app.cwd_raw = home.path().display().to_string();
    submit(&mut app, "/export");
    assert_eq!(app.status, AppStatus::Ready);
    assert!(app.history_export.is_none());
    let message = app.transcript.messages.last().expect("connection feedback");
    assert_eq!(message.role, MessageRole::System(Some(SystemSeverity::Error)));
    assert!(message.blocks.iter().any(|b| matches!(b, MessageBlock::Text(t) if t.text.contains("export") && t.text.contains("not connected"))));
    assert!(std::fs::read_dir(home.path()).expect("output directory").next().is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn export_rejects_mismatched_chunks_blocks_submission_and_writes_only_its_history() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().expect("isolated cwd");
            let (mut app, mut commands) = fixture(home.path());
            submit(&mut app, "/export notes for review.md");
            let request_id = request(&mut commands).await;
            assert_eq!(app.status, AppStatus::CommandPending);
            submit(&mut app, "draft held during export");
            assert_eq!(app.input.text(), "draft held during export");
            no_command(&mut commands);
            for (session, id) in
                [("foreign-session", request_id.as_str()), ("export-session", "foreign-request")]
            {
                crate::app::handle_client_event(
                    &mut app,
                    history(session, id, Some("foreign history"), None),
                );
                assert_eq!(app.status, AppStatus::CommandPending);
                assert!(app.history_export.is_some());
                assert!(!home.path().join("notes for review.md").exists());
                no_command(&mut commands);
            }
            crate::app::handle_client_event(
                &mut app,
                history("export-session", &request_id, Some("chosen saved history"), None),
            );
            assert_eq!(app.status, AppStatus::Ready);
            assert!(app.history_export.is_none());
            let output = std::fs::read_to_string(home.path().join("notes for review.md"))
                .expect("exported file at exact spaced path");
            assert!(output.contains("chosen saved history"));
            assert!(
                !output.contains("foreign history") && !output.contains("draft held during export")
            );
            assert_eq!(app.input.text(), "draft held during export");
            // Reuse the public submission path after successful completion.
            submit(&mut app, "/export second.md");
            let second = request(&mut commands).await;
            assert_ne!(second, request_id);
            crate::app::handle_client_event(
                &mut app,
                history("export-session", &second, None, None),
            );
            assert_eq!(app.status, AppStatus::Ready);
            assert!(home.path().join("second.md").is_file());
            no_command(&mut commands);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn only_the_matching_export_error_releases_input_and_failed_exports_can_be_retried() {
    tokio::task::LocalSet::new().run_until(async {
        let home = tempfile::tempdir().expect("isolated cwd");
        let (mut app, mut commands) = fixture(home.path());
        submit(&mut app, "/export out.md");
        let id = request(&mut commands).await;
        app.config.pending_session_title_change = Some(crate::app::config::PendingSessionTitleChangeState {
            session_id: "export-session".into(), kind: crate::app::config::PendingSessionTitleChangeKind::Generate,
        });
        crate::app::handle_client_event(&mut app, ClientEvent::SlashCommandError {
            session_id: Some("export-session".into()), message: "title generation failed".into(),
        });
        for event in [history("foreign-session", &id, None, Some("foreign session error")), history("export-session", "wrong-request", None, Some("foreign request error"))] {
            crate::app::handle_client_event(&mut app, event);
            assert!(app.history_export.is_some());
            assert_eq!(app.status, AppStatus::CommandPending);
            assert!(!home.path().join("out.md").exists());
        }
        crate::app::handle_client_event(&mut app, history("export-session", &id, None, Some("saved-history read denied")));
        assert!(app.history_export.is_none());
        assert_eq!(app.status, AppStatus::Ready);
        assert!(!home.path().join("out.md").exists());
        assert!(app.transcript.messages.iter().flat_map(|m| &m.blocks).any(|b| matches!(b, MessageBlock::Notice(n) if n.severity == SystemSeverity::Error && n.text.text.contains("saved-history read denied"))));
        submit(&mut app, "/export retry.md");
        let retry = request(&mut commands).await;
        assert_ne!(retry, id);
        crate::app::handle_client_event(&mut app, history("export-session", &retry, Some("retried saved history"), None));
        assert_eq!(app.status, AppStatus::Ready);
        assert!(home.path().join("retry.md").is_file());
        no_command(&mut commands);
    }).await;
}

#[tokio::test(flavor = "current_thread")]
async fn export_send_failure_is_correlated_and_applied_before_releasing_input() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let home = tempfile::tempdir().expect("isolated cwd");
            let (mut app, commands) = fixture(home.path());
            drop(commands);
            let (events, mut incoming) = tokio::sync::mpsc::channel(8);
            app.event_tx = events;
            submit(&mut app, "/export failed.md");
            assert_eq!(app.status, AppStatus::CommandPending);
            let event = tokio::time::timeout(std::time::Duration::from_secs(2), incoming.recv())
                .await
                .expect("export failure deadline")
                .expect("export failure event");
            let ClientEvent::SessionHistoryChunk { session_id, chunk } = &event else {
                panic!("request-scoped history failure")
            };
            assert_eq!(session_id, "export-session");
            let id = chunk.request_id.as_deref().expect("failed export request ID");
            assert!(!id.is_empty());
            assert!(chunk.last && chunk.error.as_ref().is_some_and(|e| e.contains("export")));
            crate::app::handle_client_event(
                &mut app,
                history("export-session", "unrelated-request", None, Some("unrelated")),
            );
            assert_eq!(app.status, AppStatus::CommandPending);
            assert!(app.history_export.is_some());
            crate::app::handle_client_event(&mut app, event);
            assert_eq!(app.status, AppStatus::Ready);
            assert!(app.history_export.is_none());
            assert!(std::fs::read_dir(home.path()).expect("output directory").next().is_none());
            assert!(app.composer_access().can_submit());
        })
        .await;
}
