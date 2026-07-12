// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::app::{App, FullscreenView, SurfaceMode, UpdatePromptAction, UpdatePromptState};
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
    assert!(rendered.contains("Settings"));
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

#[test]
fn render_fullscreen_surface_draws_update_view() {
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let mut app = App::test_default();
    app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::Update);
    app.update_prompt = Some(UpdatePromptState {
        current_version: "0.13.4".to_owned(),
        latest_version: "0.14.0".to_owned(),
        release_url: "https://example.invalid".to_owned(),
        install_method: crate::install_method::InstallMethod::Npm,
        selected: UpdatePromptAction::Install,
        last_error: None,
    });

    terminal.draw(|frame| render_fullscreen_surface(frame, &mut app)).expect("draw");

    let rendered = buffer_text(&terminal);
    assert!(rendered.contains("Update Available"));
}
