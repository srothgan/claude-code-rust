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
mod markdown;
mod message;
mod message_rows;
mod session_picker;
mod spinner_verbs;
pub mod theme;
mod tool_call;
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
mod tests {
    use super::*;
    use crate::app::{App, FullscreenView, SurfaceMode};
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
    fn render_fullscreen_surface_draws_config_view() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = App::test_default();
        app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::Config);

        terminal.draw(|frame| render_fullscreen_surface(frame, &mut app)).expect("draw");

        let rendered = buffer_text(&terminal);
        assert!(rendered.contains("Config"));
    }

    #[test]
    fn render_fullscreen_surface_draws_trusted_view() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = App::test_default();
        app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::Trusted);

        terminal.draw(|frame| render_fullscreen_surface(frame, &mut app)).expect("draw");

        let rendered = buffer_text(&terminal);
        assert!(rendered.contains("Unknown Project"));
    }

    #[test]
    fn render_fullscreen_surface_draws_session_picker_view() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = App::test_default();
        app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::SessionPicker);

        terminal.draw(|frame| render_fullscreen_surface(frame, &mut app)).expect("draw");

        let rendered = buffer_text(&terminal);
        assert!(rendered.contains("Resume Session"));
    }
}
