// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use crate::app::{App, AppStatus, FocusOwner, HelpView};
use crate::ui::theme;
use crate::ui::two_column_list::{self, TwoColumnItem};
use crate::ui::wrap::{display_width, take_prefix_by_width, truncate_to_width};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

const COLUMN_GAP: usize = 4;
/// Content lines available in the help panel (excluding padding and borders).
const MAX_ROWS: usize = 10;
const HELP_VERTICAL_PADDING_LINES: usize = 1;
const SUBAGENT_NAME_MIN_WIDTH: usize = 12;
const SUBAGENT_NAME_MAX_WIDTH: usize = 28;
const SUBAGENT_NAME_MAX_SHARE_NUM: usize = 2;
const SUBAGENT_NAME_MAX_SHARE_DEN: usize = 5;
const HELP_PANEL_HEIGHT: u16 = 14;
const HELP_BUILTIN_SLASH_COMMANDS: [(&str, &str); 9] = [
    ("/config", "Open settings"),
    ("/docs", "Show in-chat help topics"),
    ("/login", "Authenticate with Claude"),
    ("/logout", "Sign out of Claude"),
    ("/mcp", "Open MCP"),
    ("/opus-version", "Pin the Opus alias version for this folder"),
    ("/plugins", "Open plugins"),
    ("/status", "Show session status"),
    ("/usage", "Open usage"),
];

pub fn is_active(app: &App) -> bool {
    app.is_help_active()
}

/// Returns the number of items in the current help tab (for key navigation).
pub fn help_item_count(app: &App) -> usize {
    build_help_items(app).len()
}

#[allow(clippy::cast_possible_truncation)]
pub fn compute_height(app: &App, _area_width: u16) -> u16 {
    if !is_active(app) {
        return 0;
    }
    HELP_PANEL_HEIGHT
}

pub(crate) fn sync_geometry_state(app: &mut App, panel_width: u16) {
    if !is_active(app) || panel_width == 0 {
        app.help_visible_count = 0;
        return;
    }

    let items = build_help_items(app);
    let visible_count = visible_count_for_view(app, &items, panel_width);
    if matches!(app.help_view, HelpView::SlashCommands | HelpView::Subagents) {
        app.help_dialog.clamp(items.len(), visible_count);
    }
    app.help_visible_count = visible_count;
}

#[allow(clippy::cast_possible_truncation)]
pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.height == 0 || area.width == 0 || !is_active(app) {
        return;
    }

    let items = build_help_items(app);
    if items.is_empty() {
        return;
    }

    match app.help_view {
        HelpView::Keys => render_keys_help(frame, area, app, &items),
        HelpView::SlashCommands | HelpView::Subagents => {
            render_two_column_help(frame, area, app, &items);
        }
    }
}

#[allow(clippy::cast_possible_truncation)]
fn render_keys_help(frame: &mut Frame, area: Rect, app: &App, items: &[(String, String)]) {
    let rows = items.len().div_ceil(2).min(MAX_ROWS);
    let max_items = rows * 2;
    let items = &items[..items.len().min(max_items)];
    let inner_width = area.width.saturating_sub(2) as usize;
    let col_width = (inner_width.saturating_sub(COLUMN_GAP)) / 2;
    let left_width = col_width;
    let right_width = col_width;
    let mut lines = Vec::with_capacity(rows + HELP_VERTICAL_PADDING_LINES * 2);

    for _ in 0..HELP_VERTICAL_PADDING_LINES {
        lines.push(Line::default());
    }

    for row in 0..rows {
        let left_idx = row;
        let right_idx = row + rows;

        let left = items.get(left_idx).cloned().unwrap_or_default();
        let right = items.get(right_idx).cloned().unwrap_or_default();

        let left_lines = format_item_cell_lines(&left, left_width);
        let right_lines = format_item_cell_lines(&right, right_width);
        lines.extend(two_column_list::join_column_lines(
            left_lines,
            right_lines,
            left_width,
            COLUMN_GAP,
        ));
    }

    for _ in 0..HELP_VERTICAL_PADDING_LINES {
        lines.push(Line::default());
    }

    let block = Block::default()
        .title(help_title(app.help_view))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded);
    frame.render_widget(Paragraph::new(Text::from(lines)).block(block), area);
}

#[allow(clippy::cast_possible_truncation)]
fn render_two_column_help(
    frame: &mut Frame,
    area: Rect,
    app: &mut App,
    items: &[(String, String)],
) {
    let inner_width = area.width.saturating_sub(2) as usize;
    let (name_width, desc_width) = help_item_column_widths(items, inner_width);
    let visible_count = visible_count_for_view(app, items, area.width);

    let start = app.help_dialog.scroll_offset;
    let end = (start + visible_count).min(items.len());
    let selected = app.help_dialog.selected;
    let visible_items = &items[start..end];
    let list_items = build_two_column_items(visible_items, selected, start);
    let mut lines = Vec::with_capacity(
        visible_count + visible_count.saturating_sub(1) + HELP_VERTICAL_PADDING_LINES * 2,
    );

    for _ in 0..HELP_VERTICAL_PADDING_LINES {
        lines.push(Line::default());
    }

    lines.extend(two_column_list::render_lines(&list_items, name_width, desc_width, COLUMN_GAP, 1));

    for _ in 0..HELP_VERTICAL_PADDING_LINES {
        lines.push(Line::default());
    }

    let block = Block::default()
        .title(help_title(app.help_view))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded);
    frame.render_widget(Paragraph::new(Text::from(lines)).block(block), area);
}

fn visible_count_for_view(app: &App, items: &[(String, String)], panel_width: u16) -> usize {
    if items.is_empty() || panel_width == 0 {
        return 0;
    }

    match app.help_view {
        HelpView::Keys => items.len().div_ceil(2).min(MAX_ROWS),
        HelpView::SlashCommands | HelpView::Subagents => {
            let inner_width = panel_width.saturating_sub(2) as usize;
            let (name_width, desc_width) = help_item_column_widths(items, inner_width);
            let count_items = items
                .iter()
                .map(|(left, right)| TwoColumnItem {
                    left: left.clone(),
                    right: right.clone(),
                    left_style: Style::default().add_modifier(Modifier::BOLD),
                    right_style: Style::default(),
                })
                .collect::<Vec<_>>();
            two_column_list::visible_item_count(
                &count_items,
                app.help_dialog.scroll_offset,
                MAX_ROWS,
                name_width,
                desc_width,
                1,
            )
        }
    }
}

fn build_help_items(app: &App) -> Vec<(String, String)> {
    match app.help_view {
        HelpView::Keys => build_key_help_items(app),
        HelpView::SlashCommands => build_slash_help_items(app),
        HelpView::Subagents => build_subagent_help_items(app),
    }
}

fn build_key_help_items(app: &App) -> Vec<(String, String)> {
    if app.status == AppStatus::Connecting {
        return blocked_input_help_items("Unavailable while connecting");
    }
    if app.status == AppStatus::CommandPending {
        return blocked_input_help_items(&format!(
            "Unavailable while command runs ({})",
            pending_command_help_label(app)
        ));
    }
    if app.status == AppStatus::Error {
        return blocked_input_help_items("Unavailable after error");
    }

    let mut items: Vec<(String, String)> = vec![
        // Global
        ("Ctrl+c".to_owned(), "Quit".to_owned()),
        ("Ctrl+q".to_owned(), "Quit".to_owned()),
        ("Ctrl+l".to_owned(), "Redraw screen".to_owned()),
        ("Shift+Tab".to_owned(), "Cycle mode".to_owned()),
        ("Ctrl+o".to_owned(), "Toggle tool collapse".to_owned()),
        ("Ctrl+t".to_owned(), "Toggle todos (when available)".to_owned()),
        // Chat scrolling
        ("Ctrl+Up/Down".to_owned(), "Scroll chat".to_owned()),
        ("Mouse wheel".to_owned(), "Scroll chat".to_owned()),
    ];
    if app.is_compacting {
        items.push(("Status".to_owned(), "Compacting context".to_owned()));
    }
    let focus_owner = app.focus_owner();

    if app.show_todo_panel && !app.todos.is_empty() && app.pending_interaction_ids.is_empty() {
        items.push(("Tab".to_owned(), "Toggle todo focus".to_owned()));
    }

    if !app.pending_interaction_ids.is_empty() {
        match focus_owner {
            FocusOwner::Input => {
                items.push(("Tab".to_owned(), "Focus pending prompt".to_owned()));
            }
            FocusOwner::Permission => {
                items.push(("Tab".to_owned(), "Return to draft".to_owned()));
            }
            _ => {}
        }
    }

    // Input + navigation (active outside todo-list and mention focus)
    if focus_owner != FocusOwner::TodoList
        && focus_owner != FocusOwner::Mention
        && focus_owner != FocusOwner::Help
        && focus_owner != FocusOwner::Permission
    {
        items.push(("Enter".to_owned(), "Send message".to_owned()));
        items.push(("Shift+Enter".to_owned(), "Insert newline".to_owned()));
        items.push(("Up/Down".to_owned(), "Move cursor / scroll chat".to_owned()));
        items.push(("Left/Right".to_owned(), "Move cursor".to_owned()));
        items.push(("Ctrl+Left/Right".to_owned(), "Word left/right".to_owned()));
        items.push(("Home/End".to_owned(), "Line start/end".to_owned()));
        items.push(("Backspace".to_owned(), "Delete before".to_owned()));
        items.push(("Delete".to_owned(), "Delete after".to_owned()));
        items.push(("Ctrl+Backspace/Delete".to_owned(), "Delete word".to_owned()));
        items.push(("Ctrl+z/y".to_owned(), "Undo/redo".to_owned()));
        items.push(("Paste".to_owned(), "Insert text".to_owned()));
    }

    // Turn control
    if matches!(app.status, crate::app::AppStatus::Thinking | crate::app::AppStatus::Running) {
        items.push(("Esc".to_owned(), "Cancel current turn".to_owned()));
    } else if focus_owner == FocusOwner::TodoList {
        items.push(("Esc".to_owned(), "Exit todo focus".to_owned()));
    } else {
        items.push(("Esc".to_owned(), "No-op (idle)".to_owned()));
    }

    // Inline interactions (permissions or questions)
    if !app.pending_interaction_ids.is_empty() && focus_owner == FocusOwner::Permission {
        if app.pending_interaction_ids.len() > 1 {
            items.push(("Up/Down".to_owned(), "Switch prompt focus".to_owned()));
        }
        if focused_question_prompt(app) {
            items.push(("Left/Right".to_owned(), "Move selection".to_owned()));
            items.push(("Tab".to_owned(), "Toggle notes editor".to_owned()));
            items.push(("Enter".to_owned(), "Confirm answer".to_owned()));
            items.push(("Esc".to_owned(), "Cancel prompt".to_owned()));
        } else {
            items.push(("Left/Right".to_owned(), "Select option".to_owned()));
            items.push(("Enter".to_owned(), "Confirm option".to_owned()));
            items.push(("Ctrl+y/a/n".to_owned(), "Quick select".to_owned()));
            items.push(("Esc".to_owned(), "Reject".to_owned()));
        }
    }
    if focus_owner == FocusOwner::TodoList {
        items.push(("Up/Down".to_owned(), "Select todo (todo focus)".to_owned()));
    }

    items
}

fn focused_question_prompt(app: &App) -> bool {
    let Some(tool_id) = app.pending_interaction_ids.first() else {
        return false;
    };
    let Some((mi, bi)) = app.lookup_tool_call(tool_id) else {
        return false;
    };
    let Some(crate::app::MessageBlock::ToolCall(tc)) =
        app.messages.get(mi).and_then(|message| message.blocks.get(bi))
    else {
        return false;
    };
    tc.pending_question.is_some()
}

fn blocked_input_help_items(input_line: &str) -> Vec<(String, String)> {
    vec![
        ("?".to_owned(), "Toggle help".to_owned()),
        ("Ctrl+c".to_owned(), "Quit".to_owned()),
        ("Ctrl+q".to_owned(), "Quit".to_owned()),
        ("Up/Down".to_owned(), "Scroll chat".to_owned()),
        ("Ctrl+Up/Down".to_owned(), "Scroll chat".to_owned()),
        ("Mouse wheel".to_owned(), "Scroll chat".to_owned()),
        ("Ctrl+l".to_owned(), "Redraw screen".to_owned()),
        ("Input keys".to_owned(), input_line.to_owned()),
    ]
}

fn pending_command_help_label(app: &App) -> String {
    app.pending_command_label.clone().unwrap_or_else(|| "Processing command...".to_owned())
}

pub(crate) fn key_help_items(app: &App) -> Vec<(String, String)> {
    build_key_help_items(app)
}

pub(crate) fn slash_help_items(app: &App) -> Vec<(String, String)> {
    build_slash_command_items(app, &HELP_BUILTIN_SLASH_COMMANDS)
}

pub(crate) fn docs_command_items(app: &App) -> Vec<(String, String)> {
    slash_help_items(app)
}

pub(crate) fn subagent_help_items(app: &App) -> Vec<(String, String)> {
    build_subagent_help_items(app)
}

fn build_slash_help_items(app: &App) -> Vec<(String, String)> {
    slash_help_items(app)
}

fn build_slash_command_items(
    app: &App,
    builtin_commands: &[(&str, &str)],
) -> Vec<(String, String)> {
    use std::collections::BTreeMap;

    let mut rows = Vec::new();
    if app.status == AppStatus::Connecting {
        rows.push(("Loading commands...".to_owned(), String::new()));
        return rows;
    }
    if app.status == AppStatus::CommandPending {
        rows.push((pending_command_help_label(app), String::new()));
        return rows;
    }

    let mut commands: BTreeMap<String, String> = builtin_commands
        .iter()
        .map(|(name, description)| ((*name).to_owned(), (*description).to_owned()))
        .collect();

    for cmd in &app.available_commands {
        let name =
            if cmd.name.starts_with('/') { cmd.name.clone() } else { format!("/{}", cmd.name) };
        match commands.get_mut(&name) {
            Some(existing) if !cmd.description.trim().is_empty() => {
                existing.clone_from(&cmd.description);
            }
            Some(_) => {}
            None => {
                commands.insert(name, cmd.description.clone());
            }
        }
    }

    if commands.is_empty() {
        rows.push((
            "No slash commands advertised".to_owned(),
            "Not advertised in this session".to_owned(),
        ));
        return rows;
    }

    for (name, desc) in commands {
        let description =
            if desc.trim().is_empty() { "No description provided".to_owned() } else { desc };
        rows.push((name, description));
    }

    rows
}

fn build_subagent_help_items(app: &App) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    if app.status == AppStatus::Connecting {
        rows.push(("Loading subagents...".to_owned(), String::new()));
        return rows;
    }
    if app.status == AppStatus::CommandPending {
        rows.push((pending_command_help_label(app), String::new()));
        return rows;
    }

    let mut agents: Vec<(String, String)> = app
        .available_agents
        .iter()
        .filter(|agent| !agent.name.trim().is_empty())
        .map(|agent| {
            let description = if agent.description.trim().is_empty() {
                "No description provided".to_owned()
            } else {
                agent.description.clone()
            };
            let label = match &agent.model {
                Some(model) if !model.trim().is_empty() => {
                    format!("&{}\nModel: {}", agent.name, model.trim())
                }
                _ => format!("&{}", agent.name),
            };
            (label, description)
        })
        .collect();

    agents.sort_by(|a, b| a.0.cmp(&b.0));
    agents.dedup_by(|a, b| a.0 == b.0);
    if agents.is_empty() {
        rows.push((
            "No subagents advertised".to_owned(),
            "Not advertised in this session".to_owned(),
        ));
        return rows;
    }

    rows.extend(agents);
    rows
}

fn help_item_column_widths(items: &[(String, String)], inner_width: usize) -> (usize, usize) {
    if inner_width == 0 {
        return (0, 0);
    }
    if inner_width <= COLUMN_GAP + 1 {
        return (inner_width, 1);
    }

    let max_name_width =
        items.iter().map(|(name, _)| display_width(name.as_str())).max().unwrap_or(0);
    let share_cap =
        inner_width.saturating_mul(SUBAGENT_NAME_MAX_SHARE_NUM) / SUBAGENT_NAME_MAX_SHARE_DEN;
    let min_name_width = SUBAGENT_NAME_MIN_WIDTH.min(share_cap.max(1));
    let preferred_name_width =
        max_name_width.max(min_name_width).min(SUBAGENT_NAME_MAX_WIDTH).min(share_cap.max(1));
    let max_name_fit = inner_width.saturating_sub(COLUMN_GAP + 1);
    let name_width = preferred_name_width.clamp(1, max_name_fit.max(1));
    let desc_width = inner_width.saturating_sub(name_width + COLUMN_GAP).max(1);

    (name_width, desc_width)
}

fn help_title(view: HelpView) -> Line<'static> {
    let keys_style = if matches!(view, HelpView::Keys) {
        Style::default().fg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme::DIM)
    };
    let slash_style = if matches!(view, HelpView::SlashCommands) {
        Style::default().fg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme::DIM)
    };
    let subagent_style = if matches!(view, HelpView::Subagents) {
        Style::default().fg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme::DIM)
    };

    let hint = if matches!(view, HelpView::SlashCommands | HelpView::Subagents) {
        "  (< > tabs  \u{25b2}\u{25bc} scroll)"
    } else {
        "  (< > switch tabs)"
    };

    Line::from(vec![
        Span::styled(
            " Help ",
            Style::default().fg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD),
        ),
        Span::styled("[", Style::default().fg(theme::DIM)),
        Span::styled("Keys", keys_style),
        Span::styled(" | ", Style::default().fg(theme::DIM)),
        Span::styled("Slash", slash_style),
        Span::styled(" | ", Style::default().fg(theme::DIM)),
        Span::styled("Subagents", subagent_style),
        Span::styled("]", Style::default().fg(theme::DIM)),
        Span::styled(hint, Style::default().fg(theme::DIM)),
    ])
}

fn format_item_cell_lines(item: &(String, String), width: usize) -> Vec<Line<'static>> {
    let (label, desc) = item;
    if width == 0 {
        return vec![Line::default()];
    }
    if label.is_empty() && desc.is_empty() {
        return vec![Line::default()];
    }

    let label = truncate_to_width(label, width);
    let label_width = display_width(label.as_str());
    let sep = " : ";
    let sep_width = display_width(sep);

    if desc.is_empty() {
        return vec![Line::from(Span::styled(
            label,
            Style::default().add_modifier(Modifier::BOLD),
        ))];
    }

    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut rest = desc.to_owned();

    if label_width + sep_width < width {
        let first_desc_width = width - label_width - sep_width;
        let (first_chunk, remaining) = take_prefix_by_width(&rest, first_desc_width);
        lines.push(Line::from(vec![
            Span::styled(label, Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(sep.to_owned(), Style::default().fg(theme::DIM)),
            Span::raw(first_chunk),
        ]));
        rest = remaining;
    } else {
        lines.push(Line::from(Span::styled(label, Style::default().add_modifier(Modifier::BOLD))));
    }

    while !rest.is_empty() {
        let (chunk, remaining) = take_prefix_by_width(&rest, width);
        if chunk.is_empty() {
            break;
        }
        lines.push(Line::raw(chunk));
        rest = remaining;
    }

    if lines.is_empty() { vec![Line::default()] } else { lines }
}

fn build_two_column_items(
    items: &[(String, String)],
    selected: usize,
    absolute_start: usize,
) -> Vec<TwoColumnItem> {
    items
        .iter()
        .enumerate()
        .map(|(view_index, (name, description))| {
            let abs_index = absolute_start + view_index;
            let is_selected = abs_index == selected;
            let left_style = if is_selected {
                Style::default().fg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD)
            } else {
                Style::default().add_modifier(Modifier::BOLD)
            };
            let right_style = if is_selected {
                Style::default().fg(theme::RUST_ORANGE)
            } else {
                Style::default()
            };
            TwoColumnItem {
                left: name.clone(),
                right: description.clone(),
                left_style,
                right_style,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::build_help_items;
    use crate::app::{App, AppStatus, FocusTarget, HelpView, TodoItem, TodoStatus};

    fn has_item(items: &[(String, String)], key: &str, desc: &str) -> bool {
        items.iter().any(|(k, d)| k == key && d == desc)
    }

    fn has_key(items: &[(String, String)], key: &str) -> bool {
        items.iter().any(|(k, _)| k == key)
    }

    fn item_for_key<'a>(items: &'a [(String, String)], key: &str) -> Option<&'a str> {
        items.iter().find(|(k, _)| k == key).map(|(_, desc)| desc.as_str())
    }

    #[test]
    fn tab_toggle_only_shown_when_todos_available() {
        let mut app = App::test_default();
        let items = build_help_items(&app);
        assert!(!has_item(&items, "Tab", "Toggle todo focus"));

        app.show_todo_panel = true;
        app.todos.push(TodoItem {
            content: "Task".into(),
            status: TodoStatus::Pending,
            active_form: String::new(),
        });
        let items = build_help_items(&app);
        assert!(has_item(&items, "Tab", "Toggle todo focus"));
    }

    #[test]
    fn key_tab_never_shows_ctrl_u_for_update_hiding() {
        let mut app = App::test_default();
        let items = build_help_items(&app);
        assert!(!has_item(&items, "Ctrl+u", "Hide update hint"));

        app.update_notice = Some(crate::app::UpdateNoticeState {
            current_version: "0.11.1".into(),
            latest_version: "0.11.2".into(),
            emitted_session_scope_epoch: None,
        });
        let items = build_help_items(&app);
        assert!(!has_item(&items, "Ctrl+u", "Hide update hint"));
    }

    #[test]
    fn permission_navigation_only_shown_when_permission_has_focus() {
        let mut app = App::test_default();
        app.pending_interaction_ids = vec!["perm-1".into(), "perm-2".into()];

        // Without permission focus claim, do not show permission-only arrows.
        let items = build_help_items(&app);
        assert!(has_item(&items, "Tab", "Focus pending prompt"));
        assert!(has_item(&items, "Enter", "Send message"));
        assert!(!has_item(&items, "Left/Right", "Select option"));
        assert!(!has_item(&items, "Up/Down", "Switch prompt focus"));

        app.claim_focus_target(FocusTarget::Permission);
        let items = build_help_items(&app);
        assert!(has_item(&items, "Tab", "Return to draft"));
        assert!(!has_item(&items, "Enter", "Send message"));
        assert!(has_item(&items, "Enter", "Confirm option"));
        assert!(has_item(&items, "Left/Right", "Select option"));
        assert!(has_item(&items, "Up/Down", "Switch prompt focus"));
    }

    #[test]
    fn slash_tab_shows_advertised_commands_with_description() {
        let mut app = App::test_default();
        app.help_view = HelpView::SlashCommands;
        app.available_commands = vec![
            crate::agent::model::AvailableCommand::new("/help", "Open help"),
            crate::agent::model::AvailableCommand::new("memory", ""),
        ];

        let items = build_help_items(&app);
        assert!(has_item(&items, "/help", "Open help"));
        assert!(has_item(&items, "/memory", "No description provided"));
    }

    #[test]
    fn slash_tab_shows_local_auth_and_config_commands_without_advertisement() {
        let mut app = App::test_default();
        app.help_view = HelpView::SlashCommands;

        let items = build_help_items(&app);
        for command in ["/config", "/docs", "/login", "/logout", "/mcp", "/usage"] {
            assert!(has_key(&items, command), "missing builtin command: {command}");
        }
        assert!(!has_item(
            &items,
            "No slash commands advertised",
            "Not advertised in this session"
        ));
    }

    #[test]
    fn slash_tab_shows_login_logout_when_advertised() {
        let mut app = App::test_default();
        app.help_view = HelpView::SlashCommands;
        app.available_commands = vec![
            crate::agent::model::AvailableCommand::new("/login", "Login"),
            crate::agent::model::AvailableCommand::new("/logout", "Logout"),
        ];

        let items = build_help_items(&app);
        assert!(has_key(&items, "/config"));
        assert_eq!(item_for_key(&items, "/login"), Some("Login"));
        assert_eq!(item_for_key(&items, "/logout"), Some("Logout"));
    }

    #[test]
    fn slash_tab_shows_loading_commands_while_connecting() {
        let mut app = App::test_default();
        app.help_view = HelpView::SlashCommands;
        app.status = AppStatus::Connecting;

        let items = build_help_items(&app);
        assert!(
            items.iter().any(|(name, _)| name.contains("Loading") && name.contains("commands"))
        );
        assert!(!has_item(
            &items,
            "No slash commands advertised",
            "Not advertised in this session"
        ));
    }

    #[test]
    fn key_tab_connecting_shows_startup_shortcuts_only() {
        let mut app = App::test_default();
        app.status = AppStatus::Connecting;

        let items = build_help_items(&app);
        assert!(has_item(&items, "?", "Toggle help"));
        assert!(has_item(&items, "Ctrl+c", "Quit"));
        assert!(has_item(&items, "Ctrl+q", "Quit"));
        assert!(has_item(&items, "Up/Down", "Scroll chat"));
        assert!(has_key(&items, "Input keys"));
        assert!(!has_item(&items, "Enter", "Send message"));
    }

    #[test]
    fn key_tab_error_shows_locked_input_shortcuts() {
        let mut app = App::test_default();
        app.status = AppStatus::Error;

        let items = build_help_items(&app);
        assert!(has_item(&items, "Ctrl+c", "Quit"));
        assert!(has_item(&items, "Ctrl+q", "Quit"));
        assert!(has_item(&items, "Up/Down", "Scroll chat"));
        assert!(has_key(&items, "Input keys"));
        assert!(!has_item(&items, "Enter", "Send message"));
    }

    #[test]
    fn subagent_tab_shows_advertised_subagents() {
        let mut app = App::test_default();
        app.help_view = HelpView::Subagents;
        app.available_agents = vec![
            crate::agent::model::AvailableAgent::new("reviewer", "Review code").model("haiku"),
            crate::agent::model::AvailableAgent::new("explore", ""),
        ];

        let items = build_help_items(&app);
        assert!(has_item(&items, "&reviewer\nModel: haiku", "Review code"));
        assert!(has_item(&items, "&explore", "No description provided"));
    }

    #[test]
    fn subagent_tab_shows_loading_while_connecting() {
        let mut app = App::test_default();
        app.help_view = HelpView::Subagents;
        app.status = AppStatus::Connecting;

        let items = build_help_items(&app);
        assert!(
            items.iter().any(|(name, _)| name.contains("Loading") && name.contains("subagents"))
        );
    }
}
