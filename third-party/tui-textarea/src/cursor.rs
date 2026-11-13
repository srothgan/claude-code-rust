use crate::textarea::TextArea;
use crate::widget::Viewport;
use crate::word::{
    find_word_inclusive_end_forward, find_word_start_backward, find_word_start_forward,
};
#[cfg(feature = "arbitrary")]
use arbitrary::Arbitrary;
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};
use std::cmp;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "arbitrary", derive(Arbitrary))]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub(crate) struct DataCursor(pub usize, pub usize);

impl DataCursor {
    pub(crate) fn to_screen_cursor(self, ta: &TextArea<'_>) -> ScreenCursor {
        ta.data_to_screen_cursor(self)
    }
}

impl PartialEq<(usize, usize)> for DataCursor {
    fn eq(&self, other: &(usize, usize)) -> bool {
        self.0 == other.0 && self.1 == other.1
    }
}

impl From<(usize, usize)> for DataCursor {
    fn from((row, col): (usize, usize)) -> Self {
        Self(row, col)
    }
}

impl From<DataCursor> for (usize, usize) {
    fn from(cursor: DataCursor) -> Self {
        (cursor.0, cursor.1)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "arbitrary", derive(Arbitrary))]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub(crate) struct ScreenCursor {
    pub row: usize,
    pub col: usize,
    pub char: Option<char>,
    pub dc: Option<DataCursor>,
}

impl ScreenCursor {
    pub(crate) fn to_data_cursor(self, ta: &TextArea<'_>) -> DataCursor {
        ta.screen_to_data_cursor(self)
    }
}

/// Specify how to move the cursor.
///
/// This type is marked as `#[non_exhaustive]` since more variations may be supported in the future.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "arbitrary", derive(Arbitrary))]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum CursorMove {
    /// Move cursor forward by one character. When the cursor is at the end of line, it moves to the head of next line.
    /// ```
    /// use tui_textarea::{TextArea, CursorMove};
    ///
    /// let mut textarea = TextArea::from(["abc"]);
    ///
    /// textarea.move_cursor(CursorMove::Forward);
    /// assert_eq!(textarea.cursor(), (0, 1));
    /// textarea.move_cursor(CursorMove::Forward);
    /// assert_eq!(textarea.cursor(), (0, 2));
    /// ```
    Forward,
    /// Move cursor backward by one character. When the cursor is at the head of line, it moves to the end of previous
    /// line.
    /// ```
    /// use tui_textarea::{TextArea, CursorMove};
    ///
    /// let mut textarea = TextArea::from(["abc"]);
    ///
    /// textarea.move_cursor(CursorMove::Forward);
    /// textarea.move_cursor(CursorMove::Forward);
    /// textarea.move_cursor(CursorMove::Back);
    /// assert_eq!(textarea.cursor(), (0, 1));
    /// ```
    Back,
    /// Move cursor up by one line.
    /// ```
    /// use tui_textarea::{TextArea, CursorMove};
    ///
    /// let mut textarea = TextArea::from(["a", "b", "c"]);
    ///
    /// textarea.move_cursor(CursorMove::Down);
    /// textarea.move_cursor(CursorMove::Down);
    /// textarea.move_cursor(CursorMove::Up);
    /// assert_eq!(textarea.cursor(), (1, 0));
    /// ```
    Up,
    /// Move cursor down by one line.
    /// ```
    /// use tui_textarea::{TextArea, CursorMove};
    ///
    /// let mut textarea = TextArea::from(["a", "b", "c"]);
    ///
    /// textarea.move_cursor(CursorMove::Down);
    /// assert_eq!(textarea.cursor(), (1, 0));
    /// textarea.move_cursor(CursorMove::Down);
    /// assert_eq!(textarea.cursor(), (2, 0));
    /// ```
    Down,
    /// Move cursor to the head of line. When the cursor is at the head of line, it moves to the end of previous line.
    /// ```
    /// use tui_textarea::{TextArea, CursorMove};
    ///
    /// let mut textarea = TextArea::from(["abc"]);
    ///
    /// textarea.move_cursor(CursorMove::Forward);
    /// textarea.move_cursor(CursorMove::Forward);
    /// textarea.move_cursor(CursorMove::Head);
    /// assert_eq!(textarea.cursor(), (0, 0));
    /// ```
    Head,
    /// Move cursor to the end of line. When the cursor is at the end of line, it moves to the head of next line.
    /// ```
    /// use tui_textarea::{TextArea, CursorMove};
    ///
    /// let mut textarea = TextArea::from(["abc"]);
    ///
    /// textarea.move_cursor(CursorMove::End);
    /// assert_eq!(textarea.cursor(), (0, 3));
    /// ```
    End,
    /// Move cursor to the top of lines.
    /// ```
    /// use tui_textarea::{TextArea, CursorMove};
    ///
    /// let mut textarea = TextArea::from(["a", "b", "c"]);
    ///
    /// textarea.move_cursor(CursorMove::Down);
    /// textarea.move_cursor(CursorMove::Down);
    /// textarea.move_cursor(CursorMove::Top);
    /// assert_eq!(textarea.cursor(), (0, 0));
    /// ```
    Top,
    /// Move cursor to the bottom of lines.
    /// ```
    /// use tui_textarea::{TextArea, CursorMove};
    ///
    /// let mut textarea = TextArea::from(["a", "b", "c"]);
    ///
    /// textarea.move_cursor(CursorMove::Bottom);
    /// assert_eq!(textarea.cursor(), (2, 0));
    /// ```
    Bottom,
    /// Move cursor forward by one word. Word boundary appears at spaces, punctuations, and others. For example
    /// `fn foo(a)` consists of words `fn`, `foo`, `(`, `a`, `)`. When the cursor is at the end of line, it moves to the
    /// head of next line.
    /// ```
    /// use tui_textarea::{TextArea, CursorMove};
    ///
    /// let mut textarea = TextArea::from(["aaa bbb ccc"]);
    ///
    /// textarea.move_cursor(CursorMove::WordForward);
    /// assert_eq!(textarea.cursor(), (0, 4));
    /// textarea.move_cursor(CursorMove::WordForward);
    /// assert_eq!(textarea.cursor(), (0, 8));
    /// ```
    WordForward,
    /// Move cursor forward to the next end of word. Word boundary appears at spaces, punctuations, and others. For example
    /// `fn foo(a)` consists of words `fn`, `foo`, `(`, `a`, `)`. When the cursor is at the end of line, it moves to the
    /// end of the first word of the next line. This is similar to the 'e' mapping of Vim in normal mode.
    /// ```
    /// use tui_textarea::{TextArea, CursorMove};
    ///
    /// let mut textarea = TextArea::from([
    ///     "aaa bbb [[[ccc]]]",
    ///     "",
    ///     " ddd",
    /// ]);
    ///
    ///
    /// textarea.move_cursor(CursorMove::WordEnd);
    /// assert_eq!(textarea.cursor(), (0, 2));      // At the end of 'aaa'
    /// textarea.move_cursor(CursorMove::WordEnd);
    /// assert_eq!(textarea.cursor(), (0, 6));      // At the end of 'bbb'
    /// textarea.move_cursor(CursorMove::WordEnd);
    /// assert_eq!(textarea.cursor(), (0, 10));     // At the end of '[[['
    /// textarea.move_cursor(CursorMove::WordEnd);
    /// assert_eq!(textarea.cursor(), (0, 13));     // At the end of 'ccc'
    /// textarea.move_cursor(CursorMove::WordEnd);
    /// assert_eq!(textarea.cursor(), (0, 16));     // At the end of ']]]'
    /// textarea.move_cursor(CursorMove::WordEnd);
    /// assert_eq!(textarea.cursor(), (2, 3));      // At the end of 'ddd'
    /// ```
    WordEnd,
    /// Move cursor backward by one word.  Word boundary appears at spaces, punctuations, and others. For example
    /// `fn foo(a)` consists of words `fn`, `foo`, `(`, `a`, `)`.When the cursor is at the head of line, it moves to
    /// the end of previous line.
    /// ```
    /// use tui_textarea::{TextArea, CursorMove};
    ///
    /// let mut textarea = TextArea::from(["aaa bbb ccc"]);
    ///
    /// textarea.move_cursor(CursorMove::End);
    /// textarea.move_cursor(CursorMove::WordBack);
    /// assert_eq!(textarea.cursor(), (0, 8));
    /// textarea.move_cursor(CursorMove::WordBack);
    /// assert_eq!(textarea.cursor(), (0, 4));
    /// textarea.move_cursor(CursorMove::WordBack);
    /// assert_eq!(textarea.cursor(), (0, 0));
    /// ```
    WordBack,
    /// Move cursor down by one paragraph. Paragraph is a chunk of non-empty lines. Cursor moves to the first line of paragraph.
    /// ```
    /// use tui_textarea::{TextArea, CursorMove};
    ///
    /// // aaa
    /// //
    /// // bbb
    /// //
    /// // ccc
    /// // ddd
    /// let mut textarea = TextArea::from(["aaa", "", "bbb", "", "ccc", "ddd"]);
    ///
    /// textarea.move_cursor(CursorMove::ParagraphForward);
    /// assert_eq!(textarea.cursor(), (2, 0));
    /// textarea.move_cursor(CursorMove::ParagraphForward);
    /// assert_eq!(textarea.cursor(), (4, 0));
    /// ```
    ParagraphForward,
    /// Move cursor up by one paragraph. Paragraph is a chunk of non-empty lines. Cursor moves to the first line of paragraph.
    /// ```
    /// use tui_textarea::{TextArea, CursorMove};
    ///
    /// // aaa
    /// //
    /// // bbb
    /// //
    /// // ccc
    /// // ddd
    /// let mut textarea = TextArea::from(["aaa", "", "bbb", "", "ccc", "ddd"]);
    ///
    /// textarea.move_cursor(CursorMove::Bottom);
    /// textarea.move_cursor(CursorMove::ParagraphBack);
    /// assert_eq!(textarea.cursor(), (4, 0));
    /// textarea.move_cursor(CursorMove::ParagraphBack);
    /// assert_eq!(textarea.cursor(), (2, 0));
    /// textarea.move_cursor(CursorMove::ParagraphBack);
    /// assert_eq!(textarea.cursor(), (0, 0));
    /// ```
    ParagraphBack,
    /// Move cursor to (row, col) position. When the position points outside the text, the cursor position is made fit
    /// within the text. Note that row and col are 0-based. (0, 0) means the first character of the first line.
    ///
    /// When there are 10 lines, jumping to row 15 moves the cursor to the last line (row is 9 in the case). When there
    /// are 10 characters in the line, jumping to col 15 moves the cursor to end of the line (col is 10 in the case).
    /// ```
    /// use tui_textarea::{TextArea, CursorMove};
    ///
    /// let mut textarea = TextArea::from(["aaaa", "bbbb", "cccc"]);
    ///
    /// textarea.move_cursor(CursorMove::Jump(1, 2));
    /// assert_eq!(textarea.cursor(), (1, 2));
    ///
    /// textarea.move_cursor(CursorMove::Jump(10,  10));
    /// assert_eq!(textarea.cursor(), (2, 4));
    /// ```
    Jump(u16, u16),
    /// Move cursor to keep it within the viewport. For example, when a viewport displays line 8 to line 16:
    ///
    /// - cursor at line 4 is moved to line 8
    /// - cursor at line 20 is moved to line 16
    /// - cursor at line 12 is not moved
    ///
    /// This is useful when you moved a cursor but you don't want to move the viewport.
    /// ```
    /// # use ratatui::buffer::Buffer;
    /// # use ratatui::layout::Rect;
    /// # use ratatui::widgets::Widget as _;
    /// use tui_textarea::{TextArea, CursorMove};
    ///
    /// // Let's say terminal height is 8.
    ///
    /// // Create textarea with 20 lines "0", "1", "2", "3", ...
    /// // The viewport is displaying from line 1 to line 8.
    /// let mut textarea: TextArea = (0..20).into_iter().map(|i| i.to_string()).collect();
    /// # // Call `render` at least once to populate terminal size
    /// # let r = Rect { x: 0, y: 0, width: 24, height: 8 };
    /// # let mut b = Buffer::empty(r.clone());
    /// # textarea.render(r, &mut b);
    ///
    /// // Move cursor to the end of lines (line 20). It is outside the viewport (line 1 to line 8)
    /// textarea.move_cursor(CursorMove::Bottom);
    /// assert_eq!(textarea.cursor(), (19, 0));
    ///
    /// // Cursor is moved to line 8 to enter the viewport
    /// textarea.move_cursor(CursorMove::InViewport);
    /// assert_eq!(textarea.cursor(), (7, 0));
    /// ```
    InViewport,
}

impl CursorMove {
    pub(crate) fn next_cursor(
        &self,
        cursor: ScreenCursor,
        ta: &TextArea<'_>,
        viewport: &Viewport,
    ) -> Option<ScreenCursor> {
        use CursorMove::*;
        let row = cursor.row;
        let col = cursor.col;
        let dc = cursor.dc.unwrap_or_else(|| cursor.to_data_cursor(ta));

        match self {
            Forward if col >= ta.screen_line_width(row) => (row + 1 < ta.screen_lines_count())
                .then(|| ScreenCursor {
                    row: row + 1,
                    col: 0,
                    char: None,
                    dc: None,
                }),
            Forward => Some(ta.increment_screen_cursor(cursor)),
            Back if col == 0 => {
                let row = row.checked_sub(1)?;
                Some(ScreenCursor {
                    row,
                    col: ta.screen_line_max_cursor_col(row),
                    char: None,
                    dc: None,
                })
            }
            Back => Some(ta.decrement_screen_cursor(cursor)),
            Up => {
                let row = row.checked_sub(1)?;
                Some(ScreenCursor {
                    row,
                    col: cmp::min(col, ta.screen_line_max_cursor_col(row)),
                    char: None,
                    dc: None,
                })
            }
            Down => {
                if row + 1 >= ta.screen_lines_count() {
                    None
                } else {
                    Some(ScreenCursor {
                        row: row + 1,
                        col: cmp::min(col, ta.screen_line_max_cursor_col(row + 1)),
                        char: None,
                        dc: None,
                    })
                }
            }
            Head => Some(DataCursor(dc.0, 0).to_screen_cursor(ta)),
            End => Some(DataCursor(dc.0, ta.lines[dc.0].chars().count()).to_screen_cursor(ta)),
            Top => {
                let col = cmp::min(dc.1, ta.lines[0].chars().count());
                Some(DataCursor(0, col).to_screen_cursor(ta))
            }
            Bottom => {
                let row = ta.lines.len() - 1;
                let col = cmp::min(dc.1, ta.lines[row].chars().count());
                Some(DataCursor(row, col).to_screen_cursor(ta))
            }
            WordEnd => {
                // `+ 1` for not accepting the current cursor position
                if let Some(col) = find_word_inclusive_end_forward(&ta.lines[dc.0], dc.1 + 1) {
                    Some(DataCursor(dc.0, col).to_screen_cursor(ta))
                } else {
                    let mut row = dc.0;
                    loop {
                        if row == ta.lines.len() - 1 {
                            break Some(
                                DataCursor(row, ta.lines[row].chars().count()).to_screen_cursor(ta),
                            );
                        }
                        row += 1;
                        if let Some(col) = find_word_inclusive_end_forward(&ta.lines[row], 0) {
                            break Some(DataCursor(row, col).to_screen_cursor(ta));
                        }
                    }
                }
            }
            WordForward => {
                if let Some(col) = find_word_start_forward(&ta.lines[dc.0], dc.1) {
                    Some(DataCursor(dc.0, col).to_screen_cursor(ta))
                } else if dc.0 + 1 < ta.lines.len() {
                    Some(DataCursor(dc.0 + 1, 0).to_screen_cursor(ta))
                } else {
                    Some(DataCursor(dc.0, ta.lines[dc.0].chars().count()).to_screen_cursor(ta))
                }
            }
            WordBack => {
                if let Some(col) = find_word_start_backward(&ta.lines[dc.0], dc.1) {
                    Some(DataCursor(dc.0, col).to_screen_cursor(ta))
                } else if dc.0 > 0 {
                    let row = dc.0 - 1;
                    Some(DataCursor(row, ta.lines[row].chars().count()).to_screen_cursor(ta))
                } else {
                    Some(DataCursor(dc.0, 0).to_screen_cursor(ta))
                }
            }
            ParagraphForward => {
                let mut prev_is_empty = ta.lines[dc.0].is_empty();
                for row in dc.0 + 1..ta.lines.len() {
                    let line = &ta.lines[row];
                    let is_empty = line.is_empty();
                    if !is_empty && prev_is_empty {
                        let col = cmp::min(dc.1, line.chars().count());
                        return Some(DataCursor(row, col).to_screen_cursor(ta));
                    }
                    prev_is_empty = is_empty;
                }
                let row = ta.lines.len() - 1;
                let col = cmp::min(dc.1, ta.lines[row].chars().count());
                Some(DataCursor(row, col).to_screen_cursor(ta))
            }
            ParagraphBack => {
                let row = dc.0.checked_sub(1)?;
                let mut prev_is_empty = ta.lines[row].is_empty();
                for row in (0..row).rev() {
                    let is_empty = ta.lines[row].is_empty();
                    if is_empty && !prev_is_empty {
                        let target = row + 1;
                        let col = cmp::min(dc.1, ta.lines[target].chars().count());
                        return Some(DataCursor(target, col).to_screen_cursor(ta));
                    }
                    prev_is_empty = is_empty;
                }
                let col = cmp::min(dc.1, ta.lines[0].chars().count());
                Some(DataCursor(0, col).to_screen_cursor(ta))
            }
            Jump(row, col) => {
                let row = cmp::min(*row as usize, ta.lines.len() - 1);
                let col = cmp::min(*col as usize, ta.lines[row].chars().count());
                Some(DataCursor(row, col).to_screen_cursor(ta))
            }
            InViewport => {
                let (row_top, col_top, row_bottom, col_bottom) = viewport.position();
                let row = row.clamp(row_top as usize, row_bottom as usize);
                let row = cmp::min(row, ta.screen_lines_count() - 1);
                let col = col.clamp(col_top as usize, col_bottom as usize);
                let col = cmp::min(col, ta.screen_line_max_cursor_col(row));
                Some(ScreenCursor {
                    row,
                    col,
                    char: None,
                    dc: None,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DataCursor;

    #[test]
    fn data_cursor_round_trips_with_tuple() {
        let dc = DataCursor::from((2, 7));
        let tuple: (usize, usize) = dc.into();
        assert_eq!(tuple, (2, 7));
    }

    #[test]
    fn in_viewport() {
        use crate::ratatui::buffer::Buffer;
        use crate::ratatui::layout::Rect;
        use crate::ratatui::widgets::Widget as _;
        use crate::{CursorMove, TextArea};

        let mut textarea: TextArea = (0..20).map(|i| i.to_string()).collect();
        let r = Rect {
            x: 0,
            y: 0,
            width: 24,
            height: 8,
        };
        let mut b = Buffer::empty(r);
        textarea.render(r, &mut b);

        textarea.move_cursor(CursorMove::Bottom);
        assert_eq!(textarea.cursor(), (19, 0));

        textarea.move_cursor(CursorMove::InViewport);
        assert_eq!(textarea.cursor(), (7, 0));
    }
}
