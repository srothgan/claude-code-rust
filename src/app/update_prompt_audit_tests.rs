// SPDX-License-Identifier: Apache-2.0
use super::*;
use clap::Parser;

fn fixture(picker: bool) -> (tempfile::TempDir, App) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.json");
    std::fs::write(&path,r#"{"updates":{"future":42,"skip_until_unix_secs":9999999999},"presentation":{"copyFullResponse":true}}"#).unwrap();
    let mut app = App::test_default();
    let cli = if picker {
        crate::Cli::parse_from(["claude-rs", "--resume"])
    } else {
        crate::Cli::parse_from(["claude-rs"])
    };
    app.startup = crate::app::state::StartupState::from_cli(&cli);
    app.global_settings = settings::load_from_path(&path).unwrap();
    settings::record_update_check_result(
        &mut app.global_settings,
        "1.0.0",
        "99.0.0",
        "https://example.invalid/selected",
        100,
    );
    app.global_settings_path = Some(path);
    app.input.set_text("draft survives update choices");
    app.update_prompt = Some(UpdatePromptState {
        current_version: "1.0.0".into(),
        latest_version: "99.0.0".into(),
        release_url: "https://example.invalid/selected".into(),
        install_method: InstallMethod::Npm,
        selected: UpdatePromptAction::Install,
        last_error: None,
    });
    app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::Update);
    (directory, app)
}
fn key(app: &mut App, code: KeyCode) {
    handle_key(app, KeyEvent::new(code, KeyModifiers::NONE));
}
fn select(app: &mut App, action: UpdatePromptAction) {
    for _ in 0..10 {
        if app.update_prompt.as_ref().unwrap().selected == action {
            return;
        }
        key(app, KeyCode::Down);
        assert!(app.post_exit_action.is_none());
        assert!(!app.shutdown_requested());
    }
    panic!("action was not reachable by keyboard");
}
fn continued(app: &App, picker: bool) {
    assert!(app.update_prompt.is_none());
    assert_eq!(
        app.surface_mode,
        if picker {
            SurfaceMode::Fullscreen(FullscreenView::SessionPicker)
        } else {
            SurfaceMode::Chat
        }
    );
    assert_eq!(app.input.text(), "draft survives update choices");
    assert!(app.post_exit_action.is_none());
    assert!(!app.shutdown_requested());
    let document: serde_json::Value =
        serde_json::from_slice(&std::fs::read(app.global_settings_path.as_ref().unwrap()).unwrap())
            .unwrap();
    assert_eq!(document["updates"]["future"], 42);
    assert_eq!(document["presentation"]["copyFullResponse"], true);
}
#[test]
fn keyboard_activation_hands_off_each_selected_install_method_and_exact_release() {
    let directory = std::path::PathBuf::from("chosen install directory");
    for (method, choice, expected) in [
        (InstallMethod::Npm, UpdatePromptAction::Install, InstallMethod::Npm),
        (
            InstallMethod::Script { install_dir: Some(directory.clone()) },
            UpdatePromptAction::Install,
            InstallMethod::Script { install_dir: Some(directory) },
        ),
        (
            InstallMethod::Unknown,
            UpdatePromptAction::InstallScript,
            InstallMethod::Script { install_dir: None },
        ),
        (InstallMethod::Unknown, UpdatePromptAction::InstallNpm, InstallMethod::Npm),
    ] {
        let (_dir, mut app) = fixture(false);
        let prompt = app.update_prompt.as_mut().unwrap();
        prompt.install_method = method.clone();
        prompt.selected = if method == InstallMethod::Unknown {
            UpdatePromptAction::InstallScript
        } else {
            UpdatePromptAction::Install
        };
        assert!(app.post_exit_action.is_none());
        if method == InstallMethod::Unknown {
            key(&mut app, KeyCode::Down);
            key(&mut app, KeyCode::Up);
        }
        select(&mut app, choice);
        assert!(!app.shutdown_requested());
        assert!(app.post_exit_action.is_none());
        key(&mut app, KeyCode::Enter);
        assert!(app.shutdown_requested());
        assert_eq!(
            app.post_exit_action,
            Some(PostExitAction::InstallUpdate {
                latest_version: "99.0.0".into(),
                method: expected
            })
        );
    }
}
#[test]
fn escape_and_skip_now_persist_six_hours_and_resume_the_requested_surface() {
    for picker in [false, true] {
        for escape in [false, true] {
            let (_dir, mut app) = fixture(picker);
            let before = super::super::update_check::unix_now_secs().unwrap();
            if escape {
                key(&mut app, KeyCode::Esc);
            } else {
                select(&mut app, UpdatePromptAction::SkipNow);
                key(&mut app, KeyCode::Enter);
            }
            let after = super::super::update_check::unix_now_secs().unwrap();
            continued(&app, picker);
            let saved =
                settings::load_from_path(app.global_settings_path.as_ref().unwrap()).unwrap();
            let deadline = saved.updates.skip_until_unix_secs.unwrap();
            assert!((before + 21_600..=after + 21_600).contains(&deadline));
        }
    }
}
#[test]
fn skip_version_persists_the_displayed_release_and_allows_a_later_release() {
    let (_dir, mut app) = fixture(true);
    select(&mut app, UpdatePromptAction::SkipVersion);
    key(&mut app, KeyCode::Enter);
    continued(&app, true);
    let mut saved = settings::load_from_path(app.global_settings_path.as_ref().unwrap()).unwrap();
    assert_eq!(saved.updates.skipped_version.as_deref(), Some("99.0.0"));
    assert_eq!(saved.updates.skip_until_unix_secs, None);
    assert_eq!(settings::update_prompt_candidate(&saved, "1.0.0", 101), None);
    settings::record_update_check_result(&mut saved, "1.0.0", "100.0.0", "later-url", 102);
    assert_eq!(
        settings::update_prompt_candidate(&saved, "1.0.0", 103),
        Some(UpdatePrompt {
            current_version: "1.0.0".into(),
            latest_version: "100.0.0".into(),
            release_url: "later-url".into(),
            last_error: None
        })
    );
}
#[test]
fn release_notes_open_the_selected_url_and_recover_without_leaving_the_prompt() {
    let (_dir, mut app) = fixture(false);
    select(&mut app, UpdatePromptAction::ReleaseNotes);
    let before = std::fs::read(app.global_settings_path.as_ref().unwrap()).unwrap();
    let mut observed = Vec::new();
    for fail in [true, false] {
        handle_key_with_opener(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            |url| {
                observed.push(url.to_owned());
                if fail { Err("fixture opener refused".into()) } else { Ok(()) }
            },
        );
        let prompt = app.update_prompt.as_ref().unwrap();
        assert_eq!(prompt.selected, UpdatePromptAction::ReleaseNotes);
        if fail {
            let error = prompt.last_error.as_ref().unwrap();
            assert!(error.contains("fixture opener refused"));
            assert!(error.contains("https://example.invalid/selected"));
        } else {
            assert_eq!(prompt.last_error, None);
        }
        assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::Update));
        assert_eq!(app.input.text(), "draft survives update choices");
        assert!(app.post_exit_action.is_none());
        assert!(!app.shutdown_requested());
        assert_eq!(std::fs::read(app.global_settings_path.as_ref().unwrap()).unwrap(), before);
    }
    assert_eq!(observed, vec!["https://example.invalid/selected"; 2]);
}
