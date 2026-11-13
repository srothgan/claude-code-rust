// SPDX-License-Identifier: Apache-2.0

use super::{
    LiveRowBoundaryKind, SerializedLiveRows, serialize_live_rows_with_boundaries_excluding,
};
use crate::agent::{events::ClientEvent, model};
use crate::app::test_support::{app_with_connection, deliver_update};
use crate::app::{App, AppStatus, HistoryOutputId, MessageBlock, ToolCallScope};
use ratatui::text::Line;
use serde_json::json;
use std::collections::BTreeSet;

fn rows(app: &mut App) -> SerializedLiveRows {
    serialize_live_rows_with_boundaries_excluding(app, 100, &BTreeSet::new(), None)
}
fn card(app: &mut App, id: &str) -> Vec<Line<'static>> {
    let rendered = rows(app);
    let target = HistoryOutputId::ToolCall(id.into());
    rendered
        .segments()
        .iter()
        .filter(|segment| segment.ids.contains(&target))
        .flat_map(|segment| rendered.segment_rows(segment).unwrap().iter().cloned())
        .collect()
}
fn text(lines: &[Line<'_>]) -> String {
    lines.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n")
}
fn admit(app: &mut App, id: &str, name: &str, title: &str, body: &str) {
    deliver_update(
        app,
        model::SessionUpdate::ToolCall(
            model::ToolCall::new(id, title)
                .kind(match name {
                    "Write" => model::ToolKind::Edit,
                    "Task" | "Agent" => model::ToolKind::Think,
                    _ => model::ToolKind::Other,
                })
                .status(model::ToolCallStatus::InProgress)
                .content(vec![model::ToolCallContent::from(body)])
                .meta(json!({"claudeCode":{"toolName":name}})),
        ),
    );
}
fn patch(app: &mut App, id: &str, fields: model::ToolCallUpdateFields) {
    deliver_update(
        app,
        model::SessionUpdate::ToolCallUpdate(model::ToolCallUpdate::new(id, fields)),
    );
}
fn chunk(app: &mut App, source: &str) {
    deliver_update(
        app,
        model::SessionUpdate::AgentMessageChunk(model::ContentChunk::new(
            model::ContentBlock::Text(model::TextContent::new(source)),
        )),
    );
}
fn complete(app: &mut App) {
    crate::app::handle_client_event(
        app,
        ClientEvent::TurnComplete {
            session_id: "session-1".into(),
            queued_turn_count: None,
            terminal_reason: None,
        },
    );
}
fn source(app: &App, id: &str) -> String {
    let tc = app.tool_call(id).unwrap();
    tc.content
        .iter()
        .map(|block| {
            let model::ToolCallContent::Content(content) = block else { panic!("text content") };
            let model::ContentBlock::Text(text) = &content.content else { panic!("text block") };
            text.text.as_str()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn targeted_tool_patches_preserve_independent_bodies_titles_and_invocation_outcomes() {
    for name in ["Write", "Task", "Agent"] {
        let (mut app, _commands) = app_with_connection();
        app.cwd_raw = "/project".into();
        admit(&mut app, "alpha", name, "Read /project/src/alpha.rs", "Alpha original body");
        admit(&mut app, "beta", name, "Read /project/src/beta.rs", "Beta original body");
        assert!(text(&card(&mut app, "alpha")).contains("Read src/alpha.rs"));
        assert_eq!(app.tool_call("alpha").unwrap().title, "Read src/alpha.rs");
        let alpha = card(&mut app, "alpha");
        patch(
            &mut app,
            "beta",
            model::ToolCallUpdateFields::new()
                .title("Read /project/src/revised.rs")
                .status(model::ToolCallStatus::Completed)
                .content(vec!["Beta complete body".into()]),
        );
        assert_eq!(source(&app, "beta"), "Beta complete body");
        assert_eq!(app.tool_call("beta").unwrap().title, "Read src/revised.rs");
        let beta = card(&mut app, "beta");
        assert!(text(&beta).contains("Beta complete body"));
        assert!(!text(&beta).contains("Beta original body"));
        assert_eq!(card(&mut app, "alpha"), alpha);
        let mut headers = Vec::new();
        for (status, body) in [
            (model::ToolCallStatus::Pending, "Alpha queued"),
            (model::ToolCallStatus::Completed, "Alpha successful"),
            (model::ToolCallStatus::Failed, "Alpha failure cause"),
            (model::ToolCallStatus::Killed, "Alpha cancelled cause"),
        ] {
            // Each outcome is a separate invocation lifecycle, not success turning into failure.
            let id = format!("alpha-{status:?}");
            admit(&mut app, &id, name, "Independent alpha outcome", "Stale alpha body");
            let _ = card(&mut app, &id);
            patch(
                &mut app,
                &id,
                model::ToolCallUpdateFields::new().status(status).content(vec![body.into()]),
            );
            assert_eq!(source(&app, &id), body);
            assert_eq!(app.tool_call(&id).unwrap().status, status);
            let rendered = card(&mut app, &id);
            let content = text(&rendered);
            assert!(content.contains(body) && !content.contains("Stale alpha"));
            headers.push(
                rendered
                    .iter()
                    .find(|line| line.to_string().contains("Independent alpha outcome"))
                    .unwrap()
                    .clone(),
            );
            patch(&mut app, &id, model::ToolCallUpdateFields::new().title("Retitled alpha"));
            assert_eq!(source(&app, &id), body);
            assert_eq!(app.tool_call(&id).unwrap().status, status);
            assert_eq!(card(&mut app, "beta"), beta);
        }
        assert_ne!(headers[0], headers[1]);
        assert_ne!(headers[1], headers[2]);
        assert_ne!(headers[1], headers[3]);
        patch(
            &mut app,
            "alpha",
            model::ToolCallUpdateFields::new().content(vec!["Alpha final replacement".into()]),
        );
        assert_eq!(source(&app, "alpha"), "Alpha final replacement");
        assert_eq!(card(&mut app, "beta"), beta);
    }
}

fn assert_rust_source(lines: &[Line<'_>], source: &str) -> Vec<ratatui::style::Style> {
    let row = lines
        .iter()
        .find(|line| line.to_string().contains(source))
        .expect("exact supplied source on its card");
    row.spans.iter().filter(|span| !span.content.trim().is_empty()).map(|span| span.style).collect()
}

#[test]
fn late_tool_names_and_location_patches_reselect_read_syntax_on_warmed_cards() {
    let source = "fn fixture() { let answer = 42; }";
    for initial_read in [false, true] {
        let (mut app, _commands) = app_with_connection();
        for id in ["target", "neighbor"] {
            let name = if initial_read { "Read" } else { "UnknownTool" };
            deliver_update(
                &mut app,
                model::SessionUpdate::ToolCall(
                    model::ToolCall::new(id, format!("Inspect {id}"))
                        .kind(model::ToolKind::Read)
                        .status(model::ToolCallStatus::Completed)
                        .locations(vec![
                            model::ToolCallLocation::new(if initial_read {
                                "unknown.zzz"
                            } else {
                                "src/main.rs"
                            })
                            .line(12),
                        ])
                        .content(vec![source.into()])
                        .meta(json!({"claudeCode":{"toolName":name}})),
                ),
            );
        }
        let plain = card(&mut app, "target");
        let neighbor = card(&mut app, "neighbor");
        let plain_styles = assert_rust_source(&plain, source);
        let fields = if initial_read {
            model::ToolCallUpdateFields::new()
                .locations(vec![model::ToolCallLocation::new("src/main.rs").line(47)])
        } else {
            model::ToolCallUpdateFields::new()
        };
        let update = model::ToolCallUpdate::new("target", fields)
            .meta(json!({"claudeCode":{"toolName":"Read"}}));
        deliver_update(&mut app, model::SessionUpdate::ToolCallUpdate(update));
        let highlighted = card(&mut app, "target");
        let styles = assert_rust_source(&highlighted, source);
        assert_ne!(
            styles, plain_styles,
            "actual source syntax changes; title has no language hint"
        );
        assert!(
            styles.windows(2).any(|pair| pair[0] != pair[1]),
            "syntax distinguishes source tokens"
        );
        assert_eq!(card(&mut app, "neighbor"), neighbor);
        assert_eq!(
            app.tool_call("target").unwrap().locations,
            vec![model::ToolCallLocation::new("src/main.rs").line(if initial_read {
                47
            } else {
                12
            })]
        );
        patch(
            &mut app,
            "target",
            model::ToolCallUpdateFields::new().status(model::ToolCallStatus::Completed),
        );
        assert_eq!(card(&mut app, "target"), highlighted);
        deliver_update(
            &mut app,
            model::SessionUpdate::ToolCall(
                model::ToolCall::new("initial-rust", "Inspect initial")
                    .kind(model::ToolKind::Read)
                    .status(model::ToolCallStatus::Completed)
                    .locations(vec![model::ToolCallLocation::new("src/main.rs").line(12)])
                    .content(vec![source.into()])
                    .meta(json!({"claudeCode":{"toolName":"Read"}})),
            ),
        );
        assert_eq!(assert_rust_source(&card(&mut app, "initial-rust"), source), styles);
        assert_eq!(
            app.tool_call("initial-rust").unwrap().locations,
            vec![model::ToolCallLocation::new("src/main.rs").line(12)]
        );
    }
}

#[test]
fn explicit_parent_links_hide_only_their_child_before_or_after_parent_arrival() {
    for late_link in [false, true] {
        for parent_first in [false, true] {
            let (mut app, _commands) = app_with_connection();
            if parent_first {
                admit(&mut app, "parent", "Agent", "Parent launch sentinel", "Parent launch body");
            }
            admit(&mut app, "main", "UnknownTool", "Main tool sentinel", "Main body sentinel");
            let child = model::ToolCall::new("child", "Child tool sentinel")
                .status(model::ToolCallStatus::InProgress)
                .content(vec!["Private child body sentinel".into()]);
            let child = if late_link {
                child
            } else {
                child.meta(
                    json!({"claudeCode":{"toolName":"UnknownTool","parentToolUseId":"parent"}}),
                )
            };
            deliver_update(&mut app, model::SessionUpdate::ToolCall(child));
            let main = card(&mut app, "main");
            if late_link {
                assert!(text(&card(&mut app, "child")).contains("Private child body sentinel"));
                deliver_update(
                    &mut app,
                    model::SessionUpdate::ToolCallUpdate(
                        model::ToolCallUpdate::new("child", model::ToolCallUpdateFields::new())
                            .meta(json!({"claudeCode":{"parentToolUseId":"parent"}})),
                    ),
                );
            }
            assert_eq!(
                app.tool_call_scope("child"),
                Some(ToolCallScope::SubagentChild { parent_tool_use_id: "parent".into() })
            );
            assert!(card(&mut app, "child").is_empty());
            assert_eq!(card(&mut app, "main"), main);
            assert!(!text(rows(&mut app).rows()).contains("Private child body sentinel"));
            if !parent_first {
                admit(&mut app, "parent", "Agent", "Parent launch sentinel", "Parent launch body");
            }
            assert_eq!(
                app.tool_call_scope("child"),
                Some(ToolCallScope::SubagentChild { parent_tool_use_id: "parent".into() })
            );
            assert!(card(&mut app, "child").is_empty());
            assert!(text(&card(&mut app, "parent")).contains("Parent launch sentinel"));
            assert_eq!(card(&mut app, "main"), main);
            assert_eq!(app.tool_call_scope("main"), Some(ToolCallScope::MainAgent));
        }
    }
}

#[test]
fn generic_tool_input_cannot_replace_confirmed_sdk_task_inventory() {
    let (mut app, _commands) = app_with_connection();
    let original = vec![
        model::TaskItem::new("one", "Confirmed alpha", model::TaskStatus::Pending),
        model::TaskItem::new("two", "Confirmed beta", model::TaskStatus::InProgress),
    ];
    deliver_update(
        &mut app,
        model::SessionUpdate::TaskStateUpdate(
            model::TaskStateUpdate::new(model::TaskUpdateSource::List)
                .tasks(original.clone())
                .complete_snapshot(true),
        ),
    );
    let conflicting =
        json!({"todos":[{"id":"one","content":"Untrusted overwrite","status":"completed"}]});
    deliver_update(
        &mut app,
        model::SessionUpdate::ToolCall(
            model::ToolCall::new("todo", "Legacy tool sentinel")
                .raw_input(conflicting.clone())
                .content(vec!["Generic result sentinel".into()])
                .meta(json!({"claudeCode":{"toolName":"TodoWrite"}})),
        ),
    );
    assert_eq!(app.sdk_inventory.tasks, original);
    assert!(text(&card(&mut app, "todo")).contains("Generic result sentinel"));
    admit(&mut app, "renamed", "UnknownTool", "Patched legacy sentinel", "Before patch");
    let _ = card(&mut app, "renamed");
    deliver_update(
        &mut app,
        model::SessionUpdate::ToolCallUpdate(
            model::ToolCallUpdate::new(
                "renamed",
                model::ToolCallUpdateFields::new()
                    .raw_input(conflicting)
                    .content(vec!["Patched generic result".into()]),
            )
            .meta(json!({"claudeCode":{"toolName":"TodoWrite"}})),
        ),
    );
    assert_eq!(app.sdk_inventory.tasks, original);
    assert!(text(&card(&mut app, "renamed")).contains("Patched generic result"));
    let changed =
        model::TaskItem::new("one", "Authoritative replacement", model::TaskStatus::Completed);
    deliver_update(
        &mut app,
        model::SessionUpdate::TaskStateUpdate(
            model::TaskStateUpdate::new(model::TaskUpdateSource::Update)
                .tasks(vec![changed.clone()]),
        ),
    );
    assert_eq!(app.sdk_inventory.tasks, vec![changed, original[1].clone()]);
    assert!(text(&card(&mut app, "todo")).contains("Generic result sentinel"));
}

#[test]
fn interleaved_tool_results_keep_their_source_order_through_turn_completion() {
    let (mut app, _commands) = app_with_connection();
    chunk(&mut app, "Earlier committed history");
    complete(&mut app);
    let history = rows(&mut app);
    let old_id = app.transcript.messages[0].id;
    deliver_update(&mut app, model::SessionUpdate::AgentResponseStarted);
    chunk(&mut app, "Before ");
    chunk(&mut app, "alpha");
    admit(&mut app, "alpha", "UnknownTool", "Alpha tool title", "Alpha pending body");
    chunk(&mut app, "Between tools");
    admit(&mut app, "beta", "UnknownTool", "Beta tool title", "Beta pending body");
    chunk(&mut app, "After beta");
    let owner = app.active_turn_assistant_idx().unwrap();
    let reply_id = app.transcript.messages[owner].id;
    let _ = rows(&mut app);
    patch(
        &mut app,
        "beta",
        model::ToolCallUpdateFields::new()
            .status(model::ToolCallStatus::Failed)
            .content(vec!["Beta diagnostic sentinel".into()]),
    );
    patch(
        &mut app,
        "alpha",
        model::ToolCallUpdateFields::new()
            .status(model::ToolCallStatus::Completed)
            .content(vec!["Alpha completed sentinel".into()]),
    );
    complete(&mut app);
    assert_eq!(app.transcript.messages[0].id, old_id);
    assert_eq!(app.transcript.messages[owner].id, reply_id);
    let sequence: Vec<_> = app.transcript.messages[owner]
        .blocks
        .iter()
        .filter_map(|block| match block {
            MessageBlock::Text(t) => Some(t.text.clone()),
            MessageBlock::ToolCall(tc) => Some(tc.id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(sequence, ["Before alpha", "alpha", "Between tools", "beta", "After beta"]);
    let rendered = rows(&mut app);
    let own: Vec<_> = rendered
        .segments()
        .iter()
        .filter(|s| s.msg_idx == owner)
        .flat_map(|s| rendered.segment_rows(s).unwrap().iter().cloned())
        .collect();
    let body = text(&own);
    let expected = [
        "Before alpha",
        "Alpha tool title",
        "Alpha completed sentinel",
        "Between tools",
        "Beta tool title",
        "Beta diagnostic sentinel",
        "After beta",
    ];
    let mut previous = 0;
    for marker in expected {
        let at = body.find(marker).expect(marker);
        assert!(at >= previous);
        assert_eq!(body.matches(marker).count(), 1);
        previous = at + marker.len();
    }
    let old: Vec<_> = rendered
        .segments()
        .iter()
        .filter(|s| s.msg_idx == 0)
        .flat_map(|s| rendered.segment_rows(s).unwrap().iter().cloned())
        .collect();
    assert_eq!(text(&old), text(history.rows()));
}

#[test]
fn terminal_tool_results_do_not_infer_a_new_activity_phase() {
    for mixed in [false, true] {
        let (mut app, _commands) = app_with_connection();
        deliver_update(
            &mut app,
            model::SessionUpdate::RuntimeSessionStateUpdate(model::RuntimeSessionState::Running),
        );
        deliver_update(
            &mut app,
            model::SessionUpdate::AgentActivityUpdate(model::AgentActivityPhase::Thinking),
        );
        admit(&mut app, "a", "UnknownTool", "Activity A", "A output");
        admit(&mut app, "b", "UnknownTool", "Activity B", "B output");
        deliver_update(
            &mut app,
            model::SessionUpdate::AgentActivityUpdate(model::AgentActivityPhase::Working),
        );
        for (id, status) in [
            ("a", model::ToolCallStatus::Completed),
            (
                "b",
                if mixed {
                    model::ToolCallStatus::Failed
                } else {
                    model::ToolCallStatus::Completed
                },
            ),
        ] {
            patch(&mut app, id, model::ToolCallUpdateFields::new().status(status));
            let rendered = rows(&mut app);
            assert!(
                !rendered
                    .segments()
                    .iter()
                    .any(|s| s.kind == LiveRowBoundaryKind::AssistantThinking)
            );
            assert_eq!(app.status, AppStatus::Running);
        }
        let activity =
            crate::ui::activity_rows::build_activity_rows(&app, 100, std::time::Instant::now());
        assert!(!activity.is_empty());
        deliver_update(
            &mut app,
            model::SessionUpdate::AgentActivityUpdate(model::AgentActivityPhase::Thinking),
        );
        let rendered = rows(&mut app);
        let thinking = rendered
            .segments()
            .iter()
            .find(|s| s.kind == LiveRowBoundaryKind::AssistantThinking)
            .expect("only SDK phase creates thinking");
        assert!(!thinking.commit_ready);
        assert!(text(rendered.segment_rows(thinking).unwrap()).contains("Thinking"));
        complete(&mut app);
        assert_eq!(app.status, AppStatus::Ready);
        assert!(
            !rows(&mut app)
                .segments()
                .iter()
                .any(|s| s.kind == LiveRowBoundaryKind::AssistantThinking)
        );
    }
}
