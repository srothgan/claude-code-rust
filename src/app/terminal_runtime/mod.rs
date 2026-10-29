// SPDX-License-Identifier: Apache-2.0

mod chat_session;
mod chat_terminal;
mod fullscreen_session;
mod history_insert;
mod input;
mod modes;
mod panic_hook;
mod release_guard;
mod tracked_cursor_backend;
pub(crate) mod viewport;

use self::chat_session::{ChatTerminalSeed, ChatTerminalSeedProvenance, ChatTerminalSession};
use self::chat_terminal::NativeTerminalIo;
use self::fullscreen_session::FullscreenTerminalSession;
pub(super) use self::input::TerminalInput;
use self::modes::{ModeIo, TerminalModes, chat_startup_actions, enter_fullscreen_actions};
use self::panic_hook::PanicRestoreHook;
use crate::app::{App, FullscreenView, SurfaceMode, TerminalLifecycleState};
use anyhow::{Context, anyhow};
use std::sync::Arc;

pub(crate) trait RuntimeIo: ModeIo {
    type FullscreenBackend: ratatui::backend::Backend<Error = std::io::Error>;
    fn size(&self) -> std::io::Result<(u16, u16)>;
    fn cursor_before_input_reader(&self) -> std::io::Result<(u16, u16)>;
    fn fullscreen_backend(&self) -> Self::FullscreenBackend;
}

impl RuntimeIo for NativeTerminalIo {
    type FullscreenBackend = ratatui::backend::CrosstermBackend<std::io::Stdout>;
    fn size(&self) -> std::io::Result<(u16, u16)> {
        crossterm::terminal::size()
    }
    fn cursor_before_input_reader(&self) -> std::io::Result<(u16, u16)> {
        crossterm::cursor::position()
    }
    fn fullscreen_backend(&self) -> Self::FullscreenBackend {
        ratatui::backend::CrosstermBackend::new(std::io::stdout())
    }
}

pub(crate) use release_guard::TerminalReleaseGuard;

enum SurfaceTerminalSession<B: ratatui::backend::Backend> {
    Chat(ChatTerminalSession),
    Fullscreen(FullscreenTerminalSession<B>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SurfaceTransitionPlan {
    Noop,
    EnterFullscreen { view: FullscreenView },
    RetargetFullscreen { from: FullscreenView, to: FullscreenView },
    ExitFullscreen { from: FullscreenView },
}

pub(crate) struct TerminalRuntime<I: RuntimeIo = NativeTerminalIo> {
    session: Option<SurfaceTerminalSession<I::FullscreenBackend>>,
    suspended_chat_session: Option<ChatTerminalSession>,
    cached_chat_seed: Option<ChatTerminalSeed>,
    active_surface: SurfaceMode,
    modes: Arc<TerminalModes<I>>,
    panic_hook: Option<PanicRestoreHook>,
}

#[derive(Debug, Clone, Copy)]
enum BootstrapSeedMode {
    MeasureBeforeInputReader,
    ConservativeAfterResume,
}

impl BootstrapSeedMode {
    fn chat_seed(self, io: &impl RuntimeIo) -> anyhow::Result<ChatTerminalSeed> {
        match self {
            Self::MeasureBeforeInputReader => ChatTerminalSeed::read_before_input_reader(io),
            Self::ConservativeAfterResume => ChatTerminalSeed::conservative_current(
                ChatTerminalSeedProvenance::ConservativeAfterResume,
                io,
            ),
        }
    }
}

impl<I: RuntimeIo> TerminalRuntime<I> {
    pub(crate) fn has_pending_surface_work(&self, app: &App) -> bool {
        app.surface_dirty.active_surface_needs_draw(app.terminal_lifecycle)
            || (matches!(
                app.terminal_lifecycle,
                TerminalLifecycleState::Running(SurfaceMode::Chat)
            ) && matches!(&self.session, Some(SurfaceTerminalSession::Chat(session)) if session.has_pending_work()))
    }

    pub(crate) fn chat_layout_ready(&self, app: &App) -> bool {
        matches!(app.terminal_lifecycle, TerminalLifecycleState::Running(SurfaceMode::Chat))
            && matches!(&self.session, Some(SurfaceTerminalSession::Chat(session)) if session.layout_ready())
    }

    fn bootstrap_inner(app: &mut App, seed_mode: BootstrapSeedMode, io: I) -> anyhow::Result<Self> {
        let target_surface = app.surface_mode;
        let modes = Arc::new(TerminalModes::new(io));
        // All acquisition, including seed reads, belongs to the same cleanup scope.
        let acquired = (|| -> anyhow::Result<_> {
            modes
                .apply(chat_startup_actions())
                .context("failed to configure terminal startup modes")?;
            let seed = seed_mode.chat_seed(&modes.io)?;
            match target_surface {
                SurfaceMode::Chat => Ok((
                    SurfaceTerminalSession::Chat(ChatTerminalSession::new_with_seed(seed)),
                    None,
                )),
                SurfaceMode::Fullscreen(_) => {
                    modes
                        .apply(enter_fullscreen_actions())
                        .context("failed to enter fullscreen terminal mode")?;
                    let session = FullscreenTerminalSession::new(modes.io.fullscreen_backend())?;
                    Ok((SurfaceTerminalSession::Fullscreen(session), Some(seed)))
                }
            }
        })();
        let (session, cached_chat_seed) = match acquired {
            Ok(acquired) => acquired,
            Err(error) => {
                restore_modes(&modes);
                return Err(error);
            }
        };
        let modes_for_hook = Arc::clone(&modes);
        let panic_hook = PanicRestoreHook::install(move || {
            restore_modes(&modes_for_hook);
        });
        app.terminal_lifecycle = TerminalLifecycleState::Running(target_surface);
        Ok(Self {
            session: Some(session),
            suspended_chat_session: None,
            cached_chat_seed,
            active_surface: target_surface,
            modes,
            panic_hook: Some(panic_hook),
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

                if let Err(err) = self.modes.apply(enter_fullscreen_actions()) {
                    self.session = Some(SurfaceTerminalSession::Chat(chat_session));
                    return Err(err).context("failed to enter fullscreen terminal mode");
                }
                app.chat_render.line_wrap_disabled = false;
                let fullscreen_session =
                    match FullscreenTerminalSession::new(self.modes.io.fullscreen_backend()) {
                        Ok(session) => session,
                        Err(err) => {
                            let _ = self.modes.exit_fullscreen();
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

                self.modes.exit_fullscreen().context("failed to exit fullscreen terminal mode")?;
                app.chat_render.line_wrap_disabled = false;
                let reused_chat_session = self.suspended_chat_session.is_some();
                let chat_session = if let Some(session) = self.suspended_chat_session.take() {
                    session
                } else {
                    let seed = match self.cached_chat_seed.take() {
                        Some(seed) => seed.with_current_size(
                            ChatTerminalSeedProvenance::CachedBeforeFullscreen,
                            &self.modes.io,
                        )?,
                        None => ChatTerminalSeed::conservative_current(
                            ChatTerminalSeedProvenance::ConservativeAfterFullscreen,
                            &self.modes.io,
                        )?,
                    };
                    ChatTerminalSession::new_with_seed(seed)
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
            SurfaceTerminalSession::Chat(session) => {
                session.apply_rebuild(app);
                Ok(())
            }
            SurfaceTerminalSession::Fullscreen(_) => Ok(()),
        }
    }

    pub(crate) fn update_active_surface(&mut self, app: &mut App) -> anyhow::Result<()> {
        match self.session_mut()? {
            SurfaceTerminalSession::Chat(session) => session.update(app),
            SurfaceTerminalSession::Fullscreen(session) => session.draw(app),
        }
    }

    pub(crate) fn invalidate_cached_chat_seed(&mut self, reason: &'static str) {
        if self.cached_chat_seed.take().is_some() {
            tracing::debug!(
                target: crate::logging::targets::APP_LIFECYCLE,
                event_name = "inline_chat_seed_invalidated",
                message = "cached chat terminal seed invalidated",
                outcome = "success",
                reason,
            );
        }
    }

    pub(crate) fn restore(&mut self, app: &mut App) {
        if !self.modes.is_restored() {
            app.terminal_lifecycle = TerminalLifecycleState::Restoring;
        }

        let session = self.session.take();
        let suspended_chat_session = self.suspended_chat_session.take();
        let _cached_chat_seed = self.cached_chat_seed.take();
        let chat_session = match &session {
            Some(SurfaceTerminalSession::Chat(chat_session)) => Some(chat_session),
            _ => suspended_chat_session.as_ref(),
        };
        if restore_modes(&self.modes) {
            // Output printed after exit, such as the resume hint, starts below the chat viewport.
            if let Some(chat_session) = chat_session
                && let Err(err) = chat_session.park_cursor_at_viewport_end()
            {
                tracing::warn!(
                    target: crate::logging::targets::APP_LIFECYCLE,
                    event_name = "terminal_cursor_park_failed",
                    message = "failed to move cursor below chat viewport",
                    outcome = "failure",
                    error_message = %err,
                );
            }
        }
        let _hook = self.panic_hook.take();
        app.terminal_lifecycle = TerminalLifecycleState::Exited;
    }

    fn session_mut(&mut self) -> anyhow::Result<&mut SurfaceTerminalSession<I::FullscreenBackend>> {
        self.session.as_mut().ok_or_else(|| anyhow!("terminal runtime has already been restored"))
    }
}

impl<I: RuntimeIo> Drop for TerminalRuntime<I> {
    fn drop(&mut self) {
        restore_modes(&self.modes);
    }
}

impl TerminalRuntime {
    pub(crate) fn bootstrap(app: &mut App) -> anyhow::Result<Self> {
        Self::bootstrap_inner(app, BootstrapSeedMode::MeasureBeforeInputReader, NativeTerminalIo)
    }
    pub(crate) fn bootstrap_with_input_reader(app: &mut App) -> anyhow::Result<Self> {
        Self::bootstrap_inner(app, BootstrapSeedMode::ConservativeAfterResume, NativeTerminalIo)
    }
}

fn restore_modes<I: ModeIo>(modes: &TerminalModes<I>) -> bool {
    match modes.restore() {
        Ok(restored) => restored,
        Err(err) => {
            tracing::warn!(
                target: crate::logging::targets::APP_LIFECYCLE,
                event_name = "terminal_restore_failed", message = "failed to restore terminal state",
                outcome = "failure", error_message = %err,
            );
            false
        }
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
        app.request_chat_purge_replay_rebuild(
            crate::app::ChatPurgeReplayOptions::chat_return_after_resize(),
        );
    } else if reused_chat_session {
        app.request_chat_fullscreen_return_rebuild();
    } else {
        app.request_chat_visible_rebuild();
    }
}

#[cfg(test)]
mod lifecycle_audit_tests;
