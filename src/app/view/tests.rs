use super::*;
use crate::app::config::{ConfigOverlayState, OutputStyle, OutputStyleOverlayState};
use crate::app::dialog::DialogState;
use crate::app::slash::{SlashContext, SlashState};
use crate::app::subagent::SubagentState;
use crate::app::{
    FocusTarget, FullscreenView, PasteSessionState, ReleaseReason, SelectionPoint, SurfaceMode,
    TerminalLifecycleState,
};

fn busy_view_test_app() -> App {
    let mut app = App::test_default();
    app.input.set_text("draft");
    app.pending_submit = Some(app.input.snapshot());
    app.pending_paste_text = "blocked".to_owned();
    app.pending_paste_session = Some(PasteSessionState {
        id: 1,
        start: SelectionPoint { row: 0, col: 0 },
        placeholder_index: Some(0),
    });
    app.active_paste_session = Some(PasteSessionState {
        id: 2,
        start: SelectionPoint { row: 0, col: 0 },
        placeholder_index: Some(1),
    });
    app.mention = Some(crate::app::mention::MentionState::new(0, 0, "rs".to_owned(), vec![]));
    app.slash = Some(SlashState {
        trigger_row: 0,
        trigger_col: 0,
        query: "/co".to_owned(),
        context: SlashContext::CommandName,
        candidates: vec![],
        dialog: DialogState::default(),
    });
    app.subagent = Some(SubagentState {
        trigger_row: 0,
        trigger_col: 0,
        query: "plan".to_owned(),
        candidates: vec![],
        dialog: DialogState::default(),
    });
    app.pending_interaction_ids.push("perm-1".to_owned());
    app.claim_focus_target(FocusTarget::Permission);
    app
}

#[test]
fn set_surface_mode_clears_transient_chat_state_but_keeps_draft() {
    let mut app = busy_view_test_app();
    app.chat_render.live_region.anchor_valid = true;
    app.chat_render.live_region.last_rendered_rows = 5;

    set_surface_mode(&mut app, SurfaceMode::Fullscreen(FullscreenView::Trusted));

    assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::Trusted));
    assert_eq!(app.input.text(), "draft");
    assert!(app.mention.is_none());
    assert!(app.slash.is_none());
    assert!(app.subagent.is_none());
    assert!(app.pending_paste_text.is_empty());
    assert!(app.pending_paste_session.is_none());
    assert!(app.active_paste_session.is_none());
    assert!(app.pending_submit.is_none());
    assert!(!app.chat_render.live_region.anchor_valid);
    assert_eq!(app.chat_render.live_region.last_rendered_rows, 0);
}

#[test]
fn set_surface_mode_switches_to_config_from_trusted() {
    let mut app = busy_view_test_app();
    app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::Trusted);

    set_surface_mode(&mut app, SurfaceMode::Fullscreen(FullscreenView::Config));

    assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::Config));
    assert!(app.pending_paste_text.is_empty());
}

#[test]
fn set_surface_mode_same_view_is_noop() {
    let mut app = busy_view_test_app();
    app.surface_dirty.chat.repaint = false;

    set_surface_mode(&mut app, SurfaceMode::Chat);

    assert_eq!(app.surface_mode, SurfaceMode::Chat);
    assert!(app.mention.is_some());
    assert!(!app.pending_paste_text.is_empty());
    assert!(app.pending_submit.is_some());
    assert!(!app.surface_dirty.chat.repaint);
}

#[test]
fn set_surface_mode_keeps_permission_unfocused_when_returning_to_chat_with_draft() {
    let mut app = busy_view_test_app();

    set_surface_mode(&mut app, SurfaceMode::Fullscreen(FullscreenView::Trusted));
    assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::Trusted));

    set_surface_mode(&mut app, SurfaceMode::Chat);

    assert_eq!(app.surface_mode, SurfaceMode::Chat);
    assert_eq!(app.focus_owner(), crate::app::FocusOwner::Input);
}

#[test]
fn leaving_config_clears_config_overlay() {
    let mut app = App::test_default();
    app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::Config);
    app.config.overlay = Some(ConfigOverlayState::OutputStyle(OutputStyleOverlayState {
        selected: OutputStyle::Default,
    }));

    set_surface_mode(&mut app, SurfaceMode::Fullscreen(FullscreenView::Trusted));

    assert!(app.config.overlay.is_none());
}

#[test]
fn surface_mode_reports_fullscreen_view_only_for_fullscreen_modes() {
    assert_eq!(SurfaceMode::Chat.fullscreen_view(), None);
    assert_eq!(
        SurfaceMode::Fullscreen(FullscreenView::Config).fullscreen_view(),
        Some(FullscreenView::Config)
    );
    assert_eq!(
        SurfaceMode::Fullscreen(FullscreenView::Trusted).fullscreen_view(),
        Some(FullscreenView::Trusted)
    );
    assert_eq!(
        SurfaceMode::Fullscreen(FullscreenView::SessionPicker).fullscreen_view(),
        Some(FullscreenView::SessionPicker)
    );
}

#[test]
fn set_surface_mode_updates_surface_and_lifecycle_while_running() {
    let mut app = App::test_default();
    app.chat_render.live_region.anchor_valid = true;

    set_surface_mode(&mut app, SurfaceMode::Fullscreen(FullscreenView::Config));

    assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::Config));
    assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::Config));
    assert_eq!(
        app.terminal_lifecycle,
        TerminalLifecycleState::Running(SurfaceMode::Fullscreen(FullscreenView::Config))
    );
    assert!(app.surface_dirty.fullscreen.redraw);
    assert!(app.surface_dirty.terminal_mode);
    assert!(!app.chat_render.live_region.anchor_valid);
}

#[test]
fn set_surface_mode_preserves_non_running_lifecycle_states() {
    let mut released_app = App::test_default();
    released_app.terminal_lifecycle =
        TerminalLifecycleState::ReleasedToChild(ReleaseReason::AuthFlow);
    set_surface_mode(&mut released_app, SurfaceMode::Fullscreen(FullscreenView::Trusted));
    assert_eq!(
        released_app.terminal_lifecycle,
        TerminalLifecycleState::ReleasedToChild(ReleaseReason::AuthFlow)
    );
    assert_eq!(released_app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::Trusted));

    let mut restoring_app = App::test_default();
    restoring_app.terminal_lifecycle = TerminalLifecycleState::Restoring;
    set_surface_mode(&mut restoring_app, SurfaceMode::Fullscreen(FullscreenView::Config));
    assert_eq!(restoring_app.terminal_lifecycle, TerminalLifecycleState::Restoring);
    assert_eq!(restoring_app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::Config));

    let mut exited_app = App::test_default();
    exited_app.terminal_lifecycle = TerminalLifecycleState::Exited;
    set_surface_mode(&mut exited_app, SurfaceMode::Fullscreen(FullscreenView::SessionPicker));
    assert_eq!(exited_app.terminal_lifecycle, TerminalLifecycleState::Exited);
    assert_eq!(exited_app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::SessionPicker));
}
