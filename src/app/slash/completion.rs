// SPDX-License-Identifier: Apache-2.0
//! Slash menu visibility and explicit dismissal belong to one state owner.

use super::{SlashContext, SlashDetection, SlashState};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CompletionRequest {
    Automatic,
    Arguments,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SlashCommandToken {
    pub row: usize,
    pub name: String,
}

#[derive(Debug)]
enum Dismissal {
    CommandName(SlashCommandToken),
    Arguments(SlashCommandToken),
}

#[derive(Debug, Default)]
enum MenuState {
    #[default]
    Inactive,
    Visible(SlashState),
    Dismissed(Dismissal),
}

#[derive(Debug, Default)]
pub struct SlashAutocomplete {
    state: MenuState,
}

impl SlashAutocomplete {
    pub(super) fn request_at(&self, detection: Option<&SlashDetection>) -> CompletionRequest {
        match (self.visible(), detection) {
            (Some(visible), Some(detection))
                if visible.trigger_row == detection.command_token.row
                    && matches!((&visible.context, &detection.context),
                        (SlashContext::Argument { command: previous, .. },
                         SlashContext::Argument { command: current, .. }) if previous == current) =>
            {
                CompletionRequest::Arguments
            }
            _ => CompletionRequest::Automatic,
        }
    }

    #[must_use]
    pub fn visible(&self) -> Option<&SlashState> {
        match &self.state {
            MenuState::Visible(state) => Some(state),
            MenuState::Inactive | MenuState::Dismissed(_) => None,
        }
    }

    #[must_use]
    pub fn visible_mut(&mut self) -> Option<&mut SlashState> {
        match &mut self.state {
            MenuState::Visible(state) => Some(state),
            MenuState::Inactive | MenuState::Dismissed(_) => None,
        }
    }

    #[must_use]
    pub fn is_visible(&self) -> bool {
        matches!(self.state, MenuState::Visible(_))
    }

    pub(crate) fn clear(&mut self) {
        self.state = MenuState::Inactive;
    }

    pub(crate) fn show(&mut self, state: SlashState) {
        self.state = MenuState::Visible(state);
    }

    pub(super) fn take_visible(&mut self) -> Option<SlashState> {
        if !self.is_visible() {
            return None;
        }
        match std::mem::take(&mut self.state) {
            MenuState::Visible(state) => Some(state),
            MenuState::Inactive | MenuState::Dismissed(_) => None,
        }
    }

    pub(super) fn dismiss(&mut self, detection: SlashDetection) {
        self.state = MenuState::Dismissed(match detection.context {
            SlashContext::CommandName => Dismissal::CommandName(detection.command_token),
            SlashContext::Argument { .. } => Dismissal::Arguments(detection.command_token),
        });
    }

    /// Argument edits and passive refreshes preserve dismissal. Editing the
    /// command name, leaving this command, or explicitly requesting completion
    /// begins a new completion interaction.
    pub(super) fn suppresses(&mut self, detection: Option<&SlashDetection>) -> bool {
        let MenuState::Dismissed(dismissal) = &self.state else {
            return false;
        };
        let suppressed = detection.is_some_and(|detection| match dismissal {
            Dismissal::CommandName(token) => token == &detection.command_token,
            Dismissal::Arguments(token) => {
                token == &detection.command_token
                    && matches!(detection.context, SlashContext::Argument { .. })
            }
        });
        if !suppressed {
            self.clear();
        }
        suppressed
    }
}
