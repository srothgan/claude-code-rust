// SPDX-License-Identifier: Apache-2.0

mod git;
#[cfg(test)]
mod tests;

use crate::app::{App, FullscreenView, SurfaceMode, view};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
pub(crate) use git::{Comparison, FileChange, FileContent, Snapshot};
use ratatui::text::Line;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};

enum ReadRequest {
    Files { cwd: PathBuf, comparison: Comparison },
    Content { root: PathBuf, comparison: Comparison, head: String, file: FileChange },
}

enum ReadResult {
    Files(Result<Snapshot, String>),
    Content(Result<FileContent, String>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Loading {
    Files,
    Content,
}

pub(crate) struct RenderedContent {
    pub width: u16,
    pub full_file: bool,
    pub lines: Vec<Line<'static>>,
    pub hunks: Vec<usize>,
}

pub(crate) struct DiffView {
    pub comparison: Comparison,
    pub snapshot: Option<Snapshot>,
    pub selected: usize,
    pub tree_scroll: usize,
    pub content_focus: bool,
    pub full_file: bool,
    pub scroll: [usize; 2],
    pub viewport_height: usize,
    pub comparison_picker: Option<usize>,
    pub content: Option<FileContent>,
    pub rendered: Option<RenderedContent>,
    pub loading: Option<Loading>,
    pub error: Option<String>,
    cwd: PathBuf,
    preferred_change: Option<(String, char)>,
    request_id: u64,
    request_tx: Option<Sender<(u64, ReadRequest)>>,
    result_rx: Receiver<(u64, ReadResult)>,
}

impl DiffView {
    fn new(cwd: PathBuf) -> Self {
        let (request_tx, request_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let worker = std::thread::Builder::new().name("git-diff-reader".into()).spawn(move || {
            while let Ok(request) = request_rx.recv() {
                // Superseded reads queued while Git was busy need not run.
                let (id, request) = request_rx.try_iter().last().unwrap_or(request);
                let result = match request {
                    ReadRequest::Files { cwd, comparison } => ReadResult::Files(
                        git::snapshot(&cwd, comparison).map_err(|error| format!("{error:#}")),
                    ),
                    ReadRequest::Content { root, comparison, head, file } => ReadResult::Content(
                        git::content(&root, comparison, &head, &file)
                            .map_err(|error| format!("{error:#}")),
                    ),
                };
                if result_tx.send((id, result)).is_err() {
                    break;
                }
            }
        });
        let (request_tx, error) = match worker {
            Ok(_) => (Some(request_tx), None),
            Err(error) => (None, Some(format!("Cannot start Git reader: {error}"))),
        };
        Self {
            comparison: Comparison::Head,
            snapshot: None,
            selected: 0,
            tree_scroll: 0,
            content_focus: false,
            full_file: false,
            scroll: [0; 2],
            viewport_height: 1,
            comparison_picker: None,
            content: None,
            rendered: None,
            loading: None,
            error,
            cwd,
            preferred_change: None,
            request_id: 0,
            request_tx,
            result_rx,
        }
    }

    pub(crate) fn selected_file(&self) -> Option<&FileChange> {
        self.snapshot.as_ref()?.files.get(self.selected)
    }

    fn request(&mut self, request: ReadRequest, loading: Loading) {
        self.request_id = self.request_id.wrapping_add(1);
        self.loading = None;
        if let Some(sender) = &self.request_tx {
            if sender.send((self.request_id, request)).is_ok() {
                self.loading = Some(loading);
                self.error = None;
            } else {
                self.error = Some("Git reader stopped. Close and reopen /diff to retry.".into());
            }
        }
    }

    fn refresh(&mut self) {
        if let Some(file) = self.selected_file() {
            self.preferred_change = Some((file.path.clone(), file.status));
        }
        self.snapshot = None;
        self.clear_content();
        self.request(
            ReadRequest::Files { cwd: self.cwd.clone(), comparison: self.comparison },
            Loading::Files,
        );
    }

    fn clear_content(&mut self) {
        self.content = None;
        self.rendered = None;
        self.scroll = [0; 2];
    }

    fn read_selection(&mut self) {
        self.clear_content();
        let Some(snapshot) = &self.snapshot else { return };
        let Some(file) = snapshot.files.get(self.selected) else { return };
        let request = ReadRequest::Content {
            root: snapshot.root.clone(),
            comparison: snapshot.comparison,
            head: snapshot.head.clone(),
            file: file.clone(),
        };
        self.request(request, Loading::Content);
    }

    fn select(&mut self, next: usize) {
        let count = self.snapshot.as_ref().map_or(0, |snapshot| snapshot.files.len());
        if count == 0 {
            return;
        }
        let next = next.min(count.saturating_sub(1));
        if next != self.selected {
            self.selected = next;
            self.read_selection();
        }
    }

    fn poll(&mut self) -> bool {
        let mut changed = false;
        loop {
            let (id, result) = match self.result_rx.try_recv() {
                Ok(result) => result,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if self.loading.take().is_some() {
                        self.error =
                            Some("Git reader stopped. Close and reopen /diff to retry.".into());
                        changed = true;
                    }
                    break;
                }
            };
            changed |= self.apply_read_result(id, result);
        }
        changed
    }

    fn apply_read_result(&mut self, id: u64, result: ReadResult) -> bool {
        if id != self.request_id {
            return false;
        }
        self.loading = None;
        match result {
            ReadResult::Files(Ok(snapshot)) => {
                self.selected = self
                    .preferred_change
                    .take()
                    .and_then(|(path, status)| {
                        snapshot
                            .files
                            .iter()
                            .position(|file| file.path == path && file.status == status)
                            .or_else(|| snapshot.files.iter().position(|file| file.path == path))
                    })
                    .unwrap_or(self.selected.min(snapshot.files.len().saturating_sub(1)));
                self.snapshot = Some(snapshot);
                self.read_selection();
            }
            ReadResult::Content(Ok(content)) => {
                self.content = Some(content);
            }
            ReadResult::Files(Err(error)) | ReadResult::Content(Err(error)) => {
                self.error = Some(error);
            }
        }
        true
    }

    fn scroll_content(&mut self, delta: isize) {
        let index = usize::from(self.full_file);
        self.scroll[index] = self.scroll[index].saturating_add_signed(delta);
    }

    fn jump_hunk(&mut self, forward: bool) {
        if self.full_file {
            return;
        }
        let Some(rendered) = &self.rendered else { return };
        let current = self.scroll[0];
        let next = if forward {
            rendered.hunks.iter().find(|row| **row > current)
        } else {
            rendered.hunks.iter().rev().find(|row| **row < current)
        };
        if let Some(row) = next {
            self.scroll[0] = *row;
        }
    }
}

pub(crate) fn open(app: &mut App) {
    let mut state = DiffView::new(PathBuf::from(&app.cwd_raw));
    state.refresh();
    app.diff_view = Some(Box::new(state));
    view::set_fullscreen_view(app, FullscreenView::Diff);
}

pub(crate) fn poll(app: &mut App) {
    if is_active(app) && app.diff_view.as_deref_mut().is_some_and(DiffView::poll) {
        app.request_fullscreen_repaint();
    }
}

pub(crate) fn handle_key(app: &mut App, key: KeyEvent) {
    if super::keys::is_ctrl_char_shortcut(key, 'q') {
        app.request_shutdown();
        return;
    }
    if key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        && !(matches!(key.code, KeyCode::Char(_))
            && super::keys::is_printable_text_modifiers(key.modifiers))
    {
        return;
    }
    let Some(state) = &mut app.diff_view else { return };
    if let Some(selected) = &mut state.comparison_picker {
        match key.code {
            KeyCode::Up => *selected = selected.saturating_sub(1),
            KeyCode::Down => {
                *selected = (*selected + 1).min(Comparison::ALL.len().saturating_sub(1));
            }
            KeyCode::Enter => {
                let comparison = Comparison::ALL[*selected];
                state.comparison_picker = None;
                if comparison != state.comparison {
                    state.comparison = comparison;
                    state.refresh();
                }
            }
            KeyCode::Esc | KeyCode::Char('s') => state.comparison_picker = None,
            _ => {}
        }
        return;
    }
    let count = state.snapshot.as_ref().map_or(0, |snapshot| snapshot.files.len());
    match key.code {
        KeyCode::Esc if !state.content_focus => {
            view::set_chat_surface(app);
        }
        KeyCode::Esc | KeyCode::Left => state.content_focus = false,
        KeyCode::Enter | KeyCode::Right => state.content_focus = true,
        KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => {
            state.select(if state.selected == 0 {
                count.saturating_sub(1)
            } else {
                state.selected - 1
            });
        }
        KeyCode::Tab => {
            state.select(if state.selected + 1 < count { state.selected + 1 } else { 0 });
        }
        KeyCode::BackTab => state.select(if state.selected == 0 {
            count.saturating_sub(1)
        } else {
            state.selected - 1
        }),
        KeyCode::Up if state.content_focus => state.scroll_content(-1),
        KeyCode::Down if state.content_focus => state.scroll_content(1),
        KeyCode::Up => state.select(state.selected.saturating_sub(1)),
        KeyCode::Down => state.select(state.selected + 1),
        KeyCode::Char('v') => {
            state.full_file = !state.full_file;
            state.rendered = None;
        }
        KeyCode::Char('s') => {
            state.comparison_picker =
                Comparison::ALL.iter().position(|scope| *scope == state.comparison);
        }
        KeyCode::Char('r') => state.refresh(),
        KeyCode::Char(']') => state.jump_hunk(true),
        KeyCode::Char('[') => state.jump_hunk(false),
        _ => {}
    }
}

pub(crate) fn is_active(app: &App) -> bool {
    app.surface_mode == SurfaceMode::Fullscreen(FullscreenView::Diff)
}
