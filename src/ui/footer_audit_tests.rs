// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::agent::{
    events::ClientEvent,
    model::{McpServerConnectionStatus, McpServerStatus},
};
use crate::app::footer::FooterColor;
use crate::app::{AppStatus, ChatMessage, ModeState};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    widgets::{Paragraph, Widget},
};
use serde_json::json;

fn layout(fields: &serde_json::Value) -> FooterLayout {
    FooterLayout::from_saved(Some(fields)).unwrap()
}
fn draw(app: &App, layout: &FooterLayout, width: u16) -> Buffer {
    render_line(serialize_footer(app, layout, width), width)
}
fn draw_current(app: &App, width: u16) -> Buffer {
    render_line(serialize_footer_rows(app, width).rows[0].clone(), width)
}
fn render_line(line: Line<'static>, width: u16) -> Buffer {
    let mut buffer = Buffer::empty(Rect::new(0, 0, width, 1));
    Paragraph::new(line).render(buffer.area, &mut buffer);
    buffer
}
fn text(buffer: &Buffer) -> String {
    let mut value = String::new();
    let mut x = 0;
    while x < buffer.area.width {
        let symbol = buffer[(x, 0)].symbol();
        value.push_str(symbol);
        x += u16::try_from(symbol.width().max(1)).unwrap();
    }
    value
}
fn region(buffer: &Buffer, needle: &str) -> (u16, u16) {
    let value = text(buffer);
    let start = value.find(needle).unwrap_or_else(|| panic!("missing {needle:?}: {value:?}"));
    let left = u16::try_from(value[..start].width()).unwrap();
    (left, left + u16::try_from(needle.width()).unwrap())
}
fn app() -> App {
    let mut app = App::test_default();
    app.session_runtime.mode = Some(ModeState {
        current_mode_id: "default".into(),
        current_mode_name: "default".into(),
        available_modes: vec![],
    });
    app
}
fn server(name: &str, status: McpServerConnectionStatus) -> McpServerStatus {
    McpServerStatus {
        name: name.into(),
        status,
        server_info: None,
        error: None,
        config: None,
        scope: None,
        source: None,
        tools: vec![],
    }
}

#[test]
fn runtime_badges_replace_old_values_and_unset_mode_exposes_only_relevant_status() {
    let mut app = app();
    let fields = layout(&json!([
        {"id":"permissionMode","visible":true,"side":"left"},
        {"id":"fastMode","visible":true,"side":"right"}
    ]));
    for (name, state, label) in [
        ("default", model::FastModeState::Off, "FAST:OFF"),
        ("plan", model::FastModeState::On, "FAST:ON"),
        ("auto", model::FastModeState::Cooldown, "FAST:CD"),
        ("default", model::FastModeState::Unknown, "FAST:?"),
    ] {
        app.session_runtime.mode.as_mut().unwrap().current_mode_name = name.into();
        app.session_runtime.fast_mode_state = state;
        let buffer = draw(&app, &fields, 80);
        assert!(region(&buffer, name).1 < region(&buffer, label).0);
        for other in ["FAST:OFF", "FAST:ON", "FAST:CD", "FAST:?"] {
            assert_eq!(text(&buffer).contains(other), other == label);
        }
    }
    app.session_runtime.mode = None;
    for (status, expected) in [
        (AppStatus::Connecting, None),
        (AppStatus::Ready, Some("READY")),
        (AppStatus::Running, Some("WORKING")),
        (AppStatus::Error, Some("ERROR")),
    ] {
        app.status = status;
        let buffer = draw(&app, &fields, 80);
        region(&buffer, "FAST:?");
        for label in ["CONNECTING", "READY", "WORKING", "ERROR"] {
            assert_eq!(text(&buffer).contains(label), expected == Some(label));
        }
    }
}

#[test]
fn reported_effort_changes_and_clear_update_only_the_effort_region() {
    let mut app = app();
    app.session_runtime.session_id = Some("footer-session".into());
    app.session_runtime.current_model =
        Some(model::CurrentModel::new("opus", "Opus", "Opus").supports_effort(true));
    let fields = layout(
        &json!([{"id":"model","visible":true,"side":"left"},{"id":"effort","visible":true,"side":"right"}]),
    );
    for (value, expected) in [
        (json!("low"), Some("Low")),
        (json!("medium"), Some("Med")),
        (json!("high"), Some("High")),
        (json!("xhigh"), Some("XHigh")),
        (json!("max"), Some("Max")),
        (json!(null), None),
    ] {
        crate::app::handle_client_event(
            &mut app,
            ClientEvent::SessionUpdate {
                session_id: "footer-session".into(),
                update: model::SessionUpdate::ConfigOptionUpdate(model::ConfigOptionUpdate {
                    option_id: "effortLevel".into(),
                    value,
                }),
            },
        );
        let buffer = draw(&app, &fields, 80);
        let model = region(&buffer, "Opus");
        if let Some(label) = expected {
            assert!(model.1 < region(&buffer, label).0);
        }
        for label in ["Low", "Med", "High", "XHigh", "Max"] {
            assert_eq!(text(&buffer).contains(&format!("[{label}]")), expected == Some(label));
        }
    }
}

#[test]
fn ultracode_visibility_follows_explicit_verified_states_and_clears_after_activation() {
    let mut app = app();
    let fields = layout(
        &json!([{"id":"model","visible":true,"side":"left"},{"id":"ultracode","visible":true,"side":"right"}]),
    );
    for (state, visible) in [
        (None, false),
        (model::UltracodeState::new(true, true, true), true),
        (model::UltracodeState::new(true, false, false), false),
        (model::UltracodeState::new(true, true, true), true),
        (model::UltracodeState::new(false, true, false), false),
        (model::UltracodeState::new(false, false, false), false),
        (None, false),
    ] {
        app.session_runtime.ultracode = state;
        let buffer = draw(&app, &fields, 80);
        let model = region(&buffer, "test-model");
        assert_eq!(buffer[(model.0, 0)].fg, FOOTER_CONTEXT_VALUE);
        assert_eq!(text(&buffer).contains("Ultracode"), visible);
        if visible {
            assert!(model.1 < region(&buffer, "Ultracode").0);
        }
    }
}

#[test]
fn auth_hint_is_startup_only_and_disappears_when_authentication_finishes() {
    for role in [
        None,
        Some(MessageRole::System(None)),
        Some(MessageRole::User),
        Some(MessageRole::Assistant),
    ] {
        let mut app = app();
        app.mcp.servers = vec![
            server("needs-auth", McpServerConnectionStatus::NeedsAuth),
            server("already-connected", McpServerConnectionStatus::Connected),
        ];
        if let Some(role) = role.clone() {
            app.transcript.messages.push(ChatMessage::new(
                role,
                vec![MessageBlock::Text(crate::app::TextBlock::new(
                    "conversation sentinel".into(),
                ))],
                None,
            ));
        }
        let width = 180;
        let buffer = draw_current(&app, width);
        region(&buffer, "test-model");
        assert_eq!(
            text(&buffer).contains("1 MCP NEEDS AUTH"),
            matches!(role, None | Some(MessageRole::System(_)))
        );
        app.mcp.servers[0].status = McpServerConnectionStatus::Connected;
        let buffer = draw_current(&app, width);
        assert!(!text(&buffer).contains("MCP NEEDS AUTH"));
        region(&buffer, "test-model");
    }
}

#[test]
fn configured_groups_keep_reading_order_opposite_anchors_and_field_colors() {
    let mut app = app();
    app.cwd = "HIDDEN_DIRECTORY".into();
    app.set_git_branch_for_test(Some("feature/footer"));
    app.session_runtime.session_usage.context_usage_percent = Some(28);
    let fields = layout(&json!([
        {"id":"model","visible":true,"side":"left","color":"cyan"},
        {"id":"fastMode","visible":true,"side":"left"},
        {"id":"contextRemaining","visible":true,"side":"right","color":"green"},
        {"id":"gitBranch","visible":true,"side":"right","color":"pink"}
    ]));
    let buffer = draw(&app, &fields, 100);
    let model = region(&buffer, "[test-model]");
    let fast = region(&buffer, "[FAST:OFF]");
    let context = region(&buffer, "Context: 72%");
    let branch = region(&buffer, "Branch: feature/footer");
    assert_eq!(model.0, 0);
    assert!(model.1 < fast.0);
    assert!(fast.1 + 1 < context.0);
    assert!(context.1 < branch.0);
    assert_eq!(branch.1, 100);
    assert!(!text(&buffer).contains("HIDDEN_DIRECTORY"));
    for (r, color) in [
        (model, FooterColor::Cyan.color()),
        (context, FooterColor::Green.color()),
        (branch, FooterColor::Pink.color()),
        (fast, theme::DIM),
    ] {
        for x in r.0..r.1 {
            assert_eq!(buffer[(x, 0)].fg, color);
        }
    }
    let fields = layout(
        &json!([{"id":"directory","visible":true,"side":"left","color":"orange"},{"id":"model","visible":true,"side":"right"}]),
    );
    let buffer = draw(&app, &fields, 100);
    let directory = region(&buffer, "Loc: HIDDEN_DIRECTORY");
    let model = region(&buffer, "test-model");
    assert_eq!(buffer[(directory.0, 0)].fg, FooterColor::Orange.color());
    assert_eq!(buffer[(model.0, 0)].fg, FOOTER_CONTEXT_VALUE);
}

#[test]
fn notice_survives_field_overflow_and_remains_right_anchored() {
    let mut app = app();
    app.mcp.servers = vec![server("needs-auth", McpServerConnectionStatus::NeedsAuth)];
    let fields = layout(&json!([{"id":"model","visible":true,"side":"left"}]));
    let ample = draw(&app, &fields, 80);
    let model = region(&ample, "[test-model]");
    let notice = region(&ample, "1 MCP NEEDS AUTH");
    assert_eq!(notice.1, 80);
    assert!(model.1 < notice.0);
    let minimum = model.1 - model.0 + notice.1 - notice.0 + 1;
    let fit = draw(&app, &fields, minimum);
    assert!(region(&fit, "test-model").1 < region(&fit, "1 MCP NEEDS AUTH").0);
    let overflow = draw(&app, &fields, minimum - 1);
    assert!(!text(&overflow).contains("test-model"));
    assert_eq!(region(&overflow, "1 MCP NEEDS AUTH").1, minimum - 1);
    let narrow = draw(&app, &fields, 10);
    assert!(text(&narrow).starts_with("1 MCP"));
    assert!(!text(&narrow).contains("test-model"));
}

#[test]
fn workspace_clipping_preserves_useful_unicode_values_and_neighboring_status() {
    let mut app = app();
    let fields = layout(
        &json!([{"id":"fastMode","visible":true,"side":"left"},{"id":"directory","visible":true,"side":"left"},{"id":"gitBranch","visible":true,"side":"right"}]),
    );
    for path in [
        "~/工作/a-very-long-parent-cafe\u{301}/project-cafe\u{301}",
        r"C:\work\a-very-long-parent-project\project-cafe",
    ] {
        app.cwd = path.into();
        app.set_git_branch_for_test(Some("topic/日本語-change"));
        for width in 0..=160 {
            assert!(serialize_footer(&app, &fields, width).width() <= usize::from(width));
        }
        let wide = draw(&app, &fields, 160);
        let fast = region(&wide, "FAST:OFF");
        let cwd = region(&wide, path);
        let branch = region(&wide, "topic/日本語-change");
        assert!(fast.1 < cwd.0 && cwd.1 < branch.0);
        let constrained = draw(&app, &fields, 55);
        region(&constrained, "FAST:OFF");
        region(&constrained, "project-caf");
        assert!(!text(&constrained).contains("parent-"));
        let tiny = draw(&app, &fields, 2);
        assert!(text(&tiny).trim().is_empty());
    }
    let branch_only = layout(&json!([{"id":"gitBranch","visible":true,"side":"left"}]));
    app.set_git_branch_for_test(Some("distinctive-feature"));
    let ample = draw(&app, &branch_only, 50);
    region(&ample, "distinctive-feature");
    let clipped = draw(&app, &branch_only, 20);
    region(&clipped, "distinct");
    for width in 0..20 {
        let buffer = draw(&app, &branch_only, width);
        let value = text(&buffer);
        assert!(value.trim().is_empty() || value.contains('d'), "useless branch: {value:?}");
    }
}

use crate::app::{InlinePermission, ToolCallInfo};
use tokio::sync::oneshot;
#[test]
fn running_agent_count_has_its_own_region_and_disappears_at_zero() {
    let subagent = |id: &str, status, terminal: Option<&str>| {
        let mut task = model::TaskItem::new(id, id, status);
        task.metadata =
            Some(serde_json::json!({ "task_type": "local_agent", "terminal_status": terminal }));
        task
    };
    let mut shell = model::TaskItem::new("shell", "shell", model::TaskStatus::InProgress);
    shell.metadata = Some(serde_json::json!({ "task_type": "local_bash" }));
    let mut app = app();
    app.sdk_inventory.tasks = vec![
        subagent("agent-1", model::TaskStatus::InProgress, None),
        subagent("agent-2", model::TaskStatus::InProgress, None),
        subagent("agent-3", model::TaskStatus::Completed, Some("completed")),
        shell,
    ];
    let fields = layout(
        &json!([{"id":"runningSubagents","visible":true,"side":"right"},{"id":"model","visible":true,"side":"left"}]),
    );
    let buffer = draw(&app, &fields, 80);
    assert_eq!(region(&buffer, "[AGENTS:2]").1, 80);
    assert!(region(&buffer, "test-model").1 < region(&buffer, "AGENTS:2").0);

    for task in &mut app.sdk_inventory.tasks {
        task.status = model::TaskStatus::Completed;
    }
    let fields = layout(
        &json!([{"id":"runningSubagents","visible":true,"side":"right"},{"id":"model","visible":true,"side":"left"}]),
    );
    let buffer = draw(&app, &fields, 80);
    let value = text(&buffer);
    assert!(!value.contains("AGENTS"), "{value}");
}
#[test]
fn permission_notices_outrank_working_status_until_actual_interactions_resolve() {
    let mut app = App::test_default();
    app.status = AppStatus::Running;
    app.session_runtime.mode = None;
    app.session_runtime.session_id = Some("footer-session".into());
    let (response_tx, _response_rx) = oneshot::channel();
    app.transcript.messages.push(ChatMessage::new(
        MessageRole::Assistant,
        vec![MessageBlock::ToolCall(Box::new(ToolCallInfo {
            title: "Read".into(),
            content: vec![],
            pending_permission: Some(InlinePermission {
                options: vec![],
                display: None,
                subagent_context: None,
                response_tx,
                selected_index: 0,
                focused: true,
            }),
            ..crate::app::test_support::named_tool_call_info(
                "perm-1",
                "Read",
                model::ToolCallStatus::Pending,
            )
        }))],
        None,
    ));
    app.index_tool_call("perm-1".into(), 0, 0);
    app.pending_interaction_ids.push("perm-1".into());

    let buffer = draw_current(&app, 80);
    let value = text(&buffer);
    assert!(!value.contains("WORKING"));
    assert_eq!(region(&buffer, "1 PEND. PERM.").1, 80);
    assert!(value.ends_with("1 PEND. PERM."));
    let directory = tempfile::tempdir().expect("preferences");
    let path = directory.path().join("settings.json");
    std::fs::write(&path, r#"{"presentation":{"footer":[]}}"#).expect("save layout");
    app.config.load_footer(Some(&path));
    let buffer = draw_current(&app, 80);
    let value = text(&buffer);
    assert!(value.ends_with("1 PEND. PERM."));
    assert!(!value.contains("FAST:"));
    assert!(!value.contains("Loc:"));
    crate::app::handle_client_event(
        &mut app,
        ClientEvent::InteractionCancelled {
            session_id: "footer-session".into(),
            interaction_id: "perm-1".into(),
        },
    );
    let buffer = draw_current(&app, 80);
    assert!(!text(&buffer).contains("PEND. PERM."));
    assert_eq!(region(&buffer, "[WORKING]").1, 80);
}
