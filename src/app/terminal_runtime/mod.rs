// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

mod chat_session;
mod chat_terminal;
mod fullscreen_session;
mod history_insert;
mod modes;
mod panic_hook;
mod release_guard;

use self::chat_session::ChatTerminalSession;
use self::fullscreen_session::FullscreenTerminalSession;
use self::modes::{
    apply_actions, chat_startup_actions, enter_fullscreen_actions, exit_fullscreen_actions,
    shutdown_restore_actions,
};
use self::panic_hook::{PanicRestoreHook, restore_once};
use crate::app::{App, ChatRebuildKind, FullscreenView, SurfaceMode, TerminalLifecycleState};
use anyhow::{Context, anyhow};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) use release_guard::TerminalReleaseGuard;

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
    suspended_chat_session: Option<ChatTerminalSession>,
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
            SurfaceMode::Chat => ChatTerminalSession::new().map(SurfaceTerminalSession::Chat),
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
            suspended_chat_session: None,
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
                let mut chat_session = match self.session.take() {
                    Some(SurfaceTerminalSession::Chat(session)) => session,
                    Some(SurfaceTerminalSession::Fullscreen(_)) | None => {
                        return Err(anyhow!("chat session missing before fullscreen entry"));
                    }
                };
                chat_session.suspend_for_fullscreen(app);

                if let Err(err) = apply_enter_fullscreen_actions() {
                    self.session = Some(SurfaceTerminalSession::Chat(chat_session));
                    return Err(err).context("failed to enter fullscreen terminal mode");
                }
                self.alternate_screen_active.store(true, Ordering::SeqCst);
                app.chat_render.line_wrap_disabled = false;
                let fullscreen_session = match FullscreenTerminalSession::new() {
                    Ok(session) => session,
                    Err(err) => {
                        let _ = apply_exit_fullscreen_actions(&self.alternate_screen_active);
                        self.session = Some(SurfaceTerminalSession::Chat(chat_session));
                        return Err(err);
                    }
                };
                self.suspended_chat_session = Some(chat_session);
                self.session = Some(SurfaceTerminalSession::Fullscreen(fullscreen_session));
                self.active_surface = SurfaceMode::Fullscreen(view);
                app.terminal_lifecycle = TerminalLifecycleState::Running(self.active_surface);
                app.surface_dirty.terminal_mode = true;
                app.request_fullscreen_repaint();
            }
            SurfaceTransitionPlan::RetargetFullscreen { to, .. } => match self.session {
                Some(SurfaceTerminalSession::Fullscreen(_)) => {
                    self.active_surface = SurfaceMode::Fullscreen(to);
                    app.terminal_lifecycle = TerminalLifecycleState::Running(self.active_surface);
                    app.request_fullscreen_repaint();
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
                let reused_chat_session = self.suspended_chat_session.is_some();
                let chat_session = match self.suspended_chat_session.take() {
                    Some(session) => session,
                    None => ChatTerminalSession::new()?,
                };
                self.session = Some(SurfaceTerminalSession::Chat(chat_session));
                self.active_surface = SurfaceMode::Chat;
                app.terminal_lifecycle = TerminalLifecycleState::Running(SurfaceMode::Chat);
                app.surface_dirty.terminal_mode = true;
                request_chat_rebuild_after_fullscreen_exit(app, reused_chat_session);
            }
        }

        Ok(())
    }

    pub(crate) fn apply_surface_rebuilds(&mut self, app: &mut App) -> anyhow::Result<()> {
        match self.session_mut()? {
            SurfaceTerminalSession::Chat(session) => match app.surface_dirty.chat.take_rebuild() {
                ChatRebuildKind::None => Ok(()),
                ChatRebuildKind::MutableViewport => {
                    session.clear_mutable_viewport(app);
                    Ok(())
                }
                ChatRebuildKind::FullscreenReturn => {
                    session.reattach_after_fullscreen(app);
                    Ok(())
                }
                ChatRebuildKind::VisibleScreen => {
                    session.clear(app);
                    Ok(())
                }
                ChatRebuildKind::ResizePurgeReplay => {
                    session.clear_for_resize_purge_replay(app);
                    Ok(())
                }
                ChatRebuildKind::SessionBoundary => {
                    session.clear_session_boundary(app);
                    Ok(())
                }
            },
            SurfaceTerminalSession::Fullscreen(_) => Ok(()),
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
        let _suspended_chat_session = self.suspended_chat_session.take();
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

fn request_chat_rebuild_after_fullscreen_exit(app: &mut App, reused_chat_session: bool) {
    if app.chat_render.take_resize_purge_replay_on_chat_return() {
        app.request_chat_resize_purge_replay_rebuild();
    } else if reused_chat_session {
        app.request_chat_fullscreen_return_rebuild();
    } else {
        app.request_chat_visible_rebuild();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::terminal_runtime::fullscreen_session::draw_fullscreen_surface_frame;
    use crate::app::view::set_surface_mode;
    use crate::app::{AppStatus, FullscreenView, SurfaceMode};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn draw_fullscreen_surface_frame_supports_fullscreen_retained_views() {
        let backend = TestBackend::new(100, 40);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = App::test_default();
        app.status = AppStatus::Ready;
        set_surface_mode(&mut app, SurfaceMode::Fullscreen(FullscreenView::Config));

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

    #[test]
    fn fullscreen_exit_rebuild_defaults_to_visible_screen_without_suspended_chat() {
        let mut app = App::test_default();
        app.surface_dirty = crate::app::SurfaceDirtyState::default();

        request_chat_rebuild_after_fullscreen_exit(&mut app, false);

        assert_eq!(app.surface_dirty.chat.rebuild, ChatRebuildKind::VisibleScreen);
        assert!(app.surface_dirty.chat.repaint);
        assert!(!app.chat_render.resize_purge_replay_on_chat_return);
    }

    #[test]
    fn fullscreen_exit_rebuild_reattaches_suspended_chat() {
        let mut app = App::test_default();
        app.surface_dirty = crate::app::SurfaceDirtyState::default();

        request_chat_rebuild_after_fullscreen_exit(&mut app, true);

        assert_eq!(app.surface_dirty.chat.rebuild, ChatRebuildKind::FullscreenReturn);
        assert!(app.surface_dirty.chat.repaint);
        assert!(!app.chat_render.resize_purge_replay_on_chat_return);
    }

    #[test]
    fn fullscreen_exit_rebuild_uses_pending_resize_purge() {
        let mut app = App::test_default();
        app.surface_dirty = crate::app::SurfaceDirtyState::default();
        app.chat_render.mark_resize_purge_replay_on_chat_return();

        request_chat_rebuild_after_fullscreen_exit(&mut app, true);

        assert_eq!(app.surface_dirty.chat.rebuild, ChatRebuildKind::ResizePurgeReplay);
        assert!(app.surface_dirty.chat.repaint);
        assert!(!app.chat_render.resize_purge_replay_on_chat_return);
    }
}
