// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

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
