// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use crate::app::{ChatMessage, MessageBlock, MessageRole, SystemSeverity, TextBlock};
use crate::ui::theme;
use crate::ui::tool_call;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};

use super::SpinnerState;
use super::message::{MessageRenderContext, render_text_block_cached, render_welcome_cached};

pub(crate) struct MessageRows {
    pub segments: Vec<MessageRowSegment>,
    pub height: usize,
    pub wrapped_lines: usize,
}

impl MessageRows {
    fn new() -> Self {
        Self { segments: Vec::new(), height: 0, wrapped_lines: 0 }
    }

    fn push_blank(&mut self) {
        self.segments.push(MessageRowSegment::Blank);
        self.height += 1;
    }

    fn push_wrapped_line(&mut self, line: Line<'static>, width: u16) {
        self.push_wrapped_lines(vec![line], width);
    }

    fn push_wrapped_lines(&mut self, lines: Vec<Line<'static>>, width: u16) {
        let height = rendered_lines_height(&lines, width);
        self.push_lines(lines, height, height);
    }

    fn push_lines(&mut self, lines: Vec<Line<'static>>, height: usize, wrapped_lines: usize) {
        if height == 0 {
            return;
        }
        self.segments.push(MessageRowSegment::Lines { lines, height });
        self.height += height;
        self.wrapped_lines += wrapped_lines;
    }
}

#[derive(Clone)]
pub(crate) enum MessageRowSegment {
    Blank,
    Lines { lines: Vec<Line<'static>>, height: usize },
}

pub(crate) struct RenderedBlockLayout {
    pub lines: Vec<Line<'static>>,
    pub height: usize,
    pub wrapped_lines: usize,
}

#[derive(Default)]
struct AssistantLayoutState {
    prev_was_tool: bool,
    has_body_content: bool,
    has_visible_content: bool,
}

pub(crate) fn build_message_rows(
    msg: &mut ChatMessage,
    spinner: &SpinnerState,
    render_context: MessageRenderContext<'_>,
) -> MessageRows {
    let mut rows = MessageRows::new();
    rows.push_wrapped_line(role_label_line(&msg.role), render_context.width);

    match msg.role {
        MessageRole::Welcome => append_welcome_blocks(msg, render_context.width, &mut rows),
        MessageRole::User => append_user_blocks(msg, render_context.width, &mut rows),
        MessageRole::Assistant => append_assistant_blocks(msg, spinner, render_context, &mut rows),
        MessageRole::System(_) => append_system_blocks(msg, render_context.width, &mut rows),
    }

    if render_context.options.include_trailing_separator {
        rows.push_blank();
    }

    rows
}

fn append_welcome_blocks(msg: &mut ChatMessage, width: u16, rows: &mut MessageRows) {
    for block in &mut msg.blocks {
        if let MessageBlock::Welcome(welcome) = block {
            let rendered = welcome_block_layout(welcome, width);
            rows.push_lines(rendered.lines, rendered.height, rendered.wrapped_lines);
        }
    }
}

fn append_user_blocks(msg: &mut ChatMessage, width: u16, rows: &mut MessageRows) {
    for block in &mut msg.blocks {
        match block {
            MessageBlock::Text(block) => {
                let trailing_gap = block.trailing_blank_lines();
                let rendered = text_block_layout(block, width, Some(theme::USER_MSG_BG), true);
                rows.push_lines(rendered.lines, rendered.height, rendered.wrapped_lines);
                for _ in 0..trailing_gap {
                    rows.push_blank();
                }
            }
            MessageBlock::ImageAttachment(img) => {
                let label = if img.count == 1 {
                    " [img] 1 image attached ".to_owned()
                } else {
                    format!(" [img] {} images attached ", img.count)
                };
                rows.push_wrapped_line(
                    Line::from(Span::styled(
                        label,
                        Style::default().fg(Color::Cyan).add_modifier(Modifier::DIM),
                    )),
                    width,
                );
            }
            _ => {}
        }
    }
}

fn append_assistant_blocks(
    msg: &mut ChatMessage,
    spinner: &SpinnerState,
    render_context: MessageRenderContext<'_>,
    rows: &mut MessageRows,
) {
    if msg.blocks.is_empty() && spinner.show_compacting {
        rows.push_wrapped_line(compacting_line(spinner.frame), render_context.width);
        return;
    }
    if msg.blocks.is_empty() && spinner.show_empty_thinking {
        rows.push_wrapped_line(thinking_line(spinner.frame), render_context.width);
        return;
    }

    let show_compacting = spinner.show_compacting;
    let deferred_interaction = deferred_hidden_interaction_render_after(&msg.blocks);
    let mut state = AssistantLayoutState::default();
    for idx in 0..msg.blocks.len() {
        if deferred_interaction.is_some_and(|(deferred_idx, _)| deferred_idx == idx) {
            continue;
        }

        append_assistant_block(&mut msg.blocks[idx], spinner, render_context, rows, &mut state);

        if let Some((deferred_idx, render_after_idx)) = deferred_interaction
            && render_after_idx == idx
        {
            append_assistant_block(
                &mut msg.blocks[deferred_idx],
                spinner,
                render_context,
                rows,
                &mut state,
            );
        }
    }

    if show_compacting {
        if state.has_body_content {
            rows.push_blank();
        }
        rows.push_wrapped_line(compacting_line(spinner.frame), render_context.width);
    }
    if spinner.show_thinking && !show_compacting {
        if state.has_body_content {
            rows.push_blank();
        }
        rows.push_wrapped_line(thinking_line(spinner.frame), render_context.width);
    }
}

fn deferred_hidden_interaction_render_after(blocks: &[MessageBlock]) -> Option<(usize, usize)> {
    let deferred_idx = blocks.iter().position(
        |block| matches!(block, MessageBlock::ToolCall(tc) if tc.is_hidden_focused_interaction()),
    )?;
    let render_after_idx = blocks
        .iter()
        .enumerate()
        .skip(deferred_idx.saturating_add(1))
        .filter_map(|(idx, block)| match block {
            MessageBlock::ToolCall(tc) if tc.is_subagent_root_tool() => Some(idx),
            _ => None,
        })
        .last()?;
    Some((deferred_idx, render_after_idx))
}

fn append_assistant_block(
    block: &mut MessageBlock,
    spinner: &SpinnerState,
    render_context: MessageRenderContext<'_>,
    rows: &mut MessageRows,
    state: &mut AssistantLayoutState,
) {
    match block {
        MessageBlock::Text(block) => {
            append_assistant_text_block(block, render_context.width, rows, state);
        }
        MessageBlock::Notice(notice) => {
            append_assistant_notice_block(notice, render_context.width, rows, state);
        }
        MessageBlock::ToolCall(tc) => {
            append_assistant_tool_block(tc.as_mut(), spinner, render_context, rows, state);
        }
        MessageBlock::Welcome(_) | MessageBlock::ImageAttachment(_) => {}
    }
}

fn append_assistant_text_block(
    block: &mut TextBlock,
    width: u16,
    rows: &mut MessageRows,
    state: &mut AssistantLayoutState,
) {
    if state.prev_was_tool {
        rows.push_blank();
    }
    let rendered = assistant_text_block_layout(block, width, !state.has_visible_content);
    let trailing_gap = trailing_gap_for_text_like_block(
        state.has_visible_content,
        rendered.height,
        block.trailing_blank_lines(),
    );
    rows.push_lines(rendered.lines, rendered.height, rendered.wrapped_lines);
    for _ in 0..trailing_gap {
        rows.push_blank();
    }
    if rendered.height > 0 {
        state.has_body_content = true;
        state.has_visible_content = true;
    }
    state.prev_was_tool = false;
}

fn append_assistant_notice_block(
    notice: &mut crate::app::NoticeBlock,
    width: u16,
    rows: &mut MessageRows,
    state: &mut AssistantLayoutState,
) {
    if state.prev_was_tool {
        rows.push_blank();
    }
    let rendered = notice_block_layout(notice, width, !state.has_visible_content, notice.severity);
    let trailing_gap = trailing_gap_for_text_like_block(
        state.has_visible_content,
        rendered.height,
        notice.trailing_blank_lines(),
    );
    rows.push_lines(rendered.lines, rendered.height, rendered.wrapped_lines);
    for _ in 0..trailing_gap {
        rows.push_blank();
    }
    if rendered.height > 0 {
        state.has_body_content = true;
        state.has_visible_content = true;
    }
    state.prev_was_tool = false;
}

fn append_assistant_tool_block(
    tc: &mut crate::app::ToolCallInfo,
    spinner: &SpinnerState,
    render_context: MessageRenderContext<'_>,
    rows: &mut MessageRows,
    state: &mut AssistantLayoutState,
) {
    if tc.hidden_unless_focused_interaction() {
        return;
    }
    if !state.prev_was_tool && state.has_body_content {
        rows.push_blank();
    }

    let mut lines = Vec::new();
    tool_call::render_tool_call_cached(
        tc,
        render_context.tool_render_context,
        render_context.width,
        spinner.frame,
        &mut lines,
    );
    let (height, wrapped_lines) = tool_call::measure_tool_call_height_cached(
        tc,
        render_context.tool_render_context,
        render_context.width,
        spinner.frame,
        render_context.layout_generation,
    );
    rows.push_lines(lines, height, wrapped_lines);
    if height > 0 {
        state.has_body_content = true;
    }
    state.has_visible_content = true;
    state.prev_was_tool = true;
}

fn trailing_gap_for_text_like_block(
    has_visible_content: bool,
    rendered_height: usize,
    trailing_blank_lines: usize,
) -> usize {
    if !has_visible_content && rendered_height == 0 { 0 } else { trailing_blank_lines }
}

fn append_system_blocks(msg: &mut ChatMessage, width: u16, rows: &mut MessageRows) {
    let color = system_severity_color(system_severity_from_role(&msg.role));
    for block in &mut msg.blocks {
        match block {
            MessageBlock::Text(block) => {
                let trailing_gap = block.trailing_blank_lines();
                let mut rendered = text_block_layout(block, width, None, false);
                tint_lines(&mut rendered.lines, color);
                rows.push_lines(rendered.lines, rendered.height, rendered.wrapped_lines);
                for _ in 0..trailing_gap {
                    rows.push_blank();
                }
            }
            MessageBlock::Notice(notice) => {
                let trailing_gap = notice.trailing_blank_lines();
                let rendered = notice_block_layout(notice, width, false, notice.severity);
                rows.push_lines(rendered.lines, rendered.height, rendered.wrapped_lines);
                for _ in 0..trailing_gap {
                    rows.push_blank();
                }
            }
            MessageBlock::ToolCall(_)
            | MessageBlock::Welcome(_)
            | MessageBlock::ImageAttachment(_) => {}
        }
    }
}

fn rendered_lines_height(lines: &[Line<'static>], width: u16) -> usize {
    if lines.is_empty() {
        return 0;
    }
    Paragraph::new(Text::from(lines.to_vec())).wrap(Wrap { trim: false }).line_count(width)
}

#[cfg(test)]
pub(crate) fn rendered_segment_height(segments: &[MessageRowSegment]) -> usize {
    segments
        .iter()
        .map(|segment| match segment {
            MessageRowSegment::Blank => 1,
            MessageRowSegment::Lines { height, .. } => *height,
        })
        .sum()
}

fn welcome_block_layout(block: &mut crate::app::WelcomeBlock, width: u16) -> RenderedBlockLayout {
    let had_height = block.cache.height_at(width).is_some();
    let mut lines = Vec::new();
    render_welcome_cached(block, width, &mut lines);
    let height = block.cache.height_at(width).unwrap_or_else(|| {
        let height = rendered_lines_height(&lines, width);
        block.cache.set_height(height, width);
        height
    });
    let wrapped_lines = if had_height { 0 } else { lines.len() };
    RenderedBlockLayout { lines, height, wrapped_lines }
}

fn text_block_layout(
    block: &mut TextBlock,
    width: u16,
    bg: Option<Color>,
    preserve_newlines: bool,
) -> RenderedBlockLayout {
    let had_height = block.cache.height_at(width).is_some();
    let mut lines = Vec::new();
    render_text_block_cached(block, width, bg, preserve_newlines, &mut lines);
    let height = block.cache.height_at(width).unwrap_or_else(|| {
        let height = rendered_lines_height(&lines, width);
        block.cache.set_height(height, width);
        height
    });
    let wrapped_lines = if had_height { 0 } else { lines.len() };
    RenderedBlockLayout { lines, height, wrapped_lines }
}

fn assistant_text_block_layout(
    block: &mut TextBlock,
    width: u16,
    trim_leading_blank_lines: bool,
) -> RenderedBlockLayout {
    let mut rendered = text_block_layout(block, width, None, false);

    if trim_leading_blank_lines {
        let leading_blank_lines = count_leading_blank_lines(&rendered.lines);
        if leading_blank_lines > 0 {
            rendered.lines.drain(..leading_blank_lines);
            rendered.height = rendered.height.saturating_sub(leading_blank_lines);
            rendered.wrapped_lines = rendered.wrapped_lines.saturating_sub(leading_blank_lines);
        }
    }

    rendered
}

fn notice_block_layout(
    block: &mut crate::app::NoticeBlock,
    width: u16,
    trim_leading_blank_lines: bool,
    severity: SystemSeverity,
) -> RenderedBlockLayout {
    let mut rendered =
        assistant_text_block_layout(&mut block.text, width, trim_leading_blank_lines);
    tint_lines(&mut rendered.lines, system_severity_color(severity));
    rendered
}

fn count_leading_blank_lines(lines: &[Line<'static>]) -> usize {
    lines.iter().take_while(|line| line_is_blank(line)).count()
}

fn line_is_blank(line: &Line<'_>) -> bool {
    line.spans.iter().all(|span| span.content.as_ref().chars().all(char::is_whitespace))
}

fn role_label_line(role: &MessageRole) -> Line<'static> {
    match role {
        MessageRole::Welcome => Line::from(Span::styled(
            "Overview",
            Style::default().fg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD),
        )),
        MessageRole::User => Line::from(Span::styled(
            "User",
            Style::default().fg(theme::DIM).add_modifier(Modifier::BOLD),
        )),
        MessageRole::Assistant => assistant_role_label_line(),
        MessageRole::System(_) => system_role_label_line(system_severity_from_role(role)),
    }
}

fn assistant_role_label_line() -> Line<'static> {
    Line::from(vec![Span::styled(
        "Claude",
        Style::default().fg(theme::ROLE_ASSISTANT).add_modifier(Modifier::BOLD),
    )])
}

fn system_role_label_line(severity: SystemSeverity) -> Line<'static> {
    let (label, color) = match severity {
        SystemSeverity::Info => ("Info", theme::DIM),
        SystemSeverity::Warning => ("Warning", theme::STATUS_WARNING),
        SystemSeverity::Error => ("Error", theme::STATUS_ERROR),
    };
    Line::from(Span::styled(label, Style::default().fg(color).add_modifier(Modifier::BOLD)))
}

fn system_severity_color(severity: SystemSeverity) -> Color {
    match severity {
        SystemSeverity::Info => theme::DIM,
        SystemSeverity::Warning => theme::STATUS_WARNING,
        SystemSeverity::Error => theme::STATUS_ERROR,
    }
}

fn system_severity_from_role(role: &MessageRole) -> SystemSeverity {
    match role {
        MessageRole::System(level) => level.unwrap_or(SystemSeverity::Error),
        _ => SystemSeverity::Error,
    }
}

fn thinking_line(frame: usize) -> Line<'static> {
    const SPINNER_FRAMES: &[char] = &[
        '\u{280B}', '\u{2819}', '\u{2839}', '\u{2838}', '\u{283C}', '\u{2834}', '\u{2826}',
        '\u{2827}', '\u{2807}', '\u{280F}',
    ];
    let ch = SPINNER_FRAMES[frame % SPINNER_FRAMES.len()];
    Line::from(Span::styled(format!("{ch} Thinking..."), Style::default().fg(theme::DIM)))
}

fn compacting_line(frame: usize) -> Line<'static> {
    const SPINNER_FRAMES: &[char] = &[
        '\u{280B}', '\u{2819}', '\u{2839}', '\u{2838}', '\u{283C}', '\u{2834}', '\u{2826}',
        '\u{2827}', '\u{2807}', '\u{280F}',
    ];
    let ch = SPINNER_FRAMES[frame % SPINNER_FRAMES.len()];
    Line::from(Span::styled(
        format!("{ch} Compacting context..."),
        Style::default().fg(theme::RUST_ORANGE),
    ))
}

fn tint_lines(lines: &mut [Line<'static>], color: Color) {
    for line in lines {
        for span in &mut line.spans {
            span.style = span.style.fg(color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{build_message_rows, rendered_segment_height};
    use crate::agent::model;
    use crate::app::{
        BlockCache, ChatMessage, InlinePermission, MessageBlock, MessageRole, NoticeBlock,
        SystemSeverity, TerminalSnapshotMode, TextBlock, TextBlockSpacing, ToolCallInfo,
    };
    use crate::ui::SpinnerState;
    use crate::ui::message::{MessageRenderContext, MessageRenderOptions};
    use ratatui::text::Line;
    use tokio::sync::oneshot;

    fn idle_spinner() -> SpinnerState {
        SpinnerState {
            frame: 0,
            is_active_turn_assistant: false,
            show_empty_thinking: false,
            show_thinking: false,
            show_compacting: false,
        }
    }

    fn render_context(include_trailing_separator: bool) -> MessageRenderContext<'static> {
        MessageRenderContext::new(None, 80, 1, MessageRenderOptions { include_trailing_separator })
    }

    fn assistant_message(blocks: Vec<MessageBlock>) -> ChatMessage {
        ChatMessage::new(MessageRole::Assistant, blocks, None)
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|span| span.content.as_ref()).collect()
    }

    fn segment_texts(rows: &crate::ui::message_rows::MessageRows) -> Vec<String> {
        let mut out = Vec::new();
        for segment in &rows.segments {
            match segment {
                super::MessageRowSegment::Blank => out.push(String::new()),
                super::MessageRowSegment::Lines { lines, .. } => {
                    out.extend(lines.iter().map(line_text));
                }
            }
        }
        out
    }

    fn make_tool(
        id: &str,
        sdk_tool_name: &str,
        hidden: bool,
        pending_permission: Option<InlinePermission>,
    ) -> MessageBlock {
        MessageBlock::ToolCall(Box::new(ToolCallInfo {
            id: id.to_owned(),
            title: "Tool".to_owned(),
            sdk_tool_name: sdk_tool_name.to_owned(),
            raw_input: None,
            raw_input_bytes: 0,
            output_metadata: None,
            task_metadata: None,
            status: model::ToolCallStatus::Completed,
            content: vec![],
            hidden,
            terminal_id: None,
            terminal_command: None,
            terminal_output: None,
            terminal_output_len: 0,
            terminal_bytes_seen: 0,
            terminal_snapshot_mode: TerminalSnapshotMode::AppendOnly,
            render_epoch: 0,
            layout_epoch: 0,
            last_measured_width: 0,
            last_measured_height: 0,
            last_measured_layout_epoch: 0,
            last_measured_layout_generation: 0,
            cache: BlockCache::default(),
            pending_permission,
            pending_question: None,
        }))
    }

    #[test]
    fn assistant_text_blocks_preserve_header_and_spacing_behavior() {
        let mut msg = assistant_message(vec![
            MessageBlock::Text(TextBlock::from_complete("First paragraph")),
            MessageBlock::Text(
                TextBlock::from_complete("Second paragraph")
                    .with_trailing_spacing(TextBlockSpacing::ParagraphBreak),
            ),
        ]);

        let rows = build_message_rows(&mut msg, &idle_spinner(), render_context(true));
        let texts = segment_texts(&rows);

        assert_eq!(texts.first().expect("header"), "Claude");
        assert!(texts.iter().any(|line| line.contains("First paragraph")));
        assert!(texts.iter().any(|line| line.contains("Second paragraph")));
        assert!(texts.iter().any(String::is_empty));
    }

    #[test]
    fn assistant_notice_blocks_serialize_as_text_like_with_tint() {
        let mut msg = assistant_message(vec![MessageBlock::Notice(NoticeBlock::new(
            SystemSeverity::Warning,
            "Warning inline".to_owned(),
        ))]);

        let rows = build_message_rows(&mut msg, &idle_spinner(), render_context(true));
        let warning_line = rows
            .segments
            .iter()
            .find_map(|segment| match segment {
                super::MessageRowSegment::Lines { lines, .. } => lines.iter().find(|line| {
                    line.spans.iter().any(|span| span.content.as_ref().contains("Warning inline"))
                }),
                super::MessageRowSegment::Blank => None,
            })
            .expect("warning line");

        assert!(
            warning_line
                .spans
                .iter()
                .filter(|span| !span.content.is_empty())
                .all(|span| span.style.fg == Some(crate::ui::theme::STATUS_WARNING))
        );
    }

    #[test]
    fn tool_after_text_inserts_exactly_one_structural_blank_row() {
        let mut msg = assistant_message(vec![
            MessageBlock::Text(TextBlock::from_complete("alpha")),
            make_tool("tool-1", "Read", false, None),
        ]);

        let rows = build_message_rows(&mut msg, &idle_spinner(), render_context(true));
        let texts = segment_texts(&rows);
        let text_idx = texts.iter().position(|line| line.contains("alpha")).expect("text");
        let tool_idx = texts.iter().position(|line| line.contains("Tool")).expect("tool");

        assert_eq!(tool_idx, text_idx + 2);
        assert!(texts[text_idx + 1].is_empty());
    }

    #[test]
    fn text_after_tool_inserts_exactly_one_structural_blank_row() {
        let mut msg = assistant_message(vec![
            make_tool("tool-1", "Read", false, None),
            MessageBlock::Text(TextBlock::from_complete("omega")),
        ]);

        let rows = build_message_rows(&mut msg, &idle_spinner(), render_context(true));
        let texts = segment_texts(&rows);
        let tool_idx = texts.iter().position(|line| line.contains("Tool")).expect("tool");
        let text_idx = texts.iter().position(|line| line.contains("omega")).expect("text");

        assert_eq!(text_idx, tool_idx + 2);
        assert!(texts[tool_idx + 1].is_empty());
    }

    #[test]
    fn tool_to_tool_stays_compact() {
        let mut msg = assistant_message(vec![
            make_tool("tool-1", "Read", false, None),
            make_tool("tool-2", "Write", false, None),
        ]);

        let rows = build_message_rows(&mut msg, &idle_spinner(), render_context(true));
        let texts = segment_texts(&rows);
        let first_tool_idx = texts.iter().position(|line| line.contains("Tool")).expect("tool");
        let second_tool_idx = texts
            .iter()
            .enumerate()
            .skip(first_tool_idx + 1)
            .find(|(_, line)| line.contains("Tool"))
            .map(|(idx, _)| idx)
            .expect("second tool");

        assert_eq!(second_tool_idx, first_tool_idx + 1);
    }

    #[test]
    fn empty_assistant_thinking_row_appears_when_requested() {
        let mut msg = assistant_message(Vec::new());
        let spinner = SpinnerState { show_empty_thinking: true, frame: 1, ..idle_spinner() };

        let rows = build_message_rows(&mut msg, &spinner, render_context(true));
        let texts = segment_texts(&rows);
        assert!(texts.iter().any(|line| line.contains("Thinking...")));
    }

    #[test]
    fn compacting_suppresses_trailing_thinking_row() {
        let mut msg = assistant_message(vec![MessageBlock::Text(TextBlock::from_complete("body"))]);
        let spinner =
            SpinnerState { show_thinking: true, show_compacting: true, frame: 1, ..idle_spinner() };

        let rows = build_message_rows(&mut msg, &spinner, render_context(true));
        let texts = segment_texts(&rows);
        assert!(texts.iter().any(|line| line.contains("Compacting context")));
        assert!(!texts.iter().any(|line| line.contains("Thinking...")));
    }

    #[test]
    fn trailing_separator_respects_render_options() {
        let mut with_separator =
            assistant_message(vec![MessageBlock::Text(TextBlock::from_complete("body"))]);
        let rows_with =
            build_message_rows(&mut with_separator, &idle_spinner(), render_context(true));
        assert!(matches!(rows_with.segments.last(), Some(super::MessageRowSegment::Blank)));

        let mut without_separator =
            assistant_message(vec![MessageBlock::Text(TextBlock::from_complete("body"))]);
        let rows_without =
            build_message_rows(&mut without_separator, &idle_spinner(), render_context(false));
        assert!(!matches!(rows_without.segments.last(), Some(super::MessageRowSegment::Blank)));
    }

    #[test]
    fn hidden_focused_child_interaction_is_rendered_after_later_subagent_root() {
        let (response_tx, _response_rx) = oneshot::channel();
        let mut msg = assistant_message(vec![
            make_tool(
                "hidden-child",
                "Read",
                true,
                Some(InlinePermission {
                    options: vec![],
                    display: None,
                    response_tx,
                    selected_index: 0,
                    focused: true,
                }),
            ),
            make_tool("root", "Task", false, None),
        ]);

        let rows = build_message_rows(&mut msg, &idle_spinner(), render_context(true));
        let texts = segment_texts(&rows);
        let task_idx = texts.iter().position(|line| line.contains("Tool")).expect("task");
        let child_idx = texts
            .iter()
            .enumerate()
            .skip(task_idx + 1)
            .find(|(_, line)| line.contains("Tool"))
            .map(|(idx, _)| idx)
            .expect("child");

        assert!(child_idx > task_idx);
    }

    #[test]
    fn message_rows_height_matches_segment_sum() {
        let mut msg = assistant_message(vec![
            MessageBlock::Text(TextBlock::from_complete("alpha")),
            make_tool("tool-1", "Read", false, None),
        ]);

        let rows = build_message_rows(&mut msg, &idle_spinner(), render_context(true));
        assert_eq!(rows.height, rendered_segment_height(&rows.segments));
    }
}
