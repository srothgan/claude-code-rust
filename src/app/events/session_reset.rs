// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use super::super::{App, ChatMessage, MessageBlock, MessageRole, TextBlock};
use crate::agent::model;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ChatResetRenderMode {
    PreserveInlineViewport,
    DeferTranscriptRender,
}

pub(super) fn reset_for_new_session(
    app: &mut App,
    session_id: model::SessionId,
    current_model: model::CurrentModel,
    mode: Option<super::super::ModeState>,
    fast_mode: model::FastModeSnapshot,
    preserve_current_welcome_tip: bool,
    render_mode: ChatResetRenderMode,
) {
    reset_session_identity_state(app, session_id, current_model, mode, fast_mode);
    reset_messages_for_new_session(app, preserve_current_welcome_tip);
    reset_input_state_for_new_session(app);
    reset_interaction_state_for_new_session(app);
    reset_render_state_for_new_session(app);
    reset_cache_and_footer_state_for_new_session(app, render_mode);
    app.sync_git_context();
}

fn reset_session_identity_state(
    app: &mut App,
    session_id: model::SessionId,
    current_model: model::CurrentModel,
    mode: Option<super::super::ModeState>,
    fast_mode: model::FastModeSnapshot,
) {
    app.bump_session_scope_epoch();
    app.session_runtime.session_id = Some(session_id);
    app.session_runtime.current_model = Some(current_model.clone());
    app.session_runtime.mode = mode;
    app.session_runtime.config_options.clear();
    if let Some(requested_id) = current_model.requested_id {
        app.session_runtime
            .config_options
            .insert("model".to_owned(), serde_json::Value::String(requested_id));
    }
    app.session_runtime.login_hint = None;
    super::clear_compaction_state(app, false);
    app.session_runtime.session_usage = super::super::SessionUsageState::default();
    app.sdk_inventory.clear_rewind_targets();
    app.status = super::super::AppStatus::Ready;
    app.session_runtime.fast_mode_state = fast_mode.state;
    app.session_runtime.fast_mode_disabled_reason = fast_mode.disabled_reason;
    app.session_runtime.runtime_session_state = None;
    app.session_runtime.prompt_suggestion = None;
    app.session_runtime.last_rate_limit_update = None;
    app.should_quit = false;
    app.files_accessed = 0;
    app.turn.clear_cancel_state();
    app.session_runtime.account_info = None;
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
}

fn reset_input_state_for_new_session(app: &mut App) {
    app.input.clear();
    app.pending_submit = None;
    app.paste.clear_all_sessions();
    app.pending_images.clear();
}

fn reset_interaction_state_for_new_session(app: &mut App) {
    app.turn.reset_for_new_session();
    app.clear_tool_scope_tracking();
    app.clear_tool_call_index();
    app.sdk_inventory.tasks.clear();
    app.focus = super::super::FocusManager::default();
    app.sdk_inventory.available_commands.clear();
    app.sdk_inventory.available_agents.clear();
    app.config.clear_overlay();
    app.config.pending_session_title_change = None;
}

fn reset_render_state_for_new_session(app: &mut App) {
    app.chat_render.reset();
    app.mention = None;
    crate::app::file_index::reset(app);
    app.slash = None;
    app.subagent = None;
}

fn reset_cache_and_footer_state_for_new_session(app: &mut App, render_mode: ChatResetRenderMode) {
    app.mcp = super::super::McpState::default();
    crate::app::usage::reset_for_session_change(app);
    crate::app::plugins::reset_for_session_change(app);
    match render_mode {
        ChatResetRenderMode::PreserveInlineViewport => app.request_chat_repaint(),
        ChatResetRenderMode::DeferTranscriptRender => {}
    }
}

fn append_resume_user_message_chunk(app: &mut App, chunk: &model::ContentChunk) {
    let model::ContentBlock::Text(text) = &chunk.content else {
        return;
    };
    if text.text.is_empty() {
        return;
    }

    if let Some(last) = app.transcript.messages.last_mut()
        && matches!(last.role, MessageRole::User)
    {
        if let Some(MessageBlock::Text(block)) = last.blocks.last_mut() {
            block.text.push_str(&text.text);
            block.markdown.append(&text.text);
            block.add_source_message_uuid(chunk.source_message_uuid.as_deref());
            block.cache.invalidate();
        } else {
            last.blocks.push(MessageBlock::Text(
                TextBlock::from_complete(&text.text)
                    .with_source_message_uuid(chunk.source_message_uuid.as_deref()),
            ));
        }
        let last_idx = app.transcript.messages.len().saturating_sub(1);
        app.sync_after_message_blocks_changed(last_idx);
        return;
    }

    app.push_message_tracked(ChatMessage::new(
        MessageRole::User,
        vec![MessageBlock::Text(
            TextBlock::from_complete(&text.text)
                .with_source_message_uuid(chunk.source_message_uuid.as_deref()),
        )],
        None,
    ));
}

pub(super) fn load_resume_history(app: &mut App, history_updates: &[model::SessionUpdate]) {
    app.show_session_overview = false;
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
    super::clear_compaction_state(app, false);
    app.status = super::super::AppStatus::Ready;
    app.turn.clear_cancel_state();
    app.enforce_history_retention_tracked();
}

#[cfg(test)]
mod tests {
    use super::{ChatResetRenderMode, reset_for_new_session};
    use crate::agent::model;
    use crate::app::{App, ChatMessage, ChatRebuildKind};

    #[test]
    fn session_reset_clears_chat_render_measurement_state() {
        let mut app = App::test_default();
        app.chat_render.terminal_width = 120;
        app.chat_render.terminal_height = 40;
        app.chat_render.composer.total_rows = 6;
        app.chat_render.live_region.anchor_valid = true;
        app.chat_render.live_region.last_rendered_rows = 9;
        app.transcript.messages.push(ChatMessage::welcome(
            "1.2.3",
            "Pro",
            "/workspace/demo",
            "session-1",
        ));

        reset_for_new_session(
            &mut app,
            model::SessionId::new("session-2"),
            model::CurrentModel::new("test", "test", "test").authoritative(true),
            None,
            model::FastModeSnapshot::new(model::FastModeState::Off, None),
            false,
            ChatResetRenderMode::DeferTranscriptRender,
        );

        assert_eq!(app.chat_render.terminal_width, 0);
        assert_eq!(app.chat_render.terminal_height, 0);
        assert_eq!(app.chat_render.composer.total_rows, 0);
        assert!(!app.chat_render.live_region.anchor_valid);
        assert_eq!(app.chat_render.live_region.last_rendered_rows, 0);
    }

    #[test]
    fn startup_session_reset_preserves_inline_viewport_for_diffed_repaint() {
        let mut app = App::test_default();
        app.transcript.messages.push(ChatMessage::welcome("1.2.3", "-", "/workspace/demo", "-"));
        app.surface_dirty.chat.rebuild = ChatRebuildKind::None;
        app.surface_dirty.chat.repaint = false;

        reset_for_new_session(
            &mut app,
            model::SessionId::new("session-2"),
            model::CurrentModel::new("test", "test", "test").authoritative(true),
            None,
            model::FastModeSnapshot::new(model::FastModeState::Off, None),
            true,
            ChatResetRenderMode::PreserveInlineViewport,
        );

        assert_eq!(app.surface_dirty.chat.rebuild, ChatRebuildKind::None);
        assert!(app.surface_dirty.chat.repaint);
    }

    #[test]
    fn replacement_session_reset_defers_transcript_render() {
        let mut app = App::test_default();
        app.transcript.messages.push(ChatMessage::welcome(
            "1.2.3",
            "Pro",
            "/workspace/demo",
            "session-1",
        ));
        app.surface_dirty.chat.rebuild = ChatRebuildKind::None;
        app.surface_dirty.chat.repaint = false;

        reset_for_new_session(
            &mut app,
            model::SessionId::new("session-2"),
            model::CurrentModel::new("test", "test", "test").authoritative(true),
            None,
            model::FastModeSnapshot::new(model::FastModeState::Off, None),
            false,
            ChatResetRenderMode::DeferTranscriptRender,
        );

        assert_eq!(app.surface_dirty.chat.rebuild, ChatRebuildKind::None);
    }

    #[test]
    fn session_reset_clears_rewind_target_state() {
        let mut app = App::test_default();
        app.sdk_inventory.rewind_targets = vec![model::RewindTarget {
            uuid: "user-1".to_owned(),
            first_text: "prompt".to_owned(),
            input_text: "prompt".to_owned(),
            index: 0,
            previous_assistant_uuid: None,
        }];
        app.sdk_inventory.rewind_targets_session_id = Some(model::SessionId::new("session-1"));
        app.sdk_inventory.rewind_targets_in_flight = true;

        reset_for_new_session(
            &mut app,
            model::SessionId::new("session-2"),
            model::CurrentModel::new("test", "test", "test").authoritative(true),
            None,
            model::FastModeSnapshot::new(model::FastModeState::Off, None),
            false,
            ChatResetRenderMode::DeferTranscriptRender,
        );

        assert!(app.sdk_inventory.rewind_targets.is_empty());
        assert!(app.sdk_inventory.rewind_targets_session_id.is_none());
        assert!(!app.sdk_inventory.rewind_targets_in_flight);
    }
}
