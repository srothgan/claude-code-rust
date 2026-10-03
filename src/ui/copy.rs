// SPDX-License-Identifier: Apache-2.0

use crate::app::App;
use crate::ui::theme;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Margin},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};

pub(crate) fn render(frame: &mut Frame, app: &App) {
    let Some(picker) = &app.copy_picker else { return };
    let area = frame.area().inner(Margin { vertical: 1, horizontal: 1 });
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1), Constraint::Length(3)])
        .split(area);
    frame.render_widget(
        Paragraph::new("Copy last response\nChoose the full response or a code block.")
            .wrap(Wrap { trim: false }),
        sections[0],
    );
    let items =
        picker.choices.iter().map(|choice| ListItem::new(Line::from(Span::raw(&choice.label))));
    let mut state = ListState::default().with_selected(Some(picker.selected));
    frame.render_stateful_widget(
        List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme::DIM)),
            )
            .highlight_style(
                Style::default()
                    .fg(Color::Black)
                    .bg(theme::RUST_ORANGE)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("› "),
        sections[1],
        &mut state,
    );
    let help = picker.error.as_ref().map_or_else(
        || "↑/↓ choose · Enter copy · Esc cancel".to_owned(),
        |error| format!("{error}\nEnter retry · Esc cancel"),
    );
    frame.render_widget(Paragraph::new(help).wrap(Wrap { trim: false }), sections[2]);
}
