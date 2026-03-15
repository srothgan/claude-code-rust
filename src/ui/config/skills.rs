use super::theme;
use crate::app::App;
use crate::app::skills::{
    SkillsViewTab, display_label, filtered_installed, filtered_marketplace_skills, search_enabled,
    visible_marketplaces,
};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

pub(super) fn render(frame: &mut Frame, area: Rect, app: &App) {
    let body = area.inner(Margin { vertical: 1, horizontal: 1 });
    let top_height = if search_enabled(app.skills.active_tab) { 3 } else { 1 };
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(top_height),
            Constraint::Min(1),
        ])
        .split(body);

    frame.render_widget(Paragraph::new(tab_header_line(app)), sections[0]);
    render_top_region(frame, sections[2], app);
    render_list_region(frame, sections[3], app);
}

fn render_top_region(frame: &mut Frame, area: Rect, app: &App) {
    if search_enabled(app.skills.active_tab) {
        frame.render_widget(
            Paragraph::new(search_field_line(app))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(if app.skills.search_focused {
                            " Search "
                        } else {
                            " Search (Up to focus) "
                        })
                        .border_style(if app.skills.search_focused {
                            Style::default().fg(theme::RUST_ORANGE)
                        } else {
                            Style::default().fg(theme::DIM)
                        }),
                )
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(" "),
            Span::styled(
                "Configured marketplaces",
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ", Style::default().fg(theme::DIM)),
            Span::styled("Add marketplace placeholder below", Style::default().fg(theme::DIM)),
        ])),
        area,
    );
}

fn render_list_region(frame: &mut Frame, area: Rect, app: &App) {
    let list_area =
        if area.width > 1 { area.inner(Margin { vertical: 0, horizontal: 1 }) } else { area };
    let rendered = match app.skills.active_tab {
        SkillsViewTab::Installed => installed_list(app, list_area.width, list_area.height),
        SkillsViewTab::Skills => skills_list(app, list_area.width, list_area.height),
        SkillsViewTab::Marketplace => marketplace_list(app, list_area.width, list_area.height),
    };
    frame.render_widget(
        Paragraph::new(rendered.lines).scroll((rendered.scroll, 0)).wrap(Wrap { trim: false }),
        list_area,
    );
}

fn tab_header_line(app: &App) -> Line<'static> {
    let spans = SkillsViewTab::ALL
        .into_iter()
        .enumerate()
        .flat_map(|(index, tab)| {
            let active = tab == app.skills.active_tab;
            let count = match tab {
                SkillsViewTab::Installed => filtered_installed(&app.skills).len(),
                SkillsViewTab::Skills => filtered_marketplace_skills(&app.skills).len(),
                SkillsViewTab::Marketplace => visible_marketplaces(&app.skills).len(),
            };
            let label = format!(" {} ({count}) ", tab.title());
            let mut spans = vec![Span::styled(
                label,
                if active {
                    Style::default()
                        .fg(Color::Black)
                        .bg(theme::RUST_ORANGE)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
                },
            )];
            if index + 1 < SkillsViewTab::ALL.len() {
                spans.push(Span::styled("  ", Style::default().fg(theme::DIM)));
            }
            spans
        })
        .collect::<Vec<_>>();
    Line::from(spans)
}

fn search_field_line(app: &App) -> Line<'static> {
    let cursor_style =
        Style::default().fg(Color::Black).bg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD);
    let text_style = Style::default().fg(Color::White);
    let hint_style = Style::default().fg(theme::DIM);
    let query = app.skills.search_query_for(app.skills.active_tab);

    if query.is_empty() {
        if app.skills.search_focused {
            return Line::from(vec![
                Span::styled(" ".to_owned(), cursor_style),
                Span::styled("Type to filter this list".to_owned(), hint_style),
            ]);
        }
        return Line::from(Span::styled("Type to filter this list", hint_style));
    }

    if app.skills.search_focused {
        return Line::from(vec![
            Span::styled(query.to_owned(), text_style),
            Span::styled(" ".to_owned(), cursor_style),
        ]);
    }

    Line::from(Span::styled(query.to_owned(), text_style))
}

fn installed_list(app: &App, viewport_width: u16, viewport_height: u16) -> RenderedList {
    let entries = filtered_installed(&app.skills);
    if entries.is_empty() {
        return RenderedList::single(
            if app.skills.loading {
                "Loading installed plugins..."
            } else if app.skills.search_query_for(SkillsViewTab::Installed).is_empty() {
                "No installed plugins found."
            } else {
                "No installed plugins match the current search."
            },
            viewport_height,
        );
    }

    let blocks = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let selected =
                index == app.skills.installed_selected_index && !app.skills.search_focused;
            let mut lines = vec![
                title_line(&display_label(&entry.id), selected),
                meta_line(
                    &format!(
                        "{} | {}{}",
                        if entry.enabled { "enabled" } else { "disabled" },
                        entry.scope,
                        entry
                            .version
                            .as_deref()
                            .map_or_else(String::new, |version| format!(" | {version}"))
                    ),
                    selected,
                ),
            ];
            if let Some(project_path) = entry.project_path.as_deref() {
                lines.push(meta_line(&format!("project | {project_path}"), selected));
            }
            lines
        })
        .collect::<Vec<_>>();
    RenderedList::from_blocks(
        &blocks,
        app.skills.installed_selected_index,
        viewport_width,
        viewport_height,
    )
}

fn skills_list(app: &App, viewport_width: u16, viewport_height: u16) -> RenderedList {
    let entries = filtered_marketplace_skills(&app.skills);
    if entries.is_empty() {
        return RenderedList::single(
            if app.skills.loading {
                "Loading marketplace skills..."
            } else if app.skills.search_query_for(SkillsViewTab::Skills).is_empty() {
                "No skills are available from the configured marketplaces."
            } else {
                "No marketplace skills match the current search."
            },
            viewport_height,
        );
    }

    let blocks = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let selected = index == app.skills.skills_selected_index && !app.skills.search_focused;
            let mut lines = vec![title_line(&display_label(&entry.name), selected)];
            lines.push(meta_line(&format!("Plugin: {}", entry.plugin_id), selected));
            if let Some(description) = entry.description.as_deref() {
                lines.push(meta_line(description, selected));
            }
            if let Some(marketplace_name) = entry.marketplace_name.as_deref() {
                lines.push(meta_line(&format!("Marketplace: {marketplace_name}"), selected));
            }
            if let Some(version) = entry.version.as_deref() {
                lines.push(meta_line(&format!("Version: {version}"), selected));
            }
            lines
        })
        .collect::<Vec<_>>();
    RenderedList::from_blocks(
        &blocks,
        app.skills.skills_selected_index,
        viewport_width,
        viewport_height,
    )
}

fn marketplace_list(app: &App, viewport_width: u16, viewport_height: u16) -> RenderedList {
    let entries = visible_marketplaces(&app.skills);
    if entries.is_empty() && app.skills.loading {
        return RenderedList::single("Loading configured marketplaces...", viewport_height);
    }
    let mut blocks = entries
        .iter()
        .enumerate()
        .map(|(index, marketplace)| {
            let selected = index == app.skills.marketplace_selected_index;
            let mut lines = vec![title_line(&display_label(&marketplace.name), selected)];
            if let Some(source) = marketplace.source.as_deref() {
                lines.push(meta_line(&format!("Source: {source}"), selected));
            }
            if let Some(repo) = marketplace.repo.as_deref() {
                lines.push(meta_line(&format!("Repo: {repo}"), selected));
            }
            lines
        })
        .collect::<Vec<_>>();

    blocks.push(vec![
        title_line("Add marketplace", false),
        meta_line("Placeholder only. Add/remove flows come later.", false),
    ]);

    if entries.is_empty() {
        blocks.push(vec![
            title_line("No configured marketplaces", false),
            meta_line("Use Add marketplace when that flow exists.", false),
        ]);
    }

    RenderedList::from_blocks(
        &blocks,
        app.skills.marketplace_selected_index,
        viewport_width,
        viewport_height,
    )
}

fn title_line(text: &str, selected: bool) -> Line<'static> {
    Line::from(Span::styled(
        text.to_owned(),
        if selected {
            Style::default().fg(Color::Black).bg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
        },
    ))
}

fn meta_line(text: &str, selected: bool) -> Line<'static> {
    Line::from(Span::styled(
        format!("  {text}"),
        if selected { Style::default().fg(Color::White) } else { Style::default().fg(theme::DIM) },
    ))
}

struct RenderedList {
    lines: Vec<Line<'static>>,
    scroll: u16,
}

impl RenderedList {
    fn single(message: &str, _viewport_height: u16) -> Self {
        Self {
            lines: vec![Line::from(Span::styled(
                message.to_owned(),
                Style::default().fg(theme::DIM),
            ))],
            scroll: 0,
        }
    }

    fn from_blocks(
        blocks: &[Vec<Line<'static>>],
        selected_index: usize,
        viewport_width: u16,
        viewport_height: u16,
    ) -> Self {
        let mut lines = Vec::new();
        let mut selected_start = 0usize;
        let mut selected_height = 1usize;
        let mut offset = 0usize;

        for (index, block) in blocks.iter().enumerate() {
            let block_height = visual_block_height(block, viewport_width).saturating_add(1);
            if index == selected_index {
                selected_start = offset;
                selected_height = block_height;
            }
            lines.extend(block.iter().cloned());
            lines.push(Line::default());
            offset = offset.saturating_add(block_height);
        }

        Self { lines, scroll: selected_scroll(selected_start, selected_height, viewport_height) }
    }
}

fn selected_scroll(selected_start: usize, selected_height: usize, viewport_height: u16) -> u16 {
    let viewport_height = usize::from(viewport_height.max(1));
    if selected_start.saturating_add(selected_height) <= viewport_height {
        0
    } else {
        u16::try_from(
            selected_start.saturating_add(selected_height).saturating_sub(viewport_height),
        )
        .unwrap_or(u16::MAX)
    }
}

fn visual_block_height(block: &[Line<'static>], viewport_width: u16) -> usize {
    block.iter().map(|line| visual_line_height(line, viewport_width)).sum::<usize>()
}

fn visual_line_height(line: &Line<'static>, viewport_width: u16) -> usize {
    let width = usize::from(viewport_width.max(1));
    let content = line.spans.iter().map(|span| span.content.as_ref()).collect::<String>();
    let visual_width = UnicodeWidthStr::width(content.as_str()).max(1);
    visual_width.div_ceil(width)
}
