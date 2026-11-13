use crate::cursor::{DataCursor, ScreenCursor};
use crate::textarea::TextArea;
use crate::wrap::{WrappedLine, wrapped_rows};
use unicode_width::UnicodeWidthChar;

#[derive(Debug, Clone, Copy)]
pub(crate) struct ScreenLine {
    pub wrapped: WrappedLine,
    pub screen_width: usize,
    pub cursor_max_col: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DataLine {
    pub first_screen_line: usize,
    pub screen_line_count: usize,
    pub pure_ascii: bool,
}

fn is_pure_ascii(line: &str) -> bool {
    line.is_ascii() && !line.contains('\t')
}

fn char_display_width(c: char, col: usize, tab_len: u8) -> usize {
    if c == '\t' {
        let tab = tab_len.max(1) as usize;
        let pad = tab - (col % tab);
        pad.max(1)
    } else {
        c.width().unwrap_or(0)
    }
}

fn display_width(text: &str, tab_len: u8) -> usize {
    let mut col = 0usize;
    for c in text.chars() {
        col += char_display_width(c, col, tab_len);
    }
    col
}

fn screen_col_for_char_offset(text: &str, char_offset: usize, tab_len: u8) -> usize {
    let mut col = 0usize;
    for c in text.chars().take(char_offset) {
        col += char_display_width(c, col, tab_len);
    }
    col
}

fn char_offset_for_screen_col(text: &str, screen_col: usize, tab_len: u8) -> usize {
    let mut col = 0usize;
    let mut chars = 0usize;
    for c in text.chars() {
        if col >= screen_col {
            break;
        }
        let width = char_display_width(c, col, tab_len);
        if col + width > screen_col {
            break;
        }
        col += width;
        chars += 1;
    }
    chars
}

impl TextArea<'_> {
    fn fallback_rows(&self) -> Vec<WrappedLine> {
        self.lines
            .iter()
            .enumerate()
            .map(|(row, line)| WrappedLine {
                row,
                start_byte: 0,
                end_byte: line.len(),
                start_col: 0,
                end_col: line.chars().count(),
                first_in_row: true,
                last_in_row: true,
            })
            .collect()
    }

    pub(crate) fn screen_map_load(&self) {
        let width = self.area.get().width;
        let wrap_width = (width > 0)
            .then(|| self.layout_metrics(width).wrap_width())
            .flatten();

        let rows = match wrap_width {
            Some(width) => wrapped_rows(&self.lines, self.wrap_mode(), width, self.tab_length()),
            None => self.fallback_rows(),
        };

        let mut screen_lines = Vec::with_capacity(rows.len());
        let mut data_pointers = vec![DataLine::default(); self.lines.len()];

        for (screen_idx, wrapped) in rows.into_iter().enumerate() {
            if data_pointers[wrapped.row].screen_line_count == 0 {
                data_pointers[wrapped.row].first_screen_line = screen_idx;
                data_pointers[wrapped.row].pure_ascii = is_pure_ascii(&self.lines[wrapped.row]);
            }
            data_pointers[wrapped.row].screen_line_count += 1;

            let fragment = &self.lines[wrapped.row][wrapped.start_byte..wrapped.end_byte];
            let char_count = wrapped.end_col.saturating_sub(wrapped.start_col);
            let cursor_max_col = if wrapped.last_in_row {
                display_width(fragment, self.tab_length())
            } else {
                screen_col_for_char_offset(
                    fragment,
                    char_count.saturating_sub(1),
                    self.tab_length(),
                )
            };
            screen_lines.push(ScreenLine {
                wrapped,
                screen_width: display_width(fragment, self.tab_length()),
                cursor_max_col,
            });
        }

        *self.screen_lines.borrow_mut() = screen_lines;
        *self.data_pointers.borrow_mut() = data_pointers;
    }

    pub(crate) fn screen_lines_count(&self) -> usize {
        self.screen_lines.borrow().len()
    }

    pub(crate) fn screen_line_width(&self, row: usize) -> usize {
        self.screen_lines.borrow()[row].screen_width
    }

    pub(crate) fn screen_line_max_cursor_col(&self, row: usize) -> usize {
        self.screen_lines.borrow()[row].cursor_max_col
    }

    pub(crate) fn screen_line(&self, row: usize) -> ScreenLine {
        self.screen_lines.borrow()[row]
    }

    pub(crate) fn screen_to_data_cursor(&self, screen: ScreenCursor) -> DataCursor {
        if let Some(dc) = screen.dc {
            return dc;
        }

        let line = self.screen_line(screen.row);
        let fragment =
            &self.lines[line.wrapped.row][line.wrapped.start_byte..line.wrapped.end_byte];
        let char_offset = char_offset_for_screen_col(fragment, screen.col, self.tab_length());
        DataCursor(line.wrapped.row, line.wrapped.start_col + char_offset)
    }

    pub(crate) fn data_to_screen_cursor(&self, data: DataCursor) -> ScreenCursor {
        let data_line = &self.data_pointers.borrow()[data.0];
        let screen_lines = self.screen_lines.borrow();

        let mut screen_idx = data_line.first_screen_line;
        for idx in
            data_line.first_screen_line..data_line.first_screen_line + data_line.screen_line_count
        {
            let line = screen_lines[idx];
            let contains = if line.wrapped.last_in_row {
                line.wrapped.start_col <= data.1 && data.1 <= line.wrapped.end_col
            } else {
                line.wrapped.start_col <= data.1 && data.1 < line.wrapped.end_col
            };
            if contains {
                screen_idx = idx;
                break;
            }
        }

        let line = screen_lines[screen_idx];
        let fragment = &self.lines[data.0][line.wrapped.start_byte..line.wrapped.end_byte];
        let char_offset = data.1.saturating_sub(line.wrapped.start_col);
        let col = screen_col_for_char_offset(fragment, char_offset, self.tab_length());

        ScreenCursor {
            row: screen_idx,
            col,
            char: self.lines[data.0].chars().nth(data.1),
            dc: Some(data),
        }
    }

    pub(crate) fn increment_screen_cursor(&self, screen: ScreenCursor) -> ScreenCursor {
        let dc = screen.to_data_cursor(self);
        self.data_to_screen_cursor(DataCursor(dc.0, dc.1 + 1))
    }

    pub(crate) fn decrement_screen_cursor(&self, screen: ScreenCursor) -> ScreenCursor {
        let dc = screen.to_data_cursor(self);
        self.data_to_screen_cursor(DataCursor(dc.0, dc.1 - 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ratatui::layout::Rect;
    use crate::wrap::WrapMode;

    fn make_textarea(lines: &[&str], wrap_mode: WrapMode, width: u16) -> TextArea<'static> {
        let mut textarea = TextArea::from(lines.iter().copied());
        textarea.set_wrap_mode(wrap_mode);
        textarea.area.set(Rect {
            x: 0,
            y: 0,
            width,
            height: 40,
        });
        textarea.screen_map_load();
        textarea
    }

    fn assert_round_trips(textarea: &TextArea<'_>) {
        for (row, line) in textarea.lines.iter().enumerate() {
            for col in 0..=line.chars().count() {
                let dc = DataCursor(row, col);
                let sc = textarea.data_to_screen_cursor(dc);
                assert_eq!(textarea.screen_to_data_cursor(sc), dc);

                let detached = ScreenCursor { dc: None, ..sc };
                assert_eq!(textarea.screen_to_data_cursor(detached), dc);
                assert_eq!(
                    textarea.data_to_screen_cursor(textarea.screen_to_data_cursor(detached)),
                    sc
                );
            }
        }
    }

    #[test]
    fn screen_map_round_trips_ascii_word_wrap() {
        let textarea = make_textarea(
            &[
                "Lorem ipsum dolor sit amet, consectetur adipiscing elit.",
                "Sed do eiusmod tempor incididunt ut labore et dolore magna aliqua.",
            ],
            WrapMode::Word,
            24,
        );

        assert!(textarea.screen_lines_count() > textarea.lines.len());
        assert_round_trips(&textarea);
    }

    #[test]
    fn screen_map_round_trips_cjk_glyph_wrap() {
        let textarea = make_textarea(
            &["混合中文字符和ASCIIabc来验证屏幕坐标与数据坐标的往返转换。"],
            WrapMode::Glyph,
            12,
        );

        assert!(textarea.screen_lines_count() > textarea.lines.len());
        assert_round_trips(&textarea);
    }

    #[test]
    fn screen_map_round_trips_multilingual_word_or_glyph_wrap() {
        let textarea = make_textarea(
            &[
                "ASCII and русский текст can share one buffer.",
                "第二行 mixed-width 内容 with English words.",
            ],
            WrapMode::WordOrGlyph,
            18,
        );

        assert!(textarea.screen_lines_count() > textarea.lines.len());
        assert_round_trips(&textarea);
    }
}
