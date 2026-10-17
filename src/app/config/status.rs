// SPDX-License-Identifier: Apache-2.0

use super::SessionTranscriptState;
use super::prelude::*;

/// What the Status tab needs when it is shown or its session changes.
pub(super) fn refresh_status_tab(app: &mut App) {
    if app.config.active_tab != ConfigTab::Status {
        return;
    }
    request_status_snapshot(app);
    refresh_session_transcript(app);
    app.config.memory_file =
        crate::claude_paths::ClaudePaths::resolve(app.settings_home_override.as_deref())
            .map(|paths| paths.default_memory_file(&app.cwd_raw));
}

/// A finished turn can have saved the session, which the open Status tab and
/// its rename dialog wait for.
pub(crate) fn refresh_session_transcript_if_shown(app: &mut App) {
    if app.surface_mode == SurfaceMode::Fullscreen(FullscreenView::Config)
        && app.config.active_tab == ConfigTab::Status
    {
        refresh_session_transcript(app);
    }
}

/// Checks whether the active session has the saved transcript that the SDK's
/// rename appends to. It is a `stat` per candidate folder (a directory read
/// only for a working directory too long for a plain folder name), so it runs
/// inline, and only at the refresh points, never per frame.
pub(super) fn refresh_session_transcript(app: &mut App) {
    let Some(session_id) = app.session_runtime.session_id.as_ref() else {
        app.config.session_transcript = None;
        return;
    };
    // A session resumed from a sibling worktree keeps writing to the folder of
    // the directory it was listed under, which the SDK's lookup also searches.
    let listed_cwd = app
        .recent_sessions
        .iter()
        .find(|session| session.session_id == session_id.as_str())
        .and_then(|session| session.cwd.as_deref())
        .filter(|cwd| *cwd != app.cwd_raw);
    let cwds: Vec<&str> = std::iter::once(app.cwd_raw.as_str()).chain(listed_cwd).collect();
    let state =
        match crate::claude_paths::ClaudePaths::resolve(app.settings_home_override.as_deref()) {
            None => SessionTranscriptState::CheckFailed(
                "Cannot check whether this session is saved: the Claude configuration directory is unknown."
                    .to_owned(),
            ),
            Some(paths) => match paths.session_transcript_saved(&cwds, session_id.as_str()) {
                Ok(true) => SessionTranscriptState::Saved,
                Ok(false) => SessionTranscriptState::NotSaved,
                Err(error) => SessionTranscriptState::CheckFailed(format!(
                    "Cannot check whether this session is saved: {error}"
                )),
            },
        };
    app.config.session_transcript = Some(state);
}

fn request_status_snapshot(app: &App) {
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
            message = "status snapshot requested",
            outcome = "start",
            session_id = %session_id,
        ),
        Err(error) => tracing::warn!(
            target: crate::logging::targets::APP_AUTH,
            event_name = "status_snapshot_request_failed",
            message = "failed to request status snapshot",
            outcome = "failure",
            session_id = %session_id,
            error_message = %error,
        ),
    }
}
