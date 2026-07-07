// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

mod autocomplete;
mod config;
mod diff;
mod document_table;
pub(crate) mod footer_rows;
pub(crate) mod help;
mod highlight;
pub(crate) mod inline_chat_rows;
pub(crate) mod input;
pub(crate) mod input_rows;
pub(crate) mod live_rows;
mod markdown;
mod message;
mod message_rows;
mod session_picker;
mod spinner_verbs;
pub mod theme;
mod tool_call;
pub(crate) mod tool_display;
mod trusted;
mod two_column_list;
mod welcome;
mod wrap;

pub use message::SpinnerState;

use crate::app::App;
use crate::app::{FullscreenView, SurfaceMode};
use ratatui::Frame;

pub fn render_fullscreen_surface(frame: &mut Frame, app: &mut App) {
    match app.surface_mode {
        SurfaceMode::Fullscreen(FullscreenView::Config) => config::render(frame, app),
        SurfaceMode::Fullscreen(FullscreenView::Trusted) => trusted::render(frame, app),
        SurfaceMode::Fullscreen(FullscreenView::SessionPicker) => {
            session_picker::render(frame, app);
        }
        SurfaceMode::Chat => {
            debug_assert!(false, "chat is rendered by the inline terminal session");
        }
    }
}

pub fn render(frame: &mut Frame, app: &mut App) {
    match app.surface_mode {
        SurfaceMode::Chat => {
            let _ = (frame, app);
            debug_assert!(false, "chat is rendered by the inline terminal session");
        }
        SurfaceMode::Fullscreen(_) => render_fullscreen_surface(frame, app),
    }
}

#[cfg(test)]
mod tests;
