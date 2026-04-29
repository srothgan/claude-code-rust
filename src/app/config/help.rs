// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::ConfigHelpSection;
use crate::app::App;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub(super) fn handle_key(app: &mut App, key: KeyEvent) -> bool {
    match (key.code, key.modifiers) {
        (KeyCode::Left, KeyModifiers::NONE) => {
            set_section(app, app.config.help_section.prev());
            true
        }
        (KeyCode::Right, KeyModifiers::NONE) => {
            set_section(app, app.config.help_section.next());
            true
        }
        (KeyCode::Up, KeyModifiers::NONE) => {
            let count = crate::ui::help::help_item_count(app, app.config.help_section);
            app.config.help_dialog.move_up(count, app.config.help_visible_count);
            true
        }
        (KeyCode::Down, KeyModifiers::NONE) => {
            let count = crate::ui::help::help_item_count(app, app.config.help_section);
            app.config.help_dialog.move_down(count, app.config.help_visible_count);
            true
        }
        _ => false,
    }
}

fn set_section(app: &mut App, next: ConfigHelpSection) {
    if app.config.help_section != next {
        app.config.help_section = next;
        app.config.help_dialog = crate::app::dialog::DialogState::default();
        app.config.help_visible_count = 0;
    }
}
