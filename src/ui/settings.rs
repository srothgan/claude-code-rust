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
    OverlayFocus, config_settings, model_overlay_options, resolved_setting, setting_detail_options,
    setting_display_value, setting_invalid_hint, supported_effort_levels_for_model,
};
use crate::app::{App, SettingsTab};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::Color;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
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

    if app.settings.model_and_effort_overlay().is_some() {
        render_model_and_effort_overlay(frame, frame_area, app);
    }

    let message = app.settings.last_error.clone().unwrap_or_default();
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

    let help = if app.settings.model_and_effort_overlay().is_some() {
        ""
    } else {
        "Space edit | Enter close | Esc close"
    };
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

fn render_model_and_effort_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let Some(overlay) = app.settings.model_and_effort_overlay() else {
        return;
    };
    let overlay_area = model_and_effort_overlay_rect(area);
    let inner = overlay_area.inner(Margin { vertical: 1, horizontal: 1 });
    let model_lines = model_overlay_lines(app);
    let effort_lines = effort_overlay_lines(app);
    let (model_height, effort_height) = model_and_effort_section_heights(inner.height);
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Length(model_height),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(effort_height),
        ])
        .split(inner);
    frame.render_widget(Clear, overlay_area);
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .title("Model and Thinking Effort")
            .border_style(Style::default().fg(theme::RUST_ORANGE)),
        overlay_area,
    );

    let model_focused = overlay.focus == OverlayFocus::Model;
    let effort_focused = overlay.focus == OverlayFocus::Effort;
    render_overlay_instructions(frame, sections[0]);
    render_overlay_header(frame, sections[1], "Model", model_focused);
    render_overlay_separator(frame, sections[3]);
    render_overlay_header(frame, sections[4], "Thinking effort", effort_focused);

    let model_scroll = model_overlay_scroll(app, sections[2].height, sections[2].width);
    frame.render_widget(
        Paragraph::new(model_lines).scroll((model_scroll, 0)).wrap(Wrap { trim: false }),
        sections[2],
    );

    frame.render_widget(Paragraph::new(effort_lines).wrap(Wrap { trim: false }), sections[5]);
}

fn model_overlay_lines(app: &App) -> Vec<Line<'static>> {
    let Some(overlay) = app.settings.model_and_effort_overlay() else {
        return Vec::new();
    };
    let mut lines = model_overlay_options(app)
        .into_iter()
        .flat_map(|option| {
            let selected = option.id == overlay.selected_model;
            let marker = if selected { ">" } else { " " };
            let support = if option.supports_effort { "effort" } else { "no effort" };
            let mut lines = vec![Line::from(Span::styled(
                format!("{marker} {} [{support}]", option.display_name),
                overlay_line_style(selected, overlay.focus == OverlayFocus::Model),
            ))];
            if let Some(description) = option.description {
                lines.push(Line::from(Span::styled(
                    format!("  {description}"),
                    Style::default().fg(theme::DIM),
                )));
            }
            lines.push(Line::default());
            lines
        })
        .collect::<Vec<_>>();
    if !lines.is_empty() {
        lines.pop();
    }
    lines
}

fn effort_overlay_lines(app: &App) -> Vec<Line<'static>> {
    let Some(overlay) = app.settings.model_and_effort_overlay() else {
        return Vec::new();
    };
    let levels = supported_effort_levels_for_model(app, &overlay.selected_model);
    if levels.is_empty() {
        return vec![
            Line::from(Span::styled(
                "  Thinking effort is not available for the selected model.",
                Style::default().fg(theme::DIM),
            )),
            Line::default(),
            Line::from(Span::styled(
                format!("  Saved value: {}", overlay.selected_effort.label()),
                Style::default().fg(Color::White),
            )),
        ];
    }
    let mut lines = levels
        .into_iter()
        .flat_map(|level| {
            let selected = level == overlay.selected_effort;
            vec![
                Line::from(Span::styled(
                    format!("{} {}", if selected { ">" } else { " " }, level.label()),
                    overlay_line_style(selected, overlay.focus == OverlayFocus::Effort),
                )),
                Line::from(Span::styled(
                    format!("  {}", level.description()),
                    Style::default().fg(theme::DIM),
                )),
                Line::default(),
            ]
        })
        .collect::<Vec<_>>();
    if !lines.is_empty() {
        lines.pop();
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

fn overlay_line_style(selected: bool, focused: bool) -> Style {
    if selected && focused {
        Style::default().fg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD)
    } else if selected {
        Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    }
}

fn render_overlay_header(frame: &mut Frame, area: Rect, title: &str, focused: bool) {
    let prefix = if focused { "> " } else { "  " };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("{prefix}{title}"),
            if focused {
                Style::default().fg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::DIM)
            },
        ))),
        area,
    );
}

fn render_overlay_instructions(frame: &mut Frame, area: Rect) {
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                "Tab switches model/effort | Enter confirm | Esc cancel",
                Style::default().fg(theme::DIM),
            )),
            Line::default(),
        ]),
        area,
    );
}

fn render_overlay_separator(frame: &mut Frame, area: Rect) {
    let width = usize::from(area.width.max(1));
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "─".repeat(width),
            Style::default().fg(theme::DIM),
        ))),
        area,
    );
}

fn model_and_effort_overlay_rect(area: Rect) -> Rect {
    const SMALL_TERMINAL_WIDTH: u16 = 90;
    const SMALL_TERMINAL_HEIGHT: u16 = 20;
    const OVERLAY_WIDTH_PERCENT: u16 = 90;
    const OVERLAY_HEIGHT_PERCENT: u16 = 84;

    if area.width < SMALL_TERMINAL_WIDTH || area.height < SMALL_TERMINAL_HEIGHT {
        return area;
    }

    let overlay_width = ((u32::from(area.width) * u32::from(OVERLAY_WIDTH_PERCENT)) / 100)
        .try_into()
        .unwrap_or(area.width)
        .clamp(1, area.width);
    let overlay_height = ((u32::from(area.height) * u32::from(OVERLAY_HEIGHT_PERCENT)) / 100)
        .try_into()
        .unwrap_or(area.height)
        .clamp(1, area.height);

    centered_rect_with_size(area, overlay_width, overlay_height)
}

fn model_and_effort_section_heights(inner_height: u16) -> (u16, u16) {
    const CHROME_HEIGHT: u16 = 5;
    const DEFAULT_EFFORT_HEIGHT: u16 = 8;

    let content_height = inner_height.saturating_sub(CHROME_HEIGHT);
    match content_height {
        0 => (0, 0),
        1 => (1, 0),
        _ => {
            let effort_height = DEFAULT_EFFORT_HEIGHT.min(content_height.saturating_sub(1));
            let model_height = content_height.saturating_sub(effort_height);
            (model_height, effort_height)
        }
    }
}

fn model_overlay_scroll(app: &App, viewport_height: u16, viewport_width: u16) -> u16 {
    let Some(overlay) = app.settings.model_and_effort_overlay() else {
        return 0;
    };
    let options = model_overlay_options(app);
    if options.is_empty() || viewport_height == 0 || viewport_width == 0 {
        return 0;
    }

    let selected_index =
        options.iter().position(|option| option.id == overlay.selected_model).unwrap_or(0);
    let selected_start = options
        .iter()
        .take(selected_index)
        .enumerate()
        .map(|(index, option)| {
            model_overlay_option_height(option, index + 1 == options.len(), viewport_width)
        })
        .sum::<usize>();
    let selected_height = model_overlay_option_height(
        &options[selected_index],
        selected_index + 1 == options.len(),
        viewport_width,
    );
    let viewport_height = usize::from(viewport_height);

    if selected_start + selected_height <= viewport_height {
        0
    } else {
        (selected_start + selected_height - viewport_height) as u16
    }
}

fn model_overlay_option_height(
    option: &crate::app::settings::OverlayModelOption,
    is_last: bool,
    viewport_width: u16,
) -> usize {
    let support = if option.supports_effort { "effort" } else { "no effort" };
    let title = format!("  {} [{support}]", option.display_name);
    let mut height = wrapped_line_count(&title, viewport_width);
    if let Some(description) = option.description.as_deref() {
        height += wrapped_line_count(&format!("  {description}"), viewport_width);
    }
    height + usize::from(!is_last)
}

fn wrapped_line_count(text: &str, viewport_width: u16) -> usize {
    let width = Line::raw(text).width().max(1);
    width.div_ceil(usize::from(viewport_width.max(1)))
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

fn centered_rect_with_size(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::model_overlay_scroll;
    use crate::agent::model::{AvailableModel, EffortLevel};
    use crate::app::App;
    use crate::app::settings::{ModelAndEffortOverlayState, OverlayFocus, SettingsOverlayState};

    #[test]
    fn model_overlay_scroll_keeps_selected_multiline_model_visible() {
        let mut app = App::test_default();
        app.available_models = vec![
            AvailableModel::new("default", "Default")
                .description("Opus 4.6")
                .supports_effort(true)
                .supported_effort_levels(vec![
                    EffortLevel::Low,
                    EffortLevel::Medium,
                    EffortLevel::High,
                ]),
            AvailableModel::new("opus-1m", "Opus (1M context)")
                .description("Extra usage")
                .supports_effort(true)
                .supported_effort_levels(vec![
                    EffortLevel::Low,
                    EffortLevel::Medium,
                    EffortLevel::High,
                ]),
            AvailableModel::new("sonnet", "Sonnet")
                .description("Everyday tasks")
                .supports_effort(true)
                .supported_effort_levels(vec![
                    EffortLevel::Low,
                    EffortLevel::Medium,
                    EffortLevel::High,
                ]),
            AvailableModel::new("haiku", "Haiku").description("Fastest").supports_effort(false),
        ];
        app.settings.overlay =
            Some(SettingsOverlayState::ModelAndEffort(ModelAndEffortOverlayState {
                focus: OverlayFocus::Model,
                selected_model: "sonnet".to_owned(),
                selected_effort: EffortLevel::High,
            }));

        assert_eq!(model_overlay_scroll(&app, 6, 40), 5);
    }

    #[test]
    fn model_overlay_scroll_accounts_for_wrapped_lines() {
        let mut app = App::test_default();
        app.available_models = vec![
            AvailableModel::new("default", "Default")
                .description("1234567890")
                .supports_effort(true)
                .supported_effort_levels(vec![
                    EffortLevel::Low,
                    EffortLevel::Medium,
                    EffortLevel::High,
                ]),
            AvailableModel::new("haiku", "Haiku").supports_effort(false),
        ];
        app.settings.overlay =
            Some(SettingsOverlayState::ModelAndEffort(ModelAndEffortOverlayState {
                focus: OverlayFocus::Model,
                selected_model: "haiku".to_owned(),
                selected_effort: EffortLevel::Medium,
            }));

        assert_eq!(model_overlay_scroll(&app, 4, 10), 3);
    }
}
