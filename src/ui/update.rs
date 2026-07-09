// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use crate::app::{App, UpdatePromptAction};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Margin};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use super::theme;

const INSTALL_COMMAND: &str = "npm install -g claude-code-rust";

pub fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let outer = Block::default()
        .borders(Borders::ALL)
        .title("Update Available!")
        .border_style(Style::default().fg(theme::DIM));
    frame.render_widget(outer, area);

    let inner = area.inner(Margin { vertical: 1, horizontal: 2 });
    let Some(prompt) = app.update_prompt.as_ref() else {
        frame.render_widget(Paragraph::new("No update prompt is active."), inner);
        return;
    };

    let title = Line::from(Span::styled(
        format!("Claude Rust v{} is available", prompt.latest_version),
        Style::default().fg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD),
    ));
    let detail = vec![
        Line::from(format!("Current version: v{}", prompt.current_version)),
        Line::from(format!("Latest version:  v{}", prompt.latest_version)),
        Line::from(format!("Install command: {INSTALL_COMMAND}")),
    ];
    let error_height = prompt.last_error.as_deref().map_or(0, |error| {
        wrapped_line_count(Text::from(Line::from(error.to_owned())), inner.width)
    });
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(4),
            Constraint::Length(error_height),
            Constraint::Min(4),
            Constraint::Length(1),
        ])
        .split(inner);

    frame.render_widget(Paragraph::new(title).wrap(Wrap { trim: false }), chunks[0]);
    frame.render_widget(Paragraph::new(detail).wrap(Wrap { trim: false }), chunks[1]);

    if let Some(error) = prompt.last_error.as_deref() {
        frame.render_widget(
            Paragraph::new(error.to_owned())
                .style(Style::default().fg(theme::STATUS_ERROR))
                .wrap(Wrap { trim: false }),
            chunks[2],
        );
    }

    frame.render_widget(Paragraph::new(action_lines(app)).wrap(Wrap { trim: false }), chunks[3]);
    frame.render_widget(
        Paragraph::new("Enter select | Esc skip now | Ctrl+Q quit")
            .style(Style::default().fg(theme::DIM)),
        chunks[4],
    );
}

fn action_lines(app: &App) -> Vec<Line<'static>> {
    let Some(prompt) = app.update_prompt.as_ref() else {
        return Vec::new();
    };
    [
        (UpdatePromptAction::Install, "Install update".to_owned()),
        (UpdatePromptAction::SkipNow, "Skip now".to_owned()),
        (UpdatePromptAction::SkipVersion, "Skip this version".to_owned()),
        (
            UpdatePromptAction::ReleaseNotes,
            format!("Read release notes for v{} (on GitHub)", prompt.latest_version),
        ),
    ]
    .into_iter()
    .flat_map(|(action, label)| {
        let mut lines = Vec::with_capacity(2);
        if action == UpdatePromptAction::ReleaseNotes {
            lines.push(Line::default());
        }
        lines.push(action_line(&label, prompt.selected == action));
        lines
    })
    .collect()
}

fn action_line(label: &str, selected: bool) -> Line<'static> {
    let marker = if selected { ">" } else { " " };
    let style = if selected {
        Style::default().fg(theme::RUST_ORANGE)
    } else {
        Style::default().fg(theme::DIM)
    }
    .add_modifier(Modifier::BOLD);

    Line::from(Span::styled(format!("{marker} {label}"), style))
}

fn wrapped_line_count(text: Text<'static>, width: u16) -> u16 {
    u16::try_from(Paragraph::new(text).wrap(Wrap { trim: false }).line_count(width))
        .unwrap_or(u16::MAX)
        .max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, FullscreenView, SurfaceMode, UpdatePromptState};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn draw_text(app: &mut App) -> String {
        let backend = TestBackend::new(80, 14);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal.draw(|frame| render(frame, app)).expect("draw");
        terminal
            .backend()
            .buffer()
            .content
            .chunks(80)
            .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn update_view_renders_versions_command_and_release_notes() {
        let mut app = App::test_default();
        app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::Update);
        app.update_prompt = Some(UpdatePromptState {
            current_version: "0.13.4".to_owned(),
            latest_version: "0.14.0".to_owned(),
            release_url: "https://example.invalid".to_owned(),
            selected: UpdatePromptAction::Install,
            last_error: None,
        });

        let text = draw_text(&mut app);

        assert!(text.contains("Update Available!"));
        assert!(text.contains("Current version: v0.13.4"));
        assert!(text.contains("Latest version:  v0.14.0"));
        assert!(text.contains(INSTALL_COMMAND));
        assert!(text.contains("Read release notes for v0.14.0 (on GitHub)"));
    }
}
