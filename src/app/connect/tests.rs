// SPDX-License-Identifier: Apache-2.0
use super::type_converters::map_session_update;
use crate::Cli;
use crate::agent::model;
use crate::agent::types;
use crate::app::{FullscreenView, SurfaceMode, TerminalLifecycleState};
use std::cell::Cell;
use std::rc::Rc;

#[tokio::test]
async fn subagent_stop_dispatch_preserves_request_and_run_identity() {
    let (connection, _commands) = crate::agent::client::AgentConnection::test_channel();
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let mut connected = true;
    super::event_dispatch::handle_bridge_event(
        &tx,
        &connection,
        &mut connected,
        false,
        crate::agent::wire::EventEnvelope {
            request_id: Some("stop-A".to_owned()),
            event: crate::agent::wire::BridgeEvent::SubagentStopResult {
                session_id: "session".to_owned(),
                task_id: "worker".to_owned(),
                run_id: Some("A".to_owned()),
                error: None,
            },
        },
    )
    .await;
    let crate::agent::events::ClientEvent::SubagentStopResult {
        session_id,
        task_id,
        run_id,
        request_id,
        error,
    } = rx.try_recv().expect("stop response")
    else {
        panic!("expected stop result")
    };
    assert_eq!(
        (session_id.as_str(), task_id.as_str(), run_id.as_deref(), request_id.as_deref()),
        ("session", "worker", Some("A"), Some("stop-A"))
    );
    assert!(error.is_none());
}

#[test]
fn task_transport_preserves_execution_ancestry_and_launch_provenance() {
    let transport: types::SessionUpdate = serde_json::from_value(serde_json::json!({
        "type": "task_state_update", "source": "task_lifecycle", "removed_task_ids": [], "is_complete_snapshot": false,
        "tasks": [{ "task_id": "worker", "run_id": "B", "parent_task_id": "parent", "source_tool_call_id": "launch-A", "source_tool_call_run_id": "A", "subject": "Worker", "status": "in_progress", "blocks": [], "blocked_by": [] }]
    })).expect("task wire payload");
    let Some(model::SessionUpdate::TaskStateUpdate(update)) = map_session_update(transport) else {
        panic!("expected task update")
    };
    let row = &update.tasks[0];
    assert_eq!(row.run_id.as_deref(), Some("B"));
    assert_eq!(row.parent_task_id.as_deref(), Some("parent"));
    assert_eq!(row.source_tool_call_id.as_deref(), Some("launch-A"));
    assert_eq!(row.source_tool_call_run_id.as_deref(), Some("A"));
}

#[test]
fn map_session_update_preserves_config_option_update() {
    let mapped = map_session_update(types::SessionUpdate::ConfigOptionUpdate {
        option_id: "model".to_owned(),
        value: serde_json::Value::String("sonnet".to_owned()),
    });

    let Some(model::SessionUpdate::ConfigOptionUpdate(cfg)) = mapped else {
        panic!("expected ConfigOptionUpdate mapping");
    };
    assert_eq!(cfg.option_id, "model");
    assert_eq!(cfg.value, serde_json::Value::String("sonnet".to_owned()));
}

#[test]
fn create_app_defers_file_index_and_routes_untrusted_cwd_to_trust_surface() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cli = Cli {
        prompt: None,
        resume: None,
        continue_session: false,
        session_options: crate::SessionOptions::default(),
        session_config: crate::SessionConfigArgs::default(),
        command: None,
        no_update_check: true,
        dir: Some(dir.path().to_path_buf()),
        bridge_script: None,
        enable_logs: false,
        diagnostics_preset: None,
        log_file: None,
        log_filter: None,
        log_append: false,
    };

    let app = super::create_app(&cli, crate::agent::wire::SessionConfig::default());

    assert!(app.file_index.root.is_none());
    assert!(app.file_index.scan.is_none());
    assert!(app.file_index.watch.is_none());
    assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::Trusted));
    assert_eq!(app.terminal_lifecycle, TerminalLifecycleState::Bootstrapping);
}

fn no_dot_components(path: &std::path::Path) -> bool {
    !path
        .components()
        .any(|part| matches!(part, std::path::Component::CurDir | std::path::Component::ParentDir))
}

#[test]
fn create_app_keeps_the_resolved_dir_for_session_state_and_display() {
    // Tests run from the package root, which has `src`; `src/..` names that root.
    let expected = std::env::current_dir().expect("process dir");
    let cli = Cli {
        prompt: None,
        resume: None,
        continue_session: false,
        session_options: crate::SessionOptions::default(),
        session_config: crate::SessionConfigArgs::default(),
        command: None,
        no_update_check: true,
        dir: Some(std::path::Path::new("src").join("..")),
        bridge_script: None,
        enable_logs: false,
        diagnostics_preset: None,
        log_file: None,
        log_filter: None,
        log_append: false,
    };

    let app = super::create_app(&cli, crate::agent::wire::SessionConfig::default());

    assert_eq!(std::path::Path::new(&app.cwd_raw), expected);
    let shown = std::path::Path::new(app.cwd.trim_start_matches('~'));
    let root_name = expected.file_name().expect("package root name");
    assert!(shown.ends_with(root_name) && no_dot_components(shown), "Loc: {}", app.cwd);
}

#[test]
fn app_client_event_queue_has_the_configured_capacity() {
    let mut app = crate::app::App::test_default();
    assert_eq!(app.event_tx.max_capacity(), super::CLIENT_EVENT_QUEUE_CAPACITY);

    for _ in 0..super::CLIENT_EVENT_QUEUE_CAPACITY {
        app.event_tx
            .try_send(crate::agent::events::ClientEvent::LogoutCompleted)
            .expect("event should fit within configured capacity");
    }
    assert!(matches!(
        app.event_tx.try_send(crate::agent::events::ClientEvent::LogoutCompleted),
        Err(tokio::sync::mpsc::error::TrySendError::Full(_))
    ));
    assert!(app.event_rx.try_recv().is_ok());
}

#[tokio::test(flavor = "current_thread")]
async fn shutdown_connection_signals_and_awaits_the_owned_bridge_task() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut app = crate::app::App::test_default();
            let completed = Rc::new(Cell::new(false));
            let completed_for_task = Rc::clone(&completed);
            let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
            let join_handle = tokio::task::spawn_local(async move {
                let _ = shutdown_rx.await;
                completed_for_task.set(true);
            });
            app.bridge_task = Some(super::BridgeTask { shutdown_tx, join_handle });

            super::shutdown_connection(&mut app).await;

            assert!(completed.get());
            assert!(app.bridge_task.is_none());
            assert!(app.session_runtime.conn.is_none());
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn forced_connection_shutdown_aborts_the_owned_bridge_task() {
    tokio::task::LocalSet::new()
        .run_until(async {
            struct DropMarker(Rc<Cell<bool>>);

            impl Drop for DropMarker {
                fn drop(&mut self) {
                    self.0.set(true);
                }
            }

            let mut app = crate::app::App::test_default();
            let dropped = Rc::new(Cell::new(false));
            let dropped_for_task = Rc::clone(&dropped);
            let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
            let join_handle = tokio::task::spawn_local(async move {
                let _marker = DropMarker(dropped_for_task);
                let _ = shutdown_rx.await;
                std::future::pending::<()>().await;
            });
            tokio::task::yield_now().await;
            app.bridge_task = Some(super::BridgeTask { shutdown_tx, join_handle });

            let shutdown = super::begin_shutdown_connection(&mut app).expect("shutdown handle");
            shutdown.force().await;

            assert!(dropped.get());
            assert!(app.bridge_task.is_none());
            assert!(app.session_runtime.conn.is_none());
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn history_export_chunks_count_the_updates_the_app_cannot_map() {
    let (connection, _commands) = crate::agent::client::AgentConnection::test_channel();
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(4);
    let mut connected_once = true;
    // An image of a type the app cannot show has no mapped form; the text next to it does.
    let envelope: crate::agent::wire::EventEnvelope = serde_json::from_value(serde_json::json!({
        "request_id": "export-1",
        "event": "session_history_export",
        "session_id": "s1",
        "updates": [
            { "type": "user_message_chunk",
              "content": { "type": "text", "text": "hello" } },
            { "type": "user_message_chunk",
              "content": { "type": "image", "mime_type": "image/x-unsupported", "data": "AAAA" } },
            { "type": "agent_message_chunk",
              "content": { "type": "image", "mime_type": "image/png", "data": "" } }
        ],
        "last": true,
        "omitted_updates": 0
    }))
    .expect("history export envelope");

    super::event_dispatch::handle_bridge_event(
        &event_tx,
        &connection,
        &mut connected_once,
        false,
        envelope,
    )
    .await;

    let crate::agent::events::ClientEvent::SessionHistoryChunk { chunk, .. } =
        event_rx.recv().await.expect("mapped event")
    else {
        panic!("expected a history chunk");
    };
    assert_eq!(chunk.updates.len(), 1);
    assert_eq!(chunk.unmapped_updates, 2);
    assert_eq!(chunk.request_id.as_deref(), Some("export-1"));
}
