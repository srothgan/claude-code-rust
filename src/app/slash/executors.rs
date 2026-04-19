// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

//! Slash command executors: dispatching parsed commands to their handler functions.

use super::{
    parse, push_system_message, push_user_message, require_active_session, require_connection,
    set_command_pending,
};
use crate::agent::events::ClientEvent;
use crate::app::config::{self, SettingFile, store};
use crate::app::connect::{SessionStartReason, begin_resume_session, start_new_session};
use crate::app::events::push_system_message_with_severity;
use crate::app::{App, AppStatus, CancelOrigin, SystemSeverity};
use std::fmt::Write as _;

const ONE_M_CONTEXT_USAGE: &str = "Usage: /1m-context <enable|disable|status>";
const OPUS_VERSION_USAGE: &str = "Usage: /opus-version <4.5|4.6|4.7|default|status>";
const OPUS_4_5_MODEL_ID: &str = "claude-opus-4-5-20251101";
const OPUS_4_6_MODEL_ID: &str = "claude-opus-4-6";
const OPUS_4_7_MODEL_ID: &str = "claude-opus-4-7";

/// Handle slash command submission.
///
/// Returns `true` if the slash input was fully handled and should not be sent as a prompt.
/// Returns `false` when the input should continue through the normal prompt path.
pub fn try_handle_submit(app: &mut App, text: &str) -> bool {
    let Some(parsed) = parse(text) else {
        return false;
    };

    match parsed.name {
        "/1m-context" => handle_1m_context_submit(app, &parsed.args),
        "/cancel" => handle_cancel_submit(app),
        "/compact" => handle_compact_submit(app, &parsed.args),
        "/config" => handle_config_submit(app, &parsed.args),
        "/docs" => handle_docs_submit(app, &parsed.args),
        "/mcp" => handle_mcp_submit(app, &parsed.args),
        "/plugins" => handle_plugins_submit(app, &parsed.args),
        "/opus-version" => handle_opus_version_submit(app, &parsed.args),
        "/status" => handle_status_submit(app, &parsed.args),
        "/usage" => handle_usage_submit(app, &parsed.args),
        "/login" => handle_login_submit(app, &parsed.args),
        "/logout" => handle_logout_submit(app, &parsed.args),
        "/mode" => handle_mode_submit(app, &parsed.args),
        "/model" => handle_model_submit(app, &parsed.args),
        "/new-session" => handle_new_session_submit(app, &parsed.args),
        "/resume" => handle_resume_submit(app, &parsed.args),
        _ => handle_unknown_submit(app, parsed.name),
    }
}

fn opus_model_id_for_version(version: &str) -> Option<&'static str> {
    match version {
        "4.5" => Some(OPUS_4_5_MODEL_ID),
        "4.6" => Some(OPUS_4_6_MODEL_ID),
        "4.7" => Some(OPUS_4_7_MODEL_ID),
        _ => None,
    }
}

fn opus_version_label_for_model_id(model_id: &str) -> Option<&'static str> {
    match model_id {
        OPUS_4_5_MODEL_ID => Some("4.5"),
        OPUS_4_6_MODEL_ID => Some("4.6"),
        OPUS_4_7_MODEL_ID => Some("4.7"),
        _ => None,
    }
}

fn handle_opus_version_submit(app: &mut App, args: &[&str]) -> bool {
    let [subcommand] = args else {
        push_system_message(app, OPUS_VERSION_USAGE);
        return true;
    };
    let subcommand = subcommand.trim();
    if subcommand.is_empty() || args.len() != 1 {
        push_system_message(app, OPUS_VERSION_USAGE);
        return true;
    }

    match subcommand {
        "status" => {
            match current_opus_version_pin(app) {
                Ok(Some(model_id)) => {
                    let message = if let Some(version) = opus_version_label_for_model_id(&model_id)
                    {
                        format!(
                            "Opus is pinned to {version} in this folder via `.claude/settings.local.json`."
                        )
                    } else {
                        format!(
                            "Opus is pinned to {model_id} in this folder via `.claude/settings.local.json`."
                        )
                    };
                    push_system_message_with_severity(app, Some(SystemSeverity::Info), &message);
                }
                Ok(None) => push_system_message_with_severity(
                    app,
                    Some(SystemSeverity::Info),
                    "Opus is using the default alias resolution in this folder.",
                ),
                Err(err) => {
                    push_system_message(app, format!("Failed to read /opus-version status: {err}"));
                }
            }
            true
        }
        "default" => {
            if let Err(err) = set_opus_version_pin(app, None) {
                push_system_message(app, format!("Failed to run /opus-version default: {err}"));
            }
            true
        }
        _ => {
            let Some(model_id) = opus_model_id_for_version(subcommand) else {
                push_system_message(app, OPUS_VERSION_USAGE);
                return true;
            };
            if let Err(err) = set_opus_version_pin(app, Some(model_id)) {
                push_system_message(
                    app,
                    format!("Failed to run /opus-version {subcommand}: {err}"),
                );
            }
            true
        }
    }
}

fn handle_1m_context_submit(app: &mut App, args: &[&str]) -> bool {
    let [subcommand] = args else {
        push_system_message(app, ONE_M_CONTEXT_USAGE);
        return true;
    };
    let subcommand = subcommand.trim();
    if subcommand.is_empty() || args.len() != 1 {
        push_system_message(app, ONE_M_CONTEXT_USAGE);
        return true;
    }

    match subcommand {
        "status" => {
            match current_1m_context_disabled(app) {
                Ok(true) => push_system_message_with_severity(
                    app,
                    Some(SystemSeverity::Info),
                    "1M context is disabled for future sessions in this folder via `.claude/settings.local.json`.",
                ),
                Ok(false) => push_system_message_with_severity(
                    app,
                    Some(SystemSeverity::Info),
                    "1M context is enabled for future sessions in this folder.",
                ),
                Err(err) => {
                    push_system_message(app, format!("Failed to read /1m-context status: {err}"));
                }
            }
            true
        }
        "disable" => {
            if let Err(err) = set_1m_context_disabled(app, true) {
                push_system_message(app, format!("Failed to run /1m-context disable: {err}"));
            }
            true
        }
        "enable" => {
            if let Err(err) = set_1m_context_disabled(app, false) {
                push_system_message(app, format!("Failed to run /1m-context enable: {err}"));
            }
            true
        }
        _ => {
            push_system_message(app, ONE_M_CONTEXT_USAGE);
            true
        }
    }
}

fn current_1m_context_disabled(app: &mut App) -> Result<bool, String> {
    config::initialize_shared_state(app)?;
    store::disable_1m_context(&app.config.committed_local_settings_document).map_err(|()| {
        "Expected `.claude/settings.local.json` env.CLAUDE_CODE_DISABLE_1M_CONTEXT to be a string"
            .to_owned()
    })
}

fn set_1m_context_disabled(app: &mut App, disabled: bool) -> Result<(), String> {
    if !app.is_project_trusted() {
        return Err(
            "Project trust must be accepted before editing folder-local 1M context settings"
                .to_owned(),
        );
    }

    config::initialize_shared_state(app)?;
    let Some(path) = app.config.path_for(SettingFile::LocalSettings).cloned() else {
        return Err("Local settings path is not available".to_owned());
    };

    let current = store::disable_1m_context(&app.config.committed_local_settings_document)
        .map_err(|()| {
            "Expected `.claude/settings.local.json` env.CLAUDE_CODE_DISABLE_1M_CONTEXT to be a string"
                .to_owned()
        })?;

    let mut next_document = app.config.committed_local_settings_document.clone();
    store::set_disable_1m_context(&mut next_document, disabled);
    store::save(&path, &next_document)?;
    app.config.committed_local_settings_document = next_document;
    app.reconcile_runtime_from_persisted_settings_change();
    app.config.last_error = None;

    let message = match (disabled, current == disabled) {
        (true, true) => {
            "1M context is already disabled for future sessions in this folder. Run /new-session to apply it."
        }
        (true, false) => {
            "Disabled 1M context for future sessions in this folder. Run /new-session to apply it."
        }
        (false, true) => {
            "1M context is already enabled for future sessions in this folder. Run /new-session to apply it."
        }
        (false, false) => {
            "Enabled 1M context for future sessions in this folder. Run /new-session to apply it."
        }
    };
    push_system_message_with_severity(app, Some(SystemSeverity::Info), message);
    Ok(())
}

fn current_opus_version_pin(app: &mut App) -> Result<Option<String>, String> {
    config::initialize_shared_state(app)?;
    store::opus_version_pin(&app.config.committed_local_settings_document).map_err(|()| {
        "Expected `.claude/settings.local.json` env.ANTHROPIC_DEFAULT_OPUS_MODEL to be a string"
            .to_owned()
    })
}

fn set_opus_version_pin(app: &mut App, model: Option<&str>) -> Result<(), String> {
    if !app.is_project_trusted() {
        return Err(
            "Project trust must be accepted before editing folder-local Opus version settings"
                .to_owned(),
        );
    }

    config::initialize_shared_state(app)?;
    let Some(path) = app.config.path_for(SettingFile::LocalSettings).cloned() else {
        return Err("Local settings path is not available".to_owned());
    };

    let current =
        store::opus_version_pin(&app.config.committed_local_settings_document).map_err(|()| {
            "Expected `.claude/settings.local.json` env.ANTHROPIC_DEFAULT_OPUS_MODEL to be a string"
                .to_owned()
        })?;

    let mut next_document = app.config.committed_local_settings_document.clone();
    store::set_opus_version_pin(&mut next_document, model);
    store::save(&path, &next_document)?;
    app.config.committed_local_settings_document = next_document;
    app.reconcile_runtime_from_persisted_settings_change();
    app.config.last_error = None;

    let message = match (model, current.as_deref()) {
        (Some(next_model), Some(current_model)) if current_model == next_model => {
            let version = opus_version_label_for_model_id(next_model).unwrap_or(next_model);
            format!(
                "Opus is already pinned to {version} for future sessions in this folder. Run /new-session to apply it."
            )
        }
        (Some(next_model), _) => {
            let version = opus_version_label_for_model_id(next_model).unwrap_or(next_model);
            format!(
                "Pinned Opus to {version} for future sessions in this folder. Run /new-session to apply it."
            )
        }
        (None, None) => "Opus is already using the default alias in this folder.".to_owned(),
        (None, Some(_)) => {
            "Cleared the project-local Opus version pin for future sessions in this folder. Run /new-session to apply it.".to_owned()
        }
    };
    push_system_message_with_severity(app, Some(SystemSeverity::Info), &message);
    Ok(())
}

fn handle_cancel_submit(app: &mut App) -> bool {
    if !matches!(app.status, AppStatus::Thinking | AppStatus::Running) {
        return true;
    }
    if let Err(message) = crate::app::input_submit::request_cancel(app, CancelOrigin::Manual) {
        push_system_message(app, format!("Failed to run /cancel: {message}"));
    }
    true
}

fn handle_compact_submit(app: &mut App, args: &[&str]) -> bool {
    if !args.is_empty() {
        push_system_message(app, "Usage: /compact");
        return true;
    }
    if require_active_session(
        app,
        "Cannot compact: not connected yet.",
        "Cannot compact: no active session.",
    )
    .is_none()
    {
        return true;
    }

    app.is_compacting = true;
    false
}

fn handle_config_submit(app: &mut App, args: &[&str]) -> bool {
    if !args.is_empty() {
        push_system_message(app, "Usage: /config");
        return true;
    }

    if let Err(err) = crate::app::config::open(app) {
        push_system_message(app, format!("Failed to open settings: {err}"));
    }
    true
}

fn handle_docs_submit(app: &mut App, args: &[&str]) -> bool {
    let topic = match args {
        [topic] if !topic.trim().is_empty() => topic.trim(),
        _ => {
            push_system_message(app, docs_usage());
            return true;
        }
    };

    let body = match topic {
        "mode" => build_docs_mode_markdown(app),
        "models" => build_docs_models_markdown(app),
        "shortcuts" => build_docs_shortcuts_markdown(app),
        "commands" => build_docs_commands_markdown(app),
        "agents" => build_docs_agents_markdown(app),
        other => {
            push_system_message(app, format!("Unknown docs topic: {other}\n{}", docs_usage()));
            return true;
        }
    };

    push_system_message_with_severity(app, Some(SystemSeverity::Info), &body);
    true
}

fn handle_plugins_submit(app: &mut App, args: &[&str]) -> bool {
    let _ = args;

    if let Err(err) = crate::app::config::open(app) {
        push_system_message(app, format!("Failed to open plugins: {err}"));
        return true;
    }
    crate::app::config::activate_tab(app, crate::app::ConfigTab::Plugins);
    true
}

fn handle_mcp_submit(app: &mut App, args: &[&str]) -> bool {
    if !args.is_empty() {
        push_system_message(app, "Usage: /mcp");
        return true;
    }

    if let Err(err) = crate::app::config::open(app) {
        push_system_message(app, format!("Failed to open MCP: {err}"));
        return true;
    }
    crate::app::config::activate_tab(app, crate::app::ConfigTab::Mcp);
    true
}

fn handle_status_submit(app: &mut App, args: &[&str]) -> bool {
    if !args.is_empty() {
        push_system_message(app, "Usage: /status");
        return true;
    }

    if let Err(err) = crate::app::config::open(app) {
        push_system_message(app, format!("Failed to open status: {err}"));
        return true;
    }
    crate::app::config::activate_tab(app, crate::app::ConfigTab::Status);
    true
}

fn handle_usage_submit(app: &mut App, args: &[&str]) -> bool {
    if !args.is_empty() {
        push_system_message(app, "Usage: /usage");
        return true;
    }

    if let Err(err) = crate::app::config::open(app) {
        push_system_message(app, format!("Failed to open usage: {err}"));
        return true;
    }
    crate::app::config::activate_tab(app, crate::app::ConfigTab::Usage);
    true
}

fn handle_login_submit(app: &mut App, args: &[&str]) -> bool {
    if !args.is_empty() {
        push_system_message(app, "Usage: /login");
        return true;
    }

    push_user_message(app, "/login");
    tracing::debug!(
        target: crate::logging::targets::APP_AUTH,
        event_name = "login_command_requested",
        message = "login slash command requested",
        outcome = "start",
    );

    if crate::app::auth::has_credentials() {
        push_system_message_with_severity(
            app,
            Some(SystemSeverity::Info),
            "Already authenticated. Use /logout first to re-authenticate.",
        );
        return true;
    }

    let Some(claude_path) = resolve_claude_cli(app, "login") else {
        return true;
    };

    set_command_pending(app, "Authenticating...", None);

    let tx = app.event_tx.clone();
    let conn = app.conn.clone();
    tokio::task::spawn_local(async move {
        tracing::debug!(
            target: crate::logging::targets::APP_AUTH,
            event_name = "auth_terminal_suspended",
            message = "terminal suspended for login command",
            outcome = "start",
            auth_command = "login",
        );
        crate::app::suspend_terminal();

        let result = tokio::process::Command::new(&claude_path)
            .args(["auth", "login"])
            .stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .status()
            .await;

        crate::app::resume_terminal();

        match result {
            Ok(status) => {
                tracing::debug!(
                    target: crate::logging::targets::APP_AUTH,
                    event_name = "auth_command_completed",
                    message = "login command completed",
                    outcome = if status.success() { "success" } else { "failure" },
                    auth_command = "login",
                    success = status.success(),
                    exit_code = ?status.code(),
                );
                if status.success() {
                    if !crate::app::auth::has_credentials() {
                        let _ = tx.send(ClientEvent::SlashCommandError(
                            "Login exited successfully but no credentials were saved. \
                             Try /login again or run `claude auth login` in another terminal."
                                .to_owned(),
                        ));
                        return;
                    }
                    if let Some(conn) = conn {
                        let _ = tx.send(ClientEvent::AuthCompleted { conn });
                    } else {
                        let _ = tx.send(ClientEvent::SlashCommandError(
                            "Login succeeded but no connection available to start a session."
                                .to_owned(),
                        ));
                    }
                } else {
                    let _ = tx.send(ClientEvent::SlashCommandError(format!(
                        "/login failed (exit code: {})",
                        status.code().map_or("unknown".to_owned(), |c| c.to_string())
                    )));
                }
            }
            Err(e) => {
                let _ = tx.send(ClientEvent::SlashCommandError(format!(
                    "Failed to run claude auth login: {e}"
                )));
            }
        }
    });
    true
}

fn handle_logout_submit(app: &mut App, args: &[&str]) -> bool {
    if !args.is_empty() {
        push_system_message(app, "Usage: /logout");
        return true;
    }

    push_user_message(app, "/logout");
    tracing::debug!(
        target: crate::logging::targets::APP_AUTH,
        event_name = "logout_command_requested",
        message = "logout slash command requested",
        outcome = "start",
    );

    if !crate::app::auth::has_credentials() {
        push_system_message_with_severity(
            app,
            Some(SystemSeverity::Info),
            "Not currently authenticated. Nothing to log out from.",
        );
        return true;
    }

    let Some(claude_path) = resolve_claude_cli(app, "logout") else {
        return true;
    };

    set_command_pending(app, "Signing out...", None);

    let tx = app.event_tx.clone();
    tokio::task::spawn_local(async move {
        tracing::debug!(
            target: crate::logging::targets::APP_AUTH,
            event_name = "auth_terminal_suspended",
            message = "terminal suspended for logout command",
            outcome = "start",
            auth_command = "logout",
        );
        crate::app::suspend_terminal();

        let result = tokio::process::Command::new(&claude_path)
            .args(["auth", "logout"])
            .stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .status()
            .await;

        crate::app::resume_terminal();

        match result {
            Ok(status) => {
                tracing::debug!(
                    target: crate::logging::targets::APP_AUTH,
                    event_name = "auth_command_completed",
                    message = "logout command completed",
                    outcome = if status.success() { "success" } else { "failure" },
                    auth_command = "logout",
                    success = status.success(),
                    exit_code = ?status.code(),
                );
                if status.success() {
                    if crate::app::auth::has_credentials() {
                        let _ = tx.send(ClientEvent::SlashCommandError(
                            "Logout exited successfully but credentials are still present. \
                             Try /logout again or run `claude auth logout` in another terminal."
                                .to_owned(),
                        ));
                        return;
                    }
                    let _ = tx.send(ClientEvent::LogoutCompleted);
                } else {
                    let _ = tx.send(ClientEvent::SlashCommandError(format!(
                        "/logout failed (exit code: {})",
                        status.code().map_or("unknown".to_owned(), |c| c.to_string())
                    )));
                }
            }
            Err(e) => {
                let _ = tx.send(ClientEvent::SlashCommandError(format!(
                    "Failed to run claude auth logout: {e}"
                )));
            }
        }
    });
    true
}

/// Resolve the `claude` CLI binary from PATH, or push an error message and return `None`.
fn resolve_claude_cli(app: &mut App, subcommand: &str) -> Option<std::path::PathBuf> {
    if let Ok(path) = which::which("claude") {
        tracing::debug!(
            target: crate::logging::targets::APP_AUTH,
            event_name = "auth_cli_resolved",
            message = "resolved claude CLI binary",
            outcome = "success",
            auth_command = subcommand,
            path = %path.display(),
        );
        Some(path)
    } else {
        push_system_message(
            app,
            format!(
                "claude CLI not found in PATH. Install it and retry /{subcommand}, \
                 or run `claude auth {subcommand}` manually in another terminal."
            ),
        );
        None
    }
}

fn handle_mode_submit(app: &mut App, args: &[&str]) -> bool {
    let [requested_mode_arg] = args else {
        push_system_message(app, "Usage: /mode <id>");
        return true;
    };
    let requested_mode = requested_mode_arg.trim();
    if requested_mode.is_empty() {
        push_system_message(app, "Usage: /mode <id>");
        return true;
    }

    let Some((conn, sid)) = require_active_session(
        app,
        "Cannot switch mode: not connected yet.",
        "Cannot switch mode: no active session.",
    ) else {
        return true;
    };

    if let Some(ref mode) = app.mode
        && !mode.available_modes.iter().any(|m| m.id == requested_mode)
    {
        push_system_message(app, format!("Unknown mode: {requested_mode}"));
        return true;
    }

    set_command_pending(app, "Switching mode...", Some(crate::app::PendingCommandAck::CurrentMode));

    let tx = app.event_tx.clone();
    let requested_mode_owned = requested_mode.to_owned();
    tokio::task::spawn_local(async move {
        match conn.set_mode(sid.to_string(), requested_mode_owned) {
            Ok(()) => {}
            Err(e) => {
                let _ =
                    tx.send(ClientEvent::SlashCommandError(format!("Failed to run /mode: {e}")));
            }
        }
    });
    true
}

fn handle_model_submit(app: &mut App, args: &[&str]) -> bool {
    let [model_name_arg] = args else {
        push_system_message(app, "Usage: /model <id>");
        return true;
    };
    let model_name = model_name_arg.trim();
    if model_name.is_empty() {
        push_system_message(app, "Usage: /model <id>");
        return true;
    }

    let Some((conn, sid)) = require_active_session(
        app,
        "Cannot switch model: not connected yet.",
        "Cannot switch model: no active session.",
    ) else {
        return true;
    };

    if !app.available_models.is_empty()
        && !app.available_models.iter().any(|candidate| candidate.id == model_name)
    {
        push_system_message(app, format!("Unknown model: {model_name}"));
        return true;
    }

    set_command_pending(
        app,
        "Switching model...",
        Some(crate::app::PendingCommandAck::CurrentModel),
    );

    let tx = app.event_tx.clone();
    let model_name = model_name.to_owned();
    tokio::task::spawn_local(async move {
        match conn.set_model(sid.to_string(), model_name) {
            Ok(()) => {}
            Err(e) => {
                let _ =
                    tx.send(ClientEvent::SlashCommandError(format!("Failed to run /model: {e}")));
            }
        }
    });
    true
}

fn handle_new_session_submit(app: &mut App, args: &[&str]) -> bool {
    if !args.is_empty() {
        push_system_message(app, "Usage: /new-session");
        return true;
    }

    push_user_message(app, "/new-session");

    let Some(conn) = require_connection(app, "Cannot create new session: not connected yet.")
    else {
        return true;
    };

    set_command_pending(app, "Starting new session...", None);

    if let Err(e) = start_new_session(app, &conn, SessionStartReason::NewSession) {
        let _ = app
            .event_tx
            .send(ClientEvent::SlashCommandError(format!("Failed to run /new-session: {e}")));
    }
    true
}

fn handle_resume_submit(app: &mut App, args: &[&str]) -> bool {
    let [session_id_arg] = args else {
        push_system_message(app, "Usage: /resume <session_id>");
        return true;
    };
    let session_id = session_id_arg.trim();
    if session_id.is_empty() {
        push_system_message(app, "Usage: /resume <session_id>");
        return true;
    }

    push_user_message(app, format!("/resume {session_id}"));
    let Some(conn) = require_connection(app, "Cannot resume session: not connected yet.") else {
        return true;
    };

    set_command_pending(app, &format!("Resuming session {session_id}..."), None);
    let session_id = session_id.to_owned();
    if let Err(e) = begin_resume_session(app, &conn, session_id) {
        let _ = app
            .event_tx
            .send(ClientEvent::SlashCommandError(format!("Failed to run /resume: {e}")));
    }
    true
}

fn handle_unknown_submit(app: &mut App, command_name: &str) -> bool {
    if super::candidates::is_supported_command(app, command_name) {
        return false;
    }
    push_system_message(app, format!("{command_name} is not yet supported"));
    true
}

fn docs_usage() -> &'static str {
    "Usage: /docs <mode|models|shortcuts|commands|agents>"
}

fn build_docs_mode_markdown(app: &App) -> String {
    let rows = app.mode.as_ref().map_or_else(
        || vec![("Unavailable".to_owned(), "Connect to load the current session mode.".to_owned())],
        |mode| {
            let mut rows: Vec<(String, String)> = mode
                .available_modes
                .iter()
                .map(|entry| {
                    let mut details = format!("ID `{}`", entry.id);
                    if entry.id == mode.current_mode_id {
                        details.push_str("; current");
                    }
                    (entry.name.clone(), details)
                })
                .collect();
            if rows.is_empty() {
                rows.push((
                    mode.current_mode_name.clone(),
                    format!("ID `{}`; current", mode.current_mode_id),
                ));
            }
            rows
        },
    );

    render_docs_table(
        "Docs: Mode",
        "Current and available session modes.",
        ("Mode", "Details"),
        rows,
    )
}

fn build_docs_models_markdown(app: &App) -> String {
    let rows = if app.available_models.is_empty() {
        vec![("Unavailable".to_owned(), "Connect to load advertised models.".to_owned())]
    } else {
        app.available_models
            .iter()
            .map(|model| {
                let name = if model.display_name.trim().is_empty() {
                    model.id.clone()
                } else {
                    model.display_name.clone()
                };
                (name, model_details(model))
            })
            .collect()
    };

    render_docs_table(
        "Docs: Models",
        "Advertised models and capabilities for the current session.",
        ("Model", "Details"),
        rows,
    )
}

fn build_docs_shortcuts_markdown(app: &App) -> String {
    render_docs_table(
        "Docs: Shortcuts",
        "Live keyboard shortcuts for the current app state.",
        ("Shortcut", "Action"),
        crate::ui::help::key_help_items(app),
    )
}

fn build_docs_commands_markdown(app: &App) -> String {
    render_docs_table(
        "Docs: Commands",
        "App-owned and advertised slash commands.",
        ("Command", "Description"),
        crate::ui::help::docs_command_items(app),
    )
}

fn build_docs_agents_markdown(app: &App) -> String {
    render_docs_table(
        "Docs: Agents",
        "Advertised subagents for the current session.",
        ("Agent", "Description"),
        crate::ui::help::subagent_help_items(app),
    )
}

fn model_details(model: &crate::agent::model::AvailableModel) -> String {
    let mut parts = Vec::new();
    parts.push(format!("ID `{}`", model.id));
    if let Some(description) = model.description.as_deref()
        && !description.trim().is_empty()
    {
        parts.push(description.trim().to_owned());
    }
    if model.supports_effort {
        parts.push("Effort".to_owned());
    }
    if model.supports_adaptive_thinking == Some(true) {
        parts.push("Adaptive thinking".to_owned());
    }
    if model.supports_fast_mode == Some(true) {
        parts.push("Fast mode".to_owned());
    }
    if model.supports_auto_mode == Some(true) {
        parts.push("Auto mode".to_owned());
    }
    parts.join("; ")
}

fn render_docs_table(
    title: &str,
    intro: &str,
    headers: (&str, &str),
    rows: Vec<(String, String)>,
) -> String {
    let mut markdown = String::new();
    let _ = writeln!(&mut markdown, "# {title}");
    let _ = writeln!(&mut markdown);
    let _ = writeln!(&mut markdown, "{intro}");
    let _ = writeln!(&mut markdown);
    let _ = writeln!(&mut markdown, "| {} | {} |", headers.0, headers.1);
    let _ = writeln!(&mut markdown, "| --- | --- |");
    for (left, right) in rows {
        let _ = writeln!(
            &mut markdown,
            "| {} | {} |",
            markdown_table_cell(&left),
            markdown_table_cell(&right),
        );
    }
    markdown
}

fn markdown_table_cell(value: &str) -> String {
    value.trim().replace('|', "\\|").replace('\r', "").replace('\n', " - ")
}
