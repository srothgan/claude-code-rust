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
    if let Some(label) = app.displayed_pending_command_label() {
        return blocked_input_help_items(app, &format!("Unavailable while command runs ({label})"));
    }
    if app.status == AppStatus::Error {
        return blocked_input_help_items(app, "Unavailable after error");
    }

    let context = active_key_help_context(app);
    let mut items = keymap_help_rows(app, context);
    if app.turn.compaction.is_active() {
        items.push(("Status".to_owned(), "Compacting context".to_owned()));
    }
    items.push(("Mouse wheel".to_owned(), "Scroll terminal history".to_owned()));
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
        app.transcript.messages.get(mi).and_then(|message| message.blocks.get(bi))
    else {
        return false;
    };
    tc.pending_question.is_some()
}

fn blocked_input_help_items(app: &App, input_line: &str) -> Vec<(String, String)> {
    let mut rows = keymap_help_rows(app, KeyContext::ChatBlocked);
    rows.push(("Mouse wheel".to_owned(), "Scroll terminal history".to_owned()));
    rows.push(("Input keys".to_owned(), input_line.to_owned()));
    rows
}

pub(crate) fn format_help_key_spec(spec: &KeySpec) -> String {
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
    if let Some(label) = app.displayed_pending_command_label() {
        rows.push((label.to_owned(), String::new()));
        return rows;
    }

    let mut commands: BTreeMap<String, String> = crate::app::slash::APP_SLASH_COMMANDS
        .iter()
        .map(|spec| (spec.name.to_owned(), spec.long_description.to_owned()))
        .collect();

    for cmd in &app.sdk_inventory.available_commands {
        let name =
            if cmd.name.starts_with('/') { cmd.name.clone() } else { format!("/{}", cmd.name) };
        if crate::app::slash::app_owned_command_name(&name).is_some() {
            continue;
        }
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
    if let Some(label) = app.displayed_pending_command_label() {
        rows.push((label.to_owned(), String::new()));
        return rows;
    }

    let mut agents: Vec<(String, String)> = app
        .sdk_inventory
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
    use super::key_help_items;
    use crate::app::keymap::{
        AppAction, KeyAction, KeyBinding, KeyBindingSource, KeyContext, ResolvedKeymap,
    };
    use crate::app::{App, AppStatus};

    fn has_row(rows: &[(String, String)], key: &str, action: &str) -> bool {
        rows.iter().any(|(left, right)| left == key && right == action)
    }

    fn has_key(rows: &[(String, String)], key: &str, action: &str) -> bool {
        rows.iter().any(|(keys, label)| label == action && keys.split(", ").any(|item| item == key))
    }

    #[test]
    fn input_help_distinguishes_blocked_and_usable_input() {
        for (status, cause, usable) in [
            (AppStatus::Ready, None, true),
            (AppStatus::CommandPending, None, true),
            (AppStatus::CommandPending, Some("Preparing cedar session"), false),
            (AppStatus::Connecting, None, false),
            (AppStatus::Error, None, false),
        ] {
            let mut app = App::test_default();
            app.status = status;
            app.turn.pending_command_label = cause.map(str::to_owned);
            let rows = key_help_items(&app);
            assert_eq!(has_key(&rows, "Enter", "Send message"), usable, "{status:?}: {rows:?}");
            assert!(
                rows.iter().any(|(keys, _)| keys.contains("Ctrl+Q")),
                "exit control missing: {rows:?}"
            );
            if let Some(cause) = cause {
                assert!(
                    rows.iter().any(|(key, detail)| key == "Input keys" && detail.contains(cause))
                );
            }
        }
    }

    #[test]
    fn interaction_help_follows_real_request_focus_and_cancellation() {
        use crate::agent::{events::ClientEvent, model};
        use crate::app::keymap::{InteractionAction, default_bindings};
        use crate::app::{FocusTarget, test_support::*};
        for question in [false, true] {
            let (mut app, _commands) = app_with_connection();
            app.status = AppStatus::Ready;
            let context =
                if question { KeyContext::InlineQuestion } else { KeyContext::InlinePermission };
            let mut bindings = default_bindings();
            bindings.retain(|binding| {
                !(binding.context == context
                    && binding.action == KeyAction::Interaction(InteractionAction::MoveNext))
            });
            bindings.push(KeyBinding::new(
                context,
                "ctrl-n".parse().unwrap(),
                KeyAction::Interaction(InteractionAction::MoveNext),
                KeyBindingSource::Config,
            ));
            app.keymap = ResolvedKeymap::from_bindings(bindings).unwrap();
            admit_tool(
                &mut app,
                model::ToolCall::new("help-request", "Choose cedar")
                    .status(model::ToolCallStatus::InProgress),
            );
            let _permission;
            let _question;
            if question {
                let (sender, receiver) = tokio::sync::oneshot::channel();
                _question = Some(receiver);
                _permission = None;
                crate::app::handle_client_event(
                    &mut app,
                    ClientEvent::QuestionRequest {
                        session_id: "session-1".into(),
                        request: model::RequestQuestionRequest::new(
                            "session-1",
                            model::ToolCallUpdate::new(
                                "help-request",
                                model::ToolCallUpdateFields::new(),
                            ),
                            model::QuestionPrompt::new(
                                "Choose cedar",
                                "Tree",
                                true,
                                vec![
                                    model::QuestionOption::new("cedar", "Cedar"),
                                    model::QuestionOption::new("elm", "Elm"),
                                ],
                            ),
                            0,
                            1,
                        ),
                        response_tx: sender,
                    },
                );
            } else {
                _question = None;
                _permission = Some(request_permission(
                    &mut app,
                    model::RequestPermissionRequest::new(
                        "session-1",
                        model::ToolCallUpdate::new(
                            "help-request",
                            model::ToolCallUpdateFields::new(),
                        ),
                        permission_options("cedar"),
                        None,
                    ),
                ));
            }
            let rows = key_help_items(&app);
            assert!(has_key(&rows, "Ctrl+N", "Next option"), "wrong request context: {rows:?}");
            for key in ["Left", "Up"] {
                assert!(has_key(&rows, key, "Previous option"));
            }
            assert!(has_key(&rows, "Enter", "Confirm option"));
            assert!(has_key(&rows, "Esc", "Cancel prompt"));
            assert!(!has_key(&rows, "Enter", "Send message"));
            if question {
                assert!(has_key(&rows, "Space", "Toggle selection"));
                assert!(has_key(&rows, "Tab", "Toggle notes"));
                assert!(has_key(&rows, "Shift+Tab", "Toggle notes"));
            }
            app.release_focus_target(FocusTarget::Permission);
            let input = key_help_items(&app);
            assert!(has_key(&input, "Enter", "Send message"));
            assert!(has_key(&input, "Tab", "Focus pending prompt"));
            crate::app::handle_client_event(
                &mut app,
                ClientEvent::InteractionCancelled {
                    session_id: "session-1".into(),
                    interaction_id: "help-request".into(),
                },
            );
            let settled = key_help_items(&app);
            assert!(has_key(&settled, "Enter", "Send message"));
            assert!(!settled.iter().any(|(_, label)| label == "Focus pending prompt"));
        }
    }

    #[test]
    fn advertised_subagent_help_preserves_metadata_and_refreshes_inventory() {
        use crate::agent::model;
        use crate::app::test_support::{app_with_connection, deliver_update};
        let (mut app, _commands) = app_with_connection();
        app.status = AppStatus::Connecting;
        let loading = super::subagent_help_items(&app);
        assert!(!loading.is_empty());
        assert!(loading.iter().all(|(label, _)| !label.starts_with('&')));
        app.status = AppStatus::Ready;
        let mut auditor = model::AvailableAgent::new("auditor", "Inspect cedar regressions");
        auditor.model = Some("model-cedar".into());
        deliver_update(
            &mut app,
            model::SessionUpdate::AvailableAgentsUpdate(model::AvailableAgentsUpdate::new(vec![
                auditor,
                model::AvailableAgent::new("solo", "Own elm description"),
                model::AvailableAgent::new("quiet", "  "),
                model::AvailableAgent::new(" ", "discard this record"),
            ])),
        );
        let rows = super::help_items(&app, crate::app::ConfigHelpSection::Subagents);
        let auditor = rows.iter().find(|(label, _)| label.starts_with("&auditor\n")).unwrap();
        assert!(auditor.0.contains("model-cedar"));
        assert_eq!(auditor.1, "Inspect cedar regressions");
        assert!(has_row(&rows, "&solo", "Own elm description"));
        let quiet = rows.iter().find(|(label, _)| label == "&quiet").unwrap();
        assert!(!quiet.1.trim().is_empty());
        assert!(!rows.iter().any(|(_, description)| description == "discard this record"));
        deliver_update(
            &mut app,
            model::SessionUpdate::AvailableAgentsUpdate(model::AvailableAgentsUpdate::new(vec![])),
        );
        let empty = super::subagent_help_items(&app);
        assert!(empty.iter().all(|(label, _)| !label.starts_with('&')));
        assert!(empty.iter().any(|(_, detail)| !detail.trim().is_empty()));
        deliver_update(
            &mut app,
            model::SessionUpdate::AvailableAgentsUpdate(model::AvailableAgentsUpdate::new(vec![
                model::AvailableAgent::new("successor", "Fresh maple description"),
            ])),
        );
        let refreshed = super::subagent_help_items(&app);
        assert!(has_row(&refreshed, "&successor", "Fresh maple description"));
        assert!(!refreshed.iter().any(|(label, _)| label.starts_with("&auditor")
            || label == "&solo"
            || label == "&quiet"));
    }

    #[test]
    fn chat_input_shortcuts_reflect_resolved_keymap_changes() {
        let defaults = key_help_items(&App::test_default());
        assert!(has_key(&defaults, "Enter", "Send message"));
        for alias in ["Shift+Enter", "Ctrl+Enter"] {
            assert!(has_key(&defaults, alias, "Insert newline"));
        }
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
}
