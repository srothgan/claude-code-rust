// SPDX-License-Identifier: Apache-2.0
use super::super::connect::take_connection_slot;
use super::super::connect::{SessionStartReason, start_new_session};
use super::super::state::RecentSessionInfo;
use super::super::{App, AppStatus, LoginHint, SystemSeverity};
use super::push_system_message_with_severity;
use super::session_reset::{ChatResetKind, load_resume_history, reset_for_new_session};
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
    pub ultracode: Option<model::UltracodeState>,
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
    pub ultracode: Option<model::UltracodeState>,
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
        ultracode,
        history_updates,
    } = event;
    let session_id_for_log = session_id.to_string();
    let history_update_count = history_updates.len();
    let available_model_count = available_models.len();
    if let Some(slot) = take_connection_slot() {
        app.session_runtime.conn = Some(slot.conn);
    }
    app.startup.mark_connected(crate::app::input_event_now());
    apply_session_cwd(app, cwd);
    reset_for_new_session(
        app,
        session_id,
        current_model,
        mode,
        model::FastModeSnapshot::new(fast_mode_state, fast_mode_disabled_reason),
        ChatResetKind::InitialConnection,
    );
    app.sdk_inventory.available_models = available_models;
    app.session_runtime.ultracode = ultracode;
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
    if !app.startup.session_picker_requested() {
        app.startup.complete_launch();
    }
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
    let selected_session_id = super::super::session_picker::selected_session(app)
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
            if matches!(
                pending_title_change.kind,
                crate::app::config::PendingSessionTitleChangeKind::Rename { .. }
            ) && app.config.session_rename_overlay().is_some()
            {
                app.config.clear_overlay();
            }
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
    if app.recent_sessions.is_empty() {
        app.startup.complete_launch();
    }
    super::super::session_picker::reconcile_selection(app, selected_session_id.as_deref());
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
    super::turn::recover_all_pending_user_messages(app);
    let method_name_for_log = method_name.clone();
    clear_pending_command(app);
    app.status = AppStatus::Ready;
    app.clear_pending_session_resume();
    app.session_runtime.login_hint = Some(LoginHint { method_name, method_description });
    app.bump_session_scope_epoch();
    app.clear_session_runtime_identity();
    super::compaction::reset(app);
    app.turn.clear_cancel_state();
    app.session_runtime.account_info = None;
    app.mcp = super::super::McpState::default();
    app.config.pending_session_title_change = None;
    crate::app::usage::reset_for_session_change(app);
    app.finalize_session_runtime_artifacts(model::ToolCallStatus::Failed);
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
    super::turn::recover_all_pending_user_messages(app);
    app.bump_session_scope_epoch();
    app.clear_session_runtime_identity();
    super::compaction::reset(app);
    app.turn.clear_cancel_state();
    app.session_runtime.account_info = None;
    app.mcp = super::super::McpState::default();
    app.config.pending_session_title_change = None;
    crate::app::usage::reset_for_session_change(app);
    app.clear_pending_session_resume();
    app.finalize_session_runtime_artifacts(model::ToolCallStatus::Failed);
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

pub(super) fn handle_connection_failure(
    app: &mut App,
    failure: &crate::agent::events::ConnectionFailure,
) {
    let message = if let Some(startup) = &failure.startup_failure {
        let (reason, errors) = startup_failure_diagnostics(startup);
        tracing::error!(
            target: crate::logging::targets::APP_SESSION,
            event_name = "sdk_startup_failed",
            startup_failure_reason = %reason,
            startup_failure_errors = ?errors,
            request_id = failure.request_id.as_deref(),
            "Claude Code startup failed",
        );
        startup_failure_message(&startup.reason)
    } else {
        &failure.message
    };
    handle_connection_failed_event(app, message);
}

fn startup_failure_diagnostics(
    startup: &crate::agent::types::StartupFailure,
) -> (String, Vec<String>) {
    let bounded_redacted = |value: &str| {
        crate::cli::redaction::redact_text(value).chars().take(1_024).collect::<String>()
    };
    (
        bounded_redacted(&startup.reason),
        startup.errors.iter().take(8).map(|error| bounded_redacted(error)).collect(),
    )
}

fn startup_failure_message(reason: &str) -> &'static str {
    match reason {
        "org_pin_api_key_conflict" => {
            "Configured API credentials conflict with organization-required sign-in. Remove the conflicting credentials and sign in again."
        }
        "provider_not_allowed" => {
            "Organization policy disallows the selected provider. Select a provider permitted by your organization."
        }
        "org_verify_failed" => {
            "Organization verification failed. Check connectivity or sign in again."
        }
        "org_pin_mismatch" => {
            "Your signed-in account belongs to an organization not permitted by policy. Sign in with an allowed account."
        }
        "managed_settings_invalid" => {
            "Managed policy settings are invalid or leave no usable model. Contact your organization administrator."
        }
        "remote_settings_required_unavailable" => {
            "Required remote managed settings could not be loaded. Check connectivity and contact your administrator."
        }
        "org_config_required_unavailable" => {
            "Required organization configuration is temporarily unavailable. Check connectivity and retry."
        }
        "org_config_refused" => {
            "Organization configuration access was refused. Sign in with an allowed account or contact your administrator."
        }
        "gateway_signin_required" => {
            "Authenticate through your organization's required gateway before starting Claude Code."
        }
        "gateway_access_denied" => {
            "The configured gateway denied access. Check your gateway account and access policy."
        }
        "proxy_invalid" => "The proxy URL is invalid. Correct your proxy URL and start again.",
        "temp_dir_unusable" => {
            "The temporary directory is unusable. Repair it or select a writable temporary directory."
        }
        "cwd_unavailable" => {
            "The working directory no longer exists or cannot be read. Restore it or start from a readable directory."
        }
        "shell_tool_missing" if cfg!(windows) => {
            "Shell tooling is unavailable. Install Git Bash or enable PowerShell tooling."
        }
        "shell_tool_missing" => {
            "Shell tooling is unavailable. Install or configure a supported shell."
        }
        "session_held_by_background" => {
            "The requested session is still owned by a background process. Stop that process or allow it to finish."
        }
        "worktree_resume_refused" => {
            "Worktree safety checks rejected resume. Review the SDK guidance in diagnostic logs before resuming."
        }
        "worktree_unverified" => {
            "Worktree verification failed temporarily. A manual retry may succeed."
        }
        "cli_version_too_old" => {
            "The bundled Claude Code runtime is too old. Update claude-rs to a release containing a newer runtime."
        }
        "bypass_root" => {
            "Bypass-permissions mode cannot run as root. Use a non-root account or another permission mode."
        }
        "sandbox_required_unavailable" => {
            "The required sandbox is unavailable. Install or repair sandbox support, or have its requirement changed."
        }
        "initialize_not_first" => {
            "Claude Code did not receive initialization first. Check the host or wrapper script for input written before initialization."
        }
        "config_file_invalid" => {
            "A configuration file contains invalid JSON. Repair the configuration before starting again; diagnostic logs identify the failure."
        }
        "managed_settings_not_delivered" => {
            "Required managed settings were not delivered during initialization. Check the host or wrapper script and the SDK guidance in diagnostic logs before starting again."
        }
        _ => "Claude Code startup failed. Review diagnostic logs for details.",
    }
}

pub(super) fn handle_slash_command_error_event(app: &mut App, msg: &str) {
    if let Some(pending) = app.config.pending_session_title_change.take() {
        // A failed rename stays in its dialog, with the draft, when it is open.
        if matches!(pending.kind, crate::app::config::PendingSessionTitleChangeKind::Rename { .. })
            && app.config.session_rename_overlay().is_some()
        {
            app.config.set_overlay_error(msg);
        } else {
            app.config.last_error = Some(msg.to_owned());
            app.config.status_message = None;
        }
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
        ultracode,
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
        ChatResetKind::Replacement,
    );
    app.session_runtime.ultracode = ultracode;
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
    app.startup.complete_launch();
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

pub(crate) fn maybe_emit_fast_mode_disabled_notice(app: &mut App, previous_reason: Option<&str>) {
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
    app.finalize_session_runtime_artifacts(model::ToolCallStatus::Failed);
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

fn sync_welcome_cwd(app: &mut App) {
    app.sync_welcome_snapshot();
}

pub(super) fn apply_session_cwd(app: &mut App, cwd_raw: String) {
    app.cwd_raw = cwd_raw;
    let home = app.settings_home_override.clone().or_else(dirs::home_dir);
    app.cwd = super::super::cwd::display(std::path::Path::new(&app.cwd_raw), home.as_deref());
    app.sync_git_context();
    sync_welcome_cwd(app);
    app.reconcile_trust_state_from_preferences_and_cwd();
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

    super::super::session_picker::open(app);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, PendingUserMessage};
    use std::time::{Duration, Instant};

    #[test]
    fn startup_reasons_are_actionable_through_connection_failure_lifecycle() {
        for (reason, guidance) in [
            ("org_pin_api_key_conflict", "organization-required sign-in"),
            ("provider_not_allowed", "Organization policy disallows the selected provider"),
            ("org_verify_failed", "Check connectivity or sign in again"),
            ("org_pin_mismatch", "organization not permitted by policy"),
            ("managed_settings_invalid", "invalid or leave no usable model"),
            (
                "remote_settings_required_unavailable",
                "Required remote managed settings could not be loaded",
            ),
            ("gateway_signin_required", "Authenticate through"),
            ("org_config_required_unavailable", "Check connectivity and retry"),
            ("org_config_refused", "Sign in with an allowed account or contact your administrator"),
            ("gateway_access_denied", "gateway denied access"),
            ("proxy_invalid", "Correct your proxy URL"),
            ("temp_dir_unusable", "select a writable temporary directory"),
            ("cwd_unavailable", "no longer exists or cannot be read"),
            (
                "shell_tool_missing",
                if cfg!(windows) {
                    "Install Git Bash or enable PowerShell tooling"
                } else {
                    "supported shell"
                },
            ),
            ("session_held_by_background", "Stop that process or allow it to finish"),
            ("worktree_resume_refused", "SDK guidance in diagnostic logs"),
            ("worktree_unverified", "manual retry may succeed"),
            ("cli_version_too_old", "Update claude-rs to a release containing a newer runtime"),
            ("bypass_root", "cannot run as root"),
            ("sandbox_required_unavailable", "sandbox"),
            ("initialize_not_first", "initialization"),
            ("config_file_invalid", "JSON"),
            ("managed_settings_not_delivered", "Check the host or wrapper script"),
            ("future_reason", "Claude Code startup failed"),
        ] {
            let mut app = App::test_default();
            app.session_runtime.session_id = Some(model::SessionId::new("session-1"));
            app.pending_user_messages
                .try_push_sending(PendingUserMessage::sending(
                    "message-1".to_owned(),
                    "pending input".to_owned(),
                    Vec::new(),
                ))
                .expect("pending input");
            app.input.set_text("draft");
            let failure = crate::agent::events::ConnectionFailure {
                message: "raw SDK detail must not appear".to_owned(),
                startup_failure: Some(crate::agent::types::StartupFailure {
                    reason: reason.to_owned(),
                    errors: vec!["original SDK worktree guidance".to_owned()],
                }),
                request_id: Some("connect-1".to_owned()),
            };
            super::super::client::handle_client_event(
                &mut app,
                crate::agent::events::ClientEvent::ConnectionFailed(failure),
            );
            super::super::client::handle_client_event(
                &mut app,
                crate::agent::events::ClientEvent::FatalError(AppError::BridgeSdkFailure),
            );
            assert_eq!(app.status, AppStatus::Error);
            assert_eq!(app.exit_error, Some(AppError::BridgeSdkFailure));
            assert!(app.shutdown_requested());
            assert!(app.session_runtime.session_id.is_none());
            assert!(app.pending_user_messages.is_empty());
            assert_eq!(app.input.text(), "pending input\ndraft");
            assert_eq!(app.transcript.messages.len(), 1);
            let crate::app::MessageBlock::Text(text) = &app.transcript.messages[0].blocks[0] else {
                panic!("error message")
            };
            assert!(text.text.contains(guidance), "{reason}: {}", text.text);
            assert!(!text.text.contains("raw SDK detail"));
            assert!(!text.text.contains("original SDK worktree guidance"));
            if reason == "provider_not_allowed" {
                assert!(!text.text.contains("credentials"));
                assert!(!text.text.contains("downtime"));
            }
        }
    }

    #[test]
    fn startup_diagnostics_preserve_reason_and_bounded_redacted_sdk_guidance() {
        let mut app = App::test_default();
        let mut errors = vec![
            format!(
                "SAFE filesystem guidance ANTHROPIC_API_KEY=error-secret {} OMITTED_LONG_TAIL",
                "\u{754c}".repeat(1500)
            ),
            "SAFE shell guidance Bearer bearer-secret".into(),
        ];
        errors.extend((2..8).map(|i| format!("SAFE guidance {i}")));
        errors.push("OMITTED_NINTH_ENTRY".into());
        let records = crate::logging::test_capture::capture(|| {
            super::super::handle_client_event(
                &mut app,
                crate::agent::events::ClientEvent::ConnectionFailed(
                    crate::agent::events::ConnectionFailure {
                        message: "RAW_PRIVATE_DETAIL".into(),
                        startup_failure: Some(crate::agent::types::StartupFailure {
                            reason: "future_reason ANTHROPIC_API_KEY=reason-secret".into(),
                            errors,
                        }),
                        request_id: Some("connect-diagnostic-owner".into()),
                    },
                ),
            );
        });
        let record = records.iter().find(|r| r["event_name"] == "sdk_startup_failed").unwrap();
        assert_eq!(record["request_id"], "connect-diagnostic-owner");
        assert!(record["startup_failure_reason"].as_str().unwrap().starts_with("future_reason"));
        let values: Vec<String> =
            serde_json::from_str(record["startup_failure_errors"].as_str().unwrap()).unwrap();
        assert_eq!(values.len(), 8);
        assert!(values[0].contains("SAFE filesystem guidance"));
        assert!(values[1].contains("SAFE shell guidance"));
        for (i, value) in values.iter().enumerate().skip(2) {
            assert_eq!(value, &format!("SAFE guidance {i}"));
        }
        assert!(values.iter().all(|v| v.chars().count() <= 1024));
        assert!(values[1].chars().count() < 1024);
        assert!(record["startup_failure_reason"].as_str().unwrap().chars().count() <= 1024);
        let captured = serde_json::to_string(&records).unwrap();
        for secret in [
            "error-secret",
            "bearer-secret",
            "reason-secret",
            "OMITTED_LONG_TAIL",
            "OMITTED_NINTH_ENTRY",
            "RAW_PRIVATE_DETAIL",
        ] {
            assert!(!captured.contains(secret), "leaked {secret}");
        }
        let shown = crate::app::test_support::transcript_region(&mut app, 0);
        assert!(shown.contains("Claude Code startup failed"));
        assert!(
            !shown.contains("SAFE filesystem")
                && !shown.contains("PRIVATE")
                && !shown.contains("secret")
        );
    }

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

    #[test]
    fn rewind_outcomes_report_owned_totals_and_release_the_pending_command() {
        use crate::agent::events::ClientEvent;
        use crate::app::test_support::*;
        for skipped in [None, Some(0), Some(4)] {
            let (mut app, mut rx) = rewind_fixture();
            begin_fixture_rewind(&mut app, &mut rx, "code");
            app.input.set_text("next unsent draft");
            let mut result = rewind_outcome(skipped);
            result.session_id = "stale-session".into();
            super::super::handle_client_event(
                &mut app,
                ClientEvent::RewindResultReceived { result: result.clone() },
            );
            assert!(!app.composer_access().can_submit());
            assert!(rewind_notices(&app).is_empty());
            result.session_id = "session-1".into();
            super::super::handle_client_event(
                &mut app,
                ClientEvent::RewindResultReceived { result },
            );
            let expected = if skipped.is_some_and(|v| v > 0) {
                SystemSeverity::Warning
            } else {
                SystemSeverity::Info
            };
            let notices = rewind_notices(&app);
            assert_eq!(notices.len(), 1);
            assert_eq!(notices[0].0, expected);
            let index = notices[0].1;
            let shown = transcript_region(&mut app, index);
            for fragment in ["3 files", "11 insertions", "7 deletions"] {
                assert!(shown.contains(fragment), "{shown}");
            }
            assert_eq!(shown.contains("4 unsafe linked paths"), skipped == Some(4));
            assert_eq!(app.input.text(), "next unsent draft");
            assert!(app.composer_access().can_submit());
            assert!(rx.try_recv().is_err());
            crate::app::input_submit::submit_input(&mut app);
            expect_session_prompt(&mut rx, "session-1", "next unsent draft", &[], &[]);
        }
    }

    #[test]
    fn session_root_handoff_replaces_selectable_file_completions() {
        use crate::app::test_support::*;
        for replacement in [false, true] {
            let old = tempfile::tempdir().unwrap();
            let new = tempfile::tempdir().unwrap();
            std::fs::write(old.path().join("before.rs"), "old root").unwrap();
            std::fs::write(new.path().join("after.rs"), "new root").unwrap();
            let (mut app, mut rx) = app_with_connection();
            super::super::handle_client_event(
                &mut app,
                session_connection("session-1", old.path()),
            );
            drain_session_refreshes(&mut rx, "session-1");
            select_root_candidate(&mut app, "@before", "before.rs");
            let event = if replacement {
                session_replacement("new-root", new.path(), vec![], None)
            } else {
                session_connection("new-root", new.path())
            };
            super::super::handle_client_event(&mut app, event);
            drain_session_refreshes(&mut rx, "new-root");
            select_root_candidate(&mut app, "@after", "after.rs");
            assert!(
                app.mention.as_ref().unwrap().candidates.iter().all(|c| c.rel_path != "before.rs")
            );
            press(&mut app, crossterm::event::KeyCode::Tab);
            assert_eq!(app.input.text(), "@'after.rs' ");
            press(&mut app, crossterm::event::KeyCode::Char('!'));
            crate::app::input_submit::submit_input(&mut app);
            expect_session_prompt(&mut rx, "new-root", "@'after.rs' !", &[], &[]);
        }
    }

    fn select_root_candidate(app: &mut App, query: &str, expected: &str) {
        app.input.set_text(query);
        crate::app::mention::sync_with_cursor(app);
        wait_for(app, Duration::from_secs(5), |app| {
            app.mention
                .as_ref()
                .is_some_and(|m| m.candidates.iter().any(|c| c.rel_path == expected))
        });
        assert_eq!(app.mention.as_ref().unwrap().candidates[0].rel_path, expected);
    }

    fn rewind_fixture() -> (App, crate::agent::client::CommandReceiver) {
        use crate::app::test_support::*;
        let (mut app, mut rx) = app_with_connection();
        app.input.set_text("Original user source");
        crate::app::input_submit::submit_input(&mut app);
        expect_session_prompt(&mut rx, "session-1", "Original user source", &[], &[]);
        deliver_update(
            &mut app,
            source_chunk("Original answer preserved", "original-answer", false),
        );
        super::super::handle_client_event(
            &mut app,
            crate::agent::events::ClientEvent::TurnComplete {
                session_id: "session-1".into(),
                queued_turn_count: None,
                terminal_reason: None,
            },
        );
        drain_session_refreshes(&mut rx, "session-1");
        (app, rx)
    }

    fn begin_fixture_rewind(
        app: &mut App,
        rx: &mut crate::agent::client::CommandReceiver,
        mode: &str,
    ) {
        app.sdk_inventory.rewind_targets = vec![model::RewindTarget {
            uuid: "target-user".into(),
            first_text: "Target source".into(),
            input_text: "Target source".into(),
            index: 0,
            previous_assistant_uuid: None,
            resume_anchor_uuid: None,
        }];
        assert!(crate::app::slash::try_handle_submit(app, &format!("/rewind target-user {mode}")));
        let crate::agent::wire::BridgeCommand::Rewind {
            session_id,
            target_user_message_id,
            restore_mode,
            ..
        } = rx.try_recv().unwrap().command
        else {
            panic!("Rewind");
        };
        assert_eq!(session_id, "session-1");
        assert_eq!(target_user_message_id, "target-user");
        assert_eq!(
            restore_mode,
            if mode == "both" {
                crate::agent::types::RewindRestoreMode::Both
            } else {
                crate::agent::types::RewindRestoreMode::Code
            }
        );
        assert!(rx.try_recv().is_err());
    }

    fn rewind_outcome(skipped_links: Option<u64>) -> model::RewindResult {
        model::RewindResult {
            session_id: "session-1".into(),
            restore_mode: model::RewindRestoreMode::Code,
            status: model::RewindResultStatus::Success,
            file_result: Some(model::RewindFilesResult {
                can_rewind: true,
                error: None,
                files_changed: vec!["one.rs".into(), "two.rs".into(), "three.rs".into()],
                insertions: Some(11),
                deletions: Some(7),
                skipped_links,
            }),
            message: None,
        }
    }

    fn rewind_notices(app: &App) -> Vec<(SystemSeverity, usize)> {
        app.transcript
            .messages
            .iter()
            .enumerate()
            .flat_map(|(i, m)| {
                m.blocks.iter().filter_map(move |b| {
                    if let crate::app::MessageBlock::Notice(n) = b {
                        Some((n.severity, i))
                    } else {
                        None
                    }
                })
            })
            .collect()
    }

    #[test]
    fn partial_rewind_preserves_the_conversation_and_reports_applied_code_with_its_failure() {
        use crate::agent::events::ClientEvent;
        use crate::app::test_support::*;
        let (mut app, mut rx) = rewind_fixture();
        let original_ids: Vec<_> = app.transcript.messages.iter().map(|m| m.id).collect();
        begin_fixture_rewind(&mut app, &mut rx, "both");
        app.input.set_text("Unsent next draft");
        let draft = app.input.snapshot();
        let mut result = rewind_outcome(Some(2));
        result.restore_mode = model::RewindRestoreMode::Both;
        result.status = model::RewindResultStatus::PartialFailure;
        result.session_id = "stale-session".into();
        result.message = Some("STALE partial failure".into());
        super::super::handle_client_event(
            &mut app,
            ClientEvent::RewindResultReceived { result: result.clone() },
        );
        assert!(rewind_notices(&app).is_empty());
        assert!(!app.composer_access().can_submit());
        result.session_id = "session-1".into();
        result.message = Some(
            "Code was restored, but conversation commit failed: fixture commit refused".into(),
        );
        super::super::handle_client_event(&mut app, ClientEvent::RewindResultReceived { result });
        assert_eq!(app.session_runtime.session_id.as_ref().unwrap().as_str(), "session-1");
        assert_eq!(app.input.snapshot(), draft);
        assert_eq!(
            app.transcript
                .messages
                .iter()
                .take(original_ids.len())
                .map(|m| m.id)
                .collect::<Vec<_>>(),
            original_ids
        );
        let notices = rewind_notices(&app);
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].0, SystemSeverity::Error);
        let shown = transcript_region(&mut app, notices[0].1);
        for fragment in
            ["Code was restored", "conversation commit failed", "fixture commit refused"]
        {
            assert!(shown.contains(fragment), "{shown}");
        }
        assert!(!shown.contains("STALE"));
        assert!(rx.try_recv().is_err());
        assert!(app.composer_access().can_submit());
        assert!(app.transcript.messages.iter().any(|m|m.blocks.iter().any(|b|matches!(b,crate::app::MessageBlock::Text(t) if t.text == "Original answer preserved"))));
        crate::app::input_submit::submit_input(&mut app);
        expect_session_prompt(&mut rx, "session-1", "Unsent next draft", &[], &[]);
    }
}
