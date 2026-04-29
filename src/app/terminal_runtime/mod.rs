// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

mod chat_session;
mod custom_inline_terminal;
mod fullscreen_session;
mod insert_history;
mod modes;
mod panic_hook;
mod screen_scroll;

use self::chat_session::ChatTerminalSession;
use self::fullscreen_session::FullscreenTerminalSession;
use self::modes::{
    apply_actions, chat_startup_actions, enter_fullscreen_actions, exit_fullscreen_actions,
    shutdown_restore_actions,
};
use self::panic_hook::{PanicRestoreHook, restore_once};
use crate::app::{App, FullscreenView, SurfaceMode, TerminalLifecycleState};
use anyhow::{Context, anyhow};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SurfaceSessionKind {
    Chat,
    Fullscreen,
}

enum SurfaceTerminalSession {
    Chat(ChatTerminalSession),
    Fullscreen(FullscreenTerminalSession),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SurfaceTransitionPlan {
    Noop,
    EnterFullscreen { view: FullscreenView },
    RetargetFullscreen { from: FullscreenView, to: FullscreenView },
    ExitFullscreen { from: FullscreenView },
}

pub(crate) struct TerminalRuntime {
    session: Option<SurfaceTerminalSession>,
    active_surface: SurfaceMode,
    alternate_screen_active: Arc<AtomicBool>,
    panic_hook: Option<PanicRestoreHook>,
    restored: Arc<AtomicBool>,
}

impl TerminalRuntime {
    pub(crate) fn bootstrap(app: &mut App) -> anyhow::Result<Self> {
        let target_surface = app.surface_mode;
        let restored = Arc::new(AtomicBool::new(false));
        let alternate_screen_active = Arc::new(AtomicBool::new(false));

        if let Err(err) = apply_startup_actions() {
            restore_once(restored.as_ref(), || {
                let _ = restore_terminal_modes(alternate_screen_active.as_ref());
            });
            return Err(err).context("failed to configure terminal startup modes");
        }

        let session = match target_surface {
            SurfaceMode::Chat => {
                app.reset_committed_output_tracking();
                ChatTerminalSession::new().map(SurfaceTerminalSession::Chat)
            }
            SurfaceMode::Fullscreen(_) => {
                if let Err(err) = apply_enter_fullscreen_actions() {
                    restore_once(restored.as_ref(), || {
                        let _ = restore_terminal_modes(alternate_screen_active.as_ref());
                    });
                    return Err(err).context("failed to enter fullscreen terminal mode");
                }
                alternate_screen_active.store(true, Ordering::SeqCst);
                FullscreenTerminalSession::new().map(SurfaceTerminalSession::Fullscreen)
            }
        };
        let session = match session {
            Ok(session) => session,
            Err(err) => {
                restore_once(restored.as_ref(), || {
                    let _ = restore_terminal_modes(alternate_screen_active.as_ref());
                });
                return Err(err);
            }
        };

        let alternate_for_hook = Arc::clone(&alternate_screen_active);
        let panic_hook = PanicRestoreHook::install(Arc::clone(&restored), move || {
            if let Err(err) = restore_terminal_modes(alternate_for_hook.as_ref()) {
                tracing::warn!(
                    target: crate::logging::targets::APP_LIFECYCLE,
                    event_name = "terminal_restore_failed",
                    message = "failed to restore terminal state",
                    outcome = "failure",
                    error_message = %err,
                );
            }
        });
        app.terminal_lifecycle = TerminalLifecycleState::Running(target_surface);

        Ok(Self {
            session: Some(session),
            active_surface: target_surface,
            alternate_screen_active,
            panic_hook: Some(panic_hook),
            restored,
        })
    }

    pub(crate) fn sync_surface(&mut self, app: &mut App) -> anyhow::Result<()> {
        match plan_surface_transition(self.active_surface, app.surface_mode) {
            SurfaceTransitionPlan::Noop => {}
            SurfaceTransitionPlan::EnterFullscreen { view } => {
                match self.session.take() {
                    Some(SurfaceTerminalSession::Chat(mut session)) => {
                        session.prepare_for_fullscreen(app)?;
                    }
                    Some(SurfaceTerminalSession::Fullscreen(_)) | None => {
                        return Err(anyhow!("chat session missing before fullscreen entry"));
                    }
                }

                apply_enter_fullscreen_actions()
                    .context("failed to enter fullscreen terminal mode")?;
                self.alternate_screen_active.store(true, Ordering::SeqCst);
                app.chat_render.line_wrap_disabled = false;
                self.session =
                    Some(SurfaceTerminalSession::Fullscreen(FullscreenTerminalSession::new()?));
                self.active_surface = SurfaceMode::Fullscreen(view);
                app.terminal_lifecycle = TerminalLifecycleState::Running(self.active_surface);
                app.force_redraw = true;
                app.needs_redraw = true;
                app.surface_dirty.fullscreen.redraw = true;
            }
            SurfaceTransitionPlan::RetargetFullscreen { to, .. } => match self.session {
                Some(SurfaceTerminalSession::Fullscreen(_)) => {
                    self.active_surface = SurfaceMode::Fullscreen(to);
                    app.terminal_lifecycle = TerminalLifecycleState::Running(self.active_surface);
                    app.needs_redraw = true;
                    app.surface_dirty.fullscreen.redraw = true;
                }
                _ => return Err(anyhow!("fullscreen session missing during fullscreen retarget")),
            },
            SurfaceTransitionPlan::ExitFullscreen { .. } => {
                match self.session.take() {
                    Some(SurfaceTerminalSession::Fullscreen(_)) => {}
                    Some(SurfaceTerminalSession::Chat(_)) | None => {
                        return Err(anyhow!("fullscreen session missing before chat return"));
                    }
                }

                apply_exit_fullscreen_actions(&self.alternate_screen_active)
                    .context("failed to exit fullscreen terminal mode")?;
                app.chat_render.line_wrap_disabled = false;
                app.reset_committed_output_tracking();
                self.session = Some(SurfaceTerminalSession::Chat(ChatTerminalSession::new()?));
                self.active_surface = SurfaceMode::Chat;
                app.terminal_lifecycle = TerminalLifecycleState::Running(SurfaceMode::Chat);
                app.force_redraw = true;
                app.needs_redraw = true;
            }
        }

        Ok(())
    }

    pub(crate) fn clear_active_surface_with_app(
        &mut self,
        app: Option<&mut App>,
    ) -> anyhow::Result<()> {
        match self.session_mut()? {
            SurfaceTerminalSession::Chat(session) => {
                let app = app.ok_or_else(|| anyhow!("chat clear requires app state"))?;
                session.clear(app)
            }
            SurfaceTerminalSession::Fullscreen(session) => session.clear(),
        }
    }

    pub(crate) fn draw_active_surface(&mut self, app: &mut App) -> anyhow::Result<()> {
        match self.session_mut()? {
            SurfaceTerminalSession::Chat(session) => session.draw(app),
            SurfaceTerminalSession::Fullscreen(session) => session.draw(app),
        }
    }

    pub(crate) fn restore(&mut self, app: &mut App) {
        if !self.restored.load(Ordering::SeqCst) {
            app.terminal_lifecycle = TerminalLifecycleState::Restoring;
        }

        let _session = self.session.take();
        restore_once(self.restored.as_ref(), || {
            if let Err(err) = restore_terminal_modes(self.alternate_screen_active.as_ref()) {
                tracing::warn!(
                    target: crate::logging::targets::APP_LIFECYCLE,
                    event_name = "terminal_restore_failed",
                    message = "failed to restore terminal state",
                    outcome = "failure",
                    error_message = %err,
                );
            }
        });
        let _hook = self.panic_hook.take();
        app.terminal_lifecycle = TerminalLifecycleState::Exited;
    }

    fn session_mut(&mut self) -> anyhow::Result<&mut SurfaceTerminalSession> {
        self.session.as_mut().ok_or_else(|| anyhow!("terminal runtime has already been restored"))
    }
}

fn apply_startup_actions() -> std::io::Result<()> {
    let mut stdout = std::io::stdout();
    apply_actions(&mut stdout, chat_startup_actions())
}

fn apply_enter_fullscreen_actions() -> std::io::Result<()> {
    let mut stdout = std::io::stdout();
    apply_actions(&mut stdout, enter_fullscreen_actions())
}

fn apply_exit_fullscreen_actions(alternate_screen_active: &AtomicBool) -> std::io::Result<()> {
    if !alternate_screen_active.swap(false, Ordering::SeqCst) {
        return Ok(());
    }

    let mut stdout = std::io::stdout();
    apply_actions(&mut stdout, exit_fullscreen_actions())
}

fn restore_terminal_modes(alternate_screen_active: &AtomicBool) -> std::io::Result<()> {
    let mut first_error = None;

    if let Err(err) = apply_exit_fullscreen_actions(alternate_screen_active)
        && first_error.is_none()
    {
        first_error = Some(err);
    }

    let mut stdout = std::io::stdout();
    if let Err(err) = apply_actions(&mut stdout, shutdown_restore_actions())
        && first_error.is_none()
    {
        first_error = Some(err);
    }

    match first_error {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

#[cfg(test)]
fn session_kind_for_surface(surface: SurfaceMode) -> SurfaceSessionKind {
    match surface {
        SurfaceMode::Chat => SurfaceSessionKind::Chat,
        SurfaceMode::Fullscreen(_) => SurfaceSessionKind::Fullscreen,
    }
}

fn plan_surface_transition(from: SurfaceMode, to: SurfaceMode) -> SurfaceTransitionPlan {
    match (from, to) {
        (SurfaceMode::Chat, SurfaceMode::Fullscreen(view)) => {
            SurfaceTransitionPlan::EnterFullscreen { view }
        }
        (SurfaceMode::Fullscreen(from_view), SurfaceMode::Fullscreen(to_view))
            if from_view != to_view =>
        {
            SurfaceTransitionPlan::RetargetFullscreen { from: from_view, to: to_view }
        }
        (SurfaceMode::Fullscreen(view), SurfaceMode::Chat) => {
            SurfaceTransitionPlan::ExitFullscreen { from: view }
        }
        _ => SurfaceTransitionPlan::Noop,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::terminal_runtime::fullscreen_session::draw_fullscreen_surface_frame;
    use crate::app::view::set_active_view;
    use crate::app::{ActiveView, AppStatus, FullscreenView, SurfaceMode};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn draw_fullscreen_surface_frame_supports_fullscreen_retained_views() {
        let backend = TestBackend::new(100, 40);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = App::test_default();
        app.status = AppStatus::Ready;
        set_active_view(&mut app, ActiveView::Config);

        draw_fullscreen_surface_frame(&mut terminal, &mut app).expect("draw fullscreen view");
    }

    #[test]
    fn session_kind_matches_surface_mode() {
        assert_eq!(session_kind_for_surface(SurfaceMode::Chat), SurfaceSessionKind::Chat);
        assert_eq!(
            session_kind_for_surface(SurfaceMode::Fullscreen(FullscreenView::Trusted)),
            SurfaceSessionKind::Fullscreen
        );
    }

    #[test]
    fn surface_transition_plan_is_noop_for_chat_to_chat() {
        assert_eq!(
            plan_surface_transition(SurfaceMode::Chat, SurfaceMode::Chat),
            SurfaceTransitionPlan::Noop
        );
    }

    #[test]
    fn surface_transition_plan_enters_fullscreen_from_chat() {
        assert_eq!(
            plan_surface_transition(
                SurfaceMode::Chat,
                SurfaceMode::Fullscreen(FullscreenView::Config)
            ),
            SurfaceTransitionPlan::EnterFullscreen { view: FullscreenView::Config }
        );
    }

    #[test]
    fn surface_transition_plan_retargets_fullscreen_without_exit() {
        assert_eq!(
            plan_surface_transition(
                SurfaceMode::Fullscreen(FullscreenView::Config),
                SurfaceMode::Fullscreen(FullscreenView::Trusted)
            ),
            SurfaceTransitionPlan::RetargetFullscreen {
                from: FullscreenView::Config,
                to: FullscreenView::Trusted,
            }
        );
    }

    #[test]
    fn surface_transition_plan_exits_fullscreen_back_to_chat() {
        assert_eq!(
            plan_surface_transition(
                SurfaceMode::Fullscreen(FullscreenView::SessionPicker),
                SurfaceMode::Chat
            ),
            SurfaceTransitionPlan::ExitFullscreen { from: FullscreenView::SessionPicker }
        );
    }
}
