// Claude Code Rust - A native Rust terminal interface for Claude Code
// Copyright (C) 2025  Simon Peter Rothgang
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as
// published by the Free Software Foundation, either version 3 of the
// License, or (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

mod mouse;
mod rate_limit;
mod session;
mod session_reset;
mod streaming;
mod tool_calls;
mod tool_updates;
mod turn;

use super::{
    ActiveView, App, AppStatus, ChatMessage, MessageBlock, MessageRole, PendingCommandAck,
    SystemSeverity, TextBlock,
};
use crate::agent::events::ClientEvent;
use crate::agent::model;
use crate::app::todos::apply_plan_todos;
#[cfg(test)]
use crossterm::event::KeyEvent;
use crossterm::event::{Event, KeyEventKind};

pub fn handle_terminal_event(app: &mut App, event: Event) {
    let changed = match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => dispatch_key_by_view(app, key),
        Event::Mouse(mouse) => {
            dispatch_mouse_by_view(app, mouse);
            true
        }
        Event::Paste(text) => dispatch_paste_by_view(app, &text),
        Event::FocusGained => {
            app.notifications.on_focus_gained();
            app.refresh_git_branch();
            true
        }
        Event::FocusLost => {
            app.notifications.on_focus_lost();
            true
        }
        Event::Resize(_, _) => {
            // Force a full terminal clear on resize. Without this, terminal
            // emulators (especially on Windows) corrupt their scrollback buffer
            // when the alternate screen is resized, causing the visible area to
            // shift even though ratatui paints the correct content. The clear
            // resets the terminal's internal state.
            app.force_redraw = true;
            true
        }
        // Non-press key events (Release, Repeat) -- ignored.
        Event::Key(_) => false,
    };
    app.needs_redraw |= changed;
}

fn dispatch_key_by_view(app: &mut App, key: crossterm::event::KeyEvent) -> bool {
    match app.active_view {
        ActiveView::Chat => {
            app.active_paste_session = None;
            super::keys::dispatch_key_by_focus(app, key)
        }
        ActiveView::Config => {
            super::config::handle_key(app, key);
            true
        }
        ActiveView::Trusted => {
            super::trust::handle_key(app, key);
            true
        }
    }
}

fn dispatch_mouse_by_view(app: &mut App, mouse: crossterm::event::MouseEvent) {
    match app.active_view {
        ActiveView::Chat => {
            app.active_paste_session = None;
            mouse::handle_mouse_event(app, mouse);
        }
        ActiveView::Config | ActiveView::Trusted => {
            let _ = mouse;
        }
    }
}

fn dispatch_paste_by_view(app: &mut App, text: &str) -> bool {
    match app.active_view {
        ActiveView::Chat => {
            if !matches!(
                app.status,
                AppStatus::Connecting | AppStatus::CommandPending | AppStatus::Error
            ) && !app.is_compacting
            {
                app.queue_paste_text(text);
                return true;
            }
            false
        }
        ActiveView::Config => super::config::handle_paste(app, text),
        ActiveView::Trusted => false,
    }
}

#[allow(clippy::too_many_lines)]
pub fn handle_client_event(app: &mut App, event: ClientEvent) {
    app.needs_redraw = true;
    match event {
        ClientEvent::SessionUpdate(update) => handle_session_update_event(app, update),
        ClientEvent::PermissionRequest { request, response_tx } => {
            turn::handle_permission_request_event(app, request, response_tx);
        }
        ClientEvent::QuestionRequest { request, response_tx } => {
            turn::handle_question_request_event(app, request, response_tx);
        }
        ClientEvent::TurnCancelled => turn::handle_turn_cancelled_event(app),
        ClientEvent::TurnComplete => turn::handle_turn_complete_event(app),
        ClientEvent::TurnError(msg) => turn::handle_turn_error_event(app, &msg, None),
        ClientEvent::TurnErrorClassified { message, class } => {
            turn::handle_turn_error_event(app, &message, Some(class));
        }
        ClientEvent::Connected {
            session_id,
            cwd,
            model_name,
            available_models,
            mode,
            history_updates,
        } => {
            session::handle_connected_client_event(
                app,
                session_id,
                cwd,
                model_name,
                available_models,
                mode,
                &history_updates,
            );
            crate::app::config::request_mcp_snapshot_if_needed(app);
        }
        ClientEvent::SessionsListed { sessions } => {
            session::handle_sessions_listed_event(app, sessions);
        }
        ClientEvent::AuthRequired { method_name, method_description } => {
            session::handle_auth_required_event(app, method_name, method_description);
        }
        ClientEvent::ConnectionFailed(msg) => {
            session::handle_connection_failed_event(app, &msg);
        }
        ClientEvent::SlashCommandError(msg) => {
            session::handle_slash_command_error_event(app, &msg);
        }
        ClientEvent::SessionReplaced {
            session_id,
            cwd,
            model_name,
            available_models,
            mode,
            history_updates,
        } => {
            session::handle_session_replaced_event(
                app,
                session_id,
                cwd,
                model_name,
                available_models,
                mode,
                &history_updates,
            );
            crate::app::config::request_mcp_snapshot_if_needed(app);
        }
        ClientEvent::UpdateAvailable { latest_version, current_version } => {
            session::handle_update_available_event(app, &latest_version, &current_version);
        }
        ClientEvent::ServiceStatus { severity, message } => {
            session::handle_service_status_event(app, severity, &message);
        }
        ClientEvent::AuthCompleted { conn } => {
            session::handle_auth_completed_event(app, &conn);
        }
        ClientEvent::LogoutCompleted => {
            session::handle_logout_completed_event(app);
        }
        ClientEvent::StatusSnapshotReceived { account } => {
            app.account_info = Some(account);
            app.needs_redraw = true;
        }
        ClientEvent::McpSnapshotReceived { servers, error } => {
            app.mcp.servers = servers;
            app.mcp.in_flight = false;
            app.mcp.last_error = error;
            app.config.mcp_selected_server_index =
                app.config.mcp_selected_server_index.min(app.mcp.servers.len().saturating_sub(1));
        }
        ClientEvent::UsageRefreshStarted => {
            crate::app::usage::apply_refresh_started(app);
        }
        ClientEvent::UsageSnapshotReceived { snapshot } => {
            crate::app::usage::apply_refresh_success(app, snapshot);
        }
        ClientEvent::UsageRefreshFailed { message, source } => {
            crate::app::usage::apply_refresh_failure(app, message, source);
        }
        ClientEvent::PluginsInventoryUpdated { snapshot, claude_path } => {
            crate::app::plugins::apply_inventory_refresh_success(app, snapshot, claude_path);
        }
        ClientEvent::PluginsInventoryRefreshFailed(message) => {
            crate::app::plugins::apply_inventory_refresh_failure(app, message);
        }
        ClientEvent::PluginsCliActionSucceeded { result } => {
            crate::app::plugins::apply_cli_action_success(app, result);
        }
        ClientEvent::PluginsCliActionFailed(message) => {
            crate::app::plugins::apply_cli_action_failure(app, message);
        }
        ClientEvent::FatalError(error) => session::handle_fatal_error_event(app, error),
    }
}

fn handle_session_update_event(app: &mut App, update: model::SessionUpdate) {
    let needs_history_retention = matches!(
        &update,
        model::SessionUpdate::AgentMessageChunk(_)
            | model::SessionUpdate::ToolCall(_)
            | model::SessionUpdate::ToolCallUpdate(_)
            | model::SessionUpdate::CompactionBoundary(_)
    );
    handle_session_update(app, update);
    if needs_history_retention {
        app.enforce_history_retention_tracked();
    }
}

fn handle_session_update(app: &mut App, update: model::SessionUpdate) {
    tracing::debug!("SessionUpdate variant: {}", session_update_name(&update));
    match update {
        model::SessionUpdate::AgentMessageChunk(chunk) => {
            clear_compaction_state(app, true);
            streaming::handle_agent_message_chunk(app, chunk);
        }
        model::SessionUpdate::ToolCall(tc) => tool_calls::handle_tool_call(app, tc),
        model::SessionUpdate::ToolCallUpdate(tcu) => {
            tool_updates::handle_tool_call_update_session(app, &tcu);
        }
        model::SessionUpdate::UserMessageChunk(_) => {}
        model::SessionUpdate::AgentThoughtChunk(chunk) => {
            tracing::debug!("Agent thought: {:?}", chunk);
            app.status = AppStatus::Thinking;
        }
        model::SessionUpdate::Plan(plan) => {
            tracing::debug!("Plan update: {:?}", plan);
            apply_plan_todos(app, &plan);
        }
        model::SessionUpdate::AvailableCommandsUpdate(cmds) => {
            tracing::debug!("Available commands: {} commands", cmds.available_commands.len());
            app.available_commands = cmds.available_commands;
            crate::app::plugins::clamp_selection(app);
            if app.slash.is_some() {
                super::slash::update_query(app);
            }
        }
        model::SessionUpdate::AvailableAgentsUpdate(agents) => {
            tracing::debug!("Available subagents: {} agents", agents.available_agents.len());
            app.available_agents = agents.available_agents;
            if app.subagent.is_some() {
                super::subagent::update_query(app);
            }
        }
        model::SessionUpdate::ModeStateUpdate(mode) => {
            app.mode = Some(mode);
            app.cached_footer_line = None;
            if matches!(app.pending_command_ack, Some(PendingCommandAck::CurrentModeUpdate)) {
                session::clear_pending_command(app);
            }
        }
        model::SessionUpdate::CurrentModeUpdate(update) => {
            let mode_id = update.current_mode_id.to_string();
            if let Some(ref mut mode) = app.mode {
                if let Some(info) = mode.available_modes.iter().find(|m| m.id == mode_id) {
                    mode.current_mode_name.clone_from(&info.name);
                    mode.current_mode_id = mode_id;
                } else {
                    mode.current_mode_name.clone_from(&mode_id);
                    mode.current_mode_id = mode_id;
                }
                app.cached_footer_line = None;
            }
            if matches!(app.pending_command_ack, Some(PendingCommandAck::CurrentModeUpdate)) {
                session::clear_pending_command(app);
            }
        }
        model::SessionUpdate::ConfigOptionUpdate(config) => {
            tracing::debug!("Config update: {:?}", config);
            let option_id = config.option_id;
            let value = config.value;
            let model_name =
                if option_id == "model" { value.as_str().map(ToOwned::to_owned) } else { None };
            app.config_options.insert(option_id.clone(), value);

            if let Some(model_name) = model_name {
                app.model_name = model_name;
                app.cached_header_line = None;
                app.update_welcome_model_if_pristine();
            } else if option_id == "model" {
                tracing::warn!("ConfigOptionUpdate for model carried non-string value");
            }

            if matches!(
                app.pending_command_ack.as_ref(),
                Some(PendingCommandAck::ConfigOptionUpdate { option_id: expected })
                    if expected == &option_id
            ) {
                session::clear_pending_command(app);
            }
        }
        model::SessionUpdate::FastModeUpdate(state) => {
            app.fast_mode_state = state;
            app.cached_footer_line = None;
        }
        model::SessionUpdate::RateLimitUpdate(update) => {
            rate_limit::handle_rate_limit_update(app, &update);
        }
        model::SessionUpdate::SessionStatusUpdate(status) => {
            // TODO(runtime-verification): confirm in real SDK sessions that compaction
            // status updates are emitted consistently; if not, add a fallback indicator.
            if matches!(status, model::SessionStatus::Compacting) {
                app.is_compacting = true;
                app.cached_footer_line = None;
            } else {
                clear_compaction_state(app, true);
            }
            tracing::debug!("SessionStatusUpdate: compacting={}", app.is_compacting);
        }
        model::SessionUpdate::CompactionBoundary(boundary) => {
            rate_limit::handle_compaction_boundary_update(app, boundary);
        }
    }
}

pub(crate) fn push_system_message_with_severity(
    app: &mut App,
    severity: Option<SystemSeverity>,
    message: &str,
) {
    app.messages.push(ChatMessage {
        role: MessageRole::System(severity),
        blocks: vec![MessageBlock::Text(TextBlock::from_complete(message))],
        usage: None,
    });
    app.enforce_history_retention_tracked();
    app.viewport.engage_auto_scroll();
}

pub(super) fn clear_compaction_state(app: &mut App, emit_manual_success: bool) {
    if !app.is_compacting && !app.pending_compact_clear {
        return;
    }
    let should_emit_success = emit_manual_success && app.pending_compact_clear;
    app.pending_compact_clear = false;
    app.is_compacting = false;
    app.cached_footer_line = None;
    if should_emit_success {
        push_system_message_with_severity(
            app,
            Some(SystemSeverity::Info),
            "Session successfully compacted.",
        );
    }
}

/// Return a human-readable name for a `SessionUpdate` variant (for debug logging).
fn session_update_name(update: &model::SessionUpdate) -> &'static str {
    match update {
        model::SessionUpdate::AgentMessageChunk(_) => "AgentMessageChunk",
        model::SessionUpdate::ToolCall(_) => "ToolCall",
        model::SessionUpdate::ToolCallUpdate(_) => "ToolCallUpdate",
        model::SessionUpdate::UserMessageChunk(_) => "UserMessageChunk",
        model::SessionUpdate::AgentThoughtChunk(_) => "AgentThoughtChunk",
        model::SessionUpdate::Plan(_) => "Plan",
        model::SessionUpdate::AvailableCommandsUpdate(_) => "AvailableCommandsUpdate",
        model::SessionUpdate::AvailableAgentsUpdate(_) => "AvailableAgentsUpdate",
        model::SessionUpdate::ModeStateUpdate(_) => "ModeStateUpdate",
        model::SessionUpdate::CurrentModeUpdate(_) => "CurrentModeUpdate",
        model::SessionUpdate::ConfigOptionUpdate(_) => "ConfigOptionUpdate",
        model::SessionUpdate::FastModeUpdate(_) => "FastModeUpdate",
        model::SessionUpdate::RateLimitUpdate(_) => "RateLimitUpdate",
        model::SessionUpdate::SessionStatusUpdate(_) => "SessionStatusUpdate",
        model::SessionUpdate::CompactionBoundary(_) => "CompactionBoundary",
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
fn dispatch_key_by_focus(app: &mut App, key: KeyEvent) {
    super::keys::dispatch_key_by_focus(app, key);
}

#[cfg(test)]
mod tests {
    // =====
    // TESTS: 40
    // =====

    use super::*;
    use crate::agent::error_handling::TurnErrorClass;
    use crate::agent::events::ServiceStatusSeverity;
    use crate::app::{
        ActiveView, BlockCache, CancelOrigin, FocusOwner, FocusTarget, HelpView, InlinePermission,
        SelectionKind, SelectionPoint, SelectionState, TextBlockSpacing, TodoItem, TodoStatus,
        ToolCallInfo, ToolCallScope, mention,
    };
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
    use pretty_assertions::assert_eq;
    use ratatui::layout::Rect;
    use std::time::{Duration, Instant};
    use tokio::sync::oneshot;

    // Helper: build a minimal ToolCallInfo with given id + status

    fn tool_call(id: &str, status: model::ToolCallStatus) -> ToolCallInfo {
        ToolCallInfo {
            id: id.into(),
            title: id.into(),
            sdk_tool_name: "Read".into(),
            raw_input: None,
            output_metadata: None,
            status,
            content: vec![],
            collapsed: false,
            hidden: false,
            terminal_id: None,
            terminal_command: None,
            terminal_output: None,
            terminal_output_len: 0,
            terminal_bytes_seen: 0,
            terminal_snapshot_mode: crate::app::TerminalSnapshotMode::AppendOnly,
            render_epoch: 0,
            layout_epoch: 0,
            last_measured_width: 0,
            last_measured_height: 0,
            last_measured_layout_epoch: 0,
            last_measured_layout_generation: 0,
            cache: BlockCache::default(),
            pending_permission: None,
            pending_question: None,
        }
    }

    fn assistant_msg(blocks: Vec<MessageBlock>) -> ChatMessage {
        ChatMessage { role: MessageRole::Assistant, blocks, usage: None }
    }

    fn user_msg(text: &str) -> ChatMessage {
        ChatMessage {
            role: MessageRole::User,
            blocks: vec![MessageBlock::Text(TextBlock::from_complete(text))],
            usage: None,
        }
    }

    // shorten_tool_title

    #[test]
    fn shorten_unix_path() {
        let result = tool_calls::shorten_tool_title(
            "Read /home/user/project/src/main.rs",
            "/home/user/project",
        );
        assert_eq!(result, "Read src/main.rs");
    }

    #[test]
    fn register_tool_call_scope_treats_agent_as_task_scope() {
        let mut app = make_test_app();
        let scope = tool_calls::register_tool_call_scope(&mut app, "tool-agent", "Agent");
        assert_eq!(scope, ToolCallScope::Task);
        assert!(app.active_task_ids.contains("tool-agent"));
    }

    #[test]
    fn register_tool_call_scope_treats_task_as_task_scope() {
        let mut app = make_test_app();
        let scope = tool_calls::register_tool_call_scope(&mut app, "tool-task", "Task");
        assert_eq!(scope, ToolCallScope::Task);
        assert!(app.active_task_ids.contains("tool-task"));
    }

    /// Regression: when a Task was cancelled mid-turn, `active_task_ids` was never cleared
    /// because `finalize_in_progress_tool_calls` doesn't call `remove_active_task` and
    /// `clear_tool_scope_tracking` (called on `TurnComplete`) did not clear `active_task_ids`.
    /// The leaked ID caused main-agent tools on the next turn to be classified as Subagent,
    /// which eventually triggered the subagent thinking indicator spuriously.
    #[test]
    fn turn_complete_after_cancelled_task_leaves_no_stale_active_task_ids() {
        let mut app = make_test_app();

        // Simulate a Task tool call arriving as InProgress (no Completed update will follow)
        let task_tc = model::ToolCall::new("task-1", "Research")
            .kind(model::ToolKind::Think)
            .status(model::ToolCallStatus::InProgress)
            .meta(serde_json::json!({"claudeCode": {"toolName": "Task"}}));
        handle_client_event(
            &mut app,
            ClientEvent::SessionUpdate(model::SessionUpdate::ToolCall(task_tc)),
        );
        assert!(app.active_task_ids.contains("task-1"), "task must be tracked while InProgress");

        // User cancels then TurnComplete finalizes the turn
        handle_client_event(&mut app, ClientEvent::TurnCancelled);
        handle_client_event(&mut app, ClientEvent::TurnComplete);

        // Stale task ID must be gone after turn boundary
        assert!(app.active_task_ids.is_empty(), "stale task id must not survive TurnComplete");
        assert!(app.active_subagent_tool_ids.is_empty());
        assert!(app.subagent_idle_since.is_none());

        // Next turn: a normal main-agent Glob must get MainAgent scope, not Subagent
        let glob_tc = model::ToolCall::new("glob-1", "Glob **/*.rs")
            .kind(model::ToolKind::Search)
            .status(model::ToolCallStatus::InProgress)
            .meta(serde_json::json!({"claudeCode": {"toolName": "Glob"}}));
        handle_client_event(
            &mut app,
            ClientEvent::SessionUpdate(model::SessionUpdate::ToolCall(glob_tc)),
        );
        assert_eq!(
            app.tool_call_scope("glob-1"),
            Some(ToolCallScope::MainAgent),
            "main-agent tool must not be misclassified as Subagent after stale task is cleared"
        );
        assert!(
            app.active_subagent_tool_ids.is_empty(),
            "main-agent tool must not enter subagent tracking"
        );
    }

    #[test]
    fn shorten_windows_path() {
        let result = tool_calls::shorten_tool_title(
            "Read C:\\Users\\me\\project\\src\\main.rs",
            "C:\\Users\\me\\project",
        );
        assert_eq!(result, "Read src/main.rs");
    }

    #[test]
    fn shorten_no_match_returns_original() {
        let result =
            tool_calls::shorten_tool_title("Read /other/path/file.rs", "/home/user/project");
        assert_eq!(result, "Read /other/path/file.rs");
    }

    // shorten_tool_title

    #[test]
    fn shorten_empty_cwd() {
        let result = tool_calls::shorten_tool_title("Read /some/path/file.rs", "");
        assert_eq!(result, "Read /some/path/file.rs");
    }

    #[test]
    fn shorten_cwd_with_trailing_slash() {
        let result = tool_calls::shorten_tool_title(
            "Read /home/user/project/file.rs",
            "/home/user/project/",
        );
        assert_eq!(result, "Read file.rs");
    }

    #[test]
    fn shorten_title_is_just_path() {
        let result =
            tool_calls::shorten_tool_title("/home/user/project/file.rs", "/home/user/project");
        assert_eq!(result, "file.rs");
    }

    #[test]
    fn shorten_mixed_separators() {
        let result = tool_calls::shorten_tool_title(
            "Read C:/Users/me/project/src/lib.rs",
            "C:\\Users\\me\\project",
        );
        assert_eq!(result, "Read src/lib.rs");
    }

    #[test]
    fn shorten_empty_title() {
        assert_eq!(tool_calls::shorten_tool_title("", "/some/cwd"), "");
    }

    #[test]
    fn shorten_title_no_path_at_all() {
        assert_eq!(tool_calls::shorten_tool_title("Read", "/home/user"), "Read");
        assert_eq!(tool_calls::shorten_tool_title("Write something", "/proj"), "Write something");
    }

    #[test]
    fn shorten_title_equals_cwd_exactly() {
        // Title IS the cwd path - after stripping, nothing left
        let result = tool_calls::shorten_tool_title("/home/user/project", "/home/user/project");
        // The cwd+/ won't match because title doesn't have trailing content after cwd
        // cwd_norm = "/home/user/project/", title doesn't contain that
        assert_eq!(result, "/home/user/project");
    }

    // shorten_tool_title

    #[test]
    fn shorten_partial_match_no_false_positive() {
        let result = tool_calls::shorten_tool_title("Read /home/username/file.rs", "/home/user");
        assert_eq!(result, "Read /home/username/file.rs");
    }

    #[test]
    fn shorten_deeply_nested_path() {
        let cwd = "/a/b/c/d/e/f/g";
        let title = "Read /a/b/c/d/e/f/g/h/i/j.rs";
        let result = tool_calls::shorten_tool_title(title, cwd);
        assert_eq!(result, "Read h/i/j.rs");
    }

    #[test]
    fn shorten_cwd_appears_multiple_times() {
        let result = tool_calls::shorten_tool_title("Diff /proj/a.rs /proj/b.rs", "/proj");
        assert_eq!(result, "Diff a.rs b.rs");
    }

    /// Spaces in path (real Windows path with spaces).
    #[test]
    fn shorten_spaces_in_path() {
        let result = tool_calls::shorten_tool_title(
            "Read C:\\Users\\Simon Peter Rothgang\\Desktop\\project\\src\\main.rs",
            "C:\\Users\\Simon Peter Rothgang\\Desktop\\project",
        );
        assert_eq!(result, "Read src/main.rs");
    }

    /// Unicode characters in path components.
    #[test]
    fn shorten_unicode_in_path() {
        let result = tool_calls::shorten_tool_title(
            "Read /home/\u{00FC}ser/\u{30D7}\u{30ED}\u{30B8}\u{30A7}\u{30AF}\u{30C8}/src/lib.rs",
            "/home/\u{00FC}ser/\u{30D7}\u{30ED}\u{30B8}\u{30A7}\u{30AF}\u{30C8}",
        );
        assert_eq!(result, "Read src/lib.rs");
    }

    /// Root as cwd (Unix).
    #[test]
    fn shorten_cwd_is_root_unix() {
        // cwd = "/" => with_sep = "/", so "/foo/bar.rs".contains("/") => replaces
        let result = tool_calls::shorten_tool_title("Read /foo/bar.rs", "/");
        // "/" is first path component = "" (empty), heuristic check uses "" which is in everything
        // After normalization: cwd = "/", with_sep = "/", title contains "/" => replaces ALL "/"
        assert_eq!(result, "Read foobar.rs");
    }

    /// Root as cwd (Windows).
    #[test]
    fn shorten_cwd_is_drive_root_windows() {
        let result = tool_calls::shorten_tool_title("Read C:\\src\\main.rs", "C:\\");
        assert_eq!(result, "Read src/main.rs");
    }

    /// Very long path (stress test).
    #[test]
    fn shorten_very_long_path() {
        let segments: String = (0..50).fold(String::new(), |mut s, i| {
            use std::fmt::Write;
            write!(s, "/seg{i}").unwrap();
            s
        });
        let cwd = segments.clone();
        let title = format!("Read {segments}/deep/file.rs");
        let result = tool_calls::shorten_tool_title(&title, &cwd);
        assert_eq!(result, "Read deep/file.rs");
    }

    /// Case sensitivity: paths are case-sensitive.
    #[test]
    fn shorten_case_sensitive() {
        let result =
            tool_calls::shorten_tool_title("Read /Home/User/Project/file.rs", "/home/user/project");
        // Different case, so the first-component heuristic "home" matches "Home"?
        // No: cwd_start = "home", title doesn't contain "home" (has "Home") => early return
        assert_eq!(result, "Read /Home/User/Project/file.rs");
    }

    /// Cwd that is a prefix at directory boundary but not at cwd boundary.
    #[test]
    fn shorten_cwd_prefix_boundary() {
        // cwd="/pro" should NOT strip from "/project/file.rs"
        let result = tool_calls::shorten_tool_title("Read /project/file.rs", "/pro");
        // cwd_start = "pro", title contains "pro" (in "project") => proceeds to normalize
        // with_sep = "/pro/", title_norm = "Read /project/file.rs", doesn't contain "/pro/"
        assert_eq!(result, "Read /project/file.rs");
    }

    #[test]
    fn split_index_prefers_double_newline() {
        let text = "first\n\nsecond";
        let split_at = streaming::find_text_block_split_index(text);
        assert_eq!(split_at, Some("first\n\n".len()));
    }

    #[test]
    fn split_index_soft_limit_prefers_newline() {
        use super::super::default_cache_split_policy;
        let prefix = "a".repeat(default_cache_split_policy().soft_limit_bytes - 1);
        let text = format!("{prefix}\n{}", "b".repeat(32));
        let split_at = streaming::find_text_block_split_index(&text).expect("expected split index");
        assert_eq!(&text[..split_at], format!("{prefix}\n"));
    }

    #[test]
    fn split_index_hard_limit_uses_sentence_when_needed() {
        use super::super::default_cache_split_policy;
        let prefix = "a".repeat(default_cache_split_policy().hard_limit_bytes + 32);
        let text = format!("{prefix}. tail");
        let split_at = streaming::find_text_block_split_index(&text).expect("expected split index");
        assert_eq!(&text[..split_at], format!("{prefix}."));
    }

    #[test]
    fn split_index_ignores_double_newline_inside_code_fence() {
        let text = "```\nline1\n\nline2\n```";
        assert!(streaming::find_text_block_split_index(text).is_none());
    }

    #[test]
    fn agent_message_chunk_splits_into_frozen_text_blocks() {
        let mut app = make_test_app();
        handle_client_event(
            &mut app,
            ClientEvent::SessionUpdate(model::SessionUpdate::AgentMessageChunk(
                model::ContentChunk::new(model::ContentBlock::Text(model::TextContent::new(
                    "p1\n\np2\n\np3",
                ))),
            )),
        );

        assert_eq!(app.messages.len(), 1);
        let Some(last) = app.messages.last() else {
            panic!("missing assistant message");
        };
        assert!(matches!(last.role, MessageRole::Assistant));
        assert_eq!(last.blocks.len(), 3);
        let Some(MessageBlock::Text(b1)) = last.blocks.first() else {
            panic!("expected first text block");
        };
        let Some(MessageBlock::Text(b2)) = last.blocks.get(1) else {
            panic!("expected second text block");
        };
        let Some(MessageBlock::Text(b3)) = last.blocks.get(2) else {
            panic!("expected third text block");
        };
        assert_eq!(b1.text, "p1\n\n");
        assert_eq!(b2.text, "p2\n\n");
        assert_eq!(b3.text, "p3");
        assert_eq!(b1.trailing_spacing, TextBlockSpacing::ParagraphBreak);
        assert_eq!(b2.trailing_spacing, TextBlockSpacing::ParagraphBreak);
        assert_eq!(b3.trailing_spacing, TextBlockSpacing::None);
    }

    // has_in_progress_tool_calls

    fn make_test_app() -> App {
        App::test_default()
    }

    fn connected_event(model_name: &str) -> ClientEvent {
        ClientEvent::Connected {
            session_id: model::SessionId::new("test-session"),
            cwd: "/test".into(),
            model_name: model_name.to_owned(),
            available_models: Vec::new(),
            mode: None,
            history_updates: Vec::new(),
        }
    }

    #[test]
    fn raw_output_string_maps_to_terminal_text() {
        let raw = serde_json::json!("hello\nworld");
        assert_eq!(
            tool_updates::raw_output_to_terminal_text(&raw).as_deref(),
            Some("hello\nworld")
        );
    }

    #[test]
    fn raw_output_text_array_maps_to_terminal_text() {
        let raw = serde_json::json!([
            {"type": "text", "text": "first"},
            {"type": "text", "text": "second"}
        ]);
        assert_eq!(
            tool_updates::raw_output_to_terminal_text(&raw).as_deref(),
            Some("first\nsecond")
        );
    }

    #[test]
    fn execute_tool_update_uses_raw_output_fallback() {
        let mut app = make_test_app();
        let tc = model::ToolCall::new("tc-exec", "Terminal")
            .kind(model::ToolKind::Execute)
            .status(model::ToolCallStatus::InProgress);
        handle_client_event(
            &mut app,
            ClientEvent::SessionUpdate(model::SessionUpdate::ToolCall(tc)),
        );

        let fields = model::ToolCallUpdateFields::new()
            .status(model::ToolCallStatus::Completed)
            .raw_output(serde_json::json!("line 1\nline 2"));
        let update = model::ToolCallUpdate::new("tc-exec", fields);
        handle_client_event(
            &mut app,
            ClientEvent::SessionUpdate(model::SessionUpdate::ToolCallUpdate(update)),
        );

        let Some((mi, bi)) = app.lookup_tool_call("tc-exec") else {
            panic!("tool call not indexed");
        };
        let Some(MessageBlock::ToolCall(tc)) = app.messages.get(mi).and_then(|m| m.blocks.get(bi))
        else {
            panic!("tool call block missing");
        };
        assert_eq!(tc.terminal_output.as_deref(), Some("line 1\nline 2"));
    }

    #[test]
    fn tool_call_update_noop_does_not_bump_epochs() {
        let mut app = make_test_app();
        let tc = model::ToolCall::new("tc-noop", "Read file")
            .kind(model::ToolKind::Read)
            .status(model::ToolCallStatus::InProgress);
        handle_client_event(
            &mut app,
            ClientEvent::SessionUpdate(model::SessionUpdate::ToolCall(tc)),
        );

        let (mi, bi) = app.lookup_tool_call("tc-noop").expect("tool call not indexed");
        let (before_render, before_layout, before_dirty_from) = {
            let MessageBlock::ToolCall(tc) = &app.messages[mi].blocks[bi] else {
                panic!("tool call block missing");
            };
            (tc.render_epoch, tc.layout_epoch, app.viewport.dirty_from)
        };

        let update = model::ToolCallUpdate::new(
            "tc-noop",
            model::ToolCallUpdateFields::new().status(model::ToolCallStatus::InProgress),
        );
        handle_client_event(
            &mut app,
            ClientEvent::SessionUpdate(model::SessionUpdate::ToolCallUpdate(update)),
        );

        let MessageBlock::ToolCall(tc) = &app.messages[mi].blocks[bi] else {
            panic!("tool call block missing");
        };
        assert_eq!(tc.render_epoch, before_render);
        assert_eq!(tc.layout_epoch, before_layout);
        assert_eq!(app.viewport.dirty_from, before_dirty_from);
    }

    #[test]
    fn todowrite_tool_call_without_todos_array_preserves_existing_todos() {
        let mut app = make_test_app();
        app.todos.push(TodoItem {
            content: "Existing todo".into(),
            status: TodoStatus::InProgress,
            active_form: String::new(),
        });
        app.show_todo_panel = true;

        let todo_call = model::ToolCall::new("tc-todo-empty", "TodoWrite")
            .kind(model::ToolKind::Other)
            .raw_input(serde_json::json!({}))
            .meta(serde_json::json!({"claudeCode": {"toolName": "TodoWrite"}}));
        handle_client_event(
            &mut app,
            ClientEvent::SessionUpdate(model::SessionUpdate::ToolCall(todo_call)),
        );

        assert_eq!(app.todos.len(), 1);
        assert_eq!(app.todos[0].content, "Existing todo");
        assert_eq!(app.todos[0].status, TodoStatus::InProgress);
        assert!(app.show_todo_panel);
    }

    #[test]
    fn todowrite_tool_call_update_without_todos_array_preserves_existing_todos() {
        let mut app = make_test_app();
        let todo_call = model::ToolCall::new("tc-todo-update", "TodoWrite")
            .kind(model::ToolKind::Other)
            .raw_input(serde_json::json!({
                "todos": [{"content": "Task A", "status": "in_progress"}]
            }))
            .meta(serde_json::json!({"claudeCode": {"toolName": "TodoWrite"}}));
        handle_client_event(
            &mut app,
            ClientEvent::SessionUpdate(model::SessionUpdate::ToolCall(todo_call)),
        );
        assert_eq!(app.todos.len(), 1);
        assert_eq!(app.todos[0].content, "Task A");

        let update = model::ToolCallUpdate::new(
            "tc-todo-update",
            model::ToolCallUpdateFields::new().raw_input(serde_json::json!({})),
        );
        handle_client_event(
            &mut app,
            ClientEvent::SessionUpdate(model::SessionUpdate::ToolCallUpdate(update)),
        );

        assert_eq!(app.todos.len(), 1);
        assert_eq!(app.todos[0].content, "Task A");
        assert_eq!(app.todos[0].status, TodoStatus::InProgress);
    }

    #[test]
    fn has_in_progress_empty_messages() {
        let app = make_test_app();
        assert!(!tool_calls::has_in_progress_tool_calls(&app));
    }

    #[test]
    fn has_in_progress_no_tool_calls() {
        let mut app = make_test_app();
        app.messages
            .push(assistant_msg(vec![MessageBlock::Text(TextBlock::from_complete("hello"))]));
        assert!(!tool_calls::has_in_progress_tool_calls(&app));
    }

    #[test]
    fn has_in_progress_with_pending_tool() {
        let mut app = make_test_app();
        app.messages.push(assistant_msg(vec![MessageBlock::ToolCall(Box::new(tool_call(
            "tc1",
            model::ToolCallStatus::Pending,
        )))]));
        assert!(tool_calls::has_in_progress_tool_calls(&app));
    }

    #[test]
    fn has_in_progress_with_in_progress_tool() {
        let mut app = make_test_app();
        app.messages.push(assistant_msg(vec![MessageBlock::ToolCall(Box::new(tool_call(
            "tc1",
            model::ToolCallStatus::InProgress,
        )))]));
        assert!(tool_calls::has_in_progress_tool_calls(&app));
    }

    #[test]
    fn has_in_progress_all_completed() {
        let mut app = make_test_app();
        app.messages.push(assistant_msg(vec![MessageBlock::ToolCall(Box::new(tool_call(
            "tc1",
            model::ToolCallStatus::Completed,
        )))]));
        assert!(!tool_calls::has_in_progress_tool_calls(&app));
    }

    #[test]
    fn has_in_progress_all_failed() {
        let mut app = make_test_app();
        app.messages.push(assistant_msg(vec![MessageBlock::ToolCall(Box::new(tool_call(
            "tc1",
            model::ToolCallStatus::Failed,
        )))]));
        assert!(!tool_calls::has_in_progress_tool_calls(&app));
    }

    // has_in_progress_tool_calls

    #[test]
    fn has_in_progress_user_message_last() {
        let mut app = make_test_app();
        app.messages.push(user_msg("hi"));
        assert!(!tool_calls::has_in_progress_tool_calls(&app));
    }

    /// Only the LAST message matters - earlier assistant messages are ignored.
    #[test]
    fn has_in_progress_only_checks_last_message() {
        let mut app = make_test_app();
        // First assistant message has in-progress tool
        app.messages.push(assistant_msg(vec![MessageBlock::ToolCall(Box::new(tool_call(
            "tc1",
            model::ToolCallStatus::InProgress,
        )))]));
        // Last message is user - should be false
        app.messages.push(user_msg("thanks"));
        assert!(!tool_calls::has_in_progress_tool_calls(&app));
    }

    /// Earlier assistant with in-progress, last assistant all completed.
    #[test]
    fn has_in_progress_ignores_earlier_assistant() {
        let mut app = make_test_app();
        app.messages.push(assistant_msg(vec![MessageBlock::ToolCall(Box::new(tool_call(
            "tc1",
            model::ToolCallStatus::InProgress,
        )))]));
        app.messages.push(user_msg("ok"));
        app.messages.push(assistant_msg(vec![MessageBlock::ToolCall(Box::new(tool_call(
            "tc2",
            model::ToolCallStatus::Completed,
        )))]));
        assert!(!tool_calls::has_in_progress_tool_calls(&app));
    }

    #[test]
    fn has_in_progress_mixed_completed_and_pending() {
        let mut app = make_test_app();
        app.messages.push(assistant_msg(vec![
            MessageBlock::ToolCall(Box::new(tool_call("tc1", model::ToolCallStatus::Completed))),
            MessageBlock::ToolCall(Box::new(tool_call("tc2", model::ToolCallStatus::InProgress))),
        ]));
        assert!(tool_calls::has_in_progress_tool_calls(&app));
    }

    /// Text blocks mixed with tool calls - text blocks are correctly skipped.
    #[test]
    fn has_in_progress_text_and_tools_mixed() {
        let mut app = make_test_app();
        app.messages.push(assistant_msg(vec![
            MessageBlock::Text(TextBlock::from_complete("thinking...")),
            MessageBlock::ToolCall(Box::new(tool_call("tc1", model::ToolCallStatus::Completed))),
            MessageBlock::Text(TextBlock::from_complete("done")),
        ]));
        assert!(!tool_calls::has_in_progress_tool_calls(&app));
    }

    /// Stress: 100 completed tool calls + 1 pending at the end.
    #[test]
    fn has_in_progress_stress_100_tools_one_pending() {
        let mut app = make_test_app();
        let mut blocks: Vec<MessageBlock> = (0..100)
            .map(|i| {
                MessageBlock::ToolCall(Box::new(tool_call(
                    &format!("tc{i}"),
                    model::ToolCallStatus::Completed,
                )))
            })
            .collect();
        blocks.push(MessageBlock::ToolCall(Box::new(tool_call(
            "tc_pending",
            model::ToolCallStatus::Pending,
        ))));
        app.messages.push(assistant_msg(blocks));
        assert!(tool_calls::has_in_progress_tool_calls(&app));
    }

    /// Stress: 100 completed tool calls, none pending.
    #[test]
    fn has_in_progress_stress_100_tools_all_done() {
        let mut app = make_test_app();
        let blocks: Vec<MessageBlock> = (0..100)
            .map(|i| {
                MessageBlock::ToolCall(Box::new(tool_call(
                    &format!("tc{i}"),
                    model::ToolCallStatus::Completed,
                )))
            })
            .collect();
        app.messages.push(assistant_msg(blocks));
        assert!(!tool_calls::has_in_progress_tool_calls(&app));
    }

    /// Mix of Failed and Completed - neither counts as in-progress.
    #[test]
    fn has_in_progress_failed_and_completed_mix() {
        let mut app = make_test_app();
        app.messages.push(assistant_msg(vec![
            MessageBlock::ToolCall(Box::new(tool_call("tc1", model::ToolCallStatus::Completed))),
            MessageBlock::ToolCall(Box::new(tool_call("tc2", model::ToolCallStatus::Failed))),
            MessageBlock::ToolCall(Box::new(tool_call("tc3", model::ToolCallStatus::Completed))),
        ]));
        assert!(!tool_calls::has_in_progress_tool_calls(&app));
    }

    /// Empty assistant message (no blocks at all).
    #[test]
    fn has_in_progress_empty_assistant_blocks() {
        let mut app = make_test_app();
        app.messages.push(assistant_msg(vec![]));
        assert!(!tool_calls::has_in_progress_tool_calls(&app));
    }

    // make_test_app - verify defaults

    #[test]
    fn test_app_defaults() {
        let app = make_test_app();
        assert!(app.messages.is_empty());
        assert_eq!(app.viewport.scroll_offset, 0);
        assert_eq!(app.viewport.scroll_target, 0);
        assert!(app.viewport.auto_scroll);
        assert!(!app.should_quit);
        assert!(app.session_id.is_none());
        assert_eq!(app.files_accessed, 0);
        assert!(app.pending_permission_ids.is_empty());
        assert!(!app.tools_collapsed);
        assert!(!app.force_redraw);
        assert!(app.todos.is_empty());
        assert!(!app.show_todo_panel);
        assert!(app.selection.is_none());
        assert!(app.mention.is_none());
        assert!(!app.cancelled_turn_pending_hint);
        assert!(app.rendered_chat_lines.is_empty());
        assert!(app.rendered_input_lines.is_empty());
        assert!(matches!(app.status, AppStatus::Ready));
    }

    #[test]
    fn turn_complete_after_cancel_renders_interrupted_hint() {
        let mut app = make_test_app();

        handle_client_event(&mut app, ClientEvent::TurnCancelled);
        assert!(app.cancelled_turn_pending_hint);

        handle_client_event(&mut app, ClientEvent::TurnComplete);

        assert!(!app.cancelled_turn_pending_hint);
        let last = app.messages.last().expect("expected interruption hint message");
        assert!(matches!(last.role, MessageRole::System(Some(SystemSeverity::Info))));
        let Some(MessageBlock::Text(block)) = last.blocks.first() else {
            panic!("expected text block");
        };
        assert_eq!(block.text, "Conversation interrupted. Tell the model how to proceed.");
    }

    #[test]
    fn turn_complete_after_manual_cancel_marks_tail_assistant_layout_dirty() {
        let mut app = make_test_app();
        app.status = AppStatus::Thinking;
        app.messages.push(user_msg("build app"));
        app.messages.push(assistant_msg(vec![MessageBlock::Text(TextBlock::from_complete(
            "partial output",
        ))]));
        app.pending_cancel_origin = Some(CancelOrigin::Manual);

        handle_client_event(&mut app, ClientEvent::TurnComplete);

        assert!(matches!(app.status, AppStatus::Ready));
        assert_eq!(app.viewport.dirty_from, Some(1));
        let Some(last) = app.messages.last() else {
            panic!("expected interruption hint message");
        };
        assert!(matches!(last.role, MessageRole::System(Some(SystemSeverity::Info))));
    }

    #[test]
    fn turn_complete_after_auto_cancel_marks_tail_assistant_layout_dirty() {
        let mut app = make_test_app();
        app.status = AppStatus::Running;
        app.messages.push(user_msg("build app"));
        app.messages.push(assistant_msg(vec![MessageBlock::Text(TextBlock::from_complete(
            "partial output",
        ))]));
        app.pending_cancel_origin = Some(CancelOrigin::AutoQueue);

        handle_client_event(&mut app, ClientEvent::TurnComplete);

        assert!(matches!(app.status, AppStatus::Ready));
        assert_eq!(app.viewport.dirty_from, Some(1));
        let Some(last) = app.messages.last() else {
            panic!("expected assistant message");
        };
        assert!(matches!(last.role, MessageRole::Assistant));
    }

    #[test]
    fn connected_updates_welcome_model_while_pristine() {
        let mut app = make_test_app();
        app.messages.push(ChatMessage::welcome("Connecting...", "/test"));

        handle_client_event(&mut app, connected_event("claude-updated"));

        let Some(first) = app.messages.first() else {
            panic!("missing welcome message");
        };
        let Some(MessageBlock::Welcome(welcome)) = first.blocks.first() else {
            panic!("expected welcome block");
        };
        assert_eq!(welcome.model_name, "claude-updated");
    }

    #[test]
    fn connected_updates_cwd_and_clears_resuming_marker() {
        let mut app = make_test_app();
        app.messages.push(ChatMessage::welcome("Connecting...", "/test"));
        app.resuming_session_id = Some("resume-123".into());

        handle_client_event(
            &mut app,
            ClientEvent::Connected {
                session_id: model::SessionId::new("session-cwd"),
                cwd: "/changed".into(),
                model_name: "claude-updated".into(),
                available_models: Vec::new(),
                mode: None,
                history_updates: Vec::new(),
            },
        );

        assert_eq!(app.cwd_raw, "/changed");
        assert_eq!(app.cwd, "/changed");
        assert!(app.resuming_session_id.is_none());
        let Some(first) = app.messages.first() else {
            panic!("missing welcome message");
        };
        let Some(MessageBlock::Welcome(welcome)) = first.blocks.first() else {
            panic!("expected welcome block");
        };
        assert_eq!(welcome.cwd, "/changed");
    }

    #[test]
    fn connected_does_not_update_welcome_after_chat_started() {
        let mut app = make_test_app();
        app.messages.push(ChatMessage::welcome("Connecting...", "/test"));
        app.messages.push(user_msg("hello"));

        handle_client_event(&mut app, connected_event("claude-updated"));

        let Some(first) = app.messages.first() else {
            panic!("missing first message");
        };
        let Some(MessageBlock::Welcome(welcome)) = first.blocks.first() else {
            panic!("expected welcome block");
        };
        assert_eq!(welcome.model_name, "Connecting...");
    }

    #[test]
    fn auth_required_sets_hint_without_prefilling_login_command() {
        let mut app = make_test_app();
        app.input.set_text("keep me");

        handle_client_event(
            &mut app,
            ClientEvent::AuthRequired {
                method_name: "oauth".into(),
                method_description: "Open browser".into(),
            },
        );

        assert!(matches!(app.status, AppStatus::Ready));
        assert_eq!(app.input.text(), "keep me");
        let Some(hint) = &app.login_hint else {
            panic!("expected login hint");
        };
        assert_eq!(hint.method_name, "oauth");
        assert_eq!(hint.method_description, "Open browser");
    }

    #[test]
    fn update_available_sets_footer_hint() {
        let mut app = make_test_app();
        assert!(app.update_check_hint.is_none());

        handle_client_event(
            &mut app,
            ClientEvent::UpdateAvailable {
                latest_version: "0.3.0".into(),
                current_version: "0.2.0".into(),
            },
        );

        assert_eq!(
            app.update_check_hint.as_deref(),
            Some("Update available: v0.3.0 (current v0.2.0)  Ctrl+U to hide")
        );
    }

    #[test]
    fn service_status_warning_pushes_system_warning_without_locking_input() {
        let mut app = make_test_app();

        handle_client_event(
            &mut app,
            ClientEvent::ServiceStatus {
                severity: ServiceStatusSeverity::Warning,
                message: "Claude Code status: Partial Outage (indicator: minor).".into(),
            },
        );

        assert!(matches!(app.status, AppStatus::Ready));
        assert!(!app.startup_status_blocking_error);
        let Some(last) = app.messages.last() else {
            panic!("expected system message");
        };
        assert!(matches!(last.role, MessageRole::System(Some(SystemSeverity::Warning))));
    }

    #[test]
    fn service_status_error_locks_input_and_survives_connected_event() {
        let mut app = make_test_app();

        handle_client_event(
            &mut app,
            ClientEvent::ServiceStatus {
                severity: ServiceStatusSeverity::Error,
                message: "Claude Code status: Major Outage (indicator: major).".into(),
            },
        );

        assert!(matches!(app.status, AppStatus::Error));
        assert!(app.startup_status_blocking_error);
        let Some(last) = app.messages.last() else {
            panic!("expected system message");
        };
        assert!(matches!(last.role, MessageRole::System(Some(SystemSeverity::Error))));

        handle_client_event(&mut app, connected_event("claude-updated"));
        assert!(matches!(app.status, AppStatus::Error));
    }

    #[test]
    fn session_replaced_resets_chat_and_transient_state() {
        let mut app = make_test_app();
        app.messages.push(user_msg("hello"));
        app.messages
            .push(assistant_msg(vec![MessageBlock::Text(TextBlock::from_complete("world"))]));
        app.status = AppStatus::Running;
        app.files_accessed = 9;
        app.pending_permission_ids.push("perm-1".into());
        app.todo_selected = 2;
        app.show_todo_panel = true;
        app.todos.push(TodoItem {
            content: "Task".into(),
            status: TodoStatus::InProgress,
            active_form: String::new(),
        });
        app.mention = Some(mention::MentionState::new(0, 0, String::new(), Vec::new()));

        handle_client_event(
            &mut app,
            ClientEvent::SessionReplaced {
                session_id: model::SessionId::new("replacement"),
                cwd: "/replacement".into(),
                model_name: "new-model".into(),
                available_models: Vec::new(),
                mode: None,
                history_updates: Vec::new(),
            },
        );

        assert!(matches!(app.status, AppStatus::Ready));
        assert_eq!(
            app.session_id.as_ref().map(ToString::to_string).as_deref(),
            Some("replacement")
        );
        assert_eq!(app.model_name, "new-model");
        assert_eq!(app.messages.len(), 1);
        assert!(matches!(app.messages[0].role, MessageRole::Welcome));
        assert_eq!(app.files_accessed, 0);
        assert!(app.pending_permission_ids.is_empty());
        assert!(app.todos.is_empty());
        assert!(!app.show_todo_panel);
        assert!(app.mention.is_none());
        assert_eq!(app.cwd_raw, "/replacement");
        assert_eq!(app.cwd, "/replacement");
        let Some(MessageBlock::Welcome(welcome)) = app.messages[0].blocks.first() else {
            panic!("expected welcome block");
        };
        assert_eq!(welcome.cwd, "/replacement");
    }

    #[test]
    fn slash_command_error_while_resuming_returns_ready_and_clears_marker() {
        let mut app = make_test_app();
        app.status = AppStatus::CommandPending;
        app.resuming_session_id = Some("resume-123".into());

        handle_client_event(&mut app, ClientEvent::SlashCommandError("resume failed".into()));

        assert!(matches!(app.status, AppStatus::Ready));
        assert!(app.resuming_session_id.is_none());
    }

    #[test]
    fn sessions_listed_completes_pending_session_rename() {
        let mut app = make_test_app();
        app.config.pending_session_title_change =
            Some(crate::app::config::PendingSessionTitleChangeState {
                session_id: "session-1".to_owned(),
                kind: crate::app::config::PendingSessionTitleChangeKind::Rename {
                    requested_title: Some("Renamed session".to_owned()),
                },
            });

        handle_client_event(
            &mut app,
            ClientEvent::SessionsListed {
                sessions: vec![crate::agent::types::SessionListEntry {
                    session_id: "session-1".to_owned(),
                    summary: "Renamed session".to_owned(),
                    last_modified_ms: 1,
                    file_size_bytes: 2,
                    cwd: Some("/test".to_owned()),
                    git_branch: None,
                    custom_title: Some("Renamed session".to_owned()),
                    first_prompt: Some("prompt".to_owned()),
                }],
            },
        );

        assert!(app.config.pending_session_title_change.is_none());
        assert_eq!(
            app.config.status_message.as_deref(),
            Some("Renamed session to Renamed session")
        );
        assert!(app.config.last_error.is_none());
        assert_eq!(app.recent_sessions.len(), 1);
    }

    #[test]
    fn slash_command_error_for_pending_session_rename_stays_in_config_feedback() {
        let mut app = make_test_app();
        app.config.pending_session_title_change =
            Some(crate::app::config::PendingSessionTitleChangeState {
                session_id: "session-1".to_owned(),
                kind: crate::app::config::PendingSessionTitleChangeKind::Rename {
                    requested_title: Some("Renamed session".to_owned()),
                },
            });

        handle_client_event(
            &mut app,
            ClientEvent::SlashCommandError("failed to rename session: boom".into()),
        );

        assert!(app.config.pending_session_title_change.is_none());
        assert_eq!(app.config.last_error.as_deref(), Some("failed to rename session: boom"));
        assert!(app.config.status_message.is_none());
        assert!(app.messages.is_empty());
    }

    #[test]
    fn sessions_listed_completes_pending_session_title_generation() {
        let mut app = make_test_app();
        app.config.pending_session_title_change =
            Some(crate::app::config::PendingSessionTitleChangeState {
                session_id: "session-1".to_owned(),
                kind: crate::app::config::PendingSessionTitleChangeKind::Generate,
            });

        handle_client_event(
            &mut app,
            ClientEvent::SessionsListed {
                sessions: vec![crate::agent::types::SessionListEntry {
                    session_id: "session-1".to_owned(),
                    summary: "Generated session".to_owned(),
                    last_modified_ms: 1,
                    file_size_bytes: 2,
                    cwd: Some("/test".to_owned()),
                    git_branch: None,
                    custom_title: Some("Generated session".to_owned()),
                    first_prompt: Some("prompt".to_owned()),
                }],
            },
        );

        assert!(app.config.pending_session_title_change.is_none());
        assert_eq!(app.config.status_message.as_deref(), Some("Generated session title"));
        assert!(app.config.last_error.is_none());
    }

    #[test]
    fn current_mode_update_clears_pending_when_expected() {
        let mut app = make_test_app();
        app.status = AppStatus::CommandPending;
        app.pending_command_label = Some("Switching mode...".into());
        app.pending_command_ack = Some(PendingCommandAck::CurrentModeUpdate);
        app.mode = Some(crate::app::ModeState {
            current_mode_id: "code".to_owned(),
            current_mode_name: "Code".to_owned(),
            available_modes: vec![
                crate::app::ModeInfo { id: "code".to_owned(), name: "Code".to_owned() },
                crate::app::ModeInfo { id: "plan".to_owned(), name: "Plan".to_owned() },
            ],
        });

        handle_client_event(
            &mut app,
            ClientEvent::SessionUpdate(model::SessionUpdate::CurrentModeUpdate(
                model::CurrentModeUpdate::new("plan"),
            )),
        );

        assert!(matches!(app.status, AppStatus::Ready));
        assert!(app.pending_command_label.is_none());
        assert!(app.pending_command_ack.is_none());
        let mode = app.mode.expect("mode should be present");
        assert_eq!(mode.current_mode_id, "plan");
        assert_eq!(mode.current_mode_name, "Plan");
    }

    #[test]
    fn model_config_option_update_updates_state_and_clears_pending_when_expected() {
        let mut app = make_test_app();
        app.status = AppStatus::CommandPending;
        app.pending_command_label = Some("Switching model...".into());
        app.pending_command_ack =
            Some(PendingCommandAck::ConfigOptionUpdate { option_id: "model".to_owned() });
        app.model_name = "old-model".to_owned();

        handle_client_event(
            &mut app,
            ClientEvent::SessionUpdate(model::SessionUpdate::ConfigOptionUpdate(
                model::ConfigOptionUpdate {
                    option_id: "model".to_owned(),
                    value: serde_json::Value::String("sonnet".to_owned()),
                },
            )),
        );

        assert!(matches!(app.status, AppStatus::Ready));
        assert_eq!(app.model_name, "sonnet");
        assert_eq!(
            app.config_options.get("model"),
            Some(&serde_json::Value::String("sonnet".to_owned()))
        );
        assert!(app.pending_command_label.is_none());
        assert!(app.pending_command_ack.is_none());
    }

    #[test]
    fn non_matching_config_option_update_keeps_pending() {
        let mut app = make_test_app();
        app.status = AppStatus::CommandPending;
        app.pending_command_label = Some("Switching model...".into());
        app.pending_command_ack =
            Some(PendingCommandAck::ConfigOptionUpdate { option_id: "model".to_owned() });

        handle_client_event(
            &mut app,
            ClientEvent::SessionUpdate(model::SessionUpdate::ConfigOptionUpdate(
                model::ConfigOptionUpdate {
                    option_id: "max_thinking_tokens".to_owned(),
                    value: serde_json::json!(2048),
                },
            )),
        );

        assert!(matches!(app.status, AppStatus::CommandPending));
        assert_eq!(app.config_options.get("max_thinking_tokens"), Some(&serde_json::json!(2048)));
        assert_eq!(app.pending_command_label.as_deref(), Some("Switching model..."));
        assert!(matches!(
            app.pending_command_ack.as_ref(),
            Some(PendingCommandAck::ConfigOptionUpdate { option_id }) if option_id == "model"
        ));
    }

    #[test]
    fn resume_does_not_add_confirmation_system_message() {
        let mut app = make_test_app();
        app.resuming_session_id = Some("requested-123".into());

        handle_client_event(
            &mut app,
            ClientEvent::SessionReplaced {
                session_id: model::SessionId::new("active-456"),
                cwd: "/replacement".into(),
                model_name: "new-model".into(),
                available_models: Vec::new(),
                mode: None,
                history_updates: Vec::new(),
            },
        );

        assert_eq!(app.messages.len(), 1);
        assert!(matches!(app.messages[0].role, MessageRole::Welcome));
        assert!(app.resuming_session_id.is_none());
        assert!(matches!(app.status, AppStatus::Ready));
    }

    #[test]
    fn resume_history_renders_user_message_chunks() {
        let mut app = make_test_app();
        let history_updates = vec![
            model::SessionUpdate::UserMessageChunk(model::ContentChunk::new(
                model::ContentBlock::Text(model::TextContent::new("first user line")),
            )),
            model::SessionUpdate::AgentMessageChunk(model::ContentChunk::new(
                model::ContentBlock::Text(model::TextContent::new("assistant reply")),
            )),
        ];

        handle_client_event(
            &mut app,
            ClientEvent::SessionReplaced {
                session_id: model::SessionId::new("active-456"),
                cwd: "/replacement".into(),
                model_name: "new-model".into(),
                available_models: Vec::new(),
                mode: None,
                history_updates,
            },
        );

        assert_eq!(app.messages.len(), 3);
        assert!(matches!(app.messages[0].role, MessageRole::Welcome));
        assert!(matches!(app.messages[1].role, MessageRole::User));
        assert!(matches!(app.messages[2].role, MessageRole::Assistant));

        let Some(MessageBlock::Text(user_text)) = app.messages[1].blocks.first() else {
            panic!("expected user text block");
        };
        assert_eq!(user_text.text, "first user line");
    }

    #[test]
    fn resume_history_forces_open_tool_calls_to_failed() {
        let mut app = make_test_app();
        let open_tool = model::ToolCall::new("resume-open", "Execute command")
            .kind(model::ToolKind::Execute)
            .status(model::ToolCallStatus::InProgress);

        handle_client_event(
            &mut app,
            ClientEvent::SessionReplaced {
                session_id: model::SessionId::new("active-789"),
                cwd: "/replacement".into(),
                model_name: "new-model".into(),
                available_models: Vec::new(),
                mode: None,
                history_updates: vec![model::SessionUpdate::ToolCall(open_tool)],
            },
        );

        let Some((mi, bi)) = app.lookup_tool_call("resume-open") else {
            panic!("missing tool call index");
        };
        let Some(MessageBlock::ToolCall(tc)) = app.messages.get(mi).and_then(|m| m.blocks.get(bi))
        else {
            panic!("expected tool call block");
        };
        assert_eq!(tc.status, model::ToolCallStatus::Failed);
    }

    #[test]
    fn turn_complete_without_cancel_does_not_render_interrupted_hint() {
        let mut app = make_test_app();
        handle_client_event(&mut app, ClientEvent::TurnComplete);
        assert!(app.messages.is_empty());
    }

    #[test]
    fn turn_complete_keeps_history_and_adds_compaction_success_after_manual_boundary() {
        let mut app = make_test_app();
        app.session_id = Some(model::SessionId::new("session-x"));
        app.messages.push(user_msg("/compact"));
        app.messages
            .push(assistant_msg(vec![MessageBlock::Text(TextBlock::from_complete("compacted"))]));
        handle_client_event(
            &mut app,
            ClientEvent::SessionUpdate(model::SessionUpdate::CompactionBoundary(
                model::CompactionBoundary {
                    trigger: model::CompactionTrigger::Manual,
                    pre_tokens: 123_456,
                },
            )),
        );
        assert!(app.pending_compact_clear);

        handle_client_event(&mut app, ClientEvent::TurnComplete);

        assert!(!app.pending_compact_clear);
        assert_eq!(app.messages.len(), 3);
        let Some(ChatMessage {
            role: MessageRole::System(Some(SystemSeverity::Info)), blocks, ..
        }) = app.messages.last()
        else {
            panic!("expected compaction success system message");
        };
        let Some(MessageBlock::Text(block)) = blocks.first() else {
            panic!("expected text block");
        };
        assert_eq!(block.text, "Session successfully compacted.");
        assert_eq!(app.session_id.as_ref().map(ToString::to_string).as_deref(), Some("session-x"));
    }

    #[test]
    fn first_agent_chunk_clears_unconfirmed_compacting_without_success_message() {
        let mut app = make_test_app();
        app.is_compacting = true;

        handle_client_event(
            &mut app,
            ClientEvent::SessionUpdate(model::SessionUpdate::AgentMessageChunk(
                model::ContentChunk::new(model::ContentBlock::Text(model::TextContent::new(
                    "regular answer",
                ))),
            )),
        );

        assert!(!app.is_compacting);
        assert!(!app.pending_compact_clear);
        assert!(app.messages.iter().all(|message| {
            !matches!(
                message,
                ChatMessage { role: MessageRole::System(Some(SystemSeverity::Info)), .. }
            )
        }));
    }

    #[test]
    fn session_status_idle_does_not_emit_compaction_success_without_boundary() {
        let mut app = make_test_app();
        app.is_compacting = true;

        handle_client_event(
            &mut app,
            ClientEvent::SessionUpdate(model::SessionUpdate::SessionStatusUpdate(
                model::SessionStatus::Idle,
            )),
        );

        assert!(!app.is_compacting);
        assert!(!app.pending_compact_clear);
        assert!(app.messages.is_empty());
    }

    #[test]
    fn turn_error_keeps_history_when_compact_pending() {
        let mut app = make_test_app();
        app.pending_compact_clear = true;
        app.messages.push(user_msg("/compact"));

        handle_client_event(&mut app, ClientEvent::TurnError("adapter failed".into()));

        assert!(!app.pending_compact_clear);
        assert!(matches!(app.status, AppStatus::Error));
        assert_eq!(app.messages.len(), 2);
        assert!(matches!(app.messages[0].role, MessageRole::User));
        let Some(ChatMessage { role: MessageRole::System(_), blocks, .. }) = app.messages.last()
        else {
            panic!("expected system error message");
        };
        let Some(MessageBlock::Text(block)) = blocks.first() else {
            panic!("expected text block");
        };
        assert!(block.text.contains("Turn failed: adapter failed"));
        assert!(block.text.contains("Press Ctrl+Q to quit and try again"));
    }

    #[test]
    fn turn_error_plan_limit_shows_next_steps_guidance() {
        let mut app = make_test_app();

        handle_client_event(
            &mut app,
            ClientEvent::TurnError("HTTP 429 Too Many Requests: max turns exceeded".into()),
        );

        assert!(matches!(app.status, AppStatus::Error));
        let Some(ChatMessage { role: MessageRole::System(_), blocks, .. }) = app.messages.last()
        else {
            panic!("expected system error message");
        };
        let Some(MessageBlock::Text(block)) = blocks.first() else {
            panic!("expected text block");
        };
        assert!(block.text.contains("Turn blocked by account or plan limits"));
        assert!(block.text.contains("Next steps:"));
        assert!(block.text.contains("Check quota/billing"));
    }

    #[test]
    fn classified_turn_error_plan_limit_uses_guidance_without_text_matching() {
        let mut app = make_test_app();

        handle_client_event(
            &mut app,
            ClientEvent::TurnErrorClassified {
                message: "turn failed".into(),
                class: TurnErrorClass::PlanLimit,
            },
        );

        assert!(matches!(app.status, AppStatus::Error));
        let Some(ChatMessage { role: MessageRole::System(_), blocks, .. }) = app.messages.last()
        else {
            panic!("expected system error message");
        };
        let Some(MessageBlock::Text(block)) = blocks.first() else {
            panic!("expected text block");
        };
        assert!(block.text.contains("Turn blocked by account or plan limits"));
        assert!(block.text.contains("Next steps:"));
    }

    #[test]
    fn classified_turn_error_auth_required_sets_exit_error_and_quits() {
        let mut app = make_test_app();

        handle_client_event(
            &mut app,
            ClientEvent::TurnErrorClassified {
                message: "auth required".into(),
                class: TurnErrorClass::AuthRequired,
            },
        );

        assert!(matches!(app.status, AppStatus::Error));
        assert!(app.should_quit);
        assert_eq!(app.exit_error, Some(crate::error::AppError::AuthRequired));
    }

    #[test]
    fn fatal_event_sets_exit_error_and_quits() {
        let mut app = make_test_app();

        handle_client_event(
            &mut app,
            ClientEvent::FatalError(crate::error::AppError::ConnectionFailed),
        );

        assert!(matches!(app.status, AppStatus::Error));
        assert!(app.should_quit);
        assert_eq!(app.exit_error, Some(crate::error::AppError::ConnectionFailed));
    }

    #[test]
    fn compaction_boundary_enables_compacting_and_records_boundary() {
        let mut app = make_test_app();
        assert!(!app.is_compacting);

        handle_client_event(
            &mut app,
            ClientEvent::SessionUpdate(model::SessionUpdate::CompactionBoundary(
                model::CompactionBoundary {
                    trigger: model::CompactionTrigger::Manual,
                    pre_tokens: 123_456,
                },
            )),
        );

        assert!(app.is_compacting);
        assert!(app.pending_compact_clear);
        assert_eq!(
            app.session_usage.last_compaction_trigger,
            Some(model::CompactionTrigger::Manual)
        );
        assert_eq!(app.session_usage.last_compaction_pre_tokens, Some(123_456));
    }

    #[test]
    fn auto_compaction_boundary_sets_compacting_without_manual_success_pending() {
        let mut app = make_test_app();
        assert!(!app.is_compacting);

        handle_client_event(
            &mut app,
            ClientEvent::SessionUpdate(model::SessionUpdate::CompactionBoundary(
                model::CompactionBoundary {
                    trigger: model::CompactionTrigger::Auto,
                    pre_tokens: 234_567,
                },
            )),
        );

        assert!(app.is_compacting);
        assert!(!app.pending_compact_clear);
        assert_eq!(app.session_usage.last_compaction_trigger, Some(model::CompactionTrigger::Auto));
        assert_eq!(app.session_usage.last_compaction_pre_tokens, Some(234_567));
    }

    #[test]
    fn fast_mode_update_sets_state_and_invalidates_footer_cache() {
        let mut app = make_test_app();
        app.cached_footer_line = Some(ratatui::text::Line::from("cached"));
        assert_eq!(app.fast_mode_state, model::FastModeState::Off);

        handle_client_event(
            &mut app,
            ClientEvent::SessionUpdate(model::SessionUpdate::FastModeUpdate(
                model::FastModeState::Cooldown,
            )),
        );

        assert_eq!(app.fast_mode_state, model::FastModeState::Cooldown);
        assert!(app.cached_footer_line.is_none());
    }

    #[test]
    fn rate_limit_warning_transitions_once_and_rejected_emits_each_event() {
        let mut app = make_test_app();

        let warning_update = model::RateLimitUpdate {
            status: model::RateLimitStatus::AllowedWarning,
            resets_at: Some(123.0),
            utilization: Some(0.92),
            rate_limit_type: Some("five_hour".to_owned()),
            overage_status: None,
            overage_resets_at: None,
            overage_disabled_reason: None,
            is_using_overage: None,
            surpassed_threshold: None,
        };

        handle_client_event(
            &mut app,
            ClientEvent::SessionUpdate(model::SessionUpdate::RateLimitUpdate(
                warning_update.clone(),
            )),
        );
        handle_client_event(
            &mut app,
            ClientEvent::SessionUpdate(model::SessionUpdate::RateLimitUpdate(
                warning_update.clone(),
            )),
        );

        assert_eq!(app.messages.len(), 1);
        assert!(matches!(app.messages[0].role, MessageRole::System(Some(SystemSeverity::Warning))));

        let rejected_update =
            model::RateLimitUpdate { status: model::RateLimitStatus::Rejected, ..warning_update };
        handle_client_event(
            &mut app,
            ClientEvent::SessionUpdate(model::SessionUpdate::RateLimitUpdate(
                rejected_update.clone(),
            )),
        );
        handle_client_event(
            &mut app,
            ClientEvent::SessionUpdate(model::SessionUpdate::RateLimitUpdate(rejected_update)),
        );

        assert_eq!(app.messages.len(), 3);
        assert!(matches!(app.messages[1].role, MessageRole::System(_)));
        assert!(matches!(app.messages[2].role, MessageRole::System(_)));
    }

    #[test]
    fn plan_limit_turn_error_includes_rate_limit_context_and_warning_severity() {
        let mut app = make_test_app();
        app.last_rate_limit_update = Some(model::RateLimitUpdate {
            status: model::RateLimitStatus::AllowedWarning,
            resets_at: Some(1_741_280_000.0),
            utilization: Some(0.95),
            rate_limit_type: Some("five_hour".to_owned()),
            overage_status: None,
            overage_resets_at: None,
            overage_disabled_reason: None,
            is_using_overage: None,
            surpassed_threshold: None,
        });

        handle_client_event(
            &mut app,
            ClientEvent::TurnErrorClassified {
                message: "HTTP 429 Too Many Requests".to_owned(),
                class: TurnErrorClass::PlanLimit,
            },
        );

        let Some(last) = app.messages.last() else {
            panic!("expected combined system message");
        };
        assert!(matches!(last.role, MessageRole::System(Some(SystemSeverity::Warning))));
        let Some(MessageBlock::Text(block)) = last.blocks.first() else {
            panic!("expected text block");
        };
        assert!(block.text.contains("Approaching rate limit"));
        assert!(block.text.contains("Turn blocked by account or plan limits"));
    }

    #[test]
    fn turn_error_after_cancel_shows_interrupted_hint_instead_of_error_block() {
        let mut app = make_test_app();
        app.messages.push(user_msg("build app"));

        handle_client_event(&mut app, ClientEvent::TurnCancelled);
        assert!(app.cancelled_turn_pending_hint);

        handle_client_event(
            &mut app,
            ClientEvent::TurnError("Error: Request was aborted.\n    at stack line".into()),
        );

        assert!(!app.cancelled_turn_pending_hint);
        assert!(matches!(app.status, AppStatus::Ready));

        let Some(last) = app.messages.last() else {
            panic!("expected interruption hint message");
        };
        assert!(matches!(last.role, MessageRole::System(Some(SystemSeverity::Info))));
        let Some(MessageBlock::Text(block)) = last.blocks.first() else {
            panic!("expected text block");
        };
        assert_eq!(block.text, "Conversation interrupted. Tell the model how to proceed.");
    }

    #[test]
    fn turn_error_after_auto_cancel_marks_tail_assistant_layout_dirty() {
        let mut app = make_test_app();
        app.status = AppStatus::Running;
        app.messages.push(user_msg("build app"));
        app.messages.push(assistant_msg(vec![MessageBlock::Text(TextBlock::from_complete(
            "partial output",
        ))]));
        app.pending_cancel_origin = Some(CancelOrigin::AutoQueue);

        handle_client_event(
            &mut app,
            ClientEvent::TurnError("Error: Request was aborted.\n    at stack line".into()),
        );

        assert!(matches!(app.status, AppStatus::Ready));
        assert_eq!(app.viewport.dirty_from, Some(1));
        assert_eq!(app.messages.len(), 2);
        let Some(last) = app.messages.last() else {
            panic!("expected assistant message");
        };
        assert!(matches!(last.role, MessageRole::Assistant));
    }

    #[test]
    fn turn_cancel_marks_active_tools_failed() {
        let mut app = make_test_app();
        app.messages.push(assistant_msg(vec![
            MessageBlock::ToolCall(Box::new(tool_call("tc1", model::ToolCallStatus::InProgress))),
            MessageBlock::ToolCall(Box::new(tool_call("tc2", model::ToolCallStatus::Pending))),
            MessageBlock::ToolCall(Box::new(tool_call("tc3", model::ToolCallStatus::Completed))),
        ]));

        handle_client_event(&mut app, ClientEvent::TurnCancelled);

        let Some(last) = app.messages.last() else {
            panic!("missing assistant message");
        };
        let statuses: Vec<model::ToolCallStatus> = last
            .blocks
            .iter()
            .filter_map(|b| match b {
                MessageBlock::ToolCall(tc) => Some(tc.status),
                _ => None,
            })
            .collect();
        assert_eq!(
            statuses,
            vec![
                model::ToolCallStatus::Failed,
                model::ToolCallStatus::Failed,
                model::ToolCallStatus::Completed
            ]
        );
    }

    #[test]
    fn turn_complete_marks_lingering_tools_completed() {
        let mut app = make_test_app();
        app.messages.push(assistant_msg(vec![
            MessageBlock::ToolCall(Box::new(tool_call("tc1", model::ToolCallStatus::InProgress))),
            MessageBlock::ToolCall(Box::new(tool_call("tc2", model::ToolCallStatus::Pending))),
        ]));

        handle_client_event(&mut app, ClientEvent::TurnComplete);

        let Some(last) = app.messages.last() else {
            panic!("missing assistant message");
        };
        let statuses: Vec<model::ToolCallStatus> = last
            .blocks
            .iter()
            .filter_map(|b| match b {
                MessageBlock::ToolCall(tc) => Some(tc.status),
                _ => None,
            })
            .collect();
        assert_eq!(
            statuses,
            vec![model::ToolCallStatus::Completed, model::ToolCallStatus::Completed]
        );
    }

    #[test]
    fn ctrl_v_not_inserted_as_text() {
        let mut app = make_test_app();
        handle_normal_key(&mut app, KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL));
        assert_eq!(app.input.text(), "");
    }

    #[test]
    fn ctrl_v_not_inserted_when_mention_key_handler_is_active() {
        let mut app = make_test_app();
        handle_mention_key(&mut app, KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL));
        assert_eq!(app.input.text(), "");
    }

    #[test]
    fn pending_paste_payload_blocks_overlapping_key_text_insertion() {
        let mut app = make_test_app();
        app.pending_paste_text = "clipboard".to_owned();

        handle_normal_key(&mut app, KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));

        assert_eq!(app.input.text(), "");
    }

    #[test]
    fn altgr_at_inserts_char_and_activates_mention() {
        let mut app = make_test_app();
        handle_normal_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('@'), KeyModifiers::CONTROL | KeyModifiers::ALT),
        );

        assert_eq!(app.input.text(), "@");
        assert!(app.mention.is_some());
    }

    #[test]
    fn ctrl_backspace_and_delete_use_word_operations() {
        let mut app = make_test_app();
        app.input.set_text("hello world");

        handle_normal_key(&mut app, KeyEvent::new(KeyCode::Backspace, KeyModifiers::CONTROL));
        assert_eq!(app.input.text(), "hello ");

        app.input.move_home();
        handle_normal_key(&mut app, KeyEvent::new(KeyCode::Delete, KeyModifiers::CONTROL));
        assert_eq!(app.input.text(), " ");
    }

    #[test]
    fn ctrl_z_and_y_undo_and_redo_textarea_history() {
        let mut app = make_test_app();
        app.input.set_text("hello world");

        handle_normal_key(&mut app, KeyEvent::new(KeyCode::Backspace, KeyModifiers::CONTROL));
        assert_eq!(app.input.text(), "hello ");

        handle_normal_key(&mut app, KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL));
        assert_eq!(app.input.text(), "hello world");

        handle_normal_key(&mut app, KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL));
        assert_eq!(app.input.text(), "hello ");
    }

    #[test]
    fn ctrl_left_right_move_by_word() {
        let mut app = make_test_app();
        app.input.set_text("hello world");
        app.input.move_home();

        handle_normal_key(&mut app, KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL));
        assert!(app.input.cursor_col() > 0);

        handle_normal_key(&mut app, KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL));
        assert_eq!(app.input.cursor_col(), 0);
    }

    #[test]
    fn help_overlay_left_right_switches_help_view_tab() {
        let mut app = make_test_app();
        app.input.set_text("?");
        app.help_view = HelpView::Keys;

        dispatch_key_by_focus(&mut app, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(app.help_view, HelpView::SlashCommands);

        dispatch_key_by_focus(&mut app, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(app.help_view, HelpView::Subagents);

        dispatch_key_by_focus(&mut app, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(app.help_view, HelpView::SlashCommands);

        dispatch_key_by_focus(&mut app, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(app.help_view, HelpView::Keys);
    }

    #[test]
    fn tab_toggles_todo_focus_target_for_open_todos() {
        let mut app = make_test_app();
        app.todos.push(TodoItem {
            content: "Task".into(),
            status: TodoStatus::Pending,
            active_form: String::new(),
        });
        app.show_todo_panel = true;

        handle_normal_key(&mut app, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.focus_owner(), FocusOwner::TodoList);

        handle_normal_key(&mut app, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.focus_owner(), FocusOwner::Input);
    }

    #[test]
    fn up_down_in_todo_focus_changes_todo_selection() {
        let mut app = make_test_app();
        app.todos = vec![
            TodoItem {
                content: "Task 1".into(),
                status: TodoStatus::Pending,
                active_form: String::new(),
            },
            TodoItem {
                content: "Task 2".into(),
                status: TodoStatus::InProgress,
                active_form: String::new(),
            },
            TodoItem {
                content: "Task 3".into(),
                status: TodoStatus::Pending,
                active_form: String::new(),
            },
        ];
        app.show_todo_panel = true;
        app.claim_focus_target(FocusTarget::TodoList);
        app.todo_selected = 1;

        let before_cursor_row = app.input.cursor_row();
        let before_cursor_col = app.input.cursor_col();
        handle_normal_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.todo_selected, 2);
        assert_eq!(app.input.cursor_row(), before_cursor_row);
        assert_eq!(app.input.cursor_col(), before_cursor_col);

        handle_normal_key(&mut app, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.todo_selected, 1);
    }

    #[test]
    fn permission_owner_overrides_todo_focus_for_up_down() {
        let mut app = make_test_app();
        app.todos.push(TodoItem {
            content: "Task".into(),
            status: TodoStatus::Pending,
            active_form: String::new(),
        });
        app.show_todo_panel = true;
        app.claim_focus_target(FocusTarget::TodoList);
        app.todo_selected = 0;
        let _rx_a = attach_pending_permission(
            &mut app,
            "perm-a",
            vec![
                model::PermissionOption::new(
                    "allow",
                    "Allow",
                    model::PermissionOptionKind::AllowOnce,
                ),
                model::PermissionOption::new(
                    "deny",
                    "Deny",
                    model::PermissionOptionKind::RejectOnce,
                ),
            ],
            true,
        );
        let _rx_b = attach_pending_permission(
            &mut app,
            "perm-b",
            vec![
                model::PermissionOption::new(
                    "allow",
                    "Allow",
                    model::PermissionOptionKind::AllowOnce,
                ),
                model::PermissionOption::new(
                    "deny",
                    "Deny",
                    model::PermissionOptionKind::RejectOnce,
                ),
            ],
            false,
        );
        app.claim_focus_target(FocusTarget::Permission);

        handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
        );

        assert_eq!(app.pending_permission_ids, vec!["perm-b", "perm-a"]);
        assert_eq!(app.todo_selected, 0);
    }

    #[test]
    fn permission_focus_allows_typing_for_non_permission_keys() {
        let mut app = make_test_app();
        app.pending_permission_ids.push("perm-1".into());
        app.claim_focus_target(FocusTarget::Permission);

        handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE)),
        );

        assert_eq!(app.input.text(), "h");
    }

    #[test]
    fn permission_focus_allows_ctrl_t_toggle_todos() {
        let mut app = make_test_app();
        app.pending_permission_ids.push("perm-1".into());
        app.claim_focus_target(FocusTarget::Permission);
        app.todos.push(TodoItem {
            content: "Task".into(),
            status: TodoStatus::Pending,
            active_form: String::new(),
        });

        assert!(!app.show_todo_panel);
        handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL)),
        );
        assert!(app.show_todo_panel);
    }

    #[test]
    fn ctrl_h_toggles_header_visibility() {
        let mut app = make_test_app();
        assert!(app.show_header);

        handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL)),
        );
        assert!(!app.show_header);

        handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL)),
        );
        assert!(app.show_header);
    }

    #[test]
    fn ctrl_u_hides_update_hint_globally() {
        let mut app = make_test_app();
        app.update_check_hint = Some("Update available: v9.9.9 (current v0.2.0)".into());
        app.todos.push(TodoItem {
            content: "Task".into(),
            status: TodoStatus::Pending,
            active_form: String::new(),
        });
        app.show_todo_panel = true;
        app.claim_focus_target(FocusTarget::TodoList);
        assert_eq!(app.focus_owner(), FocusOwner::TodoList);

        handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL)),
        );

        assert!(app.update_check_hint.is_none());
    }

    fn attach_pending_permission(
        app: &mut App,
        tool_id: &str,
        options: Vec<model::PermissionOption>,
        focused: bool,
    ) -> oneshot::Receiver<model::RequestPermissionResponse> {
        let (response_tx, response_rx) = oneshot::channel();
        let mut tc = tool_call(tool_id, model::ToolCallStatus::InProgress);
        tc.pending_permission =
            Some(InlinePermission { options, response_tx, selected_index: 0, focused });
        app.messages.push(assistant_msg(vec![MessageBlock::ToolCall(Box::new(tc))]));
        let msg_idx = app.messages.len().saturating_sub(1);
        app.index_tool_call(tool_id.into(), msg_idx, 0);
        app.pending_permission_ids.push(tool_id.into());
        app.claim_focus_target(FocusTarget::Permission);
        response_rx
    }

    fn push_todo_and_focus(app: &mut App) {
        app.todos.push(TodoItem {
            content: "Task".into(),
            status: TodoStatus::Pending,
            active_form: String::new(),
        });
        app.show_todo_panel = true;
        app.claim_focus_target(FocusTarget::TodoList);
        assert_eq!(app.focus_owner(), FocusOwner::TodoList);
    }

    #[test]
    fn permission_ctrl_y_works_even_when_todo_focus_owns_navigation() {
        let mut app = make_test_app();
        let mut response_rx = attach_pending_permission(
            &mut app,
            "perm-1",
            vec![
                model::PermissionOption::new(
                    "allow",
                    "Allow",
                    model::PermissionOptionKind::AllowOnce,
                ),
                model::PermissionOption::new(
                    "deny",
                    "Deny",
                    model::PermissionOptionKind::RejectOnce,
                ),
            ],
            true,
        );

        // Override focus owner to todo to prove the quick shortcut is global.
        push_todo_and_focus(&mut app);

        handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL)),
        );

        let resp = response_rx.try_recv().expect("ctrl+y should resolve pending permission");
        let model::RequestPermissionOutcome::Selected(selected) = resp.outcome else {
            panic!("expected selected permission response");
        };
        assert_eq!(selected.option_id.clone(), "allow");
        assert!(app.pending_permission_ids.is_empty());
    }

    #[test]
    fn permission_ctrl_a_works_even_when_todo_focus_owns_navigation() {
        let mut app = make_test_app();
        let mut response_rx = attach_pending_permission(
            &mut app,
            "perm-1",
            vec![
                model::PermissionOption::new(
                    "allow-once",
                    "Allow once",
                    model::PermissionOptionKind::AllowOnce,
                ),
                model::PermissionOption::new(
                    "allow-always",
                    "Allow always",
                    model::PermissionOptionKind::AllowAlways,
                ),
                model::PermissionOption::new(
                    "deny",
                    "Deny",
                    model::PermissionOptionKind::RejectOnce,
                ),
            ],
            true,
        );
        push_todo_and_focus(&mut app);

        handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL)),
        );

        let resp = response_rx.try_recv().expect("ctrl+a should resolve pending permission");
        let model::RequestPermissionOutcome::Selected(selected) = resp.outcome else {
            panic!("expected selected permission response");
        };
        assert_eq!(selected.option_id.clone(), "allow-always");
        assert!(app.pending_permission_ids.is_empty());
    }

    #[test]
    fn permission_ctrl_n_works_even_when_mention_focus_owns_navigation() {
        let mut app = make_test_app();
        let mut response_rx = attach_pending_permission(
            &mut app,
            "perm-1",
            vec![
                model::PermissionOption::new(
                    "allow",
                    "Allow",
                    model::PermissionOptionKind::AllowOnce,
                ),
                model::PermissionOption::new(
                    "deny",
                    "Deny",
                    model::PermissionOptionKind::RejectOnce,
                ),
            ],
            true,
        );

        app.mention = Some(mention::MentionState::new(0, 0, String::new(), Vec::new()));
        app.claim_focus_target(FocusTarget::Mention);
        assert_eq!(app.focus_owner(), FocusOwner::Mention);

        handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL)),
        );

        let resp = response_rx.try_recv().expect("ctrl+n should resolve pending permission");
        let model::RequestPermissionOutcome::Selected(selected) = resp.outcome else {
            panic!("expected selected permission response");
        };
        assert_eq!(selected.option_id.clone(), "deny");
        assert!(app.pending_permission_ids.is_empty());
    }

    #[test]
    fn connecting_state_ctrl_c_with_non_empty_selection_does_not_quit() {
        let mut app = make_test_app();
        app.status = AppStatus::Connecting;
        app.rendered_input_lines = vec!["copy".to_owned()];
        app.selection = Some(crate::app::SelectionState {
            kind: crate::app::SelectionKind::Input,
            start: crate::app::SelectionPoint { row: 0, col: 0 },
            end: crate::app::SelectionPoint { row: 0, col: 4 },
            dragging: false,
        });

        handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        );

        assert!(!app.should_quit);
        assert!(app.selection.is_none());
    }

    #[test]
    fn connecting_state_allows_navigation_and_help_shortcuts() {
        let mut app = make_test_app();
        app.status = AppStatus::Connecting;
        app.help_view = HelpView::Keys;
        app.viewport.scroll_target = 2;
        assert!(app.show_header);

        // Chat navigation remains available during startup.
        handle_terminal_event(&mut app, Event::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)));
        assert_eq!(app.viewport.scroll_target, 1);
        handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
        );
        assert_eq!(app.viewport.scroll_target, 2);

        // Help toggle via "?" remains available.
        handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE)),
        );
        assert!(app.is_help_active());

        // Help tab navigation still works.
        handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)),
        );
        assert_eq!(app.help_view, HelpView::SlashCommands);

        // Global UI navigation shortcuts still work.
        handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL)),
        );
        assert!(!app.show_header);
    }

    #[test]
    fn connecting_state_blocks_input_shortcuts_and_tab() {
        let mut app = make_test_app();
        app.status = AppStatus::Connecting;
        app.input.set_text("seed");
        app.pending_submit = None;
        app.help_view = HelpView::Keys;

        for key in [
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('@'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
        ] {
            handle_terminal_event(&mut app, Event::Key(key));
        }

        assert_eq!(app.input.text(), "seed");
        assert!(app.pending_submit.is_none());
        assert_eq!(app.help_view, HelpView::Keys);
    }

    #[test]
    fn ctrl_c_with_non_empty_selection_does_not_quit_and_clears_selection() {
        let mut app = make_test_app();
        app.rendered_input_lines = vec!["copy".to_owned()];
        app.selection = Some(crate::app::SelectionState {
            kind: crate::app::SelectionKind::Input,
            start: crate::app::SelectionPoint { row: 0, col: 0 },
            end: crate::app::SelectionPoint { row: 0, col: 4 },
            dragging: false,
        });

        handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        );

        assert!(!app.should_quit);
        assert!(app.selection.is_none());
    }

    #[test]
    fn ctrl_c_without_selection_quits() {
        let mut app = make_test_app();
        app.selection = None;

        handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        );

        assert!(app.should_quit);
    }

    #[test]
    fn ctrl_c_second_press_after_copy_quits() {
        let mut app = make_test_app();
        app.rendered_input_lines = vec!["copy".to_owned()];
        app.selection = Some(crate::app::SelectionState {
            kind: crate::app::SelectionKind::Input,
            start: crate::app::SelectionPoint { row: 0, col: 0 },
            end: crate::app::SelectionPoint { row: 0, col: 4 },
            dragging: false,
        });

        handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        );
        assert!(!app.should_quit);
        assert!(app.selection.is_none());

        handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        );
        assert!(app.should_quit);
    }

    #[test]
    fn ctrl_c_with_zero_length_selection_quits() {
        let mut app = make_test_app();
        app.rendered_input_lines = vec!["copy".to_owned()];
        app.selection = Some(crate::app::SelectionState {
            kind: crate::app::SelectionKind::Input,
            start: crate::app::SelectionPoint { row: 0, col: 0 },
            end: crate::app::SelectionPoint { row: 0, col: 0 },
            dragging: false,
        });

        handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        );

        assert!(app.should_quit);
    }

    #[test]
    fn ctrl_c_with_whitespace_selection_copies_and_clears_selection() {
        let mut app = make_test_app();
        app.rendered_input_lines = vec!["   ".to_owned()];
        app.selection = Some(crate::app::SelectionState {
            kind: crate::app::SelectionKind::Input,
            start: crate::app::SelectionPoint { row: 0, col: 0 },
            end: crate::app::SelectionPoint { row: 0, col: 1 },
            dragging: false,
        });

        handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        );

        assert!(!app.should_quit);
        assert!(app.selection.is_none());
    }

    #[test]
    fn ctrl_q_quits_even_with_selection() {
        let mut app = make_test_app();
        app.selection = Some(crate::app::SelectionState {
            kind: crate::app::SelectionKind::Input,
            start: crate::app::SelectionPoint { row: 0, col: 0 },
            end: crate::app::SelectionPoint { row: 0, col: 0 },
            dragging: false,
        });

        handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL)),
        );

        assert!(app.should_quit);
    }

    #[test]
    fn connecting_state_ctrl_q_quits() {
        let mut app = make_test_app();
        app.status = AppStatus::Connecting;

        handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL)),
        );

        assert!(app.should_quit);
    }

    #[test]
    fn error_state_blocks_input_shortcuts() {
        let mut app = make_test_app();
        app.status = AppStatus::Error;
        app.input.set_text("seed");
        app.pending_submit = None;

        for key in [
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('@'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
        ] {
            handle_terminal_event(&mut app, Event::Key(key));
        }

        assert_eq!(app.input.text(), "seed");
        assert!(app.pending_submit.is_none());
    }

    #[test]
    fn error_state_ctrl_q_quits() {
        let mut app = make_test_app();
        app.status = AppStatus::Error;

        handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL)),
        );

        assert!(app.should_quit);
    }

    #[test]
    fn error_state_ctrl_c_quits() {
        let mut app = make_test_app();
        app.status = AppStatus::Error;

        handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        );

        assert!(app.should_quit);
    }

    #[test]
    fn error_state_blocks_paste_events() {
        let mut app = make_test_app();
        app.status = AppStatus::Error;

        handle_terminal_event(&mut app, Event::Paste("blocked".into()));

        assert!(app.pending_paste_text.is_empty());
        assert!(app.input.is_empty());
    }

    #[test]
    fn mouse_scroll_clears_selection_before_scrolling() {
        let mut app = make_test_app();
        app.viewport.scroll_target = 2;
        app.selection = Some(crate::app::SelectionState {
            kind: crate::app::SelectionKind::Chat,
            start: crate::app::SelectionPoint { row: 0, col: 0 },
            end: crate::app::SelectionPoint { row: 0, col: 1 },
            dragging: false,
        });

        handle_terminal_event(
            &mut app,
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::ScrollDown,
                column: 0,
                row: 0,
                modifiers: KeyModifiers::NONE,
            }),
        );

        assert!(app.selection.is_none());
        assert_eq!(app.viewport.scroll_target, 5);
    }

    #[test]
    fn mouse_down_on_scrollbar_rail_starts_drag_and_scrolls() {
        let mut app = make_test_app();
        app.rendered_chat_area = Rect::new(0, 0, 20, 10);
        app.viewport.height_prefix_sums = vec![30];
        app.viewport.scrollbar_thumb_top = 0.0;
        app.viewport.scrollbar_thumb_size = 3.0;
        app.selection = Some(crate::app::SelectionState {
            kind: crate::app::SelectionKind::Chat,
            start: crate::app::SelectionPoint { row: 0, col: 0 },
            end: crate::app::SelectionPoint { row: 0, col: 1 },
            dragging: false,
        });

        handle_terminal_event(
            &mut app,
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
                column: 19,
                row: 9,
                modifiers: KeyModifiers::NONE,
            }),
        );

        assert!(app.scrollbar_drag.is_some());
        assert!(app.selection.is_none());
        assert!(!app.viewport.auto_scroll);
        assert!(app.viewport.scroll_target > 0);
    }

    #[test]
    fn dragging_scrollbar_thumb_can_reach_bottom_and_top() {
        let mut app = make_test_app();
        app.rendered_chat_area = Rect::new(0, 0, 20, 10);
        app.viewport.height_prefix_sums = vec![30];
        app.viewport.scrollbar_thumb_top = 0.0;
        app.viewport.scrollbar_thumb_size = 3.0;

        handle_terminal_event(
            &mut app,
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
                column: 19,
                row: 0,
                modifiers: KeyModifiers::NONE,
            }),
        );
        handle_terminal_event(
            &mut app,
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::Drag(crossterm::event::MouseButton::Left),
                column: 19,
                row: 9,
                modifiers: KeyModifiers::NONE,
            }),
        );
        assert_eq!(app.viewport.scroll_target, 20);

        handle_terminal_event(
            &mut app,
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::Drag(crossterm::event::MouseButton::Left),
                column: 19,
                row: 0,
                modifiers: KeyModifiers::NONE,
            }),
        );
        assert_eq!(app.viewport.scroll_target, 0);

        handle_terminal_event(
            &mut app,
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::Up(crossterm::event::MouseButton::Left),
                column: 19,
                row: 0,
                modifiers: KeyModifiers::NONE,
            }),
        );
        assert!(app.scrollbar_drag.is_none());
    }

    #[test]
    fn mention_owner_overrides_todo_focus_then_releases_back() {
        let mut app = make_test_app();
        app.todos.push(TodoItem {
            content: "Task".into(),
            status: TodoStatus::Pending,
            active_form: String::new(),
        });
        app.show_todo_panel = true;
        app.claim_focus_target(FocusTarget::TodoList);
        app.mention = Some(mention::MentionState::new(0, 0, String::new(), Vec::new()));
        app.claim_focus_target(FocusTarget::Mention);

        handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        );

        assert!(app.mention.is_none());
        assert_eq!(app.focus_owner(), FocusOwner::TodoList);
    }

    #[test]
    fn up_down_without_focus_scrolls_chat() {
        let mut app = make_test_app();
        app.viewport.scroll_target = 5;
        app.viewport.auto_scroll = true;

        handle_normal_key(&mut app, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.viewport.scroll_target, 4);
        assert!(!app.viewport.auto_scroll);

        handle_normal_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.viewport.scroll_target, 5);
    }

    #[test]
    fn up_down_moves_input_cursor_when_multiline() {
        let mut app = make_test_app();
        app.input.set_text("line1\nline2\nline3");
        let _ = app.input.set_cursor(1, 3);
        app.viewport.scroll_target = 7;

        handle_normal_key(&mut app, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.input.cursor_row(), 0);
        assert_eq!(app.viewport.scroll_target, 7);

        handle_normal_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.input.cursor_row(), 1);
        assert_eq!(app.viewport.scroll_target, 7);
    }

    #[test]
    fn down_at_input_bottom_falls_back_to_chat_scroll() {
        let mut app = make_test_app();
        app.input.set_text("line1\nline2");
        let _ = app.input.set_cursor(1, 0);
        app.viewport.scroll_target = 2;

        handle_normal_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));

        assert_eq!(app.input.cursor_row(), 1);
        assert_eq!(app.viewport.scroll_target, 3);
    }

    #[test]
    fn settings_view_routes_space_to_settings_handler_not_chat_input() {
        let mut app = make_test_app();
        let dir = tempfile::tempdir().expect("tempdir");
        app.settings_home_override = Some(dir.path().to_path_buf());
        app.cwd_raw = dir.path().to_string_lossy().to_string();
        crate::app::config::open(&mut app).expect("open settings");
        app.active_view = ActiveView::Config;
        app.config.selected_setting_index = crate::app::config::setting_specs()
            .iter()
            .position(|spec| spec.id == crate::app::config::SettingId::FastMode)
            .expect("fast mode setting row");
        app.input.set_text("seed");

        handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE)),
        );

        assert_eq!(app.input.text(), "seed");
        assert!(app.pending_submit.is_none());
        assert!(app.config.fast_mode_effective());
        assert!(app.config.last_error.is_none());
    }

    #[test]
    fn settings_view_routes_enter_to_close_not_chat_submit() {
        let mut app = make_test_app();
        let dir = tempfile::tempdir().expect("tempdir");
        app.settings_home_override = Some(dir.path().to_path_buf());
        app.cwd_raw = dir.path().to_string_lossy().to_string();
        crate::app::config::open(&mut app).expect("open settings");
        app.active_view = ActiveView::Config;
        app.input.set_text("seed");

        handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        );

        assert_eq!(app.active_view, ActiveView::Chat);
        assert_eq!(app.input.text(), "seed");
        assert!(app.pending_submit.is_none());
    }

    #[test]
    fn settings_view_ignores_paste_events() {
        let mut app = make_test_app();
        app.active_view = ActiveView::Config;

        handle_terminal_event(&mut app, Event::Paste("blocked".into()));

        assert!(app.pending_paste_text.is_empty());
        assert!(app.input.is_empty());
    }

    #[test]
    fn settings_view_ignores_mouse_events() {
        let mut app = make_test_app();
        app.active_view = ActiveView::Config;
        app.viewport.scroll_target = 4;
        app.selection = Some(SelectionState {
            kind: SelectionKind::Chat,
            start: SelectionPoint { row: 0, col: 0 },
            end: SelectionPoint { row: 0, col: 1 },
            dragging: false,
        });

        handle_terminal_event(
            &mut app,
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::ScrollDown,
                column: 0,
                row: 0,
                modifiers: KeyModifiers::NONE,
            }),
        );

        assert_eq!(app.viewport.scroll_target, 4);
        assert!(app.selection.is_some());
    }

    #[test]
    fn trusted_view_accept_key_does_not_edit_chat_input() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".claude.json");
        std::fs::write(&path, "{\n  \"projects\": {}\n}\n").expect("write");

        let mut app = make_test_app();
        app.active_view = ActiveView::Trusted;
        app.input.set_text("seed");
        app.cwd_raw = dir.path().join("project").to_string_lossy().to_string();
        app.config.preferences_path = Some(path);
        app.trust.status = crate::app::trust::TrustStatus::Untrusted;
        app.trust.project_key =
            crate::app::trust::store::normalize_project_key(std::path::Path::new(&app.cwd_raw));

        handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE)),
        );

        assert_eq!(app.active_view, ActiveView::Chat);
        assert_eq!(app.input.text(), "seed");
        assert!(app.pending_paste_text.is_empty());
        assert!(app.startup_connection_requested);
    }

    #[test]
    fn trusted_view_ignores_paste_events() {
        let mut app = make_test_app();
        app.active_view = ActiveView::Trusted;

        handle_terminal_event(&mut app, Event::Paste("blocked".into()));

        assert!(app.pending_paste_text.is_empty());
        assert!(app.input.is_empty());
    }

    #[test]
    fn buffered_paste_char_does_not_force_redraw() {
        let mut app = make_test_app();
        let now = Instant::now();

        assert_eq!(
            app.paste_burst.on_char('a', now),
            super::super::paste_burst::CharAction::Passthrough('a')
        );
        assert_eq!(
            app.paste_burst.on_char('b', now + Duration::from_millis(1)),
            super::super::paste_burst::CharAction::Consumed
        );
        assert_eq!(
            app.paste_burst.on_char('c', now + Duration::from_millis(2)),
            super::super::paste_burst::CharAction::RetroCapture(1)
        );

        app.needs_redraw = false;
        handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE)),
        );

        assert!(!app.needs_redraw);
        assert!(app.input.is_empty());
    }

    #[test]
    fn trusted_view_ignores_mouse_events() {
        let mut app = make_test_app();
        app.active_view = ActiveView::Trusted;
        app.viewport.scroll_target = 4;
        app.selection = Some(SelectionState {
            kind: SelectionKind::Chat,
            start: SelectionPoint { row: 0, col: 0 },
            end: SelectionPoint { row: 0, col: 1 },
            dragging: false,
        });

        handle_terminal_event(
            &mut app,
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::ScrollDown,
                column: 0,
                row: 0,
                modifiers: KeyModifiers::NONE,
            }),
        );

        assert_eq!(app.viewport.scroll_target, 4);
        assert!(app.selection.is_some());
    }

    #[test]
    fn internal_error_detection_accepts_xml_payload() {
        use crate::agent::error_handling::looks_like_internal_error;
        let payload =
            "<error><code>-32603</code><message>Adapter process crashed</message></error>";
        assert!(looks_like_internal_error(payload));
    }

    #[test]
    fn internal_error_detection_rejects_plain_bash_failure() {
        use crate::agent::error_handling::looks_like_internal_error;
        let payload = "bash: unknown_command: command not found";
        assert!(!looks_like_internal_error(payload));
    }

    #[test]
    fn summarize_internal_error_prefers_xml_message() {
        use crate::agent::error_handling::summarize_internal_error;
        let payload =
            "<error><code>-32603</code><message>Adapter process crashed</message></error>";
        assert_eq!(summarize_internal_error(payload), "Adapter process crashed");
    }

    #[test]
    fn summarize_internal_error_reads_json_rpc_message() {
        use crate::agent::error_handling::summarize_internal_error;
        let payload = r#"{"jsonrpc":"2.0","error":{"code":-32603,"message":"internal rpc fault"}}"#;
        assert_eq!(summarize_internal_error(payload), "internal rpc fault");
    }

    #[test]
    fn internal_error_detection_accepts_permission_zod_payload() {
        use crate::agent::error_handling::looks_like_internal_error;
        let payload = "Tool permission request failed: ZodError: [{\"message\":\"Invalid input\"}]";
        assert!(looks_like_internal_error(payload));
    }

    #[test]
    fn summarize_internal_error_prefers_permission_failure_summary() {
        use crate::agent::error_handling::summarize_internal_error;
        let payload = "Tool permission request failed: ZodError: [{\"message\":\"Invalid input: expected record, received undefined\"}]";
        assert_eq!(
            summarize_internal_error(payload),
            "Tool permission request failed: Invalid input: expected record, received undefined"
        );
    }
}
