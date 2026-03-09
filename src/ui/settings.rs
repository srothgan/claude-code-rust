// Claude Code Rust - A native Rust terminal interface for Claude Code
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

use crate::app::settings::{
    config_settings, resolved_setting, setting_detail_options, setting_display_value,
    setting_invalid_hint,
};
use crate::app::{App, SettingsTab};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::Color;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use std::fmt::Write as _;

use super::theme;

pub fn render(frame: &mut Frame, app: &mut App) {
    let frame_area = frame.area();
    app.cached_frame_area = frame_area;

    let outer = Block::default()
        .borders(Borders::ALL)
        .title("Config")
        .border_style(Style::default().fg(theme::DIM));
    frame.render_widget(outer, frame_area);

    let inner = frame_area.inner(Margin { vertical: 1, horizontal: 1 });
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);

    render_tab_header(frame, chunks[0], app.settings.active_tab);

    match app.settings.active_tab {
        SettingsTab::Config => render_config(frame, chunks[1], app),
        SettingsTab::Status => render_placeholder(frame, chunks[1], "Status will land here."),
        SettingsTab::Usage => render_placeholder(frame, chunks[1], "Usage will land here."),
        SettingsTab::Mcp => render_placeholder(frame, chunks[1], "MCP will land here."),
    }

    let message = app
        .settings
        .last_error
        .as_deref()
        .map(str::to_owned)
        .or_else(|| app.settings.status_message.clone())
        .unwrap_or_else(|| {
            app.settings
                .selected_config_spec()
                .and_then(|spec| app.settings.path_for(spec.file))
                .or(app.settings.settings_path.as_ref())
                .map_or_else(
                    || "File: unavailable".to_owned(),
                    |path| format!("File: {}", path.display()),
                )
        });
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            message,
            Style::default().fg(if app.settings.last_error.is_some() {
                theme::STATUS_ERROR
            } else {
                theme::DIM
            }),
        ))),
        chunks[2],
    );

    let help = "Enter edit | Ctrl+S save | Esc save and close";
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(help, Style::default().fg(theme::RUST_ORANGE)))),
        chunks[3],
    );
}

fn render_config(frame: &mut Frame, area: Rect, app: &App) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(44), Constraint::Percentage(56)])
        .spacing(1)
        .split(padded_body_area(area));

    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .title("Settings")
            .border_style(Style::default().fg(theme::DIM)),
        columns[0],
    );
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .title("Details")
            .border_style(Style::default().fg(theme::DIM)),
        columns[1],
    );

    frame.render_widget(Paragraph::new(config_lines(app)), panel_body(columns[0]));
    frame.render_widget(
        Paragraph::new(config_detail_lines(app)).wrap(Wrap { trim: false }),
        panel_body(columns[1]),
    );
}

fn render_placeholder(frame: &mut Frame, area: Rect, title: &str) {
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(title, Style::default().fg(theme::DIM)))),
        padded_body_area(area),
    );
}

fn padded_body_area(area: Rect) -> Rect {
    area.inner(Margin { vertical: 1, horizontal: 2 })
}

fn panel_body(area: Rect) -> Rect {
    area.inner(Margin { vertical: 1, horizontal: 2 })
}

fn config_lines(app: &App) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for (index, spec) in config_settings().iter().enumerate() {
        let resolved = resolved_setting(app, spec);
        lines.push(config_line(
            app.settings.selected_config_index == index,
            spec.label,
            &setting_display_value(app, spec, &resolved),
            resolved.validation.is_invalid(),
        ));
        if let Some(hint) = setting_invalid_hint(spec, resolved.validation) {
            lines.push(Line::from(Span::styled(
                format!("  {hint}"),
                Style::default().fg(theme::STATUS_ERROR),
            )));
        }
        if index + 1 < config_settings().len() {
            lines.push(Line::default());
        }
    }
    lines
}

fn config_detail_lines(app: &App) -> Vec<Line<'static>> {
    let Some(spec) = app.settings.selected_config_spec() else {
        return vec![detail_text("No setting selected.")];
    };
    let resolved = resolved_setting(app, spec);

    let mut lines = vec![detail_title(spec.label), detail_text(spec.description)];

    if !spec.supported {
        lines.push(Line::default());
        lines.push(unsupported_hint());
    }

    let options = setting_detail_options(app, spec);
    if !options.is_empty() {
        lines.push(Line::default());
        lines.push(detail_section_title("Options"));
        lines.extend(options.into_iter().map(detail_option));
    }

    if let Some(hint) = setting_invalid_hint(spec, resolved.validation) {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            format!("Invalid persisted value detected. Runtime uses the fallback until you save a valid selection. {hint}"),
            Style::default().fg(theme::STATUS_ERROR),
        )));
    }

    lines
}

fn detail_title(text: &str) -> Line<'static> {
    Line::from(Span::styled(
        text.to_owned(),
        Style::default().fg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD),
    ))
}

fn detail_text(text: &str) -> Line<'static> {
    Line::from(Span::styled(text.to_owned(), Style::default().fg(Color::White)))
}

fn detail_section_title(text: &str) -> Line<'static> {
    Line::from(Span::styled(
        text.to_owned(),
        Style::default().fg(theme::DIM).add_modifier(Modifier::BOLD),
    ))
}

fn detail_option(text: String) -> Line<'static> {
    Line::from(vec![
        Span::styled("- ", Style::default().fg(theme::DIM)),
        Span::styled(text, Style::default().fg(Color::White)),
    ])
}

fn unsupported_hint() -> Line<'static> {
    Line::from(Span::styled(
        "Warning: this setting is not supported yet and will not affect sessions.".to_owned(),
        Style::default().fg(Color::Yellow),
    ))
}

fn config_line(selected: bool, label: &str, value: &str, invalid: bool) -> Line<'static> {
    let mut line = String::new();
    line.push(if selected { '>' } else { ' ' });
    line.push(' ');
    line.push_str(label);
    let marker = if invalid { " !" } else { "" };
    let _ = write!(&mut line, ": {value}{marker}");
    Line::from(Span::styled(line, Style::default().fg(Color::White)))
}

fn render_tab_header(frame: &mut Frame, area: Rect, active_tab: SettingsTab) {
    let mut spans = Vec::new();
    for (index, tab) in SettingsTab::ALL.iter().copied().enumerate() {
        if index > 0 {
            spans.push(Span::styled(" | ", Style::default().fg(theme::DIM)));
        }

        let style = if tab == active_tab {
            Style::default().fg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        spans.push(Span::styled(tab.title().to_owned(), style));
    }

    let line = Line::from(spans);
    let content_width = u16::try_from(line.width()).unwrap_or(area.width).min(area.width);
    let header_area = centered_line_area(area, content_width);
    frame.render_widget(Paragraph::new(line), header_area);
}

fn centered_line_area(area: Rect, content_width: u16) -> Rect {
    if content_width >= area.width {
        return area;
    }

    let offset = area.width.saturating_sub(content_width) / 2;
    Rect { x: area.x + offset, y: area.y, width: content_width, height: area.height }
}
