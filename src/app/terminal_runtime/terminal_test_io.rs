// SPDX-License-Identifier: Apache-2.0

use super::{TerminalIo, TerminalSnapshot};
use ratatui::backend::{Backend, ClearType, TestBackend, WindowSize};
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::{Position, Size};
use std::cell::RefCell;
use std::io;
use std::ops::Range;
use std::rc::Rc;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Operation {
    Move(Position),
    Draw(Vec<(u16, u16, Cell)>),
    Append(u16),
    Clear(ClearType),
    Up(Range<u16>, u16),
    Down(Range<u16>, u16),
}

struct State {
    screen: TestBackend,
    operations: Vec<Operation>,
    fail: Option<&'static str>,
    queries: usize,
    resize_after_draw: Option<(u16, u16)>,
}

#[derive(Clone)]
pub(crate) struct TestIo(Rc<RefCell<State>>);

impl TestIo {
    pub(crate) fn new(width: u16, height: u16) -> Self {
        Self(Rc::new(RefCell::new(State {
            screen: TestBackend::new(width, height),
            operations: Vec::new(),
            fail: None,
            queries: 0,
            resize_after_draw: None,
        })))
    }
    pub(crate) fn resize_after_draw(&self, width: u16, height: u16) {
        self.0.borrow_mut().resize_after_draw = Some((width, height));
    }
    pub(crate) fn fail_on(&self, operation: Option<&'static str>) {
        self.0.borrow_mut().fail = operation;
    }
    fn check(&self, operation: &'static str) -> io::Result<()> {
        if self.0.borrow().fail == Some(operation) {
            Err(io::Error::other(format!("injected {operation} failure")))
        } else {
            Ok(())
        }
    }
    pub(crate) fn take_operations(&self) -> Vec<Operation> {
        std::mem::take(&mut self.0.borrow_mut().operations)
    }
    pub(crate) fn queries(&self) -> usize {
        self.0.borrow().queries
    }
    pub(crate) fn resize(&self, width: u16, height: u16) {
        self.0.borrow_mut().screen.resize(width, height);
    }
    fn rows(buffer: &Buffer) -> Vec<String> {
        buffer
            .content
            .chunks(usize::from(buffer.area.width))
            .map(|row| {
                let mut text = String::new();
                let mut column = 0;
                while column < row.len() {
                    let symbol = row[column].symbol();
                    text.push_str(symbol);
                    column += symbol.width().max(1);
                }
                text.trim_end().to_owned()
            })
            .collect()
    }
    pub(crate) fn screen_rows(&self) -> Vec<String> {
        Self::rows(self.0.borrow().screen.buffer())
    }
    pub(crate) fn transcript(&self) -> Vec<String> {
        let state = self.0.borrow();
        let mut rows = Self::rows(state.screen.scrollback());
        rows.extend(Self::rows(state.screen.buffer()));
        rows
    }
    pub(crate) fn seed_row(&self, y: u16, text: &str) {
        let cells = text
            .chars()
            .enumerate()
            .map(|(x, c)| (u16::try_from(x).unwrap(), y, Cell::default().set_char(c).clone()))
            .collect::<Vec<_>>();
        self.0.borrow_mut().screen.draw(cells.iter().map(|(x, y, c)| (*x, *y, c))).unwrap();
    }
}

pub(crate) struct ObservedBackend(TestIo);

impl Backend for ObservedBackend {
    type Error = io::Error;
    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        self.0.check("draw")?;
        let cells = content.map(|(x, y, c)| (x, y, c.clone())).collect::<Vec<_>>();
        self.0.0.borrow_mut().operations.push(Operation::Draw(cells.clone()));
        if self.0.0.borrow().fail == Some("draw-partial") {
            self.0
                .0
                .borrow_mut()
                .screen
                .draw(cells.iter().take(1).map(|(x, y, c)| (*x, *y, c)))
                .unwrap();
            return Err(io::Error::other("injected draw-partial failure"));
        }
        self.0.0.borrow_mut().screen.draw(cells.iter().map(|(x, y, c)| (*x, *y, c))).unwrap();
        let resize = self.0.0.borrow_mut().resize_after_draw.take();
        if let Some((width, height)) = resize {
            self.0.resize(width, height);
        }
        self.0.check("draw-after")
    }
    fn append_lines(&mut self, n: u16) -> io::Result<()> {
        self.0.check("append")?;
        let mut s = self.0.0.borrow_mut();
        s.operations.push(Operation::Append(n));
        s.screen.append_lines(n).unwrap();
        Ok(())
    }
    fn hide_cursor(&mut self) -> io::Result<()> {
        Ok(())
    }
    fn show_cursor(&mut self) -> io::Result<()> {
        Ok(())
    }
    fn get_cursor_position(&mut self) -> io::Result<Position> {
        self.0.0.borrow_mut().queries += 1;
        Err(io::Error::other("inner cursor query must not compete with input"))
    }
    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        self.0.check("move")?;
        let p = position.into();
        let mut s = self.0.0.borrow_mut();
        let size = s.screen.size().unwrap();
        assert!(p.x < size.width && p.y < size.height, "out-of-screen move {p:?} in {size:?}");
        s.operations.push(Operation::Move(p));
        s.screen.set_cursor_position(p).unwrap();
        Ok(())
    }
    fn clear(&mut self) -> io::Result<()> {
        self.clear_region(ClearType::All)
    }
    fn clear_region(&mut self, kind: ClearType) -> io::Result<()> {
        self.0.check("clear")?;
        let mut s = self.0.0.borrow_mut();
        s.operations.push(Operation::Clear(kind));
        s.screen.clear_region(kind).unwrap();
        Ok(())
    }
    fn size(&self) -> io::Result<Size> {
        self.0.check("size")?;
        Ok(self.0.0.borrow().screen.size().unwrap())
    }
    fn window_size(&mut self) -> io::Result<WindowSize> {
        Ok(WindowSize { columns_rows: self.size()?, pixels: Size::new(0, 0) })
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.check("flush")
    }
    fn scroll_region_up(&mut self, region: Range<u16>, n: u16) -> io::Result<()> {
        self.0.check("up")?;
        let mut s = self.0.0.borrow_mut();
        s.operations.push(Operation::Up(region.clone(), n));
        s.screen.scroll_region_up(region, n).unwrap();
        Ok(())
    }
    fn scroll_region_down(&mut self, region: Range<u16>, n: u16) -> io::Result<()> {
        self.0.check("down")?;
        let mut s = self.0.0.borrow_mut();
        s.operations.push(Operation::Down(region.clone(), n));
        s.screen.scroll_region_down(region, n).unwrap();
        Ok(())
    }
}

impl TerminalIo for TestIo {
    type Backend = ObservedBackend;
    fn backend(&self) -> Self::Backend {
        ObservedBackend(self.clone())
    }
    fn snapshot(&self) -> anyhow::Result<TerminalSnapshot> {
        let size = self.backend().size()?;
        Ok(TerminalSnapshot::new(size.width, size.height))
    }
    fn move_cursor(&self, p: Position) -> anyhow::Result<()> {
        self.backend().set_cursor_position(p)?;
        Ok(())
    }
    fn clear_lines(&self, rows: Range<u16>) -> anyhow::Result<()> {
        for row in rows {
            self.move_cursor(Position::new(0, row))?;
            self.backend().clear_region(ClearType::CurrentLine)?;
        }
        Ok(())
    }
    fn append_blank_lines(&self, bottom: u16, rows: u16) -> anyhow::Result<()> {
        self.move_cursor(Position::new(0, bottom))?;
        self.backend().append_lines(rows)?;
        self.move_cursor(Position::new(0, bottom))
    }
    fn purge(&self) -> anyhow::Result<()> {
        let size = self.backend().size()?;
        self.0.borrow_mut().screen = TestBackend::new(size.width, size.height);
        Ok(())
    }
    fn synchronized<T>(&self, draw: impl FnOnce() -> anyhow::Result<T>) -> anyhow::Result<T> {
        draw()
    }
}
