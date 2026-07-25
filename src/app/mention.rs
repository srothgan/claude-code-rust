// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang
use super::{AUTOCOMPLETE_VISIBLE_ROWS, App, FocusTarget, dialog::DialogState, file_index};
use std::time::{Duration, Instant};

/// Minimum query length before scanning the filesystem for matches.
pub const MIN_QUERY_CHARS: usize = 1;
const SCAN_BATCH_REFRESH_INTERVAL: Duration = Duration::from_millis(150);

pub struct MentionState {
    /// Character position (row, col) where the `@` was typed.
    pub trigger_row: usize,
    pub trigger_col: usize,
    /// Current query text after the `@` (e.g. "src/m" from "@src/m").
    pub query: String,
    /// Character position where confirmation replacement should stop.
    pub replace_end_col: usize,
    /// Filtered + sorted candidates.
    pub candidates: Vec<file_index::FileCandidate>,
    /// Shared autocomplete dialog navigation state.
    pub dialog: DialogState,
    search_status: MentionSearchStatus,
    next_match_sequence: u64,
    pending_match_sequence: Option<u64>,
    refresh_after_pending_match: bool,
    last_scan_batch_request_at: Option<Instant>,
    last_scan_batch_request_version: u64,
    line_char_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommittedMentionSpan {
    pub row: usize,
    pub start_col: usize,
    pub end_col: usize,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MentionSearchStatus {
    Hint,
    Searching,
    Ready,
    NoMatches,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MentionSpanSource {
    Active,
    QuotedPathClosed,
    QuotedPathOpen,
    IndexedPath,
    BareToken,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct MentionSpan {
    row: usize,
    trigger_col: usize,
    end_col: usize,
    query: String,
    source: MentionSpanSource,
}

#[derive(Clone, Copy)]
struct ActiveMentionBounds {
    row: usize,
    trigger_col: usize,
    replace_end_col: usize,
    line_char_count: usize,
}

impl MentionState {
    #[must_use]
    pub fn new(
        trigger_row: usize,
        trigger_col: usize,
        query: String,
        candidates: Vec<file_index::FileCandidate>,
    ) -> Self {
        let replace_end_col = trigger_col + 1 + query.chars().count();
        let search_status = if candidates.is_empty() {
            MentionSearchStatus::Hint
        } else {
            MentionSearchStatus::Ready
        };
        Self {
            trigger_row,
            trigger_col,
            query,
            replace_end_col,
            candidates,
            dialog: DialogState::default(),
            search_status,
            next_match_sequence: 0,
            pending_match_sequence: None,
            refresh_after_pending_match: false,
            last_scan_batch_request_at: None,
            last_scan_batch_request_version: 0,
            line_char_count: replace_end_col,
        }
    }

    #[must_use]
    pub fn placeholder_message(&self) -> Option<String> {
        if !self.candidates.is_empty() {
            return None;
        }

        match self.search_status {
            MentionSearchStatus::Hint => Some("Type a file or folder name after @".to_owned()),
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
        self.pending_match_sequence = None;
        self.refresh_after_pending_match = false;
        self.dialog.clamp(0, AUTOCOMPLETE_VISIBLE_ROWS);
    }
}

impl From<&MentionState> for ActiveMentionBounds {
    fn from(mention: &MentionState) -> Self {
        Self {
            row: mention.trigger_row,
            trigger_col: mention.trigger_col,
            replace_end_col: mention.replace_end_col,
            line_char_count: mention.line_char_count,
        }
    }
}

fn detect_mention_span_at_cursor(
    lines: &[String],
    cursor_row: usize,
    cursor_col: usize,
    file_index: Option<&file_index::FileIndexState>,
    active: Option<ActiveMentionBounds>,
) -> Option<MentionSpan> {
    let line = lines.get(cursor_row)?;
    let chars = line.chars().collect::<Vec<_>>();
    let cursor_col = cursor_col.min(chars.len());

    valid_trigger_cols_before_cursor(&chars, cursor_col)
        .into_iter()
        .rev()
        .filter_map(|trigger_col| {
            resolve_mention_span(cursor_row, &chars, trigger_col, cursor_col, file_index, active)
        })
        .find(|span| span_contains_activation_cursor(span, cursor_col))
}

fn span_contains_activation_cursor(span: &MentionSpan, cursor_col: usize) -> bool {
    cursor_col > span.trigger_col
        && match span.source {
            MentionSpanSource::QuotedPathClosed => cursor_col < span.end_col,
            MentionSpanSource::Active
            | MentionSpanSource::QuotedPathOpen
            | MentionSpanSource::IndexedPath
            | MentionSpanSource::BareToken => cursor_col <= span.end_col,
        }
}

fn valid_trigger_cols_before_cursor(chars: &[char], cursor_col: usize) -> Vec<usize> {
    chars
        .iter()
        .take(cursor_col)
        .enumerate()
        .filter_map(|(col, ch)| {
            (*ch == '@' && (col == 0 || chars[col - 1].is_whitespace())).then_some(col)
        })
        .collect()
}

/// Activate mention autocomplete after the user types `@`.
pub fn activate(app: &mut App) {
    let detection = detect_mention_span_at_cursor(
        app.input.lines(),
        app.input.cursor_row(),
        app.input.cursor_col(),
        Some(&app.file_index),
        app.mention.as_ref().map(ActiveMentionBounds::from),
    );

    let Some(span) = detection else {
        return;
    };

    let line_char_count = app.input.lines().get(span.row).map_or(0, |line| line.chars().count());
    let mut mention = MentionState::new(span.row, span.trigger_col, span.query, Vec::new());
    mention.replace_end_col = span.end_col;
    mention.line_char_count = line_char_count;
    app.mention = Some(mention);
    app.slash = None;
    app.subagent = None;
    refresh_query_state(app);
}

/// Update the query and re-filter candidates while mention is active.
pub fn update_query(app: &mut App) {
    let active = app.mention.as_ref().map(ActiveMentionBounds::from);
    let detection = detect_mention_span_at_cursor(
        app.input.lines(),
        app.input.cursor_row(),
        app.input.cursor_col(),
        Some(&app.file_index),
        active,
    );

    let Some(span) = detection else {
        deactivate(app);
        return;
    };

    let line_char_count = app.input.lines().get(span.row).map_or(0, |line| line.chars().count());
    let previous_line_char_count =
        app.mention.as_ref().map_or(line_char_count, |mention| mention.line_char_count);
    let previous_replace_end_col =
        app.mention.as_ref().map_or(span.end_col, |mention| mention.replace_end_col);
    let replace_end_col = if matches!(span.source, MentionSpanSource::Active) {
        adjust_active_replace_end(
            previous_replace_end_col,
            previous_line_char_count,
            line_char_count,
        )
        .max(span.end_col)
    } else {
        span.end_col
    };

    if let Some(ref mut mention) = app.mention {
        mention.trigger_row = span.row;
        mention.trigger_col = span.trigger_col;
        mention.query = span.query;
        mention.replace_end_col = replace_end_col;
        mention.line_char_count = line_char_count;
    }

    refresh_query_state(app);
}

fn adjust_active_replace_end(
    previous_replace_end_col: usize,
    previous_line_char_count: usize,
    line_char_count: usize,
) -> usize {
    if line_char_count >= previous_line_char_count {
        previous_replace_end_col + (line_char_count - previous_line_char_count)
    } else {
        previous_replace_end_col.saturating_sub(previous_line_char_count - line_char_count)
    }
}

pub fn refresh_from_file_index(app: &mut App) {
    request_match_for_active_mention(app, MatchRequestMode::IndexRefresh);
}

pub fn refresh_from_file_index_after_scan_batch(app: &mut App) {
    let Some(mention) = app.mention.as_mut() else {
        return;
    };

    if query_is_hint(&mention.query) {
        mention.mark_hint();
        sync_focus(app);
        return;
    }

    let index_version = app.file_index.index_version;
    if mention.last_scan_batch_request_version >= index_version {
        return;
    }
    if let Some(last_request_at) = mention.last_scan_batch_request_at
        && last_request_at.elapsed() < SCAN_BATCH_REFRESH_INTERVAL
    {
        return;
    }

    request_match_for_active_mention(app, MatchRequestMode::ScanBatchRefresh);
}

pub fn apply_match_result(app: &mut App, result: file_index::MentionMatchResult) -> bool {
    let Some(mention) = app.mention.as_mut() else {
        tracing::debug!(
            target: crate::logging::targets::APP_FILE_INDEX,
            event_name = "mention_match_result_rejected",
            message = "mention match result rejected because mention is inactive",
            generation = result.generation,
            index_version = result.index_version,
            sequence = result.sequence,
            query_chars = result.query.chars().count(),
            result_count = result.candidates.len(),
        );
        return false;
    };

    if result.generation != app.file_index.generation
        || result.query != mention.query
        || mention.pending_match_sequence != Some(result.sequence)
    {
        tracing::debug!(
            target: crate::logging::targets::APP_FILE_INDEX,
            event_name = "mention_match_result_rejected",
            message = "mention match result rejected because it is stale",
            result_generation = result.generation,
            current_generation = app.file_index.generation,
            result_index_version = result.index_version,
            current_index_version = app.file_index.index_version,
            sequence = result.sequence,
            pending_sequence = mention.pending_match_sequence,
            result_query_chars = result.query.chars().count(),
            current_query_chars = mention.query.chars().count(),
            query_matches = result.query == mention.query,
            result_count = result.candidates.len(),
        );
        return false;
    }

    let result_count = result.candidates.len();
    mention.candidates = result.candidates;
    mention.pending_match_sequence = None;
    let needs_follow_up = mention.refresh_after_pending_match
        || result.index_version < app.file_index.index_version
        || result.scan_finished != app.file_index.scan_finished;
    mention.refresh_after_pending_match = false;
    mention.search_status = if mention.candidates.is_empty() {
        if result.scan_finished {
            MentionSearchStatus::NoMatches
        } else {
            MentionSearchStatus::Searching
        }
    } else if result.scan_finished {
        MentionSearchStatus::Ready
    } else {
        MentionSearchStatus::Searching
    };
    mention.dialog.clamp(mention.candidates.len(), AUTOCOMPLETE_VISIBLE_ROWS);
    sync_focus(app);
    if needs_follow_up {
        request_match_for_active_mention(app, MatchRequestMode::IndexRefresh);
    }
    tracing::debug!(
        target: crate::logging::targets::APP_FILE_INDEX,
        event_name = "mention_match_result_applied",
        message = "mention applied file index match result",
        generation = result.generation,
        result_index_version = result.index_version,
        current_index_version = app.file_index.index_version,
        sequence = result.sequence,
        query_chars = result.query.chars().count(),
        result_count,
        result_scan_finished = result.scan_finished,
        current_scan_finished = app.file_index.scan_finished,
        follow_up_requested = needs_follow_up,
    );
    true
}

fn refresh_query_state(app: &mut App) {
    let Some(mention) = app.mention.as_mut() else {
        return;
    };

    if query_is_hint(&mention.query) {
        mention.mark_hint();
        sync_focus(app);
        return;
    }

    file_index::ensure_started(app);
    request_match_for_active_mention(app, MatchRequestMode::UserQuery);
}

#[derive(Clone, Copy)]
enum MatchRequestMode {
    UserQuery,
    IndexRefresh,
    ScanBatchRefresh,
}

impl MatchRequestMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::UserQuery => "user_query",
            Self::IndexRefresh => "index_refresh",
            Self::ScanBatchRefresh => "scan_batch_refresh",
        }
    }
}

fn request_match_for_active_mention(app: &mut App, mode: MatchRequestMode) {
    let Some(query) = app.mention.as_ref().map(|mention| mention.query.clone()) else {
        return;
    };

    if query_is_hint(&query) {
        if let Some(mention) = app.mention.as_mut() {
            mention.mark_hint();
        }
        sync_focus(app);
        return;
    }

    let generation = app.file_index.generation;
    let index_version = app.file_index.index_version;
    let Some(mention) = app.mention.as_mut() else {
        return;
    };
    if mention.pending_match_sequence.is_some() && !matches!(mode, MatchRequestMode::UserQuery) {
        mention.refresh_after_pending_match = true;
        tracing::debug!(
            target: crate::logging::targets::APP_FILE_INDEX,
            event_name = "mention_match_request_deferred",
            message = "mention deferred file index match request while one is pending",
            generation,
            index_version,
            mode = mode.as_str(),
            pending_sequence = mention.pending_match_sequence,
            query_chars = query.chars().count(),
        );
        return;
    }
    mention.next_match_sequence = mention.next_match_sequence.saturating_add(1);
    let sequence = mention.next_match_sequence;
    mention.pending_match_sequence = Some(sequence);
    mention.refresh_after_pending_match = false;
    mention.last_scan_batch_request_at = Some(Instant::now());
    mention.last_scan_batch_request_version = index_version;
    if matches!(mode, MatchRequestMode::UserQuery) && mention.candidates.is_empty() {
        mention.dialog.clamp(0, AUTOCOMPLETE_VISIBLE_ROWS);
    }
    mention.search_status = MentionSearchStatus::Searching;
    sync_focus(app);

    tracing::debug!(
        target: crate::logging::targets::APP_FILE_INDEX,
        event_name = "mention_match_request_sent",
        message = "mention requested file index match",
        generation,
        index_version,
        sequence,
        mode = mode.as_str(),
        query_chars = query.chars().count(),
        query_bytes = query.len(),
    );

    file_index::request_match(
        app,
        file_index::MentionMatchRequest { generation, index_version, sequence, query },
    );
}

fn query_is_hint(query: &str) -> bool {
    query.chars().count() < MIN_QUERY_CHARS || query.trim().is_empty()
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
    let in_mention = detect_mention_span_at_cursor(
        app.input.lines(),
        app.input.cursor_row(),
        app.input.cursor_col(),
        Some(&app.file_index),
        app.mention.as_ref().map(ActiveMentionBounds::from),
    )
    .is_some();
    match (in_mention, app.mention.is_some()) {
        (true, true) => update_query(app),
        (true, false) => activate(app),
        (false, true) => deactivate(app),
        (false, false) => {}
    }
}

/// Confirm the selected candidate: replace `@query` in input with a quoted `@'rel_path'`.
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

    let mention_end = resolve_confirm_end_col(&mention, &chars, &app.file_index);
    if mention_end <= trigger_col || mention_end > chars.len() {
        return;
    }

    let before: String = chars[..trigger_col].iter().collect();
    let after: String = chars[mention_end..].iter().collect();
    let quoted_path = quote_mention_path(&rel_path);
    let replacement = if after.is_empty() { format!("{quoted_path} ") } else { quoted_path };

    let new_line = format!("{before}{replacement}{after}");
    let new_cursor_col = trigger_col + replacement.chars().count();

    lines[trigger_row] = new_line;
    app.input.replace_lines_and_cursor(lines, trigger_row, new_cursor_col);
}

pub fn commit_literal_if_active(app: &mut App) -> bool {
    let Some(mention) = app.mention.take() else {
        return false;
    };
    if mention.has_selectable_candidates() {
        app.mention = Some(mention);
        return false;
    }

    if app.slash.is_none() && app.subagent.is_none() {
        app.release_focus_target(FocusTarget::Mention);
    }

    if query_is_hint(&mention.query) {
        app.input.highlight_version = u64::MAX;
        return true;
    }

    let mut lines = app.input.lines().to_vec();
    let Some(line) = lines.get(mention.trigger_row) else {
        app.input.highlight_version = u64::MAX;
        return true;
    };
    let chars: Vec<char> = line.chars().collect();
    if chars.get(mention.trigger_col) != Some(&'@') {
        app.input.highlight_version = u64::MAX;
        return true;
    }

    let start_col = mention.trigger_col;
    let raw_end_col = mention.replace_end_col.min(chars.len());
    let end_col = trim_trailing_whitespace(chars.as_slice(), start_col + 1, raw_end_col);
    if end_col <= start_col + 1 {
        app.input.highlight_version = u64::MAX;
        return true;
    }

    let literal_path = literal_path_from_mention_text(&chars[start_col..end_col]);
    let committed_text = quote_mention_path(&literal_path);
    let before: String = chars[..start_col].iter().collect();
    let after: String = chars[end_col..].iter().collect();
    let replacement =
        if after.is_empty() { format!("{committed_text} ") } else { committed_text.clone() };
    let new_cursor_col = start_col + replacement.chars().count();
    lines[mention.trigger_row] = format!("{before}{replacement}{after}");
    app.input.replace_lines_and_cursor(lines, mention.trigger_row, new_cursor_col);

    app.committed_mentions.push(CommittedMentionSpan {
        row: mention.trigger_row,
        start_col,
        end_col: start_col + committed_text.chars().count(),
        text: committed_text,
    });
    true
}

fn quote_mention_path(path: &str) -> String {
    format!("@'{}'", escape_quoted_path(path))
}

fn escape_quoted_path(path: &str) -> String {
    let mut escaped = String::with_capacity(path.len());
    for ch in path.chars() {
        if matches!(ch, '\\' | '\'') {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

fn literal_path_from_mention_text(chars: &[char]) -> String {
    if chars.first() != Some(&'@') {
        return chars.iter().collect();
    }
    if chars.get(1) == Some(&'\'') {
        let parsed = parse_quoted_path(chars, 2);
        return parse_quoted_path_content(chars, 2, parsed.content_end);
    }
    chars[1..].iter().collect()
}

fn trim_trailing_whitespace(chars: &[char], min_col: usize, mut end_col: usize) -> usize {
    while end_col > min_col && chars[end_col - 1].is_whitespace() {
        end_col -= 1;
    }
    end_col
}

fn resolve_confirm_end_col(
    mention: &MentionState,
    chars: &[char],
    file_index: &file_index::FileIndexState,
) -> usize {
    if mention.replace_end_col <= chars.len()
        && mention.replace_end_col > mention.trigger_col
        && chars.get(mention.trigger_col) == Some(&'@')
    {
        return mention.replace_end_col;
    }

    resolve_mention_span(
        mention.trigger_row,
        chars,
        mention.trigger_col,
        mention.trigger_col + 1,
        Some(file_index),
        None,
    )
    .map_or(mention.trigger_col, |span| span.end_col)
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
        mention.dialog.move_up(mention.candidates.len(), AUTOCOMPLETE_VISIBLE_ROWS);
    }
}

/// Move selection down in the candidate list.
pub fn move_down(app: &mut App) {
    if let Some(ref mut mention) = app.mention {
        mention.dialog.move_down(mention.candidates.len(), AUTOCOMPLETE_VISIBLE_ROWS);
    }
}

/// Find all `@path` references in a text string. Returns `(start_col, end_col, path)` tuples.
pub fn find_mention_spans(
    row: usize,
    text: &str,
    file_index: &file_index::FileIndexState,
    active: Option<&MentionState>,
) -> Vec<(usize, usize, String)> {
    let mut spans = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    let active = active.map(ActiveMentionBounds::from);

    while i < chars.len() {
        if chars[i] == '@' && (i == 0 || chars[i - 1].is_whitespace()) {
            let cursor_col = active
                .filter(|active| active.row == row && active.trigger_col == i)
                .map_or(i + 1, |active| active.replace_end_col.min(chars.len()));
            if let Some(span) =
                resolve_mention_span(row, &chars, i, cursor_col, Some(file_index), active)
            {
                i = span.end_col.max(i + 1);
                if !span.query.is_empty() {
                    spans.push((span.trigger_col, span.end_col, span.query));
                }
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }

    spans
}

pub fn retain_valid_committed_mentions(
    lines: &[String],
    committed_mentions: &mut Vec<CommittedMentionSpan>,
) {
    committed_mentions.retain(|span| committed_mention_matches(lines, span));
}

pub fn committed_mention_matches(lines: &[String], span: &CommittedMentionSpan) -> bool {
    let Some(line) = lines.get(span.row) else {
        return false;
    };
    let chars: Vec<char> = line.chars().collect();
    if span.start_col >= span.end_col || span.end_col > chars.len() {
        return false;
    }
    let text: String = chars[span.start_col..span.end_col].iter().collect();
    text == span.text
}

fn resolve_mention_span(
    row: usize,
    chars: &[char],
    trigger_col: usize,
    cursor_col: usize,
    file_index: Option<&file_index::FileIndexState>,
    active: Option<ActiveMentionBounds>,
) -> Option<MentionSpan> {
    if chars.get(trigger_col) != Some(&'@')
        || (trigger_col > 0 && !chars[trigger_col - 1].is_whitespace())
    {
        return None;
    }

    if chars.get(trigger_col + 1) == Some(&'\'') {
        return Some(resolve_quoted_mention_span(row, chars, trigger_col, cursor_col, active));
    }

    if let Some(active) = active
        && active.row == row
        && active.trigger_col == trigger_col
        && cursor_col > trigger_col
        && (cursor_col <= active.replace_end_col || chars.len() > active.line_char_count)
    {
        let end_col = active.replace_end_col.max(cursor_col).min(chars.len());
        if end_col > trigger_col {
            let query = chars[trigger_col + 1..cursor_col.min(end_col)].iter().collect();
            return Some(MentionSpan {
                row,
                trigger_col,
                end_col,
                query,
                source: MentionSpanSource::Active,
            });
        }
    }

    if let Some(file_index) = file_index
        && let Some((end_col, query)) =
            longest_indexed_path_span(chars, trigger_col, &file_index.entries)
    {
        let query = span_query(chars, trigger_col, cursor_col, end_col, &query);
        return Some(MentionSpan {
            row,
            trigger_col,
            end_col,
            query,
            source: MentionSpanSource::IndexedPath,
        });
    }

    let end_col = bare_token_end_col(chars, trigger_col);
    if end_col <= trigger_col + 1 {
        if cursor_col == trigger_col + 1 {
            return Some(MentionSpan {
                row,
                trigger_col,
                end_col: cursor_col,
                query: String::new(),
                source: MentionSpanSource::BareToken,
            });
        }
        return None;
    }
    let bare_query: String = chars[trigger_col + 1..end_col].iter().collect();
    let query = span_query(chars, trigger_col, cursor_col, end_col, &bare_query);
    Some(MentionSpan { row, trigger_col, end_col, query, source: MentionSpanSource::BareToken })
}

fn resolve_quoted_mention_span(
    row: usize,
    chars: &[char],
    trigger_col: usize,
    cursor_col: usize,
    active: Option<ActiveMentionBounds>,
) -> MentionSpan {
    let content_start = trigger_col + 2;
    let parsed = parse_quoted_path(chars, content_start);
    let mut end_col = parsed.span_end;
    if parsed.close.is_none()
        && let Some(active) = active
        && active.row == row
        && active.trigger_col == trigger_col
        && cursor_col > trigger_col
        && (cursor_col <= active.replace_end_col || chars.len() > active.line_char_count)
    {
        end_col = active.replace_end_col.max(cursor_col).min(chars.len());
    }

    let query = quoted_span_query(chars, content_start, cursor_col, parsed.content_end);
    MentionSpan {
        row,
        trigger_col,
        end_col,
        query,
        source: if parsed.close.is_some() {
            MentionSpanSource::QuotedPathClosed
        } else {
            MentionSpanSource::QuotedPathOpen
        },
    }
}

struct ParsedQuotedPath {
    content_end: usize,
    close: Option<usize>,
    span_end: usize,
}

fn parse_quoted_path(chars: &[char], content_start: usize) -> ParsedQuotedPath {
    let mut escaped = false;
    for (col, ch) in chars.iter().enumerate().skip(content_start) {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '\'' => {
                return ParsedQuotedPath { content_end: col, close: Some(col), span_end: col + 1 };
            }
            _ => {}
        }
    }

    ParsedQuotedPath { content_end: chars.len(), close: None, span_end: chars.len() }
}

fn quoted_span_query(
    chars: &[char],
    content_start: usize,
    cursor_col: usize,
    content_end: usize,
) -> String {
    let query_end = if cursor_col > content_start && cursor_col <= content_end {
        cursor_col
    } else {
        content_end
    };
    parse_quoted_path_content(chars, content_start, query_end)
}

fn parse_quoted_path_content(chars: &[char], start_col: usize, end_col: usize) -> String {
    let mut content = String::new();
    let mut escaped = false;
    for ch in chars[start_col.min(chars.len())..end_col.min(chars.len())].iter().copied() {
        if escaped {
            content.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else {
            content.push(ch);
        }
    }
    if escaped {
        content.push('\\');
    }
    content
}

fn span_query(
    chars: &[char],
    trigger_col: usize,
    cursor_col: usize,
    end_col: usize,
    fallback: &str,
) -> String {
    if cursor_col > trigger_col + 1 {
        chars[trigger_col + 1..cursor_col.min(end_col)].iter().collect()
    } else {
        fallback.to_owned()
    }
}

fn longest_indexed_path_span(
    chars: &[char],
    trigger_col: usize,
    entries: &std::collections::BTreeMap<String, file_index::FileCandidate>,
) -> Option<(usize, String)> {
    let mut longest = None;
    let mut candidate = String::new();
    for (path_col, ch) in chars.iter().enumerate().skip(trigger_col + 1) {
        candidate.push(*ch);
        let end_col = path_col + 1;
        if entries.contains_key(&candidate) {
            longest = Some((end_col, candidate.clone()));
        }
    }
    longest
}

fn bare_token_end_col(chars: &[char], trigger_col: usize) -> usize {
    let mut end_col = trigger_col + 1;
    while end_col < chars.len() && !is_bare_mention_delimiter(chars[end_col]) {
        end_col += 1;
    }
    end_col
}

fn is_bare_mention_delimiter(ch: char) -> bool {
    ch.is_whitespace() || matches!(ch, '\'' | ',' | ';' | ')' | ']' | '}')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use std::path::PathBuf;
    use std::time::Duration;

    fn app_with_temp_files(files: &[&str]) -> (App, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        for file in files {
            let path = tmp.path().join(file);
            if file.ends_with('/') {
                std::fs::create_dir_all(&path).expect("create dir");
                continue;
            }
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("create parent");
            }
            std::fs::write(&path, "").expect("write file");
        }
        let mut app = App::test_default();
        app.cwd_raw = tmp.path().to_string_lossy().into_owned();
        (app, tmp)
    }

    fn candidate(rel_path: &str) -> file_index::FileCandidate {
        file_index::FileCandidate {
            rel_path: rel_path.to_owned(),
            rel_path_lower: rel_path.to_lowercase(),
            basename_lower: rel_path
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap()
                .to_lowercase(),
            depth: rel_path.matches('/').count(),
        }
    }

    fn index_paths(app: &mut App, paths: &[&str]) {
        app.file_index.root = Some(PathBuf::from(&app.cwd_raw));
        app.file_index.respect_gitignore = app.config.respect_gitignore_effective();
        app.file_index.scan_finished = true;
        for path in paths {
            app.file_index.entries.insert((*path).to_owned(), candidate(path));
        }
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
    fn detects_and_highlights_indexed_file_path_with_spaces() {
        let mut app = App::test_default();
        index_paths(&mut app, &["docs/my file.md"]);

        let spans = find_mention_spans(0, "open @docs/my file.md now", &app.file_index, None);

        assert_eq!(spans, vec![(5, 21, "docs/my file.md".to_owned())]);
    }

    #[test]
    fn detects_and_highlights_indexed_folder_path_with_spaces() {
        let mut app = App::test_default();
        index_paths(&mut app, &["docs/my folder/"]);

        let spans = find_mention_spans(0, "open @docs/my folder/ now", &app.file_index, None);

        assert_eq!(spans, vec![(5, 21, "docs/my folder/".to_owned())]);
    }

    #[test]
    fn longest_indexed_path_wins_when_paths_share_prefixes() {
        let mut app = App::test_default();
        index_paths(&mut app, &["foo", "foo bar.md"]);

        let spans = find_mention_spans(0, "@foo bar.md", &app.file_index, None);

        assert_eq!(spans, vec![(0, 11, "foo bar.md".to_owned())]);
    }

    #[test]
    fn indexed_prefix_does_not_over_highlight_following_prose() {
        let mut app = App::test_default();
        index_paths(&mut app, &["docs/my"]);

        let spans = find_mention_spans(0, "@docs/my folder now", &app.file_index, None);

        assert_eq!(spans, vec![(0, 8, "docs/my".to_owned())]);
    }

    #[test]
    fn detects_and_highlights_quoted_indexed_path() {
        let mut app = App::test_default();
        index_paths(&mut app, &["docs/my file.md"]);

        let spans = find_mention_spans(0, "open @'docs/my file.md' now", &app.file_index, None);

        assert_eq!(spans, vec![(5, 23, "docs/my file.md".to_owned())]);
    }

    #[test]
    fn comma_after_quoted_path_does_not_reenter_autocomplete() {
        let mut app = App::test_default();
        index_paths(&mut app, &["src/app/mention.rs"]);
        app.input.set_text("@'src/app/mention.rs'");
        let _ = app.input.set_cursor(0, app.input.lines()[0].chars().count());

        sync_with_cursor(&mut app);
        assert!(app.mention.is_none());

        let _ = app.input.textarea_insert_char(',');
        sync_with_cursor(&mut app);

        assert_eq!(app.input.lines()[0], "@'src/app/mention.rs',");
        assert!(app.mention.is_none());
    }

    #[test]
    fn cursor_inside_quoted_path_reenters_autocomplete() {
        let mut app = App::test_default();
        index_paths(&mut app, &["src/app/mention.rs"]);
        app.input.set_text("@'src/app/mention.rs'");
        let _ = app.input.set_cursor(0, "@'src/app/mention".chars().count());

        sync_with_cursor(&mut app);
        run_search(&mut app);

        let mention = app.mention.as_ref().expect("mention should re-enter inside quotes");
        assert_eq!(mention.query, "src/app/mention");
        assert_eq!(mention.replace_end_col, "@'src/app/mention.rs'".chars().count());
        assert!(!mention.candidates.is_empty());
    }

    #[test]
    fn missing_right_quote_stays_active_and_confirm_repairs_delimiter() {
        let mut app = App::test_default();
        index_paths(&mut app, &["src/app/mention.rs"]);
        app.input.set_text("@'src/app/ment");
        let _ = app.input.set_cursor(0, app.input.lines()[0].chars().count());

        sync_with_cursor(&mut app);
        run_search(&mut app);
        confirm_selection(&mut app);

        assert_eq!(app.input.lines()[0], "@'src/app/mention.rs' ");
        assert!(app.mention.is_none());
    }

    #[test]
    fn deleting_left_quote_falls_back_to_bare_path_highlighting() {
        let mut app = App::test_default();
        index_paths(&mut app, &["src/app/mention.rs"]);

        let spans = find_mention_spans(0, "@src/app/mention.rs'", &app.file_index, None);

        assert_eq!(spans, vec![(0, 19, "src/app/mention.rs".to_owned())]);
    }

    #[test]
    fn deleting_right_quote_keeps_literal_highlighting_to_line_end() {
        let mut app = App::test_default();
        index_paths(&mut app, &["src/app/mention.rs"]);

        let spans = find_mention_spans(0, "@'src/app/mention.rs", &app.file_index, None);

        assert_eq!(spans, vec![(0, 20, "src/app/mention.rs".to_owned())]);
    }

    #[test]
    fn reentering_autocomplete_inside_spaced_path_uses_cursor_query_and_full_span() {
        let mut app = App::test_default();
        index_paths(&mut app, &["docs/my folder/read me.md"]);
        app.input.set_text("open @docs/my folder/read me.md now");
        let _ = app.input.set_cursor(0, "open @docs/my folder/read".chars().count());

        sync_with_cursor(&mut app);

        let mention = app.mention.as_ref().expect("mention should be active");
        assert_eq!(mention.query, "docs/my folder/read");
        assert_eq!(mention.replace_end_col, "open @docs/my folder/read me.md".chars().count());
    }

    #[test]
    fn editing_right_side_of_spaced_mention_keeps_full_replacement_span() {
        let mut app = App::test_default();
        index_paths(&mut app, &["docs/my folder/read me.txt", "docs/my folder/read me.md"]);
        app.input.set_text("open @docs/my folder/read me.txt now");
        let _ = app.input.set_cursor(0, "open @docs/my folder/read me.".chars().count());
        sync_with_cursor(&mut app);

        let _ = app.input.textarea_insert_char('m');
        update_query(&mut app);
        {
            let mention = app.mention.as_mut().expect("mention should stay active");
            mention.candidates = vec![candidate("docs/my folder/read me.md")];
            mention.search_status = MentionSearchStatus::Ready;
        }
        confirm_selection(&mut app);

        assert_eq!(app.input.lines()[0], "open @'docs/my folder/read me.md' now");
    }

    #[test]
    fn confirm_selection_replaces_full_existing_token_without_double_space() {
        let (mut app, _tmp) = app_with_temp_files(&["src/lib.rs"]);
        app.input.set_text("open @src/lib.txt now");
        let _ = app.input.set_cursor(0, "open @src/lib".chars().count());

        activate(&mut app);
        run_search(&mut app);
        confirm_selection(&mut app);

        assert_eq!(app.input.lines()[0], "open @'src/lib.rs' now");
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

        assert_eq!(app.input.lines()[0], "@'src/main.rs' ");
    }

    #[test]
    fn confirming_spaced_candidate_at_end_inserts_trailing_space() {
        let (mut app, _tmp) = app_with_temp_files(&["path with spaces.md"]);
        app.input.set_text("@path");
        let _ = app.input.set_cursor(0, "@path".chars().count());

        activate(&mut app);
        run_search(&mut app);
        confirm_selection(&mut app);

        assert_eq!(app.input.lines()[0], "@'path with spaces.md' ");
    }

    #[test]
    fn confirming_spaced_candidate_before_text_does_not_add_double_space() {
        let mut app = App::test_default();
        app.input.set_text("open @path with typo later");
        let trigger_col = "open ".chars().count();
        let mut mention = MentionState::new(
            0,
            trigger_col,
            "path with typo".to_owned(),
            vec![candidate("path with spaces.md")],
        );
        mention.replace_end_col = "open @path with typo".chars().count();
        mention.search_status = MentionSearchStatus::Ready;
        app.mention = Some(mention);

        confirm_selection(&mut app);

        assert_eq!(app.input.lines()[0], "open @'path with spaces.md' later");
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
        assert_eq!(
            mention.placeholder_message().as_deref(),
            Some("Type a file or folder name after @")
        );
    }

    #[test]
    fn whitespace_only_query_stays_hint_without_searching_everything() {
        let (mut app, _tmp) = app_with_temp_files(&["src/main.rs"]);
        app.input.set_text("@");
        let _ = app.input.set_cursor(0, 1);
        activate(&mut app);
        app.input.set_text("@ ");
        let _ = app.input.set_cursor(0, 2);
        update_query(&mut app);

        let mention = app.mention.as_ref().expect("mention should be active");
        assert_eq!(mention.query, " ");
        assert!(mention.candidates.is_empty());
        assert!(matches!(mention.search_status, MentionSearchStatus::Hint));
    }

    #[test]
    fn commit_literal_adds_trailing_space_at_line_end() {
        let mut app = App::test_default();
        app.input.set_text("@");
        let _ = app.input.set_cursor(0, 1);
        activate(&mut app);
        app.input.set_text("@docs/manual path");
        let _ = app.input.set_cursor(0, "@docs/manual path".chars().count());
        update_query(&mut app);

        assert!(commit_literal_if_active(&mut app));

        assert!(app.mention.is_none());
        assert_eq!(app.input.lines()[0], "@'docs/manual path' ");
        assert_eq!(app.input.cursor_col(), "@'docs/manual path' ".chars().count());
        assert_eq!(
            app.committed_mentions,
            vec![CommittedMentionSpan {
                row: 0,
                start_col: 0,
                end_col: "@'docs/manual path'".chars().count(),
                text: "@'docs/manual path'".to_owned(),
            }]
        );
    }

    #[test]
    fn commit_literal_before_existing_space_does_not_add_double_space() {
        let mut app = App::test_default();
        app.input.set_text("@manual path now");
        let end_col = "@manual path".chars().count();
        let _ = app.input.set_cursor(0, end_col);
        let mut mention = MentionState::new(0, 0, "manual path".to_owned(), Vec::new());
        mention.replace_end_col = end_col;
        app.mention = Some(mention);

        assert!(commit_literal_if_active(&mut app));

        assert_eq!(app.input.lines()[0], "@'manual path' now");
        assert_eq!(app.committed_mentions.len(), 1);
        assert_eq!(app.committed_mentions[0].text, "@'manual path'");
        assert_eq!(app.committed_mentions[0].end_col, "@'manual path'".chars().count());
    }

    #[test]
    fn commit_literal_with_trailing_separator_space_does_not_add_another_space() {
        let mut app = App::test_default();
        app.input.set_text("@manual path ");
        let line_end_col = "@manual path ".chars().count();
        let _ = app.input.set_cursor(0, line_end_col);
        let mut mention = MentionState::new(0, 0, "manual path ".to_owned(), Vec::new());
        mention.replace_end_col = line_end_col;
        app.mention = Some(mention);

        assert!(commit_literal_if_active(&mut app));

        assert_eq!(app.input.lines()[0], "@'manual path' ");
        assert_eq!(app.committed_mentions.len(), 1);
        assert_eq!(app.committed_mentions[0].text, "@'manual path'");
        assert_eq!(app.committed_mentions[0].end_col, "@'manual path'".chars().count());
    }

    #[test]
    fn commit_literal_empty_query_only_deactivates() {
        let mut app = App::test_default();
        app.input.set_text("@ ");
        let _ = app.input.set_cursor(0, 2);
        app.mention = Some(MentionState::new(0, 0, " ".to_owned(), Vec::new()));

        assert!(commit_literal_if_active(&mut app));

        assert!(app.mention.is_none());
        assert!(app.committed_mentions.is_empty());
        assert_eq!(app.input.lines()[0], "@ ");
    }

    #[test]
    fn committed_mentions_are_exact_text_validated() {
        let mut spans = vec![CommittedMentionSpan {
            row: 0,
            start_col: 0,
            end_col: "@'docs/manual path'".chars().count(),
            text: "@'docs/manual path'".to_owned(),
        }];
        let lines = vec!["@'docs/manual path' ".to_owned()];
        retain_valid_committed_mentions(&lines, &mut spans);
        assert_eq!(spans.len(), 1);

        let lines = vec!["@'docs/changed path' ".to_owned()];
        retain_valid_committed_mentions(&lines, &mut spans);
        assert!(spans.is_empty());
    }

    #[test]
    fn user_query_refresh_keeps_existing_candidates_visible() {
        let mut app = App::test_default();
        let mut mention = MentionState::new(0, 0, "src".to_owned(), vec![candidate("src/lib.rs")]);
        mention.search_status = MentionSearchStatus::Ready;
        app.mention = Some(mention);

        request_match_for_active_mention(&mut app, MatchRequestMode::UserQuery);

        let mention = app.mention.as_ref().expect("mention should remain active");
        assert_eq!(mention.candidates.len(), 1);
        assert_eq!(mention.candidates[0].rel_path, "src/lib.rs");
        assert!(matches!(mention.search_status, MentionSearchStatus::Searching));
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
        run_search(&mut app);

        let mention = app.mention.as_ref().expect("mention should remain active");
        assert_eq!(app.file_index.generation, initial_generation);
        assert_eq!(mention.candidates.len(), 1);
        assert_eq!(mention.candidates[0].rel_path, "src/nested/needle.rs");
    }

    #[test]
    fn scan_refresh_does_not_starve_pending_query_result() {
        let mut app = App::test_default();
        app.file_index.index_version = 5;
        app.file_index.scan_finished = false;
        let mut state = MentionState::new(0, 0, "web".to_owned(), Vec::new());
        state.next_match_sequence = 1;
        state.pending_match_sequence = Some(1);
        app.mention = Some(state);

        refresh_from_file_index_after_scan_batch(&mut app);

        let mention = app.mention.as_ref().expect("mention should remain active");
        assert_eq!(mention.pending_match_sequence, Some(1));
        assert!(mention.refresh_after_pending_match);

        let generation = app.file_index.generation;
        let applied = apply_match_result(
            &mut app,
            file_index::MentionMatchResult {
                generation,
                index_version: 1,
                sequence: 1,
                query: "web".to_owned(),
                candidates: vec![file_index::FileCandidate {
                    rel_path: "web_dev_work/".to_owned(),
                    rel_path_lower: "web_dev_work/".to_owned(),
                    basename_lower: "web_dev_work".to_owned(),
                    depth: 0,
                }],
                scan_finished: false,
            },
        );

        let mention = app.mention.as_ref().expect("mention should remain active");
        assert!(applied);
        assert_eq!(mention.candidates[0].rel_path, "web_dev_work/");
        assert_eq!(mention.pending_match_sequence, Some(2));
        assert!(!mention.refresh_after_pending_match);
    }

    #[test]
    fn basename_prefix_ranks_ahead_of_shallow_path_substring() {
        let mut app = App::test_default();
        app.file_index.root = Some(std::path::PathBuf::from(&app.cwd_raw));
        app.file_index.respect_gitignore = app.config.respect_gitignore_effective();
        app.file_index.scan_finished = true;
        app.file_index.entries.insert(
            "docs/guide-rs.txt".to_owned(),
            file_index::FileCandidate {
                rel_path: "docs/guide-rs.txt".to_owned(),
                rel_path_lower: "docs/guide-rs.txt".to_owned(),
                basename_lower: "guide-rs.txt".to_owned(),
                depth: 1,
            },
        );
        app.file_index.entries.insert(
            "src/rs-helper.rs".to_owned(),
            file_index::FileCandidate {
                rel_path: "src/rs-helper.rs".to_owned(),
                rel_path_lower: "src/rs-helper.rs".to_owned(),
                basename_lower: "rs-helper.rs".to_owned(),
                depth: 1,
            },
        );

        app.input.set_text("@rs");
        let _ = app.input.set_cursor(0, 3);
        activate(&mut app);
        run_search(&mut app);

        let mention = app.mention.as_ref().expect("mention should be active");
        assert_eq!(mention.candidates[0].rel_path, "src/rs-helper.rs");
    }
}
