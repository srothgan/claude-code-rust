// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use crate::app::{
    App, ChatMessage, MessageBlock, MessageRole, NoticeBlock, SystemSeverity, TextBlock,
    TextBlockSpacing, WelcomeBlock,
};
use crate::ui::message::{MessageRenderContext, SpinnerState, render_text_block_cached};
use crate::ui::message_rows::{MessageRowSegment, build_user_system_message_rows};
use crate::ui::spinner_verbs::random_spinner_verb;
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
    message_boundaries: Vec<LiveMessageBoundary>,
}

impl SerializedLiveRows {
    pub(crate) fn rows(&self) -> &[Line<'static>] {
        &self.rows
    }

    pub(crate) fn stable_row_count_before_message(&self, mutable_msg_idx: Option<usize>) -> usize {
        self.message_boundaries
            .iter()
            .find(|boundary| {
                !boundary.commit_ready
                    || mutable_msg_idx.is_some_and(|msg_idx| boundary.msg_idx >= msg_idx)
            })
            .map_or(self.rows.len(), |boundary| boundary.start_row)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LiveMessageBoundary {
    msg_idx: usize,
    start_row: usize,
    commit_ready: bool,
}

pub(crate) fn serialize_live_rows_with_boundaries(app: &mut App, width: u16) -> SerializedLiveRows {
    let current_mode_id = app.mode.as_ref().map(|mode| mode.current_mode_id.clone());
    let active_msg_idx = app.active_turn_assistant_idx();
    let runtime_indicator = sync_runtime_indicator(app);
    let mut rows = Vec::new();
    let mut message_boundaries = Vec::new();
    let mut previous_block_kind = None;

    for msg_idx in 0..app.messages.len() {
        let role = app.messages[msg_idx].role.clone();
        let block_kind = message_block_kind(&role);
        let commit_ready = message_commit_ready(&app.messages[msg_idx]);

        let message_rows = match role {
            MessageRole::Welcome => serialize_welcome_message(app, msg_idx, width),
            MessageRole::User | MessageRole::System(_) => {
                let rendered = build_user_system_message_rows(
                    &mut app.messages[msg_idx],
                    message_render_context(current_mode_id.as_deref(), width),
                );
                segments_to_physical_rows(&rendered.segments, width, false)
            }
            MessageRole::Assistant => {
                let items = assistant_render_items_from_message(&app.messages[msg_idx], msg_idx);
                let indicator =
                    assistant_runtime_indicator(msg_idx, active_msg_idx, runtime_indicator);
                let spinner = spinner_state_for_live(app.spinner_frame);
                let rows = render_assistant_rows(AssistantRowsRequest {
                    app: Some(app),
                    items,
                    indicator,
                    current_mode_id: current_mode_id.as_deref(),
                    width,
                    spinner,
                    show_label: true,
                    leading_blank_lines: 0,
                    has_prior_assistant_content: false,
                });
                tracing::debug!(
                    target: crate::logging::targets::APP_RENDER,
                    event_name = "inline_chat_assistant_block_built",
                    message = "assistant message block rendered from canonical app.messages",
                    outcome = "success",
                    assistant_turn_id = tracing::field::Empty,
                    show_label = true,
                    leading_blank_lines = 0,
                    committed_rendered_rows = rows.len(),
                    live_rendered_rows = 0,
                    indicator = ?indicator,
                    preview = %preview_rows(&rows, 4),
                );
                rows
            }
        };

        if message_rows.is_empty() {
            continue;
        }

        let start_row = rows.len();
        rows.extend(
            std::iter::repeat_with(Line::default)
                .take(top_level_leading_blank_lines(previous_block_kind, block_kind)),
        );
        message_boundaries.push(LiveMessageBoundary { msg_idx, start_row, commit_ready });
        rows.extend(message_rows);
        previous_block_kind = Some(block_kind);
    }

    SerializedLiveRows { rows, message_boundaries }
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

fn assistant_render_items_from_message(
    message: &ChatMessage,
    msg_idx: usize,
) -> Vec<AssistantRenderItemSpec> {
    let mut items = Vec::with_capacity(message.blocks.len());
    let mut pending_text: Option<PendingAssistantTextRun> = None;
    let mut previous_kind = None;

    for (block_idx, block) in message.blocks.iter().enumerate() {
        match block {
            MessageBlock::Text(text) => {
                if text.text.is_empty() {
                    continue;
                }
                if let Some(pending) = pending_text.as_mut() {
                    pending.append(&text.text, text.trailing_spacing);
                } else {
                    let current_kind = AssistantInlineItemKind::TextLike;
                    let leading_blank_lines =
                        leading_blank_lines_between(previous_kind, current_kind);
                    pending_text = Some(PendingAssistantTextRun::new(
                        leading_blank_lines,
                        &text.text,
                        text.trailing_spacing,
                    ));
                    previous_kind = Some(current_kind);
                }
            }
            MessageBlock::Notice(notice) => {
                flush_pending_text_run(&mut pending_text, &mut items);
                let current_kind = AssistantInlineItemKind::TextLike;
                let leading_blank_lines = leading_blank_lines_between(previous_kind, current_kind);
                items.push(AssistantRenderItemSpec {
                    leading_blank_lines,
                    item: AssistantRenderItem::Notice(NoticeBlock {
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
                    leading_blank_lines,
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
    items: Vec<AssistantRenderItemSpec>,
    indicator: Option<AssistantRuntimeIndicator>,
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
            AssistantRenderItem::CanonicalTool { msg_idx, block_idx } => {
                let Some(app) = request.app.as_deref_mut() else {
                    continue;
                };
                append_rendered_assistant_item(
                    &mut rows,
                    &mut state,
                    item.leading_blank_lines,
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
        &state,
        request.indicator,
        request.spinner,
        request.width,
    );

    if !state.has_visible_content && request.indicator.is_none() {
        return Vec::new();
    }

    trim_trailing_blank_rows(rows)
}

fn append_rendered_assistant_item(
    rows: &mut Vec<Line<'static>>,
    state: &mut AssistantInlineLayoutState,
    leading_blank_lines: usize,
    rendered: Vec<Line<'static>>,
) {
    if rendered.is_empty() {
        return;
    }
    rows.extend(std::iter::repeat_with(Line::default).take(leading_blank_lines));
    state.has_body_content = true;
    state.has_visible_content = true;
    rows.extend(rendered);
}

fn append_assistant_indicator_rows(
    rows: &mut Vec<Line<'static>>,
    state: &AssistantInlineLayoutState,
    indicator: Option<AssistantRuntimeIndicator>,
    spinner: SpinnerState,
    width: u16,
) {
    let line = match indicator {
        Some(AssistantRuntimeIndicator::Compacting) => compacting_line(spinner.frame),
        Some(AssistantRuntimeIndicator::Thinking { verb }) => thinking_line(spinner.frame, verb),
        None => return,
    };
    if state.has_body_content {
        rows.push(Line::default());
    }
    rows.extend(wrap_lines_to_physical_rows(&[line], width));
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
    use super::{serialize_live_rows_with_boundaries, thinking_line};
    use crate::agent::model;
    use crate::app::{
        App, AppStatus, BlockCache, ChatMessage, MessageBlock, MessageRole, NoticeBlock,
        TerminalSnapshotMode, TextBlock, TextBlockSpacing, ToolCallInfo,
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

    fn compact_text(rows: &[Line<'_>]) -> String {
        line_texts(rows).join("").chars().filter(|ch| !ch.is_whitespace()).collect()
    }

    fn serialize_live_rows(app: &mut App, width: u16) -> Vec<Line<'static>> {
        serialize_live_rows_with_boundaries(app, width).rows().to_vec()
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
        tool_call_block_with_interaction(id, hidden, false, false)
    }

    fn tool_call_block_with_interaction(
        id: &str,
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
            status: model::ToolCallStatus::Completed,
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

        let serialized = serialize_live_rows_with_boundaries(&mut app, 120);

        assert_eq!(serialized.stable_row_count_before_message(Some(1)), 2);
        assert_eq!(serialized.stable_row_count_before_message(None), serialized.rows().len());
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
    fn live_adjacent_text_blocks_render_as_one_text_run() {
        let mut app = App::test_default();
        app.messages.push(assistant_blocks_message(vec![
            MessageBlock::Text(
                TextBlock::from_complete("line 1: ready\n\n")
                    .with_trailing_spacing(TextBlockSpacing::ParagraphBreak),
            ),
            MessageBlock::Text(TextBlock::from_complete("line 2: ready")),
        ]));

        let rows = serialize_live_rows(&mut app, 120);

        assert_eq!(line_texts(&rows), vec!["Claude", "line 1: ready", "line 2: ready"]);
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

        let serialized = serialize_live_rows_with_boundaries(&mut app, 120);

        assert!(!serialized.rows().is_empty());
        assert_eq!(serialized.stable_row_count_before_message(None), serialized.rows().len());
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

        let serialized = serialize_live_rows_with_boundaries(&mut app, 120);

        assert!(!serialized.rows().is_empty());
        assert_eq!(serialized.stable_row_count_before_message(None), 0);
    }

    #[test]
    fn loading_welcome_blocks_later_stable_rows_from_scrollback() {
        let mut app = App::test_default();
        app.status = AppStatus::Connecting;
        app.messages.push(ChatMessage::welcome("1.2.3", "-", "/workspace/demo", "-"));
        app.push_message_tracked(user_text_message("queued while connecting"));

        let serialized = serialize_live_rows_with_boundaries(&mut app, 120);

        assert!(line_texts(serialized.rows()).iter().any(|line| line == "queued while connecting"));
        assert_eq!(serialized.stable_row_count_before_message(None), 0);
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
