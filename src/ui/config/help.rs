// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use crate::app::{App, ConfigHelpSection};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;

use super::super::theme;
use super::super::two_column_list::{self, TwoColumnItem};
use super::super::wrap::display_width;

const COLUMN_GAP: usize = 4;
const NAME_MIN_WIDTH: usize = 12;
const NAME_MAX_WIDTH: usize = 28;
const NAME_MAX_SHARE_NUM: usize = 2;
const NAME_MAX_SHARE_DEN: usize = 5;

pub(super) fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.width == 0 || area.height == 0 {
        app.config.help_visible_count = 0;
        return;
    }

    let [tabs_area, _separator_area, body_area] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1), Constraint::Min(1)])
        .areas(area);

    render_section_header(frame, tabs_area, app.config.help_section);
    render_section_body(frame, body_area, app);
}

fn render_section_header(frame: &mut Frame, area: Rect, active: ConfigHelpSection) {
    let mut spans = Vec::new();
    for (index, section) in ConfigHelpSection::ALL.iter().copied().enumerate() {
        if index > 0 {
            spans.push(Span::styled(" | ", Style::default().fg(theme::DIM)));
        }

        let style = if section == active {
            Style::default().fg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(ratatui::style::Color::White)
        };
        spans.push(Span::styled(section.title().to_owned(), style));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_section_body(frame: &mut Frame, area: Rect, app: &mut App) {
    let items = crate::ui::help::help_items(app, app.config.help_section);
    if items.is_empty() {
        app.config.help_visible_count = 0;
        return;
    }

    let inner_width = usize::from(area.width);
    let (name_width, desc_width) = item_column_widths(&items, inner_width);
    let visible_count = visible_item_count(app, &items, area.height, name_width, desc_width);
    app.config.help_visible_count = visible_count;
    app.config.help_dialog.clamp(items.len(), visible_count);

    let start = app.config.help_dialog.scroll_offset;
    let end = (start + visible_count).min(items.len());
    let selected = app.config.help_dialog.selected;
    let visible_items = &items[start..end];
    let list_items = visible_items
        .iter()
        .enumerate()
        .map(|(offset, (left, right))| {
            let row_index = start + offset;
            let is_selected = row_index == selected;
            let left_style = if is_selected {
                Style::default().fg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD)
            } else {
                Style::default().add_modifier(Modifier::BOLD)
            };
            let right_style = if is_selected {
                Style::default().fg(theme::RUST_ORANGE)
            } else {
                Style::default()
            };
            TwoColumnItem { left: left.clone(), right: right.clone(), left_style, right_style }
        })
        .collect::<Vec<_>>();

    let lines = two_column_list::render_lines(&list_items, name_width, desc_width, COLUMN_GAP, 1);
    frame.render_widget(Paragraph::new(Text::from(lines)), area);
}

fn visible_item_count(
    app: &App,
    items: &[(String, String)],
    area_height: u16,
    name_width: usize,
    desc_width: usize,
) -> usize {
    if area_height == 0 {
        return 0;
    }
    let count_items = items
        .iter()
        .map(|(left, right)| TwoColumnItem {
            left: left.clone(),
            right: right.clone(),
            left_style: Style::default().add_modifier(Modifier::BOLD),
            right_style: Style::default(),
        })
        .collect::<Vec<_>>();
    two_column_list::visible_item_count(
        &count_items,
        app.config.help_dialog.scroll_offset,
        usize::from(area_height),
        name_width,
        desc_width,
        1,
    )
}

fn item_column_widths(items: &[(String, String)], inner_width: usize) -> (usize, usize) {
    if inner_width == 0 {
        return (0, 0);
    }
    if inner_width <= COLUMN_GAP + 1 {
        return (inner_width, 1);
    }

    let max_name_width =
        items.iter().map(|(name, _)| display_width(name.as_str())).max().unwrap_or(0);
    let share_cap = inner_width.saturating_mul(NAME_MAX_SHARE_NUM) / NAME_MAX_SHARE_DEN;
    let min_name_width = NAME_MIN_WIDTH.min(share_cap.max(1));
    let preferred_name_width =
        max_name_width.max(min_name_width).min(NAME_MAX_WIDTH).min(share_cap.max(1));
    let max_name_fit = inner_width.saturating_sub(COLUMN_GAP + 1);
    let name_width = preferred_name_width.clamp(1, max_name_fit.max(1));
    let desc_width = inner_width.saturating_sub(name_width + COLUMN_GAP).max(1);

    (name_width, desc_width)
}
