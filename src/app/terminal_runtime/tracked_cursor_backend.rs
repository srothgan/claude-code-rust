// SPDX-License-Identifier: Apache-2.0

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
        let size = self.inner.size()?;
        if line_count > 0 && region.start == 0 && region.end == size.height {
            // A whole-screen scroll must append lines to native history.
            // ConPTY's region-scroll command erases rows in a one-row terminal.
            self.set_cursor_position(Position { x: 0, y: size.height.saturating_sub(1) })?;
            return self.append_lines(line_count);
        }
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
        appended_lines: Rc<CounterCell<u16>>,
    }

    struct FakeBackend {
        size: Size,
        cursor: Position,
        counters: BackendCounters,
    }

    impl FakeBackend {
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
            self.counters.appended_lines.set(self.counters.appended_lines.get() + n);
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
    fn delegated_operations_publish_only_confirmed_cursor_positions() {
        use crate::app::terminal_runtime::chat_terminal::{
            TerminalIo,
            test_io::{Operation, TestIo},
        };
        use ratatui::style::Color;
        for operation in ["move", "draw", "append", "draw-partial"] {
            let io = TestIo::new(20, 10);
            let seed = Position::new(3, 2);
            let mut backend = TrackedCursorBackend::new(io.backend(), seed);
            let mut first = Cell::new("a");
            first.set_fg(Color::Red);
            let second = Cell::new("b");
            io.fail_on(Some(operation));
            let failed = match operation {
                "move" => backend.set_cursor_position((7, 8)),
                "append" => backend.append_lines(3),
                _ => backend.draw([(2, 3, &first), (4, 5, &second)].into_iter()),
            };
            assert!(failed.unwrap_err().to_string().contains(operation));
            assert_eq!(backend.get_cursor_position().unwrap(), seed);
            if operation == "draw-partial" {
                assert!(io.screen_rows()[3].ends_with('a'));
                assert!(!io.screen_rows()[5].contains('b'));
            }
            io.fail_on(None);
            io.take_operations();
            backend.set_cursor_position((7, 8)).unwrap();
            assert_eq!(io.take_operations(), [Operation::Move(Position::new(7, 8))]);
            assert_eq!(backend.get_cursor_position().unwrap(), Position::new(7, 8));
            backend.draw([(2, 3, &first), (4, 5, &second)].into_iter()).unwrap();
            assert_eq!(
                io.take_operations(),
                [Operation::Draw(vec![(2, 3, first), (4, 5, second)])]
            );
            assert_eq!(backend.get_cursor_position().unwrap(), Position::new(4, 5));
            backend.draw(std::iter::empty()).unwrap();
            assert_eq!(backend.get_cursor_position().unwrap(), Position::new(4, 5));
            assert_eq!(io.queries(), 0);
        }
        for (seed, amount, expected) in [
            (Position::new(11, 2), 3, Position::new(11, 5)),
            (Position::new(11, 8), 5, Position::new(11, 9)),
        ] {
            let io = TestIo::new(20, 10);
            io.move_cursor(seed).unwrap();
            io.take_operations();
            let mut backend = TrackedCursorBackend::new(io.backend(), seed);
            backend.append_lines(amount).unwrap();
            assert_eq!(io.take_operations(), [Operation::Append(amount)]);
            assert_eq!(backend.get_cursor_position().unwrap(), expected);
            assert_eq!(io.queries(), 0);
        }
    }
    #[test]
    fn clearing_forwards_the_requested_region_and_preserves_the_cursor() {
        use crate::app::terminal_runtime::chat_terminal::{
            TerminalIo,
            test_io::{Operation, TestIo},
        };
        let io = TestIo::new(20, 4);
        io.seed_row(0, "outside");
        io.seed_row(1, "abc erase");
        io.seed_row(2, "erase below");
        let seed = Position::new(3, 1);
        io.move_cursor(seed).unwrap();
        io.take_operations();
        let mut backend = TrackedCursorBackend::new(io.backend(), seed);
        backend.clear_region(ClearType::AfterCursor).unwrap();
        assert_eq!(io.take_operations(), [Operation::Clear(ClearType::AfterCursor)]);
        assert_eq!(&io.screen_rows()[..3], ["outside", "abc", ""]);
        assert_eq!(backend.get_cursor_position().unwrap(), seed);
        backend.clear().unwrap();
        assert_eq!(io.take_operations(), [Operation::Clear(ClearType::All)]);
        assert!(io.screen_rows().iter().all(String::is_empty));
        assert_eq!(backend.get_cursor_position().unwrap(), seed);
        assert_eq!(io.queries(), 0);
    }
    #[test]
    fn scrolling_preserves_regions_and_orders_native_history_operations() {
        use crate::app::terminal_runtime::chat_terminal::{
            TerminalIo,
            test_io::{Operation, TestIo},
        };
        let io = TestIo::new(20, 8);
        let seed = Position::new(6, 4);
        let mut backend = TrackedCursorBackend::new(io.backend(), seed);
        backend.scroll_region_up(2..6, 2).unwrap();
        backend.scroll_region_down(1..5, 1).unwrap();
        assert_eq!(io.take_operations(), [Operation::Up(2..6, 2), Operation::Down(1..5, 1)]);
        assert_eq!(backend.get_cursor_position().unwrap(), seed);
        for (height, bottom) in [(1, 0), (8, 7)] {
            let io = TestIo::new(20, height);
            io.seed_row(0, "preserved");
            let mut backend = TrackedCursorBackend::new(io.backend(), Position::ORIGIN);
            let before = io.transcript();
            backend.scroll_region_up(0..height, 0).unwrap();
            assert_eq!(io.transcript(), before);
            assert_eq!(backend.get_cursor_position().unwrap(), Position::ORIGIN);
            assert!(
                !io.take_operations()
                    .iter()
                    .any(|op| matches!(op, Operation::Move(_) | Operation::Append(_)))
            );
            backend.scroll_region_up(0..height, 3).unwrap();
            assert_eq!(
                io.take_operations(),
                [Operation::Move(Position::new(0, bottom)), Operation::Append(3)]
            );
            assert_eq!(backend.get_cursor_position().unwrap(), Position::new(0, bottom));
            assert_eq!(io.queries(), 0);
        }
        for failure in ["size", "move"] {
            let io = TestIo::new(20, 8);
            io.fail_on(Some(failure));
            let mut backend = TrackedCursorBackend::new(io.backend(), seed);
            assert!(backend.scroll_region_up(0..8, 2).unwrap_err().to_string().contains(failure));
            assert!(!io.take_operations().iter().any(|op| matches!(op, Operation::Append(_))));
            assert_eq!(backend.get_cursor_position().unwrap(), seed);
        }
    }
    #[test]
    fn inline_adapter_preserves_history_and_repaints_without_cursor_queries() {
        use crate::app::terminal_runtime::chat_terminal::{TerminalIo, test_io::TestIo};
        let io = TestIo::new(24, 12);
        io.seed_row(0, "outside-a");
        io.seed_row(1, "outside-b");
        io.move_cursor(Position::new(0, 2)).unwrap();
        let backend = TrackedCursorBackend::new(io.backend(), Position::new(0, 2));
        let mut terminal =
            Terminal::with_options(backend, TerminalOptions { viewport: Viewport::Inline(3) })
                .unwrap();
        assert_eq!(io.queries(), 0);
        terminal
            .draw(|frame| {
                assert_eq!(frame.area(), ratatui::layout::Rect::new(0, 2, 24, 3));
                frame.render_widget(Paragraph::new("old-a\nold-b\nold-c"), frame.area());
            })
            .unwrap();
        terminal
            .insert_before(2, |buffer| {
                Paragraph::new("history-a\nhistory-b").render(buffer.area, buffer);
            })
            .unwrap();
        assert_eq!(io.queries(), 0);
        terminal.clear().unwrap();
        assert_eq!(io.queries(), 0);
        assert_eq!(
            &io.screen_rows()[..7],
            ["outside-a", "outside-b", "history-a", "history-b", "", "", ""]
        );
        terminal
            .draw(|frame| {
                assert_eq!(frame.area(), ratatui::layout::Rect::new(0, 4, 24, 3));
                frame.render_widget(Paragraph::new("new-a\nnew-b\nnew-c"), frame.area());
            })
            .unwrap();
        assert_eq!(
            &io.screen_rows()[..7],
            ["outside-a", "outside-b", "history-a", "history-b", "new-a", "new-b", "new-c"]
        );
        assert_eq!(io.queries(), 0);
    }
}
