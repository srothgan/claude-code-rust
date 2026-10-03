// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

//! Slash command executors: dispatching parsed commands to their handler functions.

use super::{
    AppSlashCommand, ResolvedSubmission, SubmissionClass, push_system_message, push_user_message,
    require_active_session, require_connection, set_command_pending,
};
use crate::agent::events::ClientEvent;
use crate::agent::types::RewindRestoreMode;
use crate::app::connect::{
    SessionStartReason, begin_resume_session, begin_rewind, start_new_session,
};
use crate::app::events::push_submission_feedback;
use crate::app::{
    App, AppStatus, FullscreenView, ReleaseReason, SessionPickerState, SystemSeverity, view,
};
use std::fmt::Write as _;
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use tokio::sync::mpsc;

/// Handle slash command submission.
///
/// Returns `true` if the slash input was fully handled and should not be sent as a prompt.
/// Returns `false` when the input should continue through the normal prompt path.
pub(crate) fn try_handle_submission(app: &mut App, submission: &ResolvedSubmission) -> bool {
    let ResolvedSubmission::Slash { text, name, args, command, .. } = submission else {
        return false;
    };
    let args = args.iter().map(String::as_str).collect::<Vec<_>>();

    let Some(command) = command else {
        return handle_unknown_submit(app, name);
    };
    if submission.class() == SubmissionClass::Invalid {
        push_system_message(app, command.usage());
        return true;
    }

    match command {
        AppSlashCommand::Btw => handle_btw_submit(app, text),
        AppSlashCommand::Cancel => handle_cancel_submit(app),
        AppSlashCommand::Compact => handle_compact_submit(app),
        AppSlashCommand::Config => handle_config_submit(app),
        AppSlashCommand::Copy => {
            crate::app::copy::open(app);
            true
        }
        AppSlashCommand::Docs => handle_docs_submit(app, &args),
        AppSlashCommand::Agent => handle_agent_submit(app, &args),
        AppSlashCommand::Effort => handle_effort_submit(app, &args),
        AppSlashCommand::Thinking => handle_thinking_submit(app, &args),
        AppSlashCommand::Ultracode => handle_ultracode_submit(app, &args),
        AppSlashCommand::Fast => handle_fast_submit(app, &args),
        AppSlashCommand::Help => handle_help_submit(app),
        AppSlashCommand::Mcp => handle_mcp_submit(app),
        AppSlashCommand::Plugins => handle_plugins_submit(app),
        AppSlashCommand::Status => handle_status_submit(app),
        AppSlashCommand::Usage => handle_usage_submit(app),
        AppSlashCommand::Login => handle_login_submit(app),
        AppSlashCommand::Logout => handle_logout_submit(app),
        AppSlashCommand::Mode => handle_mode_submit(app, &args),
        AppSlashCommand::Model => handle_model_submit(app, &args),
        AppSlashCommand::NewSession => handle_new_session_submit(app),
        AppSlashCommand::Resume => handle_resume_submit(app, &args),
        AppSlashCommand::Rewind => handle_rewind_submit(app, &args),
    }
}

fn handle_btw_submit(app: &mut App, text: &str) -> bool {
    // Submission classification has already validated the non-empty remainder.
    let question =
        text.trim_start().strip_prefix("/btw").map(str::trim).unwrap_or_default().to_owned();

    let Some(_) = require_active_session(
        app,
        "Cannot ask a side question before connecting.",
        "Cannot ask a side question without an active session.",
    ) else {
        return true;
    };
    let btw_id = uuid::Uuid::new_v4().to_string();
    if app.btw.try_push(btw_id, question).is_err() {
        push_system_message(
            app,
            format!(
                "Too many outstanding BTW questions (maximum {}). Wait for one to finish.",
                crate::app::state::BtwRequests::CAPACITY
            ),
        );
        return true;
    }

    crate::app::btw::dispatch_next(app);
    true
}

#[cfg(test)]
pub fn try_handle_submit(app: &mut App, text: &str) -> bool {
    try_handle_submission(app, &ResolvedSubmission::resolve(text.to_owned()))
}

fn handle_cancel_submit(app: &mut App) -> bool {
    if !matches!(app.status, AppStatus::Thinking | AppStatus::Running) {
        return true;
    }
    if let Err(message) = crate::app::input_submit::request_cancel(app) {
        push_system_message(app, format!("Failed to run /cancel: {message}"));
    }
    true
}

fn handle_compact_submit(app: &mut App) -> bool {
    if require_active_session(
        app,
        "Cannot compact: not connected yet.",
        "Cannot compact: no active session.",
    )
    .is_none()
    {
        return true;
    }

    app.turn.compaction.begin_manual();
    false
}

fn handle_config_submit(app: &mut App) -> bool {
    if let Err(err) = crate::app::config::open(app) {
        push_system_message(app, format!("Failed to open settings: {err}"));
    }
    true
}

fn handle_help_submit(app: &mut App) -> bool {
    if let Err(err) = crate::app::config::open_tab(app, crate::app::ConfigTab::Help) {
        push_system_message(app, format!("Failed to open help: {err}"));
        return true;
    }
    true
}

fn handle_docs_submit(app: &mut App, args: &[&str]) -> bool {
    let [topic] = args else {
        unreachable!("validated /docs arguments must contain one topic");
    };

    let body = match *topic {
        "mode" => build_docs_mode_markdown(app),
        "models" => build_docs_models_markdown(app),
        "shortcuts" => build_docs_shortcuts_markdown(app),
        "commands" => build_docs_commands_markdown(app),
        "agents" => build_docs_agents_markdown(app),
        _ => unreachable!("validated /docs topic must be supported"),
    };

    push_submission_feedback(app, SystemSeverity::Info, &body);
    true
}

fn handle_plugins_submit(app: &mut App) -> bool {
    if let Err(err) = crate::app::config::open_tab(app, crate::app::ConfigTab::Plugins) {
        push_system_message(app, format!("Failed to open plugins: {err}"));
        return true;
    }
    true
}

fn handle_mcp_submit(app: &mut App) -> bool {
    if let Err(err) = crate::app::config::open_tab(app, crate::app::ConfigTab::Mcp) {
        push_system_message(app, format!("Failed to open MCP: {err}"));
        return true;
    }
    true
}

fn handle_status_submit(app: &mut App) -> bool {
    if let Err(err) = crate::app::config::open_tab(app, crate::app::ConfigTab::Status) {
        push_system_message(app, format!("Failed to open status: {err}"));
        return true;
    }
    true
}

fn handle_usage_submit(app: &mut App) -> bool {
    if let Err(err) = crate::app::config::open_tab(app, crate::app::ConfigTab::Usage) {
        push_system_message(app, format!("Failed to open usage: {err}"));
        return true;
    }
    true
}

fn handle_login_submit(app: &mut App) -> bool {
    push_user_message(app, "/login");
    tracing::debug!(
        target: crate::logging::targets::APP_AUTH,
        event_name = "login_command_requested",
        message = "login slash command requested",
        outcome = "start",
    );

    if crate::app::auth::has_credentials() {
        push_submission_feedback(
            app,
            SystemSeverity::Info,
            "Already authenticated. Use /logout first to re-authenticate.",
        );
        return true;
    }

    let Some(claude_path) = resolve_claude_cli(app, "login") else {
        return true;
    };

    set_command_pending(app, "Authenticating...", None);

    let tx = app.event_tx.clone();
    let conn = app.session_runtime.conn.clone();
    tokio::task::spawn_local(async move {
        tracing::debug!(
            target: crate::logging::targets::APP_AUTH,
            event_name = "auth_terminal_suspended",
            message = "terminal suspended for login command",
            outcome = "start",
            auth_command = "login",
        );
        match run_auth_child_command(&tx, &claude_path, "login").await {
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
                        let _ = tx
                            .send(ClientEvent::SlashCommandError {
                                session_id: None,
                                message: "Login exited successfully but no credentials were saved. \
                                          Try /login again or run `claude auth login` in another terminal."
                                    .to_owned(),
                            })
                            .await;
                        return;
                    }
                    if let Some(conn) = conn {
                        let _ = tx.send(ClientEvent::AuthCompleted { conn }).await;
                    } else {
                        let _ = tx
                            .send(ClientEvent::SlashCommandError {
                                session_id: None,
                                message:
                                    "Login succeeded but no connection available to start a session."
                                        .to_owned(),
                            })
                            .await;
                    }
                } else {
                    let _ = tx
                        .send(ClientEvent::SlashCommandError {
                            session_id: None,
                            message: format!(
                                "/login failed (exit code: {})",
                                status.code().map_or("unknown".to_owned(), |c| c.to_string())
                            ),
                        })
                        .await;
                }
            }
            Err(message) => {
                let _ = tx.send(ClientEvent::SlashCommandError { session_id: None, message }).await;
            }
        }
    });
    true
}

fn handle_logout_submit(app: &mut App) -> bool {
    push_user_message(app, "/logout");
    tracing::debug!(
        target: crate::logging::targets::APP_AUTH,
        event_name = "logout_command_requested",
        message = "logout slash command requested",
        outcome = "start",
    );

    if !crate::app::auth::has_credentials() {
        push_submission_feedback(
            app,
            SystemSeverity::Info,
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
        match run_auth_child_command(&tx, &claude_path, "logout").await {
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
                        let _ = tx
                            .send(ClientEvent::SlashCommandError {
                                session_id: None,
                                message: "Logout exited successfully but credentials are still present. \
                                          Try /logout again or run `claude auth logout` in another terminal."
                                    .to_owned(),
                            })
                            .await;
                        return;
                    }
                    let _ = tx.send(ClientEvent::LogoutCompleted).await;
                } else {
                    let _ = tx
                        .send(ClientEvent::SlashCommandError {
                            session_id: None,
                            message: format!(
                                "/logout failed (exit code: {})",
                                status.code().map_or("unknown".to_owned(), |c| c.to_string())
                            ),
                        })
                        .await;
                }
            }
            Err(message) => {
                let _ = tx.send(ClientEvent::SlashCommandError { session_id: None, message }).await;
            }
        }
    });
    true
}

async fn run_auth_child_command(
    tx: &mpsc::Sender<ClientEvent>,
    claude_path: &Path,
    subcommand: &'static str,
) -> Result<ExitStatus, String> {
    // Enqueuing an event alone does not transfer ownership of inherited stdin.
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel();
    tx.send(ClientEvent::TerminalReleasedToChild {
        reason: ReleaseReason::AuthFlow,
        ready_tx,
        cancel_tx,
    })
    .await
    .map_err(|_| "UI stopped before terminal handoff".to_owned())?;
    ready_rx.await.map_err(|_| "UI did not acknowledge terminal handoff".to_owned())?;
    let terminal_release = match crate::app::terminal_runtime::TerminalReleaseGuard::release(
        ReleaseReason::AuthFlow,
        subcommand,
    ) {
        Ok(terminal_release) => terminal_release,
        Err(err) => {
            send_terminal_returned_from_child(tx).await;
            return Err(format!("Failed to release terminal for claude auth {subcommand}: {err}"));
        }
    };

    let result = match tokio::process::Command::new(claude_path)
        .args(["auth", subcommand])
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(mut child) => tokio::select! {
            status = child.wait() => status.map_err(|err| format!("Failed to run claude auth {subcommand}: {err}")),
            _ = &mut cancel_rx => {
                // This PID belongs to this auth task. Reap it before returning
                // terminal ownership, including during fatal bridge shutdown.
                let result = child.kill().await;
                Err(result.map_or_else(|err| format!("Failed to stop claude auth {subcommand}: {err}"), |()| "Authentication interrupted by shutdown".to_owned()))
            }
        },
        Err(err) => Err(format!("Failed to run claude auth {subcommand}: {err}")),
    };

    let restore_result = terminal_release.restore();
    send_terminal_returned_from_child(tx).await;
    restore_result.map_err(|err| {
        format!("Failed to restore terminal after claude auth {subcommand}: {err}")
    })?;

    result
}

async fn send_terminal_returned_from_child(tx: &mpsc::Sender<ClientEvent>) {
    let _ =
        tx.send(ClientEvent::TerminalReturnedFromChild { reason: ReleaseReason::AuthFlow }).await;
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
        unreachable!("validated /mode arguments must contain one value");
    };
    request_mode_change(app, requested_mode_arg)
}

pub(crate) fn request_mode_change(app: &mut App, requested_mode: &str) -> bool {
    let Some((conn, sid)) = require_active_session(
        app,
        "Cannot switch mode: not connected yet.",
        "Cannot switch mode: no active session.",
    ) else {
        return true;
    };

    set_command_pending(app, "Switching mode...", Some(crate::app::PendingCommandAck::CurrentMode));

    let tx = app.event_tx.clone();
    let requested_mode_owned = requested_mode.to_owned();
    let session_id = sid.to_string();
    tokio::task::spawn_local(async move {
        match conn.set_mode(session_id.clone(), requested_mode_owned) {
            Ok(()) => {}
            Err(e) => {
                let _ = tx
                    .send(ClientEvent::SlashCommandError {
                        session_id: Some(session_id),
                        message: format!("Failed to run /mode: {e}"),
                    })
                    .await;
            }
        }
    });
    true
}

fn handle_model_submit(app: &mut App, args: &[&str]) -> bool {
    let [model_name_arg] = args else {
        unreachable!("validated /model arguments must contain one value");
    };
    let model_name = *model_name_arg;

    let Some((conn, sid)) = require_active_session(
        app,
        "Cannot switch model: not connected yet.",
        "Cannot switch model: no active session.",
    ) else {
        return true;
    };

    set_command_pending(
        app,
        "Switching model...",
        Some(crate::app::PendingCommandAck::CurrentModel),
    );

    let tx = app.event_tx.clone();
    let model_name = model_name.to_owned();
    let session_id = sid.to_string();
    tokio::task::spawn_local(async move {
        match conn.set_model(session_id.clone(), model_name) {
            Ok(()) => {}
            Err(e) => {
                let _ = tx
                    .send(ClientEvent::SlashCommandError {
                        session_id: Some(session_id),
                        message: format!("Failed to run /model: {e}"),
                    })
                    .await;
            }
        }
    });
    true
}

fn handle_effort_submit(app: &mut App, args: &[&str]) -> bool {
    let [effort_arg] = args else {
        unreachable!("validated /effort arguments must contain one value");
    };
    let effort = (*effort_arg != "reset").then(|| (*effort_arg).to_owned());

    let Some((conn, sid)) = require_active_session(
        app,
        "Cannot switch effort: not connected yet.",
        "Cannot switch effort: no active session.",
    ) else {
        return true;
    };

    set_command_pending(
        app,
        "Switching effort...",
        Some(crate::app::PendingCommandAck::ConfigOption { option_id: "effortLevel".to_owned() }),
    );

    let tx = app.event_tx.clone();
    let session_id = sid.to_string();
    tokio::task::spawn_local(async move {
        match conn.set_effort(session_id.clone(), effort) {
            Ok(()) => {}
            Err(e) => {
                let _ = tx
                    .send(ClientEvent::SlashCommandError {
                        session_id: Some(session_id),
                        message: format!("Failed to run /effort: {e}"),
                    })
                    .await;
            }
        }
    });
    true
}

fn handle_thinking_submit(app: &mut App, args: &[&str]) -> bool {
    let Some((conn, sid)) = require_active_session(
        app,
        "Cannot change thinking: not connected yet.",
        "Cannot change thinking: no active session.",
    ) else {
        return true;
    };
    let enabled = match args {
        ["on"] => Some(true),
        ["off"] => Some(false),
        _ => None,
    };
    set_command_pending(
        app,
        "Changing thinking...",
        Some(crate::app::PendingCommandAck::ConfigOption {
            option_id: "alwaysThinkingEnabled".to_owned(),
        }),
    );
    if let Err(error) = conn.set_thinking(sid.to_string(), enabled) {
        crate::app::events::handle_local_slash_command_error(
            app,
            &format!("Failed to run /thinking: {error}"),
        );
    }
    true
}

fn handle_ultracode_submit(app: &mut App, args: &[&str]) -> bool {
    if args == ["status"] {
        let text = ultracode_status_text(app);
        push_submission_feedback(app, SystemSeverity::Info, &text);
        return true;
    }
    let Some((conn, sid)) = require_active_session(
        app,
        "Cannot change Ultracode: not connected yet.",
        "Cannot change Ultracode: no active session.",
    ) else {
        return true;
    };
    let enabled = args == ["on"];
    let label = if enabled { "Enabling Ultracode..." } else { "Disabling Ultracode..." };
    set_command_pending(app, label, Some(crate::app::PendingCommandAck::Ultracode));
    let tx = app.event_tx.clone();
    let session_id = sid.to_string();
    tokio::task::spawn_local(async move {
        if let Err(error) = conn.set_ultracode(session_id.clone(), enabled) {
            let _ = tx
                .send(ClientEvent::SlashCommandError {
                    session_id: Some(session_id),
                    message: format!("Failed to run /ultracode: {error}"),
                })
                .await;
        }
    });
    true
}

fn ultracode_status_text(app: &App) -> String {
    let Some(state) = app.session_runtime.ultracode else {
        return "Ultracode status is unknown for this session.".to_owned();
    };
    if state.effective() {
        let mut text = "Ultracode is on for this session.".to_owned();
        if let Some(effort) = app
            .session_runtime
            .config_options
            .get("effortLevel")
            .and_then(serde_json::Value::as_str)
            .and_then(crate::agent::model::EffortLevel::from_stored)
        {
            text.push_str(" Effort remains ");
            text.push_str(effort.as_stored());
            text.push('.');
        }
        text
    } else if state.requested() {
        "Ultracode is requested but unavailable for this session.".to_owned()
    } else if state.available() {
        "Ultracode is off and available for this session.".to_owned()
    } else {
        "Ultracode is off and unavailable for this session.".to_owned()
    }
}

fn handle_fast_submit(app: &mut App, args: &[&str]) -> bool {
    let Some((conn, sid)) = require_active_session(
        app,
        "Cannot toggle fast mode: not connected yet.",
        "Cannot toggle fast mode: no active session.",
    ) else {
        return true;
    };

    let enabled = match args {
        ["on"] => true,
        ["off"] => false,
        _ => match app.session_runtime.fast_mode_state {
            crate::agent::model::FastModeState::Off => true,
            crate::agent::model::FastModeState::On
            | crate::agent::model::FastModeState::Cooldown => false,
            crate::agent::model::FastModeState::Unknown => {
                push_system_message(
                    app,
                    "Fast mode is unknown. Use /fast on or /fast off to retry.",
                );
                return true;
            }
        },
    };
    let label = if enabled { "Enabling fast mode..." } else { "Disabling fast mode..." };
    set_command_pending(app, label, Some(crate::app::PendingCommandAck::FastMode));

    let tx = app.event_tx.clone();
    let session_id = sid.to_string();
    tokio::task::spawn_local(async move {
        if let Err(e) = conn.set_fast_mode(session_id.clone(), enabled) {
            let _ = tx
                .send(ClientEvent::SlashCommandError {
                    session_id: Some(session_id),
                    message: format!("Failed to run /fast: {e}"),
                })
                .await;
        }
    });
    true
}

fn handle_agent_submit(app: &mut App, args: &[&str]) -> bool {
    let [agent_arg] = args else {
        unreachable!("validated /agent arguments must contain one value");
    };
    let requested_agent = *agent_arg;

    let Some((conn, sid)) = require_active_session(
        app,
        "Cannot switch agent: not connected yet.",
        "Cannot switch agent: no active session.",
    ) else {
        return true;
    };

    let agent = if requested_agent == "reset" { None } else { Some(requested_agent.to_owned()) };

    set_command_pending(
        app,
        "Switching agent...",
        Some(crate::app::PendingCommandAck::ConfigOption { option_id: "agent".to_owned() }),
    );

    let tx = app.event_tx.clone();
    let session_id = sid.to_string();
    tokio::task::spawn_local(async move {
        match conn.set_agent(session_id.clone(), agent) {
            Ok(()) => {}
            Err(e) => {
                let _ = tx
                    .send(ClientEvent::SlashCommandError {
                        session_id: Some(session_id),
                        message: format!("Failed to run /agent: {e}"),
                    })
                    .await;
            }
        }
    });
    true
}

fn handle_new_session_submit(app: &mut App) -> bool {
    push_user_message(app, "/new-session");

    let Some(conn) = require_connection(app, "Cannot create new session: not connected yet.")
    else {
        return true;
    };

    set_command_pending(app, "Starting new session...", None);

    if let Err(e) = start_new_session(app, &conn, SessionStartReason::NewSession) {
        crate::app::events::handle_local_slash_command_error(
            app,
            &format!("Failed to run /new-session: {e}"),
        );
    }
    true
}

fn handle_resume_submit(app: &mut App, args: &[&str]) -> bool {
    if args.is_empty() {
        push_user_message(app, "/resume");
        if require_connection(app, "Cannot open resume picker: not connected yet.").is_none() {
            return true;
        }
        let current_session_id =
            app.session_runtime.session_id.as_ref().map(crate::agent::model::SessionId::as_str);
        let selected = current_session_id
            .and_then(|session_id| {
                app.recent_sessions
                    .iter()
                    .take(crate::app::session_picker::MAX_PICKER_SESSIONS)
                    .position(|session| session.session_id == session_id)
            })
            .unwrap_or(0);
        app.session_picker = SessionPickerState { selected, ..SessionPickerState::default() };
        view::set_fullscreen_view(app, FullscreenView::SessionPicker);
        return true;
    }
    let [session_id_arg] = args else {
        unreachable!("validated /resume arguments must contain at most one value");
    };
    let session_id = *session_id_arg;

    push_user_message(app, format!("/resume {session_id}"));
    let Some(conn) = require_connection(app, "Cannot resume session: not connected yet.") else {
        return true;
    };

    set_command_pending(app, &format!("Resuming session {session_id}..."), None);
    let session_id = session_id.to_owned();
    if let Err(e) = begin_resume_session(app, &conn, session_id) {
        crate::app::events::handle_local_slash_command_error(
            app,
            &format!("Failed to run /resume: {e}"),
        );
    }
    true
}

fn handle_rewind_submit(app: &mut App, args: &[&str]) -> bool {
    let [target_uuid_arg, restore_mode_arg] = args else {
        unreachable!("validated /rewind arguments must contain a target and restore mode");
    };
    let target_uuid = *target_uuid_arg;
    let Some(restore_mode) = RewindRestoreMode::from_stored(restore_mode_arg) else {
        unreachable!("validated /rewind restore mode must be supported");
    };
    let Some(target) =
        app.sdk_inventory.rewind_targets.iter().find(|target| target.uuid == target_uuid)
    else {
        push_system_message(app, format!("Unknown rewind target: {target_uuid}"));
        return true;
    };
    let target_uuid = target.uuid.clone();
    let Some((conn, session_id)) = require_active_session(
        app,
        "Cannot rewind: not connected yet.",
        "Cannot rewind: no active session.",
    ) else {
        return true;
    };

    let pending_label = match restore_mode {
        RewindRestoreMode::Conversation => "Rewinding conversation...",
        RewindRestoreMode::Code => "Restoring code...",
        RewindRestoreMode::Both => "Restoring code and conversation...",
    };
    set_command_pending(app, pending_label, None);
    let session_id = session_id.to_string();
    if let Err(e) = begin_rewind(app, &conn, session_id.clone(), target_uuid, restore_mode) {
        crate::app::events::handle_local_slash_command_error(
            app,
            &format!("Failed to run /rewind: {e}"),
        );
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

fn build_docs_mode_markdown(app: &App) -> String {
    let rows = app.session_runtime.mode.as_ref().map_or_else(
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
    let rows = if app.sdk_inventory.available_models.is_empty() {
        vec![("Unavailable".to_owned(), "Connect to load advertised models.".to_owned())]
    } else {
        app.sdk_inventory
            .available_models
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
        if model.supported_effort_levels.is_empty() {
            parts.push("Effort".to_owned());
        } else {
            let levels = model
                .supported_effort_levels
                .iter()
                .map(|level| level.label())
                .collect::<Vec<_>>()
                .join(", ");
            parts.push(format!("Effort: {levels}"));
        }
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
