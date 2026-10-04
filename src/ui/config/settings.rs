// SPDX-License-Identifier: Apache-2.0
use super::theme;
use crate::agent::settings::{SettingsApplication, SettingsScope};
use crate::app::App;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Cell, Paragraph, Row, Table},
};

pub(super) fn help_text(app: &App, width: u16) -> String {
    use crate::app::config::SettingsFocus;
    let mut hints = match app.config.settings.focus {
        SettingsFocus::CategoryTabs => {
            vec!["Left/Right pane", "Down search", "Enter list", "Tab/Shift+Tab tabs"]
        }
        SettingsFocus::Search => {
            vec!["Type to search this pane", "Up panes", "Down/Enter list", "Tab/Shift+Tab tabs"]
        }
        SettingsFocus::Content => {
            if let Some(setting) = app.config.selected_setting() {
                let writable = setting.writable_at(app.config.selected_scope);
                let mut hints = vec![
                    if !writable {
                        if setting.kind.is_structured() { "Enter inspect" } else { "Read-only" }
                    } else if setting.allows_custom {
                        "Space edit"
                    } else {
                        "Space change"
                    },
                    "Up/Down select",
                    "/ search",
                    "s scope",
                    "r refresh",
                    "Tab/Shift+Tab tabs",
                ];
                if writable {
                    hints.push("Del reset");
                }
                if !setting.allows_custom && writable {
                    hints.push("Left/Right change");
                }
                if !app.config.settings.query.is_empty() {
                    hints.push("Enter reveal");
                } else if setting.kind.is_structured() && writable {
                    hints.push("Enter edit");
                } else if !setting.kind.is_structured() {
                    hints.push("Enter close");
                }
                hints
            } else {
                vec!["/ search", "r refresh", "Tab/Shift+Tab tabs"]
            }
        }
    };
    let escape = if !app.config.settings.query.is_empty() {
        "Esc clear search"
    } else if app.config.settings.focus == SettingsFocus::Search {
        "Esc back"
    } else {
        "Esc close"
    };
    if let Some(index) = hints.iter().position(|hint| *hint == "Tab/Shift+Tab tabs") {
        let tabs = hints.remove(index);
        hints.insert(1.min(hints.len()), tabs);
    }
    super::common::hint_text_with_escape(hints, width, escape)
}
use std::fmt::Write as _;

pub(super) fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    let [scope_area, body] = render_navigation(frame, area, app);
    let Some(snapshot) = &app.config.snapshot else {
        frame.render_widget(
            Paragraph::new(format!("Save in: {}", app.config.selected_scope.label())),
            scope_area,
        );
        let loading = app.config.pending_settings_request.is_some()
            || app.status == crate::app::AppStatus::Connecting;
        super::common::render_message(
            frame,
            body,
            if loading { "Loading settings" } else { "Settings unavailable" },
            if loading {
                "Waiting for the settings snapshot."
            } else {
                "Press r to refresh settings."
            },
        );
        return;
    };
    let items = app.config.settings.items(snapshot);
    let selected = app.config.settings.selected_index(snapshot);
    let mut scope = Line::from(vec![
        Span::styled("Save in: ", Style::default().fg(theme::DIM)),
        Span::styled(
            if scope_area.width < 35 {
                app.config.selected_scope.label()
            } else {
                scope_label(app.config.selected_scope)
            },
            super::common::accent_style(),
        ),
        Span::raw("  "),
        super::common::position_counter(selected, items.len()),
    ]);
    scope.alignment = Some(ratatui::layout::Alignment::Right);
    frame.render_widget(Paragraph::new(scope), scope_area);
    let [list_area, _, details_area] = super::common::list_and_details(body);
    if items.is_empty() {
        super::common::render_message(
            frame,
            list_area,
            "No matching settings in this pane",
            "Change the search or press Esc to clear it.",
        );
        return;
    }
    render_table(frame, list_area, app, area.height >= 22);
    render_details(frame, details_area, app);
}

fn render_navigation(frame: &mut Frame, area: Rect, app: &App) -> [Rect; 2] {
    use crate::app::config::SettingsFocus;
    let cursor =
        (app.config.settings.focus == SettingsFocus::Search).then_some(app.config.settings.cursor);
    let search_height = super::input::search_height(&app.config.settings.query, cursor, area.width)
        .min(area.height.saturating_sub(3))
        .max(2);
    let [tabs, search_area, scope_area, body] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(search_height),
        Constraint::Length(1),
        Constraint::Min(1),
    ])
    .areas(area);
    let categories = app.config.snapshot.as_ref().map(|snapshot| &snapshot.categories);
    let labels = categories
        .map(|categories| {
            categories
                .iter()
                .map(|category| {
                    if area.width < 100 {
                        category.short_label.clone()
                    } else {
                        category.label.clone()
                    }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let active = categories
        .and_then(|categories| {
            categories.iter().position(|category| category.id == app.config.settings.category)
        })
        .unwrap_or(0);
    let mut category_line = super::common::tab_line(
        &labels,
        active,
        tabs.width.saturating_sub(2),
        super::common::TabStyle::Secondary,
    );
    category_line.spans.insert(
        0,
        super::common::marker_span(app.config.settings.focus == SettingsFocus::CategoryTabs),
    );
    frame.render_widget(Paragraph::new(category_line), tabs);
    super::input::render_search_field(frame, search_area, &app.config.settings.query, cursor);
    [scope_area, body]
}

fn render_table(frame: &mut Frame, list_area: Rect, app: &mut App, spacious: bool) {
    let Some(snapshot) = &app.config.snapshot else {
        return;
    };
    let items = app.config.settings.items(snapshot);
    let selected = app.config.settings.selected_index(snapshot);
    let row_height = if spacious { 2 } else { 1 };
    let viewport = (usize::from(list_area.height) / usize::from(row_height)).max(1);
    let position = app.config.settings.position_mut();
    if selected < position.scroll {
        position.scroll = selected;
    }
    if selected >= position.scroll + viewport {
        position.scroll = selected + 1 - viewport;
    }
    let offset = position.scroll;
    let rows =
        table_rows(app, &items, selected, row_height).into_iter().skip(offset).collect::<Vec<_>>();
    app.config.settings.visible_count = viewport.min(items.len().saturating_sub(offset)).max(1);
    let table =
        Table::new(rows, [Constraint::Length(1), Constraint::Percentage(55), Constraint::Min(8)])
            .column_spacing(if list_area.width < 50 { 1 } else { 2 });
    frame.render_widget(table, list_area);
}

fn table_rows<'a>(
    app: &App,
    items: &[&'a crate::agent::settings::SettingDescriptor],
    selected: usize,
    row_height: u16,
) -> Vec<Row<'a>> {
    use crate::app::config::SettingsFocus;
    let Some(snapshot) = &app.config.snapshot else {
        return vec![];
    };
    let mut rows = Vec::new();
    for (index, setting) in items.iter().enumerate() {
        let focused = index == selected && app.config.settings.focus == SettingsFocus::Content;
        let value = if setting.kind.is_structured() {
            snapshot
                .scoped(&setting.id, app.config.selected_scope)
                .and_then(|value| value.value.as_ref())
        } else {
            snapshot.value(&setting.id)
        };
        let writable = setting.writable_at(app.config.selected_scope);
        rows.push(
            Row::new(vec![
                Cell::from(super::common::marker_span(focused)),
                Cell::from(setting.label.clone()).style(Style::default().fg(if writable {
                    ratatui::style::Color::White
                } else {
                    theme::DIM
                })),
                Cell::from(value.map_or_else(
                    || {
                        if setting.kind.is_structured() {
                            "Not set here".into()
                        } else {
                            "Default".into()
                        }
                    },
                    display,
                ))
                .style(
                    Style::default()
                        .fg(if value.is_some() && writable {
                            theme::BTW_ACCENT
                        } else {
                            theme::DIM
                        })
                        .add_modifier(if value.is_some() {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        }),
                ),
            ])
            .style(super::common::selection_style(focused))
            .height(row_height),
        );
    }
    rows
}

fn render_details(frame: &mut Frame, area: Rect, app: &App) {
    let Some(snapshot) = &app.config.snapshot else {
        return;
    };
    let Some(setting) = app.config.selected_setting() else {
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
    if let Some(line) = configured_line(app, setting) {
        lines.push(line);
    }
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

fn configured_line(
    app: &App,
    setting: &crate::agent::settings::SettingDescriptor,
) -> Option<Line<'static>> {
    let snapshot = app.config.snapshot.as_ref()?;
    if let Some(value) = snapshot.values.iter().find(|value| value.id == setting.id) {
        let mut configured = format!(
            "Configured: {}",
            value.value.as_ref().map_or_else(|| "Default".into(), display)
        );
        if !value.contributors.is_empty() {
            let _ = write!(configured, "  ·  Sources: {}", value.contributors.join(", "));
        }
        let running = match setting.id.as_str() {
            "model" => app
                .session_runtime
                .current_model
                .as_ref()
                .filter(|model| model.is_authoritative)
                .map(|model| model.display_name_short.clone()),
            "defaultEffort" => app.session_runtime.config_options.get("effortLevel").map(display),
            "alwaysThinkingEnabled" | "agent" => {
                app.session_runtime.config_options.get(&setting.id).map(display)
            }
            _ => None,
        };
        if let Some(running) = running {
            let _ = write!(configured, "  ·  Current session: {running}");
        }
        return Some(Line::styled(configured, Style::default().fg(theme::DIM)));
    }
    None
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
