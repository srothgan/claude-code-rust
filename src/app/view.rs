// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::lifecycle::TerminalLifecycleState;
use crate::app::App;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveView {
    Chat,
    Config,
    Trusted,
    SessionPicker,
}

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

impl ActiveView {
    #[must_use]
    pub fn surface_mode(self) -> SurfaceMode {
        match self {
            Self::Chat => SurfaceMode::Chat,
            Self::Config => SurfaceMode::Fullscreen(FullscreenView::Config),
            Self::Trusted => SurfaceMode::Fullscreen(FullscreenView::Trusted),
            Self::SessionPicker => SurfaceMode::Fullscreen(FullscreenView::SessionPicker),
        }
    }

    #[must_use]
    pub fn fullscreen_view(self) -> Option<FullscreenView> {
        match self {
            Self::Chat => None,
            Self::Config => Some(FullscreenView::Config),
            Self::Trusted => Some(FullscreenView::Trusted),
            Self::SessionPicker => Some(FullscreenView::SessionPicker),
        }
    }
}

impl FullscreenView {
    #[must_use]
    pub fn active_view(self) -> ActiveView {
        match self {
            Self::Config => ActiveView::Config,
            Self::Trusted => ActiveView::Trusted,
            Self::SessionPicker => ActiveView::SessionPicker,
        }
    }
}

impl SurfaceMode {
    #[must_use]
    pub fn active_view(self) -> ActiveView {
        match self {
            Self::Chat => ActiveView::Chat,
            Self::Fullscreen(view) => view.active_view(),
        }
    }
}

pub fn set_active_view(app: &mut App, next: ActiveView) {
    if app.active_view == next {
        return;
    }

    let previous_surface = app.surface_mode;
    let next_surface = next.surface_mode();
    clear_transient_view_state(app);
    app.active_view = next;
    app.surface_mode = next_surface;
    app.surface_dirty.mark_view_transition(previous_surface, next_surface);
    app.chat_render.invalidate_live_anchor();
    if let TerminalLifecycleState::Running(_) = app.terminal_lifecycle {
        app.terminal_lifecycle = TerminalLifecycleState::Running(next_surface);
    }
    if next == ActiveView::Chat {
        app.rebuild_chat_focus_from_state();
    }
    app.request_active_surface_repaint();
}

fn clear_transient_view_state(app: &mut App) {
    app.active_paste_session = None;
    app.pending_paste_session = None;
    app.pending_paste_text.clear();
    app.pending_submit = None;
    app.mention = None;
    app.slash = None;
    app.subagent = None;
    if app.active_view == ActiveView::Config {
        app.config.overlay = None;
    }
    app.release_focus_target(crate::app::FocusTarget::Mention);
    app.paste_burst.on_non_char_key(Instant::now());
}

#[cfg(test)]
mod tests;
