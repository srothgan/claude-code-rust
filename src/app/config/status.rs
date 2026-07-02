// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::edit::model_overlay_options;
use super::prelude::*;

pub fn request_status_snapshot_if_needed(app: &App) {
    if app.config.active_tab != ConfigTab::Status {
        return;
    }
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

pub(crate) fn model_status_label(model: Option<&str>, app: &App) -> String {
    match model {
        None => DEFAULT_MODEL_ALIAS_LABEL.to_owned(),
        Some(model_id) => model_overlay_options(app)
            .into_iter()
            .find(|candidate| candidate.matches_model_id(model_id))
            .map_or_else(
                || {
                    if model_id == DEFAULT_MODEL_ALIAS_ID {
                        DEFAULT_MODEL_ALIAS_LABEL.to_owned()
                    } else {
                        model_id.to_owned()
                    }
                },
                |candidate| candidate.display_name,
            ),
    }
}
