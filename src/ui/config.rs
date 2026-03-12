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

mod mcp;
mod overlay;
mod settings;
mod status;
mod usage;

use crate::app::config::{
    LanguageOverlayState, OutputStyle, OverlayFocus, language_input_validation_message,
    model_overlay_options, supported_effort_levels_for_model,
};
use crate::app::{App, ConfigTab};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::Color;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use super::theme;
use overlay::{
    OverlayChrome, OverlayLayoutSpec, overlay_line_style, render_overlay_header,
    render_overlay_separator as shared_render_overlay_separator, render_overlay_shell,
};

const SETTINGS_LIMITATION_HINT: &str = "Currently, not all settings are supported by claude-rs. This project uses the official Anthropic Claude Agent SDK, which limits claude-rs implementing all Claude Code settings.";
const MIN_SETTINGS_PANEL_HEIGHT: u16 = 3;

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

    render_tab_header(frame, chunks[0], app.config.active_tab);

    match app.config.active_tab {
        ConfigTab::Settings => settings::render(frame, chunks[1], app),
        ConfigTab::Status => status::render(frame, chunks[1]),
        ConfigTab::Usage => usage::render(frame, chunks[1]),
        ConfigTab::Mcp => mcp::render(frame, chunks[1]),
    }

    if app.config.model_and_effort_overlay().is_some() {
        render_model_and_effort_overlay(frame, frame_area, app);
    } else if app.config.output_style_overlay().is_some() {
        render_output_style_overlay(frame, frame_area, app);
    } else if app.config.language_overlay().is_some() {
        render_language_overlay(frame, frame_area, app);
    }

    let message = app.config.last_error.clone().unwrap_or_default();
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            message,
            Style::default().fg(if app.config.last_error.is_some() {
                theme::STATUS_ERROR
            } else {
                theme::DIM
            }),
        ))),
        chunks[2],
    );

    let help = if app.config.model_and_effort_overlay().is_some()
        || app.config.output_style_overlay().is_some()
        || app.config.language_overlay().is_some()
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

fn render_model_and_effort_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let Some(overlay) = app.config.model_and_effort_overlay() else {
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
    let Some(overlay) = app.config.model_and_effort_overlay() else {
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
    let Some(overlay) = app.config.model_and_effort_overlay() else {
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

fn render_language_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let Some(overlay) = app.config.language_overlay() else {
        return;
    };
    let rendered = render_overlay_shell(
        frame,
        area,
        OverlayLayoutSpec {
            min_width: 56,
            min_height: 8,
            width_percent: 72,
            height_percent: 48,
            preferred_height: 10,
            fullscreen_below: Some((56, 14)),
            inner_margin: Margin { vertical: 1, horizontal: 2 },
        },
        OverlayChrome {
            title: "Language",
            subtitle: Some("Free-text prompt language for Claude sessions"),
            help: Some("Enter confirm | Esc cancel"),
        },
    );
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1)])
        .split(rendered.body_area);

    frame.render_widget(
        Paragraph::new(language_overlay_input_line(overlay)).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme::RUST_ORANGE)),
        ),
        sections[0],
    );

    let validation = language_input_validation_message(&overlay.draft);
    let (message, style) = match validation {
        Some(message) => (message, Style::default().fg(theme::STATUS_ERROR)),
        None => (
            "Examples: en, Greek, Japanese, Klingon, Pirate. Stored as prompt guidance, not UI language.",
            Style::default().fg(theme::DIM),
        ),
    };
    frame.render_widget(Paragraph::new(Line::from(Span::styled(message, style))), sections[1]);
}

fn language_overlay_input_line(overlay: &LanguageOverlayState) -> Line<'static> {
    const PLACEHOLDER: &str = "e.g. en, Greek, Japanese, Pirate";
    let cursor_style =
        Style::default().fg(Color::Black).bg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD);
    let text_style = Style::default().fg(Color::White);
    let placeholder_style = Style::default().fg(theme::DIM);

    if overlay.draft.is_empty() {
        return Line::from(vec![
            Span::styled(" ".to_owned(), cursor_style),
            Span::styled(PLACEHOLDER.to_owned(), placeholder_style),
        ]);
    }

    let cursor = overlay.cursor.min(overlay.draft.chars().count());
    let chars = overlay.draft.chars().collect::<Vec<_>>();
    let prefix = chars[..cursor].iter().collect::<String>();
    let mut spans = Vec::new();

    if !prefix.is_empty() {
        spans.push(Span::styled(prefix, text_style));
    }

    if cursor < chars.len() {
        spans.push(Span::styled(chars[cursor].to_string(), cursor_style));
        let suffix = chars[cursor + 1..].iter().collect::<String>();
        if !suffix.is_empty() {
            spans.push(Span::styled(suffix, text_style));
        }
    } else {
        spans.push(Span::styled(" ".to_owned(), cursor_style));
    }

    Line::from(spans)
}

fn output_style_overlay_lines(app: &App) -> Vec<Line<'static>> {
    let Some(overlay) = app.config.output_style_overlay() else {
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
    let Some(overlay) = app.config.model_and_effort_overlay() else {
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
        u16::try_from(selected_start + selected_height - viewport_height).unwrap_or(u16::MAX)
    }
}

fn model_overlay_option_height(
    option: &crate::app::config::OverlayModelOption,
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

fn render_tab_header(frame: &mut Frame, area: Rect, active_tab: ConfigTab) {
    let mut spans = Vec::new();
    for (index, tab) in ConfigTab::ALL.iter().copied().enumerate() {
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
    use super::{SETTINGS_LIMITATION_HINT, model_overlay_scroll};
    use crate::agent::model::{AvailableModel, EffortLevel};
    use crate::app::App;
    use crate::app::config::{
        LanguageOverlayState, ModelAndEffortOverlayState, OutputStyle, OutputStyleOverlayState,
        OverlayFocus, SettingId, SettingsOverlayState, setting_specs,
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
        app.config.overlay =
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
        app.config.overlay =
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
        app.config.overlay = Some(SettingsOverlayState::OutputStyle(OutputStyleOverlayState {
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
    fn language_overlay_input_uses_placeholder_when_empty() {
        let line = super::language_overlay_input_line(&LanguageOverlayState {
            draft: String::new(),
            cursor: 0,
        })
        .to_string();

        assert!(line.contains("e.g. en, Greek, Japanese, Pirate"));
    }

    #[test]
    fn language_overlay_renders_inline_validation_message() {
        fn buffer_text(buffer: &Buffer) -> String {
            let width = usize::from(buffer.area.width);
            buffer
                .content
                .chunks(width)
                .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n")
        }

        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = App::test_default();
        app.active_view = crate::app::ActiveView::Config;
        app.config.overlay = Some(SettingsOverlayState::Language(LanguageOverlayState {
            draft: "E".to_owned(),
            cursor: 1,
        }));

        terminal
            .draw(|frame| {
                super::render(frame, &mut app);
            })
            .expect("draw");

        let rendered = buffer_text(terminal.backend().buffer());
        assert!(rendered.contains("Language must be at least 2 characters."));
    }

    #[test]
    fn output_style_details_show_unsupported_warning() {
        let mut app = App::test_default();
        app.config.selected_setting_index = setting_specs()
            .iter()
            .position(|spec| spec.id == SettingId::OutputStyle)
            .expect("output style row");

        let rendered = super::settings::setting_detail_lines(&app)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();

        assert!(rendered.iter().any(|line| line.contains("not supported yet")));
    }

    #[test]
    fn compact_settings_layout_triggers_for_small_tuis() {
        assert!(super::settings::compact_settings_layout(Rect::new(0, 0, 89, 25)));
        assert!(super::settings::compact_settings_layout(Rect::new(0, 0, 100, 19)));
        assert!(!super::settings::compact_settings_layout(Rect::new(0, 0, 90, 20)));
    }

    #[test]
    fn compact_settings_list_inlines_unsupported_warning() {
        fn buffer_text(buffer: &Buffer) -> String {
            let width = usize::from(buffer.area.width);
            buffer
                .content
                .chunks(width)
                .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n")
        }

        let backend = TestBackend::new(80, 16);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = App::test_default();
        app.active_view = crate::app::ActiveView::Config;
        app.config.selected_setting_index = setting_specs()
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
                .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect::<String>())
                .collect()
        }

        let backend = TestBackend::new(42, 20);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = App::test_default();
        app.active_view = crate::app::ActiveView::Config;
        app.config.selected_setting_index = setting_specs()
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
    fn render_updates_settings_scroll_offset_to_keep_selection_visible() {
        let backend = TestBackend::new(80, 16);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = App::test_default();
        app.active_view = crate::app::ActiveView::Config;
        app.config.selected_setting_index = setting_specs().len().saturating_sub(1);
        app.config.settings_scroll_offset = 0;

        terminal
            .draw(|frame| {
                super::render(frame, &mut app);
            })
            .expect("draw");

        assert!(app.config.settings_scroll_offset > 0);
    }

    #[test]
    fn normal_layout_renders_settings_limitation_hint() {
        fn buffer_text(buffer: &Buffer) -> String {
            let width = usize::from(buffer.area.width);
            buffer
                .content
                .chunks(width)
                .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n")
        }

        let backend = TestBackend::new(180, 30);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = App::test_default();
        app.active_view = crate::app::ActiveView::Config;

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
                .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n")
        }

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = App::test_default();
        app.active_view = crate::app::ActiveView::Config;

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
        assert_eq!(super::settings::settings_hint_height(200), 1);
        assert!(super::settings::settings_hint_height(40) > 1);
        assert!(
            super::settings::settings_hint_height(20) > super::settings::settings_hint_height(40)
        );
        assert_eq!(super::settings::settings_hint_height(0), 0);
        assert!(!SETTINGS_LIMITATION_HINT.is_empty());
    }
}
