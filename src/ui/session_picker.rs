use crate::app::{App, RecentSessionInfo};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use std::time::{SystemTime, UNIX_EPOCH};

use super::theme;

pub fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    app.cached_frame_area = area;

    let outer = Block::default()
        .borders(Borders::ALL)
        .title("Resume Session")
        .border_style(Style::default().fg(theme::DIM));
    frame.render_widget(outer, area);

    let inner = area.inner(Margin { vertical: 1, horizontal: 2 });
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(inner);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "Select a session to resume",
            Style::default().fg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD),
        ))),
        chunks[0],
    );
    frame.render_widget(
        Paragraph::new("Recent sessions for this project directory.")
            .style(Style::default().fg(theme::DIM)),
        chunks[1],
    );

    if crate::app::session_picker::startup_picker_is_loading(app) {
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
        lines.push(Line::from(Span::styled(
            format!("{marker} {}", display_primary(session)),
            base_style.add_modifier(Modifier::BOLD),
        )));
        if start + idx + 1 < end {
            lines.push(Line::default());
        }
    }

    frame.render_widget(Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false }), area);
}

fn footer_text(app: &App) -> &'static str {
    if crate::app::session_picker::startup_picker_is_loading(app) {
        "Preparing session picker | Ctrl+Q to quit"
    } else {
        "Enter to resume | Esc to start new session | Ctrl+Q to quit"
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
    use crate::app::{ActiveView, App, RecentSessionInfo};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn session(id: &str, title: &str) -> RecentSessionInfo {
        let now_ms =
            u64::try_from(SystemTime::now().duration_since(UNIX_EPOCH).expect("time").as_millis())
                .expect("millis fit in u64");
        RecentSessionInfo {
            session_id: id.to_owned(),
            summary: format!("summary {title}"),
            last_modified_ms: now_ms,
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
        app.active_view = ActiveView::SessionPicker;
        app.recent_sessions = vec![session("s1", "First Session")];

        let text = draw_text(&mut app);

        assert!(text.contains("Resume Session"));
        assert!(text.contains("just now - First Session"));
    }

    #[test]
    fn highlights_selected_session_with_marker() {
        let mut app = App::test_default();
        app.active_view = ActiveView::SessionPicker;
        app.recent_sessions = vec![session("s1", "First"), session("s2", "Second")];
        app.session_picker.selected = 1;

        let text = draw_text(&mut app);

        assert!(text.contains("> just now - Second"));
    }

    #[test]
    fn renders_empty_state_when_no_sessions_exist() {
        let mut app = App::test_default();
        app.active_view = ActiveView::SessionPicker;

        let text = draw_text(&mut app);

        assert!(text.contains("No recent sessions found for this directory."));
    }

    #[test]
    fn renders_loading_state_before_sessions_are_ready() {
        let mut app = App::test_default();
        app.active_view = ActiveView::SessionPicker;
        app.startup_session_picker_requested = true;

        let text = draw_text(&mut app);

        assert!(text.contains("Loading recent sessions..."));
        assert!(text.contains("Preparing session picker"));
    }

    #[test]
    fn limits_picker_to_ten_recent_sessions() {
        let mut app = App::test_default();
        app.active_view = ActiveView::SessionPicker;
        app.recent_sessions =
            (1..=11).map(|idx| session(&format!("s{idx}"), &format!("Session {idx}"))).collect();

        let text = draw_text_with_size(&mut app, 80, 30);

        assert!(text.contains("Session 10"));
        assert!(!text.contains("Session 11"));
    }
}
