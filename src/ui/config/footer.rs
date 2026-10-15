// SPDX-License-Identifier: Apache-2.0

use super::common;
use super::overlay::{OverlayChrome, OverlayLayoutSpec, render_overlay_shell};
use crate::app::{
    App,
    config::{ConfigOverlayState, FooterOverlayState},
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Margin, Rect},
    style::Style,
    text::Line,
    widgets::{Cell, Paragraph, Row, Table, TableState},
};
use unicode_width::UnicodeWidthChar;

pub(super) fn render(frame: &mut Frame, area: Rect, app: &App) {
    let Some(ConfigOverlayState::Footer(overlay)) = &app.config.overlay else {
        return;
    };
    let layout = overlay.layout();
    let compact = area.width < 56 || area.height < 18;
    let help = help_text(app, overlay, compact);
    // Keep the selected row and controls visible even after a save error.
    let compact_message = app.config.overlay_message.as_ref().filter(|_| compact).map(|message| {
        let mut used = 0;
        let mut text: String = message
            .text
            .lines()
            .next()
            .unwrap_or_default()
            .chars()
            .take_while(|ch| {
                used += ch.width().unwrap_or(0);
                used <= usize::from(area.width.saturating_sub(7))
            })
            .collect();
        if text.len() < message.text.len() {
            text.push_str("...");
        }
        crate::app::config::OverlayMessage { kind: message.kind, text }
    });
    let shell = render_overlay_shell(
        frame,
        area,
        OverlayLayoutSpec {
            min_width: 56,
            min_height: 16,
            width_percent: 85,
            height_percent: 85,
            preferred_height: 20,
            fullscreen_below: Some((56, 16)),
            inner_margin: Margin { vertical: 1, horizontal: 2 },
        },
        OverlayChrome {
            title: "Footer",
            subtitle: (!compact).then_some(if app.config.pending_settings_request.is_some() {
                "User preference · Saving..."
            } else {
                "User preference · Changes save automatically"
            }),
            help: Some(&help),
            message: compact_message.as_ref().or(app.config.overlay_message.as_ref()),
        },
    );
    let counter = common::position_counter(overlay.selected, layout.fields().len());
    let counter_width = u16::try_from(counter.width()).unwrap_or_default();
    frame.render_widget(
        Paragraph::new(Line::from(counter)),
        Rect::new(
            shell.rect.right().saturating_sub(counter_width + 2),
            shell.rect.y,
            counter_width,
            1,
        ),
    );
    let sections =
        Layout::vertical([Constraint::Min(2), Constraint::Length(1), Constraint::Length(1)])
            .split(shell.body_area);
    let rows = layout.fields().iter().map(|field| {
        Row::new([
            Cell::from(if field.visible { "[x]" } else { "[ ]" }),
            Cell::from(field.id.label()),
            Cell::from(field.side.label()),
        ])
    });
    let table =
        Table::new(rows, [Constraint::Length(3), Constraint::Min(8), Constraint::Length(5)])
            .column_spacing(1)
            .header(Row::new(["", "Field", "Side"]).style(common::title_style()))
            .row_highlight_style(common::selection_style(true))
            .highlight_symbol("› ");
    let mut state = TableState::default().with_selected(Some(overlay.selected));
    frame.render_stateful_widget(table, sections[0], &mut state);
    frame.render_widget(
        Paragraph::new("Preview").style(Style::default().fg(super::theme::DIM)),
        sections[1],
    );
    frame.render_widget(
        Paragraph::new(crate::ui::footer_rows::serialize_footer(app, &layout, sections[2].width)),
        sections[2],
    );
}

fn help_text(app: &App, overlay: &FooterOverlayState, compact: bool) -> String {
    let read_only = !overlay.editor.setting.writable_at(overlay.editor.scope);
    let mut help = if read_only {
        "Read-only | Up/Down select"
    } else if compact {
        "↑/↓ select | Space toggle | ←/→ side | Ctrl+↑/↓ move | Ctrl+R reset"
    } else {
        "Up/Down select | Space on/off | Left/Right side | Ctrl+Up/Down move | Ctrl+R reset"
    }
    .to_owned();
    if !read_only
        && overlay.can_retry()
        && app
            .config
            .overlay_message
            .as_ref()
            .is_some_and(|message| message.kind == crate::app::config::OverlayMessageKind::Error)
    {
        help.push_str(if compact {
            " | Enter retry | Esc discard"
        } else {
            " | Enter retry | Esc discard unsaved"
        });
    } else if read_only {
        help.push_str(" | Esc back");
    } else {
        help.push_str(" | Esc close");
    }
    help
}
