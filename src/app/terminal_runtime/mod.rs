// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

mod chat_session;
mod chat_terminal;
mod fullscreen_session;
mod history_insert;
mod modes;
mod panic_hook;
mod release_guard;
#[cfg(any(unix, test))]
mod tracked_cursor_backend;

use self::chat_session::{ChatTerminalSeed, ChatTerminalSeedProvenance, ChatTerminalSession};
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
    cached_chat_seed: Option<ChatTerminalSeed>,
    active_surface: SurfaceMode,
    alternate_screen_active: Arc<AtomicBool>,
    panic_hook: Option<PanicRestoreHook>,
    restored: Arc<AtomicBool>,
}

#[derive(Debug, Clone, Copy)]
enum BootstrapSeedMode {
    MeasureBeforeEventStream,
    ConservativeAfterResume,
}

impl BootstrapSeedMode {
    fn chat_seed(self) -> anyhow::Result<ChatTerminalSeed> {
        match self {
            Self::MeasureBeforeEventStream => ChatTerminalSeed::read_before_event_stream(),
            Self::ConservativeAfterResume => ChatTerminalSeed::conservative_current(
                ChatTerminalSeedProvenance::ConservativeAfterResume,
            ),
        }
    }

    fn chat_session(self) -> anyhow::Result<ChatTerminalSession> {
        match self {
            Self::MeasureBeforeEventStream => ChatTerminalSession::new_before_event_stream(),
            Self::ConservativeAfterResume => {
                Ok(ChatTerminalSession::new_with_seed(self.chat_seed()?))
            }
        }
    }
}

impl TerminalRuntime {
    pub(crate) fn bootstrap(app: &mut App) -> anyhow::Result<Self> {
        Self::bootstrap_inner(app, BootstrapSeedMode::MeasureBeforeEventStream)
    }

    pub(crate) fn bootstrap_after_event_stream(app: &mut App) -> anyhow::Result<Self> {
        Self::bootstrap_inner(app, BootstrapSeedMode::ConservativeAfterResume)
    }

    fn bootstrap_inner(app: &mut App, seed_mode: BootstrapSeedMode) -> anyhow::Result<Self> {
        let target_surface = app.surface_mode;
        let restored = Arc::new(AtomicBool::new(false));
        let alternate_screen_active = Arc::new(AtomicBool::new(false));

        if let Err(err) = apply_startup_actions() {
            restore_once(restored.as_ref(), || {
                let _ = restore_terminal_modes(alternate_screen_active.as_ref());
            });
            return Err(err).context("failed to configure terminal startup modes");
        }

        let cached_chat_seed = match target_surface {
            SurfaceMode::Chat => None,
            SurfaceMode::Fullscreen(_) => Some(seed_mode.chat_seed()?),
        };

        let session = match target_surface {
            SurfaceMode::Chat => seed_mode.chat_session().map(SurfaceTerminalSession::Chat),
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
            cached_chat_seed,
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
                let chat_session = if let Some(session) = self.suspended_chat_session.take() {
                    session
                } else {
                    let seed = match self.cached_chat_seed.take() {
                        Some(seed) => seed.with_current_size(
                            ChatTerminalSeedProvenance::CachedBeforeFullscreen,
                        )?,
                        None => ChatTerminalSeed::conservative_current(
                            ChatTerminalSeedProvenance::ConservativeAfterFullscreen,
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
                ChatRebuildKind::PurgeReplay(options) => {
                    session.clear_for_purge_replay(app, options);
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
        if !self.restored.load(Ordering::SeqCst) {
            app.terminal_lifecycle = TerminalLifecycleState::Restoring;
        }

        let _session = self.session.take();
        let _suspended_chat_session = self.suspended_chat_session.take();
        let _cached_chat_seed = self.cached_chat_seed.take();
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
mod tests;
