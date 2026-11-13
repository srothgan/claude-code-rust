use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::Widget as _;
use tui_textarea::{CursorMove, TextArea, WrapMode};

fn cell_symbol(cell: &ratatui::buffer::Cell) -> &str {
    cell.symbol()
}

fn render_lines(textarea: &TextArea<'_>, width: u16, height: u16) -> Vec<String> {
    let area = Rect {
        x: 0,
        y: 0,
        width,
        height,
    };
    let mut buf = Buffer::empty(area);
    textarea.render(area, &mut buf);

    (0..height)
        .map(|y| {
            let mut line = String::new();
            for x in 0..width {
                line.push_str(cell_symbol(&buf[(x, y)]));
            }
            line
        })
        .collect()
}

fn render_buffer(textarea: &TextArea<'_>, width: u16, height: u16) -> Buffer {
    let area = Rect {
        x: 0,
        y: 0,
        width,
        height,
    };
    let mut buf = Buffer::empty(area);
    textarea.render(area, &mut buf);
    buf
}

#[test]
fn wrap_mode_default_is_none() {
    let textarea = TextArea::default();
    assert_eq!(textarea.wrap_mode(), WrapMode::None);
}

#[test]
fn none_mode_keeps_horizontal_behavior() {
    let textarea = TextArea::from(["abcdef"]);
    let lines = render_lines(&textarea, 5, 2);
    assert_eq!(lines, vec!["abcde".to_string(), "     ".to_string()]);
}

#[test]
fn word_or_glyph_mode_soft_wraps() {
    let mut textarea = TextArea::from(["abcdef"]);
    textarea.set_wrap_mode(WrapMode::WordOrGlyph);
    let lines = render_lines(&textarea, 5, 2);
    assert_eq!(lines, vec!["abcd ".to_string(), "ef   ".to_string()]);
}

#[test]
fn word_mode_splits_oversized_word_to_keep_all_text_visible() {
    let mut textarea = TextArea::from(["abcdefgh"]);
    textarea.set_wrap_mode(WrapMode::Word);
    let lines = render_lines(&textarea, 5, 2);
    assert_eq!(lines, vec!["abcd ".to_string(), "efgh ".to_string()]);
}

#[test]
fn word_mode_wraps_with_newlines() {
    let mut textarea = TextArea::from(["hello world", "again here"]);
    textarea.set_wrap_mode(WrapMode::Word);

    let lines = render_lines(&textarea, 6, 4);
    assert_eq!(
        lines,
        vec![
            "hello ".to_string(),
            "world ".to_string(),
            "again ".to_string(),
            "here  ".to_string(),
        ]
    );
}

#[test]
fn none_mode_long_multiline_text_stays_unwrapped() {
    let textarea = TextArea::from(["first very long line", "second very long line"]);
    let lines = render_lines(&textarea, 8, 4);
    assert_eq!(
        lines,
        vec![
            "first ve".to_string(),
            "second v".to_string(),
            "        ".to_string(),
            "        ".to_string(),
        ]
    );
}

#[test]
fn word_mode_long_paragraph_wraps_by_words() {
    let mut textarea = TextArea::from(["lorem ipsum dolor sit amet consectetur adipiscing elit"]);
    textarea.set_wrap_mode(WrapMode::Word);
    let lines = render_lines(&textarea, 12, 6);
    assert_eq!(
        lines,
        vec![
            "lorem ipsum ".to_string(),
            "dolor sit   ".to_string(),
            "amet        ".to_string(),
            "consectetur ".to_string(),
            "adipiscing  ".to_string(),
            "elit        ".to_string(),
        ]
    );
}

#[test]
fn glyph_mode_splits_long_token() {
    let mut textarea = TextArea::from(["0123456789abcdefghijklmnopqrstuvwxyz"]);
    textarea.set_wrap_mode(WrapMode::Glyph);
    let lines = render_lines(&textarea, 11, 4);
    assert_eq!(
        lines,
        vec![
            "0123456789 ".to_string(),
            "abcdefghij ".to_string(),
            "klmnopqrst ".to_string(),
            "uvwxyz     ".to_string(),
        ]
    );
}

#[test]
fn word_and_word_or_glyph_are_compatible_for_long_words() {
    let text = "alpha supercalifragilisticexpialidocious omega";

    let mut word = TextArea::from([text]);
    word.set_wrap_mode(WrapMode::Word);
    let word_lines = render_lines(&word, 10, 7);

    let mut word_or_glyph = TextArea::from([text]);
    word_or_glyph.set_wrap_mode(WrapMode::WordOrGlyph);
    let word_or_glyph_lines = render_lines(&word_or_glyph, 10, 7);
    assert_eq!(word_lines, word_or_glyph_lines);
    assert_eq!(
        word_lines,
        vec![
            "alpha     ".to_string(),
            "supercali ".to_string(),
            "fragilist ".to_string(),
            "icexpiali ".to_string(),
            "docious   ".to_string(),
            " omega    ".to_string(),
            "          ".to_string(),
        ]
    );
}

#[test]
fn wrapped_mode_line_numbers_on_continuation_rows() {
    let mut textarea = TextArea::from(["abcdefghijk", "xy"]);
    textarea.set_wrap_mode(WrapMode::WordOrGlyph);
    textarea.set_line_number_style(Style::default());

    let lines = render_lines(&textarea, 9, 4);
    assert_eq!(
        lines,
        vec![
            " 1 abcde ".to_string(),
            "   fghij ".to_string(),
            "   k     ".to_string(),
            " 2 xy    ".to_string(),
        ]
    );
}

#[test]
fn wrapped_mode_handles_tiny_width() {
    let mut textarea = TextArea::from(["abcd"]);
    textarea.set_wrap_mode(WrapMode::WordOrGlyph);

    let lines = render_lines(&textarea, 1, 4);
    assert_eq!(
        lines,
        vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
        ]
    );
}

#[test]
fn wrapped_mode_preserves_empty_logical_lines() {
    let mut textarea = TextArea::from(["ab", "", "cd"]);
    textarea.set_wrap_mode(WrapMode::WordOrGlyph);

    let lines = render_lines(&textarea, 3, 3);
    assert_eq!(
        lines,
        vec!["ab ".to_string(), "   ".to_string(), "cd ".to_string(),]
    );
}

#[test]
fn glyph_mode_combining_grapheme_renders_in_two_rows() {
    let mut textarea = TextArea::from(["e\u{301}x"]);
    textarea.set_wrap_mode(WrapMode::Glyph);

    let lines = render_lines(&textarea, 1, 2);
    assert_eq!(lines, vec!["e".to_string(), "x".to_string()]);
}

#[test]
fn wrapped_mode_scrolls_to_cursor_visual_row() {
    let mut textarea = TextArea::from(["abcdefghijklmnopqrstuvwxyz"]);
    textarea.set_wrap_mode(WrapMode::WordOrGlyph);
    textarea.move_cursor(CursorMove::End);

    let lines = render_lines(&textarea, 6, 2);
    assert_eq!(lines, vec!["uvwxy ".to_string(), "z     ".to_string()]);
}

#[test]
fn wrapped_mode_with_tab_does_not_hide_following_text() {
    let mut textarea = TextArea::from(["\tX"]);
    textarea.set_wrap_mode(WrapMode::WordOrGlyph);
    let lines = render_lines(&textarea, 2, 2);
    assert_eq!(lines, vec!["  ".to_string(), "X ".to_string()]);
}

#[test]
fn cursor_line_style_applies_to_wrapped_continuation_rows() {
    let mut textarea = TextArea::from(["abcdefghij"]);
    textarea.set_wrap_mode(WrapMode::WordOrGlyph);
    let style = Style::default().bg(Color::Red);
    textarea.set_cursor_line_style(style);

    let buf = render_buffer(&textarea, 5, 2);
    assert_eq!(buf[(1, 1)].style().bg, style.bg);
}

#[test]
fn selection_does_not_extend_with_synthetic_space_on_wrap_boundary() {
    let mut textarea = TextArea::from(["hello world again"]);
    textarea.set_wrap_mode(WrapMode::Word);
    textarea.set_cursor_line_style(Style::default());
    let select_style = Style::default().bg(Color::Blue);
    textarea.set_selection_style(select_style);
    textarea.start_selection();
    textarea.move_cursor(CursorMove::End);

    let buf = render_buffer(&textarea, 10, 3);
    assert_eq!(buf[(5, 0)].style().bg, select_style.bg);
    assert_ne!(buf[(6, 0)].style().bg, select_style.bg);
}

// --- Wrapped line cursor navigation tests ---

fn render(textarea: &TextArea<'_>, width: u16, height: u16) {
    let area = Rect {
        x: 0,
        y: 0,
        width,
        height,
    };
    let mut buf = Buffer::empty(area);
    textarea.render(area, &mut buf);
}

#[test]
fn wrapped_cursor_down_moves_within_same_logical_line() {
    // "abcdefghij" at width=5 wraps into 2 visual lines:
    //   visual 0: "abcde"  (row 0, cols 0..5)
    //   visual 1: "fghij"  (row 0, cols 5..10)
    let mut textarea = TextArea::from(["abcdefghij"]);
    textarea.set_wrap_mode(WrapMode::WordOrGlyph);
    render(&textarea, 6, 4);

    // Cursor starts at (0, 0) — top of visual line 0
    assert_eq!(textarea.cursor(), (0, 0));

    // Down should move to the second visual line within the same logical line
    textarea.move_cursor(CursorMove::Down);
    assert_eq!(textarea.cursor(), (0, 5));

    // Down again should not move (already at last visual row)
    textarea.move_cursor(CursorMove::Down);
    assert_eq!(textarea.cursor(), (0, 5));
}

#[test]
fn wrapped_cursor_up_moves_within_same_logical_line() {
    // "abcdefghij" at width=5 wraps into 2 visual lines:
    //   visual 0: cols 0..4  (end_col=5 exclusive for non-last)
    //   visual 1: cols 5..10
    let mut textarea = TextArea::from(["abcdefghij"]);
    textarea.set_wrap_mode(WrapMode::WordOrGlyph);
    render(&textarea, 6, 4);

    // Move to end of text (row 0, col 10) which is on visual line 1
    textarea.move_cursor(CursorMove::End);
    assert_eq!(textarea.cursor(), (0, 10));

    // Up should move to visual line 0, col clamped to max of visual line 0 (col 4)
    textarea.move_cursor(CursorMove::Up);
    assert_eq!(textarea.cursor(), (0, 4));

    // Up again should not move (already at first visual row)
    textarea.move_cursor(CursorMove::Up);
    assert_eq!(textarea.cursor(), (0, 4));
}

#[test]
fn wrapped_cursor_down_crosses_logical_line_boundary() {
    // Two logical lines, first one wraps:
    //   visual 0: "abcde"  (row 0, cols 0..4)
    //   visual 1: "fghij"  (row 0, cols 5..10)
    //   visual 2: "xy"     (row 1, cols 0..2)
    let mut textarea = TextArea::from(["abcdefghij", "xy"]);
    textarea.set_wrap_mode(WrapMode::WordOrGlyph);
    render(&textarea, 6, 4);

    assert_eq!(textarea.cursor(), (0, 0));

    // Down: visual 0 → visual 1
    textarea.move_cursor(CursorMove::Down);
    assert_eq!(textarea.cursor(), (0, 5));

    // Down: visual 1 → visual 2 (crosses into second logical line, visual offset 0 preserved)
    textarea.move_cursor(CursorMove::Down);
    assert_eq!(textarea.cursor(), (1, 0));

    // Down: already at bottom
    textarea.move_cursor(CursorMove::Down);
    assert_eq!(textarea.cursor(), (1, 0));
}

#[test]
fn wrapped_cursor_up_crosses_logical_line_boundary() {
    // visual 0: "abcde"  (row 0, cols 0..4)
    // visual 1: "fghij"  (row 0, cols 5..10)
    // visual 2: "xy"     (row 1, cols 0..2)
    let mut textarea = TextArea::from(["abcdefghij", "xy"]);
    textarea.set_wrap_mode(WrapMode::WordOrGlyph);
    render(&textarea, 6, 4);

    // Start at second logical line
    textarea.move_cursor(CursorMove::Jump(1, 0));
    assert_eq!(textarea.cursor(), (1, 0));

    // Up: visual 2 → visual 1 (visual offset 0 → start of visual row 1)
    textarea.move_cursor(CursorMove::Up);
    assert_eq!(textarea.cursor(), (0, 5));

    // Up: visual 1 → visual 0 (visual offset 0 preserved → col 0)
    textarea.move_cursor(CursorMove::Up);
    assert_eq!(textarea.cursor(), (0, 0));

    // Up: already at top
    textarea.move_cursor(CursorMove::Up);
    assert_eq!(textarea.cursor(), (0, 0));
}

#[test]
fn wrapped_cursor_column_preserved_across_visual_lines() {
    // "abcdefghij" at width=5:
    //   visual 0: "abcde" (cols 0..4)
    //   visual 1: "fghij" (cols 5..10)
    let mut textarea = TextArea::from(["abcdefghij"]);
    textarea.set_wrap_mode(WrapMode::WordOrGlyph);
    render(&textarea, 6, 4);

    // Start at col 2 on visual line 0
    textarea.move_cursor(CursorMove::Jump(0, 2));
    assert_eq!(textarea.cursor(), (0, 2));

    // Down: visual offset 2 preserved → target col 5+2 = 7
    textarea.move_cursor(CursorMove::Down);
    assert_eq!(textarea.cursor(), (0, 7));
}

#[test]
fn wrapped_cursor_column_clamped_to_shorter_visual_line() {
    // "abcdefgh" + "xy" at width=5:
    //   visual 0: "abcde" (row 0, cols 0..4)
    //   visual 1: "fgh"   (row 0, cols 5..8)
    //   visual 2: "xy"    (row 1, cols 0..2)
    let mut textarea = TextArea::from(["abcdefgh", "xy"]);
    textarea.set_wrap_mode(WrapMode::WordOrGlyph);
    render(&textarea, 6, 4);

    // Start at col 4 on visual line 0
    textarea.move_cursor(CursorMove::Jump(0, 4));
    assert_eq!(textarea.cursor(), (0, 4));

    // Down: visual offset 4 → target col 5+4 = 9, clamped to line len 8
    textarea.move_cursor(CursorMove::Down);
    assert_eq!(textarea.cursor(), (0, 8));
}

#[test]
fn wrapped_cursor_down_preserves_visual_column_for_mixed_width_same_logical_line() {
    let mut textarea = TextArea::from(["a中bcde"]);
    textarea.set_wrap_mode(WrapMode::Glyph);
    render(&textarea, 5, 4);

    // After `a中`, the data column is 2 but the visual column is 3.
    textarea.move_cursor(CursorMove::Jump(0, 2));
    assert_eq!(textarea.cursor(), (0, 2));
    textarea.move_cursor(CursorMove::Down);
    assert_eq!(textarea.cursor(), (0, 6));
}

#[test]
fn wrapped_cursor_up_preserves_visual_column_for_mixed_width_same_logical_line() {
    let mut textarea = TextArea::from(["a中bcde"]);
    textarea.set_wrap_mode(WrapMode::Glyph);
    render(&textarea, 5, 4);

    textarea.move_cursor(CursorMove::Jump(0, 6));
    assert_eq!(textarea.cursor(), (0, 6));
    textarea.move_cursor(CursorMove::Up);
    assert_eq!(textarea.cursor(), (0, 2));
}

#[test]
fn wrapped_cursor_down_preserves_visual_column_when_crossing_into_mixed_width_line() {
    let mut textarea = TextArea::from(["abcd", "a中bcde"]);
    textarea.set_wrap_mode(WrapMode::Glyph);
    render(&textarea, 5, 4);

    // Visual column 3 in ASCII should land at visual column 3 in the mixed-width row.
    textarea.move_cursor(CursorMove::Jump(0, 3));
    assert_eq!(textarea.cursor(), (0, 3));
    textarea.move_cursor(CursorMove::Down);
    assert_eq!(textarea.cursor(), (1, 2));
}

#[test]
fn wrapped_cursor_up_preserves_visual_column_when_crossing_out_of_mixed_width_line() {
    let mut textarea = TextArea::from(["abcd", "a中bcde"]);
    textarea.set_wrap_mode(WrapMode::Glyph);
    render(&textarea, 5, 4);

    textarea.move_cursor(CursorMove::Jump(1, 2));
    assert_eq!(textarea.cursor(), (1, 2));
    textarea.move_cursor(CursorMove::Up);
    assert_eq!(textarea.cursor(), (0, 3));
}

#[test]
fn wrapped_cursor_down_up_with_word_wrap_mode() {
    // "hello world" at width=6 with Word wrap:
    //   visual 0: "hello " (row 0, cols 0..5 for non-last)
    //   visual 1: "world"  (row 0, cols 6..11)
    let mut textarea = TextArea::from(["hello world"]);
    textarea.set_wrap_mode(WrapMode::Word);
    render(&textarea, 6, 4);

    assert_eq!(textarea.cursor(), (0, 0));

    textarea.move_cursor(CursorMove::Down);
    assert_eq!(textarea.cursor(), (0, 6));

    // Up: visual offset 0 preserved → col 0
    textarea.move_cursor(CursorMove::Up);
    assert_eq!(textarea.cursor(), (0, 0));
}

#[test]
fn wrapped_cursor_no_wrap_short_lines_behave_normally() {
    // Lines fit within width — no wrapping happens, normal Up/Down
    let mut textarea = TextArea::from(["abc", "def", "ghi"]);
    textarea.set_wrap_mode(WrapMode::WordOrGlyph);
    render(&textarea, 10, 4);

    assert_eq!(textarea.cursor(), (0, 0));
    textarea.move_cursor(CursorMove::Down);
    assert_eq!(textarea.cursor(), (1, 0));
    textarea.move_cursor(CursorMove::Down);
    assert_eq!(textarea.cursor(), (2, 0));
    textarea.move_cursor(CursorMove::Up);
    assert_eq!(textarea.cursor(), (1, 0));
    textarea.move_cursor(CursorMove::Up);
    assert_eq!(textarea.cursor(), (0, 0));
}

#[test]
fn wrapped_cursor_three_visual_lines_from_one_logical() {
    // "abcdefghijklmno" at width=5:
    //   visual 0: "abcde" (cols 0..4)
    //   visual 1: "fghij" (cols 5..9)
    //   visual 2: "klmno" (cols 10..15)
    let mut textarea = TextArea::from(["abcdefghijklmno"]);
    textarea.set_wrap_mode(WrapMode::WordOrGlyph);
    render(&textarea, 6, 4);

    assert_eq!(textarea.cursor(), (0, 0));

    textarea.move_cursor(CursorMove::Down);
    assert_eq!(textarea.cursor(), (0, 5));

    textarea.move_cursor(CursorMove::Down);
    assert_eq!(textarea.cursor(), (0, 10));

    textarea.move_cursor(CursorMove::Down);
    assert_eq!(textarea.cursor(), (0, 10)); // already at bottom

    // Up: visual offset 0 preserved → col 5
    textarea.move_cursor(CursorMove::Up);
    assert_eq!(textarea.cursor(), (0, 5));

    // Up: visual offset 0 preserved → col 0
    textarea.move_cursor(CursorMove::Up);
    assert_eq!(textarea.cursor(), (0, 0));
}

#[test]
fn viewport_growth_and_reflow_restore_source_without_resetting_edits() {
    for (width, height) in [(12, 1), (4, 2)] {
        let mut textarea = TextArea::from(["FIRST", "LAST"]);
        textarea.set_wrap_mode(WrapMode::WordOrGlyph);
        textarea.move_cursor(CursorMove::Bottom);
        textarea.move_cursor(CursorMove::End);
        textarea.insert_char('!');
        let cursor = textarea.cursor();
        let _ = render_lines(&textarea, width, height);
        let expanded = render_lines(&textarea, 12, 4);
        assert_eq!(
            expanded.iter().map(|line| line.trim_end()).collect::<Vec<_>>(),
            ["FIRST", "LAST!", "", ""]
        );
        assert_eq!(textarea.cursor(), cursor);
        assert_eq!(textarea.lines(), ["FIRST", "LAST!"]);
        assert!(textarea.undo());
        assert_eq!(textarea.lines(), ["FIRST", "LAST"]);
        assert_eq!(textarea.cursor(), (1, 4));
    }
}
