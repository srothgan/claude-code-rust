// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use crate::app::input::parse_paste_placeholder_ranges;
use crate::app::mention;
use crate::app::subagent;
use crate::app::{App, FocusOwner};
use crate::ui::theme;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use tui_textarea::TextArea;

use super::autocomplete;

/// Horizontal padding to match header/footer inset.
const INPUT_PAD: u16 = 2;

/// Extra right-side breathing room so text doesn't touch the padded edge.
const INPUT_RIGHT_PAD: u16 = 1;

/// Prompt column width: "❯ " = 2 columns (icon + space)
const PROMPT_WIDTH: u16 = 2;

/// Maximum input area height (lines) to prevent the input from consuming the entire screen.
const MAX_INPUT_HEIGHT: u16 = 12;
const HIGHLIGHT_SLASH_PRIORITY: u8 = 6;
const HIGHLIGHT_MENTION_PRIORITY: u8 = 7;
const HIGHLIGHT_SUBAGENT_PRIORITY: u8 = 8;
const HIGHLIGHT_PASTE_PRIORITY: u8 = 9;
const HIGHLIGHT_IMAGE_BADGE_PRIORITY: u8 = 10;

/// Height of the login hint banner in lines (0 when no hint is active).
/// Used internally by `visual_line_count` so layout calculation stays in sync.
const LOGIN_HINT_LINES: u16 = 2;
const CANCEL_HINT_LINES: u16 = 1;
const PROMPT_SUGGESTION_HINT_LINES: u16 = 1;

#[derive(Clone, Copy)]
pub(crate) struct InputRenderGeometry {
    pub prompt: Rect,
    pub text: Rect,
}

/// Whether a login hint banner is active.
fn has_login_hint(app: &App) -> bool {
    app.login_hint.is_some()
}

fn has_cancel_hint(app: &App) -> bool {
    app.pending_cancel_origin.is_some()
}

fn has_prompt_suggestion_hint(app: &App) -> bool {
    app.input.is_empty()
        && app.focus_owner() == FocusOwner::Input
        && !autocomplete::is_active(app)
        && app.prompt_suggestion.as_deref().is_some_and(|suggestion| !suggestion.trim().is_empty())
}

pub(crate) fn hint_line_count(app: &App) -> u16 {
    let login = if has_login_hint(app) { LOGIN_HINT_LINES } else { 0 };
    let cancel = if has_cancel_hint(app) { CANCEL_HINT_LINES } else { 0 };
    let autocomplete = autocomplete::composer_hint_height(app);
    let suggestion = if has_prompt_suggestion_hint(app) { PROMPT_SUGGESTION_HINT_LINES } else { 0 };
    login + cancel + autocomplete + suggestion
}

pub(crate) fn compute_render_geometry(area: Rect, hint_lines: u16) -> InputRenderGeometry {
    let input_main_area = if hint_lines > 0 {
        let [hint, main] =
            Layout::vertical([Constraint::Length(hint_lines), Constraint::Min(1)]).areas(area);
        let _ = hint;
        main
    } else {
        area
    };

    let padded = Rect {
        x: input_main_area.x.saturating_add(INPUT_PAD),
        y: input_main_area.y,
        width: input_main_area.width.saturating_sub(INPUT_PAD * 2 + INPUT_RIGHT_PAD),
        height: input_main_area.height,
    };
    let [prompt, text] =
        Layout::horizontal([Constraint::Length(PROMPT_WIDTH), Constraint::Min(1)]).areas(padded);

    InputRenderGeometry { prompt, text }
}

pub(crate) fn prompt_prefix_text() -> String {
    format!("{} ", theme::PROMPT_CHAR)
}

pub(crate) fn configure_input_textarea(app: &mut App) {
    let needs_highlight_update = app.input.highlight_version != app.input.content_version;

    {
        let textarea = app.input.editor_mut();
        textarea.set_placeholder_text("Type a message...");
        textarea.set_placeholder_style(Style::default().fg(theme::DIM));
        textarea.set_cursor_line_style(Style::default());
        textarea.set_cursor_style(Style::default().add_modifier(Modifier::REVERSED));
    }

    if needs_highlight_update {
        let lines = app.input.lines().to_vec();
        let textarea = app.input.editor_mut();
        textarea.clear_custom_highlight();
        apply_textarea_highlights(textarea, &lines);
        app.input.highlight_version = app.input.content_version;
    }
}

fn apply_textarea_highlights(textarea: &mut TextArea<'_>, lines: &[String]) {
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

        for (start, end, _) in mention::find_mention_spans(line) {
            textarea.custom_highlight(
                ((row, start), (row, end)),
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

        for (start, end) in parse_paste_placeholder_ranges(line) {
            textarea.custom_highlight(
                ((row, start), (row, end)),
                paste_style,
                HIGHLIGHT_PASTE_PRIORITY,
            );
        }

        for (start, end, _) in crate::app::clipboard_image::find_image_badge_spans(line) {
            textarea.custom_highlight(
                ((row, start), (row, end)),
                image_badge_style,
                HIGHLIGHT_IMAGE_BADGE_PRIORITY,
            );
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

/// Total visual height for the input area: input lines + hint banners.
/// Called by the layout to allocate the correct input area height.
pub fn visual_line_count(app: &mut App, area_width: u16) -> u16 {
    let hint = hint_line_count(app);
    let content_width =
        area_width.saturating_sub(INPUT_PAD * 2 + INPUT_RIGHT_PAD).saturating_sub(PROMPT_WIDTH);
    let input_lines = app.input.measure_visual_lines(content_width, MAX_INPUT_HEIGHT);
    hint + input_lines
}

#[cfg(test)]
mod tests {
    use super::{
        CANCEL_HINT_LINES, LOGIN_HINT_LINES, MAX_INPUT_HEIGHT, PROMPT_SUGGESTION_HINT_LINES,
        configure_input_textarea, slash_command_range, visual_line_count,
    };
    use crate::app::subagent::find_subagent_spans;
    use crate::app::{App, CancelOrigin, FocusTarget, LoginHint};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::Modifier;
    use ratatui::widgets::Widget;

    #[test]
    fn slash_range_matches_leading_command_token() {
        assert_eq!(slash_command_range("/mode plan"), Some((0, 5)));
        assert_eq!(slash_command_range("  /mode plan"), Some((2, 7)));
    }

    #[test]
    fn slash_range_ignores_non_command_lines() {
        assert_eq!(slash_command_range("hello /mode"), None);
        assert_eq!(slash_command_range("/"), None);
        assert_eq!(slash_command_range("   "), None);
    }

    #[test]
    fn subagent_spans_match_valid_ampersand_tokens() {
        let spans = find_subagent_spans("&reviewer and &explore");
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].2, "reviewer");
        assert_eq!(spans[1].2, "explore");
    }

    #[test]
    fn subagent_spans_reject_double_ampersand_shell_syntax() {
        let spans = find_subagent_spans("cargo test && cargo clippy");
        assert!(spans.is_empty());
    }

    #[test]
    fn visual_line_count_uses_textarea_max_rows() {
        let mut app = App::test_default();
        app.input.set_text(&"x".repeat(500));
        assert_eq!(visual_line_count(&mut app, 8), MAX_INPUT_HEIGHT);
    }

    #[test]
    fn visual_line_count_includes_login_hint_rows() {
        let mut app = App::test_default();
        app.login_hint = Some(LoginHint {
            method_name: "oauth".to_owned(),
            method_description: "Sign in".to_owned(),
        });
        assert_eq!(visual_line_count(&mut app, 80), LOGIN_HINT_LINES + 1);
    }

    #[test]
    fn visual_line_count_includes_cancel_hint_row() {
        let mut app = App::test_default();
        app.pending_cancel_origin = Some(CancelOrigin::AutoQueue);
        assert_eq!(visual_line_count(&mut app, 80), CANCEL_HINT_LINES + 1);
    }

    #[test]
    fn visual_line_count_includes_prompt_suggestion_hint_row() {
        let mut app = App::test_default();
        app.prompt_suggestion = Some("Write tests for the retry flow".to_owned());
        assert_eq!(visual_line_count(&mut app, 80), PROMPT_SUGGESTION_HINT_LINES + 1);
    }

    #[test]
    fn visual_line_count_includes_autocomplete_hint_rows() {
        let mut app = App::test_default();
        app.input.set_text("@");
        let _ = app.input.set_cursor(0, 1);
        crate::app::mention::activate(&mut app);

        assert_eq!(visual_line_count(&mut app, 80), 2);
    }

    #[test]
    fn visual_line_count_hides_prompt_suggestion_hint_when_input_not_empty() {
        let mut app = App::test_default();
        app.prompt_suggestion = Some("Write tests for the retry flow".to_owned());
        app.input.set_text("draft");
        assert_eq!(visual_line_count(&mut app, 80), 1);
    }

    #[test]
    fn visual_line_count_hides_prompt_suggestion_hint_when_input_lacks_focus() {
        let mut app = App::test_default();
        app.prompt_suggestion = Some("Write tests for the retry flow".to_owned());
        app.pending_interaction_ids.push("perm-1".to_owned());
        app.claim_focus_target(FocusTarget::Permission);
        assert_eq!(visual_line_count(&mut app, 80), 1);
    }

    #[test]
    fn direct_textarea_render_preserves_cursor_cell_at_line_end() {
        let mut app = App::test_default();
        app.input.set_text("hello");
        let _ = app.input.set_cursor(0, 5);
        configure_input_textarea(&mut app);

        let area = Rect::new(0, 0, 12, 1);
        let mut buffer = Buffer::empty(area);
        app.input.editor().render(area, &mut buffer);

        let cursor_cell = buffer.cell((5, 0)).expect("cursor cell");
        assert_eq!(cursor_cell.symbol(), " ");
        assert!(cursor_cell.style().add_modifier.contains(Modifier::REVERSED));
    }
}
