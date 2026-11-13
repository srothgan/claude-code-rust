// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::app::trust_test_support::{choices, path_region, row_text};
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};

fn draw(path: &str, error: Option<&str>, width: u16, height: u16) -> Buffer {
    let mut app = App::test_default();
    app.trust.project_key = path.into();
    app.trust.last_error = error.map(str::to_owned);
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    terminal.backend().buffer().clone()
}

#[test]
fn prompt_identifies_the_directory_with_separate_readable_controls() {
    for width in [100, 24] {
        let buffer = draw("/team/project", None, width, 24);
        let (path_rows, path) = path_region(&buffer);
        assert_eq!(path, "/team/project");
        let (yes, no) = choices(&buffer);
        assert!(path_rows.iter().all(|y| *y < yes));
        assert!(no < buffer.area.height - 1);
        let question = (1..path_rows[0])
            .map(|y| row_text(&buffer, y, 2, width - 2).trim().to_owned())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(question.contains("Trust this project directory?"), "{question}");
    }
}

#[test]
fn fitting_and_wrapped_paths_preserve_meaningful_characters() {
    for (path, width) in [
        ("/work/project", 70),
        ("C:/demo/Client Projects/some service/checkout", 40),
        ("/012345678901234567890123456789  Client/Project ", 40),
        (
            "/Users/someone/Github/a-rather-long-organisation-name/client-projects/deeply-nested-service-checkout",
            40,
        ),
    ] {
        let buffer = draw(path, None, width, 24);
        let (rows, actual) = path_region(&buffer);
        assert_eq!(actual, path);
        let (yes, _) = choices(&buffer);
        assert!(rows.iter().all(|y| *y < yes));
    }
}

#[test]
fn clipped_paths_keep_identifiable_ascii_and_wide_character_tails() {
    let ascii = "/Users/someone/Github/a-rather-long-organisation-name/client-projects/deeply-nested-service-checkout";
    let wide = "/Users/田中/Github/プロジェクト一覧/顧客向け-サービス/チェックアウト画面v2";
    for (path, width, height, suffix) in [
        (ascii, 40, 7, "service-checkout"),
        (ascii, 20, 5, "checkout"),
        (ascii, 30, 5, "service-checkout"),
        (ascii, 44, 5, "service-checkout"),
        (wide, 35, 6, "画面v2"),
        (wide, 37, 6, "画面v2"),
        (wide, 39, 6, "画面v2"),
        (wide, 41, 6, "画面v2"),
    ] {
        let buffer = draw(path, None, width, height);
        let (rows, actual) = path_region(&buffer);
        let tail = actual.strip_prefix('…').unwrap();
        assert!(tail.ends_with(suffix));
        assert!(path.ends_with(tail));
        let (yes, _) = choices(&buffer);
        assert!(!rows.is_empty());
        assert!(rows.iter().all(|y| *y < yes));
    }
}

#[test]
fn narrow_unlabelled_paths_retain_the_directory_identity() {
    for (path, width, height, expected) in [
        ("/long/organization/project42", 12, 5, "…oject42"),
        ("/long/organization/画面v2", 12, 5, "…/画面v2"),
        ("/短", 12, 5, "/短"),
    ] {
        let buffer = draw(path, None, width, height);
        let (rows, actual) = path_region(&buffer);
        assert_eq!(actual, expected);
        let (yes, _) = choices(&buffer);
        assert!(rows.iter().all(|y| *y < yes));
        assert!((1..yes).all(|y| !row_text(&buffer, y, 2, width - 2).contains("Path:")));
    }
}

#[test]
fn error_regions_preserve_the_cause_and_take_priority_over_path_and_hint() {
    let path = "/organization/long/Client Projects/deeply-nested-service-checkout";
    let error = "Save denied: Client  Project /protected/file.json";
    for (width, height, clipped, path_visible) in
        [(100, 16, false, true), (40, 8, false, true), (40, 6, false, false), (40, 5, true, false)]
    {
        let buffer = draw(path, Some(error), width, height);
        let (yes, _) = choices(&buffer);
        let rows =
            (1..yes).filter(|&y| buffer[(2, y)].fg == theme::STATUS_ERROR).collect::<Vec<_>>();
        assert!(!rows.is_empty());
        let mut actual = String::new();
        for &y in &rows {
            let end =
                (2..width - 2).rev().find(|&x| buffer[(x, y)].fg == theme::STATUS_ERROR).unwrap()
                    + 1;
            actual.push_str(&row_text(&buffer, y, 2, end));
        }
        if clipped {
            let prefix = actual.strip_suffix('…').unwrap();
            assert!(prefix.starts_with("Save denied:"));
            assert!(error.starts_with(prefix));
        } else {
            assert_eq!(actual, error);
        }
        let (path_rows, shown) = path_region(&buffer);
        assert_eq!(!path_rows.is_empty(), path_visible);
        if path_visible {
            assert!(path.ends_with(shown.trim_start_matches('…')));
            assert!(path_rows.iter().all(|y| !rows.contains(y)));
        }
        assert!((1..yes).all(|y| !row_text(&buffer, y, 2, width - 2).contains("Choose Yes")));
    }
}

#[test]
fn selected_choice_moves_its_marker_and_highlight_without_leaving_stale_selection() {
    let mut terminal = Terminal::new(TestBackend::new(70, 16)).unwrap();
    let mut app = App::test_default();
    app.trust.project_key = "/work/project".into();
    for selection in [TrustSelection::Yes, TrustSelection::No, TrustSelection::Yes] {
        app.trust.selection = selection;
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        let (yes, no) = choices(buffer);
        let selected = if selection == TrustSelection::Yes { yes } else { no };
        let other = if selected == yes { no } else { yes };
        assert_eq!(buffer[(2, selected)].symbol(), ">");
        assert_eq!(buffer[(2, other)].symbol(), " ");
        assert_ne!(buffer[(4, selected)].bg, buffer[(4, other)].bg);
    }
}
