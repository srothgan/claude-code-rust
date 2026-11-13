// SPDX-License-Identifier: Apache-2.0
use claude_code_rust::app::{App, FullscreenView, SurfaceMode, handle_terminal_event};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

#[test]
fn ctrl_q_still_quits_from_config() {
    let mut app = App::test_default();
    app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::Config);

    handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL)),
    );

    assert!(app.shutdown_requested());
}
