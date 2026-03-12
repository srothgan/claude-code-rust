use crate::app::App;
use crate::app::TrustSelection;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Margin};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use super::theme;

pub fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    app.cached_frame_area = area;

    let outer = Block::default()
        .borders(Borders::ALL)
        .title("Unknown Project")
        .border_style(Style::default().fg(theme::DIM));
    frame.render_widget(outer, area);

    let inner = area.inner(Margin { vertical: 1, horizontal: 2 });
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(3),
        ])
        .split(inner);

    frame.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            "Trust this project directory?",
            Style::default().fg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD),
        )])),
        chunks[0],
    );

    frame.render_widget(
        Paragraph::new(vec![
            Line::from(
                "Claude Rust will wait here until you choose whether to trust this workspace.",
            ),
            Line::default(),
            Line::from("Review the project before continuing if you are unsure."),
        ])
        .wrap(Wrap { trim: false }),
        chunks[1],
    );

    let message = app
        .trust
        .last_error
        .clone()
        .unwrap_or_else(|| "Choose Yes to continue or No to close Claude Rust.".to_owned());
    let message_style = if app.trust.last_error.is_some() {
        Style::default().fg(theme::STATUS_ERROR)
    } else {
        Style::default().fg(theme::DIM)
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(message, message_style))).wrap(Wrap { trim: false }),
        chunks[2],
    );

    frame.render_widget(Paragraph::new(action_lines(app)).wrap(Wrap { trim: false }), chunks[3]);
}

fn action_lines(app: &App) -> Vec<Line<'static>> {
    vec![
        action_line("Yes", app.trust.selection == TrustSelection::Yes),
        action_line("No", app.trust.selection == TrustSelection::No),
    ]
}

fn action_line(label: &str, selected: bool) -> Line<'static> {
    let marker = if selected { ">" } else { " " };
    let style = if selected {
        Style::default().fg(ratatui::style::Color::White).bg(theme::RUST_ORANGE)
    } else {
        Style::default().fg(theme::DIM)
    }
    .add_modifier(Modifier::BOLD);

    Line::from(Span::styled(format!("{marker} {label}"), style))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    fn buffer_text(buffer: &Buffer) -> String {
        let mut out = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn draw_text(app: &mut App) -> String {
        let backend = TestBackend::new(70, 14);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal.draw(|frame| render(frame, app)).expect("draw");
        buffer_text(terminal.backend().buffer())
    }

    #[test]
    fn trusted_view_shows_selection_at_top_without_storage_details() {
        let mut app = App::test_default();
        app.trust.selection = TrustSelection::Yes;
        app.cwd_raw = r"C:\work\project".to_owned();

        let text = draw_text(&mut app);

        assert!(text.contains("Unknown Project"));
        assert!(!text.contains("~/.claude.json"));
        assert!(!text.contains("hasTrustDialogAccepted"));
        assert!(!text.contains("Directory:"));
        assert!(!text.contains(r"C:\work\project"));
    }

    #[test]
    fn trusted_view_highlights_no_when_selected() {
        let mut app = App::test_default();
        app.trust.selection = TrustSelection::No;

        let text = draw_text(&mut app);

        assert!(text.contains("  Yes"));
        assert!(text.contains("> No"));
    }

    #[test]
    fn trusted_view_renders_actions_below_body_text() {
        let mut app = App::test_default();
        app.trust.selection = TrustSelection::Yes;

        let text = draw_text(&mut app);
        let body_idx = text.find("Claude Rust will wait here").expect("body text");
        let action_idx = text.find("> Yes").expect("yes action");

        assert!(action_idx > body_idx);
    }
}
