use crate::util::Pos;
use crate::word::CharKind;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

// A pause longer than this ends the current typing run, as in most editors
const GROUP_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Clone, Debug)]
pub enum EditKind {
    InsertChar(char),
    DeleteChar(char),
    InsertNewline,
    DeleteNewline,
    InsertStr(String),
    DeleteStr(String),
    InsertChunk(Vec<String>),
    DeleteChunk(Vec<String>),
}

impl EditKind {
    pub(crate) fn apply(&self, lines: &mut Vec<String>, before: &Pos, after: &Pos) {
        match self {
            EditKind::InsertChar(c) => {
                lines[before.row].insert(before.offset, *c);
            }
            EditKind::DeleteChar(_) => {
                lines[before.row].remove(after.offset);
            }
            EditKind::InsertNewline => {
                let line = &mut lines[before.row];
                let next_line = line[before.offset..].to_string();
                line.truncate(before.offset);
                lines.insert(before.row + 1, next_line);
            }
            EditKind::DeleteNewline => {
                debug_assert!(before.row > 0, "invalid pos: {before:?}");
                let line = lines.remove(before.row);
                lines[before.row - 1].push_str(&line);
            }
            EditKind::InsertStr(s) => {
                lines[before.row].insert_str(before.offset, s.as_str());
            }
            EditKind::DeleteStr(s) => {
                lines[after.row].drain(after.offset..after.offset + s.len());
            }
            EditKind::InsertChunk(c) => {
                debug_assert!(c.len() > 1, "Chunk size must be > 1: {c:?}");

                // Handle first line of chunk
                let first_line = &mut lines[before.row];
                let mut last_line = first_line.drain(before.offset..).as_str().to_string();
                first_line.push_str(&c[0]);

                // Handle last line of chunk
                let next_row = before.row + 1;
                last_line.insert_str(0, c.last().unwrap());
                lines.insert(next_row, last_line);

                // Handle middle lines of chunk
                lines.splice(next_row..next_row, c[1..c.len() - 1].iter().cloned());
            }
            EditKind::DeleteChunk(c) => {
                debug_assert!(c.len() > 1, "Chunk size must be > 1: {c:?}");

                // Remove middle lines of chunk
                let mut last_line = lines
                    .drain(after.row + 1..after.row + c.len())
                    .next_back()
                    .unwrap();
                // Remove last line of chunk
                last_line.drain(..c[c.len() - 1].len());

                // Remove first line of chunk and concat remaining
                let first_line = &mut lines[after.row];
                first_line.truncate(after.offset);
                first_line.push_str(&last_line);
            }
        }
    }

    // Only single-character edits can grow a run. Where the run ends is decided by continues_run
    fn keeps_run_open(&self) -> bool {
        matches!(self, EditKind::InsertChar(_) | EditKind::DeleteChar(_))
    }

    // A run covers one CharKind, so undo stops on the same boundaries as delete_word and
    // CursorMove::WordForward. Whitespace joins the run it ends instead of starting a new one.
    fn continues_run(&self, next: char) -> bool {
        if next.is_whitespace() {
            return true;
        }
        // The character the run last grew by: the end when inserting, the front when backspacing
        let edge = match self {
            EditKind::InsertChar(c) | EditKind::DeleteChar(c) => Some(*c),
            EditKind::InsertStr(s) => s.chars().next_back(),
            EditKind::DeleteStr(s) => s.chars().next(),
            _ => None,
        };
        edge.is_some_and(|prev| CharKind::new(prev) == CharKind::new(next))
    }

    fn invert(&self) -> Self {
        use EditKind::*;
        match self.clone() {
            InsertChar(c) => DeleteChar(c),
            DeleteChar(c) => InsertChar(c),
            InsertNewline => DeleteNewline,
            DeleteNewline => InsertNewline,
            InsertStr(s) => DeleteStr(s),
            DeleteStr(s) => InsertStr(s),
            InsertChunk(c) => DeleteChunk(c),
            DeleteChunk(c) => InsertChunk(c),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Edit {
    kind: EditKind,
    before: Pos,
    after: Pos,
}

impl Edit {
    pub fn new(kind: EditKind, before: Pos, after: Pos) -> Self {
        Self {
            kind,
            before,
            after,
        }
    }

    pub fn redo(&self, lines: &mut Vec<String>) {
        self.kind.apply(lines, &self.before, &self.after);
    }

    pub fn undo(&self, lines: &mut Vec<String>) {
        self.kind.invert().apply(lines, &self.after, &self.before); // Undo is redo of inverted edit
    }

    pub fn cursor_before(&self) -> (usize, usize) {
        (self.before.row, self.before.col)
    }

    pub fn cursor_after(&self) -> (usize, usize) {
        (self.after.row, self.after.col)
    }

    // Grow this run by `other` when it starts exactly where this one ended
    fn merge(&mut self, other: &Edit) -> bool {
        if self.after != other.before {
            return false;
        }

        let next = match other.kind {
            EditKind::InsertChar(c) | EditKind::DeleteChar(c) => c,
            _ => return false,
        };

        if !self.kind.continues_run(next) {
            return false;
        }

        // Promote the run's first character so it can grow in place
        let promoted = match (&self.kind, &other.kind) {
            (EditKind::InsertChar(c), EditKind::InsertChar(_)) => {
                Some(EditKind::InsertStr(c.to_string()))
            }
            (EditKind::DeleteChar(c), EditKind::DeleteChar(_)) => {
                Some(EditKind::DeleteStr(c.to_string()))
            }
            _ => None,
        };

        if let Some(kind) = promoted {
            self.kind = kind;
        }

        match (&mut self.kind, &other.kind) {
            (EditKind::InsertStr(s), EditKind::InsertChar(c)) => s.push(*c),
            // Backspace walks the buffer backwards, so the run grows from the front
            (EditKind::DeleteStr(s), EditKind::DeleteChar(c)) => s.insert(0, *c),
            _ => return false,
        }

        self.after = other.after.clone();
        true
    }
}

#[derive(Clone, Debug)]
pub struct History {
    index: usize,
    max_items: usize,
    edits: VecDeque<Edit>,
    run_open: bool,
    last_edit_at: Option<Instant>,
}

impl History {
    pub fn new(max_items: usize) -> Self {
        Self {
            index: 0,
            max_items,
            edits: VecDeque::new(),
            run_open: false,
            last_edit_at: None,
        }
    }

    pub fn push(&mut self, edit: Edit, coalesce: bool, now: Instant) {
        if self.max_items == 0 {
            return;
        }

        if coalesce
            && self.can_extend_run(now)
            && self.edits.back_mut().is_some_and(|last| last.merge(&edit))
        {
            self.run_open = edit.kind.keeps_run_open();
            self.last_edit_at = Some(now);
            return;
        }

        if self.edits.len() == self.max_items {
            self.edits.pop_front();
            self.index = self.index.saturating_sub(1);
        }

        if self.index < self.edits.len() {
            self.edits.truncate(self.index);
        }

        self.index += 1;
        self.run_open = edit.kind.keeps_run_open();
        self.last_edit_at = Some(now);
        self.edits.push_back(edit);
    }

    // The newest edit is an open run, nothing has been undone since, and the pause was short enough
    fn can_extend_run(&self, now: Instant) -> bool {
        self.run_open
            && self.index == self.edits.len()
            && self
                .last_edit_at
                .is_some_and(|at| now.duration_since(at) < GROUP_INTERVAL)
    }

    pub fn break_run(&mut self) {
        self.run_open = false;
    }

    pub fn redo(&mut self, lines: &mut Vec<String>) -> Option<(usize, usize)> {
        if self.index == self.edits.len() {
            return None;
        }
        self.run_open = false;
        let edit = &self.edits[self.index];
        edit.redo(lines);
        self.index += 1;
        Some(edit.cursor_after())
    }

    pub fn undo(&mut self, lines: &mut Vec<String>) -> Option<(usize, usize)> {
        self.index = self.index.checked_sub(1)?;
        self.run_open = false;
        let edit = &self.edits[self.index];
        edit.undo(lines);
        Some(edit.cursor_before())
    }

    pub fn max_items(&self) -> usize {
        self.max_items
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Typing one character at `col`, as TextArea::insert_char builds it
    fn typed(c: char, col: usize) -> Edit {
        Edit::new(
            EditKind::InsertChar(c),
            Pos::new(0, col, col),
            Pos::new(0, col + 1, col + 1),
        )
    }

    // Instants are passed in rather than read from the clock, so these tests need no sleeps
    #[test]
    fn a_short_pause_extends_the_run() {
        let start = Instant::now();
        let mut history = History::new(50);

        history.push(typed('a', 0), true, start);
        history.push(typed('b', 1), true, start + Duration::from_millis(100));

        assert_eq!(history.edits.len(), 1);
    }

    #[test]
    fn a_long_pause_ends_the_run() {
        let start = Instant::now();
        let mut history = History::new(50);

        history.push(typed('a', 0), true, start);
        history.push(typed('b', 1), true, start + GROUP_INTERVAL);

        assert_eq!(history.edits.len(), 2);
    }

    #[test]
    fn the_pause_is_measured_from_the_last_edit_not_the_run_start() {
        let start = Instant::now();
        let mut history = History::new(50);

        // Each gap stays under the interval, so a slow but steady typist keeps one run
        for (i, c) in "abcd".chars().enumerate() {
            let at = start + GROUP_INTERVAL / 2 * i as u32;
            history.push(typed(c, i), true, at);
        }

        assert_eq!(history.edits.len(), 1);
    }

    #[test]
    fn a_long_pause_does_not_split_runs_when_coalescing_is_off() {
        let start = Instant::now();
        let mut history = History::new(50);

        history.push(typed('a', 0), false, start);
        history.push(typed('b', 1), false, start + Duration::from_secs(10));

        assert_eq!(history.edits.len(), 2);
    }

    #[test]
    fn insert_delete_chunk() {
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
                // (row, col) position before edit
                (0, 0),
                // Chunk to be inserted
                &[
                    "x", "y",
                ][..],
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
                &[
                    "x", "y",
                ][..],
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
                &[
                    "x", "y",
                ][..],
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
                &[
                    "x", "y",
                ][..],
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
                &[
                    "x", "y",
                ][..],
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
                &[
                    "x", "y",
                ][..],
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
                &[
                    "x", "y",
                ][..],
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
                &[
                    "x", "y",
                ][..],
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
                &[
                    "x", "y",
                ][..],
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
                &[
                    "x", "y", "z", "w"
                ][..],
                &[
                    "ab",
                    "cx",
                    "y",
                    "z",
                    "wd",
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
                &[
                    "x", "y", "z"
                ][..],
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
                &[
                    "x", "y", "z"
                ][..],
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
                &[
                    "x", "y", "z"
                ][..],
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
                &[
                    "x", "y", "z"
                ][..],
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
                &[
                    "", "", "",
                ][..],
                &[
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
                &[
                    "", "", "",
                ][..],
                &[
                    "ab",
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
                &[
                    "", "", "",
                ][..],
                &[
                    "ab",
                    "c",
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
                &[
                    "", "", "",
                ][..],
                &[
                    "ab",
                    "cd",
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
                &[
                    "", "", "",
                ][..],
                &[
                    "ab",
                    "cd",
                    "ef",
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
                &[
                    "🐷", "🐼", "🐴",
                ][..],
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
                &[
                    "🐷", "🐼", "🐴",
                ][..],
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
                &[
                    "🐷", "🐼", "🐴",
                ][..],
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
                &[
                    "🐷", "🐼", "🐴",
                ][..],
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
                &[
                    "🐷", "🐼", "🐴",
                ][..],
                &[
                    "🐶🐱",
                    "🐮🐰",
                    "🐧🐭🐷",
                    "🐼",
                    "🐴",
                ][..],
            ),
        ];

        for test in tests {
            let (before, pos, input, expected) = test;
            let (row, col) = pos;
            let before_pos = {
                let offset = before[row]
                    .char_indices()
                    .map(|(i, _)| i)
                    .nth(col)
                    .unwrap_or(before[row].len());
                Pos::new(row, col, offset)
            };
            let mut lines: Vec<_> = before.iter().map(|s| s.to_string()).collect();
            let chunk: Vec<_> = input.iter().map(|s| s.to_string()).collect();
            let after_pos = {
                let row = row + input.len() - 1;
                let last = input.last().unwrap();
                let col = last.chars().count();
                Pos::new(row, col, last.len())
            };

            let edit = EditKind::InsertChunk(chunk.clone());
            edit.apply(&mut lines, &before_pos, &after_pos);
            assert_eq!(&lines, expected, "{test:?}");

            let edit = EditKind::DeleteChunk(chunk);
            edit.apply(&mut lines, &after_pos, &before_pos);
            assert_eq!(&lines, &before, "{test:?}");
        }
    }
}
