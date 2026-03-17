// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use crate::agent::model;
use crate::app::App;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::theme;

const FOOTER_PAD: u16 = 2;
const FOOTER_COLUMN_GAP: u16 = 1;
type FooterItem = Option<(String, Color)>;

pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    let padded = Rect {
        x: area.x.saturating_add(FOOTER_PAD),
        y: area.y,
        width: area.width.saturating_sub(FOOTER_PAD * 2),
        height: area.height,
    };

    if app.cached_footer_line.is_none() {
        let line = if let Some(ref mode) = app.mode {
            let color = mode_color(&mode.current_mode_id);
            let (fast_mode_text, fast_mode_color) = fast_mode_badge(app.fast_mode_state);
            Line::from(vec![
                Span::styled("[", Style::default().fg(color)),
                Span::styled(mode.current_mode_name.clone(), Style::default().fg(color)),
                Span::styled("]", Style::default().fg(color)),
                Span::raw("  "),
                Span::styled("[", Style::default().fg(fast_mode_color)),
                Span::styled(fast_mode_text, Style::default().fg(fast_mode_color)),
                Span::styled("]", Style::default().fg(fast_mode_color)),
                Span::raw("  "),
                Span::styled("?", Style::default().fg(Color::White)),
                Span::styled(" : Help", Style::default().fg(theme::DIM)),
            ])
        } else {
            Line::from(vec![
                Span::styled("?", Style::default().fg(Color::White)),
                Span::styled(" : Help", Style::default().fg(theme::DIM)),
            ])
        };
        app.cached_footer_line = Some(line);
    }

    if let Some(line) = &app.cached_footer_line {
        let left_min = u16::try_from(line.width()).unwrap_or(u16::MAX);

        if let Some((hint_text, hint_color)) = footer_update_hint(app) {
            let (left_area, right_area) = split_footer_columns_hint(padded, left_min);
            frame.render_widget(Paragraph::new(line.clone()), left_area);
            render_footer_right_info(frame, right_area, &hint_text, hint_color);
        } else {
            frame.render_widget(Paragraph::new(line.clone()), padded);
        }
    }
}

fn footer_update_hint(app: &App) -> FooterItem {
    app.update_check_hint.as_ref().map(|hint| (hint.clone(), theme::RUST_ORANGE))
}

fn split_footer_columns_hint(area: Rect, left_min_width: u16) -> (Rect, Rect) {
    if area.width == 0 {
        return (area, Rect { width: 0, ..area });
    }

    let [left, right] =
        Layout::horizontal([Constraint::Length(left_min_width), Constraint::Fill(1)])
            .spacing(FOOTER_COLUMN_GAP)
            .areas(area);
    (left, right)
}

fn fit_footer_right_text(text: &str, max_width: usize) -> Option<String> {
    if max_width == 0 || text.trim().is_empty() {
        return None;
    }

    if UnicodeWidthStr::width(text) <= max_width {
        return Some(text.to_owned());
    }

    if max_width <= 3 {
        return Some(".".repeat(max_width));
    }

    let mut fitted = String::new();
    let mut width: usize = 0;
    for ch in text.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width.saturating_add(ch_width).saturating_add(3) > max_width {
            break;
        }
        fitted.push(ch);
        width = width.saturating_add(ch_width);
    }

    if fitted.is_empty() {
        return Some("...".to_owned());
    }
    fitted.push_str("...");
    Some(fitted)
}

fn render_footer_right_info(frame: &mut Frame, area: Rect, right_text: &str, right_color: Color) {
    if area.width == 0 {
        return;
    }
    let Some(fitted) = fit_footer_right_text(right_text, usize::from(area.width)) else {
        return;
    };

    let line = Line::from(Span::styled(fitted, Style::default().fg(right_color)));
    frame.render_widget(Paragraph::new(line).alignment(Alignment::Right), area);
}

fn mode_color(mode_id: &str) -> Color {
    match mode_id {
        "default" => theme::DIM,
        "plan" => Color::Blue,
        "acceptEdits" => Color::Yellow,
        "bypassPermissions" | "dontAsk" => Color::Red,
        _ => Color::Magenta,
    }
}

fn fast_mode_badge(state: model::FastModeState) -> (&'static str, Color) {
    match state {
        model::FastModeState::Off => ("FAST:OFF", theme::DIM),
        model::FastModeState::Cooldown => ("FAST:CD", Color::Yellow),
        model::FastModeState::On => ("FAST:ON", theme::RUST_ORANGE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::model;
    use crate::app::App;

    #[test]
    fn split_footer_columns_hint_left_gets_its_minimum() {
        let area = Rect::new(0, 0, 80, 1);
        let left_min = 24u16;
        let (left, right) = split_footer_columns_hint(area, left_min);
        assert_eq!(left.width + FOOTER_COLUMN_GAP + right.width, 80);
        assert!(left.width >= left_min);
    }

    #[test]
    fn split_footer_columns_hint_right_fills_remainder() {
        let area = Rect::new(0, 0, 80, 1);
        let left_min = 24u16;
        let (left, right) = split_footer_columns_hint(area, left_min);
        assert_eq!(left.width, left_min);
        assert_eq!(right.width, 80 - FOOTER_COLUMN_GAP - left_min);
    }

    #[test]
    fn split_footer_columns_hint_zero_width() {
        let area = Rect::new(0, 0, 0, 1);
        let (left, right) = split_footer_columns_hint(area, 24);
        assert_eq!(left.width, 0);
        assert_eq!(right.width, 0);
    }

    #[test]
    fn fit_footer_right_text_truncates_when_needed() {
        let text = "Update available: v9.9.9 (current v0.2.0)";
        let fitted = fit_footer_right_text(text, 12).expect("fitted text");
        assert!(fitted.ends_with("..."));
        assert!(UnicodeWidthStr::width(fitted.as_str()) <= 12);
    }

    #[test]
    fn fit_footer_right_text_keeps_prefix() {
        let text = "Compacting context now and applying update hint";
        let fitted = fit_footer_right_text(text, 20).expect("fitted text");
        assert!(fitted.starts_with("Compacting"));
        assert!(UnicodeWidthStr::width(fitted.as_str()) <= 20);
    }

    #[test]
    fn footer_update_hint_none_without_hint() {
        let app = App::test_default();
        assert_eq!(footer_update_hint(&app), None);
    }

    #[test]
    fn footer_update_hint_returns_text_when_present() {
        let mut app = App::test_default();
        app.update_check_hint = Some("Update available".to_owned());
        assert_eq!(
            footer_update_hint(&app),
            Some(("Update available".to_owned(), theme::RUST_ORANGE))
        );
    }

    #[test]
    fn fast_mode_badge_maps_cooldown_to_cd() {
        let (label, _) = fast_mode_badge(model::FastModeState::Cooldown);
        assert_eq!(label, "FAST:CD");
    }
}
