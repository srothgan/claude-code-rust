// SPDX-License-Identifier: Apache-2.0
use super::{common, theme};
use crate::app::App;
use crate::app::plugins::{
    InstalledPluginEntry, PluginsViewTab, display_label, filtered_marketplace_plugins,
    ordered_installed, search_enabled, visible_marketplaces,
};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

pub(super) fn render(frame: &mut Frame, area: Rect, app: &App) {
    let top_height = top_region_height(app, area.width).min(area.height.saturating_sub(3));
    let sections = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(top_height),
        Constraint::Min(1),
    ])
    .split(area);
    let blocks = list_content(app);
    let selected = app.plugins.selected_index_for(app.plugins.active_tab);
    let labels = PluginsViewTab::ALL
        .iter()
        .map(|tab| {
            let count = match tab {
                PluginsViewTab::Installed => ordered_installed(&app.plugins, &app.cwd_raw).len(),
                PluginsViewTab::Plugins => filtered_marketplace_plugins(&app.plugins).len(),
                PluginsViewTab::Marketplace => visible_marketplaces(&app.plugins).len(),
            };
            format!("{} ({count})", tab.title())
        })
        .collect::<Vec<_>>();
    let header = Layout::horizontal([Constraint::Min(1), Constraint::Length(7)]).split(sections[0]);
    let active =
        PluginsViewTab::ALL.iter().position(|tab| *tab == app.plugins.active_tab).unwrap_or(0);
    frame.render_widget(
        Paragraph::new(common::tab_line(
            &labels,
            active,
            header[0].width,
            common::TabStyle::Secondary,
        )),
        header[0],
    );
    frame.render_widget(
        Paragraph::new(common::position_counter(selected, blocks.len()))
            .alignment(Alignment::Right),
        header[1],
    );
    render_top_region(frame, sections[2], app);
    if blocks.is_empty() {
        let loading = app.plugins.loading;
        let body = if loading {
            "Waiting for the plugin inventory."
        } else if !app.plugins.search_query_for(app.plugins.active_tab).is_empty() {
            "No plugins match the current search."
        } else {
            "No plugins found. Switch lists or press r to refresh."
        };
        common::render_message(
            frame,
            sections[3],
            if loading { "Loading plugins" } else { "No plugins" },
            body,
        );
    } else {
        common::render_selection_blocks(
            frame,
            sections[3],
            blocks,
            selected,
            !app.plugins.search_focused || !search_enabled(app.plugins.active_tab),
        );
    }
}

fn render_top_region(frame: &mut Frame, area: Rect, app: &App) {
    if search_enabled(app.plugins.active_tab) {
        frame.render_widget(
            Paragraph::new(search_field_line(app))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(if app.plugins.search_focused {
                            " Search "
                        } else {
                            " Search (Up to focus) "
                        })
                        .border_style(if app.plugins.search_focused {
                            Style::default().fg(theme::RUST_ORANGE)
                        } else {
                            Style::default().fg(theme::DIM)
                        }),
                )
                .wrap(Wrap { trim: false })
                .scroll((
                    common::selected_scroll(
                        usize::from(common::wrapped_height(
                            search_field_line(app),
                            area.width.saturating_sub(2),
                        ))
                        .saturating_sub(1),
                        1,
                        area.height.saturating_sub(2),
                    ),
                    0,
                )),
            area,
        );
        return;
    }

    frame.render_widget(
        Paragraph::new(Line::styled("Configured marketplaces", common::title_style())),
        area,
    );
}

fn top_region_height(app: &App, width: u16) -> u16 {
    if !search_enabled(app.plugins.active_tab) {
        return 1;
    }

    common::wrapped_height(search_field_line(app), width.saturating_sub(2)).saturating_add(2)
}

fn search_field_line(app: &App) -> Line<'static> {
    let cursor_style = common::cursor_style();
    let text_style = Style::default().fg(Color::White);
    let hint_style = Style::default().fg(theme::DIM);
    let query = app.plugins.search_query_for(app.plugins.active_tab);

    if query.is_empty() {
        if app.plugins.search_focused {
            return Line::from(vec![
                Span::styled(" ".to_owned(), cursor_style),
                Span::styled("Type to filter this list".to_owned(), hint_style),
            ]);
        }
        return Line::from(Span::styled("Type to filter this list", hint_style));
    }

    if app.plugins.search_focused {
        return Line::from(vec![
            Span::styled(query.to_owned(), text_style),
            Span::styled(" ".to_owned(), cursor_style),
        ]);
    }

    Line::from(Span::styled(query.to_owned(), text_style))
}

fn list_content(app: &App) -> Vec<Vec<Line<'static>>> {
    match app.plugins.active_tab {
        PluginsViewTab::Installed => installed_content(app),
        PluginsViewTab::Plugins => plugins_content(app),
        PluginsViewTab::Marketplace => marketplace_content(app),
    }
}

fn installed_content(app: &App) -> Vec<Vec<Line<'static>>> {
    ordered_installed(&app.plugins, &app.cwd_raw)
        .iter()
        .map(|entry| {
            let mut lines = vec![
                Line::from(vec![
                    Span::styled(display_label(&entry.id), common::title_style()),
                    Span::raw("  "),
                    installed_plugin_badge(entry),
                ]),
                common::detail_kv("Plugin", &entry.id, Color::White),
                common::detail_kv(
                    "Status",
                    if entry.enabled { "enabled" } else { "disabled" },
                    Color::White,
                ),
                common::detail_kv("Scope", &entry.scope, Color::White),
            ];
            if let Some(description) = app
                .plugins
                .marketplace
                .iter()
                .find(|plugin| plugin.plugin_id == entry.id)
                .and_then(|plugin| plugin.description.as_ref())
            {
                lines.push(common::detail_kv("Description", description, Color::White));
            }
            if let Some(version) = &entry.version {
                lines.push(common::detail_kv("Version", version, Color::White));
            }
            if let Some(path) = &entry.project_path {
                lines.push(common::detail_kv("Project", path, Color::White));
            }
            if let Some(installed_at) = &entry.installed_at {
                lines.push(common::detail_kv("Installed", installed_at, Color::White));
            }
            if let Some(last_updated) = &entry.last_updated {
                lines.push(common::detail_kv("Updated", last_updated, Color::White));
            }
            if !entry.mcp_server_names.is_empty() {
                lines.push(common::detail_kv(
                    "MCP servers",
                    &entry.mcp_server_names.join(", "),
                    Color::White,
                ));
            }
            lines
        })
        .collect()
}

fn plugins_content(app: &App) -> Vec<Vec<Line<'static>>> {
    filtered_marketplace_plugins(&app.plugins)
        .iter()
        .map(|entry| {
            let mut lines = vec![
                Line::styled(display_label(&entry.name), common::title_style()),
                common::detail_kv("Plugin", &entry.plugin_id, Color::White),
            ];
            if let Some(description) = &entry.description {
                lines.push(common::detail_kv("Description", description, Color::White));
            }
            if let Some(marketplace) = &entry.marketplace_name {
                lines.push(common::detail_kv("Marketplace", marketplace, Color::White));
            }
            if let Some(version) = &entry.version {
                lines.push(common::detail_kv("Version", version, Color::White));
            }
            if let Some(count) = entry.install_count {
                lines.push(common::detail_kv("Installs", &count.to_string(), Color::White));
            }
            if let Some(source) = &entry.source {
                let source = source.as_str().map_or_else(|| source.to_string(), str::to_owned);
                lines.push(common::detail_kv("Source", &source, Color::White));
            }
            lines
        })
        .collect()
}

fn marketplace_content(app: &App) -> Vec<Vec<Line<'static>>> {
    let entries = visible_marketplaces(&app.plugins);
    if entries.is_empty() && app.plugins.loading {
        return Vec::new();
    }
    let mut blocks = entries
        .iter()
        .map(|entry| {
            let mut lines = vec![
                Line::styled(display_label(&entry.name), common::title_style()),
                common::detail_kv("Marketplace", &entry.name, Color::White),
            ];
            if let Some(source) = &entry.source {
                lines.push(common::detail_kv("Source", source, Color::White));
            }
            if let Some(repo) = &entry.repo {
                lines.push(common::detail_kv("Repo", repo, Color::White));
            }
            lines
        })
        .collect::<Vec<_>>();
    blocks.push(vec![
        Line::styled("Add marketplace", common::title_style()),
        Line::from("Add a marketplace from a GitHub repo, URL, or local path."),
    ]);
    blocks
}

fn installed_plugin_badge(entry: &InstalledPluginEntry) -> Span<'static> {
    if entry.mcp_server_names.is_empty() {
        common::badge_span("SKILL", Color::White, Color::Rgb(64, 64, 64))
    } else {
        common::badge_span("MCP", Color::White, Color::Rgb(34, 92, 124))
    }
}

#[cfg(test)]
mod tests {
    use super::{search_field_line, top_region_height};
    use crate::app::App;
    use crate::app::plugins::PluginsViewTab;
    use ratatui::widgets::{Paragraph, Wrap};

    #[test]
    fn top_region_height_grows_for_wrapped_search_query() {
        let mut app = App::test_default();
        app.plugins.active_tab = PluginsViewTab::Installed;
        app.plugins.search_focused = true;
        app.plugins.installed_search_query =
            "search query that should wrap across multiple lines".to_owned();

        let expected = Paragraph::new(search_field_line(&app))
            .wrap(Wrap { trim: false })
            .line_count(10)
            .max(1)
            .saturating_add(2);

        assert_eq!(usize::from(top_region_height(&app, 12)), expected);
        assert!(top_region_height(&app, 12) > 3);
    }
}
