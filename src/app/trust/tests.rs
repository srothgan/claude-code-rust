// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::app::{FullscreenView, SurfaceMode};
use serde_json::json;

#[test]
fn initialize_routes_untrusted_projects_to_trusted_view() {
    let mut app = App::test_default();
    app.cwd_raw = if cfg!(windows) {
        r"C:\work\project".to_owned()
    } else {
        "/home/user/work/project".to_owned()
    };
    app.config.preferences_path = Some(std::path::PathBuf::from("prefs.json"));
    app.config.committed_preferences_document = json!({
        "projects": {}
    });

    initialize(&mut app);

    assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::Trusted));
    assert!(!app.is_project_trusted());
    assert_eq!(app.trust.selection, TrustSelection::Yes);
    assert!(!app.startup.mark_connection_started());
}

#[test]
fn initialize_allows_trusted_projects_into_chat() {
    let project_path = if cfg!(windows) { "C:/work/project" } else { "/home/user/work/project" };

    let mut app = App::test_default();
    app.cwd_raw = project_path.to_owned();
    app.config.preferences_path = Some(std::path::PathBuf::from("prefs.json"));
    let mut prefs = json!({ "projects": {} });
    prefs["projects"][project_path] = json!({
        "hasTrustDialogAccepted": true
    });
    app.config.committed_preferences_document = prefs;

    initialize(&mut app);

    assert_eq!(app.surface_mode, SurfaceMode::Chat);
    assert!(app.is_project_trusted());
    assert!(app.startup.mark_connection_started());
}

#[test]
fn accept_persists_trust_and_switches_to_chat() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(".claude.json");
    std::fs::write(&path, "{\n  \"projects\": {}\n}\n").expect("write");

    let mut app = App::test_default();
    app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::Trusted);
    app.cwd_raw = dir.path().join("project").to_string_lossy().to_string();
    app.config.preferences_path = Some(path.clone());
    app.trust.status = TrustStatus::Untrusted;
    app.trust.project_key = store::normalize_project_key(std::path::Path::new(&app.cwd_raw));

    accept(&mut app).expect("accept");

    let raw = std::fs::read_to_string(path).expect("read");
    assert!(raw.contains("\"hasTrustDialogAccepted\": true"));
    assert_eq!(app.surface_mode, SurfaceMode::Chat);
    assert!(app.is_project_trusted());
    assert!(app.startup.mark_connection_started());
}

#[test]
fn initialize_routes_trusted_resume_picker_startup_to_picker_view() {
    let project_path = if cfg!(windows) { "C:/work/project" } else { "/home/user/work/project" };

    let mut app = App::test_default();
    app.cwd_raw = project_path.to_owned();
    app.startup = crate::app::state::StartupState::new(None, None, true);
    app.config.preferences_path = Some(std::path::PathBuf::from("prefs.json"));
    let mut prefs = json!({ "projects": {} });
    prefs["projects"][project_path] = json!({
        "hasTrustDialogAccepted": true
    });
    app.config.committed_preferences_document = prefs;

    initialize(&mut app);

    assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::SessionPicker));
    assert!(app.startup.mark_connection_started());
}

#[test]
fn accept_routes_resume_picker_startup_to_picker_view() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(".claude.json");
    std::fs::write(&path, "{\n  \"projects\": {}\n}\n").expect("write");

    let mut app = App::test_default();
    app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::Trusted);
    app.startup = crate::app::state::StartupState::new(None, None, true);
    app.cwd_raw = dir.path().join("project").to_string_lossy().to_string();
    app.config.preferences_path = Some(path);
    app.trust.status = TrustStatus::Untrusted;
    app.trust.project_key = store::normalize_project_key(std::path::Path::new(&app.cwd_raw));

    accept(&mut app).expect("accept");

    assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::SessionPicker));
    assert!(app.startup.mark_connection_started());
}

#[test]
fn accept_routes_update_prompt_before_resume_picker() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(".claude.json");
    std::fs::write(&path, "{\n  \"projects\": {}\n}\n").expect("write");

    let mut app = App::test_default();
    app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::Trusted);
    app.startup = crate::app::state::StartupState::new(None, None, true);
    app.cwd_raw = dir.path().join("project").to_string_lossy().to_string();
    app.config.preferences_path = Some(path);
    app.trust.status = TrustStatus::Untrusted;
    app.trust.project_key = store::normalize_project_key(std::path::Path::new(&app.cwd_raw));
    app.update_prompt = Some(crate::app::UpdatePromptState {
        current_version: "0.13.4".to_owned(),
        latest_version: "0.14.0".to_owned(),
        release_url: "https://example.invalid".to_owned(),
        install_method: crate::install_method::InstallMethod::Npm,
        selected: crate::app::UpdatePromptAction::Install,
        last_error: None,
    });

    accept(&mut app).expect("accept");

    assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::Update));
    assert!(app.startup.mark_connection_started());
}

#[test]
fn handle_key_declines_with_n() {
    let mut app = App::test_default();
    app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::Trusted);

    handle_key(&mut app, KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));

    assert!(app.shutdown_requested());
}

#[test]
fn handle_key_moves_selection_with_up_and_down() {
    let mut app = App::test_default();
    app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::Trusted);
    app.trust.selection = TrustSelection::Yes;

    handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.trust.selection, TrustSelection::No);

    handle_key(&mut app, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(app.trust.selection, TrustSelection::Yes);
}

#[test]
fn handle_key_enter_declines_when_no_is_selected() {
    let mut app = App::test_default();
    app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::Trusted);
    app.trust.selection = TrustSelection::No;

    handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert!(app.shutdown_requested());
}
