// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use super::prelude::*;

impl App {
    /// Resolve the effective focus owner for Up/Down and other directional keys.
    #[must_use]
    pub fn focus_owner(&self) -> FocusOwner {
        self.focus.owner(self.focus_context())
    }

    #[must_use]
    pub fn has_draft_input_for_focus(&self) -> bool {
        !self.input.is_empty()
    }

    pub fn rebuild_chat_focus_from_state(&mut self) {
        if self.surface_mode != SurfaceMode::Chat {
            return;
        }

        self.normalize_focus_stack();

        if self.turn.pending_interaction_ids.is_empty() {
            clear_inline_interaction_focus(self);
        } else if self.focus_owner() == FocusOwner::Permission || !self.has_draft_input_for_focus()
        {
            focus_next_inline_interaction(self);
        } else {
            clear_inline_interaction_focus(self);
        }

        if self.autocomplete_focus_available() {
            self.claim_focus_target(FocusTarget::Mention);
        } else {
            self.release_focus_target(FocusTarget::Mention);
        }

        self.normalize_focus_stack();
    }

    /// Claim key routing for a navigation target.
    /// The latest claimant wins.
    pub fn claim_focus_target(&mut self, target: FocusTarget) {
        let context = self.focus_context();
        self.focus.claim(target, context);
    }

    /// Release key routing claim for a navigation target.
    pub fn release_focus_target(&mut self, target: FocusTarget) {
        let context = self.focus_context();
        self.focus.release(target, context);
    }

    /// Drop claims that are no longer valid for current state.
    pub fn normalize_focus_stack(&mut self) {
        let context = self.focus_context();
        self.focus.normalize(context);
    }

    #[must_use]
    fn focus_context(&self) -> FocusContext {
        FocusContext::new(
            self.autocomplete_focus_available(),
            !self.turn.pending_interaction_ids.is_empty(),
        )
    }
}
