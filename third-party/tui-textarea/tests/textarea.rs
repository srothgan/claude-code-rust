use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Widget as _;
use ratatui::widgets::{Block, Borders};
use std::cmp;
use std::fmt::Debug;
use tui_textarea::{CursorMove, CursorRenderMode, TextArea, WrapMode};

fn assert_undo_redo<T: Debug>(
    before_pos: (usize, usize),
    before_buf: &[&str],
    after_buf: &[&str],
    t: &mut TextArea<'_>,
    context: T,
) {
    let after_pos = t.cursor();
    let modified = before_buf != after_buf;
    assert_eq!(t.cursor(), after_pos, "pos before undo: {context:?}");
    assert_eq!(t.undo(), modified, "undo modification: {context:?}");
    assert_eq!(t.lines(), before_buf, "buf after undo: {context:?}");
    assert_eq!(t.cursor(), before_pos, "pos after undo: {context:?}");
    assert_eq!(t.redo(), modified, "redo modification: {context:?}");
    assert_eq!(t.lines(), after_buf, "buf after redo: {context:?}");
    assert_eq!(t.cursor(), after_pos, "pos after redo: {context:?}");
}

fn assert_no_undo_redo<T: Debug>(t: &mut TextArea<'_>, context: T) {
    let pos = t.cursor();
    let buf: Vec<_> = t.lines().to_vec();
    assert!(!t.undo(), "undo modification: {context:?}");
    assert_eq!(t.lines(), &buf, "buf after undo: {context:?}");
    assert_eq!(t.cursor(), pos, "pos after undo: {context:?}");
    assert!(!t.redo(), "redo modification: {context:?}");
    assert_eq!(t.lines(), &buf, "buf after redo: {context:?}");
    assert_eq!(t.cursor(), pos, "pos after redo: {context:?}");
}

fn render_textarea(textarea: &TextArea<'_>, area: Rect) -> Buffer {
    let mut buf = Buffer::empty(area);
    textarea.render(area, &mut buf);
    buf
}

#[test]
fn test_insert_soft_tab() {
    for test in [
        ("", 0, "    ", 4),
        ("a", 1, "a   ", 3),
        ("abcd", 4, "abcd    ", 4),
        ("a", 0, "    a", 4),
        ("ab", 1, "a   b", 3),
        ("abcdefgh", 4, "abcd    efgh", 4),
        ("あ", 1, "あ  ", 2),
        ("🐶", 1, "🐶  ", 2),
        ("あ", 0, "    あ", 4),
        ("あい", 1, "あ  い", 2),
    ] {
        let (input, col, expected, width) = test;
        let mut t = TextArea::from([input.to_string()]);
        t.move_cursor(CursorMove::Jump(0, col));
        assert!(t.insert_tab(), "{test:?}");
        assert_eq!(t.lines(), [expected], "{test:?}");
        assert_eq!(t.cursor(), (0, col as usize + width), "{test:?}");
        assert_undo_redo((0, col as _), &[input], &[expected], &mut t, test);
    }
}

#[test]
fn test_insert_hard_tab() {
    let mut t = TextArea::default();
    t.set_hard_tab_indent(true);
    assert!(t.insert_tab());
    assert_eq!(t.cursor(), (0, 1));
    assert_undo_redo((0, 0), &[""], &["\t"], &mut t, "");

    let mut t = TextArea::default();
    t.set_hard_tab_indent(true);
    t.set_tab_length(0);
    t.insert_tab();
    assert!(!t.insert_tab());
    assert_eq!(t.lines(), [""]);
    assert_eq!(t.cursor(), (0, 0));
}

#[test]
fn test_insert_char() {
    let tests = [
        (0, 'x', &["xab"][..]),
        (1, 'x', &["axb"][..]),
        (2, 'x', &["abx"][..]),
        (1, 'あ', &["aあb"][..]),
        (1, '\n', &["a", "b"][..]),
    ];

    for test in tests {
        let (col, ch, want) = test;
        let mut t = TextArea::from(["ab"]);
        t.move_cursor(CursorMove::Jump(0, col));
        t.insert_char(ch);
        assert_eq!(t.lines(), want, "{test:?}");
        let pos = if ch == '\n' {
            (1, 0)
        } else {
            (0, col as usize + 1)
        };
        assert_eq!(t.cursor(), pos, "{test:?}");
        assert_undo_redo((0, col as _), &["ab"], want, &mut t, test);
    }
}

#[test]
fn test_insert_str_one_line() {
    for i in 0..="ab".len() {
        let mut t = TextArea::from(["ab"]);
        t.move_cursor(CursorMove::Jump(0, i as u16));
        assert!(t.insert_str("x"), "{i}");

        let mut want = "ab".to_string();
        want.insert(i, 'x');
        let want = want.as_str();
        assert_eq!(t.lines(), [want], "{i}");
        assert_eq!(t.cursor(), (0, i + 1));
        assert_undo_redo((0, i), &["ab"], &[want], &mut t, i);
    }

    let mut t = TextArea::default();
    assert!(t.insert_str("x"));
    assert_eq!(t.cursor(), (0, 1));
    assert_undo_redo((0, 0), &[""], &["x"], &mut t, "");
}

#[test]
fn test_insert_str_empty_line() {
    let mut t = TextArea::from(["ab"]);
    assert!(!t.insert_str(""));
    assert_eq!(t.lines(), ["ab"]);
    assert_eq!(t.cursor(), (0, 0));
    assert_no_undo_redo(&mut t, "");
}

#[test]
fn test_insert_str_multiple_lines() {
    #[rustfmt::skip]
    let tests = [
        // Positions
        (
            // Text before edit
            &[
                "ab",
                "cd",
                "ef",
            ][..],
            // (row, offset) position before edit
            (0, 0),
            // String to be inserted
            "x\ny",
            // (row, offset) position after edit
            (1, 1),
            // Text after edit
            &[
                "x",
                "yab",
                "cd",
                "ef",
            ][..],
        ),
        (
            &[
                "ab",
                "cd",
                "ef",
            ][..],
            (0, 1),
            "x\ny",
            (1, 1),
            &[
                "ax",
                "yb",
                "cd",
                "ef",
            ][..],
        ),
        (
            &[
                "ab",
                "cd",
                "ef",
            ][..],
            (0, 2),
            "x\ny",
            (1, 1),
            &[
                "abx",
                "y",
                "cd",
                "ef",
            ][..],
        ),
        (
            &[
                "ab",
                "cd",
                "ef",
            ][..],
            (1, 0),
            "x\ny",
            (2, 1),
            &[
                "ab",
                "x",
                "ycd",
                "ef",
            ][..],
        ),
        (
            &[
                "ab",
                "cd",
                "ef",
            ][..],
            (1, 1),
            "x\ny",
            (2, 1),
            &[
                "ab",
                "cx",
                "yd",
                "ef",
            ][..],
        ),
        (
            &[
                "ab",
                "cd",
                "ef",
            ][..],
            (1, 2),
            "x\ny",
            (2, 1),
            &[
                "ab",
                "cdx",
                "y",
                "ef",
            ][..],
        ),
        (
            &[
                "ab",
                "cd",
                "ef",
            ][..],
            (2, 0),
            "x\ny",
            (3, 1),
            &[
                "ab",
                "cd",
                "x",
                "yef",
            ][..],
        ),
        (
            &[
                "ab",
                "cd",
                "ef",
            ][..],
            (2, 1),
            "x\ny",
            (3, 1),
            &[
                "ab",
                "cd",
                "ex",
                "yf",
            ][..],
        ),
        (
            &[
                "ab",
                "cd",
                "ef",
            ][..],
            (2, 2),
            "x\ny",
            (3, 1),
            &[
                "ab",
                "cd",
                "efx",
                "y",
            ][..],
        ),
        // More than 2 lines
        (
            &[
                "ab",
                "cd",
                "ef",
            ][..],
            (1, 1),
            "x\ny\nz\nw",
            (4, 1),
            &[
                "ab",
                "cx",
                "y",
                "z",
                "wd",
                "ef",
            ][..],
        ),
        // Newline at end of line
        (
            &[
                "ab",
                "cd",
                "ef",
            ][..],
            (1, 1),
            "x\ny\n",
            (3, 0),
            &[
                "ab",
                "cx",
                "y",
                "d",
                "ef",
            ][..],
        ),
        // Empty lines
        (
            &[
                "",
                "",
                "",
            ][..],
            (0, 0),
            "x\ny\nz",
            (2, 1),
            &[
                "x",
                "y",
                "z",
                "",
                "",
            ][..],
        ),
        (
            &[
                "",
                "",
                "",
            ][..],
            (1, 0),
            "x\ny\nz",
            (3, 1),
            &[
                "",
                "x",
                "y",
                "z",
                "",
            ][..],
        ),
        (
            &[
                "",
                "",
                "",
            ][..],
            (2, 0),
            "x\ny\nz",
            (4, 1),
            &[
                "",
                "",
                "x",
                "y",
                "z",
            ][..],
        ),
        // Empty buffer
        (
            &[
                "",
            ][..],
            (0, 0),
            "x\ny\nz",
            (2, 1),
            &[
                "x",
                "y",
                "z",
            ][..],
        ),
        // Insert empty lines
        (
            &[
                "ab",
                "cd",
                "ef",
            ][..],
            (0, 0),
            "\n\n\n",
            (3, 0),
            &[
                "",
                "",
                "",
                "ab",
                "cd",
                "ef",
            ][..],
        ),
        (
            &[
                "ab",
                "cd",
                "ef",
            ][..],
            (1, 0),
            "\n\n\n",
            (4, 0),
            &[
                "ab",
                "",
                "",
                "",
                "cd",
                "ef",
            ][..],
        ),
        (
            &[
                "ab",
                "cd",
                "ef",
            ][..],
            (1, 1),
            "\n\n\n",
            (4, 0),
            &[
                "ab",
                "c",
                "",
                "",
                "d",
                "ef",
            ][..],
        ),
        (
            &[
                "ab",
                "cd",
                "ef",
            ][..],
            (1, 2),
            "\n\n\n",
            (4, 0),
            &[
                "ab",
                "cd",
                "",
                "",
                "",
                "ef",
            ][..],
        ),
        (
            &[
                "ab",
                "cd",
                "ef",
            ][..],
            (2, 2),
            "\n\n\n",
            (5, 0),
            &[
                "ab",
                "cd",
                "ef",
                "",
                "",
                "",
            ][..],
        ),
        // Multi-byte characters
        (
            &[
                "🐶🐱",
                "🐮🐰",
                "🐧🐭",
            ][..],
            (0, 0),
            "🐷\n🐼\n🐴",
            (2, 1),
            &[
                "🐷",
                "🐼",
                "🐴🐶🐱",
                "🐮🐰",
                "🐧🐭",
            ][..],
        ),
        (
            &[
                "🐶🐱",
                "🐮🐰",
                "🐧🐭",
            ][..],
            (0, 2),
            "🐷\n🐼\n🐴",
            (2, 1),
            &[
                "🐶🐱🐷",
                "🐼",
                "🐴",
                "🐮🐰",
                "🐧🐭",
            ][..],
        ),
        (
            &[
                "🐶🐱",
                "🐮🐰",
                "🐧🐭",
            ][..],
            (1, 0),
            "🐷\n🐼\n🐴",
            (3, 1),
            &[
                "🐶🐱",
                "🐷",
                "🐼",
                "🐴🐮🐰",
                "🐧🐭",
            ][..],
        ),
        (
            &[
                "🐶🐱",
                "🐮🐰",
                "🐧🐭",
            ][..],
            (1, 1),
            "🐷\n🐼\n🐴",
            (3, 1),
            &[
                "🐶🐱",
                "🐮🐷",
                "🐼",
                "🐴🐰",
                "🐧🐭",
            ][..],
        ),
        (
            &[
                "🐶🐱",
                "🐮🐰",
                "🐧🐭",
            ][..],
            (2, 2),
            "🐷\n🐼\n🐴",
            (4, 1),
            &[
                "🐶🐱",
                "🐮🐰",
                "🐧🐭🐷",
                "🐼",
                "🐴",
            ][..],
        ),
        // Handle \r\n as newlines
        (
            &[
                "ab",
                "cd",
                "ef",
            ][..],
            (1, 1),
            "x\r\ny\r\nz",
            (3, 1),
            &[
                "ab",
                "cx",
                "y",
                "zd",
                "ef",
            ][..],
        ),
        (
            &[
                "ab",
                "cd",
                "ef",
            ][..],
            (1, 1),
            "x\ny\r\nz",
            (3, 1),
            &[
                "ab",
                "cx",
                "y",
                "zd",
                "ef",
            ][..],
        ),
    ];

    for test in tests {
        let (before, before_pos, input, after_pos, expected) = test;

        let mut t = TextArea::from(before.iter().map(|s| s.to_string()));
        let (row, col) = before_pos;
        t.move_cursor(CursorMove::Jump(row as _, col as _));

        assert!(t.insert_str(input), "{test:?}");
        assert_eq!(t.lines(), expected, "{test:?}");
        assert_eq!(t.cursor(), after_pos, "{test:?}");

        assert_undo_redo(before_pos, before, expected, &mut t, test);
    }
}

#[test]
fn test_delete_str_nothing() {
    for i in 0..="ab".len() {
        let mut t = TextArea::from(["ab"]);
        assert!(!t.delete_str(0), "{i}");
        assert_eq!(t.cursor(), (0, 0));
    }
    let mut t = TextArea::default();
    assert!(!t.delete_str(0));
    assert_eq!(t.cursor(), (0, 0));
}

#[test]
fn test_delete_str_within_line() {
    for i in 0.."abc".len() {
        for j in 1..="abc".len() - i {
            let mut t = TextArea::from(["abc"]);
            t.move_cursor(CursorMove::Jump(0, i as _));
            assert!(t.delete_str(j), "at {i}, size={j}");

            let mut want = "abc".to_string();
            want.drain(i..i + j);
            let want = want.as_str();
            assert_eq!(t.lines(), [want], "at {i}, size={j}");
            assert_eq!(t.cursor(), (0, i));

            // delete_str deletes string as if moving cursor at the end of the deleted string
            assert_undo_redo((0, i + j), &["abc"], &[want], &mut t, (i, j));
        }
    }
}

#[test]
fn test_delete_str_multiple_lines() {
    #[rustfmt::skip]
    let tests = [
        // Length
        (
            // Text before edit
            &[
                "ab",
                "cd",
                "ef",
            ][..],
            // (row, offset) cursor position
            (0, 0),
            // Chars to be deleted
            3,
            // Deleted text
            "ab\n",
            // Text after edit
            &[
                "cd",
                "ef",
            ][..],
        ),
        (
            &[
                "ab",
                "cd",
                "ef",
            ][..],
            (0, 0),
            4,
            "ab\nc",
            &[
                "d",
                "ef",
            ][..],
        ),
        (
            &[
                "ab",
                "cd",
                "ef",
            ][..],
            (0, 0),
            5,
            "ab\ncd",
            &[
                "",
                "ef",
            ][..],
        ),
        (
            &[
                "ab",
                "cd",
                "ef",
            ][..],
            (0, 0),
            6,
            "ab\ncd\n",
            &[
                "ef",
            ][..],
        ),
        (
            &[
                "ab",
                "cd",
                "ef",
            ][..],
            (0, 0),
            7,
            "ab\ncd\ne",
            &[
                "f",
            ][..],
        ),
        (
            &[
                "ab",
                "cd",
                "ef",
            ][..],
            (0, 0),
            8,
            "ab\ncd\nef",
            &[
                "",
            ][..],
        ),
        (
            &[
                "ab",
                "cd",
                "ef",
            ][..],
            (0, 0),
            9,
            "ab\ncd\nef",
            &[
                "",
            ][..],
        ),
        // Positions
        (
            &[
                "ab",
                "cd",
                "ef",
            ][..],
            (0, 1),
            3,
            "b\nc",
            &[
                "ad",
                "ef",
            ][..],
        ),
        (
            &[
                "ab",
                "cd",
                "ef",
            ][..],
            (0, 2),
            4,
            "\ncd\n",
            &[
                "abef",
            ][..],
        ),
        (
            &[
                "ab",
                "cd",
                "ef",
            ][..],
            (1, 0),
            4,
            "cd\ne",
            &[
                "ab",
                "f",
            ][..],
        ),
        (
            &[
                "ab",
                "cd",
                "ef",
            ][..],
            (2, 0),
            3,
            "ef",
            &[
                "ab",
                "cd",
                "",
            ][..],
        ),
        (
            &[
                "ab",
                "cd",
                "ef",
            ][..],
            (2, 1),
            2,
            "f",
            &[
                "ab",
                "cd",
                "e",
            ][..],
        ),
        (
            &[
                "ab",
                "cd",
                "ef",
            ][..],
            (2, 2),
            1,
            "",
            &[
                "ab",
                "cd",
                "ef",
            ][..],
        ),
        // Empty lines
        (
            &[
                "",
                "",
                "",
            ][..],
            (0, 0),
            1,
            "\n",
            &[
                "",
                "",
            ][..],
        ),
        (
            &[
                "",
                "",
                "",
            ][..],
            (0, 0),
            2,
            "\n\n",
            &[
                "",
            ][..],
        ),
        (
            &[
                "",
                "",
                "",
            ][..],
            (0, 0),
            3,
            "\n\n",
            &[
                "",
            ][..],
        ),
        (
            &[
                "",
                "",
                "",
            ][..],
            (1, 0),
            1,
            "\n",
            &[
                "",
                "",
            ][..],
        ),
        (
            &[
                "",
                "",
                "",
            ][..],
            (2, 0),
            1,
            "",
            &[
                "",
                "",
                "",
            ][..],
        ),
        // Empty buffer
        (
            &[
                "",
            ][..],
            (0, 0),
            1,
            "",
            &[
                "",
            ][..],
        ),
    ];

    for test in tests {
        let (before, (row, col), chars, deleted, after) = test;

        let mut t = TextArea::from(before.iter().map(|s| s.to_string()));
        t.move_cursor(CursorMove::Jump(row as _, col as _));

        assert!(t.delete_str(chars), "{test:?}");
        assert_eq!(t.cursor(), (row, col), "{test:?}");
        assert_eq!(t.lines(), after, "{test:?}");
        assert_eq!(t.yank_text(), deleted, "{test:?}");

        let pos = t.cursor();
        assert!(t.undo(), "{test:?}");
        assert_eq!(t.lines(), before, "{test:?}");
        assert!(t.redo(), "{test:?}");
        assert_eq!(t.lines(), after, "{test:?}");
        assert_eq!(t.cursor(), pos, "{test:?}");
    }
}

#[test]
fn test_copy_single_line() {
    for i in 0..="abc".len() {
        for j in i.."abc".len() {
            let mut t = TextArea::from(["abc"]);

            t.move_cursor(CursorMove::Jump(0, i as u16));
            t.start_selection();
            t.move_cursor(CursorMove::Jump(0, j as u16));
            t.copy();

            assert_eq!(t.yank_text(), &"abc"[i..j], "from {i} to {j}");
            assert_eq!(t.lines(), ["abc"], "from {i} to {j}");

            assert_no_undo_redo(&mut t, (i, j));
        }
    }
}

#[test]
fn test_cut_single_line() {
    for i in 0.."abc".len() {
        for j in i + 1.."abc".len() {
            let mut t = TextArea::from(["abc"]);

            t.move_cursor(CursorMove::Jump(0, i as u16));
            t.start_selection();
            t.move_cursor(CursorMove::Jump(0, j as u16));
            t.cut();

            assert_eq!(t.yank_text(), &"abc"[i..j], "from {i} to {j}");

            let mut after = "abc".to_string();
            after.replace_range(i..j, "");
            let after = after.as_str();
            assert_eq!(t.lines(), [after], "from {i} to {j}");
            assert_eq!(t.cursor(), (0, i));
            assert_undo_redo((0, j), &["abc"], &[after], &mut t, (i, j));

            t.paste();
            assert_eq!(t.lines(), ["abc"], "from {i} to {j}");
            assert_undo_redo((0, i), &[after], &["abc"], &mut t, (i, j));
        }
    }
}

#[test]
fn test_copy_cut_empty() {
    for row in 0..=2 {
        for col in 0..=2 {
            let check = |f: fn(&mut TextArea<'_>)| {
                let mut t = TextArea::from(["ab", "cd", "ef"]);
                t.move_cursor(CursorMove::Jump(row, col));
                t.start_selection();
                t.move_cursor(CursorMove::Jump(row, col));
                f(&mut t);
                assert!(!t.is_selecting());
                assert_eq!(t.cursor(), (row as _, col as _));
                assert_eq!(t.lines(), ["ab", "cd", "ef"]);
                assert_no_undo_redo(&mut t, "");
            };

            check(|t| {
                assert!(!t.cut());
            });
            check(|t| t.copy());
        }
    }
}

#[test]
fn test_copy_cut_paste_multi_lines() {
    #[rustfmt::skip]
    let tests = [
        (
            // Initial text
            &[
                "ab",
                "cd",
                "ef",
            ][..],
            // Start position of selection
            (0, 0),
            // End position of selection
            (1, 0),
            // Expected yanked text
            "ab\n",
            // Text buffer after cut
            &[
                "cd",
                "ef",
            ][..]
        ),
        (
            &[
                "ab",
                "cd",
                "ef",
            ][..],
            (0, 0),
            (1, 1),
            "ab\nc",
            &[
                "d",
                "ef",
            ][..]
        ),
        (
            &[
                "ab",
                "cd",
                "ef",
            ][..],
            (0, 0),
            (1, 2),
            "ab\ncd",
            &[
                "",
                "ef",
            ][..]
        ),
        (
            &[
                "ab",
                "cd",
                "ef",
            ][..],
            (0, 0),
            (2, 0),
            "ab\ncd\n",
            &[
                "ef",
            ][..]
        ),
        (
            &[
                "ab",
                "cd",
                "ef",
            ][..],
            (0, 0),
            (2, 1),
            "ab\ncd\ne",
            &[
                "f",
            ][..]
        ),
        (
            &[
                "ab",
                "cd",
                "ef",
            ][..],
            (0, 0),
            (2, 2),
            "ab\ncd\nef",
            &[
                "",
            ][..]
        ),
        (
            &[
                "ab",
                "cd",
                "ef",
            ][..],
            (0, 1),
            (1, 1),
            "b\nc",
            &[
                "ad",
                "ef",
            ][..]
        ),
        (
            &[
                "ab",
                "cd",
                "ef",
            ][..],
            (0, 2),
            (1, 1),
            "\nc",
            &[
                "abd",
                "ef",
            ][..]
        ),
        (
            &[
                "ab",
                "cd",
                "ef",
            ][..],
            (1, 0),
            (2, 1),
            "cd\ne",
            &[
                "ab",
                "f",
            ][..]
        ),
        (
            &[
                "ab",
                "cd",
                "ef",
            ][..],
            (0, 2),
            (1, 0),
            "\n",
            &[
                "abcd",
                "ef",
            ][..]
        ),
        (
            &[
                "ab",
                "cd",
                "ef",
            ][..],
            (0, 2),
            (2, 0),
            "\ncd\n",
            &[
                "abef",
            ][..]
        ),
        // Multi-byte characters
        (
            &[
                "あい",
                "うえ",
                "おか",
            ][..],
            (0, 0),
            (2, 2),
            "あい\nうえ\nおか",
            &[
                "",
            ][..]
        ),
        (
            &[
                "あい",
                "うえ",
                "おか",
            ][..],
            (0, 1),
            (2, 1),
            "い\nうえ\nお",
            &[
                "あか",
            ][..]
        ),
        (
            &[
                "あい",
                "うえ",
                "おか",
            ][..],
            (0, 2),
            (2, 0),
            "\nうえ\n",
            &[
                "あいおか",
            ][..]
        ),
        (
            &[
                "あい",
                "うえ",
                "おか",
            ][..],
            (0, 2),
            (1, 2),
            "\nうえ",
            &[
                "あい",
                "おか",
            ][..]
        ),
        (
            &[
                "あい",
                "うえ",
                "おか",
            ][..],
            (0, 2),
            (1, 1),
            "\nう",
            &[
                "あいえ",
                "おか",
            ][..]
        ),
        (
            &[
                "あい",
                "うえ",
                "おか",
            ][..],
            (0, 2),
            (1, 0),
            "\n",
            &[
                "あいうえ",
                "おか",
            ][..]
        ),
    ];

    for test in tests {
        let (init_text, (srow, scol), (erow, ecol), yanked, after_cut) = test;

        {
            let mut t = TextArea::from(init_text.iter().map(|s| s.to_string()));
            t.move_cursor(CursorMove::Jump(srow as _, scol as _));
            t.start_selection();
            t.move_cursor(CursorMove::Jump(erow as _, ecol as _));
            t.copy();

            assert_eq!(t.cursor(), (erow, ecol), "{test:?}");
            assert_eq!(t.yank_text(), yanked, "{test:?}");
            assert_eq!(t.lines(), init_text, "{test:?}");
            assert_no_undo_redo(&mut t, test);
        }

        {
            let mut t = TextArea::from(init_text.iter().map(|s| s.to_string()));
            t.move_cursor(CursorMove::Jump(srow as _, scol as _));
            t.start_selection();
            t.move_cursor(CursorMove::Jump(erow as _, ecol as _));
            t.cut();

            assert_eq!(t.cursor(), (srow, scol), "{test:?}");
            assert_eq!(t.yank_text(), yanked, "{test:?}");
            assert_eq!(t.lines(), after_cut, "{test:?}");
            assert_undo_redo((erow, ecol), init_text, after_cut, &mut t, test);

            t.paste();
            assert_eq!(t.lines(), init_text, "{test:?}");
            assert_undo_redo((srow, scol), after_cut, init_text, &mut t, test);
        }

        // Reverse positions
        {
            let mut t = TextArea::from(init_text.iter().map(|s| s.to_string()));
            t.move_cursor(CursorMove::Jump(erow as _, ecol as _));
            t.start_selection();
            t.move_cursor(CursorMove::Jump(srow as _, scol as _));
            t.copy();

            assert_eq!(t.cursor(), (srow, scol), "{test:?}");
            assert_eq!(t.yank_text(), yanked, "{test:?}");
            assert_eq!(t.lines(), init_text, "{test:?}");
            assert_no_undo_redo(&mut t, test);
        }

        {
            let mut t = TextArea::from(init_text.iter().map(|s| s.to_string()));
            t.move_cursor(CursorMove::Jump(erow as _, ecol as _));
            t.start_selection();
            t.move_cursor(CursorMove::Jump(srow as _, scol as _));
            t.cut();

            assert_eq!(t.cursor(), (srow, scol), "{test:?}");
            assert_eq!(t.yank_text(), yanked, "{test:?}");
            assert_eq!(t.lines(), after_cut, "{test:?}");
            assert_undo_redo((erow, ecol), init_text, after_cut, &mut t, test);

            t.paste();
            assert_eq!(t.lines(), init_text, "{test:?}");
            assert_undo_redo((srow, scol), after_cut, init_text, &mut t, test);
        }
    }
}

#[test]
fn test_delete_selection_on_delete_operations() {
    macro_rules! test_case {
        ($name:ident($($args:expr),*)) => {
            (
                stringify!($name),
                (|t| t.$name($($args),*)) as fn(&mut TextArea) -> bool,
            )
        };
    }

    let tests = [
        test_case!(delete_char()),
        test_case!(delete_next_char()),
        test_case!(delete_line_by_end()),
        test_case!(delete_line_by_head()),
        test_case!(delete_word()),
        test_case!(delete_next_word()),
        test_case!(delete_str(3)),
    ];

    for (n, f) in tests {
        let mut t = TextArea::from(["ab", "cd", "ef"]);
        t.move_cursor(CursorMove::Jump(0, 1));
        t.start_selection();
        t.move_cursor(CursorMove::Jump(2, 1));

        let modified = f(&mut t);
        assert!(modified, "{n}");
        assert_eq!(t.lines(), ["af"], "{n}");
        assert_eq!(t.cursor(), (0, 1), "{n}");

        assert_undo_redo((2, 1), &["ab", "cd", "ef"], &["af"], &mut t, n);
    }
}

#[test]
fn test_delete_selection_on_delete_edge_cases() {
    macro_rules! test_case {
        ($name:ident($($args:expr),*), $pos:expr) => {
            (
                stringify!($name),
                (|t| t.$name($($args),*)) as fn(&mut TextArea) -> bool,
                $pos,
            )
        };
    }

    // When deleting nothing and deleting newline
    let tests = [
        test_case!(delete_char(), (0, 0)),
        test_case!(delete_char(), (1, 0)),
        test_case!(delete_next_char(), (2, 2)),
        test_case!(delete_next_char(), (1, 2)),
        test_case!(delete_line_by_end(), (0, 2)),
        test_case!(delete_line_by_end(), (2, 2)),
        test_case!(delete_line_by_head(), (0, 0)),
        test_case!(delete_line_by_head(), (1, 0)),
        test_case!(delete_word(), (0, 0)),
        test_case!(delete_word(), (1, 0)),
        test_case!(delete_next_word(), (2, 2)),
        test_case!(delete_next_word(), (1, 2)),
        test_case!(delete_str(0), (0, 0)),
        test_case!(delete_str(100), (2, 2)),
    ];

    for (n, f, pos) in tests {
        let mut t = TextArea::from(["ab", "cd", "ef"]);
        t.move_cursor(CursorMove::Jump(1, 1));
        t.start_selection();
        t.move_cursor(CursorMove::Jump(pos.0 as _, pos.1 as _));

        assert!(f(&mut t), "{n}, {pos:?}");
        assert_eq!(t.cursor(), cmp::min(pos, (1, 1)), "{n}, {pos:?}");

        t.undo();
        assert_eq!(t.lines(), ["ab", "cd", "ef"], "{n}, {pos:?}");
    }
}

#[test]
fn test_delete_selection_before_insert() {
    macro_rules! test_case {
        ($name:ident($($args:expr),*), $want:expr) => {
            (
                stringify!($name),
                (|t| {
                    t.$name($($args),*);
                }) as fn(&mut TextArea),
                &$want as &[_],
            )
        };
    }

    let tests = [
        test_case!(insert_newline(), ["a", "f"]),
        test_case!(insert_char('x'), ["axf"]),
        test_case!(insert_tab(), ["a   f"]), // Default tab is 4 spaces
        test_case!(insert_str("xyz"), ["axyzf"]),
    ];

    for (n, f, after) in tests {
        let mut t = TextArea::from(["ab", "cd", "ef"]);
        t.move_cursor(CursorMove::Jump(0, 1));
        t.start_selection();
        t.move_cursor(CursorMove::Jump(2, 1));

        f(&mut t);
        assert_eq!(t.lines(), after, "{n}");

        // XXX: Deleting selection and inserting text are separate undo units for now
        t.undo();
        t.undo();
        assert_eq!(t.lines(), ["ab", "cd", "ef"], "{n}");
    }
}

#[test]
fn test_clear_on_empty_text() {
    let mut t = TextArea::default();

    let was_cleared = t.clear();
    assert!(!was_cleared);
}

#[test]
fn test_clear_text() {
    let mut t = TextArea::default();
    let input = "
Hello world

Again
";
    t.insert_str(input);
    let was_cleared = t.clear();
    assert!(
        t.lines().join("\n").is_empty(),
        "Textarea should be empty after clearing the text"
    );
    assert!(was_cleared);

    t.undo();
    assert_eq!(
        t.lines().join("\n"),
        input,
        "Textare should have the previous content before the reset. Orginal input: {input}",
    );

    t.redo();
    assert!(
        t.lines().join("\n").is_empty(),
        "Textarea should be empty after clearing the text again"
    );
}

#[test]
fn test_undo_redo_stop_selection() {
    fn check(t: &mut TextArea, f: fn(&mut TextArea) -> bool) {
        t.move_cursor(CursorMove::Jump(0, 0));
        t.start_selection();
        t.move_cursor(CursorMove::Jump(0, 1));
        assert!(t.is_selecting());
        assert!(f(t));
        assert!(!t.is_selecting());
    }

    let mut t = TextArea::default();
    t.insert_char('a');

    check(&mut t, |t| t.undo());
    assert_eq!(t.lines(), [""]);
    check(&mut t, |t| t.redo());
    assert_eq!(t.lines(), ["a"]);
}

#[test]
fn test_set_yank_paste_text() {
    let tests = [
        ("", &[""][..], (0, 0)),
        ("abc", &["abc"][..], (0, 3)),
        ("abc\ndef", &["abc", "def"][..], (1, 3)),
        ("\n\n", &["", "", ""][..], (2, 0)),
    ];

    for test in tests {
        let (text, want, pos) = test;
        let mut t = TextArea::default();
        t.set_yank_text(text);
        t.paste();
        assert_eq!(t.lines(), want, "{test:?}");
        assert_eq!(t.yank_text(), text, "{test:?}");
        assert_eq!(t.cursor(), pos, "{test:?}");
        assert_undo_redo((0, 0), &[""], want, &mut t, test);
    }
}

#[test]
fn test_set_yank_crlf() {
    let tests = [
        ("\r\n", &["", ""][..], "\n"),
        ("\r\n\r\n", &["", "", ""][..], "\n\n"),
        ("a\r\nb", &["a", "b"][..], "a\nb"),
        ("a\r\nb\r\n", &["a", "b", ""][..], "a\nb\n"),
    ];
    for test in tests {
        let (pasted, lines, yanked) = test;
        let mut t = TextArea::default();
        t.set_yank_text(pasted);
        t.paste();
        assert_eq!(t.lines(), lines, "{test:?}");
        assert_eq!(t.yank_text(), yanked, "{test:?}");
    }
}

#[test]
fn test_select_all() {
    let mut t = TextArea::from(["aaa", "bbb", "ccc"]);
    t.select_all();
    assert!(t.is_selecting());
    assert_eq!(t.cursor(), (2, 3));
    t.cut();
    assert_eq!(t.lines(), [""]);
    assert_eq!(t.yank_text(), "aaa\nbbb\nccc");
    assert_undo_redo((2, 3), &["aaa", "bbb", "ccc"], &[""], &mut t, "");
}

#[test]
fn test_paste_while_selection() {
    let mut t = TextArea::from(["ab", "cd"]);
    t.move_cursor(CursorMove::Jump(0, 1));
    t.start_selection();
    t.move_cursor(CursorMove::Jump(1, 1));
    t.set_yank_text("x\ny");
    assert!(t.paste());
    assert_eq!(t.lines(), ["ax", "yd"]);
    assert_eq!(t.cursor(), (1, 1));
    assert!(!t.is_selecting());

    let mut t = TextArea::from(["ab", "cd"]);
    t.select_all();
    t.set_yank_text("xy\nzw");
    assert!(t.paste());
    assert_eq!(t.lines(), ["xy", "zw"]);
    assert_eq!(t.cursor(), (1, 2));
    assert!(!t.is_selecting());
}

#[test]
fn test_selection_range() {
    #[rustfmt::skip]
    let mut t = TextArea::from([
        "あいうえお",
        "Hello",
        "🐶🐱🐰🐮🐹",
    ]);

    assert_eq!(t.selection_range(), None);

    for (from, to) in [
        ((0, 0), (0, 0)),
        ((2, 5), (2, 5)),
        ((0, 2), (2, 3)),
        ((2, 1), (0, 4)),
        ((0, 0), (2, 5)),
        ((2, 5), (0, 0)),
    ] {
        let (x, y) = from;
        t.move_cursor(CursorMove::Jump(x as _, y as _));

        t.start_selection();

        let (x, y) = to;
        t.move_cursor(CursorMove::Jump(x as _, y as _));

        let have = t.selection_range().unwrap();
        let want = if from <= to { (from, to) } else { (to, from) };
        assert_eq!(have, want, "selection from {from:?} to {to:?}");

        t.cancel_selection();
        let range = t.selection_range();
        assert_eq!(range, None, "selection from {from:?} to {to:?}");
    }
}

#[test]
fn test_set_lines_preserves_non_content_configuration() {
    let mut t = TextArea::from(["hello"]);
    let base_style = Style::default().fg(Color::Green);
    let cursor_style = Style::default().bg(Color::Red);
    let cursor_line_style = Style::default().add_modifier(Modifier::BOLD);
    let selection_style = Style::default().bg(Color::Blue);
    let line_number_style = Style::default().fg(Color::Yellow);
    let placeholder_style = Style::default().fg(Color::Cyan);

    t.set_wrap_mode(tui_textarea::WrapMode::WordOrGlyph);
    t.set_block(Block::default().borders(Borders::ALL).title("Input"));
    t.set_style(base_style);
    t.set_cursor_style(cursor_style);
    t.set_cursor_render_mode(CursorRenderMode::Hidden);
    t.set_cursor_line_style(cursor_line_style);
    t.set_selection_style(selection_style);
    t.set_tab_length(8);
    t.set_alignment(Alignment::Right);
    t.set_line_number_style(line_number_style);
    t.set_placeholder_text("placeholder");
    t.set_placeholder_style(placeholder_style);
    t.set_mask_char('*');
    t.set_yank_text("copied");
    t.set_min_rows(2);
    t.set_max_rows(6);
    t.set_max_histories(3);

    t.set_lines(vec!["abc".to_string()], (0, 1));

    assert_eq!(t.lines(), ["abc"]);
    assert_eq!(t.cursor(), (0, 1));
    assert_eq!(t.wrap_mode(), tui_textarea::WrapMode::WordOrGlyph);
    assert!(t.block().is_some());
    assert_eq!(t.style(), base_style);
    assert_eq!(t.cursor_style(), cursor_style);
    assert_eq!(t.cursor_render_mode(), CursorRenderMode::Hidden);
    assert_eq!(t.cursor_line_style(), cursor_line_style);
    assert_eq!(t.selection_style(), selection_style);
    assert_eq!(t.tab_length(), 8);
    assert_eq!(t.alignment(), Alignment::Right);
    assert_eq!(t.line_number_style(), Some(line_number_style));
    assert_eq!(t.placeholder_text(), "placeholder");
    assert_eq!(t.placeholder_style(), Some(placeholder_style));
    assert_eq!(t.mask_char(), Some('*'));
    assert_eq!(t.yank_text(), "copied");
    assert_eq!(t.min_rows(), 2);
    assert_eq!(t.max_rows(), 6);
    assert_eq!(t.max_histories(), 3);
}

#[test]
fn cursor_render_mode_defaults_to_cell() {
    assert_eq!(
        TextArea::default().cursor_render_mode(),
        CursorRenderMode::Cell
    );
}

#[test]
fn set_cursor_render_mode_round_trips() {
    let mut textarea = TextArea::default();
    textarea.set_cursor_render_mode(CursorRenderMode::Hidden);
    assert_eq!(textarea.cursor_render_mode(), CursorRenderMode::Hidden);

    textarea.set_cursor_render_mode(CursorRenderMode::Cell);
    assert_eq!(textarea.cursor_render_mode(), CursorRenderMode::Cell);
}

#[test]
fn cell_mode_keeps_existing_cursor_rendering() {
    let mut textarea = TextArea::from(["abc"]);
    let cursor_style = Style::default().bg(Color::Red);
    textarea.set_cursor_style(cursor_style);
    textarea.set_cursor_line_style(Style::default());
    textarea.move_cursor(CursorMove::Jump(0, 1));

    let buf = render_textarea(&textarea, Rect::new(0, 0, 5, 1));

    assert_eq!(buf[(1, 0)].symbol(), "b");
    assert_eq!(buf[(1, 0)].style().bg, cursor_style.bg);
}

#[test]
fn hidden_mode_does_not_draw_cursor_cell_inside_text() {
    let mut textarea = TextArea::from(["abc"]);
    textarea.set_cursor_style(Style::default().bg(Color::Red));
    textarea.set_cursor_line_style(Style::default());
    textarea.set_cursor_render_mode(CursorRenderMode::Hidden);
    textarea.move_cursor(CursorMove::Jump(0, 1));

    let buf = render_textarea(&textarea, Rect::new(0, 0, 5, 1));

    assert_eq!(buf[(1, 0)].symbol(), "b");
    assert_ne!(buf[(1, 0)].style().bg, Some(Color::Red));
}

#[test]
fn hidden_mode_reserves_an_unstyled_cursor_space_at_line_end() {
    let mut textarea = TextArea::from(["abc"]);
    let cursor_style = Style::default().bg(Color::Red);
    textarea.set_cursor_style(cursor_style);
    textarea.set_cursor_line_style(Style::default());
    textarea.set_cursor_render_mode(CursorRenderMode::Hidden);
    textarea.move_cursor(CursorMove::End);

    let buf = render_textarea(&textarea, Rect::new(0, 0, 5, 1));

    assert_eq!(buf[(0, 0)].symbol(), "a");
    assert_eq!(buf[(1, 0)].symbol(), "b");
    assert_eq!(buf[(2, 0)].symbol(), "c");
    assert_eq!(buf[(3, 0)].symbol(), " ");
    assert_ne!(buf[(3, 0)].style().bg, cursor_style.bg);
}

#[test]
fn hidden_mode_placeholder_does_not_draw_cursor_cell() {
    let mut textarea = TextArea::default();
    textarea.set_placeholder_text("Type here");
    textarea.set_cursor_style(Style::default().bg(Color::Red));
    textarea.set_cursor_render_mode(CursorRenderMode::Hidden);

    let buf = render_textarea(&textarea, Rect::new(0, 0, 12, 1));

    assert_eq!(buf[(0, 0)].symbol(), "T");
    assert_eq!(
        textarea.rendered_cursor_position(),
        Some(Position { x: 0, y: 0 })
    );
}

#[test]
fn styled_placeholder_preserves_text_structure_and_legacy_accessors() {
    let mut textarea = TextArea::default();
    let base_style = Style::default().bg(Color::Blue);
    let placeholder = Text::from_iter([
        Line::from(vec![
            Span::styled("Required: ", Style::default().fg(Color::Red)),
            Span::styled("name", Style::default().fg(Color::Green)),
        ]),
        Line::raw("Second line"),
    ])
    .style(base_style);

    textarea.set_styled_placeholder(placeholder.clone());

    assert_eq!(textarea.placeholder(), &placeholder);
    assert_eq!(textarea.placeholder_text(), "Required: ");
    assert_eq!(textarea.placeholder_style(), Some(base_style));

    textarea.set_placeholder_text("Plain");
    assert_eq!(textarea.placeholder().lines, [Line::raw("Plain")]);
    assert_eq!(textarea.placeholder_style(), Some(base_style));

    textarea.set_placeholder_text("");
    assert!(textarea.placeholder().lines.is_empty());
    assert_eq!(textarea.placeholder_text(), "");
    assert_eq!(textarea.placeholder_style(), None);
}

#[test]
fn styled_placeholder_renders_spans_and_lines() {
    let mut textarea = TextArea::default();
    textarea.set_cursor_render_mode(CursorRenderMode::Hidden);
    textarea.set_styled_placeholder(Text::from_iter([
        Line::from(vec![
            Span::styled("red", Style::default().fg(Color::Red)),
            Span::styled("green", Style::default().fg(Color::Green)),
        ]),
        Line::styled("blue", Style::default().fg(Color::Blue)),
    ]));

    let buf = render_textarea(&textarea, Rect::new(0, 0, 12, 2));

    assert_eq!(buf[(0, 0)].symbol(), "r");
    assert_eq!(buf[(0, 0)].style().fg, Some(Color::Red));
    assert_eq!(buf[(3, 0)].symbol(), "g");
    assert_eq!(buf[(3, 0)].style().fg, Some(Color::Green));
    assert_eq!(buf[(0, 1)].symbol(), "b");
    assert_eq!(buf[(0, 1)].style().fg, Some(Color::Blue));
}

#[test]
fn cell_cursor_keeps_styled_placeholder_spans() {
    let mut textarea = TextArea::default();
    let cursor_style = Style::default().bg(Color::Yellow);
    textarea.set_cursor_style(cursor_style);
    textarea.set_styled_placeholder(Line::from(vec![Span::styled(
        "hint",
        Style::default().fg(Color::Cyan),
    )]));

    let buf = render_textarea(&textarea, Rect::new(0, 0, 8, 1));

    assert_eq!(buf[(0, 0)].symbol(), " ");
    assert_eq!(buf[(0, 0)].style().bg, Some(Color::Yellow));
    assert_eq!(buf[(1, 0)].symbol(), "h");
    assert_eq!(buf[(1, 0)].style().fg, Some(Color::Cyan));
}

#[test]
fn rendered_cursor_position_returns_none_before_first_render() {
    let textarea = TextArea::default();
    assert_eq!(textarea.rendered_cursor_position(), None);
}

#[test]
fn rendered_cursor_position_after_default_render() {
    let mut textarea = TextArea::from(["abc"]);
    textarea.move_cursor(CursorMove::Jump(0, 2));

    render_textarea(&textarea, Rect::new(10, 5, 20, 3));

    assert_eq!(
        textarea.rendered_cursor_position(),
        Some(Position { x: 12, y: 5 })
    );
}

#[test]
fn rendered_cursor_position_accounts_for_block() {
    let mut textarea = TextArea::from(["abc"]);
    textarea.set_block(Block::default().borders(Borders::ALL));
    textarea.move_cursor(CursorMove::Jump(0, 2));

    render_textarea(&textarea, Rect::new(10, 5, 20, 5));

    assert_eq!(
        textarea.rendered_cursor_position(),
        Some(Position { x: 13, y: 6 })
    );
}

#[test]
fn rendered_cursor_position_accounts_for_line_numbers() {
    let mut textarea = TextArea::from(["abc"]);
    textarea.set_line_number_style(Style::default());
    textarea.move_cursor(CursorMove::Jump(0, 1));

    render_textarea(&textarea, Rect::new(10, 5, 20, 3));

    assert_eq!(
        textarea.rendered_cursor_position(),
        Some(Position { x: 14, y: 5 })
    );
}

#[test]
fn rendered_cursor_position_accounts_for_horizontal_scroll() {
    let mut textarea = TextArea::from(["abcdefghi"]);
    textarea.move_cursor(CursorMove::End);

    render_textarea(&textarea, Rect::new(0, 0, 5, 1));

    assert_eq!(
        textarea.rendered_cursor_position(),
        Some(Position { x: 4, y: 0 })
    );
}

#[test]
fn rendered_cursor_position_accounts_for_wrap() {
    let mut textarea = TextArea::from(["abcdefghij"]);
    textarea.set_wrap_mode(WrapMode::WordOrGlyph);
    textarea.move_cursor(CursorMove::Jump(0, 7));

    render_textarea(&textarea, Rect::new(4, 3, 5, 3));

    assert_eq!(
        textarea.rendered_cursor_position(),
        Some(Position { x: 7, y: 4 })
    );
}

#[test]
fn rendered_cursor_position_is_visible_at_exact_wrapped_width() {
    for mode in [WrapMode::Word, WrapMode::Glyph, WrapMode::WordOrGlyph] {
        let mut textarea = TextArea::from(["abcde"]);
        textarea.set_wrap_mode(mode);
        textarea.set_cursor_render_mode(CursorRenderMode::Hidden);
        textarea.move_cursor(CursorMove::End);

        render_textarea(&textarea, Rect::new(0, 0, 5, 2));

        assert_eq!(
            textarea.rendered_cursor_position(),
            Some(Position { x: 1, y: 1 }),
            "{mode:?}"
        );
    }
}

#[test]
fn rendered_cursor_position_reserves_wrapped_caret_when_right_aligned() {
    let mut textarea = TextArea::from(["abcd"]);
    textarea.set_wrap_mode(WrapMode::Glyph);
    textarea.set_cursor_render_mode(CursorRenderMode::Hidden);
    textarea.set_alignment(Alignment::Right);
    textarea.move_cursor(CursorMove::End);

    render_textarea(&textarea, Rect::new(0, 0, 5, 1));

    assert_eq!(
        textarea.rendered_cursor_position(),
        Some(Position { x: 4, y: 0 })
    );
}

#[test]
fn rendered_cursor_position_does_not_move_a_whitespace_run_to_the_next_row() {
    for mode in [WrapMode::Word, WrapMode::WordOrGlyph] {
        let mut textarea = TextArea::from(["abcd    "]);
        textarea.set_wrap_mode(mode);
        textarea.set_cursor_render_mode(CursorRenderMode::Hidden);
        textarea.move_cursor(CursorMove::End);

        render_textarea(&textarea, Rect::new(0, 0, 7, 2));

        assert_eq!(
            textarea.rendered_cursor_position(),
            Some(Position { x: 2, y: 1 }),
            "{mode:?}"
        );
    }
}

#[test]
fn rendered_cursor_position_returns_none_for_empty_area() {
    let textarea = TextArea::default();

    render_textarea(&textarea, Rect::new(0, 0, 0, 1));

    assert_eq!(textarea.rendered_cursor_position(), None);
}

#[test]
fn rendered_cursor_position_handles_wide_unicode() {
    let mut textarea = TextArea::from(["aあb"]);
    textarea.move_cursor(CursorMove::Jump(0, 2));

    render_textarea(&textarea, Rect::new(0, 0, 10, 1));

    assert_eq!(
        textarea.rendered_cursor_position(),
        Some(Position { x: 3, y: 0 })
    );
}

#[test]
fn rendered_cursor_position_handles_tabs() {
    let mut textarea = TextArea::from(["a\tb"]);
    textarea.move_cursor(CursorMove::Jump(0, 2));

    render_textarea(&textarea, Rect::new(0, 0, 10, 1));

    assert_eq!(
        textarea.rendered_cursor_position(),
        Some(Position { x: 4, y: 0 })
    );
}

#[test]
fn cursor_at_position_requires_a_rendered_inner_area() {
    let mut textarea = TextArea::from(["abc"]);
    assert_eq!(textarea.cursor_at_position(Position::new(0, 0)), None);

    textarea.set_block(Block::default().borders(Borders::ALL));
    render_textarea(&textarea, Rect::new(10, 5, 8, 3));

    assert_eq!(textarea.cursor_at_position(Position::new(10, 6)), None);
    assert_eq!(
        textarea.cursor_at_position(Position::new(11, 6)),
        Some((0, 0))
    );
    assert_eq!(
        textarea.cursor_at_position(Position::new(13, 6)),
        Some((0, 2))
    );
    assert_eq!(textarea.cursor_at_position(Position::new(11, 7)), None);
}

#[test]
fn cursor_at_position_accounts_for_line_numbers() {
    let lines = (0..10).map(|_| "abc").collect::<Vec<_>>();
    let mut textarea = TextArea::from(lines);
    textarea.set_line_number_style(Style::default());
    render_textarea(&textarea, Rect::new(0, 0, 10, 3));

    assert_eq!(
        textarea.cursor_at_position(Position::new(0, 0)),
        Some((0, 0))
    );
    assert_eq!(
        textarea.cursor_at_position(Position::new(3, 0)),
        Some((0, 0))
    );
    assert_eq!(
        textarea.cursor_at_position(Position::new(5, 0)),
        Some((0, 1))
    );
}

#[test]
fn cursor_at_position_accounts_for_vertical_and_horizontal_scroll() {
    let mut textarea = TextArea::from((0..10).map(|row| format!("row{row}abcdefgh")));
    textarea.move_cursor(CursorMove::Jump(9, 11));
    render_textarea(&textarea, Rect::new(4, 3, 5, 3));

    assert_eq!(
        textarea.cursor_at_position(Position::new(4, 3)),
        Some((7, 7))
    );
    assert_eq!(
        textarea.cursor_at_position(Position::new(8, 5)),
        Some((9, 11))
    );
}

#[test]
fn cursor_at_position_composes_horizontal_scroll_and_line_numbers() {
    let mut textarea = TextArea::from((0..10).map(|_| "abcdefghij"));
    textarea.set_line_number_style(Style::default());
    textarea.move_cursor(CursorMove::Jump(0, 10));
    render_textarea(&textarea, Rect::new(0, 0, 6, 2));

    assert_eq!(
        textarea.cursor_at_position(Position::new(0, 0)),
        Some((0, 5))
    );
    assert_eq!(
        textarea.cursor_at_position(Position::new(5, 0)),
        Some((0, 10))
    );
}

#[test]
fn cursor_at_position_follows_wrapped_rows_and_clamps_each_row() {
    let mut textarea = TextArea::from(["abcdefghij"]);
    textarea.set_wrap_mode(WrapMode::WordOrGlyph);
    render_textarea(&textarea, Rect::new(2, 4, 5, 2));

    assert_eq!(
        textarea.cursor_at_position(Position::new(4, 5)),
        Some((0, 6))
    );
    assert_eq!(
        textarea.cursor_at_position(Position::new(6, 4)),
        Some((0, 3))
    );
}

#[test]
fn cursor_at_position_handles_tabs_and_wide_unicode() {
    let textarea = TextArea::from(["a中\tb"]);
    render_textarea(&textarea, Rect::new(0, 0, 8, 1));

    assert_eq!(
        textarea.cursor_at_position(Position::new(1, 0)),
        Some((0, 1))
    );
    assert_eq!(
        textarea.cursor_at_position(Position::new(2, 0)),
        Some((0, 1))
    );
    assert_eq!(
        textarea.cursor_at_position(Position::new(3, 0)),
        Some((0, 2))
    );
    assert_eq!(
        textarea.cursor_at_position(Position::new(4, 0)),
        Some((0, 3))
    );
}

#[test]
fn cursor_at_position_accounts_for_center_and_right_alignment() {
    let mut textarea = TextArea::from(["abc"]);
    textarea.set_cursor_render_mode(CursorRenderMode::Hidden);
    textarea.set_alignment(Alignment::Center);
    render_textarea(&textarea, Rect::new(0, 0, 9, 1));

    assert_eq!(
        textarea.cursor_at_position(Position::new(2, 0)),
        Some((0, 0))
    );
    assert_eq!(
        textarea.cursor_at_position(Position::new(4, 0)),
        Some((0, 1))
    );
    assert_eq!(
        textarea.rendered_cursor_position(),
        Some(Position::new(3, 0))
    );

    textarea.move_cursor(CursorMove::Jump(0, 1));
    textarea.set_alignment(Alignment::Right);
    render_textarea(&textarea, Rect::new(0, 0, 9, 1));

    assert_eq!(
        textarea.cursor_at_position(Position::new(5, 0)),
        Some((0, 0))
    );
    assert_eq!(
        textarea.cursor_at_position(Position::new(7, 0)),
        Some((0, 1))
    );
    assert_eq!(
        textarea.rendered_cursor_position(),
        Some(Position::new(7, 0))
    );
}

#[test]
fn cursor_at_position_clamps_blank_space_and_placeholder() {
    let textarea = TextArea::from(["ab", "c"]);
    render_textarea(&textarea, Rect::new(0, 0, 8, 4));
    assert_eq!(
        textarea.cursor_at_position(Position::new(7, 3)),
        Some((1, 1))
    );

    let mut placeholder = TextArea::default();
    placeholder.set_placeholder_text("Type here");
    render_textarea(&placeholder, Rect::new(0, 0, 8, 2));
    assert_eq!(
        placeholder.cursor_at_position(Position::new(7, 1)),
        Some((0, 0))
    );
}

#[test]
fn test_set_lines_clamps_cursor_and_clears_selection_and_history() {
    let mut t = TextArea::from(["hello", "world"]);
    assert!(t.insert_str("!"));
    assert!(t.undo());
    assert!(t.redo());
    t.move_cursor(CursorMove::Jump(0, 1));
    t.start_selection();
    t.move_cursor(CursorMove::Jump(1, 3));
    assert!(t.is_selecting());

    t.set_lines(vec!["ab".to_string(), "c".to_string()], (9, 9));

    assert_eq!(t.lines(), ["ab", "c"]);
    assert_eq!(t.cursor(), (1, 1));
    assert!(!t.is_selecting());
    assert!(!t.undo());
}

#[test]
#[should_panic(expected = "lines must not be empty; use vec![String::new()] for empty content")]
fn test_set_lines_panics_on_empty_lines() {
    let mut t = TextArea::default();
    t.set_lines(vec![], (0, 0));
}

struct DeleteTester(&'static [&'static str], fn(&mut TextArea) -> bool);
impl DeleteTester {
    fn test(&self, before: (usize, usize), after: (usize, usize, &[&str], &str)) {
        let Self(buf_before, op) = *self;
        let (row, col) = before;

        let mut t = TextArea::from(buf_before.iter().map(|s| s.to_string()));
        t.move_cursor(CursorMove::Jump(row as _, col as _));
        let modified = op(&mut t);

        let (row, col, buf_after, yank) = after;
        assert_eq!(t.lines(), buf_after);
        assert_eq!(t.cursor(), (row, col));
        assert_eq!(modified, buf_before != buf_after);
        assert_eq!(t.yank_text(), yank);

        if modified {
            t.undo();
            assert_eq!(t.lines(), buf_before);
            t.redo();
            assert_eq!(t.lines(), buf_after);
        } else {
            assert_no_undo_redo(&mut t, "");
        }
    }
}

#[test]
fn test_delete_newline() {
    let t = DeleteTester(&["a", "b", "c"], |t| t.delete_newline());
    t.test((0, 0), (0, 0, t.0, ""));
    t.test((1, 0), (0, 1, &["ab", "c"], ""));
    t.test((2, 0), (1, 1, &["a", "bc"], ""));
}

#[test]
fn test_delete_char() {
    let t = DeleteTester(&["ab", "c"], |t| t.delete_char());
    t.test((0, 0), (0, 0, t.0, ""));
    t.test((0, 1), (0, 0, &["b", "c"], ""));
    t.test((0, 2), (0, 1, &["a", "c"], ""));
    t.test((1, 0), (0, 2, &["abc"], ""));
}

#[test]
fn test_delete_next_char() {
    let t = DeleteTester(&["ab", "c"], |t| t.delete_next_char());
    t.test((0, 0), (0, 0, &["b", "c"], ""));
    t.test((0, 1), (0, 1, &["a", "c"], ""));
    t.test((0, 2), (0, 2, &["abc"], ""));
    t.test((1, 1), (1, 1, t.0, ""));
}

#[test]
fn test_delete_line_by_end() {
    let t = DeleteTester(&["aaa bbb", "d"], |t| t.delete_line_by_end());
    t.test((0, 0), (0, 0, &["", "d"], "aaa bbb"));
    t.test((0, 3), (0, 3, &["aaa", "d"], " bbb"));
    t.test((0, 6), (0, 6, &["aaa bb", "d"], "b"));
    t.test((0, 7), (0, 7, &["aaa bbbd"], "")); // Newline is not yanked
    t.test((1, 1), (1, 1, t.0, ""));
}

#[test]
fn test_delete_line_by_head() {
    let t = DeleteTester(&["aaa bbb", "d"], |t| t.delete_line_by_head());
    t.test((0, 0), (0, 0, t.0, ""));
    t.test((0, 3), (0, 0, &[" bbb", "d"], "aaa"));
    t.test((0, 7), (0, 0, &["", "d"], "aaa bbb"));
    t.test((1, 0), (0, 7, &["aaa bbbd"], "")); // Newline is not yanked
}

#[test]
fn test_delete_word() {
    let t = DeleteTester(&["word  ことば 🐶", " x"], |t| t.delete_word());
    t.test((0, 0), (0, 0, t.0, ""));
    t.test((0, 2), (0, 0, &["rd  ことば 🐶", " x"], "wo"));
    t.test((0, 4), (0, 0, &["  ことば 🐶", " x"], "word"));
    t.test((0, 5), (0, 0, &[" ことば 🐶", " x"], "word "));
    t.test((0, 6), (0, 0, &["ことば 🐶", " x"], "word  "));
    t.test((0, 7), (0, 6, &["word  とば 🐶", " x"], "こ"));
    t.test((0, 9), (0, 6, &["word   🐶", " x"], "ことば"));
    t.test((0, 10), (0, 6, &["word  🐶", " x"], "ことば "));
    t.test((0, 11), (0, 10, &["word  ことば ", " x"], "🐶"));
    t.test((1, 0), (0, 11, &["word  ことば 🐶 x"], ""));
    t.test((1, 1), (1, 0, &["word  ことば 🐶", "x"], " "));
    t.test((1, 2), (1, 1, &["word  ことば 🐶", " "], "x"));
}

#[test]
fn test_delete_next_word() {
    let t = DeleteTester(&["word  ことば 🐶", " x"], |t| t.delete_next_word());
    t.test((0, 0), (0, 0, &["  ことば 🐶", " x"], "word"));
    t.test((0, 2), (0, 2, &["wo  ことば 🐶", " x"], "rd"));
    t.test((0, 4), (0, 4, &["word 🐶", " x"], "  ことば"));
    t.test((0, 5), (0, 5, &["word  🐶", " x"], " ことば"));
    t.test((0, 6), (0, 6, &["word   🐶", " x"], "ことば"));
    t.test((0, 9), (0, 9, &["word  ことば", " x"], " 🐶"));
    t.test((0, 10), (0, 10, &["word  ことば ", " x"], "🐶"));
    t.test((0, 11), (0, 11, &["word  ことば 🐶 x"], ""));
    t.test((1, 0), (1, 0, &["word  ことば 🐶", ""], " x"));
    t.test((1, 2), (1, 2, t.0, ""));
}
