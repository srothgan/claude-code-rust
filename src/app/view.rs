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

use crate::app::App;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveView {
    Chat,
    Config,
    Trusted,
}

pub fn set_active_view(app: &mut App, next: ActiveView) {
    if app.active_view == next {
        return;
    }

    clear_transient_view_state(app);
    app.active_view = next;
    app.needs_redraw = true;
}

fn clear_transient_view_state(app: &mut App) {
    app.selection = None;
    app.scrollbar_drag = None;
    app.active_paste_session = None;
    app.pending_paste_session = None;
    app.pending_paste_text.clear();
    app.pending_submit = None;
    app.mention = None;
    app.slash = None;
    app.subagent = None;
    app.release_focus_target(crate::app::FocusTarget::TodoList);
    app.release_focus_target(crate::app::FocusTarget::Permission);
    app.release_focus_target(crate::app::FocusTarget::Help);
    app.release_focus_target(crate::app::FocusTarget::Mention);
    app.paste_burst.on_non_char_key(Instant::now());
}

#[cfg(test)]
mod tests;
