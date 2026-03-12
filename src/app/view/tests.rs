use super::*;
use crate::app::dialog::DialogState;
use crate::app::slash::{SlashContext, SlashState};
use crate::app::state::types::ScrollbarDragState;
use crate::app::subagent::SubagentState;
use crate::app::{
    FocusTarget, PasteSessionState, SelectionKind, SelectionPoint, SelectionState, TodoItem,
    TodoStatus,
};

fn busy_view_test_app() -> App {
    let mut app = App::test_default();
    app.input.set_text("draft");
    app.selection = Some(SelectionState {
        kind: SelectionKind::Chat,
        start: SelectionPoint { row: 0, col: 0 },
        end: SelectionPoint { row: 0, col: 4 },
        dragging: true,
    });
    app.scrollbar_drag = Some(ScrollbarDragState { thumb_grab_offset: 1 });
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
    app.pending_permission_ids.push("perm-1".to_owned());
    app.claim_focus_target(FocusTarget::Permission);
    app
}

#[test]
fn set_active_view_clears_transient_chat_state_but_keeps_draft() {
    let mut app = busy_view_test_app();

    set_active_view(&mut app, ActiveView::Trusted);

    assert_eq!(app.active_view, ActiveView::Trusted);
    assert_eq!(app.input.text(), "draft");
    assert!(app.selection.is_none());
    assert!(app.scrollbar_drag.is_none());
    assert!(app.mention.is_none());
    assert!(app.slash.is_none());
    assert!(app.subagent.is_none());
    assert!(app.pending_paste_text.is_empty());
    assert!(app.pending_paste_session.is_none());
    assert!(app.active_paste_session.is_none());
    assert!(app.pending_submit.is_none());
}

#[test]
fn set_active_view_switches_to_config_from_trusted() {
    let mut app = busy_view_test_app();
    app.active_view = ActiveView::Trusted;

    set_active_view(&mut app, ActiveView::Config);

    assert_eq!(app.active_view, ActiveView::Config);
    assert!(app.selection.is_none());
    assert!(app.pending_paste_text.is_empty());
}

#[test]
fn set_active_view_same_view_is_noop() {
    let mut app = busy_view_test_app();
    app.needs_redraw = false;

    set_active_view(&mut app, ActiveView::Chat);

    assert_eq!(app.active_view, ActiveView::Chat);
    assert!(app.selection.is_some());
    assert!(app.mention.is_some());
    assert!(!app.pending_paste_text.is_empty());
    assert!(app.pending_submit.is_some());
    assert!(!app.needs_redraw);
}
