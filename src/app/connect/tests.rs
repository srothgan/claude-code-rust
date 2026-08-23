// SPDX-License-Identifier: Apache-2.0
use super::type_converters::map_session_update;
use crate::Cli;
use crate::agent::model;
use crate::agent::types;
use crate::app::{FullscreenView, SurfaceMode, TerminalLifecycleState};
use std::cell::Cell;
use std::rc::Rc;

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
        command: None,
        no_update_check: true,
        dir: Some(dir.path().to_path_buf()),
        bridge_script: None,
        enable_logs: false,
        diagnostics_preset: None,
        log_file: None,
        log_filter: None,
        log_append: false,
        enable_perf: false,
        perf_log: None,
        perf_append: false,
    };

    let app = super::create_app(&cli);

    assert!(app.file_index.root.is_none());
    assert!(app.file_index.scan.is_none());
    assert!(app.file_index.watch.is_none());
    assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::Trusted));
    assert_eq!(app.terminal_lifecycle, TerminalLifecycleState::Bootstrapping);
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
