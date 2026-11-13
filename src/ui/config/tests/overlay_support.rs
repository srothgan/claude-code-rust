// SPDX-License-Identifier: Apache-2.0
pub(super) use crate::app::config::*;
pub(super) use crate::app::plugins::{MarketplaceEntry, MarketplaceSourceEntry, PluginsViewTab};
pub(super) use crate::app::{App, ConfigTab, FullscreenView, SurfaceMode};
pub(super) use crossterm::event::KeyCode;
use crossterm::event::{Event, KeyEvent, KeyModifiers};
pub(super) use ratatui::{buffer::Buffer, layout::Rect};

pub(super) fn app(tab: ConfigTab) -> App {
    let mut app = App::test_default();
    app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::Config);
    app.config.active_tab = tab;
    app.session_runtime.session_id = Some("overlay-session".into());
    app
}
pub(super) fn key(app: &mut App, code: KeyCode) {
    crate::app::handle_terminal_event(app, Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
}
pub(super) fn paste(app: &mut App, text: &str) {
    crate::app::handle_terminal_event(app, Event::Paste(text.into()));
}
pub(super) fn draw(app: &mut App, width: u16, height: u16) -> Buffer {
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| crate::ui::render_fullscreen_surface(frame, app)).unwrap();
    terminal.backend().buffer().clone()
}
pub(super) fn rows(buffer: &Buffer, area: Rect) -> Vec<String> {
    (area.y..area.bottom())
        .map(|y| (area.x..area.right()).map(|x| buffer[(x, y)].symbol()).collect())
        .collect()
}
pub(super) fn boxed(buffer: &Buffer, title: &str) -> Rect {
    let y = (0..buffer.area.height)
        .find(|&y| {
            let line = &rows(buffer, Rect::new(0, y, buffer.area.width, 1))[0];
            line.contains(title) && line.contains('\u{250c}')
        })
        .unwrap();
    let x = (0..buffer.area.width).find(|&x| buffer[(x, y)].symbol() == "\u{250c}").unwrap();
    let right =
        (x + 1..buffer.area.width).find(|&x| buffer[(x, y)].symbol() == "\u{2510}").unwrap();
    let bottom =
        (y + 1..buffer.area.height).find(|&y| buffer[(x, y)].symbol() == "\u{2514}").unwrap();
    Rect::new(x + 1, y + 1, right - x - 1, bottom - y - 1)
}
pub(super) fn find(lines: &[String], text: &str) -> usize {
    lines
        .iter()
        .position(|s| s.contains(text))
        .unwrap_or_else(|| panic!("missing {text}:\n{}", lines.join("\n")))
}
pub(super) fn selected(lines: &[String], label: &str) {
    find(lines, &format!("\u{203a} {label}"));
}
pub(super) fn help(lines: &[String]) -> String {
    let start = find(lines, "Esc");
    lines[start.saturating_sub(1)..].join(" ").split_whitespace().collect::<Vec<_>>().join(" ")
}
pub(super) fn catalog(name: &str, description: &str) -> MarketplaceEntry {
    MarketplaceEntry {
        plugin_id: format!("{name}@market"),
        name: name.into(),
        description: Some(description.into()),
        marketplace_name: Some("market".into()),
        version: Some("catalog-version".into()),
        install_count: Some(123),
        source: None,
    }
}
pub(super) fn wrapped(lines: &[String], expected: &str) {
    let mut remaining = expected;
    for line in lines.iter().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        remaining = remaining
            .strip_prefix(line)
            .unwrap_or_else(|| panic!("unexpected fragment {line:?}, remaining {remaining:?}"));
        if let Some(next) = remaining.strip_prefix(' ') {
            remaining = next;
        }
    }
    assert!(remaining.is_empty(), "missing source tail: {remaining}");
}
pub(super) fn field(lines: &[String], label: &str, next: &str, expected: &str) {
    let start = find(lines, label);
    let end = if next.is_empty() { lines.len() } else { find(lines, next) };
    wrapped(&lines[start..end], expected);
}
