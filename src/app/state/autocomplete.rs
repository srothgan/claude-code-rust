// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutocompleteKind {
    Mention,
    Slash,
    Subagent,
}

impl App {
    #[must_use]
    pub fn active_autocomplete_kind(&self) -> Option<AutocompleteKind> {
        if self.mention.is_some() {
            Some(AutocompleteKind::Mention)
        } else if self.slash.is_some() {
            Some(AutocompleteKind::Slash)
        } else if self.subagent.is_some() {
            Some(AutocompleteKind::Subagent)
        } else {
            None
        }
    }

    #[must_use]
    pub fn autocomplete_focus_available(&self) -> bool {
        self.mention.as_ref().is_some_and(mention::MentionState::has_selectable_candidates)
            || self.slash.is_some()
            || self.subagent.is_some()
    }
}
