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
    let message_height = wrapped_line_count(Text::from(message.clone()), width);

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
    // An error whose wrapped rows do not all fit is laid out by columns instead,
    // ending in an ellipsis if it is still cut, so a cut shows instead of the
    // text stopping mid-word.
    let message = match &app.trust.last_error {
        Some(error) if message_rows < message_height => Paragraph::new(
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
    use crate::app::App;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// The label the review asked for in front of the path. Rows under it
    /// that continue the path are indented by its width.
    const LABEL: &str = "Path: ";

    /// The screen rows as text. The cell after a double-width character is
    /// skipped, so a row reads as the characters that were drawn.
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

    fn draw_text(app: &mut App) -> String {
        draw_rows(app, 70, 14).join("\n")
    }

    /// A row without the frame and its padding (the margin is two columns, one of
    /// them the frame's own).
    fn content(row: &str) -> String {
        let inside = row.trim_start_matches('\u{2502}').trim_end_matches('\u{2502}');
        inside.strip_prefix(' ').unwrap_or(inside).trim_end().to_owned()
    }

    /// The rows inside the frame, without the frame and its padding.
    fn inner_rows(app: &mut App, width: u16, height: u16) -> Vec<String> {
        let rows = draw_rows(app, width, height);
        rows[1..rows.len() - 1].iter().map(|row| content(row)).collect()
    }

    /// The path region: the index of the labelled row, and the path text of
    /// that row and of each row indented under it.
    fn path_region(rows: &[String]) -> Option<(usize, Vec<String>)> {
        let first = rows.iter().position(|row| row.starts_with(LABEL))?;
        let indent = " ".repeat(LABEL.len());
        let mut parts = vec![rows[first][LABEL.len()..].to_owned()];
        parts.extend(rows[first + 1..].iter().map_while(|row| {
            row.strip_prefix(indent.as_str())
                .filter(|rest| !rest.trim().is_empty())
                .map(str::to_owned)
        }));
        Some((first, parts))
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
    const CJK_PATH: &str =
        "/Users/田中/Github/プロジェクト一覧/顧客向け-サービス/チェックアウト画面v2";
    const SHORT_ERROR: &str = "Invalid trust preferences: /Users/someone/.claude.json";
    const LONG_ERROR: &str = "Invalid trust preferences: /Users/someone/Library/Application Support/claude-profiles/a-rather-long-organisation-name/work/.claude.json";

    #[test]
    fn trusted_view_explains_the_prompt_before_the_labelled_path() {
        let mut app = App::test_default();
        app.trust.selection = TrustSelection::Yes;
        app.trust.project_key = "/work/project".to_owned();

        let rows = inner_rows(&mut app, 100, 16);

        // The layout from the review: the question and the first explanation
        // line, the path between blank rows, the rest, then the choices.
        let expected = [
            "Trust this project directory?",
            "Claude Rust will wait here",
            "",
            "Path: /work/project",
            "",
            "Review the project",
            "Choose Yes",
            "> Yes",
            "  No",
        ];
        let shape: Vec<&str> = rows
            .iter()
            .zip(expected)
            .map(|(row, want)| {
                if row.starts_with(want) && row.is_empty() == want.is_empty() { want } else { row }
            })
            .collect();
        assert_eq!(shape, expected, "{rows:#?}");
        assert_eq!(rows[3], "Path: /work/project", "{rows:#?}");
        let text = rows.join("\n");
        assert!(!text.contains("~/.claude.json"), "{text}");
        assert!(!text.contains("hasTrustDialogAccepted"), "{text}");
    }

    #[test]
    fn trusted_view_wraps_a_long_directory_without_losing_characters() {
        // Width 40 leaves 30 columns beside the label and puts no row break on
        // a space, which a row's padding would hide.
        for key in [LONG_PATH, "C:/demo/Client Projects/some service/checkout"] {
            let mut app = App::test_default();
            app.trust.project_key = key.to_owned();

            let rows = inner_rows(&mut app, 40, 20);

            let (_, path) = path_region(&rows).unwrap_or_else(|| panic!("{rows:#?}"));
            assert!(path.len() > 1, "{rows:#?}");
            assert_eq!(path.concat(), key, "{rows:#?}");
            choices_at(&rows);
        }
    }

    #[test]
    fn trusted_view_keeps_the_end_of_a_path_behind_a_leading_ellipsis() {
        let mut app = App::test_default();
        app.trust.project_key = LONG_PATH.to_owned();

        // Five rows inside: the choices and three of the four the path needs.
        let rows = inner_rows(&mut app, 40, 7);

        let (first, path) = path_region(&rows).unwrap_or_else(|| panic!("{rows:#?}"));
        assert!(path.len() > 1, "{rows:#?}");
        assert!(path[0].starts_with('\u{2026}'), "{rows:#?}");
        let tail = path.concat();
        let tail = tail.trim_start_matches('\u{2026}');
        assert!(LONG_PATH.ends_with(tail), "{rows:#?}");
        assert!(tail.ends_with("service-checkout"), "{rows:#?}");
        assert_eq!(choices_at(&rows), first + path.len(), "{rows:#?}");
    }

    #[test]
    fn trusted_view_keeps_the_path_in_the_last_row_left_after_the_choices() {
        // Three rows inside: the choices take two, and the path, not the
        // question, takes the last one.
        for width in [20u16, 30, 44] {
            let mut app = App::test_default();
            app.trust.project_key = LONG_PATH.to_owned();

            let rows = inner_rows(&mut app, width, 5);

            let (first, path) = path_region(&rows).unwrap_or_else(|| panic!("{width}: {rows:#?}"));
            assert_eq!(path.len(), 1, "{width}: {rows:#?}");
            let tail = path[0]
                .strip_prefix('\u{2026}')
                .unwrap_or_else(|| panic!("no leading ellipsis at {width}: {rows:#?}"));
            assert!(tail.len() >= 8 && LONG_PATH.ends_with(tail), "{width}: {rows:#?}");
            assert_eq!(choices_at(&rows), first + 1, "{width}: {rows:#?}");
        }
    }

    #[test]
    fn trusted_view_gives_the_error_its_rows_before_the_path_on_a_small_screen() {
        // At width 40 the short error wraps to two rows and the long one to
        // five; the choices take two of the rows inside the frame. The error is
        // shown whole and the path gets only the rows left after it.
        for (error, height, path_shown) in
            [(SHORT_ERROR, 8u16, true), (LONG_ERROR, 9, false), (LONG_ERROR, 10, true)]
        {
            let mut app = App::test_default();
            app.trust.project_key = LONG_PATH.to_owned();
            app.trust.last_error = Some(error.to_owned());

            let rows = inner_rows(&mut app, 40, height);

            let choices = choices_at(&rows);
            let region = path_region(&rows);
            assert_eq!(region.is_some(), path_shown, "{height}: {rows:#?}");
            let error_start = region.as_ref().map_or(0, |(first, path)| first + path.len());
            let squash = |text: &str| text.split_whitespace().collect::<String>();
            assert_eq!(
                squash(&rows[error_start..choices].concat()),
                squash(error),
                "the whole error, between the path and the choices: {rows:#?}"
            );
            if let Some((_, path)) = region {
                let shown = path.concat();
                let tail = shown.strip_prefix('\u{2026}').unwrap_or_else(|| panic!("{rows:#?}"));
                assert!(!tail.is_empty() && LONG_PATH.ends_with(tail), "{rows:#?}");
            }
            assert!(
                !rows.iter().any(|row| row.starts_with("Trust this")
                    || row.starts_with("Claude Rust will wait")
                    || row.starts_with("Review the project")),
                "the question and explanation should give way first: {rows:#?}"
            );
        }
    }

    #[test]
    fn trusted_view_gives_the_last_row_to_the_error_and_marks_its_cut() {
        // Three rows inside: the choices take two, and the error, not the
        // path, takes the last one.
        let mut app = App::test_default();
        app.trust.project_key = LONG_PATH.to_owned();
        app.trust.last_error = Some(LONG_ERROR.to_owned());

        let rows = inner_rows(&mut app, 40, 5);

        assert!(path_region(&rows).is_none(), "{rows:#?}");
        assert_eq!(choices_at(&rows), 1, "{rows:#?}");
        let shown = rows[0]
            .strip_suffix('\u{2026}')
            .unwrap_or_else(|| panic!("the cut error has no trailing ellipsis: {rows:#?}"));
        assert!(
            shown.starts_with("Invalid trust preferences:") && LONG_ERROR.starts_with(shown),
            "{rows:#?}"
        );
    }

    #[test]
    fn trusted_view_shows_an_error_in_place_of_the_hint() {
        let mut app = App::test_default();
        app.trust.project_key = "/work/project".to_owned();
        app.trust.last_error = Some(SHORT_ERROR.to_owned());

        let rows = inner_rows(&mut app, 100, 16);

        let choices = choices_at(&rows);
        assert_eq!(rows[choices - 1], SHORT_ERROR, "{rows:#?}");
        assert!(rows[choices - 2].starts_with("Review the project"), "{rows:#?}");
        assert!(!rows.iter().any(|row| row.starts_with("Choose Yes")), "{rows:#?}");
    }

    #[test]
    fn trusted_view_spaces_the_path_on_both_sides_or_neither() {
        for height in 6u16..=16 {
            let mut app = App::test_default();
            app.trust.project_key = "/work/project".to_owned();

            let rows = inner_rows(&mut app, 60, height);

            let (first, path) = path_region(&rows).unwrap_or_else(|| panic!("{height}: {rows:#?}"));
            let blank_above = first > 0 && rows[first - 1].is_empty();
            let blank_below = rows[first + path.len()].is_empty();
            assert_eq!(blank_above, blank_below, "{height}: {rows:#?}");
        }
    }

    #[test]
    fn trusted_view_keeps_the_end_of_a_wide_character_path_at_odd_widths() {
        // A double-width character that does not fit leaves a column unused at
        // the end of a row, so a budget counted in columns can need a third
        // row. The label leaves an odd number of columns at these widths.
        for inner_width in [31u16, 33, 35, 37] {
            let mut app = App::test_default();
            app.trust.project_key = CJK_PATH.to_owned();

            let rows = inner_rows(&mut app, inner_width + 4, 6);

            let (first, path) =
                path_region(&rows).unwrap_or_else(|| panic!("{inner_width}: {rows:#?}"));
            assert!(path.len() <= 2, "{inner_width}: {rows:#?}");
            assert!(path[0].starts_with('\u{2026}'), "{inner_width}: {rows:#?}");
            let shown = path.concat();
            assert!(shown.ends_with("画面v2"), "{inner_width}: {rows:#?}");
            assert!(CJK_PATH.ends_with(shown.trim_start_matches('\u{2026}')), "{rows:#?}");
            assert_eq!(choices_at(&rows), first + path.len(), "{inner_width}: {rows:#?}");
        }
    }

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

    #[test]
    fn trusted_view_does_not_mark_a_path_that_fits() {
        let mut app = App::test_default();
        app.trust.project_key = "/work/project".to_owned();

        let rows = inner_rows(&mut app, 70, 14);

        let (_, path) = path_region(&rows).unwrap_or_else(|| panic!("{rows:#?}"));
        assert_eq!(path, ["/work/project"], "{rows:#?}");
    }

    #[test]
    fn trusted_view_highlights_no_when_selected() {
        let mut app = App::test_default();
        app.trust.selection = TrustSelection::No;

        let text = draw_text(&mut app);

        assert!(text.contains("  Yes"));
        assert!(text.contains("> No"));
    }

    #[test]
    fn trusted_view_renders_actions_below_body_text() {
        let mut app = App::test_default();
        app.trust.selection = TrustSelection::Yes;

        let text = draw_text(&mut app);
        let body_idx = text.find("Claude Rust will wait here").expect("body text");
        let action_idx = text.find("> Yes").expect("yes action");

        assert!(action_idx > body_idx);
    }

    #[test]
    fn trusted_view_wraps_title_on_narrow_widths() {
        let mut app = App::test_default();
        let rows = draw_rows(&mut app, 18, 16);

        assert!(rows.iter().any(|row| row.contains("Trust this")));
        assert!(rows.iter().any(|row| row.contains("directory?")));
    }
}
