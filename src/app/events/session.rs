// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang
use super::super::connect::take_connection_slot;
use super::super::connect::{SessionStartReason, start_new_session};
use super::super::state::RecentSessionInfo;
use super::super::view::{self, FullscreenView};
use super::super::{App, AppStatus, LoginHint, SystemSeverity};
use super::push_system_message_with_severity;
use super::session_reset::{ChatResetRenderMode, load_resume_history, reset_for_new_session};
use crate::agent::client::AgentConnection;
use crate::agent::events::ServiceStatusSeverity;
use crate::agent::model;
use crate::error::AppError;
use std::rc::Rc;

const TURN_ERROR_INPUT_LOCK_HINT: &str =
    "Input disabled after an error. Press Ctrl+Q to quit and try again.";

pub(super) struct SessionReplacedEventData {
    pub session_id: model::SessionId,
    pub cwd: String,
    pub current_model: model::CurrentModel,
    pub available_models: Vec<model::AvailableModel>,
    pub mode: Option<super::super::ModeState>,
    pub fast_mode_state: model::FastModeState,
    pub fast_mode_disabled_reason: Option<String>,
    pub history_updates: Vec<model::SessionUpdate>,
    pub restored_input: Option<String>,
}

pub(super) struct ConnectedEventData {
    pub session_id: model::SessionId,
    pub cwd: String,
    pub current_model: model::CurrentModel,
    pub available_models: Vec<model::AvailableModel>,
    pub mode: Option<super::super::ModeState>,
    pub fast_mode_state: model::FastModeState,
    pub fast_mode_disabled_reason: Option<String>,
    pub history_updates: Vec<model::SessionUpdate>,
}

pub(super) fn handle_connected_client_event(app: &mut App, event: ConnectedEventData) {
    let ConnectedEventData {
        session_id,
        cwd,
        current_model,
        available_models,
        mode,
        fast_mode_state,
        fast_mode_disabled_reason,
        history_updates,
    } = event;
    let session_id_for_log = session_id.to_string();
    let history_update_count = history_updates.len();
    let available_model_count = available_models.len();
    if let Some(slot) = take_connection_slot() {
        app.session_runtime.conn = Some(slot.conn);
    }
    apply_session_cwd(app, cwd);
    reset_for_new_session(
        app,
        session_id,
        current_model,
        mode,
        model::FastModeSnapshot::new(fast_mode_state, fast_mode_disabled_reason),
        true,
        ChatResetRenderMode::PreserveInlineViewport,
    );
    app.sdk_inventory.available_models = available_models;
    app.sync_welcome_snapshot();
    if !history_updates.is_empty() {
        load_resume_history(app, &history_updates);
    }
    maybe_emit_fast_mode_disabled_notice(app, None);
    clear_pending_command(app);
    app.clear_pending_session_resume();
    crate::app::file_index::restart(app);
    app.rebuild_chat_focus_from_state();
    crate::app::config::refresh_runtime_tabs_for_session_change(app);
    maybe_open_startup_session_picker(app);
    tracing::info!(
        target: crate::logging::targets::APP_SESSION,
        event_name = "session_connected",
        message = "session connected and applied",
        outcome = "success",
        session_id = %session_id_for_log,
        cwd = %app.cwd_raw,
        current_model = ?app.session_runtime.current_model.as_ref().map(|model| model.resolved_id.clone()),
        history_update_count,
        available_model_count,
    );
}

pub(super) fn handle_sessions_listed_event(
    app: &mut App,
    sessions: Vec<crate::agent::types::SessionListEntry>,
) {
    let session_count = sessions.len();
    let pending_title_change = app.config.pending_session_title_change.take();
    let selected_session_id = app
        .recent_sessions
        .get(app.session_picker.selected)
        .map(|session| session.session_id.clone());
    let had_pending_title_change = pending_title_change.is_some();
    app.recent_sessions = sessions
        .into_iter()
        .map(|entry| RecentSessionInfo {
            session_id: entry.session_id,
            summary: entry.summary,
            last_modified_ms: entry.last_modified_ms,
            file_size_bytes: entry.file_size_bytes,
            cwd: entry.cwd,
            git_branch: entry.git_branch,
            custom_title: entry.custom_title,
            first_prompt: entry.first_prompt,
        })
        .collect();
    let mut pending_title_change_resolved = false;
    if let Some(pending_title_change) = pending_title_change {
        let renamed_session_present = app
            .recent_sessions
            .iter()
            .any(|session| session.session_id == pending_title_change.session_id);
        pending_title_change_resolved = renamed_session_present;
        if renamed_session_present {
            app.config.last_error = None;
            app.config.status_message = Some(match pending_title_change.kind {
                crate::app::config::PendingSessionTitleChangeKind::Rename { requested_title } => {
                    match requested_title {
                        Some(title) => format!("Renamed session to {title}"),
                        None => "Cleared session name".to_owned(),
                    }
                }
                crate::app::config::PendingSessionTitleChangeKind::Generate => {
                    "Generated session title".to_owned()
                }
            });
        }
    }
    app.startup.mark_recent_sessions_loaded();
    reconcile_session_picker_selection(app, selected_session_id.as_deref());
    maybe_open_startup_session_picker(app);
    tracing::info!(
        target: crate::logging::targets::APP_SESSION,
        event_name = "sessions_list_updated",
        message = "sessions list applied",
        outcome = "success",
        session_count,
        had_pending_title_change,
        pending_title_change_resolved,
    );
}

pub(super) fn handle_auth_required_event(
    app: &mut App,
    method_name: String,
    method_description: String,
) {
    let method_name_for_log = method_name.clone();
    clear_pending_command(app);
    app.status = AppStatus::Ready;
    app.clear_pending_session_resume();
    app.session_runtime.login_hint = Some(LoginHint { method_name, method_description });
    app.bump_session_scope_epoch();
    app.clear_session_runtime_identity();
    super::compaction::reset(app);
    app.session_runtime.last_rate_limit_update = None;
    app.turn.clear_cancel_state();
    app.session_runtime.account_info = None;
    app.mcp = super::super::McpState::default();
    app.config.pending_session_title_change = None;
    crate::app::usage::reset_for_session_change(app);
    app.finalize_turn_runtime_artifacts(model::ToolCallStatus::Failed);
    app.turn.reset_for_new_session();
    tracing::warn!(
        target: crate::logging::targets::APP_AUTH,
        event_name = "auth_required_detected",
        message = "auth required cleared active session state",
        outcome = "blocked",
        method_name = %method_name_for_log,
    );
}

pub(super) fn handle_connection_failed_event(app: &mut App, msg: &str) {
    app.bump_session_scope_epoch();
    app.clear_session_runtime_identity();
    super::compaction::reset(app);
    app.turn.clear_cancel_state();
    app.session_runtime.last_rate_limit_update = None;
    app.session_runtime.account_info = None;
    app.mcp = super::super::McpState::default();
    app.config.pending_session_title_change = None;
    crate::app::usage::reset_for_session_change(app);
    app.clear_pending_session_resume();
    app.finalize_turn_runtime_artifacts(model::ToolCallStatus::Failed);
    app.input.clear();
    app.pending_submit = None;
    app.status = AppStatus::Error;
    app.turn.reset_for_new_session();
    push_connection_error_message(app, msg);
    tracing::error!(
        target: crate::logging::targets::APP_SESSION,
        event_name = "session_connection_failed",
        message = "session connection failure applied",
        outcome = "failure",
        error_message = %msg,
    );
}

pub(super) fn handle_slash_command_error_event(app: &mut App, msg: &str) {
    if app.config.pending_session_title_change.take().is_some() {
        app.config.last_error = Some(msg.to_owned());
        app.config.status_message = None;
        app.request_active_surface_repaint();
        return;
    }
    super::notices::emit_system_notice(app, SystemSeverity::Error, msg);
    clear_pending_command(app);
    app.clear_pending_session_resume();
}

pub(super) fn handle_session_resume_failed_event(
    app: &mut App,
    session_id: &str,
    operation_id: &str,
    message: &str,
) {
    let matches_pending_operation = app.pending_session_resume_id() == Some(session_id)
        && app.pending_resume_at_operation_id() == Some(operation_id);
    if !matches_pending_operation {
        tracing::debug!(
            target: crate::logging::targets::APP_SESSION,
            event_name = "session_resume_failure_dropped",
            message = "session resume failure dropped for a stale operation",
            outcome = "dropped",
            session_id,
            operation_id,
        );
        return;
    }

    super::notices::emit_system_notice(app, SystemSeverity::Error, message);
    clear_pending_command(app);
    app.clear_pending_session_resume();
    tracing::warn!(
        target: crate::logging::targets::APP_SESSION,
        event_name = "session_resume_failed",
        message = "session resume-at operation failed",
        outcome = "failure",
        session_id,
        operation_id,
        error_message = message,
    );
}

pub(super) fn handle_auth_completed_event(app: &mut App, conn: &Rc<AgentConnection>) {
    app.session_runtime.login_hint = None;
    app.turn.pending_command_label = Some("Starting session...".to_owned());
    app.turn.pending_command_ack = None;
    push_system_message_with_severity(
        app,
        Some(SystemSeverity::Info),
        "Authentication successful. Starting new session...",
    );
    app.request_chat_visible_rebuild();
    tracing::info!(
        target: crate::logging::targets::APP_AUTH,
        event_name = "login_completed",
        message = "login completed and session restart requested",
        outcome = "success",
    );

    if let Err(e) = start_new_session(app, conn, SessionStartReason::Login) {
        tracing::error!(
            target: crate::logging::targets::APP_AUTH,
            event_name = "login_session_restart_failed",
            message = "failed to start session after login",
            outcome = "failure",
            error_message = %e,
        );
        clear_pending_command(app);
        push_system_message_with_severity(
            app,
            Some(SystemSeverity::Error),
            &format!("Failed to start session after login: {e}"),
        );
    }
}

pub(super) fn handle_logout_completed_event(app: &mut App) {
    // Clear the session and start a new one. The bridge now checks auth
    // during initialization and will fire AuthRequired immediately.
    app.bump_session_scope_epoch();
    app.clear_session_runtime_identity();
    app.session_runtime.account_info = None;
    app.mcp = super::super::McpState::default();
    app.config.pending_session_title_change = None;
    crate::app::usage::reset_for_session_change(app);
    app.request_chat_visible_rebuild();
    tracing::info!(
        target: crate::logging::targets::APP_AUTH,
        event_name = "logout_completed",
        message = "logout cleared active session state",
        outcome = "success",
    );

    if let Some(conn) = app.session_runtime.conn.clone() {
        app.turn.pending_command_label = Some("Starting session...".to_owned());
        app.turn.pending_command_ack = None;
        if let Err(e) = start_new_session(app, &conn, SessionStartReason::Logout) {
            tracing::error!(
                target: crate::logging::targets::APP_AUTH,
                event_name = "logout_session_restart_failed",
                message = "failed to start replacement session after logout",
                outcome = "failure",
                error_message = %e,
            );
            clear_pending_command(app);
            push_system_message_with_severity(
                app,
                Some(SystemSeverity::Error),
                &format!("Failed to start new session after logout: {e}"),
            );
        }
    } else {
        tracing::warn!(
            target: crate::logging::targets::APP_AUTH,
            event_name = "logout_session_restart_unavailable",
            message = "logout completed without a connection to start a replacement session",
            outcome = "blocked",
            reason = "missing_connection",
        );
        clear_pending_command(app);
        push_system_message_with_severity(
            app,
            Some(SystemSeverity::Warning),
            "Logged out, but no connection available to start a new session.",
        );
    }
}

pub(super) fn handle_session_replaced_event(app: &mut App, event: SessionReplacedEventData) {
    let SessionReplacedEventData {
        session_id,
        cwd,
        current_model,
        available_models,
        mode,
        fast_mode_state,
        fast_mode_disabled_reason,
        history_updates,
        restored_input,
    } = event;
    let session_id_for_log = session_id.to_string();
    let history_update_count = history_updates.len();
    let available_model_count = available_models.len();
    super::compaction::reset(app);
    apply_session_cwd(app, cwd);
    app.sdk_inventory.available_models = available_models;
    reset_for_new_session(
        app,
        session_id,
        current_model,
        mode,
        model::FastModeSnapshot::new(fast_mode_state, fast_mode_disabled_reason),
        false,
        ChatResetRenderMode::DeferTranscriptRender,
    );
    app.sync_welcome_snapshot();
    if !history_updates.is_empty() {
        load_resume_history(app, &history_updates);
    }
    maybe_emit_fast_mode_disabled_notice(app, None);
    clear_pending_command(app);
    if let Some(restored_input) = restored_input.as_deref() {
        app.input.set_text(restored_input);
    }
    app.clear_pending_session_resume();
    crate::app::file_index::restart(app);
    crate::app::config::refresh_runtime_tabs_for_session_change(app);
    // After session replacement, terminal scrollback is stale. Rebuild from
    // app.transcript.messages, which was rebuilt only from bridge-reported session history.
    app.request_chat_purge_replay_rebuild(crate::app::ChatPurgeReplayOptions::session_replacement());
    tracing::info!(
        target: crate::logging::targets::APP_SESSION,
        event_name = "session_replaced",
        message = "replacement session applied",
        outcome = "success",
        session_id = %session_id_for_log,
        cwd = %app.cwd_raw,
        current_model = ?app.session_runtime.current_model.as_ref().map(|model| model.resolved_id.clone()),
        history_update_count,
        available_model_count,
        restored_input = restored_input.is_some(),
    );
}

pub(super) fn maybe_emit_fast_mode_disabled_notice(app: &mut App, previous_reason: Option<&str>) {
    if !app.config.fast_mode_effective()
        || !matches!(app.session_runtime.fast_mode_state, model::FastModeState::Off)
    {
        return;
    }
    let Some(reason) = app.session_runtime.fast_mode_disabled_reason.as_deref() else {
        return;
    };
    if previous_reason == Some(reason) {
        return;
    }

    let message = fast_mode_disabled_message(reason);
    tracing::info!(
        target: crate::logging::targets::APP_SESSION,
        event_name = "fast_mode_disabled_reason_observed",
        message = "fast mode disabled reason observed",
        outcome = "unavailable",
        reason,
    );
    super::notices::emit_system_notice(app, SystemSeverity::Warning, message);
}

fn fast_mode_disabled_message(reason: &str) -> &'static str {
    match reason {
        "free" => "Fast mode is unavailable on the free plan.",
        "preference" => "Fast mode is disabled by an account preference.",
        "extra_usage_disabled" => "Fast mode requires extra usage to be enabled.",
        "network_error" => "Fast mode is unavailable because its availability check failed.",
        "not_first_party" => "Fast mode is unavailable through the current API provider.",
        "disabled_by_env" => "Fast mode is disabled by the runtime environment.",
        "model_not_allowed" => "Fast mode is unavailable for the current model.",
        "sdk_opt_in_required" => "Fast mode requires SDK opt-in before it can activate.",
        "pending" => "Fast mode availability is still being determined.",
        _ => "Fast mode is currently unavailable.",
    }
}

pub(super) fn handle_rewind_result_event(app: &mut App, result: &model::RewindResult) {
    if app.session_runtime.session_id.as_ref().map(ToString::to_string).as_deref()
        != Some(result.session_id.as_str())
    {
        tracing::debug!(
            target: crate::logging::targets::APP_SESSION,
            event_name = "rewind_result_dropped",
            message = "rewind result dropped for a stale session",
            outcome = "dropped",
            session_id = %result.session_id,
            reason = "stale_session",
        );
        return;
    }

    let skipped_links =
        result.file_result.as_ref().and_then(|file_result| file_result.skipped_links);
    let severity = match result.status {
        model::RewindResultStatus::Success if skipped_links.is_some_and(|count| count > 0) => {
            SystemSeverity::Warning
        }
        model::RewindResultStatus::Success => SystemSeverity::Info,
        model::RewindResultStatus::Failure | model::RewindResultStatus::PartialFailure => {
            SystemSeverity::Error
        }
    };
    let message = rewind_result_message(result);
    super::notices::emit_system_notice(app, severity, &message);
    clear_pending_command(app);
}

fn rewind_result_message(result: &model::RewindResult) -> String {
    if let Some(message) = result.message.as_deref()
        && !message.trim().is_empty()
        && !matches!(result.status, model::RewindResultStatus::Success)
    {
        return message.to_owned();
    }

    let Some(file_result) = result.file_result.as_ref() else {
        return result
            .message
            .clone()
            .unwrap_or_else(|| format!("Rewind {} completed.", result.restore_mode.label()));
    };
    let file_count = file_result.files_changed.len();
    let insertions = file_result.insertions.unwrap_or(0);
    let deletions = file_result.deletions.unwrap_or(0);
    let file_word = if file_count == 1 { "file" } else { "files" };
    match result.status {
        model::RewindResultStatus::Success
            if file_result.skipped_links.is_some_and(|count| count > 0) =>
        {
            let skipped_links = file_result.skipped_links.unwrap_or_default();
            let path_word = if skipped_links == 1 { "path was" } else { "paths were" };
            format!(
                "Restored tracked code for {file_count} {file_word} ({insertions} insertions, {deletions} deletions), but {skipped_links} unsafe linked {path_word} skipped."
            )
        }
        model::RewindResultStatus::Success => format!(
            "Restored code for {file_count} {file_word} ({insertions} insertions, {deletions} deletions)."
        ),
        model::RewindResultStatus::Failure => file_result
            .error
            .clone()
            .or_else(|| result.message.clone())
            .unwrap_or_else(|| "Failed to restore code.".to_owned()),
        model::RewindResultStatus::PartialFailure => result.message.clone().unwrap_or_else(|| {
            "Code was restored, but the conversation could not be rewound.".to_owned()
        }),
    }
}

pub(super) fn handle_update_available_event(
    app: &mut App,
    latest_version: &str,
    current_version: &str,
) {
    let Some(release_url) = crate::app::settings::release_url_for_version(latest_version) else {
        return;
    };
    crate::app::settings::record_update_check_result(
        &mut app.global_settings,
        current_version,
        latest_version,
        &release_url,
        crate::app::update_check::unix_now_secs().unwrap_or(0),
    );
    if let Some(path) = app.global_settings_path.as_ref()
        && let Err(err) = crate::app::settings::save_global_settings(path, &app.global_settings)
    {
        tracing::warn!(
            target: crate::logging::targets::APP_UPDATE,
            event_name = "update_available_settings_save_failed",
            message = "failed to persist update availability",
            outcome = "failure",
            settings_path = %path.display(),
            error_message = %err,
        );
    }
    tracing::info!(
        target: crate::logging::targets::APP_UPDATE,
        event_name = "update_available_applied",
        message = "update availability applied",
        outcome = "success",
        latest_version = %latest_version,
        current_version = %current_version,
    );
}

pub(super) fn handle_service_status_event(
    app: &mut App,
    severity: ServiceStatusSeverity,
    message: &str,
) {
    let ui_severity = match severity {
        ServiceStatusSeverity::Warning => SystemSeverity::Warning,
        ServiceStatusSeverity::Error => SystemSeverity::Error,
    };
    push_system_message_with_severity(app, Some(ui_severity), message);
    match severity {
        ServiceStatusSeverity::Warning => tracing::warn!(
            target: crate::logging::targets::APP_NETWORK,
            event_name = "service_status_applied",
            message = "service status warning applied",
            outcome = "success",
            severity = ?severity,
            service_message = %message,
        ),
        ServiceStatusSeverity::Error => tracing::error!(
            target: crate::logging::targets::APP_NETWORK,
            event_name = "service_status_applied",
            message = "service status error applied",
            outcome = "success",
            severity = ?severity,
            service_message = %message,
        ),
    }
}

pub(super) fn handle_fatal_error_event(app: &mut App, error: AppError) {
    app.finalize_turn_runtime_artifacts(model::ToolCallStatus::Failed);
    app.turn.reset_for_new_session();
    app.exit_error = Some(error);
    app.request_shutdown();
    app.status = AppStatus::Error;
    app.pending_submit = None;
}

/// Clear pending slash-command UI. Turn and session lifecycle handlers own non-command status.
pub(super) fn clear_pending_command(app: &mut App) {
    app.turn.pending_command_label = None;
    app.turn.pending_command_ack = None;
    if matches!(app.status, AppStatus::CommandPending) {
        app.status = AppStatus::Ready;
    }
}

fn push_connection_error_message(app: &mut App, error: &str) {
    let message = format!("Connection failed: {error}\n\n{TURN_ERROR_INPUT_LOCK_HINT}");
    push_system_message_with_severity(app, None, &message);
}

fn shorten_cwd_display(cwd_raw: &str) -> String {
    if let Some(home) = dirs::home_dir() {
        let home_str = home.to_string_lossy();
        if cwd_raw.starts_with(home_str.as_ref()) {
            return format!("~{}", &cwd_raw[home_str.len()..]);
        }
    }
    cwd_raw.to_owned()
}

fn sync_welcome_cwd(app: &mut App) {
    app.sync_welcome_snapshot();
}

pub(super) fn apply_session_cwd(app: &mut App, cwd_raw: String) {
    app.cwd_raw = cwd_raw;
    app.cwd = shorten_cwd_display(&app.cwd_raw);
    app.sync_git_context();
    sync_welcome_cwd(app);
    app.reconcile_trust_state_from_preferences_and_cwd();
}

fn reconcile_session_picker_selection(app: &mut App, selected_session_id: Option<&str>) {
    let session_count = super::super::session_picker::picker_session_count(app);
    if session_count == 0 {
        app.session_picker.selected = 0;
        app.session_picker.scroll_offset = 0;
        return;
    }

    if let Some(session_id) = selected_session_id
        && let Some(idx) =
            app.recent_sessions.iter().position(|session| session.session_id == session_id)
        && idx < session_count
    {
        app.session_picker.selected = idx;
    } else {
        app.session_picker.selected =
            app.session_picker.selected.min(session_count.saturating_sub(1));
    }
    app.session_picker.scroll_offset =
        app.session_picker.scroll_offset.min(app.session_picker.selected);
}

fn maybe_open_startup_session_picker(app: &mut App) {
    if app.update_prompt.is_some() {
        return;
    }
    if app.session_runtime.conn.is_none() || !app.startup.startup_picker_is_ready() {
        return;
    }

    app.startup.resolve_session_picker();
    let session_count = super::super::session_picker::picker_session_count(app);
    if session_count == 0 {
        push_system_message_with_severity(
            app,
            Some(SystemSeverity::Info),
            "No recent sessions found for this directory; continuing with a new session.",
        );
        return;
    }

    app.session_picker.selected = app.session_picker.selected.min(session_count - 1);
    app.session_picker.scroll_offset = 0;
    view::set_fullscreen_view(app, FullscreenView::SessionPicker);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::file_index::FileCandidate;
    use crate::app::{App, MessageRole};
    use std::time::{Duration, Instant};

    fn wait_for(app: &mut App, timeout: Duration, mut predicate: impl FnMut(&App) -> bool) {
        let start = Instant::now();
        while start.elapsed() < timeout {
            crate::app::file_index::drain_events(app);
            if predicate(app) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        crate::app::file_index::drain_events(app);
        assert!(predicate(app), "condition not met before timeout");
    }

    fn candidate(rel_path: &str) -> FileCandidate {
        FileCandidate {
            rel_path: rel_path.to_owned(),
            rel_path_lower: rel_path.to_lowercase(),
            basename_lower: rel_path.rsplit('/').next().unwrap_or(rel_path).to_lowercase(),
            depth: rel_path.matches('/').count(),
        }
    }

    fn successful_rewind(skipped_links: Option<u64>) -> model::RewindResult {
        model::RewindResult {
            session_id: "session-1".to_owned(),
            restore_mode: model::RewindRestoreMode::Code,
            status: model::RewindResultStatus::Success,
            file_result: Some(model::RewindFilesResult {
                can_rewind: true,
                error: None,
                files_changed: vec!["src/main.rs".to_owned()],
                insertions: Some(2),
                deletions: Some(1),
                skipped_links,
            }),
            message: None,
        }
    }

    #[test]
    fn successful_rewind_reports_skipped_unsafe_paths_without_inflating_file_count() {
        assert_eq!(
            rewind_result_message(&successful_rewind(None)),
            "Restored code for 1 file (2 insertions, 1 deletions)."
        );
        assert_eq!(
            rewind_result_message(&successful_rewind(Some(0))),
            "Restored code for 1 file (2 insertions, 1 deletions)."
        );
        assert_eq!(
            rewind_result_message(&successful_rewind(Some(2))),
            "Restored tracked code for 1 file (2 insertions, 1 deletions), but 2 unsafe linked paths were skipped."
        );
    }

    #[test]
    fn successful_rewind_with_skips_emits_warning() {
        let mut app = App::test_default();
        app.session_runtime.session_id = Some(model::SessionId::new("session-1"));

        handle_rewind_result_event(&mut app, &successful_rewind(Some(1)));

        let message = app.transcript.messages.last().expect("rewind notice");
        assert!(matches!(message.role, MessageRole::System(Some(SystemSeverity::Warning))));
    }

    #[test]
    fn fast_mode_disabled_reason_wording_covers_target_values() {
        for reason in [
            "free",
            "preference",
            "extra_usage_disabled",
            "network_error",
            "unknown",
            "not_first_party",
            "disabled_by_env",
            "model_not_allowed",
            "sdk_opt_in_required",
            "pending",
            "future-reason",
        ] {
            assert!(!fast_mode_disabled_message(reason).is_empty());
        }
    }

    #[test]
    fn connected_refreshes_file_index_candidates_for_new_cwd() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("new.rs"), "").expect("write file");
        let mut app = App::test_default();
        app.file_index.generation = 3;
        app.file_index.entries.insert("stale.rs".to_owned(), candidate("stale.rs"));
        app.file_index.scan_finished = true;

        handle_connected_client_event(
            &mut app,
            ConnectedEventData {
                session_id: model::SessionId::new("session-1"),
                cwd: dir.path().to_string_lossy().into_owned(),
                current_model: model::CurrentModel::new("model", "model", "model")
                    .authoritative(true),
                available_models: Vec::new(),
                mode: None,
                fast_mode_state: model::FastModeState::Off,
                fast_mode_disabled_reason: None,
                history_updates: Vec::new(),
            },
        );

        assert_eq!(app.file_index.root.as_deref(), Some(dir.path()));
        assert!(app.file_index.generation > 3);
        assert!(app.file_index.scan.is_some());
        assert!(app.file_index.watch.is_some());
        assert!(app.file_index.entries.is_empty());
        assert!(app.mention.is_none());
        wait_for(&mut app, Duration::from_secs(2), |app| {
            app.file_index.scan_finished && app.file_index.entries.contains_key("new.rs")
        });
        assert_eq!(
            app.file_index.entries.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["new.rs"]
        );
    }

    #[test]
    fn session_replaced_refreshes_file_index_candidates_for_replaced_cwd() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("after.rs"), "").expect("write file");
        let mut app = App::test_default();
        app.file_index.generation = 8;
        app.file_index.entries.insert("before.rs".to_owned(), candidate("before.rs"));
        app.file_index.scan_finished = true;

        handle_session_replaced_event(
            &mut app,
            SessionReplacedEventData {
                session_id: model::SessionId::new("session-2"),
                cwd: dir.path().to_string_lossy().into_owned(),
                current_model: model::CurrentModel::new("model", "model", "model")
                    .authoritative(true),
                available_models: Vec::new(),
                mode: None,
                fast_mode_state: model::FastModeState::Off,
                fast_mode_disabled_reason: None,
                history_updates: Vec::new(),
                restored_input: None,
            },
        );

        assert_eq!(app.file_index.root.as_deref(), Some(dir.path()));
        assert!(app.file_index.generation > 8);
        assert!(app.file_index.scan.is_some());
        assert!(app.file_index.watch.is_some());
        assert!(app.file_index.entries.is_empty());
        assert!(app.mention.is_none());
        wait_for(&mut app, Duration::from_secs(2), |app| {
            app.file_index.scan_finished && app.file_index.entries.contains_key("after.rs")
        });
        assert_eq!(
            app.file_index.entries.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["after.rs"]
        );
    }
}
