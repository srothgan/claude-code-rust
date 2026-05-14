// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use crate::app::{ChatMessage, MessageBlock, MessageRole, SystemSeverity, TextBlock};
use crate::ui::theme;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

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

    rows.push_lines(vec![role_label_line(&msg.role)]);

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
            MessageBlock::ToolCall(_)
            | MessageBlock::Welcome(_)
            | MessageBlock::ImageAttachment(_) => {}
        }
    }
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
        ChatMessage, ImageAttachmentBlock, MessageBlock, MessageRole, NoticeBlock, SystemSeverity,
        TextBlock, TextBlockSpacing,
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
