// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use crate::app::handoff::projection::{
    InlineOutputId, InlineOutputItem, InlineOutputItemKind, InlineOutputStatus,
    inline_live_projection, inline_live_projection_after_static_insert,
};
use crate::app::handoff::types::{
    AssistantCommittedUnit, AssistantTranscriptEntry, CommittedAssistantKind,
    LiveAssistantIndicator, LiveAssistantUnit, TerminalMutationState, ToolTranscriptSnapshot,
    TranscriptEntry, UserTranscriptBlock, WelcomeTranscriptEntry,
};
use crate::app::{
    App, BlockCache, ChatMessage, ImageAttachmentBlock, MessageBlock, MessageRole, NoticeBlock,
    SystemSeverity, TerminalSnapshotMode, TextBlock, TextBlockSpacing, ToolCallInfo,
};
use crate::ui::message::{MessageRenderContext, SpinnerState, render_text_block_cached};
use crate::ui::message_rows::{MessageRowSegment, build_user_system_message_rows};
use crate::ui::theme;
use crate::ui::tool_call;
use crate::ui::welcome;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Widget, Wrap};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TopLevelInlineBlockKind {
    ExistingHistory,
    Welcome,
    User,
    System,
    Assistant,
}

pub(crate) fn serialize_transcript_rows(
    app: &App,
    entries: &[TranscriptEntry],
    has_prior_committed_history: bool,
    width: u16,
) -> Vec<Line<'static>> {
    serialize_transcript_row_batches(app, entries, has_prior_committed_history, width)
        .into_iter()
        .flatten()
        .collect()
}

pub(crate) fn serialize_transcript_row_batches(
    app: &App,
    entries: &[TranscriptEntry],
    has_prior_committed_history: bool,
    width: u16,
) -> Vec<Vec<Line<'static>>> {
    let mut batches = Vec::new();
    let mut previous_block_kind =
        has_prior_committed_history.then_some(TopLevelInlineBlockKind::ExistingHistory);
    extend_serialized_transcript_row_batches(
        app,
        entries,
        &mut previous_block_kind,
        width,
        &mut batches,
    );
    batches
}

fn extend_serialized_transcript_row_batches(
    app: &App,
    entries: &[TranscriptEntry],
    previous_block_kind: &mut Option<TopLevelInlineBlockKind>,
    width: u16,
    batches: &mut Vec<Vec<Line<'static>>>,
) {
    let current_mode_id = app.mode.as_ref().map(|mode| mode.current_mode_id.as_str());
    let mut idx = 0usize;
    while idx < entries.len() {
        match &entries[idx] {
            TranscriptEntry::AssistantOpen(_) | TranscriptEntry::AssistantContinue(_) => {
                let start_idx = idx;
                while idx < entries.len()
                    && matches!(
                        entries[idx],
                        TranscriptEntry::AssistantOpen(_) | TranscriptEntry::AssistantContinue(_)
                    )
                {
                    idx += 1;
                }
                let batch_kind = TopLevelInlineBlockKind::Assistant;
                let batch_rows = serialize_assistant_transcript_batch(
                    entries[start_idx..idx].iter(),
                    current_mode_id,
                    width,
                );
                let mut rows = Vec::new();
                if !batch_rows.is_empty() {
                    rows.extend(
                        std::iter::repeat_with(Line::default)
                            .take(top_level_leading_blank_lines(*previous_block_kind, batch_kind)),
                    );
                    rows.extend(batch_rows);
                    *previous_block_kind = Some(batch_kind);
                }
                batches.push(rows);
            }
            entry => {
                let block_kind = transcript_entry_block_kind(entry);
                let mut rows = Vec::new();
                if let TranscriptEntry::Welcome(welcome) = entry {
                    let welcome_rows = serialize_compact_welcome_entry(app, welcome, width);
                    if !welcome_rows.is_empty() {
                        rows.extend(
                            std::iter::repeat_with(Line::default).take(
                                top_level_leading_blank_lines(*previous_block_kind, block_kind),
                            ),
                        );
                        rows.extend(welcome_rows);
                        *previous_block_kind = Some(block_kind);
                    }
                } else {
                    let entry_rows =
                        serialize_single_transcript_entry(entry, current_mode_id, width);
                    if !entry_rows.is_empty() {
                        rows.extend(
                            std::iter::repeat_with(Line::default).take(
                                top_level_leading_blank_lines(*previous_block_kind, block_kind),
                            ),
                        );
                        rows.extend(entry_rows);
                        *previous_block_kind = Some(block_kind);
                    }
                }
                batches.push(rows);
                idx += 1;
            }
        }
    }
}

fn extend_serialized_transcript_rows(
    app: &App,
    entries: &[TranscriptEntry],
    previous_block_kind: &mut Option<TopLevelInlineBlockKind>,
    width: u16,
    rows: &mut Vec<Line<'static>>,
) {
    let mut batches = Vec::new();
    extend_serialized_transcript_row_batches(
        app,
        entries,
        previous_block_kind,
        width,
        &mut batches,
    );
    rows.extend(batches.into_iter().flatten());
}

pub(crate) fn serialize_live_rows(app: &mut App, width: u16) -> Vec<Line<'static>> {
    let projection = inline_live_projection(app);
    serialize_live_projection_rows(app, projection, width)
}

pub(crate) fn serialize_live_rows_after_static_insert(
    app: &mut App,
    width: u16,
    inserted_ids: &[InlineOutputId],
) -> Vec<Line<'static>> {
    let projection = inline_live_projection_after_static_insert(app, inserted_ids);
    serialize_live_projection_rows(app, projection, width)
}

fn serialize_live_projection_rows(
    app: &mut App,
    projection: Vec<InlineOutputItem>,
    width: u16,
) -> Vec<Line<'static>> {
    let mut rows = Vec::new();
    let mut previous_block_kind = None;

    if let Some(welcome) = uncommitted_live_welcome_entry(app) {
        let welcome_rows = serialize_compact_welcome_entry(app, &welcome, width);
        if !welcome_rows.is_empty() {
            rows.extend(welcome_rows);
            previous_block_kind = Some(TopLevelInlineBlockKind::Welcome);
        }
    }

    if projection.is_empty() {
        return rows;
    }

    let mut transcript_batch = Vec::new();

    for item in projection {
        match item.kind {
            InlineOutputItemKind::Transcript {
                entry,
                status: InlineOutputStatus::PendingInsert,
            } => transcript_batch.push(entry),
            InlineOutputItemKind::Transcript { status: InlineOutputStatus::Inserted, .. } => {}
            InlineOutputItemKind::AssistantLive { msg_idx, turn_id } => {
                flush_live_transcript_batch(
                    app,
                    &mut transcript_batch,
                    &mut previous_block_kind,
                    width,
                    &mut rows,
                );
                let live_rows = serialize_assistant_live_slot(
                    app,
                    msg_idx,
                    turn_id,
                    previous_block_kind,
                    width,
                );
                if !live_rows.is_empty() {
                    rows.extend(live_rows);
                    previous_block_kind = Some(TopLevelInlineBlockKind::Assistant);
                }
            }
        }
    }

    flush_live_transcript_batch(
        app,
        &mut transcript_batch,
        &mut previous_block_kind,
        width,
        &mut rows,
    );
    rows
}

fn flush_live_transcript_batch(
    app: &App,
    transcript_batch: &mut Vec<TranscriptEntry>,
    previous_block_kind: &mut Option<TopLevelInlineBlockKind>,
    width: u16,
    rows: &mut Vec<Line<'static>>,
) {
    if transcript_batch.is_empty() {
        return;
    }

    extend_serialized_transcript_rows(app, transcript_batch, previous_block_kind, width, rows);
    transcript_batch.clear();
}

fn serialize_assistant_live_slot(
    app: &mut App,
    msg_idx: usize,
    turn_id: crate::app::handoff::types::AssistantTurnId,
    previous_block_kind: Option<TopLevelInlineBlockKind>,
    width: u16,
) -> Vec<Line<'static>> {
    if app.active_turn_assistant_idx() != Some(msg_idx) {
        return Vec::new();
    }
    let Some(turn) = app.handoff_shadow.active_turn.as_ref() else {
        return Vec::new();
    };
    if turn.live.turn_id != turn_id {
        return Vec::new();
    }
    if turn.live.units.is_empty() && turn.live.live_indicator.is_none() {
        return Vec::new();
    }
    let current_mode_id = app.mode.as_ref().map(|mode| mode.current_mode_id.clone());
    let formatting = turn.live.formatting.clone();
    let units = turn.live.units.clone();
    let turn_id = turn.live.turn_id.0;
    let indicator = turn.live.live_indicator;
    let spinner = spinner_state_for_live(app.spinner_frame);
    let render_items = assistant_render_items_from_live(&units, formatting.previous_committed_kind);
    let show_label = !formatting.header_printed;
    let rows = render_assistant_rows(AssistantRowsRequest {
        app: Some(app),
        items: render_items,
        indicator,
        current_mode_id: current_mode_id.as_deref(),
        width,
        spinner,
        show_label,
        leading_blank_lines: top_level_leading_blank_lines(
            previous_block_kind,
            TopLevelInlineBlockKind::Assistant,
        ),
        has_prior_assistant_content: formatting.header_printed,
    });
    tracing::debug!(
        target: crate::logging::targets::APP_RENDER,
        event_name = "inline_chat_assistant_block_built",
        message = "assistant live block rendered from shared inline assistant layout",
        outcome = "success",
        assistant_turn_id = turn_id,
        show_label,
        leading_blank_lines = 0,
        committed_rendered_rows = 0,
        live_rendered_rows = rows.len(),
        indicator = ?indicator,
        preview = %preview_rows(&rows, 4),
    );
    rows
}

fn serialize_single_transcript_entry(
    entry: &TranscriptEntry,
    current_mode_id: Option<&str>,
    width: u16,
) -> Vec<Line<'static>> {
    let mut message = match entry {
        TranscriptEntry::Welcome(_) => return Vec::new(),
        TranscriptEntry::User(entry) => ChatMessage::new(
            MessageRole::User,
            entry
                .blocks
                .iter()
                .map(|block| match block {
                    UserTranscriptBlock::Text(text) => {
                        MessageBlock::Text(TextBlock::from_complete(text))
                    }
                    UserTranscriptBlock::ImageAttachment { count } => {
                        MessageBlock::ImageAttachment(ImageAttachmentBlock::new(*count))
                    }
                })
                .collect(),
            None,
        ),
        TranscriptEntry::System(entry) => ChatMessage::new(
            MessageRole::System(entry.severity),
            vec![MessageBlock::Text(TextBlock::from_complete(&entry.text))],
            None,
        ),
        TranscriptEntry::AssistantOpen(_) | TranscriptEntry::AssistantContinue(_) => {
            return Vec::new();
        }
    };

    let rendered = build_user_system_message_rows(
        &mut message,
        message_render_context(current_mode_id, width),
    );
    segments_to_physical_rows(&rendered.segments, width, false)
}

fn serialize_compact_welcome_entry(
    app: &App,
    entry: &WelcomeTranscriptEntry,
    width: u16,
) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(Span::styled(
        "Overview",
        Style::default().fg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD),
    ))];
    lines.extend(welcome::overview_lines(
        &crate::app::WelcomeBlock {
            version: entry.version.clone(),
            subscription: entry.subscription.clone(),
            cwd: entry.cwd.clone(),
            session_id: entry.session_id.clone(),
            tip_seed: entry.tip_seed,
            cache: BlockCache::default(),
        },
        Some(status_label(app)),
    ));

    wrap_lines_to_physical_rows(&lines, width)
}

fn uncommitted_live_welcome_entry(app: &App) -> Option<WelcomeTranscriptEntry> {
    if !app.show_session_overview {
        return None;
    }
    if inline_output_contains_welcome(&app.handoff_shadow.inline_output) {
        return None;
    }

    let first = app.messages.first()?;
    if !matches!(first.role, MessageRole::Welcome) {
        return None;
    }
    let MessageBlock::Welcome(welcome) = first.blocks.first()? else {
        return None;
    };
    if welcome_metadata_ready(welcome) {
        return None;
    }

    Some(WelcomeTranscriptEntry {
        version: welcome.version.clone(),
        subscription: welcome.subscription.clone(),
        cwd: welcome.cwd.clone(),
        session_id: welcome.session_id.clone(),
        tip_seed: welcome.tip_seed,
    })
}

fn inline_output_contains_welcome(
    inline_output: &crate::app::handoff::projection::InlineOutputState,
) -> bool {
    inline_output.items().iter().any(|item| {
        matches!(
            &item.kind,
            InlineOutputItemKind::Transcript { entry: TranscriptEntry::Welcome(_), .. }
        )
    })
}

fn welcome_metadata_ready(welcome: &crate::app::WelcomeBlock) -> bool {
    !welcome.session_id.trim().is_empty()
        && welcome.session_id != "-"
        && !welcome.subscription.trim().is_empty()
        && welcome.subscription != "-"
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

fn serialize_assistant_transcript_batch<'a>(
    entries: impl Iterator<Item = &'a TranscriptEntry>,
    current_mode_id: Option<&str>,
    width: u16,
) -> Vec<Line<'static>> {
    let entries = entries.collect::<Vec<_>>();
    if entries.is_empty() {
        return Vec::new();
    }
    let show_label = matches!(entries.first(), Some(TranscriptEntry::AssistantOpen(_)));
    let rows = render_assistant_rows(AssistantRowsRequest {
        app: None,
        items: assistant_render_items_from_committed(&entries),
        indicator: None,
        current_mode_id,
        width,
        spinner: idle_spinner(),
        show_label,
        leading_blank_lines: 0,
        has_prior_assistant_content: !show_label,
    });
    tracing::debug!(
        target: crate::logging::targets::APP_RENDER,
        event_name = "inline_chat_assistant_block_built",
        message = "assistant transcript block rendered from shared inline assistant layout",
        outcome = "success",
        assistant_turn_id = tracing::field::Empty,
        show_label,
        leading_blank_lines = 0,
        committed_rendered_rows = rows.len(),
        live_rendered_rows = 0,
        indicator = "none",
        preview = %preview_rows(&rows, 4),
    );
    rows
}

const fn transcript_entry_block_kind(entry: &TranscriptEntry) -> TopLevelInlineBlockKind {
    match entry {
        TranscriptEntry::Welcome(_) => TopLevelInlineBlockKind::Welcome,
        TranscriptEntry::User(_) => TopLevelInlineBlockKind::User,
        TranscriptEntry::System(_) => TopLevelInlineBlockKind::System,
        TranscriptEntry::AssistantOpen(_) | TranscriptEntry::AssistantContinue(_) => {
            TopLevelInlineBlockKind::Assistant
        }
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
    LiveTool(crate::app::handoff::types::LiveToolUnit),
    CommittedTool(ToolTranscriptSnapshot),
}

struct AssistantRenderItemSpec {
    leading_blank_lines: usize,
    item: AssistantRenderItem,
}

struct PendingAssistantTextRun {
    leading_blank_lines: usize,
    text: String,
    trailing_spacing: TextBlockSpacing,
}

impl PendingAssistantTextRun {
    fn new(leading_blank_lines: usize, text: &str, trailing_spacing: TextBlockSpacing) -> Self {
        Self { leading_blank_lines, text: text.to_owned(), trailing_spacing }
    }

    fn append(&mut self, text: &str, trailing_spacing: TextBlockSpacing) {
        append_text_run(&mut self.text, self.trailing_spacing, text);
        self.trailing_spacing = trailing_spacing;
    }

    fn into_render_item(self) -> AssistantRenderItemSpec {
        AssistantRenderItemSpec {
            leading_blank_lines: self.leading_blank_lines,
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

fn assistant_render_items_from_committed(
    entries: &[&TranscriptEntry],
) -> Vec<AssistantRenderItemSpec> {
    let mut items = Vec::with_capacity(entries.len());
    let mut pending_text: Option<PendingAssistantTextRun> = None;

    for entry in entries {
        let (TranscriptEntry::AssistantOpen(entry) | TranscriptEntry::AssistantContinue(entry)) =
            entry
        else {
            continue;
        };

        match &entry.unit {
            AssistantCommittedUnit::Text(text) => {
                if entry.leading_blank_lines == 0
                    && let Some(pending) = pending_text.as_mut()
                {
                    pending.append(&text.text, text.trailing_spacing);
                } else {
                    flush_pending_text_run(&mut pending_text, &mut items);
                    pending_text = Some(PendingAssistantTextRun::new(
                        usize::from(entry.leading_blank_lines),
                        &text.text,
                        text.trailing_spacing,
                    ));
                }
            }
            AssistantCommittedUnit::Notice(_) | AssistantCommittedUnit::Tool(_) => {
                flush_pending_text_run(&mut pending_text, &mut items);
                items.push(AssistantRenderItemSpec {
                    leading_blank_lines: usize::from(entry.leading_blank_lines),
                    item: assistant_render_item_from_committed(entry),
                });
            }
        }
    }

    flush_pending_text_run(&mut pending_text, &mut items);
    items
}

fn assistant_render_items_from_live(
    units: &[LiveAssistantUnit],
    initial_previous_kind: Option<CommittedAssistantKind>,
) -> Vec<AssistantRenderItemSpec> {
    let mut items = Vec::with_capacity(units.len());
    let mut pending_text: Option<PendingAssistantTextRun> = None;
    let mut previous_kind = initial_previous_kind;
    for unit in ordered_live_units_for_render(units) {
        if let Some((text, trailing_spacing)) = plain_text_unit(unit) {
            if let Some(pending) = pending_text.as_mut() {
                pending.append(text, trailing_spacing);
            } else {
                let current_kind = CommittedAssistantKind::TextLike;
                let leading_blank_lines = leading_blank_lines_between(previous_kind, current_kind);
                pending_text =
                    Some(PendingAssistantTextRun::new(leading_blank_lines, text, trailing_spacing));
                previous_kind = Some(current_kind);
            }
            continue;
        }

        flush_pending_text_run(&mut pending_text, &mut items);
        let current_kind = live_unit_kind(unit);
        let leading_blank_lines = leading_blank_lines_between(previous_kind, current_kind);
        items.push(AssistantRenderItemSpec {
            leading_blank_lines,
            item: assistant_render_item_from_live(unit),
        });
        previous_kind = Some(current_kind);
    }
    flush_pending_text_run(&mut pending_text, &mut items);
    items
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

fn plain_text_unit(unit: &LiveAssistantUnit) -> Option<(&str, TextBlockSpacing)> {
    match unit {
        LiveAssistantUnit::StableText(text) => Some((&text.text, text.trailing_spacing)),
        LiveAssistantUnit::MutableTextTail(text) => Some((&text.text, TextBlockSpacing::None)),
        LiveAssistantUnit::Notice(_) | LiveAssistantUnit::Tool(_) => None,
    }
}

fn leading_blank_lines_between(
    previous_kind: Option<CommittedAssistantKind>,
    current_kind: CommittedAssistantKind,
) -> usize {
    match (previous_kind, current_kind) {
        (None, _)
        | (Some(CommittedAssistantKind::TextLike), CommittedAssistantKind::TextLike)
        | (Some(CommittedAssistantKind::Tool), CommittedAssistantKind::Tool) => 0,
        (Some(CommittedAssistantKind::TextLike), CommittedAssistantKind::Tool)
        | (Some(CommittedAssistantKind::Tool), CommittedAssistantKind::TextLike) => 1,
    }
}

fn ordered_live_units_for_render(units: &[LiveAssistantUnit]) -> Vec<&LiveAssistantUnit> {
    let Some(hidden_idx) = units.iter().position(is_hidden_tool_unit) else {
        return units.iter().collect();
    };
    let Some(last_later_root_idx) = units
        .iter()
        .enumerate()
        .skip(hidden_idx + 1)
        .filter_map(|(idx, unit)| is_visible_subagent_root_unit(unit).then_some(idx))
        .next_back()
    else {
        return units.iter().collect();
    };

    let hidden_unit = &units[hidden_idx];
    let mut ordered = Vec::with_capacity(units.len());
    for (idx, unit) in units.iter().enumerate() {
        if idx == hidden_idx {
            continue;
        }
        ordered.push(unit);
        if idx == last_later_root_idx {
            ordered.push(hidden_unit);
        }
    }
    ordered
}

fn is_hidden_tool_unit(unit: &LiveAssistantUnit) -> bool {
    matches!(unit, LiveAssistantUnit::Tool(tool) if tool.snapshot.hidden)
}

fn is_visible_subagent_root_unit(unit: &LiveAssistantUnit) -> bool {
    matches!(
        unit,
        LiveAssistantUnit::Tool(tool)
            if !tool.snapshot.hidden
                && matches!(tool.snapshot.sdk_tool_name.as_str(), "Task" | "Agent")
    )
}

fn assistant_render_item_from_committed(entry: &AssistantTranscriptEntry) -> AssistantRenderItem {
    match &entry.unit {
        AssistantCommittedUnit::Text(text) => AssistantRenderItem::Text(
            TextBlock::from_complete(&text.text).with_trailing_spacing(text.trailing_spacing),
        ),
        AssistantCommittedUnit::Notice(notice) => AssistantRenderItem::Notice(NoticeBlock {
            severity: notice.severity,
            text: TextBlock::from_complete(&notice.text)
                .with_trailing_spacing(notice.trailing_spacing),
            dedup_key: None,
        }),
        AssistantCommittedUnit::Tool(tool) => {
            AssistantRenderItem::CommittedTool(tool.snapshot.clone())
        }
    }
}

fn assistant_render_item_from_live(unit: &LiveAssistantUnit) -> AssistantRenderItem {
    match unit {
        LiveAssistantUnit::StableText(text) => AssistantRenderItem::Text(
            TextBlock::from_complete(&text.text).with_trailing_spacing(text.trailing_spacing),
        ),
        LiveAssistantUnit::MutableTextTail(text) => {
            AssistantRenderItem::Text(TextBlock::from_complete(&text.text))
        }
        LiveAssistantUnit::Notice(notice) => AssistantRenderItem::Notice(NoticeBlock {
            severity: notice.severity,
            text: TextBlock::from_complete(&notice.text)
                .with_trailing_spacing(notice.trailing_spacing),
            dedup_key: notice.dedup_key.clone(),
        }),
        LiveAssistantUnit::Tool(tool) => AssistantRenderItem::LiveTool(tool.clone()),
    }
}

fn live_unit_kind(unit: &LiveAssistantUnit) -> CommittedAssistantKind {
    match unit {
        LiveAssistantUnit::StableText(_)
        | LiveAssistantUnit::MutableTextTail(_)
        | LiveAssistantUnit::Notice(_) => CommittedAssistantKind::TextLike,
        LiveAssistantUnit::Tool(_) => CommittedAssistantKind::Tool,
    }
}

struct AssistantRowsRequest<'a> {
    app: Option<&'a mut App>,
    items: Vec<AssistantRenderItemSpec>,
    indicator: Option<LiveAssistantIndicator>,
    current_mode_id: Option<&'a str>,
    width: u16,
    spinner: SpinnerState,
    show_label: bool,
    leading_blank_lines: usize,
    has_prior_assistant_content: bool,
}

fn render_assistant_rows(mut request: AssistantRowsRequest<'_>) -> Vec<Line<'static>> {
    let render_context = message_render_context(request.current_mode_id, request.width);
    let mut rows = Vec::new();
    rows.extend(std::iter::repeat_with(Line::default).take(request.leading_blank_lines));
    if request.show_label {
        rows.extend(wrap_lines_to_physical_rows(&[assistant_role_label_line()], request.width));
    }

    let mut state = AssistantInlineLayoutState {
        has_body_content: request.has_prior_assistant_content,
        has_visible_content: request.has_prior_assistant_content,
    };

    for item in request.items {
        match item.item {
            AssistantRenderItem::Text(block) => {
                let trailing_gap = block.trailing_blank_lines();
                let rendered =
                    render_assistant_text_block(block, request.width, !state.has_visible_content);
                if !rendered.is_empty() {
                    rows.extend(
                        std::iter::repeat_with(Line::default).take(item.leading_blank_lines),
                    );
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
                    rows.extend(
                        std::iter::repeat_with(Line::default).take(item.leading_blank_lines),
                    );
                    state.has_body_content = true;
                    state.has_visible_content = true;
                    rows.extend(rendered);
                    rows.extend(std::iter::repeat_with(Line::default).take(trailing_gap));
                }
            }
            AssistantRenderItem::LiveTool(tool) => {
                let Some(app) = request.app.as_deref_mut() else {
                    continue;
                };
                let rendered = render_live_tool_rows(app, &tool, render_context, request.spinner);
                if !rendered.is_empty() {
                    rows.extend(
                        std::iter::repeat_with(Line::default).take(item.leading_blank_lines),
                    );
                    state.has_body_content = true;
                    state.has_visible_content = true;
                    rows.extend(rendered);
                }
            }
            AssistantRenderItem::CommittedTool(snapshot) => {
                let rendered =
                    render_committed_tool_rows(&snapshot, render_context, request.spinner);
                if !rendered.is_empty() {
                    rows.extend(
                        std::iter::repeat_with(Line::default).take(item.leading_blank_lines),
                    );
                    state.has_body_content = true;
                    state.has_visible_content = true;
                    rows.extend(rendered);
                }
            }
        }
    }

    match request.indicator {
        Some(LiveAssistantIndicator::Compacting) => {
            if state.has_body_content {
                rows.push(Line::default());
            }
            rows.extend(wrap_lines_to_physical_rows(
                &[compacting_line(request.spinner.frame)],
                request.width,
            ));
        }
        Some(LiveAssistantIndicator::Thinking { verb }) => {
            if state.has_body_content {
                rows.push(Line::default());
            }
            rows.extend(wrap_lines_to_physical_rows(
                &[thinking_line(request.spinner.frame, verb)],
                request.width,
            ));
        }
        None => {}
    }

    if !state.has_visible_content && request.indicator.is_none() {
        return Vec::new();
    }

    trim_trailing_blank_rows(rows)
}

fn tool_call_info_from_snapshot(
    snapshot: &ToolTranscriptSnapshot,
    terminal_mutation: TerminalMutationState,
) -> ToolCallInfo {
    ToolCallInfo {
        id: snapshot.tool_call_id.clone(),
        title: snapshot.title.clone(),
        sdk_tool_name: snapshot.sdk_tool_name.clone(),
        raw_input: snapshot.raw_input.clone(),
        raw_input_bytes: snapshot
            .raw_input
            .as_ref()
            .map_or(0, ToolCallInfo::estimate_json_value_bytes),
        output_metadata: snapshot.output_metadata.clone(),
        task_metadata: snapshot.task_metadata.clone(),
        status: snapshot.status,
        content: snapshot.content.clone(),
        hidden: snapshot.hidden,
        terminal_id: None,
        terminal_command: snapshot.terminal_command.clone(),
        terminal_output: snapshot.terminal_output.clone(),
        terminal_output_len: snapshot.terminal_output.as_ref().map_or(0, String::len),
        terminal_bytes_seen: snapshot.terminal_output.as_ref().map_or(0, String::len),
        terminal_snapshot_mode: match terminal_mutation {
            TerminalMutationState::Streaming => TerminalSnapshotMode::AppendOnly,
            TerminalMutationState::AwaitingFinalSnapshot => TerminalSnapshotMode::ReplaceSnapshot,
            TerminalMutationState::None | TerminalMutationState::Settled => {
                TerminalSnapshotMode::AppendOnly
            }
        },
        cache: BlockCache::default(),
        pending_permission: None,
        pending_question: None,
    }
}

fn spinner_state_for_live(frame: usize) -> SpinnerState {
    SpinnerState { frame }
}

fn message_render_context(current_mode_id: Option<&str>, width: u16) -> MessageRenderContext<'_> {
    MessageRenderContext::new(current_mode_id, width)
}

fn idle_spinner() -> SpinnerState {
    SpinnerState { frame: 0 }
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

fn render_live_tool_rows(
    app: &mut App,
    tool: &crate::app::handoff::types::LiveToolUnit,
    render_context: MessageRenderContext<'_>,
    spinner: SpinnerState,
) -> Vec<Line<'static>> {
    if let Some((msg_idx, block_idx)) = app.lookup_tool_call(&tool.snapshot.tool_call_id)
        && let Some(MessageBlock::ToolCall(tc)) =
            app.messages.get_mut(msg_idx).and_then(|message| message.blocks.get_mut(block_idx))
    {
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
        return wrap_lines_to_physical_rows(&rows, render_context.width);
    }

    if tool.snapshot.hidden {
        return Vec::new();
    }
    let mut fallback = tool_call_info_from_snapshot(&tool.snapshot, tool.terminal_mutation);
    let mut rows = Vec::new();
    tool_call::render_tool_call_cached(
        &mut fallback,
        render_context.tool_render_context,
        render_context.width,
        spinner.frame,
        &mut rows,
    );
    wrap_lines_to_physical_rows(&rows, render_context.width)
}

fn render_committed_tool_rows(
    snapshot: &ToolTranscriptSnapshot,
    render_context: MessageRenderContext<'_>,
    spinner: SpinnerState,
) -> Vec<Line<'static>> {
    if snapshot.hidden {
        return Vec::new();
    }
    let mut fallback = tool_call_info_from_snapshot(snapshot, TerminalMutationState::Settled);
    let mut rows = Vec::new();
    tool_call::render_tool_call_cached(
        &mut fallback,
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

fn wrap_lines_to_physical_rows(lines: &[Line<'static>], width: u16) -> Vec<Line<'static>> {
    if lines.is_empty() {
        return Vec::new();
    }
    if width == 0 {
        return vec![Line::default(); lines.len()];
    }

    let height = Paragraph::new(Text::from(lines.to_vec()))
        .wrap(Wrap { trim: false })
        .line_count(width)
        .max(1);
    let area = Rect::new(0, 0, width, u16::try_from(height).unwrap_or(u16::MAX));
    let mut buffer = Buffer::empty(area);
    Paragraph::new(Text::from(lines.to_vec())).wrap(Wrap { trim: false }).render(area, &mut buffer);

    (0..area.height).map(|row| buffer_row_to_line(&buffer, area, row)).collect()
}

fn buffer_row_to_line(buf: &Buffer, area: Rect, row: u16) -> Line<'static> {
    let y = area.y.saturating_add(row);
    let mut spans = Vec::new();
    let mut current_style = None;
    let mut current_text = String::new();

    for x in 0..area.width {
        let Some(cell) = buf.cell((area.x.saturating_add(x), y)) else {
            continue;
        };
        let symbol = cell.symbol();
        if symbol.is_empty() {
            continue;
        }
        let style = cell.style();
        match current_style {
            Some(existing) if existing == style => current_text.push_str(symbol),
            Some(existing) => {
                spans
                    .push(ratatui::text::Span::styled(std::mem::take(&mut current_text), existing));
                current_text.push_str(symbol);
                current_style = Some(style);
            }
            None => {
                current_text.push_str(symbol);
                current_style = Some(style);
            }
        }
    }

    if let Some(style) = current_style {
        spans.push(ratatui::text::Span::styled(current_text, style));
    }
    Line::from(spans)
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
        serialize_live_rows, serialize_transcript_row_batches, serialize_transcript_rows,
        thinking_line,
    };
    use crate::agent::model;
    use crate::app::handoff::shadow::ActiveAssistantShadowTurn;
    use crate::app::handoff::types::{
        AssistantCommittedUnit, AssistantTranscriptEntry, AssistantTurnId, CommittedAssistantKind,
        CommittedTextUnit, CommittedToolUnit, LiveAssistantTurn, LiveAssistantUnit, LiveToolUnit,
        LiveUnitId, MutableTextTailUnit, StableTextUnit, TerminalMutationState,
        ToolTranscriptSnapshot, TranscriptEntry, UserTranscriptBlock, UserTranscriptEntry,
        WelcomeTranscriptEntry,
    };
    use crate::app::{
        App, AppStatus, ChatMessage, MessageBlock, MessageRole, TextBlock, TextBlockSpacing,
    };
    use ratatui::text::Line;

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

    fn system_text_message(text: &str) -> ChatMessage {
        ChatMessage::new(
            MessageRole::System(Some(crate::app::SystemSeverity::Info)),
            vec![MessageBlock::Text(TextBlock::from_complete(text))],
            None,
        )
    }

    fn live_turn_with_tail(turn_id: AssistantTurnId, text: &str) -> LiveAssistantTurn {
        let mut live = LiveAssistantTurn::new(turn_id);
        let unit_id = live.allocate_unit_id();
        live.units.push(LiveAssistantUnit::MutableTextTail(MutableTextTailUnit {
            id: unit_id,
            text: text.to_owned(),
        }));
        live
    }

    fn live_turn_with_split_text(
        turn_id: AssistantTurnId,
        first: &str,
        first_spacing: TextBlockSpacing,
        tail: &str,
    ) -> LiveAssistantTurn {
        let mut live = LiveAssistantTurn::new(turn_id);
        live.units.push(LiveAssistantUnit::StableText(StableTextUnit {
            id: LiveUnitId(1),
            text: first.to_owned(),
            trailing_spacing: first_spacing,
        }));
        live.units.push(LiveAssistantUnit::MutableTextTail(MutableTextTailUnit {
            id: LiveUnitId(2),
            text: tail.to_owned(),
        }));
        live.current_text_tail = Some(LiveUnitId(2));
        live
    }

    fn tool_snapshot(id: &str, hidden: bool) -> ToolTranscriptSnapshot {
        ToolTranscriptSnapshot {
            tool_call_id: id.to_owned(),
            title: "Child Tool".to_owned(),
            sdk_tool_name: "Bash".to_owned(),
            status: model::ToolCallStatus::Completed,
            hidden,
            raw_input: None,
            output_metadata: None,
            task_metadata: None,
            content: Vec::new(),
            terminal_command: None,
            terminal_output: None,
        }
    }

    fn hidden_live_tool(id: &str) -> LiveAssistantUnit {
        LiveAssistantUnit::Tool(LiveToolUnit {
            id: LiveUnitId(10),
            snapshot: tool_snapshot(id, true),
            pending_permission: false,
            pending_question: false,
            terminal_mutation: TerminalMutationState::Settled,
        })
    }

    fn install_active_live_turn(
        app: &mut App,
        msg_idx: usize,
        turn_id: AssistantTurnId,
        live: LiveAssistantTurn,
    ) {
        app.bind_active_turn_assistant(msg_idx);
        app.handoff_shadow.active_turn =
            Some(ActiveAssistantShadowTurn { committed_entries: Vec::new(), live });
        app.handoff_shadow.inline_output.record_assistant_live_slot(msg_idx, turn_id);
    }

    #[test]
    fn welcome_transcript_uses_compact_inline_variant() {
        let mut app = App::test_default();
        app.mode = Some(crate::app::ModeState {
            current_mode_id: "plan".to_owned(),
            current_mode_name: "Plan".to_owned(),
            available_modes: Vec::new(),
        });
        let rows = serialize_transcript_rows(
            &app,
            &[TranscriptEntry::Welcome(WelcomeTranscriptEntry {
                version: "1.2.3".to_owned(),
                subscription: "Pro".to_owned(),
                cwd: "/workspace/demo".to_owned(),
                session_id: "session-123".to_owned(),
                tip_seed: 7,
            })],
            false,
            120,
        );
        let text: Vec<String> = rows.iter().map(line_text).collect();

        assert!(text.iter().any(|line| line.contains("Overview")));
        assert!(text.iter().any(|line| line.contains("_~^~^~_")));
        assert!(text.iter().any(|line| line.contains("Version: 1.2.3")));
        assert!(text.iter().any(|line| line.contains("Cwd: /workspace/demo")));
        assert!(text.iter().any(|line| line.contains("Session ID: session-123")));
        assert!(text.iter().any(|line| line.contains("Subscription: Pro")));
        assert!(text.iter().any(|line| line.contains("Tips: ")));
        assert!(!text.iter().any(|line| line.contains("Welcome back to Claude, in Rust!")));
    }

    #[test]
    fn compact_welcome_uses_side_by_side_crab_layout() {
        let compact_rows = serialize_transcript_rows(
            &App::test_default(),
            &[TranscriptEntry::Welcome(WelcomeTranscriptEntry {
                version: "1.2.3".to_owned(),
                subscription: "Pro".to_owned(),
                cwd: "/workspace/demo".to_owned(),
                session_id: "session-123".to_owned(),
                tip_seed: 7,
            })],
            false,
            80,
        );
        let text = line_texts(&compact_rows);

        assert_eq!(text.first().map(String::as_str), Some("Overview"));
        assert!(text.iter().any(|line| line.contains("_~^~^~_")));
        assert!(text.iter().any(|line| line.contains("Version: 1.2.3")));
        assert!(text.iter().any(|line| line.contains("Subscription: Pro")));
        assert!(!text.iter().any(|line| line.contains("Welcome back to Claude, in Rust!")));
    }

    #[test]
    fn transcript_rows_do_not_insert_synthetic_blank_between_user_and_assistant() {
        let app = App::test_default();
        let rows = serialize_transcript_rows(
            &app,
            &[
                TranscriptEntry::User(UserTranscriptEntry {
                    blocks: vec![UserTranscriptBlock::Text("hello".to_owned())],
                }),
                TranscriptEntry::AssistantOpen(AssistantTranscriptEntry {
                    leading_blank_lines: 0,
                    unit: AssistantCommittedUnit::Text(CommittedTextUnit {
                        text: "hi".to_owned(),
                        trailing_spacing: crate::app::TextBlockSpacing::None,
                    }),
                }),
            ],
            false,
            120,
        );
        let text = rows.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(text, vec!["User", "hello", "Claude", "hi"]);
    }

    #[test]
    fn committed_adjacent_text_entries_render_as_one_text_run() {
        let app = App::test_default();
        let rows = serialize_transcript_rows(
            &app,
            &[
                TranscriptEntry::AssistantOpen(AssistantTranscriptEntry {
                    leading_blank_lines: 0,
                    unit: AssistantCommittedUnit::Text(CommittedTextUnit {
                        text: "line 1\n\n".to_owned(),
                        trailing_spacing: TextBlockSpacing::ParagraphBreak,
                    }),
                }),
                TranscriptEntry::AssistantContinue(AssistantTranscriptEntry {
                    leading_blank_lines: 0,
                    unit: AssistantCommittedUnit::Text(CommittedTextUnit {
                        text: "line 2".to_owned(),
                        trailing_spacing: TextBlockSpacing::None,
                    }),
                }),
            ],
            false,
            120,
        );

        assert_eq!(line_texts(&rows), vec!["Claude", "line 1", "line 2"]);
    }

    #[test]
    fn committed_assistant_text_preserves_single_newline_rows() {
        let app = App::test_default();
        let rows = serialize_transcript_rows(
            &app,
            &[TranscriptEntry::AssistantOpen(AssistantTranscriptEntry {
                leading_blank_lines: 0,
                unit: AssistantCommittedUnit::Text(CommittedTextUnit {
                    text: "line 1: ready\nline 2: ready\nline 3: ready".to_owned(),
                    trailing_spacing: TextBlockSpacing::None,
                }),
            })],
            false,
            120,
        );

        assert_eq!(
            line_texts(&rows),
            vec!["Claude", "line 1: ready", "line 2: ready", "line 3: ready"]
        );
    }

    #[test]
    fn transcript_row_batches_preserve_flattened_transcript_rendering() {
        let app = App::test_default();
        let entries = vec![
            TranscriptEntry::User(UserTranscriptEntry {
                blocks: vec![UserTranscriptBlock::Text("hello".to_owned())],
            }),
            TranscriptEntry::AssistantOpen(AssistantTranscriptEntry {
                leading_blank_lines: 0,
                unit: AssistantCommittedUnit::Text(CommittedTextUnit {
                    text: "hi".to_owned(),
                    trailing_spacing: crate::app::TextBlockSpacing::None,
                }),
            }),
            TranscriptEntry::AssistantContinue(AssistantTranscriptEntry {
                leading_blank_lines: 0,
                unit: AssistantCommittedUnit::Text(CommittedTextUnit {
                    text: "again".to_owned(),
                    trailing_spacing: crate::app::TextBlockSpacing::None,
                }),
            }),
        ];

        let flat_rows = serialize_transcript_rows(&app, &entries, false, 120);
        let batched_rows = serialize_transcript_row_batches(&app, &entries, false, 120)
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();

        assert_eq!(line_texts(&batched_rows), line_texts(&flat_rows));
    }

    #[test]
    fn committed_assistant_after_prior_history_starts_without_blank_row() {
        let app = App::test_default();
        let rows = serialize_transcript_rows(
            &app,
            &[TranscriptEntry::AssistantOpen(AssistantTranscriptEntry {
                leading_blank_lines: 0,
                unit: AssistantCommittedUnit::Text(CommittedTextUnit {
                    text: "hi".to_owned(),
                    trailing_spacing: crate::app::TextBlockSpacing::None,
                }),
            })],
            true,
            120,
        );

        assert!(rows.first().is_some_and(|line| !line_text(line).trim().is_empty()));
        assert_eq!(line_text(&rows[0]), "Claude");
    }

    #[test]
    fn live_rows_do_not_start_with_synthetic_blank_row() {
        let mut app = App::test_default();
        let turn_id = AssistantTurnId(1);
        app.messages.push(assistant_message());
        install_active_live_turn(&mut app, 0, turn_id, live_turn_with_tail(turn_id, "hi"));

        let rows = serialize_live_rows(&mut app, 120);

        assert!(rows.first().is_some_and(|line| !line_text(line).trim().is_empty()));
        assert_eq!(line_text(&rows[0]), "Claude");
    }

    #[test]
    fn live_projection_renders_user_row_while_assistant_streams() {
        let mut app = App::test_default();
        app.push_message_tracked(user_text_message("hello"));
        let turn_id = AssistantTurnId(2);
        app.messages.push(assistant_message());
        install_active_live_turn(
            &mut app,
            1,
            turn_id,
            live_turn_with_tail(turn_id, "still streaming"),
        );

        let rows = serialize_live_rows(&mut app, 120);
        let text = line_texts(&rows);

        assert_eq!(text, vec!["User", "hello", "Claude", "still streaming"]);
    }

    #[test]
    fn live_projection_renders_committed_assistant_prefix_before_live_tail() {
        let mut app = App::test_default();
        let turn_id = AssistantTurnId(3);
        app.messages.push(assistant_message());
        let mut live = live_turn_with_tail(turn_id, "tail");
        live.formatting.header_printed = true;
        live.formatting.previous_committed_kind = Some(CommittedAssistantKind::TextLike);
        install_active_live_turn(&mut app, 0, turn_id, live);
        app.handoff_shadow.inline_output.record_assistant_committed_entries(
            0,
            turn_id,
            vec![TranscriptEntry::AssistantOpen(AssistantTranscriptEntry {
                leading_blank_lines: 0,
                unit: AssistantCommittedUnit::Text(CommittedTextUnit {
                    text: "prefix".to_owned(),
                    trailing_spacing: crate::app::TextBlockSpacing::None,
                }),
            })],
        );

        let rows = serialize_live_rows(&mut app, 120);
        let text = line_texts(&rows);

        assert_eq!(text, vec!["Claude", "prefix", "tail"]);
    }

    #[test]
    fn live_adjacent_text_units_render_as_one_text_run() {
        let mut app = App::test_default();
        let turn_id = AssistantTurnId(6);
        app.messages.push(assistant_message());
        install_active_live_turn(
            &mut app,
            0,
            turn_id,
            live_turn_with_split_text(
                turn_id,
                "line 1: ready\n\n",
                TextBlockSpacing::ParagraphBreak,
                "line 2: ready",
            ),
        );

        let rows = serialize_live_rows(&mut app, 120);

        assert_eq!(line_texts(&rows), vec!["Claude", "line 1: ready", "line 2: ready"]);
    }

    #[test]
    fn live_assistant_text_preserves_single_newline_rows() {
        let mut app = App::test_default();
        let turn_id = AssistantTurnId(7);
        app.messages.push(assistant_message());
        install_active_live_turn(
            &mut app,
            0,
            turn_id,
            live_turn_with_tail(turn_id, "line 1: ready\nline 2: ready\nline 3: ready"),
        );

        let rows = serialize_live_rows(&mut app, 120);

        assert_eq!(
            line_texts(&rows),
            vec!["Claude", "line 1: ready", "line 2: ready", "line 3: ready"]
        );
    }

    #[test]
    fn live_projection_keeps_system_row_during_active_assistant_turn() {
        let mut app = App::test_default();
        let turn_id = AssistantTurnId(4);
        app.messages.push(assistant_message());
        install_active_live_turn(&mut app, 0, turn_id, live_turn_with_tail(turn_id, "streaming"));
        app.push_message_tracked(system_text_message("during turn"));

        let rows = serialize_live_rows(&mut app, 120);
        let text = line_texts(&rows);
        let assistant_pos = text.iter().position(|line| line == "streaming").expect("assistant");
        let system_pos = text.iter().position(|line| line == "during turn").expect("system");

        assert!(assistant_pos < system_pos);
    }

    #[test]
    fn live_projection_renders_welcome_once() {
        let mut app = App::test_default();
        app.handoff_shadow.inline_output.record_message_transcript_entries(
            0,
            vec![TranscriptEntry::Welcome(WelcomeTranscriptEntry {
                version: "1.2.3".to_owned(),
                subscription: "Pro".to_owned(),
                cwd: "/workspace/demo".to_owned(),
                session_id: "session-123".to_owned(),
                tip_seed: 7,
            })],
        );

        let rows = serialize_live_rows(&mut app, 120);
        let text = line_texts(&rows);

        assert_eq!(text.iter().filter(|line| line.as_str() == "Overview").count(), 1);
    }

    #[test]
    fn live_projection_renders_uncommitted_loading_welcome() {
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
    fn committed_hidden_tool_transcript_renders_no_rows() {
        let app = App::test_default();
        let rows = serialize_transcript_rows(
            &app,
            &[TranscriptEntry::AssistantOpen(AssistantTranscriptEntry {
                leading_blank_lines: 0,
                unit: AssistantCommittedUnit::Tool(Box::new(CommittedToolUnit {
                    snapshot: tool_snapshot("child-1", true),
                })),
            })],
            false,
            120,
        );

        assert!(rows.is_empty());
    }

    #[test]
    fn stale_hidden_live_tool_renders_no_rows_or_label() {
        let mut app = App::test_default();
        let turn_id = AssistantTurnId(5);
        let mut live = LiveAssistantTurn::new(turn_id);
        live.units.push(hidden_live_tool("child-1"));
        app.messages.push(assistant_message());
        install_active_live_turn(&mut app, 0, turn_id, live);

        let rows = serialize_live_rows(&mut app, 120);

        assert!(rows.is_empty());
    }
}
