// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use ratatui::backend::{Backend, ClearType, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};

pub(super) struct TrackedCursorBackend<B> {
    inner: B,
    tracked_cursor: Position,
}

impl<B> TrackedCursorBackend<B> {
    pub(super) fn new(inner: B, seed: Position) -> Self {
        Self { inner, tracked_cursor: seed }
    }

    #[cfg(test)]
    fn tracked_cursor(&self) -> Position {
        self.tracked_cursor
    }
}

impl<B: Backend> Backend for TrackedCursorBackend<B> {
    type Error = B::Error;

    fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        let mut last_pos = None;
        self.inner.draw(content.inspect(|(x, y, _)| {
            last_pos = Some(Position { x: *x, y: *y });
        }))?;
        if let Some(position) = last_pos {
            self.tracked_cursor = position;
        }
        Ok(())
    }

    fn append_lines(&mut self, n: u16) -> Result<(), Self::Error> {
        self.inner.append_lines(n)?;
        let max_y = self.inner.size().ok().map_or(u16::MAX, |size| size.height.saturating_sub(1));
        self.tracked_cursor.y = self.tracked_cursor.y.saturating_add(n).min(max_y);
        Ok(())
    }

    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        self.inner.hide_cursor()
    }

    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        self.inner.show_cursor()
    }

    fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
        Ok(self.tracked_cursor)
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> Result<(), Self::Error> {
        let position = position.into();
        self.inner.set_cursor_position(position)?;
        self.tracked_cursor = position;
        Ok(())
    }

    fn clear(&mut self) -> Result<(), Self::Error> {
        self.inner.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> Result<(), Self::Error> {
        self.inner.clear_region(clear_type)
    }

    fn size(&self) -> Result<Size, Self::Error> {
        self.inner.size()
    }

    fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
        self.inner.window_size()
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        self.inner.flush()
    }

    fn scroll_region_up(
        &mut self,
        region: core::ops::Range<u16>,
        line_count: u16,
    ) -> Result<(), Self::Error> {
        self.inner.scroll_region_up(region, line_count)
    }

    fn scroll_region_down(
        &mut self,
        region: core::ops::Range<u16>,
        line_count: u16,
    ) -> Result<(), Self::Error> {
        self.inner.scroll_region_down(region, line_count)
    }
}

#[cfg(test)]
mod tests {
    use super::TrackedCursorBackend;
    use ratatui::backend::{Backend, ClearType, WindowSize};
    use ratatui::buffer::Cell;
    use ratatui::layout::{Position, Size};
    use ratatui::text::Line;
    use ratatui::widgets::{Paragraph, Widget};
    use ratatui::{Terminal, TerminalOptions, Viewport};
    use std::cell::Cell as CounterCell;
    use std::convert::Infallible;
    use std::ops::Range;
    use std::rc::Rc;

    #[derive(Clone, Default)]
    struct BackendCounters {
        cursor_queries: Rc<CounterCell<usize>>,
        clears: Rc<CounterCell<usize>>,
        scrolls: Rc<CounterCell<usize>>,
    }

    struct FakeBackend {
        size: Size,
        cursor: Position,
        counters: BackendCounters,
    }

    impl FakeBackend {
        fn new(width: u16, height: u16, cursor: Position) -> Self {
            Self { size: Size::new(width, height), cursor, counters: BackendCounters::default() }
        }

        fn with_counters(
            width: u16,
            height: u16,
            cursor: Position,
            counters: BackendCounters,
        ) -> Self {
            Self { size: Size::new(width, height), cursor, counters }
        }
    }

    impl Backend for FakeBackend {
        type Error = Infallible;

        fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
        where
            I: Iterator<Item = (u16, u16, &'a Cell)>,
        {
            for (x, y, _) in content {
                self.cursor = Position { x, y };
            }
            Ok(())
        }

        fn append_lines(&mut self, n: u16) -> Result<(), Self::Error> {
            self.cursor.y = self.cursor.y.saturating_add(n).min(self.size.height.saturating_sub(1));
            Ok(())
        }

        fn hide_cursor(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }

        fn show_cursor(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }

        fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
            self.counters.cursor_queries.set(self.counters.cursor_queries.get() + 1);
            Ok(self.cursor)
        }

        fn set_cursor_position<P: Into<Position>>(
            &mut self,
            position: P,
        ) -> Result<(), Self::Error> {
            self.cursor = position.into();
            Ok(())
        }

        fn clear(&mut self) -> Result<(), Self::Error> {
            self.counters.clears.set(self.counters.clears.get() + 1);
            Ok(())
        }

        fn clear_region(&mut self, _clear_type: ClearType) -> Result<(), Self::Error> {
            self.counters.clears.set(self.counters.clears.get() + 1);
            Ok(())
        }

        fn size(&self) -> Result<Size, Self::Error> {
            Ok(self.size)
        }

        fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
            Ok(WindowSize { columns_rows: self.size, pixels: Size::new(0, 0) })
        }

        fn flush(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }

        fn scroll_region_up(
            &mut self,
            _region: Range<u16>,
            _line_count: u16,
        ) -> Result<(), Self::Error> {
            self.counters.scrolls.set(self.counters.scrolls.get() + 1);
            Ok(())
        }

        fn scroll_region_down(
            &mut self,
            _region: Range<u16>,
            _line_count: u16,
        ) -> Result<(), Self::Error> {
            self.counters.scrolls.set(self.counters.scrolls.get() + 1);
            Ok(())
        }
    }

    #[test]
    fn tracked_backend_returns_seed_without_inner_cursor_query() {
        let counters = BackendCounters::default();
        let inner = FakeBackend::with_counters(80, 24, Position { x: 40, y: 20 }, counters.clone());
        let mut backend = TrackedCursorBackend::new(inner, Position { x: 3, y: 4 });

        assert_eq!(backend.get_cursor_position().unwrap(), Position { x: 3, y: 4 });
        assert_eq!(counters.cursor_queries.get(), 0);
    }

    #[test]
    fn tracked_backend_updates_on_set_cursor_position() {
        let inner = FakeBackend::new(80, 24, Position::ORIGIN);
        let mut backend = TrackedCursorBackend::new(inner, Position { x: 1, y: 2 });

        backend.set_cursor_position(Position { x: 7, y: 8 }).unwrap();

        assert_eq!(backend.get_cursor_position().unwrap(), Position { x: 7, y: 8 });
    }

    #[test]
    fn tracked_backend_tracks_draw_position() {
        let inner = FakeBackend::new(80, 24, Position::ORIGIN);
        let mut backend = TrackedCursorBackend::new(inner, Position::ORIGIN);
        let first = Cell::new("a");
        let second = Cell::new("b");
        let updates = [(2, 3, &first), (4, 5, &second)];

        backend.draw(updates.into_iter()).unwrap();

        assert_eq!(backend.tracked_cursor(), Position { x: 4, y: 5 });
    }

    #[test]
    fn tracked_backend_updates_append_lines_from_nonzero_column() {
        let inner = FakeBackend::new(80, 10, Position::ORIGIN);
        let mut backend = TrackedCursorBackend::new(inner, Position { x: 11, y: 8 });

        backend.append_lines(5).unwrap();

        assert_eq!(backend.get_cursor_position().unwrap(), Position { x: 11, y: 9 });
    }

    #[test]
    fn tracked_backend_delegates_clear_without_cursor_query() {
        let counters = BackendCounters::default();
        let inner = FakeBackend::with_counters(80, 24, Position { x: 40, y: 20 }, counters.clone());
        let mut backend = TrackedCursorBackend::new(inner, Position { x: 3, y: 4 });

        backend.clear().unwrap();
        backend.clear_region(ClearType::AfterCursor).unwrap();

        assert_eq!(counters.clears.get(), 2);
        assert_eq!(counters.cursor_queries.get(), 0);
        assert_eq!(backend.get_cursor_position().unwrap(), Position { x: 3, y: 4 });
    }

    #[test]
    fn tracked_backend_scrolling_regions_keep_cursor_contract_explicit() {
        let counters = BackendCounters::default();
        let inner = FakeBackend::with_counters(80, 24, Position::ORIGIN, counters.clone());
        let mut backend = TrackedCursorBackend::new(inner, Position { x: 6, y: 7 });

        backend.scroll_region_up(0..7, 2).unwrap();
        backend.scroll_region_down(7..12, 1).unwrap();

        assert_eq!(counters.scrolls.get(), 2);
        assert_eq!(counters.cursor_queries.get(), 0);
        assert_eq!(backend.get_cursor_position().unwrap(), Position { x: 6, y: 7 });
    }

    #[test]
    fn inline_terminal_with_options_does_not_query_inner_cursor() {
        let counters = BackendCounters::default();
        let inner = FakeBackend::with_counters(80, 24, Position { x: 40, y: 20 }, counters.clone());
        let backend = TrackedCursorBackend::new(inner, Position { x: 0, y: 4 });

        let _terminal =
            Terminal::with_options(backend, TerminalOptions { viewport: Viewport::Inline(5) })
                .unwrap();

        assert_eq!(counters.cursor_queries.get(), 0);
    }

    #[test]
    fn inline_terminal_clear_does_not_query_inner_cursor() {
        let counters = BackendCounters::default();
        let inner = FakeBackend::with_counters(80, 24, Position { x: 40, y: 20 }, counters.clone());
        let backend = TrackedCursorBackend::new(inner, Position { x: 0, y: 4 });
        let mut terminal =
            Terminal::with_options(backend, TerminalOptions { viewport: Viewport::Inline(5) })
                .unwrap();

        terminal.clear().unwrap();

        assert_eq!(counters.cursor_queries.get(), 0);
    }

    #[test]
    fn inline_terminal_insert_before_then_clear_does_not_query_inner_cursor() {
        let counters = BackendCounters::default();
        let inner = FakeBackend::with_counters(80, 24, Position { x: 40, y: 20 }, counters.clone());
        let backend = TrackedCursorBackend::new(inner, Position { x: 0, y: 4 });
        let mut terminal =
            Terminal::with_options(backend, TerminalOptions { viewport: Viewport::Inline(5) })
                .unwrap();

        terminal
            .insert_before(1, |buffer| {
                Paragraph::new(Line::from("inserted")).render(buffer.area, buffer);
            })
            .unwrap();
        terminal.clear().unwrap();

        assert_eq!(counters.cursor_queries.get(), 0);
    }
}
