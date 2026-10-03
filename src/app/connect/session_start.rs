// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use crate::agent::client::AgentConnection;
use crate::agent::types::RewindRestoreMode;
use crate::agent::wire::SessionLaunchSettings;
use crate::app::App;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionStartReason {
    Startup,
    NewSession,
    Resume,
    Rewind,
    Login,
    Logout,
}

impl SessionStartReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::Startup => "startup",
            Self::NewSession => "new_session",
            Self::Resume => "resume",
            Self::Rewind => "rewind",
            Self::Login => "login",
            Self::Logout => "logout",
        }
    }

    fn event_name(self) -> &'static str {
        match self {
            Self::Startup => "session_start_requested",
            Self::Resume | Self::Rewind => "session_resume_requested",
            Self::NewSession | Self::Login | Self::Logout => "session_restart_requested",
        }
    }
}

pub(crate) fn session_launch_settings_for_reason(
    app: &App,
    reason: SessionStartReason,
) -> SessionLaunchSettings {
    match reason {
        SessionStartReason::Logout => SessionLaunchSettings::default(),
        SessionStartReason::Startup
        | SessionStartReason::NewSession
        | SessionStartReason::Resume
        | SessionStartReason::Rewind
        | SessionStartReason::Login => {
            let options = app.startup.session_options();
            SessionLaunchSettings {
                model: options.and_then(|options| options.model.clone()),
                permission_mode: options
                    .and_then(|options| options.permission_mode)
                    .map(|mode| mode.as_stored().to_owned()),
                effort: options.and_then(|options| options.effort),
                agent: options.and_then(|options| options.agent.clone()),
            }
        }
    }
}

fn log_session_request(
    app: &App,
    reason: SessionStartReason,
    launch_settings: &SessionLaunchSettings,
    session_id: Option<&str>,
) {
    let has_launch_overrides = !launch_settings.is_empty();
    if let Some(session_id) = session_id {
        tracing::info!(
            target: crate::logging::targets::APP_SESSION,
            event_name = reason.event_name(),
            message = "session request queued",
            outcome = "start",
            reason = reason.as_str(),
            session_id = %session_id,
            cwd = %app.cwd_raw,
            has_launch_overrides,
        );
    } else {
        tracing::info!(
            target: crate::logging::targets::APP_SESSION,
            event_name = reason.event_name(),
            message = "session request queued",
            outcome = "start",
            reason = reason.as_str(),
            cwd = %app.cwd_raw,
            has_launch_overrides,
        );
    }
}

pub(crate) fn start_new_session(
    app: &mut App,
    conn: &AgentConnection,
    reason: SessionStartReason,
) -> anyhow::Result<()> {
    app.show_session_overview = true;
    let launch_settings = session_launch_settings_for_reason(app, reason);
    log_session_request(app, reason, &launch_settings, None);
    conn.new_session(app.cwd_raw.clone(), launch_settings)
}

pub(crate) fn resume_session(
    app: &App,
    conn: &AgentConnection,
    session_id: String,
) -> anyhow::Result<()> {
    let launch_settings = session_launch_settings_for_reason(app, SessionStartReason::Resume);
    log_session_request(app, SessionStartReason::Resume, &launch_settings, Some(&session_id));
    conn.resume_session(session_id, launch_settings)
}

/// Begin a session resume by marking the target session and sending the command.
///
/// Caller owns UI concerns such as entering `CommandPending` and surfacing
/// synchronous errors.
pub(crate) fn begin_resume_session(
    app: &mut App,
    conn: &AgentConnection,
    session_id: String,
) -> anyhow::Result<()> {
    app.set_pending_session_resume(session_id.clone(), None);
    app.show_session_overview = false;
    resume_session(app, conn, session_id)
}

pub(crate) fn begin_resume_session_at(
    app: &mut App,
    conn: &AgentConnection,
    session_id: String,
    target_user_message_id: String,
) -> anyhow::Result<()> {
    let operation_id = uuid::Uuid::new_v4().to_string();
    app.set_pending_session_resume(session_id.clone(), Some(operation_id.clone()));
    app.show_session_overview = false;
    let launch_settings = session_launch_settings_for_reason(app, SessionStartReason::Resume);
    log_session_request(app, SessionStartReason::Resume, &launch_settings, Some(&session_id));
    conn.resume_session_at(session_id, target_user_message_id, launch_settings, operation_id)
}

pub(crate) fn begin_rewind(
    app: &App,
    conn: &AgentConnection,
    session_id: String,
    target_user_message_id: String,
    restore_mode: RewindRestoreMode,
) -> anyhow::Result<()> {
    let launch_settings = session_launch_settings_for_reason(app, SessionStartReason::Rewind);
    log_session_request(app, SessionStartReason::Rewind, &launch_settings, Some(&session_id));
    conn.rewind(session_id, target_user_message_id, restore_mode, launch_settings)
}

#[cfg(test)]
mod tests {
    use super::{SessionStartReason, session_launch_settings_for_reason};
    use crate::agent::model::EffortLevel;
    use crate::app::App;

    #[test]
    fn explicit_cli_choices_apply_until_startup_completes() {
        use clap::Parser;
        let cli = crate::Cli::try_parse_from([
            "claude-rs",
            "--resume",
            "--model",
            "opus",
            "--effort",
            "max",
            "--permission-mode",
            "plan",
            "--agent",
            "reviewer",
        ])
        .expect("CLI");
        let mut app = App::test_default();
        app.startup = crate::app::state::StartupState::from_cli(&cli);
        app.config.committed_settings_document = serde_json::json!({
            "model": "haiku", "permissions": {"defaultMode": "default"},
            "modelSettings": {"claude-opus-5-5": {"effortLevel": "low"}}
        });
        let saved = app.config.committed_settings_document.clone();
        for reason in [SessionStartReason::Startup, SessionStartReason::Resume] {
            let launch = session_launch_settings_for_reason(&app, reason);
            assert_eq!(launch.model.as_deref(), Some("opus"));
            assert_eq!(launch.permission_mode.as_deref(), Some("plan"));
            assert_eq!(launch.effort, Some(EffortLevel::Max));
            assert_eq!(launch.agent.as_deref(), Some("reviewer"));
        }
        assert_eq!(app.config.committed_settings_document, saved);
        app.startup.complete_launch();
        let next = session_launch_settings_for_reason(&app, SessionStartReason::NewSession);
        assert_eq!(next, crate::agent::wire::SessionLaunchSettings::default());
    }
}
