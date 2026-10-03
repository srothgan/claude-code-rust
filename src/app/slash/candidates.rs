// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

//! Slash command candidate detection, filtering, and building.

use super::{
    APP_SLASH_COMMANDS, AppSlashCommand, CompletionRequest, SlashCandidate, SlashCommandToken,
    SlashContext, SlashDetection, SlashState, SubmissionClass, command_spec, normalize_slash_name,
};
use crate::agent::model::EffortLevel;
use crate::app::App;
use crate::app::dialog::DialogState;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_ARGUMENT_CANDIDATES: usize = 50;

pub(super) fn detect_argument_at_cursor(
    chars: &[char],
    mut idx: usize,
    cursor_col: usize,
) -> Option<(usize, usize, usize)> {
    if cursor_col > chars.len() {
        return None;
    }

    let mut arg_index = 0usize;
    loop {
        while idx < chars.len() && chars[idx].is_whitespace() {
            if cursor_col == idx {
                return Some((arg_index, cursor_col, cursor_col));
            }
            idx += 1;
        }

        if idx >= chars.len() {
            if cursor_col >= idx {
                return Some((arg_index, cursor_col, cursor_col));
            }
            return None;
        }

        let token_start = idx;
        while idx < chars.len() && !chars[idx].is_whitespace() {
            idx += 1;
        }
        let token_end = idx;
        if (token_start..=token_end).contains(&cursor_col) {
            return Some((arg_index, token_start, token_end));
        }
        arg_index += 1;
    }
}

pub(super) fn detect_slash_at_cursor(
    lines: &[String],
    cursor_row: usize,
    cursor_col: usize,
) -> Option<SlashDetection> {
    let line = lines.get(cursor_row)?;
    // Submission recognizes a command only at the start of the message.
    // Later lines are prompt/argument text, even if they begin with a slash.
    if lines[..cursor_row].iter().any(|line| !line.trim().is_empty()) {
        return None;
    }
    let chars: Vec<char> = line.chars().collect();
    let first_non_ws = chars.iter().position(|c| !c.is_whitespace())?;
    if chars.get(first_non_ws).copied() != Some('/') {
        return None;
    }

    let token_start = first_non_ws;
    let token_end =
        (token_start + 1..chars.len()).find(|&i| chars[i].is_whitespace()).unwrap_or(chars.len());

    if cursor_col <= token_start || cursor_col > chars.len() {
        return None;
    }

    let command: String = chars[token_start..token_end].iter().collect();
    let command_token = SlashCommandToken { row: cursor_row, name: command.clone() };

    if cursor_col <= token_end {
        let query: String = chars[token_start + 1..cursor_col].iter().collect();
        if query.chars().any(char::is_whitespace) {
            return None;
        }
        return Some(SlashDetection {
            command_token,
            trigger_col: token_start,
            query,
            context: SlashContext::CommandName,
        });
    }

    let (arg_index, token_start, token_end) =
        detect_argument_at_cursor(&chars, token_end, cursor_col)?;
    let query: String = chars[token_start..cursor_col.min(token_end)].iter().collect();

    Some(SlashDetection {
        command_token,
        trigger_col: token_start,
        query,
        context: SlashContext::Argument {
            command,
            arg_index,
            token_range: (token_start, token_end),
        },
    })
}

fn advertised_commands(app: &App) -> Vec<String> {
    app.sdk_inventory
        .available_commands
        .iter()
        .flat_map(|cmd| {
            std::iter::once(cmd.name.as_str())
                .chain(cmd.aliases.iter().map(String::as_str))
                .map(normalize_slash_name)
        })
        .filter(|name| command_spec(name).is_none())
        .collect()
}

pub(super) fn find_advertised_command<'a>(
    app: &'a App,
    command_name: &str,
) -> Option<&'a crate::agent::model::AvailableCommand> {
    if command_spec(command_name).is_some() {
        return None;
    }
    let canonical = app
        .sdk_inventory
        .available_commands
        .iter()
        .filter(|cmd| normalize_slash_name(&cmd.name) == command_name);
    if let Some(command) = canonical.clone().find(|cmd| cmd.builtin) {
        return Some(command);
    }
    if let Some(command) = canonical.into_iter().next() {
        return Some(command);
    }
    app.sdk_inventory
        .available_commands
        .iter()
        .find(|cmd| cmd.aliases.iter().any(|alias| normalize_slash_name(alias) == command_name))
}

fn is_builtin_variable_input_command(command_name: &str) -> bool {
    command_spec(command_name).is_some_and(|spec| {
        !spec.args.is_empty()
            || matches!(
                spec.command,
                AppSlashCommand::Agent
                    | AppSlashCommand::Effort
                    | AppSlashCommand::Mode
                    | AppSlashCommand::Model
                    | AppSlashCommand::Resume
                    | AppSlashCommand::Rewind
            )
    })
}

pub(super) fn builtin_argument_confirmation_closes(command_name: &str, arg_index: usize) -> bool {
    if command_name == "/rewind" {
        return arg_index == 1;
    }
    arg_index == 0 && is_builtin_variable_input_command(command_name)
}

pub(super) fn is_variable_input_command(app: &App, command_name: &str) -> bool {
    if is_builtin_variable_input_command(command_name) {
        return true;
    }

    find_advertised_command(app, command_name)
        .and_then(|cmd| cmd.input_hint.as_ref())
        .is_some_and(|hint| !hint.trim().is_empty())
}

pub(super) fn supported_command_candidates(app: &App) -> Vec<SlashCandidate> {
    use std::collections::BTreeMap;

    let mut by_name: BTreeMap<String, String> = BTreeMap::new();
    for spec in APP_SLASH_COMMANDS {
        by_name.insert(spec.name.to_owned(), spec.short_description.to_owned());
    }

    for cmd in &app.sdk_inventory.available_commands {
        for name in std::iter::once(cmd.name.as_str()).chain(cmd.aliases.iter().map(String::as_str))
        {
            let name = normalize_slash_name(name);
            if command_spec(&name).is_some() {
                continue;
            }
            let description = find_advertised_command(app, &name)
                .map_or_else(|| cmd.description.clone(), |selected| selected.description.clone());
            by_name.insert(name, description);
        }
    }

    by_name
        .into_iter()
        .map(|(name, description)| SlashCandidate {
            insert_value: name.clone(),
            primary: name,
            secondary: if description.trim().is_empty() { None } else { Some(description) },
        })
        .collect()
}

pub(super) fn filter_command_candidates(
    candidates: &[SlashCandidate],
    query: &str,
) -> Vec<SlashCandidate> {
    if query.is_empty() {
        return candidates.to_vec();
    }

    let query_lower = query.to_lowercase();
    let mut matches = candidates
        .iter()
        .filter(|candidate| {
            let body = candidate.primary.strip_prefix('/').unwrap_or(&candidate.primary);
            body.to_lowercase().contains(&query_lower)
        })
        .collect::<Vec<_>>();
    // An exact command must precede substring matches before Enter can submit.
    // Keep all other matches in inventory order for deliberate arrow selection.
    matches.sort_by_key(|candidate| {
        candidate.primary.strip_prefix('/').unwrap_or(&candidate.primary).to_lowercase()
            != query_lower
    });
    matches.into_iter().cloned().collect()
}

fn candidate_matches(candidate: &SlashCandidate, query_lower: &str) -> bool {
    candidate.primary.to_lowercase().contains(query_lower)
        || candidate.insert_value.to_lowercase().contains(query_lower)
        || candidate
            .secondary
            .as_ref()
            .is_some_and(|secondary| secondary.to_lowercase().contains(query_lower))
}

pub(super) fn filter_argument_candidates(
    candidates: &[SlashCandidate],
    query: &str,
) -> Vec<SlashCandidate> {
    if query.is_empty() {
        return candidates.iter().take(MAX_ARGUMENT_CANDIDATES).cloned().collect();
    }

    let query_lower = query.to_lowercase();
    let mut matches = candidates
        .iter()
        .filter(|candidate| candidate_matches(candidate, &query_lower))
        .collect::<Vec<_>>();
    // A fully typed value must precede description-only matches: e.g. the
    // description of rewind's "both" also contains the word "conversation".
    matches.sort_by_key(|candidate| candidate.insert_value.to_lowercase() != query_lower);
    matches.into_iter().take(MAX_ARGUMENT_CANDIDATES).cloned().collect()
}

fn static_argument_candidates(command_name: &str) -> Vec<SlashCandidate> {
    command_spec(command_name).map_or_else(Vec::new, |spec| {
        spec.args
            .iter()
            .map(|arg| SlashCandidate {
                insert_value: arg.value.to_owned(),
                primary: arg.value.to_owned(),
                secondary: Some(arg.description.to_owned()),
            })
            .collect()
    })
}

fn effort_argument_candidates(app: &App) -> Vec<SlashCandidate> {
    let mut levels = match app.session_runtime.current_model.as_ref() {
        Some(model) if !model.supports_effort => Vec::new(),
        Some(model) if !model.supported_effort_levels.is_empty() => {
            model.supported_effort_levels.clone()
        }
        _ => EffortLevel::ALL.to_vec(),
    };
    if !levels.contains(&EffortLevel::Max) {
        levels.push(EffortLevel::Max);
    }

    levels
        .into_iter()
        .map(|level| SlashCandidate {
            insert_value: level.as_stored().to_owned(),
            primary: level.as_stored().to_owned(),
            secondary: Some(format!("{} - {}", level.label(), level.description())),
        })
        .collect()
}

fn agent_argument_candidates(app: &App) -> Vec<SlashCandidate> {
    let mut candidates = Vec::with_capacity(app.sdk_inventory.available_agents.len() + 1);
    candidates.push(SlashCandidate {
        insert_value: "reset".to_owned(),
        primary: "reset".to_owned(),
        secondary: Some("Clear active agent".to_owned()),
    });
    candidates.extend(app.sdk_inventory.available_agents.iter().map(|agent| {
        let description = agent.description.trim();
        let model = agent.model.as_deref().map(str::trim).filter(|model| !model.is_empty());
        let secondary = match (description.is_empty(), model) {
            (false, Some(model)) => Some(format!("{description} - {model}")),
            (false, None) => Some(description.to_owned()),
            (true, Some(model)) => Some(format!("Model: {model}")),
            (true, None) => None,
        };
        SlashCandidate { insert_value: agent.name.clone(), primary: agent.name.clone(), secondary }
    }));
    candidates
}

fn now_epoch_seconds() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_secs()).unwrap_or(i64::MAX),
        Err(_) => 0,
    }
}

fn format_relative_age(epoch_seconds: i64) -> String {
    let now_seconds = now_epoch_seconds();
    let delta_seconds = if now_seconds >= epoch_seconds {
        now_seconds - epoch_seconds
    } else {
        epoch_seconds - now_seconds
    };

    if delta_seconds < 5 * 60 {
        return "<5m".to_owned();
    }
    if delta_seconds < 60 * 60 {
        return format!("{}m", delta_seconds / 60);
    }
    if delta_seconds < 24 * 60 * 60 {
        return format!("{}h", delta_seconds / (60 * 60));
    }

    let total_hours = delta_seconds / (60 * 60);
    let days = total_hours / 24;
    let hours = total_hours % 24;
    format!("{days}d {hours}h")
}

fn session_age_label(last_modified_ms: Option<u64>) -> String {
    let Some(last_modified_ms) = last_modified_ms else {
        return "--".to_owned();
    };
    let epoch = i64::try_from(last_modified_ms / 1_000).ok();
    let Some(epoch) = epoch else {
        return "--".to_owned();
    };
    format_relative_age(epoch)
}

pub(super) fn argument_candidates(
    app: &App,
    command_name: &str,
    arg_index: usize,
) -> Vec<SlashCandidate> {
    if arg_index > 0 && !(command_name == "/rewind" && arg_index == 1) {
        return Vec::new();
    }

    match command_name {
        "/docs" | "/ultracode" => static_argument_candidates(command_name),
        "/agent" => agent_argument_candidates(app),
        "/effort" => effort_argument_candidates(app),
        "/resume" => app
            .recent_sessions
            .iter()
            .map(|session| {
                let summary = session.summary.trim();
                let summary = if summary.is_empty() { "(no summary)" } else { summary };
                let age = session_age_label(Some(session.last_modified_ms));
                SlashCandidate {
                    insert_value: session.session_id.clone(),
                    primary: format!("{age} - {summary}"),
                    secondary: Some(session.session_id.clone()),
                }
            })
            .collect(),
        "/rewind" => {
            if arg_index == 1 {
                return rewind_restore_mode_candidates();
            }
            if app.session_runtime.session_id.as_ref()
                == app.sdk_inventory.rewind_targets_session_id.as_ref()
            {
                app.sdk_inventory
                    .rewind_targets
                    .iter()
                    .map(|target| SlashCandidate {
                        insert_value: target.uuid.clone(),
                        primary: truncate_rewind_label(&target.first_text),
                        secondary: Some(target.uuid.clone()),
                    })
                    .collect()
            } else {
                Vec::new()
            }
        }
        "/mode" => app
            .session_runtime
            .mode
            .as_ref()
            .map(|mode| {
                mode.available_modes
                    .iter()
                    .map(|entry| SlashCandidate {
                        insert_value: entry.id.clone(),
                        primary: entry.name.clone(),
                        secondary: Some(entry.id.clone()),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        "/model" => app
            .sdk_inventory
            .available_models
            .iter()
            .map(|model| SlashCandidate {
                insert_value: model.id.clone(),
                primary: model.display_name.clone(),
                secondary: model
                    .description
                    .clone()
                    .or_else(|| (model.display_name != model.id).then(|| model.id.clone())),
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn rewind_restore_mode_candidates() -> Vec<SlashCandidate> {
    vec![
        SlashCandidate {
            insert_value: "both".to_owned(),
            primary: "Restore code and conversation".to_owned(),
            secondary: Some("revert both code and conversation to that point".to_owned()),
        },
        SlashCandidate {
            insert_value: "conversation".to_owned(),
            primary: "Restore conversation".to_owned(),
            secondary: Some("rewind to that message while keeping current code".to_owned()),
        },
        SlashCandidate {
            insert_value: "code".to_owned(),
            primary: "Restore code".to_owned(),
            secondary: Some("revert file changes while keeping the conversation".to_owned()),
        },
    ]
}

fn rewind_placeholder(app: &App, query: &str) -> String {
    if app.sdk_inventory.rewind_targets_in_flight {
        return "Loading messages".to_owned();
    }

    let Some(session_id) = app.session_runtime.session_id.as_ref() else {
        return "Connect to load messages".to_owned();
    };

    if app.sdk_inventory.rewind_targets_session_id.as_ref() != Some(session_id) {
        if app.session_runtime.conn.is_none() {
            return "Connect to load messages".to_owned();
        }
        return "Loading messages".to_owned();
    }

    if app.sdk_inventory.rewind_targets.is_empty() || query.trim().is_empty() {
        "No previous user messages".to_owned()
    } else {
        "No matching messages".to_owned()
    }
}

fn placeholder_for_empty_candidates(
    app: &App,
    detection: &SlashDetection,
    candidates: &[SlashCandidate],
) -> Option<String> {
    if !candidates.is_empty() {
        return None;
    }

    match &detection.context {
        SlashContext::Argument { command, arg_index, .. }
            if command == "/rewind" && *arg_index == 0 =>
        {
            Some(rewind_placeholder(app, &detection.query))
        }
        SlashContext::Argument { command, arg_index, .. }
            if command == "/rewind" && *arg_index == 1 =>
        {
            Some("Select restore mode".to_owned())
        }
        SlashContext::Argument { command, .. } => {
            command_spec(command).map(|spec| spec.usage.to_owned()).or_else(|| {
                find_advertised_command(app, command)
                    .and_then(|command| command.input_hint.as_ref())
                    .map(|hint| format!("Arguments: {hint}"))
            })
        }
        SlashContext::CommandName => Some("No matching commands".to_owned()),
    }
}

fn truncate_rewind_label(text: &str) -> String {
    const MAX_CHARS: usize = 80;
    let text = text.trim();
    let mut chars = text.chars();
    let mut label: String = chars.by_ref().take(MAX_CHARS).collect();
    if chars.next().is_some() {
        label.push_str("...");
    }
    if label.is_empty() { "(empty user message)".to_owned() } else { label }
}

pub(super) fn build_slash_state(
    app: &App,
    detection: SlashDetection,
    request: CompletionRequest,
) -> Option<SlashState> {
    let candidates = match &detection.context {
        SlashContext::CommandName => {
            filter_command_candidates(&supported_command_candidates(app), &detection.query)
        }
        SlashContext::Argument { command, arg_index, .. } => {
            // App validation owns whether an argument is required. SDK hints
            // are opaque help text; they never impose local arity rules.
            let requires_arguments = command_spec(command)
                .is_some_and(|spec| spec.command.submission_class(&[]) == SubmissionClass::Invalid);
            if request == CompletionRequest::Automatic && !requires_arguments {
                return None;
            }
            if !is_variable_input_command(app, command) {
                return None;
            }
            filter_argument_candidates(
                &argument_candidates(app, command, *arg_index),
                &detection.query,
            )
        }
    };
    let placeholder = placeholder_for_empty_candidates(app, &detection, &candidates);
    Some(SlashState {
        trigger_row: detection.command_token.row,
        trigger_col: detection.trigger_col,
        query: detection.query,
        context: detection.context,
        candidates,
        placeholder,
        dialog: DialogState::default(),
    })
}

pub fn is_supported_command(app: &App, command_name: &str) -> bool {
    command_spec(command_name).is_some()
        || advertised_commands(app).iter().any(|c| c == command_name)
}
