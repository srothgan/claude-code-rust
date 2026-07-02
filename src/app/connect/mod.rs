// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

//! App creation and bridge connection lifecycle.
//!
//! Submodules:
//! - `bridge_lifecycle`: spawning the bridge process, init handshake, event loop
//! - `event_dispatch`: routing `BridgeEvent` envelopes to `ClientEvent` messages
//! - `type_converters`: bridge wire types -> app model types

mod bridge_lifecycle;
mod event_dispatch;
mod session_start;
mod type_converters;

use super::config::ConfigState;
use super::plugins::PluginsState;
use super::state::{
    CacheMetrics, HistoryRetentionPolicy, HistoryRetentionStats, RenderCacheBudget,
    SessionPickerState, StartupState, Transcript,
};
use super::trust;
use super::view::SurfaceMode;
use super::{App, AppStatus, FocusManager};
use super::{SurfaceDirtyState, TerminalLifecycleState};
use crate::agent::client::AgentConnection;
use crate::agent::events::ClientEvent;
use crate::agent::model;
use crate::agent::wire::SessionLaunchSettings;
use crate::error::AppError;
use crate::{Cli, Command};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use tokio::sync::mpsc;

/// Shorten cwd for display: use `~` for the home directory prefix.
fn shorten_cwd(cwd: &std::path::Path) -> String {
    let cwd_str = cwd.to_string_lossy().to_string();
    if let Some(home) = dirs::home_dir() {
        let home_str = home.to_string_lossy().to_string();
        if cwd_str.starts_with(&home_str) {
            return format!("~{}", &cwd_str[home_str.len()..]);
        }
    }
    cwd_str
}

fn resolve_startup_cwd(cli: &Cli) -> PathBuf {
    cli.dir
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

fn extract_app_error(err: &anyhow::Error) -> Option<AppError> {
    err.chain().find_map(|cause| cause.downcast_ref::<AppError>().cloned())
}

struct StartConnectionParams {
    event_tx: mpsc::UnboundedSender<ClientEvent>,
    cwd_raw: String,
    bridge_script: Option<std::path::PathBuf>,
    resume_id: Option<String>,
    resume_requested: bool,
    session_launch_settings: SessionLaunchSettings,
}

pub(crate) use session_start::{
    SessionStartReason, begin_resume_session, begin_rewind, start_new_session,
};

/// Create the `App` struct in `Connecting` state and load shared settings state.
#[allow(clippy::too_many_lines)]
pub fn create_app(cli: &Cli) -> App {
    let cwd = resolve_startup_cwd(cli);

    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let (file_index_event_tx, file_index_event_rx) = std::sync::mpsc::channel();
    let terminals: crate::agent::events::TerminalMap =
        Rc::new(std::cell::RefCell::new(HashMap::new()));
    let perf_path = match crate::logging::resolve_perf_path(cli) {
        Ok(path) => path,
        Err(err) => {
            tracing::warn!(
                target: crate::logging::targets::APP_PERF,
                event_name = "perf_telemetry_unavailable",
                message = "failed to resolve perf telemetry sidecar path",
                outcome = "failure",
                telemetry_channel = "perf_sidecar",
                perf_schema = "claude-rs-perf/v1",
                perf_append = cli.perf_append,
                error = %err,
            );
            None
        }
    };
    let perf = perf_path.as_deref().and_then(|path| {
        let logger = crate::perf::PerfLogger::open(path, cli.perf_append);
        if logger.is_some() {
            tracing::info!(
                target: crate::logging::targets::APP_PERF,
                event_name = "perf_telemetry_enabled",
                message = "perf telemetry sidecar enabled",
                outcome = "success",
                telemetry_channel = "perf_sidecar",
                perf_schema = "claude-rs-perf/v1",
                perf_log = %path.display(),
                perf_append = cli.perf_append,
            );
        } else {
            tracing::warn!(
                target: crate::logging::targets::APP_PERF,
                event_name = "perf_telemetry_unavailable",
                message = "failed to enable perf telemetry sidecar",
                outcome = "failure",
                telemetry_channel = "perf_sidecar",
                perf_schema = "claude-rs-perf/v1",
                perf_log = %path.display(),
                perf_append = cli.perf_append,
            );
        }
        logger
    });

    let cwd_display = shorten_cwd(&cwd);
    let mut app = App {
        surface_mode: SurfaceMode::Chat,
        terminal_lifecycle: TerminalLifecycleState::Bootstrapping,
        surface_dirty: SurfaceDirtyState::initial_chat(),
        config: ConfigState::default(),
        trust: trust::TrustState::default(),
        settings_home_override: None,
        transcript: Transcript::new(vec![super::ChatMessage::welcome(
            env!("CARGO_PKG_VERSION"),
            "-",
            &cwd_display,
            "-",
        )]),
        input: super::InputState::new(),
        status: AppStatus::Connecting,
        resuming_session_id: None,
        show_session_overview: !matches!(
            &cli.command,
            Some(Command::Resume { session_id: Some(_) })
        ),
        turn: super::state::TurnState::default(),
        should_quit: false,
        exit_error: None,
        session_id: None,
        conn: None,
        session_scope_epoch: 0,
        current_model: None,
        cwd_raw: cwd.to_string_lossy().to_string(),
        cwd: cwd_display,
        files_accessed: 0,
        mode: None,
        config_options: std::collections::BTreeMap::new(),
        login_hint: None,
        event_tx,
        event_rx,
        file_index_event_tx,
        file_index_event_rx,
        spinner_frame: 0,
        spinner_last_advance_at: None,
        tool_call_scopes: HashMap::new(),
        terminals,
        tasks: Vec::new(),
        focus: FocusManager::default(),
        keymap: super::keymap::ResolvedKeymap::defaults(),
        available_commands: Vec::new(),
        rewind_targets: Vec::new(),
        rewind_targets_session_id: None,
        rewind_targets_in_flight: false,
        plugins: PluginsState::default(),
        available_agents: Vec::new(),
        available_models: Vec::new(),
        recent_sessions: Vec::new(),
        session_picker: SessionPickerState::default(),
        chat_render: super::ChatRenderState::default(),
        mention: None,
        file_index: super::file_index::FileIndexState::default(),
        slash: None,
        subagent: None,
        pending_submit: None,
        paste: super::state::PasteState::default(),
        pending_images: Vec::new(),
        git_context: super::git_context::GitContextState::default(),
        update_notice: None,
        session_usage: super::SessionUsageState::default(),
        usage: super::UsageState::default(),
        mcp: super::McpState::default(),
        fast_mode_state: model::FastModeState::Off,
        runtime_session_state: None,
        prompt_suggestion: None,
        last_rate_limit_update: None,
        account_info: None,
        notifications: super::notify::NotificationManager::new(),
        perf,
        render_cache_budget: RenderCacheBudget::default(),
        history_retention: HistoryRetentionPolicy::default(),
        history_retention_stats: HistoryRetentionStats::default(),
        cache_metrics: CacheMetrics::default(),
        fps_ema: None,
        last_frame_at: None,
        last_chat_render_trace_state: None,
        startup: StartupState::new(
            cli.bridge_script.clone(),
            match &cli.command {
                Some(Command::Resume { session_id: Some(id) }) => Some(id.clone()),
                _ => None,
            },
            matches!(&cli.command, Some(Command::Resume { session_id: None })),
        ),
    };

    if let Err(err) = super::config::initialize_shared_state(&mut app) {
        tracing::warn!(
            target: crate::logging::targets::APP_CONFIG,
            event_name = "shared_settings_init_failed",
            message = "failed to initialize shared settings state",
            outcome = "failure",
            error_message = %err,
        );
        app.config.last_error = Some(err);
    }

    app.rebuild_history_retention_accounting();
    app.rebuild_render_cache_accounting();
    trust::initialize(&mut app);
    app.sync_git_context();
    super::file_index::restart(&mut app);
    app
}

/// Spawn the background bridge task.
pub fn start_connection(app: &mut App) {
    if !app.startup.mark_connection_started() {
        return;
    }

    let params = StartConnectionParams {
        event_tx: app.event_tx.clone(),
        cwd_raw: app.cwd_raw.clone(),
        bridge_script: app.startup.bridge_script().cloned(),
        resume_id: app.startup.resume_id().map(str::to_owned),
        resume_requested: app.startup.resume_requested(),
        session_launch_settings: session_start::session_launch_settings_for_reason(
            app,
            session_start::SessionStartReason::Startup,
        ),
    };
    let conn_slot: Rc<std::cell::RefCell<Option<ConnectionSlot>>> =
        Rc::new(std::cell::RefCell::new(None));
    let conn_slot_writer = Rc::clone(&conn_slot);

    tokio::task::spawn_local(async move {
        bridge_lifecycle::run_connection_task(params, conn_slot_writer).await;
    });

    CONN_SLOT.with(|slot| {
        debug_assert!(
            slot.borrow().is_none(),
            "CONN_SLOT already populated -- start_connection() called twice?"
        );
        *slot.borrow_mut() = Some(conn_slot);
    });
}

/// Shared slot for passing `Rc<AgentConnection>` from the background task to the event loop.
pub struct ConnectionSlot {
    pub conn: Rc<AgentConnection>,
}

thread_local! {
    pub static CONN_SLOT: std::cell::RefCell<Option<Rc<std::cell::RefCell<Option<ConnectionSlot>>>>> =
        const { std::cell::RefCell::new(None) };
}

/// Take the connection data from the thread-local slot.
pub(super) fn take_connection_slot() -> Option<ConnectionSlot> {
    CONN_SLOT.with(|slot| slot.borrow().as_ref().and_then(|inner| inner.borrow_mut().take()))
}

#[cfg(test)]
mod tests;
