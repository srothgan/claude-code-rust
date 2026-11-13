// SPDX-License-Identifier: Apache-2.0

use super::{SubagentStatus, running_subagent_count, subagent_rows};
use crate::agent::client::{AgentConnection, CommandReceiver};
use crate::agent::events::ClientEvent;
use crate::agent::model::{self, SessionId, TaskItem, TaskStatus, TaskUpdateSource};
use crate::agent::types::{SubagentMessage, SubagentMessageRole};
use crate::agent::wire::{BridgeCommand, CommandEnvelope};
use crate::app::events::{handle_client_event, handle_terminal_event};
use crate::app::{App, FullscreenView, SurfaceMode};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use std::rc::Rc;

const SESSION: &str = "session-1";

fn connected_app() -> (App, CommandReceiver) {
    let mut app = App::test_default();
    let (conn, commands) = AgentConnection::test_channel();
    app.session_runtime.conn = Some(Rc::new(conn));
    app.session_runtime.session_id = Some(SessionId::new(SESSION));
    (app, commands)
}

fn press(app: &mut App, code: KeyCode) {
    handle_terminal_event(app, Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
}

fn task(id: &str, task_type: &str, status: TaskStatus, terminal: Option<&str>) -> TaskItem {
    let mut task = TaskItem::new(id, format!("Task {id}"), status);
    let mut metadata = serde_json::json!({ "task_type": task_type });
    if let Some(terminal) = terminal {
        metadata["terminal_status"] = serde_json::json!(terminal);
    }
    task.metadata = Some(metadata);
    task
}

/// Delivers lifecycle rows the way the bridge reports them.
fn lifecycle(app: &mut App, tasks: Vec<TaskItem>) {
    handle_client_event(
        app,
        ClientEvent::SessionUpdate {
            session_id: SESSION.to_owned(),
            update: model::SessionUpdate::TaskStateUpdate(
                model::TaskStateUpdate::new(TaskUpdateSource::Lifecycle).tasks(tasks),
            ),
        },
    );
}

fn sent_envelopes(commands: &mut CommandReceiver) -> Vec<CommandEnvelope> {
    std::iter::from_fn(|| commands.try_recv().ok()).collect()
}

fn sent(commands: &mut CommandReceiver) -> Vec<BridgeCommand> {
    sent_envelopes(commands).into_iter().map(|envelope| envelope.command).collect()
}

fn read_command(agent_id: &str) -> BridgeCommand {
    BridgeCommand::GetSubagentMessages {
        session_id: SESSION.to_owned(),
        agent_id: agent_id.to_owned(),
    }
}

/// The transcript reads among the sent commands, with their request ids.
fn sent_reads(commands: &mut CommandReceiver) -> Vec<(BridgeCommand, String)> {
    sent_envelopes(commands)
        .into_iter()
        .filter(|envelope| matches!(envelope.command, BridgeCommand::GetSubagentMessages { .. }))
        .map(|envelope| {
            let request_id = envelope.request_id.expect("a read names its request");
            (envelope.command, request_id)
        })
        .collect()
}

fn answer(agent_id: &str, request_id: &str, messages: Vec<SubagentMessage>) -> ClientEvent {
    ClientEvent::SubagentMessagesReceived {
        session_id: SESSION.to_owned(),
        agent_id: agent_id.to_owned(),
        request_id: Some(request_id.to_owned()),
        messages,
        error: None,
    }
}

fn reply(text: &str) -> SubagentMessage {
    SubagentMessage { role: SubagentMessageRole::Assistant, text: text.to_owned() }
}

fn statuses(app: &App) -> Vec<(String, SubagentStatus)> {
    subagent_rows(app).iter().map(|row| (row.task.task_id.clone(), row.status)).collect()
}

fn list_command() -> BridgeCommand {
    BridgeCommand::ListSubagents { session_id: SESSION.to_owned() }
}

#[test]
fn left_arrow_opens_agents_only_from_an_empty_composer() {
    let (mut app, mut commands) = connected_app();
    app.input.set_text("draft");
    let cursor = app.input.cursor_col();
    press(&mut app, KeyCode::Left);
    assert_eq!(app.surface_mode, SurfaceMode::Chat);
    assert_eq!(app.input.cursor_col(), cursor - 1);
    assert_eq!(app.input.text(), "draft");
    expect_commands(&mut commands, &[]);
    app.input.clear();
    handle_terminal_event(&mut app, Event::Paste("queued paste".into()));
    press(&mut app, KeyCode::Left);
    assert_eq!(app.surface_mode, SurfaceMode::Chat);
    expect_commands(&mut commands, &[]);

    let (mut app, mut commands) = connected_app();
    crate::app::test_support::admit_tool(
        &mut app,
        model::ToolCall::new("permission", "Permission target"),
    );
    let mut permission = crate::app::test_support::request_permission(
        &mut app,
        model::RequestPermissionRequest::new(
            SESSION,
            model::ToolCallUpdate::new("permission", model::ToolCallUpdateFields::new()),
            crate::app::test_support::permission_options("target"),
            None,
        ),
    );
    press(&mut app, KeyCode::Left);
    assert_eq!(app.surface_mode, SurfaceMode::Chat);
    assert!(matches!(permission.try_recv(), Err(tokio::sync::oneshot::error::TryRecvError::Empty)));
    expect_commands(&mut commands, &[]);

    let (mut app, mut commands) = connected_app();
    lifecycle(
        &mut app,
        vec![
            task("first", "local_agent", TaskStatus::InProgress, None),
            task("second", "local_agent", TaskStatus::InProgress, None),
        ],
    );
    press(&mut app, KeyCode::Left);
    expect_commands(&mut commands, &[list_command()]);
    assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::Agents));
    press(&mut app, KeyCode::Esc);
    assert!(app.input.is_empty());
    app.input.set_text("first line\n世界 draft");
    press(&mut app, KeyCode::Left);
    let cursor = app.input.cursor_col();
    super::open(&mut app);
    expect_commands(&mut commands, &[list_command()]);
    press(&mut app, KeyCode::Down);
    assert_eq!(app.agents.selected_task_id.as_deref(), Some("second"));
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.surface_mode, SurfaceMode::Chat);
    assert_eq!(app.input.text(), "first line\n世界 draft");
    assert_eq!(app.input.cursor_col(), cursor);
    press(&mut app, KeyCode::Char('!'));
    crate::app::test_support::press(&mut app, KeyCode::Enter);
    let (prompts, refreshes): (Vec<_>, Vec<_>) = sent_envelopes(&mut commands)
        .into_iter()
        .partition(|env| matches!(env.command, BridgeCommand::Prompt { .. }));
    for refresh in refreshes {
        assert!(
            matches!(refresh.command, BridgeCommand::GetContextUsage {ref session_id} if session_id==SESSION)
        );
    }
    let [command] = prompts.as_slice() else { panic!("exactly one submitted prompt: {prompts:?}") };
    let BridgeCommand::Prompt { session_id, chunks, inline_pastes, .. } = &command.command else {
        panic!("submitted prompt")
    };
    assert_eq!(session_id, SESSION);
    assert!(inline_pastes.is_empty());
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].kind, "text");
    assert_eq!(chunks[0].value, serde_json::json!("first line\n\u{4e16}\u{754c} draf!t"));
}

#[test]
fn rows_are_subagent_lifecycle_tasks_counted_once_while_running() {
    let (mut app, _commands) = connected_app();
    let mut ambient = task("watcher", "local_agent", TaskStatus::InProgress, None);
    ambient.metadata.as_mut().unwrap()["ambient"] = serde_json::json!(true);
    let running = task("active", "local_agent", TaskStatus::InProgress, None);
    lifecycle(
        &mut app,
        vec![
            running.clone(),
            completed("done"),
            task("failed", "local_agent", TaskStatus::Completed, Some("failed")),
            task("other", "local_agent", TaskStatus::InProgress, None),
            task("shell", "local_bash", TaskStatus::InProgress, None),
            ambient,
        ],
    );
    lifecycle(&mut app, vec![running]);
    let mut observed = statuses(&app);
    observed.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        observed,
        vec![
            ("active".into(), SubagentStatus::Running),
            ("done".into(), SubagentStatus::Completed),
            ("failed".into(), SubagentStatus::Failed),
            ("other".into(), SubagentStatus::Running)
        ]
    );
    assert_eq!(running_subagent_count(&app), 2);
    lifecycle(&mut app, vec![completed("active")]);
    assert_eq!(status(&app, "active"), SubagentStatus::Completed);
    assert_eq!(status(&app, "other"), SubagentStatus::Running);
    assert_eq!(running_subagent_count(&app), 1);
}

#[test]
fn stop_is_sent_for_a_running_subagent_and_only_the_lifecycle_reports_it_stopped() {
    for rejected in [false, true] {
        let (mut app, mut commands) = connected_app();
        lifecycle(
            &mut app,
            vec![
                completed("done"),
                execution("run-stop", TaskStatus::InProgress),
                task("interrupted", "local_agent", TaskStatus::Completed, Some("interrupted")),
            ],
        );
        super::open(&mut app);
        expect_commands(&mut commands, &[list_command()]);
        press(&mut app, KeyCode::Char('s'));
        press(&mut app, KeyCode::Char('s'));
        let first = expect_stop(&mut commands, "worker", Some("run-stop"));
        assert_eq!(status(&app, "worker"), SubagentStatus::Stopping);
        assert_eq!(running_subagent_count(&app), 1);
        stop_result(
            &mut app,
            SESSION,
            "worker",
            Some("run-stop"),
            &first,
            rejected.then_some("SDK stop failure"),
        );
        if rejected {
            assert_eq!(status(&app, "worker"), SubagentStatus::Running);
            assert!(screen(&mut app).contains("SDK stop failure"));
            press(&mut app, KeyCode::Char('s'));
            let second = expect_stop(&mut commands, "worker", Some("run-stop"));
            assert_ne!(first, second);
            stop_result(&mut app, SESSION, "worker", Some("run-stop"), &second, None);
        }
        assert_eq!(status(&app, "worker"), SubagentStatus::Stopping);
        assert_eq!(running_subagent_count(&app), 1);
        let mut stopped = execution("run-stop", TaskStatus::Completed);
        stopped.metadata.as_mut().unwrap()["terminal_status"] = serde_json::json!("killed");
        lifecycle(&mut app, vec![stopped]);
        assert_eq!(status(&app, "worker"), SubagentStatus::Stopped);
        assert_eq!(app.agents.selected_task_id.as_deref(), Some("worker"));
        assert_eq!(running_subagent_count(&app), 0);
        press(&mut app, KeyCode::Char('s'));
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Char('s'));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char('s'));
        assert_eq!(status(&app, "interrupted"), SubagentStatus::Interrupted);
        expect_commands(&mut commands, &[]);
    }
}

#[test]
fn ctrl_c_closes_the_saved_agent_reader_without_stopping_or_submitting() {
    let (mut app, mut commands) = connected_app();
    app.status = crate::app::AppStatus::Ready;
    app.input.set_text("agents draft 界");
    lifecycle(&mut app, vec![completed("target")]);
    super::open(&mut app);
    expect_commands(&mut commands, &[list_command()]);
    listed(&mut app, &["target"]);
    let request = expect_read(&mut commands, "target");
    handle_client_event(&mut app, answer("target", &request, vec![reply("AGENT_READER_CEDAR")]));
    press(&mut app, KeyCode::Enter);
    assert!(app.agents.transcript.is_some());
    handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
    );
    assert_eq!(app.surface_mode, SurfaceMode::Chat);
    assert!(app.agents.transcript.is_none());
    assert_eq!(status(&app, "target"), SubagentStatus::Completed);
    assert!(!app.shutdown_requested());
    assert_eq!(app.input.text(), "agents draft 界");
    expect_commands(&mut commands, &[]);
    super::open(&mut app);
    expect_commands(&mut commands, &[list_command()]);
    assert!(app.agents.transcript.is_none(), "reopen must show the list, not the cancelled reader");
    handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
    );
    handle_terminal_event(&mut app, Event::Paste("!".into()));
    crate::app::finalize_pending_paste_event(&mut app);
    crate::app::test_support::press(&mut app, KeyCode::Enter);
    crate::app::test_support::expect_session_prompt(
        &mut commands,
        SESSION,
        "agents draft 界!",
        &[],
        &[],
    );
}

#[test]
fn only_a_listed_id_has_a_transcript_and_reading_it_leaves_status_alone() {
    for completed_first in [false, true] {
        let (mut app, mut commands) = connected_app();
        let mut target = task("target", "local_agent", TaskStatus::InProgress, None);
        target.source_tool_call_id = Some("launch-id".into());
        lifecycle(
            &mut app,
            vec![target, task("neighbor", "local_agent", TaskStatus::InProgress, None)],
        );
        super::open(&mut app);
        expect_commands(&mut commands, &[list_command()]);
        press(&mut app, KeyCode::Enter);
        expect_commands(&mut commands, &[]);
        assert!(app.agents.transcript.is_none());
        listed(&mut app, &["launch-id", "neighbor"]);
        press(&mut app, KeyCode::Enter);
        expect_commands(&mut commands, &[]);
        assert!(app.agents.transcript.is_none());
        if completed_first {
            lifecycle(&mut app, vec![completed("target")]);
        } else {
            listed(&mut app, &["target", "neighbor"]);
        }
        expect_commands(&mut commands, &[]);
        if completed_first {
            listed(&mut app, &["target", "neighbor"]);
        } else {
            lifecycle(&mut app, vec![completed("target")]);
        }
        let request = expect_read(&mut commands, "target");
        handle_client_event(
            &mut app,
            answer("target", &request, vec![reply("Target saved content")]),
        );
        assert_eq!(response(&app, "target"), Some(Ok("Target saved content".into())));
        assert_eq!(status(&app, "target"), SubagentStatus::Completed);
        assert_eq!(status(&app, "neighbor"), SubagentStatus::Running);
        press(&mut app, KeyCode::Enter);
        expect_commands(&mut commands, &[]);
        assert_eq!(
            app.agents.read.as_ref().unwrap().messages,
            Some(vec![reply("Target saved content")])
        );
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Char('r'));
        let envelopes = expect_commands(&mut commands, &[list_command(), read_command("target")]);
        let refreshed = envelopes[1].request_id.as_deref().unwrap();
        handle_client_event(
            &mut app,
            ClientEvent::SubagentsListed {
                session_id: SESSION.into(),
                agent_ids: vec![],
                error: Some("directory refused".into()),
            },
        );
        assert!(screen(&mut app).contains("directory refused"));
        handle_client_event(
            &mut app,
            answer("target", refreshed, vec![reply("Accessible after list failure")]),
        );
        press(&mut app, KeyCode::Enter);
        expect_commands(&mut commands, &[]);
        assert!(app.agents.transcript.is_some());
        assert_eq!(
            app.agents.read.as_ref().unwrap().messages,
            Some(vec![reply("Accessible after list failure")])
        );
        assert_eq!(status(&app, "target"), SubagentStatus::Completed);
        assert_eq!(status(&app, "neighbor"), SubagentStatus::Running);
    }
}

#[test]
fn a_new_session_forgets_stop_requests_and_transcripts() {
    let (mut app, mut commands) = connected_app();
    lifecycle(&mut app, vec![execution("old-run", TaskStatus::InProgress)]);
    super::open(&mut app);
    expect_commands(&mut commands, &[list_command()]);
    press(&mut app, KeyCode::Char('s'));
    let old_stop = expect_stop(&mut commands, "worker", Some("old-run"));
    listed(&mut app, &["worker"]);
    press(&mut app, KeyCode::Enter);
    let old_read = expect_read(&mut commands, "worker");
    handle_client_event(
        &mut app,
        answer("worker", &old_read, vec![reply("Old session transcript")]),
    );
    press(&mut app, KeyCode::Char('r'));
    let outstanding = expect_read(&mut commands, "worker");
    replace_session_with_new_worker(&mut app, &mut commands);
    super::open(&mut app);
    expect_commands(
        &mut commands,
        &[BridgeCommand::ListSubagents { session_id: "replacement".into() }],
    );
    handle_client_event(
        &mut app,
        answer("worker", &outstanding, vec![reply("Late old transcript")]),
    );
    stop_result(
        &mut app,
        SESSION,
        "worker",
        Some("old-run"),
        &old_stop,
        Some("Late old stop failure"),
    );
    listed(&mut app, &["new-worker", "worker"]);
    press(&mut app, KeyCode::Enter);
    expect_commands(&mut commands, &[]);
    assert!(app.agents.transcript.is_none());
    assert!(app.agents.read.is_none());
    assert_eq!(status(&app, "new-worker"), SubagentStatus::Running);
    assert!(!screen(&mut app).contains("Late old stop failure"));
    handle_client_event(
        &mut app,
        ClientEvent::SubagentsListed {
            session_id: "replacement".into(),
            agent_ids: vec!["new-worker".into()],
            error: None,
        },
    );
    press(&mut app, KeyCode::Enter);
    let env = expect_commands(
        &mut commands,
        &[BridgeCommand::GetSubagentMessages {
            session_id: "replacement".into(),
            agent_id: "new-worker".into(),
        }],
    );
    handle_client_event(
        &mut app,
        ClientEvent::SubagentMessagesReceived {
            session_id: "replacement".into(),
            agent_id: "new-worker".into(),
            request_id: env[0].request_id.clone(),
            messages: vec![reply("New session content")],
            error: None,
        },
    );
    assert_eq!(
        app.agents.read.as_ref().unwrap().messages,
        Some(vec![reply("New session content")])
    );
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Char('s'));
    let env = expect_commands(
        &mut commands,
        &[BridgeCommand::StopSubagent {
            session_id: "replacement".into(),
            task_id: "new-worker".into(),
            expected_run_id: None,
        }],
    );
    stop_result(
        &mut app,
        "replacement",
        "new-worker",
        None,
        env[0].request_id.as_deref().unwrap(),
        None,
    );
    assert_eq!(status(&app, "new-worker"), SubagentStatus::Stopping);
    expect_commands(&mut commands, &[]);
}

#[test]
fn a_selection_whose_subagent_leaves_is_not_moved_to_another_row() {
    let (mut app, mut commands) = connected_app();
    lifecycle(
        &mut app,
        vec![
            task("agent-1", "local_agent", TaskStatus::InProgress, None),
            task("agent-2", "local_agent", TaskStatus::InProgress, None),
        ],
    );
    super::open(&mut app);
    press(&mut app, KeyCode::Down);
    let _ = sent(&mut commands);
    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate {
            session_id: SESSION.to_owned(),
            update: model::SessionUpdate::TaskStateUpdate(
                model::TaskStateUpdate::new(TaskUpdateSource::Lifecycle)
                    .removed_task_ids(vec!["agent-2".to_owned()]),
            ),
        },
    );
    press(&mut app, KeyCode::Char('s'));
    assert!(sent(&mut commands).is_empty(), "s never stops a subagent the user did not choose");
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Char('s'));
    assert_eq!(
        sent(&mut commands),
        vec![BridgeCommand::StopSubagent {
            session_id: SESSION.to_owned(),
            task_id: "agent-1".to_owned(),
            expected_run_id: None,
        }]
    );
}

#[test]
fn answers_for_another_session_and_a_stop_that_lost_the_race_change_nothing() {
    for error in [None, Some("Late stopped-task failure")] {
        let (mut app, mut commands) = connected_app();
        lifecycle(&mut app, vec![execution("run", TaskStatus::InProgress)]);
        super::open(&mut app);
        expect_commands(&mut commands, &[list_command()]);
        press(&mut app, KeyCode::Char('s'));
        let stop = expect_stop(&mut commands, "worker", Some("run"));
        handle_client_event(
            &mut app,
            ClientEvent::SubagentsListed {
                session_id: "previous-session".into(),
                agent_ids: vec!["worker".into()],
                error: None,
            },
        );
        press(&mut app, KeyCode::Enter);
        expect_commands(&mut commands, &[]);
        assert!(app.agents.transcript.is_none());
        stop_result(
            &mut app,
            "previous-session",
            "worker",
            Some("run"),
            &stop,
            Some("Wrong-session stop"),
        );
        assert_eq!(status(&app, "worker"), SubagentStatus::Stopping);
        listed(&mut app, &["worker"]);
        press(&mut app, KeyCode::Enter);
        let read = expect_read(&mut commands, "worker");
        let mut wrong = answer("worker", &read, vec![reply("Wrong session text")]);
        if let ClientEvent::SubagentMessagesReceived { session_id, .. } = &mut wrong {
            *session_id = "previous-session".into();
        }
        handle_client_event(&mut app, wrong);
        assert!(app.agents.read.as_ref().unwrap().messages.is_none());
        handle_client_event(&mut app, answer("worker", &read, vec![reply("Current session text")]));
        assert_eq!(
            app.agents.read.as_ref().unwrap().messages,
            Some(vec![reply("Current session text")])
        );
        stop_result(&mut app, SESSION, "worker", Some("run"), &stop, None);
        assert_eq!(status(&app, "worker"), SubagentStatus::Stopping);
        lifecycle(&mut app, vec![execution("run", TaskStatus::Completed)]);
        let notice = app.agents.notice.clone();
        stop_result(&mut app, SESSION, "worker", Some("run"), &stop, error);
        assert_eq!(status(&app, "worker"), SubagentStatus::Completed);
        assert_eq!(app.agents.notice, notice);
        expect_commands(&mut commands, &[]);
    }
}

#[test]
fn r_reads_the_open_transcript_again_and_only_the_latest_request_is_applied() {
    let (mut app, mut commands) = connected_app();
    lifecycle(&mut app, vec![task("agent-1", "local_agent", TaskStatus::InProgress, None)]);
    super::open(&mut app);
    super::apply_transcript_list(&mut app, vec!["agent-1".to_owned()], None);
    press(&mut app, KeyCode::Enter);
    let first = sent_reads(&mut commands).pop().expect("read sent").1;
    handle_client_event(&mut app, answer("agent-1", &first, vec![reply("step 1")]));

    press(&mut app, KeyCode::Char('r'));
    let reads = sent_reads(&mut commands);
    assert_eq!(reads.len(), 1, "r reads the open transcript again");
    let (command, second) = &reads[0];
    assert_eq!(command, &read_command("agent-1"));
    assert_ne!(second, &first, "each read is its own request");
    let messages = |app: &App| app.agents.read.as_ref().and_then(|read| read.messages.clone());
    assert_eq!(messages(&app), Some(vec![reply("step 1")]), "shown until the answer");

    // An answer to the superseded request arrives late and changes nothing.
    handle_client_event(&mut app, answer("agent-1", &first, vec![reply("stale")]));
    assert_eq!(messages(&app), Some(vec![reply("step 1")]));
    handle_client_event(
        &mut app,
        answer("agent-1", second, vec![reply("step 1"), reply("step 2")]),
    );
    assert_eq!(messages(&app), Some(vec![reply("step 1"), reply("step 2")]));
    assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::Agents));
    assert!(app.agents.transcript.is_some(), "the transcript stays open");
}

fn response(app: &App, task_id: &str) -> Option<Result<String, String>> {
    let rows = subagent_rows(app);
    let row = rows.iter().find(|row| row.task.task_id == task_id)?;
    super::saved_response(app, row).map(|found| found.map(str::to_owned).map_err(str::to_owned))
}

fn completed(id: &str) -> TaskItem {
    task(id, "local_agent", TaskStatus::Completed, Some("completed"))
}

fn execution(run: &str, status: TaskStatus) -> TaskItem {
    let mut row = task(
        "worker",
        "local_agent",
        status,
        (status == TaskStatus::Completed).then_some("completed"),
    );
    row.run_id = Some(run.to_owned());
    row
}

#[test]
fn background_snapshots_preserve_lifecycle_order_and_task_selection() {
    let (mut app, mut commands) = connected_app();
    let mut first = task("first", "local_agent", TaskStatus::InProgress, None);
    let mut second = task("second", "local_agent", TaskStatus::InProgress, None);
    for t in [&mut first, &mut second] {
        t.metadata.as_mut().unwrap()["sdk_background_task"] = serde_json::json!(true);
    }
    lifecycle(&mut app, vec![first.clone(), second.clone()]);
    super::open(&mut app);
    expect_commands(&mut commands, &[list_command()]);
    press(&mut app, KeyCode::Down);
    first.subject = "Updated first task".into();
    second.subject = "Updated second task".into();
    second.metadata.as_mut().unwrap()["summary"] = serde_json::json!("Second progress sentinel");
    lifecycle_snapshot(&mut app, vec![second, first]);
    assert_eq!(
        subagent_rows(&app).iter().map(|r| r.task.task_id.as_str()).collect::<Vec<_>>(),
        ["first", "second"]
    );
    assert_eq!(app.agents.selected_task_id.as_deref(), Some("second"));
    let rendered = screen(&mut app);
    assert!(rendered.contains("Second progress sentinel"));
    assert!(rendered.contains("Updated second task"));
    press(&mut app, KeyCode::Char('s'));
    expect_stop(&mut commands, "second", None);
    assert_eq!(status(&app, "first"), SubagentStatus::Running);
    assert_eq!(status(&app, "second"), SubagentStatus::Stopping);
}

#[test]
fn a_new_completed_run_invalidates_the_preview_even_without_a_running_transition() {
    for versioned in [false, true] {
        let (mut app, mut commands) = connected_app();
        lifecycle(
            &mut app,
            vec![if versioned {
                execution("A", TaskStatus::Completed)
            } else {
                completed("worker")
            }],
        );
        super::open(&mut app);
        expect_commands(&mut commands, &[list_command()]);
        listed(&mut app, &["worker"]);
        let old = expect_read(&mut commands, "worker");
        handle_client_event(&mut app, answer("worker", &old, vec![reply("A report")]));
        assert_eq!(response(&app, "worker"), Some(Ok("A report".into())));
        if !versioned {
            lifecycle(&mut app, vec![task("worker", "local_agent", TaskStatus::InProgress, None)]);
            assert_eq!(response(&app, "worker"), None);
            expect_commands(&mut commands, &[]);
        }
        lifecycle(
            &mut app,
            vec![if versioned {
                execution("B", TaskStatus::Completed)
            } else {
                completed("worker")
            }],
        );
        let new = expect_read(&mut commands, "worker");
        assert_ne!(old, new);
        assert_eq!(response(&app, "worker"), None);
        handle_client_event(&mut app, answer("worker", &old, vec![reply("Superseded answer")]));
        assert_eq!(response(&app, "worker"), None);
        expect_commands(&mut commands, &[]);
        let messages = if versioned {
            vec![reply("A report")]
        } else {
            vec![
                reply("A report"),
                SubagentMessage { role: SubagentMessageRole::User, text: "Next request".into() },
                reply("Latest resumed answer"),
            ]
        };
        handle_client_event(&mut app, answer("worker", &new, messages));
        assert_eq!(
            response(&app, "worker"),
            Some(Ok(if versioned { "A report" } else { "Latest resumed answer" }.into()))
        );
        assert_eq!(
            app.sdk_inventory
                .tasks
                .iter()
                .find(|t| t.task_id == "worker")
                .unwrap()
                .run_id
                .as_deref(),
            versioned.then_some("B")
        );
        assert_eq!(status(&app, "worker"), SubagentStatus::Completed);
    }
}

#[test]
fn a_run_change_while_the_view_is_closed_invalidates_old_preview_and_stop_state() {
    let (mut app, mut commands) = connected_app();
    lifecycle(&mut app, vec![execution("A", TaskStatus::InProgress)]);
    super::open(&mut app);
    expect_commands(&mut commands, &[list_command()]);
    press(&mut app, KeyCode::Char('s'));
    let stop_a = expect_stop(&mut commands, "worker", Some("A"));
    lifecycle(&mut app, vec![execution("A", TaskStatus::Completed)]);
    listed(&mut app, &["worker"]);
    let read = expect_read(&mut commands, "worker");
    handle_client_event(&mut app, answer("worker", &read, vec![reply("A report")]));
    press(&mut app, KeyCode::Esc);
    lifecycle(&mut app, vec![execution("B", TaskStatus::InProgress)]);
    assert_eq!(response(&app, "worker"), None);
    expect_commands(&mut commands, &[]);
    super::open(&mut app);
    expect_commands(&mut commands, &[list_command()]);
    press(&mut app, KeyCode::Char('s'));
    let stop_b = expect_stop(&mut commands, "worker", Some("B"));
    for error in [None, Some("Old A failure")] {
        stop_result(&mut app, SESSION, "worker", Some("A"), &stop_a, error);
        assert_eq!(status(&app, "worker"), SubagentStatus::Stopping);
        assert!(app.agents.notice.is_none());
    }
    stop_result(&mut app, SESSION, "worker", Some("B"), &stop_b, Some("B rejection"));
    assert_eq!(status(&app, "worker"), SubagentStatus::Running);
    assert!(screen(&mut app).contains("B rejection"));
    press(&mut app, KeyCode::Char('s'));
    let retry = expect_stop(&mut commands, "worker", Some("B"));
    assert_ne!(retry, stop_b);
    for error in [None, Some("Old B rejection")] {
        stop_result(&mut app, SESSION, "worker", Some("B"), &stop_b, error);
        assert_eq!(status(&app, "worker"), SubagentStatus::Stopping);
        assert!(app.agents.notice.is_none());
    }
    stop_result(&mut app, SESSION, "worker", Some("B"), &retry, None);
    assert_eq!(status(&app, "worker"), SubagentStatus::Stopping);
    assert!(app.agents.notice.is_some());
    expect_commands(&mut commands, &[]);
}

#[test]
fn a_read_made_while_a_subagent_ran_never_stands_as_its_response() {
    let (mut app, mut commands) = connected_app();
    lifecycle(&mut app, vec![task("worker", "local_agent", TaskStatus::InProgress, None)]);
    super::open(&mut app);
    expect_commands(&mut commands, &[list_command()]);
    listed(&mut app, &["worker"]);
    press(&mut app, KeyCode::Enter);
    let old = expect_read(&mut commands, "worker");
    handle_client_event(&mut app, answer("worker", &old, vec![reply("Running-time reply")]));
    press(&mut app, KeyCode::Esc);
    let filler = saturate(&app);
    lifecycle(&mut app, vec![completed("worker")]);
    assert_eq!(response(&app, "worker"), None);
    let rendered = screen(&mut app);
    assert!(!rendered.contains("Running-time reply"));
    assert!(rendered.contains("regular command queue is full"));
    expect_commands(&mut commands, &filler);
    press(&mut app, KeyCode::Char('r'));
    let env = expect_commands(&mut commands, &[list_command(), read_command("worker")]);
    let request = env[1].request_id.as_deref().unwrap();
    handle_client_event(&mut app, answer("worker", request, vec![reply("Completed saved reply")]));
    assert_eq!(response(&app, "worker"), Some(Ok("Completed saved reply".into())));
    assert!(screen(&mut app).contains("Completed saved reply"));
}

#[test]
fn a_failed_response_read_is_retried_by_r_and_not_by_every_lifecycle_update() {
    let (mut app, mut commands) = connected_app();
    lifecycle(&mut app, vec![completed("agent-1")]);
    super::open(&mut app);
    super::apply_transcript_list(&mut app, vec!["agent-1".to_owned()], None);
    let (_, request) = sent_reads(&mut commands).pop().expect("read sent");
    handle_client_event(
        &mut app,
        ClientEvent::SubagentMessagesReceived {
            session_id: SESSION.to_owned(),
            agent_id: "agent-1".to_owned(),
            request_id: Some(request),
            messages: Vec::new(),
            error: Some("transcript unreadable".to_owned()),
        },
    );
    assert_eq!(response(&app, "agent-1"), Some(Err("transcript unreadable".to_owned())));

    lifecycle(&mut app, vec![task("other", "local_bash", TaskStatus::InProgress, None)]);
    assert!(sent_reads(&mut commands).is_empty(), "an unrelated update does not retry the read");

    press(&mut app, KeyCode::Char('r'));
    let (_, request) = sent_reads(&mut commands).pop().expect("r reads it again");
    handle_client_event(&mut app, answer("agent-1", &request, vec![reply("Checked")]));
    assert_eq!(response(&app, "agent-1"), Some(Ok("Checked".to_owned())));
}

#[test]
fn moving_along_the_list_keeps_one_response_read_in_flight() {
    for run_change in [false, true] {
        for failed in [false, true] {
            let (mut app, mut commands) = connected_app();
            let (initial, current) =
                if run_change { ("worker", "worker") } else { ("agent-a", "agent-c") };
            lifecycle(
                &mut app,
                if run_change {
                    vec![execution("A", TaskStatus::Completed)]
                } else {
                    vec![completed("agent-a"), completed("agent-b"), completed("agent-c")]
                },
            );
            super::open(&mut app);
            expect_commands(&mut commands, &[list_command()]);
            listed(&mut app, &["worker", "agent-a", "agent-b", "agent-c"]);
            let old = expect_read(&mut commands, initial);
            if run_change {
                lifecycle(&mut app, vec![execution("B", TaskStatus::Completed)]);
            } else {
                press(&mut app, KeyCode::Down);
                press(&mut app, KeyCode::Down);
            }
            expect_commands(&mut commands, &[]);
            handle_client_event(
                &mut app,
                answer("unrelated", "unrelated-request", vec![reply("Unrelated answer")]),
            );
            expect_commands(&mut commands, &[]);
            assert_eq!(response(&app, current), None);
            let mut event = answer(initial, &old, vec![reply("Superseded answer")]);
            if failed && let ClientEvent::SubagentMessagesReceived { error, .. } = &mut event {
                *error = Some("Superseded failure".into());
            }
            handle_client_event(&mut app, event);
            let new = expect_read(&mut commands, current);
            assert_ne!(old, new);
            assert_eq!(response(&app, current), None);
            handle_client_event(
                &mut app,
                answer(current, &new, vec![reply("Selected current answer")]),
            );
            handle_client_event(&mut app, answer(initial, &old, vec![reply("Late old answer")]));
            assert_eq!(response(&app, current), Some(Ok("Selected current answer".into())));
            expect_commands(&mut commands, &[]);
        }
    }
}

#[test]
fn an_open_transcript_changes_only_when_the_user_asks_and_a_closed_view_reads_nothing() {
    let (mut app, mut commands) = connected_app();
    lifecycle(&mut app, vec![task("worker", "local_agent", TaskStatus::InProgress, None)]);
    super::open(&mut app);
    expect_commands(&mut commands, &[list_command()]);
    listed(&mut app, &["worker"]);
    press(&mut app, KeyCode::Enter);
    let old = expect_read(&mut commands, "worker");
    handle_client_event(&mut app, answer("worker", &old, vec![reply("Running snapshot")]));
    lifecycle(&mut app, vec![completed("worker")]);
    expect_commands(&mut commands, &[]);
    assert_eq!(app.agents.read.as_ref().unwrap().messages, Some(vec![reply("Running snapshot")]));
    press(&mut app, KeyCode::Esc);
    let new = expect_read(&mut commands, "worker");
    assert_eq!(response(&app, "worker"), None);
    handle_client_event(&mut app, answer("worker", &new, vec![reply("Completed snapshot")]));
    assert_eq!(response(&app, "worker"), Some(Ok("Completed snapshot".into())));
    for read_pending in [false, true] {
        let (mut app, mut commands) = connected_app();
        lifecycle(&mut app, vec![completed("worker")]);
        super::open(&mut app);
        expect_commands(&mut commands, &[list_command()]);
        let request = if read_pending {
            listed(&mut app, &["worker"]);
            Some(expect_read(&mut commands, "worker"))
        } else {
            None
        };
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.surface_mode, SurfaceMode::Chat);
        if let Some(request) = &request {
            handle_client_event(
                &mut app,
                answer("worker", request, vec![reply("Arrived while closed")]),
            );
        } else {
            listed(&mut app, &["worker"]);
        }
        expect_commands(&mut commands, &[]);
        super::open(&mut app);
        if read_pending {
            expect_commands(&mut commands, &[list_command()]);
            assert_eq!(response(&app, "worker"), Some(Ok("Arrived while closed".into())));
        } else {
            let env = expect_commands(&mut commands, &[list_command(), read_command("worker")]);
            handle_client_event(
                &mut app,
                answer(
                    "worker",
                    env[1].request_id.as_deref().unwrap(),
                    vec![reply("Visible fresh read")],
                ),
            );
            assert_eq!(response(&app, "worker"), Some(Ok("Visible fresh read".into())));
        }
    }
}

fn listed(app: &mut App, ids: &[&str]) {
    handle_client_event(
        app,
        ClientEvent::SubagentsListed {
            session_id: SESSION.into(),
            agent_ids: ids.iter().map(|id| (*id).into()).collect(),
            error: None,
        },
    );
}
fn expect_commands(
    commands: &mut CommandReceiver,
    expected: &[BridgeCommand],
) -> Vec<CommandEnvelope> {
    let envelopes = sent_envelopes(commands);
    assert_eq!(
        envelopes.iter().map(|e| &e.command).collect::<Vec<_>>(),
        expected.iter().collect::<Vec<_>>()
    );
    envelopes
}
fn expect_read(commands: &mut CommandReceiver, id: &str) -> String {
    expect_commands(commands, &[read_command(id)]).remove(0).request_id.expect("correlated read")
}
#[test]
fn resumed_unversioned_agent_requires_a_fresh_response_after_leaving_its_open_reader() {
    let (mut app, mut commands) = connected_app();
    lifecycle(&mut app, vec![completed("worker")]);
    super::open(&mut app);
    expect_commands(&mut commands, &[list_command()]);
    listed(&mut app, &["worker"]);
    let old = expect_read(&mut commands, "worker");
    handle_client_event(&mut app, answer("worker", &old, vec![reply("Previous saved response")]));
    press(&mut app, KeyCode::Enter);
    expect_commands(&mut commands, &[]);
    lifecycle(&mut app, vec![task("worker", "local_agent", TaskStatus::InProgress, None)]);
    lifecycle(&mut app, vec![completed("worker")]);
    assert_eq!(
        app.agents.read.as_ref().unwrap().messages,
        Some(vec![reply("Previous saved response")])
    );
    assert!(app.agents.transcript.is_some());
    expect_commands(&mut commands, &[]);
    press(&mut app, KeyCode::Esc);
    let current = expect_read(&mut commands, "worker");
    assert_ne!(current, old);
    assert_eq!(response(&app, "worker"), None, "the old report is not a new completion preview");
    handle_client_event(
        &mut app,
        answer(
            "worker",
            &current,
            vec![reply("Previous saved response"), reply("Resumed saved response")],
        ),
    );
    assert_eq!(response(&app, "worker"), Some(Ok("Resumed saved response".into())));
}

fn status(app: &App, id: &str) -> SubagentStatus {
    subagent_rows(app).iter().find(|r| r.task.task_id == id).expect("named task").status
}
fn expect_stop(commands: &mut CommandReceiver, id: &str, run: Option<&str>) -> String {
    expect_commands(
        commands,
        &[BridgeCommand::StopSubagent {
            session_id: SESSION.into(),
            task_id: id.into(),
            expected_run_id: run.map(str::to_owned),
        }],
    )
    .remove(0)
    .request_id
    .expect("correlated stop")
}
fn stop_result(
    app: &mut App,
    session: &str,
    id: &str,
    run: Option<&str>,
    request: &str,
    error: Option<&str>,
) {
    handle_client_event(
        app,
        ClientEvent::SubagentStopResult {
            session_id: session.into(),
            task_id: id.into(),
            run_id: run.map(str::to_owned),
            request_id: Some(request.into()),
            error: error.map(str::to_owned),
        },
    );
}
fn screen(app: &mut App) -> String {
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 32)).unwrap();
    terminal.draw(|frame| crate::ui::render_fullscreen_surface(frame, app)).unwrap();
    terminal
        .backend()
        .buffer()
        .content
        .chunks(100)
        .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}
fn saturate(app: &App) -> Vec<BridgeCommand> {
    let conn = app.session_runtime.conn.as_ref().unwrap();
    let mut admitted = Vec::new();
    for n in 0..4096 {
        let model = format!("capacity-fixture-{n}");
        match conn.set_model(SESSION.into(), model.clone()) {
            Ok(()) => admitted.push(BridgeCommand::SetModel { session_id: SESSION.into(), model }),
            Err(error) => {
                assert!(error.to_string().contains("regular command queue is full"));
                return admitted;
            }
        }
    }
    panic!("bounded saturation fixture did not reach refusal")
}
#[test]
fn a_locally_refused_stop_keeps_the_selected_execution_retryable() {
    let (mut app, mut commands) = connected_app();
    lifecycle(&mut app, vec![execution("run-admission", TaskStatus::InProgress)]);
    super::open(&mut app);
    expect_commands(&mut commands, &[list_command()]);
    let filler = saturate(&app);
    press(&mut app, KeyCode::Char('s'));
    assert_eq!(status(&app, "worker"), SubagentStatus::Running);
    assert!(screen(&mut app).contains("regular command queue is full"));
    expect_commands(&mut commands, &filler);
    press(&mut app, KeyCode::Char('s'));
    let request = expect_stop(&mut commands, "worker", Some("run-admission"));
    stop_result(&mut app, SESSION, "worker", Some("run-admission"), &request, None);
    assert_eq!(status(&app, "worker"), SubagentStatus::Stopping);
    assert_eq!(running_subagent_count(&app), 1);
    press(&mut app, KeyCode::Char('s'));
    expect_commands(&mut commands, &[]);
}

fn lifecycle_snapshot(app: &mut App, tasks: Vec<TaskItem>) {
    handle_client_event(
        app,
        ClientEvent::SessionUpdate {
            session_id: SESSION.into(),
            update: model::SessionUpdate::TaskStateUpdate(
                model::TaskStateUpdate::new(TaskUpdateSource::BackgroundTasks)
                    .tasks(tasks)
                    .complete_snapshot(true),
            ),
        },
    );
}

fn replace_session_with_new_worker(app: &mut App, commands: &mut CommandReceiver) {
    handle_client_event(
        app,
        ClientEvent::SessionReplaced {
            session_id: SessionId::new("replacement"),
            cwd: "/replacement".into(),
            current_model: model::CurrentModel::new("new", "new", "new").authoritative(true),
            available_models: vec![],
            mode: None,
            fast_mode_state: model::FastModeState::Off,
            fast_mode_disabled_reason: None,
            ultracode: None,
            history_updates: vec![],
            restored_input: None,
        },
    );
    assert!(app.agents.read.is_none());
    assert!(app.agents.transcript.is_none());
    let setup = sent(commands);
    let expected = [
        BridgeCommand::GetMcpSnapshot { session_id: "replacement".into() },
        BridgeCommand::GetStatusSnapshot { session_id: "replacement".into() },
        BridgeCommand::GetContextUsage { session_id: "replacement".into() },
        BridgeCommand::InspectSettings {
            session_id: "replacement".into(),
            app_settings_path: None,
        },
    ];
    for command in setup {
        assert!(expected.contains(&command), "only current-session setup reads: {command:?}");
    }
    handle_client_event(
        app,
        ClientEvent::SessionUpdate {
            session_id: "replacement".into(),
            update: model::SessionUpdate::TaskStateUpdate(
                model::TaskStateUpdate::new(TaskUpdateSource::Lifecycle).tasks(vec![task(
                    "new-worker",
                    "local_agent",
                    TaskStatus::InProgress,
                    None,
                )]),
            ),
        },
    );
}
