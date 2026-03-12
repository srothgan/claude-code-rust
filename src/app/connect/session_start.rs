// Claude Code Rust - A native Rust terminal interface for Claude Code
// Copyright (C) 2025  Simon Peter Rothgang
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as
// published by the Free Software Foundation, either version 3 of the
// License, or (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

use crate::agent::client::AgentConnection;
use crate::agent::model::EffortLevel;
use crate::agent::wire::SessionLaunchSettings;
use crate::app::App;
use crate::app::settings::{language_input_validation_message, model_supports_effort, store};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionStartReason {
    Startup,
    NewSession,
    Resume,
    Login,
    Logout,
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
            let always_thinking =
                store::always_thinking_enabled(&app.settings.committed_settings_document)
                    .unwrap_or(false);
            let model = store::model(&app.settings.committed_settings_document).ok().flatten();
            let language = store::language(&app.settings.committed_settings_document)
                .ok()
                .flatten()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
                .filter(|value| language_input_validation_message(value).is_none());
            SessionLaunchSettings {
                model: model.clone(),
                language,
                permission_mode: Some(
                    store::default_permission_mode(&app.settings.committed_settings_document)
                        .unwrap_or_default()
                        .as_stored()
                        .to_owned(),
                ),
                thinking_mode: Some(
                    if always_thinking { "adaptive" } else { "disabled" }.to_owned(),
                ),
                effort_level: always_thinking
                    .then(|| {
                        model.as_deref().is_none_or(|model_id| model_supports_effort(app, model_id))
                    })
                    .filter(|supports_effort| *supports_effort)
                    .map(|_| {
                        store::thinking_effort_level(&app.settings.committed_settings_document)
                            .unwrap_or(EffortLevel::Medium)
                    }),
            }
        }
    }
}

pub(crate) fn start_new_session(
    app: &App,
    conn: &AgentConnection,
    reason: SessionStartReason,
) -> anyhow::Result<()> {
    conn.new_session(app.cwd_raw.clone(), session_launch_settings_for_reason(app, reason))
}

pub(crate) fn resume_session(
    app: &App,
    conn: &AgentConnection,
    session_id: String,
) -> anyhow::Result<()> {
    conn.resume_session(
        session_id,
        session_launch_settings_for_reason(app, SessionStartReason::Resume),
    )
}

#[cfg(test)]
mod tests {
    use super::{SessionStartReason, session_launch_settings_for_reason};
    use crate::agent::model::EffortLevel;
    use crate::app::App;
    use crate::app::settings::{DefaultPermissionMode, store};

    #[test]
    fn persisted_launch_settings_include_model_and_permission_mode() {
        let mut app = App::test_default();
        store::set_model(&mut app.settings.committed_settings_document, Some("haiku"));
        store::set_default_permission_mode(
            &mut app.settings.committed_settings_document,
            DefaultPermissionMode::Plan,
        );
        store::set_language(&mut app.settings.committed_settings_document, Some("German"));
        store::set_always_thinking_enabled(&mut app.settings.committed_settings_document, true);
        store::set_thinking_effort_level(
            &mut app.settings.committed_settings_document,
            EffortLevel::High,
        );

        let launch_settings = session_launch_settings_for_reason(&app, SessionStartReason::Startup);

        assert_eq!(launch_settings.model.as_deref(), Some("haiku"));
        assert_eq!(launch_settings.language.as_deref(), Some("German"));
        assert_eq!(launch_settings.permission_mode.as_deref(), Some("plan"));
        assert_eq!(launch_settings.thinking_mode.as_deref(), Some("adaptive"));
        assert_eq!(launch_settings.effort_level, Some(EffortLevel::High));
    }

    #[test]
    fn persisted_launch_settings_trim_language_value() {
        let mut app = App::test_default();
        app.settings.committed_settings_document = serde_json::json!({ "language": "  German  " });

        let launch_settings = session_launch_settings_for_reason(&app, SessionStartReason::Startup);

        assert_eq!(launch_settings.language.as_deref(), Some("German"));
    }

    #[test]
    fn persisted_launch_settings_default_permission_mode_when_missing() {
        let app = App::test_default();

        let launch_settings =
            session_launch_settings_for_reason(&app, SessionStartReason::NewSession);

        assert_eq!(launch_settings.model, None);
        assert_eq!(launch_settings.language, None);
        assert_eq!(launch_settings.permission_mode.as_deref(), Some("default"));
        assert_eq!(launch_settings.thinking_mode.as_deref(), Some("disabled"));
        assert_eq!(launch_settings.effort_level, None);
    }

    #[test]
    fn persisted_launch_settings_omit_effort_for_models_without_effort_support() {
        let mut app = App::test_default();
        app.available_models =
            vec![crate::agent::model::AvailableModel::new("haiku", "Haiku").supports_effort(false)];
        store::set_model(&mut app.settings.committed_settings_document, Some("haiku"));
        store::set_always_thinking_enabled(&mut app.settings.committed_settings_document, true);
        store::set_thinking_effort_level(
            &mut app.settings.committed_settings_document,
            EffortLevel::High,
        );

        let launch_settings = session_launch_settings_for_reason(&app, SessionStartReason::Startup);

        assert_eq!(launch_settings.model.as_deref(), Some("haiku"));
        assert_eq!(launch_settings.language, None);
        assert_eq!(launch_settings.thinking_mode.as_deref(), Some("adaptive"));
        assert_eq!(launch_settings.effort_level, None);
    }

    #[test]
    fn persisted_launch_settings_omit_invalid_language_value() {
        let mut app = App::test_default();
        app.settings.committed_settings_document = serde_json::json!({ "language": "E" });

        let launch_settings = session_launch_settings_for_reason(&app, SessionStartReason::Startup);

        assert_eq!(launch_settings.language, None);
    }

    #[test]
    fn persisted_launch_settings_omit_whitespace_only_language_value() {
        let mut app = App::test_default();
        app.settings.committed_settings_document = serde_json::json!({ "language": "   " });

        let launch_settings = session_launch_settings_for_reason(&app, SessionStartReason::Startup);

        assert_eq!(launch_settings.language, None);
    }

    #[test]
    fn logout_launch_settings_omit_all_overrides() {
        let mut app = App::test_default();
        store::set_model(&mut app.settings.committed_settings_document, Some("haiku"));
        store::set_default_permission_mode(
            &mut app.settings.committed_settings_document,
            DefaultPermissionMode::Plan,
        );
        store::set_always_thinking_enabled(&mut app.settings.committed_settings_document, true);

        let launch_settings = session_launch_settings_for_reason(&app, SessionStartReason::Logout);

        assert!(launch_settings.is_empty());
    }
}
