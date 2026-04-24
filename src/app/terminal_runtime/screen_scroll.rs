// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use std::io;
use std::io::Write;

use crossterm::cursor::MoveTo;
use crossterm::queue;
use crossterm::style::Print;
use crossterm::terminal::ScrollDown;
use ratatui::backend::Backend;
use ratatui::layout::Position;

use super::custom_inline_terminal::Terminal;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScreenScrollDirection {
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScreenScrollRequest {
    pub(crate) direction: ScreenScrollDirection,
    pub(crate) rows: u16,
}

impl ScreenScrollRequest {
    pub(crate) const fn up(rows: u16) -> Self {
        Self { direction: ScreenScrollDirection::Up, rows }
    }

    pub(crate) const fn down(rows: u16) -> Self {
        Self { direction: ScreenScrollDirection::Down, rows }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScreenScrollOutcome {
    pub(crate) requested_rows: u16,
    pub(crate) applied_rows: u16,
}

pub(crate) fn scroll_screen<B>(
    terminal: &mut Terminal<B>,
    request: ScreenScrollRequest,
) -> io::Result<ScreenScrollOutcome>
where
    B: Backend<Error = io::Error> + Write,
{
    let screen_height = terminal.size()?.height;
    let applied_rows = clamp_scroll_rows(request.rows, screen_height);
    if applied_rows == 0 {
        return Ok(ScreenScrollOutcome { requested_rows: request.rows, applied_rows });
    }

    let cursor_pos = scroll_adjusted_cursor_position(
        terminal.last_known_cursor_pos,
        request.direction,
        applied_rows,
        screen_height,
    );
    {
        let writer = terminal.backend_mut();
        match request.direction {
            ScreenScrollDirection::Up => {
                queue!(writer, MoveTo(0, screen_height.saturating_sub(1)))?;
                for _ in 0..applied_rows {
                    queue!(writer, Print("\n"))?;
                }
            }
            ScreenScrollDirection::Down => queue!(writer, ScrollDown(applied_rows))?,
        }
        queue!(writer, MoveTo(cursor_pos.x, cursor_pos.y))?;
    }

    terminal.apply_screen_scroll_bookkeeping(request.direction, applied_rows, cursor_pos);
    tracing::debug!(
        target: crate::logging::targets::APP_RENDER,
        event_name = "inline_terminal_screen_scroll",
        message = "inline terminal screen scrolled",
        outcome = "success",
        direction = ?request.direction,
        requested_rows = request.rows,
        applied_rows,
        cursor_row = cursor_pos.y,
    );

    Ok(ScreenScrollOutcome { requested_rows: request.rows, applied_rows })
}

fn clamp_scroll_rows(rows: u16, screen_height: u16) -> u16 {
    if screen_height == 0 { 0 } else { rows.min(screen_height) }
}

fn scroll_adjusted_cursor_position(
    cursor_pos: Position,
    direction: ScreenScrollDirection,
    rows: u16,
    screen_height: u16,
) -> Position {
    let max_y = screen_height.saturating_sub(1);
    let y = match direction {
        ScreenScrollDirection::Up => cursor_pos.y.saturating_sub(rows),
        ScreenScrollDirection::Down => cursor_pos.y.saturating_add(rows).min(max_y),
    };
    Position { x: cursor_pos.x, y }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::io::Write;

    use ratatui::backend::{Backend, ClearType, WindowSize};
    use ratatui::buffer::Cell;
    use ratatui::layout::{Position, Rect, Size};

    use super::{
        ScreenScrollDirection, ScreenScrollRequest, clamp_scroll_rows,
        scroll_adjusted_cursor_position, scroll_screen,
    };
    use crate::app::terminal_runtime::custom_inline_terminal::Terminal;

    #[test]
    fn clamp_scroll_rows_limits_to_screen_height() {
        assert_eq!(clamp_scroll_rows(8, 3), 3);
        assert_eq!(clamp_scroll_rows(3, 8), 3);
        assert_eq!(clamp_scroll_rows(3, 0), 0);
    }

    #[test]
    fn adjusted_cursor_tracks_screen_content() {
        assert_eq!(
            scroll_adjusted_cursor_position(Position::new(4, 10), ScreenScrollDirection::Up, 3, 20,),
            Position::new(4, 7)
        );
        assert_eq!(
            scroll_adjusted_cursor_position(
                Position::new(4, 18),
                ScreenScrollDirection::Down,
                7,
                20,
            ),
            Position::new(4, 19)
        );
    }

    #[test]
    fn scroll_up_writes_command_and_updates_bookkeeping() {
        let mut terminal = Terminal::with_options(RecordingBackend::new(80, 24)).unwrap();
        terminal.set_viewport_area(Rect::new(0, 10, 80, 14));
        terminal.record_history_insert(4, 6);
        terminal.last_known_cursor_pos = Position::new(5, 9);

        let outcome = scroll_screen(&mut terminal, ScreenScrollRequest::up(3)).unwrap();

        assert_eq!(outcome.applied_rows, 3);
        assert_eq!(terminal.last_known_cursor_pos, Position::new(5, 6));
        assert_eq!(terminal.history_bounds(), Some((1, 7)));
        let written = String::from_utf8_lossy(&terminal.backend_mut().written);
        assert_eq!(written.matches('\n').count(), 3);
    }

    #[test]
    fn scroll_down_writes_command_and_updates_bookkeeping() {
        let mut terminal = Terminal::with_options(RecordingBackend::new(80, 24)).unwrap();
        terminal.set_viewport_area(Rect::new(0, 10, 80, 14));
        terminal.record_history_insert(1, 5);
        terminal.last_known_cursor_pos = Position::new(5, 8);

        let outcome = scroll_screen(&mut terminal, ScreenScrollRequest::down(4)).unwrap();

        assert_eq!(outcome.applied_rows, 4);
        assert_eq!(terminal.last_known_cursor_pos, Position::new(5, 12));
        assert_eq!(terminal.history_bounds(), Some((5, 10)));
        assert!(terminal.backend_mut().written.contains(&b'T'));
    }

    #[derive(Debug)]
    struct RecordingBackend {
        size: Size,
        cursor: Position,
        written: Vec<u8>,
    }

    impl RecordingBackend {
        const fn new(width: u16, height: u16) -> Self {
            Self { size: Size::new(width, height), cursor: Position::ORIGIN, written: Vec::new() }
        }
    }

    impl Write for RecordingBackend {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.written.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Backend for RecordingBackend {
        type Error = io::Error;

        fn draw<'a, I>(&mut self, _content: I) -> io::Result<()>
        where
            I: Iterator<Item = (u16, u16, &'a Cell)>,
        {
            Ok(())
        }

        fn hide_cursor(&mut self) -> io::Result<()> {
            Ok(())
        }

        fn show_cursor(&mut self) -> io::Result<()> {
            Ok(())
        }

        fn get_cursor_position(&mut self) -> io::Result<Position> {
            Ok(self.cursor)
        }

        fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
            self.cursor = position.into();
            Ok(())
        }

        fn clear(&mut self) -> io::Result<()> {
            Ok(())
        }

        fn clear_region(&mut self, _clear_type: ClearType) -> io::Result<()> {
            Ok(())
        }

        fn size(&self) -> io::Result<Size> {
            Ok(self.size)
        }

        fn window_size(&mut self) -> io::Result<WindowSize> {
            Ok(WindowSize { columns_rows: self.size, pixels: self.size })
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }

        fn scroll_region_up(
            &mut self,
            _region: std::ops::Range<u16>,
            _line_count: u16,
        ) -> io::Result<()> {
            Ok(())
        }

        fn scroll_region_down(
            &mut self,
            _region: std::ops::Range<u16>,
            _line_count: u16,
        ) -> io::Result<()> {
            Ok(())
        }
    }
}
