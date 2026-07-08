// SPDX-License-Identifier: Apache-2.0
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
fn surface_transition_plan_is_noop_for_chat_to_chat() {
    assert_eq!(
        plan_surface_transition(SurfaceMode::Chat, SurfaceMode::Chat),
        SurfaceTransitionPlan::Noop
    );
}

#[test]
fn surface_transition_plan_enters_fullscreen_from_chat() {
    assert_eq!(
        plan_surface_transition(SurfaceMode::Chat, SurfaceMode::Fullscreen(FullscreenView::Config)),
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

    assert_eq!(
        app.surface_dirty.chat.rebuild,
        ChatRebuildKind::PurgeReplay(crate::app::ChatPurgeReplayOptions::chat_return_after_resize())
    );
    assert!(app.surface_dirty.chat.repaint);
    assert!(!app.chat_render.resize_purge_replay_on_chat_return);
}
