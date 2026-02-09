// claude_rust — A native Rust terminal interface for Claude Code
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

// TODO: Replace custom InputState with tui-textarea when it supports ratatui 0.30
// Track: https://github.com/rhysd/tui-textarea/pull/118

use crate::app::App;
use crate::ui::theme;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

/// Horizontal padding to match header/footer inset.
const INPUT_PAD: u16 = 2;

/// Prompt column width: "❯ " = 2 columns (icon + space)
const PROMPT_WIDTH: u16 = 2;

/// Maximum input area height (lines) to prevent the input from consuming the entire screen.
const MAX_INPUT_HEIGHT: u16 = 12;

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let padded = Rect {
        x: area.x + INPUT_PAD,
        y: area.y,
        width: area.width.saturating_sub(INPUT_PAD * 2),
        height: area.height,
    };

    // Split into prompt icon column (fixed) and input column (remaining)
    let [prompt_area, input_area] =
        Layout::horizontal([Constraint::Length(PROMPT_WIDTH), Constraint::Min(1)])
            .areas(padded);

    // Render prompt icon
    let prompt = Line::from(Span::styled(
        format!("{} ", theme::PROMPT_CHAR),
        Style::default().fg(theme::RUST_ORANGE),
    ));
    frame.render_widget(Paragraph::new(prompt), prompt_area);

    if app.input.is_empty() {
        // Placeholder
        let placeholder = Line::from(Span::styled(
            "Type a message...",
            Style::default().fg(theme::DIM),
        ));
        frame.render_widget(Paragraph::new(placeholder), input_area);

        // Cursor at start of input area
        frame.set_cursor_position((input_area.x, input_area.y));
        return;
    }

    // Build input lines (no prefix needed -- icon is in its own column)
    let lines: Vec<Line> = app
        .input
        .lines
        .iter()
        .map(|text| Line::from(Span::raw(text.clone())))
        .collect();

    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, input_area);

    // Place terminal cursor accounting for visual wrapping.
    let content_width = input_area.width as usize;
    if content_width == 0 {
        return;
    }

    let mut visual_row: u16 = 0;
    for row in 0..app.input.lines.len() {
        let line_chars = app.input.lines[row].chars().count();
        let wrapped_lines = if content_width > 0 {
            ((line_chars + content_width) / content_width).max(1) as u16
        } else {
            1
        };

        if row == app.input.cursor_row {
            let cursor_col = app.input.cursor_col;
            let wrap_row = (cursor_col / content_width) as u16;
            let wrap_col = (cursor_col % content_width) as u16;

            let cursor_x = input_area.x + wrap_col;
            let cursor_y = input_area.y + visual_row + wrap_row;

            if cursor_x < input_area.right() && cursor_y < input_area.bottom() {
                frame.set_cursor_position((cursor_x, cursor_y));
            }
            return;
        }
        visual_row += wrapped_lines;
    }
}

/// Compute the number of visual lines the input occupies, accounting for wrapping.
/// Used by the layout to allocate the correct input area height.
pub fn visual_line_count(app: &App, area_width: u16) -> u16 {
    if app.input.is_empty() {
        return 1;
    }
    // Input content width = total area minus horizontal padding minus prompt column
    let content_width = area_width
        .saturating_sub(INPUT_PAD * 2)
        .saturating_sub(PROMPT_WIDTH) as usize;
    if content_width == 0 {
        return app.input.line_count();
    }

    let mut total: u16 = 0;
    for line in &app.input.lines {
        let chars = line.chars().count();
        let wrapped = ((chars + content_width) / content_width).max(1) as u16;
        total = total.saturating_add(wrapped);
    }
    total.min(MAX_INPUT_HEIGHT)
}
