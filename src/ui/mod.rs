// SPDX-License-Identifier: Apache-2.0

pub(crate) mod activity_rows;
mod autocomplete;
mod config;
mod copy;
mod diff;
pub(crate) mod footer_rows;
mod git_diff;
pub(crate) mod help;
mod highlight;
pub(crate) mod host_tips;
pub(crate) mod inline_chat_rows;
pub(crate) mod input;
pub(crate) mod input_rows;
pub(crate) mod layout_control;
pub(crate) mod live_rows;
mod markdown;
mod message;
pub(crate) mod message_layout;
mod message_rows;
mod session_picker;
pub(crate) mod spinner_verbs;
mod terminal_output;
pub mod theme;
mod tool_call;
pub(crate) mod tool_display;
mod trusted;
mod two_column_list;
mod update;
mod welcome;
mod wrap;

pub use message::SpinnerState;
pub(crate) use welcome::{ferris_hello_live, welcome_is_whole_transcript};

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
        SurfaceMode::Fullscreen(FullscreenView::Update) => update::render(frame, app),
        SurfaceMode::Fullscreen(FullscreenView::Copy) => copy::render(frame, app),
        SurfaceMode::Fullscreen(FullscreenView::Diff) => git_diff::render(frame, app),
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
