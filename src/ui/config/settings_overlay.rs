// SPDX-License-Identifier: Apache-2.0
use crate::agent::model::EffortLevel;
use crate::app::App;
use crate::app::config::{
    DEFAULT_MODEL_ALIAS_ID, OutputStyle, language_input_validation_message, model_overlay_options,
    supported_effort_levels_for_model,
};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};

use super::input::render_text_input_field;
use super::overlay::{
    OverlayChrome, OverlayLayoutSpec, overlay_line_style, render_overlay_shell, selected_scroll,
};
use super::{theme, wrapped_text_height};

pub(super) fn render_model_overlay(frame: &mut Frame, area: Rect, app: &App) {
    if app.config.model_overlay().is_none() {
        return;
    }
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
            title: "Model",
            subtitle: None,
            help: Some("Up/Down select | Enter confirm | Esc cancel"),
            message: app.config.overlay_message.as_ref(),
        },
    );
    let model_lines = model_overlay_lines(app);
    let model_scroll =
        model_overlay_scroll(app, rendered.body_area.height, rendered.body_area.width);
    frame.render_widget(
        Paragraph::new(model_lines).scroll((model_scroll, 0)).wrap(Wrap { trim: false }),
        rendered.body_area,
    );
}

pub(super) fn render_thinking_effort_overlay(frame: &mut Frame, area: Rect, app: &App) {
    if app.config.thinking_effort_overlay().is_none() {
        return;
    }
    let rendered = render_overlay_shell(
        frame,
        area,
        OverlayLayoutSpec {
            min_width: 1,
            min_height: 1,
            width_percent: 76,
            height_percent: 70,
            preferred_height: 16,
            fullscreen_below: Some((72, 16)),
            inner_margin: Margin { vertical: 1, horizontal: 2 },
        },
        OverlayChrome {
            title: "Thinking effort",
            subtitle: Some("Available effort levels depend on the selected model."),
            help: Some("Up/Down select | Enter confirm | Esc cancel"),
            message: app.config.overlay_message.as_ref(),
        },
    );
    let effort_lines = effort_overlay_lines(app);
    let effort_scroll =
        effort_overlay_scroll(app, rendered.body_area.height, rendered.body_area.width);
    frame.render_widget(
        Paragraph::new(effort_lines).scroll((effort_scroll, 0)).wrap(Wrap { trim: false }),
        rendered.body_area,
    );
}

pub(super) fn render_output_style_overlay(frame: &mut Frame, area: Rect, app: &App) {
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
            message: app.config.overlay_message.as_ref(),
        },
    );
    let scroll =
        output_style_overlay_scroll(app, rendered.body_area.height, rendered.body_area.width);
    frame.render_widget(
        Paragraph::new(output_style_overlay_lines(app))
            .scroll((scroll, 0))
            .wrap(Wrap { trim: false }),
        rendered.body_area,
    );
}

pub(super) fn render_language_overlay(frame: &mut Frame, area: Rect, app: &App) {
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
            message: app.config.overlay_message.as_ref(),
        },
    );
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(rendered.body_area);

    render_text_input_field(
        frame,
        sections[0],
        &overlay.draft,
        overlay.cursor,
        "e.g. en, Greek, Japanese, Pirate",
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

pub(super) fn render_session_rename_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let Some(overlay) = app.config.session_rename_overlay() else {
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
            title: "Rename session",
            subtitle: Some("Set a custom title for the current session"),
            help: Some("Enter confirm | Esc cancel"),
            message: app.config.overlay_message.as_ref(),
        },
    );
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(rendered.body_area);

    render_text_input_field(
        frame,
        sections[0],
        &overlay.draft,
        overlay.cursor,
        "Custom session name",
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "Leave the field empty to clear the custom session name.",
            Style::default().fg(theme::DIM),
        ))),
        sections[1],
    );
}

pub(super) fn model_overlay_lines(app: &App) -> Vec<Line<'static>> {
    let Some(overlay) = app.config.model_overlay() else {
        return Vec::new();
    };
    let mut lines = model_overlay_options(app)
        .into_iter()
        .flat_map(|option| {
            let selected = option.id == overlay.selected_model;
            let marker = if selected { ">" } else { " " };
            let mut lines = vec![model_overlay_title_line(&option, marker, selected, true)];
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
    let Some(overlay) = app.config.thinking_effort_overlay() else {
        return Vec::new();
    };
    let selected_model = selected_model_for_effort(app);
    let levels = supported_effort_levels_for_model(app, &selected_model);
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
                    overlay_line_style(selected, true),
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

pub(super) fn output_style_overlay_lines(app: &App) -> Vec<Line<'static>> {
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

fn output_style_overlay_scroll(app: &App, viewport_height: u16, viewport_width: u16) -> u16 {
    let Some(overlay) = app.config.output_style_overlay() else {
        return 0;
    };
    if viewport_height == 0 || viewport_width == 0 {
        return 0;
    }
    let selected_index =
        OutputStyle::ALL.iter().position(|style| *style == overlay.selected).unwrap_or(0);
    let selected_start = OutputStyle::ALL
        .iter()
        .take(selected_index)
        .enumerate()
        .map(|(index, style)| {
            output_style_option_height(*style, index + 1 == OutputStyle::ALL.len(), viewport_width)
        })
        .sum::<usize>();
    let selected_height = output_style_option_height(
        OutputStyle::ALL[selected_index],
        selected_index + 1 == OutputStyle::ALL.len(),
        viewport_width,
    );
    selected_scroll(selected_start, selected_height, viewport_height)
}

fn output_style_option_height(style: OutputStyle, is_last: bool, viewport_width: u16) -> usize {
    let lines = vec![
        Line::from(format!("  {}", style.label())),
        Line::from(Span::styled(
            format!("   {}", style.description()),
            Style::default().fg(theme::DIM),
        )),
    ];
    wrapped_text_height(Text::from(lines), viewport_width) + usize::from(!is_last)
}

pub(super) fn model_overlay_scroll(app: &App, viewport_height: u16, viewport_width: u16) -> u16 {
    let Some(overlay) = app.config.model_overlay() else {
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
    selected_scroll(selected_start, selected_height, viewport_height)
}

pub(super) fn effort_overlay_scroll(app: &App, viewport_height: u16, viewport_width: u16) -> u16 {
    let Some(overlay) = app.config.thinking_effort_overlay() else {
        return 0;
    };
    let selected_model = selected_model_for_effort(app);
    let levels = supported_effort_levels_for_model(app, &selected_model);
    if levels.is_empty() || viewport_height == 0 || viewport_width == 0 {
        return 0;
    }

    let selected_index =
        levels.iter().position(|level| *level == overlay.selected_effort).unwrap_or(0);
    let selected_start = levels
        .iter()
        .take(selected_index)
        .enumerate()
        .map(|(index, level)| {
            effort_overlay_option_height(*level, index + 1 == levels.len(), viewport_width)
        })
        .sum::<usize>();
    let selected_height = effort_overlay_option_height(
        levels[selected_index],
        selected_index + 1 == levels.len(),
        viewport_width,
    );
    selected_scroll(selected_start, selected_height, viewport_height)
}

fn selected_model_for_effort(app: &App) -> String {
    app.config.model_effective().unwrap_or_else(|| DEFAULT_MODEL_ALIAS_ID.to_owned())
}

fn effort_overlay_option_height(level: EffortLevel, is_last: bool, viewport_width: u16) -> usize {
    let lines = vec![
        Line::from(format!("  {}", level.label())),
        Line::from(Span::styled(
            format!("  {}", level.description()),
            Style::default().fg(theme::DIM),
        )),
    ];
    wrapped_text_height(Text::from(lines), viewport_width) + usize::from(!is_last)
}

fn model_overlay_option_height(
    option: &crate::app::config::OverlayModelOption,
    is_last: bool,
    viewport_width: u16,
) -> usize {
    let title = model_overlay_title_line(option, " ", false, false);
    let mut height = wrapped_text_height(Text::from(vec![title]), viewport_width);
    if let Some(description) = option.description.as_deref() {
        height += wrapped_text_height(
            Text::from(vec![Line::from(Span::styled(
                format!("  {description}"),
                Style::default().fg(theme::DIM),
            ))]),
            viewport_width,
        );
    }
    height + usize::from(!is_last)
}

struct CapabilityBadge {
    label: &'static str,
    bg: Color,
    fg: Color,
}

pub(super) fn model_overlay_title_line(
    option: &crate::app::config::OverlayModelOption,
    marker: &str,
    selected: bool,
    focused: bool,
) -> Line<'static> {
    Line::from(model_overlay_title_spans(option, marker, selected, focused))
}

fn model_overlay_title_spans(
    option: &crate::app::config::OverlayModelOption,
    marker: &str,
    selected: bool,
    focused: bool,
) -> Vec<Span<'static>> {
    let mut spans = vec![Span::styled(
        format!("{marker} {}", option.display_name),
        overlay_line_style(selected, focused),
    )];
    let badges = model_capability_badges(option);
    if badges.is_empty() {
        return spans;
    }
    spans.push(Span::styled("  ", Style::default().fg(theme::DIM)));
    for (index, badge) in badges.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled("  ", Style::default().fg(theme::DIM)));
        }
        spans.push(Span::styled(
            format!(" {} ", badge.label),
            Style::default().fg(badge.fg).bg(badge.bg).add_modifier(Modifier::BOLD),
        ));
    }
    spans
}

fn model_capability_badges(
    option: &crate::app::config::OverlayModelOption,
) -> Vec<CapabilityBadge> {
    let mut badges = Vec::new();
    if option.supports_effort {
        badges.push(CapabilityBadge {
            label: "Effort",
            bg: Color::Rgb(64, 64, 64),
            fg: Color::White,
        });
    }
    if option.supports_adaptive_thinking == Some(true) {
        badges.push(CapabilityBadge {
            label: "Adaptive thinking",
            bg: Color::Rgb(34, 92, 124),
            fg: Color::White,
        });
    }
    if option.supports_fast_mode == Some(true) {
        badges.push(CapabilityBadge {
            label: "Fast mode",
            bg: Color::Rgb(24, 120, 82),
            fg: Color::White,
        });
    }
    if option.supports_auto_mode == Some(true) {
        badges.push(CapabilityBadge {
            label: "Auto mode",
            bg: Color::Rgb(152, 106, 0),
            fg: Color::Black,
        });
    }
    badges
}
