// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

mod api_retry;
mod client;
mod notices;
mod rate_limit;
mod retraction;
mod session;
mod session_reset;
mod streaming;
mod tool_calls;
mod tool_updates;
mod turn;

use super::{
    App, AppStatus, ChatMessage, FullscreenView, InvalidationLevel, MessageBlock, MessageRole,
    PendingCommandAck, SurfaceMode, SystemSeverity, TerminalSizeChange, TextBlock,
};
use crate::agent::model;
#[cfg(all(test, target_os = "macos"))]
use crate::app::keys::CMD_MOD;
#[cfg(test)]
use crate::app::keys::WORD_NAV_MOD;
use crate::app::keys::{KeyOutcome, RuntimeCommand, reclaim_input_from_inline_prompt_if_needed};
use crate::app::tasks::apply_task_state_update;
#[cfg(test)]
use crossterm::event::KeyEvent;
use crossterm::event::{Event, KeyEventKind};

pub use client::handle_client_event;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TerminalEventOutcome {
    changed: bool,
    runtime_command: Option<RuntimeCommand>,
}

impl TerminalEventOutcome {
    pub(crate) fn runtime_command(self) -> Option<RuntimeCommand> {
        self.runtime_command
    }

    fn ignored() -> Self {
        Self { changed: false, runtime_command: None }
    }

    fn handled(changed: bool) -> Self {
        Self { changed, runtime_command: None }
    }

    fn from_key_outcome(outcome: KeyOutcome) -> Self {
        Self { changed: outcome.changed(), runtime_command: outcome.runtime_command() }
    }
}

pub fn handle_terminal_event(app: &mut App, event: Event) -> TerminalEventOutcome {
    if matches!(app.terminal_lifecycle, super::TerminalLifecycleState::ReleasedToChild(_))
        && !matches!(&event, Event::Resize(_, _))
    {
        return TerminalEventOutcome::ignored();
    }

    let outcome = match event {
        Event::Key(key) if should_dispatch_key_event(key) => dispatch_key_by_view(app, key),
        Event::Mouse(mouse) => {
            dispatch_mouse_by_view(app, mouse);
            TerminalEventOutcome::handled(true)
        }
        Event::Paste(text) => TerminalEventOutcome::handled(dispatch_paste_by_view(app, &text)),
        Event::FocusGained => {
            app.notifications.on_focus_gained();
            app.sync_git_context();
            TerminalEventOutcome::handled(true)
        }
        Event::FocusLost => {
            app.notifications.on_focus_lost();
            TerminalEventOutcome::handled(true)
        }
        Event::Resize(width, height) => {
            TerminalEventOutcome::handled(handle_resize(app, width, height))
        }
        // Non-press key events (Release, Repeat) -- ignored.
        Event::Key(_) => TerminalEventOutcome::ignored(),
    };
    if outcome.changed {
        app.request_active_surface_repaint();
    }
    outcome
}

fn should_dispatch_key_event(key: crossterm::event::KeyEvent) -> bool {
    key.kind == KeyEventKind::Press
        || (key.kind == KeyEventKind::Release && super::keys::is_clipboard_paste_shortcut(key))
}

fn handle_resize(app: &mut App, width: u16, height: u16) -> bool {
    let size_change = app.chat_render.observe_terminal_size(width, height);
    let mut active_surface_repaint = false;
    let action = match size_change {
        TerminalSizeChange::Unchanged { .. } => "ignored_same_size",
        TerminalSizeChange::Initial { .. } => {
            app.chat_render.clear_measurements();
            app.chat_render.invalidate_live_anchor();
            match app.terminal_lifecycle {
                super::TerminalLifecycleState::Running(super::SurfaceMode::Chat) => {
                    app.request_chat_visible_rebuild();
                    active_surface_repaint = true;
                    "record_initial_chat_size"
                }
                super::TerminalLifecycleState::Running(super::SurfaceMode::Fullscreen(_)) => {
                    app.request_fullscreen_repaint();
                    active_surface_repaint = true;
                    "record_initial_fullscreen_size"
                }
                super::TerminalLifecycleState::Bootstrapping
                | super::TerminalLifecycleState::ReleasedToChild(_)
                | super::TerminalLifecycleState::Restoring
                | super::TerminalLifecycleState::Exited => "record_initial_hidden_size",
            }
        }
        TerminalSizeChange::Changed { .. } => {
            app.chat_render.clear_measurements();
            app.chat_render.invalidate_live_anchor();
            match app.terminal_lifecycle {
                super::TerminalLifecycleState::Running(super::SurfaceMode::Chat) => {
                    app.request_chat_purge_replay_rebuild(super::ChatPurgeReplayOptions::resize());
                    if matches!(app.status, AppStatus::Thinking | AppStatus::Running) {
                        app.chat_render.mark_resize_purge_replay_during_turn();
                    }
                    active_surface_repaint = true;
                    "request_chat_purge_replay"
                }
                super::TerminalLifecycleState::Running(super::SurfaceMode::Fullscreen(_)) => {
                    app.request_fullscreen_repaint();
                    app.chat_render.mark_resize_purge_replay_on_chat_return();
                    active_surface_repaint = true;
                    "defer_chat_resize_purge_until_return"
                }
                super::TerminalLifecycleState::Bootstrapping
                | super::TerminalLifecycleState::ReleasedToChild(_)
                | super::TerminalLifecycleState::Restoring
                | super::TerminalLifecycleState::Exited => "record_hidden_size_change",
            }
        }
    };
    log_resize_classification(app, size_change, action);
    active_surface_repaint
}

fn log_resize_classification(app: &App, size_change: TerminalSizeChange, action: &'static str) {
    let previous = size_change.previous();
    let current = size_change.current();
    let event_name = if matches!(size_change, TerminalSizeChange::Unchanged { .. }) {
        "terminal_resize_same_size_ignored"
    } else {
        "terminal_resize_classified"
    };
    tracing::debug!(
        target: crate::logging::targets::APP_RENDER,
        event_name = event_name,
        message = "terminal resize event classified before surface rebuild",
        outcome = "success",
        classification = size_change.label(),
        action,
        previous_width = ?previous.map(|size| size.width),
        previous_height = ?previous.map(|size| size.height),
        current_width = current.width,
        current_height = current.height,
        lifecycle = ?app.terminal_lifecycle,
        surface_mode = ?app.surface_mode,
    );
}

fn dispatch_key_by_view(app: &mut App, key: crossterm::event::KeyEvent) -> TerminalEventOutcome {
    match app.surface_mode {
        SurfaceMode::Chat => {
            app.paste.clear_active_session();
            TerminalEventOutcome::from_key_outcome(super::keys::dispatch_key_by_focus(app, key))
        }
        SurfaceMode::Fullscreen(FullscreenView::Config) => {
            super::config::handle_key(app, key);
            TerminalEventOutcome::handled(true)
        }
        SurfaceMode::Fullscreen(FullscreenView::Trusted) => {
            super::trust::handle_key(app, key);
            TerminalEventOutcome::handled(true)
        }
        SurfaceMode::Fullscreen(FullscreenView::SessionPicker) => {
            super::session_picker::handle_key(app, key);
            TerminalEventOutcome::handled(true)
        }
    }
}

fn dispatch_mouse_by_view(app: &mut App, mouse: crossterm::event::MouseEvent) {
    match app.surface_mode {
        SurfaceMode::Chat => {
            app.paste.clear_active_session();
            let _ = mouse;
        }
        SurfaceMode::Fullscreen(_) => {
            let _ = mouse;
        }
    }
}

fn dispatch_paste_by_view(app: &mut App, text: &str) -> bool {
    match app.surface_mode {
        SurfaceMode::Chat => {
            if !matches!(
                app.status,
                AppStatus::Connecting | AppStatus::CommandPending | AppStatus::Error
            ) {
                reclaim_input_from_inline_prompt_if_needed(app);
                app.queue_paste_text(text);
                return true;
            }
            false
        }
        SurfaceMode::Fullscreen(FullscreenView::Config) => super::config::handle_paste(app, text),
        SurfaceMode::Fullscreen(FullscreenView::Trusted | FullscreenView::SessionPicker) => false,
    }
}

fn handle_session_update_event(app: &mut App, update: model::SessionUpdate) {
    let needs_history_retention = matches!(
        &update,
        model::SessionUpdate::AgentMessageChunk(_)
            | model::SessionUpdate::ToolCall(_)
            | model::SessionUpdate::ToolCallUpdate(_)
            | model::SessionUpdate::TranscriptRetraction(_)
            | model::SessionUpdate::CompactionBoundary(_)
    );
    handle_session_update(app, update);
    if needs_history_retention {
        app.enforce_history_retention_tracked();
    }
}

#[allow(clippy::too_many_lines)]
fn handle_session_update(app: &mut App, update: model::SessionUpdate) {
    match update {
        model::SessionUpdate::AgentMessageChunk(chunk) => {
            clear_compaction_state(app, true);
            streaming::handle_agent_message_chunk(app, chunk);
        }
        model::SessionUpdate::ToolCall(tc) => tool_calls::handle_tool_call(app, tc),
        model::SessionUpdate::ToolCallUpdate(tcu) => {
            tool_updates::handle_tool_call_update_session(app, &tcu);
        }
        model::SessionUpdate::TranscriptRetraction(retraction) => {
            retraction::handle_transcript_retraction(app, &retraction);
        }
        model::SessionUpdate::TaskStateUpdate(update) => {
            tracing::debug!(
                target: crate::logging::targets::APP_SESSION,
                event_name = "task_state_update_applied",
                message = "task state update applied",
                outcome = "success",
                task_count = update.tasks.len(),
                removed_task_count = update.removed_task_ids.len(),
                complete_snapshot = update.is_complete_snapshot,
            );
            apply_task_state_update(app, update);
        }
        model::SessionUpdate::UserMessageChunk(_) => {}
        model::SessionUpdate::AgentThoughtChunk(chunk) => {
            let chunk_chars = match &chunk.content {
                model::ContentBlock::Text(text) => text.text.chars().count(),
                model::ContentBlock::Image(_) => 0,
            };
            tracing::trace!(
                target: crate::logging::targets::APP_SESSION,
                event_name = "agent_thought_chunk_applied",
                message = "agent thought chunk applied",
                outcome = "success",
                chunk_chars,
            );
            app.status = AppStatus::Thinking;
        }
        model::SessionUpdate::AvailableCommandsUpdate(cmds) => {
            tracing::debug!(
                target: crate::logging::targets::APP_SESSION,
                event_name = "available_commands_applied",
                message = "available commands update applied",
                outcome = "success",
                command_count = cmds.available_commands.len(),
                source = cmds.source.as_deref().unwrap_or("unknown"),
                generation = cmds.generation,
            );
            app.sdk_inventory.available_commands = cmds.available_commands;
            crate::app::plugins::clamp_selection(app);
            if app.slash.is_some() {
                super::slash::update_query(app);
            }
        }
        model::SessionUpdate::AvailableAgentsUpdate(agents) => {
            tracing::debug!(
                target: crate::logging::targets::APP_SESSION,
                event_name = "available_agents_applied",
                message = "available agents update applied",
                outcome = "success",
                agent_count = agents.available_agents.len(),
            );
            app.sdk_inventory.available_agents = agents.available_agents;
            if app.subagent.is_some() {
                super::subagent::update_query(app);
            }
        }
        model::SessionUpdate::ModeStateUpdate(mode) => {
            let mode_changed =
                app.session_runtime.mode.as_ref().map(|current| current.current_mode_id.as_str())
                    != Some(mode.current_mode_id.as_str());
            app.session_runtime.mode = Some(mode);
            if mode_changed {
                app.invalidate_layout(InvalidationLevel::Global);
            }
            if matches!(app.turn.pending_command_ack, Some(PendingCommandAck::CurrentMode)) {
                session::clear_pending_command(app);
            }
        }
        model::SessionUpdate::CurrentModeUpdate(update) => {
            let mode_id = update.current_mode_id.to_string();
            let mut mode_changed = false;
            if let Some(ref mut mode) = app.session_runtime.mode {
                mode_changed = mode.current_mode_id != mode_id;
                if let Some(info) = mode.available_modes.iter().find(|m| m.id == mode_id) {
                    mode.current_mode_name.clone_from(&info.name);
                    mode.current_mode_id = mode_id;
                } else {
                    mode.current_mode_name.clone_from(&mode_id);
                    mode.current_mode_id = mode_id;
                }
            }
            if mode_changed {
                app.invalidate_layout(InvalidationLevel::Global);
            }
            if matches!(app.turn.pending_command_ack, Some(PendingCommandAck::CurrentMode)) {
                session::clear_pending_command(app);
            }
        }
        model::SessionUpdate::CurrentModelUpdate(update) => {
            let next_resolved_id = update.current_model.resolved_id.clone();
            let next_display_short = update.current_model.display_name_short.clone();
            let next_display_long = update.current_model.display_name_long.clone();
            let pending_ack_before = format!("{:?}", app.turn.pending_command_ack);
            app.session_runtime.current_model = Some(update.current_model);
            let clearing_pending =
                matches!(app.turn.pending_command_ack, Some(PendingCommandAck::CurrentModel));
            if matches!(app.turn.pending_command_ack, Some(PendingCommandAck::CurrentModel)) {
                session::clear_pending_command(app);
            }
            tracing::debug!(
                target: crate::logging::targets::APP_SESSION,
                event_name = "current_model_update_applied",
                message = "current model update applied",
                outcome = "success",
                resolved_id = %next_resolved_id,
                display_name_short = %next_display_short,
                display_name_long = %next_display_long,
                clearing_pending = clearing_pending,
                pending_ack_before = %pending_ack_before,
            );
        }
        model::SessionUpdate::ConfigOptionUpdate(config) => {
            handle_config_option_update(app, config);
        }
        model::SessionUpdate::FastModeUpdate(state) => {
            app.session_runtime.fast_mode_state = state;
        }
        model::SessionUpdate::RateLimitUpdate(update) => {
            rate_limit::handle_rate_limit_update(app, &update);
        }
        model::SessionUpdate::ApiRetryUpdate {
            attempt,
            max_retries,
            retry_delay_ms,
            error_status,
            error,
        } => {
            api_retry::handle_api_retry_update(
                app,
                attempt,
                max_retries,
                retry_delay_ms,
                error_status,
                error,
            );
        }
        model::SessionUpdate::PromptSuggestionUpdate(suggestion) => {
            app.session_runtime.prompt_suggestion =
                (!suggestion.trim().is_empty()).then_some(suggestion);
        }
        model::SessionUpdate::RuntimeSessionStateUpdate(state) => {
            handle_runtime_session_state_update(app, state);
        }
        model::SessionUpdate::SettingsParseError { file, path, message } => {
            handle_settings_parse_error(app, file.as_deref(), &path, &message);
        }
        model::SessionUpdate::SessionStatusUpdate(status) => {
            // TODO(runtime-verification): confirm in real SDK sessions that compaction
            // status updates are emitted consistently; if not, add a fallback indicator.
            let was_compacting = app.turn.is_compacting;
            if matches!(status, model::SessionStatus::Compacting) {
                app.turn.is_compacting = true;
            } else {
                clear_compaction_state(app, true);
            }
            if was_compacting && matches!(status, model::SessionStatus::Idle) {
                crate::app::session_runtime::request_context_usage_refresh(app);
            }
            tracing::debug!(
                target: crate::logging::targets::APP_SESSION,
                event_name = "session_status_applied",
                message = "session status update applied",
                outcome = "success",
                session_status = ?status,
                compacting = app.turn.is_compacting,
            );
        }
        model::SessionUpdate::SystemNoticeUpdate { severity, message } => {
            let severity = match severity {
                model::SystemNoticeSeverity::Info => SystemSeverity::Info,
                model::SystemNoticeSeverity::Warning => SystemSeverity::Warning,
                model::SystemNoticeSeverity::Error => SystemSeverity::Error,
            };
            notices::emit_system_notice(app, severity, &message);
        }
        model::SessionUpdate::CompactionBoundary(boundary) => {
            rate_limit::handle_compaction_boundary_update(app, boundary);
        }
    }
}

fn handle_runtime_session_state_update(app: &mut App, state: model::RuntimeSessionState) {
    app.session_runtime.runtime_session_state = Some(state);
    match state {
        model::RuntimeSessionState::Running => {
            if matches!(app.status, AppStatus::Ready | AppStatus::Thinking | AppStatus::Running)
                && !app.turn.is_compacting
            {
                app.status = AppStatus::Running;
            }
        }
        model::RuntimeSessionState::RequiresAction => {}
        model::RuntimeSessionState::Idle => {
            if matches!(app.status, AppStatus::Thinking | AppStatus::Running)
                && !app.turn.is_compacting
                && app.turn.pending_cancel_origin.is_none()
            {
                app.status = AppStatus::Ready;
            }
        }
    }
}

fn handle_settings_parse_error(app: &mut App, file: Option<&str>, path: &str, message: &str) {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return;
    }
    let rendered = match (file.filter(|value| !value.trim().is_empty()), path.trim()) {
        (Some(file), "") => format!("Settings parse error in {file}: {trimmed}"),
        (Some(file), path) => format!("Settings parse error in {file} at {path}: {trimmed}"),
        (None, "") => format!("Settings parse error: {trimmed}"),
        (None, path) => format!("Settings parse error at {path}: {trimmed}"),
    };
    push_system_message_with_severity(app, Some(SystemSeverity::Error), &rendered);
}

pub(crate) fn push_system_message_with_severity(
    app: &mut App,
    severity: Option<SystemSeverity>,
    message: &str,
) {
    app.push_message_tracked(ChatMessage::new(
        MessageRole::System(severity),
        vec![MessageBlock::Text(TextBlock::from_complete(message))],
        None,
    ));
    app.enforce_history_retention_tracked();
}

pub(super) fn clear_compaction_state(app: &mut App, emit_manual_success: bool) {
    if !app.turn.is_compacting && !app.turn.pending_compact_clear {
        return;
    }
    let should_emit_success = emit_manual_success && app.turn.pending_compact_clear;
    app.turn.pending_compact_clear = false;
    app.turn.is_compacting = false;
    if should_emit_success {
        push_system_message_with_severity(
            app,
            Some(SystemSeverity::Info),
            "Session successfully compacted.",
        );
    }
}

fn handle_config_option_update(app: &mut App, config: model::ConfigOptionUpdate) {
    let option_id = config.option_id;
    let value = config.value;
    let value_kind = match &value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    };
    app.session_runtime.config_options.insert(option_id.clone(), value);
    tracing::debug!(
        target: crate::logging::targets::APP_CONFIG,
        event_name = "config_option_update_applied",
        message = "config option update applied",
        outcome = "success",
        option_id = %option_id,
        value_kind,
    );

    if matches!(
        app.turn.pending_command_ack.as_ref(),
        Some(PendingCommandAck::ConfigOption { option_id: expected }) if expected == &option_id
    ) {
        session::clear_pending_command(app);
    }
}

#[cfg(test)]
fn handle_normal_key(app: &mut App, key: KeyEvent) {
    super::keys::handle_normal_key(app, key);
}

#[cfg(test)]
fn handle_mention_key(app: &mut App, key: KeyEvent) {
    super::keys::handle_mention_key(app, key);
}

#[cfg(test)]
mod tests;
