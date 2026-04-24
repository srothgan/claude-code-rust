// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use crate::app::{App, AppStatus, FocusOwner};
use crate::ui::input;
use crate::ui::theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const SPINNER_FRAMES: &[char] = &[
    '\u{280B}', '\u{2819}', '\u{2839}', '\u{2838}', '\u{283C}', '\u{2834}', '\u{2826}', '\u{2827}',
    '\u{2807}', '\u{280F}',
];

#[allow(dead_code, clippy::struct_field_names)]
pub(crate) struct SerializedInputRows {
    pub hint_rows: Vec<Line<'static>>,
    pub editor_rows: Vec<Line<'static>>,
    pub plain_editor_rows: Vec<String>,
    pub measurement: InputRowsMeasurement,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InputRowsMeasurement {
    pub hint_rows: u16,
    pub editor_rows: u16,
    pub caret_row: u16,
    pub caret_col: u16,
}

pub(crate) fn build_input_hint_rows(app: &App) -> Vec<Line<'static>> {
    let mut rows = Vec::new();

    if let Some(hint) = &app.login_hint {
        rows.push(Line::from(Span::styled(
            format!("Authentication required: {} -- {}", hint.method_name, hint.method_description),
            Style::default().fg(ratatui::style::Color::Yellow),
        )));
        rows.push(Line::from(Span::styled(
            "Type /login to authenticate, or run `claude auth login` in another terminal",
            Style::default().fg(theme::DIM),
        )));
    }

    if app.pending_cancel_origin.is_some() {
        let spinner_ch = SPINNER_FRAMES[app.spinner_frame % SPINNER_FRAMES.len()];
        rows.push(Line::from(vec![
            Span::styled(format!("{spinner_ch} "), Style::default().fg(theme::DIM)),
            Span::styled(
                "Cancelling current turn... draft will auto-submit when ready.",
                Style::default().fg(theme::DIM),
            ),
        ]));
    }

    if app.input.is_empty()
        && app.focus_owner() == FocusOwner::Input
        && let Some(suggestion) = app.prompt_suggestion.as_deref()
        && !suggestion.trim().is_empty()
    {
        rows.push(Line::from(vec![
            Span::styled("Suggestion: ", Style::default().fg(theme::DIM)),
            Span::styled(
                suggestion.trim().to_owned(),
                Style::default().fg(ratatui::style::Color::White),
            ),
            Span::styled("    Tab to accept", Style::default().fg(theme::DIM)),
        ]));
    }

    rows
}

pub(crate) fn serialize_input_rows(app: &mut App, area_width: u16) -> SerializedInputRows {
    let hint_rows = build_input_hint_rows(app);
    let hint_row_count = u16::try_from(hint_rows.len()).unwrap_or(u16::MAX);
    let geometry =
        input::compute_render_geometry(Rect::new(0, 0, area_width, 1), input::hint_line_count(app));
    if matches!(app.status, AppStatus::Connecting | AppStatus::CommandPending | AppStatus::Error) {
        return serialize_blocked_input_rows(app, hint_rows, hint_row_count, geometry.padded.width);
    }

    let editor_width = geometry.text.width;
    if editor_width == 0 {
        return SerializedInputRows {
            hint_rows,
            editor_rows: Vec::new(),
            plain_editor_rows: Vec::new(),
            measurement: InputRowsMeasurement {
                hint_rows: hint_row_count,
                editor_rows: 0,
                caret_row: 0,
                caret_col: 0,
            },
        };
    }

    let editor_height =
        input::visual_line_count(app, area_width).saturating_sub(input::hint_line_count(app));
    let editor_area = Rect::new(0, 0, editor_width, editor_height.max(1));
    let mut buf = Buffer::empty(editor_area);

    input::configure_input_textarea(app);
    app.input.editor().render(editor_area, &mut buf);
    if let Some(selection) = app.selection
        && selection.kind == crate::app::SelectionKind::Input
    {
        SelectionOverlay { selection }.render(editor_area, &mut buf);
    }

    let measurement = InputRowsMeasurement {
        hint_rows: hint_row_count,
        editor_rows: editor_height.max(1),
        ..measure_input_caret(app, editor_width)
    };
    let mut editor_rows = buffer_rows_to_lines(&buf, editor_area);
    let mut plain_editor_rows = buffer_rows_to_plain_strings(&buf, editor_area);
    let measurement = apply_prompt_prefix(&mut editor_rows, &mut plain_editor_rows, measurement);

    SerializedInputRows { hint_rows, editor_rows, plain_editor_rows, measurement }
}

fn serialize_blocked_input_rows(
    app: &App,
    hint_rows: Vec<Line<'static>>,
    hint_row_count: u16,
    padded_width: u16,
) -> SerializedInputRows {
    if padded_width == 0 {
        return SerializedInputRows {
            hint_rows,
            editor_rows: Vec::new(),
            plain_editor_rows: Vec::new(),
            measurement: InputRowsMeasurement {
                hint_rows: hint_row_count,
                editor_rows: 0,
                caret_row: 0,
                caret_col: 0,
            },
        };
    }

    let editor_rows = blocked_input_lines(app);
    let editor_row_count = u16::try_from(editor_rows.len()).unwrap_or(u16::MAX);
    let plain_editor_rows = editor_rows.iter().map(line_plain_text).collect();

    SerializedInputRows {
        hint_rows,
        editor_rows,
        plain_editor_rows,
        measurement: InputRowsMeasurement {
            hint_rows: hint_row_count,
            editor_rows: editor_row_count,
            caret_row: 0,
            caret_col: 0,
        },
    }
}

fn blocked_input_lines(app: &App) -> Vec<Line<'static>> {
    match app.status {
        AppStatus::Connecting => {
            let spinner_ch = SPINNER_FRAMES[app.spinner_frame % SPINNER_FRAMES.len()];
            vec![Line::from(vec![
                Span::styled(format!("{spinner_ch} "), Style::default().fg(theme::DIM)),
                Span::styled("Connecting to Claude Code...", Style::default().fg(theme::DIM)),
            ])]
        }
        AppStatus::CommandPending => {
            let spinner_ch = SPINNER_FRAMES[app.spinner_frame % SPINNER_FRAMES.len()];
            let label = app.pending_command_label.as_deref().unwrap_or("Processing command...");
            vec![Line::from(vec![
                Span::styled(format!("{spinner_ch} "), Style::default().fg(theme::DIM)),
                Span::styled(label.to_owned(), Style::default().fg(theme::DIM)),
            ])]
        }
        AppStatus::Error => vec![
            Line::from(Span::styled(
                "Input disabled due to error",
                Style::default().fg(theme::STATUS_ERROR),
            )),
            Line::from(Span::styled(
                "Press Ctrl+Q to quit and try again.",
                Style::default().fg(theme::DIM),
            )),
        ],
        AppStatus::Ready | AppStatus::Thinking | AppStatus::Running => Vec::new(),
    }
}

fn apply_prompt_prefix(
    editor_rows: &mut Vec<Line<'static>>,
    plain_editor_rows: &mut Vec<String>,
    mut measurement: InputRowsMeasurement,
) -> InputRowsMeasurement {
    let prefix = prompt_prefix_text();
    let prefix_style = Style::default().fg(theme::RUST_ORANGE);

    if let Some(first_row) = editor_rows.first_mut() {
        let existing = std::mem::take(&mut first_row.spans);
        let mut spans = Vec::with_capacity(existing.len().saturating_add(1));
        spans.push(Span::styled(prefix.clone(), prefix_style));
        spans.extend(existing);
        *first_row = Line::from(spans);
    }

    if let Some(first_plain_row) = plain_editor_rows.first_mut() {
        first_plain_row.insert_str(0, &prefix);
    } else {
        plain_editor_rows.push(prefix.clone());
    }

    if measurement.caret_row == 0 {
        measurement.caret_col = measurement.caret_col.saturating_add(prompt_prefix_width());
    }
    measurement
}

fn prompt_prefix_text() -> String {
    format!("{} ", theme::PROMPT_CHAR)
}

fn prompt_prefix_width() -> u16 {
    u16::try_from(UnicodeWidthStr::width(prompt_prefix_text().as_str())).unwrap_or(u16::MAX)
}

fn line_plain_text(line: &Line<'_>) -> String {
    line.spans.iter().map(|span| span.content.as_ref()).collect()
}

pub(super) struct SelectionOverlay {
    pub selection: crate::app::SelectionState,
}

impl Widget for SelectionOverlay {
    #[allow(clippy::cast_possible_truncation)]
    fn render(self, area: Rect, buf: &mut Buffer) {
        let (start, end) =
            crate::app::normalize_selection(self.selection.start, self.selection.end);
        for row in start.row..=end.row {
            let y = area.y.saturating_add(row as u16);
            if y >= area.bottom() {
                break;
            }
            let row_start = if row == start.row { start.col } else { 0 };
            let row_end = if row == end.row { end.col } else { area.width as usize };
            for col in row_start..row_end {
                let x = area.x.saturating_add(col as u16);
                if x >= area.right() {
                    break;
                }
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_style(cell.style().add_modifier(Modifier::REVERSED));
                }
            }
        }
    }
}

fn buffer_rows_to_lines(buf: &Buffer, area: Rect) -> Vec<Line<'static>> {
    (0..area.height).map(|row| buffer_row_to_line(buf, area, row)).collect()
}

fn buffer_rows_to_plain_strings(buf: &Buffer, area: Rect) -> Vec<String> {
    let mut rows = Vec::with_capacity(area.height as usize);
    for row in 0..area.height {
        let y = area.y.saturating_add(row);
        let mut line = String::new();
        for x in 0..area.width {
            if let Some(cell) = buf.cell((area.x.saturating_add(x), y)) {
                line.push_str(cell.symbol());
            }
        }
        rows.push(line.trim_end().to_owned());
    }
    rows
}

fn buffer_row_to_line(buf: &Buffer, area: Rect, row: u16) -> Line<'static> {
    let y = area.y.saturating_add(row);
    let mut cells = Vec::with_capacity(usize::from(area.width));
    for x in 0..area.width {
        if let Some(cell) = buf.cell((area.x.saturating_add(x), y)) {
            cells.push((cell.symbol().to_owned(), cell.style()));
        }
    }

    let Some(last_non_blank) = cells
        .iter()
        .rposition(|(symbol, _)| !symbol.is_empty() && !symbol.chars().all(char::is_whitespace))
    else {
        return Line::default();
    };

    let mut spans = Vec::new();
    let mut current_style = None;
    let mut current_text = String::new();

    for (symbol, style) in cells.into_iter().take(last_non_blank + 1) {
        if symbol.is_empty() {
            continue;
        }
        match current_style {
            Some(existing) if existing == style => current_text.push_str(&symbol),
            Some(existing) => {
                spans.push(Span::styled(std::mem::take(&mut current_text), existing));
                current_text.push_str(&symbol);
                current_style = Some(style);
            }
            None => {
                current_text.push_str(&symbol);
                current_style = Some(style);
            }
        }
    }

    if let Some(style) = current_style
        && !current_text.is_empty()
    {
        spans.push(Span::styled(current_text, style));
    }

    Line::from(spans)
}

fn measure_input_caret(app: &App, editor_width: u16) -> InputRowsMeasurement {
    if app.input.is_empty() || editor_width == 0 {
        return InputRowsMeasurement::default();
    }

    let (cursor_row, cursor_col) = app.input.cursor();
    let (caret_row, caret_col) =
        caret_visual_position(app.input.lines(), cursor_row, cursor_col, editor_width);

    InputRowsMeasurement { caret_row, caret_col, ..InputRowsMeasurement::default() }
}

fn caret_visual_position(
    lines: &[String],
    target_row: usize,
    target_col: usize,
    width: u16,
) -> (u16, u16) {
    let width = usize::from(width);
    if width == 0 {
        return (0, 0);
    }

    let mut visual_row = 0u16;
    for (row_idx, line) in lines.iter().enumerate() {
        let mut visual_col = 0usize;
        let mut char_idx = 0usize;

        if row_idx == target_row && target_col == 0 {
            return (visual_row, 0);
        }

        for ch in line.chars() {
            if row_idx == target_row && char_idx == target_col {
                return (visual_row, u16::try_from(visual_col).unwrap_or(u16::MAX));
            }

            let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
            if ch_width > 0 && visual_col + ch_width > width && visual_col > 0 {
                visual_row = visual_row.saturating_add(1);
                visual_col = 0;
            }

            if ch_width > width && visual_col == 0 {
                visual_row = visual_row.saturating_add(1);
                char_idx += 1;
                continue;
            }

            visual_col += ch_width;
            char_idx += 1;
        }

        if row_idx == target_row && char_idx == target_col {
            if visual_col >= width {
                return (visual_row.saturating_add(1), 0);
            }
            return (visual_row, u16::try_from(visual_col).unwrap_or(u16::MAX));
        }

        visual_row = visual_row.saturating_add(1);
    }

    (visual_row, 0)
}

#[cfg(test)]
mod tests {
    use super::{build_input_hint_rows, serialize_input_rows};
    use crate::app::{
        App, AppStatus, CancelOrigin, FocusTarget, LoginHint, SelectionKind, SelectionPoint,
        SelectionState,
    };
    use crate::ui::theme;
    use ratatui::style::Modifier;

    fn line_text(line: &ratatui::text::Line<'_>) -> String {
        line.spans.iter().map(|span| span.content.as_ref()).collect()
    }

    #[test]
    fn build_input_hint_rows_preserves_login_hint_content() {
        let mut app = App::test_default();
        app.login_hint = Some(LoginHint {
            method_name: "oauth".to_owned(),
            method_description: "Sign in".to_owned(),
        });

        let rows = build_input_hint_rows(&app);
        assert_eq!(rows.len(), 2);
        assert!(line_text(&rows[0]).contains("Authentication required: oauth -- Sign in"));
    }

    #[test]
    fn build_input_hint_rows_preserves_cancel_and_suggestion_rows() {
        let mut app = App::test_default();
        app.pending_cancel_origin = Some(CancelOrigin::AutoQueue);
        app.prompt_suggestion = Some("Write tests".to_owned());

        let rows = build_input_hint_rows(&app);
        assert_eq!(rows.len(), 2);
        assert!(line_text(&rows[0]).contains("Cancelling current turn"));
        assert!(line_text(&rows[1]).contains("Suggestion: Write tests"));
    }

    #[test]
    fn serialize_input_rows_preserves_placeholder_behavior_for_empty_input() {
        let mut app = App::test_default();

        let serialized = serialize_input_rows(&mut app, 80);
        assert_eq!(serialized.editor_rows.len(), 1);
        assert!(line_text(&serialized.editor_rows[0]).contains(theme::PROMPT_CHAR));
        assert!(line_text(&serialized.editor_rows[0]).contains("Type a message..."));
        assert!(serialized.plain_editor_rows[0].starts_with(theme::PROMPT_CHAR));
        assert_eq!(serialized.measurement.editor_rows, 1);
        assert_eq!(serialized.measurement.caret_row, 0);
        assert_eq!(serialized.measurement.caret_col, 2);
    }

    #[test]
    fn multiline_snapshot_matches_plain_rows() {
        let mut app = App::test_default();
        app.input.set_text("alpha beta gamma delta\nepsilon");

        let serialized = serialize_input_rows(&mut app, 16);
        assert!(serialized.plain_editor_rows.iter().any(|row| row.contains("alpha")));
        assert!(serialized.plain_editor_rows.iter().any(|row| row.contains("epsilon")));
    }

    #[test]
    fn input_measurement_reports_single_line_caret_position() {
        let mut app = App::test_default();
        app.input.set_text("hello");
        let _ = app.input.set_cursor(0, 5);

        let serialized = serialize_input_rows(&mut app, 80);

        assert_eq!(serialized.measurement.hint_rows, 0);
        assert_eq!(serialized.measurement.editor_rows, 1);
        assert_eq!(serialized.measurement.caret_row, 0);
        assert_eq!(serialized.measurement.caret_col, 7);
    }

    #[test]
    fn input_measurement_reports_wrapped_caret_position() {
        let mut app = App::test_default();
        app.input.set_text("helloX");
        let _ = app.input.set_cursor(0, 6);

        let serialized = serialize_input_rows(&mut app, 12);

        assert_eq!(serialized.measurement.editor_rows, 2);
        assert_eq!(serialized.measurement.caret_row, 1);
        assert_eq!(serialized.measurement.caret_col, 1);
    }

    #[test]
    fn input_measurement_reports_multiline_caret_position() {
        let mut app = App::test_default();
        app.input.set_text("abc\ndefg");
        let _ = app.input.set_cursor(1, 2);

        let serialized = serialize_input_rows(&mut app, 80);

        assert_eq!(serialized.measurement.editor_rows, 2);
        assert_eq!(serialized.measurement.caret_row, 1);
        assert_eq!(serialized.measurement.caret_col, 2);
    }

    #[test]
    fn slash_highlight_survives_row_serialization() {
        let mut app = App::test_default();
        app.input.set_text("/mode plan");

        let serialized = serialize_input_rows(&mut app, 80);
        let slash_span = serialized.editor_rows[0]
            .spans
            .iter()
            .find(|span| span.content.as_ref().contains("/mode"))
            .expect("slash span");
        assert_eq!(slash_span.style.fg, Some(crate::ui::theme::SLASH_COMMAND));
    }

    #[test]
    fn mention_highlight_survives_row_serialization() {
        let mut app = App::test_default();
        app.input.set_text("@src/main.rs");

        let serialized = serialize_input_rows(&mut app, 80);
        let mention_span = serialized.editor_rows[0]
            .spans
            .iter()
            .find(|span| span.content.as_ref().contains("@src/main.rs"))
            .expect("mention span");
        assert_eq!(mention_span.style.fg, Some(ratatui::style::Color::Cyan));
    }

    #[test]
    fn subagent_highlight_survives_row_serialization() {
        let mut app = App::test_default();
        app.input.set_text("&reviewer");

        let serialized = serialize_input_rows(&mut app, 80);
        let span = serialized.editor_rows[0]
            .spans
            .iter()
            .find(|span| span.content.as_ref().contains("&reviewer"))
            .expect("subagent span");
        assert_eq!(span.style.fg, Some(crate::ui::theme::SUBAGENT_TOKEN));
    }

    #[test]
    fn paste_placeholder_highlight_survives_row_serialization() {
        let mut app = App::test_default();
        app.input.set_text("[Pasted Text 1]");

        let serialized = serialize_input_rows(&mut app, 80);
        let span = serialized.editor_rows[0]
            .spans
            .iter()
            .find(|span| span.content.as_ref().contains("[Pasted Text 1]"))
            .expect("paste span");
        assert_eq!(span.style.fg, Some(ratatui::style::Color::Green));
    }

    #[test]
    fn image_badge_highlight_survives_row_serialization() {
        let mut app = App::test_default();
        app.input.set_text("[Image #1]");

        let serialized = serialize_input_rows(&mut app, 80);
        let span = serialized.editor_rows[0]
            .spans
            .iter()
            .find(|span| span.content.as_ref().contains("[Image #1]"))
            .expect("image badge span");
        assert_eq!(span.style.fg, Some(ratatui::style::Color::Cyan));
        assert!(span.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn input_selection_overlay_is_reflected_in_serialized_rows() {
        let mut app = App::test_default();
        app.input.set_text("abcdef");
        app.selection = Some(SelectionState {
            kind: SelectionKind::Input,
            start: SelectionPoint { row: 0, col: 1 },
            end: SelectionPoint { row: 0, col: 4 },
            dragging: false,
        });

        let serialized = serialize_input_rows(&mut app, 80);
        let selected_span = serialized.editor_rows[0]
            .spans
            .iter()
            .find(|span| span.style.add_modifier.contains(Modifier::REVERSED))
            .expect("selected span");
        assert!(selected_span.content.as_ref().contains("bcd"));
        assert_eq!(serialized.plain_editor_rows[0], "abcdef");
    }

    #[test]
    fn prompt_suggestion_hint_requires_input_focus() {
        let mut app = App::test_default();
        app.prompt_suggestion = Some("Write tests".to_owned());
        app.show_todo_panel = true;
        app.todos.push(crate::app::TodoItem {
            content: "todo".to_owned(),
            status: crate::app::TodoStatus::Pending,
            active_form: String::new(),
        });
        app.claim_focus_target(FocusTarget::TodoList);

        let rows = build_input_hint_rows(&app);
        assert!(rows.is_empty());
    }

    #[test]
    fn serialize_input_rows_shows_connecting_status_as_visible_editor_row() {
        let mut app = App::test_default();
        app.status = AppStatus::Connecting;
        app.spinner_frame = 3;

        let serialized = serialize_input_rows(&mut app, 80);

        assert_eq!(serialized.editor_rows.len(), 1);
        assert!(line_text(&serialized.editor_rows[0]).contains("Connecting to Claude Code..."));
        assert_eq!(serialized.measurement.editor_rows, 1);
        assert_eq!(serialized.measurement.caret_col, 0);
    }

    #[test]
    fn serialize_input_rows_shows_pending_command_label() {
        let mut app = App::test_default();
        app.status = AppStatus::CommandPending;
        app.pending_command_label = Some("Switching model...".to_owned());

        let serialized = serialize_input_rows(&mut app, 80);

        assert_eq!(serialized.editor_rows.len(), 1);
        assert!(line_text(&serialized.editor_rows[0]).contains("Switching model..."));
        assert_eq!(serialized.measurement.editor_rows, 1);
    }

    #[test]
    fn serialize_input_rows_shows_error_rows() {
        let mut app = App::test_default();
        app.status = AppStatus::Error;

        let serialized = serialize_input_rows(&mut app, 80);

        assert_eq!(serialized.editor_rows.len(), 2);
        assert!(line_text(&serialized.editor_rows[0]).contains("Input disabled due to error"));
        assert!(
            line_text(&serialized.editor_rows[1]).contains("Press Ctrl+Q to quit and try again.")
        );
        assert_eq!(serialized.measurement.editor_rows, 2);
    }
}
