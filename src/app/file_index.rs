// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::App;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender, TryRecvError};
use std::time::{Duration, SystemTime};

const SCAN_BATCH_SIZE: usize = 256;
const EVENT_DRAIN_BUDGET: usize = 64;
const WATCH_POLL_INTERVAL: Duration = Duration::from_millis(100);
const MAX_CANDIDATES: usize = 50;

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
    pub entries: BTreeMap<String, FileCandidate>,
    pub scan_finished: bool,
    pub rebuild_pending: bool,
    scan_overrides: ScanOverrides,
    pub scan: Option<FileIndexScanHandle>,
    pub watch: Option<FileIndexWatchHandle>,
}

pub struct FileIndexScanHandle {
    cancel: Arc<AtomicBool>,
}

pub struct FileIndexWatchHandle {
    cancel: Arc<AtomicBool>,
}

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
    app.file_index.root = None;
    app.file_index.respect_gitignore = app.config.respect_gitignore_effective();
    app.file_index.entries.clear();
    app.file_index.scan_finished = false;
    app.file_index.rebuild_pending = false;
    app.file_index.scan_overrides = ScanOverrides::default();
    app.file_index.scan = None;
    app.file_index.watch = None;
}

pub fn restart(app: &mut App) {
    reset(app);
    let root = PathBuf::from(&app.cwd_raw);
    let generation = app.file_index.generation;
    let respect_gitignore = app.config.respect_gitignore_effective();
    app.file_index.root = Some(root.clone());
    app.file_index.respect_gitignore = respect_gitignore;
    app.file_index.scan_finished = false;
    app.file_index.rebuild_pending = false;
    app.file_index.scan_overrides = ScanOverrides::default();
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
    let needs_restart = app.file_index.root.as_ref() != Some(&current_root)
        || app.file_index.respect_gitignore != respect_gitignore
        || (app.file_index.root.is_none())
        || (!app.file_index.scan_finished && app.file_index.scan.is_none());
    if needs_restart {
        restart(app);
    }
}

pub fn drain_events(app: &mut App) {
    let mut handled = 0;
    loop {
        if handled >= EVENT_DRAIN_BUDGET {
            break;
        }
        let event = match app.file_index_event_rx.try_recv() {
            Ok(event) => event,
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
        };
        apply_event(app, event);
        handled += 1;
    }
}

pub fn visible_candidates(
    entries: &BTreeMap<String, FileCandidate>,
    query: &str,
) -> Vec<FileCandidate> {
    let query_lower = query.to_lowercase();
    let mut filtered: Vec<FileCandidate> = entries
        .values()
        .filter(|candidate| match_tier(candidate, &query_lower).is_some())
        .cloned()
        .collect();
    rank_and_truncate_candidates(&mut filtered, &query_lower);
    filtered
}

pub fn rank_and_truncate_candidates(candidates: &mut Vec<FileCandidate>, query_lower: &str) {
    let tiers: Vec<Option<u8>> = candidates.iter().map(|c| match_tier(c, query_lower)).collect();
    let mut indices: Vec<usize> = (0..candidates.len()).collect();
    indices.sort_by(|&i, &j| {
        tiers[i]
            .cmp(&tiers[j])
            .then_with(|| candidates[i].depth.cmp(&candidates[j].depth))
            .then_with(|| candidates[i].rel_path.cmp(&candidates[j].rel_path))
    });

    indices.truncate(MAX_CANDIDATES);
    *candidates = indices.into_iter().map(|i| candidates[i].clone()).collect();
}

fn match_tier(candidate: &FileCandidate, query_lower: &str) -> Option<u8> {
    if query_lower.is_empty() {
        return Some(0);
    }

    if candidate.basename_lower.starts_with(query_lower) {
        Some(0)
    } else if candidate.rel_path_lower.starts_with(query_lower) {
        Some(1)
    } else if candidate.basename_lower.contains(query_lower) {
        Some(2)
    } else if candidate.rel_path_lower.contains(query_lower) {
        Some(3)
    } else {
        None
    }
}

fn apply_event(app: &mut App, event: FileIndexEvent) {
    match event {
        FileIndexEvent::ScanBatch { generation, entries } => {
            if generation != app.file_index.generation {
                return;
            }
            for entry in entries {
                if app.file_index.scan_overrides.blocks(&entry.rel_path) {
                    continue;
                }
                app.file_index.entries.insert(entry.rel_path.clone(), entry);
            }
            refresh_after_mutation(app);
        }
        FileIndexEvent::ScanFinished { generation } => {
            if generation != app.file_index.generation {
                return;
            }
            app.file_index.scan_finished = true;
            app.file_index.scan_overrides = ScanOverrides::default();
            app.file_index.scan = None;
            refresh_after_mutation(app);
        }
        FileIndexEvent::FsBatch { generation, changes } => {
            if generation != app.file_index.generation {
                return;
            }
            for change in changes {
                if !app.file_index.scan_finished {
                    app.file_index.scan_overrides.record_change(&change);
                }
                apply_change(&mut app.file_index.entries, change);
            }
            refresh_after_mutation(app);
        }
        FileIndexEvent::RebuildRequested { generation } => {
            if generation != app.file_index.generation {
                return;
            }
            restart(app);
            refresh_after_mutation(app);
        }
    }
}

fn refresh_after_mutation(app: &mut App) {
    if app.mention.is_some() {
        super::mention::refresh_from_file_index(app);
    }
    app.needs_redraw = true;
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

fn spawn_scan(
    root: PathBuf,
    generation: u64,
    respect_gitignore: bool,
    event_tx: Sender<FileIndexEvent>,
) -> FileIndexScanHandle {
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_clone = Arc::clone(&cancel);
    std::thread::spawn(move || {
        let mut batch = Vec::with_capacity(SCAN_BATCH_SIZE);
        let mut emit_candidate = |candidate| {
            batch.push(candidate);
            if batch.len() < SCAN_BATCH_SIZE {
                return true;
            }
            event_tx
                .send(FileIndexEvent::ScanBatch { generation, entries: std::mem::take(&mut batch) })
                .is_ok()
        };
        if !for_each_candidate(
            &root,
            &root,
            respect_gitignore,
            Some(&cancel_clone),
            &mut emit_candidate,
        ) {
            return;
        }
        if !batch.is_empty()
            && event_tx.send(FileIndexEvent::ScanBatch { generation, entries: batch }).is_err()
        {
            return;
        }
        let _ = event_tx.send(FileIndexEvent::ScanFinished { generation });
    });
    FileIndexScanHandle { cancel }
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
        let (watch_tx, watch_rx) = mpsc::channel();
        let mut watcher = match notify::recommended_watcher(move |result| {
            let _ = watch_tx.send(result);
        }) {
            Ok(watcher) => watcher,
            Err(err) => {
                tracing::warn!(%err, "file index watcher setup failed");
                return;
            }
        };
        if let Err(err) =
            notify::Watcher::watch(&mut watcher, &root_for_thread, notify::RecursiveMode::Recursive)
        {
            tracing::warn!(%err, "file index watcher start failed");
            return;
        }

        while !cancel_clone.load(AtomicOrdering::Relaxed) {
            let event = match watch_rx.recv_timeout(WATCH_POLL_INTERVAL) {
                Ok(event) => event,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break,
            };
            match event {
                Ok(event) => {
                    if let Some(next_event) = normalize_watch_event(
                        &root_for_thread,
                        generation,
                        respect_gitignore,
                        &event,
                    ) {
                        let _ = event_tx.send(next_event);
                    }
                }
                Err(err) => {
                    tracing::warn!(%err, "file index watcher event failed");
                    let _ = event_tx.send(FileIndexEvent::RebuildRequested { generation });
                }
            }
        }
    });
    FileIndexWatchHandle { cancel }
}

fn normalize_watch_event(
    root: &Path,
    generation: u64,
    respect_gitignore: bool,
    event: &notify::Event,
) -> Option<FileIndexEvent> {
    use notify::event::{CreateKind, EventKind, ModifyKind, RemoveKind, RenameMode};

    if matches_ignore_semantics_change(root, &event.paths) {
        return Some(FileIndexEvent::RebuildRequested { generation });
    }

    let changes = match event.kind {
        EventKind::Modify(ModifyKind::Name(RenameMode::Any | RenameMode::Both)) => {
            collect_rename_changes(root, respect_gitignore, &event.paths)
        }
        EventKind::Create(CreateKind::Any | CreateKind::File | CreateKind::Folder)
        | EventKind::Modify(
            ModifyKind::Any
            | ModifyKind::Data(_)
            | ModifyKind::Metadata(_)
            | ModifyKind::Name(RenameMode::To),
        ) => collect_create_or_modify_changes(root, respect_gitignore, &event.paths),
        EventKind::Modify(ModifyKind::Name(RenameMode::From))
        | EventKind::Remove(RemoveKind::Any | RemoveKind::File | RemoveKind::Folder) => {
            collect_remove_changes(root, &event.paths)
        }
        EventKind::Other => return Some(FileIndexEvent::RebuildRequested { generation }),
        _ => Vec::new(),
    };

    (!changes.is_empty()).then_some(FileIndexEvent::FsBatch { generation, changes })
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
    let _ = for_each_candidate(root, walk_root, respect_gitignore, cancel, &mut |candidate| {
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
    fn ranking_prefers_text_order_over_recency_for_equal_matches() {
        let mut candidates = vec![
            FileCandidate {
                rel_path: "src/zebra.rs".to_owned(),
                rel_path_lower: "src/zebra.rs".to_owned(),
                basename_lower: "zebra.rs".to_owned(),
                depth: 1,
                modified: SystemTime::UNIX_EPOCH + Duration::from_secs(20),
                is_dir: false,
            },
            FileCandidate {
                rel_path: "src/alpha.rs".to_owned(),
                rel_path_lower: "src/alpha.rs".to_owned(),
                basename_lower: "alpha.rs".to_owned(),
                depth: 1,
                modified: SystemTime::UNIX_EPOCH + Duration::from_secs(1),
                is_dir: false,
            },
        ];

        rank_and_truncate_candidates(&mut candidates, "rs");

        assert_eq!(candidates[0].rel_path, "src/alpha.rs");
        assert_eq!(candidates[1].rel_path, "src/zebra.rs");
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
    fn fs_batch_create_updates_visible_candidates_without_real_watcher() {
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

        assert!(app.needs_redraw);
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
