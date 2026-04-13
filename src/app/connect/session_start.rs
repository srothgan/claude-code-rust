// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use crate::agent::client::AgentConnection;
use crate::agent::wire::SessionLaunchSettings;
use crate::app::App;
use crate::app::config::{language_input_validation_message, store};
use serde_json::{Map, Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionStartReason {
    Startup,
    NewSession,
    Resume,
    Login,
    Logout,
}

impl SessionStartReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::Startup => "startup",
            Self::NewSession => "new_session",
            Self::Resume => "resume",
            Self::Login => "login",
            Self::Logout => "logout",
        }
    }

    fn event_name(self) -> &'static str {
        match self {
            Self::Startup => "session_start_requested",
            Self::Resume => "session_resume_requested",
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
        | SessionStartReason::Login => {
            let language = store::language(&app.config.committed_settings_document)
                .ok()
                .flatten()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
                .filter(|value| language_input_validation_message(value).is_none());
            SessionLaunchSettings {
                language,
                settings: Some(build_session_settings_object(app)),
                agent_progress_summaries: Some(true),
            }
        }
    }
}

fn build_session_settings_object(app: &App) -> Value {
    let mut settings = Map::new();

    settings.insert(
        "alwaysThinkingEnabled".to_owned(),
        Value::Bool(app.config.always_thinking_effective()),
    );

    if let Some(model) = store::model(&app.config.committed_settings_document).ok().flatten() {
        settings.insert("model".to_owned(), Value::String(model));
    }

    settings.insert(
        "permissions".to_owned(),
        json!({
            "defaultMode": app.config.default_permission_mode_effective().as_stored()
        }),
    );
    settings.insert("fastMode".to_owned(), Value::Bool(app.config.fast_mode_effective()));
    settings.insert(
        "effortLevel".to_owned(),
        Value::String(app.config.thinking_effort_effective().as_stored().to_owned()),
    );
    settings.insert(
        "outputStyle".to_owned(),
        Value::String(app.config.output_style_effective().as_stored().to_owned()),
    );
    settings.insert(
        "spinnerTipsEnabled".to_owned(),
        Value::Bool(
            store::spinner_tips_enabled(&app.config.committed_local_settings_document)
                .unwrap_or(true),
        ),
    );
    settings.insert(
        "terminalProgressBarEnabled".to_owned(),
        Value::Bool(
            store::terminal_progress_bar_enabled(&app.config.committed_preferences_document)
                .unwrap_or(true),
        ),
    );
    if let Some(mut sandbox) =
        app.config.committed_settings_document.get("sandbox").and_then(Value::as_object).cloned()
    {
        if sandbox.get("enabled").and_then(Value::as_bool) == Some(true)
            && !sandbox.contains_key("failIfUnavailable")
        {
            sandbox.insert("failIfUnavailable".to_owned(), Value::Bool(false));
        }
        settings.insert("sandbox".to_owned(), Value::Object(sandbox));
    }

    Value::Object(settings)
}

fn log_session_request(
    app: &App,
    reason: SessionStartReason,
    launch_settings: &SessionLaunchSettings,
    session_id: Option<&str>,
) {
    let has_language = launch_settings.language.is_some();
    let has_settings = launch_settings.settings.is_some();
    let agent_progress_summaries_enabled =
        launch_settings.agent_progress_summaries.unwrap_or(false);
    if let Some(session_id) = session_id {
        tracing::info!(
            target: crate::logging::targets::APP_SESSION,
            event_name = reason.event_name(),
            message = "session request queued",
            outcome = "start",
            reason = reason.as_str(),
            session_id = %session_id,
            cwd = %app.cwd_raw,
            has_language,
            has_settings,
            agent_progress_summaries_enabled,
        );
    } else {
        tracing::info!(
            target: crate::logging::targets::APP_SESSION,
            event_name = reason.event_name(),
            message = "session request queued",
            outcome = "start",
            reason = reason.as_str(),
            cwd = %app.cwd_raw,
            has_language,
            has_settings,
            agent_progress_summaries_enabled,
        );
    }
}

pub(crate) fn start_new_session(
    app: &App,
    conn: &AgentConnection,
    reason: SessionStartReason,
) -> anyhow::Result<()> {
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
    app.resuming_session_id = Some(session_id.clone());
    resume_session(app, conn, session_id)
}

#[cfg(test)]
mod tests {
    use super::{SessionStartReason, session_launch_settings_for_reason};
    use crate::agent::model::EffortLevel;
    use crate::agent::wire::SessionLaunchSettings;
    use crate::app::App;
    use crate::app::config::{DefaultPermissionMode, store};
    use serde_json::{Map, Value};

    #[test]
    fn persisted_launch_settings_include_model_and_permission_mode() {
        let mut app = App::test_default();
        store::set_model(&mut app.config.committed_settings_document, Some("haiku"));
        store::set_default_permission_mode(
            &mut app.config.committed_settings_document,
            DefaultPermissionMode::Plan,
        );
        store::set_language(&mut app.config.committed_settings_document, Some("German"));
        store::set_always_thinking_enabled(&mut app.config.committed_settings_document, true);
        store::set_thinking_effort_level(
            &mut app.config.committed_settings_document,
            EffortLevel::High,
        );

        let launch_settings = session_launch_settings_for_reason(&app, SessionStartReason::Startup);

        assert_eq!(launch_settings.language.as_deref(), Some("German"));
        assert_setting_value(&launch_settings, "alwaysThinkingEnabled", &Value::Bool(true));
        assert_setting_value(&launch_settings, "model", &Value::String("haiku".to_owned()));
        assert_permission_mode(&launch_settings, "plan");
        assert_setting_value(&launch_settings, "fastMode", &Value::Bool(false));
        assert_setting_value(&launch_settings, "effortLevel", &Value::String("high".to_owned()));
        assert_setting_value(&launch_settings, "outputStyle", &Value::String("Default".to_owned()));
        assert_setting_value(&launch_settings, "spinnerTipsEnabled", &Value::Bool(true));
        assert_setting_value(&launch_settings, "terminalProgressBarEnabled", &Value::Bool(true));
        assert_eq!(launch_settings.agent_progress_summaries, Some(true));
    }

    #[test]
    fn persisted_launch_settings_include_auto_permission_mode() {
        let mut app = App::test_default();
        store::set_default_permission_mode(
            &mut app.config.committed_settings_document,
            DefaultPermissionMode::Auto,
        );

        let launch_settings = session_launch_settings_for_reason(&app, SessionStartReason::Startup);

        assert_permission_mode(&launch_settings, "auto");
    }

    #[test]
    fn persisted_launch_settings_preserve_sandbox_settings_and_make_fallback_explicit() {
        let mut app = App::test_default();
        app.config.committed_settings_document = serde_json::json!({
            "sandbox": {
                "enabled": true,
                "allowUnsandboxedCommands": false
            }
        });

        let launch_settings = session_launch_settings_for_reason(&app, SessionStartReason::Startup);

        assert_setting_value(
            &launch_settings,
            "sandbox",
            &serde_json::json!({
                "enabled": true,
                "allowUnsandboxedCommands": false,
                "failIfUnavailable": false
            }),
        );
    }

    #[test]
    fn persisted_launch_settings_preserve_explicit_sandbox_fail_if_unavailable() {
        let mut app = App::test_default();
        app.config.committed_settings_document = serde_json::json!({
            "sandbox": {
                "enabled": true,
                "failIfUnavailable": true
            }
        });

        let launch_settings = session_launch_settings_for_reason(&app, SessionStartReason::Startup);

        assert_setting_value(
            &launch_settings,
            "sandbox",
            &serde_json::json!({
                "enabled": true,
                "failIfUnavailable": true
            }),
        );
    }

    #[test]
    fn persisted_launch_settings_trim_language_value() {
        let mut app = App::test_default();
        app.config.committed_settings_document = serde_json::json!({ "language": "  German  " });

        let launch_settings = session_launch_settings_for_reason(&app, SessionStartReason::Startup);

        assert_eq!(launch_settings.language.as_deref(), Some("German"));
    }

    #[test]
    fn persisted_launch_settings_default_permission_mode_when_missing() {
        let app = App::test_default();

        let launch_settings =
            session_launch_settings_for_reason(&app, SessionStartReason::NewSession);

        assert_eq!(launch_settings.language, None);
        assert_setting_absent(&launch_settings, "model");
        assert_setting_value(&launch_settings, "alwaysThinkingEnabled", &Value::Bool(false));
        assert_permission_mode(&launch_settings, "default");
        assert_setting_value(&launch_settings, "fastMode", &Value::Bool(false));
        assert_setting_value(&launch_settings, "effortLevel", &Value::String("medium".to_owned()));
        assert_setting_value(&launch_settings, "outputStyle", &Value::String("Default".to_owned()));
        assert_setting_value(&launch_settings, "spinnerTipsEnabled", &Value::Bool(true));
        assert_setting_value(&launch_settings, "terminalProgressBarEnabled", &Value::Bool(true));
        assert_eq!(launch_settings.agent_progress_summaries, Some(true));
    }

    #[test]
    fn persisted_launch_settings_include_supported_settings_json_without_model_when_unset() {
        let mut app = App::test_default();
        store::set_always_thinking_enabled(&mut app.config.committed_settings_document, true);
        store::set_thinking_effort_level(
            &mut app.config.committed_settings_document,
            EffortLevel::High,
        );
        store::set_fast_mode(&mut app.config.committed_settings_document, true);
        store::set_output_style(
            &mut app.config.committed_local_settings_document,
            crate::app::config::OutputStyle::Learning,
        );
        store::set_spinner_tips_enabled(&mut app.config.committed_local_settings_document, false);
        store::set_terminal_progress_bar_enabled(
            &mut app.config.committed_preferences_document,
            false,
        );

        let launch_settings = session_launch_settings_for_reason(&app, SessionStartReason::Startup);

        assert_eq!(launch_settings.language, None);
        assert_setting_absent(&launch_settings, "model");
        assert_setting_value(&launch_settings, "alwaysThinkingEnabled", &Value::Bool(true));
        assert_permission_mode(&launch_settings, "default");
        assert_setting_value(&launch_settings, "fastMode", &Value::Bool(true));
        assert_setting_value(&launch_settings, "effortLevel", &Value::String("high".to_owned()));
        assert_setting_value(
            &launch_settings,
            "outputStyle",
            &Value::String("Learning".to_owned()),
        );
        assert_setting_value(&launch_settings, "spinnerTipsEnabled", &Value::Bool(false));
        assert_setting_value(&launch_settings, "terminalProgressBarEnabled", &Value::Bool(false));
        assert_eq!(launch_settings.agent_progress_summaries, Some(true));
    }

    #[test]
    fn persisted_launch_settings_omit_invalid_language_value() {
        let mut app = App::test_default();
        app.config.committed_settings_document = serde_json::json!({ "language": "E" });

        let launch_settings = session_launch_settings_for_reason(&app, SessionStartReason::Startup);

        assert_eq!(launch_settings.language, None);
    }

    #[test]
    fn persisted_launch_settings_omit_whitespace_only_language_value() {
        let mut app = App::test_default();
        app.config.committed_settings_document = serde_json::json!({ "language": "   " });

        let launch_settings = session_launch_settings_for_reason(&app, SessionStartReason::Startup);

        assert_eq!(launch_settings.language, None);
    }

    #[test]
    fn logout_launch_settings_omit_all_overrides() {
        let mut app = App::test_default();
        store::set_model(&mut app.config.committed_settings_document, Some("haiku"));
        store::set_default_permission_mode(
            &mut app.config.committed_settings_document,
            DefaultPermissionMode::Plan,
        );
        store::set_always_thinking_enabled(&mut app.config.committed_settings_document, true);

        let launch_settings = session_launch_settings_for_reason(&app, SessionStartReason::Logout);

        assert!(launch_settings.is_empty());
    }

    fn settings_object(launch_settings: &SessionLaunchSettings) -> &Map<String, Value> {
        launch_settings.settings.as_ref().and_then(Value::as_object).expect("settings object")
    }

    fn assert_setting_value(launch_settings: &SessionLaunchSettings, key: &str, expected: &Value) {
        assert_eq!(settings_object(launch_settings).get(key), Some(expected));
    }

    fn assert_setting_absent(launch_settings: &SessionLaunchSettings, key: &str) {
        assert!(
            !settings_object(launch_settings).contains_key(key),
            "expected `{key}` to be absent"
        );
    }

    fn assert_permission_mode(launch_settings: &SessionLaunchSettings, expected: &str) {
        let permissions = settings_object(launch_settings)
            .get("permissions")
            .and_then(Value::as_object)
            .expect("permissions object");
        assert_eq!(permissions.get("defaultMode"), Some(&Value::String(expected.to_owned())));
    }
}
