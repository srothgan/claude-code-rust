use std::borrow::Cow;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use crate::app::config::OverlayMessage;
use crate::ui::theme;

use super::overlay::{
    OverlayChrome, OverlayLayoutSpec, overlay_line_style, render_overlay_separator,
    render_overlay_shell, selected_scroll,
};

pub(super) struct ActionOverlayRow<'a> {
    label: Cow<'a, str>,
}

impl<'a> ActionOverlayRow<'a> {
    pub(super) fn new(label: impl Into<Cow<'a, str>>) -> Self {
        Self { label: label.into() }
    }
}

pub(super) struct ActionOverlayView<'a> {
    pub(super) title: &'a str,
    pub(super) heading: &'a str,
    pub(super) description: &'a str,
    pub(super) selected_index: usize,
    pub(super) actions: &'a [ActionOverlayRow<'a>],
    pub(super) message: Option<&'a OverlayMessage>,
}

pub(super) fn render(frame: &mut Frame, area: Rect, view: &ActionOverlayView<'_>) {
    let rendered = render_overlay_shell(
        frame,
        area,
        OverlayLayoutSpec {
            min_width: 56,
            min_height: 10,
            width_percent: 70,
            height_percent: 62,
            preferred_height: 14,
            fullscreen_below: Some((56, 16)),
            inner_margin: Margin { vertical: 1, horizontal: 2 },
        },
        OverlayChrome {
            title: view.title,
            subtitle: None,
            help: Some("Up/Down select | Enter run | Esc cancel"),
            message: view.message,
        },
    );
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(rendered.body_area);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            view.heading.to_owned(),
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        ))),
        sections[0],
    );
    super::render_clipped_plain_text(
        frame,
        sections[1],
        view.description,
        Style::default().fg(theme::DIM),
    );
    render_overlay_separator(frame, sections[2]);
    let action_scroll = action_overlay_scroll(view.selected_index, sections[3].height);
    frame.render_widget(
        Paragraph::new(action_overlay_lines(view.actions, view.selected_index))
            .scroll((action_scroll, 0))
            .wrap(Wrap { trim: false }),
        sections[3],
    );
}

fn action_overlay_lines(
    actions: &[ActionOverlayRow<'_>],
    selected_index: usize,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for (index, action) in actions.iter().enumerate() {
        let selected = index == selected_index;
        lines.push(Line::from(Span::styled(
            format!("{} {}", if selected { ">" } else { " " }, action.label),
            overlay_line_style(selected, true),
        )));
        if index + 1 < actions.len() {
            lines.push(Line::default());
        }
    }
    lines
}

fn action_overlay_scroll(selected_index: usize, viewport_height: u16) -> u16 {
    selected_scroll(selected_index.saturating_mul(2), 1, viewport_height)
}
