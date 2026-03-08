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

use crate::app::settings::{model_is_unavailable, model_status_label};
use crate::app::{App, SettingsTab};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::Color;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use std::fmt::Write as _;

use super::theme;

struct SettingDetail<'a> {
    title: &'a str,
    description: &'a str,
    supported: bool,
    options: Vec<String>,
    invalid: bool,
}

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

    frame.render_widget(render_tab_header(app.settings.active_tab), chunks[0]);

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
            app.settings.path.as_ref().map_or_else(
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

    let help = "Enter toggle/edit | Ctrl+S save | Esc save and close";
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(help, Style::default().fg(theme::RUST_ORANGE)))),
        chunks[3],
    );
}

fn render_config(frame: &mut Frame, area: ratatui::layout::Rect, app: &App) {
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

fn render_placeholder(frame: &mut Frame, area: ratatui::layout::Rect, title: &str) {
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
    lines.push(config_line(
        app.settings.selected_config_index == 0,
        "Fast mode",
        if app.settings.fast_mode_effective() { "On" } else { "Off" },
        app.settings.fast_mode_invalid(),
    ));
    if app.settings.fast_mode_invalid() {
        lines.push(Line::from(Span::styled(
            "  invalid value, using default",
            Style::default().fg(theme::STATUS_ERROR),
        )));
    }

    lines.push(Line::default());
    lines.push(config_line(
        app.settings.selected_config_index == 1,
        "Default permission mode",
        app.settings.default_permission_mode_effective().label(),
        app.settings.default_permission_mode_invalid(),
    ));
    if app.settings.default_permission_mode_invalid() {
        lines.push(Line::from(Span::styled(
            "  invalid value, using default",
            Style::default().fg(theme::STATUS_ERROR),
        )));
    }

    lines.push(Line::default());
    let model_unavailable = model_is_unavailable(app, app.settings.model_effective().as_deref());
    lines.push(config_line(
        app.settings.selected_config_index == 2,
        "Default model",
        &model_status_label(app.settings.model_effective().as_deref(), app),
        app.settings.model_invalid() || model_unavailable,
    ));
    if app.settings.model_invalid() {
        lines.push(Line::from(Span::styled(
            "  invalid value, using automatic",
            Style::default().fg(theme::STATUS_ERROR),
        )));
    } else if model_unavailable {
        lines.push(Line::from(Span::styled(
            "  model not advertised by current SDK session",
            Style::default().fg(theme::STATUS_ERROR),
        )));
    }

    lines
}

fn config_detail_lines(app: &App) -> Vec<Line<'static>> {
    let Some(detail) = selected_setting_detail(app) else {
        return vec![detail_text("No setting selected.")];
    };

    let mut lines = vec![detail_title(detail.title), detail_text(detail.description)];

    if !detail.supported {
        lines.push(Line::default());
        lines.push(unsupported_hint());
    }

    if !detail.options.is_empty() {
        lines.push(Line::default());
        lines.push(detail_section_title("Options"));
        lines.extend(detail.options.into_iter().map(detail_option));
    }

    if detail.invalid {
        lines.push(Line::default());
        lines.push(invalid_hint(true));
    }

    lines
}

fn selected_setting_detail(app: &App) -> Option<SettingDetail<'static>> {
    match app.settings.selected_config_index {
        0 => Some(SettingDetail {
            title: "Fast mode",
            description: "Controls the persisted fast-mode preference for future sessions.",
            supported: false,
            options: vec!["Off".to_owned(), "On".to_owned()],
            invalid: app.settings.fast_mode_invalid(),
        }),
        1 => Some(SettingDetail {
            title: "Default permission mode",
            description: "Stored in settings.json and applied to new sessions through the bridge.",
            supported: true,
            options: crate::app::settings::DefaultPermissionMode::ALL
                .iter()
                .map(|mode| mode.label().to_owned())
                .collect(),
            invalid: app.settings.default_permission_mode_invalid(),
        }),
        2 => {
            let current_model = app.settings.model_effective();
            let invalid =
                app.settings.model_invalid() || model_is_unavailable(app, current_model.as_deref());
            let options = if app.available_models.is_empty() {
                vec!["Automatic".to_owned(), "Connect to load available models".to_owned()]
            } else {
                let mut options = Vec::with_capacity(app.available_models.len() + 1);
                options.push("Automatic".to_owned());
                options.extend(app.available_models.iter().map(|model| model.display_name.clone()));
                options
            };

            Some(SettingDetail {
                title: "Default model",
                description: "Stored in settings.json and applied to new sessions through the bridge.",
                supported: true,
                options,
                invalid,
            })
        }
        _ => None,
    }
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

fn invalid_hint(invalid: bool) -> Line<'static> {
    if invalid {
        Line::from(Span::styled(
            "Invalid persisted value detected. Runtime uses the fallback until you save a valid selection."
                .to_owned(),
            Style::default().fg(theme::STATUS_ERROR),
        ))
    } else {
        Line::default()
    }
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

fn render_tab_header(active_tab: SettingsTab) -> Paragraph<'static> {
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

    Paragraph::new(Line::from(spans)).alignment(Alignment::Center)
}
