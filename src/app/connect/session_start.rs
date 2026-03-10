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
use crate::agent::wire::SessionLaunchSettings;
use crate::app::App;
use crate::app::settings::store;

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
        | SessionStartReason::Login => SessionLaunchSettings {
            model: store::model(&app.settings.committed_settings_document).ok().flatten(),
            permission_mode: Some(
                store::default_permission_mode(&app.settings.committed_settings_document)
                    .unwrap_or_default()
                    .as_stored()
                    .to_owned(),
            ),
            thinking_mode: Some(
                if store::always_thinking_enabled(&app.settings.committed_settings_document)
                    .unwrap_or(false)
                {
                    "adaptive"
                } else {
                    "disabled"
                }
                .to_owned(),
            ),
        },
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
        store::set_always_thinking_enabled(&mut app.settings.committed_settings_document, true);

        let launch_settings = session_launch_settings_for_reason(&app, SessionStartReason::Startup);

        assert_eq!(launch_settings.model.as_deref(), Some("haiku"));
        assert_eq!(launch_settings.permission_mode.as_deref(), Some("plan"));
        assert_eq!(launch_settings.thinking_mode.as_deref(), Some("adaptive"));
    }

    #[test]
    fn persisted_launch_settings_default_permission_mode_when_missing() {
        let app = App::test_default();

        let launch_settings =
            session_launch_settings_for_reason(&app, SessionStartReason::NewSession);

        assert_eq!(launch_settings.model, None);
        assert_eq!(launch_settings.permission_mode.as_deref(), Some("default"));
        assert_eq!(launch_settings.thinking_mode.as_deref(), Some("disabled"));
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
