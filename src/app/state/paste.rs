// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::prelude::*;

/// Paste ingestion state: burst detection, queued chunks, and paste-session
/// tracking for placeholder continuation.
pub struct PasteState {
    /// Timing-based paste burst detector. Detects rapid character streams
    /// (paste delivered as individual key events) and buffers them into a
    /// single paste payload. Fallback for terminals without bracketed paste.
    pub burst: paste_burst::PasteBurstDetector,
    /// Buffered `Event::Paste` payload for this drain cycle.
    /// Some terminals split one clipboard paste into multiple chunks; we merge
    /// them and apply placeholder threshold to the merged content once per cycle.
    pub pending_text: String,
    /// Pending paste session metadata for the currently queued `Event::Paste` payload.
    pub pending_session: Option<PasteSessionState>,
    /// Most recent active placeholder paste session, used for safe chunk continuation.
    pub active_session: Option<PasteSessionState>,
    /// Monotonic counter for paste session identifiers. Private so IDs can only
    /// be handed out via [`PasteState::allocate_session_id`].
    next_session_id: u64,
}

impl PasteState {
    #[must_use]
    pub fn has_pending_text(&self) -> bool {
        !self.pending_text.is_empty()
    }

    /// Hand out the next paste session ID; IDs are unique per app lifetime.
    pub fn allocate_session_id(&mut self) -> u64 {
        let id = self.next_session_id;
        self.next_session_id = self.next_session_id.saturating_add(1);
        id
    }

    pub fn take_pending_text(&mut self) -> String {
        std::mem::take(&mut self.pending_text)
    }

    pub fn take_pending_session(&mut self) -> Option<PasteSessionState> {
        self.pending_session.take()
    }

    pub fn set_active_session(&mut self, session: PasteSessionState) {
        self.active_session = Some(session);
    }

    pub fn clear_active_session(&mut self) {
        self.active_session = None;
    }

    pub fn clear_pending_queue(&mut self) {
        self.pending_text.clear();
        self.pending_session = None;
    }

    pub fn clear_all_sessions(&mut self) {
        self.clear_pending_queue();
        self.clear_active_session();
    }

    pub fn clear_sessions_for_placeholder(&mut self, index: usize) {
        if self.active_session.is_some_and(|session| session.placeholder_index == Some(index)) {
            self.active_session = None;
        }
        if self.pending_session.is_some_and(|session| session.placeholder_index == Some(index)) {
            self.pending_session = None;
        }
    }
}

impl Default for PasteState {
    fn default() -> Self {
        Self {
            burst: paste_burst::PasteBurstDetector::new(),
            pending_text: String::new(),
            pending_session: None,
            active_session: None,
            next_session_id: 1,
        }
    }
}

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
        if self.paste.pending_text.is_empty() {
            let continued_session = self.paste.active_session.and_then(|session| {
                let current_line = self.input.lines().get(self.input.cursor_row())?;
                let idx =
                    parse_paste_placeholder_before_cursor(current_line, self.input.cursor_col())?;
                (session.placeholder_index == Some(idx)).then_some(session)
            });
            self.paste.pending_session =
                Some(continued_session.unwrap_or_else(|| PasteSessionState {
                    id: self.paste.allocate_session_id(),
                    start: SelectionPoint {
                        row: self.input.cursor_row(),
                        col: self.input.cursor_col(),
                    },
                    placeholder_index: None,
                }));
            if let Some(session) = self.paste.pending_session {
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
        self.paste.pending_text.push_str(text);
        tracing::debug!(
            target: crate::logging::targets::APP_PASTE,
            event_name = "paste_queue_updated",
            message = "paste queue updated",
            outcome = "success",
            chunk_chars,
            pending_chars = self.paste.pending_text.chars().count(),
            had_pending_submit,
        );
    }
}
