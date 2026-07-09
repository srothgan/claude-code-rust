// SPDX-License-Identifier: Apache-2.0
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::config::{OverlayMessage, OverlayMessageKind};
use crate::ui::theme;

#[derive(Debug, Clone, Copy)]
pub(super) struct OverlayLayoutSpec {
    pub min_width: u16,
    pub min_height: u16,
    pub width_percent: u16,
    pub height_percent: u16,
    pub preferred_height: u16,
    pub fullscreen_below: Option<(u16, u16)>,
    pub inner_margin: Margin,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct OverlayChrome<'a> {
    pub title: &'a str,
    pub subtitle: Option<&'a str>,
    pub help: Option<&'a str>,
    pub message: Option<&'a OverlayMessage>,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct RenderedOverlay {
    pub rect: Rect,
    pub body_area: Rect,
    pub message_area: Rect,
    pub help_area: Rect,
}

pub(super) fn render_overlay_shell(
    frame: &mut Frame,
    area: Rect,
    layout_spec: OverlayLayoutSpec,
    chrome: OverlayChrome<'_>,
) -> RenderedOverlay {
    let overlay_area = overlay_rect(area, layout_spec, chrome);
    frame.render_widget(Clear, overlay_area);
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .title(chrome.title)
            .border_style(Style::default().fg(theme::RUST_ORANGE)),
        overlay_area,
    );

    let inner = overlay_area.inner(layout_spec.inner_margin);
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(u16::from(chrome.subtitle.is_some())),
            Constraint::Min(1),
            Constraint::Length(u16::from(chrome.message.is_some())),
            Constraint::Length(u16::from(chrome.help.is_some())),
        ])
        .split(inner);

    if let Some(subtitle) = chrome.subtitle {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(subtitle, Style::default().fg(theme::DIM)))),
            sections[0],
        );
    }
    if let Some(help) = chrome.help {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(help, Style::default().fg(theme::RUST_ORANGE)))),
            sections[3],
        );
    }
    if let Some(message) = chrome.message {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                message.text.clone(),
                overlay_message_style(message.kind),
            ))),
            sections[2],
        );
    }

    RenderedOverlay {
        rect: overlay_area,
        body_area: sections[1],
        message_area: sections[2],
        help_area: sections[3],
    }
}

pub(super) fn overlay_line_style(selected: bool, focused: bool) -> Style {
    if selected && focused {
        Style::default().fg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD)
    } else if selected {
        Style::default().fg(ratatui::style::Color::White).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(ratatui::style::Color::White)
    }
}

pub(super) fn render_overlay_separator(frame: &mut Frame, area: Rect) {
    let width = usize::from(area.width.max(1));
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "-".repeat(width),
            Style::default().fg(theme::DIM),
        ))),
        area,
    );
}

pub(super) fn selected_scroll(
    selected_start: usize,
    selected_height: usize,
    viewport_height: u16,
) -> u16 {
    let viewport_height = usize::from(viewport_height);
    if viewport_height == 0 || selected_start + selected_height <= viewport_height {
        0
    } else {
        u16::try_from(selected_start + selected_height - viewport_height).unwrap_or(u16::MAX)
    }
}

fn overlay_message_style(kind: OverlayMessageKind) -> Style {
    match kind {
        OverlayMessageKind::Info => Style::default().fg(theme::DIM),
        OverlayMessageKind::Error => Style::default().fg(theme::STATUS_ERROR),
    }
}

fn overlay_rect(area: Rect, spec: OverlayLayoutSpec, chrome: OverlayChrome<'_>) -> Rect {
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

    let candidate = centered_rect_with_size(area, overlay_width, overlay_height);
    if overlay_needs_fullscreen(candidate, area, spec, chrome) { area } else { candidate }
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

fn overlay_needs_fullscreen(
    candidate: Rect,
    frame: Rect,
    spec: OverlayLayoutSpec,
    chrome: OverlayChrome<'_>,
) -> bool {
    if candidate == frame {
        return false;
    }

    let required_inner_height = 1
        + u16::from(chrome.subtitle.is_some())
        + u16::from(chrome.message.is_some())
        + u16::from(chrome.help.is_some());
    let required_height = required_inner_height
        .saturating_add(spec.inner_margin.vertical.saturating_mul(2))
        .saturating_add(2);
    candidate.height < required_height.min(frame.height)
}
