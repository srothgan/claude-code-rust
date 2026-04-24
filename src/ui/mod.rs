// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

mod autocomplete;
mod chat;
mod chat_view;
pub(crate) mod composer_measure;
mod config;
mod diff;
mod document_table;
mod footer;
pub(crate) mod footer_rows;
pub(crate) mod help;
mod highlight;
pub(crate) mod inline_chat_rows;
mod input;
pub(crate) mod input_rows;
mod layout;
mod markdown;
mod message;
mod message_rows;
mod session_picker;
pub mod theme;
mod todo;
mod tool_call;
mod trusted;
mod two_column_list;
mod wrap;

pub use message::{SpinnerState, measure_message_height_cached};

use crate::app::ActiveView;
use crate::app::App;
use ratatui::Frame;

pub fn render_chat_surface(frame: &mut Frame, app: &mut App) {
    chat_view::render(frame, app);
}

pub fn render_fullscreen_surface(frame: &mut Frame, app: &mut App) {
    match app.active_view {
        ActiveView::Config => config::render(frame, app),
        ActiveView::Trusted => trusted::render(frame, app),
        ActiveView::SessionPicker => session_picker::render(frame, app),
        ActiveView::Chat => {
            debug_assert!(false, "render_fullscreen_surface called while chat is active");
            chat_view::render(frame, app);
        }
    }
}

pub fn render(frame: &mut Frame, app: &mut App) {
    match app.active_view {
        ActiveView::Chat => render_chat_surface(frame, app),
        ActiveView::Config | ActiveView::Trusted | ActiveView::SessionPicker => {
            render_fullscreen_surface(frame, app);
        }
    }
}

pub(crate) fn refresh_selection_snapshot(app: &mut App) {
    let Some(selection) = app.selection else {
        return;
    };

    match (app.active_view, selection.kind) {
        (ActiveView::Chat, crate::app::SelectionKind::Chat) => {
            chat::refresh_selection_snapshot(app);
        }
        (ActiveView::Chat, crate::app::SelectionKind::Input) => {
            input::refresh_selection_snapshot(app);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{ActiveView, App};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
        let buffer = terminal.backend().buffer();
        let width = usize::from(buffer.area.width);
        buffer
            .content
            .chunks(width)
            .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn render_chat_surface_draws_chat_view() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = App::test_default();

        terminal.draw(|frame| render_chat_surface(frame, &mut app)).expect("draw");

        let rendered = buffer_text(&terminal);
        assert!(rendered.contains("Claude Rust"));
    }

    #[test]
    fn render_fullscreen_surface_draws_config_view() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = App::test_default();
        app.active_view = ActiveView::Config;

        terminal.draw(|frame| render_fullscreen_surface(frame, &mut app)).expect("draw");

        let rendered = buffer_text(&terminal);
        assert!(rendered.contains("Config"));
    }

    #[test]
    fn render_fullscreen_surface_draws_trusted_view() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = App::test_default();
        app.active_view = ActiveView::Trusted;

        terminal.draw(|frame| render_fullscreen_surface(frame, &mut app)).expect("draw");

        let rendered = buffer_text(&terminal);
        assert!(rendered.contains("Unknown Project"));
    }

    #[test]
    fn render_fullscreen_surface_draws_session_picker_view() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = App::test_default();
        app.active_view = ActiveView::SessionPicker;

        terminal.draw(|frame| render_fullscreen_surface(frame, &mut app)).expect("draw");

        let rendered = buffer_text(&terminal);
        assert!(rendered.contains("Resume Session"));
    }
}
