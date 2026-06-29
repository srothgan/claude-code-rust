// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::App;
use nucleo::pattern::{CaseMatching, Normalization};
use nucleo::{Config as NucleoConfig, Injector, Nucleo, Utf32String};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::time::{Duration, Instant, SystemTime};

const SCAN_BATCH_SIZE: usize = 256;
const EVENT_DRAIN_BUDGET: usize = 64;
const WATCH_POLL_INTERVAL: Duration = Duration::from_millis(100);
const WATCH_DEBOUNCE_INTERVAL: Duration = Duration::from_millis(50);
const MAX_CANDIDATES: usize = 50;
const NUCLEO_COLUMNS: u32 = 1;
const NUCLEO_QUERY_TICK_MS: u64 = 10;
const MATCHER_IDLE_POLL_INTERVAL: Duration = Duration::from_millis(5);

#[derive(Clone, Debug)]
pub struct FileCandidate {
    pub rel_path: String,
    pub rel_path_lower: String,
    pub basename_lower: String,
    pub depth: usize,
    pub modified: SystemTime,
    pub is_dir: bool,
}

#[derive(Default)]
pub struct FileIndexState {
    pub root: Option<PathBuf>,
    pub respect_gitignore: bool,
    pub generation: u64,
    pub index_version: u64,
    pub entries: BTreeMap<String, FileCandidate>,
    pub scan_finished: bool,
    pub rebuild_pending: bool,
    scan_overrides: ScanOverrides,
    pub scan: Option<FileIndexScanHandle>,
    pub watch: Option<FileIndexWatchHandle>,
    pub matcher: Option<FileIndexMatcherHandle>,
}

pub struct FileIndexScanHandle {
    cancel: Arc<AtomicBool>,
}

pub struct FileIndexWatchHandle {
    cancel: Arc<AtomicBool>,
}

pub struct FileIndexMatcherHandle {
    index_tx: Sender<FileIndexMatcherCommand>,
    query_tx: Sender<MentionMatchRequest>,
}

#[derive(Clone, Copy, Default)]
struct EventApplyStats {
    index_changed: bool,
    mention_changed: bool,
    scan_entries: usize,
    fs_changes: usize,
    rebuilds: usize,
    scan_finished: bool,
    match_results: usize,
}

impl EventApplyStats {
    fn merge(&mut self, other: Self) {
        self.index_changed |= other.index_changed;
        self.mention_changed |= other.mention_changed;
        self.scan_entries += other.scan_entries;
        self.fs_changes += other.fs_changes;
        self.rebuilds += other.rebuilds;
        self.scan_finished |= other.scan_finished;
        self.match_results += other.match_results;
    }
}

fn elapsed_ms(started_at: Instant) -> u128 {
    started_at.elapsed().as_millis()
}

#[derive(Clone)]
pub enum FileIndexChange {
    Upsert(FileCandidate),
    RemoveExact { rel_path: String },
    RemovePrefix { rel_prefix: String },
    ReplacePrefix { rel_prefix: String, entries: Vec<FileCandidate> },
}

pub enum FileIndexEvent {
    ScanBatch { generation: u64, entries: Vec<FileCandidate> },
    ScanFinished { generation: u64 },
    FsBatch { generation: u64, changes: Vec<FileIndexChange> },
    RebuildRequested { generation: u64 },
    MatchResult(MentionMatchResult),
}

#[derive(Clone, Debug)]
pub struct MentionMatchRequest {
    pub generation: u64,
    pub index_version: u64,
    pub sequence: u64,
    pub query: String,
}

#[derive(Clone, Debug)]
pub struct MentionMatchResult {
    pub generation: u64,
    pub index_version: u64,
    pub sequence: u64,
    pub query: String,
    pub candidates: Vec<FileCandidate>,
    pub scan_finished: bool,
}

enum FileIndexMatcherCommand {
    Reset { generation: u64, index_version: u64, entries: Vec<FileCandidate>, scan_finished: bool },
    ScanBatch { generation: u64, index_version: u64, entries: Vec<FileCandidate> },
    ScanFinished { generation: u64, index_version: u64 },
    FsBatch { generation: u64, index_version: u64, changes: Vec<FileIndexChange> },
}

#[derive(Default)]
struct ScanOverrides {
    exact_paths: BTreeSet<String>,
    blocked_prefixes: Vec<String>,
}

impl Drop for FileIndexScanHandle {
    fn drop(&mut self) {
        self.cancel.store(true, AtomicOrdering::Relaxed);
    }
}

impl Drop for FileIndexWatchHandle {
    fn drop(&mut self) {
        self.cancel.store(true, AtomicOrdering::Relaxed);
    }
}

pub fn reset(app: &mut App) {
    app.file_index.generation = app.file_index.generation.saturating_add(1);
    app.file_index.index_version = 0;
    app.file_index.root = None;
    app.file_index.respect_gitignore = app.config.respect_gitignore_effective();
    app.file_index.entries.clear();
    app.file_index.scan_finished = false;
    app.file_index.rebuild_pending = false;
    app.file_index.scan_overrides = ScanOverrides::default();
    app.file_index.scan = None;
    app.file_index.watch = None;
    app.file_index.matcher = None;
}

pub fn restart(app: &mut App) {
    let previous_root = app.file_index.root.clone();
    let previous_entries = app.file_index.entries.len();
    let previous_generation = app.file_index.generation;
    reset(app);
    let root = PathBuf::from(&app.cwd_raw);
    let generation = app.file_index.generation;
    let respect_gitignore = app.config.respect_gitignore_effective();
    tracing::info!(
        target: crate::logging::targets::APP_FILE_INDEX,
        event_name = "file_index_restart",
        message = "file index restart requested",
        previous_generation,
        generation,
        previous_root = %previous_root
            .as_ref()
            .map_or_else(|| "<none>".to_owned(), |root| root.display().to_string()),
        root = %root.display(),
        previous_entries,
        respect_gitignore,
    );
    app.file_index.root = Some(root.clone());
    app.file_index.respect_gitignore = respect_gitignore;
    app.file_index.scan_finished = false;
    app.file_index.rebuild_pending = false;
    app.file_index.scan_overrides = ScanOverrides::default();
    app.file_index.matcher = Some(spawn_matcher(generation, app.file_index_event_tx.clone()));
    send_matcher_command(
        app,
        FileIndexMatcherCommand::Reset {
            generation,
            index_version: app.file_index.index_version,
            entries: Vec::new(),
            scan_finished: false,
        },
    );
    app.file_index.scan = Some(spawn_scan(
        root.clone(),
        generation,
        respect_gitignore,
        app.file_index_event_tx.clone(),
    ));
    app.file_index.watch =
        Some(spawn_watch(&root, generation, respect_gitignore, app.file_index_event_tx.clone()));
}

pub fn ensure_started(app: &mut App) {
    let respect_gitignore = app.config.respect_gitignore_effective();
    let current_root = PathBuf::from(&app.cwd_raw);
    let root_changed = app.file_index.root.as_ref() != Some(&current_root);
    let respect_gitignore_changed = app.file_index.respect_gitignore != respect_gitignore;
    let missing_root = app.file_index.root.is_none();
    let scan_missing_while_unfinished =
        !app.file_index.scan_finished && app.file_index.scan.is_none();
    let needs_restart =
        root_changed || respect_gitignore_changed || missing_root || scan_missing_while_unfinished;
    if needs_restart {
        tracing::debug!(
            target: crate::logging::targets::APP_FILE_INDEX,
            event_name = "file_index_ensure_restart",
            message = "file index ensure_started requested restart",
            root_changed,
            respect_gitignore_changed,
            missing_root,
            scan_missing_while_unfinished,
            current_root = %current_root.display(),
            indexed_root = %app.file_index.root
                .as_ref()
                .map_or_else(|| "<none>".to_owned(), |root| root.display().to_string()),
            generation = app.file_index.generation,
            entries = app.file_index.entries.len(),
            scan_finished = app.file_index.scan_finished,
        );
        restart(app);
    }
}

pub fn request_match(app: &mut App, request: MentionMatchRequest) {
    ensure_matcher(app);
    if let Some(matcher) = app.file_index.matcher.as_ref()
        && matcher.query_tx.send(request).is_err()
    {
        tracing::debug!(
            target: crate::logging::targets::APP_FILE_INDEX,
            event_name = "file_index_matcher_send_failed",
            message = "file index matcher query send failed",
            generation = app.file_index.generation,
        );
    }
}

pub fn drain_events(app: &mut App) {
    let started_at = Instant::now();
    let mut handled = 0;
    let mut stats = EventApplyStats::default();
    loop {
        if handled >= EVENT_DRAIN_BUDGET {
            break;
        }
        let event = match app.file_index_event_rx.try_recv() {
            Ok(event) => event,
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
        };
        stats.merge(apply_event(app, event));
        handled += 1;
    }
    if stats.index_changed {
        refresh_after_mutation(app, &stats);
    }
    if stats.index_changed || stats.mention_changed {
        app.request_chat_repaint();
    }
    if handled > 0 {
        let duration_ms = elapsed_ms(started_at);
        tracing::debug!(
            target: crate::logging::targets::APP_FILE_INDEX,
            event_name = "file_index_events_drained",
            message = "file index events drained on app thread",
            duration_ms,
            handled,
            index_changed = stats.index_changed,
            mention_changed = stats.mention_changed,
            scan_entries = stats.scan_entries,
            fs_changes = stats.fs_changes,
            rebuilds = stats.rebuilds,
            scan_finished_event = stats.scan_finished,
            match_results = stats.match_results,
            budget_hit = handled >= EVENT_DRAIN_BUDGET,
            total_entries = app.file_index.entries.len(),
            index_version = app.file_index.index_version,
            generation = app.file_index.generation,
            scan_finished = app.file_index.scan_finished,
            mention_active = app.mention.is_some(),
        );
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct MatcherUpsertStats {
    inserted: usize,
    replaced: usize,
}

#[derive(Default)]
struct MatcherEntries {
    entries: Vec<FileCandidate>,
    indices: HashMap<String, usize>,
}

impl MatcherEntries {
    fn len(&self) -> usize {
        self.entries.len()
    }

    fn iter(&self) -> impl Iterator<Item = &FileCandidate> {
        self.entries.iter()
    }

    fn reset(&mut self, entries: Vec<FileCandidate>) -> MatcherUpsertStats {
        self.entries.clear();
        self.indices.clear();
        self.entries.reserve(entries.len());
        self.indices.reserve(entries.len());

        let mut stats = MatcherUpsertStats::default();
        for entry in entries {
            let upsert_stats = self.upsert(entry);
            stats.inserted += upsert_stats.inserted;
            stats.replaced += upsert_stats.replaced;
        }
        stats
    }

    fn upsert(&mut self, candidate: FileCandidate) -> MatcherUpsertStats {
        if let Some(index) = self.indices.get(&candidate.rel_path).copied() {
            self.entries[index] = candidate;
            MatcherUpsertStats { replaced: 1, ..MatcherUpsertStats::default() }
        } else {
            let index = self.entries.len();
            self.indices.insert(candidate.rel_path.clone(), index);
            self.entries.push(candidate);
            MatcherUpsertStats { inserted: 1, ..MatcherUpsertStats::default() }
        }
    }

    fn remove_exact(&mut self, rel_path: &str) -> bool {
        let Some(index) = self.indices.remove(rel_path) else {
            return false;
        };

        self.entries.swap_remove(index);
        if index < self.entries.len() {
            self.indices.insert(self.entries[index].rel_path.clone(), index);
        }
        true
    }

    fn remove_prefix(&mut self, rel_prefix: &str) -> usize {
        let before = self.entries.len();
        self.entries.retain(|candidate| !candidate.rel_path.starts_with(rel_prefix));
        if self.entries.len() != before {
            self.rebuild_indices();
        }
        before - self.entries.len()
    }

    fn replace_prefix(
        &mut self,
        rel_prefix: &str,
        entries: Vec<FileCandidate>,
    ) -> (usize, MatcherUpsertStats) {
        let removed = self.remove_prefix(rel_prefix);
        let mut stats = MatcherUpsertStats::default();
        for entry in entries {
            let upsert_stats = self.upsert(entry);
            stats.inserted += upsert_stats.inserted;
            stats.replaced += upsert_stats.replaced;
        }
        (removed, stats)
    }

    fn apply_change(&mut self, change: FileIndexChange) -> MatcherChangeStats {
        match change {
            FileIndexChange::Upsert(candidate) => {
                let stats = self.upsert(candidate);
                MatcherChangeStats {
                    inserted: stats.inserted,
                    replaced: stats.replaced,
                    ..MatcherChangeStats::default()
                }
            }
            FileIndexChange::RemoveExact { rel_path } => MatcherChangeStats {
                removed: usize::from(self.remove_exact(&rel_path)),
                ..MatcherChangeStats::default()
            },
            FileIndexChange::RemovePrefix { rel_prefix } => MatcherChangeStats {
                removed: self.remove_prefix(&rel_prefix),
                ..MatcherChangeStats::default()
            },
            FileIndexChange::ReplacePrefix { rel_prefix, entries } => {
                let (removed, stats) = self.replace_prefix(&rel_prefix, entries);
                MatcherChangeStats { inserted: stats.inserted, replaced: stats.replaced, removed }
            }
        }
    }

    fn rebuild_indices(&mut self) {
        self.indices.clear();
        self.indices.reserve(self.entries.len());
        for (index, entry) in self.entries.iter().enumerate() {
            self.indices.insert(entry.rel_path.clone(), index);
        }
    }
}

struct MentionMatcher {
    entries: MatcherEntries,
    engine: Nucleo<FileCandidate>,
    injector: Injector<FileCandidate>,
    last_query: String,
}

impl Default for MentionMatcher {
    fn default() -> Self {
        let mut config = NucleoConfig::DEFAULT.match_paths();
        config.prefer_prefix = true;
        let notify = Arc::new(|| {});
        let engine = Nucleo::new(config, notify, None, NUCLEO_COLUMNS);
        let injector = engine.injector();
        Self { entries: MatcherEntries::default(), engine, injector, last_query: String::new() }
    }
}

impl MentionMatcher {
    fn len(&self) -> usize {
        self.entries.len()
    }

    fn reset(&mut self, entries: Vec<FileCandidate>) -> MatcherUpsertStats {
        let stats = self.entries.reset(entries);
        self.rebuild_stream(true);
        stats
    }

    fn upsert_scan_batch(&mut self, batch: Vec<FileCandidate>) -> MatcherUpsertStats {
        let mut stats = MatcherUpsertStats::default();
        let mut needs_rebuild = false;
        for entry in batch {
            let upsert_stats = self.entries.upsert(entry.clone());
            stats.inserted += upsert_stats.inserted;
            stats.replaced += upsert_stats.replaced;
            if upsert_stats.inserted > 0 {
                self.inject(entry);
            }
            needs_rebuild |= upsert_stats.replaced > 0;
        }
        if needs_rebuild {
            self.rebuild_stream(false);
        }
        stats
    }

    fn apply_change_batch(&mut self, changes: Vec<FileIndexChange>) -> MatcherChangeStats {
        let mut stats = MatcherChangeStats::default();
        for change in changes {
            stats.merge(self.entries.apply_change(change));
        }
        if stats.inserted > 0 || stats.replaced > 0 || stats.removed > 0 {
            self.rebuild_stream(false);
        }
        stats
    }

    fn set_query(&mut self, query: &str) {
        let append = query.starts_with(&self.last_query);
        self.last_query.clear();
        self.last_query.push_str(query);
        self.engine.pattern.reparse(0, query, CaseMatching::Smart, Normalization::Smart, append);
    }

    fn tick(&mut self, timeout_ms: u64) -> nucleo::Status {
        self.engine.tick(timeout_ms)
    }

    fn candidates(&self, limit: usize) -> Vec<FileCandidate> {
        let snapshot = self.engine.snapshot();
        let limit = u32::try_from(limit).unwrap_or(u32::MAX);
        let end = snapshot.matched_item_count().min(limit);
        snapshot.matched_items(0..end).map(|item| item.data.clone()).collect()
    }

    fn matched_item_count(&self) -> u32 {
        self.engine.snapshot().matched_item_count()
    }

    fn injected_items(&self) -> u32 {
        self.injector.injected_items()
    }

    fn rebuild_stream(&mut self, clear_snapshot: bool) {
        self.engine.restart(clear_snapshot);
        self.injector = self.engine.injector();
        let entries = self.entries.iter().cloned().collect::<Vec<_>>();
        for entry in entries {
            self.inject(entry);
        }
    }

    fn inject(&self, candidate: FileCandidate) {
        self.injector.push(candidate, |candidate, columns| {
            columns[0] = Utf32String::from(candidate.rel_path.as_str());
        });
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct MatcherChangeStats {
    inserted: usize,
    replaced: usize,
    removed: usize,
}

impl MatcherChangeStats {
    fn merge(&mut self, other: Self) {
        self.inserted += other.inserted;
        self.replaced += other.replaced;
        self.removed += other.removed;
    }
}

fn apply_event(app: &mut App, event: FileIndexEvent) -> EventApplyStats {
    match event {
        FileIndexEvent::ScanBatch { generation, entries } => {
            if generation != app.file_index.generation {
                return EventApplyStats::default();
            }
            let scan_entries = entries.len();
            let mut matcher_entries = Vec::with_capacity(entries.len());
            for entry in entries {
                if app.file_index.scan_overrides.blocks(&entry.rel_path) {
                    continue;
                }
                matcher_entries.push(entry.clone());
                app.file_index.entries.insert(entry.rel_path.clone(), entry);
            }
            if matcher_entries.is_empty() {
                return EventApplyStats { scan_entries, ..EventApplyStats::default() };
            }
            let index_version = bump_index_version(app);
            send_matcher_command(
                app,
                FileIndexMatcherCommand::ScanBatch {
                    generation,
                    index_version,
                    entries: matcher_entries,
                },
            );
            EventApplyStats { index_changed: true, scan_entries, ..EventApplyStats::default() }
        }
        FileIndexEvent::ScanFinished { generation } => {
            if generation != app.file_index.generation {
                return EventApplyStats::default();
            }
            app.file_index.scan_finished = true;
            app.file_index.scan_overrides = ScanOverrides::default();
            app.file_index.scan = None;
            let index_version = bump_index_version(app);
            send_matcher_command(
                app,
                FileIndexMatcherCommand::ScanFinished { generation, index_version },
            );
            EventApplyStats {
                index_changed: true,
                scan_finished: true,
                ..EventApplyStats::default()
            }
        }
        FileIndexEvent::FsBatch { generation, changes } => {
            if generation != app.file_index.generation {
                return EventApplyStats::default();
            }
            let fs_changes = changes.len();
            let matcher_changes = changes.clone();
            for change in changes {
                if !app.file_index.scan_finished {
                    app.file_index.scan_overrides.record_change(&change);
                }
                apply_change(&mut app.file_index.entries, change);
            }
            let index_version = bump_index_version(app);
            send_matcher_command(
                app,
                FileIndexMatcherCommand::FsBatch {
                    generation,
                    index_version,
                    changes: matcher_changes,
                },
            );
            EventApplyStats { index_changed: true, fs_changes, ..EventApplyStats::default() }
        }
        FileIndexEvent::RebuildRequested { generation } => {
            if generation != app.file_index.generation {
                return EventApplyStats::default();
            }
            restart(app);
            EventApplyStats { index_changed: true, rebuilds: 1, ..EventApplyStats::default() }
        }
        FileIndexEvent::MatchResult(result) => {
            let mention_changed = super::mention::apply_match_result(app, result);
            EventApplyStats { mention_changed, match_results: 1, ..EventApplyStats::default() }
        }
    }
}

fn refresh_after_mutation(app: &mut App, stats: &EventApplyStats) {
    if app.mention.is_some() {
        if stats.scan_finished || stats.fs_changes > 0 || stats.rebuilds > 0 {
            super::mention::refresh_from_file_index(app);
        } else {
            super::mention::refresh_from_file_index_after_scan_batch(app);
        }
    }
}

fn bump_index_version(app: &mut App) -> u64 {
    app.file_index.index_version = app.file_index.index_version.saturating_add(1);
    app.file_index.index_version
}

fn apply_change(entries: &mut BTreeMap<String, FileCandidate>, change: FileIndexChange) {
    match change {
        FileIndexChange::Upsert(candidate) => {
            entries.insert(candidate.rel_path.clone(), candidate);
        }
        FileIndexChange::RemoveExact { rel_path } => {
            entries.remove(&rel_path);
        }
        FileIndexChange::RemovePrefix { rel_prefix } => {
            entries.retain(|path, _| !path.starts_with(&rel_prefix));
        }
        FileIndexChange::ReplacePrefix { rel_prefix, entries: next_entries } => {
            entries.retain(|path, _| !path.starts_with(&rel_prefix));
            for entry in next_entries {
                entries.insert(entry.rel_path.clone(), entry);
            }
        }
    }
}

fn ensure_matcher(app: &mut App) {
    if app.file_index.matcher.is_some() {
        return;
    }
    let generation = app.file_index.generation;
    let index_version = app.file_index.index_version;
    let entries = app.file_index.entries.values().cloned().collect();
    let scan_finished = app.file_index.scan_finished;
    app.file_index.matcher = Some(spawn_matcher(generation, app.file_index_event_tx.clone()));
    send_matcher_command(
        app,
        FileIndexMatcherCommand::Reset { generation, index_version, entries, scan_finished },
    );
}

fn send_matcher_command(app: &mut App, command: FileIndexMatcherCommand) {
    let Some(matcher) = app.file_index.matcher.as_ref() else {
        return;
    };
    if matcher.index_tx.send(command).is_err() {
        app.file_index.matcher = None;
        tracing::warn!(
            target: crate::logging::targets::APP_FILE_INDEX,
            event_name = "file_index_matcher_send_failed",
            message = "file index matcher command channel is closed",
            generation = app.file_index.generation,
            index_version = app.file_index.index_version,
        );
    }
}

fn spawn_matcher(generation: u64, event_tx: Sender<FileIndexEvent>) -> FileIndexMatcherHandle {
    let (index_tx, index_rx) = mpsc::channel();
    let (query_tx, query_rx) = mpsc::channel();
    std::thread::spawn(move || {
        FileIndexMatcherRuntime::new(generation, index_rx, query_rx, event_tx).run();
    });
    FileIndexMatcherHandle { index_tx, query_tx }
}

struct FileIndexMatcherRuntime {
    generation: u64,
    index_version: u64,
    matcher: MentionMatcher,
    scan_finished: bool,
    index_rx: Receiver<FileIndexMatcherCommand>,
    query_rx: Receiver<MentionMatchRequest>,
    event_tx: Sender<FileIndexEvent>,
}

struct CoalescedQuery {
    request: MentionMatchRequest,
    replaced_queries: usize,
}

impl FileIndexMatcherRuntime {
    fn new(
        generation: u64,
        index_rx: Receiver<FileIndexMatcherCommand>,
        query_rx: Receiver<MentionMatchRequest>,
        event_tx: Sender<FileIndexEvent>,
    ) -> Self {
        Self {
            generation,
            index_version: 0,
            matcher: MentionMatcher::default(),
            scan_finished: false,
            index_rx,
            query_rx,
            event_tx,
        }
    }

    fn run(mut self) {
        tracing::debug!(
            target: crate::logging::targets::APP_FILE_INDEX,
            event_name = "file_index_matcher_started",
            message = "file index matcher thread started",
            generation = self.generation,
        );

        loop {
            if !self.handle_next_command() {
                break;
            }
        }

        tracing::debug!(
            target: crate::logging::targets::APP_FILE_INDEX,
            event_name = "file_index_matcher_stopped",
            message = "file index matcher thread stopped",
            generation = self.generation,
            index_version = self.index_version,
        );
    }

    fn handle_next_command(&mut self) -> bool {
        if let Ok(request) = self.query_rx.try_recv() {
            return self.handle_query(request);
        }

        match self.index_rx.recv_timeout(MATCHER_IDLE_POLL_INTERVAL) {
            Ok(command) => {
                self.apply_index_command(command);
                true
            }
            Err(RecvTimeoutError::Timeout) => true,
            Err(RecvTimeoutError::Disconnected) => {
                self.query_rx.try_recv().is_ok_and(|request| self.handle_query(request))
            }
        }
    }

    fn handle_query(&mut self, request: MentionMatchRequest) -> bool {
        let coalesced = self.coalesce_query(request);
        if coalesced.request.generation != self.generation {
            tracing::debug!(
                target: crate::logging::targets::APP_FILE_INDEX,
                event_name = "file_index_matcher_query_rejected",
                message = "file index matcher rejected stale mention query",
                matcher_generation = self.generation,
                request_generation = coalesced.request.generation,
                sequence = coalesced.request.sequence,
                query_chars = coalesced.request.query.chars().count(),
                replaced_queries = coalesced.replaced_queries,
            );
            return true;
        }
        self.complete_query(coalesced)
    }

    fn coalesce_query(&mut self, mut latest: MentionMatchRequest) -> CoalescedQuery {
        let mut replaced_queries = 0;
        loop {
            match self.query_rx.try_recv() {
                Ok(request) => {
                    latest = request;
                    replaced_queries += 1;
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => {
                    return CoalescedQuery { request: latest, replaced_queries };
                }
            }
        }
    }

    fn apply_index_command(&mut self, command: FileIndexMatcherCommand) {
        match command {
            FileIndexMatcherCommand::Reset {
                generation,
                index_version,
                entries,
                scan_finished,
            } => {
                self.reset(generation, index_version, entries, scan_finished);
            }
            FileIndexMatcherCommand::ScanBatch { generation, index_version, entries: batch } => {
                self.apply_scan_batch(generation, index_version, batch);
            }
            FileIndexMatcherCommand::ScanFinished { generation, index_version } => {
                self.apply_scan_finished(generation, index_version);
            }
            FileIndexMatcherCommand::FsBatch { generation, index_version, changes } => {
                self.apply_fs_batch(generation, index_version, changes);
            }
        }
    }

    fn apply_scan_batch(&mut self, generation: u64, index_version: u64, batch: Vec<FileCandidate>) {
        if generation != self.generation {
            self.log_ignored_index_command(
                "file index matcher ignored stale scan batch",
                generation,
                index_version,
                Some(batch.len()),
                None,
            );
            return;
        }

        let entries_before = self.matcher.len();
        let batch_len = batch.len();
        let upserts = self.matcher.upsert_scan_batch(batch);
        self.index_version = index_version;
        tracing::debug!(
            target: crate::logging::targets::APP_FILE_INDEX,
            event_name = "file_index_matcher_scan_batch_applied",
            message = "file index matcher applied scan batch",
            generation = self.generation,
            index_version = self.index_version,
            batch_entries = batch_len,
            inserted = upserts.inserted,
            replaced = upserts.replaced,
            entries_before,
            entries_after = self.matcher.len(),
            injected_items = self.matcher.injected_items(),
        );
    }

    fn apply_scan_finished(&mut self, generation: u64, index_version: u64) {
        if generation != self.generation {
            self.log_ignored_index_command(
                "file index matcher ignored stale scan finished",
                generation,
                index_version,
                None,
                None,
            );
            return;
        }

        self.scan_finished = true;
        self.index_version = index_version;
        tracing::debug!(
            target: crate::logging::targets::APP_FILE_INDEX,
            event_name = "file_index_matcher_scan_finished_applied",
            message = "file index matcher applied scan finished",
            generation = self.generation,
            index_version = self.index_version,
            entries = self.matcher.len(),
            injected_items = self.matcher.injected_items(),
        );
    }

    fn apply_fs_batch(
        &mut self,
        generation: u64,
        index_version: u64,
        changes: Vec<FileIndexChange>,
    ) {
        if generation != self.generation {
            self.log_ignored_index_command(
                "file index matcher ignored stale filesystem batch",
                generation,
                index_version,
                None,
                Some(changes.len()),
            );
            return;
        }

        let entries_before = self.matcher.len();
        let changes_len = changes.len();
        let change_stats = self.matcher.apply_change_batch(changes);
        self.index_version = index_version;
        tracing::debug!(
            target: crate::logging::targets::APP_FILE_INDEX,
            event_name = "file_index_matcher_fs_batch_applied",
            message = "file index matcher applied filesystem batch",
            generation = self.generation,
            index_version = self.index_version,
            changes = changes_len,
            inserted = change_stats.inserted,
            replaced = change_stats.replaced,
            removed = change_stats.removed,
            entries_before,
            entries_after = self.matcher.len(),
            injected_items = self.matcher.injected_items(),
        );
    }

    fn log_ignored_index_command(
        &self,
        message: &'static str,
        generation: u64,
        index_version: u64,
        batch_entries: Option<usize>,
        changes: Option<usize>,
    ) {
        tracing::debug!(
            target: crate::logging::targets::APP_FILE_INDEX,
            event_name = "file_index_matcher_index_command_ignored",
            message,
            matcher_generation = self.generation,
            command_generation = generation,
            index_version,
            batch_entries,
            changes,
        );
    }

    fn reset(
        &mut self,
        generation: u64,
        index_version: u64,
        entries: Vec<FileCandidate>,
        scan_finished: bool,
    ) {
        let entries_before = self.matcher.len();
        let incoming_entries = entries.len();
        self.generation = generation;
        self.index_version = index_version;
        let upserts = self.matcher.reset(entries);
        self.scan_finished = scan_finished;
        tracing::debug!(
            target: crate::logging::targets::APP_FILE_INDEX,
            event_name = "file_index_matcher_reset",
            message = "file index matcher state reset",
            generation = self.generation,
            index_version = self.index_version,
            entries_before,
            incoming_entries,
            entries = self.matcher.len(),
            injected_items = self.matcher.injected_items(),
            inserted = upserts.inserted,
            replaced = upserts.replaced,
            scan_finished = self.scan_finished,
        );
    }

    fn complete_query(&mut self, mut coalesced: CoalescedQuery) -> bool {
        let started_at = Instant::now();
        self.matcher.set_query(&coalesced.request.query);
        let mut tick_count = 0usize;
        let mut interrupted_queries = 0usize;
        let mut status = self.matcher.tick(0);
        while status.running {
            status = self.matcher.tick(NUCLEO_QUERY_TICK_MS);
            tick_count += 1;
            match self.query_rx.try_recv() {
                Ok(request) => {
                    coalesced.request = request;
                    coalesced.replaced_queries += 1;
                    interrupted_queries += 1;
                    self.matcher.set_query(&coalesced.request.query);
                    status = self.matcher.tick(0);
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => break,
            }
        }
        let request = coalesced.request;
        if request.generation != self.generation {
            tracing::debug!(
                target: crate::logging::targets::APP_FILE_INDEX,
                event_name = "file_index_matcher_query_rejected",
                message = "file index matcher rejected stale mention query after nucleo work",
                matcher_generation = self.generation,
                request_generation = request.generation,
                sequence = request.sequence,
                query_chars = request.query.chars().count(),
                replaced_queries = coalesced.replaced_queries,
            );
            return true;
        }
        let candidates = self.matcher.candidates(MAX_CANDIDATES);
        let duration_ms = elapsed_ms(started_at);
        tracing::debug!(
            target: crate::logging::targets::APP_FILE_INDEX,
            event_name = "file_index_matcher_query_finished",
            message = "file index matcher completed mention query",
            duration_ms,
            generation = self.generation,
            index_version = self.index_version,
            requested_index_version = request.index_version,
            sequence = request.sequence,
            entries = self.matcher.len(),
            injected_items = self.matcher.injected_items(),
            matched_items = self.matcher.matched_item_count(),
            query_chars = request.query.chars().count(),
            query_bytes = request.query.len(),
            result_count = candidates.len(),
            scan_finished = self.scan_finished,
            match_strategy = "nucleo",
            nucleo_ticks = tick_count,
            nucleo_running = status.running,
            interrupted_queries,
            replaced_queries = coalesced.replaced_queries,
        );
        let result = MentionMatchResult {
            generation: self.generation,
            index_version: self.index_version,
            sequence: request.sequence,
            query: request.query,
            candidates,
            scan_finished: self.scan_finished,
        };
        self.event_tx.send(FileIndexEvent::MatchResult(result)).is_ok()
    }
}

fn spawn_scan(
    root: PathBuf,
    generation: u64,
    respect_gitignore: bool,
    event_tx: Sender<FileIndexEvent>,
) -> FileIndexScanHandle {
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_clone = Arc::clone(&cancel);
    std::thread::spawn(move || {
        run_scan(&root, generation, respect_gitignore, &cancel_clone, &event_tx);
    });
    FileIndexScanHandle { cancel }
}

fn run_scan(
    root: &Path,
    generation: u64,
    respect_gitignore: bool,
    cancel: &Arc<AtomicBool>,
    event_tx: &Sender<FileIndexEvent>,
) {
    let started_at = Instant::now();
    tracing::info!(
        target: crate::logging::targets::APP_FILE_INDEX,
        event_name = "file_index_scan_started",
        message = "file index scan started",
        generation,
        root = %root.display(),
        respect_gitignore,
        batch_size = SCAN_BATCH_SIZE,
    );
    let mut batch = Vec::with_capacity(SCAN_BATCH_SIZE);
    let mut candidates_seen = 0usize;
    let mut batches_sent = 0usize;
    let mut emit_candidate = |candidate: FileCandidate| {
        candidates_seen += 1;
        batch.push(candidate);
        if batch.len() < SCAN_BATCH_SIZE {
            return true;
        }
        batches_sent += 1;
        if batches_sent == 1 || batches_sent % 64 == 0 {
            tracing::debug!(
                target: crate::logging::targets::APP_FILE_INDEX,
                event_name = "file_index_scan_progress",
                message = "file index scan emitted candidate batch",
                generation,
                candidates_seen,
                batches_sent,
                duration_ms = elapsed_ms(started_at),
            );
        }
        event_tx
            .send(FileIndexEvent::ScanBatch { generation, entries: std::mem::take(&mut batch) })
            .is_ok()
    };
    if !for_each_candidate(
        root,
        root,
        respect_gitignore,
        Some(1),
        Some(cancel),
        &mut emit_candidate,
    ) {
        tracing::info!(
            target: crate::logging::targets::APP_FILE_INDEX,
            event_name = "file_index_scan_cancelled",
            message = "file index shallow scan cancelled before completion",
            generation,
            candidates_seen,
            batches_sent,
            duration_ms = elapsed_ms(started_at),
        );
        return;
    }
    if !for_each_candidate(root, root, respect_gitignore, None, Some(cancel), &mut emit_candidate) {
        tracing::info!(
            target: crate::logging::targets::APP_FILE_INDEX,
            event_name = "file_index_scan_cancelled",
            message = "file index scan cancelled before completion",
            generation,
            candidates_seen,
            batches_sent,
            duration_ms = elapsed_ms(started_at),
        );
        return;
    }
    let final_batch_len = batch.len();
    if !batch.is_empty() {
        batches_sent += 1;
        if event_tx.send(FileIndexEvent::ScanBatch { generation, entries: batch }).is_err() {
            tracing::info!(
                target: crate::logging::targets::APP_FILE_INDEX,
                event_name = "file_index_scan_abandoned",
                message = "file index scan receiver dropped before final batch",
                generation,
                candidates_seen,
                batches_sent,
                final_batch_len,
                duration_ms = elapsed_ms(started_at),
            );
            return;
        }
    }
    let _ = event_tx.send(FileIndexEvent::ScanFinished { generation });
    tracing::info!(
        target: crate::logging::targets::APP_FILE_INDEX,
        event_name = "file_index_scan_finished",
        message = "file index scan finished",
        generation,
        candidates_seen,
        batches_sent,
        final_batch_len,
        duration_ms = elapsed_ms(started_at),
    );
}

fn spawn_watch(
    root: &Path,
    generation: u64,
    respect_gitignore: bool,
    event_tx: Sender<FileIndexEvent>,
) -> FileIndexWatchHandle {
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_clone = Arc::clone(&cancel);
    let root_for_thread = root.to_path_buf();
    std::thread::spawn(move || {
        tracing::info!(
            target: crate::logging::targets::APP_FILE_INDEX,
            event_name = "file_index_watch_started",
            message = "file index watcher thread started",
            generation,
            root = %root_for_thread.display(),
            respect_gitignore,
            debounce_ms = WATCH_DEBOUNCE_INTERVAL.as_millis(),
        );
        let (watch_tx, watch_rx) = mpsc::channel();
        let mut watcher = match notify::recommended_watcher(move |result| {
            let _ = watch_tx.send(result);
        }) {
            Ok(watcher) => watcher,
            Err(err) => {
                tracing::warn!(
                    target: crate::logging::targets::APP_FILE_INDEX,
                    event_name = "file_index_watch_setup_failed",
                    message = "file index watcher setup failed",
                    generation,
                    root = %root_for_thread.display(),
                    error_message = %err,
                );
                return;
            }
        };
        if let Err(err) =
            notify::Watcher::watch(&mut watcher, &root_for_thread, notify::RecursiveMode::Recursive)
        {
            tracing::warn!(
                target: crate::logging::targets::APP_FILE_INDEX,
                event_name = "file_index_watch_start_failed",
                message = "file index watcher start failed",
                generation,
                root = %root_for_thread.display(),
                error_message = %err,
            );
            return;
        }

        let mut pending = Vec::new();
        while !cancel_clone.load(AtomicOrdering::Relaxed) {
            let timeout =
                if pending.is_empty() { WATCH_POLL_INTERVAL } else { WATCH_DEBOUNCE_INTERVAL };
            match watch_rx.recv_timeout(timeout) {
                Ok(event) => pending.push(event),
                Err(RecvTimeoutError::Timeout) if pending.is_empty() => {}
                Err(RecvTimeoutError::Timeout) => {
                    if let Some(next_event) = normalize_watch_events(
                        &root_for_thread,
                        generation,
                        respect_gitignore,
                        pending.drain(..),
                    ) {
                        let _ = event_tx.send(next_event);
                    }
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        if let Some(next_event) = normalize_watch_events(
            &root_for_thread,
            generation,
            respect_gitignore,
            pending.drain(..),
        ) {
            let _ = event_tx.send(next_event);
        }
    });
    FileIndexWatchHandle { cancel }
}

fn normalize_watch_events(
    root: &Path,
    generation: u64,
    respect_gitignore: bool,
    events: impl IntoIterator<Item = notify::Result<notify::Event>>,
) -> Option<FileIndexEvent> {
    use notify::event::{CreateKind, EventKind, ModifyKind, RemoveKind, RenameMode};

    let mut rebuild = false;
    let mut create_or_modify_paths = Vec::new();
    let mut rename_paths = Vec::new();
    let mut remove_paths = Vec::new();
    let mut raw_events = 0usize;

    for event in events {
        raw_events += 1;
        let event = match event {
            Ok(event) => event,
            Err(err) => {
                tracing::warn!(
                    target: crate::logging::targets::APP_FILE_INDEX,
                    event_name = "file_index_watch_event_failed",
                    message = "file index watcher event failed",
                    generation,
                    error_message = %err,
                );
                rebuild = true;
                continue;
            }
        };

        if matches_ignore_semantics_change(root, &event.paths) {
            rebuild = true;
            continue;
        }

        match event.kind {
            EventKind::Modify(ModifyKind::Name(RenameMode::Any | RenameMode::Both)) => {
                extend_unique_paths(&mut rename_paths, event.paths);
            }
            EventKind::Create(CreateKind::Any | CreateKind::File | CreateKind::Folder)
            | EventKind::Modify(
                ModifyKind::Any
                | ModifyKind::Data(_)
                | ModifyKind::Metadata(_)
                | ModifyKind::Name(RenameMode::To),
            ) => {
                extend_unique_paths(&mut create_or_modify_paths, event.paths);
            }
            EventKind::Modify(ModifyKind::Name(RenameMode::From))
            | EventKind::Remove(RemoveKind::Any | RemoveKind::File | RemoveKind::Folder) => {
                extend_unique_paths(&mut remove_paths, event.paths);
            }
            EventKind::Other => {
                rebuild = true;
            }
            _ => {}
        }
    }

    if rebuild {
        tracing::debug!(
            target: crate::logging::targets::APP_FILE_INDEX,
            event_name = "file_index_watch_batch_rebuild",
            message = "file index watcher batch requested rebuild",
            generation,
            raw_events,
            create_or_modify_paths = create_or_modify_paths.len(),
            rename_paths = rename_paths.len(),
            remove_paths = remove_paths.len(),
        );
        return Some(FileIndexEvent::RebuildRequested { generation });
    }

    let mut changes = Vec::new();
    changes.extend(collect_rename_changes(root, respect_gitignore, &rename_paths));
    changes.extend(collect_create_or_modify_changes(
        root,
        respect_gitignore,
        &create_or_modify_paths,
    ));
    changes.extend(collect_remove_changes(root, &remove_paths));
    if raw_events > 0 {
        tracing::debug!(
            target: crate::logging::targets::APP_FILE_INDEX,
            event_name = "file_index_watch_batch_normalized",
            message = "file index watcher batch normalized",
            generation,
            raw_events,
            create_or_modify_paths = create_or_modify_paths.len(),
            rename_paths = rename_paths.len(),
            remove_paths = remove_paths.len(),
            changes = changes.len(),
        );
    }

    (!changes.is_empty()).then_some(FileIndexEvent::FsBatch { generation, changes })
}

fn extend_unique_paths(paths: &mut Vec<PathBuf>, next_paths: impl IntoIterator<Item = PathBuf>) {
    for path in next_paths {
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
}

fn matches_ignore_semantics_change(root: &Path, paths: &[PathBuf]) -> bool {
    paths.iter().any(|path| {
        let Some(rel) = normalize_relative_path(root, path) else {
            return false;
        };
        rel == ".gitignore"
            || rel == ".ignore"
            || rel.ends_with("/.gitignore")
            || rel.ends_with("/.ignore")
    }) || paths.iter().any(|path| {
        path.file_name().is_some_and(|name| name == "exclude")
            && path.parent().and_then(Path::file_name).is_some_and(|name| name == "info")
            && path
                .parent()
                .and_then(Path::parent)
                .and_then(Path::file_name)
                .is_some_and(|name| name == ".git")
    })
}

fn collect_create_or_modify_changes(
    root: &Path,
    respect_gitignore: bool,
    paths: &[PathBuf],
) -> Vec<FileIndexChange> {
    let mut changes = Vec::new();
    for path in paths {
        if path.is_dir() {
            if let Some(change) = replace_subtree_change(root, path, respect_gitignore) {
                changes.push(change);
            }
        } else if path.is_file() {
            let mut entries = scan_subtree(root, path, respect_gitignore);
            if let Some(candidate) = entries.pop() {
                changes.push(FileIndexChange::Upsert(candidate));
            } else if let Some(rel_path) = normalize_relative_path(root, path) {
                changes.push(FileIndexChange::RemoveExact { rel_path });
            }
        }
    }
    changes
}

fn collect_remove_changes(root: &Path, paths: &[PathBuf]) -> Vec<FileIndexChange> {
    let mut changes = Vec::new();
    for path in paths {
        let Some(rel_path) = normalize_relative_path(root, path) else {
            continue;
        };
        changes.push(FileIndexChange::RemoveExact { rel_path: rel_path.clone() });
        changes.push(FileIndexChange::RemovePrefix { rel_prefix: ensure_dir_suffix(rel_path) });
    }
    changes
}

fn collect_rename_changes(
    root: &Path,
    respect_gitignore: bool,
    paths: &[PathBuf],
) -> Vec<FileIndexChange> {
    if paths.len() < 2 {
        // macOS FSEvents emits two separate RenameMode::Any events (one per
        // path) instead of a single paired event. If the path no longer exists
        // it is the "from" side of the rename and should be treated as a remove.
        if paths.first().is_some_and(|p| !p.exists()) {
            return collect_remove_changes(root, paths);
        }
        return collect_parent_rescan_changes(root, respect_gitignore, paths);
    }
    collect_parent_rescan_changes(root, respect_gitignore, paths)
}

fn scan_subtree(root: &Path, path: &Path, respect_gitignore: bool) -> Vec<FileCandidate> {
    collect_candidates(root, path, respect_gitignore, None)
}

fn collect_parent_rescan_changes(
    root: &Path,
    respect_gitignore: bool,
    paths: &[PathBuf],
) -> Vec<FileIndexChange> {
    let mut changes = Vec::new();
    let mut seen_prefixes = BTreeSet::new();
    for path in paths {
        let Some(parent) = path.parent() else { continue };
        let Some(change) = replace_subtree_change(root, parent, respect_gitignore) else {
            continue;
        };
        let FileIndexChange::ReplacePrefix { rel_prefix, .. } = &change else {
            continue;
        };
        if seen_prefixes.insert(rel_prefix.clone()) {
            changes.push(change);
        }
    }
    changes
}

fn replace_subtree_change(
    root: &Path,
    path: &Path,
    respect_gitignore: bool,
) -> Option<FileIndexChange> {
    let rel_prefix = if path == root { String::new() } else { normalized_prefix(root, path)? };
    let entries = scan_subtree(root, path, respect_gitignore);
    Some(FileIndexChange::ReplacePrefix { rel_prefix, entries })
}

fn for_each_candidate(
    root: &Path,
    walk_root: &Path,
    respect_gitignore: bool,
    max_depth: Option<usize>,
    cancel: Option<&Arc<AtomicBool>>,
    emit: &mut impl FnMut(FileCandidate) -> bool,
) -> bool {
    let mut builder = ignore::WalkBuilder::new(walk_root);
    builder
        .hidden(false)
        .git_ignore(respect_gitignore)
        .git_global(respect_gitignore)
        .git_exclude(respect_gitignore)
        .sort_by_file_path(std::cmp::Ord::cmp);
    builder.max_depth(max_depth);

    for result in builder.build() {
        if cancel.is_some_and(|flag| flag.load(AtomicOrdering::Relaxed)) {
            return false;
        }
        let Ok(entry) = result else { continue };
        let Some(candidate) = candidate_from_entry(root, &entry) else { continue };
        if !emit(candidate) {
            return false;
        }
    }

    true
}

fn collect_candidates(
    root: &Path,
    walk_root: &Path,
    respect_gitignore: bool,
    cancel: Option<&Arc<AtomicBool>>,
) -> Vec<FileCandidate> {
    let mut candidates = Vec::new();
    let _ =
        for_each_candidate(root, walk_root, respect_gitignore, None, cancel, &mut |candidate| {
            candidates.push(candidate);
            true
        });
    candidates
}

fn candidate_from_entry(root: &Path, entry: &ignore::DirEntry) -> Option<FileCandidate> {
    let ft = entry.file_type()?;
    let is_dir = ft.is_dir();
    let is_file = ft.is_file();
    if !is_dir && !is_file {
        return None;
    }

    let path = entry.path();
    let rel = path.strip_prefix(root).ok()?;
    let rel_str = rel.to_string_lossy().replace('\\', "/");
    if rel_str.is_empty() {
        return None;
    }

    let depth = rel_str.matches('/').count();
    let rel_path = if is_dir { format!("{rel_str}/") } else { rel_str };
    let modified = entry
        .metadata()
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let rel_path_lower = rel_path.to_lowercase();
    let basename_lower = candidate_basename(&rel_path).to_lowercase();

    Some(FileCandidate { rel_path, rel_path_lower, basename_lower, depth, modified, is_dir })
}

fn normalize_relative_path(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let rel_str = rel.to_string_lossy().replace('\\', "/");
    (!rel_str.is_empty()).then_some(rel_str)
}

fn normalized_prefix(root: &Path, path: &Path) -> Option<String> {
    normalize_relative_path(root, path).map(ensure_dir_suffix)
}

fn ensure_dir_suffix(mut rel_path: String) -> String {
    if !rel_path.ends_with('/') {
        rel_path.push('/');
    }
    rel_path
}

fn candidate_basename(rel_path: &str) -> &str {
    let trimmed = rel_path.trim_end_matches('/');
    trimmed.rsplit('/').next().unwrap_or(trimmed)
}

impl ScanOverrides {
    fn record_change(&mut self, change: &FileIndexChange) {
        match change {
            FileIndexChange::Upsert(candidate) => {
                self.exact_paths.insert(candidate.rel_path.clone());
            }
            FileIndexChange::RemoveExact { rel_path } => {
                self.exact_paths.insert(rel_path.clone());
            }
            FileIndexChange::RemovePrefix { rel_prefix }
            | FileIndexChange::ReplacePrefix { rel_prefix, .. } => {
                self.blocked_prefixes.push(rel_prefix.clone());
            }
        }
    }

    fn blocks(&self, rel_path: &str) -> bool {
        self.exact_paths.contains(rel_path)
            || self.blocked_prefixes.iter().any(|prefix| rel_path.starts_with(prefix))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, mention};
    use std::time::{Duration, Instant};

    fn app_with_temp_files(files: &[&str]) -> (App, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        // Canonicalize so the watcher root matches the paths reported by FSEvents
        // on macOS (where /tmp is a symlink to /private/tmp).
        let canonical = tmp.path().canonicalize().expect("canonicalize tempdir");
        for file in files {
            let path = canonical.join(file);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("create parent");
            }
            std::fs::write(&path, "").expect("write file");
        }
        let mut app = App::test_default();
        app.cwd_raw = canonical.to_string_lossy().into_owned();
        (app, tmp)
    }

    fn wait_for(app: &mut App, timeout: Duration, mut predicate: impl FnMut(&App) -> bool) {
        let start = Instant::now();
        while start.elapsed() < timeout {
            drain_events(app);
            if predicate(app) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        drain_events(app);
        assert!(predicate(app), "condition not met before timeout");
    }

    fn candidate(rel_path: &str) -> FileCandidate {
        FileCandidate {
            rel_path: rel_path.to_owned(),
            rel_path_lower: rel_path.to_lowercase(),
            basename_lower: candidate_basename(rel_path).to_lowercase(),
            depth: rel_path.matches('/').count(),
            modified: SystemTime::UNIX_EPOCH,
            is_dir: rel_path.ends_with('/'),
        }
    }

    #[test]
    fn reopening_mention_reuses_existing_generation() {
        let (mut app, _tmp) = app_with_temp_files(&["src/main.rs"]);
        app.input.set_text("@rs");
        let _ = app.input.set_cursor(0, 3);

        mention::activate(&mut app);
        wait_for(&mut app, Duration::from_secs(2), |app| {
            app.file_index.scan_finished && !app.file_index.entries.is_empty()
        });
        let generation = app.file_index.generation;

        mention::deactivate(&mut app);
        app.input.set_text("@src");
        let _ = app.input.set_cursor(0, 4);
        mention::activate(&mut app);

        assert_eq!(app.file_index.generation, generation);
    }

    #[test]
    fn stale_scan_event_is_ignored_after_reset() {
        let mut app = App::test_default();
        let stale_generation = app.file_index.generation;
        reset(&mut app);
        app.file_index_event_tx
            .send(FileIndexEvent::ScanBatch {
                generation: stale_generation,
                entries: vec![FileCandidate {
                    rel_path: "stale.rs".to_owned(),
                    rel_path_lower: "stale.rs".to_owned(),
                    basename_lower: "stale.rs".to_owned(),
                    depth: 0,
                    modified: SystemTime::UNIX_EPOCH,
                    is_dir: false,
                }],
            })
            .expect("send stale scan batch");

        drain_events(&mut app);

        assert!(app.file_index.entries.is_empty());
    }

    #[test]
    fn live_remove_blocks_late_scan_entry_from_same_generation() {
        let mut app = App::test_default();
        app.file_index.generation = 7;
        app.file_index.scan_finished = false;

        app.file_index_event_tx
            .send(FileIndexEvent::FsBatch {
                generation: 7,
                changes: vec![FileIndexChange::RemoveExact { rel_path: "stale.rs".to_owned() }],
            })
            .expect("send live remove");
        app.file_index_event_tx
            .send(FileIndexEvent::ScanBatch {
                generation: 7,
                entries: vec![FileCandidate {
                    rel_path: "stale.rs".to_owned(),
                    rel_path_lower: "stale.rs".to_owned(),
                    basename_lower: "stale.rs".to_owned(),
                    depth: 0,
                    modified: SystemTime::UNIX_EPOCH,
                    is_dir: false,
                }],
            })
            .expect("send stale scan batch");

        drain_events(&mut app);

        assert!(!app.file_index.entries.contains_key("stale.rs"));
    }

    #[test]
    fn live_upsert_beats_late_scan_entry_for_same_path() {
        let mut app = App::test_default();
        app.file_index.generation = 11;
        app.file_index.scan_finished = false;

        app.file_index_event_tx
            .send(FileIndexEvent::FsBatch {
                generation: 11,
                changes: vec![FileIndexChange::Upsert(FileCandidate {
                    rel_path: "fresh.rs".to_owned(),
                    rel_path_lower: "fresh.rs".to_owned(),
                    basename_lower: "fresh.rs".to_owned(),
                    depth: 0,
                    modified: SystemTime::UNIX_EPOCH + Duration::from_secs(20),
                    is_dir: false,
                })],
            })
            .expect("send live upsert");
        app.file_index_event_tx
            .send(FileIndexEvent::ScanBatch {
                generation: 11,
                entries: vec![FileCandidate {
                    rel_path: "fresh.rs".to_owned(),
                    rel_path_lower: "fresh.rs".to_owned(),
                    basename_lower: "fresh.rs".to_owned(),
                    depth: 0,
                    modified: SystemTime::UNIX_EPOCH + Duration::from_secs(1),
                    is_dir: false,
                }],
            })
            .expect("send stale scan batch");

        drain_events(&mut app);

        let candidate = app.file_index.entries.get("fresh.rs").expect("fresh candidate");
        assert_eq!(candidate.modified, SystemTime::UNIX_EPOCH + Duration::from_secs(20));
    }

    #[test]
    fn matcher_entries_repair_lookup_after_swap_remove() {
        let mut entries = MatcherEntries::default();
        entries.upsert(candidate("a.rs"));
        entries.upsert(candidate("b.rs"));
        entries.upsert(candidate("c.rs"));

        assert!(entries.remove_exact("b.rs"));
        entries.upsert(candidate("c.rs"));
        assert!(entries.remove_exact("c.rs"));

        let remaining =
            entries.iter().map(|candidate| candidate.rel_path.as_str()).collect::<Vec<_>>();
        assert_eq!(remaining, vec!["a.rs"]);
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn matcher_entries_replace_prefix_keeps_lookup_consistent() {
        let mut entries = MatcherEntries::default();
        entries.upsert(candidate("src/a.rs"));
        entries.upsert(candidate("src/nested/b.rs"));
        entries.upsert(candidate("tests/c.rs"));

        let (removed, stats) =
            entries.replace_prefix("src/", vec![candidate("src/new.rs"), candidate("src/lib.rs")]);

        assert_eq!(removed, 2);
        assert_eq!(stats.inserted, 2);
        assert_eq!(stats.replaced, 0);
        assert!(!entries.remove_exact("src/a.rs"));
        assert!(entries.remove_exact("src/new.rs"));
        assert!(entries.remove_exact("src/lib.rs"));
        assert!(entries.remove_exact("tests/c.rs"));
        assert_eq!(entries.len(), 0);
    }

    fn mention_matcher_paths(matcher: &mut MentionMatcher, query: &str) -> Vec<String> {
        matcher.set_query(query);
        while matcher.tick(NUCLEO_QUERY_TICK_MS).running {}
        matcher.candidates(MAX_CANDIDATES).into_iter().map(|candidate| candidate.rel_path).collect()
    }

    #[test]
    fn mention_matcher_streams_scan_batches_into_nucleo() {
        let mut matcher = MentionMatcher::default();
        let batch =
            (0..75).map(|idx| candidate(&format!("src/file-{idx:03}.rs"))).collect::<Vec<_>>();

        let stats = matcher.upsert_scan_batch(batch);
        let paths = mention_matcher_paths(&mut matcher, "file");

        assert_eq!(stats.inserted, 75);
        assert_eq!(matcher.injected_items(), 75);
        assert_eq!(paths.len(), MAX_CANDIDATES);
        assert!(paths.iter().all(|path| path.contains("file")));
    }

    #[test]
    fn mention_matcher_finds_root_level_directory() {
        let mut matcher = MentionMatcher::default();
        matcher.upsert_scan_batch(vec![
            candidate("aaa_huge/file-000.rs"),
            candidate("web_dev_work/"),
            candidate("src/lib.rs"),
        ]);

        let paths = mention_matcher_paths(&mut matcher, "web_dev_work");

        assert!(paths.iter().any(|path| path == "web_dev_work/"));
    }

    #[test]
    fn mention_matcher_rebuilds_after_filesystem_removal() {
        let mut matcher = MentionMatcher::default();
        matcher.reset(vec![candidate("old.rs"), candidate("src/current.rs")]);

        assert!(mention_matcher_paths(&mut matcher, "old").iter().any(|path| path == "old.rs"));

        let stats = matcher.apply_change_batch(vec![FileIndexChange::RemoveExact {
            rel_path: "old.rs".to_owned(),
        }]);
        let paths = mention_matcher_paths(&mut matcher, "old");

        assert_eq!(stats.removed, 1);
        assert!(!paths.iter().any(|path| path == "old.rs"));
    }

    #[test]
    fn matcher_runtime_prioritizes_query_over_pending_scan_batches() {
        let (index_tx, index_rx) = mpsc::channel();
        let (query_tx, query_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let mut runtime = FileIndexMatcherRuntime::new(1, index_rx, query_rx, event_tx);
        runtime.reset(1, 1, vec![candidate("target.rs")], false);
        for idx in 0..32 {
            index_tx
                .send(FileIndexMatcherCommand::ScanBatch {
                    generation: 1,
                    index_version: 2 + idx,
                    entries: vec![candidate(&format!("bulk/file-{idx:03}.rs"))],
                })
                .expect("queue scan batch");
        }
        query_tx
            .send(MentionMatchRequest {
                generation: 1,
                index_version: 1,
                sequence: 7,
                query: "target".to_owned(),
            })
            .expect("queue query");

        assert!(runtime.handle_next_command());

        let event = event_rx.recv_timeout(Duration::from_secs(2)).expect("match result");
        let FileIndexEvent::MatchResult(result) = event else {
            panic!("expected match result");
        };
        assert_eq!(result.sequence, 7);
        assert!(result.candidates.iter().any(|candidate| candidate.rel_path == "target.rs"));
        assert_eq!(runtime.matcher.len(), 1);
    }

    #[test]
    fn spawn_scan_streams_batches_before_finished_event() {
        let tmp = tempfile::tempdir().expect("tempdir");
        for idx in 0..300 {
            let path = tmp.path().join("src").join(format!("file-{idx}.rs"));
            std::fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
            std::fs::write(path, "").expect("write file");
        }
        let (tx, rx) = mpsc::channel();
        let _scan = spawn_scan(tmp.path().to_path_buf(), 1, true, tx);

        let first = rx.recv_timeout(Duration::from_secs(2)).expect("first scan event");
        assert!(matches!(first, FileIndexEvent::ScanBatch { .. }));
    }

    #[test]
    fn spawn_scan_emits_root_children_before_deep_subtrees() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(tmp.path().join("aaa_huge")).expect("create huge sibling");
        std::fs::create_dir_all(tmp.path().join("web_dev_work")).expect("create target sibling");
        for idx in 0..300 {
            let path = tmp.path().join("aaa_huge").join(format!("file-{idx}.rs"));
            std::fs::write(path, "").expect("write file");
        }
        let (tx, rx) = mpsc::channel();
        let _scan = spawn_scan(tmp.path().to_path_buf(), 1, true, tx);

        let first = rx.recv_timeout(Duration::from_secs(2)).expect("first scan event");
        let FileIndexEvent::ScanBatch { entries, .. } = first else {
            panic!("expected first scan batch");
        };

        assert!(entries.iter().any(|candidate| candidate.rel_path == "web_dev_work/"));
    }

    #[test]
    fn fs_batch_create_updates_matcher_candidates_without_real_watcher() {
        let mut app = App::test_default();
        app.file_index.generation = 5;
        app.file_index.scan_finished = true;
        app.file_index.entries.insert("existing.rs".to_owned(), candidate("existing.rs"));
        app.mention = Some(mention::MentionState::new(0, 0, "new".to_owned(), Vec::new()));

        app.file_index_event_tx
            .send(FileIndexEvent::FsBatch {
                generation: 5,
                changes: vec![FileIndexChange::Upsert(candidate("new.rs"))],
            })
            .expect("send fs batch");

        drain_events(&mut app);

        assert!(app.surface_dirty.chat.repaint);
        wait_for(&mut app, Duration::from_secs(1), |app| {
            app.mention.as_ref().is_some_and(|mention| {
                mention.candidates.iter().any(|candidate| candidate.rel_path == "new.rs")
            })
        });
        let mention = app.mention.as_ref().expect("mention");
        assert_eq!(
            mention
                .candidates
                .iter()
                .map(|candidate| candidate.rel_path.as_str())
                .collect::<Vec<_>>(),
            vec!["new.rs"]
        );
    }

    #[test]
    fn fs_batch_rename_replaces_old_path_without_real_watcher() {
        let (mut app, tmp) = app_with_temp_files(&["before.rs", "keep.rs"]);
        let root = tmp.path().canonicalize().expect("canonicalize tempdir");
        app.file_index.generation = 9;
        app.file_index.scan_finished = true;
        app.file_index.root = Some(root.clone());
        app.file_index.entries.insert("before.rs".to_owned(), candidate("before.rs"));
        app.file_index.entries.insert("keep.rs".to_owned(), candidate("keep.rs"));
        app.mention = Some(mention::MentionState::new(0, 0, "rs".to_owned(), Vec::new()));

        std::fs::rename(root.join("before.rs"), root.join("after.rs"))
            .expect("rename watched file");
        let changes =
            collect_rename_changes(&root, true, &[root.join("before.rs"), root.join("after.rs")]);
        app.file_index_event_tx
            .send(FileIndexEvent::FsBatch { generation: 9, changes })
            .expect("send rename fs batch");

        drain_events(&mut app);

        assert!(!app.file_index.entries.contains_key("before.rs"));
        assert!(app.file_index.entries.contains_key("after.rs"));
        assert!(app.file_index.entries.contains_key("keep.rs"));
        wait_for(&mut app, Duration::from_secs(1), |app| {
            app.mention.as_ref().is_some_and(|mention| {
                mention.candidates.iter().any(|candidate| candidate.rel_path == "after.rs")
            })
        });
        let mention = app.mention.as_ref().expect("mention");
        let visible = mention
            .candidates
            .iter()
            .map(|candidate| candidate.rel_path.as_str())
            .collect::<Vec<_>>();
        assert!(visible.contains(&"after.rs"));
        assert!(visible.contains(&"keep.rs"));
        assert!(!visible.contains(&"before.rs"));
    }

    #[test]
    fn batched_watch_events_emit_single_fs_batch() {
        use notify::Event;
        use notify::event::{CreateKind, EventKind};

        let (_app, tmp) = app_with_temp_files(&["a.rs", "b.rs"]);
        let root = tmp.path().canonicalize().expect("canonicalize tempdir");
        let events = vec![
            Ok(Event::new(EventKind::Create(CreateKind::File)).add_path(root.join("a.rs"))),
            Ok(Event::new(EventKind::Create(CreateKind::File)).add_path(root.join("b.rs"))),
        ];

        let event = normalize_watch_events(&root, 3, true, events).expect("watch event");

        let FileIndexEvent::FsBatch { generation, changes } = event else {
            panic!("expected fs batch");
        };
        assert_eq!(generation, 3);
        assert_eq!(changes.len(), 2);
        assert!(changes.iter().any(|change| {
            matches!(change, FileIndexChange::Upsert(candidate) if candidate.rel_path == "a.rs")
        }));
        assert!(changes.iter().any(|change| {
            matches!(change, FileIndexChange::Upsert(candidate) if candidate.rel_path == "b.rs")
        }));
    }

    #[test]
    fn batched_watch_events_prefer_rebuild_for_ignore_semantics_change() {
        use notify::Event;
        use notify::event::{CreateKind, EventKind};

        let (_app, tmp) = app_with_temp_files(&["a.rs"]);
        let root = tmp.path().canonicalize().expect("canonicalize tempdir");
        let events = vec![
            Ok(Event::new(EventKind::Create(CreateKind::File)).add_path(root.join("a.rs"))),
            Ok(Event::new(EventKind::Create(CreateKind::File)).add_path(root.join(".gitignore"))),
        ];

        let event = normalize_watch_events(&root, 5, true, events).expect("watch event");

        assert!(matches!(event, FileIndexEvent::RebuildRequested { generation: 5 }));
    }

    #[test]
    fn root_file_rename_rescans_root_subtree() {
        let (_app, tmp) = app_with_temp_files(&["before.rs", "keep.rs"]);
        let root = tmp.path().canonicalize().expect("canonicalize tempdir");
        std::fs::rename(root.join("before.rs"), root.join("after.rs"))
            .expect("rename watched file");

        let changes =
            collect_rename_changes(&root, true, &[root.join("before.rs"), root.join("after.rs")]);

        assert_eq!(changes.len(), 1);
        let FileIndexChange::ReplacePrefix { rel_prefix, entries } = &changes[0] else {
            panic!("expected replace prefix");
        };
        assert_eq!(rel_prefix, "");
        assert!(entries.iter().any(|candidate| candidate.rel_path == "after.rs"));
        assert!(entries.iter().any(|candidate| candidate.rel_path == "keep.rs"));
        assert!(!entries.iter().any(|candidate| candidate.rel_path == "before.rs"));
    }
}
