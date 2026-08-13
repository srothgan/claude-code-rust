// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use super::settings;
use super::view::{self, FullscreenView, SurfaceMode};
use super::{App, PostExitAction, UpdatePrompt, UpdatePromptAction, UpdatePromptState};
use crate::install_method::{InstallMethod, detect_install_method};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

impl From<UpdatePrompt> for UpdatePromptState {
    fn from(prompt: UpdatePrompt) -> Self {
        let install_method = detect_install_method();
        let selected = if install_method == InstallMethod::Unknown {
            UpdatePromptAction::InstallScript
        } else {
            UpdatePromptAction::Install
        };
        Self {
            current_version: prompt.current_version,
            latest_version: prompt.latest_version,
            release_url: prompt.release_url,
            install_method,
            selected,
            last_error: prompt.last_error,
        }
    }
}

pub fn post_trust_surface(app: &App) -> SurfaceMode {
    if app.update_prompt.is_some() {
        SurfaceMode::Fullscreen(FullscreenView::Update)
    } else if app.startup.session_picker_requested() {
        SurfaceMode::Fullscreen(FullscreenView::SessionPicker)
    } else {
        SurfaceMode::Chat
    }
}

pub fn handle_key(app: &mut App, key: KeyEvent) {
    if is_ctrl(key, 'q') {
        app.request_shutdown();
        return;
    }

    match (key.code, key.modifiers) {
        (KeyCode::Up | KeyCode::Char('k'), KeyModifiers::NONE) => move_selection(app, -1),
        (KeyCode::Down | KeyCode::Char('j'), KeyModifiers::NONE) => move_selection(app, 1),
        (KeyCode::Enter, KeyModifiers::NONE) => activate_selection(app),
        (KeyCode::Esc, KeyModifiers::NONE) => skip_now(app),
        _ => {}
    }
}

fn move_selection(app: &mut App, delta: isize) {
    let Some(prompt) = app.update_prompt.as_mut() else {
        return;
    };
    let actions = actions_for(&prompt.install_method);
    let current = action_index(actions, prompt.selected);
    let next = current.saturating_add_signed(delta).min(actions.len().saturating_sub(1));
    prompt.selected = actions[next];
}

fn activate_selection(app: &mut App) {
    let Some(selected) = app.update_prompt.as_ref().map(|prompt| prompt.selected) else {
        continue_startup(app);
        return;
    };

    match selected {
        UpdatePromptAction::Install => install(app, None),
        UpdatePromptAction::InstallScript => {
            install(app, Some(InstallMethod::Script { install_dir: None }));
        }
        UpdatePromptAction::InstallNpm => install(app, Some(InstallMethod::Npm)),
        UpdatePromptAction::SkipNow => skip_now(app),
        UpdatePromptAction::SkipVersion => skip_version(app),
        UpdatePromptAction::ReleaseNotes => open_release_notes(app),
    }
}

fn install(app: &mut App, method_override: Option<InstallMethod>) {
    let Some(prompt) = app.update_prompt.as_ref() else {
        return;
    };
    let method = method_override.unwrap_or_else(|| prompt.install_method.clone());
    if method == InstallMethod::Unknown {
        return;
    }
    app.post_exit_action = Some(PostExitAction::InstallUpdate {
        latest_version: prompt.latest_version.clone(),
        method,
    });
    app.request_shutdown();
}

fn skip_now(app: &mut App) {
    let now = super::update_check::unix_now_secs().unwrap_or(0);
    settings::record_skip_now(&mut app.global_settings, now);
    persist_settings(app);
    continue_startup(app);
}

fn skip_version(app: &mut App) {
    let Some(latest_version) =
        app.update_prompt.as_ref().map(|prompt| prompt.latest_version.clone())
    else {
        continue_startup(app);
        return;
    };
    settings::record_skip_version(&mut app.global_settings, &latest_version);
    persist_settings(app);
    continue_startup(app);
}

fn open_release_notes(app: &mut App) {
    let Some(release_url) = app.update_prompt.as_ref().map(|prompt| prompt.release_url.clone())
    else {
        return;
    };
    match crate::app::config::open_url_in_browser(&release_url) {
        Ok(()) => {
            if let Some(prompt) = app.update_prompt.as_mut() {
                prompt.last_error = None;
            }
        }
        Err(err) => {
            if let Some(prompt) = app.update_prompt.as_mut() {
                prompt.last_error = Some(format!("{err}. URL: {release_url}"));
            }
        }
    }
}

fn persist_settings(app: &mut App) {
    let Some(path) = app.global_settings_path.as_ref() else {
        return;
    };
    if let Err(err) = settings::save_global_settings(path, &app.global_settings) {
        tracing::warn!(
            target: crate::logging::targets::APP_UPDATE,
            event_name = "update_prompt_settings_save_failed",
            message = "failed to persist update prompt choice",
            outcome = "failure",
            settings_path = %path.display(),
            error_message = %err,
        );
    }
}

fn continue_startup(app: &mut App) {
    app.update_prompt = None;
    let next = if app.startup.session_picker_requested() {
        SurfaceMode::Fullscreen(FullscreenView::SessionPicker)
    } else {
        SurfaceMode::Chat
    };
    view::set_surface_mode(app, next);
}

fn is_ctrl(key: KeyEvent, ch: char) -> bool {
    matches!(key.code, KeyCode::Char(c) if c == ch) && key.modifiers == KeyModifiers::CONTROL
}

fn action_index(actions: &[UpdatePromptAction], action: UpdatePromptAction) -> usize {
    actions.iter().position(|candidate| *candidate == action).unwrap_or(0)
}

pub(crate) fn actions_for(method: &InstallMethod) -> &'static [UpdatePromptAction] {
    if *method == InstallMethod::Unknown { &UNKNOWN_ACTIONS } else { &KNOWN_ACTIONS }
}

const KNOWN_ACTIONS: [UpdatePromptAction; 4] = [
    UpdatePromptAction::Install,
    UpdatePromptAction::SkipNow,
    UpdatePromptAction::SkipVersion,
    UpdatePromptAction::ReleaseNotes,
];

const UNKNOWN_ACTIONS: [UpdatePromptAction; 5] = [
    UpdatePromptAction::InstallScript,
    UpdatePromptAction::InstallNpm,
    UpdatePromptAction::SkipNow,
    UpdatePromptAction::SkipVersion,
    UpdatePromptAction::ReleaseNotes,
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, FullscreenView, SurfaceMode};

    fn app_with_prompt() -> App {
        let mut app = App::test_default();
        app.update_prompt = Some(UpdatePromptState {
            current_version: "0.13.4".to_owned(),
            latest_version: "0.14.0".to_owned(),
            release_url: "https://example.invalid/v0.14.0".to_owned(),
            install_method: InstallMethod::Npm,
            selected: UpdatePromptAction::Install,
            last_error: None,
        });
        app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::Update);
        app
    }

    #[test]
    fn install_sets_post_exit_action_and_quits() {
        let mut app = app_with_prompt();

        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert!(app.shutdown_requested());
        assert_eq!(
            app.post_exit_action,
            Some(PostExitAction::InstallUpdate {
                latest_version: "0.14.0".to_owned(),
                method: InstallMethod::Npm,
            })
        );
    }

    #[test]
    fn unknown_layout_offers_script_and_npm_install_actions() {
        let mut app = app_with_prompt();
        let prompt = app.update_prompt.as_mut().expect("prompt");
        prompt.install_method = InstallMethod::Unknown;
        prompt.selected = UpdatePromptAction::InstallScript;

        handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(
            app.update_prompt.as_ref().map(|prompt| prompt.selected),
            Some(UpdatePromptAction::InstallNpm)
        );

        handle_key(&mut app, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            app.post_exit_action,
            Some(PostExitAction::InstallUpdate {
                latest_version: "0.14.0".to_owned(),
                method: InstallMethod::Script { install_dir: None },
            })
        );
    }

    #[test]
    fn down_changes_selection() {
        let mut app = app_with_prompt();

        handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));

        assert_eq!(
            app.update_prompt.as_ref().map(|prompt| prompt.selected),
            Some(UpdatePromptAction::SkipNow)
        );
    }

    #[test]
    fn esc_skips_now_and_returns_to_chat() {
        let mut app = app_with_prompt();

        handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        assert_eq!(app.surface_mode, SurfaceMode::Chat);
        assert!(app.update_prompt.is_none());
        assert!(app.global_settings.updates.skip_until_unix_secs.is_some());
    }
}
