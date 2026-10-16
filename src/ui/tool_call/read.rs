// SPDX-License-Identifier: Apache-2.0

//! Read previews: file source with a line-number gutter. The preview limit applies
//! to source lines before wrapping; wrapped rows stay within the tool body height.

use crate::ui::highlight;
use crate::ui::terminal_output;
use crate::ui::theme;
use crate::ui::wrap::split_line_at_width;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use std::ops::Range;
use std::path::Path;
use two_face::theme::EmbeddedThemeName;
use unicode_segmentation::UnicodeSegmentation;

use super::{TOOL_BODY_MAX_LINES, truncate_spans_to_width};

const HEAD_LINES: usize = 3;
const TAIL_LINES: usize = 4;
const GUTTER_GAP: usize = 2;
const TAB_WIDTH: usize = 4;
const SYNTAX_THEME: EmbeddedThemeName = EmbeddedThemeName::MonokaiExtendedBright;

pub(super) struct ReadSource<'a> {
    pub text: &'a str,
    /// File line number of the first text line; `None` renders without a gutter.
    pub start_line: Option<usize>,
    pub path: Option<&'a Path>,
    pub language_fallback: Option<&'a str>,
}

enum Entry {
    Source { number: Option<usize>, rows: Vec<Line<'static>> },
    Hidden(usize),
}

pub(super) fn render_read_source(source: &ReadSource<'_>, width: u16) -> Vec<Line<'static>> {
    let lines = source.text.lines().collect::<Vec<_>>();
    if lines.is_empty() || width == 0 {
        return Vec::new();
    }
    let number_width = source
        .start_line
        .map_or(0, |start| start.saturating_add(lines.len() - 1).to_string().len());
    let gutter_width = if source.start_line.is_some() { number_width + GUTTER_GAP } else { 0 };
    let text_width = usize::from(width).saturating_sub(gutter_width).max(1);

    let hidden = if lines.len() > HEAD_LINES + TAIL_LINES + 1 {
        HEAD_LINES..lines.len() - TAIL_LINES
    } else {
        0..0
    };
    let mut entries = source_entries(source, &lines, hidden, text_width);
    clip_to_height(&mut entries, TOOL_BODY_MAX_LINES, text_width);

    let gutter_style = Style::default().fg(theme::DIM);
    let mut out = Vec::new();
    for entry in entries {
        match entry {
            Entry::Source { number, rows } => {
                for (row_index, row) in rows.into_iter().enumerate() {
                    let mut spans = Vec::with_capacity(row.spans.len() + 1);
                    if let Some(number) = number {
                        let gutter = if row_index == 0 {
                            format!("{number:>number_width$}{}", " ".repeat(GUTTER_GAP))
                        } else {
                            " ".repeat(gutter_width)
                        };
                        spans.push(Span::styled(gutter, gutter_style));
                    }
                    spans.extend(row.spans);
                    out.push(Line::from(spans));
                }
            }
            Entry::Hidden(count) => {
                let full = format!("... {count} lines hidden ...");
                let text = if full.len() <= usize::from(width) {
                    full
                } else {
                    let compact = format!("… {count} hidden");
                    if crate::ui::wrap::display_width(&compact) <= usize::from(width) {
                        compact
                    } else {
                        format!("… {count}")
                    }
                };
                out.push(Line::from(truncate_spans_to_width(
                    vec![Span::styled(
                        text,
                        Style::default().fg(theme::DIM).add_modifier(Modifier::ITALIC),
                    )],
                    usize::from(width),
                )));
            }
        }
    }
    out
}

/// Parse all source lines to retain multiline syntax state, but build spans and rows
/// only for the selected head and tail lines.
fn source_entries(
    source: &ReadSource<'_>,
    lines: &[&str],
    hidden: Range<usize>,
    text_width: usize,
) -> Vec<Entry> {
    let highlighted = highlight::highlight_source_lines(
        lines.iter().enumerate().map(|(index, line)| {
            let mut display = display_source_line(line);
            display.push('\n');
            (display, !hidden.contains(&index))
        }),
        source.path,
        source.language_fallback,
        SYNTAX_THEME,
    );
    let mut highlighted = highlighted.into_iter();
    lines
        .iter()
        .enumerate()
        .filter_map(|(index, _)| {
            if hidden.contains(&index) {
                return (index == hidden.start).then_some(Entry::Hidden(hidden.len()));
            }
            let line = highlighted.next().unwrap_or_default();
            Some(Entry::Source {
                number: source.start_line.map(|start| start + index),
                rows: split_line_at_width(line, text_width),
            })
        })
        .collect()
}

/// Escape sequences are not source text and tabs advance to the next tab stop.
fn display_source_line(line: &str) -> String {
    let line = terminal_output::strip_ansi(line);
    if !line.contains('\t') {
        return line;
    }
    let mut out = String::with_capacity(line.len());
    let mut column = 0;
    for grapheme in line.graphemes(true) {
        if grapheme == "\t" {
            let advance = TAB_WIDTH - column % TAB_WIDTH;
            out.extend(std::iter::repeat_n(' ', advance));
            column += advance;
        } else {
            out.push_str(grapheme);
            column += crate::ui::wrap::display_width(grapheme);
        }
    }
    out
}

/// Limit every source line to the same number of rows so the preview fits `max_rows`.
/// A clipped line ends in an ellipsis on its last visible row.
fn clip_to_height(entries: &mut [Entry], max_rows: usize, text_width: usize) {
    let rows_with_limit = |limit: usize| -> usize {
        entries
            .iter()
            .map(|entry| match entry {
                Entry::Source { rows, .. } => rows.len().min(limit),
                Entry::Hidden(_) => 1,
            })
            .sum()
    };
    let mut limit = max_rows;
    while limit > 1 && rows_with_limit(limit) > max_rows {
        limit -= 1;
    }
    for entry in entries {
        let Entry::Source { rows, .. } = entry else { continue };
        if rows.len() <= limit {
            continue;
        }
        let mut overflow = rows.drain(limit - 1..);
        let mut spans = overflow.next().map(|row| row.spans).unwrap_or_default();
        spans.extend(overflow.next().map(|row| row.spans).unwrap_or_default());
        drop(overflow);
        rows.push(Line::from(truncate_spans_to_width(spans, text_width)));
    }
}
