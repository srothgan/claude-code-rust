// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

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

pub const RESIZE_PURGE_REPLAY_MAX_ROWS: usize = 9_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatPurgeReplayReason {
    Resize,
    ChatReturnAfterResize,
    PostTurnResize,
    SessionReplacement,
    TerminalHistoryOutOfSync,
}

impl ChatPurgeReplayReason {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Resize => "resize",
            Self::ChatReturnAfterResize => "chat_return_after_resize",
            Self::PostTurnResize => "post_turn_resize",
            Self::SessionReplacement => "session_replacement",
            Self::TerminalHistoryOutOfSync => "terminal_history_out_of_sync",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChatPurgeReplayOptions {
    pub reason: ChatPurgeReplayReason,
    pub max_replay_rows: Option<usize>,
}

impl ChatPurgeReplayOptions {
    pub const fn resize() -> Self {
        Self {
            reason: ChatPurgeReplayReason::Resize,
            max_replay_rows: Some(RESIZE_PURGE_REPLAY_MAX_ROWS),
        }
    }

    pub const fn chat_return_after_resize() -> Self {
        Self {
            reason: ChatPurgeReplayReason::ChatReturnAfterResize,
            max_replay_rows: Some(RESIZE_PURGE_REPLAY_MAX_ROWS),
        }
    }

    pub const fn post_turn_resize() -> Self {
        Self {
            reason: ChatPurgeReplayReason::PostTurnResize,
            max_replay_rows: Some(RESIZE_PURGE_REPLAY_MAX_ROWS),
        }
    }

    pub const fn session_replacement() -> Self {
        Self { reason: ChatPurgeReplayReason::SessionReplacement, max_replay_rows: None }
    }

    pub const fn terminal_history_out_of_sync() -> Self {
        Self { reason: ChatPurgeReplayReason::TerminalHistoryOutOfSync, max_replay_rows: None }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ChatRebuildKind {
    #[default]
    None,
    MutableViewport,
    FullscreenReturn,
    VisibleScreen,
    PurgeReplay(ChatPurgeReplayOptions),
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
        self.request_rebuild(ChatRebuildKind::MutableViewport);
        self.repaint = true;
    }

    pub fn request_visible_screen_rebuild(&mut self) {
        self.request_rebuild(ChatRebuildKind::VisibleScreen);
        self.repaint = true;
    }

    pub fn request_fullscreen_return_rebuild(&mut self) {
        self.request_rebuild(ChatRebuildKind::FullscreenReturn);
        self.repaint = true;
    }

    pub fn request_purge_replay_rebuild(&mut self, options: ChatPurgeReplayOptions) {
        self.request_rebuild(ChatRebuildKind::PurgeReplay(options));
        self.repaint = true;
    }

    fn request_rebuild(&mut self, next: ChatRebuildKind) {
        if should_replace_rebuild(self.rebuild, next) {
            self.rebuild = next;
        }
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

const fn rebuild_priority(kind: ChatRebuildKind) -> u8 {
    match kind {
        ChatRebuildKind::None => 0,
        ChatRebuildKind::MutableViewport | ChatRebuildKind::FullscreenReturn => 1,
        ChatRebuildKind::VisibleScreen => 2,
        ChatRebuildKind::PurgeReplay(_) => 3,
    }
}

fn should_replace_rebuild(current: ChatRebuildKind, next: ChatRebuildKind) -> bool {
    let current_priority = rebuild_priority(current);
    let next_priority = rebuild_priority(next);

    if next_priority != current_priority {
        return next_priority > current_priority;
    }

    match (current, next) {
        (ChatRebuildKind::PurgeReplay(current), ChatRebuildKind::PurgeReplay(next)) => {
            should_replace_purge_replay(current, next)
        }
        _ => true,
    }
}

fn should_replace_purge_replay(
    current: ChatPurgeReplayOptions,
    next: ChatPurgeReplayOptions,
) -> bool {
    !matches!((current.max_replay_rows, next.max_replay_rows), (None, Some(_)))
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
    fn chat_purge_replay_rebuild_dominates_visible_rebuild() {
        let mut dirty = ChatSurfaceDirtyState::default();
        let options = ChatPurgeReplayOptions::resize();

        dirty.request_visible_screen_rebuild();
        dirty.request_purge_replay_rebuild(options);
        dirty.request_mutable_rebuild();
        dirty.request_visible_screen_rebuild();

        assert_eq!(dirty.rebuild, ChatRebuildKind::PurgeReplay(options));
        assert!(dirty.repaint);
    }

    #[test]
    fn later_chat_purge_replay_request_replaces_options() {
        let mut dirty = ChatSurfaceDirtyState::default();
        let resize_options = ChatPurgeReplayOptions::resize();
        let replacement_options = ChatPurgeReplayOptions::session_replacement();

        dirty.request_visible_screen_rebuild();
        dirty.request_purge_replay_rebuild(resize_options);
        dirty.request_mutable_rebuild();
        dirty.request_visible_screen_rebuild();
        dirty.request_purge_replay_rebuild(replacement_options);

        assert_eq!(dirty.rebuild, ChatRebuildKind::PurgeReplay(replacement_options));
        assert!(dirty.repaint);
    }

    #[test]
    fn session_replacement_purge_replay_survives_later_resize_request() {
        let mut dirty = ChatSurfaceDirtyState::default();
        let replacement_options = ChatPurgeReplayOptions::session_replacement();

        dirty.request_purge_replay_rebuild(replacement_options);
        dirty.request_mutable_rebuild();
        dirty.request_visible_screen_rebuild();
        dirty.request_purge_replay_rebuild(ChatPurgeReplayOptions::resize());

        assert_eq!(dirty.rebuild, ChatRebuildKind::PurgeReplay(replacement_options));
        assert!(dirty.repaint);
    }

    #[test]
    fn terminal_history_out_of_sync_purge_replay_survives_later_post_turn_resize_request() {
        let mut dirty = ChatSurfaceDirtyState::default();
        let out_of_sync_options = ChatPurgeReplayOptions::terminal_history_out_of_sync();

        dirty.request_purge_replay_rebuild(out_of_sync_options);
        dirty.request_purge_replay_rebuild(ChatPurgeReplayOptions::post_turn_resize());

        assert_eq!(dirty.rebuild, ChatRebuildKind::PurgeReplay(out_of_sync_options));
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
