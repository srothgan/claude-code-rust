// SPDX-License-Identifier: Apache-2.0

use super::App;
use std::time::Instant;

/// Rust owns the FIFO. Only its active item crosses the bridge boundary.
pub(super) fn dispatch_next(app: &mut App) {
    let Some(conn) = app.session_runtime.conn.clone() else { return };
    let Some(session_id) = app.session_runtime.session_id.clone() else { return };
    while let Some(item) = app.btw.take_next() {
        match conn.ask_side_question(session_id.to_string(), item.id.clone(), item.question) {
            Ok(()) => break,
            Err(error) => {
                let _ = app.btw.fail(&item.id, format!("could not send: {error}"), Instant::now());
                tracing::warn!(
                    target: crate::logging::targets::APP_INPUT,
                    event_name = "side_question_dispatch_failed",
                    message = "side question could not enter the bridge command queue",
                    outcome = "failure",
                    session_id = %session_id,
                    btw_id = %item.id,
                    error = %error,
                );
            }
        }
    }
    app.request_active_surface_repaint();
}
