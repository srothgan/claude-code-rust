// SPDX-License-Identifier: Apache-2.0

//! The `/agents` screen: the active session's subagents, the selected
//! subagent's details, and one saved transcript.

use super::config::common::{
    badge_span, detail_kv, help_style, hint_text_with_escape, position_counter, render_message,
    section_heading, selection_line, selection_style, title_style, wrapped_height,
};
use super::session_picker::fit_with_ellipsis;
use super::theme;
use crate::agent::types::{SubagentMessage, SubagentMessageRole};
use crate::app::agents::{
    NoticeKind, SubagentRow, SubagentStatus, WrappedTranscript, launching_call, recent_child_tools,
    running_subagent_count, saved_response, subagent_rows,
};
use crate::app::{App, ToolCallInfo};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{List, ListItem, ListState, Paragraph, Wrap};

const TITLE: &str = "Subagents";
/// Below this width the details sit under the list instead of beside it.
const SIDE_BY_SIDE_MIN_WIDTH: u16 = 110;
/// Rows the details show of a prompt or a response; the transcript has all of it.
const PREVIEW_LINES: usize = 6;

pub(crate) fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    if area.width < 30 || area.height < 8 {
        frame.render_widget(
            Paragraph::new("Window too small. Resize to view subagents.\nEsc close")
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    let compact = area.height < 16;
    let inner = area.inner(Margin { vertical: u16::from(!compact), horizontal: 2 });
    let notice = notice_lines(app);
    let hint = hint(app, inner.width);
    let notice_height =
        if notice.lines.is_empty() { 0 } else { wrapped_height(notice.clone(), inner.width) };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(u16::from(!compact)),
            Constraint::Min(3),
            Constraint::Length(notice_height),
            Constraint::Length(wrapped_height(hint.as_str(), inner.width).clamp(1, 3)),
        ])
        .split(inner);

    if app.agents.transcript.is_some() {
        render_transcript(frame, chunks[0], chunks[2], app);
    } else {
        app.agents.detail_scroll = render_agents(frame, chunks[0], chunks[2], app);
    }
    frame.render_widget(Paragraph::new(notice).wrap(Wrap { trim: false }), chunks[3]);
    frame.render_widget(
        Paragraph::new(hint).style(help_style()).wrap(Wrap { trim: false }),
        chunks[4],
    );
}

/// The transcript check's state under the list, then the last action's notice.
fn notice_lines(app: &App) -> Text<'static> {
    let mut lines = Vec::new();
    if app.agents.transcript.is_none() {
        if app.agents.listing {
            lines.push(Line::styled(
                "Checking saved transcripts...",
                Style::default().fg(theme::DIM),
            ));
        } else if let Some(error) = &app.agents.list_error {
            lines.push(Line::styled(
                format!("Could not check transcripts: {error}"),
                Style::default().fg(theme::STATUS_ERROR),
            ));
        }
    }
    if let Some(notice) = &app.agents.notice {
        let color = match notice.kind {
            NoticeKind::Error => theme::STATUS_ERROR,
            NoticeKind::Info => theme::DIM,
        };
        lines.push(Line::styled(notice.text.clone(), Style::default().fg(color)));
    }
    Text::from(lines)
}

fn hint(app: &App, width: u16) -> String {
    if app.agents.transcript.is_some() {
        return hint_text_with_escape(
            vec!["Up/Down scroll", "r refresh", "PageUp/PageDown page"],
            width,
            "Esc/Left back",
        );
    }
    hint_text_with_escape(
        vec![
            "Up/Down select",
            "Enter transcript",
            "s stop",
            "r refresh",
            "PageUp/PageDown details",
        ],
        width,
        "Esc close",
    )
}

/// Draws the list and the selected subagent's details; returns the details' clamped scroll.
fn render_agents(frame: &mut Frame, title_area: Rect, body: Rect, app: &App) -> u16 {
    let rows = subagent_rows(app);
    let selected = app.agents.selected_index(&rows);
    let counter = selected.map_or_else(
        || Span::styled(format!("-/{}", rows.len()), Style::default().fg(theme::DIM)),
        |index| position_counter(index, rows.len()),
    );
    let mut title =
        vec![Span::styled(TITLE, title_style().fg(theme::RUST_ORANGE)), Span::raw("  "), counter];
    let running = running_subagent_count(app);
    if running > 0 {
        title.push(Span::styled(format!("  {running} running"), Style::default().fg(theme::DIM)));
    }
    frame.render_widget(Paragraph::new(Line::from(title)), title_area);
    if app.session_runtime.session_id.is_none() {
        render_message(
            frame,
            body,
            "No active session",
            "Start or resume a session to see its subagents.",
        );
        return 0;
    }
    if rows.is_empty() {
        render_message(
            frame,
            body,
            "No subagents yet",
            "Subagents this session starts appear here with their live status.",
        );
        return 0;
    }
    let (list_area, detail_area) = split_body(body, rows.len());
    render_list(frame, list_area, app, &rows, selected);
    let Some(row) = selected.and_then(|index| rows.get(index)) else {
        render_message(frame, detail_area, "No subagent selected", "Up/Down selects a subagent.");
        return 0;
    };

    let details = Text::from(detail_lines(app, row, detail_area.width));
    let total =
        Paragraph::new(details.clone()).wrap(Wrap { trim: false }).line_count(detail_area.width);
    let max_scroll =
        u16::try_from(total.saturating_sub(usize::from(detail_area.height))).unwrap_or(u16::MAX);
    let scroll = app.agents.detail_scroll.min(max_scroll);
    frame.render_widget(
        Paragraph::new(details).wrap(Wrap { trim: false }).scroll((scroll, 0)),
        detail_area,
    );
    scroll
}

/// Beside each other when wide; otherwise the list takes at most a third of
/// the height and the details fill the rest.
fn split_body(body: Rect, row_count: usize) -> (Rect, Rect) {
    if body.width >= SIDE_BY_SIDE_MIN_WIDTH {
        let panes = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(45), Constraint::Length(2), Constraint::Min(0)])
            .split(body);
        return (panes[0], panes[2]);
    }
    let list_rows = u16::try_from(row_count.saturating_mul(2)).unwrap_or(u16::MAX);
    let list_height = list_rows.min((body.height / 3).max(4));
    let panes = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(list_height), Constraint::Length(1), Constraint::Min(0)])
        .split(body);
    (panes[0], panes[2])
}

fn render_list(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    rows: &[SubagentRow<'_>],
    selected: Option<usize>,
) {
    let items = rows.iter().enumerate().map(|(index, row)| {
        let is_selected = Some(index) == selected;
        ListItem::new(row_lines(app, row, is_selected, area.width))
            .style(selection_style(is_selected))
    });
    let mut state = ListState::default().with_selected(selected);
    frame.render_stateful_widget(List::new(items).highlight_symbol(""), area, &mut state);
}

fn status_badge(status: SubagentStatus) -> Span<'static> {
    let (fg, bg) = match status {
        SubagentStatus::Pending => (Color::Black, Color::Cyan),
        SubagentStatus::Running => (Color::Black, theme::RUST_ORANGE),
        SubagentStatus::Stopping => (Color::Black, theme::STATUS_WARNING),
        SubagentStatus::Completed => (Color::Black, Color::White),
        SubagentStatus::Failed => (Color::White, theme::STATUS_ERROR),
        SubagentStatus::Stopped | SubagentStatus::Interrupted => (Color::White, Color::DarkGray),
    };
    badge_span(status.label(), fg, bg)
}

/// One list row: the title, shortened so its status badge stays visible, then
/// the subagent type and transcript state.
fn row_lines(app: &App, row: &SubagentRow<'_>, selected: bool, width: u16) -> Vec<Line<'static>> {
    let mut details = Vec::new();
    if let Some(kind) = row.subagent_type() {
        details.push(kind.to_owned());
    }
    if let Some(parent) = known_parent(app, row) {
        details.push(format!("from {}", parent.subject));
    }
    match app.agents.has_transcript(&row.task.task_id) {
        Some(true) => details.push("transcript".to_owned()),
        Some(false) => details.push("no transcript".to_owned()),
        None => {}
    }
    let badge = status_badge(row.status);
    // Selection marker and its space, then two spaces before the badge.
    let title_width = usize::from(width).saturating_sub(4 + badge.width());
    let mut lines = vec![selection_line(
        Line::from(vec![
            Span::styled(fit_with_ellipsis(&row.task.subject, title_width), title_style()),
            Span::raw("  "),
            badge,
        ]),
        selected,
    )];
    if !details.is_empty() {
        lines.push(Line::styled(
            format!("  {}", details.join(" · ")),
            Style::default().fg(theme::DIM),
        ));
    }
    lines
}

/// What one tool call is doing, on one line.
fn tool_activity(tool: &ToolCallInfo) -> String {
    let title = tool.title.lines().next().unwrap_or_default().trim();
    if title.is_empty() { tool.sdk_tool_name.clone() } else { title.to_owned() }
}

/// Model provenance from this run's invocation, inheritance or agent definition.
fn model_choice(app: &App, row: &SubagentRow<'_>) -> Option<String> {
    let tool = launching_call(app, row.task);
    let current_invocation = row.task.source_tool_call_run_id == row.task.run_id;
    let default = row
        .subagent_type()
        .and_then(|kind| app.sdk_inventory.available_agents.iter().find(|agent| agent.name == kind))
        .and_then(|agent| agent.model.as_deref());
    super::tool_call::agent_model_choice(
        tool.filter(|_| current_invocation),
        row.subagent_type(),
        default,
    )
    .map(|(model, source)| format!("{model} ({source})"))
}

fn known_parent<'a>(
    app: &'a App,
    row: &SubagentRow<'_>,
) -> Option<&'a crate::agent::model::TaskItem> {
    let id = row.task.parent_task_id.as_deref()?;
    app.sdk_inventory.tasks.iter().find(|task| task.task_id == id)
}

fn transcript_state(app: &App, task_id: &str) -> &'static str {
    match app.agents.has_transcript(task_id) {
        Some(true) => "saved, Enter to read",
        Some(false) => "not saved yet, r to check again",
        None if app.agents.listing => "checking...",
        None => "unknown, r to check",
    }
}

/// The selected subagent: identity and model choice, transcript, activity and prompt.
fn detail_lines(app: &App, row: &SubagentRow<'_>, width: u16) -> Vec<Line<'static>> {
    let value = Color::White;
    let mut lines = vec![
        Line::from(vec![
            Span::styled(row.task.subject.clone(), title_style()),
            Span::raw("  "),
            status_badge(row.status),
        ]),
        Line::default(),
    ];
    if let Some(kind) = row.subagent_type() {
        lines.push(detail_kv("Type", kind, value));
    }
    if let Some(model) = model_choice(app, row) {
        lines.push(detail_kv("Model", &model, value));
    }
    if let Some(parent) = known_parent(app, row) {
        let kind =
            parent.metadata.as_ref().and_then(|metadata| metadata.get("subagent_type")?.as_str());
        let label = kind
            .map_or_else(|| parent.subject.clone(), |kind| format!("{} ({kind})", parent.subject));
        lines.push(detail_kv("Parent", &label, value));
    }
    if let Some(run) = &row.task.run_id {
        lines.push(detail_kv("Run", run, value));
    }
    lines.push(detail_kv("Transcript", transcript_state(app, &row.task.task_id), value));

    // One row per tool call; the transcript has the full calls.
    let one_row = |text: String, indent: usize| {
        fit_with_ellipsis(&text, usize::from(width).saturating_sub(indent))
    };
    let mut children = recent_child_tools(app, row.task).into_iter().map(tool_activity);
    if row.status.is_running() {
        let now = children.next().or_else(|| row.summary().map(str::to_owned));
        if let Some(now) = now {
            lines.push(detail_kv("Now", &one_row(now, "Now: ".len()), value));
        }
    } else if let Some(outcome) = row.failure().or_else(|| row.summary()) {
        // An interrupted row has no outcome; its text is the last progress.
        let label =
            if row.status == SubagentStatus::Interrupted { "Last progress" } else { "Result" };
        lines.push(detail_kv(label, outcome, value));
    }
    let recent: Vec<_> = children.collect();
    if !recent.is_empty() {
        lines.push(Line::default());
        lines.push(section_heading("Recent"));
        lines.extend(
            recent.into_iter().map(|activity| Line::from(format!("  {}", one_row(activity, 2)))),
        );
    }

    let prompt = launching_call(app, row.task)
        .and_then(|tool| tool.raw_input.as_ref()?.get("prompt")?.as_str())
        .or_else(|| row.prompt());
    if let Some(prompt) = prompt {
        lines.push(Line::default());
        lines.push(section_heading("Prompt"));
        lines.extend(preview_lines(prompt, width));
    }
    if let Some(response) = saved_response(app, row) {
        lines.push(Line::default());
        lines.push(section_heading("Saved response"));
        match response {
            Ok(response) => lines.extend(preview_lines(response, width)),
            Err(error) => lines.push(Line::styled(
                format!("Could not read the transcript: {error}. Press r to try again."),
                Style::default().fg(theme::STATUS_ERROR),
            )),
        }
    }
    lines
}

/// The first `PREVIEW_LINES` rows of `text` wrapped to `width`, then `...` when
/// it goes on.
fn preview_lines(text: &str, width: u16) -> Vec<Line<'static>> {
    let width = usize::from(width.max(1));
    // A row holds at most `width` characters, so this head fills more rows than
    // are shown even where wrapping drops spaces: a long response costs a frame
    // no more than a short one.
    let head_end =
        text.char_indices().nth((PREVIEW_LINES + 1) * width * 2).map_or(text.len(), |(end, _)| end);
    let rows = crate::ui::wrap::wrap_plain(&text[..head_end], width);
    let mut lines: Vec<_> =
        rows.iter().take(PREVIEW_LINES).map(|row| Line::from(row.clone())).collect();
    if rows.len() > PREVIEW_LINES || head_end < text.len() {
        lines.push(Line::styled("...", Style::default().fg(theme::DIM)));
    }
    lines
}

fn render_transcript(frame: &mut Frame, title_area: Rect, body: Rect, app: &mut App) {
    let agents = &mut *app.agents;
    let (Some(read), Some(view)) = (agents.read.as_ref(), agents.transcript.as_mut()) else {
        return;
    };
    let subject = app
        .sdk_inventory
        .tasks
        .iter()
        .find(|task| task.task_id == read.agent_id)
        .map_or_else(|| read.agent_id.clone(), |task| task.subject.clone());
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(TITLE, title_style().fg(theme::RUST_ORANGE)),
            Span::styled(" › ", Style::default().fg(theme::DIM)),
            Span::styled(
                if read.saved_snapshot { format!("{subject} (saved snapshot)") } else { subject },
                title_style(),
            ),
        ])),
        title_area,
    );
    if let Some(error) = &read.error {
        render_message(frame, body, "Could not read the transcript", error);
        return;
    }
    let Some(messages) = &read.messages else {
        if read.saved_snapshot {
            render_message(
                frame,
                body,
                "No saved snapshot",
                "The execution changed before the read completed. Press r to read the saved conversation.",
            );
            return;
        }
        render_message(
            frame,
            body,
            "Reading transcript",
            "Waiting for the saved subagent transcript.",
        );
        return;
    };
    if messages.is_empty() {
        render_message(
            frame,
            body,
            "Empty transcript",
            "The saved transcript has no messages yet.",
        );
        return;
    }
    let width = body.width.max(1);
    if view.wrapped.as_ref().is_none_or(|cached| cached.width != width) {
        // One logical line at a time: the helper caps its result at u16::MAX
        // rows, which a whole long transcript can exceed but one line cannot.
        let rows = transcript_lines(messages, width)
            .into_iter()
            .flat_map(|line| crate::ui::wrap::wrap_lines_to_physical_rows(&[line], width))
            .collect();
        view.wrapped = Some(WrappedTranscript { width, rows });
    }
    let Some(wrapped) = &view.wrapped else { return };
    // A re-read keeps the position; one that shrank the transcript ends at its last page.
    let max_scroll = wrapped.rows.len().saturating_sub(usize::from(body.height));
    view.scroll = view.scroll.min(max_scroll);
    let visible: Vec<Line<'static>> =
        wrapped.rows.iter().skip(view.scroll).take(usize::from(body.height)).cloned().collect();
    frame.render_widget(Paragraph::new(visible), body);
}

/// Prompts and SDK reminders as written and replies through the chat's
/// Markdown rendering, each under its label; tool calls as one dim line each.
fn transcript_lines(messages: &[SubagentMessage], width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for message in messages {
        let (label, body) = match message.role {
            SubagentMessageRole::Tool => {
                lines.push(Line::styled(
                    format!("• {}", message.text),
                    Style::default().fg(theme::DIM),
                ));
                continue;
            }
            SubagentMessageRole::User => (
                Line::styled("Prompt", help_style()),
                message.text.lines().map(|line| Line::from(line.to_owned())).collect(),
            ),
            SubagentMessageRole::SystemReminder => (
                Line::styled("System reminder", Style::default().fg(theme::DIM)),
                message
                    .text
                    .lines()
                    .map(|line| Line::styled(line.to_owned(), Style::default().fg(theme::DIM)))
                    .collect(),
            ),
            SubagentMessageRole::Assistant => (
                Line::styled("Reply", title_style()),
                super::markdown::render_markdown_at_width(&message.text, width, None, false),
            ),
        };
        lines.push(label);
        lines.extend(body);
        lines.push(Line::default());
    }
    lines
}

#[cfg(test)]
mod tests;
