// SPDX-License-Identifier: Apache-2.0
use super::input::render_text_input_field;
use super::overlay::{OverlayChrome, OverlayLayoutSpec, render_overlay_shell};
use super::theme;
use crate::agent::settings::{SettingDescriptor, SettingsApplication};
use crate::app::App;
use crate::app::config::SettingOverlayState;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Margin, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};

pub(super) fn render_setting_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let Some(overlay) = app.config.setting_overlay() else {
        return;
    };
    let Some(setting) = app
        .config
        .snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.catalog.iter().find(|setting| setting.id == overlay.id))
    else {
        return;
    };
    let has_options = !setting.options.is_empty();
    let free_text = setting.allows_custom;
    let rendered = render_overlay_shell(
        frame,
        area,
        OverlayLayoutSpec {
            min_width: 56,
            min_height: if setting.options.len() > 3 { 18 } else { 14 },
            width_percent: 80,
            height_percent: 55,
            preferred_height: if setting.options.len() > 3 { 18 } else { 14 },
            fullscreen_below: Some((56, 14)),
            inner_margin: Margin { vertical: 1, horizontal: 2 },
        },
        OverlayChrome {
            title: &setting.label,
            subtitle: Some(super::settings::scope_label(overlay.scope)),
            help: Some(if app.config.pending_settings_request.is_some() {
                "Saving..."
            } else if has_options {
                "Up/Down options | Enter save | Ctrl+R reset | Esc cancel"
            } else {
                "Enter save | Ctrl+R reset | Esc cancel"
            }),
            message: app.config.overlay_message.as_ref(),
        },
    );
    if !free_text && setting.options.len() > 3 {
        render_choice_picker(frame, rendered.body_area, app, setting, overlay);
        return;
    }
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1), Constraint::Min(1)])
        .split(rendered.body_area);
    render_value(frame, sections[0], setting, overlay);
    frame.render_widget(
        Paragraph::new(setting_details(app, setting, overlay)).wrap(Wrap { trim: false }),
        sections[2],
    );
}

fn render_value(
    frame: &mut Frame,
    area: Rect,
    setting: &SettingDescriptor,
    overlay: &SettingOverlayState,
) {
    if setting.allows_custom {
        render_text_input_field(frame, area, &overlay.draft, overlay.cursor, "Enter a value");
    } else {
        let choice = setting.options.iter().find(|value| {
            value.as_str().map_or_else(|| value.to_string(), str::to_owned) == overlay.draft
        });
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("Value: ", Style::default().fg(theme::DIM)),
                Span::styled(
                    choice.map_or_else(
                        || {
                            if overlay.draft.is_empty() {
                                "Default".to_owned()
                            } else {
                                overlay.draft.clone()
                            }
                        },
                        super::settings::display,
                    ),
                    Style::default().fg(theme::BTW_ACCENT).add_modifier(Modifier::BOLD),
                ),
            ])),
            area,
        );
    }
}

fn render_choice_picker(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    setting: &SettingDescriptor,
    overlay: &SettingOverlayState,
) {
    let details = Paragraph::new(setting_details(app, setting, overlay)).wrap(Wrap { trim: false });
    let detail_height = u16::try_from(details.line_count(area.width.max(1)))
        .unwrap_or(u16::MAX)
        .min(area.height / 2);
    let sections = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(2),
        Constraint::Length(1),
        Constraint::Length(detail_height),
    ])
    .split(area);
    render_value(frame, sections[0], setting, overlay);
    let visible = usize::from(sections[2].height.saturating_sub(1)).max(1);
    let selected = setting.options.iter().position(|value| {
        value.as_str().map_or_else(|| value.to_string(), str::to_owned) == overlay.draft
    });
    let offset = selected
        .unwrap_or(0)
        .saturating_sub(visible / 2)
        .min(setting.options.len().saturating_sub(visible));
    let mut lines = vec![Line::styled(
        format!(
            "{}{}{}",
            if setting.id == "model" { "Models" } else { "Options" },
            if offset > 0 { "  ↑" } else { "" },
            if offset + visible < setting.options.len() { "  ↓" } else { "" },
        ),
        Style::default().fg(theme::DIM),
    )];
    for (index, value) in setting.options.iter().enumerate().skip(offset).take(visible) {
        let active = selected == Some(index);
        lines.push(Line::from(vec![
            Span::styled(if active { "› " } else { "  " }, Style::default().fg(theme::RUST_ORANGE)),
            Span::styled(
                super::settings::display(value),
                if active {
                    Style::default()
                        .fg(theme::BTW_ACCENT)
                        .bg(theme::USER_MSG_BG)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme::DIM)
                },
            ),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), sections[2]);
    frame.render_widget(details, sections[4]);
}

fn setting_details(
    app: &App,
    setting: &SettingDescriptor,
    overlay: &SettingOverlayState,
) -> Vec<Line<'static>> {
    let current = app
        .config
        .snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.scoped(&overlay.id, overlay.scope))
        .and_then(|value| value.value.as_ref())
        .map_or_else(|| "not set".to_owned(), super::settings::display);
    let mut lines = vec![Line::from(setting.description.clone()), Line::from("")];
    if !setting.options.is_empty() && setting.options.len() <= 3 {
        let mut choices = vec![Span::styled("Options: ", Style::default().fg(theme::DIM))];
        for value in &setting.options {
            let selected =
                value.as_str().map_or_else(|| value.to_string(), str::to_owned) == overlay.draft;
            choices.push(Span::styled(
                format!(" {} ", super::settings::display(value)),
                if selected {
                    Style::default()
                        .fg(theme::BTW_ACCENT)
                        .bg(theme::USER_MSG_BG)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme::DIM)
                },
            ));
        }
        lines.push(Line::from(choices));
        lines.push(Line::from(""));
    }
    lines.push(Line::styled(
        format!("Saved in {}: {current}", overlay.scope.label()),
        Style::default().fg(theme::DIM),
    ));
    lines.push(Line::styled(setting.reset.clone(), Style::default().fg(theme::DIM)));
    if setting.application == SettingsApplication::Host {
        lines.push(Line::styled("Applies immediately.", Style::default().fg(theme::DIM)));
    }
    lines
}

pub(super) fn render_session_rename_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let Some(overlay) = app.config.session_rename_overlay() else {
        return;
    };
    let rendered = render_overlay_shell(
        frame,
        area,
        OverlayLayoutSpec {
            min_width: 56,
            min_height: 8,
            width_percent: 72,
            height_percent: 48,
            preferred_height: 10,
            fullscreen_below: Some((56, 14)),
            inner_margin: Margin { vertical: 1, horizontal: 2 },
        },
        OverlayChrome {
            title: "Rename session",
            subtitle: Some("Set a custom title for the current session"),
            help: Some("Enter confirm | Esc cancel"),
            message: app.config.overlay_message.as_ref(),
        },
    );
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(rendered.body_area);

    render_text_input_field(
        frame,
        sections[0],
        &overlay.draft,
        overlay.cursor,
        "Custom session name",
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "Leave the field empty to clear the custom session name.",
            Style::default().fg(theme::DIM),
        ))),
        sections[1],
    );
}
