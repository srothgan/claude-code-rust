// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use super::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChatRenderTraceState {
    pub width: u16,
    pub content_height: usize,
    pub viewport_height: usize,
    pub auto_scroll: bool,
    pub pinned_to_bottom: bool,
    pub scroll_target: usize,
    pub scroll_offset: usize,
    pub max_scroll: usize,
    pub first_visible: usize,
    pub render_start: usize,
    pub local_scroll: usize,
    pub rendered_msgs: usize,
    pub last_rendered_idx: Option<usize>,
    pub rendered_line_count: usize,
    pub last_message_idx: Option<usize>,
    pub last_message_height: Option<usize>,
}

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

    /// Mark one presented frame at `now`, updating smoothed FPS.
    pub fn mark_frame_presented(&mut self, now: Instant) {
        let Some(prev) = self.last_frame_at.replace(now) else {
            return;
        };
        let dt = now.saturating_duration_since(prev).as_secs_f32();
        if dt <= f32::EPSILON {
            return;
        }
        let fps = (1.0 / dt).clamp(0.0, 240.0);
        self.fps_ema = Some(match self.fps_ema {
            Some(current) => current * 0.9 + fps * 0.1,
            None => fps,
        });
    }

    #[must_use]
    pub fn frame_fps(&self) -> Option<f32> {
        self.fps_ema
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
