// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::prelude::*;

impl App {
    /// Queue a paste payload for drain-cycle finalization.
    ///
    /// This is fed by paste payloads captured from terminal events.
    pub fn queue_paste_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let chunk_chars = text.chars().count();
        let had_pending_submit = self.pending_submit.is_some();
        self.pending_submit = None;
        if self.pending_paste_text.is_empty() {
            let continued_session = self.active_paste_session.and_then(|session| {
                let current_line = self.input.lines().get(self.input.cursor_row())?;
                let idx =
                    parse_paste_placeholder_before_cursor(current_line, self.input.cursor_col())?;
                (session.placeholder_index == Some(idx)).then_some(session)
            });
            self.pending_paste_session = Some(continued_session.unwrap_or_else(|| {
                let id = self.next_paste_session_id;
                self.next_paste_session_id = self.next_paste_session_id.saturating_add(1);
                PasteSessionState {
                    id,
                    start: SelectionPoint {
                        row: self.input.cursor_row(),
                        col: self.input.cursor_col(),
                    },
                    placeholder_index: None,
                }
            }));
            if let Some(session) = self.pending_paste_session {
                tracing::debug!(
                    target: crate::logging::targets::APP_PASTE,
                    event_name = "paste_queue_opened",
                    message = "paste queue session opened",
                    outcome = "start",
                    session_id = session.id,
                    start_row = session.start.row,
                    start_col = session.start.col,
                    placeholder_index = ?session.placeholder_index,
                    chunk_chars,
                    had_pending_submit,
                );
            }
        }
        self.pending_paste_text.push_str(text);
        tracing::debug!(
            target: crate::logging::targets::APP_PASTE,
            event_name = "paste_queue_updated",
            message = "paste queue updated",
            outcome = "success",
            chunk_chars,
            pending_chars = self.pending_paste_text.chars().count(),
            had_pending_submit,
        );
    }
}
