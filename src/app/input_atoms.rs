// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use tui_textarea::{AtomicDeleteDirection, AtomicRange};

const IMAGE_PREFIX: &str = "[Image #";
const PASTE_PREFIX: &str = "[Pasted Text ";
const PASTE_SEPARATOR: &str = " - ";
const PASTE_SUFFIX: &str = " chars]";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputAtomKind {
    ImageBadge {
        one_based_index: usize,
    },
    /// Zero-based index into `InputState::paste_blocks`.
    PasteBlock {
        index: usize,
        char_count: usize,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputAtom {
    pub kind: InputAtomKind,
    pub row: usize,
    pub start_byte: usize,
    pub end_byte: usize,
    pub start_col: usize,
    pub end_col: usize,
}

// File/folder mentions and skill mentions are intentionally excluded for now.
// They need a separate design because they interact with autocomplete and command routing.

#[must_use]
pub fn resolve_input_atoms(lines: &[String]) -> Vec<InputAtom> {
    let mut atoms = Vec::new();
    for (row, line) in lines.iter().enumerate() {
        atoms.extend(resolve_line_atoms(row, line));
    }
    atoms
}

#[must_use]
pub fn resolve_line_atoms(row: usize, line: &str) -> Vec<InputAtom> {
    let mut candidates = Vec::new();
    collect_image_atoms(row, line, &mut candidates);
    collect_paste_atoms(row, line, &mut candidates);
    candidates.sort_by_key(|atom| (atom.row, atom.start_byte));

    let mut atoms: Vec<InputAtom> = Vec::with_capacity(candidates.len());
    for atom in candidates {
        let overlaps_previous = atoms.last().is_some_and(|previous| {
            previous.row == atom.row && atom.start_byte < previous.end_byte
        });
        if !overlaps_previous {
            atoms.push(atom);
        }
    }
    atoms
}

#[must_use]
pub fn atom_at_cursor(
    lines: &[String],
    cursor_row: usize,
    cursor_col: usize,
    direction: AtomicDeleteDirection,
) -> Option<InputAtom> {
    lines.get(cursor_row).into_iter().flat_map(|line| resolve_line_atoms(cursor_row, line)).find(
        |atom| match direction {
            AtomicDeleteDirection::Backward => {
                atom.start_col < cursor_col && cursor_col <= atom.end_col
            }
            AtomicDeleteDirection::Forward => {
                atom.start_col <= cursor_col && cursor_col < atom.end_col
            }
        },
    )
}

#[must_use]
pub fn atomic_ranges_for_textarea(lines: &[String]) -> Vec<AtomicRange> {
    resolve_input_atoms(lines)
        .into_iter()
        .map(|atom| AtomicRange { row: atom.row, start_col: atom.start_col, end_col: atom.end_col })
        .collect()
}

fn collect_image_atoms(row: usize, line: &str, atoms: &mut Vec<InputAtom>) {
    let mut search_from = 0usize;
    while search_from < line.len() {
        let Some(rel_start) = line[search_from..].find(IMAGE_PREFIX) else {
            break;
        };
        let start = search_from + rel_start;
        if let Some((one_based_index, end)) = parse_image_badge_at(line, start) {
            atoms.push(new_atom(
                InputAtomKind::ImageBadge { one_based_index },
                row,
                line,
                start,
                end,
            ));
            search_from = end;
        } else {
            search_from = start + IMAGE_PREFIX.len();
        }
    }
}

fn collect_paste_atoms(row: usize, line: &str, atoms: &mut Vec<InputAtom>) {
    let mut search_from = 0usize;
    while search_from < line.len() {
        let Some(rel_start) = line[search_from..].find(PASTE_PREFIX) else {
            break;
        };
        let start = search_from + rel_start;
        if let Some((index, char_count, end)) = parse_paste_placeholder_at(line, start) {
            atoms.push(new_atom(
                InputAtomKind::PasteBlock { index, char_count },
                row,
                line,
                start,
                end,
            ));
            search_from = end;
        } else {
            search_from = start + PASTE_PREFIX.len();
        }
    }
}

fn parse_image_badge_at(line: &str, start: usize) -> Option<(usize, usize)> {
    let rest = line.get(start..)?.strip_prefix(IMAGE_PREFIX)?;
    let digits_len = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits_len == 0 || rest.as_bytes().get(digits_len).copied() != Some(b']') {
        return None;
    }
    let one_based_index: usize = rest[..digits_len].parse().ok()?;
    if one_based_index == 0 {
        return None;
    }
    Some((one_based_index, start + IMAGE_PREFIX.len() + digits_len + 1))
}

fn parse_paste_placeholder_at(line: &str, start: usize) -> Option<(usize, usize, usize)> {
    let rest = line.get(start..)?.strip_prefix(PASTE_PREFIX)?;
    let n_len = rest.bytes().take_while(u8::is_ascii_digit).count();
    if n_len == 0 {
        return None;
    }
    let one_based_index: usize = rest[..n_len].parse().ok()?;
    if one_based_index == 0 {
        return None;
    }

    let rest = rest.get(n_len..)?.strip_prefix(PASTE_SEPARATOR)?;
    let char_count_len = rest.bytes().take_while(u8::is_ascii_digit).count();
    if char_count_len == 0 {
        return None;
    }
    let char_count: usize = rest[..char_count_len].parse().ok()?;
    if char_count == 0 || !rest.get(char_count_len..)?.starts_with(PASTE_SUFFIX) {
        return None;
    }

    let end = start
        + PASTE_PREFIX.len()
        + n_len
        + PASTE_SEPARATOR.len()
        + char_count_len
        + PASTE_SUFFIX.len();
    Some((one_based_index - 1, char_count, end))
}

fn new_atom(
    kind: InputAtomKind,
    row: usize,
    line: &str,
    start_byte: usize,
    end_byte: usize,
) -> InputAtom {
    InputAtom {
        kind,
        row,
        start_byte,
        end_byte,
        start_col: byte_to_char_index(line, start_byte),
        end_col: byte_to_char_index(line, end_byte),
    }
}

fn byte_to_char_index(s: &str, byte_idx: usize) -> usize {
    s[..byte_idx].chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn detects_image_badges() {
        let atoms = resolve_line_atoms(0, "hello [Image #1] world [Image #23]");
        assert_eq!(
            atoms.iter().map(|atom| atom.kind).collect::<Vec<_>>(),
            vec![
                InputAtomKind::ImageBadge { one_based_index: 1 },
                InputAtomKind::ImageBadge { one_based_index: 23 },
            ]
        );
    }

    #[test]
    fn rejects_malformed_image_badges() {
        let atoms =
            resolve_line_atoms(0, "[Image #0] [Image #] [Image #abc] [Image #1x] [Image #2]");
        assert_eq!(atoms.len(), 1);
        assert_eq!(atoms[0].kind, InputAtomKind::ImageBadge { one_based_index: 2 });
    }

    #[test]
    fn detects_paste_placeholders() {
        let atoms = resolve_line_atoms(1, "[Pasted Text 1 - 42 chars] x [Pasted Text 3 - 5 chars]");
        assert_eq!(
            atoms.iter().map(|atom| atom.kind).collect::<Vec<_>>(),
            vec![
                InputAtomKind::PasteBlock { index: 0, char_count: 42 },
                InputAtomKind::PasteBlock { index: 2, char_count: 5 },
            ]
        );
        assert!(atoms.iter().all(|atom| atom.row == 1));
    }

    #[test]
    fn rejects_malformed_paste_placeholders() {
        let atoms = resolve_line_atoms(
            0,
            "[Pasted Text 0 - 1 chars] [Pasted Text 1] [Pasted Text 1 - 0 chars] \
             [Pasted Text 2 - 3 words] [Pasted Text 4 - 9 chars]",
        );
        assert_eq!(atoms.len(), 1);
        assert_eq!(atoms[0].kind, InputAtomKind::PasteBlock { index: 3, char_count: 9 });
    }

    #[test]
    fn returns_byte_and_char_ranges_after_unicode() {
        let line = "é [Image #7]";
        let atoms = resolve_line_atoms(2, line);
        assert_eq!(atoms.len(), 1);
        assert_eq!(atoms[0].start_byte, "é ".len());
        assert_eq!(atoms[0].start_col, "é ".chars().count());
        assert_eq!(atoms[0].end_byte, line.len());
        assert_eq!(atoms[0].end_col, line.chars().count());
    }

    #[test]
    fn detects_mixed_atoms_in_order() {
        let atoms = resolve_input_atoms(&[
            "x [Pasted Text 2 - 3 chars]".to_owned(),
            "[Image #1]".to_owned(),
        ]);
        assert_eq!(
            atoms.iter().map(|atom| atom.kind).collect::<Vec<_>>(),
            vec![
                InputAtomKind::PasteBlock { index: 1, char_count: 3 },
                InputAtomKind::ImageBadge { one_based_index: 1 },
            ]
        );
    }

    #[test]
    fn atom_at_cursor_uses_directional_boundaries() {
        let lines = vec!["ab[Image #1]cd".to_owned()];
        let start = "ab".chars().count();
        let end = "ab[Image #1]".chars().count();

        assert!(atom_at_cursor(&lines, 0, start, AtomicDeleteDirection::Backward).is_none());
        assert!(atom_at_cursor(&lines, 0, end, AtomicDeleteDirection::Forward).is_none());
        assert!(atom_at_cursor(&lines, 0, end, AtomicDeleteDirection::Backward).is_some());
        assert!(atom_at_cursor(&lines, 0, start, AtomicDeleteDirection::Forward).is_some());
        assert!(atom_at_cursor(&lines, 0, start + 1, AtomicDeleteDirection::Backward).is_some());
    }

    #[test]
    fn converts_atoms_to_textarea_ranges() {
        let ranges = atomic_ranges_for_textarea(&["x[Image #1]".to_owned()]);
        assert_eq!(
            ranges,
            vec![AtomicRange { row: 0, start_col: 1, end_col: "x[Image #1]".chars().count() }]
        );
    }
}
