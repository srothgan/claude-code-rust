use super::theme;
use ratatui::Frame;
use ratatui::layout::Margin;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

pub(super) fn render(frame: &mut Frame, area: Rect) {
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "MCP will land here.",
            Style::default().fg(theme::DIM),
        ))),
        area.inner(Margin { vertical: 1, horizontal: 2 }),
    );
}
