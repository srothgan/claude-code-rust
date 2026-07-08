// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::prelude::*;

impl App {
    #[must_use]
    pub fn active_turn_assistant_idx(&self) -> Option<usize> {
        self.turn.assistant_message_idx.filter(|&idx| {
            self.transcript
                .messages
                .get(idx)
                .is_some_and(|msg| matches!(msg.role, MessageRole::Assistant))
        })
    }

    pub fn bind_active_turn_assistant(&mut self, idx: usize) {
        self.turn.assistant_message_idx = self
            .transcript
            .messages
            .get(idx)
            .is_some_and(|msg| matches!(msg.role, MessageRole::Assistant))
            .then_some(idx);
    }

    pub fn bind_active_turn_assistant_to_tail(&mut self) {
        if let Some(idx) = self.transcript.messages.len().checked_sub(1) {
            self.bind_active_turn_assistant(idx);
        } else {
            self.clear_active_turn_assistant();
        }
    }

    pub fn clear_active_turn_assistant(&mut self) {
        self.turn.assistant_message_idx = None;
    }

    pub fn bump_session_scope_epoch(&mut self) {
        self.session_runtime.bump_session_scope_epoch();
    }

    pub fn clear_session_runtime_identity(&mut self) {
        self.session_runtime.clear_identity();
        self.sdk_inventory.clear_rewind_targets();
    }

    pub fn reconcile_trust_state_from_preferences_and_cwd(&mut self) {
        let lookup = crate::app::trust::store::read_status(
            &self.config.committed_preferences_document,
            Path::new(&self.cwd_raw),
        );
        self.trust.project_key = lookup.project_key;
        self.trust.status = if lookup.trusted {
            crate::app::trust::TrustStatus::Trusted
        } else {
            crate::app::trust::TrustStatus::Untrusted
        };
        self.trust.selection = crate::app::trust::TrustSelection::Yes;
        self.trust.last_error = self
            .config
            .preferences_path
            .is_none()
            .then(|| "Trust preferences path is not available".to_owned());
    }

    pub fn reconcile_runtime_from_persisted_settings_change(&mut self) {
        self.reconcile_trust_state_from_preferences_and_cwd();
    }

    pub(crate) fn shift_active_turn_assistant_for_insert(&mut self, idx: usize) {
        if let Some(owner_idx) = self.turn.assistant_message_idx
            && idx <= owner_idx
        {
            self.turn.assistant_message_idx = Some(owner_idx.saturating_add(1));
        }
    }

    pub(crate) fn shift_active_turn_assistant_for_remove(&mut self, idx: usize) {
        let Some(owner_idx) = self.turn.assistant_message_idx else {
            return;
        };
        self.turn.assistant_message_idx = match idx.cmp(&owner_idx) {
            std::cmp::Ordering::Less => Some(owner_idx.saturating_sub(1)),
            std::cmp::Ordering::Equal => None,
            std::cmp::Ordering::Greater => Some(owner_idx),
        };
    }
}
