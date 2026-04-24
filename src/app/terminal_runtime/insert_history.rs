// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use std::io;
use std::io::Write;

use crossterm::cursor::MoveTo;
use crossterm::queue;
use crossterm::style::Colors;
use crossterm::style::Print;
use crossterm::style::SetAttribute;
use crossterm::style::SetBackgroundColor;
use crossterm::style::SetColors;
use crossterm::style::SetForegroundColor;
use crossterm::terminal::Clear;
use crossterm::terminal::ClearType;
use ratatui::backend::Backend;
use ratatui::style::{Color, Modifier};
use ratatui::text::{Line, Span};

use super::custom_inline_terminal::Terminal;
use super::screen_scroll::{ScreenScrollRequest, scroll_screen};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HistoryInsertOutcome {
    pub(crate) insert_top: u16,
    pub(crate) inserted_rows: u16,
    pub(crate) scroll_up_amount: u16,
}

pub(crate) fn insert_history_lines<B>(
    terminal: &mut Terminal<B>,
    lines: &[Line<'static>],
) -> io::Result<HistoryInsertOutcome>
where
    B: Backend<Error = io::Error> + Write,
{
    if lines.is_empty() {
        return Ok(HistoryInsertOutcome { insert_top: 0, inserted_rows: 0, scroll_up_amount: 0 });
    }

    let screen_height = terminal.size()?.height;
    let mut planner = HistoryInsertPlanner::new(
        terminal.viewport_area.top(),
        screen_height,
        terminal.history_bottom_exclusive(),
    );
    let inserted_rows = u16::try_from(lines.len()).unwrap_or(u16::MAX);
    let mut first_insert_top = None;
    let mut total_scrolled = 0u16;
    let mut next_line_idx = 0usize;

    while next_line_idx < lines.len() {
        let segment = planner.next_segment(lines.len().saturating_sub(next_line_idx));
        if segment.scroll_up_amount > 0 {
            let outcome =
                scroll_screen(terminal, ScreenScrollRequest::up(segment.scroll_up_amount))?;
            total_scrolled = total_scrolled.saturating_add(outcome.applied_rows);
        }

        let segment_end = next_line_idx.saturating_add(usize::from(segment.segment_rows));
        write_history_segment(terminal, segment.insert_top, &lines[next_line_idx..segment_end])?;
        terminal.record_history_insert(segment.insert_top, segment.segment_rows);
        terminal.invalidate_viewport();

        first_insert_top.get_or_insert(segment.insert_top);
        next_line_idx = segment_end;
    }

    Ok(HistoryInsertOutcome {
        insert_top: first_insert_top.unwrap_or(0),
        inserted_rows,
        scroll_up_amount: total_scrolled,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HistoryInsertSegment {
    insert_top: u16,
    segment_rows: u16,
    scroll_up_amount: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HistoryInsertPlanner {
    viewport_top: u16,
    screen_height: u16,
    history_bottom_exclusive: Option<u16>,
}

impl HistoryInsertPlanner {
    const fn new(
        viewport_top: u16,
        screen_height: u16,
        history_bottom_exclusive: Option<u16>,
    ) -> Self {
        Self { viewport_top, screen_height, history_bottom_exclusive }
    }

    fn next_segment(&mut self, remaining_rows: usize) -> HistoryInsertSegment {
        let max_segment_rows = self.max_segment_rows();
        let remaining_rows = u16::try_from(remaining_rows).unwrap_or(u16::MAX);
        let segment_rows = remaining_rows.min(max_segment_rows);

        match self.history_bottom_exclusive {
            Some(history_bottom) => {
                let available_gap = self.viewport_top.saturating_sub(history_bottom);
                if available_gap > 0 {
                    let direct_rows = segment_rows.min(available_gap);
                    let segment = HistoryInsertSegment {
                        insert_top: history_bottom,
                        segment_rows: direct_rows,
                        scroll_up_amount: 0,
                    };
                    self.history_bottom_exclusive =
                        Some(history_bottom.saturating_add(direct_rows).min(self.viewport_top));
                    segment
                } else {
                    let actual_scroll = segment_rows.min(self.screen_height);
                    let segment = HistoryInsertSegment {
                        insert_top: self.viewport_top.saturating_sub(actual_scroll),
                        segment_rows,
                        scroll_up_amount: actual_scroll,
                    };
                    self.history_bottom_exclusive = Some(self.viewport_top);
                    segment
                }
            }
            None => {
                if self.viewport_top > 0 {
                    let direct_rows = segment_rows.min(self.viewport_top);
                    let segment = HistoryInsertSegment {
                        insert_top: self.viewport_top.saturating_sub(direct_rows),
                        segment_rows: direct_rows,
                        scroll_up_amount: 0,
                    };
                    self.history_bottom_exclusive = Some(self.viewport_top);
                    segment
                } else {
                    HistoryInsertSegment { insert_top: 0, segment_rows, scroll_up_amount: 0 }
                }
            }
        }
    }

    fn max_segment_rows(self) -> u16 {
        if self.viewport_top > 0 {
            self.viewport_top.min(self.screen_height.max(1))
        } else {
            self.screen_height.max(1)
        }
    }
}

fn write_history_segment<B>(
    terminal: &mut Terminal<B>,
    insert_top: u16,
    lines: &[Line<'static>],
) -> io::Result<()>
where
    B: Backend<Error = io::Error> + Write,
{
    if lines.is_empty() {
        return Ok(());
    }

    let cursor_pos = terminal.last_known_cursor_pos;
    {
        let writer = terminal.backend_mut();
        queue!(writer, MoveTo(0, insert_top))?;
        for (idx, line) in lines.iter().enumerate() {
            if idx > 0 {
                queue!(writer, Print("\r\n"))?;
            }
            write_history_line(writer, line)?;
        }
        queue!(writer, MoveTo(cursor_pos.x, cursor_pos.y))?;
    }

    Ok(())
}

fn write_history_line<W: Write>(writer: &mut W, line: &Line<'static>) -> io::Result<()> {
    queue!(
        writer,
        SetColors(Colors::new(
            line.style.fg.map_or(crossterm::style::Color::Reset, to_crossterm_color),
            line.style.bg.map_or(crossterm::style::Color::Reset, to_crossterm_color),
        )),
        Clear(ClearType::UntilNewLine)
    )?;

    let merged_spans = line
        .spans
        .iter()
        .map(|span| Span { style: span.style.patch(line.style), content: span.content.clone() })
        .collect::<Vec<_>>();
    write_spans(writer, merged_spans.iter())?;
    queue!(
        writer,
        SetForegroundColor(crossterm::style::Color::Reset),
        SetBackgroundColor(crossterm::style::Color::Reset),
        SetAttribute(crossterm::style::Attribute::Reset)
    )?;
    Ok(())
}

fn write_spans<'a, W>(writer: &mut W, spans: impl Iterator<Item = &'a Span<'a>>) -> io::Result<()>
where
    W: Write,
{
    for span in spans {
        if let Some(color) = span.style.fg {
            queue!(writer, SetForegroundColor(to_crossterm_color(color)))?;
        }
        if let Some(color) = span.style.bg {
            queue!(writer, SetBackgroundColor(to_crossterm_color(color)))?;
        }
        if span.style.add_modifier.contains(Modifier::BOLD) {
            queue!(writer, SetAttribute(crossterm::style::Attribute::Bold))?;
        }
        if span.style.add_modifier.contains(Modifier::ITALIC) {
            queue!(writer, SetAttribute(crossterm::style::Attribute::Italic))?;
        }
        if span.style.add_modifier.contains(Modifier::UNDERLINED) {
            queue!(writer, SetAttribute(crossterm::style::Attribute::Underlined))?;
        }
        if span.style.add_modifier.contains(Modifier::REVERSED) {
            queue!(writer, SetAttribute(crossterm::style::Attribute::Reverse))?;
        }
        if span.style.add_modifier.contains(Modifier::DIM) {
            queue!(writer, SetAttribute(crossterm::style::Attribute::Dim))?;
        }
        if span.style.add_modifier.contains(Modifier::CROSSED_OUT) {
            queue!(writer, SetAttribute(crossterm::style::Attribute::CrossedOut))?;
        }
        queue!(writer, Print(span.content.as_ref()))?;
        if span.style.fg.is_some() || span.style.bg.is_some() || !span.style.add_modifier.is_empty()
        {
            queue!(
                writer,
                SetForegroundColor(crossterm::style::Color::Reset),
                SetBackgroundColor(crossterm::style::Color::Reset),
                SetAttribute(crossterm::style::Attribute::Reset)
            )?;
        }
    }
    Ok(())
}

fn to_crossterm_color(color: Color) -> crossterm::style::Color {
    match color {
        Color::Reset => crossterm::style::Color::Reset,
        Color::Black => crossterm::style::Color::Black,
        Color::Red => crossterm::style::Color::DarkRed,
        Color::Green => crossterm::style::Color::DarkGreen,
        Color::Yellow => crossterm::style::Color::DarkYellow,
        Color::Blue => crossterm::style::Color::DarkBlue,
        Color::Magenta => crossterm::style::Color::DarkMagenta,
        Color::Cyan => crossterm::style::Color::DarkCyan,
        Color::Gray => crossterm::style::Color::Grey,
        Color::DarkGray => crossterm::style::Color::DarkGrey,
        Color::LightRed => crossterm::style::Color::Red,
        Color::LightGreen => crossterm::style::Color::Green,
        Color::LightYellow => crossterm::style::Color::Yellow,
        Color::LightBlue => crossterm::style::Color::Blue,
        Color::LightMagenta => crossterm::style::Color::Magenta,
        Color::LightCyan => crossterm::style::Color::Cyan,
        Color::White => crossterm::style::Color::White,
        Color::Indexed(idx) => crossterm::style::Color::AnsiValue(idx),
        Color::Rgb(r, g, b) => crossterm::style::Color::Rgb { r, g, b },
    }
}

#[cfg(test)]
mod tests {
    use super::{HistoryInsertPlanner, HistoryInsertSegment};

    #[test]
    fn existing_history_uses_freed_gap_without_scroll() {
        let mut planner = HistoryInsertPlanner::new(44, 50, Some(42));

        assert_eq!(
            planner.next_segment(2),
            HistoryInsertSegment { insert_top: 42, segment_rows: 2, scroll_up_amount: 0 }
        );
    }

    #[test]
    fn existing_history_appends_in_viewport_sized_scroll_chunks() {
        let mut planner = HistoryInsertPlanner::new(37, 40, Some(37));

        assert_eq!(
            planner.next_segment(467),
            HistoryInsertSegment { insert_top: 0, segment_rows: 37, scroll_up_amount: 37 }
        );
        assert_eq!(
            planner.next_segment(430),
            HistoryInsertSegment { insert_top: 0, segment_rows: 37, scroll_up_amount: 37 }
        );
    }

    #[test]
    fn first_history_insert_bottom_stacks_small_batch_without_scroll() {
        let mut planner = HistoryInsertPlanner::new(44, 50, None);

        assert_eq!(
            planner.next_segment(2),
            HistoryInsertSegment { insert_top: 42, segment_rows: 2, scroll_up_amount: 0 }
        );
    }

    #[test]
    fn first_large_insert_seeds_visible_history_before_scrolling() {
        let mut planner = HistoryInsertPlanner::new(37, 40, None);

        assert_eq!(
            planner.next_segment(467),
            HistoryInsertSegment { insert_top: 0, segment_rows: 37, scroll_up_amount: 0 }
        );
        assert_eq!(
            planner.next_segment(430),
            HistoryInsertSegment { insert_top: 0, segment_rows: 37, scroll_up_amount: 37 }
        );
    }
}
