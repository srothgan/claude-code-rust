// SPDX-License-Identifier: Apache-2.0

use crate::app::{App, UpdatePromptAction};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Margin};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use super::theme;

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
        Line::from(format!("Install method:  {}", prompt.install_method.label())),
    ];
    let error_height = prompt.last_error.as_deref().map_or(0, |error| {
        wrapped_line_count(Text::from(Line::from(error.to_owned())), inner.width)
    });
    let (actions, selected_line) = action_lines(app);
    let selected_start = actions[..selected_line].iter().fold(0_u16, |height, line| {
        height.saturating_add(wrapped_line_count(Text::from(line.clone()), inner.width))
    });
    let selected_height =
        wrapped_line_count(Text::from(actions[selected_line].clone()), inner.width);
    let help = "Enter select | Esc skip now | Ctrl+Q quit";

    // Keep the selected action and its controls reachable before allocating
    // the remaining space to metadata and a potentially long failure.
    let mut available = inner.height;
    let action_total = wrapped_line_count(Text::from(actions.clone()), inner.width);
    let mut action_height = selected_height.min(available);
    available -= action_height;
    let help_height = wrapped_line_count(Text::from(help), inner.width).min(available);
    available -= help_height;
    let minimum_error = error_height.min(1).min(available);
    available -= minimum_error;
    let detail_height = wrapped_line_count(Text::from(detail.clone()), inner.width).min(available);
    available -= detail_height;
    let title_height = wrapped_line_count(Text::from(title.clone()), inner.width).min(available);
    available -= title_height;
    let extra_actions = action_total.saturating_sub(action_height).min(available);
    action_height += extra_actions;
    available -= extra_actions;
    let error_height = minimum_error + error_height.saturating_sub(minimum_error).min(available);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(title_height),
            Constraint::Length(detail_height),
            Constraint::Length(error_height),
            Constraint::Length(action_height),
            Constraint::Min(0),
            Constraint::Length(help_height),
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

    let scroll = selected_start
        .saturating_add(selected_height.min(action_height))
        .saturating_sub(action_height);
    frame.render_widget(
        Paragraph::new(actions).wrap(Wrap { trim: false }).scroll((scroll, 0)),
        chunks[3],
    );
    frame.render_widget(
        Paragraph::new(help).wrap(Wrap { trim: false }).style(Style::default().fg(theme::DIM)),
        chunks[5],
    );
}

fn action_lines(app: &App) -> (Vec<Line<'static>>, usize) {
    let Some(prompt) = app.update_prompt.as_ref() else {
        return (Vec::new(), 0);
    };
    let mut lines = Vec::new();
    let mut selected_line = 0;
    for (action, label) in
        crate::app::update_prompt_actions(&prompt.install_method).iter().copied().map(|action| {
            let label = match action {
                UpdatePromptAction::Install => "Install update".to_owned(),
                UpdatePromptAction::InstallScript => "Install update with script".to_owned(),
                UpdatePromptAction::InstallNpm => "Install update with npm".to_owned(),
                UpdatePromptAction::SkipNow => "Skip now".to_owned(),
                UpdatePromptAction::SkipVersion => "Skip this version".to_owned(),
                UpdatePromptAction::ReleaseNotes => {
                    format!("Read release notes for v{} (on GitHub)", prompt.latest_version)
                }
            };
            (action, label)
        })
    {
        if action == UpdatePromptAction::ReleaseNotes {
            lines.push(Line::default());
        }
        if prompt.selected == action {
            selected_line = lines.len();
        }
        lines.push(action_line(&label, prompt.selected == action));
    }
    (lines, selected_line)
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

    #[test]
    fn update_fields_and_selection_follow_the_install_method_and_real_keys() {
        use crate::install_method::InstallMethod;
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        for (method, expected_method, next_label) in [
            (InstallMethod::Npm, "npm", "Skip now"),
            (InstallMethod::Script { install_dir: None }, "install script", "Skip now"),
            (InstallMethod::Unknown, "unknown", "Install update with npm"),
        ] {
            let unknown = method == InstallMethod::Unknown;
            let mut app = App::test_default();
            app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::Update);
            app.update_prompt = Some(UpdatePromptState {
                current_version: "7.2.1".into(),
                latest_version: "9.4.8".into(),
                release_url: "https://example.invalid/release-948".into(),
                install_method: method,
                selected: if unknown {
                    UpdatePromptAction::InstallScript
                } else {
                    UpdatePromptAction::Install
                },
                last_error: None,
            });
            let mut terminal = Terminal::new(TestBackend::new(80, 18)).unwrap();
            for (code, selected) in [
                (None, if unknown { "Install update with script" } else { "Install update" }),
                (Some(KeyCode::Down), next_label),
                (
                    Some(KeyCode::Up),
                    if unknown { "Install update with script" } else { "Install update" },
                ),
            ] {
                if let Some(code) = code {
                    crate::app::handle_terminal_event(
                        &mut app,
                        crossterm::event::Event::Key(KeyEvent::new(code, KeyModifiers::NONE)),
                    );
                }
                terminal.draw(|frame| render(frame, &mut app)).unwrap();
                let rows = terminal
                    .backend()
                    .buffer()
                    .content
                    .chunks(80)
                    .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect::<String>())
                    .collect::<Vec<_>>();
                for (label, value) in [
                    ("Current version:", "7.2.1"),
                    ("Latest version:", "9.4.8"),
                    ("Install method:", expected_method),
                ] {
                    assert!(
                        rows.iter().any(|row| row.contains(label) && row.contains(value)),
                        "{rows:?}"
                    );
                }
                let selected_rows =
                    rows.iter().filter(|row| row.contains("> ")).collect::<Vec<_>>();
                assert_eq!(selected_rows.len(), 1, "selection must mark one action: {rows:?}");
                assert!(selected_rows[0].contains(selected), "{rows:?}");
                assert!(
                    rows.iter().any(|row| row.contains("release notes") && row.contains("9.4.8"))
                );
                if unknown {
                    for label in ["Install update with script", "Install update with npm"] {
                        assert!(rows.iter().any(|row| row.contains(label)));
                    }
                    assert!(
                        !rows.iter().any(|row| row
                            .trim_matches([' ', '│'])
                            .trim_start_matches('>')
                            .trim()
                            == "Install update")
                    );
                }
            }
        }
    }
}
