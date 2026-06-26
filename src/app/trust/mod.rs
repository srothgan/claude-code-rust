pub(crate) mod store;

use super::App;
use super::view::{self, FullscreenView, SurfaceMode};
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

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TrustState {
    pub status: TrustStatus,
    pub selection: TrustSelection,
    pub project_key: String,
    pub last_error: Option<String>,
}

impl TrustState {
    #[must_use]
    pub fn is_trusted(&self) -> bool {
        matches!(self.status, TrustStatus::Trusted)
    }
}

pub fn initialize(app: &mut App) {
    let lookup = store::read_status(
        &app.config.committed_preferences_document,
        std::path::Path::new(&app.cwd_raw),
    );
    app.trust.project_key = lookup.project_key;
    app.trust.status = if lookup.trusted { TrustStatus::Trusted } else { TrustStatus::Untrusted };
    app.trust.selection = TrustSelection::Yes;
    app.trust.last_error = app.config.preferences_path.is_none().then(|| {
        app.config
            .last_error
            .clone()
            .unwrap_or_else(|| "Trust preferences path is not available".to_owned())
    });
    app.startup_connection_requested = app.trust.is_trusted();
    if app.trust.is_trusted() {
        let next_view = if app.startup_session_picker_requested {
            SurfaceMode::Fullscreen(FullscreenView::SessionPicker)
        } else {
            SurfaceMode::Chat
        };
        view::set_surface_mode(app, next_view);
    } else {
        view::set_fullscreen_view(app, FullscreenView::Trusted);
    }
}

pub fn handle_key(app: &mut App, key: KeyEvent) {
    if is_ctrl_shortcut(key, 'q') || is_ctrl_shortcut(key, 'c') {
        app.should_quit = true;
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
    let Some(path) = app.config.preferences_path.clone() else {
        return Err("Trust preferences path is not available".to_owned());
    };

    let mut next_document = app.config.committed_preferences_document.clone();
    app.trust.project_key =
        store::set_trusted(&mut next_document, std::path::Path::new(&app.cwd_raw));
    crate::app::config::store::save(&path, &next_document)?;

    app.config.committed_preferences_document = next_document;
    app.trust.status = TrustStatus::Trusted;
    app.trust.last_error = None;
    app.startup_connection_requested = true;
    let next_view = if app.startup_session_picker_requested {
        SurfaceMode::Fullscreen(FullscreenView::SessionPicker)
    } else {
        SurfaceMode::Chat
    };
    view::set_surface_mode(app, next_view);
    Ok(())
}

pub fn decline(app: &mut App) {
    app.should_quit = true;
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
