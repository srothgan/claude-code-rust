// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use crate::app::{App, AppStatus, ConfigHelpSection, FocusOwner};

pub(crate) fn help_items(app: &App, section: ConfigHelpSection) -> Vec<(String, String)> {
    match section {
        ConfigHelpSection::Shortcuts => build_key_help_items(app),
        ConfigHelpSection::Commands => build_slash_help_items(app),
        ConfigHelpSection::Subagents => build_subagent_help_items(app),
    }
}

pub(crate) fn help_item_count(app: &App, section: ConfigHelpSection) -> usize {
    help_items(app, section).len()
}

pub(crate) fn key_help_items(app: &App) -> Vec<(String, String)> {
    build_key_help_items(app)
}

pub(crate) fn docs_command_items(app: &App) -> Vec<(String, String)> {
    build_slash_command_items(app)
}

pub(crate) fn subagent_help_items(app: &App) -> Vec<(String, String)> {
    build_subagent_help_items(app)
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
        ("Ctrl+c".to_owned(), "Quit".to_owned()),
        ("Ctrl+q".to_owned(), "Quit".to_owned()),
        ("Ctrl+l".to_owned(), "Redraw screen".to_owned()),
        ("Shift+Tab".to_owned(), "Cycle mode".to_owned()),
        ("Ctrl+Up/Down".to_owned(), "Scroll chat".to_owned()),
        ("Mouse wheel".to_owned(), "Scroll chat".to_owned()),
    ];
    if app.is_compacting {
        items.push(("Status".to_owned(), "Compacting context".to_owned()));
    }
    let focus_owner = app.focus_owner();

    if !app.pending_interaction_ids.is_empty() {
        match focus_owner {
            FocusOwner::Input => {
                items.push(("Tab".to_owned(), "Focus pending prompt".to_owned()));
            }
            FocusOwner::Permission => {
                items.push(("Tab".to_owned(), "Return to draft".to_owned()));
            }
            FocusOwner::Mention => {}
        }
    }

    if focus_owner != FocusOwner::Mention && focus_owner != FocusOwner::Permission {
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

    if matches!(app.status, AppStatus::Thinking | AppStatus::Running) {
        items.push(("Esc".to_owned(), "Cancel current turn".to_owned()));
    } else {
        items.push(("Esc".to_owned(), "No-op (idle)".to_owned()));
    }

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

fn build_slash_help_items(app: &App) -> Vec<(String, String)> {
    build_slash_command_items(app)
}

fn build_slash_command_items(app: &App) -> Vec<(String, String)> {
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

    let mut commands: BTreeMap<String, String> = crate::app::slash::APP_SLASH_COMMANDS
        .iter()
        .map(|spec| (spec.name.to_owned(), spec.long_description.to_owned()))
        .collect();

    for cmd in &app.available_commands {
        let name =
            if cmd.name.starts_with('/') { cmd.name.clone() } else { format!("/{}", cmd.name) };
        commands.entry(name).or_insert_with(|| cmd.description.clone());
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
