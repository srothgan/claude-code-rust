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
}

impl ChatPurgeReplayOptions {
    pub const fn resize() -> Self {
        Self { reason: ChatPurgeReplayReason::Resize }
    }

    pub const fn chat_return_after_resize() -> Self {
        Self { reason: ChatPurgeReplayReason::ChatReturnAfterResize }
    }

    pub const fn post_turn_resize() -> Self {
        Self { reason: ChatPurgeReplayReason::PostTurnResize }
    }

    pub const fn session_replacement() -> Self {
        Self { reason: ChatPurgeReplayReason::SessionReplacement }
    }

    pub const fn terminal_history_out_of_sync() -> Self {
        Self { reason: ChatPurgeReplayReason::TerminalHistoryOutOfSync }
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
    rebuild_priority(next) >= rebuild_priority(current)
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

    #[test]
    fn coalesced_rebuilds_preserve_required_work_in_either_arrival_order() {
        use ChatRebuildKind::{FullscreenReturn, MutableViewport, PurgeReplay, VisibleScreen};
        for (first, second, expected) in [
            (VisibleScreen, MutableViewport, "visible"),
            (VisibleScreen, FullscreenReturn, "visible"),
            (PurgeReplay(ChatPurgeReplayOptions::resize()), VisibleScreen, "purge"),
            (PurgeReplay(ChatPurgeReplayOptions::resize()), MutableViewport, "purge"),
            (
                PurgeReplay(ChatPurgeReplayOptions::session_replacement()),
                PurgeReplay(ChatPurgeReplayOptions::resize()),
                "purge",
            ),
            (
                PurgeReplay(ChatPurgeReplayOptions::terminal_history_out_of_sync()),
                PurgeReplay(ChatPurgeReplayOptions::post_turn_resize()),
                "purge",
            ),
        ] {
            for requests in [[first, second], [second, first]] {
                let mut dirty = ChatSurfaceDirtyState::default();
                for request in requests {
                    match request {
                        MutableViewport => dirty.request_mutable_rebuild(),
                        FullscreenReturn => dirty.request_fullscreen_return_rebuild(),
                        VisibleScreen => dirty.request_visible_screen_rebuild(),
                        PurgeReplay(options) => dirty.request_purge_replay_rebuild(options),
                        ChatRebuildKind::None => unreachable!(),
                    }
                }
                match expected {
                    "visible" => assert_eq!(dirty.take_rebuild(), VisibleScreen),
                    "purge" => assert!(matches!(dirty.take_rebuild(), PurgeReplay(_))),
                    _ => unreachable!(),
                }
                assert!(dirty.take_repaint(), "the chosen rebuild still needs a frame");
            }
        }
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
