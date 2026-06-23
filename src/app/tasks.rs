// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::{App, InvalidationLevel, MessageBlock, ToolCallInfo};
use crate::agent::model;
use std::collections::{BTreeSet, HashSet};

pub(super) fn apply_task_state_update(app: &mut App, update: model::TaskStateUpdate) {
    let previous_tasks = app.tasks.clone();
    let removed: HashSet<&str> =
        update.removed_task_ids.iter().map(std::string::String::as_str).collect();

    if update.is_complete_snapshot {
        app.tasks = update
            .tasks
            .into_iter()
            .filter(|task| !removed.contains(task.task_id.as_str()))
            .collect();
        refresh_task_tool_displays_with_previous(app, &previous_tasks);
        return;
    }

    if !removed.is_empty() {
        app.tasks.retain(|task| !removed.contains(task.task_id.as_str()));
    }

    for task in update.tasks {
        if removed.contains(task.task_id.as_str()) {
            continue;
        }
        if let Some(existing) =
            app.tasks.iter_mut().find(|existing| existing.task_id == task.task_id)
        {
            *existing = task;
        } else {
            app.tasks.push(task);
        }
    }
    refresh_task_tool_displays_with_previous(app, &previous_tasks);
}

pub(super) fn refresh_task_tool_displays(app: &mut App) -> bool {
    let previous_tasks = app.tasks.clone();
    refresh_task_tool_displays_with_previous(app, &previous_tasks)
}

fn refresh_task_tool_displays_with_previous(
    app: &mut App,
    previous_tasks: &[model::TaskItem],
) -> bool {
    let current_tasks = app.tasks.clone();
    let mut dirty_blocks = Vec::new();

    for (message_idx, message) in app.messages.iter_mut().enumerate() {
        for (block_idx, block) in message.blocks.iter_mut().enumerate() {
            let MessageBlock::ToolCall(tool_call) = block else {
                continue;
            };
            if refresh_task_tool_display(tool_call.as_mut(), &current_tasks, previous_tasks) {
                tool_call.invalidate_render_cache();
                dirty_blocks.push((message_idx, block_idx));
            }
        }
    }

    if dirty_blocks.is_empty() {
        return false;
    }

    let mut dirty_messages = BTreeSet::new();
    for (message_idx, block_idx) in dirty_blocks {
        app.sync_render_cache_slot(message_idx, block_idx);
        dirty_messages.insert(message_idx);
    }
    for message_idx in dirty_messages {
        app.recompute_message_retained_bytes(message_idx);
        app.invalidate_layout(InvalidationLevel::MessageChanged(message_idx));
    }
    true
}

fn refresh_task_tool_display(
    tool_call: &mut ToolCallInfo,
    current_tasks: &[model::TaskItem],
    previous_tasks: &[model::TaskItem],
) -> bool {
    match tool_call.sdk_tool_name.as_str() {
        "TaskCreate" => refresh_task_create_tool(tool_call, current_tasks, previous_tasks),
        "TaskUpdate" => refresh_task_update_tool(tool_call, current_tasks, previous_tasks),
        _ => false,
    }
}

fn refresh_task_create_tool(
    tool_call: &mut ToolCallInfo,
    current_tasks: &[model::TaskItem],
    previous_tasks: &[model::TaskItem],
) -> bool {
    let Some(task) = task_by_source_tool_call_id(current_tasks, previous_tasks, &tool_call.id)
    else {
        return false;
    };
    sync_string(&mut tool_call.title, format!("Create task #{}: {}", task.task_id, task.subject))
}

fn refresh_task_update_tool(
    tool_call: &mut ToolCallInfo,
    current_tasks: &[model::TaskItem],
    previous_tasks: &[model::TaskItem],
) -> bool {
    let Some(input) = tool_call.raw_input.as_ref().and_then(serde_json::Value::as_object) else {
        return false;
    };
    let Some(task_id) = json_string(input, "taskId") else {
        return false;
    };
    let task = task_by_id(current_tasks, previous_tasks, task_id);
    let subject = json_string(input, "subject").or_else(|| task.map(|task| task.subject.as_str()));
    let status = json_string(input, "status");
    let action = task_update_action(status);
    let mut changed =
        sync_string(&mut tool_call.title, task_update_title(action, task_id, subject));

    if !matches!(tool_call.status, model::ToolCallStatus::Failed | model::ToolCallStatus::Killed) {
        let desired_content = task_update_content(input, task, status);
        if tool_call.content != desired_content {
            tool_call.content = desired_content;
            changed = true;
        }
    }
    changed
}

fn task_by_source_tool_call_id<'a>(
    current_tasks: &'a [model::TaskItem],
    previous_tasks: &'a [model::TaskItem],
    tool_call_id: &str,
) -> Option<&'a model::TaskItem> {
    current_tasks
        .iter()
        .find(|task| task.source_tool_call_id.as_deref() == Some(tool_call_id))
        .or_else(|| {
            previous_tasks
                .iter()
                .find(|task| task.source_tool_call_id.as_deref() == Some(tool_call_id))
        })
}

fn task_by_id<'a>(
    current_tasks: &'a [model::TaskItem],
    previous_tasks: &'a [model::TaskItem],
    task_id: &str,
) -> Option<&'a model::TaskItem> {
    current_tasks
        .iter()
        .find(|task| task.task_id == task_id)
        .or_else(|| previous_tasks.iter().find(|task| task.task_id == task_id))
}

fn task_update_action(status: Option<&str>) -> &'static str {
    match status {
        Some("in_progress" | "running") => "Start",
        Some("completed") => "Complete",
        Some("deleted") => "Delete",
        Some("pending") => "Queue",
        _ => "Update",
    }
}

fn task_update_title(action: &str, task_id: &str, subject: Option<&str>) -> String {
    if let Some(subject) = subject.filter(|subject| !subject.trim().is_empty()) {
        format!("{action} task #{task_id}: {subject}")
    } else {
        format!("{action} task #{task_id}")
    }
}

fn task_update_content(
    input: &serde_json::Map<String, serde_json::Value>,
    task: Option<&model::TaskItem>,
    status: Option<&str>,
) -> Vec<model::ToolCallContent> {
    if !matches!(status, Some("in_progress" | "running")) {
        return Vec::new();
    }
    let activity = json_string(input, "activeForm")
        .or_else(|| task.and_then(|task| task.active_form.as_deref()))
        .filter(|activity| !activity.trim().is_empty());
    activity
        .map(|activity| vec![model::ToolCallContent::from(format!("Activity: {activity}"))])
        .unwrap_or_default()
}

fn json_string<'a>(
    input: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Option<&'a str> {
    input.get(key).and_then(serde_json::Value::as_str).filter(|value| !value.is_empty())
}

fn sync_string(current: &mut String, next: String) -> bool {
    if current == &next {
        return false;
    }
    *current = next;
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{
        BlockCache, ChatMessage, MessageBlock, MessageRole, TerminalSnapshotMode, ToolCallInfo,
    };
    use serde_json::json;

    fn task(id: &str, subject: &str, status: model::TaskStatus) -> model::TaskItem {
        model::TaskItem::new(id, subject, status)
    }

    fn task_tool_call(
        id: &str,
        sdk_tool_name: &str,
        title: &str,
        raw_input: serde_json::Value,
        status: model::ToolCallStatus,
    ) -> ToolCallInfo {
        ToolCallInfo {
            id: id.to_owned(),
            source_message_uuids: Vec::new(),
            title: title.to_owned(),
            sdk_tool_name: sdk_tool_name.to_owned(),
            raw_input: Some(raw_input),
            raw_input_bytes: 0,
            locations: Vec::new(),
            output_metadata: None,
            task_metadata: None,
            status,
            content: Vec::new(),
            hidden: false,
            terminal_id: None,
            terminal_command: None,
            terminal_output: None,
            terminal_output_len: 0,
            terminal_bytes_seen: 0,
            terminal_snapshot_mode: TerminalSnapshotMode::AppendOnly,
            cache: BlockCache::default(),
            pending_permission: None,
            pending_question: None,
        }
    }

    fn insert_tool_call(app: &mut App, tool_call: ToolCallInfo) {
        let id = tool_call.id.clone();
        let message_idx = app.messages.len();
        app.push_message_tracked(ChatMessage::new(
            MessageRole::Assistant,
            vec![MessageBlock::ToolCall(Box::new(tool_call))],
            None,
        ));
        app.index_tool_call(id, message_idx, 0);
    }

    fn only_tool_call(app: &App) -> &ToolCallInfo {
        let MessageBlock::ToolCall(tool_call) = &app.messages[0].blocks[0] else {
            panic!("expected task tool call");
        };
        tool_call
    }

    fn text_content(tool_call: &ToolCallInfo) -> Vec<String> {
        tool_call
            .content
            .iter()
            .filter_map(|content| {
                let model::ToolCallContent::Content(content) = content else {
                    return None;
                };
                let model::ContentBlock::Text(text) = &content.content else {
                    return None;
                };
                Some(text.text.clone())
            })
            .collect()
    }

    #[test]
    fn singular_updates_merge_by_task_id() {
        let mut app = App::test_default();
        app.tasks.push(task("task-1", "old", model::TaskStatus::Pending));

        apply_task_state_update(
            &mut app,
            model::TaskStateUpdate::new(model::TaskUpdateSource::Update).tasks(vec![task(
                "task-1",
                "new",
                model::TaskStatus::InProgress,
            )]),
        );

        assert_eq!(app.tasks.len(), 1);
        assert_eq!(app.tasks[0].subject, "new");
        assert_eq!(app.tasks[0].status, model::TaskStatus::InProgress);
    }

    #[test]
    fn removed_task_ids_remove_state() {
        let mut app = App::test_default();
        app.tasks.push(task("task-1", "one", model::TaskStatus::Pending));
        app.tasks.push(task("task-2", "two", model::TaskStatus::Pending));

        apply_task_state_update(
            &mut app,
            model::TaskStateUpdate::new(model::TaskUpdateSource::Update)
                .removed_task_ids(vec!["task-1".to_owned()]),
        );

        assert_eq!(app.tasks.len(), 1);
        assert_eq!(app.tasks[0].task_id, "task-2");
    }

    #[test]
    fn complete_snapshot_replaces_membership() {
        let mut app = App::test_default();
        app.tasks.push(task("task-1", "one", model::TaskStatus::Pending));
        app.tasks.push(task("task-2", "two", model::TaskStatus::Pending));

        apply_task_state_update(
            &mut app,
            model::TaskStateUpdate::new(model::TaskUpdateSource::List)
                .tasks(vec![task("task-2", "two updated", model::TaskStatus::Completed)])
                .complete_snapshot(true),
        );

        assert_eq!(app.tasks.len(), 1);
        assert_eq!(app.tasks[0].task_id, "task-2");
        assert_eq!(app.tasks[0].subject, "two updated");
    }

    #[test]
    fn create_tool_title_uses_numbered_task_state() {
        let mut app = App::test_default();
        insert_tool_call(
            &mut app,
            task_tool_call(
                "tool-create",
                "TaskCreate",
                "Create task: Scaffold Next.js 15 app via create-next-app",
                json!({
                    "subject": "Scaffold Next.js 15 app via create-next-app",
                    "description": "Run create-next-app"
                }),
                model::ToolCallStatus::Completed,
            ),
        );
        let mut created =
            task("1", "Scaffold Next.js 15 app via create-next-app", model::TaskStatus::Pending);
        created.source_tool_call_id = Some("tool-create".to_owned());

        apply_task_state_update(
            &mut app,
            model::TaskStateUpdate::new(model::TaskUpdateSource::Create).tasks(vec![created]),
        );

        assert_eq!(
            only_tool_call(&app).title,
            "Create task #1: Scaffold Next.js 15 app via create-next-app"
        );
    }

    #[test]
    fn in_progress_update_uses_task_subject_and_activity() {
        let mut app = App::test_default();
        app.tasks.push(
            task("1", "Scaffold Next.js 15 app via create-next-app", model::TaskStatus::Pending)
                .active_form("Scaffolding Next.js app"),
        );
        insert_tool_call(
            &mut app,
            task_tool_call(
                "tool-update",
                "TaskUpdate",
                "Update task: 1",
                json!({ "taskId": "1", "status": "in_progress" }),
                model::ToolCallStatus::Completed,
            ),
        );

        refresh_task_tool_displays(&mut app);

        let tool_call = only_tool_call(&app);
        assert_eq!(tool_call.title, "Start task #1: Scaffold Next.js 15 app via create-next-app");
        assert_eq!(text_content(tool_call), vec!["Activity: Scaffolding Next.js app"]);
    }

    #[test]
    fn completed_update_clears_redundant_activity_content() {
        let mut app = App::test_default();
        app.tasks.push(task(
            "1",
            "Scaffold Next.js 15 app via create-next-app",
            model::TaskStatus::Completed,
        ));
        let mut update = task_tool_call(
            "tool-update",
            "TaskUpdate",
            "Update task: 1",
            json!({ "taskId": "1", "status": "completed" }),
            model::ToolCallStatus::Completed,
        );
        update.content = vec![model::ToolCallContent::from("Activity: Scaffolding Next.js app")];
        insert_tool_call(&mut app, update);

        refresh_task_tool_displays(&mut app);

        let tool_call = only_tool_call(&app);
        assert_eq!(
            tool_call.title,
            "Complete task #1: Scaffold Next.js 15 app via create-next-app"
        );
        assert!(tool_call.content.is_empty());
    }

    #[test]
    fn deleted_update_uses_previous_task_subject_after_removal() {
        let mut app = App::test_default();
        app.tasks.push(task("1", "Remove temporary files", model::TaskStatus::Pending));
        insert_tool_call(
            &mut app,
            task_tool_call(
                "tool-delete",
                "TaskUpdate",
                "Update task: 1",
                json!({ "taskId": "1", "status": "deleted" }),
                model::ToolCallStatus::Completed,
            ),
        );

        apply_task_state_update(
            &mut app,
            model::TaskStateUpdate::new(model::TaskUpdateSource::Update)
                .removed_task_ids(vec!["1".to_owned()]),
        );

        assert!(app.tasks.is_empty());
        assert_eq!(only_tool_call(&app).title, "Delete task #1: Remove temporary files");
    }
}
