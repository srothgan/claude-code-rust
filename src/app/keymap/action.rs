// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::catalog::{KeyActionDescriptor, action_catalog, action_descriptor};
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum KeyAction {
    App(AppAction),
    Input(InputAction),
    Autocomplete(AutocompleteAction),
    Interaction(InteractionAction),
    Terminal(TerminalAction),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AppAction {
    Quit,
    ClearInputOrQuit,
    Redraw,
    CancelTurn,
    SubmitInput,
    FocusPromptOrAcceptSuggestion,
    CycleMode,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum InputAction {
    MoveCharLeft,
    MoveCharRight,
    MoveWordLeft,
    MoveWordRight,
    MoveLineStart,
    MoveLineEnd,
    MoveUp,
    MoveDown,
    DeleteCharBefore,
    DeleteCharAfter,
    DeleteWordBefore,
    DeleteWordAfter,
    KillLineStart,
    KillLineEnd,
    Yank,
    Undo,
    Redo,
    InsertNewline,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AutocompleteAction {
    MovePrevious,
    MoveNext,
    Confirm,
    Cancel,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum InteractionAction {
    MovePrevious,
    MoveNext,
    MoveStart,
    MoveEnd,
    Confirm,
    Cancel,
    FocusNext,
    ToggleSelection,
    ToggleNotes,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TerminalAction {
    Suspend,
}

impl KeyAction {
    pub fn id(self) -> &'static str {
        self.descriptor().id
    }

    pub fn label(self) -> &'static str {
        self.descriptor().label
    }

    pub fn description(self) -> &'static str {
        self.descriptor().description
    }

    pub fn descriptor(self) -> &'static KeyActionDescriptor {
        action_descriptor(self).unwrap_or_else(|| {
            unreachable!("key action {self:?} is missing from the action catalog")
        })
    }

    pub fn from_id(id: &str) -> Option<Self> {
        action_catalog()
            .iter()
            .find(|descriptor| descriptor.id == id)
            .map(|descriptor| descriptor.action)
    }
}
