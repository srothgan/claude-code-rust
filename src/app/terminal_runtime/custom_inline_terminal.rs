// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use std::io;
use std::io::Write;

use ratatui::backend::{Backend, ClearType};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect, Size};
use ratatui::widgets::Widget;

use super::screen_scroll::ScreenScrollDirection;

#[derive(Debug, Hash)]
pub(crate) struct Frame<'a> {
    cursor_position: Option<Position>,
    viewport_area: Rect,
    buffer: &'a mut Buffer,
}

impl Frame<'_> {
    pub(crate) const fn area(&self) -> Rect {
        self.viewport_area
    }

    pub(crate) fn render_widget<W: Widget>(&mut self, widget: W, area: Rect) {
        widget.render(area, self.buffer);
    }

    pub(crate) fn set_cursor_position<P: Into<Position>>(&mut self, position: P) {
        self.cursor_position = Some(position.into());
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct CompletedFrame<'a> {
    pub(crate) buffer: &'a Buffer,
    pub(crate) area: Rect,
}

#[derive(Debug)]
pub(crate) struct Terminal<B>
where
    B: Backend<Error = io::Error> + Write,
{
    backend: B,
    buffers: [Buffer; 2],
    current: usize,
    hidden_cursor: bool,
    pub(crate) viewport_area: Rect,
    pub(crate) last_known_screen_size: Size,
    pub(crate) last_known_cursor_pos: Position,
    history_top: Option<u16>,
    history_bottom_exclusive: u16,
}

impl<B> Drop for Terminal<B>
where
    B: Backend<Error = io::Error> + Write,
{
    fn drop(&mut self) {
        if self.hidden_cursor {
            let _ = self.show_cursor();
        }
    }
}

impl<B> Terminal<B>
where
    B: Backend<Error = io::Error> + Write,
{
    pub(crate) fn with_options(mut backend: B) -> io::Result<Self> {
        let screen_size = backend.size()?;
        let cursor_pos = backend.get_cursor_position().unwrap_or(Position::ORIGIN);
        let viewport_area = Rect::new(0, cursor_pos.y, 0, 0);
        Ok(Self {
            backend,
            buffers: [Buffer::empty(viewport_area), Buffer::empty(viewport_area)],
            current: 0,
            hidden_cursor: false,
            viewport_area,
            last_known_screen_size: screen_size,
            last_known_cursor_pos: cursor_pos,
            history_top: None,
            history_bottom_exclusive: 0,
        })
    }

    fn get_frame(&mut self) -> Frame<'_> {
        Frame {
            cursor_position: None,
            viewport_area: self.viewport_area,
            buffer: &mut self.buffers[self.current],
        }
    }

    pub(crate) fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    pub(crate) fn size(&self) -> io::Result<Size> {
        self.backend.size()
    }

    pub(crate) fn set_viewport_area(&mut self, area: Rect) {
        self.buffers[self.current].resize(area);
        self.buffers[1 - self.current].resize(area);
        self.viewport_area = area;
        self.history_bottom_exclusive = self.history_bottom_exclusive.min(area.top());
        if self.history_top.is_some_and(|top| top >= self.history_bottom_exclusive) {
            self.history_top = None;
            self.history_bottom_exclusive = 0;
        }
    }

    fn autoresize(&mut self) -> io::Result<()> {
        let screen_size = self.size()?;
        if screen_size != self.last_known_screen_size {
            self.last_known_screen_size = screen_size;
        }
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        let previous_buffer = &self.buffers[1 - self.current];
        let current_buffer = &self.buffers[self.current];
        let updates = previous_buffer.diff(current_buffer);
        if let Some((x, y, _)) = updates.last() {
            self.last_known_cursor_pos = Position { x: *x, y: *y };
        }
        self.backend.draw(updates.into_iter())
    }

    pub(crate) fn draw<F>(&mut self, render_callback: F) -> io::Result<CompletedFrame<'_>>
    where
        F: FnOnce(&mut Frame<'_>),
    {
        self.autoresize()?;

        let mut frame = self.get_frame();
        render_callback(&mut frame);
        let cursor_position = frame.cursor_position;

        self.flush()?;

        match cursor_position {
            None => self.hide_cursor()?,
            Some(position) => {
                self.show_cursor()?;
                self.set_cursor_position(position)?;
            }
        }

        self.swap_buffers();
        Backend::flush(&mut self.backend)?;

        Ok(CompletedFrame { buffer: &self.buffers[1 - self.current], area: self.viewport_area })
    }

    pub(crate) fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        let position = position.into();
        self.backend.set_cursor_position(position)?;
        self.last_known_cursor_pos = position;
        Ok(())
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.backend.hide_cursor()?;
        self.hidden_cursor = true;
        Ok(())
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.backend.show_cursor()?;
        self.hidden_cursor = false;
        Ok(())
    }

    pub(crate) fn clear(&mut self) -> io::Result<()> {
        if self.viewport_area.is_empty() {
            return Ok(());
        }

        self.backend.set_cursor_position(self.viewport_area.as_position())?;
        self.backend.clear_region(ClearType::AfterCursor)?;
        self.buffers[1 - self.current].reset();
        Ok(())
    }

    pub(crate) fn invalidate_viewport(&mut self) {
        self.buffers[1 - self.current].reset();
    }

    pub(crate) fn history_is_visible(&self) -> bool {
        self.history_top.is_some()
    }

    pub(crate) fn history_bounds(&self) -> Option<(u16, u16)> {
        self.history_top.map(|top| (top, self.history_bottom_exclusive))
    }

    pub(crate) fn history_bottom_exclusive(&self) -> Option<u16> {
        self.history_is_visible().then_some(self.history_bottom_exclusive)
    }

    pub(crate) fn record_history_insert(&mut self, insert_top: u16, inserted_rows: u16) {
        if inserted_rows == 0 {
            return;
        }

        let insert_bottom = insert_top.saturating_add(inserted_rows).min(self.viewport_area.top());
        self.history_top = Some(self.history_top.map_or(insert_top, |top| top.min(insert_top)));
        self.history_bottom_exclusive = self.history_bottom_exclusive.max(insert_bottom);
        self.history_bottom_exclusive = self.history_bottom_exclusive.min(self.viewport_area.top());
        if self.history_top.is_some_and(|top| top >= self.history_bottom_exclusive) {
            self.history_top = None;
            self.history_bottom_exclusive = 0;
        }
    }

    pub(crate) fn apply_screen_scroll_bookkeeping(
        &mut self,
        direction: ScreenScrollDirection,
        rows: u16,
        cursor_pos: Position,
    ) {
        self.last_known_cursor_pos = cursor_pos;
        match direction {
            ScreenScrollDirection::Up => self.shift_history_up(rows),
            ScreenScrollDirection::Down => self.shift_history_down(rows),
        }
        self.invalidate_viewport();
    }

    fn shift_history_up(&mut self, rows: u16) {
        let Some(top) = self.history_top else {
            return;
        };

        self.history_top = Some(top.saturating_sub(rows));
        self.history_bottom_exclusive = self.history_bottom_exclusive.saturating_sub(rows);
        if self.history_top.is_some_and(|next_top| next_top >= self.history_bottom_exclusive) {
            self.history_top = None;
            self.history_bottom_exclusive = 0;
        }
    }

    fn shift_history_down(&mut self, rows: u16) {
        let Some(top) = self.history_top else {
            return;
        };

        self.history_top = Some(top.saturating_add(rows));
        self.history_bottom_exclusive = self.history_bottom_exclusive.saturating_add(rows);
        if self.history_top.is_some_and(|next_top| next_top >= self.history_bottom_exclusive) {
            self.history_top = None;
            self.history_bottom_exclusive = 0;
        }
    }

    fn swap_buffers(&mut self) {
        self.buffers[1 - self.current].reset();
        self.current = 1 - self.current;
    }
}
