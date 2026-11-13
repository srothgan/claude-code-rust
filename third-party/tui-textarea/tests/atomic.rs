use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget as _;
use tui_textarea::{
    AtomicDeleteDirection, AtomicRange, AtomicRangeRejectReason, CursorMove, TextArea, WrapMode,
};

fn atom(start_col: usize, end_col: usize) -> AtomicRange {
    AtomicRange {
        row: 0,
        start_col,
        end_col,
    }
}

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
fn atomic_ranges_validate_sort_and_clear() {
    let mut textarea = TextArea::from(["abcdef"]);

    textarea
        .try_set_atomic_ranges([atom(4, 6), atom(1, 3)])
        .unwrap();
    assert_eq!(textarea.atomic_ranges(), &[atom(1, 3), atom(4, 6)]);

    textarea.clear_atomic_ranges();
    assert!(textarea.atomic_ranges().is_empty());

    textarea.set_atomic_ranges([atom(1, 3)]);
    let err = textarea
        .try_set_atomic_ranges([
            atom(2, 2),
            AtomicRange {
                row: 9,
                start_col: 0,
                end_col: 1,
            },
            atom(5, 7),
            atom(1, 4),
            atom(3, 5),
        ])
        .unwrap_err();

    let reasons: Vec<_> = err
        .rejected
        .iter()
        .map(|rejected| rejected.reason)
        .collect();
    assert_eq!(
        reasons,
        [
            AtomicRangeRejectReason::Empty,
            AtomicRangeRejectReason::OverlapsPrevious,
            AtomicRangeRejectReason::ColumnOutOfBounds,
            AtomicRangeRejectReason::RowOutOfBounds,
        ]
    );
    assert_eq!(textarea.atomic_ranges(), &[atom(1, 3)]);

    textarea.set_atomic_ranges([]);
    assert!(textarea.atomic_ranges().is_empty());
}

#[test]
#[should_panic(expected = "invalid atomic ranges")]
fn set_atomic_ranges_panics_on_invalid_ranges() {
    let mut textarea = TextArea::from(["abc"]);
    textarea.set_atomic_ranges([atom(1, 1)]);
}

#[test]
fn setting_atomic_ranges_normalizes_cursor_and_selection_start() {
    let mut textarea = TextArea::from(["abcdef"]);
    textarea.move_cursor(CursorMove::Jump(0, 2));
    textarea.set_atomic_ranges([atom(1, 4)]);
    assert_eq!(textarea.cursor(), (0, 4));

    let mut textarea = TextArea::from(["abcdef"]);
    textarea.move_cursor(CursorMove::Jump(0, 2));
    textarea.start_selection();
    textarea.move_cursor(CursorMove::Jump(0, 6));
    textarea.set_atomic_ranges([atom(1, 4)]);
    assert_eq!(textarea.selection_range(), Some(((0, 1), (0, 6))));

    let mut textarea = TextArea::from(["abcdef"]);
    textarea.move_cursor(CursorMove::Jump(0, 2));
    textarea.start_selection();
    textarea.move_cursor(CursorMove::Jump(0, 1));
    textarea.set_atomic_ranges([atom(1, 4)]);
    assert!(!textarea.is_selecting());
    assert_eq!(textarea.cursor(), (0, 1));
}

#[test]
fn cursor_movement_skips_atom_interiors() {
    let mut textarea = TextArea::from(["abXYZcd"]);
    textarea.set_atomic_ranges([atom(2, 5)]);

    textarea.move_cursor(CursorMove::Jump(0, 2));
    textarea.move_cursor(CursorMove::Forward);
    assert_eq!(textarea.cursor(), (0, 5));

    textarea.move_cursor(CursorMove::Back);
    assert_eq!(textarea.cursor(), (0, 2));

    textarea.move_cursor(CursorMove::Jump(0, 3));
    assert_eq!(textarea.cursor(), (0, 5));

    let mut textarea = TextArea::from(["aa XYZ bb"]);
    textarea.set_atomic_ranges([atom(3, 6)]);
    textarea.move_cursor(CursorMove::Jump(0, 3));
    textarea.move_cursor(CursorMove::WordEnd);
    assert_eq!(textarea.cursor(), (0, 6));
}

#[test]
fn directional_atomic_deletion_respects_boundaries() {
    let mut textarea = TextArea::from(["abXYZcd"]);
    textarea.set_atomic_ranges([atom(2, 5)]);
    textarea.move_cursor(CursorMove::Jump(0, 5));
    assert_eq!(
        textarea.atomic_range_at_cursor(AtomicDeleteDirection::Backward),
        Some(atom(2, 5))
    );
    assert!(textarea.delete_char());
    assert_eq!(textarea.lines(), ["abcd"]);
    assert_eq!(textarea.cursor(), (0, 2));
    assert!(textarea.atomic_ranges().is_empty());
    assert!(textarea.undo());
    assert_eq!(textarea.lines(), ["abXYZcd"]);
    assert!(textarea.atomic_ranges().is_empty());
    assert!(textarea.redo());
    assert_eq!(textarea.lines(), ["abcd"]);

    let mut textarea = TextArea::from(["abXYZcd"]);
    textarea.set_atomic_ranges([atom(2, 5)]);
    textarea.move_cursor(CursorMove::Jump(0, 2));
    assert!(textarea.delete_next_char());
    assert_eq!(textarea.lines(), ["abcd"]);

    let mut textarea = TextArea::from(["abXYZcd"]);
    textarea.set_atomic_ranges([atom(2, 5)]);
    textarea.move_cursor(CursorMove::Jump(0, 2));
    assert!(textarea.delete_char());
    assert_eq!(textarea.lines(), ["aXYZcd"]);

    let mut textarea = TextArea::from(["abXYZcd"]);
    textarea.set_atomic_ranges([atom(2, 5)]);
    textarea.move_cursor(CursorMove::Jump(0, 5));
    assert!(textarea.delete_next_char());
    assert_eq!(textarea.lines(), ["abXYZd"]);
}

#[test]
fn range_deletion_expands_partial_atom_coverage() {
    let mut textarea = TextArea::from(["abXYZcd"]);
    textarea.set_atomic_ranges([atom(2, 5)]);
    textarea.move_cursor(CursorMove::Jump(0, 2));
    assert!(textarea.delete_str(2));
    assert_eq!(textarea.lines(), ["abcd"]);
    assert_eq!(textarea.yank_text(), "XYZ");
    assert_eq!(textarea.cursor(), (0, 2));

    let mut textarea = TextArea::from(["abXYZcd"]);
    textarea.set_atomic_ranges([atom(2, 5)]);
    textarea.move_cursor(CursorMove::Jump(0, 1));
    textarea.start_selection();
    textarea.move_cursor(CursorMove::Jump(0, 3));
    textarea.set_atomic_ranges([atom(2, 5)]);
    assert!(textarea.cut());
    assert_eq!(textarea.lines(), ["acd"]);
    assert_eq!(textarea.yank_text(), "bXYZ");
    assert_eq!(textarea.cursor(), (0, 1));
}

#[test]
fn word_and_line_deletions_use_atomic_boundaries() {
    let mut textarea = TextArea::from(["abXYZ cd"]);
    textarea.set_atomic_ranges([atom(2, 5)]);
    textarea.move_cursor(CursorMove::Jump(0, 5));
    assert!(textarea.delete_word());
    assert_eq!(textarea.lines(), ["ab cd"]);
    assert_eq!(textarea.cursor(), (0, 2));

    let mut textarea = TextArea::from(["abXYZ cd"]);
    textarea.set_atomic_ranges([atom(2, 5)]);
    textarea.move_cursor(CursorMove::Jump(0, 2));
    assert!(textarea.delete_next_word());
    assert_eq!(textarea.lines(), ["ab cd"]);

    let mut textarea = TextArea::from(["abXYZcd"]);
    textarea.set_atomic_ranges([atom(2, 5)]);
    textarea.move_cursor(CursorMove::Jump(0, 5));
    assert!(textarea.delete_line_by_head());
    assert_eq!(textarea.lines(), ["cd"]);
    assert_eq!(textarea.yank_text(), "abXYZ");
}

#[test]
fn insertion_after_atom_normalized_jump_clears_ranges() {
    let mut textarea = TextArea::from(["abXYZcd"]);
    textarea.set_atomic_ranges([atom(2, 5)]);
    textarea.move_cursor(CursorMove::Jump(0, 3));
    assert_eq!(textarea.cursor(), (0, 5));
    textarea.insert_char('!');
    assert_eq!(textarea.lines(), ["abXYZ!cd"]);
    assert!(textarea.atomic_ranges().is_empty());

    let mut textarea = TextArea::from(["abXYZcd"]);
    textarea.set_atomic_ranges([atom(2, 5)]);
    textarea.move_cursor(CursorMove::Jump(0, 3));
    assert!(textarea.insert_str("!!"));
    assert_eq!(textarea.lines(), ["abXYZ!!cd"]);
}

#[test]
fn set_lines_and_clear_content_mutations_clear_atomic_ranges() {
    let mut textarea = TextArea::from(["abXYZcd"]);
    textarea.set_atomic_ranges([atom(2, 5)]);
    textarea.set_lines(vec!["abc".to_string()], (0, 0));
    assert!(textarea.atomic_ranges().is_empty());

    textarea.set_atomic_ranges([atom(0, 2)]);
    assert!(textarea.clear());
    assert!(textarea.atomic_ranges().is_empty());
}

#[test]
fn wrapped_vertical_movement_normalizes_atom_interiors() {
    let mut textarea = TextArea::from(["abcdefghij"]);
    textarea.set_wrap_mode(WrapMode::WordOrGlyph);
    render(&textarea, 5, 4);
    textarea.set_atomic_ranges([atom(6, 8)]);
    textarea.move_cursor(CursorMove::Jump(0, 3));
    textarea.move_cursor(CursorMove::Down);
    assert_eq!(textarea.cursor(), (0, 8));

    let mut textarea = TextArea::from(["abcdefghij"]);
    textarea.set_wrap_mode(WrapMode::WordOrGlyph);
    render(&textarea, 5, 4);
    textarea.set_atomic_ranges([atom(2, 4)]);
    textarea.move_cursor(CursorMove::Jump(0, 7));
    textarea.move_cursor(CursorMove::Up);
    assert_eq!(textarea.cursor(), (0, 2));
}
