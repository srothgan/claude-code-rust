// SPDX-License-Identifier: Apache-2.0
use super::theme;
use crate::agent::settings::{SettingsApplication, SettingsScope};
use crate::app::App;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Cell, Paragraph, Row, Table},
};

pub(super) fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    let Some(snapshot) = &app.config.snapshot else {
        let loading = app.config.pending_settings_request.is_some()
            || app.status == crate::app::AppStatus::Connecting;
        super::common::render_message(
            frame,
            area,
            if loading { "Loading settings" } else { "Settings unavailable" },
            if loading {
                "Waiting for the settings snapshot."
            } else {
                "Press r to refresh settings."
            },
        );
        return;
    };
    let [list_area, _, details_area] = super::common::list_and_details(area);
    let panels = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(if area.width < 70 { 2 } else { 1 }),
            Constraint::Length(u16::from(details_area.height > 0)),
            Constraint::Min(3),
        ])
        .split(list_area);
    let headers = if area.width < 70 {
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).split(panels[0])
    } else {
        Layout::horizontal([Constraint::Min(25), Constraint::Length(8)]).split(panels[0])
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Save in: ", Style::default().fg(theme::DIM)),
            Span::styled(
                if headers[0].width < 40 {
                    app.config.selected_scope.label()
                } else {
                    scope_label(app.config.selected_scope)
                },
                super::common::accent_style(),
            ),
        ])),
        headers[0],
    );

    let row_height = if area.height >= 22 { 2 } else { 1 };
    let visible = usize::from(panels[2].height.saturating_sub(2) / row_height).max(1);
    let selected = app.config.selected_setting_index;
    if selected < app.config.settings_scroll_offset {
        app.config.settings_scroll_offset = selected;
    }
    if selected >= app.config.settings_scroll_offset.saturating_add(visible) {
        app.config.settings_scroll_offset = selected.saturating_add(1).saturating_sub(visible);
    }
    frame.render_widget(
        Paragraph::new(super::common::position_counter(selected, snapshot.catalog.len()))
            .alignment(ratatui::layout::Alignment::Right),
        headers[1],
    );
    render_table(frame, panels[2], app, row_height, visible);
    render_details(frame, details_area, app);
}

fn render_table(frame: &mut Frame, area: Rect, app: &App, row_height: u16, visible: usize) {
    let Some(snapshot) = &app.config.snapshot else {
        return;
    };
    let offset = app.config.settings_scroll_offset;
    let selected = app.config.selected_setting_index;
    let rows =
        snapshot.catalog.iter().enumerate().skip(offset).take(visible).map(|(index, setting)| {
            let selected = index == selected;
            let value = if setting.kind.is_structured() {
                snapshot
                    .scoped(&setting.id, app.config.selected_scope)
                    .and_then(|value| value.value.as_ref())
            } else {
                snapshot.value(&setting.id)
            };
            let label_style =
                Style::default().fg(if setting.writable_at(app.config.selected_scope) {
                    ratatui::style::Color::White
                } else {
                    theme::DIM
                });
            let value_style = Style::default()
                .fg(if value.is_some() && setting.writable_at(app.config.selected_scope) {
                    theme::BTW_ACCENT
                } else {
                    theme::DIM
                })
                .add_modifier(if value.is_some() { Modifier::BOLD } else { Modifier::empty() });
            Row::new(vec![
                Cell::from(super::common::marker_span(selected)),
                Cell::from(setting.label.clone()).style(label_style),
                Cell::from(value.map_or_else(|| "Default".to_owned(), display)).style(value_style),
            ])
            .style(super::common::selection_style(selected))
            .height(row_height)
        });
    frame.render_widget(
        Table::new(rows, [Constraint::Length(1), Constraint::Percentage(55), Constraint::Min(12)])
            .column_spacing(2)
            .header(
                Row::new(["", "Setting", "Value"])
                    .style(Style::default().fg(theme::DIM))
                    .bottom_margin(1),
            ),
        area,
    );
}

fn render_details(frame: &mut Frame, area: Rect, app: &App) {
    let Some(snapshot) = &app.config.snapshot else {
        return;
    };
    let Some(setting) = snapshot.catalog.get(app.config.selected_setting_index) else {
        return;
    };
    let saved = snapshot
        .scoped(&setting.id, app.config.selected_scope)
        .and_then(|value| value.value.as_ref());
    let mut context = format!(
        "Saved in {}: {}",
        app.config.selected_scope.label(),
        saved.map_or_else(|| "not set".to_owned(), display)
    );
    if snapshot.values.iter().any(|value| value.id == setting.id && value.policy_restricted) {
        context.push_str("  ·  Controlled by your organization");
    } else if let Some(reason) = &setting.unavailable {
        context.push_str("  ·  ");
        context.push_str(reason);
    } else if !setting.writable_at(app.config.selected_scope) {
        context.push_str("  ·  Save in: ");
        context.push_str(
            &setting
                .writable_scopes
                .iter()
                .map(|scope| scope.label())
                .collect::<Vec<_>>()
                .join(", "),
        );
    } else if saved.is_some()
        && snapshot.value(&setting.id).is_some()
        && saved != snapshot.value(&setting.id)
    {
        context.push_str(if setting.kind.is_structured() {
            "  ·  Other scopes also supply values"
        } else {
            "  ·  Another scope overrides this value"
        });
    } else if saved.is_none() && snapshot.value(&setting.id).is_some() {
        context.push_str("  ·  Using another scope's value");
    }
    if setting.application == SettingsApplication::Host {
        context.push_str("  ·  Applies immediately");
    }
    let mut lines = vec![
        Line::from(setting.description.clone()),
        Line::styled(context, Style::default().fg(theme::DIM)),
    ];
    if let Some(error) = snapshot
        .sources
        .iter()
        .find(|source| source.scope == app.config.selected_scope)
        .and_then(|source| source.error.as_ref())
    {
        lines.push(Line::styled(error.clone(), Style::default().fg(theme::STATUS_WARNING)));
    }
    lines.push(Line::styled(
        "Saved changes apply to new sessions unless marked immediate.",
        Style::default().fg(theme::DIM),
    ));
    super::common::render_details(frame, area, lines);
}

pub(super) fn display(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Bool(true) => "On".to_owned(),
        serde_json::Value::Bool(false) => "Off".to_owned(),
        serde_json::Value::Array(items) => {
            format!("{} {}", items.len(), if items.len() == 1 { "item" } else { "items" })
        }
        serde_json::Value::Object(entries) => {
            format!("{} {}", entries.len(), if entries.len() == 1 { "entry" } else { "entries" })
        }
        _ => value.as_str().map_or_else(|| value.to_string(), str::to_owned),
    }
}

pub(super) const fn scope_label(scope: SettingsScope) -> &'static str {
    match scope {
        SettingsScope::User => "User (all projects)",
        SettingsScope::Project => "Project (shared)",
        SettingsScope::Local => "Local (this project, private)",
    }
}
