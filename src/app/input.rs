// SPDX-License-Identifier: Apache-2.0

use tui_textarea::{AtomicRange, CursorMove, TextArea, WrapMode};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputSnapshot {
    pub lines: Vec<String>,
    pub cursor_row: usize,
    pub cursor_col: usize,
    pub paste_blocks: Vec<String>,
    /// Backing images retained while editor history can restore their badges.
    pub(crate) image_payloads: Vec<super::clipboard_image::ImageAttachment>,
}

#[derive(Debug)]
pub struct InputState {
    /// Monotonically increasing version counter. Bumped on every content or cursor change
    /// so that downstream caches (e.g. wrap result) can detect staleness cheaply.
    pub version: u64,
    /// Bumped only when visible input content changes (not cursor-only movement).
    /// Used to key wrap/highlight caches that don't depend on cursor position.
    pub content_version: u64,
    /// Stored paste blocks: each entry holds the full text of a large paste (>1000 chars).
    /// A placeholder token `[Pasted Text N]` is inserted into `lines` at the paste point.
    /// On `text()`, placeholders are expanded back to the original pasted content.
    pub paste_blocks: Vec<String>,
    /// Backing images retained while editor history can restore their badges.
    pub(crate) image_payloads: Vec<super::clipboard_image::ImageAttachment>,
    /// Cached visual line measurement: (`content_version`, width, `max_rows`, result).
    cached_measure: Option<(u64, u16, u16, u16)>,
    /// Tracks which `content_version` highlights were last applied for.
    /// Initialized to `u64::MAX` so the first render always applies highlights.
    pub highlight_version: u64,
    editor: TextArea<'static>,
}

/// Prefix/suffix used to identify paste placeholder lines in the input buffer.
const PASTE_PREFIX: &str = "[Pasted Text ";
const PASTE_SUFFIX: &str = "]";
/// Character threshold above which pasted content is collapsed to a placeholder.
pub const PASTE_PLACEHOLDER_CHAR_THRESHOLD: usize = 1000;

/// One local yank buffer owns both the deleted source and any referenced payloads.
#[derive(Clone, Debug)]
pub(crate) enum InputYankPart {
    Text(String),
    Image(super::clipboard_image::ImageAttachment),
    Paste(String),
}

impl InputState {
    fn configure_editor(editor: &mut TextArea<'static>) {
        editor.set_wrap_mode(WrapMode::WordOrGlyph);
    }

    fn bump_cursor_version(&mut self) {
        self.version += 1;
    }

    fn bump_content_version(&mut self) {
        self.version += 1;
        self.content_version += 1;
    }

    pub fn new() -> Self {
        let mut editor = TextArea::default();
        Self::configure_editor(&mut editor);
        Self {
            version: 0,
            content_version: 0,
            paste_blocks: Vec::new(),
            image_payloads: Vec::new(),
            cached_measure: None,
            highlight_version: u64::MAX,
            editor,
        }
    }

    #[must_use]
    pub fn editor(&self) -> &TextArea<'static> {
        &self.editor
    }

    pub fn editor_mut(&mut self) -> &mut TextArea<'static> {
        &mut self.editor
    }

    #[must_use]
    pub fn lines(&self) -> &[String] {
        self.editor.lines()
    }

    #[must_use]
    pub fn cursor(&self) -> (usize, usize) {
        self.editor.cursor()
    }

    #[must_use]
    pub fn cursor_row(&self) -> usize {
        self.cursor().0
    }

    #[must_use]
    pub fn cursor_col(&self) -> usize {
        self.cursor().1
    }

    pub fn set_cursor(&mut self, row: usize, col: usize) -> bool {
        self.apply_cursor_edit(|textarea| {
            textarea.move_cursor(CursorMove::Jump(
                u16::try_from(row).unwrap_or(u16::MAX),
                u16::try_from(col).unwrap_or(u16::MAX),
            ));
        })
    }

    pub fn set_cursor_col(&mut self, col: usize) -> bool {
        self.set_cursor(self.cursor_row(), col)
    }

    pub fn replace_lines_and_cursor(
        &mut self,
        mut lines: Vec<String>,
        cursor_row: usize,
        cursor_col: usize,
    ) {
        if lines.is_empty() {
            lines.push(String::new());
        }
        self.editor.set_lines(lines, (cursor_row, cursor_col));
        // Replacing the editor drops its history, so hidden payloads have no owner.
        self.discard_image_redo();
        self.refresh_atomic_ranges();
        self.bump_content_version();
    }

    fn refresh_atomic_ranges(&mut self) {
        let lines = self.lines().to_vec();
        let ranges = crate::app::input_atoms::atomic_ranges_for_textarea(&lines);
        self.editor.set_atomic_ranges(ranges);
    }

    #[must_use]
    pub fn atomic_ranges(&self) -> &[AtomicRange] {
        self.editor.atomic_ranges()
    }

    pub fn clear_custom_highlights(&mut self) {
        self.editor.clear_custom_highlight();
    }

    #[must_use]
    pub fn text(&self) -> String {
        if self.paste_blocks.is_empty() {
            return self.lines().join("\n");
        }
        // Expand paste placeholders back to their full content.
        let mut result = String::new();
        for (i, line) in self.lines().iter().enumerate() {
            if i > 0 {
                result.push('\n');
            }
            result.push_str(&expand_placeholders_in_line(line, &self.paste_blocks));
        }
        result
    }

    #[must_use]
    pub fn inline_pastes(&self) -> Vec<String> {
        crate::app::input_atoms::resolve_input_atoms(self.lines())
            .into_iter()
            .filter_map(|atom| match atom.kind {
                crate::app::input_atoms::InputAtomKind::PasteBlock { index, .. } => {
                    self.paste_blocks.get(index).cloned()
                }
                crate::app::input_atoms::InputAtomKind::ImageBadge { .. } => None,
            })
            .collect()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lines().len() == 1 && self.lines()[0].is_empty()
    }

    /// Visible badges select outgoing payloads; hidden images remain available to editor redo.
    pub(crate) fn images(&self) -> Vec<super::clipboard_image::ImageAttachment> {
        let ids: std::collections::BTreeSet<_> =
            crate::app::input_atoms::resolve_input_atoms(self.lines())
                .into_iter()
                .filter_map(|atom| match atom.kind {
                    crate::app::input_atoms::InputAtomKind::ImageBadge { one_based_index } => {
                        Some(one_based_index)
                    }
                    crate::app::input_atoms::InputAtomKind::PasteBlock { .. } => None,
                })
                .collect();
        self.image_payloads
            .iter()
            .enumerate()
            .filter(|(index, _)| ids.contains(&(index + 1)))
            .map(|(_, image)| image.clone())
            .collect()
    }

    fn visible_image_extent(&self) -> usize {
        crate::app::input_atoms::resolve_input_atoms(self.lines())
            .into_iter()
            .filter_map(|atom| match atom.kind {
                crate::app::input_atoms::InputAtomKind::ImageBadge { one_based_index } => {
                    Some(one_based_index)
                }
                crate::app::input_atoms::InputAtomKind::PasteBlock { .. } => None,
            })
            .max()
            .unwrap_or(0)
            .min(self.image_payloads.len())
    }

    fn discard_image_redo(&mut self) {
        self.image_payloads.truncate(self.visible_image_extent());
    }

    /// Insert one composer edit with its payloads. The editor remains the only undo authority.
    pub(crate) fn insert_parts(&mut self, parts: impl IntoIterator<Item = InputYankPart>) {
        use std::fmt::Write as _;
        self.discard_image_redo();
        let mut text = String::new();
        for part in parts {
            match part {
                InputYankPart::Text(value) => text.push_str(&value),
                InputYankPart::Image(image) => {
                    self.image_payloads.push(image);
                    let _ = write!(text, "[Image #{}]", self.image_payloads.len());
                }
                InputYankPart::Paste(value) => {
                    let placeholder = self.allocate_paste_block_for_insertion(&value, &text);
                    text.push_str(&placeholder);
                }
            }
        }
        self.insert_str_inner(&text);
    }

    pub fn clear(&mut self) {
        self.paste_blocks.clear();
        self.image_payloads.clear();
        self.replace_lines_and_cursor(vec![String::new()], 0, 0);
    }

    #[must_use]
    pub fn snapshot(&self) -> InputSnapshot {
        let (cursor_row, cursor_col) = self.cursor();
        InputSnapshot {
            lines: self.lines().to_vec(),
            cursor_row,
            cursor_col,
            paste_blocks: self.paste_blocks.clone(),
            image_payloads: self.image_payloads.clone(),
        }
    }

    pub fn restore_snapshot(&mut self, snapshot: InputSnapshot) {
        self.paste_blocks = snapshot.paste_blocks;
        self.image_payloads = snapshot.image_payloads;
        self.replace_lines_and_cursor(snapshot.lines, snapshot.cursor_row, snapshot.cursor_col);
    }

    /// Replace the input with the given text, placing the cursor at the end.
    pub fn set_text(&mut self, text: &str) {
        let normalized = normalize_line_endings(text);
        let mut lines: Vec<String> = normalized.split('\n').map(String::from).collect();
        if lines.is_empty() {
            lines.push(String::new());
        }
        let row = lines.len().saturating_sub(1);
        let col = lines[row].chars().count();
        self.paste_blocks.clear();
        self.image_payloads.clear();
        self.replace_lines_and_cursor(lines, row, col);
    }

    fn apply_cursor_edit(&mut self, edit: impl FnOnce(&mut TextArea<'_>)) -> bool {
        let before = self.editor.cursor();
        edit(&mut self.editor);
        let changed = before != self.editor.cursor();
        if changed {
            self.bump_cursor_version();
        }
        changed
    }

    pub fn textarea_insert_char(&mut self, c: char) -> bool {
        self.discard_image_redo();
        self.editor.insert_char(c);
        self.refresh_atomic_ranges();
        self.bump_content_version();
        true
    }

    pub fn textarea_insert_newline(&mut self) -> bool {
        self.discard_image_redo();
        self.editor.insert_newline();
        self.refresh_atomic_ranges();
        self.bump_content_version();
        true
    }

    pub fn textarea_delete_char_before(&mut self) -> bool {
        let image_extent = self.visible_image_extent();
        let changed = self.editor.delete_char();
        if changed {
            self.image_payloads.truncate(image_extent);
            self.refresh_atomic_ranges();
            self.bump_content_version();
        }
        changed
    }

    pub fn textarea_delete_char_after(&mut self) -> bool {
        let image_extent = self.visible_image_extent();
        let changed = self.editor.delete_next_char();
        if changed {
            self.image_payloads.truncate(image_extent);
            self.refresh_atomic_ranges();
            self.bump_content_version();
        }
        changed
    }

    pub fn textarea_move_left(&mut self) -> bool {
        self.apply_cursor_edit(|textarea| {
            textarea.move_cursor(CursorMove::Back);
        })
    }

    pub fn textarea_move_right(&mut self) -> bool {
        self.apply_cursor_edit(|textarea| {
            textarea.move_cursor(CursorMove::Forward);
        })
    }

    pub fn textarea_move_up(&mut self) -> bool {
        self.apply_cursor_edit(|textarea| {
            textarea.move_cursor(CursorMove::Up);
        })
    }

    pub fn textarea_move_down(&mut self) -> bool {
        self.apply_cursor_edit(|textarea| {
            textarea.move_cursor(CursorMove::Down);
        })
    }

    pub fn textarea_move_home(&mut self) -> bool {
        self.apply_cursor_edit(|textarea| {
            textarea.move_cursor(CursorMove::Head);
        })
    }

    pub fn textarea_move_end(&mut self) -> bool {
        self.apply_cursor_edit(|textarea| {
            textarea.move_cursor(CursorMove::End);
        })
    }

    pub fn textarea_move_line_start_or_up(&mut self) -> bool {
        let (row, col) = self.cursor();
        let target_row = if col == 0 { row.saturating_sub(1) } else { row };
        self.set_cursor(target_row, 0)
    }

    pub fn textarea_move_line_end_or_down(&mut self) -> bool {
        let (row, col) = self.cursor();
        let current_line_end = self.lines()[row].chars().count();
        let target_row =
            if col == current_line_end && row + 1 < self.lines().len() { row + 1 } else { row };
        let target_col = self.lines()[target_row].chars().count();
        self.set_cursor(target_row, target_col)
    }

    pub fn textarea_undo(&mut self) -> bool {
        let changed = self.editor.undo();
        if changed {
            self.refresh_atomic_ranges();
            self.bump_content_version();
        }
        changed
    }

    pub fn textarea_redo(&mut self) -> bool {
        let changed = self.editor.redo();
        if changed {
            self.refresh_atomic_ranges();
            self.bump_content_version();
        }
        changed
    }

    pub fn textarea_move_word_left(&mut self) -> bool {
        self.apply_cursor_edit(|textarea| {
            textarea.move_cursor(CursorMove::WordBack);
        })
    }

    pub fn textarea_move_word_right(&mut self) -> bool {
        self.apply_cursor_edit(|textarea| {
            textarea.move_cursor(CursorMove::WordForward);
        })
    }

    pub fn textarea_delete_word_before(&mut self) -> bool {
        let image_extent = self.visible_image_extent();
        let changed = self.editor.delete_word();
        if changed {
            self.image_payloads.truncate(image_extent);
            self.refresh_atomic_ranges();
            self.bump_content_version();
        }
        changed
    }

    pub fn textarea_delete_word_after(&mut self) -> bool {
        let image_extent = self.visible_image_extent();
        let changed = self.editor.delete_next_word();
        if changed {
            self.image_payloads.truncate(image_extent);
            self.refresh_atomic_ranges();
            self.bump_content_version();
        }
        changed
    }

    pub fn textarea_delete_line_before(&mut self) -> bool {
        let image_extent = self.visible_image_extent();
        let changed = self.editor.delete_line_by_head();
        if changed {
            self.image_payloads.truncate(image_extent);
            self.refresh_atomic_ranges();
            self.bump_content_version();
        }
        changed
    }

    pub fn textarea_delete_line_after(&mut self) -> bool {
        let image_extent = self.visible_image_extent();
        let changed = self.editor.delete_line_by_end();
        if changed {
            self.image_payloads.truncate(image_extent);
            self.refresh_atomic_ranges();
            self.bump_content_version();
        }
        changed
    }

    /// Transfer the editor's killed text to the composer, which owns its atom payloads.
    pub(super) fn take_yank_text(&mut self) -> String {
        let text = self.editor.yank_text();
        self.editor.set_yank_text("");
        text
    }

    pub fn insert_str(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        self.discard_image_redo();
        self.insert_str_inner(s);
    }

    fn insert_str_inner(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        let normalized = normalize_line_endings(s);
        if normalized.is_empty() {
            return;
        }
        let changed = self.editor.insert_str(&normalized);
        if changed {
            self.refresh_atomic_ranges();
            self.bump_content_version();
        }
    }

    /// Insert a large paste as a compact placeholder token at the cursor.
    /// The full text is stored in `paste_blocks` and expanded by `text()` on submit.
    /// Returns the placeholder label for display purposes.
    pub fn insert_paste_block(&mut self, text: &str) -> String {
        let placeholder = self.allocate_paste_block_placeholder(text);
        let (cursor_row, cursor_col) = self.cursor();
        let mut lines = self.lines().to_vec();
        let Some(current_line) = lines.get_mut(cursor_row) else {
            return placeholder;
        };
        let byte_idx = char_to_byte_index(current_line, cursor_col);
        current_line.insert_str(byte_idx, &placeholder);
        let new_cursor_col = cursor_col + placeholder.chars().count();
        self.replace_lines_and_cursor(lines, cursor_row, new_cursor_col);
        placeholder
    }

    /// Store the full text as a paste block and return its placeholder label.
    pub fn allocate_paste_block_placeholder(&mut self, text: &str) -> String {
        self.allocate_paste_block_for_insertion(text, "")
    }

    /// Reserve a distinct block while assembling one editor insertion with multiple atoms.
    pub(super) fn allocate_paste_block_for_insertion(
        &mut self,
        text: &str,
        pending: &str,
    ) -> String {
        let normalized = normalize_line_endings(text);
        let mut used_lines = self.lines().to_vec();
        used_lines.extend(pending.split('\n').map(str::to_owned));
        let idx = next_free_paste_block_index(&used_lines, &self.paste_blocks);
        if idx < self.paste_blocks.len() {
            normalized.clone_into(&mut self.paste_blocks[idx]);
        } else {
            self.paste_blocks.push(normalized.clone());
        }
        paste_placeholder_label(idx, count_text_chars(&normalized))
    }

    /// Append text to the paste block under the cursor if the cursor currently
    /// sits at the end of a placeholder token.
    ///
    /// This handles terminals that deliver one clipboard paste as multiple
    /// `Event::Paste` chunks.
    pub fn append_to_active_paste_block(&mut self, text: &str) -> bool {
        let (cursor_row, cursor_col) = self.cursor();
        let Some(current_line) = self.lines().get(cursor_row).cloned() else {
            return false;
        };
        let Some((start, end, idx)) = find_placeholder_ending_at_col(&current_line, cursor_col)
        else {
            return false;
        };

        let normalized = normalize_line_endings(text);
        let Some(block) = self.paste_blocks.get_mut(idx) else {
            return false;
        };
        block.push_str(&normalized);

        let head = &current_line[..start];
        let tail = &current_line[end..];
        let placeholder = paste_placeholder_label(idx, count_text_chars(block));
        let mut lines = self.lines().to_vec();
        lines[cursor_row] = format!("{head}{placeholder}{tail}");
        let new_col = head.chars().count() + placeholder.chars().count();
        self.replace_lines_and_cursor(lines, cursor_row, new_col);
        true
    }

    pub fn measure_visual_lines(&mut self, content_width: u16, max_rows: u16) -> u16 {
        if let Some((v, w, m, result)) = self.cached_measure
            && v == self.content_version
            && w == content_width
            && m == max_rows
        {
            return result;
        }

        let result = if content_width == 0 {
            1
        } else {
            self.editor.set_min_rows(1);
            self.editor.set_max_rows(max_rows);
            self.editor.measure(content_width).preferred_rows
        };

        self.cached_measure = Some((self.content_version, content_width, max_rows, result));
        result
    }

    /// Renumber all `[Image #N]` badges in the input to match the current
    /// attachment indices (1-based). Call after removing an image.
    pub fn renumber_image_badges(&mut self) {
        let mut lines = self.lines().to_vec();
        let (cursor_row, cursor_col) = self.cursor();
        let mut new_cursor_col = cursor_col;
        let mut counter = 0usize;
        for (row_idx, line) in lines.iter_mut().enumerate() {
            let atoms = crate::app::input_atoms::resolve_line_atoms(row_idx, line);
            let mut result = String::with_capacity(line.len());
            let mut search_from = 0usize;
            let mut col_delta: isize = 0;
            for atom in atoms {
                if !matches!(atom.kind, crate::app::input_atoms::InputAtomKind::ImageBadge { .. }) {
                    continue;
                }
                counter += 1;
                let new_badge = format!("[Image #{counter}]");
                result.push_str(&line[search_from..atom.start_byte]);
                result.push_str(&new_badge);
                if row_idx == cursor_row && cursor_col >= atom.end_col {
                    let old_len = atom.end_col.saturating_sub(atom.start_col);
                    let new_len = new_badge.chars().count();
                    col_delta += new_len.cast_signed() - old_len.cast_signed();
                }
                search_from = atom.end_byte;
            }
            result.push_str(&line[search_from..]);
            *line = result;
            if row_idx == cursor_row {
                new_cursor_col =
                    usize::try_from(cursor_col.cast_signed() + col_delta).unwrap_or(cursor_col);
            }
        }
        self.replace_lines_and_cursor(lines, cursor_row, new_cursor_col);
    }
}

impl Default for InputState {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert a character index to a byte index within a string.
pub(crate) fn char_to_byte_index(s: &str, char_idx: usize) -> usize {
    s.char_indices().nth(char_idx).map_or(s.len(), |(i, _)| i)
}

/// Normalize mixed line endings to `\n` while preserving trailing blank lines.
fn normalize_line_endings(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\r' {
            if chars.peek().is_some_and(|next| *next == '\n') {
                let _ = chars.next();
            }
            out.push('\n');
        } else {
            out.push(ch);
        }
    }
    out
}

/// Count Unicode scalar characters in a text payload.
#[must_use]
pub fn count_text_chars(text: &str) -> usize {
    text.chars().count()
}

fn paste_placeholder_label(idx: usize, char_count: usize) -> String {
    format!("{PASTE_PREFIX}{} - {char_count} chars{PASTE_SUFFIX}", idx + 1)
}

fn find_next_placeholder_with_suffix(
    line: &str,
    search_from: usize,
) -> Option<(usize, usize, usize)> {
    crate::app::input_atoms::resolve_line_atoms(0, line).into_iter().find_map(|atom| {
        if atom.start_byte < search_from {
            return None;
        }
        match atom.kind {
            crate::app::input_atoms::InputAtomKind::PasteBlock { index, .. } => {
                Some((atom.start_byte, atom.end_byte, index))
            }
            crate::app::input_atoms::InputAtomKind::ImageBadge { .. } => None,
        }
    })
}

fn expand_placeholders_in_line(line: &str, paste_blocks: &[String]) -> String {
    let mut out = String::new();
    let mut cursor = 0usize;
    while let Some((start, end, idx)) = find_next_placeholder_with_suffix(line, cursor) {
        out.push_str(&line[cursor..start]);
        if let Some(content) = paste_blocks.get(idx) {
            out.push_str(content);
        } else {
            out.push_str(&line[start..end]);
        }
        cursor = end;
    }
    if cursor == 0 {
        return line.to_owned();
    }
    out.push_str(&line[cursor..]);
    out
}

fn next_free_paste_block_index(lines: &[String], paste_blocks: &[String]) -> usize {
    let mut used = vec![false; paste_blocks.len()];
    for line in lines {
        let mut cursor = 0usize;
        while let Some((_, end, idx)) = find_next_placeholder_with_suffix(line, cursor) {
            if idx < used.len() {
                used[idx] = true;
            }
            cursor = end;
        }
    }
    used.iter().position(|in_use| !*in_use).unwrap_or(paste_blocks.len())
}

fn find_placeholder_ending_at_col(line: &str, cursor_col: usize) -> Option<(usize, usize, usize)> {
    let cursor_byte = char_to_byte_index(line, cursor_col);
    let mut search_from = 0usize;
    while let Some((start, end, idx)) = find_next_placeholder_with_suffix(line, search_from) {
        if end == cursor_byte {
            return Some((start, end, idx));
        }
        if end > cursor_byte {
            return None;
        }
        search_from = end;
    }
    None
}

/// Parse the placeholder index directly before the cursor, even when the
/// placeholder is embedded within a line.
pub fn parse_paste_placeholder_before_cursor(line: &str, cursor_col: usize) -> Option<usize> {
    find_placeholder_ending_at_col(line, cursor_col).map(|(_, _, idx)| idx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use tui_textarea::AtomicRange;

    // char_to_byte_index

    #[test]
    fn char_to_byte_index_handles_ascii_unicode_and_bounds() {
        let cases = [
            ("hello", 0, 0),
            ("hello", 2, 2),
            ("hello", 5, 5),
            ("cafe\u{0301}", 4, 4),
            ("\u{1F600}hello", 0, 0),
            ("\u{1F600}hello", 1, 4),
            ("ab", 10, 2),
            ("", 0, 0),
            ("", 5, 0),
        ];

        for (text, char_index, expected) in cases {
            assert_eq!(char_to_byte_index(text, char_index), expected, "{text:?} at {char_index}");
        }
    }

    // InputState::new / Default

    #[test]
    fn move_line_start_or_up_stops_at_each_logical_line_start() {
        let mut input = InputState::new();
        input.set_text("alpha\nbeta\ngamma");
        let _ = input.set_cursor(1, 2);

        assert!(input.textarea_move_line_start_or_up());
        assert_eq!(input.cursor(), (1, 0));

        assert!(input.textarea_move_line_start_or_up());
        assert_eq!(input.cursor(), (0, 0));

        assert!(!input.textarea_move_line_start_or_up());
        assert_eq!(input.cursor(), (0, 0));
    }

    #[test]
    fn move_line_end_or_down_stops_at_each_logical_line_end() {
        let mut input = InputState::new();
        input.set_text("alpha\nbeta\ngamma");
        let _ = input.set_cursor(1, 2);

        assert!(input.textarea_move_line_end_or_down());
        assert_eq!(input.cursor(), (1, 4));

        assert!(input.textarea_move_line_end_or_down());
        assert_eq!(input.cursor(), (2, 5));

        assert!(!input.textarea_move_line_end_or_down());
        assert_eq!(input.cursor(), (2, 5));
    }

    #[test]
    fn line_boundary_navigation_handles_empty_and_unicode_lines() {
        let mut input = InputState::new();
        input.set_text("α\n\n猫犬");

        let _ = input.set_cursor(2, 0);
        assert!(input.textarea_move_line_start_or_up());
        assert_eq!(input.cursor(), (1, 0));
        assert!(input.textarea_move_line_start_or_up());
        assert_eq!(input.cursor(), (0, 0));

        let _ = input.set_cursor(0, 1);
        assert!(input.textarea_move_line_end_or_down());
        assert_eq!(input.cursor(), (1, 0));
        assert!(input.textarea_move_line_end_or_down());
        assert_eq!(input.cursor(), (2, 2));
    }

    // line_count

    #[test]
    fn set_text_registers_atomic_ranges() {
        let mut input = InputState::new();
        input.set_text("é [Image #1] [Pasted Text 2 - 3 chars]");

        assert_eq!(
            input.atomic_ranges(),
            &[
                AtomicRange {
                    row: 0,
                    start_col: "é ".chars().count(),
                    end_col: "é [Image #1]".chars().count(),
                },
                AtomicRange {
                    row: 0,
                    start_col: "é [Image #1] ".chars().count(),
                    end_col: "é [Image #1] [Pasted Text 2 - 3 chars]".chars().count(),
                },
            ]
        );

        let mut input = InputState::new();
        input.set_text("\u{e9}[Pasted Text 2 - 3 chars][Image #7]\n\u{732b} [Image #3]");
        assert_eq!(
            input.atomic_ranges(),
            &[
                AtomicRange { row: 0, start_col: 1, end_col: 26 },
                AtomicRange { row: 0, start_col: 26, end_col: 36 },
                AtomicRange { row: 1, start_col: 2, end_col: 12 },
            ]
        );
        input.set_cursor(0, 1);
        input.textarea_insert_char('!');
        assert_eq!(
            input.atomic_ranges(),
            &[
                AtomicRange { row: 0, start_col: 2, end_col: 27 },
                AtomicRange { row: 0, start_col: 27, end_col: 37 },
                AtomicRange { row: 1, start_col: 2, end_col: 12 },
            ]
        );
        input.set_cursor(1, 2);
        input.textarea_delete_char_after();
        assert_eq!(input.lines(), ["\u{e9}![Pasted Text 2 - 3 chars][Image #7]", "\u{732b} "]);
        assert_eq!(
            input.atomic_ranges(),
            &[
                AtomicRange { row: 0, start_col: 2, end_col: 27 },
                AtomicRange { row: 0, start_col: 27, end_col: 37 },
            ]
        );
    }

    #[test]
    fn atomic_deletion_refreshes_ranges_and_undo_restores_them() {
        let mut input = InputState::new();
        input.set_text("a[Image #1]b");
        let _ = input.set_cursor_col("a[Image #1]".chars().count());

        let deleted = input.textarea_delete_char_before();

        assert!(deleted);
        assert_eq!(input.text(), "ab");
        assert!(input.atomic_ranges().is_empty());

        assert!(input.textarea_undo());
        assert_eq!(input.text(), "a[Image #1]b");
        assert_eq!(input.atomic_ranges().len(), 1);
    }

    #[test]
    fn paste_block_storage_normalizes_line_endings() {
        let mut input = InputState::new();

        input.insert_paste_block("a\r\nb\rc");

        assert_eq!(input.lines(), vec!["[Pasted Text 1 - 5 chars]"]);
        assert_eq!(input.paste_blocks, vec!["a\nb\nc"]);
        assert_eq!(input.text(), "a\nb\nc");
    }

    #[test]
    fn parse_placeholder_before_cursor_detects_inline_placeholder() {
        let line = "a[Pasted Text 2 - 42 chars]tail";
        let cursor_col = "a[Pasted Text 2 - 42 chars]".chars().count();
        assert_eq!(parse_paste_placeholder_before_cursor(line, cursor_col), Some(1));
    }

    #[test]
    fn text_expands_placeholder_even_with_trailing_text() {
        let mut input = InputState::new();
        input.insert_paste_block("line1\nline2");
        input.replace_lines_and_cursor(vec!["[Pasted Text 1 - 11 chars] + extra".to_owned()], 0, 0);
        let _ = input.set_cursor_col(input.lines()[0].chars().count());
        assert_eq!(input.text(), "line1\nline2 + extra");
    }

    #[test]
    fn insert_paste_block_inserts_inline_at_cursor() {
        let mut input = InputState::new();
        input.insert_str("beforeafter");
        let _ = input.set_cursor_col("before".chars().count());
        input.insert_paste_block(&"x".repeat(1001));

        assert_eq!(input.lines(), vec!["before[Pasted Text 1 - 1001 chars]after"]);
        assert_eq!(input.text(), format!("before{}after", "x".repeat(1001)));
    }

    #[test]
    fn inline_pastes_follow_active_placeholders_in_document_order() {
        let mut input = InputState::new();
        input.insert_paste_block(&"a".repeat(1_001));
        input.insert_str(" between ");
        input.insert_paste_block(&"b".repeat(1_002));

        assert_eq!(input.inline_pastes(), vec!["a".repeat(1_001), "b".repeat(1_002)]);
    }

    #[test]
    fn insert_paste_block_reuses_number_after_placeholder_removed() {
        let mut input = InputState::new();
        input.insert_paste_block(&"a".repeat(1001));
        input.replace_lines_and_cursor(vec![String::new()], 0, 0);
        let _ = input.set_cursor_col(0);

        input.insert_paste_block(&"b".repeat(1001));

        assert_eq!(input.lines(), vec!["[Pasted Text 1 - 1001 chars]"]);
        assert_eq!(input.text(), "b".repeat(1001));
    }

    #[test]
    fn append_to_active_paste_block_merges_chunks_and_updates_label() {
        let mut input = InputState::new();
        let original = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk";
        input.insert_paste_block(original);
        assert!(input.append_to_active_paste_block("\r\nl\rm"));
        assert_eq!(input.lines()[0], "[Pasted Text 1 - 25 chars]");
        assert_eq!(input.text(), "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\nm");
    }

    #[test]
    fn append_to_active_paste_block_supports_inline_placeholder() {
        let mut input = InputState::new();
        input.insert_str("prefix-suffix");
        let _ = input.set_cursor_col("prefix-".chars().count());
        input.insert_paste_block("abc");
        assert_eq!(input.lines()[0], "prefix-[Pasted Text 1 - 3 chars]suffix");
        assert!(input.append_to_active_paste_block("XYZ"));
        assert_eq!(input.lines()[0], "prefix-[Pasted Text 1 - 6 chars]suffix");
        assert_eq!(input.text(), "prefix-abcXYZsuffix");
    }

    #[test]
    fn append_to_active_paste_block_rejects_dirty_placeholder_line() {
        let mut input = InputState::new();
        input.insert_paste_block("a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk");
        input.replace_lines_and_cursor(vec!["[Pasted Text 1 - 21 chars]tail".to_owned()], 0, 0);
        let _ = input.set_cursor_col(input.lines()[0].chars().count());
        assert!(!input.append_to_active_paste_block("x"));
    }

    #[test]
    fn clear_discards_paste_payloads_and_restarts_placeholder_allocation() {
        let mut input = InputState::new();
        input.insert_str("old ");
        input.insert_paste_block(&"x".repeat(1001));
        input.insert_paste_block(&"y".repeat(1002));
        input.clear();
        assert_eq!(input.text(), "");
        assert_eq!(input.cursor(), (0, 0));
        assert!(input.inline_pastes().is_empty());
        assert!(input.paste_blocks.is_empty());
        input.insert_paste_block(&"n".repeat(1003));
        assert_eq!(input.lines(), ["[Pasted Text 1 - 1003 chars]"]);
        assert_eq!(input.text(), "n".repeat(1003));
        assert_eq!(input.inline_pastes(), ["n".repeat(1003)]);
    }

    #[test]
    fn insertion_normalizes_all_line_endings_without_losing_source_or_suffix() {
        for (source, expected) in [
            ("a\rb\rc", vec!["La", "b", "cR"]),
            ("a\r\nb\r\nc", vec!["La", "b", "cR"]),
            ("a\rb\nc\r\nd", vec!["La", "b", "c", "dR"]),
            ("\r\n\u{e9}\r\u{732b}\n", vec!["L", "\u{e9}", "\u{732b}", "R"]),
        ] {
            let mut input = InputState::new();
            input.set_text("LR");
            input.set_cursor(0, 1);
            input.insert_str(source);
            assert_eq!(input.lines(), expected, "{source:?}");
        }
    }
}
