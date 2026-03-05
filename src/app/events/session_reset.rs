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

use super::super::{App, BlockCache, ChatMessage, IncrementalMarkdown, MessageBlock, MessageRole};
use crate::agent::model;

pub(super) fn reset_for_new_session(
    app: &mut App,
    session_id: model::SessionId,
    model_name: String,
    mode: Option<super::super::ModeState>,
) {
    crate::agent::events::kill_all_terminals(&app.terminals);

    reset_session_identity_state(app, session_id, model_name, mode);
    reset_messages_for_new_session(app);
    reset_input_state_for_new_session(app);
    reset_interaction_state_for_new_session(app);
    reset_render_state_for_new_session(app);
    reset_cache_and_footer_state_for_new_session(app);
    app.refresh_git_branch();
}

fn reset_session_identity_state(
    app: &mut App,
    session_id: model::SessionId,
    model_name: String,
    mode: Option<super::super::ModeState>,
) {
    app.session_id = Some(session_id);
    app.model_name = model_name;
    app.mode = mode;
    app.config_options.clear();
    app.config_options
        .insert("model".to_owned(), serde_json::Value::String(app.model_name.clone()));
    app.login_hint = None;
    super::clear_compaction_state(app, false);
    app.session_usage = super::super::SessionUsageState::default();
    app.fast_mode_state = model::FastModeState::Off;
    app.last_rate_limit_update = None;
    app.should_quit = false;
    app.files_accessed = 0;
    app.cancelled_turn_pending_hint = false;
    app.pending_cancel_origin = None;
    app.pending_auto_submit_after_cancel = false;
}

fn reset_messages_for_new_session(app: &mut App) {
    app.messages.clear();
    app.history_retention_stats = super::super::state::HistoryRetentionStats::default();
    app.messages.push(ChatMessage::welcome_with_recent(
        &app.model_name,
        &app.cwd,
        &app.recent_sessions,
    ));
    app.viewport = super::super::ChatViewport::new();
}

fn reset_input_state_for_new_session(app: &mut App) {
    app.input.clear();
    app.pending_submit = false;
    app.drain_key_count = 0;
    app.paste_burst.reset();
    app.pending_paste_text.clear();
    app.pending_paste_session = None;
    app.active_paste_session = None;
    app.paste_burst_start = None;
}

fn reset_interaction_state_for_new_session(app: &mut App) {
    app.pending_permission_ids.clear();
    app.clear_tool_scope_tracking();
    app.tool_call_index.clear();
    app.todos.clear();
    app.show_todo_panel = false;
    app.todo_scroll = 0;
    app.todo_selected = 0;
    app.focus = super::super::FocusManager::default();
    app.available_commands.clear();
    app.available_agents.clear();
}

fn reset_render_state_for_new_session(app: &mut App) {
    app.selection = None;
    app.scrollbar_drag = None;
    app.rendered_chat_lines.clear();
    app.rendered_chat_area = ratatui::layout::Rect::default();
    app.rendered_input_lines.clear();
    app.rendered_input_area = ratatui::layout::Rect::default();
    app.mention = None;
    app.slash = None;
    app.subagent = None;
    app.file_cache = None;
}

fn reset_cache_and_footer_state_for_new_session(app: &mut App) {
    app.cached_todo_compact = None;
    app.cached_header_line = None;
    app.cached_footer_line = None;
    app.terminal_tool_calls.clear();
    app.force_redraw = true;
    app.needs_redraw = true;
}

fn append_resume_user_message_chunk(app: &mut App, chunk: &model::ContentChunk) {
    let model::ContentBlock::Text(text) = &chunk.content else {
        return;
    };
    if text.text.is_empty() {
        return;
    }

    if let Some(last) = app.messages.last_mut()
        && matches!(last.role, MessageRole::User)
    {
        if let Some(MessageBlock::Text(existing, cache, incr)) = last.blocks.last_mut() {
            existing.push_str(&text.text);
            incr.append(&text.text);
            cache.invalidate();
        } else {
            let mut incr = IncrementalMarkdown::default();
            incr.append(&text.text);
            last.blocks.push(MessageBlock::Text(text.text.clone(), BlockCache::default(), incr));
        }
        return;
    }

    let mut incr = IncrementalMarkdown::default();
    incr.append(&text.text);
    app.messages.push(ChatMessage {
        role: MessageRole::User,
        blocks: vec![MessageBlock::Text(text.text.clone(), BlockCache::default(), incr)],
        usage: None,
    });
}

pub(super) fn load_resume_history(app: &mut App, history_updates: &[model::SessionUpdate]) {
    app.messages.clear();
    app.history_retention_stats = super::super::state::HistoryRetentionStats::default();
    app.messages.push(ChatMessage::welcome_with_recent(
        &app.model_name,
        &app.cwd,
        &app.recent_sessions,
    ));
    for update in history_updates {
        match update {
            model::SessionUpdate::UserMessageChunk(chunk) => {
                append_resume_user_message_chunk(app, chunk);
            }
            _ => super::handle_session_update(app, update.clone()),
        }
    }
    let _ = app.finalize_in_progress_tool_calls(model::ToolCallStatus::Failed);
    app.enforce_history_retention_tracked();
    app.viewport = super::super::ChatViewport::new();
    app.viewport.engage_auto_scroll();
}
