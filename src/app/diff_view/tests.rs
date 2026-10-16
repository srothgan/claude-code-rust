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
        let output = Command::new("git")
            .current_dir(self.path())
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
        staged.files.iter().map(|file| file.path.as_str()).collect::<Vec<_>>(),
        ["deleted.txt", "new name.txt", "src/change.txt"]
    );
    assert_eq!(file(&staged, "src/change.txt").counts, Some((1, 1)));
    let changed = contents(&staged, "src/change.txt");
    assert_eq!(changed.full_text.as_deref(), Some("base\nstaged\n"));
    assert!(!changed.patch.contains("working"));

    let unstaged = git::snapshot(repo.path(), Comparison::Unstaged).expect("unstaged comparison");
    assert_eq!(
        unstaged.files.iter().map(|file| file.path.as_str()).collect::<Vec<_>>(),
        ["binary.dat", "new ünicode.txt", "src/change.txt"]
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
        .map(|y| (area.x..area.right()).map(|x| buffer[(x, y)].symbol()).collect::<String>())
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
    let buffer = draw(&mut app, 120, 20);
    let left = region(&buffer, Rect::new(1, 3, 38, 13));
    assert!(left.contains("src/") && left.contains("nested/"));
    let selected_y = (3..16)
        .find(|y| region(&buffer, Rect::new(1, *y, 38, 1)).contains("a.txt"))
        .expect("file row");
    assert_eq!(buffer[(5, selected_y)].bg, crate::ui::theme::RUST_ORANGE);
    assert!(region(&buffer, Rect::new(41, 3, 78, 13)).contains("first new 0"));
    assert!(!region(&buffer, Rect::new(41, 3, 78, 13)).contains("second new"));

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
    assert!(!region(&scrolled, Rect::new(41, 3, 78, 13)).contains("first new 0"));
    assert_eq!(app.diff_view.as_ref().unwrap().selected_file().unwrap().path, "src/nested/a.txt");
    key(&mut app, KeyCode::Tab);
    settle(&mut app);
    assert!(app.diff_view.as_ref().unwrap().content_focus);
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
}

#[test]
fn full_file_mode_preserves_nested_indentation_after_resize() {
    let repo = Repo::new();
    repo.write(
        "src/example.rs",
        "impl Example {\n    fn run(&self) {\n        if self.ready() {\n            self.work();\n        }\n    }\n}",
    );
    let mut app = open_repo(&repo);
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Char('v'));

    for width in [120, 60, 120] {
        let buffer = draw(&mut app, width, 24);
        let tree_width = (width / 3).clamp(18, 48);
        let content_x = tree_width + 1;
        let content_width = width - tree_width - 2;
        let rows = (3..10)
            .map(|y| {
                region(&buffer, Rect::new(content_x, y, content_width, 1)).trim_end().to_owned()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            rows,
            [
                "1  impl Example {",
                "2      fn run(&self) {",
                "3          if self.ready() {",
                "4              self.work();",
                "5          }",
                "6      }",
                "7  }",
            ],
            "full file source at width {width}"
        );
        assert_eq!(app.diff_view.as_ref().unwrap().selected_file().unwrap().path, "src/example.rs");
        assert!(app.diff_view.as_ref().unwrap().full_file);
        assert!(matches!(buffer[(content_x + 7, 4)].fg, ratatui::style::Color::Rgb(..)));
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
    key(&mut app, KeyCode::Enter);
    settle(&mut app);
    key(&mut app, KeyCode::Char('v'));
    let buffer = draw(&mut app, 100, 18);
    let content = region(&buffer, Rect::new(34, 3, 65, 11));
    assert!(content.contains("staged"));
    assert!(!content.contains("working"));
    key(&mut app, KeyCode::Char('s'));
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Esc);
    assert_eq!(app.diff_view.as_ref().unwrap().comparison, Comparison::Staged);
    repo.write("a.txt", "new staged file\n");
    repo.git(&["add", "a.txt"]);
    key(&mut app, KeyCode::Char('r'));
    settle(&mut app);
    assert_eq!(app.diff_view.as_ref().unwrap().selected_file().unwrap().path, "b.txt");
    assert_eq!(app.diff_view.as_ref().unwrap().selected, 1);
    // Remove the selected staged change. Refresh must select an existing file, not retain a stale row.
    repo.write("c.txt", "another staged file\n");
    repo.git(&["add", "c.txt"]);
    repo.git(&["reset", "-q", "HEAD", "--", "b.txt"]);
    key(&mut app, KeyCode::Char('r'));
    key(&mut app, KeyCode::Down); // No tree rows are selectable while the read is pending.
    settle(&mut app);
    assert_eq!(app.diff_view.as_ref().unwrap().selected_file().unwrap().path, "c.txt");
    repo.git(&["reset", "-q", "HEAD", "--", "c.txt"]);
    key(&mut app, KeyCode::Char('r'));
    settle(&mut app);
    assert_eq!(app.diff_view.as_ref().unwrap().selected_file().unwrap().path, "a.txt");
}

#[test]
fn a_superseded_file_read_cannot_replace_the_current_selection() {
    let repo = Repo::new();
    repo.write("a.txt", "first\n");
    repo.write("b.txt", "second\n");
    let mut app = open_repo(&repo);
    let state = app.diff_view.as_ref().unwrap();
    let stale_id = state.request_id;
    key(&mut app, KeyCode::Down);
    let state = app.diff_view.as_mut().unwrap();
    let accepted = state.apply_read_result(
        stale_id,
        ReadResult::Content(Ok(FileContent {
            patch: "obsolete first file".into(),
            full_text: Some("obsolete first file".into()),
            notice: None,
            deleted: false,
        })),
    );
    assert!(!accepted);
    settle(&mut app);
    let buffer = draw(&mut app, 100, 18);
    let content = region(&buffer, Rect::new(34, 3, 65, 11));
    assert!(content.contains("second"));
    assert!(!content.contains("obsolete first file"));
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
    for index in 0..15 {
        repo.write(&format!("deep/nested/folder/file{index:02}.txt"), "new line\n");
    }
    let mut app = open_repo(&repo);
    for _ in 0..14 {
        key(&mut app, KeyCode::Down);
    }
    settle(&mut app);
    for (width, height) in [(48, 12), (60, 12), (120, 20)] {
        let buffer = draw(&mut app, width, height);
        let tree_width = (width / 3).clamp(18, 48);
        let tree = region(&buffer, Rect::new(1, 3, tree_width - 2, height - 7));
        // The selected row must be onscreen and highlighted; folder rows may scroll away.
        let selected = (3..height - 2)
            .find(|y| buffer[(1, *y)].bg == crate::ui::theme::RUST_ORANGE)
            .expect("visible selected row");
        let selected_text = region(&buffer, Rect::new(1, selected, tree_width - 2, 1));
        assert!(selected_text.contains("+1"));
        assert!(!selected_text.contains("-0"));
        assert!(tree.contains("file"));
        assert!(
            region(&buffer, Rect::new(tree_width + 1, 3, width - tree_width - 2, height - 7))
                .contains("new line")
        );
        assert!(region(&buffer, Rect::new(0, height - 4, width, 4)).contains("v diff/full"));
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
    repo.write("a-added.txt", "one\ntwo\n");
    std::fs::remove_file(repo.path().join("b-removed.txt")).expect("deleted fixture");
    repo.write("c-mixed.txt", "after\n");
    repo.git(&["mv", "old.txt", "d-renamed.txt"]);
    repo.write("e-binary.dat", [0, 1, 255]);
    repo.write("f-empty.txt", "");
    let mut app = open_repo(&repo);

    // Check every file's own row with additions selected, then removals selected.
    for selected_name in ["a-added.txt", "b-removed.txt"] {
        let buffer = draw(&mut app, 120, 24);
        for (name, added, removed) in [
            ("a-added.txt", "+2", ""),
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
                let x = u16::try_from(row.find(count).expect("nonzero count beside its file"))
                    .expect("sidebar column")
                    + 1;
                for col in x..x + u16::try_from(count.len()).expect("count width") {
                    assert_eq!(buffer[(col, y)].fg, color, "{name}: {count}");
                    assert_ne!(buffer[(col, y)].bg, crate::ui::theme::RUST_ORANGE);
                }
            }
            if added.is_empty() && removed.is_empty() {
                assert!(row.trim_end().ends_with(name), "zero-change file has no counts");
            }
            if name == selected_name {
                assert_eq!(buffer[(1, y)].bg, crate::ui::theme::RUST_ORANGE);
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
    let wide = draw(&mut app, 200, 14);
    let footer = region(&wide, Rect::new(0, 13, 200, 1));
    for control in ["Up/Down", "[ prev hunk", "] next hunk", "Ctrl+Q quit"] {
        assert!(footer.contains(control), "all hints fit on one row: {control}");
    }
    assert_eq!(wide[(0, 12)].symbol(), "└", "the file pane uses the row above the footer");

    let narrow = draw(&mut app, 48, 14);
    let pane_bottom =
        (3..13).find(|y| narrow[(0, *y)].symbol() == "└").expect("bottom of file pane");
    assert!(pane_bottom < 12, "narrow terminals need multiple hint rows");
    let footer = (pane_bottom + 1..14)
        .map(|y| region(&narrow, Rect::new(0, y, 48, 1)).trim().to_owned())
        .collect::<Vec<_>>()
        .join(" ");
    for control in ["Up/Down", "[ prev hunk", "] next hunk", "Ctrl+Q quit"] {
        assert!(footer.contains(control), "wrapped footer retains {control}");
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
    assert_eq!(summary.split_whitespace().find_map(|word| word.parse::<usize>().ok()), Some(1));
}

#[test]
fn refreshing_after_all_changes_are_removed_clears_the_preview_and_keeps_tree_focus() {
    let repo = Repo::new();
    repo.write("file.txt", "baseline\n");
    repo.commit();
    repo.write("file.txt", "temporary change\n");
    let mut app = open_repo(&repo);
    let changed = draw(&mut app, 100, 18);
    assert!(region(&changed, Rect::new(34, 3, 65, 11)).contains("temporary change"));
    repo.write("file.txt", "baseline\n");
    key(&mut app, KeyCode::Char('r'));
    settle(&mut app);
    let clean = draw(&mut app, 100, 18);
    let state = app.diff_view.as_ref().unwrap();
    assert!(!state.content_focus);
    assert!(state.selected_file().is_none());
    assert!(region(&clean, Rect::new(1, 3, 31, 11)).contains("No changes"));
    let content = region(&clean, Rect::new(34, 3, 65, 11));
    assert!(content.contains("No changes"));
    assert!(!content.contains("temporary change"));
    // Navigation on an empty tree must remain harmless and Esc must still close.
    for code in [KeyCode::Up, KeyCode::Down, KeyCode::Tab, KeyCode::BackTab] {
        key(&mut app, code);
    }
    key(&mut app, KeyCode::Esc);
    assert_eq!(app.surface_mode, SurfaceMode::Chat);
    assert!(app.diff_view.is_none());
}

#[test]
fn refreshing_an_undone_open_file_selects_its_successor_and_keeps_content_focus() {
    let repo = Repo::new();
    for name in ["a", "b", "c", "d"] {
        repo.write(&format!("{name}.txt"), format!("baseline {name}\n"));
    }
    repo.commit();
    for name in ["a", "b", "c", "d"] {
        repo.write(&format!("{name}.txt"), format!("changed {name}\n"));
    }
    let mut app = open_repo(&repo);
    key(&mut app, KeyCode::Down);
    settle(&mut app);
    key(&mut app, KeyCode::Enter);
    let before = draw(&mut app, 100, 18);
    assert!(region(&before, Rect::new(34, 3, 65, 11)).contains("changed b"));
    repo.write("b.txt", "baseline b\n");
    key(&mut app, KeyCode::Char('r'));
    key(&mut app, KeyCode::Down); // The loading tree has no selectable rows yet.
    settle(&mut app);
    let after = draw(&mut app, 100, 18);
    assert!(app.diff_view.as_ref().unwrap().content_focus);
    let selected_row = (3..14)
        .find(|y| after[(1, *y)].bg == crate::ui::theme::RUST_ORANGE)
        .expect("selected file row");
    assert!(region(&after, Rect::new(1, selected_row, 31, 1)).contains("c.txt"));
    let content = region(&after, Rect::new(34, 3, 65, 11));
    assert!(content.contains("changed c"));
    assert!(!content.contains("changed b"));
    assert_eq!(after[(33, 2)].fg, crate::ui::theme::RUST_ORANGE, "content pane retains focus");
}
