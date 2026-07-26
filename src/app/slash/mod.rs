// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

//! Slash command types, parsing, and delegation.
//!
//! Submodules:
//! - `candidates`: candidate detection, filtering, and building
//! - `navigation`: autocomplete activation, movement, and confirm
//! - `executors`: slash command execution handlers

mod candidates;
mod catalog;
mod executors;
mod navigation;

use super::{
    App, AppStatus, ChatMessage, MessageBlock, MessageRole, SystemSeverity, TextBlock,
    dialog::DialogState,
};
use crate::agent::model;
use crate::app::events::push_system_message_with_severity;
use std::rc::Rc;

const MAX_CANDIDATES: usize = 50;
// Re-export public API
pub(crate) use catalog::{APP_SLASH_COMMANDS, AppSlashCommand, command_spec};
pub use executors::try_handle_submit;
pub use navigation::{
    confirm_selection, deactivate, move_down, move_up, sync_with_cursor, update_query,
};

#[derive(Debug, Clone)]
pub struct SlashCandidate {
    pub insert_value: String,
    pub primary: String,
    pub secondary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashContext {
    CommandName,
    Argument { command: String, arg_index: usize, token_range: (usize, usize) },
}

#[derive(Debug, Clone)]
pub struct SlashState {
    /// Character position where `/` token starts.
    pub trigger_row: usize,
    pub trigger_col: usize,
    /// Current typed query for the active slash context.
    pub query: String,
    /// Command-name or argument context.
    pub context: SlashContext,
    /// Filtered list of supported candidates.
    pub candidates: Vec<SlashCandidate>,
    /// Non-selectable line rendered when no candidates are available.
    pub placeholder: Option<String>,
    /// Shared autocomplete dialog navigation state.
    pub dialog: DialogState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SlashDetection {
    trigger_row: usize,
    trigger_col: usize,
    query: String,
    context: SlashContext,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedSlash<'a> {
    name: &'a str,
    args: Vec<&'a str>,
}

fn parse(text: &str) -> Option<ParsedSlash<'_>> {
    let trimmed = text.trim();
    if !trimmed.starts_with('/') {
        return None;
    }
    let mut parts = trimmed.split_whitespace();
    let name = parts.next()?;
    Some(ParsedSlash { name, args: parts.collect() })
}

pub fn is_cancel_command(text: &str) -> bool {
    parse(text).is_some_and(|parsed| parsed.name == "/cancel")
}

pub(crate) fn app_owned_command_name(name: &str) -> Option<&'static str> {
    let normalized = normalize_slash_name(name.trim());
    command_spec(&normalized).map(|spec| spec.name)
}

fn normalize_slash_name(name: &str) -> String {
    if name.starts_with('/') { name.to_owned() } else { format!("/{name}") }
}

fn push_system_message(app: &mut App, text: impl Into<String>) {
    let text = text.into();
    push_system_message_with_severity(app, Some(SystemSeverity::Error), &text);
}

fn push_user_message(app: &mut App, text: impl Into<String>) {
    let text = text.into();
    app.push_message_tracked(ChatMessage::new(
        MessageRole::User,
        vec![MessageBlock::Text(TextBlock::from_complete(&text))],
        None,
    ));
    app.enforce_history_retention_tracked();
}

fn require_connection(
    app: &mut App,
    not_connected_msg: &'static str,
) -> Option<Rc<crate::agent::client::AgentConnection>> {
    let Some(conn) = app.session_runtime.conn.as_ref() else {
        push_system_message(app, not_connected_msg);
        return None;
    };
    Some(Rc::clone(conn))
}

fn require_active_session(
    app: &mut App,
    not_connected_msg: &'static str,
    no_session_msg: &'static str,
) -> Option<(Rc<crate::agent::client::AgentConnection>, model::SessionId)> {
    let conn = require_connection(app, not_connected_msg)?;
    let Some(session_id) = app.session_runtime.session_id.clone() else {
        push_system_message(app, no_session_msg);
        return None;
    };
    Some((conn, session_id))
}

/// Block the input field while a slash command is in flight.
fn set_command_pending(app: &mut App, label: &str, ack: Option<super::PendingCommandAck>) {
    app.status = AppStatus::CommandPending;
    app.turn.pending_command_label = Some(label.to_owned());
    app.turn.pending_command_ack = ack;
}

#[cfg(test)]
mod tests;
