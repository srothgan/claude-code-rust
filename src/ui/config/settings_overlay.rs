// SPDX-License-Identifier: Apache-2.0
use super::input::{render_multiline_input_field, render_text_input_field};
use super::overlay::{OverlayChrome, OverlayLayoutSpec, render_overlay_shell};
use super::theme;
use crate::agent::settings::{SettingDescriptor, SettingsApplication};
use crate::app::App;
use crate::app::config::SettingOverlayState;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Margin, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};

pub(super) fn render_setting_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let Some(overlay) = app.config.setting_overlay() else {
        return;
    };
    let setting = &overlay.setting;
    if overlay.structured.as_ref().is_some_and(|form| !form.advanced) {
        super::settings_form::render(frame, area, app, overlay);
        return;
    }
    let multiline = setting.kind.is_structured();
    let rendered = render_overlay_shell(
        frame,
        area,
        OverlayLayoutSpec {
            min_width: 56,
            min_height: 14,
            width_percent: 80,
            height_percent: if multiline { 85 } else { 55 },
            preferred_height: if multiline { 32 } else { 14 },
            fullscreen_below: Some((56, 14)),
            inner_margin: Margin { vertical: 1, horizontal: 2 },
        },
        OverlayChrome {
            title: &setting.label,
            subtitle: Some(super::settings::scope_label(overlay.scope)),
            help: Some(if app.config.pending_settings_request.is_some() {
                "Saving..."
            } else if overlay.structured.as_ref().is_some_and(|form| form.read_only) {
                "Read-only | Ctrl+F form | Esc back"
            } else if overlay.structured.is_some() {
                "Ctrl+S save | Ctrl+F form | Enter new line | Esc cancel"
            } else if multiline {
                "Ctrl+S save | Enter new line | Ctrl+U clear | Ctrl+R reset | Esc cancel"
            } else {
                "Enter save | Ctrl+R reset | Esc cancel"
            }),
            message: app.config.overlay_message.as_ref(),
        },
    );
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints(if multiline {
            [Constraint::Min(3), Constraint::Length(1), Constraint::Length(5)]
        } else {
            [Constraint::Length(1), Constraint::Length(1), Constraint::Min(1)]
        })
        .split(rendered.body_area);
    if multiline {
        render_multiline_input_field(frame, sections[0], &overlay.draft, overlay.cursor);
    } else {
        render_text_input_field(
            frame,
            sections[0],
            &overlay.draft,
            overlay.cursor,
            "Enter a value",
        );
    }
    frame.render_widget(
        Paragraph::new(setting_details(app, setting, overlay)).wrap(Wrap { trim: false }),
        sections[2],
    );
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
        .and_then(|snapshot| snapshot.scoped(&overlay.setting.id, overlay.scope))
        .and_then(|value| value.value.as_ref())
        .map_or_else(|| "not set".to_owned(), super::settings::display);
    let mut lines = vec![super::settings::description_line(&setting.description), Line::from("")];
    if stale_context(app, overlay) {
        lines.push(Line::styled(
            "Settings location changed. Cancel and reopen to save.",
            Style::default().fg(theme::STATUS_WARNING),
        ));
        return lines;
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

fn stale_context(app: &App, overlay: &SettingOverlayState) -> bool {
    app.config
        .snapshot
        .as_ref()
        .is_none_or(|snapshot| !snapshot.matches_context(&overlay.context, &app.cwd_raw))
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
