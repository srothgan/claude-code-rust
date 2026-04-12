// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

const WATCH_DEBOUNCE: Duration = Duration::from_millis(75);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BranchDisplayState {
    Named(String),
    Detached,
    NoRepo,
    Unknown,
}

impl BranchDisplayState {
    #[must_use]
    pub(crate) fn as_deref(&self) -> Option<&str> {
        match self {
            Self::Named(branch) => Some(branch.as_str()),
            Self::Detached | Self::NoRepo | Self::Unknown => None,
        }
    }
}

#[derive(Debug)]
pub(crate) struct GitContextState {
    repo: Option<ResolvedRepo>,
    branch: BranchDisplayState,
    watcher: Option<RecommendedWatcher>,
    event_rx: Option<Receiver<notify::Result<Event>>>,
    pending_refresh_since: Option<Instant>,
}

impl Default for GitContextState {
    fn default() -> Self {
        Self {
            repo: None,
            branch: BranchDisplayState::NoRepo,
            watcher: None,
            event_rx: None,
            pending_refresh_since: None,
        }
    }
}

impl GitContextState {
    #[must_use]
    pub(crate) fn branch_name(&self) -> Option<&str> {
        self.branch.as_deref()
    }

    pub(crate) fn sync_to_cwd(&mut self, cwd: &Path) -> bool {
        let discovered = ResolvedRepo::discover(cwd);
        let repo_changed = self.repo.as_ref() != discovered.as_ref();
        let should_install_watcher = repo_changed
            || (discovered.is_some() && (self.watcher.is_none() || self.event_rx.is_none()));

        if should_install_watcher {
            self.install_watcher(discovered.as_ref());
        } else if discovered.is_none() {
            self.watcher = None;
            self.event_rx = None;
            self.pending_refresh_since = None;
        }

        let new_branch = match discovered.as_ref() {
            Some(repo) => repo.resolve_branch_state(),
            None => BranchDisplayState::NoRepo,
        };

        self.repo = discovered;
        if new_branch != self.branch {
            self.branch = new_branch;
            return true;
        }
        false
    }

    pub(crate) fn tick(&mut self, cwd: &Path, now: Instant) -> bool {
        let mut should_refresh = false;

        if let Some(event_rx) = &self.event_rx {
            loop {
                match event_rx.try_recv() {
                    Ok(Ok(event)) => {
                        if event.need_rescan()
                            || self.repo.as_ref().is_some_and(|repo| repo.is_relevant_event(&event))
                        {
                            should_refresh = true;
                        }
                    }
                    Ok(Err(err)) => {
                        tracing::debug!(error = %err, "git watcher reported an error");
                        should_refresh = true;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        self.watcher = None;
                        self.event_rx = None;
                        should_refresh = true;
                        break;
                    }
                }
            }
        }

        if should_refresh && self.pending_refresh_since.is_none() {
            self.pending_refresh_since = Some(now);
        }

        if self
            .pending_refresh_since
            .is_some_and(|started| now.duration_since(started) >= WATCH_DEBOUNCE)
        {
            self.pending_refresh_since = None;
            return self.sync_to_cwd(cwd);
        }

        false
    }

    #[cfg(test)]
    pub(crate) fn set_branch_for_test(&mut self, branch: Option<&str>) {
        self.branch = match branch {
            Some(branch) => BranchDisplayState::Named(branch.to_owned()),
            None => BranchDisplayState::NoRepo,
        };
    }

    fn install_watcher(&mut self, repo: Option<&ResolvedRepo>) {
        self.pending_refresh_since = None;
        self.watcher = None;
        self.event_rx = None;

        let Some(repo) = repo else {
            return;
        };

        let (event_tx, event_rx) = mpsc::channel();
        let watcher_result = notify::recommended_watcher(move |event| {
            let _ = event_tx.send(event);
        });

        let mut watcher = match watcher_result {
            Ok(watcher) => watcher,
            Err(err) => {
                tracing::warn!(error = %err, "failed to initialize git metadata watcher");
                return;
            }
        };

        for (path, mode) in repo.watch_directories() {
            if let Err(err) = watcher.watch(&path, mode) {
                tracing::warn!(path = %path.display(), error = %err, "failed to watch git metadata path");
            }
        }

        self.event_rx = Some(event_rx);
        self.watcher = Some(watcher);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedRepo {
    worktree_root: PathBuf,
    dot_git_path: PathBuf,
    effective_git_dir: PathBuf,
    common_git_dir: PathBuf,
    head_path: PathBuf,
    packed_refs_path: PathBuf,
    commondir_path: Option<PathBuf>,
    heads_dir: PathBuf,
}

impl ResolvedRepo {
    fn discover(cwd: &Path) -> Option<Self> {
        let normalized_cwd = normalize_path(cwd);
        for ancestor in normalized_cwd.ancestors() {
            let dot_git_path = ancestor.join(".git");
            let Ok(metadata) = fs::metadata(&dot_git_path) else {
                continue;
            };

            let worktree_root = normalize_path(ancestor);
            let effective_git_dir = if metadata.is_dir() {
                normalize_path(&dot_git_path)
            } else if metadata.is_file() {
                let gitdir = parse_gitdir_target(&dot_git_path)?;
                resolve_relative_path(&worktree_root, &gitdir)
            } else {
                continue;
            };

            let commondir_path = effective_git_dir.join("commondir");
            let common_git_dir = read_optional_target(&commondir_path).map_or_else(
                || effective_git_dir.clone(),
                |target| resolve_relative_path(&effective_git_dir, &target),
            );
            let heads_dir = common_git_dir.join("refs").join("heads");

            return Some(Self {
                worktree_root,
                dot_git_path: normalize_path(&dot_git_path),
                effective_git_dir: normalize_path(&effective_git_dir),
                common_git_dir: normalize_path(&common_git_dir),
                head_path: normalize_path(&effective_git_dir.join("HEAD")),
                packed_refs_path: normalize_path(&common_git_dir.join("packed-refs")),
                commondir_path: commondir_path.exists().then(|| normalize_path(&commondir_path)),
                heads_dir: normalize_path(&heads_dir),
            });
        }
        None
    }

    fn resolve_branch_state(&self) -> BranchDisplayState {
        let Ok(head) = fs::read_to_string(&self.head_path) else {
            return BranchDisplayState::Unknown;
        };

        let trimmed = head.trim();
        if trimmed.is_empty() {
            return BranchDisplayState::Unknown;
        }

        let Some(reference) = trimmed.strip_prefix("ref:") else {
            return BranchDisplayState::Detached;
        };
        let reference = reference.trim();
        if let Some(branch) = reference.strip_prefix("refs/heads/") {
            return BranchDisplayState::Named(branch.to_owned());
        }

        BranchDisplayState::Detached
    }

    fn is_relevant_event(&self, event: &Event) -> bool {
        event.paths.iter().any(|path| self.is_relevant_path(path))
    }

    fn is_relevant_path(&self, path: &Path) -> bool {
        let normalized = normalize_path(path);
        if normalized.starts_with(&self.heads_dir) {
            return true;
        }

        let Some(file_name) = normalized.file_name().and_then(|name| name.to_str()) else {
            return false;
        };

        if normalized.parent() == Some(self.worktree_root.as_path())
            && matches!(file_name, ".git" | ".git.lock")
        {
            return true;
        }

        if normalized.parent() == Some(self.effective_git_dir.as_path())
            && matches!(file_name, "HEAD" | "HEAD.lock" | "commondir")
        {
            return true;
        }

        normalized.parent() == Some(self.common_git_dir.as_path())
            && matches!(file_name, "packed-refs" | "packed-refs.lock")
    }

    fn watch_directories(&self) -> Vec<(PathBuf, RecursiveMode)> {
        let mut watched = BTreeMap::new();
        insert_watch_path(&mut watched, self.worktree_root.clone(), RecursiveMode::NonRecursive);
        insert_watch_path(
            &mut watched,
            self.effective_git_dir.clone(),
            RecursiveMode::NonRecursive,
        );
        insert_watch_path(&mut watched, self.common_git_dir.clone(), RecursiveMode::NonRecursive);

        if self.heads_dir.exists() {
            insert_watch_path(&mut watched, self.heads_dir.clone(), RecursiveMode::Recursive);
        }

        watched.into_iter().collect()
    }
}

fn insert_watch_path(
    watched: &mut BTreeMap<PathBuf, RecursiveMode>,
    path: PathBuf,
    recursive_mode: RecursiveMode,
) {
    match watched.get_mut(&path) {
        Some(mode) if recursive_mode == RecursiveMode::Recursive => {
            *mode = RecursiveMode::Recursive;
        }
        Some(_) => {}
        None => {
            watched.insert(path, recursive_mode);
        }
    }
}

fn parse_gitdir_target(dot_git_path: &Path) -> Option<PathBuf> {
    let content = fs::read_to_string(dot_git_path).ok()?;
    let raw = content.lines().find_map(|line| line.trim().strip_prefix("gitdir:"))?;
    let target = raw.trim();
    (!target.is_empty()).then(|| PathBuf::from(target))
}

fn read_optional_target(path: &Path) -> Option<PathBuf> {
    let content = fs::read_to_string(path).ok()?;
    let trimmed = content.trim();
    (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
}

fn resolve_relative_path(base: &Path, target: &Path) -> PathBuf {
    if target.is_absolute() { normalize_path(target) } else { normalize_path(&base.join(target)) }
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            Component::Normal(part) => normalized.push(part),
        }
    }

    if normalized.as_os_str().is_empty() { PathBuf::from(".") } else { normalized }
}

#[cfg(test)]
mod tests {
    use super::{BranchDisplayState, GitContextState, ResolvedRepo, WATCH_DEBOUNCE};
    use notify::{Event, EventKind};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    fn write_file(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(path, contents).expect("write file");
    }

    fn create_standard_repo(root: &Path, branch: &str) -> PathBuf {
        let repo = root.join("repo");
        fs::create_dir_all(repo.join("src")).expect("create repo");
        write_file(&repo.join(".git").join("HEAD"), &format!("ref: refs/heads/{branch}\n"));
        write_file(&repo.join(".git").join("refs").join("heads").join(branch), "deadbeef\n");
        repo
    }

    fn create_worktree_repo(root: &Path, branch: &str) -> PathBuf {
        let repo = root.join("worktree");
        let effective = root.join("admin").join("worktrees").join("wt-1");
        let common = root.join("admin").join("common");
        fs::create_dir_all(repo.join("src")).expect("create worktree");
        write_file(&repo.join(".git"), "gitdir: ../admin/worktrees/wt-1\n");
        write_file(&effective.join("HEAD"), &format!("ref: refs/heads/{branch}\n"));
        write_file(&effective.join("commondir"), "../../common\n");
        write_file(&common.join("refs").join("heads").join(branch), "cafebabe\n");
        repo
    }

    #[test]
    fn discovers_repo_root_from_nested_cwd() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = create_standard_repo(dir.path(), "main");

        let resolved = ResolvedRepo::discover(&repo.join("src").join("nested")).expect("repo");

        assert_eq!(resolved.worktree_root, repo);
        assert_eq!(resolved.effective_git_dir, repo.join(".git"));
        assert_eq!(resolved.common_git_dir, repo.join(".git"));
    }

    #[test]
    fn resolves_git_file_and_commondir_layout() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = create_worktree_repo(dir.path(), "feature/footer");

        let resolved = ResolvedRepo::discover(&repo.join("src")).expect("repo");

        assert_eq!(resolved.worktree_root, repo);
        assert_eq!(
            resolved.effective_git_dir,
            dir.path().join("admin").join("worktrees").join("wt-1")
        );
        assert_eq!(resolved.common_git_dir, dir.path().join("admin").join("common"));
        assert_eq!(
            resolved.heads_dir,
            dir.path().join("admin").join("common").join("refs").join("heads")
        );
    }

    #[test]
    fn resolves_named_branch_from_head() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = create_standard_repo(dir.path(), "feature/footer");
        let resolved = ResolvedRepo::discover(&repo).expect("repo");

        assert_eq!(
            resolved.resolve_branch_state(),
            BranchDisplayState::Named("feature/footer".to_owned())
        );
    }

    #[test]
    fn resolves_detached_head_state() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = create_standard_repo(dir.path(), "main");
        write_file(&repo.join(".git").join("HEAD"), "0123456789abcdef\n");
        let resolved = ResolvedRepo::discover(&repo).expect("repo");

        assert_eq!(resolved.resolve_branch_state(), BranchDisplayState::Detached);
    }

    #[test]
    fn returns_none_when_outside_repo() {
        let dir = tempfile::tempdir().expect("tempdir");

        assert!(ResolvedRepo::discover(dir.path()).is_none());
    }

    #[test]
    fn filters_relevant_git_metadata_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = create_standard_repo(dir.path(), "main");
        let resolved = ResolvedRepo::discover(&repo).expect("repo");

        let head_event = Event::new(EventKind::Any).add_path(repo.join(".git").join("HEAD"));
        let packed_refs_event =
            Event::new(EventKind::Any).add_path(repo.join(".git").join("packed-refs"));
        let branch_ref_event = Event::new(EventKind::Any)
            .add_path(repo.join(".git").join("refs").join("heads").join("main"));
        let unrelated_event = Event::new(EventKind::Any).add_path(repo.join(".git").join("index"));

        assert!(resolved.is_relevant_event(&head_event));
        assert!(resolved.is_relevant_event(&packed_refs_event));
        assert!(resolved.is_relevant_event(&branch_ref_event));
        assert!(!resolved.is_relevant_event(&unrelated_event));
    }

    #[test]
    fn debounces_bursty_events_into_one_refresh() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = create_standard_repo(dir.path(), "main");
        let mut state = GitContextState::default();
        assert!(state.sync_to_cwd(&repo));
        write_file(&repo.join(".git").join("HEAD"), "ref: refs/heads/feature/footer\n");
        write_file(
            &repo.join(".git").join("refs").join("heads").join("feature").join("footer"),
            "feedface\n",
        );

        let (event_tx, event_rx) = mpsc::channel();
        state.event_rx = Some(event_rx);
        state.watcher = None;
        let first = Event::new(EventKind::Any).add_path(repo.join(".git").join("HEAD"));
        let second =
            Event::new(EventKind::Any).add_path(repo.join(".git").join("refs").join("heads"));
        event_tx.send(Ok(first)).expect("send first");
        event_tx.send(Ok(second)).expect("send second");

        let started = Instant::now();
        assert!(!state.tick(&repo, started));
        assert_eq!(state.branch_name(), Some("main"));
        assert!(!state.tick(&repo, started + Duration::from_millis(10)));
        assert!(state.tick(&repo, started + WATCH_DEBOUNCE + Duration::from_millis(10)));
        assert_eq!(state.branch_name(), Some("feature/footer"));
    }

    #[test]
    fn clears_branch_state_when_cwd_leaves_repo() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = create_standard_repo(dir.path(), "main");
        let outside = dir.path().join("outside");
        fs::create_dir_all(&outside).expect("outside dir");
        let mut state = GitContextState::default();

        assert!(state.sync_to_cwd(&repo));
        assert_eq!(state.branch_name(), Some("main"));
        assert!(state.sync_to_cwd(&outside));
        assert_eq!(state.branch_name(), None);
    }

    #[test]
    fn repo_identity_is_stable_across_nested_cwds() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = create_standard_repo(dir.path(), "main");
        let nested = repo.join("src").join("nested");
        fs::create_dir_all(&nested).expect("nested dir");
        let mut state = GitContextState::default();

        assert!(state.sync_to_cwd(&repo));
        let original = state.repo.clone().expect("repo");
        assert!(!state.sync_to_cwd(&nested));
        assert_eq!(state.repo.as_ref(), Some(&original));
    }
}
