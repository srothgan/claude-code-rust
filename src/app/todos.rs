// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::{App, FocusTarget, TodoItem, TodoStatus};
use crate::agent::model;

/// Parse a `TodoWrite` `raw_input` JSON value into a `Vec<TodoItem>`.
/// Expected shape: `{"todos": [{"content": "...", "status": "...", "activeForm": "..."}]}`
#[cfg(test)]
pub(super) fn parse_todos(raw_input: &serde_json::Value) -> Vec<TodoItem> {
    parse_todos_if_present(raw_input).unwrap_or_default()
}

/// Parse todos only when a concrete `todos` array is present in `raw_input`.
/// Returns `None` for transient/incomplete payloads (missing or non-array `todos`).
pub(super) fn parse_todos_if_present(raw_input: &serde_json::Value) -> Option<Vec<TodoItem>> {
    let arr = raw_input.get("todos")?.as_array()?;
    Some(
        arr.iter()
            .filter_map(|item| {
                let content = item.get("content")?.as_str()?.to_owned();
                let status_str = item.get("status")?.as_str()?;
                let active_form =
                    item.get("activeForm").and_then(|v| v.as_str()).unwrap_or("").to_owned();
                let status = match status_str {
                    "in_progress" => TodoStatus::InProgress,
                    "completed" => TodoStatus::Completed,
                    _ => TodoStatus::Pending,
                };
                Some(TodoItem { content, status, active_form })
            })
            .collect(),
    )
}

pub(super) fn set_todos(app: &mut App, todos: Vec<TodoItem>) {
    app.cached_todo_compact = None;
    if todos.is_empty() {
        app.todos.clear();
        app.show_todo_panel = false;
        app.todo_scroll = 0;
        app.todo_selected = 0;
        app.release_focus_target(FocusTarget::TodoList);
        return;
    }

    let all_done = todos.iter().all(|t| t.status == TodoStatus::Completed);
    if all_done {
        app.todos.clear();
        app.show_todo_panel = false;
        app.todo_scroll = 0;
        app.todo_selected = 0;
        app.release_focus_target(FocusTarget::TodoList);
    } else {
        app.todos = todos;
        if app.todo_selected >= app.todos.len() {
            app.todo_selected = app.todos.len().saturating_sub(1);
        }
        if !app.show_todo_panel {
            app.release_focus_target(FocusTarget::TodoList);
        }
    }
}

/// Convert bridge plan entries into the local todo list.
pub(super) fn apply_plan_todos(app: &mut App, plan: &model::Plan) {
    app.cached_todo_compact = None;
    let mut todos = Vec::with_capacity(plan.entries.len());
    for entry in &plan.entries {
        let status_str = format!("{:?}", entry.status);
        let status = match status_str.as_str() {
            "InProgress" => TodoStatus::InProgress,
            "Completed" => TodoStatus::Completed,
            _ => TodoStatus::Pending,
        };
        todos.push(TodoItem { content: entry.content.clone(), status, active_form: String::new() });
    }
    set_todos(app, todos);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use serde_json::json;

    fn todo(content: &str, status: TodoStatus) -> TodoItem {
        TodoItem { content: content.to_owned(), status, active_form: String::new() }
    }

    #[test]
    fn parse_valid_items_preserve_fields_and_default_missing_active_form() {
        let input = json!({
            "todos": [
                {"content": "Task A", "status": "pending", "activeForm": "Doing A"},
                {"content": "Task B", "status": "in_progress"},
                {"content": "Task C", "status": "completed", "activeForm": "Done C"}
            ]
        });

        let todos = parse_todos(&input);

        assert_eq!(todos.len(), 3);
        assert_eq!(todos[0].content, "Task A");
        assert_eq!(todos[0].status, TodoStatus::Pending);
        assert_eq!(todos[0].active_form, "Doing A");
        assert_eq!(todos[1].status, TodoStatus::InProgress);
        assert_eq!(todos[1].active_form, "");
        assert_eq!(todos[2].status, TodoStatus::Completed);
        assert_eq!(todos[2].active_form, "Done C");
    }

    #[test]
    fn parse_todos_if_present_requires_a_concrete_array() {
        for input in [json!({"other": 1}), json!({"todos": {"not": "array"}})] {
            assert!(parse_todos_if_present(&input).is_none());
        }
        assert!(matches!(parse_todos_if_present(&json!({"todos": []})), Some(v) if v.is_empty()));
    }

    #[test]
    fn parse_skips_invalid_items_and_maps_unknown_statuses_to_pending() {
        let input = json!({
            "todos": [
                {"status": "pending"},
                {"content": "missing status"},
                {"content": 42, "status": "pending"},
                {"content": "Good", "status": "completed"},
                {"content": "Also good", "status": "in_progress"},
                {"content": "Fallback", "status": "banana"},
                {"content": "Case sensitive", "status": "COMPLETED"}
            ]
        });
        let todos = parse_todos(&input);

        assert_eq!(todos.len(), 4);
        assert_eq!(todos[0].content, "Good");
        assert_eq!(todos[0].status, TodoStatus::Completed);
        assert_eq!(todos[1].content, "Also good");
        assert_eq!(todos[1].status, TodoStatus::InProgress);
        assert_eq!(todos[2].status, TodoStatus::Pending);
        assert_eq!(todos[3].status, TodoStatus::Pending);
    }

    #[test]
    fn parse_returns_empty_for_non_todo_shapes() {
        let invalid_inputs = [
            serde_json::Value::Null,
            json!({"todos": null}),
            json!({"todos": 42}),
            json!({"todos": "not an array"}),
            json!([{"content": "Task", "status": "pending"}]),
            json!("just a string"),
            json!(true),
            json!(42),
        ];

        for input in invalid_inputs {
            assert!(parse_todos(&input).is_empty(), "unexpected todos for {input}");
        }
    }

    #[test]
    fn parse_preserves_special_content_without_shape_coupling() {
        let long_content = "A".repeat(10_000);
        let input = json!({
            "metadata": {"nested": {"deep": true}},
            "todos": [
                {"content": "\u{1F680} Deploy to prod", "status": "in_progress", "activeForm": "\u{1F525} Deploying"},
                {"content": "line1\nline2\ttab\r\nwindows", "status": "pending"},
                {"content": long_content, "status": "pending"}
            ],
            "other": [1, 2, 3]
        });

        let todos = parse_todos(&input);

        assert_eq!(todos.len(), 3);
        assert_eq!(todos[0].content, "\u{1F680} Deploy to prod");
        assert_eq!(todos[0].active_form, "\u{1F525} Deploying");
        assert!(todos[1].content.contains('\n'));
        assert!(todos[1].content.contains('\t'));
        assert_eq!(todos[2].content.len(), 10_000);
    }

    #[test]
    fn set_todos_clears_panel_state_for_empty_or_completed_inputs() {
        for todos in [
            Vec::new(),
            vec![todo("done", TodoStatus::Completed), todo("done too", TodoStatus::Completed)],
        ] {
            let mut app = App::test_default();
            app.show_todo_panel = true;
            app.todo_scroll = 4;
            app.todo_selected = 3;
            app.claim_focus_target(FocusTarget::TodoList);

            set_todos(&mut app, todos);

            assert!(app.todos.is_empty());
            assert!(!app.show_todo_panel);
            assert_eq!(app.todo_scroll, 0);
            assert_eq!(app.todo_selected, 0);
            assert_eq!(app.focus_owner(), crate::app::focus::FocusOwner::Input);
        }
    }

    #[test]
    fn set_todos_retains_visible_items_and_clamps_selection() {
        let mut app = App::test_default();
        app.todos = vec![todo("old", TodoStatus::Pending), todo("old-2", TodoStatus::Pending)];
        app.show_todo_panel = true;
        app.todo_selected = 5;

        set_todos(
            &mut app,
            vec![todo("new-a", TodoStatus::Pending), todo("new-b", TodoStatus::InProgress)],
        );

        assert_eq!(app.todos.len(), 2);
        assert!(app.show_todo_panel);
        assert_eq!(app.todo_selected, 1);
        assert_eq!(app.todos[0].content, "new-a");
        assert_eq!(app.todos[1].status, TodoStatus::InProgress);
    }

    #[test]
    fn apply_plan_todos_maps_bridge_statuses_and_hides_completed_plans() {
        let mut app = App::test_default();
        let active_plan = model::Plan::new(vec![
            model::PlanEntry::new(
                "pending",
                model::PlanEntryPriority::Medium,
                model::PlanEntryStatus::Pending,
            ),
            model::PlanEntry::new(
                "running",
                model::PlanEntryPriority::High,
                model::PlanEntryStatus::InProgress,
            ),
        ]);

        apply_plan_todos(&mut app, &active_plan);

        assert_eq!(app.todos.len(), 2);
        assert_eq!(app.todos[0].status, TodoStatus::Pending);
        assert_eq!(app.todos[1].status, TodoStatus::InProgress);

        let completed_plan = model::Plan::new(vec![model::PlanEntry::new(
            "done",
            model::PlanEntryPriority::Low,
            model::PlanEntryStatus::Completed,
        )]);

        apply_plan_todos(&mut app, &completed_plan);

        assert!(app.todos.is_empty());
        assert!(!app.show_todo_panel);
    }
}
