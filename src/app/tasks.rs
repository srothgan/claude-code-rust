// SPDX-License-Identifier: Apache-2.0
use super::{App, InvalidationLevel, MessageBlock, ToolCallInfo};
use crate::agent::model;
use std::collections::{BTreeSet, HashSet};

pub(super) fn apply_task_state_update(app: &mut App, update: model::TaskStateUpdate) {
    let previous_tasks = app.sdk_inventory.tasks.clone();
    let removed: HashSet<&str> =
        update.removed_task_ids.iter().map(std::string::String::as_str).collect();

    if update.is_complete_snapshot {
        if update.source == model::TaskUpdateSource::BackgroundTasks {
            let incoming: HashSet<&str> =
                update.tasks.iter().map(|task| task.task_id.as_str()).collect();
            app.sdk_inventory.tasks.retain(|task| {
                (!is_sdk_background_task(task) || incoming.contains(task.task_id.as_str()))
                    && !removed.contains(task.task_id.as_str())
            });
            for task in update.tasks {
                if !removed.contains(task.task_id.as_str()) {
                    if let Some(existing) = app
                        .sdk_inventory
                        .tasks
                        .iter_mut()
                        .find(|existing| existing.task_id == task.task_id)
                    {
                        *existing = task;
                    } else {
                        app.sdk_inventory.tasks.push(task);
                    }
                }
            }
            refresh_task_tool_displays_with_previous(app, &previous_tasks);
            return;
        }
        app.sdk_inventory.tasks = update
            .tasks
            .into_iter()
            .filter(|task| !removed.contains(task.task_id.as_str()))
            .collect();
        refresh_task_tool_displays_with_previous(app, &previous_tasks);
        return;
    }

    if !removed.is_empty() {
        app.sdk_inventory.tasks.retain(|task| !removed.contains(task.task_id.as_str()));
    }

    for task in update.tasks {
        if removed.contains(task.task_id.as_str()) {
            continue;
        }
        if let Some(existing) =
            app.sdk_inventory.tasks.iter_mut().find(|existing| existing.task_id == task.task_id)
        {
            *existing = task;
        } else {
            app.sdk_inventory.tasks.push(task);
        }
    }
    refresh_task_tool_displays_with_previous(app, &previous_tasks);
}

fn is_sdk_background_task(task: &model::TaskItem) -> bool {
    task.metadata
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .and_then(|metadata| metadata.get("sdk_background_task"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// Re-derive one task tool row after its own tool event. Returns whether the
/// block changed; the caller owns the cache-slot and layout sync.
pub(super) fn refresh_task_tool_display_at(app: &mut App, mi: usize, bi: usize) -> bool {
    let current_tasks = &app.sdk_inventory.tasks;
    let Some(MessageBlock::ToolCall(tool_call)) =
        app.transcript.messages.get_mut(mi).and_then(|message| message.blocks.get_mut(bi))
    else {
        return false;
    };
    if !refresh_task_tool_display(tool_call.as_mut(), current_tasks, &[]) {
        return false;
    }
    tool_call.invalidate_render_cache();
    true
}

fn refresh_task_tool_displays_with_previous(
    app: &mut App,
    previous_tasks: &[model::TaskItem],
) -> bool {
    let current_tasks = &app.sdk_inventory.tasks;
    let mut dirty_blocks = Vec::new();

    for (message_idx, message) in app.transcript.messages.iter_mut().enumerate() {
        for (block_idx, block) in message.blocks.iter_mut().enumerate() {
            let MessageBlock::ToolCall(tool_call) = block else {
                continue;
            };
            if refresh_task_tool_display(tool_call.as_mut(), current_tasks, previous_tasks) {
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
    let title = task_update_title(action, task_id, subject);
    // Once inventory removes a task, a later tool result cannot re-resolve its
    // subject. Keep the already resolved title for this same task and action.
    let retains_known_subject = subject.is_none()
        && tool_call.title.strip_prefix(&title).is_some_and(|suffix| suffix.starts_with(": "));
    let mut changed = !retains_known_subject && sync_string(&mut tool_call.title, title);

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
    use crate::app::test_support::{app_with_connection, deliver_update};
    use serde_json::json;

    fn inventory(app: &mut App, update: model::TaskStateUpdate) {
        deliver_update(app, model::SessionUpdate::TaskStateUpdate(update));
    }

    fn assert_tasks(app: &App, mut expected: Vec<model::TaskItem>) {
        let mut actual = app.sdk_inventory.tasks.clone();
        actual.sort_by(|a, b| a.task_id.cmp(&b.task_id));
        expected.sort_by(|a, b| a.task_id.cmp(&b.task_id));
        assert_eq!(actual, expected);
    }

    fn admit_task_tool(app: &mut App, id: &str, name: &str, input: serde_json::Value) {
        deliver_update(
            app,
            model::SessionUpdate::ToolCall(
                model::ToolCall::new(id, format!("{name} {id}"))
                    .status(model::ToolCallStatus::InProgress)
                    .raw_input(input)
                    .meta(json!({"claudeCode":{"toolName":name}})),
            ),
        );
    }

    fn transcript_rows(app: &mut App) -> crate::ui::inline_chat_rows::SerializedLiveRows {
        crate::ui::inline_chat_rows::serialize_live_rows_with_boundaries_excluding(
            app,
            100,
            &std::collections::BTreeSet::new(),
            None,
        )
    }

    fn card(app: &mut App, id: &str) -> Vec<String> {
        let rendered = transcript_rows(app);
        let target = crate::app::HistoryOutputId::ToolCall(id.to_owned());
        let rows: Vec<String> = rendered
            .segments()
            .iter()
            .filter(|segment| segment.ids.contains(&target))
            .flat_map(|segment| rendered.segment_rows(segment).expect("owned card rows"))
            .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect::<String>())
            .skip_while(|row| row.trim().is_empty())
            .collect();
        assert!(!rows.is_empty(), "missing rendered card {id}");
        rows
    }

    fn task(id: &str, subject: &str, status: model::TaskStatus) -> model::TaskItem {
        model::TaskItem::new(id, subject, status)
    }

    fn background_task(id: &str, subject: &str) -> model::TaskItem {
        let mut task = model::TaskItem::new(id, subject, model::TaskStatus::InProgress);
        task.metadata = Some(json!({ "sdk_background_task": true }));
        task
    }

    #[test]
    fn task_inventory_reconciles_deltas_removals_and_authoritative_snapshots() {
        for complete in [false, true] {
            let (mut app, _commands) = app_with_connection();
            inventory(
                &mut app,
                model::TaskStateUpdate::new(model::TaskUpdateSource::List)
                    .tasks(vec![
                        task("keep", "Unrelated subject", model::TaskStatus::Pending),
                        task("change", "Original subject", model::TaskStatus::Pending),
                        task("remove", "Removed subject", model::TaskStatus::InProgress),
                    ])
                    .complete_snapshot(true),
            );
            let update = model::TaskStateUpdate::new(model::TaskUpdateSource::Update)
                .tasks(vec![
                    task("change", "Changed subject", model::TaskStatus::Completed),
                    task("insert", "Inserted subject", model::TaskStatus::InProgress),
                    task("remove", "Must not resurrect", model::TaskStatus::Completed),
                ])
                .removed_task_ids(vec!["remove".into()])
                .complete_snapshot(complete);
            let before = app.sdk_inventory.tasks.clone();
            crate::app::handle_client_event(
                &mut app,
                crate::agent::events::ClientEvent::SessionUpdate {
                    session_id: "old-session".into(),
                    update: model::SessionUpdate::TaskStateUpdate(update.clone()),
                },
            );
            assert_tasks(&app, before);
            inventory(&mut app, update);
            let mut expected = vec![
                task("change", "Changed subject", model::TaskStatus::Completed),
                task("insert", "Inserted subject", model::TaskStatus::InProgress),
            ];
            if !complete {
                expected.push(task("keep", "Unrelated subject", model::TaskStatus::Pending));
            }
            assert_tasks(&app, expected.clone());
            inventory(&mut app, model::TaskStateUpdate::new(model::TaskUpdateSource::Update));
            assert_tasks(&app, expected);
            inventory(
                &mut app,
                model::TaskStateUpdate::new(model::TaskUpdateSource::List).complete_snapshot(true),
            );
            assert_tasks(&app, vec![]);
        }
    }

    #[test]
    fn background_snapshots_replace_only_their_membership_and_preserve_identity_across_sources() {
        let (mut app, _commands) = app_with_connection();
        let foreground = task("foreground", "Foreground survives", model::TaskStatus::Pending);
        inventory(
            &mut app,
            model::TaskStateUpdate::new(model::TaskUpdateSource::List)
                .tasks(vec![
                    foreground.clone(),
                    background_task("old", "Omitted background"),
                    background_task("retained", "Prior background"),
                ])
                .complete_snapshot(true),
        );
        inventory(
            &mut app,
            model::TaskStateUpdate::new(model::TaskUpdateSource::BackgroundTasks)
                .tasks(vec![
                    background_task("retained", "Updated background"),
                    background_task("new", "New background"),
                ])
                .complete_snapshot(true),
        );
        assert_tasks(
            &app,
            vec![
                foreground.clone(),
                background_task("retained", "Updated background"),
                background_task("new", "New background"),
            ],
        );
        inventory(
            &mut app,
            model::TaskStateUpdate::new(model::TaskUpdateSource::BackgroundTasks)
                .complete_snapshot(true),
        );
        assert_tasks(&app, vec![foreground.clone()]);
        let mut agent = task("agent", "Foreground execution", model::TaskStatus::InProgress);
        agent.metadata = Some(json!({"task_type":"local_agent"}));
        inventory(
            &mut app,
            model::TaskStateUpdate::new(model::TaskUpdateSource::Lifecycle).tasks(vec![agent]),
        );
        let mut background = background_task("agent", "Detached execution");
        background.metadata = Some(
            json!({"task_type":"local_agent","sdk_background_task":true,"summary":"Independent background progress"}),
        );
        inventory(
            &mut app,
            model::TaskStateUpdate::new(model::TaskUpdateSource::BackgroundTasks)
                .tasks(vec![background.clone()])
                .complete_snapshot(true),
        );
        assert_tasks(&app, vec![foreground.clone(), background]);
        let mut completed = task("agent", "Completed execution", model::TaskStatus::Completed);
        completed.metadata = Some(
            json!({"task_type":"local_agent","terminal_status":"completed","summary":"Final result"}),
        );
        inventory(
            &mut app,
            model::TaskStateUpdate::new(model::TaskUpdateSource::Lifecycle)
                .tasks(vec![completed.clone()]),
        );
        assert_tasks(&app, vec![foreground, completed]);
    }

    #[tokio::test]
    async fn inventory_events_do_not_fabricate_tool_execution_or_steal_the_active_turn() {
        let (mut app, mut commands) = app_with_connection();
        let tasks = tokio::task::LocalSet::new();
        tasks
            .run_until(async {
                app.input.set_text("inspect task ownership");
                crate::app::test_support::press(&mut app, crossterm::event::KeyCode::Enter);
            })
            .await;
        tokio::time::timeout(std::time::Duration::from_secs(2), tasks)
            .await
            .expect("submit finishes");
        let prompt = commands.try_recv().expect("admitted turn");
        assert!(
            matches!(prompt.command,crate::agent::wire::BridgeCommand::Prompt {ref session_id,..} if session_id=="session-1")
        );
        admit_task_tool(&mut app, "existing-tool", "Read", json!({"file_path":"owned.rs"}));
        let before = card(&mut app, "existing-tool");
        let owner = app.active_turn_assistant_idx();
        let transcript = transcript_rows(&mut app);
        let mut ambient = background_task("ambient-watch", "Housekeeping");
        ambient.metadata = Some(json!({"sdk_background_task":true,"ambient":true}));
        let ordinary = task("inventory-only", "Planning only", model::TaskStatus::Pending);
        inventory(
            &mut app,
            model::TaskStateUpdate::new(model::TaskUpdateSource::BackgroundTasks)
                .tasks(vec![ambient.clone()])
                .complete_snapshot(true),
        );
        inventory(
            &mut app,
            model::TaskStateUpdate::new(model::TaskUpdateSource::Update)
                .tasks(vec![ordinary.clone()]),
        );
        assert_tasks(&app, vec![ambient, ordinary]);
        assert_eq!(app.active_turn_assistant_idx(), owner);
        assert_eq!(transcript_rows(&mut app), transcript);
        assert_eq!(card(&mut app, "existing-tool"), before);
        assert_eq!(
            app.indexed_tool_calls().map(|(_, tool)| tool.id.as_str()).collect::<Vec<_>>(),
            ["existing-tool"]
        );
        for id in ["ambient-watch", "inventory-only"] {
            assert!(app.tool_call_scope(id).is_none());
        }
        admit_task_tool(&mut app, "actual-tool", "Read", json!({"file_path":"actual.rs"}));
        assert!(app.tool_call_scope("actual-tool").is_some());
        assert!(card(&mut app, "actual-tool")[0].contains("actual-tool"));
        assert_eq!(card(&mut app, "existing-tool"), before);
    }

    #[test]
    fn created_task_cards_follow_their_source_invocation_in_either_event_order() {
        for inventory_first in [false, true] {
            let (mut app, _commands) = app_with_connection();
            let mut first = task("41", "Alpha subject", model::TaskStatus::Pending);
            first.source_tool_call_id = Some("create-alpha".into());
            let mut second = task("82", "Beta subject", model::TaskStatus::Pending);
            second.source_tool_call_id = Some("create-beta".into());
            let update = model::TaskStateUpdate::new(model::TaskUpdateSource::Create)
                .tasks(vec![second.clone(), first.clone()]);
            if inventory_first {
                inventory(&mut app, update.clone());
            }
            admit_task_tool(
                &mut app,
                "create-alpha",
                "TaskCreate",
                json!({"subject":"Alpha subject","description":"Alpha body"}),
            );
            admit_task_tool(
                &mut app,
                "create-beta",
                "TaskCreate",
                json!({"subject":"Beta subject","description":"Beta body"}),
            );
            let _ = card(&mut app, "create-alpha");
            let _ = card(&mut app, "create-beta");
            if !inventory_first {
                inventory(&mut app, update);
            }
            for (id, number, subject, body, rival) in [
                ("create-alpha", "41", "Alpha subject", "Alpha body", "Beta"),
                ("create-beta", "82", "Beta subject", "Beta body", "Alpha"),
            ] {
                let rendered = card(&mut app, id);
                assert!(
                    rendered[0].contains(number) && rendered[0].contains(subject),
                    "{rendered:?}"
                );
                assert!(rendered[1..].iter().any(|row| row.contains(body)));
                assert!(!rendered.iter().any(|row| row.contains(rival)));
            }
            let beta = card(&mut app, "create-beta");
            first.subject = "Revised alpha".into();
            inventory(
                &mut app,
                model::TaskStateUpdate::new(model::TaskUpdateSource::Update).tasks(vec![first]),
            );
            assert!(card(&mut app, "create-alpha")[0].contains("Revised alpha"));
            assert_eq!(card(&mut app, "create-beta"), beta);
        }
    }

    #[test]
    fn task_update_cards_resolve_input_inventory_and_removed_subjects() {
        for inventory_first in [false, true] {
            for (
                status,
                subject,
                activity,
                expected_subject,
                expected_activity,
                inventory_activity,
            ) in [
                (
                    "in_progress",
                    None,
                    None,
                    "Inventory subject",
                    Some("Inventory activity"),
                    Some("Inventory activity"),
                ),
                ("in_progress", None, None, "Inventory subject", None, None),
                (
                    "running",
                    Some("Explicit subject"),
                    Some("Explicit activity"),
                    "Explicit subject",
                    Some("Explicit activity"),
                    Some("Inventory activity"),
                ),
                (
                    "in_progress",
                    None,
                    Some("   "),
                    "Inventory subject",
                    None,
                    Some("Inventory activity"),
                ),
                ("completed", None, None, "Inventory subject", None, Some("Inventory activity")),
                ("deleted", None, None, "Inventory subject", None, Some("Inventory activity")),
            ] {
                assert_update_case(
                    inventory_first,
                    status,
                    subject,
                    activity,
                    expected_subject,
                    expected_activity,
                    inventory_activity,
                );
            }
        }
        assert_completion_replaces_activity();
    }

    fn assert_update_case(
        inventory_first: bool,
        status: &str,
        subject: Option<&str>,
        activity: Option<&str>,
        expected_subject: &str,
        expected_activity: Option<&str>,
        inventory_activity: Option<&str>,
    ) {
        let (mut app, _commands) = app_with_connection();
        let mut target = task("target", "Inventory subject", model::TaskStatus::Pending);
        target.active_form = inventory_activity.map(str::to_owned);
        let neighbor = task("neighbor", "Unrelated task", model::TaskStatus::Pending);
        let update = model::TaskStateUpdate::new(model::TaskUpdateSource::List)
            .tasks(vec![neighbor.clone(), target]);
        let mut input = json!({"taskId":"target","status":status});
        if let Some(subject) = subject {
            input["subject"] = json!(subject);
        }
        if let Some(activity) = activity {
            input["activeForm"] = json!(activity);
        }
        if inventory_first {
            inventory(&mut app, update.clone());
        }
        admit_task_tool(&mut app, "unrelated", "Read", json!({"file_path":"neighbor.rs"}));
        deliver_update(
            &mut app,
            model::SessionUpdate::ToolCallUpdate(model::ToolCallUpdate::new(
                "unrelated",
                model::ToolCallUpdateFields::new()
                    .status(model::ToolCallStatus::Failed)
                    .content(vec![model::ToolCallContent::from("independent read diagnostic")]),
            )),
        );
        let unrelated = card(&mut app, "unrelated");
        admit_task_tool(&mut app, "update-target", "TaskUpdate", input.clone());
        let _ = card(&mut app, "update-target");
        if !inventory_first {
            inventory(&mut app, update);
        }
        if status == "deleted" {
            inventory(
                &mut app,
                model::TaskStateUpdate::new(model::TaskUpdateSource::Update)
                    .removed_task_ids(vec!["target".into()]),
            );
        }
        assert_update_card(&mut app, expected_subject, expected_activity);
        // A real terminal tool result must not retain stale derived activity for a completed/deleted operation.
        deliver_update(
            &mut app,
            model::SessionUpdate::ToolCallUpdate(model::ToolCallUpdate::new(
                "update-target",
                model::ToolCallUpdateFields::new()
                    .status(model::ToolCallStatus::Completed)
                    .content(vec![model::ToolCallContent::from("Activity: stale result activity")]),
            )),
        );
        assert_update_card(&mut app, expected_subject, expected_activity);
        assert!(!card(&mut app, "update-target").join("\n").contains("stale result activity"));
        if status == "deleted" {
            inventory(
                &mut app,
                model::TaskStateUpdate::new(model::TaskUpdateSource::Update)
                    .tasks(vec![neighbor.clone()]),
            );
            assert_update_card(&mut app, expected_subject, None);
            inventory(
                &mut app,
                model::TaskStateUpdate::new(model::TaskUpdateSource::Update).tasks(vec![task(
                    "target",
                    "Replacement subject",
                    model::TaskStatus::Pending,
                )]),
            );
            assert_update_card(&mut app, "Replacement subject", None);
        }
        assert_eq!(card(&mut app, "unrelated"), unrelated);
        assert!(app.sdk_inventory.tasks.contains(&neighbor));
    }

    fn assert_completion_replaces_activity() {
        let (mut app, _commands) = app_with_connection();
        inventory(
            &mut app,
            model::TaskStateUpdate::new(model::TaskUpdateSource::Update).tasks(vec![
                task("target", "Active then complete", model::TaskStatus::InProgress)
                    .active_form("Working through source"),
            ]),
        );
        admit_task_tool(
            &mut app,
            "start",
            "TaskUpdate",
            json!({"taskId":"target","status":"in_progress"}),
        );
        assert!(card(&mut app, "start")[1..].join("\n").contains("Working through source"));
        admit_task_tool(
            &mut app,
            "update-target",
            "TaskUpdate",
            json!({"taskId":"target","status":"completed"}),
        );
        let _ = card(&mut app, "update-target");
        deliver_update(
            &mut app,
            model::SessionUpdate::ToolCallUpdate(model::ToolCallUpdate::new(
                "update-target",
                model::ToolCallUpdateFields::new()
                    .status(model::ToolCallStatus::Completed)
                    .content(vec![model::ToolCallContent::from(
                        "Activity: Working through source",
                    )]),
            )),
        );
        inventory(
            &mut app,
            model::TaskStateUpdate::new(model::TaskUpdateSource::Update).tasks(vec![
                task("target", "Active then complete", model::TaskStatus::Completed)
                    .active_form("Working through source"),
            ]),
        );
        assert_update_card(&mut app, "Active then complete", None);
        assert_tasks(
            &app,
            vec![
                task("target", "Active then complete", model::TaskStatus::Completed)
                    .active_form("Working through source"),
            ],
        );
    }

    fn assert_update_card(app: &mut App, subject: &str, activity: Option<&str>) {
        let rendered = card(app, "update-target");
        assert!(rendered[0].contains("target") && rendered[0].contains(subject), "{rendered:?}");
        let body = rendered[1..].join("\n");
        if let Some(activity) = activity {
            assert!(body.contains(activity), "{body}");
        } else {
            assert!(!body.contains("Activity"), "{body}");
        }
        assert!(!body.contains("Unrelated task"));
    }

    #[test]
    fn failed_and_killed_task_updates_keep_their_diagnostic_through_inventory_reconciliation() {
        for outcome in [model::ToolCallStatus::Failed, model::ToolCallStatus::Killed] {
            for requested in ["in_progress", "completed", "deleted"] {
                let (mut app, _commands) = app_with_connection();
                inventory(
                    &mut app,
                    model::TaskStateUpdate::new(model::TaskUpdateSource::List).tasks(vec![task(
                        "target",
                        "Original subject",
                        model::TaskStatus::Pending,
                    )]),
                );
                admit_task_tool(
                    &mut app,
                    "failed-update",
                    "TaskUpdate",
                    json!({"taskId":"target","status":requested}),
                );
                deliver_update(
                    &mut app,
                    model::SessionUpdate::ToolCallUpdate(model::ToolCallUpdate::new(
                        "failed-update",
                        model::ToolCallUpdateFields::new().status(outcome).content(vec![
                            model::ToolCallContent::from("fixture task update was refused"),
                        ]),
                    )),
                );
                let before = card(&mut app, "failed-update");
                assert!(before[1..].join("\n").contains("fixture task update was refused"));
                assert_eq!(
                    app.sdk_inventory.tasks[0].status,
                    model::TaskStatus::Pending,
                    "failed invocation is not an inventory update"
                );
                admit_task_tool(
                    &mut app,
                    "successful-update",
                    "TaskUpdate",
                    json!({"taskId":"other","status":"in_progress"}),
                );
                let _ = card(&mut app, "successful-update");
                inventory(
                    &mut app,
                    model::TaskStateUpdate::new(model::TaskUpdateSource::Update).tasks(vec![
                        task("target", "Reconciled subject", model::TaskStatus::Completed)
                            .active_form("must not overwrite error"),
                        task("other", "Other subject", model::TaskStatus::InProgress)
                            .active_form("New independent activity"),
                    ]),
                );
                let after = card(&mut app, "failed-update");
                assert!(after[0].contains("target") && after[0].contains("Reconciled subject"));
                assert!(after[1..].join("\n").contains("fixture task update was refused"));
                assert!(
                    !after[1..].iter().any(|row| row.contains("must not overwrite error")
                        || row.contains("Deleted task"))
                );
                assert_eq!(
                    app.tool_call("failed-update").expect("failed invocation").status,
                    outcome
                );
                let success = card(&mut app, "successful-update");
                assert!(
                    success[0].contains("other")
                        && success[1..].join("\n").contains("New independent activity")
                );
            }
        }
    }
}
