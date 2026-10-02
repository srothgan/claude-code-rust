// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use super::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutInvalidation {
    MessageChanged(usize),
    MessagesFrom(usize),
    Global,
}

impl App {
    pub(crate) fn request_chat_repaint(&mut self) {
        self.surface_dirty.chat.request_repaint();
    }

    pub(crate) fn request_chat_mutable_rebuild(&mut self) {
        self.surface_dirty.chat.request_mutable_rebuild();
    }

    pub(crate) fn request_chat_visible_rebuild(&mut self) {
        self.surface_dirty.chat.request_visible_screen_rebuild();
    }

    pub(crate) fn request_chat_fullscreen_return_rebuild(&mut self) {
        self.surface_dirty.chat.request_fullscreen_return_rebuild();
    }

    pub(crate) fn request_chat_purge_replay_rebuild(&mut self, options: ChatPurgeReplayOptions) {
        self.surface_dirty.chat.request_purge_replay_rebuild(options);
    }

    pub(crate) fn request_fullscreen_repaint(&mut self) {
        self.surface_dirty.fullscreen.redraw = true;
    }

    pub(crate) fn request_active_surface_repaint(&mut self) {
        match self.terminal_lifecycle {
            TerminalLifecycleState::Running(SurfaceMode::Fullscreen(_)) => {
                self.request_fullscreen_repaint();
            }
            TerminalLifecycleState::Running(SurfaceMode::Chat)
            | TerminalLifecycleState::Bootstrapping => {
                self.request_chat_repaint();
            }
            TerminalLifecycleState::ReleasedToChild(_)
            | TerminalLifecycleState::Restoring
            | TerminalLifecycleState::Exited => {}
        }
    }

    pub fn invalidate_layout(&mut self, _level: LayoutInvalidation) {
        self.chat_render.clear_measurements();
        self.chat_render.invalidate_live_anchor();
        self.request_chat_repaint();
    }

    pub(crate) fn invalidate_message_set<I>(&mut self, indices: I)
    where
        I: IntoIterator<Item = usize>,
    {
        let unique: BTreeSet<_> =
            indices.into_iter().filter(|&idx| idx < self.transcript.messages.len()).collect();
        if !unique.is_empty() {
            self.invalidate_layout(LayoutInvalidation::Global);
        }
    }
}
