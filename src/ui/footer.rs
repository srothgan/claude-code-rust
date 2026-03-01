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
const FOOTER_SPINNER_FRAMES: &[char] = &[
    '\u{280B}', '\u{2819}', '\u{2839}', '\u{2838}', '\u{283C}', '\u{2834}', '\u{2826}', '\u{2827}',
    '\u{2807}', '\u{280F}',
];

pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    // #1 fix: saturating_add prevents u16 overflow when area.x is near u16::MAX.
    let padded = Rect {
        x: area.x.saturating_add(FOOTER_PAD),
        y: area.y,
        width: area.width.saturating_sub(FOOTER_PAD * 2),
        height: area.height,
    };

    if app.cached_footer_line.is_none() {
        let line = if let Some(ref mode) = app.mode {
            let color = mode_color(&mode.current_mode_id);
            Line::from(vec![
                Span::styled("[", Style::default().fg(color)),
                Span::styled(mode.current_mode_name.clone(), Style::default().fg(color)),
                Span::styled("]", Style::default().fg(color)),
                Span::raw("  "),
                Span::styled("?", Style::default().fg(Color::White)),
                Span::styled(" : Shortcuts + Commands", Style::default().fg(theme::DIM)),
            ])
        } else {
            Line::from(vec![
                Span::styled("?", Style::default().fg(Color::White)),
                Span::styled(" : Shortcuts + Commands", Style::default().fg(theme::DIM)),
            ])
        };
        app.cached_footer_line = Some(line);
    }

    if let Some(line) = &app.cached_footer_line {
        // Measure the natural display width of the left column once per render so both
        // split functions can use Constraint::Min to give it priority.
        // #2 fix: try_from instead of `as u16` to avoid silent truncation.
        let left_min = u16::try_from(line.width()).unwrap_or(u16::MAX);

        let (telemetry, update_hint) = footer_right_items(app);
        match (telemetry, update_hint) {
            (Some((telem_text, telem_color)), Some((hint_text, hint_color))) => {
                // Three columns: left=mode/help (anchored), mid=update hint (fills remainder),
                // right=context (anchored to exact width). The update hint can be truncated.
                let ctx_width =
                    u16::try_from(UnicodeWidthStr::width(telem_text.as_str())).unwrap_or(u16::MAX);
                let (left_area, mid_area, right_area) =
                    split_footer_three_columns(padded, left_min, ctx_width);
                frame.render_widget(Paragraph::new(line.clone()), left_area);
                render_footer_right_info(frame, mid_area, &hint_text, hint_color);
                render_footer_right_info(frame, right_area, &telem_text, telem_color);
            }
            (Some((telem_text, telem_color)), None) => {
                // Two columns, context only: context is always short (≤13 chars) so giving it
                // an exact Length and letting left Fill is sufficient — no minimum needed.
                let right_width =
                    u16::try_from(UnicodeWidthStr::width(telem_text.as_str())).unwrap_or(u16::MAX);
                let (left_area, right_area) = split_footer_columns(padded, right_width);
                frame.render_widget(Paragraph::new(line.clone()), left_area);
                render_footer_right_info(frame, right_area, &telem_text, telem_color);
            }
            (None, Some((hint_text, hint_color))) => {
                // Two columns, hint only: left is anchored via Min so a long hint can never
                // squeeze the mode badge off screen. The hint fills the remainder and truncates
                // gracefully if the terminal is narrow (Ctrl+U dismisses it).
                let (left_area, right_area) = split_footer_columns_hint(padded, left_min);
                frame.render_widget(Paragraph::new(line.clone()), left_area);
                render_footer_right_info(frame, right_area, &hint_text, hint_color);
            }
            (None, None) => {
                frame.render_widget(Paragraph::new(line.clone()), padded);
            }
        }
    }
}

fn context_remaining_percent_rounded(used: u64, window: u64) -> Option<u64> {
    if window == 0 {
        return None;
    }
    let used_percent = (u128::from(used) * 100 + (u128::from(window) / 2)) / u128::from(window);
    Some(100_u64.saturating_sub(used_percent.min(100) as u64))
}

fn context_text(window: Option<u64>, used: Option<u64>, show_new_session_default: bool) -> String {
    if show_new_session_default {
        return "100%".to_owned();
    }

    window
        .zip(used)
        .and_then(|(w, u)| context_remaining_percent_rounded(u, w))
        .map_or_else(|| "-".to_owned(), |percent| format!("{percent}%"))
}

fn footer_telemetry_text(app: &App) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    let totals = &app.session_usage;
    let is_new_session = app.session_id.is_some()
        && totals.total_tokens() == 0
        && totals.context_used_tokens().is_none();
    if app.session_id.is_some()
        || totals.context_window.is_some()
        || totals.context_used_tokens().is_some()
    {
        let context_text =
            context_text(totals.context_window, totals.context_used_tokens(), is_new_session);
        parts.push(format!("Context: {context_text}"));
    }

    if parts.is_empty() && !app.is_compacting {
        return None;
    }

    let mut text = parts.join(" | ");
    if app.is_compacting {
        let ch = FOOTER_SPINNER_FRAMES[app.spinner_frame % FOOTER_SPINNER_FRAMES.len()];
        text = if text.is_empty() {
            format!("{ch} Compacting...")
        } else {
            format!("{ch} Compacting...  {text}")
        };
    }
    Some(text)
}

/// Returns `(telemetry, update_hint)` -- either or both may be `None`.
fn footer_right_items(app: &App) -> (FooterItem, FooterItem) {
    let telemetry = footer_telemetry_text(app).map(|text| {
        let color = if app.is_compacting { theme::RUST_ORANGE } else { theme::DIM };
        (text, color)
    });
    let update_hint = app.update_check_hint.as_ref().map(|hint| (hint.clone(), theme::RUST_ORANGE));
    (telemetry, update_hint)
}

/// Two-column split for context-only: left (mode/shortcuts) | right (context text).
///
/// Context is always short (≤13 chars for "Context: 100%") so `Length` gives it its
/// exact footprint; `Fill(1)` hands everything else to left.
fn split_footer_columns(area: Rect, right_text_width: u16) -> (Rect, Rect) {
    if area.width == 0 {
        return (area, Rect { width: 0, ..area });
    }

    let [left, right] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(right_text_width)])
            .spacing(FOOTER_COLUMN_GAP)
            .areas(area);
    (left, right)
}

/// Two-column split for hint-only: left (mode/shortcuts) | right (update hint).
///
/// Left is pinned to exactly `left_min_width` via `Length` so the mode badge always
/// has its natural space and the update hint gets the maximum possible remainder.
/// The hint is truncated with `...` by `render_footer_right_info` if still too narrow.
/// The user can dismiss the hint entirely with Ctrl+U.
///
/// Note: `Length` (not `Min`) is intentional — `Min` allows the left widget to grow
/// beyond the minimum when there is excess space, stealing room from the hint.
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

/// Three-column split: left (mode/shortcuts) | mid (update hint) | right (context/telemetry).
///
/// Priority order — columns 1 and 3 are anchored first, mid gets whatever is left:
/// - `left` : `Length(left_min_width)` — pinned to its natural display width.
/// - `right`: `Length(context_width)`  — exact fit for "Context: 0%"…"Context: 100%".
/// - `mid`  : `Fill(1)` — update hint fills the remainder; truncated with `...` if narrow.
///
/// Note: `Length` (not `Min`) is intentional for left — `Min` allows left to grow beyond
/// its minimum when there is excess space, which would steal that space from the hint.
///
/// Example at 80 cols (78 available after 2 gaps), `left_min=24` ("? : Shortcuts + Commands"),
/// `context_width=13` ("Context: 37%"):
///   left  = 24  (pinned, Length satisfied)
///   right = 13  (exact, Length satisfied)
///   mid   = 41  (78 − 24 − 13 = 41 for the update hint)
///
/// With [plan] mode (`left_min=32`) on 80 cols:
///   left  = 32, mid = 33, right = 13
fn split_footer_three_columns(
    area: Rect,
    left_min_width: u16,
    context_width: u16,
) -> (Rect, Rect, Rect) {
    if area.width == 0 {
        let zero = Rect { width: 0, ..area };
        return (area, zero, zero);
    }

    let [left, mid, right] = Layout::horizontal([
        Constraint::Length(left_min_width),
        Constraint::Fill(1),
        Constraint::Length(context_width),
    ])
    .spacing(FOOTER_COLUMN_GAP)
    .areas(area);
    (left, mid, right)
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

/// Returns a color for the given mode ID.
fn mode_color(mode_id: &str) -> Color {
    match mode_id {
        "default" => theme::DIM,
        "plan" => Color::Blue,
        "acceptEdits" => Color::Yellow,
        "bypassPermissions" | "dontAsk" => Color::Red,
        _ => Color::Magenta,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::model;
    use crate::app::{
        App, BlockCache, ChatMessage, IncrementalMarkdown, MessageBlock, MessageRole,
    };

    // ── split_footer_columns (context-only, right is exact) ──────────────────

    #[test]
    fn split_footer_columns_right_gets_exact_width() {
        let area = Rect::new(0, 0, 80, 1);
        let right_width = 13u16; // "Context: 37%"
        let (left, right) = split_footer_columns(area, right_width);
        assert_eq!(
            left.width + FOOTER_COLUMN_GAP + right.width,
            80,
            "columns + gap must equal area width"
        );
        assert_eq!(right.width, right_width, "right column gets exactly the measured width");
        assert_eq!(left.width, 80 - FOOTER_COLUMN_GAP - right_width, "left fills the remainder");
    }

    #[test]
    fn split_footer_columns_short_context_gives_left_more_room() {
        // "Context: 0%" is 11 chars — left should get 80-1-11 = 68 chars.
        let area = Rect::new(0, 0, 80, 1);
        let (left, right) = split_footer_columns(area, 11);
        assert_eq!(right.width, 11);
        assert_eq!(left.width, 68);
    }

    #[test]
    fn split_footer_columns_zero_width() {
        let area = Rect::new(0, 0, 0, 1);
        let (left, right) = split_footer_columns(area, 13);
        assert_eq!(left.width, 0);
        assert_eq!(right.width, 0);
    }

    // ── split_footer_columns_hint (hint-only, left is anchored) ─────────────

    #[test]
    fn split_footer_columns_hint_left_gets_its_minimum() {
        // Left (mode/help, 24 chars "? : Shortcuts + Commands") must never shrink
        // even when the update hint is wide.
        let area = Rect::new(0, 0, 80, 1);
        let left_min = 24u16;
        let (left, right) = split_footer_columns_hint(area, left_min);
        assert_eq!(left.width + FOOTER_COLUMN_GAP + right.width, 80);
        assert!(
            left.width >= left_min,
            "left (mode/help) must keep at least its natural width; got {}",
            left.width
        );
    }

    #[test]
    fn split_footer_columns_hint_right_fills_remainder() {
        // With left_min=24 on 80 cols: right should get 80-1-24 = 55 chars —
        // enough for "Update available: v9.9.9 (current v0.2.0)" (42 chars).
        let area = Rect::new(0, 0, 80, 1);
        let left_min = 24u16;
        let (left, right) = split_footer_columns_hint(area, left_min);
        assert_eq!(left.width, left_min, "Min constraint: left gets exactly its minimum");
        assert_eq!(right.width, 80 - FOOTER_COLUMN_GAP - left_min);
    }

    #[test]
    fn split_footer_columns_hint_zero_width() {
        let area = Rect::new(0, 0, 0, 1);
        let (left, right) = split_footer_columns_hint(area, 24);
        assert_eq!(left.width, 0);
        assert_eq!(right.width, 0);
    }

    // ── split_footer_three_columns ───────────────────────────────────────────

    #[test]
    fn split_footer_three_columns_preserves_total_width() {
        let area = Rect::new(0, 0, 80, 1);
        let (left, mid, right) = split_footer_three_columns(area, 24, 13);
        assert_eq!(
            left.width
                .saturating_add(FOOTER_COLUMN_GAP)
                .saturating_add(mid.width)
                .saturating_add(FOOTER_COLUMN_GAP)
                .saturating_add(right.width),
            80,
            "all columns + gaps must equal area width"
        );
    }

    #[test]
    fn split_footer_three_columns_anchors_left_and_right() {
        // Columns 1 and 3 are anchored; mid (update hint) gets whatever is left.
        let area = Rect::new(0, 0, 80, 1);
        let left_min = 24u16; // "? : Shortcuts + Commands"
        let ctx_width = 13u16; // "Context: 37%"
        let (left, mid, right) = split_footer_three_columns(area, left_min, ctx_width);

        assert_eq!(right.width, ctx_width, "right (context) gets its exact measured width");
        assert!(
            left.width >= left_min,
            "left (mode/help) must keep at least its natural width; got {}",
            left.width
        );
        // mid gets the flex remainder: 80 - 2 gaps - left_min - ctx_width = 41
        assert_eq!(mid.width, 80 - 2 * FOOTER_COLUMN_GAP - left_min - ctx_width);
    }

    #[test]
    fn split_footer_three_columns_mid_gets_useful_width_for_hint() {
        // Real hint: "Update available: v9.9.9 (current v0.2.0)" = 42 chars.
        // On 80 cols with left_min=24 and ctx=13: mid should get 41.
        // On 120 cols it gets 81 — plenty of room.
        for terminal_width in [80u16, 100, 120, 160, 200] {
            let area = Rect::new(0, 0, terminal_width, 1);
            let (_, mid, _) = split_footer_three_columns(area, 24, 13);
            assert!(
                mid.width >= 30,
                "mid should have room for the update hint at width {terminal_width}, got {}",
                mid.width
            );
        }
    }

    #[test]
    fn split_footer_three_columns_context_variants() {
        // All real context text widths must be reflected exactly in the right column.
        for (label, ctx_w) in [
            ("Context: 100%", 13u16),
            ("Context: 37%", 12),
            ("Context: 0%", 11),
            ("Context: -", 10),
        ] {
            let area = Rect::new(0, 0, 80, 1);
            let (left, mid, right) = split_footer_three_columns(area, 24, ctx_w);
            assert_eq!(
                right.width, ctx_w,
                "right column width should match measured width of \"{label}\""
            );
            assert_eq!(
                left.width + FOOTER_COLUMN_GAP + mid.width + FOOTER_COLUMN_GAP + right.width,
                80
            );
        }
    }

    #[test]
    fn split_footer_three_columns_zero_width() {
        let area = Rect::new(0, 0, 0, 1);
        let (left, mid, right) = split_footer_three_columns(area, 24, 13);
        assert_eq!(left.width, 0);
        assert_eq!(mid.width, 0);
        assert_eq!(right.width, 0);
    }

    // ── fit_footer_right_text ────────────────────────────────────────────────

    #[test]
    fn fit_footer_right_text_truncates_when_needed() {
        let text = "Context: 37%";
        let fitted = fit_footer_right_text(text, 8).expect("fitted text");
        assert!(fitted.ends_with("..."));
        assert!(UnicodeWidthStr::width(fitted.as_str()) <= 8);
    }

    #[test]
    fn fit_footer_right_text_keeps_compacting_prefix() {
        let text = "\u{280B} Compacting...  Context: 37%";
        let fitted = fit_footer_right_text(text, 20).expect("fitted text");
        assert!(fitted.starts_with('\u{280B}'));
        assert!(UnicodeWidthStr::width(fitted.as_str()) <= 20);
    }

    // ── context helpers ──────────────────────────────────────────────────────

    #[test]
    fn context_text_new_session_defaults_to_full() {
        assert_eq!(context_text(None, None, true), "100%");
        assert_eq!(context_text(Some(200_000), None, true), "100%");
    }

    #[test]
    fn context_text_unknown_when_not_new_session() {
        assert_eq!(context_text(None, None, false), "-");
        assert_eq!(context_text(Some(200_000), None, false), "-");
    }

    #[test]
    fn context_text_computes_percent_when_defined() {
        assert_eq!(context_text(Some(200_000), Some(100_000), false), "50%");
    }

    // ── footer_telemetry_text ────────────────────────────────────────────────

    #[test]
    fn footer_telemetry_new_session_uses_unknown_defaults() {
        let mut app = App::test_default();
        app.session_id = Some(model::SessionId::new("session-new"));

        let text = footer_telemetry_text(&app).expect("footer telemetry");
        assert_eq!(text, "Context: 100%");
    }

    #[test]
    fn footer_telemetry_still_defaults_to_full_after_first_user_message() {
        let mut app = App::test_default();
        app.session_id = Some(model::SessionId::new("session-new"));
        app.messages.push(ChatMessage {
            role: MessageRole::User,
            blocks: vec![MessageBlock::Text(
                "hello".to_owned(),
                BlockCache::default(),
                IncrementalMarkdown::from_complete("hello"),
            )],
            usage: None,
        });

        let text = footer_telemetry_text(&app).expect("footer telemetry");
        assert_eq!(text, "Context: 100%");
    }

    #[test]
    fn footer_telemetry_resume_ignores_cost_and_tokens() {
        let mut app = App::test_default();
        app.session_id = Some(model::SessionId::new("session-resume"));
        app.session_usage.total_input_tokens = 400;
        app.session_usage.total_cost_usd = Some(0.35);
        app.session_usage.cost_is_since_resume = true;

        let text = footer_telemetry_text(&app).expect("footer telemetry");
        assert_eq!(text, "Context: -");
    }
}
