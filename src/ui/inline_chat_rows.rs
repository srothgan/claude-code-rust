// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use crate::agent::model;
use crate::app::{
    App, AppStatus, ChatMessage, ChatMessageId, HistoryOutputId, MessageBlock, MessageRole,
    NoticeBlock, SystemSeverity, TextBlock, TextBlockSpacing, ToolCallInfo, WelcomeBlock,
};
use crate::ui::message::{MessageRenderContext, SpinnerState, render_text_block_cached};
use crate::ui::message_rows::{MessageRowSegment, build_user_system_message_rows};
use crate::ui::spinner_verbs::random_spinner_verb;
use crate::ui::theme;
use crate::ui::tool_call;
use crate::ui::welcome;
use crate::ui::wrap::wrap_lines_to_physical_rows;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AssistantRuntimeIndicator {
    Thinking { verb: &'static str },
    Compacting,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SerializedLiveRows {
    rows: Vec<Line<'static>>,
    segments: Vec<LiveRowSegment>,
}

impl SerializedLiveRows {
    pub(crate) fn rows(&self) -> &[Line<'static>] {
        &self.rows
    }

    pub(crate) fn segments(&self) -> &[LiveRowSegment] {
        &self.segments
    }

    pub(crate) fn rows_excluding_ids(
        &self,
        excluded_ids: &BTreeSet<HistoryOutputId>,
    ) -> Vec<Line<'static>> {
        let mut rows = Vec::new();
        for segment in &self.segments {
            if segment.ids.iter().all(|id| excluded_ids.contains(id)) {
                continue;
            }
            rows.extend(self.rows[segment.start_row..segment.end_row].iter().cloned());
        }
        rows
    }

    pub(crate) fn stable_row_count(&self) -> usize {
        self.segments
            .iter()
            .find(|segment| !segment.commit_ready)
            .map_or(self.rows.len(), |segment| segment.start_row)
    }

    pub(crate) fn first_mutable_boundary_kind(&self) -> Option<LiveRowBoundaryKind> {
        self.segments.iter().find(|segment| !segment.commit_ready).map(|segment| segment.kind)
    }

    pub(crate) fn first_mutable_boundary_start(&self) -> Option<usize> {
        self.segments.iter().find(|segment| !segment.commit_ready).map(|segment| segment.start_row)
    }

    pub(crate) fn first_mutable_boundary_msg_idx(&self) -> Option<usize> {
        self.segments.iter().find(|segment| !segment.commit_ready).map(|segment| segment.msg_idx)
    }

    pub(crate) fn first_mutable_boundary_block_idx(&self) -> Option<usize> {
        self.segments
            .iter()
            .find(|segment| !segment.commit_ready)
            .and_then(|segment| segment.block_idx)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiveRowSegment {
    pub(crate) ids: Vec<HistoryOutputId>,
    pub(crate) msg_idx: usize,
    pub(crate) block_idx: Option<usize>,
    pub(crate) kind: LiveRowBoundaryKind,
    pub(crate) start_row: usize,
    pub(crate) end_row: usize,
    pub(crate) commit_ready: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LiveRowBoundary {
    ids: Vec<HistoryOutputId>,
    msg_idx: usize,
    block_idx: Option<usize>,
    kind: LiveRowBoundaryKind,
    start_row: usize,
    commit_ready: bool,
}

impl LiveRowBoundary {
    fn shifted(mut self, offset: usize) -> Self {
        self.start_row = self.start_row.saturating_add(offset);
        self
    }

    fn into_segment(self, end_row: usize) -> Option<LiveRowSegment> {
        (self.start_row < end_row).then_some(LiveRowSegment {
            ids: self.ids,
            msg_idx: self.msg_idx,
            block_idx: self.block_idx,
            kind: self.kind,
            start_row: self.start_row,
            end_row,
            commit_ready: self.commit_ready,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LiveRowBoundaryKind {
    Message,
    AssistantLabel,
    AssistantText,
    AssistantNotice,
    AssistantTool,
    AssistantIndicator,
}

pub(crate) fn serialize_live_rows_with_boundaries_excluding(
    app: &mut App,
    width: u16,
    excluded_ids: &BTreeSet<HistoryOutputId>,
) -> SerializedLiveRows {
    let current_mode_id = app.mode.as_ref().map(|mode| mode.current_mode_id.clone());
    let active_msg_idx = app.active_turn_assistant_idx();
    let runtime_indicator = sync_runtime_indicator(app);
    let mut rows = Vec::new();
    let mut row_boundaries = Vec::new();
    let mut previous_block_kind = None;

    for msg_idx in 0..app.messages.len() {
        let role = app.messages[msg_idx].role.clone();
        let block_kind = message_block_kind(&role);
        let rendered_message = render_live_message_rows(
            app,
            msg_idx,
            &role,
            LiveRowsRenderContext {
                current_mode_id: current_mode_id.as_deref(),
                active_msg_idx,
                runtime_indicator,
                width,
                excluded_ids,
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

    let segments = live_boundaries_to_segments(row_boundaries, rows.len());
    SerializedLiveRows { rows, segments }
}

#[derive(Clone, Copy)]
struct LiveRowsRenderContext<'a> {
    current_mode_id: Option<&'a str>,
    active_msg_idx: Option<usize>,
    runtime_indicator: Option<AssistantRuntimeIndicator>,
    width: u16,
    excluded_ids: &'a BTreeSet<HistoryOutputId>,
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
            context.active_msg_idx,
            context.runtime_indicator,
            context.width,
            context.excluded_ids,
        ),
    }
}

fn render_welcome_live_rows(
    app: &App,
    msg_idx: usize,
    width: u16,
    excluded_ids: &BTreeSet<HistoryOutputId>,
) -> RenderedMessageRows {
    let ids = welcome_output_ids(&app.messages[msg_idx]);
    let commit_ready = message_commit_ready(&app.messages[msg_idx]);
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
    let ids = vec![HistoryOutputId::Message(app.messages[msg_idx].id)];
    if ids_are_excluded(&ids, excluded_ids) {
        return RenderedMessageRows::skipped_transcript_content();
    }

    let rendered = build_user_system_message_rows(
        &mut app.messages[msg_idx],
        message_render_context(current_mode_id, width),
    );
    RenderedMessageRows::message(
        segments_to_physical_rows(&rendered.segments, width, false),
        LiveRowBoundary {
            ids,
            msg_idx,
            block_idx: None,
            kind: LiveRowBoundaryKind::Message,
            start_row: 0,
            commit_ready: true,
        },
    )
}

fn render_assistant_live_rows(
    app: &mut App,
    msg_idx: usize,
    current_mode_id: Option<&str>,
    active_msg_idx: Option<usize>,
    runtime_indicator: Option<AssistantRuntimeIndicator>,
    width: u16,
    excluded_ids: &BTreeSet<HistoryOutputId>,
) -> RenderedMessageRows {
    let active_mutable = active_assistant_message_is_mutable(app, msg_idx);
    let items =
        assistant_render_items_from_message(&app.messages[msg_idx], msg_idx, active_mutable);
    let selection = select_unexcluded_assistant_items(items, excluded_ids);
    let indicator = assistant_runtime_indicator(msg_idx, active_msg_idx, runtime_indicator);
    if selection.items.is_empty() && indicator.is_none() {
        return if selection.had_body_content {
            RenderedMessageRows::skipped_transcript_content()
        } else {
            RenderedMessageRows::empty()
        };
    }

    let message_id = app.messages[msg_idx].id;
    let label_ids = vec![HistoryOutputId::AssistantLabel(message_id)];
    let show_label = !ids_are_excluded(&label_ids, excluded_ids);
    let skipped_static_body = selection.skipped_body_before_rendered_content;
    let spinner = spinner_state_for_live(app.spinner_frame);
    let rendered = render_assistant_rows(AssistantRowsRequest {
        app: Some(app),
        message_id,
        msg_idx,
        items: selection.items,
        indicator,
        current_mode_id,
        width,
        spinner,
        show_label,
        leading_blank_lines: 0,
        has_prior_assistant_content: skipped_static_body,
    });
    tracing::debug!(
        target: crate::logging::targets::APP_RENDER,
        event_name = "inline_chat_assistant_block_built",
        message = "assistant message block rendered from canonical app.messages",
        outcome = "success",
        assistant_turn_id = tracing::field::Empty,
        show_label,
        leading_blank_lines = 0,
        skipped_static_body,
        committed_rendered_rows = rendered.rows.len(),
        live_rendered_rows = 0,
        indicator = ?indicator,
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

fn ids_are_excluded(ids: &[HistoryOutputId], excluded_ids: &BTreeSet<HistoryOutputId>) -> bool {
    !ids.is_empty() && ids.iter().all(|id| excluded_ids.contains(id))
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

fn live_boundaries_to_segments(
    mut boundaries: Vec<LiveRowBoundary>,
    row_count: usize,
) -> Vec<LiveRowSegment> {
    boundaries.sort_by_key(|boundary| boundary.start_row);
    let mut segments = Vec::with_capacity(boundaries.len());
    for idx in 0..boundaries.len() {
        let end_row =
            boundaries.get(idx + 1).map_or(row_count, |next| next.start_row).min(row_count);
        if let Some(segment) = boundaries[idx].clone().into_segment(end_row) {
            segments.push(segment);
        }
    }
    segments
}

fn welcome_output_ids(message: &ChatMessage) -> Vec<HistoryOutputId> {
    message
        .blocks
        .iter()
        .find_map(|block| match block {
            MessageBlock::Welcome(welcome) => Some(vec![HistoryOutputId::Block(welcome.id)]),
            MessageBlock::Text(_)
            | MessageBlock::Notice(_)
            | MessageBlock::ToolCall(_)
            | MessageBlock::ImageAttachment(_) => None,
        })
        .unwrap_or_else(|| vec![HistoryOutputId::Message(message.id)])
}

fn serialize_welcome_message(app: &App, msg_idx: usize, width: u16) -> Vec<Line<'static>> {
    if !app.show_session_overview {
        return Vec::new();
    }
    let Some(message) = app.messages.get(msg_idx) else {
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
            | MessageBlock::Notice(_)
            | MessageBlock::ToolCall(_)
            | MessageBlock::ImageAttachment(_) => None,
        })
        .is_some_and(|welcome| {
            welcome_value_ready(&welcome.subscription) && welcome_value_ready(&welcome.session_id)
        })
}

fn welcome_value_ready(value: &str) -> bool {
    !value.trim().is_empty() && value != "-"
}

fn sync_runtime_indicator(app: &mut App) -> Option<AssistantRuntimeIndicator> {
    if app.is_compacting {
        app.chat_render.thinking_verb = None;
        return Some(AssistantRuntimeIndicator::Compacting);
    }

    let thinking = matches!(app.status, crate::app::AppStatus::Thinking)
        || (matches!(app.status, crate::app::AppStatus::Running)
            && app
                .active_turn_assistant_idx()
                .and_then(|idx| app.messages.get(idx))
                .is_some_and(|msg| msg.blocks.is_empty()));

    if thinking {
        let verb = app.chat_render.thinking_verb.get_or_insert_with(random_spinner_verb);
        return Some(AssistantRuntimeIndicator::Thinking { verb });
    }

    app.chat_render.thinking_verb = None;
    None
}

fn assistant_runtime_indicator(
    msg_idx: usize,
    active_msg_idx: Option<usize>,
    runtime_indicator: Option<AssistantRuntimeIndicator>,
) -> Option<AssistantRuntimeIndicator> {
    (active_msg_idx == Some(msg_idx)).then_some(runtime_indicator).flatten()
}

fn serialize_compact_welcome_entry(
    app: &App,
    entry: &WelcomeBlock,
    width: u16,
) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(Span::styled(
        "Overview",
        Style::default().fg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD),
    ))];
    lines.extend(welcome::overview_lines(entry, Some(status_label(app))));

    wrap_lines_to_physical_rows(&lines, width)
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
        && (app.is_compacting || matches!(app.status, AppStatus::Thinking | AppStatus::Running))
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

    fn append(&mut self, id: HistoryOutputId, text: &str, trailing_spacing: TextBlockSpacing) {
        self.ids.push(id);
        append_text_run(&mut self.text, self.trailing_spacing, text);
        self.trailing_spacing = trailing_spacing;
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

fn append_text_run(existing: &mut String, existing_spacing: TextBlockSpacing, text: &str) {
    if existing.is_empty() || text.is_empty() {
        existing.push_str(text);
        return;
    }

    if !text.starts_with('\n') {
        match existing_spacing {
            TextBlockSpacing::None if !existing.ends_with('\n') => existing.push('\n'),
            TextBlockSpacing::ParagraphBreak if !existing.ends_with("\n\n") => {
                if existing.ends_with('\n') {
                    existing.push('\n');
                } else {
                    existing.push_str("\n\n");
                }
            }
            TextBlockSpacing::None | TextBlockSpacing::ParagraphBreak => {}
        }
    }

    existing.push_str(text);
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
                let commit_ready = active_tail_block_idx != Some(block_idx);
                if let Some(pending) = pending_text.as_mut()
                    && pending.can_merge(commit_ready)
                {
                    pending.append(
                        HistoryOutputId::Block(text.id),
                        &text.text,
                        text.trailing_spacing,
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
                items.push(AssistantRenderItemSpec {
                    ids: vec![HistoryOutputId::Block(notice.id)],
                    msg_idx,
                    leading_blank_lines,
                    block_idx: Some(block_idx),
                    boundary_kind: LiveRowBoundaryKind::AssistantNotice,
                    commit_ready: active_tail_block_idx != Some(block_idx),
                    item: AssistantRenderItem::Notice(NoticeBlock {
                        id: notice.id,
                        severity: notice.severity,
                        text: TextBlock::from_complete(&notice.text.text)
                            .with_trailing_spacing(notice.text.trailing_spacing),
                        dedup_key: notice.dedup_key.clone(),
                    }),
                });
                previous_kind = Some(current_kind);
            }
            MessageBlock::ToolCall(tool) => {
                if tool.hidden_unless_focused_interaction() {
                    continue;
                }
                flush_pending_text_run(&mut pending_text, &mut items);
                let current_kind = AssistantInlineItemKind::Tool;
                let leading_blank_lines = leading_blank_lines_between(previous_kind, current_kind);
                items.push(AssistantRenderItemSpec {
                    ids: vec![HistoryOutputId::ToolCall(tool.id.clone())],
                    msg_idx,
                    leading_blank_lines,
                    block_idx: Some(block_idx),
                    boundary_kind: LiveRowBoundaryKind::AssistantTool,
                    commit_ready: tool_call_commit_ready(tool),
                    item: AssistantRenderItem::CanonicalTool { msg_idx, block_idx },
                });
                previous_kind = Some(current_kind);
            }
            MessageBlock::Welcome(_) | MessageBlock::ImageAttachment(_) => {}
        }
    }

    flush_pending_text_run(&mut pending_text, &mut items);
    items
}

fn last_visible_assistant_block_idx(message: &ChatMessage) -> Option<usize> {
    message.blocks.iter().enumerate().rev().find_map(|(block_idx, block)| match block {
        MessageBlock::Text(text) if !text.text.is_empty() => Some(block_idx),
        MessageBlock::Notice(_) => Some(block_idx),
        MessageBlock::ToolCall(tool) if !tool.hidden_unless_focused_interaction() => {
            Some(block_idx)
        }
        MessageBlock::Text(_)
        | MessageBlock::ToolCall(_)
        | MessageBlock::Welcome(_)
        | MessageBlock::ImageAttachment(_) => None,
    })
}

fn tool_call_commit_ready(tool: &ToolCallInfo) -> bool {
    matches!(
        tool.status,
        model::ToolCallStatus::Completed
            | model::ToolCallStatus::Failed
            | model::ToolCallStatus::Killed
    ) && tool.pending_permission.is_none()
        && tool.pending_question.is_none()
        && tool.terminal_id.is_none()
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

struct AssistantRowsRequest<'a> {
    app: Option<&'a mut App>,
    message_id: ChatMessageId,
    msg_idx: usize,
    items: Vec<AssistantRenderItemSpec>,
    indicator: Option<AssistantRuntimeIndicator>,
    current_mode_id: Option<&'a str>,
    width: u16,
    spinner: SpinnerState,
    show_label: bool,
    leading_blank_lines: usize,
    has_prior_assistant_content: bool,
}

fn render_assistant_rows(mut request: AssistantRowsRequest<'_>) -> RenderedMessageRows {
    if request.items.is_empty() && request.indicator.is_none() {
        return RenderedMessageRows::empty();
    }

    let render_context = message_render_context(request.current_mode_id, request.width);
    let mut rows = Vec::new();
    let mut boundaries = Vec::new();
    rows.extend(std::iter::repeat_with(Line::default).take(request.leading_blank_lines));
    append_assistant_label_rows(&mut rows, &mut boundaries, &request);

    let mut state = AssistantInlineLayoutState {
        has_body_content: request.has_prior_assistant_content,
        has_visible_content: request.has_prior_assistant_content,
    };

    for item in request.items {
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
                let trailing_gap = block.trailing_blank_lines();
                let rendered =
                    render_assistant_text_block(block, request.width, !state.has_visible_content);
                if !rendered.is_empty() {
                    let boundary_start = rows.len();
                    rows.extend(
                        std::iter::repeat_with(Line::default).take(item_leading_blank_lines),
                    );
                    push_assistant_boundary(&mut boundaries, boundary, boundary_start);
                    state.has_body_content = true;
                    state.has_visible_content = true;
                    rows.extend(rendered);
                    rows.extend(std::iter::repeat_with(Line::default).take(trailing_gap));
                }
            }
            AssistantRenderItem::Notice(block) => {
                let trailing_gap = block.trailing_blank_lines();
                let rendered =
                    render_assistant_notice_block(block, request.width, !state.has_visible_content);
                if !rendered.is_empty() {
                    let boundary_start = rows.len();
                    rows.extend(
                        std::iter::repeat_with(Line::default).take(item_leading_blank_lines),
                    );
                    push_assistant_boundary(&mut boundaries, boundary, boundary_start);
                    state.has_body_content = true;
                    state.has_visible_content = true;
                    rows.extend(rendered);
                    rows.extend(std::iter::repeat_with(Line::default).take(trailing_gap));
                }
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
                        render_context,
                        request.spinner,
                    ),
                );
            }
        }
    }

    append_assistant_indicator_rows(
        &mut rows,
        &mut boundaries,
        &state,
        AssistantIndicatorMeta {
            message_id: request.message_id,
            msg_idx: request.msg_idx,
            indicator: request.indicator,
            spinner: request.spinner,
            width: request.width,
        },
    );

    if !state.has_visible_content && request.indicator.is_none() {
        return RenderedMessageRows::empty();
    }

    RenderedMessageRows::rendered(trim_trailing_blank_rows(rows), boundaries)
}

fn append_assistant_label_rows(
    rows: &mut Vec<Line<'static>>,
    boundaries: &mut Vec<LiveRowBoundary>,
    request: &AssistantRowsRequest<'_>,
) {
    if !request.show_label {
        return;
    }

    let first_commit_ready = request.items.first().is_some_and(|item| item.commit_ready);
    let label_start = rows.len().saturating_sub(request.leading_blank_lines);
    boundaries.push(LiveRowBoundary {
        ids: vec![HistoryOutputId::AssistantLabel(request.message_id)],
        msg_idx: request.msg_idx,
        block_idx: None,
        kind: LiveRowBoundaryKind::AssistantLabel,
        start_row: label_start,
        commit_ready: first_commit_ready,
    });
    rows.extend(wrap_lines_to_physical_rows(&[assistant_role_label_line()], request.width));
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

#[derive(Clone, Copy)]
struct AssistantIndicatorMeta {
    message_id: ChatMessageId,
    msg_idx: usize,
    indicator: Option<AssistantRuntimeIndicator>,
    spinner: SpinnerState,
    width: u16,
}

fn append_assistant_indicator_rows(
    rows: &mut Vec<Line<'static>>,
    boundaries: &mut Vec<LiveRowBoundary>,
    state: &AssistantInlineLayoutState,
    meta: AssistantIndicatorMeta,
) {
    let line = match meta.indicator {
        Some(AssistantRuntimeIndicator::Compacting) => compacting_line(meta.spinner.frame),
        Some(AssistantRuntimeIndicator::Thinking { verb }) => {
            thinking_line(meta.spinner.frame, verb)
        }
        None => return,
    };
    let boundary_start = rows.len();
    if state.has_body_content {
        rows.push(Line::default());
    }
    boundaries.push(LiveRowBoundary {
        ids: vec![HistoryOutputId::AssistantIndicator(meta.message_id)],
        msg_idx: meta.msg_idx,
        block_idx: None,
        kind: LiveRowBoundaryKind::AssistantIndicator,
        start_row: boundary_start,
        commit_ready: false,
    });
    rows.extend(wrap_lines_to_physical_rows(&[line], meta.width));
}

fn spinner_state_for_live(frame: usize) -> SpinnerState {
    SpinnerState { frame }
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
    wrap_lines_to_physical_rows(&lines, width)
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

fn render_canonical_tool_rows(
    app: &mut App,
    msg_idx: usize,
    block_idx: usize,
    render_context: MessageRenderContext<'_>,
    spinner: SpinnerState,
) -> Vec<Line<'static>> {
    let Some(MessageBlock::ToolCall(tc)) =
        app.messages.get_mut(msg_idx).and_then(|message| message.blocks.get_mut(block_idx))
    else {
        return Vec::new();
    };
    if tc.hidden_unless_focused_interaction() {
        return Vec::new();
    }

    let mut rows = Vec::new();
    tool_call::render_tool_call_cached(
        tc.as_mut(),
        render_context.tool_render_context,
        render_context.width,
        spinner.frame,
        &mut rows,
    );
    wrap_lines_to_physical_rows(&rows, render_context.width)
}

fn assistant_role_label_line() -> Line<'static> {
    Line::from(vec![ratatui::text::Span::styled(
        "Claude",
        Style::default().fg(theme::ROLE_ASSISTANT).add_modifier(Modifier::BOLD),
    )])
}

fn thinking_line(frame: usize, verb: &str) -> Line<'static> {
    const SPINNER_FRAMES: &[char] = &[
        '\u{280B}', '\u{2819}', '\u{2839}', '\u{2838}', '\u{283C}', '\u{2834}', '\u{2826}',
        '\u{2827}', '\u{2807}', '\u{280F}',
    ];
    let ch = SPINNER_FRAMES[frame % SPINNER_FRAMES.len()];
    Line::from(ratatui::text::Span::styled(
        format!("{ch} {verb}..."),
        Style::default().fg(theme::DIM),
    ))
}

fn compacting_line(frame: usize) -> Line<'static> {
    const SPINNER_FRAMES: &[char] = &[
        '\u{280B}', '\u{2819}', '\u{2839}', '\u{2838}', '\u{283C}', '\u{2834}', '\u{2826}',
        '\u{2827}', '\u{2807}', '\u{280F}',
    ];
    let ch = SPINNER_FRAMES[frame % SPINNER_FRAMES.len()];
    Line::from(ratatui::text::Span::styled(
        format!("{ch} Compacting context..."),
        Style::default().fg(theme::RUST_ORANGE),
    ))
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
                rows.extend(wrap_lines_to_physical_rows(lines, width));
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
        LiveRowBoundaryKind, SerializedLiveRows, serialize_live_rows_with_boundaries_excluding,
        thinking_line,
    };
    use crate::agent::model;
    use crate::app::{
        App, AppStatus, BlockCache, ChatMessage, HistoryOutputId, MessageBlock, MessageRole,
        NoticeBlock, TerminalSnapshotMode, TextBlock, TextBlockSpacing, ToolCallInfo,
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

    #[test]
    fn thinking_line_uses_selected_verb() {
        let text = line_text(&thinking_line(0, "Pondering"));

        assert!(text.contains("Pondering..."));
        assert!(!text.contains("Thinking..."));
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
            title: "Child Tool".to_owned(),
            sdk_tool_name: "Bash".to_owned(),
            raw_input: None,
            raw_input_bytes: 0,
            output_metadata: None,
            task_metadata: None,
            status,
            content: Vec::new(),
            hidden,
            terminal_id: None,
            terminal_command: None,
            terminal_output: None,
            terminal_output_len: 0,
            terminal_bytes_seen: 0,
            terminal_snapshot_mode: TerminalSnapshotMode::AppendOnly,
            cache: BlockCache::default(),
            pending_permission: None,
            pending_question: None,
        };

        if focused_permission {
            let (response_tx, _response_rx) = tokio::sync::oneshot::channel();
            tool.pending_permission = Some(crate::app::InlinePermission {
                options: Vec::new(),
                display: None,
                response_tx,
                selected_index: 0,
                focused: true,
            });
        }

        if focused_question {
            let (response_tx, _response_rx) = tokio::sync::oneshot::channel();
            tool.pending_question = Some(crate::app::InlineQuestion {
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

    #[test]
    fn live_rows_do_not_start_with_synthetic_blank_row() {
        let mut app = App::test_default();
        app.messages.push(assistant_text_message("hi"));

        let rows = serialize_live_rows(&mut app, 120);

        assert!(rows.first().is_some_and(|line| !line_text(line).trim().is_empty()));
        assert_eq!(line_text(&rows[0]), "Claude");
    }

    #[test]
    fn live_rows_render_user_row_while_assistant_streams() {
        let mut app = App::test_default();
        app.push_message_tracked(user_text_message("hello"));
        app.messages.push(assistant_text_message("still streaming"));

        let rows = serialize_live_rows(&mut app, 120);
        let text = line_texts(&rows);

        assert_eq!(text, vec!["User", "hello", "Claude", "still streaming"]);
    }

    #[test]
    fn width_rebuild_wraps_same_canonical_transcript_to_different_row_counts() {
        let mut app = App::test_default();
        app.push_message_tracked(user_text_message(
            "Resize should rebuild canonical user prose from messages with enough words to wrap \
             differently at narrow widths.",
        ));
        app.messages.push(assistant_text_message(
            "Assistant rows also come directly from app.messages, so changing width changes \
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
        app.messages.push(assistant_text_message("still streaming"));
        app.bind_active_turn_assistant(1);
        app.status = AppStatus::Running;

        let serialized = serialize_all_rows_with_boundaries(&mut app, 120);

        assert_eq!(serialized.stable_row_count(), 2);
    }

    #[test]
    fn active_assistant_commits_completed_text_before_streaming_tail() {
        let mut app = App::test_default();
        app.messages.push(assistant_blocks_message(vec![
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
        app.messages.push(assistant_blocks_message(vec![
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
        app.messages.push(assistant_blocks_message(vec![tool_call_block("done-tool", false)]));
        app.bind_active_turn_assistant(0);
        app.status = AppStatus::Running;

        let serialized = serialize_all_rows_with_boundaries(&mut app, 120);

        assert_eq!(serialized.stable_row_count(), serialized.rows().len());
    }

    #[test]
    fn active_assistant_in_progress_tool_keeps_label_mutable() {
        let mut app = App::test_default();
        app.messages.push(assistant_blocks_message(vec![tool_call_block_with_status_interaction(
            "running-tool",
            model::ToolCallStatus::InProgress,
            false,
            false,
            false,
        )]));
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
    fn live_rows_render_committed_assistant_prefix_before_live_tail() {
        let mut app = App::test_default();
        app.messages.push(assistant_blocks_message(vec![
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
        app.messages.push(assistant_blocks_message(vec![
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
    fn live_assistant_text_preserves_single_newline_rows() {
        let mut app = App::test_default();
        app.messages.push(assistant_text_message("line 1: ready\nline 2: ready\nline 3: ready"));

        let rows = serialize_live_rows(&mut app, 120);

        assert_eq!(
            line_texts(&rows),
            vec!["Claude", "line 1: ready", "line 2: ready", "line 3: ready"]
        );
    }

    #[test]
    fn live_rows_render_assistant_notice_from_messages() {
        let mut app = App::test_default();
        app.messages.push(assistant_blocks_message(vec![MessageBlock::Notice(
            NoticeBlock::from_complete(crate::app::SystemSeverity::Warning, "watch this"),
        )]));

        let rows = serialize_live_rows(&mut app, 120);
        let text = line_texts(&rows);

        assert_eq!(text, vec!["Claude", "watch this"]);
    }

    #[test]
    fn live_rows_render_visible_tool_from_canonical_message_block() {
        let mut app = App::test_default();
        app.messages.push(assistant_blocks_message(vec![tool_call_block("tool-1", false)]));

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
        app.messages.push(message);
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
        app.messages.push(message);
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
    fn empty_active_assistant_renders_thinking_from_runtime_state() {
        let mut app = App::test_default();
        app.messages.push(assistant_message());
        app.bind_active_turn_assistant(0);
        app.status = AppStatus::Thinking;
        app.chat_render.thinking_verb = Some("Pondering");

        let rows = serialize_live_rows(&mut app, 120);
        let text = line_texts(&rows);

        assert_eq!(text.first().map(String::as_str), Some("Claude"));
        assert!(text.iter().any(|line| line.contains("Pondering...")));
    }

    #[test]
    fn thinking_remains_render_only_across_width_rebuilds() {
        let mut app = App::test_default();
        app.messages.push(assistant_message());
        app.bind_active_turn_assistant(0);
        app.status = AppStatus::Thinking;
        app.chat_render.thinking_verb = Some("Pondering");

        for width in [32, 120, 32] {
            let rows = serialize_live_rows(&mut app, width);
            let text = line_texts(&rows);

            assert_eq!(text.first().map(String::as_str), Some("Claude"));
            assert!(
                text.iter().any(|line| line.contains("Pondering...")),
                "thinking indicator missing at width {width}: {text:?}"
            );
            assert!(
                app.messages[0].blocks.is_empty(),
                "thinking indicator must not be persisted into app.messages"
            );
        }
    }

    #[test]
    fn live_rows_keep_system_row_after_active_assistant_turn() {
        let mut app = App::test_default();
        app.messages.push(assistant_text_message("streaming"));
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
        app.messages.push(ChatMessage::welcome("1.2.3", "Pro", "/workspace/demo", "session-123"));

        let rows = serialize_live_rows(&mut app, 120);
        let text = line_texts(&rows);

        assert_eq!(text.iter().filter(|line| line.as_str() == "Overview").count(), 1);
    }

    #[test]
    fn welcome_renders_once_across_repeated_width_rebuilds() {
        let mut app = App::test_default();
        app.messages.push(ChatMessage::welcome("1.2.3", "Pro", "/workspace/demo", "session-123"));

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
        app.messages.push(ChatMessage::welcome("1.2.3", "Pro", "/workspace/demo", "session-123"));

        let serialized = serialize_all_rows_with_boundaries(&mut app, 120);

        assert!(!serialized.rows().is_empty());
        assert_eq!(serialized.stable_row_count(), serialized.rows().len());
    }

    #[test]
    fn live_rows_render_uncommitted_loading_welcome() {
        let mut app = App::test_default();
        app.status = AppStatus::Connecting;
        app.messages.push(ChatMessage::welcome("1.2.3", "-", "/workspace/demo", "-"));

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
        app.messages.push(ChatMessage::welcome("1.2.3", "-", "/workspace/demo", "-"));

        let serialized = serialize_all_rows_with_boundaries(&mut app, 120);

        assert!(!serialized.rows().is_empty());
        assert_eq!(serialized.stable_row_count(), 0);
    }

    #[test]
    fn loading_welcome_blocks_later_stable_rows_from_scrollback() {
        let mut app = App::test_default();
        app.status = AppStatus::Connecting;
        app.messages.push(ChatMessage::welcome("1.2.3", "-", "/workspace/demo", "-"));
        app.push_message_tracked(user_text_message("queued while connecting"));

        let serialized = serialize_all_rows_with_boundaries(&mut app, 120);

        assert!(line_texts(serialized.rows()).iter().any(|line| line == "queued while connecting"));
        assert_eq!(serialized.stable_row_count(), 0);
    }

    #[test]
    fn hidden_canonical_tool_renders_no_rows_or_label() {
        let mut app = App::test_default();
        app.messages.push(assistant_blocks_message(vec![tool_call_block("child-1", true)]));

        for width in [32, 120, 32] {
            let rows = serialize_live_rows(&mut app, width);

            assert!(rows.is_empty(), "hidden tool rendered rows at width {width}");
        }
    }

    #[test]
    fn hidden_canonical_tool_with_focused_permission_renders_interaction_rows() {
        let mut app = App::test_default();
        app.messages.push(assistant_blocks_message(vec![tool_call_block_with_interaction(
            "child-1", true, true, false,
        )]));

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

    #[test]
    fn hidden_canonical_tool_with_focused_question_renders_interaction_rows() {
        let mut app = App::test_default();
        app.messages.push(assistant_blocks_message(vec![tool_call_block_with_interaction(
            "child-1", true, false, true,
        )]));

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
