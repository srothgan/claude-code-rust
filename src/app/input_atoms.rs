// SPDX-License-Identifier: Apache-2.0

use tui_textarea::AtomicRange;

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
    fn image_tokens_preserve_identity_bounds_and_recover_after_malformed_input() {
        for (line, expected) in [
            ("[Image #1]", vec![(1, 0, 10, 0, 10)]),
            ("\u{e9} [Image #23]\u{732b}", vec![(23, 3, 14, 2, 13)]),
            ("[Image #2][Image #7]", vec![(2, 0, 10, 0, 10), (7, 10, 20, 10, 20)]),
        ] {
            let atoms = resolve_line_atoms(3, line);
            let expected: Vec<_> = expected
                .into_iter()
                .map(|(index, start_byte, end_byte, start_col, end_col)| InputAtom {
                    kind: InputAtomKind::ImageBadge { one_based_index: index },
                    row: 3,
                    start_byte,
                    end_byte,
                    start_col,
                    end_col,
                })
                .collect();
            assert_eq!(atoms, expected, "{line}");
        }
        for bad in [
            "[Image #0]",
            "[Image #]",
            "[Image #abc]",
            "[Image #1x]",
            "[Image #2",
            "[Image #9999999999999999999999999999999999999999]",
        ] {
            assert!(resolve_line_atoms(0, bad).is_empty(), "{bad}");
            let line = format!("{bad} | [Image #8]");
            let start = bad.len() + 3; // Literal ASCII fixture prefix, independent of the parser.
            assert_eq!(
                resolve_line_atoms(0, &line),
                vec![InputAtom {
                    kind: InputAtomKind::ImageBadge { one_based_index: 8 },
                    row: 0,
                    start_byte: start,
                    end_byte: start + 10,
                    start_col: start,
                    end_col: start + 10,
                }],
                "recovery after {bad}"
            );
        }
    }

    #[test]
    fn paste_tokens_preserve_storage_identity_counts_and_recover_after_malformed_input() {
        for (line, index, count, start_byte, end_byte, start_col, end_col) in [
            ("[Pasted Text 1 - 42 chars]", 0, 42, 0, 26, 0, 26),
            ("\u{e9} [Pasted Text 3 - 5 chars]\u{732b}", 2, 5, 3, 28, 2, 27),
        ] {
            assert_eq!(
                resolve_line_atoms(2, line),
                vec![InputAtom {
                    kind: InputAtomKind::PasteBlock { index, char_count: count },
                    row: 2,
                    start_byte,
                    end_byte,
                    start_col,
                    end_col,
                }]
            );
        }
        for bad in [
            "[Pasted Text 0 - 1 chars]",
            "[Pasted Text 1]",
            "[Pasted Text 1 - 0 chars]",
            "[Pasted Text 2 - 3 words]",
            "[Pasted Text 2 - 3 chars",
            "[Pasted Text 9999999999999999999999999999999999999999 - 3 chars]",
            "[Pasted Text 2 - 9999999999999999999999999999999999999999 chars]",
        ] {
            assert!(resolve_line_atoms(0, bad).is_empty(), "{bad}");
            let line = format!("{bad} | [Pasted Text 4 - 9 chars]");
            let start = bad.len() + 3;
            assert_eq!(
                resolve_line_atoms(0, &line),
                vec![InputAtom {
                    kind: InputAtomKind::PasteBlock { index: 3, char_count: 9 },
                    row: 0,
                    start_byte: start,
                    end_byte: start + 25,
                    start_col: start,
                    end_col: start + 25,
                }],
                "recovery after {bad}"
            );
        }
    }

    #[test]
    fn mixed_atoms_follow_source_order_with_independent_rows_and_spans() {
        let lines = vec![
            "\u{e9}[Pasted Text 2 - 3 chars][Image #7] [Pasted Text 4 - 9 chars]".to_owned(),
            "\u{732b} [Image #3]".to_owned(),
        ];
        assert_eq!(
            resolve_input_atoms(&lines),
            vec![
                InputAtom {
                    kind: InputAtomKind::PasteBlock { index: 1, char_count: 3 },
                    row: 0,
                    start_byte: 2,
                    end_byte: 27,
                    start_col: 1,
                    end_col: 26
                },
                InputAtom {
                    kind: InputAtomKind::ImageBadge { one_based_index: 7 },
                    row: 0,
                    start_byte: 27,
                    end_byte: 37,
                    start_col: 26,
                    end_col: 36
                },
                InputAtom {
                    kind: InputAtomKind::PasteBlock { index: 3, char_count: 9 },
                    row: 0,
                    start_byte: 38,
                    end_byte: 63,
                    start_col: 37,
                    end_col: 62
                },
                InputAtom {
                    kind: InputAtomKind::ImageBadge { one_based_index: 3 },
                    row: 1,
                    start_byte: 4,
                    end_byte: 14,
                    start_col: 2,
                    end_col: 12
                },
            ]
        );
    }

    #[test]
    fn adjacent_atoms_delete_the_correct_directional_target_in_character_coordinates() {
        use crate::app::InputState;
        for (row, col, backward, expected) in [
            (0, 11, true, "\u{e9}[Pasted Text 2 - 3 chars] z\nq[Image #3]"),
            (0, 11, false, "\u{e9}[Image #7] z\nq[Image #3]"),
            (0, 1, false, "\u{e9}[Pasted Text 2 - 3 chars] z\nq[Image #3]"),
            (0, 1, true, "[Image #7][Pasted Text 2 - 3 chars] z\nq[Image #3]"),
            (0, 36, true, "\u{e9}[Image #7] z\nq[Image #3]"),
            (0, 36, false, "\u{e9}[Image #7][Pasted Text 2 - 3 chars]z\nq[Image #3]"),
            (0, 37, true, "\u{e9}[Image #7][Pasted Text 2 - 3 chars]z\nq[Image #3]"),
            (1, 11, true, "\u{e9}[Image #7][Pasted Text 2 - 3 chars] z\nq"),
        ] {
            let mut input = InputState::new();
            input.set_text("\u{e9}[Image #7][Pasted Text 2 - 3 chars] z\nq[Image #3]");
            input.set_cursor(row, col);
            if backward {
                input.textarea_delete_char_before();
            } else {
                input.textarea_delete_char_after();
            }
            assert_eq!(input.text(), expected, "row {row}, col {col}, backward {backward}");
        }
    }
}
