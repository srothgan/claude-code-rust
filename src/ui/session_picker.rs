// SPDX-License-Identifier: Apache-2.0
use crate::app::{App, RecentSessionInfo};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use std::time::{SystemTime, UNIX_EPOCH};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::theme;

const SESSION_DESCRIPTION: &str = "Recent sessions for this project directory.";
const TURN_DESCRIPTION: &str = "The original session will not change. The selected message and everything after it will be omitted from the fork.";

pub fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();

    let selecting_turn = app.session_picker.turn_session_id.is_some();
    let outer = Block::default()
        .borders(Borders::ALL)
        .title(if selecting_turn { "Fork Before Message" } else { "Resume Session" })
        .border_style(Style::default().fg(theme::DIM));
    frame.render_widget(outer, area);

    let inner = area.inner(Margin { vertical: 1, horizontal: 2 });
    let description = if selecting_turn { TURN_DESCRIPTION } else { SESSION_DESCRIPTION };
    let description_height = wrapped_line_count(description, inner.width);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(description_height),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(inner);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            if selecting_turn {
                "Select where the fork should stop"
            } else {
                "Select a session to resume"
            },
            Style::default().fg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD),
        ))),
        chunks[0],
    );
    frame.render_widget(
        Paragraph::new(description)
            .style(Style::default().fg(theme::DIM))
            .wrap(Wrap { trim: false }),
        chunks[1],
    );

    if selecting_turn && app.sdk_inventory.rewind_targets_in_flight {
        frame.render_widget(
            Paragraph::new("Loading session messages...")
                .style(Style::default().fg(theme::DIM))
                .wrap(Wrap { trim: false }),
            chunks[2],
        );
    } else if selecting_turn && app.sdk_inventory.rewind_targets_error.is_some() {
        frame.render_widget(
            Paragraph::new(app.sdk_inventory.rewind_targets_error.as_deref().unwrap_or_default())
                .style(Style::default().fg(theme::STATUS_ERROR))
                .wrap(Wrap { trim: false }),
            chunks[2],
        );
    } else if selecting_turn && crate::app::session_picker::picker_turn_count(app) == 0 {
        frame.render_widget(
            Paragraph::new("No resumable user messages found in this session.")
                .style(Style::default().fg(theme::DIM))
                .wrap(Wrap { trim: false }),
            chunks[2],
        );
    } else if selecting_turn {
        render_turn_list(frame, chunks[2], app);
    } else if crate::app::session_picker::startup_picker_is_loading(app) {
        frame.render_widget(
            Paragraph::new("Loading recent sessions...")
                .style(Style::default().fg(theme::DIM))
                .wrap(Wrap { trim: false }),
            chunks[2],
        );
    } else if crate::app::session_picker::picker_session_count(app) == 0 {
        frame.render_widget(
            Paragraph::new("No recent sessions found for this directory.")
                .style(Style::default().fg(theme::DIM))
                .wrap(Wrap { trim: false }),
            chunks[2],
        );
    } else {
        render_session_list(frame, chunks[2], app);
    }

    frame.render_widget(
        Paragraph::new(footer_text(app)).style(Style::default().fg(theme::DIM)),
        chunks[3],
    );
}

fn wrapped_line_count(text: &str, width: u16) -> u16 {
    u16::try_from(Paragraph::new(text).wrap(Wrap { trim: false }).line_count(width.max(1)))
        .unwrap_or(u16::MAX)
        .max(1)
}

fn render_turn_list(frame: &mut Frame, area: Rect, app: &mut App) {
    let visible_count = usize::from(area.height.max(1));
    let turn_count = crate::app::session_picker::picker_turn_count(app);
    let max_offset = turn_count.saturating_sub(visible_count);
    app.session_picker.turn_scroll_offset = app.session_picker.turn_scroll_offset.min(max_offset);
    if app.session_picker.turn_selected < app.session_picker.turn_scroll_offset {
        app.session_picker.turn_scroll_offset = app.session_picker.turn_selected;
    }
    if app.session_picker.turn_selected >= app.session_picker.turn_scroll_offset + visible_count {
        app.session_picker.turn_scroll_offset =
            app.session_picker.turn_selected + 1 - visible_count;
    }

    let start = app.session_picker.turn_scroll_offset;
    let end = (start + visible_count).min(turn_count);
    let lines = app.sdk_inventory.rewind_targets[start..end]
        .iter()
        .enumerate()
        .map(|(idx, target)| {
            let selected = start + idx == app.session_picker.turn_selected;
            let style = if selected {
                Style::default().fg(ratatui::style::Color::White).bg(theme::RUST_ORANGE)
            } else {
                Style::default()
            };
            let marker = if selected { ">" } else { " " };
            Line::from(Span::styled(
                format!(
                    "{marker} Msg. {} - {}",
                    turn_count.saturating_sub(start + idx),
                    truncate(&target.first_text, 60)
                ),
                style,
            ))
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false }), area);
}

fn render_session_list(frame: &mut Frame, area: Rect, app: &mut App) {
    let lines_per_item = 2;
    let visible_count = usize::from((area.height / lines_per_item).max(1));
    let session_count = crate::app::session_picker::picker_session_count(app);
    let max_offset = session_count.saturating_sub(visible_count);
    app.session_picker.scroll_offset = app.session_picker.scroll_offset.min(max_offset);
    if app.session_picker.selected < app.session_picker.scroll_offset {
        app.session_picker.scroll_offset = app.session_picker.selected;
    }
    if app.session_picker.selected >= app.session_picker.scroll_offset + visible_count {
        app.session_picker.scroll_offset = app.session_picker.selected + 1 - visible_count;
    }

    let start = app.session_picker.scroll_offset;
    let end = (start + visible_count).min(session_count);
    let mut lines = Vec::with_capacity((end - start) * usize::from(lines_per_item));
    for (idx, session) in app.recent_sessions[start..end].iter().enumerate() {
        let selected = start + idx == app.session_picker.selected;
        let base_style = if selected {
            Style::default().fg(ratatui::style::Color::White).bg(theme::RUST_ORANGE)
        } else {
            Style::default()
        };
        let marker = if selected { ">" } else { " " };
        let current = app
            .session_runtime
            .session_id
            .as_ref()
            .is_some_and(|active| active.as_str() == session.session_id);
        let current_label = if current { " [current]" } else { "" };
        lines.push(session_line(
            area.width,
            &format!("{marker} {}{current_label}", display_primary(session)),
            base_style,
            selected,
        ));
        if start + idx + 1 < end {
            lines.push(Line::default());
        }
    }

    frame.render_widget(Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false }), area);
}

fn session_line(width: u16, left: &str, base_style: Style, selected: bool) -> Line<'static> {
    const FULL_ACTION: &str = "messages ›";
    const COMPACT_ACTION: &str = "›";

    let available_width = usize::from(width);
    let action = if available_width >= UnicodeWidthStr::width(FULL_ACTION) + 3 {
        FULL_ACTION
    } else {
        COMPACT_ACTION
    };
    let action_width = UnicodeWidthStr::width(action);
    let max_left_width = available_width.saturating_sub(action_width.saturating_add(1));
    let fitted_left = fit_with_ellipsis(left, max_left_width);
    let left_width = UnicodeWidthStr::width(fitted_left.as_str());
    let gap = available_width.saturating_sub(left_width.saturating_add(action_width));
    let action_style = if selected {
        base_style.add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme::DIM).add_modifier(Modifier::BOLD)
    };

    Line::from(vec![
        Span::styled(fitted_left, base_style.add_modifier(Modifier::BOLD)),
        Span::styled(" ".repeat(gap), base_style),
        Span::styled(action, action_style),
    ])
}

fn footer_text(app: &App) -> &'static str {
    if crate::app::session_picker::startup_picker_is_loading(app) {
        "Preparing session picker | Ctrl+Q to quit"
    } else if app.session_picker.turn_session_id.is_some() {
        "Enter to fork before message | Left/Esc back | Ctrl+Q to quit"
    } else {
        "Enter to resume | Right choose message | Esc close | Ctrl+Q to quit"
    }
}

fn display_primary(session: &RecentSessionInfo) -> String {
    format!("{} - {}", format_relative_age(session.last_modified_ms), display_title(session))
}

fn display_title(session: &RecentSessionInfo) -> String {
    let title = session
        .custom_title
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| session.first_prompt.as_deref().filter(|value| !value.trim().is_empty()))
        .or_else(|| {
            let summary = session.summary.trim();
            (!summary.is_empty()).then_some(summary)
        })
        .unwrap_or(&session.session_id);
    truncate(title, 60)
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() { format!("{truncated}...") } else { truncated }
}

fn fit_with_ellipsis(value: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(value) <= max_width {
        return value.to_owned();
    }
    if max_width <= 3 {
        return ".".repeat(max_width);
    }

    let content_width = max_width - 3;
    let mut used_width = 0;
    let mut fitted = String::new();
    for ch in value.chars() {
        let char_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used_width + char_width > content_width {
            break;
        }
        fitted.push(ch);
        used_width += char_width;
    }
    fitted.push_str("...");
    fitted
}

fn format_relative_age(last_modified_ms: u64) -> String {
    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let then_secs = last_modified_ms / 1_000;
    if then_secs == 0 || then_secs >= now_secs {
        return "just now".to_owned();
    }

    let delta = now_secs - then_secs;
    if delta < 60 {
        return format!("{delta}s ago");
    }
    if delta < 60 * 60 {
        return format!("{}m ago", delta / 60);
    }
    if delta < 24 * 60 * 60 {
        return format!("{}h ago", delta / (60 * 60));
    }
    let days = delta / (24 * 60 * 60);
    let hours = (delta / (60 * 60)) % 24;
    format!("{days}d {hours}h ago")
}

#[cfg(test)]
mod tests {
    use super::render;
    use crate::app::{App, FullscreenView, RecentSessionInfo, SurfaceMode};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn session(id: &str, title: &str) -> RecentSessionInfo {
        RecentSessionInfo {
            session_id: id.to_owned(),
            summary: format!("summary {title}"),
            // Zero maps to the stable "just now" rendering path without depending on wall-clock timing.
            last_modified_ms: 0,
            file_size_bytes: 1,
            cwd: Some("/test/project".to_owned()),
            git_branch: Some("main".to_owned()),
            custom_title: Some(title.to_owned()),
            first_prompt: Some(format!("prompt {title}")),
        }
    }

    fn draw_text_with_size(app: &mut App, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal.draw(|frame| render(frame, app)).expect("draw");
        terminal
            .backend()
            .buffer()
            .content
            .chunks(usize::from(width))
            .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn draw_text(app: &mut App) -> String {
        draw_text_with_size(app, 80, 14)
    }

    #[test]
    fn renders_session_titles() {
        let mut app = App::test_default();
        app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::SessionPicker);
        app.recent_sessions = vec![session("s1", "First Session")];

        let text = draw_text(&mut app);

        assert!(text.contains("Resume Session"));
        assert!(text.contains("just now - First Session"));
    }

    #[test]
    fn highlights_selected_session_with_marker() {
        let mut app = App::test_default();
        app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::SessionPicker);
        app.recent_sessions = vec![session("s1", "First"), session("s2", "Second")];
        app.session_picker.selected = 1;

        let text = draw_text(&mut app);

        assert!(text.contains("> just now - Second"));
    }

    #[test]
    fn renders_empty_state_when_no_sessions_exist() {
        let mut app = App::test_default();
        app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::SessionPicker);

        let text = draw_text(&mut app);

        assert!(text.contains("No recent sessions found for this directory."));
    }

    #[test]
    fn renders_loading_state_before_sessions_are_ready() {
        let mut app = App::test_default();
        app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::SessionPicker);
        app.test_request_startup_session_picker();

        let text = draw_text(&mut app);

        assert!(text.contains("Loading recent sessions..."));
        assert!(text.contains("Preparing session picker"));
    }

    #[test]
    fn limits_picker_to_ten_recent_sessions() {
        let mut app = App::test_default();
        app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::SessionPicker);
        app.recent_sessions =
            (1..=11).map(|idx| session(&format!("s{idx}"), &format!("Session {idx}"))).collect();

        let text = draw_text_with_size(&mut app, 80, 30);

        assert!(text.contains("Session 10"));
        assert!(!text.contains("Session 11"));
    }

    #[test]
    fn renders_message_drop_level_and_navigation_help() {
        let mut app = App::test_default();
        app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::SessionPicker);
        app.session_picker.turn_session_id = Some("s1".to_owned());
        app.sdk_inventory.rewind_targets = vec![
            crate::agent::model::RewindTarget {
                uuid: "user-2".to_owned(),
                first_text: "Resume from this message".to_owned(),
                input_text: "Resume from this message".to_owned(),
                index: 2,
                previous_assistant_uuid: Some("assistant-1".to_owned()),
                resume_anchor_uuid: Some("assistant-1".to_owned()),
            },
            crate::agent::model::RewindTarget {
                uuid: "user-1".to_owned(),
                first_text: "Start from the beginning".to_owned(),
                input_text: "Start from the beginning".to_owned(),
                index: 0,
                previous_assistant_uuid: None,
                resume_anchor_uuid: None,
            },
        ];

        let text = draw_text(&mut app);

        assert!(text.contains("Fork Before Message"));
        assert!(text.contains("> Msg. 2 - Resume from this message"));
        assert!(text.contains("  Msg. 1 - Start from the beginning"));
        assert!(text.contains("Left/Esc back"));
    }

    #[test]
    fn marks_the_current_session_and_exposes_message_navigation() {
        let mut app = App::test_default();
        app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::SessionPicker);
        app.session_runtime.session_id = Some(crate::agent::model::SessionId::new("s1"));
        app.recent_sessions = vec![session("s1", "Current Session")];

        let text = draw_text(&mut app);

        assert!(text.contains("[current]"));
        let session_row = text
            .lines()
            .find(|line| line.contains("Current Session"))
            .expect("current session row");
        let action_column = session_row.find("messages ›").expect("message action");
        assert!(action_column > 50, "message action should be aligned to the right: {session_row}");
        assert!(text.contains("Right choose message"));
    }

    #[test]
    fn renders_correlated_rewind_target_error_in_picker() {
        let mut app = App::test_default();
        app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::SessionPicker);
        app.session_picker.turn_session_id = Some("s1".to_owned());
        app.sdk_inventory.rewind_targets_error = Some("Session history is unavailable".to_owned());

        let text = draw_text(&mut app);

        assert!(text.contains("Session history is unavailable"));
        assert!(!text.contains("No resumable user messages"));
    }
}
