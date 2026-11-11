// SPDX-License-Identifier: Apache-2.0

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

const WATCH_DEBOUNCE: Duration = Duration::from_millis(75);

trait MetadataWatcher: std::fmt::Debug {
    fn watch(&mut self, path: &Path, mode: RecursiveMode) -> notify::Result<()>;
}

impl MetadataWatcher for RecommendedWatcher {
    fn watch(&mut self, path: &Path, mode: RecursiveMode) -> notify::Result<()> {
        Watcher::watch(self, path, mode)
    }
}

type WatchSource = (Box<dyn MetadataWatcher>, Receiver<notify::Result<Event>>);

fn native_watch_source() -> notify::Result<WatchSource> {
    let (event_tx, event_rx) = mpsc::channel();
    let watcher = notify::recommended_watcher(move |event| {
        let _ = event_tx.send(event);
    })?;
    Ok((Box::new(watcher), event_rx))
}

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
    watcher: Option<Box<dyn MetadataWatcher>>,
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
        self.sync_to_cwd_with(cwd, &mut native_watch_source)
    }

    fn sync_to_cwd_with(
        &mut self,
        cwd: &Path,
        source: &mut impl FnMut() -> notify::Result<WatchSource>,
    ) -> bool {
        let discovered = ResolvedRepo::discover(cwd);
        let repo_changed = self.repo.as_ref() != discovered.as_ref();
        let should_install_watcher = repo_changed
            || (discovered.is_some() && (self.watcher.is_none() || self.event_rx.is_none()));

        if should_install_watcher {
            self.install_watcher(discovered.as_ref(), source);
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
        self.tick_with(cwd, now, &mut native_watch_source)
    }

    fn tick_with(
        &mut self,
        cwd: &Path,
        now: Instant,
        source: &mut impl FnMut() -> notify::Result<WatchSource>,
    ) -> bool {
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
                        self.watcher = None;
                        self.event_rx = None;
                        should_refresh = true;
                        break;
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
            return self.sync_to_cwd_with(cwd, source);
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

    fn install_watcher(
        &mut self,
        repo: Option<&ResolvedRepo>,
        source: &mut impl FnMut() -> notify::Result<WatchSource>,
    ) {
        self.pending_refresh_since = None;
        self.watcher = None;
        self.event_rx = None;

        let Some(repo) = repo else {
            return;
        };

        let (mut watcher, event_rx) = match source() {
            Ok(source) => source,
            Err(err) => {
                tracing::warn!(error = %err, "failed to initialize git metadata watcher");
                return;
            }
        };

        let mut all_registered = true;
        for (path, mode) in repo.watch_directories() {
            if let Err(err) = watcher.watch(&path, mode) {
                tracing::warn!(path = %path.display(), error = %err, "failed to watch git metadata path");
                all_registered = false;
            }
        }

        // A partial subscription must remain eligible for admission on the next sync.
        if !all_registered {
            return;
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
    use super::{GitContextState, MetadataWatcher, WATCH_DEBOUNCE, WatchSource};
    use notify::{Event, EventKind, RecursiveMode};
    use std::cell::RefCell;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;
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

    #[derive(Debug, Default)]
    struct WatchControl {
        paths: Vec<(PathBuf, RecursiveMode)>,
        sender: Option<mpsc::Sender<notify::Result<Event>>>,
        fail_path: Option<PathBuf>,
    }

    impl WatchControl {
        fn notify_path(&self, path: &Path) {
            if self.paths.iter().any(|(watched, mode)| {
                if *mode == RecursiveMode::Recursive {
                    path.starts_with(watched)
                } else {
                    path.parent() == Some(watched.as_path())
                }
            }) {
                self.send(Ok(Event::new(EventKind::Any).add_path(path.to_path_buf())));
            }
        }

        fn send(&self, event: notify::Result<Event>) {
            self.sender.as_ref().expect("active event source").send(event).expect("live receiver");
        }
    }

    #[derive(Debug)]
    struct ControlledWatcher(Rc<RefCell<WatchControl>>);

    impl MetadataWatcher for ControlledWatcher {
        fn watch(&mut self, path: &Path, mode: RecursiveMode) -> notify::Result<()> {
            let mut control = self.0.borrow_mut();
            if control.fail_path.as_deref() == Some(path) {
                return Err(notify::Error::generic("fixture admission failure"));
            }
            control.paths.push((path.to_path_buf(), mode));
            Ok(())
        }
    }

    impl Drop for ControlledWatcher {
        fn drop(&mut self) {
            let mut control = self.0.borrow_mut();
            control.paths.clear();
            control.sender = None;
        }
    }

    fn controlled_source(
        control: &Rc<RefCell<WatchControl>>,
    ) -> impl FnMut() -> notify::Result<WatchSource> + use<> {
        let control = Rc::clone(control);
        move || {
            let (sender, receiver) = mpsc::channel();
            control.borrow_mut().sender = Some(sender);
            Ok((Box::new(ControlledWatcher(Rc::clone(&control))), receiver))
        }
    }

    fn fixture(root: &Path, linked: bool, branch: &str) -> (PathBuf, PathBuf, PathBuf) {
        if linked {
            let repo = create_worktree_repo(root, branch);
            let common = root.join("admin/common");
            write_file(&common.join("HEAD"), "ref: refs/heads/WRONG-common-branch\n");
            (repo, root.join("admin/worktrees/wt-1/HEAD"), common)
        } else {
            let repo = create_standard_repo(root, branch);
            let common = repo.join(".git");
            (repo, common.join("HEAD"), common)
        }
    }

    #[test]
    fn existing_nested_cwds_project_the_effective_named_or_detached_head() {
        for linked in [false, true] {
            let dir = tempfile::tempdir().expect("fixture");
            let (repo, head, _) = fixture(dir.path(), linked, "feature/footer");
            let nested = repo.join("src/nested");
            fs::create_dir_all(&nested).expect("existing nested cwd");
            let mut state = GitContextState::default();
            state.sync_to_cwd(&nested);
            assert_eq!(state.branch_name(), Some("feature/footer"));
            state.sync_to_cwd(&repo);
            assert_eq!(state.branch_name(), Some("feature/footer"));
            write_file(&head, "0123456789abcdef0123456789abcdef01234567\n");
            state.sync_to_cwd(&nested);
            assert_eq!(state.branch_name(), None, "detached HEAD clears the named branch");
            let outside = tempfile::tempdir().expect("independent outside cwd");
            state.sync_to_cwd(outside.path());
            assert_eq!(state.branch_name(), None);
        }
    }

    #[test]
    fn notified_branch_refresh_is_not_starved_by_a_continuing_burst() {
        for linked in [false, true] {
            let dir = tempfile::tempdir().expect("fixture");
            let (repo, head, common) = fixture(dir.path(), linked, "before");
            let control = Rc::new(RefCell::new(WatchControl::default()));
            let mut source = controlled_source(&control);
            let mut state = GitContextState::default();
            state.sync_to_cwd_with(&repo, &mut source);
            let start = Instant::now();
            write_file(&head, "ref: refs/heads/first\n");
            control.borrow().notify_path(&head);
            state.tick_with(&repo, start, &mut source);
            assert_eq!(state.branch_name(), Some("before"));
            write_file(&head, "ref: refs/heads/second\n");
            control.borrow().notify_path(&common.join("packed-refs"));
            state.tick_with(&repo, start + WATCH_DEBOUNCE / 2, &mut source);
            assert_eq!(state.branch_name(), Some("before"));
            write_file(&head, "ref: refs/heads/latest\n");
            control.borrow().notify_path(&common.join("refs/heads/feature/footer"));
            state.tick_with(&repo, start + WATCH_DEBOUNCE, &mut source);
            assert_eq!(
                state.branch_name(),
                Some("latest"),
                "first event bounds the refresh deadline"
            );
            write_file(&head, "ref: refs/heads/later-independent\n");
            control.borrow().notify_path(&head);
            state.tick_with(&repo, start + WATCH_DEBOUNCE * 2, &mut source);
            state.tick_with(&repo, start + WATCH_DEBOUNCE * 3, &mut source);
            assert_eq!(state.branch_name(), Some("later-independent"));
        }
    }

    #[test]
    fn failed_effective_head_watch_is_admitted_on_same_cwd_resynchronization() {
        let dir = tempfile::tempdir().expect("fixture");
        let (repo, head, _) = fixture(dir.path(), true, "before");
        let control = Rc::new(RefCell::new(WatchControl {
            fail_path: head.parent().map(Path::to_path_buf),
            ..WatchControl::default()
        }));
        let mut source = controlled_source(&control);
        let mut state = GitContextState::default();
        state.sync_to_cwd_with(&repo, &mut source);
        assert_eq!(state.branch_name(), Some("before"));
        control.borrow_mut().fail_path = None;
        state.sync_to_cwd_with(&repo, &mut source);
        write_file(&head, "ref: refs/heads/recovered-admission\n");
        control.borrow().notify_path(&head);
        let now = Instant::now();
        state.tick_with(&repo, now, &mut source);
        state.tick_with(&repo, now + WATCH_DEBOUNCE, &mut source);
        assert_eq!(state.branch_name(), Some("recovered-admission"));
    }

    #[test]
    fn rescan_error_and_disconnection_recover_and_observe_the_next_change() {
        for failure in ["rescan", "lost subscriptions", "disconnect"] {
            let dir = tempfile::tempdir().expect("fixture");
            let (repo, head, _) = fixture(dir.path(), true, "before");
            let control = Rc::new(RefCell::new(WatchControl::default()));
            let mut source = controlled_source(&control);
            let mut state = GitContextState::default();
            state.sync_to_cwd_with(&repo, &mut source);
            write_file(&head, "ref: refs/heads/after-failure\n");
            match failure {
                "rescan" => control
                    .borrow()
                    .send(Ok(Event::new(EventKind::Other).set_flag(notify::event::Flag::Rescan))),
                "lost subscriptions" => {
                    control.borrow_mut().paths.clear();
                    control
                        .borrow()
                        .send(Err(notify::Error::generic("fixture lost subscriptions")));
                }
                _ => {
                    control.borrow_mut().sender = None;
                }
            }
            let now = Instant::now();
            state.tick_with(&repo, now, &mut source);
            state.tick_with(&repo, now + WATCH_DEBOUNCE, &mut source);
            assert_eq!(state.branch_name(), Some("after-failure"), "{failure}");
            write_file(&head, "ref: refs/heads/still-watching\n");
            control.borrow().notify_path(&head);
            state.tick_with(&repo, now + WATCH_DEBOUNCE * 2, &mut source);
            state.tick_with(&repo, now + WATCH_DEBOUNCE * 3, &mut source);
            assert_eq!(state.branch_name(), Some("still-watching"), "{failure}");
        }
    }

    #[test]
    fn missing_head_clears_stale_branch_and_restoration_is_observed() {
        let dir = tempfile::tempdir().expect("fixture");
        let (repo, head, _) = fixture(dir.path(), false, "before");
        let control = Rc::new(RefCell::new(WatchControl::default()));
        let mut source = controlled_source(&control);
        let mut state = GitContextState::default();
        state.sync_to_cwd_with(&repo, &mut source);
        fs::remove_file(&head).expect("remove fixture HEAD");
        control.borrow().notify_path(&head);
        let now = Instant::now();
        state.tick_with(&repo, now, &mut source);
        state.tick_with(&repo, now + WATCH_DEBOUNCE, &mut source);
        assert_eq!(state.branch_name(), None);
        write_file(&head, "ref: refs/heads/restored\n");
        control.borrow().notify_path(&head);
        state.tick_with(&repo, now + WATCH_DEBOUNCE * 2, &mut source);
        state.tick_with(&repo, now + WATCH_DEBOUNCE * 3, &mut source);
        assert_eq!(state.branch_name(), Some("restored"));
    }

    fn replace_head(head: &Path, branch: &str) {
        let lock = head.with_extension("lock");
        write_file(&lock, &format!("ref: refs/heads/{branch}\n"));
        fs::rename(lock, head).expect("Git-style HEAD lock/rename");
    }

    fn assert_branch_region(app: &crate::app::App, expected: &str) {
        let footer = crate::ui::footer_rows::serialize_footer_rows(app, 500);
        let branches: Vec<_> = footer.rows[0]
            .spans
            .iter()
            .filter(|s| s.content.starts_with("Branch: "))
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(branches, [expected], "actual footer branch region");
    }

    fn await_native_branch(
        app: &mut crate::app::App,
        expected: &str,
        forbidden: Option<&str>,
        logical_now: &mut Instant,
    ) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while app.git_branch() != Some(expected) {
            assert!(
                Instant::now() < deadline,
                "native notification did not deliver {expected}; got {:?}",
                app.git_branch()
            );
            *logical_now += WATCH_DEBOUNCE;
            app.tick_git_context(*logical_now);
            if let Some(forbidden) = forbidden {
                assert_ne!(
                    app.git_branch(),
                    Some(forbidden),
                    "old repository must not regain ownership"
                );
            }
            std::thread::yield_now();
        }
        assert!(app.surface_dirty.chat.repaint, "notified branch change schedules repaint");
    }

    #[tokio::test]
    async fn native_head_replacement_updates_footer_and_same_name_cwd_handoff() {
        tokio::task::LocalSet::new()
            .run_until(async {
                let dir = tempfile::tempdir().expect("fixture");
                let first_root = dir.path().join("ordinary");
                let second_root = dir.path().join("linked");
                let (first, first_head, _) = fixture(&first_root, false, "same/branch");
                let (second, second_head, _) = fixture(&second_root, true, "same/branch");
                let (mut app, _commands) = crate::app::test_support::app_with_connection();
                app.settings_home_override = Some(dir.path().to_path_buf());
                app.cwd_raw = first.to_string_lossy().into_owned();
                app.cwd = "fixture".into();
                app.sync_git_context();
                assert_branch_region(&app, "Branch: same/branch");
                let mut now = Instant::now();
                app.surface_dirty.chat.repaint = false;
                replace_head(&first_head, "ordinary/updated");
                await_native_branch(&mut app, "ordinary/updated", None, &mut now);
                assert_branch_region(&app, "Branch: ordinary/updated");
                replace_head(&first_head, "same/branch");
                await_native_branch(&mut app, "same/branch", None, &mut now);
                crate::app::handle_client_event(
                    &mut app,
                    crate::app::test_support::session_replacement(
                        "session-2",
                        &second,
                        vec![],
                        None,
                    ),
                );
                assert_branch_region(&app, "Branch: same/branch");
                app.surface_dirty.chat.repaint = false;
                replace_head(&second_head, "linked/updated");
                await_native_branch(&mut app, "linked/updated", None, &mut now);
                assert_branch_region(&app, "Branch: linked/updated");
                app.surface_dirty.chat.repaint = false;
                replace_head(&first_head, "old/again");
                replace_head(&second_head, "linked/still-watching");
                await_native_branch(&mut app, "linked/still-watching", Some("old/again"), &mut now);
                assert_branch_region(&app, "Branch: linked/still-watching");
            })
            .await;
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
}
