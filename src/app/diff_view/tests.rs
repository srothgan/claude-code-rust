// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::app::events::handle_terminal_event;
use crossterm::event::Event;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use std::fmt::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

struct Repo(tempfile::TempDir);

impl Repo {
    fn new() -> Self {
        let repo = Self(tempfile::tempdir().expect("fixture directory"));
        repo.git(&["init", "-q"]);
        repo.git(&["config", "core.autocrlf", "false"]);
        repo.git(&["config", "commit.gpgsign", "false"]);
        repo.git(&["config", "user.name", "Diff test"]);
        repo.git(&["config", "user.email", "diff@example.invalid"]);
        // A viewer must not execute the user's external diff program.
        repo.git(&["config", "diff.external", "must-not-execute-external-diff"]);
        repo.git(&["config", "diff.outputIndicatorNew", ">"]);
        repo.git(&["config", "diff.outputIndicatorOld", "<"]);
        repo.git(&["config", "diff.outputIndicatorContext", "|"]);
        repo.git(&["config", "diff.suppressBlankEmpty", "true"]);
        repo
    }

    fn path(&self) -> &Path {
        self.0.path()
    }

    fn git(&self, args: &[&str]) -> Vec<u8> {
        let output = git::command(self.path())
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("fixture Git");
        assert!(output.status.success(), "{args:?}: {}", String::from_utf8_lossy(&output.stderr));
        output.stdout
    }

    fn write(&self, path: &str, bytes: impl AsRef<[u8]>) {
        let path = self.path().join(path);
        std::fs::create_dir_all(path.parent().expect("fixture parent")).expect("create parents");
        std::fs::write(path, bytes).expect("write fixture");
    }

    fn commit(&self) {
        self.git(&["add", "."]);
        self.git(&["-c", "core.hooksPath=", "commit", "-qm", "fixture baseline"]);
    }
}

fn file<'a>(snapshot: &'a Snapshot, path: &str) -> &'a FileChange {
    snapshot.files.iter().find(|file| file.path == path).expect("expected changed file")
}

fn contents(snapshot: &Snapshot, path: &str) -> FileContent {
    git::content(&snapshot.root, snapshot.comparison, &snapshot.head, file(snapshot, path))
        .expect("read changed file")
}

#[test]
fn comparisons_show_the_correct_versions_and_never_modify_the_worktree_or_index() {
    let repo = Repo::new();
    repo.write("src/change.txt", "base\nshared\n");
    repo.write("deleted.txt", "deleted baseline\n");
    repo.write("old name.txt", "unchanged rename\n");
    repo.commit();
    repo.write("src/change.txt", "base\nstaged\n");
    std::fs::remove_file(repo.path().join("deleted.txt")).expect("delete fixture");
    std::fs::rename(repo.path().join("old name.txt"), repo.path().join("new name.txt"))
        .expect("rename fixture");
    repo.git(&["add", "-A"]);
    repo.write("src/change.txt", "base\nstaged\nworking\n");
    repo.write("new ünicode.txt", "untracked one\nuntracked two\n");
    repo.write("binary.dat", [0, 1, 2, 255]);
    let index_before = std::fs::read(repo.path().join(".git/index")).expect("index fixture");
    let status_before = repo.git(&["--no-optional-locks", "status", "--porcelain=v1", "-z"]);

    let head = git::snapshot(&repo.path().join("src"), Comparison::Head)
        .expect("HEAD comparison from a subdirectory");
    assert_eq!(
        head.files.iter().map(|f| f.path.as_str()).collect::<std::collections::BTreeSet<_>>(),
        ["binary.dat", "deleted.txt", "new name.txt", "new \u{fc}nicode.txt", "src/change.txt"]
            .into_iter()
            .collect()
    );
    assert_eq!(file(&head, "src/change.txt").counts, Some((2, 1)));
    assert_eq!(file(&head, "new ünicode.txt").counts, Some((2, 0)));
    assert_eq!(file(&head, "deleted.txt").counts, Some((0, 1)));
    assert_eq!(file(&head, "new name.txt").previous_path.as_deref(), Some("old name.txt"));
    assert_eq!(file(&head, "new name.txt").status, 'R');
    assert_eq!(file(&head, "binary.dat").counts, None);
    let changed = contents(&head, "src/change.txt");
    assert!(changed.patch.contains("-shared\n"));
    assert!(changed.patch.contains("+staged\n+working\n"));
    assert_eq!(changed.full_text.as_deref(), Some("base\nstaged\nworking\n"));
    let deleted = contents(&head, "deleted.txt");
    assert!(deleted.deleted);
    assert_eq!(deleted.full_text.as_deref(), Some("deleted baseline\n"));
    assert!(contents(&head, "new name.txt").patch.contains("rename to new name.txt"));
    assert!(contents(&head, "binary.dat").full_text.is_none());

    let staged = git::snapshot(repo.path(), Comparison::Staged).expect("staged comparison");
    assert_eq!(
        staged
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<std::collections::BTreeSet<_>>(),
        ["deleted.txt", "new name.txt", "src/change.txt"].into_iter().collect()
    );
    assert_eq!(file(&staged, "src/change.txt").counts, Some((1, 1)));
    let changed = contents(&staged, "src/change.txt");
    assert_eq!(changed.full_text.as_deref(), Some("base\nstaged\n"));
    assert!(!changed.patch.contains("working"));

    let unstaged = git::snapshot(repo.path(), Comparison::Unstaged).expect("unstaged comparison");
    assert_eq!(
        unstaged
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<std::collections::BTreeSet<_>>(),
        ["binary.dat", "new ünicode.txt", "src/change.txt"].into_iter().collect()
    );
    assert_eq!(file(&unstaged, "src/change.txt").counts, Some((1, 0)));
    let changed = contents(&unstaged, "src/change.txt");
    assert!(changed.patch.contains(" staged\n+working\n"));
    assert!(!changed.patch.contains("-shared"));
    assert_eq!(
        std::fs::read(repo.path().join(".git/index")).expect("index after reads"),
        index_before
    );
    assert_eq!(repo.git(&["--no-optional-locks", "status", "--porcelain=v1", "-z"]), status_before);
    assert_eq!(
        std::fs::read(repo.path().join("src/change.txt")).expect("worktree after reads"),
        b"base\nstaged\nworking\n"
    );
    assert_eq!(
        std::fs::read(repo.path().join("binary.dat")).expect("binary after reads"),
        [0, 1, 2, 255]
    );
}

#[test]
fn unborn_clean_and_non_git_directories_have_distinct_results() {
    let repo = Repo::new();
    repo.write("first.txt", "staged first\n");
    repo.git(&["add", "."]);
    repo.write("first.txt", "working first\n");
    let head = git::snapshot(repo.path(), Comparison::Head).expect("unborn HEAD");
    assert_eq!(file(&head, "first.txt").counts, Some((1, 0)));
    assert_eq!(contents(&head, "first.txt").full_text.as_deref(), Some("working first\n"));
    let staged = git::snapshot(repo.path(), Comparison::Staged).expect("unborn staged");
    assert_eq!(contents(&staged, "first.txt").full_text.as_deref(), Some("staged first\n"));
    repo.commit();
    for comparison in Comparison::ALL {
        assert!(git::snapshot(repo.path(), comparison).expect("clean repo").files.is_empty());
    }
    let outside = tempfile::tempdir().expect("non-Git directory");
    assert!(git::snapshot(outside.path(), Comparison::Head).is_err());
}

fn key(app: &mut App, code: KeyCode) {
    handle_terminal_event(app, Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
}

fn settle(app: &mut App) {
    let state = app.diff_view.as_mut().expect("diff view");
    // Wait for correlated worker responses; no timing sleeps or rendered success text.
    while state.loading.is_some() {
        let (id, result) =
            state.result_rx.recv_timeout(Duration::from_secs(20)).expect("Git response");
        state.apply_read_result(id, result);
    }
}

fn draw(app: &mut App, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
    terminal.draw(|frame| crate::ui::render_fullscreen_surface(frame, app)).expect("draw diff");
    terminal.backend().buffer().clone()
}

fn region(buffer: &Buffer, area: Rect) -> String {
    (area.y..area.bottom())
        .map(|y| {
            let mut text = String::new();
            let mut x = area.x;
            while x < area.right() {
                let symbol = buffer[(x, y)].symbol();
                text.push_str(symbol);
                x += u16::try_from(unicode_width::UnicodeWidthStr::width(symbol).max(1)).unwrap();
            }
            text
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn open_repo(repo: &Repo) -> App {
    let mut app = App::test_default();
    app.cwd_raw = repo.path().to_string_lossy().into_owned();
    app.status = crate::app::AppStatus::Running;
    let (connection, mut commands) = crate::agent::client::AgentConnection::test_channel();
    app.session_runtime.conn = Some(std::rc::Rc::new(connection));
    app.session_runtime.session_id = Some(crate::agent::model::SessionId::new("active-session"));
    app.input.set_text("/diff");
    crate::app::input_submit::submit_input(&mut app);
    assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::Diff));
    assert!(app.input.text().is_empty());
    assert!(commands.try_recv().is_err(), "Git review must not send SDK commands");
    settle(&mut app);
    app
}

#[test]
fn keyboard_flow_selects_only_files_and_scrolls_the_focused_pane() {
    let repo = Repo::new();
    repo.write("src/nested/a.txt", "unchanged context\nfirst old\n");
    repo.write("src/z.txt", "second old\n");
    repo.commit();
    let long_file = format!(
        "unchanged context\n{}",
        (0..80).fold(String::new(), |mut text, n| {
            writeln!(text, "first new {n}").expect("fixture text");
            text
        })
    );
    repo.write("src/nested/a.txt", &long_file);
    repo.write("src/z.txt", "second new\n");
    let mut app = open_repo(&repo);
    let (conn, mut commands) = crate::agent::client::AgentConnection::test_channel();
    app.session_runtime.conn = Some(std::rc::Rc::new(conn));
    let buffer = draw(&mut app, 120, 20);
    let left = region(&buffer, Rect::new(1, 3, 38, 13));
    assert!(left.contains("src/") && left.contains("nested/"));
    let selected_y = (3..16)
        .find(|y| region(&buffer, Rect::new(1, *y, 38, 1)).contains("a.txt"))
        .expect("file row");
    assert_ne!(buffer[(5, selected_y)].bg, buffer[(5, selected_y - 1)].bg);
    assert!(region(&buffer, Rect::new(41, 3, 78, 13)).contains("first new 0"));
    assert!(!region(&buffer, Rect::new(41, 3, 78, 13)).contains("second new"));

    for (code, modifiers, path, body) in [
        (KeyCode::Tab, KeyModifiers::NONE, "src/z.txt", "second new"),
        (KeyCode::Tab, KeyModifiers::SHIFT, "src/nested/a.txt", "first new 0"),
        (KeyCode::BackTab, KeyModifiers::NONE, "src/z.txt", "second new"),
        (KeyCode::Tab, KeyModifiers::NONE, "src/nested/a.txt", "first new 0"),
    ] {
        handle_terminal_event(&mut app, Event::Key(KeyEvent::new(code, modifiers)));
        settle(&mut app);
        assert_view(&mut app, path, body, "UNEXPECTED");
    }
    key(&mut app, KeyCode::Down);
    settle(&mut app);
    assert_eq!(app.diff_view.as_ref().unwrap().selected_file().unwrap().path, "src/z.txt");
    let buffer = draw(&mut app, 120, 20);
    assert!(region(&buffer, Rect::new(41, 3, 78, 13)).contains("second new"));
    key(&mut app, KeyCode::Up);
    settle(&mut app);
    key(&mut app, KeyCode::Right);
    draw(&mut app, 120, 20);
    for _ in 0..14 {
        key(&mut app, KeyCode::Down);
    }
    let scrolled = draw(&mut app, 120, 20);
    assert!(region(&scrolled, Rect::new(41, 3, 78, 13)).contains("first new 20"));
    assert!(!region(&scrolled, Rect::new(41, 3, 78, 13)).contains("first new 0"));
    assert_eq!(app.diff_view.as_ref().unwrap().selected_file().unwrap().path, "src/nested/a.txt");
    key(&mut app, KeyCode::Tab);
    settle(&mut app);
    assert!(region(&draw(&mut app, 120, 20), Rect::new(41, 3, 78, 13)).contains("second new"));
    assert_eq!(app.diff_view.as_ref().unwrap().selected_file().unwrap().path, "src/z.txt");
    key(&mut app, KeyCode::BackTab);
    settle(&mut app);
    key(&mut app, KeyCode::Char('v'));
    let full = draw(&mut app, 120, 20);
    assert!(region(&full, Rect::new(41, 3, 78, 13)).contains("unchanged context"));
    assert!(!region(&full, Rect::new(41, 3, 78, 13)).contains("@@"));
    for _ in 0..90 {
        key(&mut app, KeyCode::Down);
    }
    let end = draw(&mut app, 120, 20);
    assert!(region(&end, Rect::new(41, 3, 78, 13)).contains("first new 79"));
    key(&mut app, KeyCode::Left);
    key(&mut app, KeyCode::Down);
    settle(&mut app);
    assert_eq!(app.diff_view.as_ref().unwrap().selected_file().unwrap().path, "src/z.txt");
    key(&mut app, KeyCode::Esc);
    assert_eq!(app.surface_mode, SurfaceMode::Chat);
    assert!(app.diff_view.is_none());
    assert_eq!(app.status, crate::app::AppStatus::Running);
    assert!(commands.try_recv().is_err());
}

#[test]
fn full_file_mode_preserves_nested_indentation_after_resize() {
    let repo = Repo::new();
    let source = "impl Example {\n    fn run() {\n        alpha_beta_gamma_delta_epsilon_zeta_eta_theta_iota_END();\n    }\n}";
    repo.write("src/example.rs", source);
    let mut app = open_repo(&repo);
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Char('v'));
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    for width in [120, 60, 120] {
        terminal.backend_mut().resize(width, 30);
        redraw(&mut terminal, &mut app);
        let b = terminal.backend().buffer();
        let area = content_area(b);
        let text = region(b, area);
        let rows = text.lines().map(str::trim_end).collect::<Vec<_>>();
        let starts = rows
            .iter()
            .enumerate()
            .filter_map(|(i, r)| r.split_whitespace().next()?.parse::<usize>().ok().map(|n| (i, n)))
            .collect::<Vec<_>>();
        assert_eq!(starts.iter().map(|(_, n)| *n).collect::<Vec<_>>(), [1, 2, 3, 4, 5]);
        let gutter = rows[starts[0].0].find("impl").unwrap();
        for (i, expected) in source.lines().enumerate() {
            let start = starts[i].0;
            let end = starts.get(i + 1).map_or(rows.len(), |r| r.0);
            let mut recovered = String::new();
            for (offset, row) in rows[start..end].iter().enumerate() {
                if row.is_empty() {
                    continue;
                }
                if offset > 0 {
                    assert!(row[..gutter].chars().all(char::is_whitespace));
                }
                recovered.push_str(&row[gutter..]);
            }
            assert_eq!(recovered, expected, "width {width}, source line {}", i + 1);
        }
        if width == 60 {
            assert!(starts[3].0 > starts[2].0 + 1);
        } else {
            assert_eq!(starts[3].0, starts[2].0 + 1);
        }
        assert!(region(b, Rect::new(area.x, 2, area.width, 1)).contains("src/example.rs"));
    }
}

#[test]
fn comparison_selection_and_refresh_keep_the_selected_file_and_show_the_selected_version() {
    let repo = Repo::new();
    repo.write("b.txt", "base\n");
    repo.commit();
    repo.write("b.txt", "staged\n");
    repo.git(&["add", "."]);
    repo.write("b.txt", "working\n");
    let mut app = open_repo(&repo);
    key(&mut app, KeyCode::Char('s'));
    key(&mut app, KeyCode::Down);
    assert!(region(&draw(&mut app, 100, 18), Rect::new(0, 0, 100, 18)).contains("> Staged:"));
    key(&mut app, KeyCode::Enter);
    settle(&mut app);
    key(&mut app, KeyCode::Char('v'));
    assert_view(&mut app, "b.txt", "staged", "working");
    let before = app.diff_view.as_ref().unwrap().request_id;
    key(&mut app, KeyCode::Char('s'));
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Esc);
    assert_eq!(
        app.diff_view.as_ref().unwrap().request_id,
        before,
        "cancel must not request another comparison"
    );
    assert_view(&mut app, "b.txt", "staged", "working");
    let b = draw(&mut app, 100, 18);
    assert!(region(&b, Rect::new(0, 0, 100, 2)).contains("Staged"));
    repo.write("a.txt", "new staged file\n");
    repo.git(&["add", "a.txt"]);
    key(&mut app, KeyCode::Char('r'));
    settle(&mut app);
    assert_view(&mut app, "b.txt", "staged", "working");
    repo.write("c.txt", "another staged file\n");
    repo.git(&["add", "c.txt"]);
    repo.git(&["reset", "-q", "HEAD", "--", "b.txt"]);
    key(&mut app, KeyCode::Char('r'));
    key(&mut app, KeyCode::Down);
    settle(&mut app);
    assert_view(&mut app, "c.txt", "another staged file", "staged\nworking");
    repo.git(&["reset", "-q", "HEAD", "--", "c.txt"]);
    key(&mut app, KeyCode::Char('r'));
    settle(&mut app);
    assert_view(&mut app, "a.txt", "new staged file", "another staged file");
    assert_content_focused_successor();
}

#[test]
fn a_superseded_file_read_cannot_replace_the_current_selection() {
    for kind in 0..4 {
        let repo = Repo::new();
        repo.write("a.txt", "first\n");
        repo.write("b.txt", "second\n");
        let mut app = open_repo(&repo);
        let stale_id = app.diff_view.as_ref().unwrap().request_id;
        key(&mut app, KeyCode::Down);
        let snapshot = git::snapshot(repo.path(), Comparison::Head).unwrap();
        let stale = match kind {
            0 => ReadResult::Content(Ok(FileContent {
                patch: "STALE body".into(),
                full_text: Some("STALE body".into()),
                notice: None,
                deleted: false,
            })),
            1 => ReadResult::Files(Ok(snapshot)),
            2 => ReadResult::Files(Err("STALE files failure".into())),
            _ => ReadResult::Content(Err("STALE content failure".into())),
        };
        let current = app.diff_view.as_ref().unwrap().request_id;
        assert!(!app.diff_view.as_mut().unwrap().apply_read_result(stale_id, stale));
        assert_eq!(app.diff_view.as_ref().unwrap().request_id, current);
        let b = draw(&mut app, 100, 18);
        let body = region(&b, content_area(&b));
        assert!(body.contains("Loading"));
        assert!(!body.contains("STALE"));
        assert_eq!(app.diff_view.as_ref().unwrap().selected_file().unwrap().path, "b.txt");
        assert!(app.diff_view.as_ref().unwrap().error.is_none());
        settle(&mut app);
        assert_view(&mut app, "b.txt", "second", "STALE");
    }
}

#[test]
fn empty_binary_and_failed_file_reads_are_visible_and_refresh_can_recover() {
    let repo = Repo::new();
    let mut app = open_repo(&repo);
    let buffer = draw(&mut app, 100, 18);
    assert!(region(&buffer, Rect::new(34, 3, 65, 11)).contains("No changes"));
    repo.write("a.dat", [0, 1, 255]);
    repo.write("b.txt", "readable\n");
    key(&mut app, KeyCode::Char('r'));
    settle(&mut app);
    key(&mut app, KeyCode::Char('v'));
    let buffer = draw(&mut app, 100, 18);
    assert!(region(&buffer, Rect::new(34, 3, 65, 11)).contains("Binary file"));
    std::fs::remove_file(repo.path().join("b.txt")).expect("remove before selection");
    key(&mut app, KeyCode::Down);
    settle(&mut app);
    let buffer = draw(&mut app, 100, 18);
    let content = region(&buffer, Rect::new(34, 3, 65, 11));
    assert!(content.contains("Cannot read") && content.contains("b.txt"));
    repo.write("b.txt", "recovered\n");
    key(&mut app, KeyCode::Char('r'));
    settle(&mut app);
    let buffer = draw(&mut app, 100, 18);
    assert!(region(&buffer, Rect::new(34, 3, 65, 11)).contains("recovered"));
    // Ctrl+C must close even while content or the comparison picker owns focus.
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Char('s'));
    handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
    );
    assert_eq!(app.surface_mode, SurfaceMode::Chat);
    assert!(app.diff_view.is_none());
    assert!(!app.shutdown_requested());
}

#[test]
fn a_short_tree_keeps_the_selected_file_and_its_statistics_visible_at_narrow_widths() {
    let repo = Repo::new();
    for n in 1..=15 {
        repo.write(&format!("nested/{n:02}name.txt"), format!("F{n:02}\n").repeat(n));
    }
    let mut app = open_repo(&repo);
    for _ in 0..14 {
        key(&mut app, KeyCode::Down);
    }
    settle(&mut app);
    for (width, height) in [(48, 14), (60, 14), (120, 20)] {
        for (code, name, count, marker) in
            [(KeyCode::Up, "14", "+14", "F14"), (KeyCode::Down, "15", "+15", "F15")]
        {
            key(&mut app, code);
            settle(&mut app);
            let b = draw(&mut app, width, height);
            let tree = tree_area(&b);
            let y = find_row(&b, tree, name);
            let row = region(&b, Rect::new(tree.x, y, tree.width, 1));
            assert!(row.contains(count), "{row}");
            assert!(row.trim_start().starts_with(&format!("? {name}")), "{row}");
            let other = (tree.y..tree.bottom())
                .find(|r| {
                    *r != y
                        && region(&b, Rect::new(tree.x, *r, tree.width, 1))
                            .trim_start()
                            .starts_with('?')
                })
                .unwrap();
            assert_ne!(b[(tree.x, y)].bg, b[(tree.x, other)].bg);
            assert!(region(&b, content_area(&b)).contains(marker));
            assert_eq!(
                app.diff_view.as_ref().unwrap().selected_file().unwrap().path,
                format!("nested/{name}name.txt")
            );
        }
    }
}

#[test]
fn sidebar_counts_show_only_nonzero_changes_in_their_colors_even_when_selected() {
    use ratatui::style::Color;

    let repo = Repo::new();
    repo.write("b-removed.txt", "one\ntwo\nthree\n");
    repo.write("c-mixed.txt", "before\n");
    repo.write("old.txt", "same contents\n");
    repo.commit();
    repo.write("a-\u{754c}.txt", "one\ntwo\n");
    std::fs::remove_file(repo.path().join("b-removed.txt")).expect("deleted fixture");
    repo.write("c-mixed.txt", "after\n");
    repo.git(&["mv", "old.txt", "d-renamed.txt"]);
    repo.write("e-binary.dat", [0, 1, 255]);
    repo.write("f-empty.txt", "");
    let mut app = open_repo(&repo);

    // Check every file's own row with additions selected, then removals selected.
    for selected_name in ["a-\u{754c}.txt", "b-removed.txt"] {
        let buffer = draw(&mut app, 120, 24);
        for (name, added, removed) in [
            ("a-\u{754c}.txt", "+2", ""),
            ("b-removed.txt", "", "-3"),
            ("c-mixed.txt", "+1", "-1"),
            ("d-renamed.txt", "", ""),
            ("f-empty.txt", "", ""),
        ] {
            let y = (3..20)
                .find(|y| region(&buffer, Rect::new(1, *y, 38, 1)).contains(name))
                .expect("file visible in sidebar");
            let row = region(&buffer, Rect::new(1, y, 38, 1));
            assert!(!row.contains("+0") && !row.contains("-0"), "{name}: {row}");
            for (count, color) in [(added, Color::Green), (removed, Color::Red)] {
                if count.is_empty() {
                    continue;
                }
                let x = cell_text_column(&buffer, y, count, 1..39);
                for col in x..x + u16::try_from(count.len()).expect("count width") {
                    assert_eq!(buffer[(col, y)].fg, color, "{name}: {count}");
                }
            }
            if added.is_empty() && removed.is_empty() {
                assert!(row.trim_end().ends_with(name), "zero-change file has no counts");
            }
            if name == selected_name {
                let other =
                    if name == "a-\u{754c}.txt" { "b-removed.txt" } else { "a-\u{754c}.txt" };
                let other_y = find_row(&buffer, Rect::new(1, 3, 38, 17), other);
                assert_ne!(buffer[(1, y)].bg, buffer[(1, other_y)].bg);
            }
        }
        let binary_y = (3..20)
            .find(|y| region(&buffer, Rect::new(1, *y, 38, 1)).contains("e-binary.dat"))
            .expect("binary row");
        assert!(region(&buffer, Rect::new(1, binary_y, 38, 1)).contains("binary/-"));
        key(&mut app, KeyCode::Down);
        settle(&mut app);
    }
}

#[test]
fn footer_uses_one_row_when_it_fits_and_wraps_without_losing_controls() {
    let repo = Repo::new();
    repo.write("file.txt", "changed line\n");
    let mut app = open_repo(&repo);
    for width in [200, 48] {
        let b = draw(&mut app, width, 18);
        let footer_y = find_row(&b, b.area, "Up/Down");
        let footer = region(&b, Rect::new(0, footer_y, width, 18 - footer_y));
        let flattened = footer.split_whitespace().collect::<Vec<_>>().join(" ");
        for hint in [
            "Up/Down navigate",
            "Enter/Right read",
            "Left/Esc back",
            "Tab/Shift+Tab files",
            "v diff/full file",
            "s comparison",
            "r refresh",
            "[ prev hunk",
            "] next hunk",
            "Ctrl+Q quit",
        ] {
            assert!(flattened.contains(hint), "{hint}: {flattened}");
        }
        let occupied = footer.lines().filter(|r| !r.trim().is_empty()).count();
        if width == 200 {
            assert_eq!(occupied, 1);
        } else {
            assert!(occupied > 1);
        }
        let body = content_area(&b);
        assert!(body.bottom() < footer_y);
        assert!(region(&b, body).contains("changed line"));
        assert!(region(&b, tree_area(&b)).contains("file.txt"));
        assert!(!region(&b, body).contains("Ctrl+Q"));
    }
}

#[test]
fn hunk_navigation_accepts_layout_modifiers_and_preserves_file_line_numbers_and_selection() {
    let repo = Repo::new();
    let original = (1..=90).fold(String::new(), |mut text, line| {
        if line == 67 {
            writeln!(text).expect("blank context line");
        } else {
            writeln!(text, "original line {line}").expect("fixture text");
        }
        text
    });
    repo.write("hunks.txt", &original);
    repo.commit();
    repo.write(
        "hunks.txt",
        original
            .replace("original line 8\n", "first changed line\n")
            .replace("original line 70\n", "second changed line\n"),
    );
    for modifiers in [
        KeyModifiers::NONE,
        KeyModifiers::SHIFT,
        KeyModifiers::CONTROL | KeyModifiers::ALT,
        KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT,
    ] {
        let mut app = open_repo(&repo);
        draw(&mut app, 120, 16);
        key(&mut app, KeyCode::Right);
        // Ctrl and Alt alone remain shortcuts, not text produced by AltGr.
        for shortcut in [KeyModifiers::CONTROL, KeyModifiers::ALT] {
            handle_terminal_event(
                &mut app,
                Event::Key(KeyEvent::new(KeyCode::Char(']'), shortcut)),
            );
            let unchanged = draw(&mut app, 120, 16);
            let content = region(&unchanged, Rect::new(41, 3, 78, 9));
            assert!(content.contains("first changed line"));
            assert!(!content.contains("second changed line"));
        }
        handle_terminal_event(&mut app, Event::Key(KeyEvent::new(KeyCode::Char(']'), modifiers)));
        let second = draw(&mut app, 120, 16);
        let content = region(&second, Rect::new(41, 3, 78, 9));
        let changed_row = content
            .lines()
            .find(|line| line.contains("second changed line"))
            .expect("second hunk visible with bracket's layout modifiers");
        assert!(changed_row.trim_start().starts_with("70"));
        assert!(!content.contains("first changed line"));
        handle_terminal_event(&mut app, Event::Key(KeyEvent::new(KeyCode::Char('['), modifiers)));
        let first = draw(&mut app, 120, 16);
        assert!(region(&first, Rect::new(41, 3, 78, 9)).contains("first changed line"));
        assert_eq!(app.diff_view.as_ref().unwrap().selected_file().unwrap().path, "hunks.txt");
    }
}

#[test]
fn hunk_navigation_reaches_later_hunks_even_when_the_entire_diff_fits_on_screen() {
    let repo = Repo::new();
    let original = (1..=120).fold(String::new(), |mut text, line| {
        writeln!(text, "original {line}").expect("fixture text");
        text
    });
    repo.write("hunks.txt", &original);
    repo.commit();
    repo.write(
        "hunks.txt",
        original
            .replace("original 10\n", "first edit\n")
            .replace("original 50\n", "middle edit\n")
            .replace("original 100\n", "last edit\n"),
    );
    let mut app = open_repo(&repo);
    let initial = draw(&mut app, 120, 45);
    let content = region(&initial, Rect::new(41, 3, 78, 38));
    for edit in ["first edit", "middle edit", "last edit"] {
        assert!(content.contains(edit), "all hunks initially fit: {edit}");
    }
    key(&mut app, KeyCode::Right);
    for (code, visible, hidden) in [
        (']', "middle edit", "first edit"),
        (']', "last edit", "middle edit"),
        ('[', "middle edit", "first edit"),
        ('[', "first edit", "last edit"),
    ] {
        key(&mut app, KeyCode::Char(code));
        let jumped = draw(&mut app, 120, 45);
        assert!(region(&jumped, Rect::new(41, 3, 78, 1)).contains("@@"));
        let top_hunk = region(&jumped, Rect::new(41, 3, 78, 8));
        assert!(top_hunk.contains(visible));
        assert!(!top_hunk.contains(hidden));
        assert_eq!(app.diff_view.as_ref().unwrap().selected_file().unwrap().path, "hunks.txt");
    }
}

#[test]
fn ctrl_q_quits_from_the_diff_but_altgr_q_does_not() {
    let repo = Repo::new();
    let mut app = open_repo(&repo);
    handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL | KeyModifiers::ALT)),
    );
    assert!(!app.shutdown_requested());
    assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::Diff));
    handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL)),
    );
    assert!(app.shutdown_requested());
}

#[test]
fn an_unmerged_index_is_not_presented_as_a_deleted_staged_file() {
    use std::io::Write;
    let repo = Repo::new();
    repo.write("conflict.txt", "base\n");
    repo.commit();
    let oid = String::from_utf8(repo.git(&["rev-parse", "HEAD:conflict.txt"])).expect("blob ID");
    let oid = oid.trim();
    let entries = format!(
        "0 {}\tconflict.txt\n100644 {oid} 1\tconflict.txt\n100644 {oid} 2\tconflict.txt\n100644 {oid} 3\tconflict.txt\n",
        "0".repeat(oid.len())
    );
    let mut child = Command::new("git")
        .current_dir(repo.path())
        .args(["update-index", "--index-info"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("seed unmerged index");
    child
        .stdin
        .take()
        .expect("index input")
        .write_all(entries.as_bytes())
        .expect("write index stages");
    assert!(child.wait_with_output().expect("index response").status.success());
    repo.write("conflict.txt", "<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> other\n");
    for comparison in [Comparison::Staged, Comparison::Unstaged] {
        let snapshot = git::snapshot(repo.path(), comparison).expect("unmerged comparison");
        assert_eq!(file(&snapshot, "conflict.txt").status, 'U');
        let content = contents(&snapshot, "conflict.txt");
        assert!(!content.deleted);
        assert!(content.full_text.as_deref().unwrap().contains("<<<<<<< HEAD"));
        assert!(content.notice.as_deref().unwrap().contains("Unmerged"));
    }
}

#[test]
fn refresh_preserves_the_untracked_copy_when_a_tracked_deletion_has_the_same_path() {
    let repo = Repo::new();
    repo.write("file.txt", "committed contents\n");
    repo.commit();
    repo.git(&["rm", "--cached", "file.txt"]);
    repo.write("file.txt", "untracked contents\n");
    let mut app = open_repo(&repo);
    key(&mut app, KeyCode::Tab);
    settle(&mut app);
    key(&mut app, KeyCode::Char('v'));
    key(&mut app, KeyCode::Char('r'));
    settle(&mut app);
    let buffer = draw(&mut app, 100, 18);
    assert_eq!(app.diff_view.as_ref().unwrap().selected_file().unwrap().status, '?');
    let content = region(&buffer, Rect::new(34, 3, 65, 11));
    assert!(content.contains("untracked contents"));
    assert!(!content.contains("committed contents"));
    let summary = region(&buffer, Rect::new(0, 0, 100, 1));
    assert!(summary.contains("| 1 file "), "{summary}");
    let tree = region(&buffer, tree_area(&buffer));
    assert!(tree.lines().any(|r| r.contains("D file.txt")));
    assert!(tree.lines().any(|r| r.contains("? file.txt")));
}

#[test]
fn refreshing_after_all_changes_are_removed_clears_the_preview_and_keeps_tree_focus() {
    let repo = Repo::new();
    repo.write("file.txt", "baseline\n");
    repo.commit();
    repo.write("file.txt", "temporary change\n");
    let mut app = open_repo(&repo);
    let mut terminal = Terminal::new(TestBackend::new(100, 18)).unwrap();
    redraw(&mut terminal, &mut app);
    let changed = terminal.backend().buffer().clone();
    assert!(region(&changed, Rect::new(34, 3, 65, 11)).contains("temporary change"));
    repo.write("file.txt", "baseline\n");
    key(&mut app, KeyCode::Char('r'));
    settle(&mut app);
    redraw(&mut terminal, &mut app);
    let clean = terminal.backend().buffer().clone();
    let state = app.diff_view.as_ref().unwrap();

    assert!(state.selected_file().is_none());
    assert!(region(&clean, Rect::new(1, 3, 31, 11)).contains("No changes"));
    let content = region(&clean, Rect::new(34, 3, 65, 11));
    assert!(content.contains("No changes"));
    assert!(!content.contains("temporary change"));
    assert!(!region(&clean, tree_area(&clean)).contains("file.txt"));
    // Navigation on an empty tree must remain harmless and Esc must still close.
    for code in [KeyCode::Up, KeyCode::Down, KeyCode::Tab, KeyCode::BackTab] {
        key(&mut app, code);
    }
    key(&mut app, KeyCode::Esc);
    assert_eq!(app.surface_mode, SurfaceMode::Chat);
    assert!(app.diff_view.is_none());
}

// Test-only subprocess isolation also applies inside the real Git reader thread.
pub(super) fn isolate_git(command: &mut Command) {
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            command.env_remove(key);
        }
    }
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", if cfg!(windows) { "NUL" } else { "/dev/null" });
}
fn redraw(terminal: &mut Terminal<TestBackend>, app: &mut App) {
    terminal.draw(|f| crate::ui::render_fullscreen_surface(f, app)).unwrap();
}
fn find_row(b: &Buffer, area: Rect, text: &str) -> u16 {
    (area.y..area.bottom())
        .find(|y| region(b, Rect::new(area.x, *y, area.width, 1)).contains(text))
        .unwrap_or_else(|| panic!("missing {text}: {}", region(b, area)))
}
fn content_area(b: &Buffer) -> Rect {
    let y = find_row(b, b.area, "Files");
    let x = (0..b.area.width)
        .find(|x| b[(*x, y)].symbol() == "D" || b[(*x, y)].symbol() == "F" && *x > 12)
        .unwrap();
    // Locate the content title's word, then its adjacent left border and inner column.
    let border = x - 2;
    let footer = find_row(b, b.area, "Up/Down");
    Rect::new(border + 1, y + 1, b.area.width - border - 2, footer - y - 2)
}
fn tree_area(b: &Buffer) -> Rect {
    let a = content_area(b);
    Rect::new(1, a.y, a.x - 3, a.height)
}
fn cell_text_column(b: &Buffer, y: u16, text: &str, range: std::ops::Range<u16>) -> u16 {
    range
        .clone()
        .find(|x| region(b, Rect::new(*x, y, range.end - *x, 1)).starts_with(text))
        .unwrap()
}
fn assert_view(app: &mut App, path: &str, shown: &str, hidden: &str) {
    let b = draw(app, 100, 20);
    let body = content_area(&b);
    assert!(region(&b, Rect::new(body.x, 2, body.width, 1)).contains(path));
    assert!(region(&b, body).contains(shown), "{}", region(&b, body));
    assert!(!region(&b, body).contains(hidden));
    assert_eq!(app.diff_view.as_ref().unwrap().selected_file().unwrap().path, path);
}
fn assert_content_focused_successor() {
    let repo = Repo::new();
    for name in ["a", "b", "c", "d"] {
        repo.write(&format!("{name}.txt"), format!("base {name}\n"));
    }
    repo.commit();
    for name in ["a", "b", "c", "d"] {
        repo.write(
            &format!("{name}.txt"),
            (0..40).fold(String::new(), |mut text, n| {
                writeln!(text, "{name} ROW_{n:02}").unwrap();
                text
            }),
        );
    }
    let mut app = open_repo(&repo);
    key(&mut app, KeyCode::Down);
    settle(&mut app);
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Char('v'));
    assert_view(&mut app, "b.txt", "b ROW_00", "c ROW_00");
    repo.write("b.txt", "base b\n");
    key(&mut app, KeyCode::Char('r'));
    key(&mut app, KeyCode::Down);
    settle(&mut app);
    assert_view(&mut app, "c.txt", "c ROW_01", "b ROW_00");
    for _ in 0..15 {
        key(&mut app, KeyCode::Down);
    }
    assert_view(&mut app, "c.txt", "c ROW_20", "c ROW_00");
}
#[test]
fn reader_transport_failures_clear_loading_and_reopening_recovers() {
    for fail_request in [true, false] {
        let repo = Repo::new();
        repo.write("recover.txt", "RECOVERED_SOURCE\n");
        let mut app = open_repo(&repo);
        let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
        redraw(&mut terminal, &mut app);
        let (tx, rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let state = app.diff_view.as_mut().unwrap();
        state.request_tx = Some(tx);
        state.result_rx = result_rx;
        if fail_request {
            drop(rx);
            key(&mut app, KeyCode::Char('r'));
        } else {
            key(&mut app, KeyCode::Char('r'));
            assert!(rx.try_recv().is_ok());
            drop(result_tx);
            poll(&mut app);
        }
        redraw(&mut terminal, &mut app);
        let b = terminal.backend().buffer();
        let body = region(b, content_area(b));
        assert!(body.contains("Git reader stopped"));
        assert!(body.contains("reopen"));
        assert!(!body.contains("Loading") && !body.contains("RECOVERED_SOURCE"));
        assert!(app.diff_view.as_ref().unwrap().loading.is_none());
        key(&mut app, KeyCode::Right);
        key(&mut app, KeyCode::Esc);
        key(&mut app, KeyCode::Esc);
        assert_eq!(app.surface_mode, SurfaceMode::Chat);
        open(&mut app);
        settle(&mut app);
        assert_view(&mut app, "recover.txt", "RECOVERED_SOURCE", "Git reader stopped");
    }
}
#[test]
fn git_special_paths_select_only_the_literal_change() {
    let repo = Repo::new();
    for (path, body) in [
        ("file[1].txt", "bracket old\n"),
        ("file1.txt", "counterpart old\n"),
        ("-option.txt", "option old\n"),
        ("old[2].txt", "rename preserved\n"),
        ("old2.txt", "other old\n"),
    ] {
        repo.write(path, body);
    }
    repo.commit();
    for (path, body) in [
        ("file[1].txt", "BRACKET_NEW\nSECOND\n"),
        ("file1.txt", "COUNTERPART_NEW\n"),
        ("-option.txt", "OPTION_NEW\n"),
        ("old2.txt", "OTHER_NEW\n"),
    ] {
        repo.write(path, body);
    }
    repo.git(&["mv", "--", "old[2].txt", "new[2].txt"]);
    let snapshot = git::snapshot(repo.path(), Comparison::Head).unwrap();
    for (path, body, counts, other) in [
        ("file[1].txt", "BRACKET_NEW\nSECOND\n", (2, 1), "COUNTERPART_NEW"),
        ("file1.txt", "COUNTERPART_NEW\n", (1, 1), "BRACKET_NEW"),
        ("-option.txt", "OPTION_NEW\n", (1, 1), "BRACKET_NEW"),
        ("old2.txt", "OTHER_NEW\n", (1, 1), "rename preserved"),
    ] {
        let content = contents(&snapshot, path);
        assert_eq!(content.full_text.as_deref(), Some(body));
        assert_eq!(file(&snapshot, path).counts, Some(counts));
        assert!(content.patch.contains(body.lines().next().unwrap()));
        assert!(!content.patch.contains(other));
    }
    let rename = contents(&snapshot, "new[2].txt");
    assert_eq!(rename.full_text.as_deref(), Some("rename preserved\n"));
    assert!(rename.patch.contains("rename from old[2].txt"));
    assert!(rename.patch.contains("rename to new[2].txt"));
    assert!(!rename.patch.contains("OTHER_NEW"));
    assert_eq!(file(&snapshot, "new[2].txt").counts, Some((0, 0)));
}
#[test]
fn gitlink_and_invalid_utf8_have_distinct_previews_and_allow_navigation() {
    let repo = Repo::new();
    repo.write("base.txt", "base\n");
    repo.commit();
    let first = String::from_utf8(repo.git(&["rev-parse", "HEAD"])).unwrap();
    repo.git(&["update-index", "--add", "--cacheinfo", &format!("160000,{},module", first.trim())]);
    repo.git(&["-c", "core.hooksPath=", "commit", "-qm", "local gitlink"]);
    let second = String::from_utf8(repo.git(&["rev-parse", "HEAD"])).unwrap();
    repo.git(&["update-index", "--cacheinfo", &format!("160000,{},module", second.trim())]);
    let staged = git::snapshot(repo.path(), Comparison::Staged).unwrap();
    let link = contents(&staged, "module");
    assert!(link.full_text.is_none());
    assert!(link.notice.as_deref().unwrap().contains("Submodule"));
    assert!(link.patch.contains(&format!("-Subproject commit {}", first.trim())));
    assert!(link.patch.contains(&format!("+Subproject commit {}", second.trim())));
    repo.write("a-invalid.txt", [0x61, 0xff, 0x62]);
    repo.write("b-readable.txt", "READABLE_SOURCE\n");
    let mut app = open_repo(&repo);
    key(&mut app, KeyCode::Char('v'));
    assert_view(&mut app, "a-invalid.txt", "not UTF-8", "READABLE_SOURCE");
    key(&mut app, KeyCode::Down);
    settle(&mut app);
    assert_view(&mut app, "b-readable.txt", "READABLE_SOURCE", "not UTF-8");
}
#[test]
fn symbolic_link_preview_shows_the_target_path_without_dereferencing() {
    let repo = Repo::new();
    repo.write("target.txt", "TARGET_CONTENT_MUST_NOT_LEAK\n");
    repo.commit();
    #[cfg(unix)]
    let result = std::os::unix::fs::symlink("target.txt", repo.path().join("link.txt"));
    #[cfg(windows)]
    let result = std::os::windows::fs::symlink_file("target.txt", repo.path().join("link.txt"));
    if let Err(error) = result {
        #[cfg(windows)]
        if error.raw_os_error() == Some(1314) {
            eprintln!(
                "A64 UNAVAILABLE: Windows symbolic-link creation requires a privilege not held by this process"
            );
            return;
        }
        panic!("cannot create symbolic-link fixture: {error}");
    }
    let snapshot = git::snapshot(repo.path(), Comparison::Head).unwrap();
    let link = contents(&snapshot, "link.txt");
    assert_eq!(link.full_text.as_deref(), Some("target.txt"));
    assert!(link.patch.contains("+target.txt"));
    assert!(!link.patch.contains("TARGET_CONTENT"));
    repo.git(&["add", "--", "link.txt"]);
    let staged = git::snapshot(repo.path(), Comparison::Staged).unwrap();
    assert_eq!(contents(&staged, "link.txt").full_text.as_deref(), Some("target.txt"));
}

#[test]
fn diff_and_full_file_restore_independent_reading_positions() {
    let repo = Repo::new();
    let old = (1..=110).fold(String::new(), |mut s, n| {
        writeln!(s, "SOURCE_{n:03}").unwrap();
        s
    });
    repo.write("positions.txt", &old);
    repo.commit();
    repo.write(
        "positions.txt",
        old.replace("SOURCE_010", "EARLY_EDIT").replace("SOURCE_090", "LATE_EDIT"),
    );
    let mut app = open_repo(&repo);
    draw(&mut app, 100, 20);
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Char(']'));
    let b = draw(&mut app, 100, 20);
    let diff_body = region(&b, content_area(&b));
    assert!(diff_body.contains("LATE_EDIT"));
    assert!(!diff_body.contains("EARLY_EDIT"));
    key(&mut app, KeyCode::Char('v'));
    for _ in 0..25 {
        key(&mut app, KeyCode::Down);
    }
    let b = draw(&mut app, 100, 20);
    let full_body = region(&b, content_area(&b));
    assert!(full_body.lines().next().unwrap().contains("SOURCE_026"));
    assert!(!full_body.contains("@@"));
    for _ in 0..3 {
        for code in ['[', ']'] {
            key(&mut app, KeyCode::Char(code));
            let b = draw(&mut app, 100, 20);
            assert_eq!(region(&b, content_area(&b)), full_body);
        }
        key(&mut app, KeyCode::Char('v'));
        let b = draw(&mut app, 100, 20);
        assert_eq!(region(&b, content_area(&b)), diff_body);
        key(&mut app, KeyCode::Char('v'));
        let b = draw(&mut app, 100, 20);
        assert_eq!(region(&b, content_area(&b)), full_body);
        assert_eq!(app.diff_view.as_ref().unwrap().selected_file().unwrap().path, "positions.txt");
    }
}
#[test]
fn undersized_git_view_recovers_selection_comparison_and_navigation() {
    let repo = Repo::new();
    repo.write("a.txt", "A_BASE\n");
    repo.write("b.txt", "B_BASE\n");
    repo.commit();
    repo.write("a.txt", "A_SELECTED\n");
    repo.write("b.txt", "B_SELECTED\n");
    repo.git(&["add", "."]);
    for (width, height, quit) in [(47, 24, false), (100, 9, true)] {
        let mut app = open_repo(&repo);
        key(&mut app, KeyCode::Down);
        settle(&mut app);
        key(&mut app, KeyCode::Char('s'));
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Enter);
        settle(&mut app);
        key(&mut app, KeyCode::Char('v'));
        key(&mut app, KeyCode::Right);
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        redraw(&mut terminal, &mut app);
        let before = terminal.backend().buffer();
        assert!(region(before, content_area(before)).contains("B_SELECTED"));
        terminal.backend_mut().resize(width, height);
        redraw(&mut terminal, &mut app);
        let b = terminal.backend().buffer();
        let compact = region(b, b.area);
        assert!(compact.contains("Resize"));
        assert!(compact.contains("Esc back/close"));
        assert!(compact.contains("Ctrl+Q quit"));
        assert!(
            !compact.contains("B_SELECTED")
                && !compact.contains("b.txt")
                && !compact.contains("A_SELECTED")
        );
        terminal.backend_mut().resize(100, 24);
        redraw(&mut terminal, &mut app);
        let b = terminal.backend().buffer();
        assert!(region(b, Rect::new(0, 0, 100, 2)).contains("Staged"));
        assert!(region(b, content_area(b)).contains("B_SELECTED"));
        assert!(
            region(b, Rect::new(content_area(b).x, 2, content_area(b).width, 1))
                .contains("Full file")
        );
        key(&mut app, KeyCode::Tab);
        settle(&mut app);
        assert_view(&mut app, "a.txt", "A_SELECTED", "B_SELECTED");
        key(&mut app, KeyCode::BackTab);
        settle(&mut app);
        assert_view(&mut app, "b.txt", "B_SELECTED", "A_SELECTED");
        terminal.backend_mut().resize(width, height);
        redraw(&mut terminal, &mut app);
        if quit {
            handle_terminal_event(
                &mut app,
                Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL)),
            );
            assert!(app.shutdown_requested());
        } else {
            key(&mut app, KeyCode::Esc);
            assert!(is_active(&app));
            key(&mut app, KeyCode::Esc);
            assert_eq!(app.surface_mode, SurfaceMode::Chat);
            assert!(app.diff_view.is_none());
        }
    }
}
