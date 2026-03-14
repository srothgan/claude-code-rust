use super::theme;
use ratatui::Frame;
use ratatui::layout::{Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

pub(super) fn render(frame: &mut Frame, area: Rect) {
    let body = area.inner(Margin { vertical: 1, horizontal: 2 });
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                "Skills placeholder",
                Style::default().fg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD),
            )),
            Line::default(),
            Line::from(Span::styled(
                "This tab is wired into Config and will host the skills and plugins workflow.",
                Style::default().fg(Color::White),
            )),
            Line::from(Span::styled(
                "Phase 1 currently stops at tab wiring, key navigation, and the /skills command.",
                Style::default().fg(theme::DIM),
            )),
        ])
        .wrap(Wrap { trim: false }),
        body,
    );
}
