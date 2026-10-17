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

/// Resumes at the next source line without re-parsing already visited lines.
pub(super) struct ReadCursor {
    highlighter: highlight::SourceHighlighter,
    next_byte: usize,
    next_line: usize,
    total_lines: usize,
    hidden: Range<usize>,
    entries: Vec<Entry>,
    width: u16,
    number_width: usize,
    gutter_width: usize,
    text_width: usize,
}

impl ReadCursor {
    pub(super) fn new(source: &ReadSource<'_>, width: u16) -> Self {
        let total_lines = source.text.lines().count();
        let number_width = source.start_line.map_or(0, |start| {
            start.saturating_add(total_lines.saturating_sub(1)).to_string().len()
        });
        let gutter_width = if source.start_line.is_some() { number_width + GUTTER_GAP } else { 0 };
        Self {
            highlighter: highlight::SourceHighlighter::new(
                source.path,
                source.language_fallback,
                SYNTAX_THEME,
            ),
            next_byte: 0,
            next_line: 0,
            total_lines,
            hidden: if total_lines > HEAD_LINES + TAIL_LINES + 1 {
                HEAD_LINES..total_lines - TAIL_LINES
            } else {
                0..0
            },
            entries: Vec::new(),
            width,
            number_width,
            gutter_width,
            text_width: usize::from(width).saturating_sub(gutter_width).max(1),
        }
    }

    pub(super) fn poll(
        &mut self,
        source: &ReadSource<'_>,
        control: &crate::ui::layout_control::LayoutControl<'_>,
    ) -> std::task::Poll<Vec<Line<'static>>> {
        while self.next_line < self.total_lines && self.width > 0 {
            if control.cancelled() {
                return std::task::Poll::Ready(Vec::new());
            }
            let remaining = &source.text[self.next_byte..];
            let bytes = remaining.find('\n').map_or(remaining.len(), |end| end + 1);
            let raw = &remaining[..bytes];
            let line =
                raw.strip_suffix('\n').map_or(raw, |line| line.strip_suffix('\r').unwrap_or(line));
            let mut display = display_source_line(line);
            display.push('\n');
            let visible = !self.hidden.contains(&self.next_line);
            if let Some(line) = self.highlighter.line(&display, visible) {
                self.entries.push(Entry::Source {
                    number: source.start_line.map(|start| start + self.next_line),
                    rows: split_line_at_width(line, self.text_width),
                });
            } else if self.next_line == self.hidden.start {
                self.entries.push(Entry::Hidden(self.hidden.len()));
            }
            self.next_byte += bytes;
            self.next_line += 1;
            if self.next_line < self.total_lines && control.yield_due() {
                return std::task::Poll::Pending;
            }
        }
        clip_to_height(&mut self.entries, TOOL_BODY_MAX_LINES, self.text_width);
        let gutter_style = Style::default().fg(theme::DIM);
        let mut out = Vec::new();
        for entry in self.entries.drain(..) {
            if control.cancelled() {
                return std::task::Poll::Ready(Vec::new());
            }
            match entry {
                Entry::Source { number, rows } => {
                    for (row_index, row) in rows.into_iter().enumerate() {
                        let mut spans = Vec::with_capacity(row.spans.len() + 1);
                        if let Some(number) = number {
                            let gutter = if row_index == 0 {
                                format!(
                                    "{number:>number_width$}{}",
                                    " ".repeat(GUTTER_GAP),
                                    number_width = self.number_width
                                )
                            } else {
                                " ".repeat(self.gutter_width)
                            };
                            spans.push(Span::styled(gutter, gutter_style));
                        }
                        spans.extend(row.spans);
                        out.push(Line::from(spans));
                    }
                }
                Entry::Hidden(count) => {
                    let full = format!("... {count} lines hidden ...");
                    let text = if full.len() <= usize::from(self.width) {
                        full
                    } else {
                        let compact = format!("… {count} hidden");
                        if crate::ui::wrap::display_width(&compact) <= usize::from(self.width) {
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
                        usize::from(self.width),
                    )));
                }
            }
        }
        std::task::Poll::Ready(out)
    }
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
