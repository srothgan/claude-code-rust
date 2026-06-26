// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::prelude::*;

impl App {
    #[must_use]
    pub fn active_turn_assistant_idx(&self) -> Option<usize> {
        self.active_turn_assistant_message_idx.filter(|&idx| {
            self.messages.get(idx).is_some_and(|msg| matches!(msg.role, MessageRole::Assistant))
        })
    }

    pub fn bind_active_turn_assistant(&mut self, idx: usize) {
        self.active_turn_assistant_message_idx = self
            .messages
            .get(idx)
            .is_some_and(|msg| matches!(msg.role, MessageRole::Assistant))
            .then_some(idx);
    }

    pub fn bind_active_turn_assistant_to_tail(&mut self) {
        if let Some(idx) = self.messages.len().checked_sub(1) {
            self.bind_active_turn_assistant(idx);
        } else {
            self.clear_active_turn_assistant();
        }
    }

    pub fn clear_active_turn_assistant(&mut self) {
        self.active_turn_assistant_message_idx = None;
    }

    pub fn bump_session_scope_epoch(&mut self) {
        self.session_scope_epoch = self.session_scope_epoch.saturating_add(1);
    }

    pub fn clear_session_runtime_identity(&mut self) {
        self.session_id = None;
        self.current_model = None;
        self.mode = None;
        self.fast_mode_state = model::FastModeState::Off;
        self.session_usage = SessionUsageState::default();
        self.rewind_targets.clear();
        self.rewind_targets_session_id = None;
        self.rewind_targets_in_flight = false;
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
        if let Some(owner_idx) = self.active_turn_assistant_message_idx
            && idx <= owner_idx
        {
            self.active_turn_assistant_message_idx = Some(owner_idx.saturating_add(1));
        }
    }

    pub(crate) fn shift_active_turn_assistant_for_remove(&mut self, idx: usize) {
        let Some(owner_idx) = self.active_turn_assistant_message_idx else {
            return;
        };
        self.active_turn_assistant_message_idx = match idx.cmp(&owner_idx) {
            std::cmp::Ordering::Less => Some(owner_idx.saturating_sub(1)),
            std::cmp::Ordering::Equal => None,
            std::cmp::Ordering::Greater => Some(owner_idx),
        };
    }
}
