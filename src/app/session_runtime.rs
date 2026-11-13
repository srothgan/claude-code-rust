// SPDX-License-Identifier: Apache-2.0

use crate::app::App;
use std::time::{Duration, Instant};

const CONTEXT_USAGE_REFRESH_INTERVAL: Duration = Duration::from_secs(5);

pub(crate) enum RuntimeReloadRequestOutcome {
    Requested,
    Unavailable,
    Failed,
}

pub(crate) fn request_runtime_reload(app: &mut App) -> RuntimeReloadRequestOutcome {
    request_plugin_folders(app, Vec::new(), false)
}

pub(crate) fn request_forced_runtime_reload(app: &mut App) -> RuntimeReloadRequestOutcome {
    if app.plugins.pending_reload.as_ref().is_some_and(|operation| {
        !operation.held
            || app
                .session_runtime
                .session_id
                .as_ref()
                .is_none_or(|id| id.to_string() != operation.session_id)
    }) {
        return RuntimeReloadRequestOutcome::Failed;
    }
    let folders = app
        .plugins
        .pending_reload
        .as_ref()
        .map(|operation| operation.folders.clone())
        .unwrap_or_default();
    request_plugin_folders(app, folders, true)
}

pub(crate) fn request_plugin_folders(
    app: &mut App,
    folders: Vec<String>,
    force: bool,
) -> RuntimeReloadRequestOutcome {
    let Some(conn) = app.session_runtime.conn.as_ref() else {
        return RuntimeReloadRequestOutcome::Unavailable;
    };
    let Some(ref sid) = app.session_runtime.session_id else {
        return RuntimeReloadRequestOutcome::Unavailable;
    };
    let session_id = sid.to_string();
    let request_id = uuid::Uuid::new_v4().to_string();
    match conn.reload_plugins(session_id.clone(), force, folders.clone(), Some(request_id.clone()))
    {
        Ok(()) => {
            app.plugins.pending_reload = Some(crate::app::plugins::PendingPluginReload {
                session_id: session_id.clone(),
                request_id,
                folders,
                held: false,
            });
            tracing::debug!(
                target: crate::logging::targets::APP_SESSION,
                event_name = "runtime_reload_requested",
                message = "session runtime plugin reload requested",
                outcome = "start",
                session_id = %session_id,
                force,
            );
            RuntimeReloadRequestOutcome::Requested
        }
        Err(error) => {
            tracing::warn!(
                target: crate::logging::targets::APP_SESSION,
                event_name = "runtime_reload_request_failed",
                message = "failed to request session runtime plugin reload",
                outcome = "failure",
                session_id = %session_id,
                force,
                error_message = %error,
            );
            RuntimeReloadRequestOutcome::Failed
        }
    }
}

pub(crate) fn request_context_usage_refresh(app: &mut App) {
    request_context_usage_refresh_at(app, Instant::now());
}

fn request_context_usage_refresh_at(app: &mut App, now: Instant) {
    if app.session_runtime.session_usage.context_usage_in_flight {
        app.session_runtime.session_usage.context_usage_last_requested_at = Some(now);
        app.session_runtime.session_usage.context_usage_refresh_pending = true;
        return;
    }

    let Some(conn) = app.session_runtime.conn.as_ref() else {
        clear_context_usage_refresh_state(app);
        return;
    };
    let Some(ref sid) = app.session_runtime.session_id else {
        clear_context_usage_refresh_state(app);
        return;
    };

    let session_id = sid.to_string();
    app.session_runtime.session_usage.context_usage_last_requested_at = Some(now);
    app.session_runtime.session_usage.context_usage_in_flight = true;
    app.session_runtime.session_usage.context_usage_refresh_pending = false;
    match conn.get_context_usage(session_id.clone()) {
        Ok(()) => tracing::debug!(
            target: crate::logging::targets::APP_SESSION,
            event_name = "context_usage_requested",
            message = "session context usage requested",
            outcome = "start",
            session_id = %session_id,
        ),
        Err(error) => {
            app.session_runtime.session_usage.context_usage_in_flight = false;
            tracing::warn!(
                target: crate::logging::targets::APP_SESSION,
                event_name = "context_usage_request_failed",
                message = "failed to request session context usage",
                outcome = "failure",
                session_id = %session_id,
                error_message = %error,
            );
        }
    }
}

pub(crate) fn tick_context_usage_refresh(app: &mut App, now: Instant) {
    if !context_usage_refresh_is_active(app) {
        return;
    }
    if app.session_runtime.conn.is_none() || app.session_runtime.session_id.is_none() {
        clear_context_usage_refresh_state(app);
        return;
    }
    if app.session_runtime.session_usage.context_usage_in_flight {
        return;
    }

    let refresh_due = app.session_runtime.session_usage.context_usage_last_requested_at.is_none_or(
        |last_requested| {
            now.saturating_duration_since(last_requested) >= CONTEXT_USAGE_REFRESH_INTERVAL
        },
    );
    if refresh_due {
        request_context_usage_refresh_at(app, now);
    }
}

fn context_usage_refresh_is_active(app: &App) -> bool {
    app.is_agent_turn_active()
}

pub(crate) fn request_status_snapshot_refresh(app: &mut App) {
    let Some(conn) = app.session_runtime.conn.as_ref() else {
        return;
    };
    let Some(ref sid) = app.session_runtime.session_id else {
        return;
    };

    let session_id = sid.to_string();
    match conn.get_status_snapshot(session_id.clone()) {
        Ok(()) => tracing::debug!(
            target: crate::logging::targets::APP_AUTH,
            event_name = "status_snapshot_requested",
            message = "session status snapshot requested",
            outcome = "start",
            session_id = %session_id,
        ),
        Err(error) => tracing::warn!(
            target: crate::logging::targets::APP_AUTH,
            event_name = "status_snapshot_request_failed",
            message = "failed to request session status snapshot",
            outcome = "failure",
            session_id = %session_id,
            error_message = %error,
        ),
    }
}

pub(crate) fn apply_context_usage_snapshot(app: &mut App, percentage: Option<u8>) {
    app.session_runtime.session_usage.context_usage_percent = percentage;
    app.session_runtime.session_usage.context_usage_in_flight = false;
    let refresh_pending =
        std::mem::take(&mut app.session_runtime.session_usage.context_usage_refresh_pending);
    if refresh_pending {
        request_context_usage_refresh(app);
    }
}

fn clear_context_usage_refresh_state(app: &mut App) {
    app.session_runtime.session_usage.context_usage_in_flight = false;
    app.session_runtime.session_usage.context_usage_refresh_pending = false;
    app.session_runtime.session_usage.context_usage_last_requested_at = None;
}

#[cfg(test)]
mod tests {
    use super::{
        RuntimeReloadRequestOutcome, request_context_usage_refresh,
        request_context_usage_refresh_at, request_forced_runtime_reload, request_runtime_reload,
        tick_context_usage_refresh,
    };
    use crate::agent::wire::BridgeCommand;
    use crate::app::{App, AppStatus};
    use std::time::{Duration, Instant};

    use crate::app::test_support::app_with_connection;

    fn expect_context_usage_command(rx: &mut crate::agent::client::CommandReceiver) {
        let envelope = rx.try_recv().expect("context usage command");
        assert!(matches!(
            envelope.command,
            BridgeCommand::GetContextUsage { session_id } if session_id == "session-1"
        ));
    }

    #[test]
    fn runtime_reload_correlates_ordinary_and_held_requests_with_their_outcomes() {
        for folders in
            [vec![], vec!["fixtures/plugin alpha".to_owned(), "fixtures/plugin-beta".to_owned()]]
        {
            for force in [false, true] {
                assert_correlated_reload(&folders, force);
            }
        }
        for missing_connection in [false, true] {
            let (mut app, mut commands) = app_with_connection();
            if missing_connection {
                app.session_runtime.conn = None;
            } else {
                app.session_runtime.session_id = None;
            }
            assert!(matches!(
                request_runtime_reload(&mut app),
                RuntimeReloadRequestOutcome::Unavailable
            ));
            assert!(commands.try_recv().is_err());
            let (connection, mut usable) = crate::agent::client::AgentConnection::test_channel();
            app.session_runtime.conn = Some(std::rc::Rc::new(connection));
            app.session_runtime.session_id = Some("session-1".into());
            assert!(matches!(
                request_runtime_reload(&mut app),
                RuntimeReloadRequestOutcome::Requested
            ));
            let id = reload_request(&mut usable, false, &[]);
            complete_reload(&mut app, "session-1", &id);
            assert!(app.plugins.pending_reload.is_none());
        }
        let (mut app, commands) = app_with_connection();
        drop(commands);
        assert!(matches!(request_runtime_reload(&mut app), RuntimeReloadRequestOutcome::Failed));
        assert!(app.plugins.pending_reload.is_none());
    }

    #[test]
    fn context_refresh_coalesces_explicit_requests_without_replaying_ticks() {
        for explicit in [false, true] {
            let (mut app, mut commands) = app_with_connection();
            active_phase(&mut app, crate::agent::model::AgentActivityPhase::Thinking);
            context_result(&mut app, "session-1", Some(17));
            request_context_usage_refresh(&mut app);
            expect_context_usage_command(&mut commands);
            for _ in 0..3 {
                if explicit {
                    request_context_usage_refresh(&mut app);
                } else {
                    tick_context_usage_refresh(&mut app, Instant::now() + Duration::from_secs(20));
                }
            }
            assert!(commands.try_recv().is_err());
            context_result(&mut app, "old-session", Some(99));
            assert_eq!(app.session_runtime.session_usage.context_usage_percent, Some(17));
            assert!(commands.try_recv().is_err());
            // A stale snapshot must not release the outstanding request either.
            tick_context_usage_refresh(&mut app, Instant::now() + Duration::from_secs(20));
            assert!(commands.try_recv().is_err());
            context_result(&mut app, "session-1", Some(0));
            assert_eq!(app.session_runtime.session_usage.context_usage_percent, Some(0));
            if explicit {
                expect_context_usage_command(&mut commands);
                assert!(commands.try_recv().is_err());
                context_result(&mut app, "session-1", None);
                assert_eq!(app.session_runtime.session_usage.context_usage_percent, None);
            }
            assert!(commands.try_recv().is_err(), "one response must finish the coalesced work");
            request_context_usage_refresh(&mut app);
            expect_context_usage_command(&mut commands);
            context_result(&mut app, "session-1", Some(62));
            assert_eq!(app.session_runtime.session_usage.context_usage_percent, Some(62));
            assert!(commands.try_recv().is_err());
        }
    }

    #[test]
    fn context_refresh_obeys_active_phase_intervals_and_manual_reset() {
        for phase in [
            crate::agent::model::AgentActivityPhase::Thinking,
            crate::agent::model::AgentActivityPhase::Working,
        ] {
            let (mut app, mut commands) = app_with_connection();
            let start = Instant::now();
            active_phase(&mut app, phase);
            tick_context_usage_refresh(&mut app, start);
            expect_context_usage_command(&mut commands);
            context_result(&mut app, "session-1", Some(12));
            tick_context_usage_refresh(&mut app, start + Duration::from_millis(4999));
            assert!(commands.try_recv().is_err());
            tick_context_usage_refresh(&mut app, start + Duration::from_secs(5));
            expect_context_usage_command(&mut commands);
            context_result(&mut app, "session-1", Some(24));
            // An explicit request uses the same monotonic clock as periodic ticks.
            request_context_usage_refresh_at(&mut app, start + Duration::from_secs(7));
            expect_context_usage_command(&mut commands);
            context_result(&mut app, "session-1", Some(31));
            tick_context_usage_refresh(&mut app, start + Duration::from_millis(11999));
            assert!(commands.try_recv().is_err());
            tick_context_usage_refresh(&mut app, start + Duration::from_secs(12));
            expect_context_usage_command(&mut commands);
            context_result(&mut app, "session-1", Some(43));
            crate::app::handle_client_event(
                &mut app,
                crate::agent::events::ClientEvent::TurnComplete {
                    session_id: "session-1".into(),
                    queued_turn_count: None,
                    terminal_reason: None,
                },
            );
            // Turn completion legitimately asks for a final snapshot.
            expect_context_usage_command(&mut commands);
            context_result(&mut app, "session-1", Some(54));
            assert_eq!(app.status, AppStatus::Ready);
            tick_context_usage_refresh(&mut app, start + Duration::from_secs(30));
            assert!(commands.try_recv().is_err());
            request_context_usage_refresh_at(&mut app, start + Duration::from_secs(30));
            expect_context_usage_command(&mut commands);
            context_result(&mut app, "session-1", Some(65));
            active_phase(&mut app, phase);
            tick_context_usage_refresh(&mut app, start + Duration::from_millis(34999));
            assert!(commands.try_recv().is_err());
            tick_context_usage_refresh(&mut app, start + Duration::from_secs(35));
            expect_context_usage_command(&mut commands);
            context_result(&mut app, "session-1", Some(76));
            assert_eq!(app.session_runtime.session_usage.context_usage_percent, Some(76));
            assert!(commands.try_recv().is_err());
        }
    }

    fn context_result(app: &mut App, session: &str, percentage: Option<u8>) {
        crate::app::handle_client_event(
            app,
            crate::agent::events::ClientEvent::ContextUsageReceived {
                session_id: session.into(),
                percentage,
            },
        );
    }

    fn active_phase(app: &mut App, phase: crate::agent::model::AgentActivityPhase) {
        crate::app::test_support::deliver_update(
            app,
            crate::agent::model::SessionUpdate::AgentResponseStarted,
        );
        crate::app::test_support::deliver_update(
            app,
            crate::agent::model::SessionUpdate::AgentActivityUpdate(phase),
        );
        assert!(app.is_agent_turn_active());
        assert_eq!(
            app.status,
            if phase == crate::agent::model::AgentActivityPhase::Thinking {
                AppStatus::Thinking
            } else {
                AppStatus::Running
            }
        );
    }

    fn reload_request(
        commands: &mut crate::agent::client::CommandReceiver,
        force: bool,
        folders: &[String],
    ) -> String {
        let envelope = commands.try_recv().expect("reload request");
        assert_eq!(
            envelope.command,
            BridgeCommand::ReloadPlugins {
                session_id: "session-1".into(),
                force,
                add_plugins: folders.to_vec()
            }
        );
        let id = envelope.request_id.expect("correlated reload");
        assert!(!id.is_empty());
        assert!(commands.try_recv().is_err());
        id
    }

    fn hold_reload(app: &mut App, session: &str, id: &str) {
        crate::app::handle_client_event(
            app,
            crate::agent::events::ClientEvent::RuntimeReloadHeld {
                session_id: session.into(),
                request_id: Some(id.into()),
                error_count: 0,
                cache_impact: crate::agent::types::RuntimeReloadCacheImpact {
                    mcp_servers_added: vec!["fixture:tools".into()],
                    mcp_servers_removed: vec![],
                    lsp_tool_change: None,
                    invalid_server_name_count: 0,
                },
            },
        );
    }

    fn complete_reload(app: &mut App, session: &str, id: &str) {
        crate::app::handle_client_event(
            app,
            crate::agent::events::ClientEvent::RuntimeReloadCompleted {
                session_id: session.into(),
                request_id: Some(id.into()),
                error_count: 0,
                reconciliation_error: None,
            },
        );
    }

    fn assert_correlated_reload(folders: &[String], force: bool) {
        let (mut app, mut commands) = app_with_connection();
        app.trust.status = crate::app::trust::TrustStatus::Trusted;
        crate::app::config::open_tab(&mut app, crate::app::ConfigTab::Help).expect("open config");
        let installed = vec![crate::agent::types::SessionPlugin {
            name: "confirmed".into(),
            path: "old-folder".into(),
            source: None,
            version: Some("1.2.3".into()),
        }];
        crate::app::handle_client_event(
            &mut app,
            crate::agent::events::ClientEvent::PluginInventory {
                session_id: "session-1".into(),
                plugins: installed.clone(),
            },
        );
        let outcome = if folders.is_empty() {
            request_runtime_reload(&mut app)
        } else {
            super::request_plugin_folders(&mut app, folders.to_vec(), false)
        };
        assert!(matches!(outcome, RuntimeReloadRequestOutcome::Requested));
        let first = reload_request(&mut commands, false, folders);
        // An admitted operation cannot be forced before the bridge holds it.
        assert!(matches!(
            request_forced_runtime_reload(&mut app),
            RuntimeReloadRequestOutcome::Failed
        ));
        assert!(commands.try_recv().is_err());
        hold_reload(&mut app, "session-1", "wrong-id");
        hold_reload(&mut app, "other-session", &first);
        complete_reload(&mut app, "session-1", "wrong-id");
        assert!(app.config.confirmation_overlay().is_none());
        assert_eq!(app.plugins.pending_reload.as_ref().unwrap().request_id, first);
        assert_eq!(app.plugins.session_plugins, installed);
        let current = if force {
            hold_reload(&mut app, "session-1", &first);
            assert!(app.config.confirmation_overlay().is_some());
            crate::app::test_support::press(&mut app, crossterm::event::KeyCode::Down);
            crate::app::test_support::press(&mut app, crossterm::event::KeyCode::Enter);
            let second = reload_request(&mut commands, true, folders);
            assert_ne!(first, second);
            complete_reload(&mut app, "session-1", &first);
            hold_reload(&mut app, "session-1", &first);
            crate::app::handle_client_event(
                &mut app,
                crate::agent::events::ClientEvent::RuntimeReloadFailed {
                    session_id: "session-1".into(),
                    request_id: Some(first),
                    message: "stale failure".into(),
                },
            );
            assert_eq!(app.plugins.pending_reload.as_ref().unwrap().request_id, second);
            assert!(app.plugins.last_error.is_none());
            second
        } else {
            first
        };
        complete_reload(&mut app, "other-session", &current);
        assert_eq!(app.plugins.pending_reload.as_ref().unwrap().request_id, current);
        complete_reload(&mut app, "session-1", &current);
        assert!(app.plugins.pending_reload.is_none());
        assert_eq!(
            app.plugins.session_plugins, installed,
            "completion must not fabricate inventory"
        );
        let new = vec![crate::agent::types::SessionPlugin {
            name: "new".into(),
            path: "confirmed-folder".into(),
            source: None,
            version: None,
        }];
        crate::app::handle_client_event(
            &mut app,
            crate::agent::events::ClientEvent::PluginInventory {
                session_id: "session-1".into(),
                plugins: new.clone(),
            },
        );
        assert_eq!(app.plugins.session_plugins, new);
        assert!(commands.try_recv().is_err());
    }

    #[test]
    fn refused_context_refresh_retains_data_and_allows_scheduled_or_manual_retry() {
        for manual in [false, true] {
            let (mut app, commands) = app_with_connection();
            active_phase(&mut app, crate::agent::model::AgentActivityPhase::Thinking);
            app.input.set_text("unsent refresh draft");
            app.input.set_cursor(0, 6);
            context_result(&mut app, "session-1", Some(37));
            drop(commands);
            let start = Instant::now();
            request_context_usage_refresh_at(&mut app, start);
            assert_eq!(app.session_runtime.session_usage.context_usage_percent, Some(37));
            assert_eq!(app.input.text(), "unsent refresh draft");
            assert_eq!(app.input.cursor(), (0, 6));
            let (connection, mut commands) = crate::agent::client::AgentConnection::test_channel();
            app.session_runtime.conn = Some(std::rc::Rc::new(connection));
            for millis in if manual { [0, 1, 500, 999] } else { [0, 1, 2500, 4999] } {
                tick_context_usage_refresh(&mut app, start + Duration::from_millis(millis));
                assert!(
                    commands.try_recv().is_err(),
                    "failed admission must not cause a tick flood"
                );
            }
            if manual {
                request_context_usage_refresh_at(&mut app, start + Duration::from_secs(1));
            } else {
                tick_context_usage_refresh(&mut app, start + Duration::from_secs(5));
            }
            expect_context_usage_command(&mut commands);
            context_result(&mut app, "session-1", Some(68));
            assert_eq!(app.session_runtime.session_usage.context_usage_percent, Some(68));
            assert_eq!(app.input.text(), "unsent refresh draft");
            assert_eq!(app.input.cursor(), (0, 6));
            assert!(commands.try_recv().is_err());
            request_context_usage_refresh_at(&mut app, start + Duration::from_secs(9));
            expect_context_usage_command(&mut commands);
            context_result(&mut app, "session-1", Some(72));
            assert_eq!(app.session_runtime.session_usage.context_usage_percent, Some(72));
        }
    }
}
