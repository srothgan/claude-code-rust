// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::view::SurfaceMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseReason {
    SlashCommand,
    AuthFlow,
    ExternalEditor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalLifecycleState {
    Bootstrapping,
    Running(SurfaceMode),
    ReleasedToChild(ReleaseReason),
    Restoring,
    Exited,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct FullscreenSurfaceDirtyState {
    pub redraw: bool,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceDirtyState {
    pub fullscreen: FullscreenSurfaceDirtyState,
    pub terminal_mode: bool,
}

impl SurfaceDirtyState {
    pub fn mark_resize(&mut self, lifecycle: TerminalLifecycleState) {
        match lifecycle {
            TerminalLifecycleState::Running(SurfaceMode::Chat) => {}
            TerminalLifecycleState::Running(SurfaceMode::Fullscreen(_)) => {
                self.fullscreen.redraw = true;
            }
            TerminalLifecycleState::Bootstrapping
            | TerminalLifecycleState::ReleasedToChild(_)
            | TerminalLifecycleState::Restoring
            | TerminalLifecycleState::Exited => {}
        }
    }

    pub fn mark_view_transition(&mut self, from: SurfaceMode, to: SurfaceMode) {
        match (from, to) {
            (SurfaceMode::Chat, SurfaceMode::Fullscreen(_)) => {
                self.fullscreen.redraw = true;
                self.terminal_mode = true;
            }
            (SurfaceMode::Fullscreen(_), SurfaceMode::Chat) => {
                self.terminal_mode = true;
            }
            (SurfaceMode::Fullscreen(from_view), SurfaceMode::Fullscreen(to_view))
                if from_view != to_view =>
            {
                self.fullscreen.redraw = true;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::FullscreenView;

    #[test]
    fn view_transition_marks_chat_to_fullscreen() {
        let mut dirty = SurfaceDirtyState::default();

        dirty.mark_view_transition(
            SurfaceMode::Chat,
            SurfaceMode::Fullscreen(FullscreenView::Config),
        );

        assert!(dirty.fullscreen.redraw);
        assert!(dirty.terminal_mode);
    }

    #[test]
    fn view_transition_marks_fullscreen_to_chat() {
        let mut dirty = SurfaceDirtyState::default();

        dirty.mark_view_transition(
            SurfaceMode::Fullscreen(FullscreenView::Trusted),
            SurfaceMode::Chat,
        );

        assert!(dirty.terminal_mode);
        assert!(!dirty.fullscreen.redraw);
    }

    #[test]
    fn view_transition_marks_fullscreen_to_fullscreen() {
        let mut dirty = SurfaceDirtyState::default();

        dirty.mark_view_transition(
            SurfaceMode::Fullscreen(FullscreenView::Config),
            SurfaceMode::Fullscreen(FullscreenView::SessionPicker),
        );

        assert!(dirty.fullscreen.redraw);
        assert!(!dirty.terminal_mode);
    }

    #[test]
    fn view_transition_same_surface_is_noop() {
        let mut dirty = SurfaceDirtyState::default();

        dirty.mark_view_transition(SurfaceMode::Chat, SurfaceMode::Chat);

        assert_eq!(dirty, SurfaceDirtyState::default());
    }
}
