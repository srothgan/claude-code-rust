// SPDX-License-Identifier: Apache-2.0

use super::super::{App, ChatMessage, MessageBlock, MessageRole, TextBlock};
use crate::agent::model;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ChatResetKind {
    InitialConnection,
    Replacement,
}

impl ChatResetKind {
    const fn preserves_draft(self) -> bool {
        matches!(self, Self::InitialConnection)
    }

    const fn preserves_welcome_tip(self) -> bool {
        matches!(self, Self::InitialConnection)
    }

    const fn repaints_inline_viewport(self) -> bool {
        matches!(self, Self::InitialConnection)
    }
}

pub(super) fn reset_for_new_session(
    app: &mut App,
    session_id: model::SessionId,
    current_model: model::CurrentModel,
    mode: Option<super::super::ModeState>,
    fast_mode: model::FastModeSnapshot,
    reset_kind: ChatResetKind,
) {
    reset_session_identity_state(app, session_id, current_model, mode, fast_mode);
    reset_messages_for_new_session(app, reset_kind.preserves_welcome_tip());
    reset_input_state_for_new_session(app, reset_kind);
    reset_interaction_state_for_new_session(app);
    reset_render_state_for_new_session(app);
    reset_cache_and_footer_state_for_new_session(app, reset_kind);
    app.sync_git_context();
}

pub(super) fn reset_for_conversation(
    app: &mut App,
    new_conversation_id: &str,
    trigger: Option<&str>,
    timestamp: Option<&str>,
    user_message_uuid: Option<&str>,
) {
    app.pending_user_messages.clear();
    app.bump_session_scope_epoch();
    app.session_runtime.conversation_id = Some(new_conversation_id.to_owned());
    super::compaction::reset(app);
    app.session_runtime.session_usage = super::super::SessionUsageState::default();
    app.sdk_inventory.clear_rewind_targets();
    app.status = super::super::AppStatus::Ready;
    app.session_runtime.runtime_session_state = None;
    app.session_runtime.prompt_suggestion = None;
    app.turn.reset_for_new_session();
    app.pending_interaction_ids.clear();
    app.clear_tool_call_index();
    app.sdk_inventory.tasks.clear();
    app.focus = super::super::FocusManager::default();
    clear_cached_active_session_title(app);
    reset_messages_for_new_session(app, false);
    app.chat_render.reset();
    app.mention = None;
    app.slash.clear();
    app.subagent = None;
    crate::app::usage::reset_for_session_change(app);
    app.request_chat_repaint();
    tracing::info!(
        target: crate::logging::targets::APP_SESSION,
        event_name = "conversation_reset_applied",
        message = "SDK conversation reset applied",
        outcome = "success",
        new_conversation_id,
        trigger,
        timestamp,
        user_message_uuid,
    );
}

fn clear_cached_active_session_title(app: &mut App) {
    // The bridge sends the title the session reports after the reset.
    app.session_runtime.session_title = None;
    let Some(session_id) = app.session_runtime.session_id.as_ref() else { return };
    let Some(session) =
        app.recent_sessions.iter_mut().find(|session| session.session_id == session_id.as_str())
    else {
        return;
    };
    session.custom_title = None;
    session.summary.clear();
    session.first_prompt = None;
}

fn reset_session_identity_state(
    app: &mut App,
    session_id: model::SessionId,
    current_model: model::CurrentModel,
    mode: Option<super::super::ModeState>,
    fast_mode: model::FastModeSnapshot,
) {
    app.pending_user_messages.clear();
    app.bump_session_scope_epoch();
    app.session_runtime.activate_session(session_id);
    app.session_runtime.current_model = Some(current_model.clone());
    app.session_runtime.mode = mode;
    app.session_runtime.config_options.clear();
    if let Some(requested_id) = current_model.requested_id {
        app.session_runtime
            .config_options
            .insert("model".to_owned(), serde_json::Value::String(requested_id));
    }
    app.session_runtime.login_hint = None;
    super::compaction::reset(app);
    app.session_runtime.session_usage = super::super::SessionUsageState::default();
    app.sdk_inventory.clear_rewind_targets();
    app.status = super::super::AppStatus::Ready;
    app.session_runtime.fast_mode_state = fast_mode.state;
    app.session_runtime.fast_mode_disabled_reason = fast_mode.disabled_reason;
    app.session_runtime.runtime_session_state = None;
    app.session_runtime.prompt_suggestion = None;
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

fn reset_input_state_for_new_session(app: &mut App, reset_kind: ChatResetKind) {
    if !reset_kind.preserves_draft() {
        app.input.clear();
        app.committed_mentions.clear();
        app.paste.discard_input();
    }
    app.pending_submit = None;
}

fn reset_interaction_state_for_new_session(app: &mut App) {
    app.turn.reset_for_new_session();
    app.pending_interaction_ids.clear();
    app.clear_tool_call_index();
    app.sdk_inventory.tasks.clear();
    app.focus = super::super::FocusManager::default();
    app.sdk_inventory.available_commands.clear();
    app.sdk_inventory.available_agents.clear();
}

fn reset_render_state_for_new_session(app: &mut App) {
    app.chat_render.reset();
    app.mention = None;
    crate::app::file_index::reset(app);
    app.slash.clear();
    app.subagent = None;
}

fn reset_cache_and_footer_state_for_new_session(app: &mut App, reset_kind: ChatResetKind) {
    app.mcp = super::super::McpState::default();
    crate::app::usage::reset_for_session_change(app);
    crate::app::plugins::reset_for_session_change(app);
    if reset_kind.repaints_inline_viewport() {
        app.request_chat_repaint();
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
        && chunk.source_message_uuid.as_deref().is_none_or(|uuid| {
            last.blocks.iter().all(|block| match block {
                MessageBlock::Text(text) => {
                    text.source_message_uuids.is_empty() || text.has_source_message_uuid(uuid)
                }
                _ => true,
            })
        })
    {
        if let Some(MessageBlock::Text(block)) = last.blocks.last_mut() {
            block.text.push_str(&text.text);
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
        let completed_tool = match update {
            model::SessionUpdate::ToolCall(tool) if tool.status.is_terminal() => {
                Some(tool.tool_call_id.as_str())
            }
            model::SessionUpdate::ToolCallUpdate(tool)
                if tool.fields.status.is_some_and(model::ToolCallStatus::is_terminal) =>
            {
                Some(tool.tool_call_id.as_str())
            }
            _ => None,
        };
        if let Some(tool_id) = completed_tool
            && let Some(session_id) = &app.session_runtime.session_id
        {
            app.notifications.observe_history_tool(session_id.as_str(), tool_id);
        }
        match update {
            model::SessionUpdate::NotificationUpdate { notification, .. } => {
                super::notifications::handle_sdk_notification(app, notification, true);
            }
            model::SessionUpdate::UserMessageChunk(chunk) => {
                app.clear_active_turn_assistant();
                append_resume_user_message_chunk(app, chunk);
            }
            _ => super::handle_session_update(
                app,
                update.clone(),
                super::TranscriptUpdateSource::HistoryReplay,
            ),
        }
    }
    for message in &mut app.transcript.messages {
        message.timing.discard_replay_observations();
    }
    app.finalize_turn_runtime_artifacts(model::ToolCallStatus::Failed);
    app.clear_active_turn_assistant();
    super::compaction::reset(app);
    app.status = super::super::AppStatus::Ready;
    app.turn.clear_cancel_state();
    app.enforce_history_retention_tracked();
}

#[cfg(test)]
mod tests {
    use super::{ChatResetKind, reset_for_new_session};
    use crate::agent::model;
    use crate::app::App;

    #[test]
    fn session_reset_does_not_cancel_requested_shutdown() {
        let mut app = App::test_default();
        app.request_shutdown();

        reset_for_new_session(
            &mut app,
            model::SessionId::new("session-2"),
            model::CurrentModel::new("test", "test", "test").authoritative(true),
            None,
            model::FastModeSnapshot::new(model::FastModeState::Off, None),
            ChatResetKind::Replacement,
        );

        assert!(app.shutdown_requested());
    }
    #[test]
    fn connecting_paste_survives_acknowledgement_before_input_drain_finalization() {
        use crate::app::test_support::{
            app_with_connection, drain_session_refreshes, expect_session_prompt, press,
            session_connection, session_image,
        };
        for pasted in ["accepted \u{754c}".to_owned(), "large \u{1f980} paste\n".repeat(120)] {
            let directory = tempfile::tempdir().unwrap();
            let (mut app, mut rx) = app_with_connection();
            app.status = crate::app::AppStatus::Connecting;
            app.session_runtime.session_id = None;
            app.input.set_text("pre \u{754c} |tail [Image #1]");
            app.input.set_cursor(0, 6);
            let image = session_image();
            app.input.image_payloads.push(image.clone());
            crate::app::handle_terminal_event(
                &mut app,
                crossterm::event::Event::Paste(pasted.clone()),
            );
            crate::app::events::handle_client_event(
                &mut app,
                session_connection("connected-session", directory.path()),
            );
            drain_session_refreshes(&mut rx, "connected-session");
            crate::app::finalize_pending_paste_event(&mut app);
            let snapshot = app.input.snapshot();
            crate::app::finalize_pending_paste_event(&mut app);
            assert_eq!(
                app.input.snapshot(),
                snapshot,
                "finalization does not duplicate accepted paste"
            );
            assert_eq!(app.input.images(), std::slice::from_ref(&image));
            assert!(rx.try_recv().is_err());
            press(&mut app, crossterm::event::KeyCode::Char('!'));
            crate::app::input_submit::submit_input(&mut app);
            let pastes = if pasted.lines().count() > 1 { vec![pasted.clone()] } else { vec![] };
            expect_session_prompt(
                &mut rx,
                "connected-session",
                &format!("pre \u{754c} {pasted}!|tail [Image #1]"),
                &[image],
                &pastes,
            );
            assert!(app.input.is_empty());
        }
    }
    #[test]
    fn replacement_discards_accepted_paste_and_held_typing_before_restoring_input() {
        use crate::app::test_support::{
            EventClock, app_with_connection, drain_session_refreshes, expect_session_prompt, press,
            session_replacement,
        };
        for bracketed in [false, true] {
            let clock = EventClock::new();
            let directory = tempfile::tempdir().unwrap();
            let (mut app, mut rx) = app_with_connection();
            app.input.set_text("old draft ");
            if bracketed {
                crate::app::handle_terminal_event(
                    &mut app,
                    crossterm::event::Event::Paste("old queued paste".into()),
                );
            } else {
                for ch in "abc".chars() {
                    press(&mut app, crossterm::event::KeyCode::Char(ch));
                }
            }
            crate::app::events::handle_client_event(
                &mut app,
                session_replacement(
                    "replacement",
                    directory.path(),
                    vec![],
                    Some("restored source"),
                ),
            );
            drain_session_refreshes(&mut rx, "replacement");
            if let Some(action) = app.paste.burst.tick(clock.at(1)) {
                crate::app::apply_paste_burst_flush(&mut app, action);
            }
            crate::app::finalize_pending_paste_event(&mut app);
            assert_eq!(app.input.text(), "restored source");
            press(&mut app, crossterm::event::KeyCode::Char('!'));
            crate::app::input_submit::submit_input(&mut app);
            expect_session_prompt(&mut rx, "replacement", "restored source!", &[], &[]);
        }
    }
}
