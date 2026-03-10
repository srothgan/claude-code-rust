// Claude Code Rust - A native Rust terminal interface for Claude Code
// Copyright (C) 2025  Simon Peter Rothgang
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as
// published by the Free Software Foundation, either version 3 of the
// License, or (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

use super::{App, FocusTarget, dialog::DialogState};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::cmp::Ordering;
use std::collections::{HashSet, VecDeque};
use std::fs::{self, ReadDir};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

/// Maximum candidates shown in the dropdown.
pub const MAX_VISIBLE: usize = 8;

/// Maximum total candidates kept after filtering.
const MAX_CANDIDATES: usize = 50;
/// Minimum query length before scanning the filesystem for matches.
pub const MIN_QUERY_CHARS: usize = 1;
/// Maximum filesystem entries scanned within one tick.
const SEARCH_ENTRY_BUDGET: usize = 400;

pub struct MentionState {
    /// Character position (row, col) where the `@` was typed.
    pub trigger_row: usize,
    pub trigger_col: usize,
    /// Current query text after the `@` (e.g. "src/m" from "@src/m").
    pub query: String,
    /// Filtered + sorted candidates.
    pub candidates: Vec<FileCandidate>,
    /// Shared autocomplete dialog navigation state.
    pub dialog: DialogState,
    search_status: MentionSearchStatus,
    search_session: Option<MentionSearchSession>,
}

#[derive(Clone)]
pub struct FileCandidate {
    /// Relative path from cwd (forward slashes, e.g. "src/main.rs").
    /// Directories have a trailing `/` (e.g. "src/").
    pub rel_path: String,
    /// Depth (number of `/` separators) for grouping.
    pub depth: usize,
    /// Last modified time for sorting within depth groups.
    pub modified: SystemTime,
    /// Whether this candidate is a directory (true) or a file (false).
    pub is_dir: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MentionSearchStatus {
    Hint,
    Searching,
    Ready,
    NoMatches,
}

struct MentionSearchSession {
    root: PathBuf,
    query_lower: String,
    current_level: VecDeque<PathBuf>,
    next_level: VecDeque<PathBuf>,
    active_dir: Option<ActiveDirectoryScan>,
    candidates: Vec<FileCandidate>,
    finished: bool,
    ignore_state: Option<IgnoreState>,
}

struct ActiveDirectoryScan {
    read_dir: ReadDir,
}

struct IgnoreState {
    root: PathBuf,
    local_builder: GitignoreBuilder,
    local_matcher: Gitignore,
    global_matcher: Gitignore,
    git_exclude_matcher: Gitignore,
    loaded_gitignores: HashSet<PathBuf>,
}

impl MentionState {
    #[must_use]
    pub fn new(
        trigger_row: usize,
        trigger_col: usize,
        query: String,
        candidates: Vec<FileCandidate>,
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
            search_session: None,
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
        self.search_session = None;
        self.dialog.clamp(0, MAX_VISIBLE);
    }

    fn start_search(&mut self, cwd: &str, respect_gitignore: bool) {
        self.candidates.clear();
        self.search_status = MentionSearchStatus::Searching;
        self.search_session =
            Some(MentionSearchSession::new(PathBuf::from(cwd), respect_gitignore, &self.query));
        self.dialog.clamp(0, MAX_VISIBLE);
    }

    fn advance_search(&mut self) {
        let Some(search_session) = self.search_session.as_mut() else {
            self.search_status = MentionSearchStatus::NoMatches;
            self.dialog.clamp(0, MAX_VISIBLE);
            return;
        };

        search_session.scan_chunk();
        self.candidates = search_session.candidates.clone();
        self.search_status = if search_session.finished {
            if self.candidates.is_empty() {
                MentionSearchStatus::NoMatches
            } else {
                MentionSearchStatus::Ready
            }
        } else {
            MentionSearchStatus::Searching
        };
        self.dialog.clamp(self.candidates.len(), MAX_VISIBLE);
    }

    fn invalidate_session_cache(&mut self) {
        self.search_session = None;
    }
}

impl MentionSearchSession {
    fn new(root: PathBuf, respect_gitignore: bool, query: &str) -> Self {
        let ignore_state = respect_gitignore.then(|| IgnoreState::new(&root));
        let mut current_level = VecDeque::new();
        current_level.push_back(root.clone());

        Self {
            root,
            query_lower: query.to_lowercase(),
            current_level,
            next_level: VecDeque::new(),
            active_dir: None,
            candidates: Vec::new(),
            finished: false,
            ignore_state,
        }
    }

    fn scan_chunk(&mut self) {
        if self.finished {
            return;
        }

        let mut entries_processed = 0;

        while entries_processed < SEARCH_ENTRY_BUDGET {
            if self.active_dir.is_none() && !self.open_next_directory() {
                break;
            }

            let next_entry = {
                let Some(active_dir) = self.active_dir.as_mut() else {
                    continue;
                };
                active_dir.read_dir.next()
            };

            match next_entry {
                Some(Ok(entry)) => {
                    entries_processed += 1;
                    self.process_entry(&entry);
                }
                Some(Err(_)) => {
                    entries_processed += 1;
                }
                None => {
                    self.active_dir = None;
                }
            }
        }

        rank_and_truncate_candidates(&mut self.candidates, &self.query_lower);
    }

    fn open_next_directory(&mut self) -> bool {
        loop {
            if let Some(dir) = self.current_level.pop_front() {
                if let Some(ignore_state) = self.ignore_state.as_mut() {
                    ignore_state.add_directory_gitignore(&dir);
                }

                match fs::read_dir(&dir) {
                    Ok(read_dir) => {
                        self.active_dir = Some(ActiveDirectoryScan { read_dir });
                        return true;
                    }
                    Err(_) => continue,
                }
            }

            if self.next_level.is_empty() {
                self.finished = true;
                return false;
            }

            self.current_level = std::mem::take(&mut self.next_level);
            return false;
        }
    }

    fn process_entry(&mut self, entry: &fs::DirEntry) {
        let Ok(file_type) = entry.file_type() else {
            return;
        };
        let is_dir = file_type.is_dir();
        let is_file = file_type.is_file();
        if !is_dir && !is_file {
            return;
        }

        let path = entry.path();
        if self.is_ignored(&path, is_dir) {
            return;
        }

        if is_dir {
            self.next_level.push_back(path.clone());
        }

        let Some(candidate) = candidate_from_path(&self.root, &path, is_dir, entry.metadata().ok())
        else {
            return;
        };

        if match_tier(&candidate, &self.query_lower).is_some() {
            self.candidates.push(candidate);
        }
    }

    fn is_ignored(&self, path: &Path, is_dir: bool) -> bool {
        let Some(ignore_state) = self.ignore_state.as_ref() else {
            return false;
        };
        ignore_state.is_ignored(path, is_dir)
    }
}

impl IgnoreState {
    fn new(root: &Path) -> Self {
        let mut local_builder = GitignoreBuilder::new(root);
        let mut loaded_gitignores = HashSet::new();
        for ancestor_ignore in ancestor_gitignore_paths(root) {
            let _ = local_builder.add(&ancestor_ignore);
            loaded_gitignores.insert(ancestor_ignore);
        }
        let local_matcher = local_builder.build().unwrap_or_else(|_| Gitignore::empty());

        let (global_matcher, _) = GitignoreBuilder::new(root).build_global();
        let git_exclude_matcher = find_git_exclude_matcher(root);

        Self {
            root: root.to_path_buf(),
            local_builder,
            local_matcher,
            global_matcher,
            git_exclude_matcher,
            loaded_gitignores,
        }
    }

    fn add_directory_gitignore(&mut self, dir: &Path) {
        let gitignore_path = dir.join(".gitignore");
        if !gitignore_path.is_file() || !self.loaded_gitignores.insert(gitignore_path.clone()) {
            return;
        }

        let _ = self.local_builder.add(&gitignore_path);
        if let Ok(matcher) = self.local_builder.build() {
            self.local_matcher = matcher;
        }
    }

    fn is_ignored(&self, path: &Path, is_dir: bool) -> bool {
        let Ok(relative_path) = path.strip_prefix(&self.root) else {
            return false;
        };

        let local = self.local_matcher.matched_path_or_any_parents(relative_path, is_dir);
        if local.is_ignore() {
            return true;
        }
        if local.is_whitelist() {
            return false;
        }

        let git_exclude = self.git_exclude_matcher.matched(path, is_dir);
        if git_exclude.is_ignore() {
            return true;
        }
        if git_exclude.is_whitelist() {
            return false;
        }

        self.global_matcher.matched(path, is_dir).is_ignore()
    }
}

fn ancestor_gitignore_paths(root: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for ancestor in root.ancestors().skip(1) {
        let path = ancestor.join(".gitignore");
        if path.is_file() {
            paths.push(path);
        }
    }
    paths.reverse();
    paths
}

fn find_git_exclude_matcher(root: &Path) -> Gitignore {
    for ancestor in root.ancestors() {
        let git_exclude = ancestor.join(".git").join("info").join("exclude");
        if git_exclude.is_file() {
            let (matcher, _) = Gitignore::new(git_exclude);
            return matcher;
        }
    }
    Gitignore::empty()
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
    refresh_query_state(app, Instant::now());
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

    refresh_query_state(app, Instant::now());
}

pub fn tick(app: &mut App, now: Instant) {
    let Some(mention) = app.mention.as_mut() else {
        return;
    };

    match mention.search_status {
        MentionSearchStatus::Searching => {
            mention.advance_search();
            sync_focus(app);
        }
        MentionSearchStatus::Hint | MentionSearchStatus::Ready | MentionSearchStatus::NoMatches => {
            let _ = now;
        }
    }
}

pub fn invalidate_session_cache(app: &mut App) {
    if let Some(mention) = app.mention.as_mut() {
        mention.invalidate_session_cache();
        if mention.query.chars().count() < MIN_QUERY_CHARS {
            mention.mark_hint();
        } else {
            mention.start_search(&app.cwd_raw, app.settings.respect_gitignore_effective());
        }
    }
    sync_focus(app);
}

fn refresh_query_state(app: &mut App, _now: Instant) {
    let Some(mention) = app.mention.as_mut() else {
        return;
    };

    if mention.query.chars().count() < MIN_QUERY_CHARS {
        mention.mark_hint();
        sync_focus(app);
        return;
    }

    mention.start_search(&app.cwd_raw, app.settings.respect_gitignore_effective());
    sync_focus(app);
}

fn sync_focus(app: &mut App) {
    if app.mention.as_ref().is_some_and(MentionState::has_selectable_candidates) {
        app.claim_focus_target(FocusTarget::Mention);
    } else {
        app.release_focus_target(FocusTarget::Mention);
    }
}

fn candidate_from_path(
    root: &Path,
    path: &Path,
    is_dir: bool,
    metadata: Option<fs::Metadata>,
) -> Option<FileCandidate> {
    let rel = path.strip_prefix(root).ok()?;
    let rel_str = rel.to_string_lossy().replace('\\', "/");
    if rel_str.is_empty() {
        return None;
    }

    let depth = rel_str.matches('/').count();
    let rel_path = if is_dir { format!("{rel_str}/") } else { rel_str };
    let modified = metadata.and_then(|item| item.modified().ok()).unwrap_or(SystemTime::UNIX_EPOCH);
    Some(FileCandidate { rel_path, depth, modified, is_dir })
}

fn rank_and_truncate_candidates(candidates: &mut Vec<FileCandidate>, query_lower: &str) {
    candidates.sort_by(|left, right| compare_candidates(left, right, query_lower));
    candidates.truncate(MAX_CANDIDATES);
}

fn compare_candidates(left: &FileCandidate, right: &FileCandidate, query_lower: &str) -> Ordering {
    match_tier(left, query_lower)
        .cmp(&match_tier(right, query_lower))
        .then_with(|| left.depth.cmp(&right.depth))
        .then_with(|| right.is_dir.cmp(&left.is_dir))
        .then_with(|| right.modified.cmp(&left.modified))
        .then_with(|| left.rel_path.cmp(&right.rel_path))
}

fn match_tier(candidate: &FileCandidate, query_lower: &str) -> Option<u8> {
    if query_lower.is_empty() {
        return Some(0);
    }

    let rel_path_lower = candidate.rel_path.to_lowercase();
    let basename_lower = candidate_basename(&candidate.rel_path).to_lowercase();

    if basename_lower.starts_with(query_lower) {
        Some(0)
    } else if rel_path_lower.starts_with(query_lower) {
        Some(1)
    } else if basename_lower.contains(query_lower) {
        Some(2)
    } else if rel_path_lower.contains(query_lower) {
        Some(3)
    } else {
        None
    }
}

fn candidate_basename(rel_path: &str) -> &str {
    let trimmed = rel_path.trim_end_matches('/');
    trimmed.rsplit('/').next().unwrap_or(trimmed)
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
    use std::time::Duration;

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

    fn run_search(app: &mut App, now: Instant) {
        for step in 0..64 {
            tick(app, now + Duration::from_millis(step));
            let is_settled = app.mention.as_ref().is_none_or(|mention| {
                !matches!(mention.search_status, MentionSearchStatus::Searching)
            });
            if is_settled {
                return;
            }
        }
    }

    fn run_single_search_tick(app: &mut App, now: Instant) {
        tick(app, now);
    }

    #[test]
    fn sync_with_cursor_activates_inside_existing_mention() {
        let (mut app, _tmp) = app_with_temp_files(&["src/main.rs", "tests/integration.rs"]);
        app.input.set_text("open @src/main.rs now");
        let _ = app.input.set_cursor(0, "open @src".chars().count());

        sync_with_cursor(&mut app);
        run_search(&mut app, Instant::now());

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
        run_search(&mut app, Instant::now());
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
        run_search(&mut app, Instant::now());
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
        run_search(&mut app, Instant::now());
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
        run_search(&mut app, Instant::now());

        let mention = app.mention.as_ref().expect("mention should be active");
        assert!(mention.candidates.iter().any(|candidate| candidate.rel_path == "visible.rs"));
        assert!(!mention.candidates.iter().any(|candidate| candidate.rel_path == "ignored.rs"));
    }

    #[test]
    fn activate_includes_gitignored_files_when_setting_is_disabled() {
        let (mut app, tmp) = app_with_temp_files(&["visible.rs", "ignored.rs"]);
        std::fs::create_dir_all(tmp.path().join(".git")).expect("create .git");
        std::fs::write(tmp.path().join(".gitignore"), "ignored.rs\n").expect("write .gitignore");
        crate::app::settings::store::set_respect_gitignore(
            &mut app.settings.committed_preferences_document,
            false,
        );
        app.input.set_text("@rs");
        let _ = app.input.set_cursor(0, 3);

        activate(&mut app);
        run_search(&mut app, Instant::now());

        let mention = app.mention.as_ref().expect("mention should be active");
        assert!(mention.candidates.iter().any(|candidate| candidate.rel_path == "visible.rs"));
        assert!(mention.candidates.iter().any(|candidate| candidate.rel_path == "ignored.rs"));
    }

    #[test]
    fn nested_gitignore_hides_same_directory_children() {
        let (mut app, _tmp) =
            app_with_temp_files(&["src/.gitignore", "src/visible.rs", "src/hidden.rs"]);
        let root = PathBuf::from(&app.cwd_raw);
        std::fs::write(root.join("src").join(".gitignore"), "hidden.rs\n")
            .expect("write .gitignore");
        app.input.set_text("@rs");
        let _ = app.input.set_cursor(0, 3);

        activate(&mut app);
        run_search(&mut app, Instant::now());

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
        run_search(&mut app, Instant::now());

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
        run_single_search_tick(&mut app, Instant::now());

        let mention = app.mention.as_ref().expect("mention should be active");
        assert!(mention.candidates.iter().any(|candidate| candidate.rel_path == "root.rs"));
        assert!(
            !mention.candidates.iter().any(|candidate| candidate.rel_path == "src/nested/deep.rs")
        );
        assert_eq!(mention.search_status, MentionSearchStatus::Searching);

        run_search(&mut app, Instant::now());

        let mention = app.mention.as_ref().expect("mention should remain active");
        assert!(
            mention.candidates.iter().any(|candidate| candidate.rel_path == "src/nested/deep.rs")
        );
        assert_eq!(mention.search_status, MentionSearchStatus::Ready);
    }

    #[test]
    fn query_change_drops_partial_results_and_starts_fresh_search() {
        let (mut app, _tmp) =
            app_with_temp_files(&["root.rs", "src/nested/needle.rs", "src/nested/other.rs"]);
        app.input.set_text("@rs");
        let _ = app.input.set_cursor(0, 3);

        activate(&mut app);
        run_single_search_tick(&mut app, Instant::now());
        assert!(app.mention.as_ref().is_some_and(|mention| {
            mention.candidates.iter().any(|candidate| candidate.rel_path == "root.rs")
        }));

        app.input.set_text("@needle");
        let _ = app.input.set_cursor(0, "@needle".chars().count());
        update_query(&mut app);

        let mention = app.mention.as_ref().expect("mention should remain active");
        assert!(mention.candidates.is_empty());
        assert_eq!(mention.placeholder_message().as_deref(), Some("Searching files..."));

        run_search(&mut app, Instant::now());

        let mention = app.mention.as_ref().expect("mention should remain active");
        assert_eq!(mention.candidates.len(), 1);
        assert_eq!(mention.candidates[0].rel_path, "src/nested/needle.rs");
    }

    #[test]
    fn basename_prefix_ranks_ahead_of_shallow_path_substring() {
        let mut candidates = vec![
            FileCandidate {
                rel_path: "docs/guide-rs.txt".to_owned(),
                depth: 1,
                modified: SystemTime::UNIX_EPOCH,
                is_dir: false,
            },
            FileCandidate {
                rel_path: "src/rs-helper.rs".to_owned(),
                depth: 1,
                modified: SystemTime::UNIX_EPOCH,
                is_dir: false,
            },
        ];

        rank_and_truncate_candidates(&mut candidates, "rs");

        assert_eq!(candidates[0].rel_path, "src/rs-helper.rs");
    }
}
