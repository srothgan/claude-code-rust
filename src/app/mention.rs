// SPDX-License-Identifier: Apache-2.0
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
    pending_match: Option<PendingMatch>,
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

/// An in-flight match request, tagged with the file index generation it was
/// issued against.
///
/// The generation is part of the identity because a restart voids every request
/// belonging to the previous one: the matcher drops stale-generation requests
/// without replying, so a request from a dead generation will never be answered
/// and must not be mistaken for one that is still live.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PendingMatch {
    sequence: u64,
    generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MentionSearchStatus {
    Hint,
    WaitingForIndex,
    Searching,
    Ready,
    NoMatches,
    Limited,
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
            pending_match: None,
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
            MentionSearchStatus::WaitingForIndex => {
                Some("File search will start when the session is ready".to_owned())
            }
            MentionSearchStatus::Searching => Some("Searching files...".to_owned()),
            MentionSearchStatus::NoMatches => Some("No matching files or folders".to_owned()),
            MentionSearchStatus::Limited => Some(format!(
                "No match in the first {} indexed files or folders",
                file_index::MAX_INDEX_ENTRIES
            )),
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
        self.pending_match = None;
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
    app.slash.clear();
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

    // The pending request must match on both identities. Sequence alone is not
    // enough, because a result from a dead generation can still match a pending
    // request recorded under that same dead generation.
    let matches_pending = mention.pending_match.is_some_and(|pending| {
        pending.generation == result.generation && pending.sequence == result.sequence
    });

    if result.generation != app.file_index.generation
        || result.query != mention.query
        || !matches_pending
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
            pending_sequence = mention.pending_match.map(|pending| pending.sequence),
            pending_generation = mention.pending_match.map(|pending| pending.generation),
            result_query_chars = result.query.chars().count(),
            current_query_chars = mention.query.chars().count(),
            query_matches = result.query == mention.query,
            result_count = result.candidates.len(),
        );
        return false;
    }

    let result_count = result.candidates.len();
    mention.candidates = result.candidates;
    mention.pending_match = None;
    let needs_follow_up = mention.refresh_after_pending_match
        || result.index_version < app.file_index.index_version
        || result.status != app.file_index.status;
    mention.refresh_after_pending_match = false;
    mention.search_status = if mention.candidates.is_empty() {
        if result.status.is_limited() {
            MentionSearchStatus::Limited
        } else if result.status.is_finished() {
            MentionSearchStatus::NoMatches
        } else {
            MentionSearchStatus::Searching
        }
    } else if result.status.is_finished() {
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
        result_index_status = ?result.status,
        current_index_status = ?app.file_index.status,
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

    if app.file_index.root.is_none() {
        mention.search_status = MentionSearchStatus::WaitingForIndex;
        sync_focus(app);
        return;
    }
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
    // Only a request belonging to the current generation can still be answered,
    // so only that one may hold off another refresh.
    let has_live_pending =
        mention.pending_match.is_some_and(|pending| pending.generation == generation);
    if has_live_pending && !matches!(mode, MatchRequestMode::UserQuery) {
        mention.refresh_after_pending_match = true;
        tracing::debug!(
            target: crate::logging::targets::APP_FILE_INDEX,
            event_name = "mention_match_request_deferred",
            message = "mention deferred file index match request while one is pending",
            generation,
            index_version,
            mode = mode.as_str(),
            pending_sequence = mention.pending_match.map(|pending| pending.sequence),
            query_chars = query.chars().count(),
        );
        return;
    }
    mention.next_match_sequence = mention.next_match_sequence.saturating_add(1);
    let sequence = mention.next_match_sequence;
    mention.pending_match = Some(PendingMatch { sequence, generation });
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

    if app.slash.visible().is_none() && app.subagent.is_none() {
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
    if app.slash.visible().is_none() && app.subagent.is_none() {
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
        if entries.contains_key(&candidate)
            && chars.get(end_col).is_none_or(|ch| is_bare_mention_delimiter(*ch))
        {
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

    /// Budget for the background walk and matcher to settle before a test reads
    /// mention state. Sized for the slowest CI runner, not local hardware.
    const SEARCH_SETTLE_TIMEOUT: Duration = Duration::from_secs(30);
    const SEARCH_SETTLE_POLL_INTERVAL: Duration = Duration::from_millis(5);

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
        app.file_index.status = file_index::FileIndexStatus::Complete;
        for path in paths {
            app.file_index.entries.insert((*path).to_owned(), candidate(path));
        }
    }

    fn start_session_index(app: &mut App) {
        file_index::restart(app);
    }

    /// Drives the background file index until the mention search settles, and
    /// fails loudly if it does not. Returning silently on timeout would let a
    /// test assert against a half-scanned index and report a candidate
    /// mismatch instead of the timeout that actually caused it.
    fn run_search(app: &mut App) {
        let deadline = Instant::now() + SEARCH_SETTLE_TIMEOUT;
        loop {
            crate::app::file_index::drain_events(app);
            let index_is_settled =
                !matches!(app.file_index.status, file_index::FileIndexStatus::Scanning);
            let mention_is_settled = app.mention.as_ref().is_none_or(|mention| {
                mention.pending_match.is_none()
                    && !matches!(mention.search_status, MentionSearchStatus::Searching)
            });
            if index_is_settled && mention_is_settled {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "mention search did not settle within {SEARCH_SETTLE_TIMEOUT:?}: \
                 index_status={:?}, search_status={:?}, pending_match={:?}",
                app.file_index.status,
                app.mention.as_ref().map(|mention| mention.search_status),
                app.mention.as_ref().and_then(|mention| mention.pending_match),
            );
            std::thread::sleep(SEARCH_SETTLE_POLL_INTERVAL);
        }
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

    fn select_path(app: &mut App, path: &str) {
        run_search(app);
        let candidates = &app.mention.as_ref().unwrap().candidates;
        let target = candidates
            .iter()
            .position(|c| c.rel_path == path)
            .unwrap_or_else(|| panic!("missing {path}: {candidates:?}"));
        for _ in 0..target {
            move_down(app);
        }
        assert_eq!(
            app.mention.as_ref().unwrap().candidates[app.mention.as_ref().unwrap().dialog.selected]
                .rel_path,
            path
        );
    }
    #[test]
    fn mention_span_table_preserves_indexed_and_partial_quote_boundaries() {
        for (source, path, start, end) in [
            ("open @docs/my file.md now", "docs/my file.md", 5, 21),
            ("open @docs/my folder/ now", "docs/my folder/", 5, 21),
            ("open @'docs/my file.md' now", "docs/my file.md", 5, 23),
            ("@src/app/mention.rs'", "src/app/mention.rs", 0, 19),
            ("@'src/app/mention.rs", "src/app/mention.rs", 0, 20),
            ("界 @'é.md' tail", "é.md", 2, 9),
        ] {
            let mut app = App::test_default();
            index_paths(&mut app, &[path]);
            assert_eq!(
                find_mention_spans(1, source, &app.file_index, None),
                [(start, end, path.to_owned())],
                "{source}"
            );
        }
    }
    #[test]
    fn mention_reentry_confirms_the_real_match_without_consuming_the_suffix() {
        for (source, cursor_prefix, target, completed, caret) in [
            (
                "open @src/main.rs now",
                "open @src",
                "src/main.rs",
                "open @'src/main.rs' now",
                "open @'src/main.rs'",
            ),
            (
                "open @'src/main.rs' now",
                "open @'src/ma",
                "src/main.rs",
                "open @'src/main.rs' now",
                "open @'src/main.rs'",
            ),
            (
                "界 @docs/my folder/read me.md now",
                "界 @docs/my folder/read",
                "docs/my folder/read me.md",
                "界 @'docs/my folder/read me.md' now",
                "界 @'docs/my folder/read me.md'",
            ),
        ] {
            let (mut app, _tmp) = app_with_temp_files(&[
                target,
                "src/mapping.rs",
                "docs/my folder/reader.md",
                "unrelated.txt",
            ]);
            start_session_index(&mut app);
            run_search(&mut app);
            app.input.set_text(&format!("FIRST\n{source}\nLAST"));
            app.input.set_cursor(1, cursor_prefix.chars().count());
            sync_with_cursor(&mut app);
            select_path(&mut app, target);
            confirm_selection(&mut app);
            assert_eq!(app.input.text(), format!("FIRST\n{completed}\nLAST"));
            assert_eq!(app.input.cursor(), (1, caret.chars().count()));
            assert!(app.mention.is_none());
        }
    }
    #[test]
    fn mention_edit_receives_the_matching_file_before_replacing_the_entire_token() {
        let (mut app, _tmp) = app_with_temp_files(&[
            "docs/my folder/read me.txt",
            "docs/my folder/read me.md",
            "other.txt",
        ]);
        start_session_index(&mut app);
        run_search(&mut app);
        app.input.set_text("open @docs/my folder/read me.txt now");
        app.input.set_cursor(0, "open @docs/my folder/read me.".chars().count());
        sync_with_cursor(&mut app);
        run_search(&mut app);
        app.input.textarea_insert_char('m');
        update_query(&mut app);
        select_path(&mut app, "docs/my folder/read me.md");
        confirm_selection(&mut app);
        assert_eq!(app.input.text(), "open @'docs/my folder/read me.md' now");
        assert_eq!(app.input.cursor(), (0, "open @'docs/my folder/read me.md'".chars().count()));
        app.input.textarea_insert_char('!');
        assert_eq!(app.input.text(), "open @'docs/my folder/read me.md'! now");
    }
    #[test]
    fn mention_confirmation_quotes_selected_paths_and_preserves_separators() {
        for (source, prefix, target, expected, caret) in [
            (
                "open @src/lib.txt now",
                "open @src/lib",
                "src/lib.rs",
                "open @'src/lib.rs' now",
                "open @'src/lib.rs'",
            ),
            ("@src/mai", "@src/mai", "src/main.rs", "@'src/main.rs' ", "@'src/main.rs' "),
            (
                "@path",
                "@path",
                "path with spaces.md",
                "@'path with spaces.md' ",
                "@'path with spaces.md' ",
            ),
            (
                "open @path with typo later",
                "open @path with",
                "path with spaces.md",
                "open @'path with spaces.md' later",
                "open @'path with spaces.md'",
            ),
        ] {
            let (mut app, _tmp) = app_with_temp_files(&[target, "path with typo", "other.txt"]);
            start_session_index(&mut app);
            run_search(&mut app);
            app.input.set_text(&format!("FIRST\n{source}\nLAST"));
            app.input.set_cursor(1, prefix.chars().count());
            sync_with_cursor(&mut app);
            assert!(
                app.mention.is_some(),
                "missing menu for {source:?}, cursor {:?}, index {:?}",
                app.input.cursor(),
                app.file_index.entries.keys().collect::<Vec<_>>()
            );
            select_path(&mut app, target);
            confirm_selection(&mut app);
            assert_eq!(app.input.text(), format!("FIRST\n{expected}\nLAST"));
            assert_eq!(app.input.cursor(), (1, caret.chars().count()));
            assert!(app.mention.is_none());
            assert_eq!(app.focus_owner(), crate::app::FocusOwner::Input);
        }
    }
    #[test]
    fn literal_mention_commit_preserves_spacing_and_remains_editable() {
        for (suffix, typed, expected, caret) in [
            ("", "manual path", "界 @'manual path' ", "界 @'manual path' "),
            (" now", "manual path", "界 @'manual path' now", "界 @'manual path'"),
            ("", "manual path ", "界 @'manual path' ", "界 @'manual path'"),
            ("", " ", "界 @ ", "界 @ "),
        ] {
            let (mut app, _tmp) = app_with_temp_files(&["unrelated.txt"]);
            start_session_index(&mut app);
            run_search(&mut app);
            app.input.set_text(&format!("FIRST\n界 @{suffix}\nLAST"));
            app.input.set_cursor(1, 3);
            activate(&mut app);
            for c in typed.chars() {
                app.input.textarea_insert_char(c);
                update_query(&mut app);
            }
            run_search(&mut app);
            assert!(app.mention.as_ref().unwrap().candidates.is_empty());
            assert!(commit_literal_if_active(&mut app));
            assert_eq!(app.input.text(), format!("FIRST\n{expected}\nLAST"));
            assert_eq!(app.input.cursor(), (1, caret.chars().count()));
            assert!(app.mention.is_none());
            assert_eq!(app.focus_owner(), crate::app::FocusOwner::Input);
            app.input.textarea_insert_char('!');
            assert_eq!(
                app.input.text(),
                format!("FIRST\n{caret}!{}\nLAST", expected.strip_prefix(caret).unwrap())
            );
        }
    }
    fn search_paths(app: &App) -> Vec<String> {
        let mut paths = app
            .mention
            .as_ref()
            .map(|m| m.candidates.iter().map(|c| c.rel_path.clone()).collect::<Vec<_>>())
            .unwrap_or_default();
        paths.sort();
        paths
    }
    fn search_menu(app: &App) -> Vec<String> {
        crate::ui::input_rows::build_composer_hint_rows(app)
            .iter()
            .map(ToString::to_string)
            .collect()
    }
    fn deliver_match(
        app: &mut App,
        request: &file_index::MentionMatchRequest,
        paths: &[&str],
        version: u64,
        status: file_index::FileIndexStatus,
    ) {
        app.file_index_event_tx
            .send(file_index::FileIndexEvent::MatchResult(file_index::MentionMatchResult {
                generation: request.generation,
                index_version: version,
                sequence: request.sequence,
                query: request.query.clone(),
                candidates: paths.iter().map(|p| candidate(p)).collect(),
                status,
            }))
            .unwrap();
        file_index::drain_events(app);
    }
    fn current_match(app: &mut App, request: &file_index::MentionMatchRequest, paths: &[&str]) {
        deliver_match(app, request, paths, app.file_index.index_version, app.file_index.status);
    }
    #[test]
    fn blank_mentions_release_selection_and_one_character_search_recovers() {
        let (mut app, _tmp) = app_with_temp_files(&["alpha.txt", "beta.txt"]);
        start_session_index(&mut app);
        run_search(&mut app);
        app.input.set_text("@");
        activate(&mut app);
        run_search(&mut app);
        assert!(search_paths(&app).is_empty());
        assert_eq!(app.focus_owner(), crate::app::FocusOwner::Input);
        app.input.insert_str("a");
        update_query(&mut app);
        run_search(&mut app);
        assert_eq!(search_paths(&app), ["alpha.txt", "beta.txt"]);
        app.input.textarea_delete_char_before();
        update_query(&mut app);
        run_search(&mut app);
        assert!(search_paths(&app).is_empty());
        assert_eq!(app.input.text(), "@");
        assert_eq!(app.focus_owner(), crate::app::FocusOwner::Input);
        app.input.insert_str(" ");
        update_query(&mut app);
        run_search(&mut app);
        assert!(search_paths(&app).is_empty());
        assert_eq!(app.input.text(), "@ ");
        assert!(!search_menu(&app).iter().any(|row| row.contains("> ")));
        app.input.textarea_delete_char_before();
        app.input.insert_str("beta");
        update_query(&mut app);
        select_path(&mut app, "beta.txt");
        confirm_selection(&mut app);
        assert_eq!(app.input.text(), "@'beta.txt' ");
    }
    #[test]
    fn waiting_mention_uses_session_index_without_retyping_its_query() {
        let (mut app, _tmp) = app_with_temp_files(&["ready.txt", "other.md"]);
        app.input.set_text("open @ready later");
        app.input.set_cursor(0, 11);
        activate(&mut app);
        assert!(app.file_index.root.is_none() && app.file_index.scan.is_none());
        assert!(search_paths(&app).is_empty());
        assert_eq!(app.focus_owner(), crate::app::FocusOwner::Input);
        start_session_index(&mut app);
        select_path(&mut app, "ready.txt");
        confirm_selection(&mut app);
        assert_eq!(app.input.text(), "open @'ready.txt' later");
    }
    #[test]
    fn saved_ignore_policy_controls_selectable_paths_and_confirmation() {
        for respect in [true, false] {
            let (mut app, tmp) = app_with_temp_files(&["visible.rs", "ignored.rs", ".gitignore"]);
            std::fs::create_dir(tmp.path().join(".git")).unwrap();
            std::fs::write(tmp.path().join(".gitignore"), "ignored.rs\n").unwrap();
            if !respect {
                app.config.snapshot = Some(crate::agent::settings::SettingsSnapshot::test_value(
                    "respectGitignore",
                    serde_json::json!(false),
                ));
            }
            start_session_index(&mut app);
            app.input.set_text("@rs");
            activate(&mut app);
            run_search(&mut app);
            assert_eq!(
                search_paths(&app),
                if respect { vec!["visible.rs"] } else { vec!["ignored.rs", "visible.rs"] }
            );
            let target = if respect { "visible.rs" } else { "ignored.rs" };
            select_path(&mut app, target);
            confirm_selection(&mut app);
            assert_eq!(app.input.text(), format!("@'{target}' "));
        }
    }
    #[test]
    fn refresh_keeps_selected_rows_until_the_current_result_replaces_them() {
        let (mut app, _tmp) = app_with_temp_files(&["alpha.rs", "beta.rs", "next.txt"]);
        start_session_index(&mut app);
        app.input.set_text("@rs");
        activate(&mut app);
        run_search(&mut app);
        let initial = search_menu(&app).into_iter().find(|r| r.contains("> ")).unwrap();
        move_down(&mut app);
        let before = search_menu(&app);
        let selected = before.iter().filter(|r| r.contains("> ")).collect::<Vec<_>>();
        assert_eq!(selected.len(), 1);
        assert_ne!(selected[0], &initial);
        assert!(selected[0].contains("alpha.rs") || selected[0].contains("beta.rs"));
        let held = file_index::test_support::HeldMatcher::install(&mut app);
        app.input.set_text("@next");
        update_query(&mut app);
        let request = held.request();
        assert_eq!(search_menu(&app), before);
        current_match(&mut app, &request, &["next.txt"]);
        assert_eq!(search_paths(&app), ["next.txt"]);
        assert!(search_menu(&app).iter().any(|r| r.contains("> ") && r.contains("next.txt")));
        confirm_selection(&mut app);
        assert_eq!(app.input.text(), "@'next.txt' ");
    }
    #[test]
    fn completed_tree_search_excludes_nonmatches_and_confirms_deep_file() {
        let (mut app, _tmp) = app_with_temp_files(&["root.rs", "nested/deep.rs", "unrelated.txt"]);
        start_session_index(&mut app);
        app.input.set_text("@rs");
        activate(&mut app);
        run_search(&mut app);
        assert_eq!(search_paths(&app), ["nested/deep.rs", "root.rs"]);
        select_path(&mut app, "nested/deep.rs");
        confirm_selection(&mut app);
        assert_eq!(app.input.text(), "@'nested/deep.rs' ");
    }
    #[test]
    fn index_rebuild_replaces_a_held_request_with_new_generation_content() {
        let (mut app, tmp) = app_with_temp_files(&["old.rs"]);
        start_session_index(&mut app);
        app.input.set_text("@rs");
        activate(&mut app);
        run_search(&mut app);
        app.file_index.watch = None;
        let held = file_index::test_support::HeldMatcher::install(&mut app);
        update_query(&mut app);
        let stale = held.request();
        std::fs::remove_file(tmp.path().join("old.rs")).unwrap();
        std::fs::write(tmp.path().join("new.rs"), "").unwrap();
        app.file_index_event_tx
            .send(file_index::FileIndexEvent::RebuildRequested { generation: stale.generation })
            .unwrap();
        file_index::drain_events(&mut app);
        run_search(&mut app);
        assert_eq!(search_paths(&app), ["new.rs"]);
        select_path(&mut app, "new.rs");
        confirm_selection(&mut app);
        assert_eq!(app.input.text(), "@'new.rs' ");
    }
    #[test]
    fn scan_batches_preserve_a_live_partial_result_then_deliver_new_candidates() {
        let mut app = App::test_default();
        index_paths(&mut app, &["web-old.rs"]);
        app.file_index.status = file_index::FileIndexStatus::Scanning;
        let held = file_index::test_support::HeldMatcher::install(&mut app);
        app.input.set_text("@web");
        activate(&mut app);
        let first = held.request();
        app.mention.as_mut().unwrap().last_scan_batch_request_at =
            Instant::now().checked_sub(SCAN_BATCH_REFRESH_INTERVAL);
        app.file_index_event_tx
            .send(file_index::FileIndexEvent::ScanBatch {
                generation: first.generation,
                entries: vec![candidate("web-new.rs")],
            })
            .unwrap();
        file_index::drain_events(&mut app);
        assert!(held.has_no_request());
        deliver_match(
            &mut app,
            &first,
            &["web-old.rs"],
            first.index_version,
            file_index::FileIndexStatus::Scanning,
        );
        assert_eq!(search_paths(&app), ["web-old.rs"]);
        assert!(search_menu(&app).iter().any(|r| r.contains("web-old.rs")));
        let follow_up = held.request();
        assert_eq!(follow_up.generation, app.file_index.generation);
        current_match(&mut app, &follow_up, &["web-old.rs", "web-new.rs"]);
        move_down(&mut app);
        confirm_selection(&mut app);
        assert_eq!(app.input.text(), "@'web-new.rs' ");
    }
    #[test]
    fn stale_match_events_preserve_current_selection_and_do_not_consume_live_requests() {
        for scenario in ["superseded", "old-query", "old-generation", "dismissed"] {
            let mut app = App::test_default();
            index_paths(&mut app, &["seed-a.rs", "seed-b.rs", "current.rs", "obsolete.rs"]);
            let mut held = file_index::test_support::HeldMatcher::install(&mut app);
            app.input.set_text("@seed");
            activate(&mut app);
            let first = held.request();
            current_match(&mut app, &first, &["seed-a.rs", "seed-b.rs"]);
            move_down(&mut app);
            let stale;
            match scenario {
                "superseded" => {
                    update_query(&mut app);
                    stale = held.request();
                    update_query(&mut app);
                }
                "old-query" => {
                    deactivate(&mut app);
                    app.input.set_text("@old");
                    activate(&mut app);
                    stale = held.request();
                    deactivate(&mut app);
                    app.input.set_text("@current");
                    activate(&mut app);
                }
                "old-generation" => {
                    update_query(&mut app);
                    stale = held.request();
                    file_index::reset(&mut app);
                    index_paths(&mut app, &["current.rs"]);
                    held = file_index::test_support::HeldMatcher::install(&mut app);
                    app.input.set_text("@current");
                    update_query(&mut app);
                }
                _ => {
                    update_query(&mut app);
                    stale = held.request();
                    deactivate(&mut app);
                }
            }
            let current = if scenario == "dismissed" { None } else { Some(held.request()) };
            if scenario == "old-query" {
                assert_eq!(stale.sequence, current.as_ref().unwrap().sequence);
                assert_ne!(stale.query, current.as_ref().unwrap().query);
            }
            let menu = search_menu(&app);
            let draft = app.input.text();
            let focus = app.focus_owner();
            let pending = app.mention.as_ref().and_then(|m| m.pending_match);
            current_match(&mut app, &stale, &["obsolete.rs"]);
            assert_eq!(search_menu(&app), menu, "{scenario}");
            assert_eq!(app.input.text(), draft);
            assert_eq!(app.focus_owner(), focus);
            assert_eq!(app.mention.as_ref().and_then(|m| m.pending_match), pending);
            let current = current.unwrap_or_else(|| {
                assert!(app.mention.is_none());
                app.input.set_text("@current");
                activate(&mut app);
                held.request()
            });
            current_match(&mut app, &current, &["current.rs"]);
            assert_eq!(search_paths(&app), ["current.rs"]);
            confirm_selection(&mut app);
            assert_eq!(app.input.text(), "@'current.rs' ");
        }
    }
    fn assert_escaped_highlight(app: &mut App, source: &str, token: &str) {
        use ratatui::{buffer::Buffer, layout::Rect, widgets::Widget};
        crate::ui::input::configure_input_textarea(app);
        let area = Rect::new(0, 0, 120, 1);
        let mut b = Buffer::empty(area);
        app.input.editor().render(area, &mut b);
        let cells = b
            .content
            .iter()
            .flat_map(|c| {
                c.symbol().chars().filter(|ch| !ch.is_whitespace()).map(move |ch| (ch, c.style()))
            })
            .collect::<Vec<_>>();
        let plain = source.chars().filter(|c| !c.is_whitespace()).collect::<String>();
        assert_eq!(cells.iter().map(|x| x.0).collect::<String>(), plain);
        let token = token.chars().filter(|c| !c.is_whitespace()).collect::<String>();
        let begin = plain[..plain.find(&token).unwrap()].chars().count();
        let end = begin + token.chars().count();
        let ordinary = cells.last().unwrap().1;
        assert_ne!(cells[begin].1, ordinary);
        for (i, (_, style)) in cells.iter().enumerate() {
            if (begin..end).contains(&i) {
                assert_eq!(*style, cells[begin].1);
            } else {
                assert_eq!(*style, ordinary);
            }
        }
    }
    #[test]
    fn escaped_selected_and_literal_paths_preserve_identity_on_reentry_and_rendering() {
        let (mut app, _tmp) = app_with_temp_files(&["docs/it's me.md"]);
        start_session_index(&mut app);
        run_search(&mut app);
        app.input.set_text("界 @docs/it END");
        app.input.set_cursor(0, "界 @docs/it".chars().count());
        activate(&mut app);
        select_path(&mut app, "docs/it's me.md");
        confirm_selection(&mut app);
        let expected = "界 @'docs/it\\'s me.md' END";
        let token = "@'docs/it\\'s me.md'";
        assert_eq!(app.input.text(), expected);
        assert_escaped_highlight(&mut app, expected, token);
        app.input.textarea_insert_char('!');
        assert_eq!(app.input.text(), "界 @'docs/it\\'s me.md'! END");
        app.input.textarea_delete_char_before();
        app.input.set_cursor(0, "界 @'docs/it\\'s me.md".chars().count());
        sync_with_cursor(&mut app);
        run_search(&mut app);
        let m = app.mention.as_ref().unwrap();
        assert_eq!(m.query, "docs/it's me.md");
        assert_eq!(m.replace_end_col, "界 @'docs/it\\'s me.md'".chars().count());
        select_path(&mut app, "docs/it's me.md");
        confirm_selection(&mut app);
        assert_eq!(app.input.text(), expected);
        for (literal, expected, token, decoded) in [
            ("it's missing", "界 @'it\\'s missing' END", "@'it\\'s missing'", "it's missing"),
            (r"folder\notes", "界 @'folder\\\\notes' END", "@'folder\\\\notes'", r"folder\notes"),
        ] {
            let mut app = App::test_default();
            app.input.set_text("界 @ END");
            app.input.set_cursor(0, 3);
            activate(&mut app);
            app.input.insert_str(literal);
            update_query(&mut app);
            assert!(commit_literal_if_active(&mut app));
            assert_eq!(app.input.text(), expected);
            assert_escaped_highlight(&mut app, expected, token);
            let caret = 2 + token.chars().count();
            assert_eq!(app.input.cursor(), (0, caret));
            app.input.textarea_insert_char('!');
            assert_eq!(app.input.text(), format!("界 {token}! END"));
            app.input.textarea_delete_char_before();
            app.input.set_cursor(0, caret - 1);
            sync_with_cursor(&mut app);
            let m = app.mention.as_ref().unwrap();
            assert_eq!(m.query, decoded);
            assert_eq!(m.replace_end_col, caret);
            assert!(commit_literal_if_active(&mut app));
            assert_eq!(app.input.text(), expected);
        }
    }
}
