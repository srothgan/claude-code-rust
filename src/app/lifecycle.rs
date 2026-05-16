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

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ChatRebuildKind {
    #[default]
    None,
    MutableViewport,
    FullscreenReturn,
    VisibleScreen,
    ResizePurgeReplay,
    SessionBoundary,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ChatSurfaceDirtyState {
    pub repaint: bool,
    pub rebuild: ChatRebuildKind,
}

impl ChatSurfaceDirtyState {
    pub fn request_repaint(&mut self) {
        self.repaint = true;
    }

    pub fn request_mutable_rebuild(&mut self) {
        self.rebuild = self.rebuild.max(ChatRebuildKind::MutableViewport);
        self.repaint = true;
    }

    pub fn request_visible_screen_rebuild(&mut self) {
        self.rebuild = self.rebuild.max(ChatRebuildKind::VisibleScreen);
        self.repaint = true;
    }

    pub fn request_fullscreen_return_rebuild(&mut self) {
        self.rebuild = self.rebuild.max(ChatRebuildKind::FullscreenReturn);
        self.repaint = true;
    }

    pub fn request_resize_purge_replay_rebuild(&mut self) {
        self.rebuild = self.rebuild.max(ChatRebuildKind::ResizePurgeReplay);
        self.repaint = true;
    }

    pub fn request_session_boundary_rebuild(&mut self) {
        self.rebuild = self.rebuild.max(ChatRebuildKind::SessionBoundary);
        self.repaint = true;
    }

    pub fn take_rebuild(&mut self) -> ChatRebuildKind {
        let rebuild = self.rebuild;
        self.rebuild = ChatRebuildKind::None;
        rebuild
    }

    pub fn take_repaint(&mut self) -> bool {
        let repaint = self.repaint;
        self.repaint = false;
        repaint
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceDirtyState {
    pub chat: ChatSurfaceDirtyState,
    pub fullscreen: FullscreenSurfaceDirtyState,
    pub terminal_mode: bool,
}

impl SurfaceDirtyState {
    pub fn initial_chat() -> Self {
        let mut dirty = Self::default();
        dirty.chat.request_repaint();
        dirty
    }

    pub fn active_surface_needs_draw(self, lifecycle: TerminalLifecycleState) -> bool {
        match lifecycle {
            TerminalLifecycleState::Running(SurfaceMode::Fullscreen(_)) => self.fullscreen.redraw,
            TerminalLifecycleState::Running(SurfaceMode::Chat)
            | TerminalLifecycleState::Bootstrapping => self.chat.repaint,
            TerminalLifecycleState::ReleasedToChild(_)
            | TerminalLifecycleState::Restoring
            | TerminalLifecycleState::Exited => false,
        }
    }

    pub fn clear_for_child_release(&mut self) {
        self.chat.repaint = false;
        self.chat.rebuild = ChatRebuildKind::None;
        self.fullscreen.redraw = false;
    }

    pub fn mark_view_transition(&mut self, from: SurfaceMode, to: SurfaceMode) {
        match (from, to) {
            (SurfaceMode::Chat, SurfaceMode::Fullscreen(_)) => {
                self.fullscreen.redraw = true;
                self.terminal_mode = true;
            }
            (SurfaceMode::Fullscreen(_), SurfaceMode::Chat) => {
                self.chat.request_fullscreen_return_rebuild();
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
        assert_eq!(dirty.chat.rebuild, ChatRebuildKind::FullscreenReturn);
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

    #[test]
    fn chat_visible_screen_rebuild_dominates_mutable_rebuild() {
        let mut dirty = ChatSurfaceDirtyState::default();

        dirty.request_mutable_rebuild();
        dirty.request_fullscreen_return_rebuild();
        dirty.request_visible_screen_rebuild();
        dirty.request_mutable_rebuild();
        dirty.request_fullscreen_return_rebuild();

        assert_eq!(dirty.rebuild, ChatRebuildKind::VisibleScreen);
        assert!(dirty.repaint);
    }

    #[test]
    fn chat_visible_screen_rebuild_dominates_fullscreen_return() {
        let mut dirty = ChatSurfaceDirtyState::default();

        dirty.request_fullscreen_return_rebuild();
        dirty.request_visible_screen_rebuild();
        dirty.request_fullscreen_return_rebuild();

        assert_eq!(dirty.rebuild, ChatRebuildKind::VisibleScreen);
        assert!(dirty.repaint);
    }

    #[test]
    fn chat_resize_purge_replay_rebuild_dominates_visible_rebuild() {
        let mut dirty = ChatSurfaceDirtyState::default();

        dirty.request_visible_screen_rebuild();
        dirty.request_resize_purge_replay_rebuild();
        dirty.request_mutable_rebuild();
        dirty.request_visible_screen_rebuild();

        assert_eq!(dirty.rebuild, ChatRebuildKind::ResizePurgeReplay);
        assert!(dirty.repaint);
    }

    #[test]
    fn chat_session_boundary_rebuild_dominates_visible_rebuild() {
        let mut dirty = ChatSurfaceDirtyState::default();

        dirty.request_visible_screen_rebuild();
        dirty.request_resize_purge_replay_rebuild();
        dirty.request_session_boundary_rebuild();
        dirty.request_mutable_rebuild();
        dirty.request_visible_screen_rebuild();
        dirty.request_resize_purge_replay_rebuild();

        assert_eq!(dirty.rebuild, ChatRebuildKind::SessionBoundary);
        assert!(dirty.repaint);
    }

    #[test]
    fn chat_rebuild_take_clears_rebuild_without_clearing_repaint() {
        let mut dirty = ChatSurfaceDirtyState::default();
        dirty.request_mutable_rebuild();

        assert_eq!(dirty.take_rebuild(), ChatRebuildKind::MutableViewport);
        assert_eq!(dirty.rebuild, ChatRebuildKind::None);
        assert!(dirty.repaint);
    }
}
