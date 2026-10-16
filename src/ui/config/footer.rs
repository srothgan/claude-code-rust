// SPDX-License-Identifier: Apache-2.0

use super::common;
use super::overlay::{OverlayChrome, OverlayLayoutSpec, render_overlay_shell};
use crate::app::{
    App,
    config::{ConfigOverlayState, FooterOverlayState},
    footer::{FooterColor, FooterField},
};
use ratatui::{
    Frame,
    layout::{Constraint, Margin, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Cell, Paragraph, Row, Table},
};
use unicode_width::UnicodeWidthChar;

pub(super) fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    let columns = usize::from(area.width.saturating_sub(10) / 4).clamp(1, 10);
    if let Some(ConfigOverlayState::Footer(overlay)) = &mut app.config.overlay {
        overlay.color_columns = columns;
    }
    let Some(ConfigOverlayState::Footer(overlay)) = &app.config.overlay else {
        return;
    };
    let layout = overlay.layout();
    let compact = area.width < 56 || area.height < 18;
    let help = help_text(app, overlay, compact);
    let compact_message = compact_message(app, compact, area.width);
    let shell = render_overlay_shell(
        frame,
        area,
        OverlayLayoutSpec {
            min_width: 56,
            min_height: 16,
            width_percent: 85,
            height_percent: 85,
            preferred_height: 20,
            fullscreen_below: Some((56, 18)),
            inner_margin: if compact {
                Margin { vertical: 1, horizontal: 1 }
            } else {
                Margin { vertical: 1, horizontal: 2 }
            },
        },
        OverlayChrome {
            title: "Footer",
            subtitle: (!compact).then_some(if app.config.footer.pending.is_some() {
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
        Paragraph::new(counter),
        Rect::new(
            shell.rect.right().saturating_sub(counter_width + 2),
            shell.rect.y,
            counter_width,
            1,
        ),
    );

    let mut body = shell.body_area;
    if body.height == 0 {
        return;
    }
    if body.height >= 4 {
        render_row(frame, Rect::new(body.x, body.y, body.width, 1), None, false);
        body.y += 1;
        body.height -= 1;
    }
    let palette = if overlay.color_selection.is_some() {
        palette_lines(overlay, body.height.saturating_sub(1))
    } else {
        vec![]
    };
    // Scroll the expanded field as a unit, retaining its label and focused swatch.
    let selected_end = overlay.selected + 1 + palette.len();
    let offset = selected_end.saturating_sub(usize::from(body.height));
    let mut y = body.y;
    for (index, field) in layout.fields().iter().enumerate().skip(offset) {
        if y >= body.bottom() {
            break;
        }
        render_row(
            frame,
            Rect::new(body.x, y, body.width, 1),
            Some(field),
            index == overlay.selected,
        );
        y += 1;
        if index == overlay.selected {
            for line in &palette {
                if y >= body.bottom() {
                    break;
                }
                frame.render_widget(
                    Paragraph::new(line.clone()),
                    Rect::new(body.x, y, body.width, 1),
                );
                y += 1;
            }
        }
    }
}

fn render_row(frame: &mut Frame, area: Rect, field: Option<&FooterField>, selected: bool) {
    let wide = area.width >= 48;
    let tiny = area.width < 24;
    let color = field.map_or_else(
        || Line::from(if wide { "Color" } else { "" }),
        |field| {
            if !field.id.supports_color() {
                Line::from(Span::styled("Fixed", common::title_style()))
            } else if let Some(color) = field.color {
                let mut spans = vec![swatch(color)];
                if wide {
                    spans.push(Span::raw(format!(" {}", color.label())));
                }
                Line::from(spans)
            } else {
                Line::from(if wide { "Current" } else { "·" })
            }
        },
    );
    let row = Row::new([
        Cell::from(if selected { "›" } else { "" }),
        Cell::from(field.map_or("", |field| if field.visible { "[x]" } else { "[ ]" })),
        Cell::from(field.map_or("Field", |field| field.id.label())),
        Cell::from(field.map_or("Side", |field| {
            if tiny {
                match field.side {
                    crate::app::footer::FooterSide::Left => "L",
                    crate::app::footer::FooterSide::Right => "R",
                }
            } else {
                field.side.label()
            }
        })),
        Cell::from(color),
    ])
    .style(if field.is_none() {
        common::title_style()
    } else {
        common::selection_style(selected)
    });
    let table = Table::new(
        [row],
        [
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Min(4),
            Constraint::Length(if tiny { 1 } else { 5 }),
            Constraint::Length(if tiny {
                0
            } else if wide {
                11
            } else {
                5
            }),
        ],
    )
    .column_spacing(1);
    frame.render_widget(table, area);
}

fn swatch(color: FooterColor) -> Span<'static> {
    Span::styled("  ", Style::default().bg(color.color()))
}

fn palette_lines(overlay: &FooterOverlayState, height: u16) -> Vec<Line<'static>> {
    let Some(selected) = overlay.color_selection else {
        return vec![];
    };
    let columns = overlay.color_columns;
    let mut rows = vec![
        Line::from(vec![
            Span::raw(if selected == 0 { "  › " } else { "    " }),
            Span::raw("Current — no override"),
        ])
        .style(if selected == 0 {
            common::selection_style(true)
        } else {
            Style::default()
        }),
    ];
    for (row, colors) in FooterColor::ALL.chunks(columns).enumerate() {
        if row > 0 {
            rows.push(Line::default());
        }
        let mut spans = vec![Span::raw("    ")];
        for (column, &color) in colors.iter().enumerate() {
            let focused = selected == row * columns + column + 1;
            spans.push(Span::styled(
                if focused { "[" } else { " " },
                Style::default().fg(Color::White),
            ));
            spans.push(swatch(color));
            spans.push(Span::styled(
                if focused { "]" } else { " " },
                Style::default().fg(Color::White),
            ));
        }
        rows.push(Line::from(spans));
    }
    rows.push(Line::from(format!(
        "    {}",
        overlay.selected_color().map_or("Current", |color| color.label())
    )));
    if rows.len() > usize::from(height) {
        let focused = if selected == 0 { 0 } else { 2 * ((selected - 1) / columns) + 1 };
        let label = rows.pop().unwrap_or_default();
        let focused_row = rows.remove(focused.min(rows.len().saturating_sub(1)));
        rows = vec![focused_row];
        if height >= 2 {
            rows.push(label);
        }
    }
    rows.truncate(usize::from(height));
    rows
}

fn help_text(app: &App, overlay: &FooterOverlayState, compact: bool) -> String {
    if overlay.color_selection.is_some() {
        return if compact {
            "Arrows color | Enter apply | Esc cancel"
        } else {
            "Arrow keys select color | Home current color | Enter apply | Esc cancel"
        }
        .into();
    }
    let read_only = overlay.read_only();
    let mut help = if read_only {
        "Read-only | Up/Down select"
    } else if compact {
        "↑/↓ select | Space toggle | ←/→ side | Ctrl+↑/↓ move | Ctrl+R reset"
    } else {
        "Up/Down select | Space on/off | Left/Right side | Ctrl+Up/Down move | Ctrl+R reset"
    }
    .to_owned();
    if !read_only && overlay.layout().fields()[overlay.selected].id.supports_color() {
        help.push_str(" | C color");
    }
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
    } else {
        help.push_str(if read_only { " | Esc back" } else { " | Esc close" });
    }
    help
}

fn compact_message(
    app: &App,
    compact: bool,
    width: u16,
) -> Option<crate::app::config::OverlayMessage> {
    app.config.overlay_message.as_ref().filter(|_| compact).map(|message| {
        let mut used = 0;
        let mut text: String = message
            .text
            .lines()
            .next()
            .unwrap_or_default()
            .chars()
            .take_while(|ch| {
                used += ch.width().unwrap_or(0);
                used <= usize::from(width.saturating_sub(7))
            })
            .collect();
        if text.len() < message.text.len() {
            text.push_str("...");
        }
        crate::app::config::OverlayMessage { kind: message.kind, text }
    })
}
