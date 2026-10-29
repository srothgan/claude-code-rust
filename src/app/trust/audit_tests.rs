// SPDX-License-Identifier: Apache-2.0

use super::test_support::{choices, path_region, physical_key, row_text};
use super::*;
use crate::app::{FullscreenView, SurfaceMode};
use serde_json::{Value, json};
use std::path::PathBuf;

struct Fixture {
    directory: tempfile::TempDir,
    project: PathBuf,
    preferences: PathBuf,
    key: String,
}
impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let project = directory.path().join("Client Projects");
        std::fs::create_dir(&project).unwrap();
        let preferences = directory.path().join("profile/.claude.json");
        Self { key: physical_key(&project), project, preferences, directory }
    }
    fn write(&self, document: &Value) {
        std::fs::create_dir_all(self.preferences.parent().unwrap()).unwrap();
        std::fs::write(&self.preferences, serde_json::to_vec(document).unwrap()).unwrap();
    }
    fn app(&self) -> App {
        let mut app = App::test_default();
        app.cwd_raw = self.project.to_string_lossy().into_owned();
        app.trust.preferences_path = Some(self.preferences.clone());
        app.input.set_text("KEEP_DRAFT");
        app
    }
    fn saved(&self) -> Value {
        serde_json::from_slice(&std::fs::read(&self.preferences).unwrap()).unwrap()
    }
    fn expected(&self, original: &Value) -> Value {
        let mut value = original.clone();
        value["projects"][&self.key] = json!({"hasTrustDialogAccepted":true});
        value
    }
}
fn press(app: &mut App, code: KeyCode) {
    handle_key(app, KeyEvent::new(code, KeyModifiers::NONE));
}
fn prompt(app: &mut App) -> ratatui::buffer::Buffer {
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(220, 24)).unwrap();
    terminal.draw(|frame| crate::ui::render_fullscreen_surface(frame, app)).unwrap();
    terminal.backend().buffer().clone()
}
fn intent(app: &mut App, kind: u8) -> SurfaceMode {
    if kind > 0 {
        app.startup =
            crate::app::state::StartupState::new(None, crate::StartupLaunch::SessionPicker);
    }
    if kind == 2 {
        app.update_prompt = Some(crate::app::UpdatePromptState {
            current_version: "0.1.0".into(),
            latest_version: "0.2.0".into(),
            release_url: "https://example.invalid".into(),
            install_method: crate::install_method::InstallMethod::Npm,
            selected: crate::app::UpdatePromptAction::Install,
            last_error: None,
        });
        SurfaceMode::Fullscreen(FullscreenView::Update)
    } else if kind == 1 {
        SurfaceMode::Fullscreen(FullscreenView::SessionPicker)
    } else {
        SurfaceMode::Chat
    }
}
fn assert_blocked(app: &mut App) {
    assert!(!app.is_project_trusted());
    assert!(!app.startup.mark_connection_started());
    assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::Trusted));
}

#[test]
fn saved_trust_controls_read_only_startup_admission_and_launch_intent() {
    for kind in 0..3 {
        for approved in [false, true] {
            let fixture = Fixture::new();
            let original = json!({"account":"keep","projects":{
                &fixture.key:{"hasTrustDialogAccepted":approved},"unrelated":{"hasTrustDialogAccepted":true}
            }});
            fixture.write(&original);
            let bytes = std::fs::read(&fixture.preferences).unwrap();
            let mut app = fixture.app();
            let target = intent(&mut app, kind);
            initialize(&mut app);
            assert_eq!(std::fs::read(&fixture.preferences).unwrap(), bytes);
            assert_eq!(app.trust.project_key, fixture.key);
            if approved {
                assert_eq!(app.surface_mode, target);
                assert!(app.startup.mark_connection_started());
            } else {
                assert_blocked(&mut app);
            }
        }
    }
    let fixture = Fixture::new();
    fixture.write(&json!({"projects":{"other":{"hasTrustDialogAccepted":true}}}));
    let mut app = fixture.app();
    initialize(&mut app);
    assert_blocked(&mut app);
}

#[test]
fn acceptance_keys_persist_only_the_project_before_continuing_startup() {
    for kind in 0..3 {
        for missing in [false, true] {
            let fixture = Fixture::new();
            let original = if missing {
                json!({})
            } else {
                json!({"account":"keep","projects":{"unrelated":{"hasTrustDialogAccepted":false,"tools":["git"]}}})
            };
            if !missing {
                fixture.write(&original);
            }
            let mut app = fixture.app();
            let target = intent(&mut app, kind);
            initialize(&mut app);
            if missing {
                press(&mut app, KeyCode::Down);
                press(&mut app, KeyCode::Up);
                assert!(!fixture.preferences.exists());
                assert!(!app.shutdown_requested());
                assert_blocked(&mut app);
                press(&mut app, KeyCode::Enter);
            } else {
                press(&mut app, KeyCode::Char(if kind == 1 { 'Y' } else { 'y' }));
            }
            assert_eq!(fixture.saved(), fixture.expected(&original));
            assert_eq!(app.surface_mode, target);
            assert!(app.is_project_trusted());
            assert!(app.trust.last_error.is_none());
            assert!(app.startup.mark_connection_started());
            assert_eq!(app.input.text(), "KEEP_DRAFT");
            if kind == 2 {
                app.global_settings_path = Some(fixture.directory.path().join("app-settings.json"));
                crate::app::update_prompt::handle_key(
                    &mut app,
                    KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                );
                assert_eq!(
                    app.surface_mode,
                    SurfaceMode::Fullscreen(FullscreenView::SessionPicker)
                );
                assert!(app.post_exit_action.is_none());
            }
        }
    }
}

#[test]
fn refusal_controls_never_save_approval_or_admit_startup() {
    for route in 0..5 {
        for missing in [false, true] {
            let fixture = Fixture::new();
            let original =
                json!({"projects":{"other":{"hasTrustDialogAccepted":true}},"account":"keep"});
            if !missing {
                fixture.write(&original);
            }
            let bytes = std::fs::read(&fixture.preferences).ok();
            let mut app = fixture.app();
            initialize(&mut app);
            match route {
                0 => press(&mut app, KeyCode::Char('n')),
                1 => press(&mut app, KeyCode::Char('N')),
                2 => press(&mut app, KeyCode::Esc),
                3 => {
                    press(&mut app, KeyCode::Down);
                    assert!(!app.shutdown_requested());
                    assert_blocked(&mut app);
                    press(&mut app, KeyCode::Enter);
                }
                _ => handle_key(&mut app, KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL)),
            }
            assert!(app.shutdown_requested());
            assert_blocked(&mut app);
            assert_eq!(std::fs::read(&fixture.preferences).ok(), bytes);
            assert_eq!(app.input.text(), "KEEP_DRAFT");
        }
    }
}

#[test]
fn malformed_preferences_report_failure_and_allow_repaired_acceptance() {
    for raw in ["{broken", r#"{"projects":42,"account":"keep"}"#] {
        let fixture = Fixture::new();
        fixture.write(&json!({}));
        std::fs::write(&fixture.preferences, raw).unwrap();
        let mut app = fixture.app();
        initialize(&mut app);
        press(&mut app, KeyCode::Enter);
        assert_blocked(&mut app);
        assert!(
            app.trust
                .last_error
                .as_ref()
                .is_some_and(|e| e.contains("trust") || e.contains("Trust"))
        );
        assert!(!app.shutdown_requested());
        assert_eq!(std::fs::read_to_string(&fixture.preferences).unwrap(), raw);
        let repaired = json!({"account":"repaired","projects":{}});
        fixture.write(&repaired);
        press(&mut app, KeyCode::Char('y'));
        assert_eq!(fixture.saved(), fixture.expected(&repaired));
        assert!(app.trust.last_error.is_none());
        assert!(app.startup.mark_connection_started());
    }
}

#[test]
fn failed_atomic_replacement_keeps_trust_blocked_until_a_successful_retry() {
    let fixture = Fixture::new();
    let original =
        json!({"account":"keep","projects":{"unrelated":{"hasTrustDialogAccepted":true}}});
    fixture.write(&original);
    let bytes = std::fs::read(&fixture.preferences).unwrap();
    let mut app = fixture.app();
    initialize(&mut app);
    crate::json_file::with_replacement_failure(&fixture.preferences, || {
        press(&mut app, KeyCode::Enter);
    });
    assert_blocked(&mut app);
    assert_eq!(std::fs::read(&fixture.preferences).unwrap(), bytes);
    let error = app.trust.last_error.as_deref().unwrap();
    assert!(error.contains("save") && error.contains("injected replacement denial"));
    assert!(!app.shutdown_requested());
    assert!(app.post_exit_action.is_none());
    press(&mut app, KeyCode::Enter);
    assert_eq!(fixture.saved(), fixture.expected(&original));
    assert!(app.trust.last_error.is_none());
    assert!(app.is_project_trusted());
    assert_eq!(app.surface_mode, SurfaceMode::Chat);
    assert!(app.startup.mark_connection_started());
}

#[test]
fn invalid_document_refresh_displays_the_current_directory_and_recovers() {
    let fixture = Fixture::new();
    fixture.write(&json!({}));
    let mut app = fixture.app();
    initialize(&mut app);
    assert_eq!(path_region(&prompt(&mut app)).1, fixture.key);
    let next = fixture.directory.path().join("Different Project");
    std::fs::create_dir(&next).unwrap();
    std::fs::write(&fixture.preferences, "{broken").unwrap();
    app.cwd_raw = next.to_string_lossy().into_owned();
    refresh(&mut app);
    let screen = prompt(&mut app);
    assert_eq!(path_region(&screen).1, physical_key(&next));
    let cause = app.trust.last_error.clone().unwrap();
    let (yes, _) = choices(&screen);
    assert!((1..yes).any(|y| row_text(&screen, y, 2, screen.area.width - 2).trim_end() == cause));
    press(&mut app, KeyCode::Enter);
    assert_blocked(&mut app);
    assert_eq!(std::fs::read_to_string(&fixture.preferences).unwrap(), "{broken");
    fixture.write(&json!({"account":"repaired"}));
    refresh(&mut app);
    assert!(app.trust.last_error.is_none());
    assert_eq!(path_region(&prompt(&mut app)).1, physical_key(&next));
    press(&mut app, KeyCode::Enter);
    assert_eq!(
        fixture.saved(),
        json!({"account":"repaired","projects":{physical_key(&next):{"hasTrustDialogAccepted":true}}})
    );
}

#[cfg(unix)]
#[test]
fn symlink_prompt_and_saved_approval_identify_the_resolved_project() {
    let fixture = Fixture::new();
    let link = fixture.directory.path().join("shortcut");
    std::os::unix::fs::symlink(&fixture.project, &link).unwrap();
    let original = json!({"projects":{"other":{"hasTrustDialogAccepted":true,"keep":7}}});
    fixture.write(&original);
    let mut app = fixture.app();
    app.cwd_raw = link.to_string_lossy().into_owned();
    initialize(&mut app);
    assert_eq!(path_region(&prompt(&mut app)).1, fixture.key);
    press(&mut app, KeyCode::Char('y'));
    assert_eq!(fixture.saved(), fixture.expected(&original));
}
