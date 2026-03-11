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

mod overlay;

use crate::app::settings::{
    OutputStyle, OverlayFocus, config_settings, model_overlay_options, resolved_setting,
    setting_detail_options, setting_display_value, setting_invalid_hint,
    supported_effort_levels_for_model,
};
use crate::app::{App, SettingsTab};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::Color;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use std::fmt::Write as _;

use super::theme;
use overlay::{
    OverlayChrome, OverlayLayoutSpec, overlay_line_style, render_overlay_header,
    render_overlay_separator as shared_render_overlay_separator, render_overlay_shell,
};

const SETTINGS_LIMITATION_HINT: &str = "Currently, not all settings are supported by claude-rs. This project uses the official Anthropic Claude Agent SDK, which limits claude-rs implementing all Claude Code settings.";
const MIN_CONFIG_PANEL_HEIGHT: u16 = 3;

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
    } else if app.settings.output_style_overlay().is_some() {
        render_output_style_overlay(frame, frame_area, app);
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

    let help = if app.settings.model_and_effort_overlay().is_some()
        || app.settings.output_style_overlay().is_some()
    {
        ""
    } else {
        "Space edit | Enter close | Esc close"
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(help, Style::default().fg(theme::RUST_ORANGE)))),
        chunks[3],
    );
}

fn render_config(frame: &mut Frame, area: Rect, app: &mut App) {
    if compact_settings_layout(area) {
        let sections = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(MIN_CONFIG_PANEL_HEIGHT),
                Constraint::Length(reserved_hint_height(area)),
            ])
            .split(area);

        frame.render_widget(
            Block::default()
                .borders(Borders::ALL)
                .title("Settings")
                .border_style(Style::default().fg(theme::DIM)),
            sections[0],
        );
        render_config_list(frame, panel_body(sections[0]), app, true);
        render_settings_limitation_hint(frame, hint_area(area, sections[1]));
        return;
    }

    let content = padded_body_area(area);
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(MIN_CONFIG_PANEL_HEIGHT),
            Constraint::Length(reserved_hint_height(content)),
        ])
        .split(content);
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(44), Constraint::Percentage(56)])
        .spacing(1)
        .split(sections[0]);

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

    render_config_list(frame, panel_body(columns[0]), app, false);
    frame.render_widget(
        Paragraph::new(config_detail_lines(app)).wrap(Wrap { trim: false }),
        panel_body(columns[1]),
    );
    render_settings_limitation_hint(frame, hint_area(content, sections[1]));
}

fn render_config_list(frame: &mut Frame, area: Rect, app: &mut App, compact: bool) {
    let mut state = ListState::default()
        .with_selected(Some(app.settings.selected_config_index))
        .with_offset(app.settings.config_scroll_offset);
    let list = List::new(config_items(app, compact, area.width));
    frame.render_stateful_widget(list, area, &mut state);
    app.settings.config_scroll_offset = state.offset();
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

fn reserved_hint_height(base_area: Rect) -> u16 {
    if base_area.height == 0 {
        return 0;
    }

    let desired = settings_hint_height(hint_text_width(base_area)).max(1);
    let max_hint = base_area.height.saturating_sub(MIN_CONFIG_PANEL_HEIGHT).max(1);
    desired.min(max_hint)
}

fn hint_text_width(base_area: Rect) -> u16 {
    base_area.width.saturating_sub(4)
}

fn hint_area(base_area: Rect, hint_row: Rect) -> Rect {
    Rect {
        x: base_area.x.saturating_add(2),
        y: hint_row.y,
        width: hint_text_width(base_area),
        height: hint_row.height,
    }
}

fn settings_hint_height(viewport_width: u16) -> u16 {
    if viewport_width == 0 {
        return 0;
    }

    let line_count = Paragraph::new(SETTINGS_LIMITATION_HINT)
        .wrap(Wrap { trim: false })
        .line_count(viewport_width);
    u16::try_from(line_count).unwrap_or(u16::MAX)
}

fn render_settings_limitation_hint(frame: &mut Frame, area: Rect) {
    frame.render_widget(
        Paragraph::new(SETTINGS_LIMITATION_HINT)
            .style(Style::default().fg(Color::White))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn config_items(app: &App, compact: bool, viewport_width: u16) -> Vec<ListItem<'static>> {
    let mut items = Vec::new();
    for (index, spec) in config_settings().iter().enumerate() {
        let resolved = resolved_setting(app, spec);
        let mut lines = vec![config_line(
            app.settings.selected_config_index == index,
            spec.label,
            &setting_display_value(app, spec, &resolved),
            resolved.validation.is_invalid(),
        )];
        if let Some(hint) = setting_invalid_hint(spec, resolved.validation) {
            lines.extend(wrap_styled_text(
                &format!("  {hint}"),
                Style::default().fg(theme::STATUS_ERROR),
                viewport_width,
            ));
        }
        if compact && !spec.supported {
            lines.extend(unsupported_hint_lines(viewport_width));
        }
        if index + 1 < config_settings().len() {
            lines.push(Line::default());
        }
        items.push(ListItem::new(lines));
    }
    items
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
    let rendered = render_overlay_shell(
        frame,
        area,
        OverlayLayoutSpec {
            min_width: 1,
            min_height: 1,
            width_percent: 90,
            height_percent: 84,
            preferred_height: u16::MAX,
            fullscreen_below: Some((90, 20)),
            inner_margin: Margin { vertical: 1, horizontal: 1 },
        },
        OverlayChrome {
            title: "Model and Thinking Effort",
            subtitle: None,
            help: Some("Tab switches model/effort | Enter confirm | Esc cancel"),
        },
    );
    let model_lines = model_overlay_lines(app);
    let effort_lines = effort_overlay_lines(app);
    let (model_height, effort_height) = model_and_effort_section_heights(rendered.body_area.height);
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(model_height),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(effort_height),
        ])
        .split(rendered.body_area);

    let model_focused = overlay.focus == OverlayFocus::Model;
    let effort_focused = overlay.focus == OverlayFocus::Effort;
    render_overlay_header(frame, sections[0], "Model", model_focused);
    shared_render_overlay_separator(frame, sections[2]);
    render_overlay_header(frame, sections[3], "Thinking effort", effort_focused);

    let model_scroll = model_overlay_scroll(app, sections[1].height, sections[1].width);
    frame.render_widget(
        Paragraph::new(model_lines).scroll((model_scroll, 0)).wrap(Wrap { trim: false }),
        sections[1],
    );

    frame.render_widget(Paragraph::new(effort_lines).wrap(Wrap { trim: false }), sections[4]);
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

fn render_output_style_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let rendered = render_overlay_shell(
        frame,
        area,
        OverlayLayoutSpec {
            min_width: 72,
            min_height: 8,
            width_percent: 84,
            height_percent: 80,
            preferred_height: 14,
            fullscreen_below: Some((72, 16)),
            inner_margin: Margin { vertical: 1, horizontal: 2 },
        },
        OverlayChrome {
            title: "Preferred output style",
            subtitle: Some("This changes how Claude Code communicates with you"),
            help: Some("Enter confirm | Esc cancel"),
        },
    );
    frame.render_widget(
        Paragraph::new(output_style_overlay_lines(app)).wrap(Wrap { trim: false }),
        rendered.body_area,
    );
}

fn output_style_overlay_lines(app: &App) -> Vec<Line<'static>> {
    let Some(overlay) = app.settings.output_style_overlay() else {
        return Vec::new();
    };

    let mut lines = Vec::new();
    for (index, style) in OutputStyle::ALL.iter().copied().enumerate() {
        let selected = style == overlay.selected;
        let marker = if selected { ">" } else { " " };
        lines.push(Line::from(vec![
            Span::styled(format!("{marker} {}. ", index + 1), overlay_line_style(selected, true)),
            Span::styled(style.label().to_owned(), overlay_line_style(selected, true)),
        ]));
        lines.push(Line::from(Span::styled(
            format!("   {}", style.description()),
            Style::default().fg(theme::DIM),
        )));
        if index + 1 < OutputStyle::ALL.len() {
            lines.push(Line::default());
        }
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
        "  Warning: not supported yet; this setting will not affect sessions.".to_owned(),
        Style::default().fg(Color::Yellow),
    ))
}

fn unsupported_hint_lines(viewport_width: u16) -> Vec<Line<'static>> {
    wrap_styled_text(
        "  Warning: not supported yet; this setting will not affect sessions.",
        Style::default().fg(Color::Yellow),
        viewport_width,
    )
}

fn wrap_styled_text(text: &str, style: Style, viewport_width: u16) -> Vec<Line<'static>> {
    let width = usize::from(viewport_width.max(1));
    if width == 0 {
        return Vec::new();
    }

    let indent_len = text.chars().take_while(|ch| ch.is_whitespace()).count();
    let indent = text.chars().take(indent_len).collect::<String>();
    let content = text.chars().skip(indent_len).collect::<String>();
    let indent_width = Line::raw(indent.as_str()).width();
    let available_width = width.saturating_sub(indent_width).max(1);

    let mut wrapped = Vec::new();
    let mut current = String::new();

    for word in content.split_whitespace() {
        push_wrapped_word(&mut wrapped, &mut current, word, available_width);
    }

    if !current.is_empty() {
        wrapped.push(format!("{indent}{current}"));
    }

    if wrapped.is_empty() {
        wrapped.push(indent);
    }

    wrapped.into_iter().map(|line| Line::from(Span::styled(line, style))).collect()
}

fn push_wrapped_word(
    wrapped: &mut Vec<String>,
    current: &mut String,
    word: &str,
    available_width: usize,
) {
    let mut remaining = word;
    while !remaining.is_empty() {
        let candidate = if current.is_empty() {
            remaining.to_owned()
        } else {
            format!("{current} {remaining}")
        };

        if Line::raw(candidate.as_str()).width() <= available_width {
            current.clear();
            current.push_str(&candidate);
            break;
        }

        if current.is_empty() {
            let split = split_to_width(remaining, available_width);
            let head = remaining[..split].to_owned();
            wrapped.push(head);
            remaining = &remaining[split..];
        } else {
            wrapped.push(std::mem::take(current));
        }
    }
}

fn split_to_width(text: &str, available_width: usize) -> usize {
    let mut width = 0;
    let mut split = 0;
    for (byte_index, ch) in text.char_indices() {
        let ch_width = Line::raw(ch.to_string()).width();
        if byte_index > 0 && width + ch_width > available_width {
            break;
        }
        width += ch_width;
        split = byte_index + ch.len_utf8();
    }
    split.max(1)
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

const COMPACT_SETTINGS_MIN_WIDTH: u16 = 90;
const COMPACT_SETTINGS_MIN_HEIGHT: u16 = 20;

fn compact_settings_layout(area: Rect) -> bool {
    area.width < COMPACT_SETTINGS_MIN_WIDTH || area.height < COMPACT_SETTINGS_MIN_HEIGHT
}

#[allow(dead_code)]
fn render_overlay_separator_legacy(frame: &mut Frame, area: Rect) {
    let width = usize::from(area.width.max(1));
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "─".repeat(width),
            Style::default().fg(theme::DIM),
        ))),
        area,
    );
}

#[allow(dead_code)]
fn overlay_rect(area: Rect, spec: OverlayLayoutSpec) -> Rect {
    if spec
        .fullscreen_below
        .is_some_and(|(min_width, min_height)| area.width < min_width || area.height < min_height)
    {
        return area;
    }

    let overlay_width = ((u32::from(area.width) * u32::from(spec.width_percent)) / 100)
        .try_into()
        .unwrap_or(area.width)
        .max(spec.min_width)
        .clamp(1, area.width);
    let percent_height = ((u32::from(area.height) * u32::from(spec.height_percent)) / 100)
        .try_into()
        .unwrap_or(area.height)
        .clamp(1, area.height);
    let overlay_height = percent_height
        .min(spec.preferred_height.min(area.height))
        .max(spec.min_height.min(area.height));

    centered_rect_with_size(area, overlay_width, overlay_height)
}

fn model_and_effort_section_heights(inner_height: u16) -> (u16, u16) {
    const CHROME_HEIGHT: u16 = 3;
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

#[allow(dead_code)]
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
    use super::{
        SETTINGS_LIMITATION_HINT, compact_settings_layout, model_overlay_scroll,
        settings_hint_height,
    };
    use crate::agent::model::{AvailableModel, EffortLevel};
    use crate::app::App;
    use crate::app::settings::{
        ModelAndEffortOverlayState, OutputStyle, OutputStyleOverlayState, OverlayFocus, SettingId,
        SettingsOverlayState, config_settings,
    };
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

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

    #[test]
    fn output_style_overlay_lists_expected_options() {
        let mut app = App::test_default();
        app.settings.overlay = Some(SettingsOverlayState::OutputStyle(OutputStyleOverlayState {
            selected: OutputStyle::Explanatory,
        }));

        let rendered = super::output_style_overlay_lines(&app)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();

        assert!(rendered.iter().any(|line| line.contains("1. Default")));
        assert!(rendered.iter().any(|line| line.contains("2. Explanatory")));
        assert!(rendered.iter().any(|line| line.contains("3. Learning")));
    }

    #[test]
    fn output_style_details_show_unsupported_warning() {
        let mut app = App::test_default();
        app.settings.selected_config_index = config_settings()
            .iter()
            .position(|spec| spec.id == SettingId::OutputStyle)
            .expect("output style row");

        let rendered = super::config_detail_lines(&app)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();

        assert!(rendered.iter().any(|line| line.contains("not supported yet")));
    }

    #[test]
    fn compact_settings_layout_triggers_for_small_tuis() {
        assert!(compact_settings_layout(Rect::new(0, 0, 89, 25)));
        assert!(compact_settings_layout(Rect::new(0, 0, 100, 19)));
        assert!(!compact_settings_layout(Rect::new(0, 0, 90, 20)));
    }

    #[test]
    fn compact_settings_list_inlines_unsupported_warning() {
        fn buffer_text(buffer: &Buffer) -> String {
            let width = usize::from(buffer.area.width);
            buffer
                .content
                .chunks(width)
                .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n")
        }

        let backend = TestBackend::new(80, 16);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = App::test_default();
        app.active_view = crate::app::ActiveView::Settings;
        app.settings.selected_config_index = config_settings()
            .iter()
            .position(|spec| spec.id == SettingId::OutputStyle)
            .expect("output style row");

        terminal
            .draw(|frame| {
                super::render(frame, &mut app);
            })
            .expect("draw");

        let rendered = buffer_text(terminal.backend().buffer());

        assert!(rendered.contains("not supported yet"));
    }

    #[test]
    fn compact_settings_warning_wraps_on_narrow_widths() {
        fn buffer_lines(buffer: &Buffer) -> Vec<String> {
            let width = usize::from(buffer.area.width);
            buffer
                .content
                .chunks(width)
                .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
                .collect()
        }

        let backend = TestBackend::new(42, 20);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = App::test_default();
        app.active_view = crate::app::ActiveView::Settings;
        app.settings.selected_config_index = config_settings()
            .iter()
            .position(|spec| spec.id == SettingId::OutputStyle)
            .expect("output style row");

        terminal
            .draw(|frame| {
                super::render(frame, &mut app);
            })
            .expect("draw");

        let rendered = buffer_lines(terminal.backend().buffer());

        let warning_lines = rendered
            .iter()
            .filter(|line| {
                line.contains("Warning: not supported yet;")
                    || line.contains("this setting")
                    || line.contains("affect")
                    || line.contains("sessions.")
            })
            .count();

        assert!(warning_lines >= 2);
    }

    #[test]
    fn render_updates_config_scroll_offset_to_keep_selection_visible() {
        let backend = TestBackend::new(80, 16);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = App::test_default();
        app.active_view = crate::app::ActiveView::Settings;
        app.settings.selected_config_index = config_settings().len().saturating_sub(1);
        app.settings.config_scroll_offset = 0;

        terminal
            .draw(|frame| {
                super::render(frame, &mut app);
            })
            .expect("draw");

        assert!(app.settings.config_scroll_offset > 0);
    }

    #[test]
    fn normal_layout_renders_settings_limitation_hint() {
        fn buffer_text(buffer: &Buffer) -> String {
            let width = usize::from(buffer.area.width);
            buffer
                .content
                .chunks(width)
                .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n")
        }

        let backend = TestBackend::new(180, 30);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = App::test_default();
        app.active_view = crate::app::ActiveView::Settings;

        terminal
            .draw(|frame| {
                super::render(frame, &mut app);
            })
            .expect("draw");

        let rendered = buffer_text(terminal.backend().buffer());

        assert!(rendered.contains("supported by claude-rs"));
        assert!(rendered.contains("Anthropic Claude Agent SDK"));
    }

    #[test]
    fn compact_layout_renders_settings_limitation_hint() {
        fn buffer_text(buffer: &Buffer) -> String {
            let width = usize::from(buffer.area.width);
            buffer
                .content
                .chunks(width)
                .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n")
        }

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = App::test_default();
        app.active_view = crate::app::ActiveView::Settings;

        terminal
            .draw(|frame| {
                super::render(frame, &mut app);
            })
            .expect("draw");

        let rendered = buffer_text(terminal.backend().buffer());

        assert!(rendered.contains("supported by claude-rs"));
        assert!(rendered.contains("Anthropic Claude Agent SDK"));
    }

    #[test]
    fn settings_limitation_hint_wraps_on_narrow_widths() {
        assert_eq!(settings_hint_height(200), 1);
        assert!(settings_hint_height(40) > 1);
        assert!(settings_hint_height(20) > settings_hint_height(40));
        assert_eq!(settings_hint_height(0), 0);
        assert_eq!(SETTINGS_LIMITATION_HINT.is_empty(), false);
    }
}
