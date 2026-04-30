// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::lifecycle::TerminalLifecycleState;
use crate::app::App;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FullscreenView {
    Config,
    Trusted,
    SessionPicker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceMode {
    Chat,
    Fullscreen(FullscreenView),
}

impl SurfaceMode {
    #[must_use]
    pub fn fullscreen_view(self) -> Option<FullscreenView> {
        let Self::Fullscreen(view) = self else {
            return None;
        };
        Some(view)
    }
}

pub fn set_surface_mode(app: &mut App, next: SurfaceMode) {
    if app.surface_mode == next {
        return;
    }

    let previous_surface = app.surface_mode;
    clear_transient_view_state(app);
    app.surface_mode = next;
    app.surface_dirty.mark_view_transition(previous_surface, next);
    app.chat_render.invalidate_live_anchor();
    if let TerminalLifecycleState::Running(_) = app.terminal_lifecycle {
        app.terminal_lifecycle = TerminalLifecycleState::Running(next);
    }
    if next == SurfaceMode::Chat {
        app.rebuild_chat_focus_from_state();
    }
    app.request_active_surface_repaint();
}

pub fn set_fullscreen_view(app: &mut App, next: FullscreenView) {
    set_surface_mode(app, SurfaceMode::Fullscreen(next));
}

pub fn set_chat_surface(app: &mut App) {
    set_surface_mode(app, SurfaceMode::Chat);
}

fn clear_transient_view_state(app: &mut App) {
    app.active_paste_session = None;
    app.pending_paste_session = None;
    app.pending_paste_text.clear();
    app.pending_submit = None;
    app.mention = None;
    app.slash = None;
    app.subagent = None;
    if app.surface_mode == SurfaceMode::Fullscreen(FullscreenView::Config) {
        app.config.overlay = None;
    }
    app.release_focus_target(crate::app::FocusTarget::Mention);
    app.paste_burst.on_non_char_key(Instant::now());
}

#[cfg(test)]
mod tests;
