// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use crate::app::App;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::widgets::Paragraph;

use super::footer_rows;

const FOOTER_PAD: u16 = 2;

pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.height == 0 {
        return;
    }

    let padded = Rect {
        x: area.x.saturating_add(FOOTER_PAD),
        y: area.y,
        width: area.width.saturating_sub(FOOTER_PAD * 2),
        height: area.height,
    };

    let [first_row, second_row] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(padded);
    let serialized = footer_rows::serialize_footer_rows(app, padded.width);

    frame.render_widget(Paragraph::new(serialized.rows[0].clone()), first_row);
    frame.render_widget(Paragraph::new(serialized.rows[1].clone()), second_row);
}

#[cfg(test)]
mod tests {
    use super::render;
    use crate::app::{App, ModeState};
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
    fn footer_render_paints_two_rows_from_serialized_content() {
        let backend = TestBackend::new(80, 2);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = App::test_default();
        app.mode = Some(ModeState {
            current_mode_id: "default".to_owned(),
            current_mode_name: "default".to_owned(),
            available_modes: Vec::new(),
        });

        terminal.draw(|frame| render(frame, frame.area(), &mut app)).expect("draw");

        let rendered = buffer_text(&terminal);
        assert!(rendered.contains("[default]"));
        assert!(rendered.contains("Loc:"));
    }
}
