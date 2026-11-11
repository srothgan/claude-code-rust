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
pub(crate) fn serialize_live_rows_at(
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
        .filter(|(_, boundary)| !boundary.kind.uses_assistant_gutter())
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
    let render_context = MessageRenderContext {
        tool_render_context: tool_call::ToolCallRenderContext {
            current_mode_id: render_context.tool_render_context.current_mode_id,
            control: render_context.tool_render_context.control,
            clock: crate::app::presentation::ClockFormat {
                format: app.config.time_format(),
                zone: app.config.time_zone(),
            },
        },
        width: render_context.width,
    };
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
    super::wrap::wrap_lines_with_control(
        &rows,
        render_context.width,
        render_context.tool_render_context.control.unwrap_or(&uninterrupted),
    )
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
        tc.parent_tool_use_id = Some("parent-agent".into());
        tc.cache.invalidate();
        assert!(cursor.next(&mut app, 80, &BTreeSet::new(), &control).is_none());
        assert!(control.cancelled());
        assert!(
            !serialize_live_rows(&mut app, 80)
                .iter()
                .any(|row| row.to_string().contains("STALE BODY"))
        );
    }

    use super::{
        LiveRowBoundaryKind, SerializedLiveRows, serialize_live_rows_with_boundaries_excluding,
    };

    use crate::agent::model;
    use crate::app::{
        App, AppStatus, ChatMessage, HistoryOutputId, MessageBlock, MessageRole, TextBlock,
        TextBlockSpacing, ToolCallInfo,
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

    fn serialize_live_rows(app: &mut App, width: u16) -> Vec<Line<'static>> {
        serialize_all_rows_with_boundaries(app, width).rows().to_vec()
    }

    fn serialize_all_rows_with_boundaries(app: &mut App, width: u16) -> SerializedLiveRows {
        serialize_live_rows_with_boundaries_excluding(app, width, &BTreeSet::new(), None)
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

    fn assistant_text_message(text: &str) -> ChatMessage {
        assistant_blocks_message(vec![MessageBlock::Text(TextBlock::from_complete(text))])
    }

    fn assistant_blocks_message(blocks: Vec<MessageBlock>) -> ChatMessage {
        ChatMessage::new(MessageRole::Assistant, blocks, None)
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
        tool.kind = crate::app::test_support::test_tool_kind(sdk_tool_name);
        block
    }

    #[test]
    fn live_tool_titles_preserve_canonical_sources_across_discard_resize_and_history_exclusion() {
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
            assert!(rows.iter().any(|row| line_text(row).ends_with(expected)));
            assert!(segment.commit_ready);
        }
        let mcp_segment = rendered
            .segments()
            .iter()
            .find(|segment| {
                segment.ids.contains(&HistoryOutputId::ToolCall("mcp-title".to_owned()))
            })
            .expect("MCP tool has its own segment");
        let mcp_rows = rendered.segment_rows(mcp_segment).unwrap();
        assert!(line_text(&mcp_rows[0]).contains("Search MCP documents"));
        assert!(line_text(&mcp_rows[0]).ends_with('…'));
        let replay = serialize_all_rows_with_boundaries(&mut app, 100);
        assert_eq!(replay.rows(), rendered.rows());

        for (idx, source) in [(0, &command), (1, &read_title), (2, &mcp_title)] {
            let MessageBlock::ToolCall(tool) = &mut app.transcript.messages[0].blocks[idx] else {
                unreachable!("seeded tool block")
            };
            assert_eq!(&tool.title, source);
            tool.cache.evict_cached_render();
        }
        let resized = serialize_all_rows_with_boundaries(&mut app, 24);
        assert!(resized.rows().iter().all(|row| row.width() <= 24));
        let restored = serialize_all_rows_with_boundaries(&mut app, 100);
        assert_eq!(restored.rows(), rendered.rows());
        for (idx, source) in [(0, &command), (1, &read_title), (2, &mcp_title)] {
            let MessageBlock::ToolCall(tool) = &app.transcript.messages[0].blocks[idx] else {
                unreachable!("seeded tool block")
            };
            assert_eq!(
                &tool.title, source,
                "resize and replay preserve the complete canonical title"
            );
        }

        let confirmed: BTreeSet<_> =
            restored.segments().iter().flat_map(|segment| segment.ids.iter().cloned()).collect();
        let live = serialize_live_rows_with_boundaries_excluding(&mut app, 100, &confirmed, None);
        assert!(live.rows().is_empty(), "confirmed tool titles must not reappear in live output");
    }

    fn tool_call_block_with_status_interaction(
        id: &str,
        status: model::ToolCallStatus,
        hidden: bool,
        focused_permission: bool,
        focused_question: bool,
    ) -> MessageBlock {
        let mut tool = ToolCallInfo {
            title: "Child Tool".to_owned(),
            parent_tool_use_id: (hidden).then(|| "parent-agent".to_owned()),
            ..crate::app::test_support::named_tool_call_info(id, "Bash", status)
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
        tool.kind = model::ToolKind::Other;
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

    fn child_interaction_region(serialized: &SerializedLiveRows, id: &str) -> Vec<Line<'static>> {
        let segment = serialized
            .segments()
            .iter()
            .find(|segment| segment.ids.contains(&HistoryOutputId::ToolCall(id.to_owned())))
            .expect("child's own transcript segment");
        assert!(!segment.commit_ready, "pending interaction must remain mutable");
        serialized.segment_rows(segment).expect("child rows").to_vec()
    }

    struct ChildInteractionFixture {
        app: App,
        permission_rx: Option<tokio::sync::oneshot::Receiver<model::RequestPermissionResponse>>,
        question_rx: Option<tokio::sync::oneshot::Receiver<model::RequestQuestionResponse>>,
        before: Option<Vec<String>>,
    }

    fn deliver_child_parent_metadata(app: &mut App) {
        crate::app::handle_client_event(
            app,
            crate::agent::events::ClientEvent::SessionUpdate {
                session_id: "interaction-render".into(),
                update: model::SessionUpdate::ToolCallUpdate(
                    model::ToolCallUpdate::new("child-request", model::ToolCallUpdateFields::new())
                        .meta(serde_json::json!({"claudeCode":{"parentToolUseId":"parent-agent"}})),
                ),
            },
        );
    }

    fn child_interaction_fixture(question: bool, late_parent: bool) -> ChildInteractionFixture {
        use crate::ui::tool_call::test_support::{
            attach_permission, attach_question, interaction_app,
        };
        let mut child = crate::app::test_support::named_tool_call_info(
            "child-request",
            if question { "AskUserQuestion" } else { "Write" },
            model::ToolCallStatus::InProgress,
        );
        child.title = "Child operation".into();
        child.parent_tool_use_id = (!late_parent).then(|| "parent-agent".into());
        let mut neighbor = crate::app::test_support::named_tool_call_info(
            "neighbor",
            "Read",
            model::ToolCallStatus::Completed,
        );
        neighbor.title = "Neighbor result".into();
        neighbor.content = vec![model::ToolCallContent::from("Neighbor-only content")];
        let mut app = interaction_app(vec![neighbor, child]);
        let before = (!late_parent)
            .then(|| line_texts(serialize_all_rows_with_boundaries(&mut app, 100).rows()));
        if let Some(before) = &before {
            assert!(before.iter().any(|row| row.contains("Neighbor-only content")));
            assert!(!before.iter().any(|row| row.contains("Child operation")));
        }
        app.input.set_text("unfinished user draft");
        let (permission_rx, question_rx) = if question {
            (
                None,
                Some(attach_question(
                    &mut app,
                    "child-request",
                    model::QuestionPrompt::new(
                        "Choose the child's tree",
                        "Child choice",
                        false,
                        vec![
                            model::QuestionOption::new("oak-c", "Oak-C"),
                            model::QuestionOption::new("elm-c", "Elm-C"),
                            model::QuestionOption::new("ash-c", "Ash-C"),
                        ],
                    ),
                )),
            )
        } else {
            (
                Some(attach_permission(
                    &mut app,
                    "child-request",
                    vec![
                        model::PermissionOption::new(
                            "once-c",
                            "Once-C",
                            model::PermissionOptionKind::AllowOnce,
                        ),
                        model::PermissionOption::new(
                            "save-c",
                            "Save-C",
                            model::PermissionOptionKind::AllowSession,
                        ),
                        model::PermissionOption::new(
                            "deny-c",
                            "Deny-C",
                            model::PermissionOptionKind::RejectOnce,
                        ),
                    ],
                    None,
                )),
                None,
            )
        };
        if late_parent {
            // The request arrives before parent metadata: absent context is reachable.
            deliver_child_parent_metadata(&mut app);
            assert!(
                app.tool_call("child-request")
                    .unwrap()
                    .pending_permission
                    .as_ref()
                    .unwrap()
                    .subagent_context
                    .is_none()
            );
        }
        ChildInteractionFixture { app, permission_rx, question_rx, before }
    }

    #[test]
    fn hidden_child_controls_remain_targeted_mutable_and_disappear_after_settlement() {
        use crate::ui::tool_call::test_support::{assert_focused_option, press};
        use crossterm::event::KeyCode;
        for (question, late_parent) in [(false, false), (false, true), (true, false)] {
            let ChildInteractionFixture { mut app, mut permission_rx, mut question_rx, before } =
                child_interaction_fixture(question, late_parent);
            let labels =
                if question { ["Oak-C", "Elm-C", "Ash-C"] } else { ["Once-C", "Save-C", "Deny-C"] };
            for width in [48, 100] {
                let serialized = serialize_all_rows_with_boundaries(&mut app, width);
                let rows = child_interaction_region(&serialized, "child-request");
                assert!(
                    !rows
                        .iter()
                        .flat_map(|row| &row.spans)
                        .any(|span| labels.contains(&span.content.as_ref()))
                );
                if question {
                    assert!(
                        line_texts(&rows).iter().any(|row| row.contains("Choose the child's tree"))
                    );
                } else {
                    assert!(line_texts(&rows).iter().any(|row| row.contains("Child operation")));
                }
                assert!(!line_texts(&rows).iter().any(|row| row.contains("Neighbor-only content")));
            }
            app.claim_focus_target(crate::app::FocusTarget::Permission);
            app.rebuild_chat_focus_from_state();
            for (selected, width) in [(0, 48), (1, 100), (1, 48)] {
                if selected == 1 && width == 100 {
                    press(&mut app, KeyCode::Right);
                }
                let serialized = serialize_all_rows_with_boundaries(&mut app, width);
                let rows = child_interaction_region(&serialized, "child-request");
                assert_focused_option(&rows, labels, selected);
                let neighbor = serialized
                    .segments()
                    .iter()
                    .find(|segment| {
                        segment.ids.contains(&HistoryOutputId::ToolCall("neighbor".into()))
                    })
                    .expect("neighbor segment");
                assert!(
                    !serialized
                        .segment_rows(neighbor)
                        .unwrap()
                        .iter()
                        .flat_map(|row| &row.spans)
                        .any(|span| labels.contains(&span.content.as_ref()))
                );
            }
            press(&mut app, KeyCode::Enter);
            if let Some(rx) = permission_rx.as_mut() {
                assert_eq!(
                    rx.try_recv().expect("child permission answer").outcome,
                    model::RequestPermissionOutcome::Selected(
                        model::SelectedPermissionOutcome::new("save-c")
                    )
                );
            }
            if let Some(rx) = question_rx.as_mut() {
                let model::RequestQuestionOutcome::Answered(answer) =
                    rx.try_recv().expect("child question answer").outcome
                else {
                    panic!("answer");
                };
                assert_eq!(answer.selected_option_ids, ["elm-c"]);
            }
            let settled = serialize_all_rows_with_boundaries(&mut app, 100);
            assert!(!settled.segments().iter().any(|segment| {
                segment.ids.contains(&HistoryOutputId::ToolCall("child-request".into()))
            }));
            let text = line_texts(settled.rows());
            assert!(text.iter().any(|row| row.contains("Neighbor-only content")));
            for label in labels {
                assert!(!text.iter().any(|row| row.contains(label)));
            }
            if let Some(before) = before {
                assert_eq!(
                    text, before,
                    "settlement must not leave an orphan child marker or controls"
                );
            }
        }
    }
}

#[cfg(test)]
mod lifecycle_tests;

#[cfg(test)]
mod welcome_tests;

#[cfg(test)]
mod layout_tests;

#[cfg(test)]
mod presentation_tests;
