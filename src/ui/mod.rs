// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

mod autocomplete;
mod chat;
mod chat_view;
mod config;
mod diff;
mod footer;
mod header;
pub(crate) mod help;
mod highlight;
mod input;
mod layout;
mod markdown;
mod message;
mod tables;
pub mod theme;
mod todo;
mod tool_call;
mod trusted;

pub use message::{SpinnerState, measure_message_height_cached};

use crate::app::ActiveView;
use crate::app::App;
use ratatui::Frame;

pub fn render(frame: &mut Frame, app: &mut App) {
    match app.active_view {
        ActiveView::Chat => chat_view::render(frame, app),
        ActiveView::Config => config::render(frame, app),
        ActiveView::Trusted => trusted::render(frame, app),
    }
}
