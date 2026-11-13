// SPDX-License-Identifier: Apache-2.0

use crate::app::input_atoms::{self, InputAtomKind};
use crate::app::mention;
use crate::app::subagent;
use crate::app::{App, FocusOwner};
use crate::ui::theme;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use tui_textarea::{CursorRenderMode, TextArea};

use super::message_layout::{MessageLayout, VERTICAL_PADDING as INPUT_VERTICAL_PAD};

/// Maximum input content height (lines) before the surrounding vertical padding is added.
const MAX_INPUT_HEIGHT: u16 = 12;
const HIGHLIGHT_SLASH_PRIORITY: u8 = 6;
const HIGHLIGHT_MENTION_PRIORITY: u8 = 7;
const HIGHLIGHT_SUBAGENT_PRIORITY: u8 = 8;
const HIGHLIGHT_PASTE_PRIORITY: u8 = 9;
const HIGHLIGHT_IMAGE_BADGE_PRIORITY: u8 = 10;

#[derive(Clone, Copy)]
pub(crate) struct InputRenderGeometry {
    pub field: Rect,
    pub prompt: Rect,
    pub text: Rect,
}

pub(crate) fn compute_render_geometry(area: Rect) -> InputRenderGeometry {
    let vertical_pad = if area.height > INPUT_VERTICAL_PAD * 2 { INPUT_VERTICAL_PAD } else { 0 };
    let layout = MessageLayout::new(area.width);
    let content = Rect {
        x: area.x.saturating_add(layout.horizontal_padding),
        y: area.y.saturating_add(vertical_pad),
        width: area.width.saturating_sub(layout.horizontal_padding * 2),
        height: area.height.saturating_sub(vertical_pad * 2),
    };
    let [prompt, text] =
        Layout::horizontal([Constraint::Length(layout.marker_width), Constraint::Min(1)])
            .areas(content);

    InputRenderGeometry { field: area, prompt, text }
}

pub(crate) fn prompt_prefix_text() -> String {
    format!("{} ", theme::PROMPT_CHAR)
}

pub(crate) fn configure_input_textarea(app: &mut App) {
    let needs_highlight_update = app.input.highlight_version != app.input.content_version;

    {
        let textarea = app.input.editor_mut();
        textarea.set_style(Style::default().bg(theme::USER_MSG_BG));
        textarea.set_placeholder_text("Type a message...");
        textarea.set_placeholder_style(Style::default().fg(theme::DIM).bg(theme::USER_MSG_BG));
        textarea.set_cursor_line_style(Style::default());
        textarea.set_cursor_render_mode(CursorRenderMode::Hidden);
        textarea.set_cursor_style(Style::default().add_modifier(Modifier::REVERSED));
    }

    if needs_highlight_update || app.mention.is_some() {
        let lines = app.input.lines().to_vec();
        mention::retain_valid_committed_mentions(&lines, &mut app.committed_mentions);
        let committed_mentions = app.committed_mentions.clone();
        let active_mention = app.mention.as_ref();
        let file_index = &app.file_index;
        let textarea = app.input.editor_mut();
        textarea.clear_custom_highlight();
        apply_textarea_highlights(
            textarea,
            &lines,
            file_index,
            active_mention,
            &committed_mentions,
        );
        app.input.highlight_version = app.input.content_version;
    }
}

pub(crate) fn should_show_native_textarea_cursor(app: &App) -> bool {
    app.focus_owner() == FocusOwner::Input && app.composer_access().can_edit()
}

fn apply_textarea_highlights(
    textarea: &mut TextArea<'_>,
    lines: &[String],
    file_index: &crate::app::file_index::FileIndexState,
    active_mention: Option<&mention::MentionState>,
    committed_mentions: &[mention::CommittedMentionSpan],
) {
    let slash_style = Style::default().fg(theme::SLASH_COMMAND);
    let mention_style = Style::default().fg(Color::Cyan);
    let subagent_style = Style::default().fg(theme::SUBAGENT_TOKEN);
    let paste_style = Style::default().fg(Color::Green);
    let image_badge_style = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);

    for (row, line) in lines.iter().enumerate() {
        if let Some((start, end)) = slash_command_range(line) {
            textarea.custom_highlight(
                ((row, start), (row, end)),
                slash_style,
                HIGHLIGHT_SLASH_PRIORITY,
            );
        }

        for (start, end, _) in mention::find_mention_spans(row, line, file_index, active_mention) {
            textarea.custom_highlight(
                (
                    (row, crate::app::input::char_to_byte_index(line, start)),
                    (row, crate::app::input::char_to_byte_index(line, end)),
                ),
                mention_style,
                HIGHLIGHT_MENTION_PRIORITY,
            );
        }

        for span in committed_mentions.iter().filter(|span| span.row == row) {
            textarea.custom_highlight(
                (
                    (row, crate::app::input::char_to_byte_index(line, span.start_col)),
                    (row, crate::app::input::char_to_byte_index(line, span.end_col)),
                ),
                mention_style,
                HIGHLIGHT_MENTION_PRIORITY,
            );
        }

        for (start, end, _) in subagent::find_subagent_spans(line) {
            textarea.custom_highlight(
                ((row, start), (row, end)),
                subagent_style,
                HIGHLIGHT_SUBAGENT_PRIORITY,
            );
        }

        for atom in input_atoms::resolve_line_atoms(row, line) {
            match atom.kind {
                InputAtomKind::PasteBlock { .. } => textarea.custom_highlight(
                    ((row, atom.start_byte), (row, atom.end_byte)),
                    paste_style,
                    HIGHLIGHT_PASTE_PRIORITY,
                ),
                InputAtomKind::ImageBadge { .. } => textarea.custom_highlight(
                    ((row, atom.start_byte), (row, atom.end_byte)),
                    image_badge_style,
                    HIGHLIGHT_IMAGE_BADGE_PRIORITY,
                ),
            }
        }
    }
}

fn slash_command_range(line: &str) -> Option<(usize, usize)> {
    let start = line.find(|c: char| !c.is_whitespace())?;
    if line.as_bytes().get(start).copied() != Some(b'/') {
        return None;
    }
    let rel_end =
        line[start..].find(char::is_whitespace).unwrap_or_else(|| line.len().saturating_sub(start));
    let end = start + rel_end;
    if end <= start + 1 {
        return None;
    }
    Some((start, end))
}

/// Desired editor height, independent of the separately produced hint rows.
pub fn editor_height(app: &mut App, area_width: u16) -> u16 {
    let content_width = MessageLayout::new(area_width).content_width;
    let input_lines = app.input.measure_visual_lines(content_width, MAX_INPUT_HEIGHT);
    input_lines.saturating_add(INPUT_VERTICAL_PAD * 2)
}

/// Field height for read-only status rows rendered in place of the draft.
pub(crate) fn status_field_height(status_rows: usize) -> u16 {
    u16::try_from(status_rows).unwrap_or(u16::MAX).saturating_add(INPUT_VERTICAL_PAD * 2)
}
