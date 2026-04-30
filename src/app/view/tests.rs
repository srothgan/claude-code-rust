use super::*;
use crate::app::config::{ConfigOverlayState, OutputStyle, OutputStyleOverlayState};
use crate::app::dialog::DialogState;
use crate::app::slash::{SlashContext, SlashState};
use crate::app::subagent::SubagentState;
use crate::app::{
    FocusTarget, FullscreenView, PasteSessionState, ReleaseReason, SelectionPoint, SurfaceMode,
    TerminalLifecycleState, TodoItem, TodoStatus,
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
    app.show_todo_panel = true;
    app.todos = vec![TodoItem {
        content: "todo".to_owned(),
        status: TodoStatus::Pending,
        active_form: "todo".to_owned(),
    }];
    app.claim_focus_target(FocusTarget::TodoList);
    app.pending_interaction_ids.push("perm-1".to_owned());
    app.claim_focus_target(FocusTarget::Permission);
    app
}

#[test]
fn set_active_view_clears_transient_chat_state_but_keeps_draft() {
    let mut app = busy_view_test_app();
    app.chat_render.live_region.anchor_valid = true;
    app.chat_render.live_region.last_rendered_rows = 5;

    set_active_view(&mut app, ActiveView::Trusted);

    assert_eq!(app.active_view, ActiveView::Trusted);
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
fn set_active_view_switches_to_config_from_trusted() {
    let mut app = busy_view_test_app();
    app.active_view = ActiveView::Trusted;

    set_active_view(&mut app, ActiveView::Config);

    assert_eq!(app.active_view, ActiveView::Config);
    assert!(app.pending_paste_text.is_empty());
}

#[test]
fn set_active_view_same_view_is_noop() {
    let mut app = busy_view_test_app();
    app.surface_dirty.chat.repaint = false;

    set_active_view(&mut app, ActiveView::Chat);

    assert_eq!(app.active_view, ActiveView::Chat);
    assert!(app.mention.is_some());
    assert!(!app.pending_paste_text.is_empty());
    assert!(app.pending_submit.is_some());
    assert!(!app.surface_dirty.chat.repaint);
}

#[test]
fn set_active_view_keeps_permission_unfocused_when_returning_to_chat_with_draft() {
    let mut app = busy_view_test_app();

    set_active_view(&mut app, ActiveView::Trusted);
    assert_eq!(app.active_view, ActiveView::Trusted);

    set_active_view(&mut app, ActiveView::Chat);

    assert_eq!(app.active_view, ActiveView::Chat);
    assert_eq!(app.focus_owner(), crate::app::FocusOwner::TodoList);
}

#[test]
fn leaving_config_clears_config_overlay() {
    let mut app = App::test_default();
    app.active_view = ActiveView::Config;
    app.config.overlay = Some(ConfigOverlayState::OutputStyle(OutputStyleOverlayState {
        selected: OutputStyle::Default,
    }));

    set_active_view(&mut app, ActiveView::Trusted);

    assert!(app.config.overlay.is_none());
}

#[test]
fn active_view_surface_mode_mapping_covers_all_views() {
    assert_eq!(ActiveView::Chat.surface_mode(), SurfaceMode::Chat);
    assert_eq!(ActiveView::Chat.fullscreen_view(), None);

    assert_eq!(ActiveView::Config.surface_mode(), SurfaceMode::Fullscreen(FullscreenView::Config));
    assert_eq!(ActiveView::Config.fullscreen_view(), Some(FullscreenView::Config));

    assert_eq!(
        ActiveView::Trusted.surface_mode(),
        SurfaceMode::Fullscreen(FullscreenView::Trusted)
    );
    assert_eq!(ActiveView::Trusted.fullscreen_view(), Some(FullscreenView::Trusted));

    assert_eq!(
        ActiveView::SessionPicker.surface_mode(),
        SurfaceMode::Fullscreen(FullscreenView::SessionPicker)
    );
    assert_eq!(ActiveView::SessionPicker.fullscreen_view(), Some(FullscreenView::SessionPicker));
}

#[test]
fn surface_mode_active_view_mapping_covers_all_modes() {
    assert_eq!(SurfaceMode::Chat.active_view(), ActiveView::Chat);
    assert_eq!(FullscreenView::Config.active_view(), ActiveView::Config);
    assert_eq!(FullscreenView::Trusted.active_view(), ActiveView::Trusted);
    assert_eq!(FullscreenView::SessionPicker.active_view(), ActiveView::SessionPicker);
    assert_eq!(SurfaceMode::Fullscreen(FullscreenView::Config).active_view(), ActiveView::Config);
}

#[test]
fn set_active_view_updates_surface_and_lifecycle_while_running() {
    let mut app = App::test_default();
    app.chat_render.live_region.anchor_valid = true;

    set_active_view(&mut app, ActiveView::Config);

    assert_eq!(app.active_view, ActiveView::Config);
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
fn set_active_view_preserves_non_running_lifecycle_states() {
    let mut released_app = App::test_default();
    released_app.terminal_lifecycle =
        TerminalLifecycleState::ReleasedToChild(ReleaseReason::AuthFlow);
    set_active_view(&mut released_app, ActiveView::Trusted);
    assert_eq!(
        released_app.terminal_lifecycle,
        TerminalLifecycleState::ReleasedToChild(ReleaseReason::AuthFlow)
    );
    assert_eq!(released_app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::Trusted));

    let mut restoring_app = App::test_default();
    restoring_app.terminal_lifecycle = TerminalLifecycleState::Restoring;
    set_active_view(&mut restoring_app, ActiveView::Config);
    assert_eq!(restoring_app.terminal_lifecycle, TerminalLifecycleState::Restoring);
    assert_eq!(restoring_app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::Config));

    let mut exited_app = App::test_default();
    exited_app.terminal_lifecycle = TerminalLifecycleState::Exited;
    set_active_view(&mut exited_app, ActiveView::SessionPicker);
    assert_eq!(exited_app.terminal_lifecycle, TerminalLifecycleState::Exited);
    assert_eq!(exited_app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::SessionPicker));
}
