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
use crate::app::session_picker::{picker_sessions, session_title};

const SESSION_DESCRIPTION: &str =
    "Recent sessions for this project. Search by name, ID, or branch.";
const TURN_DESCRIPTION: &str = "The original session will not change. The selected message and everything after it will be omitted from the fork.";
const TURN_CONTENT_INDENT: &str = "  ";

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
    let footer = footer_text(app);
    let footer_height = wrapped_line_count(footer, inner.width);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(description_height),
            Constraint::Length(if selecting_turn { 0 } else { 2 }),
            Constraint::Min(3),
            Constraint::Length(footer_height),
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
    if !selecting_turn {
        let matching = crate::app::session_picker::picker_session_count(app);
        frame.render_widget(
            Paragraph::new(format!(
                "Search: {}  ({matching}/{})",
                app.session_picker.query,
                app.recent_sessions.len()
            ))
            .style(Style::default().fg(theme::RUST_ORANGE)),
            chunks[2],
        );
    }

    render_body(frame, chunks[3], app);

    frame.render_widget(
        Paragraph::new(footer).style(Style::default().fg(theme::DIM)).wrap(Wrap { trim: false }),
        chunks[4],
    );
}

fn render_body(frame: &mut Frame, area: Rect, app: &mut App) {
    let selecting_turn = app.session_picker.turn_session_id.is_some();
    if selecting_turn && app.sdk_inventory.rewind_targets_in_flight {
        frame.render_widget(
            Paragraph::new("Loading session messages...")
                .style(Style::default().fg(theme::DIM))
                .wrap(Wrap { trim: false }),
            area,
        );
    } else if selecting_turn && app.sdk_inventory.rewind_targets_error.is_some() {
        frame.render_widget(
            Paragraph::new(app.sdk_inventory.rewind_targets_error.as_deref().unwrap_or_default())
                .style(Style::default().fg(theme::STATUS_ERROR))
                .wrap(Wrap { trim: false }),
            area,
        );
    } else if selecting_turn && crate::app::session_picker::picker_turn_count(app) == 0 {
        frame.render_widget(
            Paragraph::new("No resumable user messages found in this session.")
                .style(Style::default().fg(theme::DIM))
                .wrap(Wrap { trim: false }),
            area,
        );
    } else if selecting_turn && app.session_picker.turn_reading {
        render_turn_reader(frame, area, app);
    } else if selecting_turn {
        render_turn_list(frame, area, app);
    } else if crate::app::session_picker::startup_picker_is_loading(app) {
        frame.render_widget(
            Paragraph::new("Loading recent sessions...")
                .style(Style::default().fg(theme::DIM))
                .wrap(Wrap { trim: false }),
            area,
        );
    } else if crate::app::session_picker::picker_session_count(app) == 0 {
        frame.render_widget(
            Paragraph::new(if app.recent_sessions.is_empty() {
                "No recent sessions found for this directory."
            } else {
                "No sessions match your search."
            })
            .style(Style::default().fg(theme::DIM))
            .wrap(Wrap { trim: false }),
            area,
        );
    } else {
        render_session_list(frame, area, app);
    }
}

fn wrapped_line_count(text: &str, width: u16) -> u16 {
    u16::try_from(Paragraph::new(text).wrap(Wrap { trim: false }).line_count(width.max(1)))
        .unwrap_or(u16::MAX)
        .max(1)
}

fn turn_text(
    target: &crate::agent::model::RewindTarget,
    number: usize,
    selected: bool,
    width: u16,
) -> Text<'static> {
    let marker = if selected { ">" } else { " " };
    let heading_style = Style::default()
        .fg(if selected { theme::RUST_ORANGE } else { theme::DIM })
        .add_modifier(Modifier::BOLD);
    let plain = super::terminal_output::strip_ansi(&target.input_text);
    let plain = plain
        .chars()
        .filter(|ch| !ch.is_control() || matches!(ch, '\n' | '\t'))
        .collect::<String>();
    let prefix = [super::wrap::StyledChunk {
        text: format!("{marker:<gutter$}", gutter = TURN_CONTENT_INDENT.len()),
        style: heading_style,
    }];
    let body = [
        super::wrap::StyledChunk { text: format!("Message {number}\n"), style: heading_style },
        super::wrap::StyledChunk { text: plain, style: Style::default() },
    ];
    Text::from(super::wrap::wrap_styled_chunks_with_hanging_prefix(
        &prefix,
        &body,
        usize::from(width),
        Style::default(),
    ))
}

fn render_turn_reader(frame: &mut Frame, area: Rect, app: &mut App) {
    let Some(target) = app.sdk_inventory.rewind_targets.get(app.session_picker.turn_selected)
    else {
        return;
    };
    let number = app.sdk_inventory.rewind_targets.len() - app.session_picker.turn_selected;
    let text = turn_text(target, number, true, area.width);
    let max_scroll = text.lines.len().saturating_sub(usize::from(area.height));
    app.session_picker.turn_read_scroll = app.session_picker.turn_read_scroll.min(max_scroll);
    let visible = text
        .lines
        .into_iter()
        .skip(app.session_picker.turn_read_scroll)
        .take(usize::from(area.height))
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(Text::from(visible)), area);
}

fn render_turn_list(frame: &mut Frame, area: Rect, app: &mut App) {
    const MAX_PREVIEW_LINES: usize = 4;
    let count = app.sdk_inventory.rewind_targets.len();
    // Reserve a row for the expansion hint when the viewport is short.
    let preview_limit = usize::from(area.height).saturating_sub(1).clamp(1, MAX_PREVIEW_LINES + 1);
    let previews = app
        .sdk_inventory
        .rewind_targets
        .iter()
        .enumerate()
        .map(|(index, target)| {
            let text = turn_text(
                target,
                count - index,
                index == app.session_picker.turn_selected,
                area.width,
            );
            let full_height = text.lines.len();
            let paragraph = Paragraph::new(text);
            let more = full_height > preview_limit;
            let preview_height = full_height.min(preview_limit);
            (paragraph, preview_height, more)
        })
        .collect::<Vec<_>>();
    let heights =
        previews.iter().map(|(_, height, more)| height + usize::from(*more)).collect::<Vec<_>>();
    let selected = app.session_picker.turn_selected;
    let mut start = app.session_picker.turn_scroll_offset.min(selected);
    let mut occupied =
        heights[start..=selected].iter().map(|height| height + 1).sum::<usize>().saturating_sub(1);
    while start < selected && occupied > usize::from(area.height) {
        occupied = occupied.saturating_sub(heights[start] + 1);
        start += 1;
    }
    app.session_picker.turn_scroll_offset = start;
    let mut y = area.y;
    for (paragraph, preview_height, more) in previews.into_iter().skip(start) {
        let height =
            u16::try_from(preview_height).unwrap_or(u16::MAX).min(area.bottom().saturating_sub(y));
        if height == 0 {
            break;
        }
        frame.render_widget(paragraph, Rect::new(area.x, y, area.width, height));
        y = y.saturating_add(height);
        if more && y < area.bottom() {
            frame.render_widget(
                Paragraph::new(format!("{TURN_CONTENT_INDENT}... more - Right to read"))
                    .style(Style::default().fg(theme::DIM)),
                Rect::new(area.x, y, area.width, 1),
            );
            y = y.saturating_add(1);
        }
        y = y.saturating_add(1);
    }
}

fn render_session_list(frame: &mut Frame, area: Rect, app: &mut App) {
    let visible_count = usize::from((area.height / 2).max(1));
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
    let mut lines = Vec::with_capacity((end - start) * 2);
    for (idx, session) in picker_sessions(app).skip(start).take(end - start).enumerate() {
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
            &format!("{marker}{current_label} {}", display_primary(session)),
            base_style,
            selected,
        ));
        let metadata =
            session.git_branch.as_deref().filter(|branch| !branch.is_empty()).map_or_else(
                || session.session_id.clone(),
                |branch| format!("{branch} | {}", session.session_id),
            );
        lines.push(Line::from(Span::styled(
            fit_with_ellipsis(&format!("  {metadata}"), usize::from(area.width)),
            if selected { base_style } else { Style::default().fg(theme::DIM) },
        )));
    }

    frame.render_widget(Paragraph::new(Text::from(lines)), area);
}

fn session_line(width: u16, left: &str, base_style: Style, selected: bool) -> Line<'static> {
    const FULL_ACTION: &str = "fork ›";
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
    } else if app.session_picker.turn_reading {
        "Up/Down scroll | Enter to fork before message | Left/Esc back to messages | Ctrl+Q to quit"
    } else if app.session_picker.turn_session_id.is_some() {
        "Up/Down select | Right read message | Enter to fork before message | Left/Esc back | Ctrl+Q to quit"
    } else {
        "Type to search | Up/Down select | Enter to resume | Right choose fork point | Backspace edit | Esc close | Ctrl+Q to quit"
    }
}

fn display_primary(session: &RecentSessionInfo) -> String {
    format!("{} - {}", format_relative_age(session.last_modified_ms), session_title(session))
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
    fn scrolls_to_sessions_beyond_the_first_ten() {
        let mut app = App::test_default();
        app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::SessionPicker);
        app.recent_sessions =
            (1..=11).map(|idx| session(&format!("s{idx}"), &format!("Session {idx}"))).collect();

        crate::app::session_picker::handle_key(
            &mut app,
            crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::End,
                crossterm::event::KeyModifiers::NONE,
            ),
        );
        let text = draw_text_with_size(&mut app, 80, 14);

        let rows = text.lines().collect::<Vec<_>>();
        let selected = rows
            .iter()
            .position(|row| row.contains("> just now - Session 11"))
            .expect("last session visible and selected");
        assert!(rows[selected + 1].contains("main | s11"));
        assert!(!text.contains("Session 1 "));
        assert!(app.session_picker.scroll_offset > 0);
    }

    #[test]
    fn filtered_rows_associate_selection_with_the_correct_metadata() {
        let mut app = App::test_default();
        app.recent_sessions =
            vec![session("hidden-id", "Unrelated"), session("visible-id", "Target")];
        app.recent_sessions[1].git_branch = Some("feature/resume".into());
        app.session_runtime.session_id = Some(crate::agent::model::SessionId::new("visible-id"));
        crate::app::session_picker::open(&mut app);
        crate::app::session_picker::paste_search(&mut app, "target");
        let mut terminal = Terminal::new(TestBackend::new(100, 16)).expect("terminal");
        terminal.draw(|frame| render(frame, &mut app)).expect("draw");
        let buffer = terminal.backend().buffer();
        let rows = buffer
            .content
            .chunks(100)
            .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect::<String>())
            .collect::<Vec<_>>();
        let selected =
            rows.iter().position(|row| row.contains("Target")).expect("matching session");
        assert!(rows[selected].contains("> [current] just now - Target"));
        assert!(rows[selected + 1].contains("feature/resume | visible-id"));
        assert_eq!(
            buffer[(2, u16::try_from(selected).expect("row"))].bg,
            super::theme::RUST_ORANGE
        );
        assert_eq!(
            buffer[(2, u16::try_from(selected + 1).expect("row"))].bg,
            super::theme::RUST_ORANGE
        );
        assert!(!rows.iter().any(|row| row.contains("hidden-id") || row.contains("Unrelated")));
    }

    #[test]
    fn empty_search_results_clear_previous_session_rows_and_show_recovery_controls() {
        let mut app = App::test_default();
        app.recent_sessions = vec![session("s1", "First Session")];
        crate::app::session_picker::open(&mut app);
        let mut terminal = Terminal::new(TestBackend::new(80, 16)).expect("terminal");
        terminal.draw(|frame| render(frame, &mut app)).expect("initial draw");
        crate::app::session_picker::paste_search(&mut app, "missing");
        terminal.draw(|frame| render(frame, &mut app)).expect("filtered draw");
        let rows = terminal
            .backend()
            .buffer()
            .content
            .chunks(80)
            .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect::<String>())
            .collect::<Vec<_>>();
        assert!(rows.iter().any(|row| row.contains("No sessions match")));
        assert!(!rows.iter().any(|row| row.contains("First Session") || row.contains("main | s1")));
        assert!(rows.iter().any(|row| row.contains("Backspace edit")));
    }

    #[test]
    fn narrow_rows_keep_the_current_marker_and_fork_action_inside_the_session_row() {
        let mut app = App::test_default();
        app.recent_sessions = vec![session("long-id", &"界".repeat(100))];
        app.session_runtime.session_id = Some(crate::agent::model::SessionId::new("long-id"));
        crate::app::session_picker::open(&mut app);
        let text = draw_text_with_size(&mut app, 40, 20);
        let rows = text.lines().collect::<Vec<_>>();
        let selected =
            rows.iter().position(|row| row.contains("> [current]")).expect("selected session");
        assert!(rows[selected].contains("fork ›"));
        assert!(rows[selected + 1].contains("main | long-id"));
        assert!(rows.iter().any(|row| row.contains("Esc close")));
    }

    fn fork_target(id: &str, input: String) -> crate::agent::model::RewindTarget {
        crate::agent::model::RewindTarget {
            uuid: id.into(),
            first_text: "Summary only".into(),
            input_text: input,
            index: 0,
            previous_assistant_uuid: None,
            resume_anchor_uuid: None,
        }
    }

    fn first_content_column(buffer: &ratatui::buffer::Buffer, row: usize) -> Option<usize> {
        let width = usize::from(buffer.area.width);
        buffer
            .content
            .chunks(width)
            .nth(row)
            .expect("rendered row")
            .iter()
            .enumerate()
            .skip(2)
            .take(width.saturating_sub(4))
            .find_map(|(column, cell)| {
                cell.symbol().chars().any(|ch| !ch.is_whitespace()).then_some(column)
            })
    }

    #[test]
    fn fork_previews_align_headings_body_continuations_and_more_hints() {
        for width in [40, 80, 120] {
            let mut app = App::test_default();
            app.session_picker.turn_session_id = Some("s1".into());
            app.sdk_inventory.rewind_targets = vec![
                fork_target("selected", "\u{754c} alpha beta gamma ".repeat(80)),
                fork_target("earlier", "Earlier message".into()),
            ];
            let mut terminal = Terminal::new(TestBackend::new(width, 30)).expect("terminal");
            terminal.draw(|frame| render(frame, &mut app)).expect("draw");
            let buffer = terminal.backend().buffer();
            let rows = buffer
                .content
                .chunks(usize::from(width))
                .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect::<String>())
                .collect::<Vec<_>>();
            let heading =
                rows.iter().position(|row| row.contains("> Message 2")).expect("selected heading");
            assert_eq!(buffer[(4, u16::try_from(heading).expect("row"))].symbol(), "M");
            for row in heading + 1..=heading + 5 {
                assert_eq!(
                    first_content_column(buffer, row),
                    Some(4),
                    "body or expansion hint at width {width}, row {row}"
                );
            }
            assert!(rows[heading + 5].contains("more"));
            let earlier =
                rows.iter().position(|row| row.contains("Message 1")).expect("unselected heading");
            assert_eq!(first_content_column(buffer, earlier), Some(4));
            assert_eq!(first_content_column(buffer, earlier + 1), Some(4));
            assert!(rows[earlier + 1].contains("Earlier message"));
        }
    }

    #[test]
    fn fork_reader_uses_the_heading_gutter_without_clipping_wrapped_message_text() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let input = "0123456789".repeat(30);
        for width in [30, 50, 80] {
            let mut app = App::test_default();
            app.session_picker.turn_session_id = Some("s1".into());
            app.sdk_inventory.rewind_targets = vec![fork_target("message", input.clone())];
            crate::app::session_picker::handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
            );
            let mut terminal = Terminal::new(TestBackend::new(width, 50)).expect("terminal");
            terminal.draw(|frame| render(frame, &mut app)).expect("draw");
            let buffer = terminal.backend().buffer();
            let rows = buffer.content.chunks(usize::from(width)).collect::<Vec<_>>();
            let heading = rows
                .iter()
                .position(|row| {
                    row.iter()
                        .map(ratatui::buffer::Cell::symbol)
                        .collect::<String>()
                        .contains("> Message 1")
                })
                .expect("heading");
            let content_width = usize::from(width) - 6;
            let mut displayed = String::new();
            for (row, cells) in
                rows.iter().enumerate().skip(heading + 1).take(input.len().div_ceil(content_width))
            {
                assert_eq!(first_content_column(buffer, row), Some(4));
                let text = cells[4..usize::from(width) - 2]
                    .iter()
                    .map(ratatui::buffer::Cell::symbol)
                    .collect::<String>();
                displayed.push_str(text.trim_end());
            }
            assert_eq!(
                displayed, input,
                "wrapped text must not disappear behind the right margin at width {width}"
            );
            terminal.backend_mut().resize(width, 20);
            terminal.draw(|frame| render(frame, &mut app)).expect("resized draw");
            for _ in 0..3 {
                crate::app::session_picker::handle_key(
                    &mut app,
                    KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
                );
                terminal.draw(|frame| render(frame, &mut app)).expect("scroll");
            }
            let buffer = terminal.backend().buffer();
            let mut message_rows = 0;
            for (row_index, row) in buffer.content.chunks(usize::from(width)).enumerate() {
                if row[4].symbol().chars().any(|ch| ch.is_ascii_digit()) {
                    message_rows += 1;
                    assert_eq!(
                        first_content_column(buffer, row_index),
                        Some(4),
                        "resized reader at width {width}"
                    );
                }
            }
            assert!(message_rows > 0, "message text must remain visible after resizing");
            if width == 30 {
                assert!(
                    app.session_picker.turn_read_scroll > 0,
                    "exercise alignment after scrolling"
                );
            }
        }
    }

    #[test]
    fn fork_messages_wrap_the_full_input_and_preserve_paragraph_spacing() {
        let mut app = App::test_default();
        app.session_picker.turn_session_id = Some("s1".into());
        app.sdk_inventory.rewind_targets = vec![fork_target(
            "user-1",
            format!(
                "{}continuation-marker\n\nSecond-paragraph",
                "Read this message carefully. ".repeat(2)
            ),
        )];
        let text = draw_text_with_size(&mut app, 60, 25);
        let rows = text.lines().collect::<Vec<_>>();
        let heading =
            rows.iter().position(|row| row.contains("> Message 1")).expect("message heading");
        let continuation = rows
            .iter()
            .position(|row| row.contains("continuation-marker"))
            .expect("text after the former 60-character cutoff");
        let second =
            rows.iter().position(|row| row.contains("Second-paragraph")).expect("second paragraph");
        assert!(continuation > heading + 1, "long input must wrap");
        assert_eq!(second, continuation + 2, "paragraph break must remain visible");
        assert!(!text.contains("Summary only"));
    }

    #[test]
    fn fork_preview_removes_terminal_sequences_without_changing_the_stored_prompt() {
        let mut app = App::test_default();
        app.session_picker.turn_session_id = Some("s1".into());
        let input = "\u{1b}[31mRed\u{1b}[0m plain\u{1b}]52;c;payload\u{7}\n\nKeep paragraphs";
        app.sdk_inventory.rewind_targets = vec![fork_target("user-1", input.into())];
        let text = draw_text_with_size(&mut app, 80, 20);
        let rows = text.lines().collect::<Vec<_>>();
        let heading =
            rows.iter().position(|row| row.contains("> Message 1")).expect("message heading");
        assert!(rows[heading + 1].contains("Red plain"));
        assert!(rows[heading + 3].contains("Keep paragraphs"));
        assert!(!rows[heading + 1].contains("31m") && !rows[heading + 1].contains("payload"));
        assert_eq!(app.sdk_inventory.rewind_targets[0].input_text, input);
    }

    #[test]
    fn thousand_character_messages_have_four_line_previews_and_keep_other_messages_visible() {
        for width in [40, 80, 120] {
            let mut app = App::test_default();
            app.session_picker.turn_session_id = Some("s1".into());
            let input = format!("{}END-OF-MESSAGE", "Long message text. ".repeat(60));
            assert!(input.len() > 1_000);
            app.sdk_inventory.rewind_targets =
                vec![fork_target("user-2", input), fork_target("user-1", "Earlier prompt".into())];
            let text = draw_text_with_size(&mut app, width, 30);
            let rows = text.lines().collect::<Vec<_>>();
            let heading = rows
                .iter()
                .position(|row| row.contains("> Message 2"))
                .expect("long message heading");
            assert!(
                rows[heading + 5].contains("more"),
                "four text rows then expansion hint: {width}"
            );
            assert!(
                rows[heading + 7].contains("Message 1"),
                "next message must remain visible: {width}"
            );
            assert!(rows[heading + 8].contains("Earlier prompt"));
            assert!(!rows.iter().any(|row| row.contains("END-OF-MESSAGE")));
        }
    }

    #[test]
    fn long_fork_message_reader_reaches_the_end_and_returns_to_the_same_selection() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = App::test_default();
        app.session_picker.turn_session_id = Some("s1".into());
        let input = (1..=40).fold(String::new(), |mut text, line| {
            use std::fmt::Write as _;
            writeln!(text, "Detail line {line}: extended text of this message")
                .expect("write fixture");
            text
        });
        app.sdk_inventory.rewind_targets =
            vec![fork_target("user-2", input), fork_target("user-1", "Earlier message".into())];
        crate::app::session_picker::handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
        );
        assert!(app.session_picker.turn_reading);
        for _ in 0..80 {
            crate::app::session_picker::handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            );
            let _ = draw_text_with_size(&mut app, 70, 16);
        }
        let text = draw_text_with_size(&mut app, 70, 16);
        let rows = text.lines().collect::<Vec<_>>();
        assert!(
            rows.iter().any(|row| row.contains("Detail line 40")),
            "end of long input must be reachable"
        );
        assert!(
            !rows
                .iter()
                .any(|row| row.contains("Detail line 1 ") || row.contains("Earlier message"))
        );
        assert_eq!(app.session_picker.turn_selected, 0);
        assert!(app.session_picker.turn_read_scroll > 0);
        crate::app::session_picker::handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
        );
        assert!(!app.session_picker.turn_reading);
        let text = draw_text_with_size(&mut app, 70, 16);
        assert!(text.lines().any(|row| row.contains("> Message 2")));
        assert!(text.lines().any(|row| row.contains("Detail line 1")));
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
        let rows = text.lines().collect::<Vec<_>>();
        let selected =
            rows.iter().position(|row| row.contains("> Message 2")).expect("selected message");
        assert!(rows[selected + 1].contains("Resume from this message"));
        let earlier =
            rows.iter().position(|row| row.contains("Message 1")).expect("earlier message");
        assert!(rows[earlier + 1].contains("Start from the beginning"));
        assert!(text.contains("Left/Esc back"));
    }

    #[test]
    fn marks_the_current_session_and_exposes_fork_navigation() {
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
        let action_column = session_row.find("fork ›").expect("fork action");
        assert!(action_column > 50, "fork action should be aligned to the right: {session_row}");
        assert!(text.contains("Right choose fork point"));
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
