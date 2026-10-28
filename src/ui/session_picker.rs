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
    render_at(frame, app, SystemTime::now());
}

fn render_at(frame: &mut Frame, app: &mut App, now: SystemTime) {
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

    render_body(frame, chunks[3], app, now);

    frame.render_widget(
        Paragraph::new(footer).style(Style::default().fg(theme::DIM)).wrap(Wrap { trim: false }),
        chunks[4],
    );
}

fn render_body(frame: &mut Frame, area: Rect, app: &mut App, now: SystemTime) {
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
        render_session_list(frame, area, app, now);
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

fn render_session_list(frame: &mut Frame, area: Rect, app: &mut App, now: SystemTime) {
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
            &format!("{marker}{current_label} {}", display_primary(session, now)),
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

fn display_primary(session: &RecentSessionInfo, now: SystemTime) -> String {
    format!("{} - {}", format_relative_age(session.last_modified_ms, now), session_title(session))
}

pub(super) fn fit_with_ellipsis(value: &str, max_width: usize) -> String {
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

fn format_relative_age(last_modified_ms: u64, now: SystemTime) -> String {
    let now_secs = now.duration_since(UNIX_EPOCH).map(|duration| duration.as_secs()).unwrap_or(0);
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

    #[test]
    fn startup_loading_clears_stale_rows_and_ready_inventory_restores_controls() {
        let mut app = App::test_default();
        app.recent_sessions = vec![session("stale-id", "Stale title")];
        crate::app::session_picker::open(&mut app);
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        redraw(&mut terminal, &mut app);
        assert_session_pair(terminal.backend().buffer(), "Stale title", "stale-id", true, false);
        app.test_request_startup_session_picker();
        redraw(&mut terminal, &mut app);
        let loading = rows(terminal.backend().buffer());
        let body = row_with(&loading, "Loading recent");
        let footer = row_with(&loading, "Ctrl+Q");
        assert!(body < footer);
        assert!(!loading.iter().any(|r| r.contains("Stale title")
            || r.contains("stale-id")
            || r.contains("Enter to resume")));
        let (conn, _rx) = crate::agent::client::AgentConnection::test_channel();
        app.session_runtime.conn = Some(std::rc::Rc::new(conn));
        app.startup.mark_recent_sessions_loaded();
        app.recent_sessions = vec![session("ready-id", "Ready title")];
        redraw(&mut terminal, &mut app);
        assert_session_pair(terminal.backend().buffer(), "Ready title", "ready-id", true, false);
        assert!(rows(terminal.backend().buffer()).iter().any(|r| r.contains("Enter to resume")));
        app.startup.resolve_session_picker();
        app.recent_sessions.clear();
        crate::app::session_picker::open(&mut app);
        redraw(&mut terminal, &mut app);
        let empty = rows(terminal.backend().buffer());
        assert!(row_with(&empty, "No recent sessions") < row_with(&empty, "Esc close"));
        assert!(!empty.iter().any(|r| r.contains("Ready title")
            || r.contains("ready-id")
            || r.contains("Loading recent")
            || r.contains("No sessions match")));
    }

    #[test]
    fn navigation_keeps_selected_session_metadata_visible_across_viewport_sizes() {
        let mut app = App::test_default();
        app.recent_sessions =
            (1..=24).map(|i| session(&format!("id-{i:03}"), &format!("TITLE_{i:03}"))).collect();
        crate::app::session_picker::open(&mut app);
        let mut terminal = Terminal::new(TestBackend::new(80, 17)).unwrap();
        for (width, height, code, title, id) in [
            (80, 17, crossterm::event::KeyCode::End, "TITLE_024", "id-024"),
            (52, 20, crossterm::event::KeyCode::Up, "TITLE_023", "id-023"),
            (65, 15, crossterm::event::KeyCode::Home, "TITLE_001", "id-001"),
        ] {
            key(&mut app, code);
            terminal.backend_mut().resize(width, height);
            redraw(&mut terminal, &mut app);
            assert_session_pair(terminal.backend().buffer(), title, id, true, false);
            let shown = rows(terminal.backend().buffer());
            assert!(shown.iter().any(|r| r.contains("Esc close")));
            let hidden = if title == "TITLE_001" { "TITLE_024" } else { "TITLE_001" };
            assert!(!shown.iter().any(|r| r.contains(hidden)));
        }
    }

    #[test]
    fn filtered_rows_associate_selection_with_the_correct_metadata() {
        let mut app = App::test_default();
        app.recent_sessions = vec![
            session("hidden-id", "Hidden"),
            session("alpha-id", "Target Alpha"),
            session("beta-id", "ignored"),
        ];
        app.recent_sessions[1].git_branch = Some("branch-alpha".into());
        app.recent_sessions[2].custom_title = Some(" ".into());
        app.recent_sessions[2].first_prompt = Some("Target Beta".into());
        app.recent_sessions[2].git_branch = Some("branch-beta".into());
        app.session_runtime.session_id = Some("beta-id".into());
        crate::app::session_picker::open(&mut app);
        crate::app::session_picker::paste_search(&mut app, "target");
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        let mut previous_colors = None;
        for (code, selected) in [
            (crossterm::event::KeyCode::Home, "alpha-id"),
            (crossterm::event::KeyCode::Down, "beta-id"),
        ] {
            key(&mut app, code);
            redraw(&mut terminal, &mut app);
            let b = terminal.backend().buffer();
            let alpha = assert_session_pair(
                b,
                "Target Alpha",
                "branch-alpha | alpha-id",
                selected == "alpha-id",
                false,
            );
            let beta = assert_session_pair(
                b,
                "Target Beta",
                "branch-beta | beta-id",
                selected == "beta-id",
                true,
            );
            let column = cell_column(b, alpha, "T");
            let color_a = b[(column, u16::try_from(alpha).unwrap())].bg;
            let color_b = b[(column, u16::try_from(beta).unwrap())].bg;
            assert_ne!(color_a, color_b);
            if let Some((previous_a, previous_b)) = previous_colors {
                assert_eq!(color_a, previous_b, "old selection must lose its highlight");
                assert_eq!(color_b, previous_a, "new selection must receive the highlight");
            }
            previous_colors = Some((color_a, color_b));
            assert_eq!(color_a, b[(column, u16::try_from(alpha + 1).unwrap())].bg);
            assert_eq!(color_b, b[(column, u16::try_from(beta + 1).unwrap())].bg);
            assert!(!rows(b).iter().any(|r| r.contains("Hidden") || r.contains("hidden-id")));
        }
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
        for (width, title) in [
            (100, "Long ASCII title ".repeat(20)),
            (40, "\u{754c}".repeat(100)),
            (12, "Compact".into()),
        ] {
            let mut app = App::test_default();
            app.recent_sessions = vec![session("long-id", &title)];
            app.session_runtime.session_id = Some("long-id".into());
            crate::app::session_picker::open(&mut app);
            let mut terminal = Terminal::new(TestBackend::new(width, 100)).unwrap();
            redraw(&mut terminal, &mut app);
            let b = terminal.backend().buffer();
            let shown = rows(b);
            let row = shown.iter().position(|r| r.contains('>') && r.contains('\u{203a}')).unwrap();
            let action = cell_column(b, row, "\u{203a}");
            assert_eq!(action, width - 3, "fork action must reach the right edge of the body");
            assert!(b[(action + 1, u16::try_from(row).unwrap())].symbol().trim().is_empty());
            assert_eq!(b[(width - 1, u16::try_from(row).unwrap())].symbol(), "\u{2502}");
            if width >= 40 {
                assert!(shown[row].contains("[current]"));
                assert!(shown[row].contains("fork"));
                assert!(shown[row + 1].contains("main | long-id"));
                assert!(shown[row].contains(if width == 40 { "\u{754c}" } else { "Long" }));
            } else {
                assert!(!shown[row].contains("fork"));
            }
            assert!(!shown[row + 1].contains('\u{203a}'));
            assert!(shown.iter().skip(row + 2).any(|r| r.contains("Esc")));
        }
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
    fn fork_previews_associate_bounded_content_and_expansion_with_each_message() {
        for width in [40, 80, 120] {
            let mut app = turn_app(vec![
                fork_target(
                    "long",
                    format!("{}OMITTED_TAIL", "\u{754c} alpha beta gamma ".repeat(80)),
                ),
                fork_target("short", "Neighbor source".into()),
            ]);
            let mut terminal = Terminal::new(TestBackend::new(width, 30)).unwrap();
            redraw(&mut terminal, &mut app);
            let b = terminal.backend().buffer();
            let shown = rows(b);
            let heading = row_with(&shown, "> Message 2");
            let neighbor = row_with(&shown, "Message 1");
            let gutter = usize::from(cell_column(b, heading, "M"));
            let hint = shown
                .iter()
                .enumerate()
                .skip(heading + 1)
                .take(neighbor - heading - 1)
                .find(|(_, r)| r.contains("more") && r.contains("Right"))
                .map(|(i, _)| i)
                .unwrap();
            assert!(hint > heading + 1);
            for row in heading + 1..=hint {
                assert_eq!(first_content_column(b, row), Some(gutter));
            }
            assert!(shown[neighbor + 1].contains("Neighbor source"));
            assert!(!shown.iter().any(|r| r.contains("OMITTED_TAIL")));
            assert!(row_with(&shown, "Left/Esc") > neighbor);
            key(&mut app, crossterm::event::KeyCode::Down);
            terminal.backend_mut().resize(width, 20);
            redraw(&mut terminal, &mut app);
            let shown = rows(terminal.backend().buffer());
            let selected = row_with(&shown, "> Message 1");
            assert!(shown[selected + 1].contains("Neighbor source"));
            assert_eq!(
                app.sdk_inventory.rewind_targets[app.session_picker.turn_selected].uuid,
                "short"
            );
        }
    }

    #[test]
    fn fork_reader_preserves_all_wrapped_source_and_clamped_scroll_slices() {
        let input = "abc0123456789XYZ".repeat(20);
        for width in [30, 50, 80] {
            let mut app = turn_app(vec![
                fork_target("chosen", input.clone()),
                fork_target("neighbor", "NEIGHBOR_BODY".into()),
            ]);
            key(&mut app, crossterm::event::KeyCode::Right);
            let mut terminal = Terminal::new(TestBackend::new(width, 50)).unwrap();
            redraw(&mut terminal, &mut app);
            let b = terminal.backend().buffer();
            let shown = rows(b);
            let heading = row_with(&shown, "> Message 2");
            let gutter = cell_column(b, heading, "M");
            let footer = row_with(&shown, "Up/Down scroll");
            let mut recovered = String::new();
            for row in heading + 1..footer {
                let text = (gutter..width - 2)
                    .map(|x| b[(x, u16::try_from(row).unwrap())].symbol())
                    .collect::<String>();
                if !text.trim().is_empty() {
                    assert_eq!(first_content_column(b, row), Some(usize::from(gutter)));
                    recovered.push_str(text.trim_end());
                }
            }
            assert_eq!(recovered, input);
            assert!(!shown.iter().any(|r| r.contains("NEIGHBOR_BODY")));
        }
        assert_reader_scroll_slices();
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
    fn turn_result_states_clear_stale_body_and_preserve_back_controls() {
        let mut app = turn_app(vec![fork_target("old", "STALE_BODY".into())]);
        key(&mut app, crossterm::event::KeyCode::Right);
        let mut terminal = Terminal::new(TestBackend::new(100, 25)).unwrap();
        redraw(&mut terminal, &mut app);
        assert!(rows(terminal.backend().buffer()).iter().any(|r| r.contains("STALE_BODY")));
        for (loading, error, body) in [
            (true, None, "Loading session messages"),
            (false, Some("Fixture history unavailable"), "Fixture history unavailable"),
            (false, None, "No resumable user messages"),
        ] {
            app.session_picker.reset_turn_navigation();
            app.sdk_inventory.rewind_targets.clear();
            app.sdk_inventory.rewind_targets_in_flight = loading;
            app.sdk_inventory.rewind_targets_error = error.map(str::to_owned);
            redraw(&mut terminal, &mut app);
            let shown = rows(terminal.backend().buffer());
            assert!(row_with(&shown, body) < row_with(&shown, "Left/Esc"));
            assert!(!shown.iter().any(|r| r.contains("STALE_BODY")));
            if error.is_some() {
                assert!(!shown.iter().any(|r| r.contains("No resumable")));
            }
            assert!(shown.iter().any(|r| r.contains("Ctrl+Q")));
        }
        app.sdk_inventory.rewind_targets = vec![fork_target("new", "RECOVERED_BODY".into())];
        redraw(&mut terminal, &mut app);
        let shown = rows(terminal.backend().buffer());
        assert!(shown[row_with(&shown, "> Message 1") + 1].contains("RECOVERED_BODY"));
        assert!(!shown.iter().any(|r| r.contains("unavailable") || r.contains("No resumable")));
    }
    fn key(app: &mut App, code: crossterm::event::KeyCode) {
        crate::app::session_picker::handle_key(
            app,
            crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::NONE),
        );
    }
    fn rows(buffer: &ratatui::buffer::Buffer) -> Vec<String> {
        buffer
            .content
            .chunks(usize::from(buffer.area.width))
            .map(|r| r.iter().map(ratatui::buffer::Cell::symbol).collect())
            .collect()
    }
    fn redraw(terminal: &mut Terminal<TestBackend>, app: &mut App) {
        terminal.draw(|f| render(f, app)).unwrap();
    }
    fn row_with(rows: &[String], text: &str) -> usize {
        rows.iter()
            .position(|r| r.contains(text))
            .unwrap_or_else(|| panic!("missing {text}: {rows:?}"))
    }
    fn cell_column(b: &ratatui::buffer::Buffer, row: usize, symbol: &str) -> u16 {
        (0..b.area.width).find(|x| b[(*x, u16::try_from(row).unwrap())].symbol() == symbol).unwrap()
    }
    fn assert_session_pair(
        b: &ratatui::buffer::Buffer,
        title: &str,
        metadata: &str,
        selected: bool,
        current: bool,
    ) -> usize {
        let shown = rows(b);
        let row = row_with(&shown, title);
        assert_eq!(shown[row].contains('>'), selected);
        assert_eq!(shown[row].contains("[current]"), current);
        assert!(shown[row + 1].contains(metadata), "{:?}", shown[row + 1]);
        assert!(!shown[row + 1].contains("fork"));
        row
    }
    fn turn_app(targets: Vec<crate::agent::model::RewindTarget>) -> App {
        let mut app = App::test_default();
        app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::SessionPicker);
        app.session_picker.turn_session_id = Some("inspected".into());
        app.sdk_inventory.rewind_targets = targets;
        app
    }
    fn assert_reader_scroll_slices() {
        use crossterm::event::KeyCode;
        let input = (1..=40).map(|n| format!("ROW_{n:03}")).collect::<Vec<_>>().join("\n");
        let mut app = turn_app(vec![
            fork_target("neighbor", "NEIGHBOR_BODY".into()),
            fork_target("chosen", input),
        ]);
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Right);
        let mut terminal = Terminal::new(TestBackend::new(70, 18)).unwrap();
        redraw(&mut terminal, &mut app);
        let visible = |b: &ratatui::buffer::Buffer| {
            rows(b)
                .into_iter()
                .filter_map(|r| r.find("ROW_").map(|i| r[i..i + 7].to_owned()))
                .collect::<Vec<_>>()
        };
        let beginning = visible(terminal.backend().buffer());
        assert_eq!(
            beginning,
            (1..=beginning.len()).map(|n| format!("ROW_{n:03}")).collect::<Vec<_>>()
        );
        assert!(!beginning.is_empty());
        for _ in 0..3 {
            key(&mut app, KeyCode::Down);
        }
        redraw(&mut terminal, &mut app);
        let middle = visible(terminal.backend().buffer());
        assert_eq!(
            middle,
            (3..3 + middle.len()).map(|n| format!("ROW_{n:03}")).collect::<Vec<_>>()
        );
        terminal.backend_mut().resize(45, 22);
        redraw(&mut terminal, &mut app);
        let resized = visible(terminal.backend().buffer());
        assert_eq!(
            resized,
            (3..3 + resized.len()).map(|n| format!("ROW_{n:03}")).collect::<Vec<_>>()
        );
        for _ in 0..80 {
            key(&mut app, KeyCode::Down);
        }
        redraw(&mut terminal, &mut app);
        let end = visible(terminal.backend().buffer());
        assert_eq!(end, (41 - end.len()..=40).map(|n| format!("ROW_{n:03}")).collect::<Vec<_>>());
        assert_eq!(end.last().unwrap(), "ROW_040");
        assert!(!rows(terminal.backend().buffer()).iter().any(|r| r.contains("NEIGHBOR_BODY")));
        key(&mut app, KeyCode::Left);
        redraw(&mut terminal, &mut app);
        let shown = rows(terminal.backend().buffer());
        assert!(shown[row_with(&shown, "> Message 1") + 1].contains("ROW_001"));
        assert_eq!(
            app.sdk_inventory.rewind_targets[app.session_picker.turn_selected].uuid,
            "chosen"
        );
        key(&mut app, KeyCode::Right);
        redraw(&mut terminal, &mut app);
        assert_eq!(visible(terminal.backend().buffer()).first().unwrap(), "ROW_001");
    }
    #[test]
    fn relative_age_boundaries_and_rendered_session_associations_use_one_fixed_clock() {
        let now = super::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        for (millis, expected) in [
            (0, "just now"),
            (1_000_001_000, "just now"),
            (1_000_000_000, "just now"),
            (999_999_999, "1s ago"),
            (999_941_000, "59s ago"),
            (999_940_000, "1m ago"),
            (996_401_000, "59m ago"),
            (996_400_000, "1h ago"),
            (913_601_000, "23h ago"),
            (913_600_000, "1d 0h ago"),
            (827_200_000, "2d 0h ago"),
            (823_600_000, "2d 1h ago"),
        ] {
            assert_eq!(super::format_relative_age(millis, now), expected, "timestamp={millis}");
        }
        let mut app = App::test_default();
        app.recent_sessions =
            vec![session("minute-id", "Minute session"), session("day-id", "Day session")];
        app.recent_sessions[0].last_modified_ms = 999_940_000;
        app.recent_sessions[1].last_modified_ms = 823_600_000;
        crate::app::session_picker::open(&mut app);
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal.draw(|f| super::render_at(f, &mut app, now)).unwrap();
        let b = terminal.backend().buffer();
        let minute = assert_session_pair(b, "Minute session", "minute-id", true, false);
        let day = assert_session_pair(b, "Day session", "day-id", false, false);
        let shown = rows(b);
        assert!(shown[minute].contains("1m ago"));
        assert!(!shown[minute].contains("2d"));
        assert!(shown[day].contains("2d 1h ago"));
        assert!(!shown[day].contains("1m ago"));
    }
}
