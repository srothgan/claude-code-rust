// SPDX-License-Identifier: Apache-2.0
use super::App;
use super::inline_interactions::{
    focus_next_inline_interaction, focused_interaction, focused_interaction_dirty_idx,
    focused_interaction_is_active, get_focused_interaction_tc, invalidate_if_changed,
    normalize_pending_interaction_queue, pop_next_valid_interaction_id,
};
use crate::agent::model;
use crate::app::keymap::InteractionAction;
use crate::app::keys::KeyOutcome;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::time::Instant;

/// Read one monotonic time source for admission, focus and routed activity.
pub(super) fn activity_now() -> Instant {
    #[cfg(test)]
    if let Some(now) = super::test_support::question_clock_now() {
        return now;
    }
    Instant::now()
}

/// Question activity and expiry belong to the rendered interaction, not app settings.
pub(super) fn record_activity(app: &mut App, now: Instant) {
    let ids = app.pending_interaction_ids.clone();
    for id in ids {
        let Some((mi, bi)) = app.lookup_tool_call(&id) else {
            continue;
        };
        if let Some(tool) = app.tool_call_at_mut(mi, bi)
            && let Some(question) = tool.pending_question.as_mut()
        {
            question.last_activity = now;
        }
    }
}

pub(super) fn tick_idle_timeout(app: &mut App, now: Instant) {
    if app.surface_mode != super::SurfaceMode::Chat || !focused_interaction_is_active(app) {
        record_activity(app, now);
        return;
    }
    if focused_question(app).is_some_and(|question| {
        question
            .idle_timeout
            .is_some_and(|timeout| now.saturating_duration_since(question.last_activity) >= timeout)
    }) {
        respond_question_with_selection(app, true);
        app.request_active_surface_repaint();
    }
}

fn focused_question(app: &App) -> Option<&crate::app::InlineQuestion> {
    focused_interaction(app)?.pending_question.as_ref()
}

pub(super) fn has_focused_question(app: &App) -> bool {
    focused_question(app).is_some()
}

pub(super) fn focused_question_is_editing_notes(app: &App) -> bool {
    focused_question(app).is_some_and(|question| question.editing_notes)
}

fn focused_question_option_count(app: &App) -> usize {
    focused_question(app).map_or(0, |question| question.prompt.options.len())
}

pub(super) fn execute_question_action(
    app: &mut App,
    action: InteractionAction,
    key: KeyEvent,
) -> KeyOutcome {
    normalize_pending_interaction_queue(app);
    if !has_focused_question(app) || !focused_interaction_is_active(app) {
        return KeyOutcome::Ignored;
    }
    record_activity(app, activity_now());

    if focused_question_is_editing_notes(app) {
        return execute_question_note_action(app, action, key);
    }

    let option_count = focused_question_option_count(app);
    let outcome = match action {
        InteractionAction::MovePrevious => {
            if option_count == 0 {
                return KeyOutcome::Handled(false);
            }
            move_question_option_left(app);
            KeyOutcome::Handled(true)
        }
        InteractionAction::MoveNext => {
            if option_count == 0 {
                return KeyOutcome::Handled(false);
            }
            move_question_option_right(app);
            KeyOutcome::Handled(true)
        }
        InteractionAction::MoveStart => {
            if option_count == 0 {
                return KeyOutcome::Handled(false);
            }
            move_question_option_to_start(app);
            KeyOutcome::Handled(true)
        }
        InteractionAction::MoveEnd => {
            if option_count == 0 {
                return KeyOutcome::Handled(false);
            }
            move_question_option_to_end(app);
            KeyOutcome::Handled(true)
        }
        InteractionAction::ToggleSelection => {
            if option_count == 0 {
                return KeyOutcome::Handled(false);
            }
            toggle_question_selection(app);
            KeyOutcome::Handled(true)
        }
        InteractionAction::ToggleNotes => {
            set_question_notes_editing(app, true);
            KeyOutcome::Handled(true)
        }
        InteractionAction::Confirm => {
            if option_count == 0 {
                return KeyOutcome::Ignored;
            }
            respond_question(app);
            KeyOutcome::Handled(true)
        }
        InteractionAction::Cancel => {
            respond_question_cancel(app);
            KeyOutcome::Handled(true)
        }
        InteractionAction::FocusNext => KeyOutcome::Ignored,
    };
    if matches!(
        action,
        InteractionAction::MovePrevious
            | InteractionAction::MoveNext
            | InteractionAction::MoveStart
            | InteractionAction::MoveEnd
    ) {
        // Navigation is an explicit single-choice selection; initial focus isn't.
        let dirty_idx = focused_interaction_dirty_idx(app);
        if let Some(tool) = get_focused_interaction_tc(app)
            && let Some(question) = tool.pending_question.as_mut()
            && !question.prompt.multi_select
        {
            question.selected_option_indices.clear();
            question.selected_option_indices.insert(question.focused_option_index);
            tool.invalidate_render_cache();
        }
        invalidate_if_changed(app, dirty_idx, true);
    }
    outcome
}

fn execute_question_note_action(
    app: &mut App,
    action: InteractionAction,
    key: KeyEvent,
) -> KeyOutcome {
    match action {
        InteractionAction::MovePrevious => {
            if !matches!(key.code, KeyCode::Up) {
                move_question_notes_cursor(app, -1);
            }
            KeyOutcome::Handled(true)
        }
        InteractionAction::MoveNext => {
            if !matches!(key.code, KeyCode::Down) {
                move_question_notes_cursor(app, 1);
            }
            KeyOutcome::Handled(true)
        }
        InteractionAction::MoveStart => {
            move_question_notes_cursor_to_start(app);
            KeyOutcome::Handled(true)
        }
        InteractionAction::MoveEnd => {
            move_question_notes_cursor_to_end(app);
            KeyOutcome::Handled(true)
        }
        InteractionAction::ToggleNotes => {
            set_question_notes_editing(app, false);
            KeyOutcome::Handled(true)
        }
        InteractionAction::Confirm => {
            respond_question(app);
            KeyOutcome::Handled(true)
        }
        InteractionAction::Cancel => {
            respond_question_cancel(app);
            KeyOutcome::Handled(true)
        }
        InteractionAction::FocusNext | InteractionAction::ToggleSelection => KeyOutcome::Ignored,
    }
}

pub(super) fn handle_question_note_key(app: &mut App, key: KeyEvent) -> Option<KeyOutcome> {
    normalize_pending_interaction_queue(app);
    if !has_focused_question(app)
        || !focused_interaction_is_active(app)
        || !focused_question_is_editing_notes(app)
    {
        return None;
    }

    match key.code {
        KeyCode::Backspace => {
            delete_question_note_char_before(app);
            Some(KeyOutcome::Handled(true))
        }
        KeyCode::Delete => {
            delete_question_note_char_after(app);
            Some(KeyOutcome::Handled(true))
        }
        KeyCode::Char(ch) if is_printable_question_note_modifiers(key.modifiers) => {
            insert_question_note_char(app, ch);
            Some(KeyOutcome::Handled(true))
        }
        _ => None,
    }
}

fn is_printable_question_note_modifiers(modifiers: KeyModifiers) -> bool {
    let ctrl_alt =
        modifiers.contains(KeyModifiers::CONTROL) && modifiers.contains(KeyModifiers::ALT);
    !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) || ctrl_alt
}

fn move_question_option_left(app: &mut App) {
    let dirty_idx = focused_interaction_dirty_idx(app);
    let mut changed = false;
    if let Some(tc) = get_focused_interaction_tc(app)
        && let Some(ref mut question) = tc.pending_question
    {
        let next = question.focused_option_index.saturating_sub(1);
        if next != question.focused_option_index {
            question.focused_option_index = next;
            tc.invalidate_render_cache();
            changed = true;
        }
    }
    invalidate_if_changed(app, dirty_idx, changed);
}

fn move_question_option_right(app: &mut App) {
    let dirty_idx = focused_interaction_dirty_idx(app);
    let mut changed = false;
    if let Some(tc) = get_focused_interaction_tc(app)
        && let Some(ref mut question) = tc.pending_question
        && question.focused_option_index + 1 < question.prompt.options.len()
    {
        question.focused_option_index += 1;
        tc.invalidate_render_cache();
        changed = true;
    }
    invalidate_if_changed(app, dirty_idx, changed);
}

fn move_question_option_to_start(app: &mut App) {
    let dirty_idx = focused_interaction_dirty_idx(app);
    let mut changed = false;
    if let Some(tc) = get_focused_interaction_tc(app)
        && let Some(ref mut question) = tc.pending_question
        && question.focused_option_index != 0
    {
        question.focused_option_index = 0;
        tc.invalidate_render_cache();
        changed = true;
    }
    invalidate_if_changed(app, dirty_idx, changed);
}

fn move_question_option_to_end(app: &mut App) {
    let dirty_idx = focused_interaction_dirty_idx(app);
    let mut changed = false;
    if let Some(tc) = get_focused_interaction_tc(app)
        && let Some(ref mut question) = tc.pending_question
        && let Some(last_idx) = question.prompt.options.len().checked_sub(1)
        && question.focused_option_index != last_idx
    {
        question.focused_option_index = last_idx;
        tc.invalidate_render_cache();
        changed = true;
    }
    invalidate_if_changed(app, dirty_idx, changed);
}

fn question_notes_byte_index(notes: &str, cursor: usize) -> usize {
    notes.char_indices().nth(cursor).map_or(notes.len(), |(idx, _)| idx)
}

fn insert_question_note_char(app: &mut App, ch: char) {
    let dirty_idx = focused_interaction_dirty_idx(app);
    let mut changed = false;
    if let Some(tc) = get_focused_interaction_tc(app)
        && let Some(ref mut question) = tc.pending_question
    {
        let idx = question_notes_byte_index(&question.notes, question.notes_cursor);
        question.notes.insert(idx, ch);
        question.notes_cursor += 1;
        tc.invalidate_render_cache();
        changed = true;
    }
    invalidate_if_changed(app, dirty_idx, changed);
}

fn delete_question_note_char_before(app: &mut App) {
    let dirty_idx = focused_interaction_dirty_idx(app);
    let mut changed = false;
    if let Some(tc) = get_focused_interaction_tc(app)
        && let Some(ref mut question) = tc.pending_question
        && question.notes_cursor > 0
    {
        let start = question_notes_byte_index(&question.notes, question.notes_cursor - 1);
        let end = question_notes_byte_index(&question.notes, question.notes_cursor);
        question.notes.replace_range(start..end, "");
        question.notes_cursor -= 1;
        tc.invalidate_render_cache();
        changed = true;
    }
    invalidate_if_changed(app, dirty_idx, changed);
}

fn delete_question_note_char_after(app: &mut App) {
    let dirty_idx = focused_interaction_dirty_idx(app);
    let mut changed = false;
    if let Some(tc) = get_focused_interaction_tc(app)
        && let Some(ref mut question) = tc.pending_question
        && question.notes_cursor < question.notes.chars().count()
    {
        let start = question_notes_byte_index(&question.notes, question.notes_cursor);
        let end = question_notes_byte_index(&question.notes, question.notes_cursor + 1);
        question.notes.replace_range(start..end, "");
        tc.invalidate_render_cache();
        changed = true;
    }
    invalidate_if_changed(app, dirty_idx, changed);
}

fn move_question_notes_cursor(app: &mut App, direction: i32) {
    let dirty_idx = focused_interaction_dirty_idx(app);
    let mut changed = false;
    if let Some(tc) = get_focused_interaction_tc(app)
        && let Some(ref mut question) = tc.pending_question
    {
        let max = question.notes.chars().count();
        let next = if direction < 0 {
            question.notes_cursor.saturating_sub(1)
        } else {
            (question.notes_cursor + 1).min(max)
        };
        if next != question.notes_cursor {
            question.notes_cursor = next;
            tc.invalidate_render_cache();
            changed = true;
        }
    }
    invalidate_if_changed(app, dirty_idx, changed);
}

fn move_question_notes_cursor_to_start(app: &mut App) {
    let dirty_idx = focused_interaction_dirty_idx(app);
    let mut changed = false;
    if let Some(tc) = get_focused_interaction_tc(app)
        && let Some(ref mut question) = tc.pending_question
        && question.notes_cursor != 0
    {
        question.notes_cursor = 0;
        tc.invalidate_render_cache();
        changed = true;
    }
    invalidate_if_changed(app, dirty_idx, changed);
}

fn move_question_notes_cursor_to_end(app: &mut App) {
    let dirty_idx = focused_interaction_dirty_idx(app);
    let mut changed = false;
    if let Some(tc) = get_focused_interaction_tc(app)
        && let Some(ref mut question) = tc.pending_question
    {
        let next = question.notes.chars().count();
        if question.notes_cursor != next {
            question.notes_cursor = next;
            tc.invalidate_render_cache();
            changed = true;
        }
    }
    invalidate_if_changed(app, dirty_idx, changed);
}

fn set_question_notes_editing(app: &mut App, editing_notes: bool) {
    let dirty_idx = focused_interaction_dirty_idx(app);
    let mut changed = false;
    if let Some(tc) = get_focused_interaction_tc(app)
        && let Some(ref mut question) = tc.pending_question
        && question.editing_notes != editing_notes
    {
        question.editing_notes = editing_notes;
        tc.invalidate_render_cache();
        changed = true;
    }
    invalidate_if_changed(app, dirty_idx, changed);
}

fn toggle_question_selection(app: &mut App) {
    let dirty_idx = focused_interaction_dirty_idx(app);
    let mut changed = false;
    if let Some(tc) = get_focused_interaction_tc(app)
        && let Some(ref mut question) = tc.pending_question
    {
        let idx = question.focused_option_index;
        if question.prompt.multi_select {
            if !question.selected_option_indices.insert(idx) {
                question.selected_option_indices.remove(&idx);
            }
        } else {
            question.selected_option_indices.clear();
            question.selected_option_indices.insert(idx);
        }
        tc.invalidate_render_cache();
        changed = true;
    }
    invalidate_if_changed(app, dirty_idx, changed);
}

fn question_selected_indices(question: &crate::app::InlineQuestion) -> Vec<usize> {
    if question.prompt.multi_select {
        if question.selected_option_indices.is_empty() {
            return vec![question.focused_option_index];
        }
        return question.selected_option_indices.iter().copied().collect();
    }
    vec![question.focused_option_index]
}

fn question_annotation(
    question: &crate::app::InlineQuestion,
    selected_indices: &[usize],
) -> Option<model::QuestionAnnotation> {
    let preview = selected_indices
        .iter()
        .filter_map(|idx| question.prompt.options.get(*idx))
        .filter_map(|option| option.preview.as_deref())
        .map(str::trim)
        .filter(|preview| !preview.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    let notes = question.notes.trim();
    if preview.is_empty() && notes.is_empty() {
        return None;
    }

    Some(
        model::QuestionAnnotation::new()
            .preview((!preview.is_empty()).then_some(preview))
            .notes((!notes.is_empty()).then_some(notes.to_owned())),
    )
}

fn respond_question(app: &mut App) {
    respond_question_with_selection(app, false);
}

fn respond_question_with_selection(app: &mut App, auto_continue: bool) {
    let Some(tool_id) = pop_next_valid_interaction_id(app) else {
        return;
    };

    if let Some(pending) = app.mutate_tool_call(&tool_id, |tool| tool.pending_question.take()) {
        let selected_indices = if auto_continue {
            pending.selected_option_indices.iter().copied().collect()
        } else {
            question_selected_indices(&pending)
        };
        let selected_option_ids = selected_indices
            .iter()
            .filter_map(|idx| pending.prompt.options.get(*idx))
            .map(|option| option.option_id.clone())
            .collect::<Vec<_>>();
        let annotation = question_annotation(&pending, &selected_indices);

        if selected_option_ids.is_empty() && !auto_continue {
            tracing::warn!(
                target: crate::logging::targets::APP_PERMISSION,
                event_name = "question_response_rejected",
                message = "question response rejected because no valid option IDs were resolved",
                outcome = "failure",
                tool_call_id = %tool_id,
                selected_option_count = selected_indices.len(),
            );
            let _ = pending.response_tx.send(model::RequestQuestionResponse::new(
                model::RequestQuestionOutcome::Cancelled,
            ));
        } else {
            tracing::debug!(
                target: crate::logging::targets::APP_PERMISSION,
                event_name = "question_response_applied",
                message = "question response applied",
                outcome = "success",
                tool_call_id = %tool_id,
                selected_option_count = selected_option_ids.len(),
                has_annotation = annotation.is_some(),
            );
            let _ = pending.response_tx.send(model::RequestQuestionResponse::new(
                model::RequestQuestionOutcome::Answered(
                    model::AnsweredQuestionOutcome::new(selected_option_ids).annotation(annotation),
                ),
            ));
        }

        app.request_chat_mutable_rebuild();
    }

    focus_next_inline_interaction(app);
}

fn respond_question_cancel(app: &mut App) {
    let Some(tool_id) = pop_next_valid_interaction_id(app) else {
        return;
    };

    if let Some(pending) = app.mutate_tool_call(&tool_id, |tool| tool.pending_question.take()) {
        let _ = pending
            .response_tx
            .send(model::RequestQuestionResponse::new(model::RequestQuestionOutcome::Cancelled));
        app.request_chat_mutable_rebuild();
    }

    focus_next_inline_interaction(app);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, ChatMessage, InlineQuestion, MessageBlock, MessageRole, ToolCallInfo};
    use pretty_assertions::assert_eq;
    use std::collections::BTreeSet;
    use tokio::sync::oneshot;

    fn test_tool_call(id: &str) -> ToolCallInfo {
        ToolCallInfo {
            title: format!("Tool {id}"),
            ..crate::app::test_support::named_tool_call_info(
                id,
                "AskUserQuestion",
                model::ToolCallStatus::InProgress,
            )
        }
    }

    fn assistant_tool_msg(tc: ToolCallInfo) -> ChatMessage {
        ChatMessage::new(MessageRole::Assistant, vec![MessageBlock::ToolCall(Box::new(tc))], None)
    }

    fn add_question(
        app: &mut App,
        tool_id: &str,
        prompt: model::QuestionPrompt,
        focused: bool,
    ) -> oneshot::Receiver<model::RequestQuestionResponse> {
        let msg_idx = app.transcript.messages.len();
        app.transcript.messages.push(assistant_tool_msg(test_tool_call(tool_id)));
        app.index_tool_call(tool_id.to_owned(), msg_idx, 0);

        let (tx, rx) = oneshot::channel();
        if let Some(MessageBlock::ToolCall(tc)) =
            app.transcript.messages.get_mut(msg_idx).and_then(|m| m.blocks.get_mut(0))
        {
            tc.pending_question = Some(InlineQuestion {
                idle_timeout: None,
                last_activity: std::time::Instant::now(),
                prompt,
                response_tx: tx,
                focused_option_index: 0,
                selected_option_indices: BTreeSet::new(),
                notes: String::new(),
                notes_cursor: 0,
                editing_notes: false,
                focused,
                question_index: 0,
                total_questions: 1,
            });
        }
        app.pending_interaction_ids.push(tool_id.to_owned());
        rx
    }

    #[test]
    fn question_idle_workflow_submits_explicit_selection_and_skips_untouched_question() {
        use crate::app::{AppStatus, FocusTarget};
        use std::time::Duration;
        for selected in [false, true] {
            let mut app = App::test_default();
            app.status = AppStatus::Running;
            let mut response = add_question(
                &mut app,
                "idle-question",
                model::QuestionPrompt::new(
                    "Choose a target",
                    "Target",
                    false,
                    vec![
                        model::QuestionOption::new("a", "Staging"),
                        model::QuestionOption::new("b", "Production"),
                    ],
                ),
                true,
            );
            app.claim_focus_target(FocusTarget::Permission);
            let started = Instant::now();
            let (mi, bi) = app.lookup_tool_call("idle-question").expect("tool");
            let Some(MessageBlock::ToolCall(tool)) =
                app.transcript.messages.get_mut(mi).and_then(|message| message.blocks.get_mut(bi))
            else {
                panic!("tool");
            };
            let question = tool.pending_question.as_mut().expect("question");
            question.idle_timeout = Some(Duration::from_secs(60));
            question.last_activity = started;
            if selected {
                crate::app::events::handle_terminal_event(
                    &mut app,
                    crossterm::event::Event::Key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)),
                );
            }
            let activity = focused_question(&app).expect("pending").last_activity;
            tick_idle_timeout(&mut app, activity + Duration::from_secs(59));
            assert!(matches!(response.try_recv(), Err(oneshot::error::TryRecvError::Empty)));
            tick_idle_timeout(&mut app, activity + Duration::from_secs(60));
            let result = response.try_recv().expect("continued");
            let model::RequestQuestionOutcome::Answered(answer) = result.outcome else {
                panic!("answer");
            };
            assert_eq!(
                answer.selected_option_ids,
                if selected { vec!["b".to_owned()] } else { Vec::new() }
            );
            assert!(app.pending_interaction_ids.is_empty());
        }
    }

    fn admitted_question(
        app: &mut App,
        id: &str,
        multi: bool,
        timeout: Option<u64>,
        previews: bool,
    ) -> oneshot::Receiver<model::RequestQuestionResponse> {
        use crate::app::test_support::admit_tool;
        admit_tool(
            app,
            model::ToolCall::new(id, format!("Question {id}"))
                .status(model::ToolCallStatus::InProgress)
                .meta(serde_json::json!({"claudeCode":{"toolName":"AskUserQuestion"}})),
        );
        let options = [
            ("a", "Amber", "Preview A"),
            ("b", "Blue", "Preview B"),
            ("c", "Cyan", "Preview C"),
            ("d", "Dark", "  "),
        ]
        .into_iter()
        .map(|(suffix, label, preview)| {
            model::QuestionOption::new(format!("{id}-{suffix}"), label)
                .preview(previews.then(|| format!(" {preview} ")))
        })
        .collect();
        let request = model::RequestQuestionRequest::new(
            "session-1",
            model::ToolCallUpdate::new(id, model::ToolCallUpdateFields::new()),
            model::QuestionPrompt::new("Choose a deployment", "Deployment", multi, options),
            0,
            1,
        )
        .idle_timeout_ms(timeout);
        let (response_tx, receiver) = oneshot::channel();
        crate::app::handle_client_event(
            app,
            crate::agent::events::ClientEvent::QuestionRequest {
                session_id: "session-1".into(),
                request,
                response_tx,
            },
        );
        receiver
    }

    fn question_key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
        crate::app::handle_terminal_event(
            app,
            crossterm::event::Event::Key(KeyEvent::new(code, modifiers)),
        );
        crate::app::finalize_deferred_submit(app);
    }

    fn answer(
        receiver: &mut oneshot::Receiver<model::RequestQuestionResponse>,
        ids: &[&str],
        preview: Option<&str>,
        notes: Option<&str>,
    ) {
        let annotation = if preview.is_none() && notes.is_none() {
            None
        } else {
            Some(
                model::QuestionAnnotation::new()
                    .preview(preview.map(str::to_owned))
                    .notes(notes.map(str::to_owned)),
            )
        };
        assert_eq!(
            receiver.try_recv().expect("answer delivered").outcome,
            model::RequestQuestionOutcome::Answered(
                model::AnsweredQuestionOutcome::new(
                    ids.iter().map(|id| (*id).to_owned()).collect()
                )
                .annotation(annotation)
            )
        );
    }

    #[test]
    fn physical_question_keys_deliver_only_selected_ids_previews_and_notes() {
        use crate::app::test_support::{app_with_connection, assert_pending, press};
        for (multi, navigation, expected, preview) in [
            (false, vec![KeyCode::Right], vec!["target-b"], Some("Preview B")),
            (true, vec![], vec!["target-a"], Some("Preview A")),
            (
                true,
                vec![
                    KeyCode::Right,
                    KeyCode::Home,
                    KeyCode::Char(' '),
                    KeyCode::Right,
                    KeyCode::Char(' '),
                    KeyCode::Char(' '),
                    KeyCode::Right,
                    KeyCode::Char(' '),
                    KeyCode::Left,
                ],
                vec!["target-a", "target-c"],
                Some("Preview A\n\nPreview C"),
            ),
        ] {
            let (mut app, mut commands) = app_with_connection();
            app.status = crate::app::AppStatus::Running;
            let mut receiver = admitted_question(&mut app, "target", multi, None, true);
            let mut peer = admitted_question(&mut app, "peer", false, None, false);
            app.input.set_text("unrelated draft");
            for key in navigation {
                press(&mut app, key);
            }
            question_key(&mut app, KeyCode::BackTab, KeyModifiers::SHIFT);
            for ch in " note ".chars() {
                press(&mut app, KeyCode::Char(ch));
            }
            press(&mut app, KeyCode::Tab);
            press(&mut app, KeyCode::Tab);
            press(&mut app, KeyCode::Enter);
            answer(&mut receiver, &expected, preview, Some("note"));
            assert_pending(&mut peer);
            press(&mut app, KeyCode::Enter);
            answer(&mut peer, &["peer-a"], None, None);
            assert_eq!(app.input.text(), "unrelated draft");
            assert!(commands.try_recv().is_err());
        }
        let (mut app, _commands) = app_with_connection();
        app.status = crate::app::AppStatus::Running;
        let mut receiver = admitted_question(&mut app, "blank", false, None, true);
        press(&mut app, KeyCode::End);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char(' '));
        press(&mut app, KeyCode::Enter);
        answer(&mut receiver, &["blank-d"], None, None);
    }

    #[test]
    fn unicode_note_edits_follow_scalar_boundaries_and_reach_only_the_addressed_question() {
        use crate::app::test_support::{app_with_connection, assert_pending, press};
        let (mut app, mut commands) = app_with_connection();
        app.status = crate::app::AppStatus::Running;
        let mut receiver = admitted_question(&mut app, "edit", false, None, true);
        let mut peer = admitted_question(&mut app, "neighbor", false, None, false);
        app.input.set_text("draft unchanged");
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Tab);
        for ch in "A\u{e9}\u{4e2d}\u{1f680}Z".chars() {
            press(&mut app, KeyCode::Char(ch));
        }
        for key in [
            KeyCode::Left,
            KeyCode::Left,
            KeyCode::Backspace,
            KeyCode::Delete,
            KeyCode::Char('\u{754c}'),
            KeyCode::Left,
            KeyCode::Right,
            KeyCode::Home,
            KeyCode::Backspace,
        ] {
            press(&mut app, key);
        }
        question_key(&mut app, KeyCode::Char('\u{3a9}'), KeyModifiers::CONTROL | KeyModifiers::ALT);
        for key in [
            KeyCode::End,
            KeyCode::Delete,
            KeyCode::Left,
            KeyCode::Backspace,
            KeyCode::Char('\u{1f680}'),
            KeyCode::End,
            KeyCode::Right,
            KeyCode::Delete,
            KeyCode::Home,
            KeyCode::Left,
            KeyCode::Delete,
            KeyCode::Char('\u{4e2d}'),
            KeyCode::Enter,
        ] {
            press(&mut app, key);
        }
        answer(&mut receiver, &["edit-b"], Some("Preview B"), Some("\u{4e2d}A\u{e9}\u{1f680}Z"));
        assert_pending(&mut peer);
        assert_eq!(app.input.text(), "draft unchanged");
        press(&mut app, KeyCode::Enter);
        answer(&mut peer, &["neighbor-a"], None, None);
        assert!(commands.try_recv().is_err());
    }

    #[test]
    fn typing_notes_renews_the_idle_deadline_but_timer_ticks_do_not() {
        use crate::app::test_support::{QuestionClock, app_with_connection, assert_pending, press};
        let clock = QuestionClock::new();
        let (mut app, mut commands) = app_with_connection();
        app.status = crate::app::AppStatus::Running;
        let mut receiver = admitted_question(&mut app, "renew", false, Some(60_000), true);
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Tab);
        tick_idle_timeout(&mut app, clock.at(30));
        assert_pending(&mut receiver);
        clock.at(55);
        press(&mut app, KeyCode::Char('n'));
        for seconds in [60, 114] {
            tick_idle_timeout(&mut app, clock.at(seconds));
            assert_pending(&mut receiver);
        }
        tick_idle_timeout(&mut app, clock.at(115));
        answer(&mut receiver, &["renew-b"], Some("Preview B"), Some("n"));
        tick_idle_timeout(&mut app, clock.at(116));
        assert!(matches!(receiver.try_recv(), Err(oneshot::error::TryRecvError::Closed)));
        assert!(commands.try_recv().is_err());
    }

    #[tokio::test]
    async fn fullscreen_and_composer_focus_pause_idle_answers_until_a_full_resumed_interval() {
        use crate::app::test_support::{QuestionClock, app_with_connection, assert_pending, press};
        for fullscreen in [false, true] {
            let clock = QuestionClock::new();
            let (mut app, mut commands) = app_with_connection();
            app.status = crate::app::AppStatus::Running;
            let mut receiver = admitted_question(&mut app, "pause", false, Some(60_000), true);
            press(&mut app, KeyCode::Right);
            press(&mut app, KeyCode::Tab);
            press(&mut app, KeyCode::Char('n'));
            press(&mut app, KeyCode::Tab);
            clock.at(50);
            if fullscreen {
                crate::app::config::open_tab(&mut app, crate::app::ConfigTab::Settings)
                    .expect("open settings");
                assert_eq!(
                    app.surface_mode,
                    crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config)
                );
            } else {
                press(&mut app, KeyCode::Char('d'));
                assert_eq!(app.focus_owner(), crate::app::FocusOwner::Input);
            }
            for seconds in [90, 150] {
                tick_idle_timeout(&mut app, clock.at(seconds));
                assert_pending(&mut receiver);
            }
            clock.at(160);
            if fullscreen {
                press(&mut app, KeyCode::Esc);
            }
            assert_eq!(app.surface_mode, crate::app::SurfaceMode::Chat);
            if app.focus_owner() != crate::app::FocusOwner::Permission {
                press(&mut app, KeyCode::Tab);
            }
            assert_eq!(app.focus_owner(), crate::app::FocusOwner::Permission);
            tick_idle_timeout(&mut app, clock.at(219));
            assert_pending(&mut receiver);
            tick_idle_timeout(&mut app, clock.at(220));
            answer(&mut receiver, &["pause-b"], Some("Preview B"), Some("n"));
            assert_eq!(app.input.text(), if fullscreen { "" } else { "d" });
            while let Ok(envelope) = commands.try_recv() {
                assert!(!matches!(
                    envelope.command,
                    crate::agent::wire::BridgeCommand::Prompt { .. }
                        | crate::agent::wire::BridgeCommand::CancelTurn { .. }
                ));
            }
        }
    }
}
