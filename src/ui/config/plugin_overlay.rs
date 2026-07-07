use crate::app::App;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use super::action_overlay::{self, ActionOverlayRow, ActionOverlayView};
use super::input::{add_marketplace_example_lines, render_text_input_field};
use super::overlay::{OverlayChrome, OverlayLayoutSpec, render_overlay_shell};

pub(super) fn render_installed_plugin_actions_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let Some(overlay) = app.config.installed_plugin_actions_overlay() else {
        return;
    };
    let actions = overlay
        .actions
        .iter()
        .copied()
        .map(|action| ActionOverlayRow::new(action.label()))
        .collect::<Vec<_>>();
    action_overlay::render(
        frame,
        area,
        &ActionOverlayView {
            title: "Installed plugin",
            heading: &overlay.title,
            description: &overlay.description,
            selected_index: overlay.selected_index,
            actions: &actions,
            message: app.config.overlay_message.as_ref(),
        },
    );
}

pub(super) fn render_plugin_install_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let Some(overlay) = app.config.plugin_install_overlay() else {
        return;
    };
    let actions = overlay
        .actions
        .iter()
        .copied()
        .map(|action| ActionOverlayRow::new(action.label()))
        .collect::<Vec<_>>();
    action_overlay::render(
        frame,
        area,
        &ActionOverlayView {
            title: "Install plugin",
            heading: &overlay.title,
            description: &overlay.description,
            selected_index: overlay.selected_index,
            actions: &actions,
            message: app.config.overlay_message.as_ref(),
        },
    );
}

pub(super) fn render_marketplace_actions_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let Some(overlay) = app.config.marketplace_actions_overlay() else {
        return;
    };
    let actions = overlay
        .actions
        .iter()
        .copied()
        .map(|action| ActionOverlayRow::new(action.label()))
        .collect::<Vec<_>>();
    action_overlay::render(
        frame,
        area,
        &ActionOverlayView {
            title: "Marketplace",
            heading: &overlay.title,
            description: &overlay.description,
            selected_index: overlay.selected_index,
            actions: &actions,
            message: app.config.overlay_message.as_ref(),
        },
    );
}

pub(super) fn render_add_marketplace_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let Some(overlay) = app.config.add_marketplace_overlay() else {
        return;
    };
    let rendered = render_overlay_shell(
        frame,
        area,
        OverlayLayoutSpec {
            min_width: 60,
            min_height: 13,
            width_percent: 72,
            height_percent: 66,
            preferred_height: 15,
            fullscreen_below: Some((60, 18)),
            inner_margin: Margin { vertical: 1, horizontal: 2 },
        },
        OverlayChrome {
            title: "Add Marketplace",
            subtitle: None,
            help: Some("Enter add | Esc cancel"),
            message: app.config.overlay_message.as_ref(),
        },
    );
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(5),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .split(rendered.body_area);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "Enter marketplace source:",
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        ))),
        sections[0],
    );
    frame.render_widget(
        Paragraph::new(add_marketplace_example_lines()).wrap(Wrap { trim: false }),
        sections[1],
    );
    render_text_input_field(
        frame,
        sections[3],
        &overlay.draft,
        overlay.cursor,
        "owner/repo or URL",
    );
}
