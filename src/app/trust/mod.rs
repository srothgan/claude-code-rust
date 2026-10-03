// SPDX-License-Identifier: Apache-2.0
pub(crate) mod store;

use super::App;
use super::view::{self, FullscreenView};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrustStatus {
    #[default]
    Trusted,
    Untrusted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrustSelection {
    #[default]
    Yes,
    No,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrustState {
    pub status: TrustStatus,
    pub selection: TrustSelection,
    pub project_key: String,
    pub last_error: Option<String>,
    pub preferences_path: Option<std::path::PathBuf>,
}

impl TrustState {
    #[must_use]
    pub fn is_trusted(&self) -> bool {
        matches!(self.status, TrustStatus::Trusted)
    }
}

pub fn initialize(app: &mut App) {
    refresh(app);
    if app.trust.is_trusted() {
        app.startup.request_connection();
    }
    if app.trust.is_trusted() {
        view::set_surface_mode(app, super::update_prompt::post_trust_surface(app));
    } else {
        view::set_fullscreen_view(app, FullscreenView::Trusted);
    }
}

pub(crate) fn refresh(app: &mut App) {
    let path = app.trust.preferences_path.clone().or_else(|| {
        crate::claude_paths::ClaudePaths::resolve(app.settings_home_override.as_deref())
            .map(|paths| paths.preferences)
    });
    let result = path
        .as_deref()
        .ok_or_else(|| "Trust preferences path is not available".to_owned())
        .and_then(store::read_document);
    app.trust.preferences_path = path;
    match result {
        Ok(document) => {
            let lookup = store::read_status(&document, std::path::Path::new(&app.cwd_raw));
            app.trust.project_key = lookup.project_key;
            app.trust.status =
                if lookup.trusted { TrustStatus::Trusted } else { TrustStatus::Untrusted };
            app.trust.last_error = None;
        }
        Err(error) => {
            app.trust.status = TrustStatus::Untrusted;
            app.trust.last_error = Some(error);
        }
    }
    app.trust.selection = TrustSelection::Yes;
}

pub fn handle_key(app: &mut App, key: KeyEvent) {
    if is_ctrl_shortcut(key, 'q') {
        app.request_shutdown();
        return;
    }

    match (key.code, key.modifiers) {
        (KeyCode::Up, KeyModifiers::NONE) => app.trust.selection = TrustSelection::Yes,
        (KeyCode::Down, KeyModifiers::NONE) => app.trust.selection = TrustSelection::No,
        (KeyCode::Enter, KeyModifiers::NONE) => activate_selection(app),
        (KeyCode::Char('y' | 'Y'), KeyModifiers::NONE) => {
            app.trust.selection = TrustSelection::Yes;
            activate_selection(app);
        }
        (KeyCode::Esc | KeyCode::Char('n' | 'N'), KeyModifiers::NONE) => {
            app.trust.selection = TrustSelection::No;
            activate_selection(app);
        }
        _ => {}
    }
}

pub fn accept(app: &mut App) -> Result<(), String> {
    let path = app
        .trust
        .preferences_path
        .as_ref()
        .ok_or_else(|| "Trust preferences path is not available".to_owned())?;
    let project_key = store::accept_at(path, std::path::Path::new(&app.cwd_raw))?;
    app.trust.project_key = project_key;
    app.trust.status = TrustStatus::Trusted;
    app.trust.last_error = None;
    app.startup.request_connection();
    view::set_surface_mode(app, super::update_prompt::post_trust_surface(app));
    Ok(())
}

pub fn decline(app: &mut App) {
    app.request_shutdown();
}

fn activate_selection(app: &mut App) {
    match app.trust.selection {
        TrustSelection::Yes => {
            if let Err(err) = accept(app) {
                app.trust.last_error = Some(err);
            }
        }
        TrustSelection::No => decline(app),
    }
}

fn is_ctrl_shortcut(key: KeyEvent, ch: char) -> bool {
    matches!(key.code, KeyCode::Char(candidate) if candidate == ch)
        && key.modifiers == KeyModifiers::CONTROL
}

#[cfg(test)]
mod tests;
