// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::app::{App, FullscreenView, SurfaceMode, UpdatePromptAction, UpdatePromptState};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
    let buffer = terminal.backend().buffer();
    let width = usize::from(buffer.area.width);
    buffer
        .content
        .chunks(width)
        .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn fullscreen_dispatch_renders_the_selected_views_own_data_and_controls() {
    use crate::app::{ConfigTab, RecentSessionInfo};
    for view in [
        FullscreenView::Config,
        FullscreenView::Trusted,
        FullscreenView::SessionPicker,
        FullscreenView::Update,
    ] {
        let mut app = App::test_default();
        app.config.active_tab = ConfigTab::Status;
        app.cwd = "/config-cedar".into();
        app.session_runtime.session_id =
            Some(crate::agent::model::SessionId::new("config-session-42"));
        app.trust.project_key = "/trust-orchid".into();
        app.recent_sessions = vec![RecentSessionInfo {
            session_id: "picker-elm-73".into(),
            custom_title: Some("Elm investigation".into()),
            summary: "Distinct picker summary".into(),
            first_prompt: None,
            cwd: Some("/picker-elm".into()),
            git_branch: Some("branch-elm".into()),
            file_size_bytes: 1,
            last_modified_ms: 0,
        }];
        crate::app::session_picker::open(&mut app);
        app.update_prompt = Some(UpdatePromptState {
            current_version: "7.2.1".into(),
            latest_version: "9.4.8".into(),
            release_url: "https://example.invalid/release-948".into(),
            install_method: crate::install_method::InstallMethod::Npm,
            selected: UpdatePromptAction::Install,
            last_error: None,
        });
        app.surface_mode = SurfaceMode::Fullscreen(view);
        let mut terminal = Terminal::new(TestBackend::new(90, 26)).unwrap();
        terminal.draw(|frame| render_fullscreen_surface(frame, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        let rows = text.lines().skip(1).take(24).collect::<Vec<_>>();
        match view {
            FullscreenView::Config => {
                assert!(
                    rows.iter()
                        .any(|row| row.contains("Session ID") && row.contains("config-session-42")),
                    "{text}"
                );
                assert!(
                    rows.iter().any(|row| row.contains("cwd") && row.contains("/config-cedar")),
                    "{text}"
                );
            }
            FullscreenView::Trusted => {
                assert!(rows.iter().any(|row| row.contains("/trust-orchid")), "{text}");
                for choice in ["Yes", "No"] {
                    assert!(rows.iter().any(|row| row.contains(choice)), "{text}");
                }
            }
            FullscreenView::SessionPicker => {
                let selected = rows
                    .iter()
                    .position(|row| row.contains("Elm investigation"))
                    .expect("named session");
                assert!(rows[selected].contains('>'), "session must be selectable: {text}");
                assert!(rows[selected + 1].contains("picker-elm-73"), "session metadata: {text}");
                assert!(rows.iter().any(|row| row.contains("Enter to resume")), "{text}");
            }
            FullscreenView::Update => {
                assert!(
                    rows.iter()
                        .any(|row| row.contains("Current version:") && row.contains("7.2.1")),
                    "{text}"
                );
                assert!(
                    rows.iter().any(|row| row.contains("Latest version:") && row.contains("9.4.8")),
                    "{text}"
                );
                assert!(rows.iter().any(|row| row.contains("> Install update")), "{text}");
            }
            _ => unreachable!(),
        }
    }
}
