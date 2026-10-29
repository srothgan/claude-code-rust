use tui_textarea::{CursorMove, TextArea};

// Regression test for #4
#[test]
fn disable_history() {
    let mut t = TextArea::default();
    t.set_max_histories(0);
    assert!(t.insert_str("hello"));
    assert_eq!(t.lines(), ["hello"]);
}

fn type_chars(t: &mut TextArea<'_>, s: &str) {
    for c in s.chars() {
        t.insert_char(c);
    }
}

// Coalescing is opt-in, so the tests below enable it explicitly
fn coalescing() -> TextArea<'static> {
    let mut t = TextArea::default();
    t.set_undo_coalescing(true);
    t
}

fn coalescing_from<const N: usize>(lines: [&str; N]) -> TextArea<'static> {
    let mut t = TextArea::from(lines);
    t.set_undo_coalescing(true);
    t
}

#[test]
fn coalescing_is_disabled_by_default() {
    let mut t = TextArea::default();
    assert!(!t.undo_coalescing());

    type_chars(&mut t, "hello");
    for expected in ["hell", "hel", "he", "h", ""] {
        assert!(t.undo());
        assert_eq!(t.lines(), [expected]);
    }
}

#[test]
fn typed_run_undoes_and_redoes_as_one_step() {
    let mut t = coalescing();
    type_chars(&mut t, "hello");
    assert_eq!(t.cursor(), (0, 5));

    assert!(t.undo());
    assert_eq!(t.lines(), [""]);
    assert_eq!(t.cursor(), (0, 0));
    assert!(!t.undo());

    assert!(t.redo());
    assert_eq!(t.lines(), ["hello"]);
    assert_eq!(t.cursor(), (0, 5));
    assert!(!t.redo());
}

#[test]
fn typed_run_coalesces_multi_byte_chars() {
    let mut t = coalescing();
    type_chars(&mut t, "🐶🐱🐰");

    assert!(t.undo());
    assert_eq!(t.lines(), [""]);
    assert!(t.redo());
    assert_eq!(t.lines(), ["🐶🐱🐰"]);
}

#[test]
fn punctuation_breaks_run() {
    let mut t = coalescing();
    type_chars(&mut t, "foo();bar()");

    for expected in ["foo();bar", "foo();", "foo", ""] {
        assert!(t.undo());
        assert_eq!(t.lines(), [expected]);
    }
    assert!(!t.undo());
}

#[test]
fn underscores_and_digits_stay_in_one_run() {
    // CharKind treats `_` as a word character, so identifiers undo as a unit
    let mut t = coalescing();
    type_chars(&mut t, "my_var2");

    assert!(t.undo());
    assert_eq!(t.lines(), [""]);
}

#[test]
fn backspace_run_breaks_at_punctuation() {
    let mut t = coalescing_from(["foo();bar"]);
    t.move_cursor(CursorMove::End);
    while t.delete_char() {}

    for expected in ["foo", "foo();", "foo();bar"] {
        assert!(t.undo());
        assert_eq!(t.lines(), [expected]);
    }
    assert!(!t.undo());
}

/// Undo boundaries must line up with the word boundaries the crate already uses, so that undoing
/// typed text and deleting it with `delete_word` walk back in the same pieces.
#[test]
fn undo_boundaries_match_delete_word() {
    fn undo_chunks(s: &str) -> Vec<String> {
        let mut t = coalescing();
        type_chars(&mut t, s);
        collect(&mut t, |t| t.undo())
    }

    fn delete_word_chunks(s: &str) -> Vec<String> {
        let mut t = coalescing_from([s]);
        t.move_cursor(CursorMove::End);
        collect(&mut t, |t| t.delete_word())
    }

    fn collect(
        t: &mut TextArea<'_>,
        mut shrink: impl FnMut(&mut TextArea<'_>) -> bool,
    ) -> Vec<String> {
        let mut chunks = vec![];
        loop {
            let before = t.lines()[0].to_string();
            if !shrink(t) {
                break;
            }
            chunks.push(before[t.lines()[0].len()..].to_string());
        }
        chunks.reverse();
        chunks
    }

    for s in [
        "foo();bar()",
        "#[derive(Clone)]",
        "my_var2 = 3",
        "hello big world",
        "hello  world",
        "hello   world",
        "a\t\tb",
        "x  =  1",
    ] {
        assert_eq!(
            undo_chunks(s),
            delete_word_chunks(s),
            "boundaries differ for {s:?}"
        );
    }
}

#[test]
fn whitespace_breaks_run_into_words() {
    let mut t = coalescing();
    type_chars(&mut t, "the quick brown fox");

    // Trailing spaces are intentional: each space belongs to the word it ends
    for expected in ["the quick brown ", "the quick ", "the ", ""] {
        assert!(t.undo());
        assert_eq!(t.lines(), [expected]);
    }
    assert!(!t.undo());

    for expected in [
        "the ",
        "the quick ",
        "the quick brown ",
        "the quick brown fox",
    ] {
        assert!(t.redo());
        assert_eq!(t.lines(), [expected]);
    }
}

#[test]
fn consecutive_whitespace_stays_in_one_run() {
    let mut t = coalescing();
    type_chars(&mut t, "hello  world");

    // Both spaces belong to the run they end, exactly as delete_word groups them
    assert!(t.undo());
    assert_eq!(t.lines(), ["hello  "]);
    assert!(t.undo());
    assert_eq!(t.lines(), [""]);
}

#[test]
fn consecutive_tabs_stay_in_one_run() {
    let mut t = coalescing();
    type_chars(&mut t, "a\t\tb");

    assert!(t.undo());
    assert_eq!(t.lines(), ["a\t\t"]);
    assert!(t.undo());
    assert_eq!(t.lines(), [""]);
}

#[test]
fn backspace_run_keeps_consecutive_whitespace_together() {
    let mut t = coalescing_from(["hello  world"]);
    t.move_cursor(CursorMove::End);
    while t.delete_char() {}
    assert_eq!(t.lines(), [""]);

    // Backspace walks backwards, so both spaces attach to the "world" run and restore with it
    assert!(t.undo());
    assert_eq!(t.lines(), ["hello"]);
    assert!(t.undo());
    assert_eq!(t.lines(), ["hello  world"]);
    assert!(!t.undo());
}

#[test]
fn backspace_run_keeps_consecutive_tabs_together() {
    let mut t = coalescing_from(["a\t\tb"]);
    t.move_cursor(CursorMove::End);
    while t.delete_char() {}

    assert!(t.undo());
    assert_eq!(t.lines(), ["a"]);
    assert!(t.undo());
    assert_eq!(t.lines(), ["a\t\tb"]);
}

#[test]
fn backspace_run_breaks_at_words() {
    let mut t = coalescing_from(["hello big world"]);
    t.move_cursor(CursorMove::End);
    while t.delete_char() {}
    assert_eq!(t.lines(), [""]);

    for expected in ["hello", "hello big", "hello big world"] {
        assert!(t.undo());
        assert_eq!(t.lines(), [expected]);
    }
    assert!(!t.undo());
}

#[test]
fn tab_is_its_own_step() {
    let mut t = coalescing();
    type_chars(&mut t, "ab");
    assert!(t.insert_tab());
    type_chars(&mut t, "cd");

    // Two spaces, not four: the tab aligns to the next tab stop from column 2
    assert!(t.undo());
    assert_eq!(t.lines(), ["ab  "]);
    assert!(t.undo());
    assert_eq!(t.lines(), ["ab"]);
    assert!(t.undo());
    assert_eq!(t.lines(), [""]);
}

#[test]
fn newline_breaks_run() {
    let mut t = coalescing();
    type_chars(&mut t, "ab");
    t.insert_newline();
    type_chars(&mut t, "cd");

    assert!(t.undo());
    assert_eq!(t.lines(), ["ab", ""]);
    assert!(t.undo());
    assert_eq!(t.lines(), ["ab"]);
    assert!(t.undo());
    assert_eq!(t.lines(), [""]);
}

#[test]
fn cursor_move_breaks_run() {
    let mut t = coalescing();
    type_chars(&mut t, "ab");
    // Move away and back so the next insertion is still adjacent to the previous one
    t.move_cursor(CursorMove::Back);
    t.move_cursor(CursorMove::Forward);
    type_chars(&mut t, "cd");
    assert_eq!(t.lines(), ["abcd"]);

    assert!(t.undo());
    assert_eq!(t.lines(), ["ab"]);
    assert!(t.undo());
    assert_eq!(t.lines(), [""]);
}

#[test]
fn paste_is_its_own_step() {
    let mut t = coalescing();
    type_chars(&mut t, "ab");
    assert!(t.insert_str("XY"));
    type_chars(&mut t, "cd");
    assert_eq!(t.lines(), ["abXYcd"]);

    assert!(t.undo());
    assert_eq!(t.lines(), ["abXY"]);
    assert!(t.undo());
    assert_eq!(t.lines(), ["ab"]);
    assert!(t.undo());
    assert_eq!(t.lines(), [""]);
}

#[test]
fn backspace_run_undoes_as_one_step() {
    let mut t = coalescing_from(["hello"]);
    t.move_cursor(CursorMove::End);
    for _ in 0..3 {
        assert!(t.delete_char());
    }
    assert_eq!(t.lines(), ["he"]);
    assert_eq!(t.cursor(), (0, 2));

    assert!(t.undo());
    assert_eq!(t.lines(), ["hello"]);
    assert_eq!(t.cursor(), (0, 5));

    assert!(t.redo());
    assert_eq!(t.lines(), ["he"]);
    assert_eq!(t.cursor(), (0, 2));
}

#[test]
fn backspace_run_coalesces_multi_byte_chars() {
    let mut t = coalescing_from(["🐶🐱🐰"]);
    t.move_cursor(CursorMove::End);
    for _ in 0..3 {
        assert!(t.delete_char());
    }
    assert_eq!(t.lines(), [""]);

    assert!(t.undo());
    assert_eq!(t.lines(), ["🐶🐱🐰"]);
}

#[test]
fn insertions_and_deletions_do_not_merge() {
    let mut t = coalescing();
    type_chars(&mut t, "ab");
    assert!(t.delete_char());
    assert_eq!(t.lines(), ["a"]);

    assert!(t.undo());
    assert_eq!(t.lines(), ["ab"]);
    assert!(t.undo());
    assert_eq!(t.lines(), [""]);
}

#[test]
fn backspace_over_a_newline_breaks_run() {
    let mut t = coalescing_from(["ab", "cd"]);
    t.move_cursor(CursorMove::End);
    t.move_cursor(CursorMove::Down);
    t.move_cursor(CursorMove::End);
    for _ in 0..3 {
        assert!(t.delete_char());
    }
    assert_eq!(t.lines(), ["ab"]);

    assert!(t.undo());
    assert_eq!(t.lines(), ["ab", ""]);
    assert!(t.undo());
    assert_eq!(t.lines(), ["ab", "cd"]);
}

#[test]
fn typing_after_an_undo_starts_a_new_run() {
    let mut t = coalescing();
    type_chars(&mut t, "ab");
    type_chars(&mut t, "cd");
    assert!(t.undo());
    assert_eq!(t.lines(), [""]);

    type_chars(&mut t, "xy");
    assert_eq!(t.lines(), ["xy"]);
    assert!(t.undo());
    assert_eq!(t.lines(), [""]);
    assert!(!t.undo());
}

#[test]
fn disabling_coalescing_restores_per_character_undo() {
    let mut t = coalescing();
    t.set_undo_coalescing(false);
    assert!(!t.undo_coalescing());
    type_chars(&mut t, "hello");

    for expected in ["hell", "hel", "he", "h", ""] {
        assert!(t.undo());
        assert_eq!(t.lines(), [expected]);
    }
    assert!(!t.undo());
}

#[test]
fn disabling_coalescing_mid_run_breaks_it() {
    let mut t = coalescing();
    type_chars(&mut t, "ab");
    t.set_undo_coalescing(false);
    type_chars(&mut t, "cd");

    assert!(t.undo());
    assert_eq!(t.lines(), ["abc"]);
    assert!(t.undo());
    assert_eq!(t.lines(), ["ab"]);
    assert!(t.undo());
    assert_eq!(t.lines(), [""]);
}

#[test]
fn enabling_coalescing_mid_run_breaks_it() {
    let mut t = TextArea::default();
    t.set_undo_coalescing(false);
    type_chars(&mut t, "ab");
    t.set_undo_coalescing(true);
    type_chars(&mut t, "cd");

    // "cd" must be its own run, not an extension of the "b" typed while disabled
    assert!(t.undo());
    assert_eq!(t.lines(), ["ab"]);
    assert!(t.undo());
    assert_eq!(t.lines(), ["a"]);
    assert!(t.undo());
    assert_eq!(t.lines(), [""]);
}

fn assert_round_trip(t: &mut TextArea<'_>) {
    let lines: Vec<String> = t.lines().to_vec();
    let cursor = t.cursor();

    let mut undos = 0;
    while t.undo() {
        undos += 1;
    }
    let mut redos = 0;
    while t.redo() {
        redos += 1;
    }

    assert_eq!(undos, redos, "undo and redo disagree on step count");
    assert_eq!(t.lines(), lines.as_slice());
    assert_eq!(t.cursor(), cursor);
}

#[test]
fn undo_redo_round_trips_over_mixed_edits() {
    let mut t = coalescing();
    type_chars(&mut t, "the quick brown fox");
    assert_round_trip(&mut t);

    let mut t = coalescing();
    type_chars(&mut t, "🐶🐱 🐰🐮 ab");
    assert_round_trip(&mut t);

    let mut t = coalescing();
    type_chars(&mut t, "hello wrld");
    for _ in 0..4 {
        assert!(t.delete_char());
    }
    type_chars(&mut t, "world");
    t.insert_newline();
    assert!(t.insert_str("pasted"));
    type_chars(&mut t, " tail");
    assert!(t.delete_word());
    assert_round_trip(&mut t);

    let mut t = coalescing_from(["alpha beta gamma"]);
    t.move_cursor(CursorMove::End);
    while t.delete_char() {}
    assert_round_trip(&mut t);
}

#[test]
fn redo_is_discarded_after_a_new_edit() {
    let mut t = coalescing();
    type_chars(&mut t, "one two");
    assert!(t.undo());
    assert_eq!(t.lines(), ["one "]);

    t.insert_char('X');
    assert!(!t.redo(), "redo must not resurrect the truncated branch");
    assert_eq!(t.lines(), ["one X"]);
}

#[test]
fn coalescing_does_not_consume_history_slots() {
    let mut t = coalescing();
    t.set_max_histories(1);
    type_chars(&mut t, "hello");

    assert!(t.undo());
    assert_eq!(t.lines(), [""]);
    assert!(t.redo());
    assert_eq!(t.lines(), ["hello"]);
}
