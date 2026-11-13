// SPDX-License-Identifier: Apache-2.0

use crate::agent::client::{AgentConnection, CommandReceiver};
use crate::agent::events::ClientEvent;
use crate::agent::model::{self, SessionId, TaskItem, TaskStatus};
use crate::agent::types::{SubagentMessage, SubagentMessageRole};
use crate::agent::wire::BridgeCommand;
use crate::app::App;
use crate::app::agents::{apply_transcript_list, handle_key, open};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use std::rc::Rc;

fn screen(app: &mut App, width: u16, height: u16) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal.draw(|frame| super::render(frame, app)).expect("draw");
    let buffer = terminal.backend().buffer();
    buffer
        .content
        .chunks(usize::from(buffer.area.width))
        .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect::<String>())
        .collect()
}

fn row_with<'a>(lines: &'a [String], needle: &str) -> (usize, &'a str) {
    let Some(index) = lines.iter().position(|line| line.contains(needle)) else {
        panic!("no row contains {needle:?}:\n{}", lines.join("\n"));
    };
    (index, lines[index].as_str())
}

fn subagent(
    id: &str,
    subject: &str,
    status: TaskStatus,
    mut metadata: serde_json::Value,
) -> TaskItem {
    let mut task = TaskItem::new(id, subject, status);
    metadata["task_type"] = serde_json::json!("local_agent");
    task.metadata = Some(metadata);
    task
}

fn app_with_subagents() -> (App, CommandReceiver) {
    let mut app = App::test_default();
    let (conn, commands) = AgentConnection::test_channel();
    app.session_runtime.conn = Some(Rc::new(conn));
    app.session_runtime.session_id = Some(SessionId::new("session-1"));
    app.sdk_inventory.tasks = vec![
        subagent(
            "agent-1",
            "Run the slow check",
            TaskStatus::InProgress,
            serde_json::json!({ "subagent_type": "general-purpose" }),
        ),
        TaskItem::new("shell-1", "Background shell", TaskStatus::InProgress),
        subagent(
            "agent-2",
            "Review the parser",
            TaskStatus::Completed,
            serde_json::json!({ "subagent_type": "reviewer", "terminal_status": "completed", "summary": "Parser looks fine" }),
        ),
        subagent(
            "agent-3",
            "Abandoned search",
            TaskStatus::Completed,
            serde_json::json!({ "terminal_status": "killed" }),
        ),
    ];
    open(&mut app);
    (app, commands)
}

fn press(app: &mut App, code: KeyCode) {
    handle_key(app, KeyEvent::new(code, KeyModifiers::NONE));
}

/// The id of the newest transcript read sent to the bridge.
fn last_read_request(commands: &mut CommandReceiver) -> String {
    std::iter::from_fn(|| commands.try_recv().ok())
        .filter(|envelope| matches!(envelope.command, BridgeCommand::GetSubagentMessages { .. }))
        .last()
        .and_then(|envelope| envelope.request_id)
        .expect("a transcript read was sent")
}

/// Opens the completed subagent's transcript as a user does; returns the read's request id.
fn open_transcript(app: &mut App, commands: &mut CommandReceiver) -> String {
    apply_transcript_list(app, vec!["agent-2".to_owned()], None);
    app.agents.selected_task_id = Some("agent-2".to_owned());
    press(app, KeyCode::Enter);
    last_read_request(commands)
}

fn answer(app: &mut App, request_id: &str, outcome: Result<Vec<SubagentMessage>, &str>) {
    answer_for(app, "agent-2", request_id, outcome);
}

fn answer_for(
    app: &mut App,
    agent_id: &str,
    request_id: &str,
    outcome: Result<Vec<SubagentMessage>, &str>,
) {
    let (messages, error) = match outcome {
        Ok(messages) => (messages, None),
        Err(error) => (Vec::new(), Some(error.to_owned())),
    };
    crate::app::handle_client_event(
        app,
        ClientEvent::SubagentMessagesReceived {
            session_id: "session-1".to_owned(),
            agent_id: agent_id.to_owned(),
            request_id: Some(request_id.to_owned()),
            messages,
            error,
        },
    );
}

fn message(role: SubagentMessageRole, text: &str) -> SubagentMessage {
    SubagentMessage { role, text: text.to_owned() }
}

fn reply(text: &str) -> SubagentMessage {
    message(SubagentMessageRole::Assistant, text)
}

fn drawn(app: &mut App, width: u16, height: u16) -> ratatui::buffer::Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal.draw(|frame| super::render(frame, app)).expect("draw");
    terminal.backend().buffer().clone()
}

fn region(buffer: &ratatui::buffer::Buffer, area: ratatui::layout::Rect) -> Vec<String> {
    (area.y..area.bottom())
        .map(|y| {
            (area.x..area.right())
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect()
}

fn locate(buffer: &ratatui::buffer::Buffer, text: &str) -> (u16, u16) {
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            let suffix: String =
                (x..buffer.area.width).map(|column| buffer[(column, y)].symbol()).collect();
            if suffix.starts_with(text) {
                return (x, y);
            }
        }
    }
    panic!("missing {text:?}: {:?}", region(buffer, buffer.area));
}

fn details(buffer: &ratatui::buffer::Buffer) -> Vec<String> {
    // Locate the rendered pane by its own field and control anchors, not a layout percentage.
    let (x, type_y) = locate(buffer, "Type:");
    let (_, hint_y) = locate(buffer, "Up/Down select");
    region(
        buffer,
        ratatui::layout::Rect::new(
            x,
            type_y.saturating_sub(2),
            buffer.area.width - x,
            hint_y - type_y + 2,
        ),
    )
}

fn reader_body(buffer: &ratatui::buffer::Buffer) -> Vec<String> {
    let (_, title_y) = locate(buffer, "Subagents");
    let (_, hint_y) = locate(buffer, "Up/Down scroll");
    region(
        buffer,
        ratatui::layout::Rect::new(0, title_y + 1, buffer.area.width, hint_y - title_y - 1),
    )
}

fn event_key(app: &mut App, code: KeyCode) {
    crate::app::test_support::press(app, code);
}

fn expect_list(commands: &mut CommandReceiver) {
    let request = commands.try_recv().expect("transcript list request");
    assert!(
        matches!(request.command,BridgeCommand::ListSubagents {ref session_id} if session_id=="session-1")
    );
}

fn expect_read(commands: &mut CommandReceiver, agent: &str) -> String {
    let request = commands.try_recv().expect("transcript request");
    assert!(
        matches!(request.command,BridgeCommand::GetSubagentMessages {ref session_id,ref agent_id} if session_id=="session-1" && agent_id==agent)
    );
    assert!(commands.try_recv().is_err(), "no duplicate read or unrelated command");
    request.request_id.expect("correlated read")
}

fn listed(app: &mut App, ids: &[&str], error: Option<&str>) {
    crate::app::handle_client_event(
        app,
        ClientEvent::SubagentsListed {
            session_id: "session-1".into(),
            agent_ids: ids.iter().map(|id| (*id).into()).collect(),
            error: error.map(str::to_owned),
        },
    );
}

fn lifecycle(app: &mut App, tasks: Vec<TaskItem>, removed: &[&str]) {
    crate::app::test_support::deliver_update(
        app,
        model::SessionUpdate::TaskStateUpdate(
            model::TaskStateUpdate::new(model::TaskUpdateSource::Lifecycle)
                .tasks(tasks)
                .removed_task_ids(removed.iter().map(|id| (*id).into()).collect()),
        ),
    );
}

fn audit_agents() -> (App, CommandReceiver) {
    let (app, mut commands) = app_with_subagents();
    expect_list(&mut commands);
    assert!(commands.try_recv().is_err());
    (app, commands)
}

fn selected_reader(app: &mut App, commands: &mut CommandReceiver) -> String {
    listed(app, &["agent-2"], None);
    event_key(app, KeyCode::Down);
    let request = expect_read(commands, "agent-2");
    event_key(app, KeyCode::Enter);
    assert!(
        commands.try_recv().is_err(),
        "opening the pending preview does not duplicate its read"
    );
    request
}

fn long_notes(prefix: &str) -> Vec<SubagentMessage> {
    vec![message(
        SubagentMessageRole::User,
        &(1..=60).map(|n| format!("{prefix}-{n:02}")).collect::<Vec<_>>().join("\n"),
    )]
}

#[test]
fn ancestry_runs_and_fork_model_provenance_stay_with_the_selected_row() {
    let (mut app, mut commands) = audit_agents();
    listed(&mut app, &[], None);
    tool_call(
        &mut app,
        "fork-launch",
        "Agent",
        "Fork launch",
        serde_json::json!({"subagent_type":"fork","model":"ignored-model","effort":"max"}),
        None,
    );
    tool_call(
        &mut app,
        "parent-launch",
        "Agent",
        "Parent launch",
        serde_json::json!({"model":"parent-requested"}),
        None,
    );
    let mut child = subagent(
        "agent-1",
        "Run the slow check",
        TaskStatus::InProgress,
        serde_json::json!({"subagent_type":"fork"}),
    );
    child.run_id = Some("run-A".into());
    child.source_tool_call_run_id = Some("run-A".into());
    child.source_tool_call_id = Some("fork-launch".into());
    child.parent_task_id = Some("agent-2".into());
    let mut parent = subagent(
        "agent-2",
        "Review the parser",
        TaskStatus::Completed,
        serde_json::json!({"subagent_type":"reviewer","terminal_status":"completed"}),
    );
    parent.run_id = Some("parent-run".into());
    parent.source_tool_call_run_id = Some("parent-run".into());
    parent.source_tool_call_id = Some("parent-launch".into());
    lifecycle(&mut app, vec![child.clone(), parent], &[]);
    let pane = details(&drawn(&mut app, 150, 38));
    assert!(row_with(&pane, "Model:").1.contains("parent model (inherited)"));
    assert!(row_with(&pane, "Parent:").1.contains("Review the parser"));
    assert!(row_with(&pane, "Run:").1.contains("run-A"));
    assert!(!pane.iter().any(|row| row.contains("ignored-model") || row.contains("max")));
    crate::app::test_support::deliver_update(
        &mut app,
        model::SessionUpdate::ToolCallUpdate(model::ToolCallUpdate::new(
            "fork-launch",
            model::ToolCallUpdateFields::new().output_metadata(
                model::ToolOutputMetadata::new().agent(Some(
                    model::AgentOutputMetadata::new()
                        .resolved_model(Some("observed-sonnet".into()))
                        .models_used(Some(vec!["observed-opus".into(), "observed-sonnet".into()])),
                )),
            ),
        )),
    );
    let pane = details(&drawn(&mut app, 150, 38));
    assert!(row_with(&pane, "Model:").1.contains("observed-opus -> observed-sonnet (observed)"));
    event_key(&mut app, KeyCode::Down);
    let other = details(&drawn(&mut app, 150, 38));
    assert!(row_with(&other, "Model:").1.contains("parent-requested"));
    assert!(row_with(&other, "Run:").1.contains("parent-run"));
    assert!(!other.join("\n").contains("observed-opus"));
    event_key(&mut app, KeyCode::Up);
    child.run_id = Some("run-B".into());
    lifecycle(&mut app, vec![child], &[]);
    let pane = details(&drawn(&mut app, 150, 38));
    assert!(row_with(&pane, "Model:").1.contains("parent model (inherited)"));
    assert!(!pane.join("\n").contains("observed-sonnet"));
    assert!(row_with(&pane, "Run:").1.contains("run-B"));
    lifecycle(&mut app, vec![], &["agent-2"]);
    let pane = details(&drawn(&mut app, 150, 38));
    assert!(pane[0].contains("Run the slow check"));
    assert!(!pane.iter().any(|row| row.contains("Parent:")));
    event_key(&mut app, KeyCode::Char('s'));
    let stop = commands.try_recv().expect("child stop remains usable");
    assert!(
        matches!(stop.command,BridgeCommand::StopSubagent {ref session_id,ref task_id,ref expected_run_id} if session_id=="session-1" && task_id=="agent-1" && expected_run_id.as_deref()==Some("run-B"))
    );
    assert!(stop.request_id.is_some());
    assert!(commands.try_recv().is_err());
}

#[test]
fn a_new_run_labels_the_open_saved_snapshot_without_replacing_its_rendered_content() {
    let (mut app, mut commands) = audit_agents();
    let mut task =
        app.sdk_inventory.tasks.iter().find(|task| task.task_id == "agent-2").unwrap().clone();
    task.run_id = Some("run-A".into());
    lifecycle(&mut app, vec![task.clone()], &[]);
    let request = selected_reader(&mut app, &mut commands);
    answer(&mut app, &request, Ok(long_notes("snapshot-A")));
    let _ = drawn(&mut app, 80, 24);
    event_key(&mut app, KeyCode::PageDown);
    let before = reader_body(&drawn(&mut app, 80, 24));
    assert!(before.iter().any(|row| row.contains("snapshot-A-20")));
    assert!(!before.iter().any(|row| row.contains("snapshot-A-01")));
    task.run_id = Some("run-B".into());
    lifecycle(&mut app, vec![task], &[]);
    let saved = drawn(&mut app, 80, 24);
    assert_eq!(reader_body(&saved), before);
    let header =
        region(&saved, ratatui::layout::Rect::new(0, locate(&saved, "Subagents").1, 80, 1));
    assert!(header[0].contains("saved snapshot") && header[0].contains("Review the parser"));
    assert!(commands.try_recv().is_err());
    event_key(&mut app, KeyCode::Char('r'));
    let current = expect_read(&mut commands, "agent-2");
    assert_ne!(current, request);
    answer(&mut app, &current, Ok(long_notes("snapshot-B")));
    let replaced = drawn(&mut app, 80, 24);
    let body = reader_body(&replaced);
    assert!(body.iter().any(|row| row.contains("snapshot-B-20")));
    assert!(!body.join("\n").contains("snapshot-A"));
    assert!(
        !region(&replaced, replaced.area)[locate(&replaced, "Subagents").1 as usize]
            .contains("saved snapshot")
    );
    assert!(commands.try_recv().is_err());
}

#[test]
fn rows_show_each_subagents_status_type_and_transcript_under_their_own_title() {
    let (mut app, _commands) = app_with_subagents();
    apply_transcript_list(&mut app, vec!["agent-2".to_owned()], None);
    handle_key(&mut app, KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
    let lines = screen(&mut app, 80, 30);

    let (title, title_line) = row_with(&lines, "Subagents");
    assert!(title_line.contains("1/3"), "{title_line}");
    assert!(title_line.contains("1 running"), "a stopping subagent still runs: {title_line}");
    assert!(
        !lines.iter().any(|line| line.contains("Background shell")),
        "only subagents are listed"
    );

    let (running, running_line) = row_with(&lines, "Run the slow check");
    assert!(running > title);
    assert!(running_line.contains('›'), "the first row is selected: {running_line}");
    assert!(running_line.contains("stopping"), "{running_line}");
    assert!(lines[running + 1].contains("general-purpose"));
    assert!(lines[running + 1].contains("no transcript"));

    let (done, done_line) = row_with(&lines, "Review the parser");
    assert!(done_line.contains("completed") && !done_line.contains('›'), "{done_line}");
    assert!(lines[done + 1].contains("reviewer") && lines[done + 1].contains("· transcript"));
    assert!(
        !lines.iter().any(|line| line.contains("Parser looks fine")),
        "an unselected subagent's result stays out of the list"
    );

    let (_, stopped_line) = row_with(&lines, "Abandoned search");
    assert!(stopped_line.contains("stopped"), "{stopped_line}");

    let (hint_row, _) = row_with(&lines, "Up/Down select");
    assert!(hint_row > running, "the hint row follows the list");
    let hint = lines[hint_row..].join(" ");
    for key in ["Enter transcript", "s stop", "r refresh", "Esc", "close"] {
        assert!(hint.contains(key), "{hint}");
    }
}

#[test]
fn list_rechecks_recover_their_error_without_clearing_an_unrelated_action_notice() {
    let (mut app, mut commands) = audit_agents();
    listed(&mut app, &[], Some("directory denied sentinel"));
    let failed = drawn(&mut app, 90, 30);
    assert_notice(&failed, "directory denied sentinel");
    event_key(&mut app, KeyCode::Char('r'));
    expect_list(&mut commands);
    listed(&mut app, &["agent-2"], None);
    assert!(
        !region(&drawn(&mut app, 90, 30), ratatui::layout::Rect::new(0, 0, 90, 30))
            .join("\n")
            .contains("directory denied sentinel")
    );
    // The interrupted row cannot be stopped and does not schedule a response read.
    event_key(&mut app, KeyCode::Down);
    let preview = expect_read(&mut commands, "agent-2");
    answer(&mut app, &preview, Ok(vec![]));
    event_key(&mut app, KeyCode::Down);
    event_key(&mut app, KeyCode::Char('s'));
    let stop_notice = app.agents.notice.as_ref().expect("non-running stop notice").text.clone();
    assert!(stop_notice.contains("running") && stop_notice.contains("stopped"));
    event_key(&mut app, KeyCode::Char('r'));
    expect_list(&mut commands);
    listed(&mut app, &[], Some("second directory cause"));
    assert_notice(&drawn(&mut app, 90, 30), "second directory cause");
    event_key(&mut app, KeyCode::Char('r'));
    expect_list(&mut commands);
    listed(&mut app, &[], None);
    let recovered = drawn(&mut app, 90, 30);
    assert_notice(&recovered, &stop_notice);
    assert!(!region(&recovered, recovered.area).join("\n").contains("second directory cause"));
    assert!(commands.try_recv().is_err());
    // Empty inventory and a populated session are distinct states on the same working connection.
    crate::app::test_support::deliver_update(
        &mut app,
        model::SessionUpdate::TaskStateUpdate(
            model::TaskStateUpdate::new(model::TaskUpdateSource::List).complete_snapshot(true),
        ),
    );
    event_key(&mut app, KeyCode::Char('r'));
    expect_list(&mut commands);
    listed(&mut app, &[], None);
    let empty = drawn(&mut app, 90, 30);
    assert!(region(&empty, empty.area).join("\n").contains("No subagents"));
    lifecycle(
        &mut app,
        vec![subagent(
            "new",
            "Recovered live agent",
            TaskStatus::InProgress,
            serde_json::json!({"subagent_type":"worker"}),
        )],
        &[],
    );
    event_key(&mut app, KeyCode::Down);
    assert!(details(&drawn(&mut app, 90, 30))[0].contains("Recovered live agent"));
}

fn assert_notice(buffer: &ratatui::buffer::Buffer, cause: &str) {
    let (_, row) = locate(buffer, cause);
    let (_, hint) = locate(buffer, "Up/Down select");
    assert!(
        row < hint && row > locate(buffer, "Transcript:").1,
        "notice belongs below content and above controls"
    );
}

#[test]
fn transcript_retry_distinguishes_pending_empty_failed_and_recovered_content() {
    let (mut app, mut commands) = audit_agents();
    let request = selected_reader(&mut app, &mut commands);
    assert!(reader_body(&drawn(&mut app, 80, 24)).join("\n").contains("Reading transcript"));
    answer(&mut app, &request, Err("first read denied sentinel"));
    assert!(
        reader_body(&drawn(&mut app, 80, 24)).join("\n").contains("first read denied sentinel")
    );
    event_key(&mut app, KeyCode::Char('r'));
    let retry = expect_read(&mut commands, "agent-2");
    assert_ne!(retry, request);
    assert!(reader_body(&drawn(&mut app, 80, 24)).join("\n").contains("Reading transcript"));
    answer(&mut app, &retry, Ok(vec![]));
    assert!(reader_body(&drawn(&mut app, 80, 24)).join("\n").contains("Empty transcript"));
    event_key(&mut app, KeyCode::Char('r'));
    let populated = expect_read(&mut commands, "agent-2");
    answer(&mut app, &populated, Ok(vec![reply("Independent parser findings")]));
    let body = reader_body(&drawn(&mut app, 80, 24));
    assert!(row_with(&body, "Independent parser findings").0 > row_with(&body, "Reply").0);
    assert!(!body.join("\n").contains("denied sentinel"));
    event_key(&mut app, KeyCode::Char('r'));
    let failed = expect_read(&mut commands, "agent-2");
    answer(&mut app, &failed, Err("later read denied sentinel"));
    assert!(
        reader_body(&drawn(&mut app, 80, 24)).join("\n").contains("later read denied sentinel")
    );
    event_key(&mut app, KeyCode::Char('r'));
    let final_read = expect_read(&mut commands, "agent-2");
    assert!(
        reader_body(&drawn(&mut app, 80, 24)).join("\n").contains("Independent parser findings")
    );
    answer(&mut app, &final_read, Ok(vec![reply("Newly recovered findings")]));
    let body = reader_body(&drawn(&mut app, 80, 24)).join("\n");
    assert!(body.contains("Newly recovered findings"));
    assert!(!body.contains("denied sentinel") && !body.contains("Independent parser findings"));
}

/// Reports one tool call the way the bridge does; `parent` makes it a subagent's own call.
fn tool_call(
    app: &mut App,
    id: &str,
    name: &str,
    title: &str,
    input: serde_json::Value,
    parent: Option<&str>,
) {
    let mut meta = serde_json::json!({ "claudeCode": { "toolName": name } });
    if let Some(parent) = parent {
        meta["claudeCode"]["parentToolUseId"] = serde_json::json!(parent);
    }
    crate::app::handle_client_event(
        app,
        ClientEvent::SessionUpdate {
            session_id: "session-1".to_owned(),
            update: model::SessionUpdate::ToolCall(
                model::ToolCall::new(id, title)
                    .status(model::ToolCallStatus::InProgress)
                    .raw_input(input)
                    .meta(meta),
            ),
        },
    );
}

#[test]
fn long_titles_keep_their_status_inside_the_selected_list_row() {
    for subject in [
        format!("ASCII selected {}", "long source ".repeat(20)),
        format!("Wide selected {}", "\u{754c}".repeat(80)),
    ] {
        let (mut app, mut commands) = audit_agents();
        listed(&mut app, &[], None);
        let mut task = app.sdk_inventory.tasks[0].clone();
        task.subject = subject.clone();
        lifecycle(&mut app, vec![task], &[]);
        for width in [120, 80, 140, 60, 120] {
            let buffer = drawn(&mut app, width, 30);
            let (list_x, list_y) = locate(&buffer, "\u{203a}");
            let (detail_x, type_y) = locate(&buffer, "Type:");
            let right = if detail_x > list_x { detail_x } else { width - 2 };
            let row =
                region(&buffer, ratatui::layout::Rect::new(list_x, list_y, right - list_x, 1))
                    .join("");
            assert!(
                row.contains(subject.split_whitespace().next().unwrap())
                    && row.contains("selected"),
                "{row}"
            );
            assert!(row.ends_with(" running"), "complete badge within list: {row}");
            assert!(!row.contains(&subject), "long source must be clipped");
            assert!(
                row.contains("...") || row.contains('\u{2026}'),
                "omission is disclosed: {row}"
            );
            if detail_x > list_x {
                assert!(type_y > list_y, "wrapped details remain in the adjacent pane");
            } else {
                assert!(type_y > list_y);
            }
        }
        assert!(commands.try_recv().is_err());
    }
}

#[test]
fn selected_details_keep_invocation_activity_prompt_and_model_provenance_together() {
    let (mut app, mut commands) = audit_agents();
    tool_call(
        &mut app,
        "launch-1",
        "Agent",
        "Primary launch",
        serde_json::json!({"prompt":"Primary prompt sentinel","model":"haiku"}),
        None,
    );
    tool_call(
        &mut app,
        "launch-rival",
        "Agent",
        "Rival launch",
        serde_json::json!({"prompt":"Rival prompt sentinel"}),
        None,
    );
    for (id, title, parent) in [
        ("older", "Primary oldest", "launch-1"),
        ("previous", "Primary previous", "launch-1"),
        ("now", "Primary newest", "launch-1"),
        ("rival-child", "Rival activity sentinel", "launch-rival"),
    ] {
        tool_call(&mut app, id, "Read", title, serde_json::json!({}), Some(parent));
    }
    let mut primary = app.sdk_inventory.tasks[0].clone();
    primary.source_tool_call_id = Some("launch-1".into());
    let mut rival = app.sdk_inventory.tasks[2].clone();
    rival.source_tool_call_id = Some("launch-rival".into());
    lifecycle(&mut app, vec![primary, rival], &[]);
    app.sdk_inventory
        .available_agents
        .push(model::AvailableAgent::new("reviewer", "Reviews code").model("sonnet"));
    listed(&mut app, &["agent-1"], None);
    for width in [140, 80] {
        let buffer = drawn(&mut app, width, 44);
        let pane = details(&buffer);
        assert!(row_with(&pane, "Model:").1.contains("haiku (requested)"));
        assert!(row_with(&pane, "Transcript:").1.contains("saved"));
        assert!(row_with(&pane, "Now:").1.contains("Primary newest"));
        let recent = row_with(&pane, "Recent").0;
        assert!(row_with(&pane, "Primary previous").0 > recent);
        assert!(row_with(&pane, "Primary previous").0 < row_with(&pane, "Primary oldest").0);
        assert!(!pane[recent..].join("\n").contains("Primary newest"));
        assert!(row_with(&pane, "Primary prompt sentinel").0 > row_with(&pane, "Prompt").0);
        assert!(!pane.join("\n").contains("Rival"));
        let (list_x, list_y) = locate(&buffer, "\u{203a}");
        let (detail_x, type_y) = locate(&buffer, "Type:");
        if width == 140 {
            assert!(detail_x > list_x);
            assert_eq!(type_y - 2, list_y);
        } else {
            assert_eq!(detail_x, list_x);
            assert!(type_y > list_y);
        }
    }
    event_key(&mut app, KeyCode::Down);
    for width in [80, 140] {
        let pane = details(&drawn(&mut app, width, 44));
        assert!(row_with(&pane, "Model:").1.contains("sonnet (agent default)"));
        assert!(row_with(&pane, "Result:").1.contains("Parser looks fine"));
        assert!(row_with(&pane, "Rival prompt sentinel").0 > row_with(&pane, "Prompt").0);
        assert!(!pane.join("\n").contains("Primary") && !pane.join("\n").contains("Now:"));
    }
    assert!(commands.try_recv().is_err());
}

#[test]
fn transcript_and_detail_keys_page_clamp_and_return_to_the_selected_task() {
    let (mut app, mut commands) = audit_agents();
    let request = selected_reader(&mut app, &mut commands);
    let mut messages = vec![
        message(SubagentMessageRole::User, "Inspect the parser"),
        message(SubagentMessageRole::Tool, "ls parser"),
        reply("Opening finding"),
    ];
    messages.extend(long_notes("note"));
    messages.push(reply("FINAL finding"));
    answer(&mut app, &request, Ok(messages));
    let initial = drawn(&mut app, 80, 24);
    let body = reader_body(&initial);
    assert!(body[row_with(&body, "Prompt").0 + 1].contains("Inspect the parser"));
    row_with(&body, "\u{2022} ls parser");
    assert!(row_with(&body, "Opening finding").0 > row_with(&body, "Reply").0);
    assert!(!body.join("\n").contains("Run the slow check"));
    locate(&initial, "Esc/Left back");
    event_key(&mut app, KeyCode::Down);
    let after_down = reader_body(&drawn(&mut app, 80, 24));
    assert_eq!(
        after_down.iter().find(|line| !line.trim().is_empty()).unwrap().trim(),
        "Inspect the parser"
    );
    row_with(&after_down, "Inspect the parser");
    let mut reached = false;
    for _ in 0..15 {
        event_key(&mut app, KeyCode::PageDown);
        if reader_body(&drawn(&mut app, 80, 24)).join("\n").contains("FINAL finding") {
            reached = true;
            break;
        }
    }
    assert!(reached, "page keys reach independent final reply");
    for _ in 0..3 {
        event_key(&mut app, KeyCode::PageDown);
        drawn(&mut app, 80, 24);
    }
    let bottom = reader_body(&drawn(&mut app, 80, 24));
    event_key(&mut app, KeyCode::Down);
    assert_eq!(reader_body(&drawn(&mut app, 80, 24)), bottom);
    let first = bottom
        .iter()
        .find_map(|line| line.trim().strip_prefix("note-").and_then(|n| n.parse::<u32>().ok()))
        .expect("tail notes");
    event_key(&mut app, KeyCode::Up);
    let earlier = reader_body(&drawn(&mut app, 80, 24));
    row_with(&earlier, &format!("note-{:02}", first - 1));
    event_key(&mut app, KeyCode::PageUp);
    let previous = reader_body(&drawn(&mut app, 80, 24));
    row_with(&previous, &format!("note-{:02}", first - 11));
    event_key(&mut app, KeyCode::Left);
    let overview = drawn(&mut app, 80, 32);
    locate(&overview, "\u{203a} Review the parser");
    assert!(details(&overview)[0].contains("Review the parser"));
    // A long real launch prompt makes the selected details independently pageable.
    tool_call(
        &mut app,
        "detail-launch",
        "Agent",
        "Parser launch",
        serde_json::json!({"prompt":(1..=30).map(|n|format!("detail-note-{n:02}")).collect::<Vec<_>>().join("\n")}),
        None,
    );
    let mut task = app.sdk_inventory.tasks.iter().find(|t| t.task_id == "agent-2").unwrap().clone();
    task.source_tool_call_id = Some("detail-launch".into());
    lifecycle(&mut app, vec![task], &[]);
    let initial = drawn(&mut app, 120, 14);
    let (x, y) = locate(&initial, "Type:");
    let (_, end) = locate(&initial, "Up/Down select");
    let area = ratatui::layout::Rect::new(x, y - 2, 120 - x, end - y + 2);
    event_key(&mut app, KeyCode::PageDown);
    let later = region(&drawn(&mut app, 120, 14), area);
    row_with(&later, "FINAL finding");
    event_key(&mut app, KeyCode::PageUp);
    assert!(region(&drawn(&mut app, 120, 14), area)[0].contains("Review the parser"));
    event_key(&mut app, KeyCode::PageDown);
    drawn(&mut app, 120, 14);
    event_key(&mut app, KeyCode::Up);
    assert!(details(&drawn(&mut app, 120, 24))[0].contains("Run the slow check"));
    assert!(commands.try_recv().is_err());
}

#[test]
fn warmed_transcript_wrapping_preserves_complete_source_and_replaces_stale_content() {
    let (mut app, mut commands) = audit_agents();
    let request = selected_reader(&mut app, &mut commands);
    let source = "alpha beta gamma delta distinct-middle-one epsilon zeta eta theta distinct-middle-two iota kappa lambda mu";
    answer(&mut app, &request, Ok(vec![reply(source)]));
    for width in [120, 30, 60, 120] {
        let body = reader_body(&drawn(&mut app, width, 32));
        let label = row_with(&body, "Reply").0;
        assert_eq!(
            body[label + 1..].join(" ").split_whitespace().collect::<Vec<_>>(),
            source.split_whitespace().collect::<Vec<_>>()
        );
    }
    event_key(&mut app, KeyCode::Char('r'));
    let fresh = expect_read(&mut commands, "agent-2");
    assert_ne!(fresh, request);
    let replacement =
        "Fresh independent answer with replacement-middle and complete final evidence";
    answer(&mut app, &fresh, Ok(vec![reply(replacement)]));
    for width in [30, 120] {
        let body = reader_body(&drawn(&mut app, width, 32));
        let label = row_with(&body, "Reply").0;
        assert_eq!(
            body[label + 1..].join(" ").split_whitespace().collect::<Vec<_>>(),
            replacement.split_whitespace().collect::<Vec<_>>()
        );
        assert!(!body.join("\n").contains("distinct-middle"));
    }
}

#[test]
fn r_rereads_the_transcript_at_the_same_position_and_ends_a_shrunk_one_on_its_last_page() {
    let (mut app, mut commands) = app_with_subagents();
    let findings = |count: usize| (1..=count).map(|n| reply(&format!("finding {n}"))).collect();
    let request = open_transcript(&mut app, &mut commands);
    answer(&mut app, &request, Ok(findings(40)));
    for _ in 0..20 {
        press(&mut app, KeyCode::Down);
    }
    let before = screen(&mut app, 60, 20);
    let (title, _) = row_with(&before, "Review the parser");
    let (hint, _) = row_with(&before, "Esc/Left back");
    assert!(
        !before.iter().any(|line| line.trim() == "finding 1"),
        "the reader is scrolled: {}",
        before.join("\n")
    );

    press(&mut app, KeyCode::Char('r'));
    let request = last_read_request(&mut commands);
    answer(&mut app, &request, Ok(findings(60)));
    let after = screen(&mut app, 60, 20);
    assert_eq!(after[title..hint], before[title..hint], "the reading position is kept");

    press(&mut app, KeyCode::Char('r'));
    let request = last_read_request(&mut commands);
    answer(&mut app, &request, Ok(findings(3)));
    let shrunk = screen(&mut app, 60, 20);
    row_with(&shrunk, "finding 3");
    row_with(&shrunk, "finding 1");
}

#[test]
fn a_bounded_saved_preview_opens_the_complete_styled_report() {
    let (mut app, mut commands) = audit_agents();
    listed(&mut app, &["agent-2"], None);
    event_key(&mut app, KeyCode::Down);
    let request = expect_read(&mut commands, "agent-2");
    let entries: Vec<_> = (1..=30).map(|n| format!("page-{n:02}")).collect();
    let report = format!(
        "## Docs survey\n\nThe **docs** cover:\n\n{}",
        entries.iter().map(|s| format!("- {s}")).collect::<Vec<_>>().join("\n")
    );
    answer(&mut app, &request, Ok(vec![reply(&report)]));
    let overview = drawn(&mut app, 80, 40);
    let pane = details(&overview);
    let start = row_with(&pane, "Saved response").0;
    let preview = pane[start + 1..].join("\n");
    assert!(preview.contains("Docs survey"));
    assert!(!preview.contains("page-30"));
    assert!(
        preview.contains("...") || preview.contains('\u{2026}'),
        "omitted source disclosed: {preview}"
    );
    locate(&overview, "Enter transcript");
    event_key(&mut app, KeyCode::Enter);
    assert!(commands.try_recv().is_err());
    let initial = drawn(&mut app, 80, 24);
    let body = reader_body(&initial);
    assert!(row_with(&body, "Docs survey").0 > row_with(&body, "Reply").0);
    for text in ["Docs survey", "docs"] {
        let (x, y) = locate(&initial, text);
        assert!(
            initial[(x, y)].modifier.contains(ratatui::style::Modifier::BOLD),
            "{text} is styled Markdown"
        );
    }
    assert!(!body.join("\n").contains("##") && !body.join("\n").contains("**"));
    let mut seen = Vec::new();
    for _ in 0..8 {
        let body = reader_body(&drawn(&mut app, 80, 24));
        for line in body {
            if let Some(entry) = entries.iter().find(|entry| line.contains(entry.as_str()))
                && !seen.contains(entry)
            {
                seen.push(entry.clone());
            }
        }
        if seen.last() == entries.last() {
            break;
        }
        event_key(&mut app, KeyCode::PageDown);
    }
    assert_eq!(seen, entries, "all source items accessible in order");
}

#[test]
fn an_sdk_reminder_is_labelled_as_one_while_tags_a_user_or_agent_wrote_stay_as_written() {
    let (mut app, mut commands) = app_with_subagents();
    let request = open_transcript(&mut app, &mut commands);
    let prompt = "Quote <system-reminder>tags</system-reminder> as written.";
    let reply_text = "Keeping <system-reminder>as written</system-reminder>";
    answer(
        &mut app,
        &request,
        Ok(vec![
            message(SubagentMessageRole::User, prompt),
            message(SubagentMessageRole::SystemReminder, "Report through SubagentHandback."),
            reply(reply_text),
        ]),
    );
    let lines = screen(&mut app, 80, 30);
    let (reminder, _) = row_with(&lines, "Report through SubagentHandback.");
    assert_eq!(lines[reminder - 1].trim(), "System reminder", "{}", lines.join("\n"));
    assert!(!lines[reminder].contains("system-reminder"), "{}", lines[reminder]);
    let (quoted, _) = row_with(&lines, prompt);
    assert_eq!(lines[quoted - 1].trim(), "Prompt");
    let (kept, _) = row_with(&lines, reply_text);
    assert_eq!(lines[kept - 1].trim(), "Reply");
}

#[test]
fn lifecycle_completion_renews_the_saved_preview_and_selects_the_final_assistant_response() {
    let (mut app, mut commands) = audit_agents();
    listed(&mut app, &["agent-1"], None);
    event_key(&mut app, KeyCode::Enter);
    let running = expect_read(&mut commands, "agent-1");
    answer_for(&mut app, "agent-1", &running, Ok(vec![reply("Halfway progress sentinel")]));
    row_with(&reader_body(&drawn(&mut app, 80, 24)), "Halfway progress sentinel");
    let mut task = app.sdk_inventory.tasks[0].clone();
    task.status = TaskStatus::Completed;
    task.metadata = Some(
        serde_json::json!({"task_type":"local_agent","subagent_type":"general-purpose","terminal_status":"completed"}),
    );
    lifecycle(&mut app, vec![task], &[]);
    assert!(commands.try_recv().is_err());
    event_key(&mut app, KeyCode::Esc);
    let completed = expect_read(&mut commands, "agent-1");
    assert_ne!(completed, running);
    assert!(!details(&drawn(&mut app, 80, 40)).join("\n").contains("Halfway"));
    answer_for(
        &mut app,
        "agent-1",
        &completed,
        Ok(vec![
            reply("Earlier answer sentinel"),
            message(SubagentMessageRole::Tool, "Intervening tool sentinel"),
            reply("Final verified findings"),
            message(SubagentMessageRole::User, "Trailing prompt sentinel"),
        ]),
    );
    let pane = details(&drawn(&mut app, 80, 40));
    let preview = pane[row_with(&pane, "Saved response").0 + 1..].join("\n");
    assert!(preview.contains("Final verified findings"));
    for stale in ["Halfway", "Earlier answer", "Intervening tool", "Trailing prompt"] {
        assert!(!preview.contains(stale));
    }
}

#[test]
fn an_interrupted_row_labels_its_text_as_last_progress_not_as_a_result() {
    let (mut app, _commands) = app_with_subagents();
    app.sdk_inventory.tasks = vec![subagent(
        "agent-9",
        "Watch the logs",
        TaskStatus::Completed,
        serde_json::json!({ "terminal_status": "interrupted", "summary": "tailing the third file" }),
    )];
    handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let lines = screen(&mut app, 120, 30);
    let (_, progress) = row_with(&lines, "Last progress: ");
    assert!(progress.contains("tailing the third file"), "{progress}");
    assert!(!lines.iter().any(|line| line.contains("Result: ")), "no outcome is claimed");
}

#[test]
fn a_transcript_longer_than_the_wrap_helpers_row_cap_stays_readable_to_the_end() {
    let (mut app, mut commands) = app_with_subagents();
    // Three rows per reply: more than u16::MAX rows in all.
    let messages: Vec<_> = (1..=25_000).map(|n| reply(&format!("finding {n}"))).collect();
    let request = open_transcript(&mut app, &mut commands);
    answer(&mut app, &request, Ok(messages));
    if let Some(view) = app.agents.transcript.as_mut() {
        view.scroll = usize::MAX;
    }
    row_with(&screen(&mut app, 60, 20), "finding 25000");
}

#[test]
fn undersized_agent_views_preserve_reader_selection_and_a_usable_return_to_chat() {
    for reader in [true, false] {
        let (mut app, mut commands) = audit_agents();
        app.input.set_text("preserved draft");
        if reader {
            let request = selected_reader(&mut app, &mut commands);
            answer(&mut app, &request, Ok(long_notes("resize-source")));
            drawn(&mut app, 80, 24);
            event_key(&mut app, KeyCode::PageDown);
        } else {
            let mut selected = app
                .sdk_inventory
                .tasks
                .iter()
                .find(|task| task.task_id == "agent-2")
                .unwrap()
                .clone();
            selected.status = TaskStatus::InProgress;
            selected.metadata =
                Some(serde_json::json!({"task_type":"local_agent","subagent_type":"reviewer"}));
            lifecycle(&mut app, vec![selected], &[]);
            listed(&mut app, &["agent-2"], None);
            event_key(&mut app, KeyCode::Down);
            assert!(commands.try_recv().is_err());
        }
        let before = drawn(&mut app, 80, 24);
        if reader {
            row_with(&reader_body(&before), "resize-source-20");
        } else {
            locate(&before, "\u{203a} Review the parser");
        }
        for (width, height) in [(29, 24), (80, 7), (29, 7)] {
            drawn(&mut app, width, height);
            let restored = drawn(&mut app, 80, 24);
            assert_eq!(restored, before, "resize preserves populated view and reading position");
            assert!(commands.try_recv().is_err());
        }
        if !reader {
            event_key(&mut app, KeyCode::Enter);
            let request = expect_read(&mut commands, "agent-2");
            answer(&mut app, &request, Ok(vec![reply("Selected saved report")]));
            row_with(&reader_body(&drawn(&mut app, 80, 24)), "Selected saved report");
        }
        drawn(&mut app, 29, 7);
        event_key(&mut app, KeyCode::Esc);
        locate(&drawn(&mut app, 80, 24), "\u{203a} Review the parser");
        drawn(&mut app, 29, 7);
        event_key(&mut app, KeyCode::Esc);
        assert_eq!(app.surface_mode, crate::app::SurfaceMode::Chat);
        assert_eq!(app.input.text(), "preserved draft");
        event_key(&mut app, KeyCode::Char('!'));
        assert_eq!(app.input.text(), "preserved draft!");
        assert!(commands.try_recv().is_err());
    }
}
