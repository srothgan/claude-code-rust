// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang
use crate::app::{
    App, AppStatus, BtwExchangeBlock, ChatMessage, ChatMessageId, HistoryOutputId, MessageBlock,
    MessageRole, NoticeBlock, SystemSeverity, TextBlock, TextBlockSpacing, ToolCallInfo,
    WelcomeBlock, markdown_table_tail_is_open, starts_with_markdown_table_row,
};
#[cfg(test)]
pub(crate) use crate::ui::live_rows::segments::LiveRowSegment;
use crate::ui::live_rows::segments::{
    LiveRowBoundary, ids_are_excluded, live_boundaries_to_segments,
};
pub(crate) use crate::ui::live_rows::segments::{LiveRowBoundaryKind, SerializedLiveRows};
use crate::ui::message::{MessageRenderContext, SpinnerState, render_text_block_cached};
use crate::ui::message_rows::{
    MessageRowSegment, build_user_system_message_rows, render_btw_exchange_lines,
};
use crate::ui::theme;
use crate::ui::tool_call;
use crate::ui::welcome;
use crate::ui::wrap::{wrap_lines_to_physical_rows, wrap_markdown_lines_to_physical_rows};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TopLevelInlineBlockKind {
    Welcome,
    User,
    System,
    Assistant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AssistantInlineItemKind {
    TextLike,
    Tool,
}

pub(crate) fn serialize_live_rows_with_boundaries_excluding(
    app: &mut App,
    width: u16,
    excluded_ids: &BTreeSet<HistoryOutputId>,
) -> SerializedLiveRows {
    let current_mode_id =
        app.session_runtime.mode.as_ref().map(|mode| mode.current_mode_id.clone());
    let mut rows = Vec::new();
    let mut row_boundaries = Vec::new();
    let mut previous_block_kind = None;

    for msg_idx in 0..app.transcript.messages.len() {
        let role = app.transcript.messages[msg_idx].role.clone();
        let block_kind = message_block_kind(&role);
        let rendered_message = render_live_message_rows(
            app,
            msg_idx,
            &role,
            LiveRowsRenderContext {
                current_mode_id: current_mode_id.as_deref(),
                width,
                excluded_ids,
                previous_block_kind,
            },
        );

        append_rendered_live_message(
            &mut rows,
            &mut row_boundaries,
            &mut previous_block_kind,
            block_kind,
            rendered_message,
        );
    }

    append_detached_tool_interactions(
        app,
        width,
        current_mode_id.as_deref(),
        &mut rows,
        &mut row_boundaries,
    );
    let segments = live_boundaries_to_segments(row_boundaries, rows.len());
    SerializedLiveRows::new(rows, segments)
}

#[derive(Clone, Copy)]
struct LiveRowsRenderContext<'a> {
    current_mode_id: Option<&'a str>,
    width: u16,
    excluded_ids: &'a BTreeSet<HistoryOutputId>,
    previous_block_kind: Option<TopLevelInlineBlockKind>,
}

fn render_live_message_rows(
    app: &mut App,
    msg_idx: usize,
    role: &MessageRole,
    context: LiveRowsRenderContext<'_>,
) -> RenderedMessageRows {
    match role {
        MessageRole::Welcome => {
            render_welcome_live_rows(app, msg_idx, context.width, context.excluded_ids)
        }
        MessageRole::User | MessageRole::System(_) => render_user_system_live_rows(
            app,
            msg_idx,
            context.current_mode_id,
            context.width,
            context.excluded_ids,
        ),
        MessageRole::Assistant => render_assistant_live_rows(
            app,
            msg_idx,
            context.current_mode_id,
            context.width,
            context.excluded_ids,
            context.previous_block_kind,
        ),
    }
}

fn render_welcome_live_rows(
    app: &App,
    msg_idx: usize,
    width: u16,
    excluded_ids: &BTreeSet<HistoryOutputId>,
) -> RenderedMessageRows {
    let ids = welcome_output_ids(&app.transcript.messages[msg_idx]);
    let commit_ready = message_commit_ready(&app.transcript.messages[msg_idx]);
    if ids_are_excluded(&ids, excluded_ids) {
        return RenderedMessageRows::skipped_transcript_content();
    }

    RenderedMessageRows::message(
        serialize_welcome_message(app, msg_idx, width),
        LiveRowBoundary {
            ids,
            msg_idx,
            block_idx: None,
            kind: LiveRowBoundaryKind::Message,
            start_row: 0,
            commit_ready,
        },
    )
}

fn render_user_system_live_rows(
    app: &mut App,
    msg_idx: usize,
    current_mode_id: Option<&str>,
    width: u16,
    excluded_ids: &BTreeSet<HistoryOutputId>,
) -> RenderedMessageRows {
    let ids = vec![HistoryOutputId::Message(app.transcript.messages[msg_idx].id)];
    if ids_are_excluded(&ids, excluded_ids) {
        return RenderedMessageRows::skipped_transcript_content();
    }

    // A message hosting an unanswered user dialog must stay in the live
    // (mutable) region so its chooser re-renders on focus/selection changes;
    // committing it would freeze it in immutable scrollback.
    let commit_ready = !message_has_pending_user_dialog(&app.transcript.messages[msg_idx])
        && (!app.config.show_message_timestamps_effective()
            || app.transcript.messages[msg_idx]
                .timing
                .timestamp
                .is_none_or(|timestamp| !timestamp.observed)
            || !matches!(app.status, AppStatus::Thinking | AppStatus::Running));
    let timestamp =
        crate::app::presentation::timestamp_label(app, &app.transcript.messages[msg_idx]);
    let mut rendered = build_user_system_message_rows(
        &mut app.transcript.messages[msg_idx],
        message_render_context(current_mode_id, width),
    );
    if let Some(timestamp) = timestamp
        && let Some(MessageRowSegment::Lines { lines }) = rendered.segments.first_mut()
        && let Some(label) = lines.first_mut()
    {
        label.spans.push(Span::styled(format!(" · {timestamp}"), Style::default().fg(theme::DIM)));
    }
    RenderedMessageRows::message(
        segments_to_physical_rows(&rendered.segments, width, false),
        LiveRowBoundary {
            ids,
            msg_idx,
            block_idx: None,
            kind: LiveRowBoundaryKind::Message,
            start_row: 0,
            commit_ready,
        },
    )
}

fn message_has_pending_user_dialog(message: &ChatMessage) -> bool {
    message
        .blocks
        .iter()
        .any(|block| matches!(block, MessageBlock::UserDialog(dialog) if dialog.outcome.is_none()))
}

fn render_assistant_live_rows(
    app: &mut App,
    msg_idx: usize,
    current_mode_id: Option<&str>,
    width: u16,
    excluded_ids: &BTreeSet<HistoryOutputId>,
    previous_block_kind: Option<TopLevelInlineBlockKind>,
) -> RenderedMessageRows {
    let active_mutable = active_assistant_message_is_mutable(app, msg_idx);
    let message_id = app.transcript.messages[msg_idx].id;
    let presentation = app.activity_presentation(std::time::Instant::now());
    let owns_activity = presentation.as_ref().is_some_and(|presentation| {
        presentation.heading == crate::app::activity::ActivityHeading::Assistant(msg_idx)
    });
    // The traversal also remembers excluded static content and skips invisible messages.
    // Message ownership and output IDs remain independent of visual speaker grouping.
    let label_visibility = if previous_block_kind == Some(TopLevelInlineBlockKind::Assistant)
        || excluded_ids.contains(&HistoryOutputId::AssistantLabel(message_id))
    {
        AssistantLabelVisibility::Hidden
    } else if owns_activity {
        AssistantLabelVisibility::WithActivity
    } else {
        AssistantLabelVisibility::WithContent
    };
    let show_thinking = presentation.as_ref().is_some_and(|presentation| {
        presentation.thinking
            && presentation.heading == crate::app::activity::ActivityHeading::Assistant(msg_idx)
    });
    let items = assistant_render_items_from_message(
        &app.transcript.messages[msg_idx],
        msg_idx,
        active_mutable,
    );
    let selection = select_unexcluded_assistant_items(items, excluded_ids);
    let duration_pending =
        crate::app::presentation::duration_label(app, &app.transcript.messages[msg_idx]).is_some()
            && !excluded_ids
                .contains(&HistoryOutputId::AssistantDuration(app.transcript.messages[msg_idx].id));
    if selection.items.is_empty()
        && !duration_pending
        && !show_thinking
        && label_visibility != AssistantLabelVisibility::WithActivity
    {
        return if selection.had_body_content {
            RenderedMessageRows::skipped_transcript_content()
        } else {
            RenderedMessageRows::empty()
        };
    }

    let skipped_static_body = selection.skipped_body_before_rendered_content;
    let spinner = SpinnerState::for_app(app);
    let mut rendered = render_assistant_rows(AssistantRowsRequest {
        app: Some(app),
        message_id,
        msg_idx,
        items: selection.items,
        current_mode_id,
        width,
        spinner,
        show_thinking,
        label_visibility,
        leading_blank_lines: 0,
        has_prior_assistant_content: skipped_static_body,
    });
    if let Some(label) =
        crate::app::presentation::duration_label(app, &app.transcript.messages[msg_idx])
    {
        let ids = vec![HistoryOutputId::AssistantDuration(message_id)];
        if !ids_are_excluded(&ids, excluded_ids) {
            rendered.had_transcript_content = true;
            rendered.boundaries.push(LiveRowBoundary {
                ids,
                msg_idx,
                block_idx: None,
                kind: LiveRowBoundaryKind::AssistantDuration,
                start_row: rendered.rows.len(),
                commit_ready: !active_mutable,
            });
            rendered.rows.extend(wrap_lines_to_physical_rows(
                &[Line::from(Span::styled(label, Style::default().fg(theme::DIM)))],
                width,
            ));
        }
    }
    tracing::debug!(
        target: crate::logging::targets::APP_RENDER,
        event_name = "inline_chat_assistant_block_built",
        message = "assistant message block rendered from canonical app.transcript.messages",
        outcome = "success",
        assistant_turn_id = tracing::field::Empty,
        show_label = label_visibility != AssistantLabelVisibility::Hidden,
        leading_blank_lines = 0,
        skipped_static_body,
        committed_rendered_rows = rendered.rows.len(),
        live_rendered_rows = 0,
        preview = %preview_rows(&rendered.rows, 4),
    );
    rendered
}

fn append_rendered_live_message(
    rows: &mut Vec<Line<'static>>,
    row_boundaries: &mut Vec<LiveRowBoundary>,
    previous_block_kind: &mut Option<TopLevelInlineBlockKind>,
    block_kind: TopLevelInlineBlockKind,
    rendered_message: RenderedMessageRows,
) {
    if !rendered_message.had_transcript_content {
        return;
    }

    if rendered_message.rows.is_empty() {
        *previous_block_kind = Some(block_kind);
        return;
    }

    let start_row = rows.len();
    rows.extend(
        std::iter::repeat_with(Line::default)
            .take(top_level_leading_blank_lines(*previous_block_kind, block_kind)),
    );
    row_boundaries.extend(
        rendered_message.boundaries.into_iter().map(|boundary| boundary.shifted(start_row)),
    );
    rows.extend(rendered_message.rows);
    *previous_block_kind = Some(block_kind);
}

struct AssistantRenderSelection {
    items: Vec<AssistantRenderItemSpec>,
    skipped_body_before_rendered_content: bool,
    had_body_content: bool,
}

fn select_unexcluded_assistant_items(
    items: Vec<AssistantRenderItemSpec>,
    excluded_ids: &BTreeSet<HistoryOutputId>,
) -> AssistantRenderSelection {
    let mut selected = Vec::with_capacity(items.len());
    let mut skipped_body_before_rendered_content = false;
    let mut had_body_content = false;
    let mut rendered_body_seen = false;

    for item in items {
        had_body_content = true;
        if ids_are_excluded(&item.ids, excluded_ids) {
            if !rendered_body_seen {
                skipped_body_before_rendered_content = true;
            }
            continue;
        }

        rendered_body_seen = true;
        selected.push(item);
    }

    AssistantRenderSelection {
        items: selected,
        skipped_body_before_rendered_content,
        had_body_content,
    }
}

fn welcome_output_ids(message: &ChatMessage) -> Vec<HistoryOutputId> {
    message
        .blocks
        .iter()
        .find_map(|block| match block {
            MessageBlock::Welcome(welcome) => Some(vec![HistoryOutputId::Block(welcome.id)]),
            MessageBlock::Text(_)
            | MessageBlock::BtwExchange(_)
            | MessageBlock::Notice(_)
            | MessageBlock::ToolCall(_)
            | MessageBlock::ToolResult { .. }
            | MessageBlock::ImageAttachment(_)
            | MessageBlock::UserDialog(_) => None,
        })
        .unwrap_or_else(|| vec![HistoryOutputId::Message(message.id)])
}

fn serialize_welcome_message(app: &App, msg_idx: usize, width: u16) -> Vec<Line<'static>> {
    if !app.show_session_overview {
        return Vec::new();
    }
    let Some(message) = app.transcript.messages.get(msg_idx) else {
        return Vec::new();
    };
    let Some(MessageBlock::Welcome(welcome)) =
        message.blocks.iter().find(|block| matches!(*block, MessageBlock::Welcome(_)))
    else {
        return Vec::new();
    };

    serialize_compact_welcome_entry(app, welcome, width)
}

const fn message_block_kind(role: &MessageRole) -> TopLevelInlineBlockKind {
    match role {
        MessageRole::Welcome => TopLevelInlineBlockKind::Welcome,
        MessageRole::User => TopLevelInlineBlockKind::User,
        MessageRole::System(_) => TopLevelInlineBlockKind::System,
        MessageRole::Assistant => TopLevelInlineBlockKind::Assistant,
    }
}

fn message_commit_ready(message: &ChatMessage) -> bool {
    match &message.role {
        MessageRole::Welcome => welcome_message_commit_ready(message),
        MessageRole::User | MessageRole::System(_) | MessageRole::Assistant => true,
    }
}

fn welcome_message_commit_ready(message: &ChatMessage) -> bool {
    message
        .blocks
        .iter()
        .find_map(|block| match block {
            MessageBlock::Welcome(welcome) => Some(welcome),
            MessageBlock::Text(_)
            | MessageBlock::BtwExchange(_)
            | MessageBlock::Notice(_)
            | MessageBlock::ToolCall(_)
            | MessageBlock::ToolResult { .. }
            | MessageBlock::ImageAttachment(_)
            | MessageBlock::UserDialog(_) => None,
        })
        .is_some_and(|welcome| {
            welcome_value_ready(&welcome.subscription) && welcome_value_ready(&welcome.session_id)
        })
}

fn welcome_value_ready(value: &str) -> bool {
    !value.trim().is_empty() && value != "-"
}

fn serialize_compact_welcome_entry(
    app: &App,
    entry: &WelcomeBlock,
    width: u16,
) -> Vec<Line<'static>> {
    let heading = Line::from(Span::styled(
        "Overview",
        Style::default().fg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD),
    ));
    let mut lines = wrap_lines_to_physical_rows(&[heading], width);
    lines.extend(welcome::overview_lines(entry, Some(status_label(app)), width));
    lines
}

fn status_label(app: &App) -> &'static str {
    match app.status {
        crate::app::AppStatus::Ready => "Ready",
        crate::app::AppStatus::Connecting => "Connecting",
        crate::app::AppStatus::CommandPending => "Working",
        crate::app::AppStatus::Thinking => "Thinking",
        crate::app::AppStatus::Running => "Running",
        crate::app::AppStatus::Error => "Error",
    }
}

const fn top_level_leading_blank_lines(
    previous: Option<TopLevelInlineBlockKind>,
    next: TopLevelInlineBlockKind,
) -> usize {
    match (previous, next) {
        (Some(TopLevelInlineBlockKind::Welcome), _) => 1,
        _ => 0,
    }
}

enum AssistantRenderItem {
    Text(TextBlock),
    Notice(NoticeBlock),
    CanonicalBtw { msg_idx: usize, block_idx: usize },
    CanonicalTool { msg_idx: usize, block_idx: usize },
}

struct AssistantRenderItemSpec {
    ids: Vec<HistoryOutputId>,
    msg_idx: usize,
    leading_blank_lines: usize,
    block_idx: Option<usize>,
    boundary_kind: LiveRowBoundaryKind,
    commit_ready: bool,
    item: AssistantRenderItem,
}

struct RenderedMessageRows {
    rows: Vec<Line<'static>>,
    boundaries: Vec<LiveRowBoundary>,
    had_transcript_content: bool,
}

impl RenderedMessageRows {
    fn empty() -> Self {
        Self { rows: Vec::new(), boundaries: Vec::new(), had_transcript_content: false }
    }

    fn message(rows: Vec<Line<'static>>, boundary: LiveRowBoundary) -> Self {
        let had_transcript_content = !rows.is_empty();
        let boundaries = if had_transcript_content { vec![boundary] } else { Vec::new() };
        Self { rows, boundaries, had_transcript_content }
    }

    fn rendered(rows: Vec<Line<'static>>, boundaries: Vec<LiveRowBoundary>) -> Self {
        let had_transcript_content = !rows.is_empty();
        Self { rows, boundaries, had_transcript_content }
    }

    fn skipped_transcript_content() -> Self {
        Self { rows: Vec::new(), boundaries: Vec::new(), had_transcript_content: true }
    }
}

fn active_assistant_message_is_mutable(app: &App, msg_idx: usize) -> bool {
    app.active_turn_assistant_idx() == Some(msg_idx)
        && (app.turn.compaction.is_active()
            || matches!(app.status, AppStatus::Thinking | AppStatus::Running))
}

struct PendingAssistantTextRun {
    ids: Vec<HistoryOutputId>,
    msg_idx: usize,
    leading_blank_lines: usize,
    block_idx: usize,
    text: String,
    trailing_spacing: TextBlockSpacing,
    commit_ready: bool,
}

impl PendingAssistantTextRun {
    fn new(
        id: HistoryOutputId,
        msg_idx: usize,
        leading_blank_lines: usize,
        block_idx: usize,
        text: &str,
        trailing_spacing: TextBlockSpacing,
        commit_ready: bool,
    ) -> Self {
        Self {
            ids: vec![id],
            msg_idx,
            leading_blank_lines,
            block_idx,
            text: text.to_owned(),
            trailing_spacing,
            commit_ready,
        }
    }

    const fn can_merge(&self, commit_ready: bool) -> bool {
        !self.commit_ready && !commit_ready
    }

    fn append(
        &mut self,
        id: HistoryOutputId,
        text: &str,
        trailing_spacing: TextBlockSpacing,
        commit_ready: bool,
    ) {
        self.ids.push(id);
        self.trailing_spacing.append_source(&mut self.text, text);
        self.trailing_spacing = trailing_spacing;
        self.commit_ready = commit_ready;
    }

    fn into_render_item(self) -> AssistantRenderItemSpec {
        AssistantRenderItemSpec {
            ids: self.ids,
            msg_idx: self.msg_idx,
            leading_blank_lines: self.leading_blank_lines,
            block_idx: Some(self.block_idx),
            boundary_kind: LiveRowBoundaryKind::AssistantText,
            commit_ready: self.commit_ready,
            item: AssistantRenderItem::Text(
                TextBlock::from_complete(&self.text).with_trailing_spacing(self.trailing_spacing),
            ),
        }
    }
}

#[derive(Default)]
struct AssistantInlineLayoutState {
    has_body_content: bool,
    has_visible_content: bool,
}

fn assistant_render_items_from_message(
    message: &ChatMessage,
    msg_idx: usize,
    active_mutable: bool,
) -> Vec<AssistantRenderItemSpec> {
    let mut items = Vec::with_capacity(message.blocks.len());
    let mut pending_text: Option<PendingAssistantTextRun> = None;
    let mut previous_kind = None;
    let active_tail_block_idx =
        active_mutable.then(|| last_visible_assistant_block_idx(message)).flatten();

    for (block_idx, block) in message.blocks.iter().enumerate() {
        match block {
            MessageBlock::Text(text) => {
                if text.text.is_empty() {
                    continue;
                }
                let commit_ready =
                    assistant_text_block_commit_ready(message, block_idx, active_tail_block_idx);
                if let Some(pending) = pending_text.as_mut()
                    && pending.can_merge(commit_ready)
                {
                    let merged_commit_ready = assistant_text_run_commit_ready(
                        message,
                        pending.block_idx,
                        block_idx,
                        active_tail_block_idx,
                    );
                    pending.append(
                        HistoryOutputId::Block(text.id),
                        &text.text,
                        text.trailing_spacing,
                        merged_commit_ready,
                    );
                } else {
                    flush_pending_text_run(&mut pending_text, &mut items);
                    let current_kind = AssistantInlineItemKind::TextLike;
                    let leading_blank_lines =
                        leading_blank_lines_between(previous_kind, current_kind);
                    pending_text = Some(PendingAssistantTextRun::new(
                        HistoryOutputId::Block(text.id),
                        msg_idx,
                        leading_blank_lines,
                        block_idx,
                        &text.text,
                        text.trailing_spacing,
                        commit_ready,
                    ));
                    previous_kind = Some(current_kind);
                }
            }
            MessageBlock::Notice(notice) => {
                flush_pending_text_run(&mut pending_text, &mut items);
                let current_kind = AssistantInlineItemKind::TextLike;
                let leading_blank_lines = leading_blank_lines_between(previous_kind, current_kind);
                items.push(notice_render_item(
                    notice,
                    msg_idx,
                    block_idx,
                    leading_blank_lines,
                    active_tail_block_idx != Some(block_idx),
                ));
                previous_kind = Some(current_kind);
            }
            MessageBlock::BtwExchange(exchange) => {
                flush_pending_text_run(&mut pending_text, &mut items);
                items.push(btw_render_item(
                    exchange,
                    msg_idx,
                    block_idx,
                    leading_blank_lines_between(previous_kind, AssistantInlineItemKind::TextLike),
                ));
                previous_kind = Some(AssistantInlineItemKind::TextLike);
            }
            MessageBlock::ToolCall(tool) | MessageBlock::ToolResult { tool, .. } => {
                if tool.display().hidden_unless_focused_interaction() {
                    continue;
                }
                flush_pending_text_run(&mut pending_text, &mut items);
                let current_kind = AssistantInlineItemKind::Tool;
                let (id, commit_ready) = if let MessageBlock::ToolResult { id, .. } = block {
                    (HistoryOutputId::Block(*id), true)
                } else {
                    (HistoryOutputId::ToolCall(tool.id.clone()), tool_call_commit_ready(tool))
                };
                items.push(canonical_tool_render_item(
                    id,
                    msg_idx,
                    block_idx,
                    leading_blank_lines_between(previous_kind, current_kind),
                    commit_ready,
                ));
                previous_kind = Some(current_kind);
            }
            MessageBlock::Welcome(_)
            | MessageBlock::ImageAttachment(_)
            | MessageBlock::UserDialog(_) => {}
        }
    }

    flush_pending_text_run(&mut pending_text, &mut items);
    items
}

fn notice_render_item(
    notice: &NoticeBlock,
    msg_idx: usize,
    block_idx: usize,
    leading_blank_lines: usize,
    commit_ready: bool,
) -> AssistantRenderItemSpec {
    AssistantRenderItemSpec {
        ids: vec![HistoryOutputId::Block(notice.id)],
        msg_idx,
        leading_blank_lines,
        block_idx: Some(block_idx),
        boundary_kind: LiveRowBoundaryKind::AssistantNotice,
        commit_ready,
        item: AssistantRenderItem::Notice(NoticeBlock {
            id: notice.id,
            severity: notice.severity,
            text: TextBlock::from_complete(&notice.text.text)
                .with_trailing_spacing(notice.text.trailing_spacing),
            dedup_key: notice.dedup_key.clone(),
        }),
    }
}

fn canonical_tool_render_item(
    id: HistoryOutputId,
    msg_idx: usize,
    block_idx: usize,
    leading_blank_lines: usize,
    commit_ready: bool,
) -> AssistantRenderItemSpec {
    AssistantRenderItemSpec {
        ids: vec![id],
        msg_idx,
        leading_blank_lines,
        block_idx: Some(block_idx),
        boundary_kind: LiveRowBoundaryKind::AssistantTool,
        commit_ready,
        item: AssistantRenderItem::CanonicalTool { msg_idx, block_idx },
    }
}

/// A frozen launch keeps its creation position; live controls follow current output.
fn append_detached_tool_interactions(
    app: &mut App,
    width: u16,
    current_mode_id: Option<&str>,
    rows: &mut Vec<Line<'static>>,
    boundaries: &mut Vec<LiveRowBoundary>,
) {
    let interactions: Vec<_> = app
        .pending_interaction_ids
        .iter()
        .filter_map(|id| {
            let (mi, bi) = app.lookup_tool_call(id)?;
            match &app.transcript.messages[mi].blocks[bi] {
                MessageBlock::ToolCall(tool) if tool.has_frozen_launch() => {
                    Some((id.clone(), mi, bi))
                }
                _ => None,
            }
        })
        .collect();
    let spinner = SpinnerState::for_app(app);
    for (id, msg_idx, block_idx) in interactions {
        boundaries.push(LiveRowBoundary {
            ids: vec![HistoryOutputId::ToolInteraction(id)],
            msg_idx,
            block_idx: Some(block_idx),
            kind: LiveRowBoundaryKind::AssistantTool,
            start_row: rows.len(),
            commit_ready: false,
        });
        rows.push(Line::default());
        rows.extend(render_canonical_tool_rows(
            app,
            msg_idx,
            block_idx,
            message_render_context(current_mode_id, width),
            spinner,
            true,
        ));
    }
}

fn btw_render_item(
    exchange: &BtwExchangeBlock,
    msg_idx: usize,
    block_idx: usize,
    leading_blank_lines: usize,
) -> AssistantRenderItemSpec {
    AssistantRenderItemSpec {
        ids: vec![HistoryOutputId::Block(exchange.id)],
        msg_idx,
        leading_blank_lines,
        block_idx: Some(block_idx),
        boundary_kind: LiveRowBoundaryKind::AssistantBtw,
        // A completed exchange is immutable, including while it is the active tail.
        commit_ready: true,
        item: AssistantRenderItem::CanonicalBtw { msg_idx, block_idx },
    }
}

fn assistant_text_block_commit_ready(
    message: &ChatMessage,
    block_idx: usize,
    active_tail_block_idx: Option<usize>,
) -> bool {
    assistant_text_run_commit_ready(message, block_idx, block_idx, active_tail_block_idx)
}

fn assistant_text_run_commit_ready(
    message: &ChatMessage,
    start_block_idx: usize,
    end_block_idx: usize,
    active_tail_block_idx: Option<usize>,
) -> bool {
    let Some(active_tail_block_idx) = active_tail_block_idx else {
        return true;
    };
    if active_tail_block_idx >= start_block_idx && active_tail_block_idx <= end_block_idx {
        return false;
    }
    if active_tail_block_idx < start_block_idx {
        return true;
    }

    !assistant_text_run_handoff_requires_markdown_context(message, start_block_idx, end_block_idx)
}

fn assistant_text_run_handoff_requires_markdown_context(
    message: &ChatMessage,
    start_block_idx: usize,
    end_block_idx: usize,
) -> bool {
    let Some((text, trailing_spacing)) =
        assistant_text_run_source(message, start_block_idx, end_block_idx)
    else {
        return false;
    };
    let Some(next) = next_visible_assistant_text(message, end_block_idx) else {
        return false;
    };

    assistant_text_handoff_continues_open_table(&text, next)
        || assistant_text_handoff_continues_list_item(&text, trailing_spacing, next)
        || markdown_source_has_open_fence(&text)
        || markdown_source_has_open_inline_delimiter(&text)
}

fn assistant_text_run_source(
    message: &ChatMessage,
    start_block_idx: usize,
    end_block_idx: usize,
) -> Option<(String, TextBlockSpacing)> {
    let mut text = String::new();
    let mut trailing_spacing = TextBlockSpacing::None;

    for block in message.blocks.get(start_block_idx..=end_block_idx)? {
        let MessageBlock::Text(block) = block else {
            continue;
        };
        if block.text.is_empty() {
            continue;
        }
        trailing_spacing.append_source(&mut text, &block.text);
        trailing_spacing = block.trailing_spacing;
    }

    (!text.is_empty()).then_some((text, trailing_spacing))
}

fn assistant_text_handoff_continues_open_table(current: &str, next: &TextBlock) -> bool {
    if !markdown_table_tail_is_open(current) {
        return false;
    }

    starts_with_markdown_table_row(&next.text)
}

fn assistant_text_handoff_continues_list_item(
    current: &str,
    trailing_spacing: TextBlockSpacing,
    next: &TextBlock,
) -> bool {
    trailing_spacing == TextBlockSpacing::None
        && markdown_source_ends_with_list_item(current)
        && markdown_source_starts_with_list_continuation(&next.text)
}

fn markdown_source_starts_with_list_continuation(text: &str) -> bool {
    markdown_source_boundary_line(text, BoundaryLine::First)
        .is_some_and(|line| !markdown_source_line_is_list_item(line))
}

fn next_visible_assistant_text(message: &ChatMessage, block_idx: usize) -> Option<&TextBlock> {
    for block in message.blocks.iter().skip(block_idx + 1) {
        match block {
            MessageBlock::Text(text) if text.text.is_empty() => {}
            MessageBlock::Text(text) => return Some(text),
            MessageBlock::ToolCall(tool) | MessageBlock::ToolResult { tool, .. }
                if tool.hidden_unless_focused_interaction() => {}
            MessageBlock::Welcome(_)
            | MessageBlock::ImageAttachment(_)
            | MessageBlock::UserDialog(_) => {}
            MessageBlock::Notice(_)
            | MessageBlock::BtwExchange(_)
            | MessageBlock::ToolCall(_)
            | MessageBlock::ToolResult { .. } => {
                return None;
            }
        }
    }
    None
}

fn last_visible_assistant_block_idx(message: &ChatMessage) -> Option<usize> {
    message.blocks.iter().enumerate().rev().find_map(|(block_idx, block)| match block {
        MessageBlock::Text(text) if !text.text.is_empty() => Some(block_idx),
        MessageBlock::Notice(_) | MessageBlock::BtwExchange(_) => Some(block_idx),
        MessageBlock::ToolCall(tool) | MessageBlock::ToolResult { tool, .. }
            if !tool.hidden_unless_focused_interaction() =>
        {
            Some(block_idx)
        }
        MessageBlock::Text(_)
        | MessageBlock::ToolCall(_)
        | MessageBlock::ToolResult { .. }
        | MessageBlock::Welcome(_)
        | MessageBlock::ImageAttachment(_)
        | MessageBlock::UserDialog(_) => None,
    })
}

fn tool_call_commit_ready(tool: &ToolCallInfo) -> bool {
    let display = tool.display();
    (tool.status.is_terminal() || tool.has_frozen_launch())
        && display.pending_permission.is_none()
        && display.pending_question.is_none()
        && display.terminal_id.is_none()
}

fn flush_pending_text_run(
    pending_text: &mut Option<PendingAssistantTextRun>,
    items: &mut Vec<AssistantRenderItemSpec>,
) {
    if let Some(pending) = pending_text.take()
        && !pending.text.is_empty()
    {
        items.push(pending.into_render_item());
    }
}

fn leading_blank_lines_between(
    previous_kind: Option<AssistantInlineItemKind>,
    current_kind: AssistantInlineItemKind,
) -> usize {
    match (previous_kind, current_kind) {
        (None, _)
        | (Some(AssistantInlineItemKind::TextLike), AssistantInlineItemKind::TextLike)
        | (Some(AssistantInlineItemKind::Tool), AssistantInlineItemKind::Tool) => 0,
        (Some(AssistantInlineItemKind::TextLike), AssistantInlineItemKind::Tool)
        | (Some(AssistantInlineItemKind::Tool), AssistantInlineItemKind::TextLike) => 1,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AssistantLabelVisibility {
    Hidden,
    WithContent,
    WithActivity,
}

struct AssistantRowsRequest<'a> {
    app: Option<&'a mut App>,
    message_id: ChatMessageId,
    msg_idx: usize,
    items: Vec<AssistantRenderItemSpec>,
    current_mode_id: Option<&'a str>,
    width: u16,
    spinner: SpinnerState,
    show_thinking: bool,
    label_visibility: AssistantLabelVisibility,
    leading_blank_lines: usize,
    has_prior_assistant_content: bool,
}

struct AssistantThinkingMeta {
    message_id: ChatMessageId,
    msg_idx: usize,
    width: u16,
    spinner: SpinnerState,
}

fn render_assistant_rows(mut request: AssistantRowsRequest<'_>) -> RenderedMessageRows {
    if request.items.is_empty()
        && !request.show_thinking
        && request.label_visibility != AssistantLabelVisibility::WithActivity
    {
        return RenderedMessageRows::empty();
    }

    let mut rows = Vec::new();
    let mut boundaries = Vec::new();
    rows.extend(std::iter::repeat_with(Line::default).take(request.leading_blank_lines));
    append_assistant_label_rows(&mut rows, &mut boundaries, &request);

    let mut state = AssistantInlineLayoutState {
        has_body_content: request.has_prior_assistant_content,
        has_visible_content: request.has_prior_assistant_content,
    };
    let thinking = request.show_thinking.then_some(AssistantThinkingMeta {
        message_id: request.message_id,
        msg_idx: request.msg_idx,
        width: request.width,
        spinner: request.spinner,
    });

    let mut items = request.items.into_iter().peekable();
    while let Some(item) = items.next() {
        let boundary = AssistantBoundaryMeta {
            ids: item.ids,
            msg_idx: item.msg_idx,
            block_idx: item.block_idx,
            kind: item.boundary_kind,
            commit_ready: item.commit_ready,
        };
        let item_leading_blank_lines = item.leading_blank_lines;
        match item.item {
            AssistantRenderItem::Text(block) => {
                let trailing_gap = assistant_text_trailing_gap(&block, items.peek());
                let rendered =
                    render_assistant_text_block(block, request.width, !state.has_visible_content);
                if !rendered.is_empty() {
                    append_rendered_assistant_item(
                        &mut rows,
                        &mut boundaries,
                        &mut state,
                        boundary,
                        item_leading_blank_lines,
                        rendered,
                    );
                    rows.extend(std::iter::repeat_with(Line::default).take(trailing_gap));
                }
            }
            AssistantRenderItem::Notice(block) => {
                let trailing_gap = assistant_text_trailing_gap(&block.text, items.peek());
                let rendered =
                    render_assistant_notice_block(block, request.width, !state.has_visible_content);
                if !rendered.is_empty() {
                    append_rendered_assistant_item(
                        &mut rows,
                        &mut boundaries,
                        &mut state,
                        boundary,
                        item_leading_blank_lines,
                        rendered,
                    );
                    rows.extend(std::iter::repeat_with(Line::default).take(trailing_gap));
                }
            }
            AssistantRenderItem::CanonicalBtw { msg_idx, block_idx } => {
                let Some(app) = request.app.as_deref_mut() else {
                    continue;
                };
                append_rendered_assistant_item(
                    &mut rows,
                    &mut boundaries,
                    &mut state,
                    boundary,
                    item_leading_blank_lines,
                    render_canonical_btw_rows(app, msg_idx, block_idx, request.width),
                );
            }
            AssistantRenderItem::CanonicalTool { msg_idx, block_idx } => {
                let Some(app) = request.app.as_deref_mut() else {
                    continue;
                };
                append_rendered_assistant_item(
                    &mut rows,
                    &mut boundaries,
                    &mut state,
                    boundary,
                    item_leading_blank_lines,
                    render_canonical_tool_rows(
                        app,
                        msg_idx,
                        block_idx,
                        message_render_context(request.current_mode_id, request.width),
                        request.spinner,
                        false,
                    ),
                );
            }
        }
    }

    finish_assistant_rows(rows, boundaries, &state, thinking, request.label_visibility)
}

fn finish_assistant_rows(
    mut rows: Vec<Line<'static>>,
    mut boundaries: Vec<LiveRowBoundary>,
    state: &AssistantInlineLayoutState,
    thinking: Option<AssistantThinkingMeta>,
    label_visibility: AssistantLabelVisibility,
) -> RenderedMessageRows {
    if !state.has_visible_content
        && thinking.is_none()
        && label_visibility != AssistantLabelVisibility::WithActivity
    {
        return RenderedMessageRows::empty();
    }
    if let Some(meta) = thinking {
        let boundary_start = rows.len();
        if state.has_body_content {
            rows.push(Line::default());
        }
        boundaries.push(LiveRowBoundary {
            ids: vec![HistoryOutputId::AssistantThinking(meta.message_id)],
            msg_idx: meta.msg_idx,
            block_idx: None,
            kind: LiveRowBoundaryKind::AssistantThinking,
            start_row: boundary_start,
            commit_ready: false,
        });
        rows.extend(wrap_lines_to_physical_rows(
            &[Line::from(Span::styled(
                format!("{} Thinking…", meta.spinner.icon()),
                Style::default().fg(theme::DIM),
            ))],
            meta.width,
        ));
    }
    RenderedMessageRows::rendered(trim_trailing_blank_rows(rows), boundaries)
}

fn assistant_text_trailing_gap(
    block: &TextBlock,
    next_item: Option<&AssistantRenderItemSpec>,
) -> usize {
    let gap = block.trailing_blank_lines();
    if gap == 0 {
        return 0;
    }

    let next_text = next_item.and_then(assistant_item_text);
    if text_boundary_touches_markdown_list(&block.text, next_text) { 0 } else { gap }
}

fn assistant_item_text(item: &AssistantRenderItemSpec) -> Option<&TextBlock> {
    match &item.item {
        AssistantRenderItem::Text(block) => Some(block),
        AssistantRenderItem::Notice(block) => Some(&block.text),
        AssistantRenderItem::CanonicalBtw { .. } | AssistantRenderItem::CanonicalTool { .. } => {
            None
        }
    }
}

fn text_boundary_touches_markdown_list(current: &str, next: Option<&TextBlock>) -> bool {
    markdown_source_ends_with_list_item(current)
        || next.is_some_and(|block| markdown_source_starts_with_list_item(&block.text))
}

fn markdown_source_starts_with_list_item(text: &str) -> bool {
    markdown_source_boundary_line(text, BoundaryLine::First)
        .is_some_and(markdown_source_line_is_list_item)
}

fn markdown_source_ends_with_list_item(text: &str) -> bool {
    markdown_source_boundary_line(text, BoundaryLine::Last)
        .is_some_and(markdown_source_line_is_list_item)
}

#[derive(Clone, Copy)]
enum BoundaryLine {
    First,
    Last,
}

fn markdown_source_boundary_line(text: &str, boundary: BoundaryLine) -> Option<&str> {
    let mut in_fenced_code = false;
    let mut first = None;
    let mut last = None;

    for line in text.lines() {
        let trimmed = line.trim();
        let is_fence = trimmed.starts_with("```") || trimmed.starts_with("~~~");
        let in_code_line = in_fenced_code || is_fence;
        if is_fence {
            in_fenced_code = !in_fenced_code;
        }
        if in_code_line || trimmed.is_empty() {
            continue;
        }
        first.get_or_insert(line);
        last = Some(line);
    }

    match boundary {
        BoundaryLine::First => first,
        BoundaryLine::Last => last,
    }
}

fn markdown_source_line_is_list_item(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("- ")
        || trimmed.starts_with("* ")
        || trimmed.starts_with("+ ")
        || markdown_source_starts_with_ordered_list_marker(trimmed)
}

fn markdown_source_starts_with_ordered_list_marker(text: &str) -> bool {
    let digit_count = text.bytes().take_while(u8::is_ascii_digit).count();
    digit_count > 0 && text[digit_count..].starts_with(". ")
}

fn markdown_source_has_open_fence(text: &str) -> bool {
    let mut in_fenced_code = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fenced_code = !in_fenced_code;
        }
    }
    in_fenced_code
}

fn markdown_source_has_open_inline_delimiter(text: &str) -> bool {
    let mut in_code = false;
    let mut strong_asterisk_open = false;
    let mut strong_underscore_open = false;
    let mut chars = text.char_indices().peekable();

    while let Some((idx, ch)) = chars.next() {
        if ch == '\\' {
            let _ = chars.next();
            continue;
        }
        if ch == '`' {
            in_code = !in_code;
            continue;
        }
        if in_code {
            continue;
        }

        let rest = &text[idx..];
        if rest.starts_with("**") {
            strong_asterisk_open = !strong_asterisk_open;
            let _ = chars.next();
        } else if rest.starts_with("__") {
            strong_underscore_open = !strong_underscore_open;
            let _ = chars.next();
        }
    }

    in_code || strong_asterisk_open || strong_underscore_open
}

fn append_assistant_label_rows(
    rows: &mut Vec<Line<'static>>,
    boundaries: &mut Vec<LiveRowBoundary>,
    request: &AssistantRowsRequest<'_>,
) {
    if request.label_visibility == AssistantLabelVisibility::Hidden {
        return;
    }

    let first_commit_ready = request.items.first().is_some_and(|item| item.commit_ready)
        && request.app.as_deref().is_none_or(|app| {
            !app.config.show_message_timestamps_effective()
                || app.transcript.messages[request.msg_idx]
                    .timing
                    .timestamp
                    .is_some_and(|timestamp| !timestamp.observed)
                || !active_assistant_message_is_mutable(app, request.msg_idx)
        });
    let label_start = rows.len().saturating_sub(request.leading_blank_lines);
    boundaries.push(LiveRowBoundary {
        ids: vec![HistoryOutputId::AssistantLabel(request.message_id)],
        msg_idx: request.msg_idx,
        block_idx: None,
        kind: LiveRowBoundaryKind::AssistantLabel,
        start_row: label_start,
        commit_ready: first_commit_ready,
    });
    let mut label = assistant_role_label_line();
    if let Some(app) = request.app.as_deref()
        && let Some(timestamp) = crate::app::presentation::timestamp_label(
            app,
            &app.transcript.messages[request.msg_idx],
        )
    {
        label.spans.push(Span::styled(format!(" · {timestamp}"), Style::default().fg(theme::DIM)));
    }
    rows.extend(wrap_lines_to_physical_rows(&[label], request.width));
}

fn append_rendered_assistant_item(
    rows: &mut Vec<Line<'static>>,
    boundaries: &mut Vec<LiveRowBoundary>,
    state: &mut AssistantInlineLayoutState,
    boundary: AssistantBoundaryMeta,
    leading_blank_lines: usize,
    rendered: Vec<Line<'static>>,
) {
    if rendered.is_empty() {
        return;
    }
    let boundary_start = rows.len();
    rows.extend(std::iter::repeat_with(Line::default).take(leading_blank_lines));
    push_assistant_boundary(boundaries, boundary, boundary_start);
    state.has_body_content = true;
    state.has_visible_content = true;
    rows.extend(rendered);
}

#[derive(Debug, Clone)]
struct AssistantBoundaryMeta {
    ids: Vec<HistoryOutputId>,
    msg_idx: usize,
    block_idx: Option<usize>,
    kind: LiveRowBoundaryKind,
    commit_ready: bool,
}

fn push_assistant_boundary(
    boundaries: &mut Vec<LiveRowBoundary>,
    boundary: AssistantBoundaryMeta,
    start_row: usize,
) {
    boundaries.push(LiveRowBoundary {
        ids: boundary.ids,
        msg_idx: boundary.msg_idx,
        block_idx: boundary.block_idx,
        kind: boundary.kind,
        start_row,
        commit_ready: boundary.commit_ready,
    });
}

fn message_render_context(current_mode_id: Option<&str>, width: u16) -> MessageRenderContext<'_> {
    MessageRenderContext::new(current_mode_id, width)
}

fn render_assistant_text_block(
    mut block: TextBlock,
    width: u16,
    trim_leading_blank_lines: bool,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    render_text_block_cached(&mut block, width, None, true, &mut lines);
    let lines = if trim_leading_blank_lines {
        let first_non_blank =
            lines.iter().position(|line| !line_is_blank(line)).unwrap_or(lines.len());
        lines.into_iter().skip(first_non_blank).collect::<Vec<_>>()
    } else {
        lines
    };
    wrap_markdown_lines_to_physical_rows(&lines, width)
}

fn trim_trailing_blank_rows(mut rows: Vec<Line<'static>>) -> Vec<Line<'static>> {
    while rows.last().is_some_and(line_is_blank) {
        rows.pop();
    }
    rows
}

fn render_assistant_notice_block(
    block: NoticeBlock,
    width: u16,
    trim_leading_blank_lines: bool,
) -> Vec<Line<'static>> {
    let mut lines = render_assistant_text_block(block.text, width, trim_leading_blank_lines);
    for line in &mut lines {
        for span in &mut line.spans {
            span.style = span.style.fg(system_severity_color(block.severity));
        }
    }
    lines
}

fn render_canonical_btw_rows(
    app: &mut App,
    msg_idx: usize,
    block_idx: usize,
    width: u16,
) -> Vec<Line<'static>> {
    let Some(MessageBlock::BtwExchange(exchange)) = app
        .transcript
        .messages
        .get_mut(msg_idx)
        .and_then(|message| message.blocks.get_mut(block_idx))
    else {
        return Vec::new();
    };
    render_btw_exchange_lines(exchange, width)
}

fn render_canonical_tool_rows(
    app: &mut App,
    msg_idx: usize,
    block_idx: usize,
    render_context: MessageRenderContext<'_>,
    spinner: SpinnerState,
    interaction: bool,
) -> Vec<Line<'static>> {
    let show_duration = app.config.show_turn_duration_effective();
    let Some(block) = app
        .transcript
        .messages
        .get_mut(msg_idx)
        .and_then(|message| message.blocks.get_mut(block_idx))
    else {
        return Vec::new();
    };
    let tc = match block {
        MessageBlock::ToolCall(tc) if interaction => tc.as_mut(),
        MessageBlock::ToolCall(tc) => tc.display_mut(),
        MessageBlock::ToolResult { tool, .. } => tool.as_mut(),
        _ => return Vec::new(),
    };
    if tc.hidden_unless_focused_interaction() {
        return Vec::new();
    }

    let mut rows = Vec::new();
    tool_call::render_tool_call_cached(
        tc,
        render_context.tool_render_context,
        render_context.width,
        spinner,
        &mut rows,
    );
    if show_duration
        && let Some(timing) =
            tc.output_metadata.as_ref().and_then(|metadata| metadata.timing.as_ref())
    {
        rows.push(Line::from(Span::styled(
            format!(
                "  {}{}",
                if timing.source == crate::agent::model::ToolTimingSource::Progress {
                    "Tool elapsed ≥ "
                } else {
                    "Task elapsed "
                },
                crate::app::presentation::elapsed(std::time::Duration::from_millis(
                    timing.duration_ms
                ))
            ),
            Style::default().fg(theme::DIM),
        )));
    }
    if interaction {
        tc.cache.evict_cached_render();
    }
    wrap_lines_to_physical_rows(&rows, render_context.width)
}

pub(crate) fn assistant_role_label_line() -> Line<'static> {
    Line::from(vec![ratatui::text::Span::styled(
        "Claude",
        Style::default().fg(theme::ROLE_ASSISTANT).add_modifier(Modifier::BOLD),
    )])
}

fn system_severity_color(severity: SystemSeverity) -> Color {
    match severity {
        SystemSeverity::Info => theme::DIM,
        SystemSeverity::Warning => theme::STATUS_WARNING,
        SystemSeverity::Error => theme::STATUS_ERROR,
    }
}

fn line_is_blank(line: &Line<'_>) -> bool {
    line.spans.iter().all(|span| span.content.as_ref().chars().all(char::is_whitespace))
}

fn segments_to_physical_rows(
    segments: &[MessageRowSegment],
    width: u16,
    skip_first_segment: bool,
) -> Vec<Line<'static>> {
    let mut rows = Vec::new();
    for (idx, segment) in segments.iter().enumerate() {
        if skip_first_segment && idx == 0 {
            continue;
        }
        match segment {
            MessageRowSegment::Blank => rows.push(Line::default()),
            MessageRowSegment::Lines { lines } => {
                rows.extend(wrap_markdown_lines_to_physical_rows(lines, width));
            }
        }
    }
    rows
}

fn preview_rows(rows: &[Line<'static>], limit: usize) -> String {
    rows.iter()
        .take(limit)
        .enumerate()
        .map(|(idx, line)| {
            let text = line.spans.iter().map(|span| span.content.as_ref()).collect::<String>();
            format!("[{idx}] {text}")
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

#[cfg(test)]
mod tests {
    use super::{
        LiveRowBoundaryKind, LiveRowSegment, SerializedLiveRows,
        serialize_live_rows_with_boundaries_excluding,
    };

    #[test]
    fn tool_spinner_stays_in_its_row_and_respects_reduced_motion() {
        let mut app = App::test_default();
        app.transcript.messages.push(assistant_blocks_message(vec![
            tool_call_block_with_status_interaction(
                "static-tool",
                model::ToolCallStatus::InProgress,
                false,
                false,
                false,
            ),
        ]));
        app.config.snapshot = Some(crate::agent::settings::SettingsSnapshot::test_value(
            "prefersReducedMotion",
            serde_json::json!(true),
        ));
        let tool = line_texts(&serialize_live_rows(&mut app, 120));
        assert!(
            tool.iter().any(|line| line.contains('◆') && line.contains("Bash Child Tool")),
            "{tool:?}"
        );
        app.spinner_frame = 8;
        assert_eq!(line_texts(&serialize_live_rows(&mut app, 120)), tool);
        app.config.snapshot = None;
        assert_ne!(line_texts(&serialize_live_rows(&mut app, 120)), tool);
    }
    use crate::agent::model;
    use crate::app::{
        App, AppStatus, BlockCache, ChatMessage, ChatMessageId, HistoryOutputId, MessageBlock,
        MessageRole, NoticeBlock, TextBlock, TextBlockSpacing, ToolCallInfo,
    };
    use ratatui::text::Line;
    use std::collections::BTreeSet;

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
            .trim_end()
            .to_owned()
    }

    fn line_texts(rows: &[Line<'_>]) -> Vec<String> {
        rows.iter().map(line_text).collect()
    }

    fn compact_text(rows: &[Line<'_>]) -> String {
        line_texts(rows).join("").chars().filter(|ch| !ch.is_whitespace()).collect()
    }

    fn serialize_live_rows(app: &mut App, width: u16) -> Vec<Line<'static>> {
        serialize_all_rows_with_boundaries(app, width).rows().to_vec()
    }

    fn serialize_all_rows_with_boundaries(app: &mut App, width: u16) -> SerializedLiveRows {
        serialize_live_rows_with_boundaries_excluding(app, width, &BTreeSet::new())
    }

    fn live_segment(start_row: usize, end_row: usize, commit_ready: bool) -> LiveRowSegment {
        LiveRowSegment {
            ids: vec![HistoryOutputId::AssistantLabel(ChatMessageId::new())],
            msg_idx: 0,
            block_idx: None,
            kind: LiveRowBoundaryKind::AssistantLabel,
            start_row,
            end_row,
            commit_ready,
        }
    }

    #[test]
    fn serialized_live_rows_skip_invalid_segment_ranges() {
        let rows = vec![Line::from("first"), Line::from("second")];
        let serialized = SerializedLiveRows::from_parts_for_test(
            rows,
            vec![live_segment(0, 1, true), live_segment(1, 3, true)],
        );

        assert_eq!(line_texts(&serialized.rows_excluding_ids(&BTreeSet::new())), vec!["first"]);
        assert!(serialized.segment_rows(&serialized.segments()[1]).is_none());
    }

    #[test]
    fn serialized_live_rows_clamp_stable_boundary_to_rows_len() {
        let serialized = SerializedLiveRows::from_parts_for_test(
            vec![Line::from("only")],
            vec![live_segment(4, 5, false)],
        );

        assert_eq!(serialized.stable_row_count(), 1);
        assert_eq!(serialized.first_mutable_boundary_start(), Some(1));
    }

    fn user_text_message(text: &str) -> ChatMessage {
        ChatMessage::new(
            MessageRole::User,
            vec![MessageBlock::Text(TextBlock::from_complete(text))],
            None,
        )
    }

    fn assistant_message() -> ChatMessage {
        ChatMessage::new(MessageRole::Assistant, Vec::new(), None)
    }

    fn assistant_text_message(text: &str) -> ChatMessage {
        assistant_blocks_message(vec![MessageBlock::Text(TextBlock::from_complete(text))])
    }

    fn assistant_blocks_message(blocks: Vec<MessageBlock>) -> ChatMessage {
        ChatMessage::new(MessageRole::Assistant, blocks, None)
    }

    fn system_text_message(text: &str) -> ChatMessage {
        ChatMessage::new(
            MessageRole::System(Some(crate::app::SystemSeverity::Info)),
            vec![MessageBlock::Text(TextBlock::from_complete(text))],
            None,
        )
    }

    fn tool_call_block(id: &str, hidden: bool) -> MessageBlock {
        tool_call_block_with_status_interaction(
            id,
            model::ToolCallStatus::Completed,
            hidden,
            false,
            false,
        )
    }

    fn named_tool_call_block(id: &str, title: &str, sdk_tool_name: &str) -> MessageBlock {
        let mut block = tool_call_block(id, false);
        let MessageBlock::ToolCall(tool) = &mut block else {
            unreachable!("tool_call_block always returns a tool call");
        };
        tool.title = title.to_owned();
        tool.sdk_tool_name = sdk_tool_name.to_owned();
        block
    }

    fn tool_call_block_with_interaction(
        id: &str,
        hidden: bool,
        focused_permission: bool,
        focused_question: bool,
    ) -> MessageBlock {
        tool_call_block_with_status_interaction(
            id,
            model::ToolCallStatus::Completed,
            hidden,
            focused_permission,
            focused_question,
        )
    }

    fn tool_call_block_with_status_interaction(
        id: &str,
        status: model::ToolCallStatus,
        hidden: bool,
        focused_permission: bool,
        focused_question: bool,
    ) -> MessageBlock {
        let mut tool = ToolCallInfo {
            id: id.to_owned(),
            source_message_uuids: Vec::new(),
            title: "Child Tool".to_owned(),
            sdk_tool_name: "Bash".to_owned(),
            raw_input: None,
            raw_input_bytes: 0,
            locations: Vec::new(),
            output_metadata: None,
            task_metadata: None,
            status,
            content: Vec::new(),
            hidden,
            terminal_id: None,
            terminal_command: None,
            terminal_output: None,
            terminal_output_len: 0,
            cache: BlockCache::default(),
            pending_permission: None,
            pending_question: None,
            history: crate::app::ToolCallHistory::Live,
        };

        if focused_permission {
            let (response_tx, _response_rx) = tokio::sync::oneshot::channel();
            tool.pending_permission = Some(crate::app::InlinePermission {
                options: Vec::new(),
                display: None,
                subagent_context: None,
                response_tx,
                selected_index: 0,
                focused: true,
            });
        }

        if focused_question {
            let (response_tx, _response_rx) = tokio::sync::oneshot::channel();
            tool.pending_question = Some(crate::app::InlineQuestion {
                idle_timeout: None,
                last_activity: std::time::Instant::now(),
                prompt: model::QuestionPrompt::new(
                    "Choose an option",
                    "Question",
                    false,
                    vec![model::QuestionOption::new("yes", "Yes")],
                ),
                response_tx,
                focused_option_index: 0,
                selected_option_indices: std::collections::BTreeSet::new(),
                notes: String::new(),
                notes_cursor: 0,
                editing_notes: false,
                focused: true,
                question_index: 0,
                total_questions: 1,
            });
        }

        MessageBlock::ToolCall(Box::new(tool))
    }

    fn tool_call_block_with_pending_permission(
        id: &str,
        hidden: bool,
        focused: bool,
    ) -> MessageBlock {
        let mut block = tool_call_block(id, hidden);
        let MessageBlock::ToolCall(tool) = &mut block else {
            unreachable!("tool_call_block always returns a tool call");
        };
        let (response_tx, _response_rx) = tokio::sync::oneshot::channel();
        tool.pending_permission = Some(crate::app::InlinePermission {
            options: Vec::new(),
            display: None,
            subagent_context: None,
            response_tx,
            selected_index: 0,
            focused,
        });
        block
    }

    fn tool_call_block_with_subagent_pending_permission(id: &str) -> MessageBlock {
        let mut block = tool_call_block_with_pending_permission(id, true, true);
        let MessageBlock::ToolCall(tool) = &mut block else {
            unreachable!("tool_call_block_with_pending_permission returns a tool call");
        };
        let permission = tool.pending_permission.as_mut().expect("pending permission");
        permission.options = vec![
            model::PermissionOption::new(
                "allow",
                "Allow once",
                model::PermissionOptionKind::AllowOnce,
            ),
            model::PermissionOption::new("deny", "Deny", model::PermissionOptionKind::RejectOnce),
        ];
        permission.subagent_context = Some(crate::app::SubagentPermissionContext {
            subagent_label: "general-purpose".to_owned(),
            child_tool_name: "Bash".to_owned(),
            child_tool_title: "Child Tool".to_owned(),
            parent_tool_call_id: "agent-1".to_owned(),
            parent_tool_title: Some("Agent: general-purpose".to_owned()),
            parent_model: Some("claude-opus-4-8".to_owned()),
            parent_raw_input: None,
        });
        block
    }

    #[test]
    fn live_rows_do_not_start_with_synthetic_blank_row() {
        let mut app = App::test_default();
        app.transcript.messages.push(assistant_text_message("hi"));

        let rows = serialize_live_rows(&mut app, 120);

        assert!(rows.first().is_some_and(|line| !line_text(line).trim().is_empty()));
        assert_eq!(line_text(&rows[0]), "Claude");
    }

    #[test]
    fn live_rows_render_user_row_while_assistant_streams() {
        let mut app = App::test_default();
        app.push_message_tracked(user_text_message("hello"));
        app.transcript.messages.push(assistant_text_message("still streaming"));

        let rows = serialize_live_rows(&mut app, 120);
        let text = line_texts(&rows);

        assert_eq!(text, vec!["User", "hello", "Claude", "still streaming"]);
    }

    #[test]
    fn user_paragraph_gap_survives_physical_wrapping_while_assistant_streams() {
        let mut app = App::test_default();
        app.push_message_tracked(user_text_message("hello hello hello hello\n\nhow are you"));
        app.transcript.messages.push(assistant_text_message("still streaming"));

        for width in [80, 12, 80] {
            let rows = serialize_live_rows(&mut app, width);
            let texts: Vec<_> =
                line_texts(&rows).into_iter().map(|line| line.trim_end().to_owned()).collect();
            let assistant =
                texts.iter().position(|line| line == "Claude").expect("assistant label");
            let expected = if width == 12 {
                vec!["User", "hello hello", "hello hello", "", "how are you"]
            } else {
                vec!["User", "hello hello hello hello", "", "how are you"]
            };
            assert_eq!(&texts[..assistant], expected, "width={width}");
        }
    }

    #[test]
    fn width_rebuild_wraps_same_canonical_transcript_to_different_row_counts() {
        let mut app = App::test_default();
        app.push_message_tracked(user_text_message(
            "Resize should rebuild canonical user prose from messages with enough words to wrap \
             differently at narrow widths.",
        ));
        app.transcript.messages.push(assistant_text_message(
            "Assistant rows also come directly from app.transcript.messages, so changing width changes \
             physical row count without changing semantic text.",
        ));

        let narrow_rows = serialize_live_rows(&mut app, 32);
        let wide_rows = serialize_live_rows(&mut app, 120);
        let narrow_text = line_texts(&narrow_rows).join("\n");
        let wide_text = line_texts(&wide_rows).join("\n");

        assert!(
            narrow_rows.len() > wide_rows.len(),
            "narrow rows should wrap more physical rows; narrow={narrow_text:?}, wide={wide_text:?}"
        );
        assert_eq!(compact_text(&narrow_rows), compact_text(&wide_rows));
        assert!(narrow_text.contains("User"));
        assert!(narrow_text.contains("Claude"));
        assert!(wide_text.contains("User"));
        assert!(wide_text.contains("Claude"));
    }

    #[test]
    fn live_row_boundaries_stop_stable_prefix_before_active_assistant() {
        let mut app = App::test_default();
        app.push_message_tracked(user_text_message("hello"));
        app.transcript.messages.push(assistant_text_message("still streaming"));
        app.bind_active_turn_assistant(1);
        app.status = AppStatus::Running;

        let serialized = serialize_all_rows_with_boundaries(&mut app, 120);

        assert_eq!(serialized.stable_row_count(), 2);
    }

    #[test]
    fn active_assistant_commits_completed_text_before_streaming_tail() {
        let mut app = App::test_default();
        app.transcript.messages.push(assistant_blocks_message(vec![
            MessageBlock::Text(TextBlock::from_complete("prefix")),
            MessageBlock::Text(TextBlock::from_complete("tail")),
        ]));
        app.bind_active_turn_assistant(0);
        app.status = AppStatus::Running;

        let serialized = serialize_all_rows_with_boundaries(&mut app, 120);
        let stable_text = line_texts(&serialized.rows()[..serialized.stable_row_count()]);
        let mutable_text = line_texts(&serialized.rows()[serialized.stable_row_count()..]);

        assert_eq!(stable_text, vec!["Claude", "prefix"]);
        assert_eq!(mutable_text, vec!["tail"]);
    }

    #[test]
    fn active_assistant_commits_completed_tool_before_pending_permission_tool() {
        let mut app = App::test_default();
        app.transcript.messages.push(assistant_blocks_message(vec![
            tool_call_block("done-tool", false),
            tool_call_block_with_status_interaction(
                "pending-tool",
                model::ToolCallStatus::InProgress,
                false,
                true,
                false,
            ),
        ]));
        app.bind_active_turn_assistant(0);
        app.status = AppStatus::Running;

        let serialized = serialize_all_rows_with_boundaries(&mut app, 120);
        let stable_text = line_texts(&serialized.rows()[..serialized.stable_row_count()]);
        let mutable_text = line_texts(&serialized.rows()[serialized.stable_row_count()..]);

        assert!(stable_text.iter().any(|line| line == "Claude"));
        assert!(stable_text.iter().any(|line| line.contains("Child Tool")));
        assert!(mutable_text.iter().any(|line| line.contains("Child Tool")));
        assert!(mutable_text.iter().any(|line| line.contains("select")));
    }

    #[test]
    fn active_assistant_completed_tool_is_commit_ready() {
        let mut app = App::test_default();
        app.transcript
            .messages
            .push(assistant_blocks_message(vec![tool_call_block("done-tool", false)]));
        app.bind_active_turn_assistant(0);
        app.status = AppStatus::Running;

        let serialized = serialize_all_rows_with_boundaries(&mut app, 120);

        assert_eq!(serialized.stable_row_count(), serialized.rows().len());
    }

    #[test]
    fn active_assistant_in_progress_tool_keeps_label_mutable() {
        let mut app = App::test_default();
        app.transcript.messages.push(assistant_blocks_message(vec![
            tool_call_block_with_status_interaction(
                "running-tool",
                model::ToolCallStatus::InProgress,
                false,
                false,
                false,
            ),
        ]));
        app.bind_active_turn_assistant(0);
        app.status = AppStatus::Running;

        let serialized = serialize_all_rows_with_boundaries(&mut app, 120);

        assert_eq!(serialized.stable_row_count(), 0);
        assert_eq!(
            serialized.first_mutable_boundary_kind(),
            Some(LiveRowBoundaryKind::AssistantLabel)
        );
    }

    #[test]
    fn detached_web_launch_is_stable_and_final_output_gets_its_own_rows() {
        let mut app = App::test_default();
        let mut block = tool_call_block_with_status_interaction(
            "web-1",
            model::ToolCallStatus::Detached,
            false,
            false,
            false,
        );
        let MessageBlock::ToolCall(tool) = &mut block else {
            unreachable!("tool helper returns a tool call")
        };
        tool.title = "Deferred web fetch".to_owned();
        tool.sdk_tool_name = "WebFetch".to_owned();
        app.transcript.messages.push(assistant_blocks_message(vec![block]));
        app.index_tool_call("web-1".to_owned(), 0, 0);
        app.sync_tool_call_history("web-1");
        app.status = AppStatus::Ready;

        let serialized = serialize_all_rows_with_boundaries(&mut app, 120);
        assert_eq!(serialized.stable_row_count(), serialized.rows().len());
        let launch = line_texts(serialized.rows());
        assert!(launch.iter().any(|line| line.contains("Deferred web fetch")
            && line.contains(crate::ui::theme::ICON_DETACHED)));

        let MessageBlock::ToolCall(tool) = &mut app.transcript.messages[0].blocks[0] else {
            unreachable!("original web call")
        };
        tool.status = model::ToolCallStatus::Completed;
        tool.content = vec![model::ToolCallContent::from("Actual web output")];
        tool.invalidate_render_cache();
        app.sync_tool_call_history("web-1");
        let serialized = serialize_all_rows_with_boundaries(&mut app, 120);
        assert_eq!(serialized.stable_row_count(), serialized.rows().len());
        let rows = line_texts(serialized.rows());
        let cards: Vec<_> =
            rows.iter().filter(|line| line.contains("Deferred web fetch")).collect();
        assert_eq!(cards.len(), 2);
        assert!(cards[0].contains(crate::ui::theme::ICON_DETACHED));
        assert!(cards[1].contains(crate::ui::theme::ICON_COMPLETED));
        assert!(rows.iter().any(|line| line.contains("Actual web output")));
    }

    #[test]
    fn live_rows_render_committed_assistant_prefix_before_live_tail() {
        let mut app = App::test_default();
        app.transcript.messages.push(assistant_blocks_message(vec![
            MessageBlock::Text(TextBlock::from_complete("prefix")),
            MessageBlock::Text(TextBlock::from_complete("tail")),
        ]));

        let rows = serialize_live_rows(&mut app, 120);
        let text = line_texts(&rows);

        assert_eq!(text, vec!["Claude", "prefix", "tail"]);
    }

    #[test]
    fn live_adjacent_text_blocks_preserve_paragraph_gap() {
        let mut app = App::test_default();
        app.transcript.messages.push(assistant_blocks_message(vec![
            MessageBlock::Text(
                TextBlock::from_complete("line 1: ready\n\n")
                    .with_trailing_spacing(TextBlockSpacing::ParagraphBreak),
            ),
            MessageBlock::Text(TextBlock::from_complete("line 2: ready")),
        ]));

        let rows = serialize_live_rows(&mut app, 120);

        assert_eq!(line_texts(&rows), vec!["Claude", "line 1: ready", "", "line 2: ready"]);
    }

    #[test]
    fn live_assistant_text_suppresses_paragraph_gap_before_list_block() {
        let mut app = App::test_default();
        app.transcript.messages.push(assistant_blocks_message(vec![
            MessageBlock::Text(
                TextBlock::from_complete("Intro\n\n")
                    .with_trailing_spacing(TextBlockSpacing::ParagraphBreak),
            ),
            MessageBlock::Text(TextBlock::from_complete("- One\n- Two")),
        ]));

        let rows = serialize_live_rows(&mut app, 120);

        assert_eq!(line_texts(&rows), vec!["Claude", "Intro", "  - One", "  - Two"]);
    }

    #[test]
    fn live_assistant_text_suppresses_paragraph_gap_after_list_block() {
        let mut app = App::test_default();
        app.transcript.messages.push(assistant_blocks_message(vec![
            MessageBlock::Text(
                TextBlock::from_complete("- One\n- Two\n\n")
                    .with_trailing_spacing(TextBlockSpacing::ParagraphBreak),
            ),
            MessageBlock::Text(TextBlock::from_complete("Outro")),
        ]));

        let rows = serialize_live_rows(&mut app, 120);

        assert_eq!(line_texts(&rows), vec!["Claude", "  - One", "  - Two", "Outro"]);
    }

    #[test]
    fn live_assistant_text_preserves_single_newline_rows() {
        let mut app = App::test_default();
        app.transcript
            .messages
            .push(assistant_text_message("line 1: ready\nline 2: ready\nline 3: ready"));

        let rows = serialize_live_rows(&mut app, 120);

        assert_eq!(
            line_texts(&rows),
            vec!["Claude", "line 1: ready", "line 2: ready", "line 3: ready"]
        );
    }

    #[test]
    fn live_rows_render_assistant_notice_from_messages() {
        let mut app = App::test_default();
        app.transcript.messages.push(assistant_blocks_message(vec![MessageBlock::Notice(
            NoticeBlock::from_complete(crate::app::SystemSeverity::Warning, "watch this"),
        )]));

        let rows = serialize_live_rows(&mut app, 120);
        let text = line_texts(&rows);

        assert_eq!(text, vec!["Claude", "watch this"]);
    }

    #[test]
    fn live_rows_render_visible_tool_from_canonical_message_block() {
        let mut app = App::test_default();
        app.transcript
            .messages
            .push(assistant_blocks_message(vec![tool_call_block("tool-1", false)]));

        let rows = serialize_live_rows(&mut app, 120);
        let text = line_texts(&rows);

        assert!(text.iter().any(|line| line == "Claude"));
        assert!(text.iter().any(|line| line.contains("Child Tool")));
    }

    #[test]
    fn excluded_static_assistant_text_prefix_is_not_rendered_into_live_rows() {
        let first = TextBlock::from_complete("first paragraph\n\n")
            .with_trailing_spacing(TextBlockSpacing::ParagraphBreak);
        let first_id = first.id;
        let second = TextBlock::from_complete("second paragraph");
        let second_id = second.id;
        let message =
            assistant_blocks_message(vec![MessageBlock::Text(first), MessageBlock::Text(second)]);
        let message_id = message.id;
        let mut app = App::test_default();
        app.transcript.messages.push(message);
        let excluded_ids = BTreeSet::from([
            HistoryOutputId::AssistantLabel(message_id),
            HistoryOutputId::Block(first_id),
        ]);

        let serialized =
            serialize_live_rows_with_boundaries_excluding(&mut app, 120, &excluded_ids);
        let text = line_texts(serialized.rows());

        assert_eq!(text, vec!["second paragraph"]);
        assert!(
            serialized
                .segments()
                .iter()
                .all(|segment| !segment.ids.contains(&HistoryOutputId::Block(first_id)))
        );
        assert!(
            serialized
                .segments()
                .iter()
                .any(|segment| segment.ids.contains(&HistoryOutputId::Block(second_id)))
        );
    }

    #[test]
    fn active_assistant_table_handoff_keeps_prefix_mutable_with_pipe_row_tail() {
        let first = TextBlock::from_complete(concat!(
            "| Hassle | What users report | Refs |\n",
            "| --- | --- | --- |\n",
            "| Input lag | Keystrokes echo late as context fills | #18943 |\n",
        ));
        let second = TextBlock::from_complete(
            "| Slow startup | Startup output lands after a long delay | #42002 |\n",
        );
        let mut app = App::test_default();
        app.transcript.messages.push(assistant_blocks_message(vec![
            MessageBlock::Text(first),
            MessageBlock::Text(second),
        ]));
        app.bind_active_turn_assistant(0);
        app.status = AppStatus::Running;

        let serialized = serialize_all_rows_with_boundaries(&mut app, 180);
        let committed_ids = serialized
            .segments()
            .iter()
            .filter(|segment| segment.commit_ready)
            .flat_map(|segment| segment.ids.iter().cloned())
            .collect::<BTreeSet<_>>();
        let remaining =
            serialize_live_rows_with_boundaries_excluding(&mut app, 180, &committed_ids);
        let remaining_text = line_texts(remaining.rows());

        assert_eq!(serialized.stable_row_count(), 0);
        assert!(
            remaining_text.iter().any(|line| line.contains("Slow startup")),
            "expected table continuation to remain visible: {remaining_text:?}"
        );
        assert!(
            !remaining_text.iter().any(|line| line.trim_start().starts_with("| Slow startup |")),
            "table continuation must not render as a raw pipe row: {remaining_text:?}"
        );
    }

    #[test]
    fn live_assistant_ordered_list_wraps_continuation_with_hanging_indent() {
        let mut app = App::test_default();
        app.transcript.messages.push(assistant_text_message(
            "3. Keymap survey - mapping the keymap system, focus dispatch, and help generation.",
        ));

        let rows = serialize_live_rows(&mut app, 38);
        let text = line_texts(&rows);

        assert!(
            text.iter().any(|line| line.starts_with("  3. Keymap survey")),
            "missing ordered list prefix: {text:?}"
        );
        assert!(
            text.iter().any(|line| line.starts_with("     keymap system")),
            "list continuation lost hanging indent: {text:?}"
        );
    }

    #[test]
    fn active_assistant_list_handoff_keeps_context_prefix_mutable() {
        let first = TextBlock::from_complete("3. Keymap survey maps the system.");
        let second = TextBlock::from_complete(" Additional detail keeps streaming.");
        let mut app = App::test_default();
        app.transcript.messages.push(assistant_blocks_message(vec![
            MessageBlock::Text(first),
            MessageBlock::Text(second),
        ]));
        app.bind_active_turn_assistant(0);
        app.status = AppStatus::Running;

        let serialized = serialize_all_rows_with_boundaries(&mut app, 38);
        let committed_ids = serialized
            .segments()
            .iter()
            .filter(|segment| segment.commit_ready)
            .flat_map(|segment| segment.ids.iter().cloned())
            .collect::<BTreeSet<_>>();
        let remaining = serialize_live_rows_with_boundaries_excluding(&mut app, 38, &committed_ids);
        let remaining_text = line_texts(remaining.rows());

        assert_eq!(serialized.stable_row_count(), 0);
        assert!(
            remaining_text.iter().any(|line| line.starts_with("  3. Keymap survey")),
            "missing ordered list prefix: {remaining_text:?}"
        );
    }

    #[test]
    fn active_assistant_inline_bold_handoff_keeps_prefix_mutable_until_closed() {
        let first = TextBlock::from_complete("This is **bold");
        let second = TextBlock::from_complete(" text** done.");
        let mut app = App::test_default();
        app.transcript.messages.push(assistant_blocks_message(vec![
            MessageBlock::Text(first),
            MessageBlock::Text(second),
        ]));
        app.bind_active_turn_assistant(0);
        app.status = AppStatus::Running;

        let serialized = serialize_all_rows_with_boundaries(&mut app, 80);
        let committed_ids = serialized
            .segments()
            .iter()
            .filter(|segment| segment.commit_ready)
            .flat_map(|segment| segment.ids.iter().cloned())
            .collect::<BTreeSet<_>>();
        let remaining = serialize_live_rows_with_boundaries_excluding(&mut app, 80, &committed_ids);
        let remaining_text = line_texts(remaining.rows());

        assert_eq!(serialized.stable_row_count(), 0);
        assert!(remaining_text.iter().any(|line| line.contains("This is bold")));
        assert!(remaining_text.iter().any(|line| line.contains("text done.")));
        assert!(
            !remaining_text.iter().any(|line| line.contains("**")),
            "inline markdown marker leaked through handoff: {remaining_text:?}"
        );
    }

    #[test]
    fn active_assistant_inline_bold_handoff_commits_after_following_block_arrives() {
        let first = TextBlock::from_complete("This is **bold");
        let second = TextBlock::from_complete(" text** done.");
        let third = TextBlock::from_complete("more");
        let mut app = App::test_default();
        app.transcript.messages.push(assistant_blocks_message(vec![
            MessageBlock::Text(first),
            MessageBlock::Text(second),
            MessageBlock::Text(third),
        ]));
        app.bind_active_turn_assistant(0);
        app.status = AppStatus::Running;

        let serialized = serialize_all_rows_with_boundaries(&mut app, 80);
        let stable_text = line_texts(&serialized.rows()[..serialized.stable_row_count()]);

        assert!(
            stable_text.iter().any(|line| line.contains("This is bold")),
            "closed inline markdown handoff did not commit: {stable_text:?}"
        );
        assert!(
            stable_text.iter().any(|line| line.contains("text done.")),
            "closed inline markdown suffix did not commit: {stable_text:?}"
        );
        assert!(
            !stable_text.iter().any(|line| line.contains("more")),
            "active tail should remain mutable: {stable_text:?}"
        );
        assert!(
            !stable_text.iter().any(|line| line.contains("**")),
            "inline markdown marker leaked through committed handoff: {stable_text:?}"
        );
    }

    #[test]
    fn excluded_static_tool_prefix_is_not_rendered_before_active_tool() {
        let mut done = named_tool_call_block("done-tool", "Done Tool", "CustomTool");
        let mut running = tool_call_block_with_status_interaction(
            "running-tool",
            model::ToolCallStatus::InProgress,
            false,
            false,
            false,
        );
        let MessageBlock::ToolCall(tool) = &mut running else {
            unreachable!("tool_call_block_with_status_interaction returns a tool call");
        };
        tool.title = "Running Tool".to_owned();
        tool.sdk_tool_name = "CustomTool".to_owned();
        let MessageBlock::ToolCall(tool) = &mut done else {
            unreachable!("named_tool_call_block returns a tool call");
        };
        tool.status = model::ToolCallStatus::Completed;

        let message = assistant_blocks_message(vec![done, running]);
        let message_id = message.id;
        let mut app = App::test_default();
        app.transcript.messages.push(message);
        app.bind_active_turn_assistant(0);
        app.status = AppStatus::Running;
        let excluded_ids = BTreeSet::from([
            HistoryOutputId::AssistantLabel(message_id),
            HistoryOutputId::ToolCall("done-tool".to_owned()),
        ]);

        let serialized =
            serialize_live_rows_with_boundaries_excluding(&mut app, 120, &excluded_ids);
        let text = line_texts(serialized.rows());

        assert!(!text.iter().any(|line| line == "Claude"));
        assert!(!text.iter().any(|line| line.contains("Done Tool")));
        assert!(text.iter().any(|line| line.contains("Running Tool")));
        assert_eq!(serialized.stable_row_count(), 0);
        assert_eq!(
            serialized.first_mutable_boundary_kind(),
            Some(LiveRowBoundaryKind::AssistantTool)
        );
    }

    #[test]
    fn empty_assistant_heading_is_paired_with_activity_independently_of_tips() {
        let mut app = App::test_default();
        app.transcript.messages.push(user_text_message("request"));
        app.transcript.messages.push(assistant_message());
        app.bind_active_turn_assistant(1);
        assert_eq!(line_texts(&serialize_live_rows(&mut app, 80)), ["User", "request"]);
        app.status = AppStatus::Running;
        app.begin_turn_activity(std::time::Instant::now());
        for width in [32, 120, 32] {
            let serialized = serialize_all_rows_with_boundaries(&mut app, width);
            assert_eq!(line_texts(serialized.rows()), ["User", "request", "Claude"]);
            assert_eq!(serialized.stable_row_count(), 2);
            assert_eq!(
                serialized.first_mutable_boundary_kind(),
                Some(LiveRowBoundaryKind::AssistantLabel)
            );
            assert!(app.transcript.messages[1].blocks.is_empty());
        }
        app.config.snapshot = Some(crate::agent::settings::SettingsSnapshot::test_value(
            "spinnerTipsEnabled",
            serde_json::json!(false),
        ));
        assert_eq!(line_texts(&serialize_live_rows(&mut app, 80)), ["User", "request", "Claude"]);
        app.config.snapshot = None;
        assert_eq!(line_texts(&serialize_live_rows(&mut app, 80)), ["User", "request", "Claude"]);
        app.session_runtime.runtime_session_state =
            Some(model::RuntimeSessionState::RequiresAction);
        assert_eq!(line_texts(&serialize_live_rows(&mut app, 80)), ["User", "request"]);
        app.session_runtime.runtime_session_state = Some(model::RuntimeSessionState::Running);
        app.pending_interaction_ids.push("permission".into());
        assert_eq!(line_texts(&serialize_live_rows(&mut app, 80)), ["User", "request"]);
        app.pending_interaction_ids.clear();
        assert_eq!(line_texts(&serialize_live_rows(&mut app, 80)), ["User", "request", "Claude"]);
        let label_id = HistoryOutputId::AssistantLabel(app.transcript.messages[1].id);
        let excluded = BTreeSet::from([label_id]);
        let serialized = serialize_live_rows_with_boundaries_excluding(&mut app, 80, &excluded);
        assert_eq!(line_texts(serialized.rows()), ["User", "request"]);
        app.status = AppStatus::Ready;
        assert_eq!(line_texts(&serialize_live_rows(&mut app, 80)), ["User", "request"]);
    }

    #[test]
    fn activity_never_enters_transcript_rows_or_history_boundaries() {
        let mut app = App::test_default();
        app.transcript.messages.push(assistant_message());
        app.bind_active_turn_assistant(0);
        app.status = AppStatus::Running;
        app.begin_turn_activity(std::time::Instant::now());
        for width in [32, 120, 32] {
            let serialized = serialize_all_rows_with_boundaries(&mut app, width);
            assert_eq!(line_texts(serialized.rows()), ["Claude"]);
            assert_eq!(serialized.stable_row_count(), 0);
            assert_eq!(serialized.segments().len(), 1);
            assert_eq!(serialized.segments()[0].kind, LiveRowBoundaryKind::AssistantLabel);
            app.turn.compaction.begin();
            assert!(app.transcript.messages[0].blocks.is_empty());
        }
    }

    #[test]
    fn sdk_thinking_renders_separately_for_empty_assistant_and_clears_on_work_or_wait() {
        let mut app = App::test_default();
        app.session_runtime.session_id = Some("thinking-render".into());
        app.transcript.messages.push(assistant_message());
        app.bind_active_turn_assistant(0);
        app.status = AppStatus::Running;
        app.begin_turn_activity(std::time::Instant::now());
        app.config.snapshot = Some(crate::agent::settings::SettingsSnapshot::test_value(
            "prefersReducedMotion",
            serde_json::json!(true),
        ));
        let verb = app.activity_presentation(std::time::Instant::now()).expect("active").text();
        for phase in [
            model::AgentActivityPhase::Thinking,
            model::AgentActivityPhase::Working,
            model::AgentActivityPhase::Thinking,
        ] {
            crate::app::handle_client_event(
                &mut app,
                crate::agent::events::ClientEvent::SessionUpdate {
                    session_id: "thinking-render".into(),
                    update: model::SessionUpdate::AgentActivityUpdate(phase),
                },
            );
            for width in [32, 120, 32] {
                let serialized = serialize_all_rows_with_boundaries(&mut app, width);
                if phase == model::AgentActivityPhase::Thinking {
                    assert_eq!(line_texts(serialized.rows()), ["Claude", "◆ Thinking…"]);
                    assert_eq!(serialized.stable_row_count(), 0);
                    assert_eq!(serialized.rows_excluding_ids(&BTreeSet::new()), serialized.rows());
                    assert!(serialized.segments().iter().all(|segment| !segment.commit_ready));
                } else {
                    assert_eq!(line_texts(serialized.rows()), ["Claude"]);
                }
                let activity = crate::ui::activity_rows::build_activity_rows(
                    &app,
                    width,
                    std::time::Instant::now(),
                );
                assert!(line_text(&activity[0]).contains(&verb));
                assert!(line_text(&activity[1]).contains("Tip:"));
                assert!(app.transcript.messages[0].blocks.is_empty());
            }
        }
        app.config.snapshot.as_mut().expect("settings").values.extend(
            crate::agent::settings::SettingsSnapshot::test_value(
                "spinnerTipsEnabled",
                serde_json::json!(false),
            )
            .values,
        );
        let activity =
            crate::ui::activity_rows::build_activity_rows(&app, 120, std::time::Instant::now());
        assert_eq!(activity.len(), 1);
        assert!(line_text(&activity[0]).contains(&verb));
        assert_eq!(line_texts(&serialize_live_rows(&mut app, 120)), ["Claude", "◆ Thinking…"]);
        app.pending_interaction_ids.push("permission".into());
        assert!(serialize_live_rows(&mut app, 120).is_empty());
        app.pending_interaction_ids.clear();
        app.turn.cancel_requested = true;
        assert_eq!(line_texts(&serialize_live_rows(&mut app, 120)), ["Claude"]);
        app.turn.cancel_requested = false;
        app.turn.compaction.begin();
        assert_eq!(line_texts(&serialize_live_rows(&mut app, 120)), ["Claude"]);
        app.turn.reset_for_turn_exit();
        app.status = AppStatus::Ready;
        assert!(serialize_live_rows(&mut app, 120).is_empty());
    }

    #[test]
    fn disabled_tips_keep_activity_and_real_output_without_duplicate_headings() {
        let session_update = |app: &mut App, update| {
            crate::app::handle_client_event(
                app,
                crate::agent::events::ClientEvent::SessionUpdate {
                    session_id: "heading-output".into(),
                    update,
                },
            );
        };
        let mut app = App::test_default();
        app.session_runtime.session_id = Some("heading-output".into());
        app.transcript.messages.push(assistant_message());
        app.transcript.messages.push(assistant_message());
        app.bind_active_turn_assistant(0);
        app.status = AppStatus::Running;
        app.begin_turn_activity(std::time::Instant::now());
        app.config.snapshot = Some(crate::agent::settings::SettingsSnapshot::test_value(
            "spinnerTipsEnabled",
            serde_json::json!(false),
        ));
        assert_eq!(line_texts(&serialize_live_rows(&mut app, 80)), ["Claude"]);
        for update in [
            model::SessionUpdate::AgentActivityUpdate(model::AgentActivityPhase::Thinking),
            model::SessionUpdate::AgentActivityUpdate(model::AgentActivityPhase::Working),
        ] {
            session_update(&mut app, update);
            let rows = line_texts(&serialize_live_rows(&mut app, 80));
            assert_eq!(rows.iter().filter(|row| row.as_str() == "Claude").count(), 1);
            assert_eq!(
                rows.iter().any(|row| row.contains("Thinking…")),
                app.status == AppStatus::Thinking
            );
        }
        session_update(
            &mut app,
            model::SessionUpdate::ToolCall(
                model::ToolCall::new("first-tool", "Read")
                    .status(model::ToolCallStatus::InProgress),
            ),
        );
        let tool_rows = line_texts(&serialize_live_rows(&mut app, 80));
        assert_eq!(tool_rows.iter().filter(|row| row.as_str() == "Claude").count(), 1);
        assert!(tool_rows.iter().any(|row| row.contains("Read")));
        session_update(
            &mut app,
            model::SessionUpdate::ToolCallUpdate(model::ToolCallUpdate::new(
                "first-tool",
                model::ToolCallUpdateFields::new().status(model::ToolCallStatus::Completed),
            )),
        );
        session_update(
            &mut app,
            model::SessionUpdate::AgentMessageChunk(model::ContentChunk::new(
                model::ContentBlock::Text(model::TextContent::new("Actual response")),
            )),
        );
        let serialized = serialize_all_rows_with_boundaries(&mut app, 80);
        let output = line_texts(serialized.rows());
        assert_eq!(output.iter().filter(|row| row.as_str() == "Claude").count(), 1);
        assert!(output.iter().any(|row| row == "Actual response"));
        let excluded =
            BTreeSet::from([HistoryOutputId::AssistantLabel(app.transcript.messages[0].id)]);
        app.config.snapshot = None;
        let after_enable = serialize_live_rows_with_boundaries_excluding(&mut app, 80, &excluded);
        assert!(!line_texts(after_enable.rows()).iter().any(|row| row == "Claude"));
        assert!(line_texts(after_enable.rows()).iter().any(|row| row == "Actual response"));
        app.config.snapshot = Some(crate::agent::settings::SettingsSnapshot::test_value(
            "spinnerTipsEnabled",
            serde_json::json!(false),
        ));
        crate::app::handle_client_event(
            &mut app,
            crate::agent::events::ClientEvent::TurnComplete {
                session_id: "heading-output".into(),
                queued_turn_count: Some(0),
                terminal_reason: None,
            },
        );
        let completed = line_texts(&serialize_live_rows(&mut app, 80));
        assert_eq!(completed.iter().filter(|row| row.as_str() == "Claude").count(), 1);
        assert!(completed.iter().any(|row| row == "Actual response"));
        session_update(
            &mut app,
            model::SessionUpdate::AgentActivityUpdate(model::AgentActivityPhase::Thinking),
        );
        assert_eq!(line_texts(&serialize_live_rows(&mut app, 80)), completed);
        assert!(
            crate::ui::activity_rows::build_activity_rows(&app, 80, std::time::Instant::now())
                .is_empty()
        );
    }

    #[test]
    fn thinking_keeps_completed_body_committable_and_survives_committed_body_exclusion() {
        let mut app = App::test_default();
        app.transcript
            .messages
            .push(assistant_blocks_message(vec![tool_call_block("done", false)]));
        app.bind_active_turn_assistant(0);
        app.status = AppStatus::Thinking;
        let serialized = serialize_all_rows_with_boundaries(&mut app, 120);
        let stable = &serialized.rows()[..serialized.stable_row_count()];
        assert!(line_texts(stable).iter().any(|row| row.contains("Child Tool")));
        assert!(!line_texts(stable).iter().any(|row| row.contains("Thinking")));
        assert_eq!(
            serialized.first_mutable_boundary_kind(),
            Some(LiveRowBoundaryKind::AssistantThinking)
        );
        let excluded: BTreeSet<_> = serialized
            .segments()
            .iter()
            .filter(|segment| segment.commit_ready)
            .flat_map(|segment| segment.ids.iter().cloned())
            .collect();
        let remaining = serialize_live_rows_with_boundaries_excluding(&mut app, 120, &excluded);
        assert_eq!(remaining.stable_row_count(), 0);
        assert!(line_texts(remaining.rows()).iter().any(|row| row.contains("Thinking…")));
        assert!(!line_texts(remaining.rows()).iter().any(|row| row.contains("Child Tool")));
        app.status = AppStatus::Running;
        assert!(
            serialize_live_rows_with_boundaries_excluding(&mut app, 120, &excluded)
                .rows()
                .is_empty()
        );
        app.status = AppStatus::Ready;
        let completed = serialize_all_rows_with_boundaries(&mut app, 120);
        assert_eq!(completed.stable_row_count(), completed.rows().len());
        assert!(!line_texts(completed.rows()).iter().any(|row| row.contains("Thinking")));
    }

    #[test]
    fn live_rows_keep_system_row_after_active_assistant_turn() {
        let mut app = App::test_default();
        app.transcript.messages.push(assistant_text_message("streaming"));
        app.push_message_tracked(system_text_message("during turn"));

        let rows = serialize_live_rows(&mut app, 120);
        let text = line_texts(&rows);
        let assistant_pos = text.iter().position(|line| line == "streaming").expect("assistant");
        let system_pos = text.iter().position(|line| line == "during turn").expect("system");

        assert!(assistant_pos < system_pos);
    }

    #[test]
    fn live_rows_render_welcome_once() {
        let mut app = App::test_default();
        app.transcript.messages.push(ChatMessage::welcome(
            "1.2.3",
            "Pro",
            "/workspace/demo",
            "session-123",
        ));

        let rows = serialize_live_rows(&mut app, 120);
        let text = line_texts(&rows);

        assert_eq!(text.iter().filter(|line| line.as_str() == "Overview").count(), 1);
    }

    #[test]
    fn live_welcome_tip_wraps_below_its_value() {
        let mut message = ChatMessage::welcome("1.2.3", "Pro", "/workspace/demo", "session-123");
        let Some(MessageBlock::Welcome(welcome)) = message.blocks.first_mut() else {
            panic!("expected welcome block");
        };
        welcome.tip_seed = 7;
        let mut app = App::test_default();
        app.transcript.messages.push(message);

        let text = line_texts(&serialize_live_rows(&mut app, 110));
        let tip_row = text.iter().position(|line| line.contains("Tips: Start")).expect("tip row");

        assert_eq!(text[tip_row].find("Tips:"), Some(20));
        assert_eq!(text[tip_row + 1].find("noise"), Some(26));
    }

    #[test]
    fn welcome_renders_once_across_repeated_width_rebuilds() {
        let mut app = App::test_default();
        app.transcript.messages.push(ChatMessage::welcome(
            "1.2.3",
            "Pro",
            "/workspace/demo",
            "session-123",
        ));

        for width in [36, 120, 36] {
            let rows = serialize_live_rows(&mut app, width);
            let text = line_texts(&rows);

            assert_eq!(
                text.iter().filter(|line| line.as_str() == "Overview").count(),
                1,
                "welcome overview duplicated at width {width}: {text:?}"
            );
            assert_eq!(
                text.iter().filter(|line| line.contains("Version:")).count(),
                1,
                "welcome version row duplicated at width {width}: {text:?}"
            );
            assert_eq!(
                text.iter().filter(|line| line.contains("Subscription:")).count(),
                1,
                "welcome subscription row duplicated at width {width}: {text:?}"
            );
            assert_eq!(
                text.iter().filter(|line| line.contains("Session ID:")).count(),
                1,
                "welcome session row duplicated at width {width}: {text:?}"
            );
        }
    }

    #[test]
    fn finalized_welcome_rows_are_commit_ready() {
        let mut app = App::test_default();
        app.transcript.messages.push(ChatMessage::welcome(
            "1.2.3",
            "Pro",
            "/workspace/demo",
            "session-123",
        ));

        let serialized = serialize_all_rows_with_boundaries(&mut app, 120);

        assert!(!serialized.rows().is_empty());
        assert_eq!(serialized.stable_row_count(), serialized.rows().len());
    }

    #[test]
    fn live_rows_render_uncommitted_loading_welcome() {
        let mut app = App::test_default();
        app.status = AppStatus::Connecting;
        app.transcript.messages.push(ChatMessage::welcome("1.2.3", "-", "/workspace/demo", "-"));

        let rows = serialize_live_rows(&mut app, 120);
        let text = line_texts(&rows);

        assert_eq!(text.iter().filter(|line| line.as_str() == "Overview").count(), 1);
        assert!(text.iter().any(|line| line.contains("_~^~^~_")));
        assert!(text.iter().any(|line| line.contains("Subscription: Connecting")));
        assert!(text.iter().any(|line| line.contains("Session ID: Connecting")));
    }

    #[test]
    fn loading_welcome_rows_are_not_commit_ready() {
        let mut app = App::test_default();
        app.status = AppStatus::Connecting;
        app.transcript.messages.push(ChatMessage::welcome("1.2.3", "-", "/workspace/demo", "-"));

        let serialized = serialize_all_rows_with_boundaries(&mut app, 120);

        assert!(!serialized.rows().is_empty());
        assert_eq!(serialized.stable_row_count(), 0);
    }

    #[test]
    fn loading_welcome_blocks_later_stable_rows_from_scrollback() {
        let mut app = App::test_default();
        app.status = AppStatus::Connecting;
        app.transcript.messages.push(ChatMessage::welcome("1.2.3", "-", "/workspace/demo", "-"));
        app.push_message_tracked(user_text_message("queued while connecting"));

        let serialized = serialize_all_rows_with_boundaries(&mut app, 120);

        assert!(line_texts(serialized.rows()).iter().any(|line| line == "queued while connecting"));
        assert_eq!(serialized.stable_row_count(), 0);
    }

    #[test]
    fn hidden_canonical_tool_renders_no_rows_or_label() {
        let mut app = App::test_default();
        app.transcript
            .messages
            .push(assistant_blocks_message(vec![tool_call_block("child-1", true)]));

        for width in [32, 120, 32] {
            let rows = serialize_live_rows(&mut app, width);

            assert!(rows.is_empty(), "hidden tool rendered rows at width {width}");
        }
    }

    #[test]
    fn hidden_canonical_tool_with_focused_permission_without_subagent_context_has_no_header() {
        let mut app = App::test_default();
        app.transcript.messages.push(assistant_blocks_message(vec![
            tool_call_block_with_interaction("child-1", true, true, false),
        ]));

        for width in [32, 120, 32] {
            let rows = serialize_live_rows(&mut app, width);
            let text = line_texts(&rows);

            assert!(text.iter().any(|line| line == "Claude"), "missing label at width {width}");
            assert!(
                text.iter().any(|line| line.contains("Child Tool")),
                "missing tool title at width {width}: {text:?}"
            );
            assert!(
                !text.iter().any(|line| line.contains("Subagent:")),
                "unexpected subagent header at width {width}: {text:?}"
            );
            assert!(
                !text.iter().any(|line| line.contains("Subagent ·")),
                "unexpected subagent title prefix at width {width}: {text:?}"
            );
        }
    }

    #[test]
    fn hidden_canonical_tool_with_unfocused_permission_renders_interaction_rows() {
        let mut app = App::test_default();
        app.transcript.messages.push(assistant_blocks_message(vec![
            tool_call_block_with_pending_permission("child-1", true, false),
        ]));

        for width in [32, 120, 32] {
            let rows = serialize_live_rows(&mut app, width);
            let text = line_texts(&rows);

            assert!(text.iter().any(|line| line == "Claude"), "missing label at width {width}");
            assert!(
                text.iter().any(|line| line.contains("Child Tool")),
                "missing tool title at width {width}: {text:?}"
            );
            assert!(
                text.iter().any(|line| line.contains("Waiting for input")),
                "missing waiting row at width {width}: {text:?}"
            );
            assert!(
                !text.iter().any(|line| line.contains("Subagent:")),
                "unexpected subagent header at width {width}: {text:?}"
            );
            assert!(
                !text.iter().any(|line| line.contains("Subagent ·")),
                "unexpected subagent title prefix at width {width}: {text:?}"
            );
        }
    }

    #[test]
    fn hidden_canonical_tool_with_subagent_permission_prefixes_child_tool_title() {
        let mut app = App::test_default();
        app.transcript.messages.push(assistant_blocks_message(vec![
            tool_call_block_with_subagent_pending_permission("child-1"),
        ]));

        for width in [80, 120, 80] {
            let rows = serialize_live_rows(&mut app, width);
            let text = line_texts(&rows);

            let child_idx = text
                .iter()
                .position(|line| line.contains("Subagent ·") && line.contains("Child Tool"))
                .expect("missing prefixed child tool line");
            assert!(
                !text[child_idx].contains("general-purpose"),
                "subagent label should not render at width {width}: {text:?}"
            );
            let permission_idx = text
                .iter()
                .position(|line| line.contains("Allow once"))
                .expect("missing permission options");

            assert!(child_idx < permission_idx, "permission rendered before child: {text:?}");
            assert_eq!(
                text.iter().filter(|line| line.contains("Subagent ·")).count(),
                1,
                "expected one prefixed subagent title at width {width}: {text:?}"
            );
            assert!(!text.iter().any(|line| line.contains("Subagent:")));
            assert!(!text.iter().any(|line| line.contains("[model:")));
            assert!(!text.iter().any(|line| line.contains("Agent:")));
            assert!(!text.iter().any(|line| line.contains("general-purpose")));
        }
    }

    #[test]
    fn hidden_canonical_tool_with_focused_question_renders_interaction_rows() {
        let mut app = App::test_default();
        app.transcript.messages.push(assistant_blocks_message(vec![
            tool_call_block_with_interaction("child-1", true, false, true),
        ]));

        for width in [32, 120, 32] {
            let rows = serialize_live_rows(&mut app, width);
            let text = line_texts(&rows);

            assert!(text.iter().any(|line| line == "Claude"), "missing label at width {width}");
            assert!(
                text.iter().any(|line| line.contains("Child Tool")),
                "missing tool title at width {width}: {text:?}"
            );
        }
    }
}
