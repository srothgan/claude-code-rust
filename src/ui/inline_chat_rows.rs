// SPDX-License-Identifier: Apache-2.0
use crate::app::{
    App, AppStatus, BtwExchangeBlock, ChatMessage, ChatMessageId, HistoryOutputId, MessageBlock,
    MessageRole, NoticeBlock, SystemSeverity, TextBlock, TextBlockSpacing, ToolCallInfo,
    WelcomeBlock,
};
use crate::ui::layout_control::LayoutControl;
#[cfg(test)]
pub(crate) use crate::ui::live_rows::segments::LiveRowSegment;
use crate::ui::live_rows::segments::{
    LiveRowBoundary, ids_are_excluded, live_boundaries_to_segments,
};
pub(crate) use crate::ui::live_rows::segments::{LiveRowBoundaryKind, SerializedLiveRows};
use crate::ui::message::{MessageRenderContext, SpinnerState};
use crate::ui::message_layout::{self, MessageLayout};
use crate::ui::message_rows::{
    MessageRowSegment, build_user_system_message_rows, render_btw_exchange_with_control,
};
use crate::ui::theme;
use crate::ui::tool_call;
use crate::ui::welcome;
use crate::ui::wrap::wrap_lines_to_physical_rows;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::collections::BTreeSet;
use std::time::Instant;

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
    control: Option<&LayoutControl<'_>>,
) -> SerializedLiveRows {
    let uninterrupted = LayoutControl::uninterrupted();
    serialize_live_rows_at(
        app,
        width,
        excluded_ids,
        Instant::now(),
        control.unwrap_or(&uninterrupted),
    )
}

/// Serializes the live rows as of `now`: one clock reading per pass, so the
/// welcome crab's pose and its commit decision agree, and the rows committed
/// to scrollback are exactly the rows this pass drew.
fn serialize_live_rows_at(
    app: &mut App,
    width: u16,
    excluded_ids: &BTreeSet<HistoryOutputId>,
    now: Instant,
    control: &LayoutControl<'_>,
) -> SerializedLiveRows {
    let mut cursor = LiveRowsCursor::new(now);
    let mut serialized = SerializedLiveRows::new(Vec::new(), Vec::new());
    while let Some(rows) = cursor.next(app, width, excluded_ids, control) {
        serialized.append(rows);
    }
    serialized
}

/// Resumable traversal. Completed messages can be inserted before later messages
/// are laid out; no transcript-sized row vector is required by replay.
pub(crate) struct LiveRowsCursor {
    next_message: usize,
    previous_block_kind: Option<TopLevelInlineBlockKind>,
    detached_done: bool,
    assistant: Option<Box<AssistantRowsCursor>>,
    now: Instant,
}

impl LiveRowsCursor {
    pub(crate) const fn is_finished(&self) -> bool {
        self.detached_done
    }

    pub(crate) fn new(now: Instant) -> Self {
        Self {
            next_message: 0,
            previous_block_kind: None,
            detached_done: false,
            assistant: None,
            now,
        }
    }

    pub(crate) fn next(
        &mut self,
        app: &mut App,
        width: u16,
        excluded_ids: &BTreeSet<HistoryOutputId>,
        control: &LayoutControl<'_>,
    ) -> Option<SerializedLiveRows> {
        if self.detached_done || control.cancelled() {
            return None;
        }
        let current_mode_id =
            app.session_runtime.mode.as_ref().map(|mode| mode.current_mode_id.clone());
        let mut rows = Vec::new();
        let mut row_boundaries = Vec::new();
        if self.next_message < app.transcript.messages.len() {
            let msg_idx = self.next_message;
            let role = app.transcript.messages[msg_idx].role.clone();
            let block_kind = message_block_kind(&role);
            let rendered_message = match role {
                MessageRole::Assistant => {
                    let assistant = self.assistant.get_or_insert_with(|| {
                        Box::new(AssistantRowsCursor::new(
                            app,
                            msg_idx,
                            excluded_ids,
                            self.previous_block_kind,
                            control,
                        ))
                    });
                    let rendered = assistant.next(app, width, current_mode_id.as_deref(), control);
                    if assistant.finished {
                        self.assistant = None;
                        self.next_message += 1;
                    }
                    rendered
                }
                MessageRole::Welcome => {
                    self.next_message += 1;
                    render_welcome_live_rows(app, msg_idx, width, excluded_ids, self.now)
                }
                MessageRole::User | MessageRole::System(_) => {
                    self.next_message += 1;
                    render_user_system_live_rows(
                        app,
                        msg_idx,
                        current_mode_id.as_deref(),
                        width,
                        excluded_ids,
                        control,
                    )
                }
            };

            append_rendered_live_message(
                &mut rows,
                &mut row_boundaries,
                &mut self.previous_block_kind,
                block_kind,
                rendered_message,
            );
        } else {
            append_detached_tool_interactions(
                app,
                width,
                current_mode_id.as_deref(),
                &mut rows,
                &mut row_boundaries,
            );
            self.detached_done = true;
        }
        if control.cancelled() {
            return None;
        }
        let segments = live_boundaries_to_segments(row_boundaries, rows.len());
        Some(SerializedLiveRows::new(rows, segments))
    }
}

fn render_welcome_live_rows(
    app: &App,
    msg_idx: usize,
    width: u16,
    excluded_ids: &BTreeSet<HistoryOutputId>,
    now: Instant,
) -> RenderedMessageRows {
    let ids = welcome_output_ids(&app.transcript.messages[msg_idx]);
    if ids_are_excluded(&ids, excluded_ids) {
        return RenderedMessageRows::skipped_transcript_content();
    }

    let commit_ready = welcome::welcome_block(&app.transcript.messages[msg_idx])
        .is_some_and(|welcome| welcome::welcome_commit_ready(app, welcome, width, now));
    RenderedMessageRows::message(
        serialize_welcome_message(app, msg_idx, width, now),
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
    control: &LayoutControl<'_>,
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
        MessageRenderContext::with_control(current_mode_id, width, control),
    );
    let is_user = matches!(app.transcript.messages[msg_idx].role, MessageRole::User);
    if is_user {
        if let Some(timestamp) = timestamp
            && let Some(MessageRowSegment::Lines { lines }) = rendered.segments.first_mut()
            && let Some(padding) = lines.first_mut()
        {
            *padding = message_layout::timestamp_row(&timestamp, width, Some(theme::USER_MSG_BG));
        }
    } else if let Some(timestamp) = timestamp
        && let Some(MessageRowSegment::Lines { lines }) = rendered.segments.first_mut()
        && let Some(label) = lines.first_mut()
    {
        label.spans.push(Span::styled(format!(" · {timestamp}"), Style::default().fg(theme::DIM)));
    }
    let rows = if is_user {
        // User cards already contain wrapped, framed physical rows.
        rendered
            .segments
            .into_iter()
            .flat_map(|segment| match segment {
                MessageRowSegment::Blank => vec![Line::default()],
                MessageRowSegment::Lines { lines } => lines,
            })
            .collect()
    } else {
        segments_to_physical_rows(&rendered.segments, width, control)
    };
    RenderedMessageRows::message(
        rows,
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

/// Owns only an active traversal's layout state. Source messages remain canonical in App.
#[allow(clippy::struct_excessive_bools)] // Independent presentation decisions for this traversal.
struct AssistantRowsCursor {
    msg_idx: usize,
    items: std::vec::IntoIter<AssistantRenderItemSpec>,
    active_item: Option<AssistantRenderItemSpec>,
    tool_cursor: tool_call::ToolRowsCursor,
    text_cursor: Option<super::markdown::DocumentCursor>,
    state: AssistantInlineLayoutState,
    label_visibility: AssistantLabelVisibility,
    show_thinking: bool,
    active_mutable: bool,
    include_duration: bool,
    include_padding: bool,
    finished: bool,
}

impl AssistantRowsCursor {
    fn new(
        app: &App,
        msg_idx: usize,
        excluded_ids: &BTreeSet<HistoryOutputId>,
        previous: Option<TopLevelInlineBlockKind>,
        control: &LayoutControl<'_>,
    ) -> Self {
        let active_mutable = active_assistant_message_is_mutable(app, msg_idx);
        let message = &app.transcript.messages[msg_idx];
        let selection = select_unexcluded_assistant_items(
            assistant_render_items_from_message(message, msg_idx, active_mutable, control),
            excluded_ids,
        );
        let prior = selection.skipped_body_before_rendered_content;
        Self {
            msg_idx,
            items: selection.items.into_iter(),
            active_item: None,
            tool_cursor: tool_call::ToolRowsCursor::default(),
            text_cursor: None,
            state: AssistantInlineLayoutState {
                has_body_content: prior,
                has_visible_content: prior,
                ..Default::default()
            },
            label_visibility: if previous == Some(TopLevelInlineBlockKind::Assistant)
                || excluded_ids.contains(&HistoryOutputId::AssistantLabel(message.id))
            {
                AssistantLabelVisibility::Hidden
            } else {
                AssistantLabelVisibility::WithContent
            },
            show_thinking: app.activity_presentation(Instant::now()).is_some_and(|p| {
                p.thinking && p.heading == crate::app::activity::ActivityHeading::Assistant(msg_idx)
            }),
            active_mutable,
            include_duration: !excluded_ids
                .contains(&HistoryOutputId::AssistantDuration(message.id)),
            include_padding: !excluded_ids.contains(&HistoryOutputId::AssistantPadding(message.id)),
            finished: false,
        }
    }

    fn next(
        &mut self,
        app: &mut App,
        width: u16,
        current_mode_id: Option<&str>,
        control: &LayoutControl<'_>,
    ) -> RenderedMessageRows {
        let started = Instant::now();
        let mut rows = Vec::new();
        let mut boundaries = Vec::new();
        let content_width = MessageLayout::new(width).content_width;
        let spinner = SpinnerState::for_app(app);
        if let Some(mut item) = self.active_item.take().or_else(|| self.items.next()) {
            let kind = item.boundary_kind;
            let block_index = item.block_idx;
            let finished = render_assistant_item(
                app,
                &item,
                width,
                current_mode_id,
                spinner,
                control,
                &mut self.state,
                &mut rows,
                &mut boundaries,
                &mut self.tool_cursor,
                &mut self.text_cursor,
            );
            if finished {
                self.tool_cursor = tool_call::ToolRowsCursor::default();
                self.text_cursor = None;
            } else {
                item.leading_blank_lines = 0;
                self.active_item = Some(item);
            }
            tracing::trace!(
                target: crate::logging::targets::APP_RENDER,
                event_name = "inline_chat_layout_item",
                message_index = self.msg_idx,
                block_index,
                kind = ?kind,
                rows = rows.len(),
                elapsed_us = started.elapsed().as_micros(),
                cancelled = control.cancelled(),
            );
        } else {
            self.finished = true;
            let message_id = app.transcript.messages[self.msg_idx].id;
            if self.show_thinking {
                append_assistant_thinking(
                    &mut rows,
                    &mut boundaries,
                    &mut self.state,
                    AssistantThinkingMeta {
                        message_id,
                        msg_idx: self.msg_idx,
                        width: content_width,
                        spinner,
                    },
                );
            }
            if (self.state.has_visible_content || !rows.is_empty())
                && self.include_duration
                && let Some(label) = crate::app::presentation::duration_label(
                    app,
                    &app.transcript.messages[self.msg_idx],
                )
            {
                boundaries.push(LiveRowBoundary {
                    ids: vec![HistoryOutputId::AssistantDuration(message_id)],
                    msg_idx: self.msg_idx,
                    block_idx: None,
                    kind: LiveRowBoundaryKind::AssistantDuration,
                    start_row: rows.len(),
                    commit_ready: !self.active_mutable,
                });
                rows.extend(wrap_lines_to_physical_rows(
                    &[Line::from(Span::styled(label, Style::default().fg(theme::DIM)))],
                    content_width,
                ));
            }
        }
        if control.cancelled() {
            return RenderedMessageRows::empty();
        }
        let mut rendered = RenderedMessageRows::rendered(rows, boundaries);
        if !rendered.rows.is_empty() {
            frame_assistant_rows(app, self.msg_idx, width, self.label_visibility, &mut rendered);
            self.label_visibility = AssistantLabelVisibility::Hidden;
        }
        rendered.had_transcript_content |= self.state.has_visible_content;
        if self.finished && rendered.had_transcript_content && self.include_padding {
            append_assistant_bottom_padding(
                &mut rendered,
                app.transcript.messages[self.msg_idx].id,
                self.msg_idx,
                width,
                !self.active_mutable,
            );
        }
        rendered
    }
}

fn append_assistant_bottom_padding(
    rendered: &mut RenderedMessageRows,
    message_id: ChatMessageId,
    msg_idx: usize,
    width: u16,
    commit_ready: bool,
) {
    // Keep the trailing gap live until completion, even after a tool or text prefix is committed.
    rendered.had_transcript_content = true;
    rendered.boundaries.push(LiveRowBoundary {
        ids: vec![HistoryOutputId::AssistantPadding(message_id)],
        msg_idx,
        block_idx: None,
        kind: LiveRowBoundaryKind::AssistantPadding,
        start_row: rendered.rows.len(),
        commit_ready,
    });
    rendered.rows.extend(
        std::iter::repeat_with(|| MessageLayout::new(width).row(Line::default(), None, None))
            .take(usize::from(message_layout::VERTICAL_PADDING)),
    );
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
}

fn select_unexcluded_assistant_items(
    items: Vec<AssistantRenderItemSpec>,
    excluded_ids: &BTreeSet<HistoryOutputId>,
) -> AssistantRenderSelection {
    let mut selected = Vec::with_capacity(items.len());
    let mut skipped_body_before_rendered_content = false;
    let mut rendered_body_seen = false;

    for item in items {
        if ids_are_excluded(&item.ids, excluded_ids) {
            if !rendered_body_seen {
                skipped_body_before_rendered_content = true;
            }
            continue;
        }

        rendered_body_seen = true;
        selected.push(item);
    }

    AssistantRenderSelection { items: selected, skipped_body_before_rendered_content }
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

fn serialize_welcome_message(
    app: &App,
    msg_idx: usize,
    width: u16,
    now: Instant,
) -> Vec<Line<'static>> {
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

    serialize_compact_welcome_entry(app, welcome, width, now)
}

const fn message_block_kind(role: &MessageRole) -> TopLevelInlineBlockKind {
    match role {
        MessageRole::Welcome => TopLevelInlineBlockKind::Welcome,
        MessageRole::User => TopLevelInlineBlockKind::User,
        MessageRole::System(_) => TopLevelInlineBlockKind::System,
        MessageRole::Assistant => TopLevelInlineBlockKind::Assistant,
    }
}

fn serialize_compact_welcome_entry(
    app: &App,
    entry: &WelcomeBlock,
    width: u16,
    now: Instant,
) -> Vec<Line<'static>> {
    let heading = Line::from(Span::styled(
        "Overview",
        Style::default().fg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD),
    ));
    let mut lines = wrap_lines_to_physical_rows(&[heading], width);
    let ferris = welcome::ferris_frame(welcome::ferris_phase(app, entry, width, now));
    lines.extend(welcome::overview_lines(entry, Some(status_label(app)), ferris, width));
    lines
}

fn status_label(app: &App) -> &'static str {
    match app.displayed_status() {
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
    Text { text: String, trailing_spacing: TextBlockSpacing },
    Notice { text: String, trailing_spacing: TextBlockSpacing, severity: SystemSeverity },
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

    fn can_merge(&self, next: &TextBlock, commit_ready: bool, control: &LayoutControl<'_>) -> bool {
        (!self.commit_ready && !commit_ready)
            || self.trailing_spacing == TextBlockSpacing::Continuation
            || assistant_markdown_block_crosses_handoff(
                &self.text,
                self.trailing_spacing,
                next,
                control,
            )
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
            item: AssistantRenderItem::Text {
                text: self.text,
                trailing_spacing: self.trailing_spacing,
            },
        }
    }
}

#[derive(Default)]
struct AssistantInlineLayoutState {
    has_body_content: bool,
    has_visible_content: bool,
    pending_blank_lines: usize,
    pending_trailing_rows: Vec<PendingAssistantBlankRows>,
}

struct PendingAssistantBlankRows {
    boundary: AssistantBoundaryMeta,
    count: usize,
}

impl AssistantInlineLayoutState {
    fn flush_trailing_rows(
        &mut self,
        rows: &mut Vec<Line<'static>>,
        boundaries: &mut Vec<LiveRowBoundary>,
    ) {
        for pending in self.pending_trailing_rows.drain(..) {
            push_assistant_boundary(boundaries, pending.boundary, rows.len());
            rows.extend(std::iter::repeat_with(Line::default).take(pending.count));
        }
    }
}

#[allow(clippy::too_many_lines)]
fn assistant_render_items_from_message(
    message: &ChatMessage,
    msg_idx: usize,
    active_mutable: bool,
    control: &LayoutControl<'_>,
) -> Vec<AssistantRenderItemSpec> {
    let mut items = Vec::with_capacity(message.blocks.len());
    let mut pending_text: Option<PendingAssistantTextRun> = None;
    let mut previous_kind = None;
    let active_tail_block_idx =
        active_mutable.then(|| last_visible_assistant_block_idx(message)).flatten();

    for (block_idx, block) in message.blocks.iter().enumerate() {
        if control.cancelled() {
            return Vec::new();
        }
        match block {
            MessageBlock::Text(text) => {
                if text.text.is_empty() {
                    continue;
                }
                let commit_ready = assistant_text_block_commit_ready(
                    message,
                    block_idx,
                    active_tail_block_idx,
                    control,
                );
                if let Some(pending) = pending_text.as_mut()
                    && pending.can_merge(text, commit_ready, control)
                {
                    let merged_commit_ready = assistant_text_run_commit_ready(
                        message,
                        pending.block_idx,
                        block_idx,
                        active_tail_block_idx,
                        control,
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
    assign_source_separators(&mut items);
    items
}

fn assign_source_separators(items: &mut [AssistantRenderItemSpec]) {
    // Source separators belong to the following item, including after the prior
    // item has been committed and excluded from the live rows.
    let mut trailing_gap = 0;
    for item in items {
        item.leading_blank_lines += trailing_gap;
        trailing_gap = match &item.item {
            AssistantRenderItem::Text { trailing_spacing, .. }
            | AssistantRenderItem::Notice { trailing_spacing, .. } => {
                trailing_spacing.blank_lines()
            }
            _ => 0,
        };
    }
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
        item: AssistantRenderItem::Notice {
            severity: notice.severity,
            text: notice.text.text.clone(),
            trailing_spacing: notice.text.trailing_spacing,
        },
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
        let content = render_canonical_tool_rows(
            app,
            msg_idx,
            block_idx,
            message_render_context(current_mode_id, MessageLayout::new(width).content_width),
            spinner,
            true,
            None,
        );
        rows.extend(message_layout::frame_rows(content, width, None, None, (false, false), &[]));
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
    control: &LayoutControl<'_>,
) -> bool {
    assistant_text_run_commit_ready(message, block_idx, block_idx, active_tail_block_idx, control)
}

fn assistant_text_run_commit_ready(
    message: &ChatMessage,
    start_block_idx: usize,
    end_block_idx: usize,
    active_tail_block_idx: Option<usize>,
    control: &LayoutControl<'_>,
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

    !assistant_text_run_handoff_requires_markdown_context(
        message,
        start_block_idx,
        end_block_idx,
        control,
    )
}

fn assistant_text_run_handoff_requires_markdown_context(
    message: &ChatMessage,
    start_block_idx: usize,
    end_block_idx: usize,
    control: &LayoutControl<'_>,
) -> bool {
    let Some((text, trailing_spacing)) =
        assistant_text_run_source(message, start_block_idx, end_block_idx)
    else {
        return false;
    };
    let Some(next) = next_visible_assistant_text(message, end_block_idx) else {
        return false;
    };

    trailing_spacing == TextBlockSpacing::Continuation
        || assistant_markdown_block_crosses_handoff(&text, trailing_spacing, next, control)
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

fn assistant_markdown_block_crosses_handoff(
    current: &str,
    spacing: TextBlockSpacing,
    next: &TextBlock,
    control: &LayoutControl<'_>,
) -> bool {
    let boundary = current.len();
    let mut source = current.to_owned();
    spacing.append_source(&mut source, &next.text);
    let next_start = source.len() - next.text.len();
    crate::markdown::context_crosses_boundary(&source, boundary, next_start, || control.cancelled())
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
}

#[derive(Clone, Copy)]
struct AssistantThinkingMeta {
    message_id: ChatMessageId,
    msg_idx: usize,
    width: u16,
    spinner: SpinnerState,
}

#[allow(clippy::too_many_arguments)]
fn render_assistant_item(
    app: &mut App,
    item: &AssistantRenderItemSpec,
    width: u16,
    current_mode_id: Option<&str>,
    spinner: SpinnerState,
    control: &LayoutControl<'_>,
    state: &mut AssistantInlineLayoutState,
    rows: &mut Vec<Line<'static>>,
    boundaries: &mut Vec<LiveRowBoundary>,
    tool_cursor: &mut tool_call::ToolRowsCursor,
    text_cursor: &mut Option<super::markdown::DocumentCursor>,
) -> bool {
    let content_width = MessageLayout::new(width).content_width;
    let boundary = AssistantBoundaryMeta {
        ids: item.ids.clone(),
        msg_idx: item.msg_idx,
        block_idx: item.block_idx,
        kind: item.boundary_kind,
        commit_ready: item.commit_ready,
    };
    let mut finished = true;
    let rendered = match &item.item {
        AssistantRenderItem::Text { text, .. } => {
            let cursor = text_cursor
                .get_or_insert_with(|| super::markdown::DocumentCursor::new(text, control));
            let mut physical = Vec::new();
            let row_budget = crate::ui::layout_control::replay_row_budget(
                app.chat_render.terminal_size().height,
            );
            loop {
                let Some(mut lines) = cursor.next(text, content_width, None, true, control) else {
                    break;
                };
                if !state.has_visible_content && physical.is_empty() {
                    let first =
                        lines.iter().position(|line| !line_is_blank(line)).unwrap_or(lines.len());
                    lines.drain(..first);
                }
                physical.extend(super::wrap::wrap_lines_with_control(
                    &lines,
                    content_width,
                    control,
                ));
                if control.can_yield() && (control.yield_due() || physical.len() >= row_budget) {
                    finished = false;
                    break;
                }
            }
            physical
        }
        AssistantRenderItem::Notice { text, severity, .. } => render_assistant_notice_block(
            text,
            *severity,
            content_width,
            !state.has_visible_content,
            control,
        ),
        AssistantRenderItem::CanonicalBtw { msg_idx, block_idx } => {
            render_canonical_btw_rows(app, *msg_idx, *block_idx, width, control)
        }
        AssistantRenderItem::CanonicalTool { msg_idx, block_idx } => render_canonical_tool_rows(
            app,
            *msg_idx,
            *block_idx,
            MessageRenderContext::with_control(current_mode_id, content_width, control),
            spinner,
            false,
            Some(tool_cursor),
        ),
    };
    append_rendered_assistant_item(
        rows,
        boundaries,
        state,
        boundary,
        item.leading_blank_lines,
        rendered,
    );
    finished
        && (!matches!(item.item, AssistantRenderItem::CanonicalTool { .. })
            || tool_cursor.is_finished())
}

fn append_assistant_thinking(
    rows: &mut Vec<Line<'static>>,
    boundaries: &mut Vec<LiveRowBoundary>,
    state: &mut AssistantInlineLayoutState,
    meta: AssistantThinkingMeta,
) {
    state.flush_trailing_rows(rows, boundaries);
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
            format!("{} Thinking\u{2026}", meta.spinner.icon()),
            Style::default().fg(theme::DIM),
        ))],
        meta.width,
    ));
}

fn frame_assistant_rows(
    app: &App,
    msg_idx: usize,
    width: u16,
    visibility: AssistantLabelVisibility,
    rendered: &mut RenderedMessageRows,
) {
    let show_marker = visibility != AssistantLabelVisibility::Hidden;
    if rendered.rows.is_empty() {
        return;
    }
    // Side-question cards already own a full-width frame and their own role heading.
    let full_width_rows = rendered
        .boundaries
        .iter()
        .enumerate()
        .filter(|(_, boundary)| boundary.kind == LiveRowBoundaryKind::AssistantBtw)
        .map(|(index, boundary)| {
            boundary.start_row
                ..rendered
                    .boundaries
                    .get(index + 1)
                    .map_or(rendered.rows.len(), |next| next.start_row)
        })
        .collect::<Vec<_>>();
    if show_marker {
        let label_id = HistoryOutputId::AssistantLabel(app.transcript.messages[msg_idx].id);
        let timestamp_ready = !app.config.show_message_timestamps_effective()
            || app.transcript.messages[msg_idx]
                .timing
                .timestamp
                .is_some_and(|timestamp| !timestamp.observed)
            || !active_assistant_message_is_mutable(app, msg_idx);
        if let Some(first) = rendered.boundaries.first_mut() {
            first.ids.push(label_id);
            first.commit_ready &= timestamp_ready;
        }
    }
    for boundary in &mut rendered.boundaries {
        boundary.start_row +=
            usize::from(show_marker) * usize::from(message_layout::VERTICAL_PADDING);
    }
    if let Some(first) = rendered.boundaries.first_mut() {
        first.start_row = 0;
    }
    rendered.rows = message_layout::frame_rows(
        std::mem::take(&mut rendered.rows),
        width,
        show_marker.then_some((theme::ASSISTANT_MESSAGE_MARKER, theme::ROLE_ASSISTANT)),
        None,
        (show_marker, false),
        &full_width_rows,
    );
    if show_marker
        && let Some(timestamp) =
            crate::app::presentation::timestamp_label(app, &app.transcript.messages[msg_idx])
        && let Some(padding) = rendered.rows.first_mut()
    {
        *padding = message_layout::timestamp_row(&timestamp, width, None);
    }
}

fn append_rendered_assistant_item(
    rows: &mut Vec<Line<'static>>,
    boundaries: &mut Vec<LiveRowBoundary>,
    state: &mut AssistantInlineLayoutState,
    boundary: AssistantBoundaryMeta,
    leading_blank_lines: usize,
    mut rendered: Vec<Line<'static>>,
) {
    state.pending_blank_lines += leading_blank_lines;
    let trailing = rendered.iter().rev().take_while(|row| line_is_blank(row)).count();
    rendered.truncate(rendered.len() - trailing);
    if rendered.is_empty() {
        if trailing > 0 {
            let count = std::mem::take(&mut state.pending_blank_lines) + trailing;
            state.pending_trailing_rows.push(PendingAssistantBlankRows { boundary, count });
        }
        return;
    }
    // Delay trailing blanks until another visible row proves they are internal,
    // retaining their original owner so stable and mutable sections can commit separately.
    state.flush_trailing_rows(rows, boundaries);
    let boundary_start = rows.len();
    rows.extend(
        std::iter::repeat_with(Line::default).take(std::mem::take(&mut state.pending_blank_lines)),
    );
    push_assistant_boundary(boundaries, boundary.clone(), boundary_start);
    state.has_body_content = true;
    state.has_visible_content = true;
    rows.extend(rendered);
    if trailing > 0 {
        state.pending_trailing_rows.push(PendingAssistantBlankRows { boundary, count: trailing });
    }
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
    text: &str,
    width: u16,
    trim_leading_blank_lines: bool,
    control: &LayoutControl<'_>,
) -> Vec<Line<'static>> {
    let lines = super::markdown::render_markdown_with_control(text, width, None, true, control);
    let lines = if trim_leading_blank_lines {
        let first_non_blank =
            lines.iter().position(|line| !line_is_blank(line)).unwrap_or(lines.len());
        lines.into_iter().skip(first_non_blank).collect::<Vec<_>>()
    } else {
        lines
    };
    super::wrap::wrap_lines_with_control(&lines, width, control)
}

fn render_assistant_notice_block(
    text: &str,
    severity: SystemSeverity,
    width: u16,
    trim_leading_blank_lines: bool,
    control: &LayoutControl<'_>,
) -> Vec<Line<'static>> {
    let mut lines = render_assistant_text_block(text, width, trim_leading_blank_lines, control);
    for line in &mut lines {
        for span in &mut line.spans {
            span.style = span.style.fg(system_severity_color(severity));
        }
    }
    lines
}

fn render_canonical_btw_rows(
    app: &mut App,
    msg_idx: usize,
    block_idx: usize,
    width: u16,
    control: &LayoutControl<'_>,
) -> Vec<Line<'static>> {
    let Some(MessageBlock::BtwExchange(exchange)) = app
        .transcript
        .messages
        .get_mut(msg_idx)
        .and_then(|message| message.blocks.get_mut(block_idx))
    else {
        return Vec::new();
    };
    render_btw_exchange_with_control(exchange, width, control)
}

fn render_canonical_tool_rows(
    app: &mut App,
    msg_idx: usize,
    block_idx: usize,
    render_context: MessageRenderContext<'_>,
    spinner: SpinnerState,
    interaction: bool,
    cursor: Option<&mut tool_call::ToolRowsCursor>,
) -> Vec<Line<'static>> {
    let unavailable = || {
        if cursor.is_some()
            && let Some(control) = render_context.tool_render_context.control
        {
            control.cancel();
        }
        Vec::new()
    };
    let show_duration = app.config.show_turn_duration_effective();
    let Some(block) = app
        .transcript
        .messages
        .get_mut(msg_idx)
        .and_then(|message| message.blocks.get_mut(block_idx))
    else {
        return unavailable();
    };
    let tc = match block {
        MessageBlock::ToolCall(tc) if interaction => tc.as_mut(),
        MessageBlock::ToolCall(tc) => tc.display_mut(),
        MessageBlock::ToolResult { tool, .. } => tool.as_mut(),
        _ => return unavailable(),
    };
    if tc.hidden_unless_focused_interaction() {
        return unavailable();
    }

    let (mut rows, finished) = if let Some(cursor) = cursor {
        let rows =
            cursor.next(tc, render_context.tool_render_context, render_context.width, spinner);
        (rows, cursor.is_finished())
    } else {
        let mut rows = Vec::new();
        tool_call::render_tool_call_cached(
            tc,
            render_context.tool_render_context,
            render_context.width,
            spinner,
            &mut rows,
        );
        (rows, true)
    };
    if finished
        && show_duration
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
    if interaction && finished {
        tc.cache.evict_cached_render();
    }
    let uninterrupted = LayoutControl::uninterrupted();
    let physical_rows = super::wrap::wrap_lines_with_control(
        &rows,
        render_context.width,
        render_context.tool_render_context.control.unwrap_or(&uninterrupted),
    );
    app.sync_render_cache_slot(msg_idx, block_idx);
    physical_rows
}

pub(crate) fn assistant_role_label_line() -> Line<'static> {
    Line::from(vec![ratatui::text::Span::styled(
        format!(" {}", theme::ASSISTANT_MESSAGE_MARKER),
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
    control: &LayoutControl<'_>,
) -> Vec<Line<'static>> {
    let mut rows = Vec::new();
    for segment in segments {
        match segment {
            MessageRowSegment::Blank => rows.push(Line::default()),
            MessageRowSegment::Lines { lines } => {
                rows.extend(super::wrap::wrap_lines_with_control(lines, width, control));
            }
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    #[test]
    fn long_code_replay_yields_before_the_end_and_preserves_every_source_line() {
        let source = format!(
            "```text\n{}\n```",
            (0..400).map(|n| format!("SOURCE_{n:04}")).collect::<Vec<_>>().join("\n")
        );
        for width in [24, 80, 120] {
            let mut app = App::test_default();
            app.transcript.messages.push(assistant_blocks_message(vec![MessageBlock::Text(
                TextBlock::from_complete(&source),
            )]));
            let mut cursor = super::LiveRowsCursor::new(std::time::Instant::now());
            let mut all = SerializedLiveRows::new(Vec::new(), Vec::new());
            let mut saw_early_output = false;
            while !cursor.is_finished() {
                let control =
                    super::LayoutControl::with_budget(&|| false, std::time::Duration::ZERO);
                let Some(chunk) = cursor.next(&mut app, width, &BTreeSet::new(), &control) else {
                    break;
                };
                saw_early_output |=
                    chunk.rows().iter().any(|row| row.to_string().contains("SOURCE_0000"))
                        && !cursor.is_finished();
                all.append(chunk);
            }
            assert!(saw_early_output);
            let text = line_texts(all.rows());
            for n in 0..400 {
                assert_eq!(
                    text.iter().filter(|row| row.trim() == format!("SOURCE_{n:04}")).count(),
                    1
                );
            }
            assert!(all.rows().iter().all(|row| row.width() <= usize::from(width)));
            assert_eq!(all.stable_row_count(), all.rows().len());
            assert_assistant_marker(all.rows(), true);
        }
    }

    #[test]
    fn replay_yields_text_before_working_on_a_tool_in_the_same_turn() {
        let mut app = App::test_default();
        let mut tool = tool_call_block("expensive", false);
        let MessageBlock::ToolCall(tc) = &mut tool else { unreachable!() };
        tc.sdk_tool_name = "Read".into();
        tc.locations = vec![model::ToolCallLocation::new("source.rs")];
        tc.content = vec![model::ToolCallContent::from("fn main() {}\n".repeat(2000))];
        app.transcript.messages.push(assistant_blocks_message(vec![
            MessageBlock::Text(TextBlock::from_complete("FIRST")),
            tool,
            MessageBlock::Text(TextBlock::from_complete("LAST")),
        ]));
        let mut cursor = super::LiveRowsCursor::new(std::time::Instant::now());
        let first = cursor
            .next(&mut app, 80, &BTreeSet::new(), &super::LayoutControl::uninterrupted())
            .unwrap();
        assert!(first.rows().iter().any(|row| row.to_string().contains("FIRST")));
        assert!(!first.rows().iter().any(|row| row.to_string().contains("LAST")));
        let MessageBlock::ToolCall(tc) = &app.transcript.messages[0].blocks[1] else {
            unreachable!()
        };
        assert!(
            tc.cache.cached_bytes() == 0,
            "the following tool must not be laid out before the ready text is returned"
        );
        let checks = std::cell::Cell::new(0);
        let resize = || {
            checks.set(checks.get() + 1);
            checks.get() > 20
        };
        let control = super::LayoutControl::new(&resize);
        while cursor.next(&mut app, 80, &BTreeSet::new(), &control).is_some() {}
        assert!(control.cancelled(), "resize must interrupt the file preview");
        let fresh = serialize_live_rows(&mut app, 31);
        assert!(fresh.iter().all(|row| row.width() <= 31));
        for marker in ["FIRST", "LAST"] {
            assert_eq!(fresh.iter().filter(|row| row.to_string().contains(marker)).count(), 1);
        }
    }

    #[test]
    fn hiding_a_tool_after_its_title_cancels_replay_before_its_body() {
        let mut app = App::test_default();
        let mut tool = tool_call_block("hidden-mid-replay", false);
        let MessageBlock::ToolCall(tc) = &mut tool else { unreachable!() };
        tc.content = vec![model::ToolCallContent::from("STALE BODY")];
        app.transcript.messages.push(assistant_blocks_message(vec![tool]));
        let mut cursor = super::LiveRowsCursor::new(std::time::Instant::now());
        let control = super::LayoutControl::new(&|| false);
        let title = cursor.next(&mut app, 80, &BTreeSet::new(), &control).unwrap();
        assert!(!title.rows().is_empty());
        assert!(!title.rows().iter().any(|row| row.to_string().contains("STALE BODY")));
        let MessageBlock::ToolCall(tc) = &mut app.transcript.messages[0].blocks[0] else {
            unreachable!()
        };
        tc.hidden = true;
        tc.cache.invalidate();
        assert!(cursor.next(&mut app, 80, &BTreeSet::new(), &control).is_none());
        assert!(control.cancelled());
        assert!(
            !serialize_live_rows(&mut app, 80)
                .iter()
                .any(|row| row.to_string().contains("STALE BODY"))
        );
    }

    #[test]
    fn interrupted_large_message_discards_partial_layout_and_restarts_at_new_width() {
        use std::cell::Cell;
        let source =
            format!("```text\nFIRST\n{}LAST\n```", "a line of source content\n".repeat(2000));
        let mut app = App::test_default();
        let block = TextBlock::from_complete(&source);
        let id = block.id;
        app.transcript.messages.push(assistant_blocks_message(vec![MessageBlock::Text(block)]));
        let checks = Cell::new(0);
        let resize = || {
            checks.set(checks.get() + 1);
            checks.get() > 200
        };
        let control = super::LayoutControl::new(&resize);
        let mut cursor = super::LiveRowsCursor::new(std::time::Instant::now());
        while cursor.next(&mut app, 80, &BTreeSet::new(), &control).is_some() {}
        assert!(control.cancelled(), "resize must be observed inside the message");

        let fresh =
            serialize_live_rows_with_boundaries_excluding(&mut app, 31, &BTreeSet::new(), None);
        let text = fresh.rows().iter().map(ToString::to_string).collect::<Vec<_>>();
        assert_eq!(
            text.iter().filter(|row| row.trim() == "FIRST" || row.trim() == "● FIRST").count(),
            1
        );
        assert_eq!(text.iter().filter(|row| row.trim() == "LAST").count(), 1);
        assert!(fresh.rows().iter().all(|row| row.width() <= 31));
        let MessageBlock::Text(block) = &app.transcript.messages[0].blocks[0] else {
            panic!("text source")
        };
        assert_eq!(block.id, id);
        assert_eq!(block.text, source);
    }

    use super::{
        LiveRowBoundaryKind, LiveRowSegment, SerializedLiveRows, serialize_live_rows_at,
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
        MessageRole, TextBlock, TextBlockSpacing, ToolCallInfo,
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
        serialize_live_rows_with_boundaries_excluding(app, width, &BTreeSet::new(), None)
    }

    fn live_segment(start_row: usize, end_row: usize, commit_ready: bool) -> LiveRowSegment {
        LiveRowSegment {
            ids: vec![HistoryOutputId::AssistantLabel(ChatMessageId::new())],
            msg_idx: 0,
            block_idx: None,
            kind: LiveRowBoundaryKind::Message,
            start_row,
            end_row,
            commit_ready,
        }
    }

    fn assert_assistant_marker(rows: &[Line<'static>], expected: bool) {
        assert_eq!(
            line_texts(rows).iter().filter(|row| row.trim_start().starts_with('\u{25cf}')).count(),
            usize::from(expected),
            "the active response owns one marker; waiting or absent responses own none"
        );
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

    fn rendered_buffer(rows: &[Line<'static>], width: u16) -> ratatui::buffer::Buffer {
        use ratatui::widgets::Widget;
        let area = ratatui::layout::Rect::new(0, 0, width, u16::try_from(rows.len()).unwrap());
        let mut buffer = ratatui::buffer::Buffer::empty(area);
        ratatui::widgets::Paragraph::new(rows.to_vec()).render(area, &mut buffer);
        buffer
    }

    #[test]
    fn user_card_fills_every_cell_and_aligns_text_images_and_paragraphs_with_the_composer() {
        for width in [30, 80, 120] {
            let mut app = App::test_default();
            let mut message = user_text_message("alpha\n\nomega");
            message
                .blocks
                .push(MessageBlock::ImageAttachment(crate::app::ImageAttachmentBlock::new(2)));
            app.transcript.messages.push(message);
            let serialized = serialize_all_rows_with_boundaries(&mut app, width);
            let buffer = rendered_buffer(serialized.rows(), width);
            assert_eq!(buffer.area.height, 6, "two padding rows, three text rows and an image row");
            for row in 0..6 {
                for column in 0..width {
                    assert_eq!(
                        buffer[(column, row)].bg,
                        crate::ui::theme::USER_MSG_BG,
                        "({column}, {row}) at width {width}"
                    );
                }
            }
            assert_eq!(buffer[(1, 1)].symbol(), "❯");
            assert_eq!(buffer[(1, 1)].fg, crate::ui::theme::DIM);
            assert_eq!(buffer[(3, 1)].symbol(), "a");
            assert_eq!(buffer[(3, 3)].symbol(), "o");
            assert_eq!(buffer[(3, 4)].symbol(), "[");
            for row in [0, 2, 5] {
                assert!((0..width).all(|column| buffer[(column, row)].symbol() == " "));
            }
            let geometry = crate::ui::input::compute_render_geometry(
                ratatui::layout::Rect::new(0, 0, width, 3),
                0,
            );
            assert_eq!(geometry.text.x, 3);
            let MessageBlock::Text(source) = &app.transcript.messages[0].blocks[0] else {
                panic!("user text");
            };
            assert_eq!(source.text, "alpha\n\nomega");
        }
    }

    #[test]
    fn long_user_cards_preserve_unicode_and_all_lines_after_resize_without_tinting_the_reply() {
        let source = (1..=24)
            .map(|number| format!("界e\u{301} item{number:02}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut app = App::test_default();
        app.transcript.messages.push(user_text_message(&source));
        app.transcript.messages.push(assistant_text_message("Reply"));

        for width in [80, 8, 12, 80] {
            let serialized = serialize_all_rows_with_boundaries(&mut app, width);
            let user = serialized.segments().iter().find(|segment| segment.msg_idx == 0).unwrap();
            let user_rows = serialized.segment_rows(user).unwrap();
            assert!(user_rows.len() >= 26, "submitted text must not inherit the editor height cap");
            assert!(serialized.rows().iter().all(|row| row.width() <= usize::from(width)));
            let buffer = rendered_buffer(serialized.rows(), width);
            let mut visible = String::new();
            for row in 0..u16::try_from(user_rows.len()).unwrap() {
                let mut column = 0;
                while column < width {
                    let cell = &buffer[(column, row)];
                    assert_eq!(cell.bg, crate::ui::theme::USER_MSG_BG);
                    if column >= 3 {
                        visible.push_str(cell.symbol());
                    }
                    // Wide glyphs paint their continuation cells; those buffer slots are unused.
                    column += u16::try_from(unicode_width::UnicodeWidthStr::width(cell.symbol()))
                        .unwrap()
                        .max(1);
                }
            }
            assert_eq!(
                visible.chars().filter(|character| !character.is_whitespace()).collect::<String>(),
                source.chars().filter(|character| !character.is_whitespace()).collect::<String>(),
                "no clipped, missing or duplicated text at width {width}"
            );
            for row in u16::try_from(user_rows.len()).unwrap()..buffer.area.height {
                assert!(
                    (0..width)
                        .all(|column| { buffer[(column, row)].bg == ratatui::style::Color::Reset })
                );
            }
            let MessageBlock::Text(text) = &app.transcript.messages[0].blocks[0] else {
                panic!("user text");
            };
            assert_eq!(text.text, source);
        }
    }

    #[test]
    fn agent_marker_and_tools_share_the_content_column_without_a_card_background() {
        for tool_first in [false, true] {
            let mut app = App::test_default();
            let mut blocks = vec![tool_call_block("layout-tool", false)];
            if !tool_first {
                blocks.insert(0, MessageBlock::Text(TextBlock::from_complete("Opening text")));
            }
            blocks.push(MessageBlock::Text(TextBlock::from_complete("Final text")));
            app.transcript.messages.push(assistant_blocks_message(blocks));
            let serialized = serialize_all_rows_with_boundaries(&mut app, 80);
            let buffer = rendered_buffer(serialized.rows(), 80);
            assert_eq!(buffer[(1, 1)].symbol(), "\u{25cf}");
            assert_eq!(buffer[(1, 1)].fg, crate::ui::theme::ROLE_ASSISTANT);
            let text = line_texts(serialized.rows());
            let tool = text.iter().position(|row| row.contains("Child Tool")).unwrap();
            assert_eq!(
                buffer[(3, u16::try_from(tool).unwrap())].symbol(),
                crate::ui::theme::ICON_COMPLETED
            );
            let final_text = text.iter().position(|row| row.contains("Final text")).unwrap();
            assert_eq!(buffer[(3, u16::try_from(final_text).unwrap())].symbol(), "F");
            if !tool_first {
                assert_eq!(buffer[(3, 1)].symbol(), "O");
            }
            assert!(buffer.content.iter().all(|cell| cell.bg == ratatui::style::Color::Reset));
            assert_eq!(text.iter().filter(|row| row.contains('\u{25cf}')).count(), 1);
            assert!(text.first().unwrap().is_empty());
            assert!(text.last().unwrap().is_empty());
        }
    }

    #[test]
    fn committed_agent_prefix_owns_its_marker_but_bottom_padding_waits_for_completion() {
        let mut app = App::test_default();
        app.transcript.messages.push(assistant_blocks_message(vec![
            MessageBlock::Text(
                TextBlock::from_complete("Stable prefix")
                    .with_trailing_spacing(TextBlockSpacing::ParagraphBreak),
            ),
            MessageBlock::Text(TextBlock::from_complete("Streaming tail")),
        ]));
        app.bind_active_turn_assistant(0);
        app.status = AppStatus::Running;
        let first = serialize_all_rows_with_boundaries(&mut app, 40);
        let stable = &first.rows()[..first.stable_row_count()];
        assert!(line_texts(stable).iter().any(|row| row == " \u{25cf} Stable prefix"));
        let excluded = first
            .segments()
            .iter()
            .filter(|segment| segment.commit_ready)
            .flat_map(|segment| segment.ids.iter().cloned())
            .collect::<BTreeSet<_>>();
        assert!(excluded.contains(&HistoryOutputId::AssistantLabel(app.transcript.messages[0].id)));
        assert!(
            !excluded.contains(&HistoryOutputId::AssistantPadding(app.transcript.messages[0].id))
        );
        let tail = serialize_live_rows_with_boundaries_excluding(&mut app, 40, &excluded, None);
        assert!(
            !line_texts(tail.rows())
                .iter()
                .any(|row| row.contains('\u{25cf}') || row.contains("Stable prefix"))
        );
        assert!(line_texts(tail.rows()).iter().any(|row| row == "   Streaming tail"));
        app.status = AppStatus::Ready;
        let complete = serialize_live_rows_with_boundaries_excluding(&mut app, 40, &excluded, None);
        assert_eq!(complete.stable_row_count(), complete.rows().len());
        let padding = complete
            .segments()
            .iter()
            .find(|segment| {
                segment
                    .ids
                    .contains(&HistoryOutputId::AssistantPadding(app.transcript.messages[0].id))
            })
            .unwrap();
        assert!(padding.commit_ready);
        assert_eq!(complete.segment_rows(padding).unwrap().len(), 1);
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

    #[test]
    fn live_tool_titles_keep_full_sources_and_obey_cache_budget_and_history_exclusion() {
        let mut app = App::test_default();
        app.status = AppStatus::Ready;
        let command =
            format!("echo '**literal** _argument_'\n{}", "    large command tail\n".repeat(500));
        let read_title = format!("**Read report**\n{}", "discarded description\n".repeat(500));
        let mcp_title = format!(
            "**Search MCP documents** {}\n{}",
            "long query ".repeat(500),
            "```unsupported-title-language\nignored\n```\n".repeat(100)
        );
        app.transcript.messages.push(assistant_blocks_message(vec![
            named_tool_call_block("shell-title", &command, "Bash"),
            named_tool_call_block("read-title", &read_title, "Read"),
            named_tool_call_block("mcp-title", &mcp_title, "mcp__docs__search"),
        ]));

        let rendered = serialize_all_rows_with_boundaries(&mut app, 100);
        for (id, expected) in
            [("shell-title", "echo '**literal** _argument_'"), ("read-title", "Read report")]
        {
            let segment = rendered
                .segments()
                .iter()
                .find(|segment| segment.ids.contains(&HistoryOutputId::ToolCall(id.to_owned())))
                .expect("tool has its own transcript segment");
            let rows = rendered.segment_rows(segment).unwrap();
            assert_eq!(
                rows.iter().filter(|row| !line_text(row).trim().is_empty()).count(),
                1,
                "title-only tool has one content row"
            );
            assert!(rows.iter().any(|row| line_text(row).ends_with(expected)));
            assert!(segment.commit_ready);
        }
        let bytes = app.enforce_render_cache_budget().total_before_bytes;
        assert!(
            bytes > 0 && bytes < command.len() + read_title.len() + mcp_title.len(),
            "cache must contain title projections, not discarded multiline tails: {bytes}"
        );
        let mcp_segment = rendered
            .segments()
            .iter()
            .find(|segment| {
                segment.ids.contains(&HistoryOutputId::ToolCall("mcp-title".to_owned()))
            })
            .expect("MCP tool has its own segment");
        let mcp_rows = rendered.segment_rows(mcp_segment).unwrap();
        assert_eq!(mcp_rows.len(), 1);
        assert!(line_text(&mcp_rows[0]).contains("Search MCP documents"));
        assert!(line_text(&mcp_rows[0]).ends_with('…'));
        let replay = serialize_all_rows_with_boundaries(&mut app, 100);
        assert_eq!(replay.rows(), rendered.rows());
        assert_eq!(app.enforce_render_cache_budget().total_before_bytes, bytes);

        app.render_cache_budget.max_bytes = 0;
        let stats = app.enforce_render_cache_budget();
        assert_eq!(stats.evicted_blocks, 3, "title-only tool caches must be evictable");
        assert_eq!(stats.evicted_bytes, bytes);
        assert_eq!(stats.total_after_bytes, 0);
        for (idx, source) in [(0, &command), (1, &read_title), (2, &mcp_title)] {
            let MessageBlock::ToolCall(tool) = &app.transcript.messages[0].blocks[idx] else {
                unreachable!("seeded tool block")
            };
            assert_eq!(&tool.title, source);
            assert_eq!(tool.cache.cached_bytes(), 0);
        }
        app.render_cache_budget.max_bytes = usize::MAX;
        let resized = serialize_all_rows_with_boundaries(&mut app, 24);
        assert!(resized.rows().iter().all(|row| row.width() <= 24));
        assert!(app.enforce_render_cache_budget().total_before_bytes > 0);
        let restored = serialize_all_rows_with_boundaries(&mut app, 100);
        assert_eq!(restored.rows(), rendered.rows());

        let confirmed: BTreeSet<_> =
            restored.segments().iter().flat_map(|segment| segment.ids.iter().cloned()).collect();
        let live = serialize_live_rows_with_boundaries_excluding(&mut app, 100, &confirmed, None);
        assert!(live.rows().is_empty(), "confirmed tool titles must not reappear in live output");
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
    fn user_paragraphs_stay_separate_from_the_streaming_reply_after_resize() {
        let mut app = App::test_default();
        app.push_message_tracked(user_text_message("hello hello hello hello\n\nhow are you"));
        app.transcript.messages.push(assistant_text_message("live reply"));
        app.bind_active_turn_assistant(1);
        app.status = AppStatus::Running;

        for width in [80, 12, 80] {
            let serialized = serialize_all_rows_with_boundaries(&mut app, width);
            let user = serialized.segments().iter().find(|segment| segment.msg_idx == 0).unwrap();
            assert!(user.commit_ready);
            let user_text = line_texts(serialized.segment_rows(user).unwrap());
            let paragraphs = user_text
                .iter()
                .map(|row| row.chars().skip(3).collect::<String>())
                .collect::<Vec<_>>();
            let paragraphs = paragraphs
                .split(|row| row.trim().is_empty())
                .filter(|paragraph| !paragraph.is_empty())
                .map(|paragraph| paragraph.join(" "))
                .collect::<Vec<_>>();
            assert_eq!(paragraphs, ["hello hello hello hello", "how are you"]);
            let reply = line_texts(&serialized.rows()[serialized.stable_row_count()..])
                .into_iter()
                .map(|row| row.chars().skip(3).collect::<String>())
                .filter(|row| !row.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            assert_eq!(reply, "live reply");
        }
    }

    fn assistant_content_rows(app: &mut App, width: u16) -> (Vec<Line<'static>>, Vec<String>) {
        let rows = serialize_live_rows(app, width);
        let text = line_texts(&rows)
            .into_iter()
            .map(|row| row.chars().skip(3).collect::<String>())
            .collect::<Vec<_>>();
        (rows, text)
    }

    #[test]
    fn assistant_markdown_shows_parsed_headings_and_verbatim_code_blocks() {
        let source = concat!(
            "## Setup\n\n",
            "Run this first:\nthen continue\n\n",
            "```bash\n#!/usr/bin/env bash\n# install deps\n  npm ci\n```\n\n",
            "1. Then check\n\n",
            "   ```sh\n   # nested\n   ```\n\n",
            "   | Name | Value |\n   | --- | --- |\n   | Alpha | Beta |",
        );
        let mut app = App::test_default();
        app.transcript.messages.push(assistant_text_message(source));
        let (rows, text) = assistant_content_rows(&mut app, 80);

        let heading = text.iter().position(|row| row == "Setup").expect("heading row");
        assert_eq!(
            text[heading..heading + 10],
            [
                "Setup",
                "",
                "Run this first:",
                "then continue",
                "",
                "bash",
                "#!/usr/bin/env bash",
                "# install deps",
                "  npm ci",
                "",
            ]
        );
        assert!(
            rows[heading]
                .spans
                .iter()
                .filter(|span| span.content.contains("Setup"))
                .all(|span| span.style.add_modifier.contains(ratatui::style::Modifier::BOLD)),
            "heading lost its style"
        );
        assert!(!text.iter().any(|row| row.contains("##") || row.contains("```")), "{text:?}");

        let body_column = text
            .iter()
            .find_map(|row| row.find("1. Then check"))
            .map(|column| column + 3)
            .expect("list item");
        for content in ["sh", "# nested", "Name"] {
            let row =
                text.iter().find(|row| row.trim() == content || row.trim().starts_with(content));
            let row = row.unwrap_or_else(|| panic!("missing {content:?}: {text:?}"));
            assert_eq!(row.find(content), Some(body_column), "{content:?} left its item: {text:?}");
        }
        let table_row = text.iter().find(|row| row.contains("Alpha")).expect("table row");
        assert!(table_row.contains("Beta"), "table cells split across rows: {text:?}");
        assert!(text.iter().any(|row| row.find('─') == Some(body_column)), "{text:?}");

        let MessageBlock::Text(block) = &app.transcript.messages[0].blocks[0] else {
            panic!("expected assistant text");
        };
        assert_eq!(block.text, source);
    }

    #[test]
    fn assistant_code_blocks_wrap_inside_the_message_column() {
        let code = "let value = compute_something_long(alpha, beta, gamma);";
        let mut app = App::test_default();
        app.transcript.messages.push(assistant_text_message(&format!("```rust\n{code}\n```")));
        let (rows, text) = assistant_content_rows(&mut app, 30);

        assert!(rows.iter().all(|row| crate::ui::wrap::line_display_width(row) <= 30));
        let label = text.iter().position(|row| row == "rust").expect("language label");
        let body =
            text[label + 1..].iter().take_while(|row| !row.is_empty()).cloned().collect::<Vec<_>>();
        assert!(body.len() > 1, "long code must wrap: {text:?}");
        assert_eq!(body.concat().replace(' ', ""), code.replace(' ', ""));
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
        assert!(narrow_text.contains(" ❯ "));
        assert!(narrow_text.contains(" ● "));
        assert!(wide_text.contains(" ❯ "));
        assert!(wide_text.contains(" ● "));
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

        assert!(stable_text.iter().any(|line| line.starts_with(" ●")));
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

        let tool = serialized
            .segments()
            .iter()
            .find(|segment| segment.ids.contains(&HistoryOutputId::ToolCall("done-tool".into())))
            .unwrap();
        assert!(tool.commit_ready);
        assert!(
            serialized
                .segment_rows(tool)
                .unwrap()
                .iter()
                .any(|row| line_text(row).contains("Child Tool"))
        );
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
            Some(LiveRowBoundaryKind::AssistantTool)
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

        let text = line_texts(&rows);
        let first = text.iter().position(|row| row.ends_with("line 1: ready")).unwrap();
        let second = text.iter().position(|row| row.ends_with("line 2: ready")).unwrap();
        assert_eq!(second - first, 2);
        assert!(text[first + 1].trim().is_empty());
    }

    #[test]
    fn live_assistant_text_preserves_paragraph_gap_before_list_block() {
        let mut app = App::test_default();
        app.transcript.messages.push(assistant_blocks_message(vec![
            MessageBlock::Text(
                TextBlock::from_complete("Intro\n\n")
                    .with_trailing_spacing(TextBlockSpacing::ParagraphBreak),
            ),
            MessageBlock::Text(TextBlock::from_complete("- One\n- Two")),
        ]));

        let rows = serialize_live_rows(&mut app, 120);

        let text = line_texts(&rows);
        let intro = text.iter().position(|row| row.ends_with("Intro")).unwrap();
        assert!(text[intro + 1].trim().is_empty());
        assert!(text[intro + 2].trim_start().starts_with("- One"));
        assert!(text[intro + 3].trim_start().starts_with("- Two"));
    }

    #[test]
    fn live_assistant_text_preserves_paragraph_gap_after_list_block() {
        let mut app = App::test_default();
        app.transcript.messages.push(assistant_blocks_message(vec![
            MessageBlock::Text(
                TextBlock::from_complete("- One\n- Two\n\n")
                    .with_trailing_spacing(TextBlockSpacing::ParagraphBreak),
            ),
            MessageBlock::Text(TextBlock::from_complete("Outro")),
        ]));

        let rows = serialize_live_rows(&mut app, 120);

        let text = line_texts(&rows);
        let last_item = text.iter().position(|row| row.trim() == "- Two").unwrap();
        assert!(text[last_item + 1].trim().is_empty());
        assert_eq!(text[last_item + 2].trim(), "Outro");
    }

    #[test]
    fn live_assistant_text_preserves_single_newline_rows() {
        let mut app = App::test_default();
        app.transcript
            .messages
            .push(assistant_text_message("line 1: ready\nline 2: ready\nline 3: ready"));

        let rows = serialize_live_rows(&mut app, 120);

        let text = line_texts(&rows);
        let first = text.iter().position(|row| row.ends_with("line 1: ready")).unwrap();
        assert_eq!(text[first + 1].trim(), "line 2: ready");
        assert_eq!(text[first + 2].trim(), "line 3: ready");
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
            serialize_live_rows_with_boundaries_excluding(&mut app, 120, &excluded_ids, None);
        let text = line_texts(serialized.rows());

        assert_eq!(
            text.iter().map(|row| row.trim()).filter(|row| !row.is_empty()).collect::<Vec<_>>(),
            ["second paragraph"]
        );
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
            serialize_live_rows_with_boundaries_excluding(&mut app, 180, &committed_ids, None);
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
    fn parsed_markdown_handoffs_keep_rows_together_through_completion_and_resize() {
        for (first, second) in [
            ("Name | Value\n- | -\nred | one\n", "blue | two\n"),
            ("Name | Value\n--- | ---\nred\n", "blue\n"),
            ("> Name | Value\n> --- | ---\n> red | one\n", "> blue | two\n"),
            ("````md\n```\nName | Value\n", "- | -\nred | one\n````"),
        ] {
            let first = TextBlock::from_complete(first);
            let second = TextBlock::from_complete(second);
            let block_ids = [HistoryOutputId::Block(first.id), HistoryOutputId::Block(second.id)];
            let source = format!("{}{}", first.text, second.text);
            let mut streamed = App::test_default();
            streamed.transcript.messages.push(assistant_blocks_message(vec![
                MessageBlock::Text(first),
                MessageBlock::Text(second),
            ]));
            streamed.bind_active_turn_assistant(0);
            let mut whole = App::test_default();
            whole.transcript.messages.push(assistant_text_message(&source));
            whole.bind_active_turn_assistant(0);
            for running in [true, false] {
                streamed.status = if running { AppStatus::Running } else { AppStatus::Ready };
                whole.status = if running { AppStatus::Running } else { AppStatus::Ready };
                for width in [24, 80, 24] {
                    let actual = serialize_all_rows_with_boundaries(&mut streamed, width);
                    let expected = serialize_all_rows_with_boundaries(&mut whole, width);
                    assert_eq!(
                        line_texts(actual.rows()),
                        line_texts(expected.rows()),
                        "{source:?}"
                    );
                    let text_segments = actual
                        .segments()
                        .iter()
                        .filter(|segment| segment.kind == LiveRowBoundaryKind::AssistantText)
                        .collect::<Vec<_>>();
                    assert_eq!(text_segments.len(), 1, "one block must have one render owner");
                    assert!(block_ids.iter().all(|id| text_segments[0].ids.contains(id)));
                    if running {
                        assert_eq!(actual.stable_row_count(), 0, "context must stay mutable");
                    } else {
                        assert_eq!(actual.stable_row_count(), actual.rows().len());
                    }
                }
            }
        }
    }

    #[test]
    fn paragraph_gap_stays_with_the_table_after_an_empty_quote_prefix() {
        let first = TextBlock::from_complete("Introduction.\n\n")
            .with_trailing_spacing(TextBlockSpacing::ParagraphBreak);
        let first_id = first.id;
        let table = TextBlock::from_complete(">");
        let table_id = table.id;
        let mut app = App::test_default();
        app.transcript.messages.push(assistant_blocks_message(vec![
            MessageBlock::Text(first),
            MessageBlock::Text(table),
        ]));
        app.bind_active_turn_assistant(0);
        app.status = AppStatus::Running;
        let initial = serialize_all_rows_with_boundaries(&mut app, 36);
        let committed = initial
            .segments()
            .iter()
            .filter(|segment| segment.commit_ready)
            .flat_map(|segment| segment.ids.iter().cloned())
            .collect::<BTreeSet<_>>();
        assert!(committed.contains(&HistoryOutputId::Block(first_id)));
        assert!(!committed.contains(&HistoryOutputId::Block(table_id)));
        let MessageBlock::Text(table) = &mut app.transcript.messages[0].blocks[1] else {
            panic!("table block")
        };
        table.text = "> Name | Value\n> --- | ---\n> red | one\n".to_owned();
        table.cache.invalidate();

        let live = serialize_live_rows_with_boundaries_excluding(&mut app, 36, &committed, None);
        let table = live
            .segments()
            .iter()
            .find(|segment| segment.ids.contains(&HistoryOutputId::Block(table_id)))
            .expect("live table segment");
        let rows = live.segment_rows(table).unwrap();
        assert!(super::line_is_blank(&rows[0]), "the table must own its leading paragraph gap");
        assert!(
            rows.iter().any(|row| line_text(row).contains("red") && line_text(row).contains("one"))
        );
    }

    #[test]
    fn a_closed_markdown_block_releases_scrollback_before_an_independent_pipe_paragraph() {
        for (source, spacing) in [
            ("Name | Value\n--- | ---\nred | one\n", TextBlockSpacing::ParagraphBreak),
            ("Name | Value\n--- | ---\nred | literal **\n", TextBlockSpacing::ParagraphBreak),
            ("Name | Value\n--- | ---\nred | literal **", TextBlockSpacing::ParagraphBreak),
            ("One | Two\n--- | --- | ---\nred | one | two\n", TextBlockSpacing::None),
            ("````md\n```\nliteral **\n````\n", TextBlockSpacing::None),
            ("````md\n```\nliteral **\n````", TextBlockSpacing::None),
        ] {
            let mut app = App::test_default();
            let first = TextBlock::from_complete(source).with_trailing_spacing(spacing);
            let first_id = first.id;
            app.transcript.messages.push(assistant_blocks_message(vec![
                MessageBlock::Text(first),
                MessageBlock::Text(TextBlock::from_complete("ordinary | paragraph")),
            ]));
            app.bind_active_turn_assistant(0);
            app.status = AppStatus::Running;

            let rows = serialize_all_rows_with_boundaries(&mut app, 80);
            let table = rows
                .segments()
                .iter()
                .find(|segment| segment.ids.contains(&HistoryOutputId::Block(first_id)))
                .expect("table segment");
            assert!(table.commit_ready, "independent pipe text must not hold the prefix");
            assert!(table.end_row <= rows.stable_row_count());
            assert!(
                rows.segment_rows(table)
                    .unwrap()
                    .iter()
                    .all(|row| !line_text(row).contains("ordinary"))
            );
        }
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
            text.iter().any(|line| line.starts_with(" ●   3. Keymap survey")),
            "missing ordered list prefix: {text:?}"
        );
        assert!(
            text.iter().any(|line| line.starts_with("        ") && !line.trim().is_empty()),
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
        let remaining =
            serialize_live_rows_with_boundaries_excluding(&mut app, 38, &committed_ids, None);
        let remaining_text = line_texts(remaining.rows());

        assert_eq!(serialized.stable_row_count(), 0);
        assert!(
            remaining_text.iter().any(|line| line.starts_with(" ●   3. Keymap survey")),
            "missing ordered list prefix: {remaining_text:?}"
        );
    }

    #[test]
    fn streamed_list_paragraphs_keep_numbering_and_never_commit_an_incomplete_list() {
        let pieces = [
            "9. Parent item.\n\n",
            "   Continuation paragraph.\n\n",
            "   - Nested child.\n",
            "10. Following item.",
        ];
        for width in [32, 80] {
            let mut streamed = App::test_default();
            streamed.transcript.messages.push(assistant_blocks_message(Vec::new()));
            streamed.bind_active_turn_assistant(0);
            streamed.status = AppStatus::Running;
            for piece in pieces {
                streamed.transcript.messages[0]
                    .blocks
                    .push(MessageBlock::Text(TextBlock::from_complete(piece)));
                let partial = serialize_all_rows_with_boundaries(&mut streamed, width);
                assert_eq!(
                    partial.stable_row_count(),
                    0,
                    "list context must stay mutable until the list ends: {piece:?}"
                );
            }
            let mut whole = App::test_default();
            whole.transcript.messages.push(assistant_text_message(&pieces.concat()));
            whole.bind_active_turn_assistant(0);
            whole.status = AppStatus::Running;
            assert_eq!(
                line_texts(&serialize_live_rows(&mut streamed, width)),
                line_texts(&serialize_live_rows(&mut whole, width)),
                "stream boundaries must not change list structure"
            );
            streamed.status = AppStatus::Ready;
            whole.status = AppStatus::Ready;
            assert_eq!(
                line_texts(&serialize_live_rows(&mut streamed, width)),
                line_texts(&serialize_live_rows(&mut whole, width)),
                "completion must preserve the list"
            );
        }
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
        let remaining =
            serialize_live_rows_with_boundaries_excluding(&mut app, 80, &committed_ids, None);
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
            serialize_live_rows_with_boundaries_excluding(&mut app, 120, &excluded_ids, None);
        let text = line_texts(serialized.rows());

        assert!(!text.iter().any(|line| line.starts_with(" ●")));
        assert!(!text.iter().any(|line| line.contains("Done Tool")));
        assert!(text.iter().any(|line| line.contains("Running Tool")));
        assert_eq!(serialized.stable_row_count(), 0);
        assert_eq!(
            serialized.first_mutable_boundary_kind(),
            Some(LiveRowBoundaryKind::AssistantTool)
        );
    }

    #[test]
    fn empty_assistant_reserves_no_transcript_space_independently_of_activity_and_tips() {
        let mut app = App::test_default();
        app.transcript.messages.push(user_text_message("request"));
        app.transcript.messages.push(assistant_message());
        app.bind_active_turn_assistant(1);
        assert_assistant_marker(&serialize_live_rows(&mut app, 80), false);
        app.status = AppStatus::Running;
        app.begin_turn_activity(std::time::Instant::now());
        for width in [32, 120, 32] {
            let serialized = serialize_all_rows_with_boundaries(&mut app, width);
            assert_assistant_marker(serialized.rows(), false);
            assert!(serialized.segments().iter().all(|segment| segment.msg_idx == 0));
            let stable = &serialized.rows()[..serialized.stable_row_count()];
            assert_assistant_marker(stable, false);
            assert!(line_texts(stable).iter().any(|row| row.ends_with("request")));
            assert_eq!(serialized.first_mutable_boundary_kind(), None);
            assert!(app.transcript.messages[1].blocks.is_empty());
        }
        app.config.snapshot = Some(crate::agent::settings::SettingsSnapshot::test_value(
            "spinnerTipsEnabled",
            serde_json::json!(false),
        ));
        assert_assistant_marker(&serialize_live_rows(&mut app, 80), false);
        app.config.snapshot = None;
        assert_assistant_marker(&serialize_live_rows(&mut app, 80), false);
        app.session_runtime.runtime_session_state =
            Some(model::RuntimeSessionState::RequiresAction);
        assert_assistant_marker(&serialize_live_rows(&mut app, 80), false);
        app.session_runtime.runtime_session_state = Some(model::RuntimeSessionState::Running);
        app.pending_interaction_ids.push("permission".into());
        assert_assistant_marker(&serialize_live_rows(&mut app, 80), false);
        app.pending_interaction_ids.clear();
        assert_assistant_marker(&serialize_live_rows(&mut app, 80), false);
        app.status = AppStatus::Ready;
        assert_assistant_marker(&serialize_live_rows(&mut app, 80), false);
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
            assert!(serialized.rows().is_empty());
            assert!(serialized.segments().is_empty());
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
                assert_eq!(
                    line_texts(serialized.rows()).iter().any(|row| row.contains("Thinking…")),
                    phase == model::AgentActivityPhase::Thinking
                );
                if phase == model::AgentActivityPhase::Thinking {
                    assert_assistant_marker(serialized.rows(), true);
                    assert_eq!(serialized.stable_row_count(), 0);
                    assert_eq!(serialized.rows_excluding_ids(&BTreeSet::new()), serialized.rows());
                    assert!(serialized.segments().iter().all(|segment| !segment.commit_ready));
                } else {
                    assert!(serialized.rows().is_empty());
                    assert!(serialized.segments().is_empty());
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
        assert_assistant_marker(&serialize_live_rows(&mut app, 120), true);
        assert!(
            line_texts(&serialize_live_rows(&mut app, 120))
                .iter()
                .any(|row| row.contains("Thinking\u{2026}"))
        );
        app.pending_interaction_ids.push("permission".into());
        assert!(serialize_live_rows(&mut app, 120).is_empty());
        app.pending_interaction_ids.clear();
        app.turn.cancel_requested = true;
        let cancelling = serialize_live_rows(&mut app, 120);
        assert!(cancelling.is_empty());
        assert!(!line_texts(&cancelling).iter().any(|row| row.contains("Thinking…")));
        app.turn.cancel_requested = false;
        app.turn.compaction.begin();
        let compacting = serialize_live_rows(&mut app, 120);
        assert!(compacting.is_empty());
        assert!(!line_texts(&compacting).iter().any(|row| row.contains("Thinking…")));
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
        assert!(serialize_live_rows(&mut app, 80).is_empty());
        for update in [
            model::SessionUpdate::AgentActivityUpdate(model::AgentActivityPhase::Thinking),
            model::SessionUpdate::AgentActivityUpdate(model::AgentActivityPhase::Working),
        ] {
            session_update(&mut app, update);
            let rows = line_texts(&serialize_live_rows(&mut app, 80));
            assert_eq!(
                rows.iter().filter(|row| row.starts_with(" \u{25cf}")).count(),
                usize::from(app.status == AppStatus::Thinking)
            );
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
        assert_eq!(tool_rows.iter().filter(|row| row.starts_with(" ●")).count(), 1);
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
        assert_eq!(output.iter().filter(|row| row.starts_with(" ●")).count(), 1);
        assert!(output.iter().any(|row| row == "   Actual response"));
        let excluded =
            BTreeSet::from([HistoryOutputId::AssistantLabel(app.transcript.messages[0].id)]);
        app.config.snapshot = None;
        let after_enable =
            serialize_live_rows_with_boundaries_excluding(&mut app, 80, &excluded, None);
        assert!(!line_texts(after_enable.rows()).iter().any(|row| row.starts_with(" ●")));
        assert!(line_texts(after_enable.rows()).iter().any(|row| row == "   Actual response"));
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
        assert_eq!(completed.iter().filter(|row| row.starts_with(" ●")).count(), 1);
        assert!(completed.iter().any(|row| row == "   Actual response"));
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
        let remaining =
            serialize_live_rows_with_boundaries_excluding(&mut app, 120, &excluded, None);
        assert_eq!(remaining.stable_row_count(), 0);
        assert!(line_texts(remaining.rows()).iter().any(|row| row.contains("Thinking…")));
        assert!(!line_texts(remaining.rows()).iter().any(|row| row.contains("Child Tool")));
        app.status = AppStatus::Running;
        assert!(
            serialize_live_rows_with_boundaries_excluding(&mut app, 120, &excluded, None)
                .rows()
                .iter()
                .all(|row| line_text(row).trim().is_empty())
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
        let assistant_pos = text.iter().position(|line| line == " ● streaming").expect("assistant");
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

        assert!(
            line_texts(serialized.rows()).iter().any(|line| line == " ❯ queued while connecting")
        );
        assert_eq!(serialized.stable_row_count(), 0);
    }

    // The crab fixtures are written out here rather than taken from
    // welcome.rs, so a changed frame table fails these tests.
    const REST_CRAB: [&str; 4] =
        [r"    _~^~^~_     ", r"\) /  o o  \ (/ ", r"  '_   -   _'   ", r"  / '-----' \   "];
    const EYES_LEFT_ROW: &str = r"\) / o o   \ (/ ";
    const EYES_RIGHT_ROW: &str = r"\) /   o o \ (/ ";
    const LEGS_B_ROW: &str = r"  \ '-----' /   ";
    const WAVE_TOP_ROW: &str = r"\)  _~^~^~_  (/ ";

    /// The four crab cells of the rendered welcome: the art sits after the
    /// two-column left padding, 16 columns wide.
    fn crab_rows(rows: &[Line<'_>]) -> Vec<String> {
        let text = line_texts(rows);
        let Some(start) = text.iter().position(|line| line.contains("_~^~^~_")) else {
            return Vec::new();
        };
        text[start..]
            .iter()
            .take(4)
            .map(|line| format!("{line:<18}").chars().skip(2).take(16).collect())
            .collect()
    }

    /// The hello is timed from the first connect, pinned to this instant, and
    /// every render passes an explicit `now`, so no test depends on how fast
    /// it runs.
    struct CrabClock(std::time::Instant);

    impl CrabClock {
        fn new() -> Self {
            Self(std::time::Instant::now())
        }

        fn at(&self, millis: u64) -> std::time::Instant {
            self.0 + std::time::Duration::from_millis(millis)
        }
    }

    fn rows_at(app: &mut App, now: std::time::Instant) -> SerializedLiveRows {
        serialize_live_rows_at(
            app,
            120,
            &BTreeSet::new(),
            now,
            &super::LayoutControl::uninterrupted(),
        )
    }

    /// A welcome whose values are still loading, `spinner_frame` frames into
    /// the connecting loop.
    fn connecting_app(spinner_frame: usize) -> App {
        let mut app = App::test_default();
        app.status = AppStatus::Connecting;
        app.spinner_frame = spinner_frame;
        app.transcript.messages.push(ChatMessage::welcome("1.2.3", "-", "/workspace/demo", "-"));
        app
    }

    /// A ready welcome whose first connect is the clock's base instant, with
    /// the settings snapshot acknowledged.
    fn connected_app(clock: &CrabClock) -> App {
        let mut app = App::test_default();
        app.status = AppStatus::Ready;
        app.startup.mark_connected(clock.0);
        app.config.snapshot = Some(crate::agent::settings::SettingsSnapshot::test_value(
            "prefersReducedMotion",
            serde_json::json!(false),
        ));
        app.transcript.messages.push(ChatMessage::welcome(
            "1.2.3",
            "Pro",
            "/workspace/demo",
            "session-123",
        ));
        app
    }

    fn reduce_motion(app: &mut App) {
        app.config.snapshot = Some(crate::agent::settings::SettingsSnapshot::test_value(
            "prefersReducedMotion",
            serde_json::json!(true),
        ));
    }

    #[test]
    fn connecting_crab_steps_through_poses_by_spinner_frame() {
        let clock = CrabClock::new();
        let mut app = connecting_app(0);
        let now = clock.at(0);
        let crab_at = |app: &mut App, spinner_frame: usize| {
            app.spinner_frame = spinner_frame;
            crab_rows(rows_at(app, now).rows())
        };

        let first = crab_at(&mut app, 0);
        let last_of_first = crab_at(&mut app, 5);
        let second = crab_at(&mut app, 6);
        let third = crab_at(&mut app, 12);
        let wrapped = crab_at(&mut app, 24);

        assert_eq!(first.len(), 4, "{first:?}");
        assert_eq!(last_of_first, first, "a pose lasts 6 spinner frames");
        assert_ne!(first, second);
        assert_eq!(first[1], EYES_LEFT_ROW);
        assert_eq!(second[3], LEGS_B_ROW);
        assert_eq!(third[1], EYES_RIGHT_ROW);
        assert_eq!(wrapped, first, "the loop repeats after four poses");
    }

    #[test]
    fn hello_waves_while_holding_the_welcome_back_from_scrollback() {
        let clock = CrabClock::new();
        let mut app = connected_app(&clock);

        let serialized = rows_at(&mut app, clock.at(0));

        assert_eq!(crab_rows(serialized.rows())[0], WAVE_TOP_ROW);
        assert_eq!(serialized.stable_row_count(), 0);
        let last_instant = rows_at(&mut app, clock.at(1_199));
        assert_eq!(crab_rows(last_instant.rows()), REST_CRAB, "the hello ends at rest");
        assert_eq!(last_instant.stable_row_count(), 0, "still held inside the window");
    }

    #[test]
    fn crab_rests_and_commits_when_the_hello_window_closes() {
        let clock = CrabClock::new();
        let mut app = connected_app(&clock);

        let serialized = rows_at(&mut app, clock.at(1_200));

        assert_eq!(crab_rows(serialized.rows()), REST_CRAB);
        assert_eq!(serialized.stable_row_count(), serialized.rows().len());
    }

    #[test]
    fn a_later_connection_does_not_replay_the_hello() {
        let clock = CrabClock::new();
        let mut app = connected_app(&clock);
        // `/new` or a resume connects again with the welcome as the whole
        // transcript, long after the first hello ended.
        app.startup.mark_connected(clock.at(60_000));

        let serialized = rows_at(&mut app, clock.at(60_100));

        assert_eq!(crab_rows(serialized.rows()), REST_CRAB);
        assert_eq!(serialized.stable_row_count(), serialized.rows().len());
    }

    #[test]
    fn a_narrow_terminal_shows_no_crab_and_does_not_hold_the_welcome_back() {
        let clock = CrabClock::new();
        let mut app = connected_app(&clock);

        let narrow = serialize_live_rows_at(
            &mut app,
            36,
            &BTreeSet::new(),
            clock.at(0),
            &super::LayoutControl::uninterrupted(),
        );

        assert!(crab_rows(narrow.rows()).is_empty(), "no crab column at 36 columns");
        assert_eq!(narrow.stable_row_count(), narrow.rows().len(), "nothing to wait for");
    }

    #[test]
    fn a_prompt_during_the_hello_snaps_the_crab_to_rest_and_commits_it() {
        let clock = CrabClock::new();
        let mut app = connected_app(&clock);
        app.push_message_tracked(user_text_message("typed first"));

        let serialized = rows_at(&mut app, clock.at(100));

        assert_eq!(crab_rows(serialized.rows()), REST_CRAB);
        assert_eq!(serialized.stable_row_count(), serialized.rows().len());
    }

    #[test]
    fn hello_waits_at_rest_until_the_motion_preference_is_known() {
        let clock = CrabClock::new();
        let mut app = connected_app(&clock);
        app.config.snapshot = None;

        let serialized = rows_at(&mut app, clock.at(100));

        assert_eq!(crab_rows(serialized.rows()), REST_CRAB);
        assert_eq!(serialized.stable_row_count(), 0, "held until the hello window closes");
    }

    #[test]
    fn reduced_motion_keeps_the_crab_at_rest() {
        let clock = CrabClock::new();
        let mut connecting = connecting_app(0);
        reduce_motion(&mut connecting);
        assert_eq!(crab_rows(rows_at(&mut connecting, clock.at(0)).rows()), REST_CRAB);

        let mut hello = connected_app(&clock);
        reduce_motion(&mut hello);
        let serialized = rows_at(&mut hello, clock.at(100));
        assert_eq!(crab_rows(serialized.rows()), REST_CRAB);
        assert_eq!(serialized.stable_row_count(), serialized.rows().len());
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

            assert!(
                text.iter().any(|line| line.starts_with(" ●")),
                "missing label at width {width}"
            );
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

            assert!(
                text.iter().any(|line| line.starts_with(" ●")),
                "missing label at width {width}"
            );
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

            assert!(
                text.iter().any(|line| line.starts_with(" ●")),
                "missing label at width {width}"
            );
            assert!(
                text.iter().any(|line| line.contains("Child Tool")),
                "missing tool title at width {width}: {text:?}"
            );
        }
    }
}
