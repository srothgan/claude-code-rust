// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::super::{
    App, BlockCache, ChatMessage, IncrementalMarkdown, MessageBlock, MessageRole, TextBlock,
    TextBlockSpacing,
};
use crate::agent::model;

pub(super) fn reset_for_new_session(
    app: &mut App,
    session_id: model::SessionId,
    current_model: model::CurrentModel,
    mode: Option<super::super::ModeState>,
    preserve_current_welcome_tip: bool,
) {
    crate::agent::events::kill_all_terminals(&app.terminals);

    reset_session_identity_state(app, session_id, current_model, mode);
    reset_messages_for_new_session(app, preserve_current_welcome_tip);
    reset_input_state_for_new_session(app);
    reset_interaction_state_for_new_session(app);
    reset_render_state_for_new_session(app);
    reset_cache_and_footer_state_for_new_session(app);
    app.sync_git_context();
}

fn reset_session_identity_state(
    app: &mut App,
    session_id: model::SessionId,
    current_model: model::CurrentModel,
    mode: Option<super::super::ModeState>,
) {
    app.bump_session_scope_epoch();
    app.session_id = Some(session_id);
    app.current_model = Some(current_model.clone());
    app.mode = mode;
    app.config_options.clear();
    if let Some(requested_id) = current_model.requested_id {
        app.config_options.insert("model".to_owned(), serde_json::Value::String(requested_id));
    }
    app.login_hint = None;
    super::clear_compaction_state(app, false);
    app.session_usage = super::super::SessionUsageState::default();
    app.fast_mode_state = model::FastModeState::Off;
    app.runtime_session_state = None;
    app.prompt_suggestion = None;
    app.last_rate_limit_update = None;
    app.should_quit = false;
    app.files_accessed = 0;
    app.cancelled_turn_pending_hint = false;
    app.pending_cancel_origin = None;
    app.pending_auto_submit_after_cancel = false;
    app.account_info = None;
}

fn reset_messages_for_new_session(app: &mut App, preserve_current_welcome_tip: bool) {
    let preserved_tip_seed =
        preserve_current_welcome_tip.then(|| app.current_welcome_tip_seed()).flatten();
    app.clear_messages_tracked();
    app.history_retention_stats = super::super::state::HistoryRetentionStats::default();
    let mut welcome = app.build_welcome_message();
    if let Some(tip_seed) = preserved_tip_seed {
        App::apply_welcome_tip_seed(&mut welcome, tip_seed);
    }
    app.push_message_tracked(welcome);
    app.sync_welcome_snapshot();
    app.viewport = super::super::ChatViewport::new();
}

fn reset_input_state_for_new_session(app: &mut App) {
    app.input.clear();
    app.help_open = false;
    app.pending_submit = None;
    app.pending_paste_text.clear();
    app.pending_paste_session = None;
    app.active_paste_session = None;
    app.pending_images.clear();
}

fn reset_interaction_state_for_new_session(app: &mut App) {
    app.pending_interaction_ids.clear();
    app.clear_tool_scope_tracking();
    app.tool_call_index.clear();
    app.todos.clear();
    app.show_todo_panel = false;
    app.todo_scroll = 0;
    app.todo_selected = 0;
    app.focus = super::super::FocusManager::default();
    app.available_commands.clear();
    app.available_agents.clear();
    app.config.overlay = None;
    app.config.pending_session_title_change = None;
}

fn reset_render_state_for_new_session(app: &mut App) {
    app.selection = None;
    app.scrollbar_drag = None;
    app.rendered_chat_lines.clear();
    app.rendered_chat_area = ratatui::layout::Rect::default();
    app.rendered_input_lines.clear();
    app.rendered_input_area = ratatui::layout::Rect::default();
    app.mention = None;
    crate::app::file_index::reset(app);
    app.slash = None;
    app.subagent = None;
    app.help_view = super::super::HelpView::default();
    app.help_dialog = crate::app::dialog::DialogState::default();
    app.help_visible_count = 0;
}

fn reset_cache_and_footer_state_for_new_session(app: &mut App) {
    app.cached_todo_compact = None;
    app.clear_terminal_tool_call_tracking();
    app.mcp = super::super::McpState::default();
    crate::app::usage::reset_for_session_change(app);
    crate::app::plugins::reset_for_session_change(app);
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
        if let Some(MessageBlock::Text(block)) = last.blocks.last_mut() {
            block.text.push_str(&text.text);
            block.markdown.append(&text.text);
            block.cache.invalidate();
        } else {
            let mut incr = IncrementalMarkdown::default();
            incr.append(&text.text);
            last.blocks.push(MessageBlock::Text(TextBlock {
                text: text.text.clone(),
                cache: BlockCache::default(),
                markdown: incr,
                trailing_spacing: TextBlockSpacing::default(),
            }));
        }
        let last_idx = app.messages.len().saturating_sub(1);
        app.sync_after_message_blocks_changed(last_idx);
        return;
    }

    let mut incr = IncrementalMarkdown::default();
    incr.append(&text.text);
    app.push_message_tracked(ChatMessage::new(
        MessageRole::User,
        vec![MessageBlock::Text(TextBlock {
            text: text.text.clone(),
            cache: BlockCache::default(),
            markdown: incr,
            trailing_spacing: TextBlockSpacing::default(),
        })],
        None,
    ));
}

pub(super) fn load_resume_history(app: &mut App, history_updates: &[model::SessionUpdate]) {
    let preserved_tip_seed = app.current_welcome_tip_seed();
    app.clear_messages_tracked();
    app.history_retention_stats = super::super::state::HistoryRetentionStats::default();
    let mut welcome = app.build_welcome_message();
    if let Some(tip_seed) = preserved_tip_seed {
        App::apply_welcome_tip_seed(&mut welcome, tip_seed);
    }
    app.push_message_tracked(welcome);
    app.sync_welcome_snapshot();
    for update in history_updates {
        match update {
            model::SessionUpdate::UserMessageChunk(chunk) => {
                app.clear_active_turn_assistant();
                append_resume_user_message_chunk(app, chunk);
            }
            _ => super::handle_session_update(app, update.clone()),
        }
    }
    app.finalize_turn_runtime_artifacts(model::ToolCallStatus::Failed);
    app.clear_active_turn_assistant();
    app.enforce_history_retention_tracked();
    app.viewport = super::super::ChatViewport::new();
    app.viewport.engage_auto_scroll();
}
