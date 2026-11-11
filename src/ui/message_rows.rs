// SPDX-License-Identifier: Apache-2.0

use crate::app::{
    BtwExchangeBlock, ChatMessage, MessageBlock, MessageRole, SystemSeverity, TextBlock,
    UserDialogBlock,
};
use crate::ui::theme;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::message::{MessageRenderContext, render_text_block_cached};
use super::message_layout::{self, MessageLayout};

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
    let uninterrupted = super::layout_control::LayoutControl::uninterrupted();
    let control = render_context.tool_render_context.control.unwrap_or(&uninterrupted);
    let mut rows = MessageRows::new();
    if !matches!(msg.role, MessageRole::User | MessageRole::System(_)) {
        return rows;
    }

    if matches!(msg.role, MessageRole::System(_))
        && !msg.blocks.iter().any(|block| matches!(block, MessageBlock::BtwExchange(_)))
    {
        rows.push_lines(vec![system_role_label_line(system_severity_from_role(&msg.role))]);
    }

    match msg.role {
        MessageRole::User => {
            let width = render_context.width;
            append_user_blocks(msg, MessageLayout::new(width).content_width, &mut rows, control);
            let mut physical = Vec::new();
            for segment in rows.segments {
                match segment {
                    MessageRowSegment::Blank => physical.push(Line::default()),
                    MessageRowSegment::Lines { lines } => {
                        physical.extend(super::wrap::wrap_lines_with_control(
                            &lines,
                            MessageLayout::new(width).content_width,
                            control,
                        ));
                    }
                }
            }
            return MessageRows {
                segments: vec![MessageRowSegment::Lines {
                    lines: message_layout::frame_rows(
                        physical,
                        width,
                        Some((theme::PROMPT_CHAR, theme::DIM)),
                        Some(theme::USER_MSG_BG),
                        (true, true),
                        &[],
                    ),
                }],
            };
        }
        MessageRole::System(_) => {
            append_system_blocks(msg, render_context.width, &mut rows, control);
        }
        MessageRole::Assistant | MessageRole::Welcome => {}
    }

    rows
}

fn append_user_blocks(
    msg: &mut ChatMessage,
    width: u16,
    rows: &mut MessageRows,
    control: &super::layout_control::LayoutControl<'_>,
) {
    for block in &mut msg.blocks {
        if control.cancelled() {
            return;
        }
        match block {
            MessageBlock::Text(block) => {
                let trailing_gap = block.trailing_blank_lines();
                rows.push_lines(text_block_lines(
                    block,
                    width,
                    Some(theme::USER_MSG_BG),
                    true,
                    control,
                ));
                for _ in 0..trailing_gap {
                    rows.push_blank();
                }
            }
            MessageBlock::ImageAttachment(img) => {
                let label = if img.count == 1 {
                    "[img] 1 image attached".to_owned()
                } else {
                    format!("[img] {} images attached", img.count)
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

fn append_system_blocks(
    msg: &mut ChatMessage,
    width: u16,
    rows: &mut MessageRows,
    control: &super::layout_control::LayoutControl<'_>,
) {
    let color = system_severity_color(system_severity_from_role(&msg.role));
    for block in &mut msg.blocks {
        if control.cancelled() {
            return;
        }
        match block {
            MessageBlock::Text(block) => {
                let trailing_gap = block.trailing_blank_lines();
                let mut lines = text_block_lines(block, width, None, false, control);
                tint_lines(&mut lines, color);
                rows.push_lines(lines);
                for _ in 0..trailing_gap {
                    rows.push_blank();
                }
            }
            MessageBlock::Notice(notice) => {
                let trailing_gap = notice.trailing_blank_lines();
                rows.push_lines(notice_block_lines(notice, width, notice.severity, control));
                for _ in 0..trailing_gap {
                    rows.push_blank();
                }
            }
            MessageBlock::BtwExchange(exchange) => {
                rows.push_lines(render_btw_exchange_with_control(exchange, width, control));
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

pub(super) fn render_btw_exchange_with_control(
    block: &mut BtwExchangeBlock,
    width: u16,
    control: &super::layout_control::LayoutControl<'_>,
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
    content.extend(render_btw_markdown(&block.question, inner_width, control));
    content.push(Line::default());
    content.extend(crate::ui::wrap::wrap_lines_to_physical_rows(
        &[Line::from(Span::styled("Answer:", label))],
        inner_width,
    ));
    content.extend(render_btw_markdown(&block.answer, inner_width, control));

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
        if control.cancelled() {
            return Vec::new();
        }
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
    let rows = crate::ui::wrap::wrap_lines_with_control(&rows, width, control);
    if control.cancelled() {
        return Vec::new();
    }
    block.cache.store(rows.clone());
    block.cache.set_height(rows.len(), width);
    rows
}

fn render_btw_markdown(
    text: &str,
    width: u16,
    control: &super::layout_control::LayoutControl<'_>,
) -> Vec<Line<'static>> {
    let logical = super::markdown::render_markdown_with_control(text, width, None, false, control);
    let mut physical = crate::ui::wrap::wrap_lines_with_control(&logical, width, control);
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
    control: &super::layout_control::LayoutControl<'_>,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    render_text_block_cached(block, width, bg, preserve_newlines, &mut lines, control);
    lines
}

fn notice_block_lines(
    block: &mut crate::app::NoticeBlock,
    width: u16,
    severity: SystemSeverity,
    control: &super::layout_control::LayoutControl<'_>,
) -> Vec<Line<'static>> {
    let mut lines = text_block_lines(&mut block.text, width, None, false, control);
    tint_lines(&mut lines, system_severity_color(severity));
    lines
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
        BtwExchangeBlock, ChatMessage, MessageBlock, MessageRole, NoticeBlock, SystemSeverity,
        TextBlock,
    };
    use crate::ui::message::MessageRenderContext;
    use ratatui::text::Line;

    fn render_context() -> MessageRenderContext<'static> {
        MessageRenderContext::new(None, 80)
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
    fn system_regions_associate_owner_and_notice_severity_independently() {
        let mut owner_styles = Vec::new();
        let mut notice_styles = Vec::new();
        for (severity, label, body) in [
            (SystemSeverity::Info, "Info", "STATUS_INFO"),
            (SystemSeverity::Warning, "Warning", "STATUS_WARNING"),
            (SystemSeverity::Error, "Error", "STATUS_ERROR"),
        ] {
            let mut message = system_message(
                vec![
                    MessageBlock::Text(TextBlock::from_complete(body)),
                    MessageBlock::Notice(NoticeBlock::new(
                        SystemSeverity::Warning,
                        "LOCAL_WARNING".into(),
                    )),
                    MessageBlock::Notice(NoticeBlock::new(
                        SystemSeverity::Error,
                        "LOCAL_ERROR".into(),
                    )),
                ],
                severity,
            );
            let rows = build_user_system_message_rows(&mut message, render_context());
            let texts = segment_texts(&rows);
            assert_eq!(texts[0], label);
            let at = |needle: &str| {
                texts.iter().position(|s| s == needle).expect("complete system content")
            };
            assert!(at(body) < at("LOCAL_WARNING") && at("LOCAL_WARNING") < at("LOCAL_ERROR"));
            let style = |needle: &str| {
                rows.segments
                    .iter()
                    .find_map(|segment| match segment {
                        super::MessageRowSegment::Lines { lines } => lines
                            .iter()
                            .flat_map(|l| &l.spans)
                            .find(|s| s.content == needle)
                            .map(|s| s.style),
                        super::MessageRowSegment::Blank => None,
                    })
                    .expect("own content style")
            };
            owner_styles.push(style(body));
            notice_styles.push((style("LOCAL_WARNING"), style("LOCAL_ERROR")));
        }
        assert_ne!(owner_styles[0], owner_styles[1]);
        assert_ne!(owner_styles[1], owner_styles[2], "warning and error must be distinguishable");
        for pair in &notice_styles {
            assert_ne!(pair.0, pair.1);
        }
        assert!(
            notice_styles.windows(2).all(|w| w[0] == w[1]),
            "notice severity belongs to its content, independent of the owner"
        );
    }

    #[test]
    fn btw_card_preserves_distinct_question_answer_code_and_cells_through_width_reuse() {
        let question = "QUESTION_ORCHID";
        let answer = "ANSWER_JASMINE 日本語😀\n\n```text\n    CODE  SPACES\n```\n\n| KEY | VALUE |\n| --- | --- |\n| CELL_IRIS | CELL_ROSE |";
        let mut message = system_message(
            vec![MessageBlock::BtwExchange(BtwExchangeBlock::new(question.into(), answer.into()))],
            SystemSeverity::Error,
        );
        for width in [60, 60, 30, 60, 8, 5, 4, 0, 60] {
            let rows = build_user_system_message_rows(
                &mut message,
                MessageRenderContext::new(None, width),
            );
            let texts = segment_texts(&rows);
            assert!(
                texts.iter().all(
                    |s| unicode_width::UnicodeWidthStr::width(s.as_str()) <= usize::from(width)
                ),
                "width {width}: {texts:?}"
            );
            if width >= 5 {
                assert!(!texts.is_empty(), "supported width must show content");
                assert!(!texts.iter().any(|s| s == "Error" || s == "Warning" || s == "Info"));
                // Read only the card body, independent of its decorative border. At tiny widths
                // words split across rows; their source order still survives.
                let body: Vec<_> = texts
                    .iter()
                    .skip(1)
                    .take(texts.len().saturating_sub(2))
                    .map(|s| {
                        let chars: Vec<_> = s.chars().collect();
                        chars[2..chars.len().saturating_sub(2)].iter().collect::<String>()
                    })
                    .collect();
                let compact: String =
                    body.join("").chars().filter(|c| !c.is_whitespace()).collect();
                let q = compact.find(question).expect("question content");
                let a = compact.find("ANSWER_JASMINE").expect("independent answer prose");
                let code = compact.find("CODESPACES").expect("independent code content");
                let table = compact.find('┌').expect("table region");
                assert!(q < a && a < code && code < table);
                // Locate the data region and read each column down its wrapped rows. Reading
                // whole rows interleaves neighboring cells when the card is only five cells wide.
                let data = compact
                    .split_once('┤')
                    .expect("header/data boundary")
                    .1
                    .split_once('└')
                    .expect("end of data")
                    .0;
                let mut cells = [String::new(), String::new()];
                for row in data.trim_matches('│').split("││") {
                    let (left, right) = row.split_once('│').expect("two data columns");
                    cells[0].push_str(left);
                    cells[1].push_str(right);
                }
                assert_eq!(
                    cells,
                    ["CELL_IRIS", "CELL_ROSE"],
                    "width {width}: independently associated cell contents"
                );
                if width >= 30 {
                    let question_row = body.iter().position(|s| s.contains(question)).unwrap();
                    let answer_label = body.iter().position(|s| s.contains("Answer:")).unwrap();
                    let prose = body
                        .iter()
                        .position(|s| s.contains("ANSWER_JASMINE 日本語😀"))
                        .expect("answer Unicode, not question Unicode");
                    assert!(question_row < answer_label && answer_label < prose);
                    assert!(
                        body.iter().any(|s| s.starts_with("    CODE  SPACES")),
                        "significant code spaces: {body:?}"
                    );
                    let cells = body.iter().find(|s| s.contains("CELL_IRIS")).unwrap();
                    assert!(
                        cells.find("CELL_IRIS").unwrap() < cells.find("CELL_ROSE").unwrap(),
                        "cells remain in the same associated row"
                    );
                }
                let warm = build_user_system_message_rows(
                    &mut message,
                    MessageRenderContext::new(None, width),
                );
                assert_eq!(segment_texts(&warm), texts, "secondary cache reuse");
            }
        }
        let MessageBlock::BtwExchange(exchange) = &message.blocks[0] else { panic!("BTW source") };
        assert_eq!(exchange.question, question);
        assert_eq!(exchange.answer, answer);
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
}
