// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::{App, FocusTarget, dialog::DialogState, file_index};

/// Maximum candidates shown in the dropdown.
pub const MAX_VISIBLE: usize = 8;
/// Minimum query length before scanning the filesystem for matches.
pub const MIN_QUERY_CHARS: usize = 1;

pub struct MentionState {
    /// Character position (row, col) where the `@` was typed.
    pub trigger_row: usize,
    pub trigger_col: usize,
    /// Current query text after the `@` (e.g. "src/m" from "@src/m").
    pub query: String,
    /// Filtered + sorted candidates.
    pub candidates: Vec<file_index::FileCandidate>,
    /// Shared autocomplete dialog navigation state.
    pub dialog: DialogState,
    search_status: MentionSearchStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MentionSearchStatus {
    Hint,
    Searching,
    Ready,
    NoMatches,
}

impl MentionState {
    #[must_use]
    pub fn new(
        trigger_row: usize,
        trigger_col: usize,
        query: String,
        candidates: Vec<file_index::FileCandidate>,
    ) -> Self {
        let search_status = if candidates.is_empty() {
            MentionSearchStatus::Hint
        } else {
            MentionSearchStatus::Ready
        };
        Self {
            trigger_row,
            trigger_col,
            query,
            candidates,
            dialog: DialogState::default(),
            search_status,
        }
    }

    #[must_use]
    pub fn placeholder_message(&self) -> Option<String> {
        if !self.candidates.is_empty() {
            return None;
        }

        match self.search_status {
            MentionSearchStatus::Hint => Some("Type to search files".to_owned()),
            MentionSearchStatus::Searching => Some("Searching files...".to_owned()),
            MentionSearchStatus::NoMatches => Some("No matching files or folders".to_owned()),
            MentionSearchStatus::Ready => None,
        }
    }

    #[must_use]
    pub fn has_selectable_candidates(&self) -> bool {
        !self.candidates.is_empty()
    }

    fn mark_hint(&mut self) {
        self.candidates.clear();
        self.search_status = MentionSearchStatus::Hint;
        self.dialog.clamp(0, MAX_VISIBLE);
    }
}

/// Detect an `@` mention at the current cursor position.
/// Scans backwards from the cursor to find `@`. The `@` must be preceded by
/// whitespace, a newline, or be at position 0 (to avoid false triggers mid-word).
/// Returns `(trigger_row, trigger_col, query)` where `trigger_col` is the
/// position of the `@` character itself.
pub fn detect_mention_at_cursor(
    lines: &[String],
    cursor_row: usize,
    cursor_col: usize,
) -> Option<(usize, usize, String)> {
    let line = lines.get(cursor_row)?;
    let chars: Vec<char> = line.chars().collect();

    let mut i = cursor_col;
    while i > 0 {
        i -= 1;
        let ch = *chars.get(i)?;
        if ch == '@' {
            if i == 0 || chars.get(i - 1).is_some_and(|c| c.is_whitespace()) {
                let query: String = chars[i + 1..cursor_col].iter().collect();
                if query.chars().all(|c| !c.is_whitespace()) {
                    return Some((cursor_row, i, query));
                }
            }
            return None;
        }
        if ch.is_whitespace() {
            return None;
        }
    }
    None
}

/// Activate mention autocomplete after the user types `@`.
pub fn activate(app: &mut App) {
    let detection =
        detect_mention_at_cursor(app.input.lines(), app.input.cursor_row(), app.input.cursor_col());

    let Some((trigger_row, trigger_col, query)) = detection else {
        return;
    };

    app.mention = Some(MentionState::new(trigger_row, trigger_col, query, Vec::new()));
    app.slash = None;
    app.subagent = None;
    refresh_query_state(app);
}

/// Update the query and re-filter candidates while mention is active.
pub fn update_query(app: &mut App) {
    let detection =
        detect_mention_at_cursor(app.input.lines(), app.input.cursor_row(), app.input.cursor_col());

    let Some((trigger_row, trigger_col, query)) = detection else {
        deactivate(app);
        return;
    };

    if let Some(ref mut mention) = app.mention {
        mention.trigger_row = trigger_row;
        mention.trigger_col = trigger_col;
        mention.query = query;
    }

    refresh_query_state(app);
}

pub fn refresh_from_file_index(app: &mut App) {
    let Some(mention) = app.mention.as_mut() else {
        return;
    };

    if mention.query.chars().count() < MIN_QUERY_CHARS {
        mention.mark_hint();
        sync_focus(app);
        return;
    }

    mention.candidates = file_index::visible_candidates(&app.file_index.entries, &mention.query);
    mention.search_status = if mention.candidates.is_empty() {
        if app.file_index.scan_finished {
            MentionSearchStatus::NoMatches
        } else {
            MentionSearchStatus::Searching
        }
    } else if app.file_index.scan_finished {
        MentionSearchStatus::Ready
    } else {
        MentionSearchStatus::Searching
    };
    mention.dialog.clamp(mention.candidates.len(), MAX_VISIBLE);
    sync_focus(app);
}

fn refresh_query_state(app: &mut App) {
    let Some(mention) = app.mention.as_mut() else {
        return;
    };

    if mention.query.chars().count() < MIN_QUERY_CHARS {
        mention.mark_hint();
        sync_focus(app);
        return;
    }

    file_index::ensure_started(app);
    refresh_from_file_index(app);
}

fn sync_focus(app: &mut App) {
    if app.mention.as_ref().is_some_and(MentionState::has_selectable_candidates) {
        app.claim_focus_target(FocusTarget::Mention);
    } else {
        app.release_focus_target(FocusTarget::Mention);
    }
}

/// Keep mention state in sync with the current cursor location.
/// - If cursor is inside a valid `@mention` token, activate/update autocomplete.
/// - Otherwise, deactivate mention autocomplete.
pub fn sync_with_cursor(app: &mut App) {
    let in_mention =
        detect_mention_at_cursor(app.input.lines(), app.input.cursor_row(), app.input.cursor_col())
            .is_some();
    match (in_mention, app.mention.is_some()) {
        (true, true) => update_query(app),
        (true, false) => activate(app),
        (false, true) => deactivate(app),
        (false, false) => {}
    }
}

/// Confirm the selected candidate: replace `@query` in input with `@rel_path`.
pub fn confirm_selection(app: &mut App) {
    let Some(mention) = app.mention.take() else {
        return;
    };
    app.release_focus_target(FocusTarget::Mention);

    let Some(candidate) = mention.candidates.get(mention.dialog.selected) else {
        return;
    };

    let rel_path = candidate.rel_path.clone();
    let trigger_row = mention.trigger_row;
    let trigger_col = mention.trigger_col;

    let mut lines = app.input.lines().to_vec();
    let Some(line) = lines.get(trigger_row) else {
        return;
    };
    let chars: Vec<char> = line.chars().collect();
    if trigger_col >= chars.len() || chars[trigger_col] != '@' {
        return;
    }

    let mention_end =
        (trigger_col + 1..chars.len()).find(|&i| chars[i].is_whitespace()).unwrap_or(chars.len());

    let before: String = chars[..trigger_col].iter().collect();
    let after: String = chars[mention_end..].iter().collect();
    let replacement =
        if after.is_empty() { format!("@{rel_path} ") } else { format!("@{rel_path}") };

    let new_line = format!("{before}{replacement}{after}");
    let new_cursor_col = trigger_col + replacement.chars().count();

    lines[trigger_row] = new_line;
    app.input.replace_lines_and_cursor(lines, trigger_row, new_cursor_col);
}

/// Deactivate mention autocomplete.
pub fn deactivate(app: &mut App) {
    app.mention = None;
    if app.slash.is_none() && app.subagent.is_none() {
        app.release_focus_target(FocusTarget::Mention);
    }
}

/// Move selection up in the candidate list.
pub fn move_up(app: &mut App) {
    if let Some(ref mut mention) = app.mention {
        mention.dialog.move_up(mention.candidates.len(), MAX_VISIBLE);
    }
}

/// Move selection down in the candidate list.
pub fn move_down(app: &mut App) {
    if let Some(ref mut mention) = app.mention {
        mention.dialog.move_down(mention.candidates.len(), MAX_VISIBLE);
    }
}

/// Find all `@path` references in a text string. Returns `(start_byte, end_byte, path)` tuples.
/// A valid `@path` must start after whitespace or at position 0, and extends until
/// the next whitespace or end of string.
pub fn find_mention_spans(text: &str) -> Vec<(usize, usize, String)> {
    let mut spans = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '@' && (i == 0 || chars[i - 1].is_whitespace()) {
            let start = i;
            i += 1;
            let path_start = i;
            while i < chars.len() && !chars[i].is_whitespace() {
                i += 1;
            }
            if i > path_start {
                let path: String = chars[path_start..i].iter().collect();
                let byte_start: usize = chars[..start].iter().map(|c| c.len_utf8()).sum();
                let byte_end: usize = chars[..i].iter().map(|c| c.len_utf8()).sum();
                spans.push((byte_start, byte_end, path));
            }
        } else {
            i += 1;
        }
    }

    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use std::time::{Duration, SystemTime};

    fn app_with_temp_files(files: &[&str]) -> (App, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        for file in files {
            let path = tmp.path().join(file);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("create parent");
            }
            std::fs::write(&path, "").expect("write file");
        }
        let mut app = App::test_default();
        app.cwd_raw = tmp.path().to_string_lossy().into_owned();
        (app, tmp)
    }

    fn run_search(app: &mut App) {
        for _ in 0..200 {
            crate::app::file_index::drain_events(app);
            std::thread::sleep(Duration::from_millis(5));
            let is_settled = app.mention.as_ref().is_none_or(|mention| {
                !matches!(mention.search_status, MentionSearchStatus::Searching)
            });
            if is_settled {
                return;
            }
        }
    }

    #[test]
    fn sync_with_cursor_activates_inside_existing_mention() {
        let (mut app, _tmp) = app_with_temp_files(&["src/main.rs", "tests/integration.rs"]);
        app.input.set_text("open @src/main.rs now");
        let _ = app.input.set_cursor(0, "open @src".chars().count());

        sync_with_cursor(&mut app);
        run_search(&mut app);

        let mention = app.mention.as_ref().expect("mention should be active");
        assert_eq!(mention.query, "src");
        assert!(!mention.candidates.is_empty());
    }

    #[test]
    fn confirm_selection_replaces_full_existing_token_without_double_space() {
        let (mut app, _tmp) = app_with_temp_files(&["src/lib.rs"]);
        app.input.set_text("open @src/lib.txt now");
        let _ = app.input.set_cursor(0, "open @src/lib".chars().count());

        activate(&mut app);
        run_search(&mut app);
        confirm_selection(&mut app);

        assert_eq!(app.input.lines()[0], "open @src/lib.rs now");
        assert!(app.mention.is_none());
    }

    #[test]
    fn confirm_selection_at_end_keeps_trailing_space() {
        let (mut app, _tmp) = app_with_temp_files(&["src/main.rs"]);
        app.input.set_text("@src/mai");
        let _ = app.input.set_cursor(0, app.input.lines()[0].chars().count());

        activate(&mut app);
        run_search(&mut app);
        confirm_selection(&mut app);

        assert_eq!(app.input.lines()[0], "@src/main.rs ");
    }

    #[test]
    fn activate_with_empty_query_keeps_empty_candidates_until_threshold() {
        let (mut app, _tmp) = app_with_temp_files(&["src/main.rs"]);
        app.input.set_text("@");
        let _ = app.input.set_cursor(0, 1);

        activate(&mut app);

        let mention = app.mention.as_ref().expect("mention should be active");
        assert_eq!(mention.query, "");
        assert!(mention.candidates.is_empty());
        assert_eq!(mention.placeholder_message().as_deref(), Some("Type to search files"));
    }

    #[test]
    fn update_query_keeps_active_when_query_becomes_empty() {
        let (mut app, _tmp) = app_with_temp_files(&["src/main.rs"]);
        app.input.set_text("@src");
        let _ = app.input.set_cursor(0, app.input.lines()[0].chars().count());
        activate(&mut app);
        run_search(&mut app);
        assert!(app.mention.is_some());

        let _ = app.input.set_cursor_col(1);
        update_query(&mut app);

        let mention = app.mention.as_ref().expect("mention should stay active");
        assert_eq!(mention.query, "");
        assert!(mention.candidates.is_empty());
    }

    #[test]
    fn activate_hides_gitignored_files_by_default() {
        let (mut app, tmp) = app_with_temp_files(&["visible.rs", "ignored.rs"]);
        std::fs::create_dir_all(tmp.path().join(".git")).expect("create .git");
        std::fs::write(tmp.path().join(".gitignore"), "ignored.rs\n").expect("write .gitignore");
        app.input.set_text("@rs");
        let _ = app.input.set_cursor(0, 3);

        activate(&mut app);
        run_search(&mut app);

        let mention = app.mention.as_ref().expect("mention should be active");
        assert!(mention.candidates.iter().any(|candidate| candidate.rel_path == "visible.rs"));
        assert!(!mention.candidates.iter().any(|candidate| candidate.rel_path == "ignored.rs"));
    }

    #[test]
    fn activate_includes_gitignored_files_when_setting_is_disabled() {
        let (mut app, tmp) = app_with_temp_files(&["visible.rs", "ignored.rs"]);
        std::fs::create_dir_all(tmp.path().join(".git")).expect("create .git");
        std::fs::write(tmp.path().join(".gitignore"), "ignored.rs\n").expect("write .gitignore");
        crate::app::config::store::set_respect_gitignore(
            &mut app.config.committed_preferences_document,
            false,
        );
        app.input.set_text("@rs");
        let _ = app.input.set_cursor(0, 3);

        activate(&mut app);
        run_search(&mut app);

        let mention = app.mention.as_ref().expect("mention should be active");
        assert!(mention.candidates.iter().any(|candidate| candidate.rel_path == "visible.rs"));
        assert!(mention.candidates.iter().any(|candidate| candidate.rel_path == "ignored.rs"));
    }

    #[test]
    fn nested_gitignore_hides_same_directory_children() {
        let (mut app, _tmp) =
            app_with_temp_files(&["src/.gitignore", "src/visible.rs", "src/hidden.rs"]);
        let root = std::path::PathBuf::from(&app.cwd_raw);
        std::fs::create_dir_all(root.join(".git")).expect("create .git");
        std::fs::write(root.join("src").join(".gitignore"), "hidden.rs\n")
            .expect("write .gitignore");
        app.input.set_text("@rs");
        let _ = app.input.set_cursor(0, 3);

        activate(&mut app);
        run_search(&mut app);

        let mention = app.mention.as_ref().expect("mention should be active");
        assert!(mention.candidates.iter().any(|candidate| candidate.rel_path == "src/visible.rs"));
        assert!(!mention.candidates.iter().any(|candidate| candidate.rel_path == "src/hidden.rs"));
    }

    #[test]
    fn update_query_loads_candidates_once_threshold_is_reached() {
        let (mut app, _tmp) = app_with_temp_files(&["src/main.rs"]);
        app.input.set_text("@s");
        let _ = app.input.set_cursor(0, 2);

        activate(&mut app);
        assert!(app.mention.as_ref().is_some_and(|mention| mention.candidates.is_empty()));

        app.input.set_text("@sr");
        let _ = app.input.set_cursor(0, 3);
        update_query(&mut app);
        run_search(&mut app);

        let mention = app.mention.as_ref().expect("mention should remain active");
        assert_eq!(mention.query, "sr");
        assert!(!mention.candidates.is_empty());
    }

    #[test]
    fn progressive_search_publishes_shallow_matches_before_deeper_levels() {
        let (mut app, _tmp) =
            app_with_temp_files(&["root.rs", "src/nested/deep.rs", "src/other.txt"]);
        app.input.set_text("@rs");
        let _ = app.input.set_cursor(0, 3);

        activate(&mut app);
        run_search(&mut app);

        let mention = app.mention.as_ref().expect("mention should be active");
        assert!(mention.candidates.iter().any(|candidate| candidate.rel_path == "root.rs"));
        assert!(
            mention.candidates.iter().any(|candidate| candidate.rel_path == "src/nested/deep.rs")
        );
        assert!(matches!(mention.search_status, MentionSearchStatus::Ready));
    }

    #[test]
    fn query_change_refilters_from_cache_without_restarting_walk() {
        let (mut app, _tmp) =
            app_with_temp_files(&["root.rs", "src/nested/needle.rs", "src/nested/other.rs"]);
        app.input.set_text("@rs");
        let _ = app.input.set_cursor(0, 3);

        activate(&mut app);
        run_search(&mut app);
        let initial_generation = app.file_index.generation;
        assert!(app.mention.as_ref().is_some_and(|mention| {
            mention.candidates.iter().any(|candidate| candidate.rel_path == "root.rs")
        }));

        app.input.set_text("@needle");
        let _ = app.input.set_cursor(0, "@needle".chars().count());
        update_query(&mut app);

        let mention = app.mention.as_ref().expect("mention should remain active");
        assert_eq!(app.file_index.generation, initial_generation);
        assert_eq!(mention.candidates.len(), 1);
        assert_eq!(mention.candidates[0].rel_path, "src/nested/needle.rs");
    }

    #[test]
    fn basename_prefix_ranks_ahead_of_shallow_path_substring() {
        let mut candidates = vec![
            file_index::FileCandidate {
                rel_path: "docs/guide-rs.txt".to_owned(),
                rel_path_lower: "docs/guide-rs.txt".to_owned(),
                basename_lower: "guide-rs.txt".to_owned(),
                depth: 1,
                modified: SystemTime::UNIX_EPOCH,
                is_dir: false,
            },
            file_index::FileCandidate {
                rel_path: "src/rs-helper.rs".to_owned(),
                rel_path_lower: "src/rs-helper.rs".to_owned(),
                basename_lower: "rs-helper.rs".to_owned(),
                depth: 1,
                modified: SystemTime::UNIX_EPOCH,
                is_dir: false,
            },
        ];

        file_index::rank_and_truncate_candidates(&mut candidates, "rs");

        assert_eq!(candidates[0].rel_path, "src/rs-helper.rs");
    }
}
