// Claude Code Rust - A native Rust terminal interface for Claude Code
// Copyright (C) 2025  Simon Peter Rothgang
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as
// published by the Free Software Foundation, either version 3 of the
// License, or (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

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
