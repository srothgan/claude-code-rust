// SPDX-License-Identifier: Apache-2.0
use crate::app::App;
use crate::app::TrustSelection;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Margin};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::theme;

const QUESTION: &str = "Trust this project directory?";
const WAIT_NOTE: &str =
    "Claude Rust will wait here until you choose whether to trust this workspace.";
const REVIEW_NOTE: &str = "Review the project before continuing if you are unsure.";
const CHOICE_HINT: &str = "Choose Yes to continue or No to close Claude Rust.";
const PATH_LABEL: &str = "Path: ";
const ELLIPSIS: char = '\u{2026}';

#[cfg(test)]
#[path = "trust_audit_tests.rs"]
mod audit_tests;

pub fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();

    let outer = Block::default()
        .borders(Borders::ALL)
        .title("Trust project")
        .border_style(Style::default().fg(theme::DIM));
    frame.render_widget(outer, area);

    let inner = area.inner(Margin { vertical: 1, horizontal: 2 });
    let width = inner.width;
    let question = Line::from(Span::styled(
        QUESTION,
        Style::default().fg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD),
    ));
    let error_style = Style::default().fg(theme::STATUS_ERROR);
    let message = match &app.trust.last_error {
        Some(error) => Line::from(Span::styled(error.clone(), error_style)),
        None => Line::from(Span::styled(CHOICE_HINT, Style::default().fg(theme::DIM))),
    };
    let actions = action_lines(app);
    // Error data may contain meaningful spaces in a directory or filename.
    // Use the same column layout to measure and render it, without word trimming.
    let message_height = app.trust.last_error.as_ref().map_or_else(
        || wrapped_line_count(Text::from(message.clone()), width),
        |error| {
            u16::try_from(chunk_columns(error.chars(), usize::from(width)).len())
                .unwrap_or(u16::MAX)
                .max(1)
        },
    );

    // Rows are handed out by priority, not in screen order: the choices, then
    // an error, then the path with what is left (cut behind a leading ellipsis
    // when that is not enough, and gone only when no row is left). The
    // question, the hint and the explanation lines follow in that order, each
    // only if all of its rows still fit, and the blank rows around the path
    // come last, both or neither.
    let mut free = inner.height;
    let action_rows = wrapped_line_count(Text::from(actions.clone()), width).min(free);
    free -= action_rows;
    let mut message_rows = 0;
    if app.trust.last_error.is_some() {
        message_rows = message_height.min(free);
        free -= message_rows;
    }
    let path = path_rows(&app.trust.project_key, width, usize::from(free));
    let path_height = u16::try_from(path.len()).unwrap_or(u16::MAX);
    free -= path_height;
    let mut fit = |height: u16| {
        if height > free {
            return 0;
        }
        free -= height;
        height
    };
    let question_rows = fit(wrapped_line_count(Text::from(question.clone()), width));
    if app.trust.last_error.is_none() {
        message_rows = fit(message_height);
    }
    let wait_rows = fit(wrapped_line_count(Text::from(WAIT_NOTE), width));
    let review_rows = fit(wrapped_line_count(Text::from(REVIEW_NOTE), width));
    // One blank row above the path and one below it, or neither.
    let gap_rows = fit(2) / 2;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(question_rows),
            Constraint::Length(wait_rows),
            Constraint::Length(gap_rows),
            Constraint::Length(path_height),
            Constraint::Length(gap_rows),
            Constraint::Length(review_rows),
            Constraint::Length(message_rows),
            Constraint::Length(action_rows),
            Constraint::Min(0),
        ])
        .split(inner);

    let wrap = Wrap { trim: false };
    // Preserve error data exactly, with a trailing ellipsis when it cannot fit.
    let message = match &app.trust.last_error {
        Some(error) => Paragraph::new(
            fit_chunks(error, usize::from(width), usize::from(message_rows), Keep::Start)
                .into_iter()
                .map(|row| Line::from(Span::styled(row, error_style)))
                .collect::<Vec<_>>(),
        ),
        _ => Paragraph::new(message).wrap(wrap),
    };
    frame.render_widget(Paragraph::new(question).wrap(wrap), chunks[0]);
    frame.render_widget(Paragraph::new(WAIT_NOTE).wrap(wrap), chunks[1]);
    frame.render_widget(Paragraph::new(path), chunks[3]);
    frame.render_widget(Paragraph::new(REVIEW_NOTE).wrap(wrap), chunks[5]);
    frame.render_widget(message, chunks[6]);
    frame.render_widget(Paragraph::new(actions).wrap(wrap), chunks[7]);
}

/// The path in rows of at most `width` columns, the first behind the label
/// and the rest indented to start under the path. Without room beside the
/// label for the ellipsis and one double-width character, the path is drawn
/// unlabelled.
fn path_rows(path: &str, width: u16, max_rows: usize) -> Vec<Line<'static>> {
    let width = usize::from(width);
    let label_width = PATH_LABEL.width();
    let (label, path_width) =
        if width >= label_width + 3 { (PATH_LABEL, width - label_width) } else { ("", width) };
    let bold = Style::default().add_modifier(Modifier::BOLD);
    // The end of a path is what tells projects apart.
    fit_chunks(path, path_width, max_rows, Keep::End)
        .into_iter()
        .enumerate()
        .map(|(index, chunk)| {
            let lead = if index == 0 { label.to_owned() } else { " ".repeat(label.width()) };
            Line::from(vec![Span::raw(lead), Span::styled(chunk, bold)])
        })
        .collect()
}

/// The end of a text that survives a cut.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Keep {
    Start,
    End,
}

/// Cuts `text` into chunks of at most `width` columns. When it needs more than
/// `max_rows`, keeps the `keep` end and puts an ellipsis where it was cut.
fn fit_chunks(text: &str, width: usize, max_rows: usize, keep: Keep) -> Vec<String> {
    if text.is_empty() || width == 0 || max_rows == 0 {
        return Vec::new();
    }
    let rows = chunk_columns(text.chars(), width);
    if rows.len() <= max_rows {
        return rows;
    }

    // Collected from the kept end, so the cut is always at the back.
    let from_kept_end: Vec<char> = match keep {
        Keep::Start => text.chars().collect(),
        Keep::End => text.chars().rev().collect(),
    };
    // Double-width characters can leave a column unused at the end of a row, so
    // the first budget may need more rows than it has columns for; shrink it
    // until the kept part, ellipsis included, fits in `max_rows`.
    let mut budget = max_rows * width - 1;
    loop {
        let mut kept: Vec<char> = Vec::new();
        let mut used = 0usize;
        for &ch in &from_kept_end {
            let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
            if used + ch_width > budget {
                break;
            }
            kept.push(ch);
            used += ch_width;
        }
        kept.push(ELLIPSIS);
        if keep == Keep::End {
            kept.reverse();
        }
        let rows = chunk_columns(kept.into_iter(), width);
        if rows.len() <= max_rows || budget == 0 {
            return rows;
        }
        budget -= 1;
    }
}

fn chunk_columns(chars: impl Iterator<Item = char>, width: usize) -> Vec<String> {
    let mut rows: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut used = 0usize;
    for ch in chars {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + ch_width > width && !current.is_empty() {
            rows.push(std::mem::take(&mut current));
            used = 0;
        }
        current.push(ch);
        used += ch_width;
    }
    if !current.is_empty() {
        rows.push(current);
    }
    rows
}

fn wrapped_line_count(text: Text<'static>, width: u16) -> u16 {
    u16::try_from(Paragraph::new(text).wrap(Wrap { trim: false }).line_count(width))
        .unwrap_or(u16::MAX)
        .max(1)
}

fn action_lines(app: &App) -> Vec<Line<'static>> {
    vec![
        action_line("Yes", app.trust.selection == TrustSelection::Yes),
        action_line("No", app.trust.selection == TrustSelection::No),
    ]
}

fn action_line(label: &str, selected: bool) -> Line<'static> {
    let marker = if selected { ">" } else { " " };
    let style = if selected {
        Style::default().fg(ratatui::style::Color::White).bg(theme::RUST_ORANGE)
    } else {
        Style::default().fg(theme::DIM)
    }
    .add_modifier(Modifier::BOLD);

    Line::from(Span::styled(format!("{marker} {label}"), style))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn draw_rows(app: &mut App, width: u16, height: u16) -> Vec<String> {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal.draw(|frame| render(frame, app)).expect("draw");
        terminal
            .backend()
            .buffer()
            .content
            .chunks(usize::from(width))
            .map(|row| {
                let mut text = String::new();
                let mut skip = 0usize;
                for cell in row {
                    if skip > 0 {
                        skip -= 1;
                        continue;
                    }
                    text.push_str(cell.symbol());
                    skip = unicode_width::UnicodeWidthStr::width(cell.symbol()).saturating_sub(1);
                }
                text
            })
            .collect()
    }

    fn content(row: &str) -> String {
        let inside = row.trim_start_matches('\u{2502}').trim_end_matches('\u{2502}');
        inside.strip_prefix(' ').unwrap_or(inside).trim_end().to_owned()
    }

    fn inner_rows(app: &mut App, width: u16, height: u16) -> Vec<String> {
        let rows = draw_rows(app, width, height);
        rows[1..rows.len() - 1].iter().map(|row| content(row)).collect()
    }

    fn choices_at(rows: &[String]) -> usize {
        let yes = rows
            .iter()
            .position(|row| row == "> Yes" || row == "  Yes")
            .unwrap_or_else(|| panic!("no Yes row: {rows:#?}"));
        assert!(
            rows.get(yes + 1).is_some_and(|row| row == "  No" || row == "> No"),
            "No is not under Yes: {rows:#?}"
        );
        yes
    }

    const LONG_PATH: &str = "/Users/someone/Github/a-rather-long-organisation-name/client-projects/deeply-nested-service-checkout";
    const LONG_ERROR: &str = "Invalid trust preferences: /Users/someone/Library/Application Support/claude-profiles/a-rather-long-organisation-name/work/.claude.json";

    #[test]
    fn trusted_view_keeps_the_choices_on_very_small_screens() {
        for (width, height) in [(30u16, 12u16), (20, 14), (12, 24), (20, 5), (20, 4), (12, 4)] {
            for error in [None, Some(LONG_ERROR)] {
                let mut app = App::test_default();
                app.trust.project_key = LONG_PATH.to_owned();
                app.trust.last_error = error.map(str::to_owned);

                let rows = inner_rows(&mut app, width, height);

                choices_at(&rows);
            }
        }
    }
}
