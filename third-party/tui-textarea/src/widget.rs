use crate::ratatui::buffer::Buffer;
use crate::ratatui::layout::{Alignment, Position, Rect};
use crate::ratatui::style::Style;
use crate::ratatui::text::{Span, Text};
use crate::ratatui::widgets::{Paragraph, Widget};
use crate::textarea::{CursorRenderMode, TextArea};
use crate::util::num_digits;
use portable_atomic::{AtomicU64, Ordering};
use std::cmp;
use unicode_width::UnicodeWidthStr as _;

// &mut 'a (u16, u16, u16, u16) is not available since `render` method takes immutable reference of TextArea
// instance. In the case, the TextArea instance cannot be accessed from any other objects since it is mutablly
// borrowed.
//
// `ratatui::Frame::render_stateful_widget` would be an assumed way to render a stateful widget. But at this
// point we stick with using `ratatui::Frame::render_widget` because it is simpler API. Users don't need to
// manage states of textarea instances separately.
// https://docs.rs/ratatui/latest/ratatui/terminal/struct.Frame.html#method.render_stateful_widget
#[derive(Default, Debug)]
pub struct Viewport(AtomicU64);

impl Clone for Viewport {
    fn clone(&self) -> Self {
        let u = self.0.load(Ordering::Relaxed);
        Viewport(AtomicU64::new(u))
    }
}

impl Viewport {
    pub fn scroll_top(&self) -> (u16, u16) {
        let u = self.0.load(Ordering::Relaxed);
        ((u >> 16) as u16, u as u16)
    }

    pub fn rect(&self) -> (u16, u16, u16, u16) {
        let u = self.0.load(Ordering::Relaxed);
        let width = (u >> 48) as u16;
        let height = (u >> 32) as u16;
        let row = (u >> 16) as u16;
        let col = u as u16;
        (row, col, width, height)
    }

    pub fn position(&self) -> (u16, u16, u16, u16) {
        let (row_top, col_top, width, height) = self.rect();
        let row_bottom = row_top.saturating_add(height).saturating_sub(1);
        let col_bottom = col_top.saturating_add(width).saturating_sub(1);

        (
            row_top,
            col_top,
            cmp::max(row_top, row_bottom),
            cmp::max(col_top, col_bottom),
        )
    }

    fn store(&self, row: u16, col: u16, width: u16, height: u16) {
        // Pack four u16 values into one u64 value
        let u =
            ((width as u64) << 48) | ((height as u64) << 32) | ((row as u64) << 16) | col as u64;
        self.0.store(u, Ordering::Relaxed);
    }

    pub fn scroll(&mut self, rows: i16, cols: i16) {
        fn apply_scroll(pos: u16, delta: i16) -> u16 {
            if delta >= 0 {
                pos.saturating_add(delta as u16)
            } else {
                pos.saturating_sub(-delta as u16)
            }
        }

        let u = self.0.get_mut();
        let row = apply_scroll((*u >> 16) as u16, rows);
        let col = apply_scroll(*u as u16, cols);
        *u = (*u & 0xffff_ffff_0000_0000) | ((row as u64) << 16) | (col as u64);
    }
}

#[inline]
fn next_scroll_top(prev_top: u16, cursor: u16, len: u16) -> u16 {
    if cursor < prev_top {
        cursor
    } else if prev_top + len <= cursor {
        cursor + 1 - len
    } else {
        prev_top
    }
}

struct RenderPlan {
    inner_area: Rect,
    top_row: u16,
    top_col: u16,
    rendered_cursor_position: Option<Position>,
    placeholder_active: bool,
}

impl<'a> TextArea<'a> {
    fn text_widget(&'a self, top_row: usize, height: usize) -> Text<'a> {
        let lnum_len = num_digits(self.lines().len());
        let screen_lines = self.screen_lines.borrow();
        let bottom_row = cmp::min(top_row + height, screen_lines.len());
        let mut lines = Vec::with_capacity(bottom_row - top_row);
        for row in &screen_lines[top_row..bottom_row] {
            let line = &self.lines()[row.wrapped.row];
            lines.push(self.line_spans_segment(line, &row.wrapped, lnum_len));
        }
        Text::from(lines)
    }

    fn placeholder_widget(&'a self) -> Text<'a> {
        let mut placeholder = self.placeholder.clone();
        if self.cursor_render_mode() == CursorRenderMode::Cell {
            let cursor = Span::styled(" ", self.cursor_style);
            if let Some(line) = placeholder.lines.first_mut() {
                line.spans.insert(0, cursor);
            }
        }
        placeholder
    }

    fn scroll_top_row(&self, prev_top: u16, height: u16) -> u16 {
        // Reflow or a taller viewport can make the previous offset exceed the last full page.
        let last_page = self.screen_lines.borrow().len().saturating_sub(usize::from(height));
        let last_page = u16::try_from(last_page).unwrap_or(u16::MAX);
        next_scroll_top(prev_top, self.screen_cursor().row as u16, height).min(last_page)
    }

    fn scroll_top_col(&self, prev_top: u16, width: u16) -> u16 {
        let mut cursor = self.screen_cursor().col as u16;

        // Adjust the cursor position due to the width of line number.
        if self.line_number_style().is_some() {
            let lnum = self.layout_metrics(width).line_number_width() as u16;
            if cursor <= lnum {
                cursor *= 2; // Smoothly slide the line number into the screen on scrolling left
            } else {
                cursor += lnum; // The cursor position is shifted by the line number part
            };
        }
        next_scroll_top(prev_top, cursor, width)
    }

    fn rendered_line_geometry(
        &self,
        screen_row: usize,
        width: u16,
        top_col: u16,
    ) -> (usize, usize) {
        let line = self.screen_line(screen_row);
        let text = &self.lines()[line.wrapped.row];
        let rendered = self.line_spans_segment(text, &line.wrapped, num_digits(self.lines().len()));
        let alignment = rendered.alignment.unwrap_or(self.alignment());
        if alignment == Alignment::Left {
            return (0, top_col as usize);
        }

        let mut rendered_width = 0u16;
        for grapheme in rendered.styled_graphemes(Style::default()) {
            let grapheme_width = u16::try_from(grapheme.symbol.width()).unwrap_or(u16::MAX);
            if grapheme_width > width {
                continue;
            }
            if rendered_width.saturating_add(grapheme_width) > width {
                break;
            }
            rendered_width += grapheme_width;
        }
        let offset = match alignment {
            Alignment::Center => width / 2 - rendered_width / 2,
            Alignment::Right => width - rendered_width,
            Alignment::Left => 0,
        };
        (offset as usize, 0)
    }

    fn render_plan(&self, area: Rect) -> RenderPlan {
        let inner_area = if let Some(b) = self.block() {
            b.inner(area)
        } else {
            area
        };

        if self.area.get() != inner_area {
            self.area.set(inner_area);
            self.screen_map_load();
        }

        let Rect { width, height, .. } = inner_area;
        let placeholder_active = !self.placeholder.lines.is_empty() && self.is_empty();
        let (prev_top_row, prev_top_col) = self.viewport.scroll_top();

        let (top_row, top_col) = if placeholder_active {
            (0, 0)
        } else {
            let top_row = self.scroll_top_row(prev_top_row, height);
            let top_col = if self.layout_metrics(width).wrap_width().is_none() {
                self.scroll_top_col(prev_top_col, width)
            } else {
                0
            };
            (top_row, top_col)
        };

        let rendered_cursor_position =
            self.rendered_position_in(inner_area, top_row, top_col, placeholder_active);

        RenderPlan {
            inner_area,
            top_row,
            top_col,
            rendered_cursor_position,
            placeholder_active,
        }
    }

    fn rendered_position_in(
        &self,
        inner_area: Rect,
        top_row: u16,
        top_col: u16,
        placeholder_active: bool,
    ) -> Option<Position> {
        if inner_area.width == 0 || inner_area.height == 0 {
            return None;
        }

        if placeholder_active {
            return Some(Position {
                x: inner_area.x,
                y: inner_area.y,
            });
        }

        let cursor = self.screen_cursor();
        let cursor_row = cursor.row;
        let top_row = top_row as usize;
        let height = inner_area.height as usize;
        if cursor_row < top_row || top_row.saturating_add(height) <= cursor_row {
            return None;
        }

        let metrics = self.layout_metrics(inner_area.width);
        let mut cursor_col = cursor.col;
        cursor_col = cursor_col.saturating_add(metrics.line_number_width());

        let (line_offset, horizontal_scroll) =
            self.rendered_line_geometry(cursor_row, inner_area.width, top_col);
        let width = inner_area.width as usize;
        if cursor_col < horizontal_scroll {
            return None;
        }
        let rendered_col = line_offset.saturating_add(cursor_col - horizontal_scroll);
        if width <= rendered_col {
            return None;
        }

        Some(Position {
            x: inner_area.x + rendered_col as u16,
            y: inner_area.y + (cursor_row - top_row) as u16,
        })
    }

    /// Resolve an absolute terminal position to a zero-based `(row, column)` cursor in the text.
    ///
    /// The position is tested against the textarea's inner area from its most recent render. Positions outside that area return `None`; positions inside its empty space clamp to the closest cursor on the corresponding line or at the end of the text. Block borders, line numbers, scrolling, wrapping, alignment, tabs, and wide Unicode characters are handled automatically.
    ///
    /// Render the textarea before calling this method, and render it again after changing its contents, cursor, layout, or rendering configuration.
    /// ```
    /// use ratatui::buffer::Buffer;
    /// use ratatui::layout::{Position, Rect};
    /// use ratatui::widgets::Widget as _;
    /// use tui_textarea::TextArea;
    ///
    /// let textarea = TextArea::from(["hello"]);
    /// let area = Rect::new(10, 5, 8, 1);
    /// (&textarea).render(area, &mut Buffer::empty(area));
    ///
    /// assert_eq!(textarea.cursor_at_position(Position::new(12, 5)), Some((0, 2)));
    /// assert_eq!(textarea.cursor_at_position(Position::new(9, 5)), None);
    /// ```
    pub fn cursor_at_position(&self, position: Position) -> Option<(usize, usize)> {
        let area = self.area.get();
        if area.is_empty() || !area.contains(position) {
            return None;
        }
        if self.is_empty() && !self.placeholder.lines.is_empty() {
            return Some((0, 0));
        }

        let (top_row, top_col) = self.viewport.scroll_top();
        let metrics = self.layout_metrics(area.width);
        let screen_row = (top_row as usize)
            .saturating_add((position.y - area.y) as usize)
            .min(self.screen_lines_count().saturating_sub(1));
        let (line_offset, horizontal_scroll) =
            self.rendered_line_geometry(screen_row, area.width, top_col);
        let rendered_col = (position.x - area.x) as usize;
        let screen_col = rendered_col
            .saturating_sub(line_offset)
            .saturating_add(horizontal_scroll)
            .saturating_sub(metrics.line_number_width())
            .min(self.screen_line_max_cursor_col(screen_row));
        Some(
            self.screen_to_data_cursor(crate::cursor::ScreenCursor {
                row: screen_row,
                col: screen_col,
                char: None,
                dc: None,
            })
            .into(),
        )
    }
}

impl Widget for &TextArea<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let plan = self.render_plan(area);
        let Rect { width, height, .. } = plan.inner_area;

        let (text, style) = if plan.placeholder_active {
            (self.placeholder_widget(), Style::default())
        } else {
            (
                self.text_widget(plan.top_row as _, height as _),
                self.style(),
            )
        };

        // To get fine control over the text color and the surrrounding block they have to be rendered separately
        // see https://github.com/ratatui/ratatui/issues/144
        let mut text_area = plan.inner_area;
        let mut inner = Paragraph::new(text)
            .style(style)
            .alignment(self.alignment());
        if let Some(b) = self.block() {
            text_area = plan.inner_area;
            b.render(area, buf)
        }
        if plan.top_col != 0 {
            inner = inner.scroll((0, plan.top_col));
        }

        // Store scroll top position for rendering on the next tick
        self.viewport
            .store(plan.top_row, plan.top_col, width, height);
        self.rendered_cursor_position
            .set(plan.rendered_cursor_position);

        inner.render(text_area, buf);
    }
}
