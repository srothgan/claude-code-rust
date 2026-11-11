// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::agent::events::ClientEvent;
use crate::app::test_support::*;
use crossterm::event::KeyCode;
use pretty_assertions::assert_eq;
use ratatui::style::Modifier;

const RETAINED_BUDGET: usize = 64 * 1024;

fn old_history(app: &mut App) {
    for label in ["OLD_ALPHA", "OLD_BETA"] {
        app.push_message_tracked(user_text_message(&format!("{label}{}", "x".repeat(128 * 1024))));
    }
}

fn message(app: &App, id: ChatMessageId) -> &ChatMessage {
    app.transcript.messages.iter().find(|m| m.id == id).expect("retained message identity")
}

fn text(message: &ChatMessage) -> String {
    message
        .blocks
        .iter()
        .filter_map(|block| match block {
            MessageBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect()
}

fn assert_selected_control(app: &mut App, id: &str, label: &str) {
    let (mi, _) = app.lookup_tool_call(id).expect("live indexed interaction");
    let is_dialog =
        matches!(app.transcript.messages[mi].blocks.first(), Some(MessageBlock::UserDialog(_)));
    let rows = crate::ui::inline_chat_rows::serialize_live_rows_with_boundaries_excluding(
        app,
        120,
        &std::collections::BTreeSet::new(),
        None,
    );
    let segment = rows
        .segments()
        .iter()
        .find(|s| {
            s.msg_idx == mi && (is_dialog || s.ids.contains(&HistoryOutputId::ToolCall(id.into())))
        })
        .expect("addressed interaction region");
    let region = rows.segment_rows(segment).expect("interaction rows");
    assert!(
        region.iter().flat_map(|row| &row.spans).any(|span| span.content.ends_with(label)
            && span.style.add_modifier.contains(Modifier::BOLD)),
        "selected label must belong to {id}: {region:?}"
    );
    assert!(
        region.iter().any(|row| row.to_string().to_lowercase().contains("enter")),
        "confirmation control is visible"
    );
}

#[test]
fn cache_validity_restoration_and_shorter_replacement_preserve_complete_contents() {
    for invalidate in [false, true] {
        let mut cache = BlockCache::default();
        assert!(cache.get().is_none());
        cache.store(vec![Line::from("OLD first"), Line::from("OLD trailing")]);
        assert_eq!(
            cache.get().unwrap().iter().map(ToString::to_string).collect::<Vec<_>>(),
            ["OLD first", "OLD trailing"]
        );
        if invalidate {
            cache.invalidate();
            assert!(cache.get().is_none(), "source invalidation rejects stale rows");
        }
        cache.store(vec![Line::from("NEW only")]);
        assert_eq!(
            cache.get().unwrap().iter().map(ToString::to_string).collect::<Vec<_>>(),
            ["NEW only"]
        );
    }
}

fn history_notice(app: &mut App, dropped: &str) -> ChatMessageId {
    let notices: Vec<_> = app
        .transcript
        .messages
        .iter()
        .enumerate()
        .filter(|(_, m)| matches!(m.role, MessageRole::System(_)))
        .map(|(i, m)| (i, m.id))
        .collect();
    assert_eq!(notices.len(), 1, "one cumulative history notice");
    let rendered = transcript_region(app, notices[0].0);
    assert!(rendered.contains(dropped), "updated dropped-message information: {rendered}");
    notices[0].1
}

#[test]
fn tracked_retention_preserves_newer_history_and_updates_one_notice_on_later_pruning() {
    let mut app = App::test_default();
    let welcome =
        ChatMessage::welcome("fixture-version", "fixture-plan", "/fixture", "fixture-session");
    let welcome_id = welcome.id;
    app.push_message_tracked(welcome);
    old_history(&mut app);
    let recent = user_text_message("RECENT first survivor");
    let recent_id = recent.id;
    app.push_message_tracked(recent);
    let original: Vec<_> = app.transcript.messages.iter().map(|m| (m.id, text(m))).collect();
    app.enforce_history_retention_tracked();
    assert_eq!(
        app.transcript.messages.iter().map(|m| (m.id, text(m))).collect::<Vec<_>>(),
        original
    );
    app.history_retention.max_bytes = RETAINED_BUDGET;
    app.enforce_history_retention_tracked();
    assert_eq!(text(message(&app, recent_id)), "RECENT first survivor");
    let MessageBlock::Welcome(welcome) = &message(&app, welcome_id).blocks[0] else {
        panic!("welcome remains");
    };
    assert_eq!(welcome.cwd, "/fixture");
    assert!(app.transcript.messages.iter().all(|m| !text(m).contains("OLD_")));
    let notice_id = history_notice(&mut app, "dropped 2 messages");
    app.enforce_history_retention_tracked();
    assert_eq!(history_notice(&mut app, "dropped 2 messages"), notice_id);
    assert_eq!(text(message(&app, recent_id)), "RECENT first survivor");
    app.push_message_tracked(user_text_message(&format!("LATER_LARGE{}", "y".repeat(128 * 1024))));
    let newest = user_text_message("NEWEST second survivor");
    let newest_id = newest.id;
    app.push_message_tracked(newest);
    app.enforce_history_retention_tracked();
    assert_eq!(history_notice(&mut app, "dropped 4 messages"), notice_id);
    assert_eq!(text(message(&app, newest_id)), "NEWEST second survivor");
    assert!(
        app.transcript
            .messages
            .iter()
            .all(|m| m.id != recent_id && !text(m).contains("LATER_LARGE"))
    );
    assert!(matches!(message(&app, welcome_id).role, MessageRole::Welcome));
}

#[test]
fn relocated_tools_route_later_output_to_the_retained_target_only() {
    for status in [
        model::ToolCallStatus::Pending,
        model::ToolCallStatus::InProgress,
        model::ToolCallStatus::Detached,
        model::ToolCallStatus::Completed,
    ] {
        let (mut app, _commands) = app_with_connection();
        old_history(&mut app);
        let target = assistant_bash_tool_message("target", status, "target-terminal");
        let id = target.id;
        let before = app.transcript.messages.len();
        app.push_message_tracked(target);
        app.index_tool_call("target".into(), before, 0);
        let neighbor = assistant_bash_tool_message(
            "neighbor",
            model::ToolCallStatus::Completed,
            "neighbor-terminal",
        );
        let neighbor_index = app.transcript.messages.len();
        app.push_message_tracked(neighbor);
        app.index_tool_call("neighbor".into(), neighbor_index, 0);
        let neighbor_output = app.tool_call("neighbor").unwrap().terminal_output.clone();
        app.history_retention.max_bytes = RETAINED_BUDGET;
        app.enforce_history_retention_tracked();
        assert!(app.transcript.messages.iter().all(|m| !text(m).contains("OLD_")));
        assert_ne!(
            app.transcript.messages.iter().position(|m| m.id == id),
            Some(before),
            "fixture must force relocation"
        );
        deliver_update(
            &mut app,
            model::SessionUpdate::ToolCallUpdate(model::ToolCallUpdate::new(
                "target",
                model::ToolCallUpdateFields::new()
                    .status(model::ToolCallStatus::Completed)
                    .raw_output(serde_json::json!("TARGET after pruning")),
            )),
        );
        let tool = app.tool_call("target").expect("original target remains addressable");
        assert_eq!(tool.terminal_output.as_deref(), Some("TARGET after pruning"));
        assert_eq!(tool.terminal_id.as_deref(), Some("target-terminal"));
        assert_eq!(tool.status, model::ToolCallStatus::Completed);
        assert_eq!(app.tool_call("neighbor").unwrap().terminal_output, neighbor_output);
    }
}

#[test]
fn retained_permissions_deliver_the_selected_response_after_real_relocation() {
    let (mut app, _commands) = app_with_connection();
    old_history(&mut app);
    let mut receivers = Vec::new();
    for id in ["target", "neighbor"] {
        admit_tool(
            &mut app,
            model::ToolCall::new(id, format!("{id} approval"))
                .status(model::ToolCallStatus::InProgress),
        );
        receivers.push(request_permission(
            &mut app,
            model::RequestPermissionRequest::new(
                "session-1",
                model::ToolCallUpdate::new(id, model::ToolCallUpdateFields::new()),
                permission_options(id),
                None,
            ),
        ));
    }
    let before = app.lookup_tool_call("target").unwrap().0;
    app.history_retention.max_bytes = RETAINED_BUDGET;
    app.enforce_history_retention_tracked();
    assert_ne!(app.lookup_tool_call("target").unwrap().0, before);
    assert_pending(&mut receivers[0]);
    assert_pending(&mut receivers[1]);
    press(&mut app, KeyCode::Right);
    assert_selected_control(&mut app, "target", "target always");
    press(&mut app, KeyCode::Enter);
    assert_permission_response(&mut receivers[0], "target-always");
    assert_pending(&mut receivers[1]);
    press(&mut app, KeyCode::Esc);
    assert_permission_response(&mut receivers[1], "neighbor-deny");
}

#[test]
fn retained_user_dialog_delivers_its_selected_option_after_real_relocation() {
    let (mut app, _commands) = app_with_connection();
    old_history(&mut app);
    let mut receivers = Vec::new();
    for id in ["target-dialog", "neighbor-dialog"] {
        let (tx, rx) = tokio::sync::oneshot::channel();
        crate::app::handle_client_event(
            &mut app,
            ClientEvent::UserDialogRequest {
                session_id: "session-1".into(),
                request: model::RequestUserDialogRequest::new(
                    "session-1",
                    id,
                    "refusal_fallback_prompt",
                    model::RefusalFallbackPayload {
                        original_model: "fixture-original".into(),
                        fallback_model: "fixture-fallback".into(),
                        ..Default::default()
                    },
                    vec![
                        model::UserDialogOption::new(format!("{id}-one"), format!("{id} first")),
                        model::UserDialogOption::new(format!("{id}-two"), format!("{id} second")),
                    ],
                ),
                response_tx: tx,
            },
        );
        receivers.push(rx);
    }
    let before = app.lookup_tool_call("target-dialog").unwrap().0;
    app.history_retention.max_bytes = RETAINED_BUDGET;
    app.enforce_history_retention_tracked();
    assert_ne!(app.lookup_tool_call("target-dialog").unwrap().0, before);
    press(&mut app, KeyCode::Down);
    assert_selected_control(&mut app, "target-dialog", "target-dialog second");
    assert_pending(&mut receivers[0]);
    assert_pending(&mut receivers[1]);
    press(&mut app, KeyCode::Enter);
    assert_eq!(
        receivers[0].try_recv().expect("original live dialog receiver").outcome,
        model::RequestUserDialogOutcome::Selected(model::SelectedUserDialogOutcome::new(
            "target-dialog-two"
        ))
    );
    assert_pending(&mut receivers[1]);
    press(&mut app, KeyCode::Esc);
    assert_eq!(
        receivers[1].try_recv().unwrap().outcome,
        model::RequestUserDialogOutcome::Cancelled
    );
}

#[test]
fn retained_active_assistant_receives_continuation_after_indices_really_move() {
    for initial in ["", "INITIAL source"] {
        let (mut app, _commands) = app_with_connection();
        old_history(&mut app);
        app.start_assistant_response();
        if !initial.is_empty() {
            deliver_update(&mut app, source_chunk(initial, "active-source", false));
        }
        let before = app.active_turn_assistant_idx().unwrap();
        let owner = app.transcript.messages[before].id;
        let neighbor = system_text_message("NEIGHBOR physical tail");
        let neighbor_id = neighbor.id;
        app.push_message_tracked(neighbor);
        app.history_retention.max_bytes = RETAINED_BUDGET;
        app.enforce_history_retention_tracked();
        assert_ne!(app.transcript.messages.iter().position(|m| m.id == owner), Some(before));
        deliver_update(&mut app, source_chunk(" continuation", "active-source", false));
        assert_eq!(text(message(&app, owner)), format!("{initial} continuation"));
        assert_eq!(text(message(&app, neighbor_id)), "NEIGHBOR physical tail");
        assert_eq!(
            app.transcript
                .messages
                .iter()
                .filter(|m| matches!(m.role, MessageRole::Assistant))
                .map(|m| m.id)
                .collect::<Vec<_>>(),
            [owner]
        );
    }
}

#[test]
fn resource_link_accounting_distinguishes_owned_allocations_with_fixed_container_shape() {
    fn linked(uri: String, title: String, annotation: String) -> ChatMessage {
        let mut msg = assistant_tool_message("fixed-tool", model::ToolCallStatus::Completed);
        let MessageBlock::ToolCall(tool) = &mut msg.blocks[0] else { unreachable!() };
        tool.content = vec![model::ToolCallContent::ResourceLink(
            model::McpResourceLink::new(uri, "fixed-name").title(Some(title)).annotations(Some(
                std::collections::BTreeMap::from([(
                    "fixture".into(),
                    serde_json::Value::String(annotation),
                )]),
            )),
        )];
        msg
    }
    let baseline = App::measure_message_bytes(&linked(String::new(), String::new(), String::new()));
    for (uri, title, annotation, expected) in [
        ("u".repeat(16384), String::new(), String::new(), 16384),
        (String::new(), "t".repeat(32768), String::new(), 32768),
        (String::new(), String::new(), "a".repeat(65536), 65536),
    ] {
        assert_eq!(
            App::measure_message_bytes(&linked(uri, title, annotation)) - baseline,
            expected
        );
    }
}

#[test]
fn turn_finalization_preserves_the_running_terminal_identity_command_and_output() {
    let mut app = App::test_default();
    let mut msg = assistant_bash_tool_message(
        "running-bash",
        model::ToolCallStatus::InProgress,
        "distinct-terminal",
    );
    let MessageBlock::ToolCall(tool) = &mut msg.blocks[0] else { unreachable!() };
    tool.terminal_command = Some("printf 'distinct command'".into());
    tool.terminal_output = Some("distinct output\n  indentation\n".into());
    app.push_message_tracked(msg);
    app.index_tool_call("running-bash".into(), 0, 0);
    app.finalize_turn_runtime_artifacts(model::ToolCallStatus::Completed);
    let tool = app.tool_call("running-bash").expect("same finalized tool");
    assert_eq!(tool.status, model::ToolCallStatus::Completed);
    assert_eq!(tool.terminal_id.as_deref(), Some("distinct-terminal"));
    assert_eq!(tool.terminal_command.as_deref(), Some("printf 'distinct command'"));
    assert_eq!(tool.terminal_output.as_deref(), Some("distinct output\n  indentation\n"));
}
