// SPDX-License-Identifier: Apache-2.0
use super::theme;
use crate::agent::settings::{SettingsApplication, SettingsScope};
use crate::app::App;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Cell, Paragraph, Row, Table, Wrap},
};
use std::fmt::Write as _;

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
                source_label(app.config.selected_scope.label())
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
    let details = details_lines(app);
    let details_height = super::common::wrapped_height(details.clone(), body.width)
        .min(body.height.saturating_sub(4));
    let [list_area, _, details_area] = Layout::vertical([
        Constraint::Min(3),
        Constraint::Length(u16::from(details_height > 0)),
        Constraint::Length(details_height),
    ])
    .areas(body);
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
    render_details(frame, details_area, details);
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

fn details_lines(app: &App) -> Vec<Line<'static>> {
    let Some(snapshot) = &app.config.snapshot else {
        return vec![];
    };
    let Some(setting) = app.config.selected_setting() else {
        return vec![];
    };
    let saved = snapshot
        .scoped(&setting.id, app.config.selected_scope)
        .and_then(|value| value.value.as_ref());
    let configured = snapshot.value(&setting.id);
    let resolved = snapshot.values.iter().find(|value| value.id == setting.id);
    let scope = source_label(app.config.selected_scope.label());
    let other_sources = resolved
        .into_iter()
        .flat_map(|value| &value.contributors)
        .filter(|source| source.as_str() != app.config.selected_scope.label())
        .map(|source| source_label(source))
        .collect::<Vec<_>>()
        .join(", ");
    let mut context = match (saved, configured) {
        (Some(value), Some(effective)) if !setting.kind.is_structured() && value != effective => {
            format!(
                "{scope} value: {} · Overridden by {}",
                display(value),
                if other_sources.is_empty() { "another scope" } else { &other_sources }
            )
        }
        (Some(_), None) if !setting.kind.is_structured() => {
            format!("Saved in {scope} · Not loaded")
        }
        (Some(_), _) => format!("Saved in {scope}"),
        (None, Some(_)) => format!(
            "From {} · Not set in {scope}",
            if other_sources.is_empty() { "another scope" } else { &other_sources }
        ),
        (None, None) if setting.kind.is_structured() => format!("Not set in {scope}"),
        (None, None) => "Using Default".to_owned(),
    };
    if saved.is_some()
        && !other_sources.is_empty()
        && (setting.kind.is_structured() || saved == configured)
    {
        let _ = write!(context, " · Also supplied by {other_sources}");
    }
    match setting.application {
        SettingsApplication::Host => context.push_str(" · Applies immediately"),
        SettingsApplication::NextSession => context.push_str(" · Applies to new sessions"),
        SettingsApplication::Blocked => {}
    }
    let mut lines = vec![description_line(&setting.description)];
    if let Some(error) = snapshot
        .sources
        .iter()
        .find(|source| source.scope == app.config.selected_scope)
        .and_then(|source| source.error.as_ref())
    {
        lines.push(Line::styled(error.clone(), Style::default().fg(theme::STATUS_WARNING)));
    }
    if let Some(restriction) = restriction_text(
        setting,
        app.config.selected_scope,
        resolved.is_some_and(|value| value.policy_restricted),
    ) {
        lines.push(Line::styled(restriction, Style::default().fg(theme::STATUS_WARNING)));
    }
    lines.push(Line::styled(
        context,
        Style::default().fg(
            if saved.is_some() && saved != configured && !setting.kind.is_structured() {
                theme::STATUS_WARNING
            } else {
                theme::DIM
            },
        ),
    ));
    if let Some(line) = session_difference(app, setting) {
        lines.push(line);
    }
    lines
}

fn restriction_text(
    setting: &crate::agent::settings::SettingDescriptor,
    scope: SettingsScope,
    policy_restricted: bool,
) -> Option<String> {
    if policy_restricted {
        Some("Controlled by your organization".to_owned())
    } else if let Some(reason) = &setting.unavailable {
        Some(reason.clone())
    } else if !setting.writable_at(scope) {
        Some(if setting.writable_scopes.is_empty() {
            "Read-only".to_owned()
        } else {
            let scopes = setting
                .writable_scopes
                .iter()
                .map(|scope| source_label(scope.label()))
                .collect::<Vec<_>>()
                .join(", ");
            format!("Can be saved in: {scopes}")
        })
    } else {
        None
    }
}

pub(super) fn description_line(description: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled("Description: ", super::common::title_style()),
        Span::raw(description.to_owned()),
    ])
}

fn render_details(frame: &mut Frame, area: Rect, mut lines: Vec<Line<'static>>) {
    if area.height == 0 || lines.is_empty() {
        return;
    }
    let description = lines.remove(0);
    let metadata_height = super::common::wrapped_height(lines.clone(), area.width);
    let description_height = super::common::wrapped_height(description.clone(), area.width)
        .min(area.height.saturating_sub(metadata_height));
    let [description_area, metadata_area] =
        Layout::vertical([Constraint::Length(description_height), Constraint::Min(0)]).areas(area);
    frame.render_widget(Paragraph::new(description).wrap(Wrap { trim: false }), description_area);
    super::common::render_details(frame, metadata_area, lines);
}

fn session_difference(
    app: &App,
    setting: &crate::agent::settings::SettingDescriptor,
) -> Option<Line<'static>> {
    let snapshot = app.config.snapshot.as_ref()?;
    let configured = snapshot.value(&setting.id)?;
    let running = match setting.id.as_str() {
        "model" => {
            let model =
                app.session_runtime.current_model.as_ref().filter(|m| m.is_authoritative)?;
            let id = configured.as_str()?;
            if model.catalog_id.as_deref() == Some(id)
                || model.resolved_id == id
                || model.display_name_short == id
            {
                return None;
            }
            model.display_name_short.clone()
        }
        "defaultEffort" => display(app.session_runtime.config_options.get("effortLevel")?),
        "alwaysThinkingEnabled" | "agent" => {
            display(app.session_runtime.config_options.get(&setting.id)?)
        }
        _ => return None,
    };
    (running != display(configured)).then(|| {
        Line::styled(format!("Current session: {running}"), Style::default().fg(theme::DIM))
    })
}

fn source_label(source: &str) -> &str {
    match source {
        "user" => "User",
        "project" => "Project",
        "local" => "Local",
        "managed" => "Organization",
        _ => source,
    }
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
