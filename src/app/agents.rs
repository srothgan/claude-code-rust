// SPDX-License-Identifier: Apache-2.0

//! The `/agents` view of the active session's subagents.
//!
//! Rows and their live status come from the task lifecycle the bridge
//! normalizes into `sdk_inventory.tasks`. This module holds only view state and
//! the outcome of its own requests: which saved transcripts the SDK lists, the
//! transcript being read, and the stop requests it has sent.

use crate::agent::events::ClientEvent;
use crate::agent::model::{TaskItem, TaskStatus};
use crate::agent::types::{SubagentMessage, SubagentMessageRole};
use crate::app::{App, FullscreenView, SurfaceMode, ToolCallInfo};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::{HashMap, HashSet};

const SUBAGENT_TASK_TYPE: &str = "local_agent";
const PAGE_ROWS: u16 = 10;
/// Child tool calls the details list for one subagent, newest first.
const RECENT_CHILD_TOOLS: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SubagentStatus {
    Pending,
    Running,
    /// Still running by the lifecycle, after this view sent a stop request.
    Stopping,
    Completed,
    Failed,
    Stopped,
    /// Was running when its process went away (a resumed session, or a missed
    /// notification). Not running, and no outcome is known.
    Interrupted,
}

impl SubagentStatus {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Stopping => "stopping",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
            Self::Interrupted => "interrupted",
        }
    }

    /// Running by the lifecycle, whether or not a stop was requested.
    pub(crate) const fn is_running(self) -> bool {
        matches!(self, Self::Running | Self::Stopping)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NoticeKind {
    Info,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Notice {
    pub kind: NoticeKind,
    pub text: String,
}

/// One subagent's saved transcript as this view last read it.
#[derive(Debug)]
pub(crate) struct TranscriptRead {
    pub agent_id: String,
    pub run_id: Option<String>,
    pub saved_snapshot: bool,
    pending: bool,
    /// The request this read waits on; an answer to any earlier one is superseded.
    request_id: String,
    /// The last successful answer; `None` until one arrives. It stays shown
    /// while a later request for the same subagent is pending.
    pub messages: Option<Vec<SubagentMessage>>,
    /// Why the latest request failed.
    pub error: Option<String>,
    /// Whether the current run had completed when this request was sent.
    after_completion: bool,
}

#[derive(Debug)]
struct PendingStop {
    run_id: Option<String>,
    request_id: String,
}

/// The open transcript reader, showing `AgentsView::read`.
#[derive(Debug, Default)]
pub(crate) struct TranscriptView {
    pub scroll: usize,
    /// The messages wrapped to the width they were last drawn at, so a frame
    /// wraps nothing until the width or the messages change.
    pub wrapped: Option<WrappedTranscript>,
}

#[derive(Debug)]
pub(crate) struct WrappedTranscript {
    pub width: u16,
    pub rows: Vec<ratatui::text::Line<'static>>,
}

#[derive(Debug, Default)]
pub(crate) struct AgentsView {
    /// The chosen subagent. It never moves to another row on its own, so `s`
    /// and Enter act only on a subagent the user chose or the view opened on.
    pub selected_task_id: Option<String>,
    /// Ids `listSubagents()` returned; `None` until the first answer.
    pub transcript_ids: Option<HashSet<String>>,
    pub listing: bool,
    /// Why the last transcript check failed. The ids it could not refresh are
    /// kept, and the next successful check clears it.
    pub list_error: Option<String>,
    stop_requested: HashMap<String, PendingStop>,
    /// The transcript last read; the open reader shows it.
    pub read: Option<TranscriptRead>,
    /// An unanswered response read the selection has moved past. The next one
    /// waits for its answer, so moving along the list keeps at most one read
    /// in the session's command lane.
    read_in_flight: Option<String>,
    pub transcript: Option<TranscriptView>,
    pub notice: Option<Notice>,
    /// Scroll offset of the selected subagent's details.
    pub detail_scroll: u16,
}

pub(crate) struct SubagentRow<'a> {
    pub task: &'a TaskItem,
    pub status: SubagentStatus,
}

impl SubagentRow<'_> {
    pub(crate) fn subagent_type(&self) -> Option<&str> {
        metadata_str(self.task, "subagent_type")
    }

    pub(crate) fn summary(&self) -> Option<&str> {
        // The SDK often repeats the description as the summary.
        metadata_str(self.task, "summary")
            .filter(|summary| !summary.trim().is_empty() && *summary != self.task.subject)
    }

    /// Why a failed subagent failed, as the lifecycle reported it.
    pub(crate) fn failure(&self) -> Option<&str> {
        if self.status != SubagentStatus::Failed {
            return None;
        }
        metadata_str(self.task, "error").or_else(|| metadata_str(self.task, "terminal_reason"))
    }

    /// The prompt the lifecycle reported, for a subagent whose Agent call is gone.
    pub(crate) fn prompt(&self) -> Option<&str> {
        metadata_str(self.task, "prompt")
    }
}

/// Last saved reply read after this run completed, or the read error.
/// Persistence can lag the lifecycle: this is a preview, not a run's result.
pub(crate) fn saved_response<'a>(
    app: &'a App,
    row: &SubagentRow<'_>,
) -> Option<Result<&'a str, &'a str>> {
    if row.status != SubagentStatus::Completed {
        return None;
    }
    let read = app.agents.read.as_ref().filter(|read| {
        read.agent_id == row.task.task_id
            && read.run_id == row.task.run_id
            && read.after_completion
            && !read.saved_snapshot
    })?;
    if let Some(error) = &read.error {
        return Some(Err(error));
    }
    let messages = read.messages.as_ref()?;
    let reply =
        messages.iter().rev().find(|message| message.role == SubagentMessageRole::Assistant);
    reply.map(|message| Ok(message.text.as_str()))
}

/// The Agent call that launched a subagent, when the transcript still holds it.
pub(crate) fn launching_call<'a>(app: &'a App, task: &TaskItem) -> Option<&'a ToolCallInfo> {
    app.tool_call(task.source_tool_call_id.as_deref()?)
}

/// The subagent's own tool calls the session has seen, newest first.
pub(crate) fn recent_child_tools<'a>(app: &'a App, task: &TaskItem) -> Vec<&'a ToolCallInfo> {
    let Some(parent) = task.source_tool_call_id.as_deref() else { return Vec::new() };
    let mut children: Vec<_> = app
        .indexed_tool_calls()
        .filter(|(_, tool)| tool.parent_tool_use_id.as_deref() == Some(parent))
        .collect();
    children.sort_unstable_by_key(|(position, _)| std::cmp::Reverse(*position));
    children.into_iter().take(RECENT_CHILD_TOOLS).map(|(_, tool)| tool).collect()
}

fn metadata_str<'a>(task: &'a TaskItem, key: &str) -> Option<&'a str> {
    task.metadata.as_ref()?.as_object()?.get(key)?.as_str()
}

/// Ambient tasks are housekeeping, not activity, and the SDK asks hosts to
/// keep them out of indicators.
fn is_ambient(task: &TaskItem) -> bool {
    task.metadata
        .as_ref()
        .and_then(|metadata| metadata.as_object()?.get("ambient")?.as_bool())
        .unwrap_or(false)
}

fn is_subagent(task: &TaskItem) -> bool {
    metadata_str(task, "task_type") == Some(SUBAGENT_TASK_TYPE) && !is_ambient(task)
}

fn lifecycle_status(task: &TaskItem) -> SubagentStatus {
    match task.status {
        TaskStatus::Pending => SubagentStatus::Pending,
        TaskStatus::InProgress => SubagentStatus::Running,
        TaskStatus::Completed => match metadata_str(task, "terminal_status") {
            Some("failed") => SubagentStatus::Failed,
            Some("killed" | "stopped") => SubagentStatus::Stopped,
            Some("interrupted") => SubagentStatus::Interrupted,
            _ => SubagentStatus::Completed,
        },
    }
}

/// The session's subagent rows in lifecycle order, one per task id.
pub(crate) fn subagent_rows(app: &App) -> Vec<SubagentRow<'_>> {
    app.sdk_inventory
        .tasks
        .iter()
        .filter(|task| is_subagent(task))
        .map(|task| {
            let status = match lifecycle_status(task) {
                SubagentStatus::Running
                    if app
                        .agents
                        .stop_requested
                        .get(&task.task_id)
                        .is_some_and(|stop| stop.run_id == task.run_id) =>
                {
                    SubagentStatus::Stopping
                }
                status => status,
            };
            SubagentRow { task, status }
        })
        .collect()
}

/// Subagents the lifecycle reports as running, including those asked to stop.
pub(crate) fn running_subagent_count(app: &App) -> usize {
    app.sdk_inventory
        .tasks
        .iter()
        .filter(|task| is_subagent(task) && lifecycle_status(task).is_running())
        .count()
}

impl AgentsView {
    /// A saved transcript exists only for an id the SDK listed; nothing is inferred.
    pub(crate) fn has_transcript(&self, task_id: &str) -> Option<bool> {
        self.transcript_ids.as_ref().map(|ids| ids.contains(task_id))
    }

    /// The chosen row among `rows`, if one is chosen and still listed.
    pub(crate) fn selected_index(&self, rows: &[SubagentRow<'_>]) -> Option<usize> {
        let id = self.selected_task_id.as_deref()?;
        rows.iter().position(|row| row.task.task_id == id)
    }

    /// Forget everything tied to the previous session.
    pub(crate) fn clear_session(&mut self) {
        *self = Self::default();
    }
}

fn active_session(
    app: &App,
) -> Option<(String, std::rc::Rc<crate::agent::client::AgentConnection>)> {
    let session_id = app.session_runtime.session_id.as_ref()?.to_string();
    let conn = app.session_runtime.conn.clone()?;
    Some((session_id, conn))
}

fn notice(app: &mut App, kind: NoticeKind, text: impl Into<String>) {
    app.agents.notice = Some(Notice { kind, text: text.into() });
}

/// Opens the view on the first running subagent, else the first one.
pub(crate) fn open(app: &mut App) {
    let rows = subagent_rows(app);
    let first = rows.iter().find(|row| row.status.is_running()).or_else(|| rows.first());
    let selected = first.map(|row| row.task.task_id.clone());
    app.agents.transcript = None;
    app.agents.notice = None;
    app.agents.selected_task_id = selected;
    app.agents.detail_scroll = 0;
    crate::app::view::set_fullscreen_view(app, crate::app::FullscreenView::Agents);
    request_transcript_list(app);
    read_selected_response(app, false);
}

fn close(app: &mut App) {
    app.agents.transcript = None;
    crate::app::view::set_chat_surface(app);
}

fn request_transcript_list(app: &mut App) {
    let Some((session_id, conn)) = active_session(app) else { return };
    match conn.list_subagents(session_id) {
        Ok(()) => app.agents.listing = true,
        Err(error) => app.agents.list_error = Some(error.to_string()),
    }
}

/// Applies the bridge's answer to one of this view's requests. An answer for
/// another session arrives after a switch and describes subagents no longer shown.
pub(crate) fn apply_client_event(app: &mut App, event: ClientEvent) {
    let (ClientEvent::SubagentsListed { session_id, .. }
    | ClientEvent::SubagentMessagesReceived { session_id, .. }
    | ClientEvent::SubagentStopResult { session_id, .. }) = &event
    else {
        tracing::warn!("client event family routed a non-subagent event to the subagent handler");
        return;
    };
    if app
        .session_runtime
        .session_id
        .as_ref()
        .is_none_or(|active| active.to_string() != *session_id)
    {
        return;
    }
    match event {
        ClientEvent::SubagentsListed { agent_ids, error, .. } => {
            apply_transcript_list(app, agent_ids, error);
        }
        ClientEvent::SubagentMessagesReceived { agent_id, request_id, messages, error, .. } => {
            apply_transcript(app, &agent_id, request_id.as_deref(), messages, error);
        }
        ClientEvent::SubagentStopResult { task_id, run_id, request_id, error, .. } => {
            apply_stop_result(app, &task_id, run_id.as_deref(), request_id.as_deref(), error);
        }
        _ => {}
    }
}

pub(crate) fn apply_transcript_list(app: &mut App, agent_ids: Vec<String>, error: Option<String>) {
    app.agents.listing = false;
    if error.is_none() {
        app.agents.transcript_ids = Some(agent_ids.into_iter().collect());
    }
    app.agents.list_error = error;
    read_selected_response(app, false);
}

/// Applies the answer to the latest read request; an answer to an earlier
/// request, for this subagent or another, changes nothing.
fn apply_transcript(
    app: &mut App,
    agent_id: &str,
    request_id: Option<&str>,
    messages: Vec<SubagentMessage>,
    error: Option<String>,
) {
    if request_id.is_some() && app.agents.read_in_flight.as_deref() == request_id {
        app.agents.read_in_flight = None;
        read_selected_response(app, false);
        return;
    }
    let Some(read) =
        app.agents.read.as_mut().filter(|read| {
            read.agent_id == agent_id && Some(read.request_id.as_str()) == request_id
        })
    else {
        return;
    };
    read.pending = false;
    if read.saved_snapshot {
        return;
    }
    match error {
        // A failed re-read keeps what was read before for the next retry.
        Some(error) => read.error = Some(error),
        None => read.messages = Some(messages),
    }
    if let Some(view) = app.agents.transcript.as_mut() {
        view.wrapped = None;
    }
}

fn apply_stop_result(
    app: &mut App,
    task_id: &str,
    run_id: Option<&str>,
    request_id: Option<&str>,
    error: Option<String>,
) {
    if !app.agents.stop_requested.get(task_id).is_some_and(|stop| {
        stop.run_id.as_deref() == run_id && Some(stop.request_id.as_str()) == request_id
    }) {
        return;
    }
    match error {
        Some(error) => {
            app.agents.stop_requested.remove(task_id);
            // A stop that loses the race with the subagent's own outcome is
            // refused as not running; the row already shows how it ended.
            if app
                .sdk_inventory
                .tasks
                .iter()
                .any(|task| task.task_id == task_id && task.status == TaskStatus::InProgress)
            {
                notice(app, NoticeKind::Error, format!("Could not stop the subagent: {error}"));
            }
        }
        // The lifecycle can report the outcome before the acknowledgement.
        None if subagent_rows(app)
            .iter()
            .any(|row| row.task.task_id == task_id && row.status == SubagentStatus::Stopping) =>
        {
            notice(app, NoticeKind::Info, "Stop requested. The status changes when it stops.");
        }
        None => {}
    }
}

fn selected_row(app: &App) -> Option<(String, SubagentStatus)> {
    let rows = subagent_rows(app);
    let row = rows.get(app.agents.selected_index(&rows)?)?;
    Some((row.task.task_id.clone(), row.status))
}

fn move_selection(app: &mut App, down: bool) {
    let rows = subagent_rows(app);
    if rows.is_empty() {
        return;
    }
    let next = match app.agents.selected_index(&rows) {
        None => 0,
        Some(current) if down => (current + 1).min(rows.len() - 1),
        Some(current) => current.saturating_sub(1),
    };
    let id = rows[next].task.task_id.clone();
    if app.agents.selected_task_id.as_deref() != Some(id.as_str()) {
        app.agents.detail_scroll = 0;
    }
    app.agents.selected_task_id = Some(id);
    read_selected_response(app, false);
}

/// Lifecycle rows changed: a selected subagent that completed while the list
/// is shown is read for its response.
pub(crate) fn tasks_changed(app: &mut App) {
    app.agents.stop_requested.retain(|id, stop| {
        app.sdk_inventory.tasks.iter().any(|task| {
            task.task_id == *id
                && task.run_id == stop.run_id
                && task.status == TaskStatus::InProgress
        })
    });
    if let Some(read) = app.agents.read.as_mut() {
        let current_run = app
            .sdk_inventory
            .tasks
            .iter()
            .find(|task| task.task_id == read.agent_id)
            .map(|task| &task.run_id);
        if current_run != Some(&read.run_id) {
            read.saved_snapshot = true;
            read.after_completion = false;
            if read.pending {
                app.agents.read_in_flight = Some(read.request_id.clone());
                read.pending = false;
            }
        }
    }
    read_selected_response(app, false);
}

/// Keeps the read on the selected subagent while the list is shown. A completed
/// one with a saved transcript is read for its response once, after it
/// completed; `again` (the user's `r`) reads it even then, also after a failure.
fn read_selected_response(app: &mut App, again: bool) {
    if app.surface_mode != SurfaceMode::Fullscreen(FullscreenView::Agents)
        || app.agents.transcript.is_some()
    {
        return;
    }
    let Some((task_id, status)) = selected_row(app) else { return };
    if let Some(passed) =
        app.agents.read.take_if(|read| read.agent_id != task_id || read.saved_snapshot)
        && passed.pending
    {
        app.agents.read_in_flight = Some(passed.request_id);
    }
    if status != SubagentStatus::Completed {
        // A subagent resumed after it completed will end with another response.
        if let Some(read) = app.agents.read.as_mut() {
            read.after_completion = false;
        }
        return;
    }
    if app.agents.has_transcript(&task_id) != Some(true) {
        return;
    }
    let held = app.agents.read.as_ref().is_some_and(|read| read.after_completion);
    if again || !(held || app.agents.read_in_flight.is_some()) {
        request_read(app, task_id);
    }
}

/// Asks for `agent_id`'s saved transcript. What was read before for the same
/// subagent stays shown, at the same position, until the answer replaces it,
/// except a read from before it completed once it has: that is not its response.
fn request_read(app: &mut App, agent_id: String) -> bool {
    if app.agents.read_in_flight.is_some() {
        return false;
    }
    let Some((session_id, conn)) = active_session(app) else { return false };
    let request_id = uuid::Uuid::new_v4().to_string();
    let run_id = app
        .sdk_inventory
        .tasks
        .iter()
        .find(|task| task.task_id == agent_id)
        .and_then(|task| task.run_id.clone());
    if let Err(error) = conn.get_subagent_messages(session_id, agent_id.clone(), request_id.clone())
    {
        notice(app, NoticeKind::Error, format!("Could not read the transcript: {error}"));
        return false;
    }
    let after_completion = subagent_rows(app)
        .iter()
        .any(|row| row.task.task_id == agent_id && row.status == SubagentStatus::Completed);
    let messages = app
        .agents
        .read
        .take()
        .filter(|read| {
            read.agent_id == agent_id
                && read.run_id == run_id
                && !read.saved_snapshot
                && (read.after_completion || !after_completion)
        })
        .and_then(|read| read.messages);
    app.agents.read = Some(TranscriptRead {
        agent_id,
        run_id,
        saved_snapshot: false,
        pending: true,
        request_id,
        messages,
        error: None,
        after_completion,
    });
    true
}

fn open_transcript(app: &mut App) {
    let Some((task_id, _)) = selected_row(app) else { return };
    if app.agents.has_transcript(&task_id) != Some(true) {
        notice(
            app,
            NoticeKind::Info,
            "No transcript for this subagent yet. Press r to check again.",
        );
        return;
    }
    // Open the saved conversation already read; persistence can still change.
    // The user explicitly refreshes it with `r`.
    let held = app.agents.read.as_ref().is_some_and(|read| {
        read.agent_id == task_id && read.after_completion && read.error.is_none()
    });
    if held || request_read(app, task_id) {
        app.agents.notice = None;
        app.agents.transcript = Some(TranscriptView::default());
    }
}

/// Reads the open transcript again, also after a failed read.
fn refresh_transcript(app: &mut App) {
    if let Some(agent_id) = app.agents.read.as_ref().map(|read| read.agent_id.clone()) {
        request_read(app, agent_id);
    }
}

fn request_stop(app: &mut App) {
    let Some((task_id, status)) = selected_row(app) else { return };
    match status {
        SubagentStatus::Running => {}
        SubagentStatus::Stopping => return,
        _ => {
            notice(app, NoticeKind::Info, "Only a running subagent can be stopped.");
            return;
        }
    }
    let Some((session_id, conn)) = active_session(app) else { return };
    // Keep the row selected after it stops running.
    app.agents.selected_task_id = Some(task_id.clone());
    let run_id = app
        .sdk_inventory
        .tasks
        .iter()
        .find(|task| task.task_id == task_id)
        .and_then(|task| task.run_id.clone());
    let request_id = uuid::Uuid::new_v4().to_string();
    match conn.stop_subagent(session_id, task_id.clone(), run_id.clone(), request_id.clone()) {
        Ok(()) => {
            app.agents.stop_requested.insert(task_id, PendingStop { run_id, request_id });
            app.agents.notice = None;
        }
        Err(error) => {
            notice(app, NoticeKind::Error, format!("Could not stop the subagent: {error}"));
        }
    }
}

pub(crate) fn handle_key(app: &mut App, key: KeyEvent) {
    if !matches!(key.modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT) {
        return;
    }
    if let Some(transcript) = app.agents.transcript.as_mut() {
        match key.code {
            KeyCode::Esc | KeyCode::Left => {
                app.agents.transcript = None;
                // The subagent may have completed while its transcript was open.
                read_selected_response(app, false);
            }
            KeyCode::Up => transcript.scroll = transcript.scroll.saturating_sub(1),
            KeyCode::Down => transcript.scroll = transcript.scroll.saturating_add(1),
            KeyCode::PageUp => {
                transcript.scroll = transcript.scroll.saturating_sub(usize::from(PAGE_ROWS));
            }
            KeyCode::PageDown => {
                transcript.scroll = transcript.scroll.saturating_add(usize::from(PAGE_ROWS));
            }
            KeyCode::Char('r') => refresh_transcript(app),
            _ => {}
        }
        return;
    }
    match key.code {
        KeyCode::Esc => close(app),
        KeyCode::Up => move_selection(app, false),
        KeyCode::Down => move_selection(app, true),
        KeyCode::PageUp => {
            app.agents.detail_scroll = app.agents.detail_scroll.saturating_sub(PAGE_ROWS);
        }
        KeyCode::PageDown => {
            app.agents.detail_scroll = app.agents.detail_scroll.saturating_add(PAGE_ROWS);
        }
        KeyCode::Enter => open_transcript(app),
        KeyCode::Char('s') => request_stop(app),
        KeyCode::Char('r') => {
            request_transcript_list(app);
            read_selected_response(app, true);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests;
