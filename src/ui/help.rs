// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use crate::app::keymap::{
    AppAction, KeyAction, KeyCodeSpec, KeyContext, KeySpec, ResolvedHelpBinding,
};
use crate::app::{App, AppStatus, AutocompleteKind, ConfigHelpSection, FocusOwner};
use crossterm::event::KeyModifiers;

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
        return blocked_input_help_items(app, "Unavailable while connecting");
    }
    if app.status == AppStatus::CommandPending {
        return blocked_input_help_items(
            app,
            &format!("Unavailable while command runs ({})", pending_command_help_label(app)),
        );
    }
    if app.status == AppStatus::Error {
        return blocked_input_help_items(app, "Unavailable after error");
    }

    let context = active_key_help_context(app);
    let mut items = keymap_help_rows(app, context);
    if app.is_compacting {
        items.push(("Status".to_owned(), "Compacting context".to_owned()));
    }
    items.push(("Mouse wheel".to_owned(), "Scroll chat".to_owned()));
    if context == KeyContext::ChatInput {
        items.push(("Paste".to_owned(), "Insert text".to_owned()));
    }

    items
}

#[derive(Clone, Debug)]
struct HelpActionGroup {
    action: KeyAction,
    keys: Vec<String>,
}

fn keymap_help_rows(app: &App, context: KeyContext) -> Vec<(String, String)> {
    let mut groups: Vec<HelpActionGroup> = Vec::new();
    for binding in app.keymap.help_bindings_for_context(context) {
        if !should_show_help_binding(app, context, &binding) {
            continue;
        }
        let key = format_help_key_spec(&binding.spec);
        if let Some(group) = groups.iter_mut().find(|group| group.action == binding.action) {
            if !group.keys.contains(&key) {
                group.keys.push(key);
            }
        } else {
            groups.push(HelpActionGroup { action: binding.action, keys: vec![key] });
        }
    }

    groups
        .into_iter()
        .map(|group| (group.keys.join(", "), help_action_label(app, group.action).to_owned()))
        .collect()
}

fn should_show_help_binding(app: &App, context: KeyContext, binding: &ResolvedHelpBinding) -> bool {
    if context == KeyContext::ChatInput
        && matches!(binding.action, KeyAction::App(AppAction::FocusPromptOrAcceptSuggestion))
        && app.pending_interaction_ids.is_empty()
    {
        return false;
    }
    true
}

fn help_action_label(app: &App, action: KeyAction) -> &'static str {
    match action {
        KeyAction::App(AppAction::CancelTurn)
            if matches!(app.status, AppStatus::Thinking | AppStatus::Running) =>
        {
            "Cancel current turn"
        }
        KeyAction::App(AppAction::CancelTurn) => "Clear pending input state",
        KeyAction::App(AppAction::FocusPromptOrAcceptSuggestion)
            if !app.pending_interaction_ids.is_empty() =>
        {
            "Focus pending prompt"
        }
        _ => action.label(),
    }
}

fn active_key_help_context(app: &App) -> KeyContext {
    match app.focus_owner() {
        FocusOwner::Mention => match app.active_autocomplete_kind() {
            Some(AutocompleteKind::Mention) => KeyContext::AutocompleteMention,
            Some(AutocompleteKind::Slash) => KeyContext::AutocompleteSlash,
            Some(AutocompleteKind::Subagent) => KeyContext::AutocompleteSubagent,
            None => KeyContext::ChatInput,
        },
        FocusOwner::Permission if focused_question_prompt(app) => KeyContext::InlineQuestion,
        FocusOwner::Permission => KeyContext::InlinePermission,
        FocusOwner::Input => KeyContext::ChatInput,
    }
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

fn blocked_input_help_items(app: &App, input_line: &str) -> Vec<(String, String)> {
    let mut rows = keymap_help_rows(app, KeyContext::ChatBlocked);
    rows.push(("Mouse wheel".to_owned(), "Scroll chat".to_owned()));
    rows.push(("Input keys".to_owned(), input_line.to_owned()));
    rows
}

fn format_help_key_spec(spec: &KeySpec) -> String {
    let mut parts = Vec::new();
    let modifiers = spec.modifiers();
    if modifiers.contains(KeyModifiers::CONTROL) {
        parts.push("Ctrl".to_owned());
    }
    if modifiers.contains(KeyModifiers::ALT) {
        parts.push("Alt".to_owned());
    }
    if modifiers.contains(KeyModifiers::SUPER) {
        parts.push("Cmd".to_owned());
    }
    if modifiers.contains(KeyModifiers::SHIFT) {
        parts.push("Shift".to_owned());
    }
    parts.push(format_help_key_code(spec.code()));
    parts.join("+")
}

fn format_help_key_code(code: KeyCodeSpec) -> String {
    match code {
        KeyCodeSpec::Char(' ') => "Space".to_owned(),
        KeyCodeSpec::Char(ch) if ch.is_ascii_alphabetic() => ch.to_ascii_uppercase().to_string(),
        KeyCodeSpec::Char(ch) => ch.to_string(),
        KeyCodeSpec::Enter => "Enter".to_owned(),
        KeyCodeSpec::Esc => "Esc".to_owned(),
        KeyCodeSpec::Backspace => "Backspace".to_owned(),
        KeyCodeSpec::Delete => "Delete".to_owned(),
        KeyCodeSpec::Insert => "Insert".to_owned(),
        KeyCodeSpec::Tab => "Tab".to_owned(),
        KeyCodeSpec::Left => "Left".to_owned(),
        KeyCodeSpec::Right => "Right".to_owned(),
        KeyCodeSpec::Up => "Up".to_owned(),
        KeyCodeSpec::Down => "Down".to_owned(),
        KeyCodeSpec::Home => "Home".to_owned(),
        KeyCodeSpec::End => "End".to_owned(),
        KeyCodeSpec::PageUp => "PageUp".to_owned(),
        KeyCodeSpec::PageDown => "PageDown".to_owned(),
        KeyCodeSpec::F(index) => format!("F{index}"),
    }
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

#[cfg(test)]
mod tests {
    use super::{key_help_items, keymap_help_rows};
    use crate::app::App;
    use crate::app::keymap::{
        AppAction, KeyAction, KeyBinding, KeyBindingSource, KeyContext, ResolvedKeymap,
    };

    fn has_row(rows: &[(String, String)], key: &str, action: &str) -> bool {
        rows.iter().any(|(left, right)| left == key && right == action)
    }

    #[test]
    fn chat_input_shortcuts_are_generated_from_keymap() {
        let app = App::test_default();

        let rows = key_help_items(&app);

        assert!(has_row(&rows, "Enter", "Send message"));
        assert!(has_row(&rows, "Shift+Enter, Ctrl+Enter", "Insert newline"));
        assert!(has_row(&rows, "Home, Ctrl+A", "Move line start"));
        assert!(has_row(&rows, "End, Ctrl+E", "Move line end"));
        assert!(has_row(&rows, "Ctrl+Y", "Yank"));
        assert!(!rows.iter().any(|(left, right)| left == "Ctrl+z/y" || right == "Undo/redo"));
    }

    #[test]
    fn chat_input_shortcuts_reflect_resolved_keymap_changes() {
        let mut app = App::test_default();
        app.keymap = ResolvedKeymap::from_bindings([KeyBinding::new(
            KeyContext::ChatInput,
            "ctrl-x".parse().expect("parse key"),
            KeyAction::App(AppAction::SubmitInput),
            KeyBindingSource::Config,
        )])
        .expect("build keymap");

        let rows = key_help_items(&app);

        assert!(has_row(&rows, "Ctrl+X", "Send message"));
        assert!(!rows.iter().any(|(left, _)| left == "Enter"));
    }

    #[test]
    fn permission_shortcuts_are_generated_without_removed_ctrl_approvals() {
        let app = App::test_default();

        let rows = keymap_help_rows(&app, KeyContext::InlinePermission);

        assert!(has_row(&rows, "Left, Up", "Previous option"));
        assert!(has_row(&rows, "Right, Down", "Next option"));
        assert!(has_row(&rows, "Enter", "Confirm option"));
        assert!(has_row(&rows, "Esc", "Cancel prompt"));
        assert!(!rows.iter().any(|(left, _)| {
            left.contains("Ctrl+A") || left.contains("Ctrl+Y") || left.contains("Ctrl+N")
        }));
    }

    #[test]
    fn question_shortcuts_include_selection_and_notes_bindings() {
        let app = App::test_default();

        let rows = keymap_help_rows(&app, KeyContext::InlineQuestion);

        assert!(has_row(&rows, "Space", "Toggle selection"));
        assert!(has_row(&rows, "Tab, Shift+Tab", "Toggle notes"));
        assert!(has_row(&rows, "Enter", "Confirm option"));
        assert!(has_row(&rows, "Esc", "Cancel prompt"));
    }
}
