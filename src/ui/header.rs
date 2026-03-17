// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use crate::app::App;
use crate::ui::theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

const HEADER_PAD: u16 = 2;

pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    let padded = Rect {
        x: area.x.saturating_add(HEADER_PAD),
        y: area.y,
        width: area.width.saturating_sub(HEADER_PAD * 2),
        height: area.height,
    };

    if app.cached_header_line.is_none() {
        let sep = || Span::styled("  \u{2502}  ", Style::default().fg(theme::DIM));
        let white = Style::default().fg(ratatui::style::Color::White);

        let mut spans = vec![
            Span::styled("\u{1F980} ", Style::default().fg(theme::RUST_ORANGE)),
            Span::styled(
                "Claude Code Rust",
                Style::default().fg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD),
            ),
            sep(),
            Span::styled("Model: ", Style::default().fg(theme::DIM)),
            Span::styled(app.model_name.clone(), white),
            sep(),
            Span::styled("Loc: ", Style::default().fg(theme::DIM)),
            Span::styled(app.cwd.clone(), white),
        ];

        if let Some(branch) = &app.git_branch {
            spans.push(sep());
            spans.push(Span::styled("Branch: ", Style::default().fg(theme::DIM)));
            spans.push(Span::styled(branch.clone(), white));
        }

        app.cached_header_line = Some(Line::from(spans));
    }

    if let Some(line) = &app.cached_header_line {
        frame.render_widget(Paragraph::new(line.clone()), padded);
    }
}
