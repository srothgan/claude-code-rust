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
    assert_eq!(app.surface_mode, SurfaceMode::Chat, "a draft keeps Left as cursor movement");
    assert_eq!(app.input.cursor_col(), cursor - 1);
    assert_eq!(app.input.text(), "draft");
    assert!(sent(&mut commands).is_empty());

    app.input.clear();
    press(&mut app, KeyCode::Left);
    assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::Agents));
    assert_eq!(sent(&mut commands), vec![list_command()], "opening reads the transcript list once");

    press(&mut app, KeyCode::Esc);
    assert_eq!(app.surface_mode, SurfaceMode::Chat);
    assert!(app.input.is_empty());
}

#[test]
fn leaving_the_view_returns_to_the_composer_with_its_draft() {
    let (mut app, _commands) = connected_app();
    app.input.set_text("keep this\nsecond line");
    super::open(&mut app);
    assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::Agents));
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.surface_mode, SurfaceMode::Chat);
    assert_eq!(app.input.text(), "keep this\nsecond line");
}

#[test]
fn rows_are_subagent_lifecycle_tasks_counted_once_while_running() {
    let (mut app, _commands) = connected_app();
    let mut detached = task("agent-1", "local_agent", TaskStatus::InProgress, None);
    detached.source_tool_call_id = Some("tool-1".to_owned());
    lifecycle(
        &mut app,
        vec![
            detached.clone(),
            task("shell-1", "local_bash", TaskStatus::InProgress, None),
            task("agent-2", "local_agent", TaskStatus::Completed, Some("completed")),
            task("agent-3", "local_agent", TaskStatus::InProgress, None),
            task("agent-4", "local_agent", TaskStatus::Completed, Some("failed")),
        ],
    );
    // A repeated report of the same execution is still one row.
    lifecycle(&mut app, vec![detached]);

    assert_eq!(
        statuses(&app),
        vec![
            ("agent-1".to_owned(), SubagentStatus::Running),
            ("agent-2".to_owned(), SubagentStatus::Completed),
            ("agent-3".to_owned(), SubagentStatus::Running),
            ("agent-4".to_owned(), SubagentStatus::Failed),
        ]
    );
    assert_eq!(running_subagent_count(&app), 2);

    lifecycle(
        &mut app,
        vec![task("agent-3", "local_agent", TaskStatus::Completed, Some("completed"))],
    );
    assert_eq!(running_subagent_count(&app), 1);
}

#[test]
fn stop_is_sent_for_a_running_subagent_and_only_the_lifecycle_reports_it_stopped() {
    let (mut app, mut commands) = connected_app();
    lifecycle(
        &mut app,
        vec![
            task("agent-2", "local_agent", TaskStatus::Completed, Some("completed")),
            task("agent-1", "local_agent", TaskStatus::InProgress, None),
        ],
    );
    super::open(&mut app);
    let _ = sent(&mut commands);

    // The view opens on the running subagent.
    press(&mut app, KeyCode::Char('s'));
    press(&mut app, KeyCode::Char('s'));
    assert_eq!(
        sent(&mut commands),
        vec![BridgeCommand::StopSubagent {
            session_id: SESSION.to_owned(),
            task_id: "agent-1".to_owned()
        }],
        "one request per running subagent"
    );
    assert_eq!(statuses(&app)[1].1, SubagentStatus::Stopping);
    assert_eq!(running_subagent_count(&app), 1, "a subagent asked to stop is still running");

    handle_client_event(
        &mut app,
        ClientEvent::SubagentStopResult {
            session_id: SESSION.to_owned(),
            task_id: "agent-1".to_owned(),
            error: None,
        },
    );
    assert_eq!(
        statuses(&app)[1].1,
        SubagentStatus::Stopping,
        "an acknowledgement is not the outcome"
    );

    lifecycle(
        &mut app,
        vec![task("agent-1", "local_agent", TaskStatus::Completed, Some("killed"))],
    );
    assert_eq!(statuses(&app)[1].1, SubagentStatus::Stopped);
    assert_eq!(running_subagent_count(&app), 0);
    assert_eq!(
        app.agents.selected_task_id.as_deref(),
        Some("agent-1"),
        "the stopped row stays selected"
    );

    press(&mut app, KeyCode::Char('s'));
    press(&mut app, KeyCode::Up);
    press(&mut app, KeyCode::Char('s'));
    assert!(sent(&mut commands).is_empty(), "a finished subagent cannot be stopped");
}

#[test]
fn a_rejected_stop_reports_the_error_and_leaves_the_row_running() {
    let (mut app, mut commands) = connected_app();
    lifecycle(&mut app, vec![task("agent-1", "local_agent", TaskStatus::InProgress, None)]);
    super::open(&mut app);
    press(&mut app, KeyCode::Char('s'));
    assert!(sent(&mut commands).contains(&BridgeCommand::StopSubagent {
        session_id: SESSION.to_owned(),
        task_id: "agent-1".to_owned()
    }));

    handle_client_event(
        &mut app,
        ClientEvent::SubagentStopResult {
            session_id: SESSION.to_owned(),
            task_id: "agent-1".to_owned(),
            error: Some("Query closed before response received".to_owned()),
        },
    );
    assert_eq!(statuses(&app)[0].1, SubagentStatus::Running);
    let notice = app.agents.notice.as_ref().expect("error notice");
    assert_eq!(notice.kind, super::NoticeKind::Error);
    assert!(notice.text.contains("Query closed before response received"), "{}", notice.text);
}

#[test]
fn only_a_listed_id_has_a_transcript_and_reading_it_leaves_status_alone() {
    let (mut app, mut commands) = connected_app();
    lifecycle(
        &mut app,
        vec![
            task("agent-1", "local_agent", TaskStatus::InProgress, None),
            task("agent-2", "local_agent", TaskStatus::Completed, Some("completed")),
        ],
    );
    press(&mut app, KeyCode::Left);
    assert_eq!(sent(&mut commands), vec![list_command()]);

    press(&mut app, KeyCode::Enter);
    assert!(
        app.agents.transcript.is_none(),
        "nothing is inferred before the SDK lists transcripts"
    );
    assert!(sent(&mut commands).is_empty());

    handle_client_event(
        &mut app,
        ClientEvent::SubagentsListed {
            session_id: SESSION.to_owned(),
            agent_ids: vec!["agent-2".to_owned(), "tool-use-id".to_owned()],
            error: None,
        },
    );
    assert_eq!(app.agents.has_transcript("agent-1"), Some(false));
    assert_eq!(app.agents.has_transcript("agent-2"), Some(true));
    press(&mut app, KeyCode::Enter);
    assert!(app.agents.transcript.is_none(), "a running subagent not yet listed has no transcript");
    assert!(sent(&mut commands).is_empty());

    // Selecting the completed subagent reads its transcript for the response,
    // and opening it shows that same read.
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Enter);
    let reads = sent_reads(&mut commands);
    assert_eq!(
        reads.iter().map(|(command, _)| command.clone()).collect::<Vec<_>>(),
        [read_command("agent-2")],
        "a completed subagent's transcript is not read twice"
    );
    handle_client_event(
        &mut app,
        answer("agent-1", "another-request", vec![reply("someone else's")]),
    );
    handle_client_event(&mut app, answer("agent-2", &reads[0].1, vec![reply("Parser checked")]));
    assert!(app.agents.transcript.is_some(), "transcript open");
    let read = app.agents.read.as_ref().expect("transcript read");
    assert_eq!(read.messages.as_deref(), Some(&[reply("Parser checked")][..]));
    assert_eq!(
        statuses(&app)[0].1,
        SubagentStatus::Running,
        "a transcript read never changes status"
    );

    press(&mut app, KeyCode::Left);
    assert!(app.agents.transcript.is_none());
    assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::Agents));

    press(&mut app, KeyCode::Char('r'));
    assert_eq!(
        sent(&mut commands),
        vec![list_command(), read_command("agent-2")],
        "r checks transcripts again and reads the selected response again"
    );
    handle_client_event(
        &mut app,
        ClientEvent::SubagentsListed {
            session_id: SESSION.to_owned(),
            agent_ids: Vec::new(),
            error: Some("subagent directory unreadable".to_owned()),
        },
    );
    assert_eq!(
        app.agents.has_transcript("agent-2"),
        Some(true),
        "a failed re-check keeps the last list"
    );
    assert!(app.agents.list_error.as_ref().is_some_and(|error| error.contains("unreadable")));
}

#[test]
fn a_new_session_forgets_stop_requests_and_transcripts() {
    let (mut app, mut commands) = connected_app();
    lifecycle(&mut app, vec![task("agent-1", "local_agent", TaskStatus::InProgress, None)]);
    super::open(&mut app);
    press(&mut app, KeyCode::Char('s'));
    let _ = sent(&mut commands);
    handle_client_event(
        &mut app,
        ClientEvent::SubagentsListed {
            session_id: SESSION.to_owned(),
            agent_ids: vec!["agent-1".to_owned()],
            error: None,
        },
    );

    app.bump_session_scope_epoch();
    assert_eq!(app.agents.has_transcript("agent-1"), None);
    assert_eq!(statuses(&app)[0].1, SubagentStatus::Running);
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
            task_id: "agent-1".to_owned()
        }]
    );
}

#[test]
fn answers_for_another_session_and_a_stop_that_lost_the_race_change_nothing() {
    let (mut app, mut commands) = connected_app();
    lifecycle(&mut app, vec![task("agent-1", "local_agent", TaskStatus::InProgress, None)]);
    super::open(&mut app);
    press(&mut app, KeyCode::Char('s'));
    let _ = sent(&mut commands);
    handle_client_event(
        &mut app,
        ClientEvent::SubagentsListed {
            session_id: "previous-session".to_owned(),
            agent_ids: vec!["agent-1".to_owned()],
            error: None,
        },
    );
    assert_eq!(app.agents.has_transcript("agent-1"), None, "another session's list is ignored");

    lifecycle(
        &mut app,
        vec![task("agent-1", "local_agent", TaskStatus::Completed, Some("completed"))],
    );
    handle_client_event(
        &mut app,
        ClientEvent::SubagentStopResult {
            session_id: SESSION.to_owned(),
            task_id: "agent-1".to_owned(),
            error: Some("Task agent-1 is not running".to_owned()),
        },
    );
    assert!(app.agents.notice.is_none(), "the row already shows how the subagent ended");
    assert_eq!(statuses(&app)[0].1, SubagentStatus::Completed);
}

#[test]
fn ambient_subagents_are_neither_listed_nor_counted() {
    let (mut app, _commands) = connected_app();
    let mut watcher = task("watch-1", "local_agent", TaskStatus::InProgress, None);
    if let Some(metadata) = watcher.metadata.as_mut() {
        metadata["ambient"] = serde_json::json!(true);
    }
    lifecycle(
        &mut app,
        vec![watcher, task("agent-1", "local_agent", TaskStatus::InProgress, None)],
    );

    assert_eq!(statuses(&app), vec![("agent-1".to_owned(), SubagentStatus::Running)]);
    assert_eq!(running_subagent_count(&app), 1);
}

#[test]
fn an_interrupted_subagent_is_shown_as_such_not_counted_and_cannot_be_stopped() {
    let (mut app, mut commands) = connected_app();
    lifecycle(
        &mut app,
        vec![task("agent-1", "local_agent", TaskStatus::Completed, Some("interrupted"))],
    );

    assert_eq!(statuses(&app), vec![("agent-1".to_owned(), SubagentStatus::Interrupted)]);
    assert_eq!(running_subagent_count(&app), 0);

    press(&mut app, KeyCode::Left);
    sent(&mut commands);
    press(&mut app, KeyCode::Char('s'));
    assert!(sent(&mut commands).is_empty(), "no stop request for a task no process owns");
}

#[test]
fn a_new_transcript_answer_replaces_what_was_wrapped_for_the_old_one() {
    let (mut app, mut commands) = connected_app();
    lifecycle(
        &mut app,
        vec![task("agent-1", "local_agent", TaskStatus::Completed, Some("completed"))],
    );
    super::open(&mut app);
    super::apply_transcript_list(&mut app, vec!["agent-1".to_owned()], None);
    press(&mut app, KeyCode::Enter);
    let first = sent_reads(&mut commands).pop().expect("read sent").1;
    handle_client_event(&mut app, answer("agent-1", &first, vec![reply("first")]));
    if let Some(view) = app.agents.transcript.as_mut() {
        view.wrapped = Some(super::WrappedTranscript { width: 80, rows: Vec::new() });
    }
    press(&mut app, KeyCode::Char('r'));
    let second = sent_reads(&mut commands).pop().expect("read sent").1;
    handle_client_event(&mut app, answer("agent-1", &second, vec![reply("second")]));
    assert!(app.agents.transcript.is_some_and(|view| view.wrapped.is_none()));
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
    super::final_response(app, row).map(|found| found.map(str::to_owned).map_err(str::to_owned))
}

fn completed(id: &str) -> TaskItem {
    task(id, "local_agent", TaskStatus::Completed, Some("completed"))
}

#[test]
fn a_selected_subagent_that_completes_in_the_list_is_read_for_its_response() {
    let (mut app, mut commands) = connected_app();
    lifecycle(&mut app, vec![task("agent-1", "local_agent", TaskStatus::InProgress, None)]);
    super::open(&mut app);
    super::apply_transcript_list(&mut app, vec!["agent-1".to_owned()], None);
    assert!(sent_reads(&mut commands).is_empty(), "a running subagent has no response yet");

    lifecycle(&mut app, vec![completed("agent-1")]);
    let reads = sent_reads(&mut commands);
    assert_eq!(
        reads.iter().map(|(command, _)| command.clone()).collect::<Vec<_>>(),
        [read_command("agent-1")]
    );
    handle_client_event(&mut app, answer("agent-1", &reads[0].1, vec![reply("All checked")]));
    assert_eq!(response(&app, "agent-1"), Some(Ok("All checked".to_owned())));
}

#[test]
fn a_read_made_while_a_subagent_ran_never_stands_as_its_response() {
    let (mut app, mut commands) = connected_app();
    lifecycle(&mut app, vec![task("agent-1", "local_agent", TaskStatus::InProgress, None)]);
    super::open(&mut app);
    super::apply_transcript_list(&mut app, vec!["agent-1".to_owned()], None);
    press(&mut app, KeyCode::Enter);
    let (_, request) = sent_reads(&mut commands).pop().expect("read sent");
    handle_client_event(&mut app, answer("agent-1", &request, vec![reply("Halfway there")]));
    press(&mut app, KeyCode::Esc);

    // The bridge is gone, so the read after completion cannot be sent.
    drop(commands);
    lifecycle(&mut app, vec![completed("agent-1")]);
    assert_eq!(response(&app, "agent-1"), None, "the running-time read is not its response");
    assert!(
        app.agents.notice.as_ref().is_some_and(|notice| notice.text.contains("Could not read"))
    );
}

#[test]
fn the_first_transcript_list_reads_the_selected_completed_subagent() {
    let (mut app, mut commands) = connected_app();
    lifecycle(&mut app, vec![completed("agent-1")]);
    super::open(&mut app);
    assert_eq!(sent(&mut commands), vec![list_command()], "nothing is read before the list");
    handle_client_event(
        &mut app,
        ClientEvent::SubagentsListed {
            session_id: SESSION.to_owned(),
            agent_ids: vec!["agent-1".to_owned()],
            error: None,
        },
    );
    assert_eq!(sent(&mut commands), vec![read_command("agent-1")]);
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
    let (mut app, mut commands) = connected_app();
    lifecycle(&mut app, vec![completed("agent-a"), completed("agent-b"), completed("agent-c")]);
    super::open(&mut app);
    super::apply_transcript_list(
        &mut app,
        ["agent-a", "agent-b", "agent-c"].map(str::to_owned).to_vec(),
        None,
    );
    let (_, first) = sent_reads(&mut commands).pop().expect("the first row is read");

    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Down);
    assert!(sent_reads(&mut commands).is_empty(), "rows passed over wait for the read in flight");

    handle_client_event(&mut app, answer("agent-a", &first, vec![reply("a done")]));
    let reads = sent_reads(&mut commands);
    assert_eq!(
        reads.iter().map(|(command, _)| command.clone()).collect::<Vec<_>>(),
        [read_command("agent-c")]
    );
    assert_eq!(response(&app, "agent-a"), None, "a passed-over answer is not shown");
    handle_client_event(&mut app, answer("agent-c", &reads[0].1, vec![reply("c done")]));
    assert_eq!(response(&app, "agent-c"), Some(Ok("c done".to_owned())));
}

#[test]
fn an_open_transcript_changes_only_when_the_user_asks_and_a_closed_view_reads_nothing() {
    let (mut app, mut commands) = connected_app();
    lifecycle(&mut app, vec![task("agent-1", "local_agent", TaskStatus::InProgress, None)]);
    super::open(&mut app);
    super::apply_transcript_list(&mut app, vec!["agent-1".to_owned()], None);
    press(&mut app, KeyCode::Enter);
    let (_, request) = sent_reads(&mut commands).pop().expect("read sent");
    handle_client_event(&mut app, answer("agent-1", &request, vec![reply("Halfway there")]));

    // Completing while its transcript is read leaves the reader as it is.
    lifecycle(&mut app, vec![completed("agent-1")]);
    assert!(sent_reads(&mut commands).is_empty());
    let shown = app.agents.read.as_ref().and_then(|read| read.messages.clone());
    assert_eq!(shown, Some(vec![reply("Halfway there")]));
    press(&mut app, KeyCode::Esc);
    assert_eq!(sent(&mut commands), vec![read_command("agent-1")], "back in the list it is read");

    // A list answer that arrives after the view closed reads nothing.
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.surface_mode, SurfaceMode::Chat);
    app.agents.read = None;
    super::apply_transcript_list(&mut app, vec!["agent-1".to_owned()], None);
    assert!(sent(&mut commands).is_empty());
}

#[test]
fn a_subagent_resumed_after_it_completed_is_read_again_for_its_new_response() {
    let (mut app, mut commands) = connected_app();
    lifecycle(&mut app, vec![completed("agent-1")]);
    super::open(&mut app);
    super::apply_transcript_list(&mut app, vec!["agent-1".to_owned()], None);
    let (_, request) = sent_reads(&mut commands).pop().expect("read sent");
    handle_client_event(&mut app, answer("agent-1", &request, vec![reply("First report")]));
    assert_eq!(response(&app, "agent-1"), Some(Ok("First report".to_owned())));

    lifecycle(&mut app, vec![task("agent-1", "local_agent", TaskStatus::InProgress, None)]);
    assert_eq!(response(&app, "agent-1"), None, "a running subagent shows no response");
    lifecycle(&mut app, vec![completed("agent-1")]);
    let (_, request) = sent_reads(&mut commands).pop().expect("read again once it completes");
    assert_eq!(response(&app, "agent-1"), None, "the first report is not its new response");
    handle_client_event(
        &mut app,
        answer("agent-1", &request, vec![reply("First report"), reply("Second report")]),
    );
    assert_eq!(response(&app, "agent-1"), Some(Ok("Second report".to_owned())));
}
