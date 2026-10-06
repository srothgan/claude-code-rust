// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use crate::app::{
    BtwExchangeBlock, ChatMessage, MessageBlock, MessageRole, SystemSeverity, TextBlock,
    UserDialogBlock,
};
use crate::ui::theme;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::message::{MessageRenderContext, render_text_block_cached};

pub(crate) struct MessageRows {
    pub segments: Vec<MessageRowSegment>,
}

impl MessageRows {
    fn new() -> Self {
        Self { segments: Vec::new() }
    }

    fn push_blank(&mut self) {
        self.segments.push(MessageRowSegment::Blank);
    }

    fn push_lines(&mut self, lines: Vec<Line<'static>>) {
        if lines.is_empty() {
            return;
        }
        self.segments.push(MessageRowSegment::Lines { lines });
    }
}

#[derive(Clone)]
pub(crate) enum MessageRowSegment {
    Blank,
    Lines { lines: Vec<Line<'static>> },
}

pub(crate) fn build_user_system_message_rows(
    msg: &mut ChatMessage,
    render_context: MessageRenderContext<'_>,
) -> MessageRows {
    let mut rows = MessageRows::new();
    if !matches!(msg.role, MessageRole::User | MessageRole::System(_)) {
        return rows;
    }

    if !msg.blocks.iter().any(|block| matches!(block, MessageBlock::BtwExchange(_))) {
        rows.push_lines(vec![role_label_line(&msg.role)]);
    }

    match msg.role {
        MessageRole::User => append_user_blocks(msg, render_context.width, &mut rows),
        MessageRole::System(_) => append_system_blocks(msg, render_context.width, &mut rows),
        MessageRole::Assistant | MessageRole::Welcome => {}
    }

    rows
}

fn append_user_blocks(msg: &mut ChatMessage, width: u16, rows: &mut MessageRows) {
    for block in &mut msg.blocks {
        match block {
            MessageBlock::Text(block) => {
                let trailing_gap = block.trailing_blank_lines();
                rows.push_lines(text_block_lines(block, width, Some(theme::USER_MSG_BG), true));
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
                rows.push_lines(vec![Line::from(Span::styled(
                    label,
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::DIM),
                ))]);
            }
            _ => {}
        }
    }
}

fn append_system_blocks(msg: &mut ChatMessage, width: u16, rows: &mut MessageRows) {
    let color = system_severity_color(system_severity_from_role(&msg.role));
    for block in &mut msg.blocks {
        match block {
            MessageBlock::Text(block) => {
                let trailing_gap = block.trailing_blank_lines();
                let mut lines = text_block_lines(block, width, None, false);
                tint_lines(&mut lines, color);
                rows.push_lines(lines);
                for _ in 0..trailing_gap {
                    rows.push_blank();
                }
            }
            MessageBlock::Notice(notice) => {
                let trailing_gap = notice.trailing_blank_lines();
                rows.push_lines(notice_block_lines(notice, width, notice.severity));
                for _ in 0..trailing_gap {
                    rows.push_blank();
                }
            }
            MessageBlock::BtwExchange(exchange) => {
                rows.push_lines(render_btw_exchange_lines(exchange, width));
            }
            MessageBlock::UserDialog(dialog) => {
                rows.push_lines(render_user_dialog_lines(dialog));
                rows.push_blank();
            }
            MessageBlock::ToolCall(_)
            | MessageBlock::ToolResult { .. }
            | MessageBlock::Welcome(_)
            | MessageBlock::ImageAttachment(_) => {}
        }
    }
}

pub(super) fn render_btw_exchange_lines(
    block: &mut BtwExchangeBlock,
    width: u16,
) -> Vec<Line<'static>> {
    if block.cache.height_at(width).is_some()
        && let Some(lines) = block.cache.get()
    {
        return lines.clone();
    }

    if width < 5 {
        return Vec::new();
    }
    let card_width = width;
    let inner_width = card_width.saturating_sub(4).max(1);
    let accent = Style::default().fg(theme::BTW_ACCENT);
    let label =
        Style::default().fg(theme::BTW_ACCENT).add_modifier(Modifier::BOLD | Modifier::ITALIC);
    let mut content = crate::ui::wrap::wrap_lines_to_physical_rows(
        &[Line::from(Span::styled("Question:", label))],
        inner_width,
    );
    content.extend(render_btw_markdown(&block.question, inner_width));
    content.push(Line::default());
    content.extend(crate::ui::wrap::wrap_lines_to_physical_rows(
        &[Line::from(Span::styled("Answer:", label))],
        inner_width,
    ));
    content.extend(render_btw_markdown(&block.answer, inner_width));

    let title = " Claude · BTW ";
    let top_inner_width = usize::from(card_width.saturating_sub(2));
    let available_title = top_inner_width.saturating_sub(1);
    let shown_title = if UnicodeWidthStr::width(title) <= available_title {
        title.to_owned()
    } else {
        title.chars().take(available_title).collect()
    };
    let title_width = UnicodeWidthStr::width(shown_title.as_str());
    let mut rows = vec![Line::from(Span::styled(
        format!("╭─{shown_title}{}╮", "─".repeat(available_title.saturating_sub(title_width))),
        accent,
    ))];
    for line in content {
        let used = line.width().min(usize::from(inner_width));
        let mut spans = Vec::with_capacity(line.spans.len() + 3);
        spans.push(Span::styled("│ ", accent));
        spans.extend(line.spans);
        spans.push(Span::raw(" ".repeat(usize::from(inner_width).saturating_sub(used))));
        spans.push(Span::styled(" │", accent));
        rows.push(Line::from(spans));
    }
    rows.push(Line::from(Span::styled(
        format!("╰{}╯", "─".repeat(usize::from(card_width.saturating_sub(2)))),
        accent,
    )));

    // Normalize the completed physical rows here so inline and standalone cards
    // share identical spans, styles, and terminal-cell layout.
    let rows = crate::ui::wrap::wrap_lines_to_physical_rows(&rows, width);
    block.cache.store(rows.clone());
    block.cache.set_height(rows.len(), width);
    rows
}

fn render_btw_markdown(text: &str, width: u16) -> Vec<Line<'static>> {
    let logical = super::document_table::render_markdown_with_tables(text, width, None);
    let mut physical = crate::ui::wrap::wrap_markdown_lines_to_physical_rows(&logical, width);
    for line in &mut physical {
        for span in &mut line.spans {
            if span.style.bg.is_none_or(|color| color == Color::Reset) {
                span.style = span.style.add_modifier(Modifier::ITALIC);
            }
        }
    }
    physical
}

/// Render a `refusal_fallback_prompt` dialog as an inline selectable chooser.
/// While pending, the focused option shows a `▸` arrow and a help line; once
/// answered, the resolved choice is shown without controls.
fn render_user_dialog_lines(dialog: &UserDialogBlock) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let title = if let Some(guidance) =
        dialog.payload.guidance_text.as_deref().map(str::trim).filter(|text| !text.is_empty())
    {
        guidance.to_owned()
    } else {
        format!("{} declined this request.", dialog.payload.original_model)
    };
    lines.push(Line::from(Span::styled(
        title,
        Style::default().fg(theme::STATUS_WARNING).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::default());

    if let Some(outcome) = &dialog.outcome {
        let resolved = match outcome {
            crate::agent::model::RequestUserDialogOutcome::Cancelled => "Cancelled".to_owned(),
            crate::agent::model::RequestUserDialogOutcome::Selected(selected) => dialog
                .options
                .iter()
                .find(|option| option.option_id == selected.option_id)
                .map_or_else(|| "Selected".to_owned(), |option| option.label.clone()),
        };
        lines.push(Line::from(Span::styled(
            format!("  Resolved: {resolved}"),
            Style::default().fg(theme::DIM),
        )));
        return lines;
    }

    for (index, option) in dialog.options.iter().enumerate() {
        let is_selected = dialog.focused && index == dialog.selected_index;
        let mut spans = Vec::new();
        if is_selected {
            spans.push(Span::styled(
                "\u{25b8} ",
                Style::default().fg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::raw("  "));
        }
        let label_style = if is_selected {
            Style::default().fg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        spans.push(Span::styled(option.label.clone(), label_style));
        lines.push(Line::from(spans));
    }

    lines.push(Line::default());
    let help = if dialog.focused {
        "\u{2191}\u{2193} select  enter confirm  esc decline"
    } else {
        "\u{25cb} Waiting for input... (Tab to focus)"
    };
    lines.push(Line::from(Span::styled(help, Style::default().fg(theme::DIM))));
    lines
}

fn text_block_lines(
    block: &mut TextBlock,
    width: u16,
    bg: Option<Color>,
    preserve_newlines: bool,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    render_text_block_cached(block, width, bg, preserve_newlines, &mut lines);
    lines
}

fn notice_block_lines(
    block: &mut crate::app::NoticeBlock,
    width: u16,
    severity: SystemSeverity,
) -> Vec<Line<'static>> {
    let mut lines = text_block_lines(&mut block.text, width, None, false);
    tint_lines(&mut lines, system_severity_color(severity));
    lines
}

fn role_label_line(role: &MessageRole) -> Line<'static> {
    match role {
        MessageRole::User => Line::from(Span::styled(
            "User",
            Style::default().fg(theme::DIM).add_modifier(Modifier::BOLD),
        )),
        MessageRole::System(_) => system_role_label_line(system_severity_from_role(role)),
        MessageRole::Assistant | MessageRole::Welcome => Line::default(),
    }
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

fn tint_lines(lines: &mut [Line<'static>], color: Color) {
    for line in lines {
        for span in &mut line.spans {
            span.style = span.style.fg(color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::build_user_system_message_rows;
    use crate::app::{
        BtwExchangeBlock, ChatMessage, ImageAttachmentBlock, MessageBlock, MessageRole,
        NoticeBlock, SystemSeverity, TextBlock, TextBlockSpacing,
    };
    use crate::ui::message::MessageRenderContext;
    use ratatui::text::Line;

    fn render_context() -> MessageRenderContext<'static> {
        MessageRenderContext::new(None, 80)
    }

    fn user_message(blocks: Vec<MessageBlock>) -> ChatMessage {
        ChatMessage::new(MessageRole::User, blocks, None)
    }

    fn system_message(blocks: Vec<MessageBlock>, severity: SystemSeverity) -> ChatMessage {
        ChatMessage::new(MessageRole::System(Some(severity)), blocks, None)
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|span| span.content.as_ref()).collect()
    }

    fn segment_texts(rows: &crate::ui::message_rows::MessageRows) -> Vec<String> {
        let mut out = Vec::new();
        for segment in &rows.segments {
            match segment {
                super::MessageRowSegment::Blank => out.push(String::new()),
                super::MessageRowSegment::Lines { lines } => {
                    out.extend(lines.iter().map(line_text));
                }
            }
        }
        out
    }

    #[test]
    fn user_text_blocks_preserve_header_and_spacing_behavior() {
        let mut msg = user_message(vec![
            MessageBlock::Text(TextBlock::from_complete("First paragraph")),
            MessageBlock::Text(
                TextBlock::from_complete("Second paragraph")
                    .with_trailing_spacing(TextBlockSpacing::ParagraphBreak),
            ),
        ]);

        let rows = build_user_system_message_rows(&mut msg, render_context());
        let texts = segment_texts(&rows);

        assert_eq!(texts.first().expect("header"), "User");
        assert!(texts.iter().any(|line| line.contains("First paragraph")));
        assert!(texts.iter().any(|line| line.contains("Second paragraph")));
        assert!(texts.iter().any(String::is_empty));
    }

    #[test]
    fn user_image_attachment_renders_attachment_row() {
        let mut msg =
            user_message(vec![MessageBlock::ImageAttachment(ImageAttachmentBlock::new(2))]);

        let rows = build_user_system_message_rows(&mut msg, render_context());
        let texts = segment_texts(&rows);

        assert_eq!(texts.first().expect("header"), "User");
        assert!(texts.iter().any(|line| line.contains("2 images attached")));
    }

    #[test]
    fn system_notice_blocks_serialize_as_text_like_with_tint() {
        let mut msg = system_message(
            vec![MessageBlock::Notice(NoticeBlock::new(
                SystemSeverity::Warning,
                "Warning inline".to_owned(),
            ))],
            SystemSeverity::Warning,
        );

        let rows = build_user_system_message_rows(&mut msg, render_context());
        let texts = segment_texts(&rows);
        assert_eq!(texts.first().expect("header"), "Warning");

        let warning_line = rows
            .segments
            .iter()
            .find_map(|segment| match segment {
                super::MessageRowSegment::Lines { lines } => lines.iter().find(|line| {
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
    fn completed_btw_exchange_renders_as_a_bounded_card_without_a_system_heading() {
        let mut msg = ChatMessage::new(
            MessageRole::System(None),
            vec![MessageBlock::BtwExchange(BtwExchangeBlock::new(
                "Which test covers **this**?".to_owned(),
                "The `workflow` test covers it.\n\n- including Markdown".to_owned(),
            ))],
            None,
        );

        let rows = build_user_system_message_rows(&mut msg, MessageRenderContext::new(None, 42));
        let texts = segment_texts(&rows);

        assert!(texts.first().is_some_and(|line| line.starts_with('╭')));
        assert!(texts.first().is_some_and(|line| line.contains("Claude · BTW")));
        assert!(texts.iter().any(|line| line.contains("Question:")));
        assert!(texts.iter().any(|line| line.contains("Answer:")));
        assert!(texts.iter().any(|line| line.contains("workflow")));
        assert!(!texts.iter().any(|line| line == "Info"));
        assert!(
            texts.iter().all(|line| unicode_width::UnicodeWidthStr::width(line.as_str()) == 42)
        );
        assert!(texts.last().is_some_and(|line| line.starts_with('╰')));
        assert!(
            rows.segments.iter().any(|segment| {
                let super::MessageRowSegment::Lines { lines } = segment else { return false };
                lines.iter().flat_map(|line| &line.spans).any(|span| {
                    span.content.contains("Which test covers")
                        && span.style.add_modifier.contains(ratatui::style::Modifier::ITALIC)
                })
            }),
            "question prose must retain italics through physical-row normalization"
        );
    }

    #[test]
    fn completed_btw_exchange_does_not_overflow_a_narrow_supported_width() {
        let mut msg = ChatMessage::new(
            MessageRole::System(None),
            vec![MessageBlock::BtwExchange(BtwExchangeBlock::new(
                "question".to_owned(),
                "answer".to_owned(),
            ))],
            None,
        );

        let rows = build_user_system_message_rows(&mut msg, MessageRenderContext::new(None, 8));

        assert!(
            segment_texts(&rows)
                .iter()
                .all(|line| unicode_width::UnicodeWidthStr::width(line.as_str()) == 8)
        );
    }

    #[test]
    fn completed_btw_exchange_wraps_unicode_code_and_tables_across_width_changes() {
        let mut msg = ChatMessage::new(
            MessageRole::System(None),
            vec![MessageBlock::BtwExchange(BtwExchangeBlock::new(
                "Why 日本語 and emoji 😀?".to_owned(),
                "Prose with **emphasis** and `inline code`.\n\n```rust\nlet value = \"日本語\";\n```\n\n| Name | Value |\n| --- | --- |\n| 日本語 | long table value |".to_owned(),
            ))], None,
        );
        for width in [60, 20, 42, 8, 5, 4, 0, 60] {
            let rows =
                build_user_system_message_rows(&mut msg, MessageRenderContext::new(None, width));
            let texts = segment_texts(&rows);
            assert!(
                texts.iter().all(|line| unicode_width::UnicodeWidthStr::width(line.as_str())
                    <= usize::from(width)),
                "width {width}: {texts:?}"
            );
            if width >= 20 {
                assert!(texts.iter().any(|line| line.contains("日本語")));
                assert!(texts.iter().any(|line| line.contains("Question:")));
                assert!(texts.iter().any(|line| line.contains("Answer:")));
            }
        }
    }

    #[test]
    fn resolved_dialog_renders_its_actual_outcome_instead_of_the_highlighted_option() {
        use crate::agent::model;
        use crate::app::UserDialogBlock;
        for outcome in [
            model::RequestUserDialogOutcome::Cancelled,
            model::RequestUserDialogOutcome::Selected(model::SelectedUserDialogOutcome::new(
                "edit_prompt",
            )),
        ] {
            let (sender, _receiver) = tokio::sync::oneshot::channel();
            let request = model::RequestUserDialogRequest::new(
                model::SessionId::new("session-1"),
                "dialog-1",
                "refusal_fallback_prompt",
                model::RefusalFallbackPayload {
                    original_model: "Original".to_owned(),
                    fallback_model: "Fallback".to_owned(),
                    ..Default::default()
                },
                vec![
                    model::UserDialogOption::new("retry_fallback", "Switch model"),
                    model::UserDialogOption::new("edit_prompt", "Edit prompt"),
                ],
            );
            let mut dialog = UserDialogBlock::new(request, sender);
            dialog.resolve(outcome.clone());
            dialog.selected_index = 0;
            let mut message =
                system_message(vec![MessageBlock::UserDialog(dialog)], SystemSeverity::Warning);
            let texts =
                segment_texts(&build_user_system_message_rows(&mut message, render_context()));
            let expected = if outcome == model::RequestUserDialogOutcome::Cancelled {
                "Resolved: Cancelled"
            } else {
                "Resolved: Edit prompt"
            };
            assert!(texts.iter().any(|line| line.contains(expected)), "{texts:?}");
        }
    }

    #[test]
    fn assistant_and_welcome_messages_render_no_rows_here() {
        let mut assistant = ChatMessage::new(
            MessageRole::Assistant,
            vec![MessageBlock::Text(TextBlock::from_complete("body"))],
            None,
        );
        let mut welcome = ChatMessage::welcome("1.2.3", "Pro", "/cwd", "session");

        let assistant_rows = build_user_system_message_rows(&mut assistant, render_context());
        let welcome_rows = build_user_system_message_rows(&mut welcome, render_context());

        assert!(assistant_rows.segments.is_empty());
        assert!(welcome_rows.segments.is_empty());
    }
}
