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
fn both_hints_name_the_page_keys_as_general_help_does() {
    let (mut app, mut commands) = app_with_subagents();
    let lines = screen(&mut app, 120, 30);
    let (_, hint) = row_with(&lines, "Esc close");
    assert!(hint.contains("PageUp/PageDown details"), "{hint}");

    open_transcript(&mut app, &mut commands);
    let lines = screen(&mut app, 120, 30);
    let (_, hint) = row_with(&lines, "Esc/Left back");
    assert!(hint.contains("PageUp/PageDown page"), "{hint}");
    assert!(hint.contains("r refresh"), "{hint}");
}

#[test]
fn a_successful_recheck_clears_the_list_error_and_leaves_other_notices() {
    let (mut app, _commands) = app_with_subagents();
    let listed = |app: &mut App, error: Option<&str>| {
        crate::app::handle_client_event(
            app,
            ClientEvent::SubagentsListed {
                session_id: "session-1".to_owned(),
                agent_ids: if error.is_some() { Vec::new() } else { vec!["agent-2".to_owned()] },
                error: error.map(str::to_owned),
            },
        );
    };
    let list_error = "Could not check transcripts: subagent directory unreadable";
    let press = |app: &mut App, code| handle_key(app, KeyEvent::new(code, KeyModifiers::NONE));

    listed(&mut app, Some("subagent directory unreadable"));
    let lines = screen(&mut app, 80, 30);
    let (error_row, _) = row_with(&lines, list_error);
    let (hint_row, _) = row_with(&lines, "Esc close");
    assert!(error_row < hint_row, "the error sits above the hint");

    press(&mut app, KeyCode::Char('r'));
    listed(&mut app, None);
    let lines = screen(&mut app, 80, 30);
    assert!(!lines.iter().any(|line| line.contains("Could not check")), "{}", lines.join("\n"));

    // A notice about something else outlives a later successful check.
    press(&mut app, KeyCode::Char('r'));
    listed(&mut app, Some("subagent directory unreadable"));
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Char('s'));
    press(&mut app, KeyCode::Char('r'));
    listed(&mut app, None);
    let lines = screen(&mut app, 80, 30);
    row_with(&lines, "Only a running subagent can be stopped.");
    assert!(!lines.iter().any(|line| line.contains("Could not check")), "{}", lines.join("\n"));
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
fn a_long_title_is_shortened_so_the_status_badge_stays_in_the_list_column() {
    let (mut app, _commands) = app_with_subagents();
    app.sdk_inventory.tasks[0].subject =
        "List every file in the working directory and summarise each one".to_owned();
    let lines = screen(&mut app, 120, 30);
    let (_, row) = row_with(&lines, "› List every file");
    let list_column: String = row.chars().take(2 + 52).collect();
    assert!(list_column.contains("... ") && list_column.contains(" running "), "{row}");
}

#[test]
fn details_show_the_selected_subagents_model_activity_and_prompt_beside_or_below_the_list() {
    let (mut app, _commands) = app_with_subagents();
    tool_call(
        &mut app,
        "launch-1",
        "Agent",
        "Run the slow check",
        serde_json::json!({ "description": "Run the slow check", "prompt": "List the files here, then summarise", "subagent_type": "general-purpose", "model": "haiku" }),
        None,
    );
    for (id, title) in
        [("child-1", "Glob **/*"), ("child-2", "Read notes.md"), ("child-3", "Bash wc -l notes.md")]
    {
        tool_call(&mut app, id, "Read", title, serde_json::json!({}), Some("launch-1"));
    }
    app.sdk_inventory.tasks[0].source_tool_call_id = Some("launch-1".to_owned());
    app.sdk_inventory
        .available_agents
        .push(model::AvailableAgent::new("reviewer", "Reviews code").model("sonnet"));
    apply_transcript_list(&mut app, vec!["agent-1".to_owned()], None);

    let wide = screen(&mut app, 120, 30);
    let (row, list_line) = row_with(&wide, "› Run the slow check");
    let detail_column =
        list_line.find("Run the slow check  ").and(list_line.rfind("Run the slow check"));
    assert!(
        detail_column.is_some_and(|column| column > 50),
        "details sit beside the list: {list_line}"
    );
    let detail = |label: &str| {
        wide.iter()
            .find(|line| line.contains(label))
            .unwrap_or_else(|| panic!("no {label:?} row:\n{}", wide.join("\n")))
            .clone()
    };
    assert!(detail("Model: ").contains("haiku (requested)"));
    assert!(detail("Transcript: ").contains("saved, Enter to read"));
    assert!(detail("Now: ").contains("Bash wc -l notes.md"), "the newest child call is current");
    let (recent, _) = row_with(&wide, "Recent");
    assert!(wide[recent + 1].contains("Read notes.md") && wide[recent + 2].contains("Glob **/*"));
    assert!(
        !wide[recent + 1..].iter().any(|line| line.contains("Bash wc -l")),
        "Now is not repeated"
    );
    let (prompt, _) = row_with(&wide, "Prompt");
    assert!(wide[prompt + 1].contains("List the files here, then summarise"));
    assert!(row < prompt);

    handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let narrow = screen(&mut app, 80, 30);
    let (selected, _) = row_with(&narrow, "› Review the parser");
    let (_, model) = row_with(&narrow, "Model: ");
    assert!(model.contains("sonnet (agent default)"), "{model}");
    let (result, result_line) = row_with(&narrow, "Result: ");
    assert!(result_line.contains("Parser looks fine"), "{result_line}");
    assert!(result > selected, "details sit below the list when narrow");
    assert!(
        !narrow.iter().any(|line| line.contains("Now: ")),
        "a finished subagent has no current activity"
    );
}

#[test]
fn transcript_replaces_the_list_in_the_same_screen_and_scrolls() {
    let (mut app, mut commands) = app_with_subagents();
    let mut messages = vec![
        message(SubagentMessageRole::User, "Inspect the parser"),
        message(SubagentMessageRole::Tool, "ls parser"),
    ];
    messages.extend((1..=30).map(|n| reply(&format!("finding {n}"))));
    let request = open_transcript(&mut app, &mut commands);
    answer(&mut app, &request, Ok(messages));

    let lines = screen(&mut app, 60, 20);
    let (_, title) = row_with(&lines, "Subagents");
    assert!(title.contains("Review the parser"), "{title}");
    assert!(!lines.iter().any(|line| line.contains("Run the slow check")), "the list is replaced");
    let (prompt, _) = row_with(&lines, "Prompt");
    assert!(lines[prompt + 1].contains("Inspect the parser"));
    row_with(&lines, "• ls parser");
    let (hint_row, _) = row_with(&lines, "Up/Down scroll");
    let hint = lines[hint_row..].join(" ");
    assert!(hint.contains("Esc/Left back"), "{hint}");

    for _ in 0..200 {
        press(&mut app, KeyCode::Down);
    }
    let bottom = screen(&mut app, 60, 20);
    row_with(&bottom, "finding 30");
    assert!(!bottom.iter().any(|line| line.contains("Inspect the parser")));
    // Scrolling past the end stops at the last page, so one Up moves back.
    press(&mut app, KeyCode::Up);
    assert_ne!(screen(&mut app, 60, 20), bottom);
}

#[test]
fn a_session_without_subagents_and_an_unread_transcript_say_so() {
    let mut app = App::test_default();
    app.session_runtime.session_id = Some(SessionId::new("session-1"));
    open(&mut app);
    let lines = screen(&mut app, 60, 16);
    row_with(&lines, "No subagents yet");

    let (mut app, mut commands) = app_with_subagents();
    open_transcript(&mut app, &mut commands);
    let lines = screen(&mut app, 60, 16);
    row_with(&lines, "Reading transcript");
}

#[test]
fn transcript_follows_width_changes_and_replaced_messages_through_its_wrap_cache() {
    let (mut app, mut commands) = app_with_subagents();
    let long = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu";
    let request = open_transcript(&mut app, &mut commands);
    answer(&mut app, &request, Ok(vec![reply(long)]));

    let wide = screen(&mut app, 100, 12);
    row_with(&wide, long);
    let narrow = screen(&mut app, 30, 12);
    assert!(!narrow.iter().any(|line| line.contains(long)), "the cached rows were rewrapped");
    row_with(&narrow, "alpha beta");
    row_with(&narrow, "lambda mu");

    press(&mut app, KeyCode::Char('r'));
    let request = last_read_request(&mut commands);
    answer(&mut app, &request, Ok(vec![reply("fresh answer")]));
    row_with(&screen(&mut app, 30, 12), "fresh answer");
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
fn r_retries_a_transcript_that_could_not_be_read() {
    let (mut app, mut commands) = app_with_subagents();
    let request = open_transcript(&mut app, &mut commands);
    answer(&mut app, &request, Err("transcript unreadable"));
    let failed = screen(&mut app, 60, 20);
    row_with(&failed, "Could not read the transcript");
    row_with(&failed, "transcript unreadable");

    press(&mut app, KeyCode::Char('r'));
    let request = last_read_request(&mut commands);
    row_with(&screen(&mut app, 60, 20), "Reading transcript");
    answer(&mut app, &request, Ok(vec![reply("Parser checked")]));
    let read = screen(&mut app, 60, 20);
    row_with(&read, "Parser checked");
    assert!(!read.iter().any(|line| line.contains("Could not read")), "{}", read.join("\n"));

    // A re-read that fails keeps what was read, shown again while the next one runs.
    press(&mut app, KeyCode::Char('r'));
    let request = last_read_request(&mut commands);
    answer(&mut app, &request, Err("transcript unreadable"));
    row_with(&screen(&mut app, 60, 20), "transcript unreadable");
    press(&mut app, KeyCode::Char('r'));
    row_with(&screen(&mut app, 60, 20), "Parser checked");
}

#[test]
fn a_completed_subagents_response_is_previewed_in_six_rows_and_read_in_full_as_markdown() {
    let (mut app, mut commands) = app_with_subagents();
    apply_transcript_list(&mut app, vec!["agent-2".to_owned()], None);
    // A report handed back as the subagent's only answer.
    let report = [
        "## Docs survey",
        "",
        "The **docs** cover:",
        "",
        "- page 1",
        "- page 2",
        "- page 3",
        "- page 4",
        "- page 5",
    ];
    let transcript = vec![
        message(SubagentMessageRole::User, "Survey the docs"),
        message(SubagentMessageRole::Tool, "Glob docs/**"),
        reply(&report.join("\n")),
    ];
    press(&mut app, KeyCode::Down);
    let request = last_read_request(&mut commands);
    answer(&mut app, &request, Ok(transcript));

    let overview = screen(&mut app, 80, 40);
    let (heading, _) = row_with(&overview, "Response");
    let preview: Vec<_> =
        overview[heading + 1..=heading + 7].iter().map(|row| row.trim()).collect();
    assert_eq!(preview[..6], report[..6], "{}", overview.join("\n"));
    assert_eq!(preview[6], "...", "a longer response ends in an ellipsis");
    assert!(!overview.iter().any(|row| row.contains("page 3")), "{}", overview.join("\n"));

    press(&mut app, KeyCode::Enter);
    let reader = screen(&mut app, 80, 40);
    let (label, _) = row_with(&reader, "Reply");
    let body = &reader[label + 1..];
    let (_, heading) = row_with(body, "Docs survey");
    assert!(!heading.contains("##"), "the heading is rendered, not shown as source: {heading}");
    let (_, bold) = row_with(body, "docs cover:");
    assert!(bold.contains("The docs cover:"), "bold markers are rendered: {bold}");
    for page in 1..=5 {
        row_with(body, &format!("page {page}"));
    }
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
fn a_transcript_read_while_running_is_not_taken_as_the_response() {
    let (mut app, mut commands) = app_with_subagents();
    apply_transcript_list(&mut app, vec!["agent-1".to_owned()], None);
    press(&mut app, KeyCode::Enter);
    let request = last_read_request(&mut commands);
    answer_for(&mut app, "agent-1", &request, Ok(vec![reply("Halfway through the files")]));

    // It completes while its transcript is open; leaving reads it again.
    app.sdk_inventory.tasks[0].status = TaskStatus::Completed;
    app.sdk_inventory.tasks[0].metadata =
        Some(serde_json::json!({ "task_type": "local_agent", "terminal_status": "completed" }));
    press(&mut app, KeyCode::Esc);
    let request = last_read_request(&mut commands);
    let overview = screen(&mut app, 80, 40);
    assert!(!overview.iter().any(|row| row.contains("Halfway")), "{}", overview.join("\n"));

    answer_for(
        &mut app,
        "agent-1",
        &request,
        Ok(vec![reply("Halfway through the files"), reply("All files checked")]),
    );
    let overview = screen(&mut app, 80, 40);
    let (heading, _) = row_with(&overview, "Response");
    assert_eq!(overview[heading + 1].trim(), "All files checked");
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
