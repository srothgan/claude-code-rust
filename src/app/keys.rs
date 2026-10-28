// SPDX-License-Identifier: Apache-2.0
use super::clipboard_image::{ClipboardRead, encode_clipboard_image};
use super::input::InputYankPart;
use super::paste_burst::CharAction;
use super::{App, AppStatus, FocusOwner};
use crate::app::SystemSeverity;
use crate::app::inline_interactions::{
    clear_inline_interaction_focus, focus_next_inline_interaction,
    normalize_pending_interaction_queue,
};
use crate::app::keymap::{
    AppAction, AutocompleteAction, InputAction, InteractionAction, KeyAction, KeyContext,
    TerminalAction,
};
use crate::app::state::AutocompleteKind;
use crate::app::{input_atoms, input_atoms::InputAtomKind};
use crate::app::{mention, permissions, questions, slash, subagent};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RuntimeCommand {
    SuspendProcess,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum KeyOutcome {
    Ignored,
    Handled(bool),
    Runtime(RuntimeCommand),
}

impl KeyOutcome {
    pub(crate) fn changed(self) -> bool {
        match self {
            Self::Ignored => false,
            Self::Handled(changed) => changed,
            Self::Runtime(_) => true,
        }
    }

    fn handled(self) -> bool {
        !matches!(self, Self::Ignored)
    }

    pub(crate) fn runtime_command(self) -> Option<RuntimeCommand> {
        match self {
            Self::Runtime(command) => Some(command),
            Self::Ignored | Self::Handled(_) => None,
        }
    }
}

impl From<bool> for KeyOutcome {
    fn from(changed: bool) -> Self {
        Self::Handled(changed)
    }
}

#[cfg(all(test, target_os = "macos"))]
pub(crate) const CMD_MOD: KeyModifiers = KeyModifiers::SUPER;

#[cfg(all(test, target_os = "macos"))]
pub(crate) const WORD_NAV_MOD: KeyModifiers = KeyModifiers::ALT;
#[cfg(all(test, not(target_os = "macos")))]
pub(crate) const WORD_NAV_MOD: KeyModifiers = KeyModifiers::CONTROL;

fn is_ctrl_shortcut(modifiers: KeyModifiers) -> bool {
    modifiers.contains(KeyModifiers::CONTROL) && !modifiers.contains(KeyModifiers::ALT)
}

fn ctrl_char(expected: char) -> Option<char> {
    let upper = expected.to_ascii_uppercase();
    if !upper.is_ascii_alphabetic() {
        return None;
    }
    Some(char::from((upper as u8) & 0x1f))
}

pub(super) fn is_ctrl_char_shortcut(key: KeyEvent, expected: char) -> bool {
    match key.code {
        KeyCode::Char(c) if c.eq_ignore_ascii_case(&expected) => is_ctrl_shortcut(key.modifiers),
        KeyCode::Char(c) if Some(c) == ctrl_char(expected) => {
            !key.modifiers.contains(KeyModifiers::ALT)
        }
        _ => false,
    }
}

pub(super) fn dispatch_key_by_focus(app: &mut App, key: KeyEvent) -> KeyOutcome {
    if !app.composer_access().can_edit() {
        return handle_keymap_context(app, KeyContext::ChatBlocked, key);
    }
    // Every composer route, including autocomplete and custom bindings, must
    // cancel stale submissions and respect a paste awaiting drain finalization.
    if should_ignore_key_during_paste(app, key) {
        return KeyOutcome::Ignored;
    }
    let input_version_before = app.input.version;
    let outcome = route_key_by_focus(app, key);
    // A deferred submit returns unchanged: prepare_submit already chose the
    // final menu state, and the drain will consume its completed draft.
    if outcome.changed() && app.input.version != input_version_before {
        slash::sync_with_cursor(app);
    }
    outcome
}

fn route_key_by_focus(app: &mut App, key: KeyEvent) -> KeyOutcome {
    match app.focus_owner() {
        FocusOwner::Mention => handle_autocomplete_key(app, key),
        FocusOwner::Permission => {
            normalize_pending_interaction_queue(app);
            if app.focus_owner() != FocusOwner::Permission {
                return handle_normal_key(app, key);
            }
            if should_reclaim_input_focus_before_inline_interaction(app, key) {
                reclaim_input_from_inline_prompt_if_needed(app);
                handle_normal_key(app, key)
            } else {
                let context = active_inline_interaction_context(app);
                if context == KeyContext::InlineQuestion
                    && let Some(outcome) = questions::handle_question_note_key(app, key)
                {
                    return outcome;
                }
                let outcome = handle_keymap_context(app, context, key);
                if outcome.handled() { outcome } else { handle_normal_key(app, key) }
            }
        }
        FocusOwner::Input => handle_normal_key(app, key),
    }
}

fn first_handled(primary: KeyOutcome, fallback: impl FnOnce() -> KeyOutcome) -> KeyOutcome {
    if primary.handled() { primary } else { fallback() }
}

fn printable_outcome(changed: bool) -> KeyOutcome {
    if changed { KeyOutcome::Handled(true) } else { KeyOutcome::Ignored }
}

fn active_inline_interaction_context(app: &App) -> KeyContext {
    if questions::has_focused_question(app) {
        KeyContext::InlineQuestion
    } else {
        KeyContext::InlinePermission
    }
}

fn handle_keymap_context(app: &mut App, context: KeyContext, key: KeyEvent) -> KeyOutcome {
    match resolve_key_action_for_context(app, context, key) {
        Some(action) => execute_key_action(app, action, key),
        None => KeyOutcome::Ignored,
    }
}

fn resolve_key_action_for_context(
    app: &App,
    context: KeyContext,
    key: KeyEvent,
) -> Option<KeyAction> {
    app.keymap.resolve_event(context, key).map(|resolved| resolved.action)
}

#[inline]
pub(super) fn is_printable_text_modifiers(modifiers: KeyModifiers) -> bool {
    let ctrl_alt =
        modifiers.contains(KeyModifiers::CONTROL) && modifiers.contains(KeyModifiers::ALT);
    !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) || ctrl_alt
}

pub(super) fn handle_normal_key(app: &mut App, key: KeyEvent) -> KeyOutcome {
    let input_version_before = app.input.version;

    let outcome = handle_chat_input_key(app, key);

    if app.input.version != input_version_before {
        mention::sync_with_cursor(app);
        subagent::sync_with_cursor(app);
    }

    outcome
}

fn should_ignore_key_during_paste(app: &mut App, key: KeyEvent) -> bool {
    let action = autocomplete_key_context(app)
        .filter(|_| app.focus_owner() == FocusOwner::Mention)
        .and_then(|context| resolve_key_action_for_context(app, context, key))
        .or_else(|| resolve_key_action_for_context(app, KeyContext::ChatInput, key));
    if matches!(
        action,
        Some(KeyAction::App(
            AppAction::ClearInputOrQuit
                | AppAction::Quit
                | AppAction::Redraw
                | AppAction::CancelTurn
        ))
    ) {
        return false;
    }
    let edits_input = is_editing_like_key(key, action);
    if edits_input {
        app.pending_submit = None;
    }
    app.paste.has_pending_text() && edits_input
}

fn is_editing_like_key(key: KeyEvent, action: Option<KeyAction>) -> bool {
    matches!(
        key.code,
        KeyCode::Char(_) | KeyCode::Enter | KeyCode::Tab | KeyCode::Backspace | KeyCode::Delete
    ) || matches!(
        action,
        Some(
            KeyAction::App(AppAction::SubmitInput | AppAction::FocusPromptOrAcceptSuggestion)
                | KeyAction::Autocomplete(AutocompleteAction::Confirm)
                | KeyAction::Input(
                    InputAction::DeleteCharBefore
                        | InputAction::DeleteCharAfter
                        | InputAction::DeleteWordBefore
                        | InputAction::DeleteWordAfter
                        | InputAction::KillLineStart
                        | InputAction::KillLineEnd
                        | InputAction::Yank
                        | InputAction::Undo
                        | InputAction::Redo
                        | InputAction::InsertNewline
                )
        )
    )
}

fn should_reclaim_input_focus_before_inline_interaction(app: &App, key: KeyEvent) -> bool {
    let question_notes_editing = questions::focused_question_is_editing_notes(app);
    match key.code {
        KeyCode::Backspace | KeyCode::Delete => !question_notes_editing,
        KeyCode::Char(' ')
            if questions::has_focused_question(app)
                && is_printable_text_modifiers(key.modifiers)
                && !question_notes_editing =>
        {
            false
        }
        KeyCode::Char(_) if is_printable_text_modifiers(key.modifiers) => !question_notes_editing,
        _ => false,
    }
}

fn handle_chat_input_key(app: &mut App, key: KeyEvent) -> KeyOutcome {
    if handle_clipboard_paste_key(app, key) {
        return KeyOutcome::Handled(true);
    }
    if let Some(action) = resolve_key_action_for_context(app, KeyContext::ChatInput, key) {
        return first_handled(execute_key_action(app, action, key), || {
            printable_outcome(handle_printable_key(app, key))
        });
    }
    printable_outcome(handle_printable_key(app, key))
}

fn execute_key_action(app: &mut App, action: KeyAction, key: KeyEvent) -> KeyOutcome {
    if action == KeyAction::Input(InputAction::MoveCharLeft)
        && key.code == KeyCode::Left
        && open_agents_from_empty_composer(app)
    {
        return KeyOutcome::Handled(true);
    }
    match action {
        KeyAction::App(action) => execute_app_action(app, action),
        KeyAction::Input(action) => execute_input_action(app, action),
        KeyAction::Autocomplete(action) => execute_autocomplete_action(app, action).into(),
        KeyAction::Interaction(action) => execute_interaction_action(app, action, key),
        KeyAction::Terminal(action) => execute_terminal_action(action),
    }
}

fn execute_app_action(app: &mut App, action: AppAction) -> KeyOutcome {
    match action {
        AppAction::Quit => {
            app.request_shutdown();
            KeyOutcome::Handled(true)
        }
        AppAction::ClearInputOrQuit => clear_input_or_quit(app).into(),
        AppAction::Redraw => {
            app.request_chat_visible_rebuild();
            KeyOutcome::Handled(true)
        }
        AppAction::CancelTurn => handle_turn_control(app).into(),
        AppAction::SubmitInput => handle_submit(app).into(),
        AppAction::FocusPromptOrAcceptSuggestion => (mention::commit_literal_if_active(app)
            || slash::request_completion(app)
            || handle_focus_toggle(app)
            || handle_prompt_suggestion(app))
        .into(),
        AppAction::ScrollChatUp | AppAction::ScrollChatDown => {
            let rows =
                isize::try_from(app.chat_render.live_region.last_rendered_rows.max(3)).unwrap_or(3);
            app.chat_render.viewport.scroll(if action == AppAction::ScrollChatUp {
                -rows
            } else {
                rows
            });
            app.request_chat_repaint();
            true.into()
        }
        AppAction::FollowChat => {
            app.chat_render.viewport.resume();
            app.request_chat_repaint();
            true.into()
        }
        AppAction::NextMode => step_mode(app, Step::Forward).into(),
        AppAction::PreviousMode => step_mode(app, Step::Backward).into(),
        AppAction::IncreaseEffort => step_effort(app, Step::Forward).into(),
        AppAction::DecreaseEffort => step_effort(app, Step::Backward).into(),
    }
}

fn has_local_draft(app: &App) -> bool {
    !app.input.is_empty()
        || !app.input.images().is_empty()
        || app.paste.has_pending_text()
        || app.paste.burst.has_held_input()
        || app.pending_submit.is_some()
}

/// Left Arrow opens `/agents` only from an empty composer that owns focus;
/// otherwise it keeps moving the cursor. Every autocomplete menu needs draft
/// text, and an inline prompt that leaves Left unbound must keep its focus.
fn open_agents_from_empty_composer(app: &mut App) -> bool {
    if has_local_draft(app) || app.focus_owner() != FocusOwner::Input {
        return false;
    }
    crate::app::agents::open(app);
    true
}

fn clear_input_or_quit(app: &mut App) -> bool {
    if !has_local_draft(app) {
        app.request_shutdown();
        return true;
    }

    app.input.clear();
    app.paste.discard_input();
    app.pending_submit = None;
    app.request_chat_repaint();
    true
}

fn execute_input_action(app: &mut App, action: InputAction) -> KeyOutcome {
    reclaim_input_from_inline_prompt_if_needed(app);
    let flushed = !matches!(action, InputAction::InsertNewline) && super::flush_held_input(app);
    let handled = match action {
        InputAction::MoveCharLeft => app.input.textarea_move_left(),
        InputAction::MoveCharRight => app.input.textarea_move_right(),
        InputAction::MoveWordLeft => app.input.textarea_move_word_left(),
        InputAction::MoveWordRight => app.input.textarea_move_word_right(),
        InputAction::MoveLineStart => app.input.textarea_move_home(),
        InputAction::MoveLineEnd => app.input.textarea_move_end(),
        InputAction::MoveLineStartOrUp => app.input.textarea_move_line_start_or_up(),
        InputAction::MoveLineEndOrDown => app.input.textarea_move_line_end_or_down(),
        InputAction::MoveUp => {
            let _ = try_move_input_cursor_up(app);
            true
        }
        InputAction::MoveDown => {
            let _ = try_move_input_cursor_down(app);
            true
        }
        InputAction::DeleteCharBefore => delete_input_char_before(app),
        InputAction::DeleteCharAfter => delete_input_char_after(app),
        InputAction::DeleteWordBefore => delete_input_word_before(app),
        InputAction::DeleteWordAfter => delete_input_word_after(app),
        InputAction::KillLineStart => {
            delete_input_content(app, super::InputState::textarea_delete_line_before)
        }
        InputAction::KillLineEnd => {
            delete_input_content(app, super::InputState::textarea_delete_line_after)
        }
        InputAction::Yank => yank_input_content(app),
        InputAction::Undo => {
            let _ = app.input.textarea_undo();
            true
        }
        InputAction::Redo => {
            let _ = app.input.textarea_redo();
            true
        }
        InputAction::InsertNewline => insert_explicit_newline(app),
    };
    (handled || flushed).into()
}

fn delete_input_char_before(app: &mut App) -> bool {
    delete_input_content(app, super::InputState::textarea_delete_char_before)
}

fn delete_input_char_after(app: &mut App) -> bool {
    delete_input_content(app, super::InputState::textarea_delete_char_after)
}

fn delete_input_word_before(app: &mut App) -> bool {
    delete_input_content(app, super::InputState::textarea_delete_word_before)
}

fn delete_input_word_after(app: &mut App) -> bool {
    delete_input_content(app, super::InputState::textarea_delete_word_after)
}

fn capture_input_yank(app: &App, text: &str) -> Vec<InputYankPart> {
    let mut parts = Vec::new();
    for (row, line) in text.split('\n').enumerate() {
        if row > 0 {
            parts.push(InputYankPart::Text("\n".to_owned()));
        }
        let mut offset = 0;
        for atom in input_atoms::resolve_line_atoms(row, line) {
            parts.push(InputYankPart::Text(line[offset..atom.start_byte].to_owned()));
            let part = match atom.kind {
                InputAtomKind::ImageBadge { one_based_index } => app
                    .input
                    .image_payloads
                    .get(one_based_index - 1)
                    .cloned()
                    .map(InputYankPart::Image),
                InputAtomKind::PasteBlock { index, .. } => {
                    app.input.paste_blocks.get(index).cloned().map(InputYankPart::Paste)
                }
            };
            parts.push(part.unwrap_or_else(|| {
                InputYankPart::Text(line[atom.start_byte..atom.end_byte].to_owned())
            }));
            offset = atom.end_byte;
        }
        parts.push(InputYankPart::Text(line[offset..].to_owned()));
    }
    parts
}

fn delete_input_content(app: &mut App, edit: impl FnOnce(&mut super::InputState) -> bool) -> bool {
    let before = input_atoms::resolve_input_atoms(app.input.lines());
    if !edit(&mut app.input) {
        return false;
    }
    let killed = app.input.take_yank_text();
    if !killed.is_empty() {
        app.input_yank = capture_input_yank(app, &killed);
    }
    let after = input_atoms::resolve_input_atoms(app.input.lines());
    let mut removed_image = false;
    for atom in before {
        if after.iter().any(|remaining| remaining.kind == atom.kind) {
            continue;
        }
        match atom.kind {
            InputAtomKind::ImageBadge { .. } => removed_image = true,
            InputAtomKind::PasteBlock { index, .. } => {
                app.paste.clear_sessions_for_placeholder(index);
            }
        }
    }
    if removed_image {
        // Resolve old badge identities before renumbering, including cursor-positioned pastes.
        app.input.image_payloads = after
            .iter()
            .filter_map(|atom| match atom.kind {
                InputAtomKind::ImageBadge { one_based_index } => {
                    app.input.image_payloads.get(one_based_index - 1).cloned()
                }
                InputAtomKind::PasteBlock { .. } => None,
            })
            .collect();
        app.input.renumber_image_badges();
    }
    app.request_chat_repaint();
    true
}

fn yank_input_content(app: &mut App) -> bool {
    if app.input_yank.is_empty() {
        return false;
    }
    app.input.insert_parts(app.input_yank.clone());
    true
}

fn insert_explicit_newline(app: &mut App) -> bool {
    if app.paste.burst.on_enter(super::input_event_now()) {
        tracing::debug!(
            target: crate::logging::targets::APP_INPUT,
            event_name = "enter_routed_to_paste_buffer",
            message = "enter was routed through the paste buffer",
            outcome = "success",
        );
        return true;
    }
    app.pending_submit = None;
    tracing::debug!(
        target: crate::logging::targets::APP_INPUT,
        event_name = "explicit_newline_inserted",
        message = "explicit newline inserted instead of submit",
        outcome = "success",
    );
    app.input.textarea_insert_newline()
}

fn execute_autocomplete_action(app: &mut App, action: AutocompleteAction) -> bool {
    match app.active_autocomplete_kind() {
        Some(AutocompleteKind::Mention) => execute_mention_action(app, action),
        Some(AutocompleteKind::Slash) => execute_slash_action(app, action),
        Some(AutocompleteKind::Subagent) => execute_subagent_action(app, action),
        None => false,
    }
}

fn execute_mention_action(app: &mut App, action: AutocompleteAction) -> bool {
    match action {
        AutocompleteAction::MovePrevious => mention::move_up(app),
        AutocompleteAction::MoveNext => mention::move_down(app),
        AutocompleteAction::Confirm => mention::confirm_selection(app),
        AutocompleteAction::Cancel => mention::deactivate(app),
    }
    true
}

fn execute_slash_action(app: &mut App, action: AutocompleteAction) -> bool {
    match action {
        AutocompleteAction::MovePrevious => {
            if app.slash.visible().is_some_and(|slash| !slash.candidates.is_empty()) {
                slash::move_up(app);
            }
        }
        AutocompleteAction::MoveNext => {
            if app.slash.visible().is_some_and(|slash| !slash.candidates.is_empty()) {
                slash::move_down(app);
            }
        }
        AutocompleteAction::Confirm => {
            if app.slash.visible().is_some_and(|slash| !slash.candidates.is_empty()) {
                slash::confirm_selection(app);
            }
        }
        AutocompleteAction::Cancel => slash::dismiss(app),
    }
    true
}

fn execute_subagent_action(app: &mut App, action: AutocompleteAction) -> bool {
    match action {
        AutocompleteAction::MovePrevious => {
            if app.subagent.as_ref().is_some_and(|subagent| !subagent.query.is_empty()) {
                subagent::move_up(app);
            }
        }
        AutocompleteAction::MoveNext => {
            if app.subagent.as_ref().is_some_and(|subagent| !subagent.query.is_empty()) {
                subagent::move_down(app);
            }
        }
        AutocompleteAction::Confirm => {
            if app.subagent.as_ref().is_some_and(|subagent| !subagent.query.is_empty()) {
                subagent::confirm_selection(app);
            }
        }
        AutocompleteAction::Cancel => subagent::deactivate(app),
    }
    true
}

fn execute_interaction_action(
    app: &mut App,
    action: InteractionAction,
    key: KeyEvent,
) -> KeyOutcome {
    if super::inline_interactions::has_focused_user_dialog(app) {
        super::user_dialog::execute_user_dialog_action(app, action, key)
    } else if questions::has_focused_question(app) {
        questions::execute_question_action(app, action, key)
    } else {
        permissions::execute_permission_action(app, action, key)
    }
}

fn execute_terminal_action(action: TerminalAction) -> KeyOutcome {
    match action {
        TerminalAction::Suspend => KeyOutcome::Runtime(RuntimeCommand::SuspendProcess),
    }
}

fn handle_turn_control(app: &mut App) -> bool {
    app.pending_submit = None;
    // Clear any pending image attachments on Escape.
    if !app.input.image_payloads.is_empty() {
        app.input.image_payloads.clear();
        app.request_chat_repaint();
    }
    if matches!(app.status, AppStatus::Thinking | AppStatus::Running)
        && let Err(message) = super::input_submit::request_cancel(app)
    {
        tracing::error!(
            target: crate::logging::targets::APP_INPUT,
            event_name = "cancel_request_failed",
            message = "failed to send manual cancel request",
            outcome = "failure",
            error_message = %message,
        );
    }
    true
}

fn handle_submit(app: &mut App) -> bool {
    if !app.composer_access().can_submit() {
        app.pending_submit = None;
        return true;
    }
    let now = super::input_event_now();

    // During an active burst or the post-burst suppression window, Enter
    // becomes a newline to keep multi-line pastes grouped.
    if app.paste.burst.on_enter(now) {
        tracing::debug!(
            target: crate::logging::targets::APP_INPUT,
            event_name = "enter_routed_to_paste_buffer",
            message = "enter was routed through the paste buffer",
            outcome = "success",
        );
        return true;
    }

    if !slash::prepare_submit(app) {
        return true;
    }

    app.pending_submit = Some(app.input.snapshot());
    tracing::debug!(
        target: crate::logging::targets::APP_INPUT,
        event_name = "deferred_submit_armed",
        message = "deferred submit snapshot armed",
        outcome = "start",
    );
    false
}

fn handle_focus_toggle(app: &mut App) -> bool {
    if app.pending_interaction_ids.is_empty() {
        false
    } else {
        match app.focus_owner() {
            FocusOwner::Permission => {
                clear_inline_interaction_focus(app);
                true
            }
            FocusOwner::Input => {
                focus_next_inline_interaction(app);
                true
            }
            FocusOwner::Mention => false,
        }
    }
}

fn handle_prompt_suggestion(app: &mut App) -> bool {
    if app.focus_owner() != FocusOwner::Input || !app.input.is_empty() {
        return false;
    }

    let Some(suggestion) = app.session_runtime.prompt_suggestion.take() else {
        return false;
    };
    if suggestion.trim().is_empty() {
        return false;
    }
    app.input.set_text(&suggestion);
    true
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Step {
    Forward,
    Backward,
}

/// Mode and effort shortcuts change the active session, so they act only
/// from the focused composer while no turn or command is in flight.
fn session_controls_ready(app: &App) -> bool {
    app.status == AppStatus::Ready && app.focus_owner() == FocusOwner::Input
}

fn is_session_control_action(action: Option<KeyAction>) -> bool {
    matches!(
        action,
        Some(KeyAction::App(
            AppAction::NextMode
                | AppAction::PreviousMode
                | AppAction::IncreaseEffort
                | AppAction::DecreaseEffort
        ))
    )
}

/// Moves through the available modes in their advertised order, wrapping at both ends.
fn step_mode(app: &mut App, step: Step) -> bool {
    if !session_controls_ready(app) {
        return false;
    }
    let Some(mode) = &app.session_runtime.mode else {
        return false;
    };
    let modes = &mode.available_modes;
    let count = modes.len();
    if count <= 1 {
        return false;
    }
    let current = modes.iter().position(|mode_info| mode_info.id == mode.current_mode_id);
    let target_index = match (current, step) {
        (Some(index), Step::Forward) => (index + 1) % count,
        (Some(index), Step::Backward) => (index + count - 1) % count,
        (None, Step::Forward) => 0,
        (None, Step::Backward) => count - 1,
    };
    let target = modes[target_index].id.clone();
    slash::request_mode_change(app, &target)
}

/// Moves to the adjacent effort level the current model supports, stopping at
/// the lowest and highest level. An unknown current effort requests nothing.
fn step_effort(app: &mut App, step: Step) -> bool {
    if !session_controls_ready(app) {
        return false;
    }
    let Some(current) = app.session_effort() else {
        return false;
    };
    let Some(model) = app.session_runtime.current_model.as_ref() else {
        return false;
    };
    let levels = model.selectable_effort_levels().iter().copied();
    let target = match step {
        Step::Forward => levels.filter(|level| *level > current).min(),
        Step::Backward => levels.filter(|level| *level < current).max(),
    };
    target.is_some_and(|target| slash::request_effort_change(app, Some(target)))
}

fn handle_clipboard_paste_key(app: &mut App, key: KeyEvent) -> bool {
    if !is_clipboard_paste_trigger(app, key) {
        return false;
    }

    let image = match (app.clipboard_paste.read)() {
        ClipboardRead::Image(image) => image,
        ClipboardRead::NoImage => return false,
        ClipboardRead::Unavailable => {
            warn_clipboard_paste(app, "Failed to access the system clipboard.");
            return true;
        }
    };
    match encode_clipboard_image(image) {
        Ok(attachment) => {
            super::flush_held_input(app);
            app.input.insert_parts([InputYankPart::Image(attachment)]);
            app.request_chat_repaint();
            tracing::debug!(
                count = app.input.image_payloads.len(),
                "clipboard_paste: attached image from clipboard"
            );
        }
        Err(error) => {
            warn_clipboard_paste(app, error.user_message());
            tracing::warn!("clipboard_paste: image attachment failed: {error:?}");
        }
    }
    true
}

fn warn_clipboard_paste(app: &mut App, message: &str) {
    super::events::push_system_message_with_severity(app, Some(SystemSeverity::Warning), message);
    app.request_chat_repaint();
}

/// The Ctrl+V event that reads an image from the clipboard. The terminal event
/// filter uses the same rule to let that one release through on Windows.
pub(super) fn is_clipboard_paste_trigger(app: &App, key: KeyEvent) -> bool {
    key.kind == app.clipboard_paste.trigger && is_ctrl_char_shortcut(key, 'v')
}

pub(super) fn reclaim_input_from_inline_prompt_if_needed(app: &mut App) {
    if app.focus_owner() == FocusOwner::Permission {
        clear_inline_interaction_focus(app);
    }
}

fn handle_printable_key(app: &mut App, key: KeyEvent) -> bool {
    let (KeyCode::Char(c), m) = (key.code, key.modifiers) else {
        // A non-character key ends the burst but must not consume its text.
        return super::flush_held_input(app);
    };
    if c.is_control() || !is_printable_text_modifiers(m) {
        return false;
    }
    reclaim_input_from_inline_prompt_if_needed(app);

    let now = super::input_event_now();
    match app.paste.burst.on_char(c, now) {
        CharAction::Consumed => {
            // Character absorbed into burst buffer. Don't insert.
            return false;
        }
        CharAction::RetroCapture(delete_count) => {
            // Burst confirmation retro-captured already-inserted leading chars.
            for _ in 0..delete_count {
                let _ = delete_input_char_before(app);
            }
            tracing::debug!(
                target: crate::logging::targets::APP_PASTE,
                event_name = "paste_retro_capture_applied",
                message = "retro-captured leaked characters from a confirmed paste burst",
                outcome = "success",
                delete_count,
            );
            return true;
        }
        CharAction::Passthrough(ch) => {
            let _ = app.input.textarea_insert_char(ch);
        }
        CharAction::ReleaseHeld(held) => {
            let _ = app.input.textarea_insert_char(held);
            let _ = app.input.textarea_insert_char(c);
        }
    }

    true
}

fn try_move_input_cursor_up(app: &mut App) -> bool {
    if app.input.is_empty()
        && let Some(text) = app.transcript.latest_user_text().map(str::to_owned)
    {
        app.input.set_text(&remove_recalled_image_badges(&text));
        return true;
    }

    let before = (app.input.cursor_row(), app.input.cursor_col());
    let _ = app.input.textarea_move_up();
    (app.input.cursor_row(), app.input.cursor_col()) != before
}

fn remove_recalled_image_badges(text: &str) -> String {
    text.split('\n')
        .enumerate()
        .map(|(row, line)| {
            let mut recalled_line = line.to_owned();
            for atom in input_atoms::resolve_line_atoms(row, line).into_iter().rev() {
                if matches!(atom.kind, InputAtomKind::ImageBadge { .. }) {
                    recalled_line.replace_range(atom.start_byte..atom.end_byte, "");
                }
            }
            recalled_line
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn try_move_input_cursor_down(app: &mut App) -> bool {
    let before = (app.input.cursor_row(), app.input.cursor_col());
    let _ = app.input.textarea_move_down();
    (app.input.cursor_row(), app.input.cursor_col()) != before
}

/// Handle keystrokes while mention/slash autocomplete dropdown is active.
pub(super) fn handle_autocomplete_key(app: &mut App, key: KeyEvent) -> KeyOutcome {
    let Some(context) = autocomplete_key_context(app) else {
        return handle_normal_key(app, key);
    };
    first_handled(handle_keymap_context(app, context, key), || {
        handle_autocomplete_fallback_key(app, key)
    })
}

fn autocomplete_key_context(app: &App) -> Option<KeyContext> {
    app.active_autocomplete_kind().map(|kind| match kind {
        AutocompleteKind::Mention => KeyContext::AutocompleteMention,
        AutocompleteKind::Slash => KeyContext::AutocompleteSlash,
        AutocompleteKind::Subagent => KeyContext::AutocompleteSubagent,
    })
}

fn handle_autocomplete_fallback_key(app: &mut App, key: KeyEvent) -> KeyOutcome {
    match app.active_autocomplete_kind() {
        Some(AutocompleteKind::Mention) => handle_mention_key(app, key),
        Some(AutocompleteKind::Slash) => handle_slash_key(app, key),
        Some(AutocompleteKind::Subagent) => handle_subagent_key(app, key),
        None => handle_normal_key(app, key),
    }
}

/// Handle keystrokes while the `@` mention autocomplete dropdown is active.
pub(super) fn handle_mention_key(app: &mut App, key: KeyEvent) -> KeyOutcome {
    match (key.code, key.modifiers) {
        (KeyCode::Up, _) => {
            mention::move_up(app);
            KeyOutcome::Handled(true)
        }
        (KeyCode::Down, _) => {
            mention::move_down(app);
            KeyOutcome::Handled(true)
        }
        (KeyCode::Enter | KeyCode::Tab, _) => {
            mention::confirm_selection(app);
            KeyOutcome::Handled(true)
        }
        (KeyCode::Esc, _) => {
            mention::deactivate(app);
            KeyOutcome::Handled(true)
        }
        (KeyCode::Backspace, _) => {
            let changed = delete_input_char_before(app);
            mention::update_query(app);
            changed.into()
        }
        (KeyCode::Char(c), m) if is_printable_text_modifiers(m) => {
            let changed = app.input.textarea_insert_char(c);
            mention::update_query(app);
            changed.into()
        }
        // An open menu keeps session controls from reaching the composer.
        _ if is_session_control_action(resolve_key_action_for_context(
            app,
            KeyContext::ChatInput,
            key,
        )) =>
        {
            KeyOutcome::Ignored
        }
        // Any other key: deactivate mention and forward to normal handling
        _ => {
            mention::deactivate(app);
            dispatch_key_by_focus(app, key)
        }
    }
}

/// Handle keystrokes while slash autocomplete dropdown is active.
fn handle_slash_key(app: &mut App, key: KeyEvent) -> KeyOutcome {
    match (key.code, key.modifiers) {
        (KeyCode::Up, _) => {
            if app.slash.visible().is_some_and(|slash| !slash.candidates.is_empty()) {
                slash::move_up(app);
            }
            KeyOutcome::Handled(true)
        }
        (KeyCode::Down, _) => {
            if app.slash.visible().is_some_and(|slash| !slash.candidates.is_empty()) {
                slash::move_down(app);
            }
            KeyOutcome::Handled(true)
        }
        (KeyCode::Enter, KeyModifiers::NONE) => handle_submit(app).into(),
        (KeyCode::Tab, KeyModifiers::NONE) => {
            if app.slash.visible().is_some_and(|slash| !slash.candidates.is_empty()) {
                slash::confirm_selection(app);
            }
            KeyOutcome::Handled(true)
        }
        (KeyCode::Esc, _) => {
            slash::dismiss(app);
            KeyOutcome::Handled(true)
        }
        (KeyCode::Backspace, _) => {
            let changed = delete_input_char_before(app);
            changed.into()
        }
        (KeyCode::Char(c), m) if is_printable_text_modifiers(m) => {
            let changed = app.input.textarea_insert_char(c);
            changed.into()
        }
        _ => handle_normal_key(app, key),
    }
}

/// Handle keystrokes while `&` subagent autocomplete dropdown is active.
fn handle_subagent_key(app: &mut App, key: KeyEvent) -> KeyOutcome {
    match (key.code, key.modifiers) {
        (KeyCode::Up, _) => {
            if app.subagent.as_ref().is_some_and(|subagent| !subagent.query.is_empty()) {
                subagent::move_up(app);
            }
            KeyOutcome::Handled(true)
        }
        (KeyCode::Down, _) => {
            if app.subagent.as_ref().is_some_and(|subagent| !subagent.query.is_empty()) {
                subagent::move_down(app);
            }
            KeyOutcome::Handled(true)
        }
        (KeyCode::Enter | KeyCode::Tab, _) => {
            if app.subagent.as_ref().is_some_and(|subagent| !subagent.query.is_empty()) {
                subagent::confirm_selection(app);
            }
            KeyOutcome::Handled(true)
        }
        (KeyCode::Esc, _) => {
            subagent::deactivate(app);
            KeyOutcome::Handled(true)
        }
        (KeyCode::Backspace, _) => {
            let changed = delete_input_char_before(app);
            subagent::update_query(app);
            changed.into()
        }
        (KeyCode::Char(c), m) if is_printable_text_modifiers(m) => {
            let changed = app.input.textarea_insert_char(c);
            subagent::update_query(app);
            changed.into()
        }
        // An open menu keeps session controls from reaching the composer.
        _ if is_session_control_action(resolve_key_action_for_context(
            app,
            KeyContext::ChatInput,
            key,
        )) =>
        {
            KeyOutcome::Ignored
        }
        _ => {
            subagent::deactivate(app);
            dispatch_key_by_focus(app, key)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::keymap::{KeyBinding, KeyBindingSource, KeyCodeSpec, KeySpec, ResolvedKeymap};
    use crate::app::{ChatMessage, FocusTarget, MessageBlock, MessageRole, TextBlock};
    use crossterm::event::{KeyCode, KeyModifiers};

    #[tokio::test(flavor = "current_thread")]
    async fn mode_shortcut_preserves_the_confirmed_mode_until_sdk_acknowledgement() {
        tokio::task::LocalSet::new().run_until(async {
            let mut app = App::test_default();
            let (connection, mut commands) = crate::agent::client::AgentConnection::test_channel();
            app.session_runtime.conn = Some(std::rc::Rc::new(connection));
            app.session_runtime.session_id = Some("session-1".into());
            let available_modes = vec![crate::app::ModeInfo { id: "default".into(), name: "Default".into() }, crate::app::ModeInfo { id: "plan".into(), name: "Plan".into() }];
            app.session_runtime.mode = Some(crate::app::ModeState { current_mode_id: "default".into(), current_mode_name: "Default".into(), available_modes: available_modes.clone() });
            app.input.set_text("keep my draft");
            let cycle = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
            handle_normal_key(&mut app, cycle);
            assert_eq!(app.session_runtime.mode.as_ref().expect("mode").current_mode_id, "default");
            assert_eq!(app.status, AppStatus::CommandPending);
            tokio::task::yield_now().await;
            assert!(matches!(commands.try_recv().expect("mode change").command, crate::agent::wire::BridgeCommand::SetMode { mode, .. } if mode == "plan"));
            crate::app::handle_client_event(&mut app, crate::agent::events::ClientEvent::SessionUpdate { session_id: "session-1".into(), update: crate::agent::model::SessionUpdate::ModeStateUpdate(crate::app::ModeState { current_mode_id: "plan".into(), current_mode_name: "Plan".into(), available_modes }) });
            assert_eq!(app.session_runtime.mode.as_ref().expect("mode").current_mode_id, "plan");
            assert_eq!(app.status, AppStatus::Ready);
            handle_normal_key(&mut app, cycle);
            tokio::task::yield_now().await;
            assert!(matches!(commands.try_recv().expect("second change").command, crate::agent::wire::BridgeCommand::SetMode { mode, .. } if mode == "default"));
            crate::app::handle_client_event(&mut app, crate::agent::events::ClientEvent::SlashCommandError { session_id: Some("session-1".into()), message: "SDK rejected mode".into() });
            assert_eq!(app.session_runtime.mode.as_ref().expect("mode").current_mode_id, "plan");
            assert_eq!(app.status, AppStatus::Ready);
            assert_eq!(app.input.text(), "keep my draft");
        }).await;
    }

    #[test]
    fn terminal_suspend_action_returns_runtime_command() {
        let mut app = App::test_default();

        let outcome = execute_key_action(
            &mut app,
            KeyAction::Terminal(TerminalAction::Suspend),
            KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL),
        );

        assert_eq!(outcome, KeyOutcome::Runtime(RuntimeCommand::SuspendProcess));
    }

    #[test]
    fn input_action_returns_handled_outcome() {
        let mut app = App::test_default();
        app.input.set_text("ab");
        let _ = app.input.set_cursor(0, 2);

        let outcome = execute_key_action(
            &mut app,
            KeyAction::Input(InputAction::MoveCharLeft),
            KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
        );

        assert_eq!(outcome, KeyOutcome::Handled(true));
        assert_eq!(app.input.cursor_col(), 1);
    }

    fn user_message(text: &str) -> ChatMessage {
        ChatMessage::new(
            MessageRole::User,
            vec![MessageBlock::Text(TextBlock::from_complete(text))],
            None,
        )
    }

    #[test]
    fn autocomplete_backspace_uses_atom_side_effects() {
        let mut app = App::test_default();
        app.input.set_text("[Image #1]");
        let _ = app.input.set_cursor_col("[Image #1]".chars().count());
        app.input.image_payloads = vec![crate::app::clipboard_image::ImageAttachment {
            data: "image".to_owned(),
            mime_type: "image/png".to_owned(),
        }];
        app.mention = Some(mention::MentionState::new(0, 0, String::new(), Vec::new()));
        app.claim_focus_target(FocusTarget::Mention);
        app.surface_dirty.chat.repaint = false;

        let outcome =
            handle_mention_key(&mut app, KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));

        assert_eq!(outcome, KeyOutcome::Handled(true));
        assert!(app.input.is_empty());
        assert!(app.input.image_payloads.is_empty());
        assert!(app.surface_dirty.chat.repaint);
    }

    #[test]
    fn ignored_key_action_allows_printable_chat_fallback() {
        let mut app = App::test_default();
        app.keymap = ResolvedKeymap::from_bindings([KeyBinding::new(
            KeyContext::ChatInput,
            KeySpec::new(KeyCodeSpec::Char('x'), KeyModifiers::NONE),
            KeyAction::Interaction(InteractionAction::MoveNext),
            KeyBindingSource::Config,
        )])
        .expect("custom test keymap should validate");

        let outcome =
            handle_chat_input_key(&mut app, KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));

        assert_eq!(outcome, KeyOutcome::Handled(true));
        assert_eq!(app.input.text(), "x");
    }

    #[test]
    fn autocomplete_focus_routes_keys_to_active_slash_state() {
        let mut app = App::test_default();
        app.slash.show(slash::SlashState {
            trigger_row: 0,
            trigger_col: 0,
            query: "d".to_owned(),
            context: slash::SlashContext::CommandName,
            candidates: vec![
                slash::SlashCandidate {
                    insert_value: "/config".to_owned(),
                    primary: "/config".to_owned(),
                    secondary: None,
                },
                slash::SlashCandidate {
                    insert_value: "/docs".to_owned(),
                    primary: "/docs".to_owned(),
                    secondary: None,
                },
            ],
            placeholder: None,
            dialog: crate::app::dialog::DialogState::default(),
        });
        app.claim_focus_target(FocusTarget::Mention);

        let handled =
            handle_autocomplete_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));

        assert!(handled.changed());
        let slash = app.slash.visible().expect("slash autocomplete should stay active");
        assert_eq!(slash.dialog.selected, 1);
    }

    #[test]
    fn mention_space_key_remains_part_of_active_query() {
        let mut app = App::test_default();
        app.input.set_text("@");
        let _ = app.input.set_cursor(0, 1);
        mention::activate(&mut app);

        let outcome =
            handle_mention_key(&mut app, KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));

        assert!(outcome.changed());
        let mention = app.mention.as_ref().expect("mention should stay active");
        assert_eq!(mention.query, " ");
    }

    #[test]
    fn chat_input_tab_commits_literal_mention_without_candidates() {
        let mut app = App::test_default();
        app.input.set_text("@");
        let _ = app.input.set_cursor(0, 1);
        mention::activate(&mut app);
        app.input.set_text("@docs/manual path");
        let _ = app.input.set_cursor(0, "@docs/manual path".chars().count());
        mention::update_query(&mut app);

        let outcome =
            handle_chat_input_key(&mut app, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));

        assert_eq!(outcome, KeyOutcome::Handled(true));
        assert!(app.mention.is_none());
        assert_eq!(app.input.text(), "@'docs/manual path' ");
        assert_eq!(app.committed_mentions.len(), 1);
        assert_eq!(app.committed_mentions[0].text, "@'docs/manual path'");
    }

    #[test]
    fn autocomplete_tab_still_confirms_mention_candidate() {
        let mut app = App::test_default();
        app.input.set_text("@src");
        let mut mention = mention::MentionState::new(
            0,
            0,
            "src".to_owned(),
            vec![crate::app::file_index::FileCandidate {
                rel_path: "src/lib.rs".to_owned(),
                rel_path_lower: "src/lib.rs".to_owned(),
                basename_lower: "lib.rs".to_owned(),
                depth: 1,
            }],
        );
        mention.replace_end_col = "@src".chars().count();
        app.mention = Some(mention);
        app.claim_focus_target(FocusTarget::Mention);

        let outcome =
            handle_autocomplete_key(&mut app, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));

        assert_eq!(outcome, KeyOutcome::Handled(true));
        assert!(app.mention.is_none());
        assert_eq!(app.input.text(), "@'src/lib.rs' ");
        assert!(app.committed_mentions.is_empty());
    }

    #[test]
    fn bare_slash_enter_confirms_visible_candidate() {
        let mut app = App::test_default();
        app.input.set_text("/");
        let _ = app.input.set_cursor(0, 1);
        slash::sync_with_cursor(&mut app);
        app.claim_focus_target(FocusTarget::Mention);

        let handled =
            handle_autocomplete_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert!(handled.changed());
        assert_eq!(app.input.text(), "/agent ");
        assert!(app.slash.is_visible());
    }

    #[test]
    fn physical_line_keys_preserve_logical_boundaries_and_the_next_edit_position() {
        use crate::app::test_support::{app_with_connection, press};
        for (code, modifiers, positions, edited) in [
            (
                KeyCode::Char('a'),
                KeyModifiers::CONTROL,
                [(1, 0), (0, 0), (0, 0)],
                [
                    "\u{3b1}\u{3b2}\n!caf\u{e9}\n\u{732b}\u{5c3e}",
                    "!\u{3b1}\u{3b2}\ncaf\u{e9}\n\u{732b}\u{5c3e}",
                    "!\u{3b1}\u{3b2}\ncaf\u{e9}\n\u{732b}\u{5c3e}",
                ],
            ),
            (
                KeyCode::Char('e'),
                KeyModifiers::CONTROL,
                [(1, 4), (2, 2), (2, 2)],
                [
                    "\u{3b1}\u{3b2}\ncaf\u{e9}!\n\u{732b}\u{5c3e}",
                    "\u{3b1}\u{3b2}\ncaf\u{e9}\n\u{732b}\u{5c3e}!",
                    "\u{3b1}\u{3b2}\ncaf\u{e9}\n\u{732b}\u{5c3e}!",
                ],
            ),
            (
                KeyCode::Home,
                KeyModifiers::NONE,
                [(1, 0); 3],
                ["\u{3b1}\u{3b2}\n!caf\u{e9}\n\u{732b}\u{5c3e}"; 3],
            ),
            (
                KeyCode::End,
                KeyModifiers::NONE,
                [(1, 4); 3],
                ["\u{3b1}\u{3b2}\ncaf\u{e9}!\n\u{732b}\u{5c3e}"; 3],
            ),
        ] {
            for (count, (position, expected_text)) in positions.into_iter().zip(edited).enumerate()
            {
                let (mut app, mut commands) = app_with_connection();
                app.status = AppStatus::Ready;
                app.input.set_text("\u{3b1}\u{3b2}\ncaf\u{e9}\n\u{732b}\u{5c3e}");
                app.input.set_cursor(1, 2);
                for _ in 0..=count {
                    crate::app::handle_terminal_event(
                        &mut app,
                        crossterm::event::Event::Key(KeyEvent::new(code, modifiers)),
                    );
                }
                assert_eq!(app.input.cursor(), position, "{code:?} press {}", count + 1);
                press(&mut app, KeyCode::Char('!'));
                assert_eq!(app.input.text(), expected_text);
                assert!(commands.try_recv().is_err());
            }
        }
    }

    #[test]
    fn physical_up_recalls_only_user_text_and_preserves_existing_drafts() {
        use crate::agent::wire::BridgeCommand;
        use crate::app::test_support::{app_with_connection, press};
        // History text, starting draft/cursor, recalled draft/cursor, then an independently expected edit.
        for (history, draft, cursor, expected, expected_cursor, edited) in [
            (
                Some("first line\nlatest \u{1f980}"),
                "",
                (0, 0),
                "first line\nlatest \u{1f980}",
                (1, 8),
                Some("first line\nlatest \u{1f980}!"),
            ),
            (
                Some("review [Image #1]\n[Image #2] keep [Image #0] [Image #x]"),
                "",
                (0, 0),
                "review \n keep [Image #0] [Image #x]",
                (1, 27),
                Some("review \n keep [Image #0] [Image #x]!"),
            ),
            (
                Some("history prompt"),
                "\u{e9}abc\nsecond",
                (1, 3),
                "\u{e9}abc\nsecond",
                (0, 3),
                Some("\u{e9}ab!c\nsecond"),
            ),
            (Some("history prompt"), " ", (0, 1), " ", (0, 1), Some(" !")),
            (None, "", (0, 0), "", (0, 0), None),
        ] {
            let (mut app, mut commands) = app_with_connection();
            app.status = AppStatus::Ready;
            if let Some(text) = history {
                app.transcript.messages.push(user_message("older prompt"));
                app.transcript.messages.push(user_message(text));
            }
            for role in [MessageRole::Assistant, MessageRole::System(None)] {
                app.transcript.messages.push(ChatMessage::new(
                    role,
                    vec![MessageBlock::Text(TextBlock::from_complete("newer non-user text"))],
                    None,
                ));
            }
            app.input.set_text(draft);
            app.input.set_cursor(cursor.0, cursor.1);
            press(&mut app, KeyCode::Up);
            assert_eq!(app.input.text(), expected);
            assert_eq!(app.input.cursor(), expected_cursor);
            assert!(commands.try_recv().is_err());
            if let Some(edited) = edited {
                press(&mut app, KeyCode::Char('!'));
                assert_eq!(app.input.text(), edited);
                press(&mut app, KeyCode::Enter);
                let BridgeCommand::Prompt { session_id, chunks, inline_pastes, .. } =
                    commands.try_recv().expect("submitted recalled or preserved draft").command
                else {
                    panic!("expected Prompt")
                };
                assert_eq!(session_id, "session-1");
                assert_eq!(chunks.len(), 1, "no historical image may be resurrected");
                assert_eq!(chunks[0].kind, "text");
                assert_eq!(chunks[0].value, serde_json::json!(edited));
                assert!(inline_pastes.is_empty());
                while let Ok(envelope) = commands.try_recv() {
                    assert!(
                        !matches!(envelope.command, BridgeCommand::Prompt { .. }),
                        "no duplicate prompt"
                    );
                }
            } else {
                press(&mut app, KeyCode::Enter);
                assert!(commands.try_recv().is_err(), "non-user history cannot invent a prompt");
            }
        }
        let (mut app, mut commands) = app_with_connection();
        app.status = AppStatus::Ready;
        press(&mut app, KeyCode::Up);
        assert_eq!(app.input.text(), "");
        assert_eq!(app.input.cursor(), (0, 0));
        press(&mut app, KeyCode::Enter);
        assert!(commands.try_recv().is_err());
    }

    fn assert_deleted_image_prompt(
        command: crate::agent::wire::BridgeCommand,
        expected: &str,
        surviving: &[usize],
    ) {
        use crate::agent::wire::BridgeCommand;
        use base64::Engine as _;
        let BridgeCommand::Prompt { session_id, chunks, .. } = command else {
            panic!("expected Prompt")
        };
        assert_eq!(session_id, "session-1");
        let texts: Vec<_> = chunks.iter().filter(|c| c.kind == "text").map(|c| &c.value).collect();
        assert_eq!(texts, vec![&serde_json::json!(expected)]);
        let images: Vec<_> = chunks.iter().filter(|c| c.kind == "image").collect();
        assert_eq!(images.len(), surviving.len(), "removed images must not travel in Prompt");
        for (chunk, identity) in images.iter().zip(surviving.iter().copied()) {
            assert_eq!(chunk.value["mime_type"], "image/png");
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(chunk.value["data"].as_str().expect("image data"))
                .expect("base64");
            let image = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
                .expect("PNG")
                .into_rgba8();
            let (width, pixels) = if identity == 0 {
                (1, vec![231, 12, 34, 255])
            } else {
                (2, vec![10, 20, 220, 128, 30, 40, 210, 255])
            };
            assert_eq!((image.width(), image.height(), image.into_raw()), (width, 1, pixels));
        }
    }

    #[test]
    fn physical_deletion_preserves_badge_payload_identity_in_the_submitted_prompt() {
        use crate::agent::wire::BridgeCommand;
        use crate::app::clipboard_image::ClipboardImagePaste;
        use crate::app::test_support::{app_with_connection, press};
        use crossterm::event::{Event, KeyEventKind};
        fn red() -> ClipboardRead {
            ClipboardRead::Image(arboard::ImageData {
                width: 1,
                height: 1,
                bytes: vec![231, 12, 34, 255].into(),
            })
        }
        fn blue() -> ClipboardRead {
            ClipboardRead::Image(arboard::ImageData {
                width: 2,
                height: 1,
                bytes: vec![10, 20, 220, 128, 30, 40, 210, 255].into(),
            })
        }
        let removed = "\u{e9}  x\nz [Image #1]\u{5c3e}";
        for (key, modifiers, col, expected, surviving) in [
            (KeyCode::Backspace, KeyModifiers::NONE, 12, removed, vec![1]),
            (KeyCode::Delete, KeyModifiers::NONE, 2, removed, vec![1]),
            (KeyCode::Backspace, KeyModifiers::CONTROL, 12, removed, vec![1]),
            (KeyCode::Delete, KeyModifiers::CONTROL, 2, removed, vec![1]),
            (KeyCode::Backspace, KeyModifiers::ALT, 12, removed, vec![1]),
            (KeyCode::Delete, KeyModifiers::ALT, 2, removed, vec![1]),
            (KeyCode::Char('w'), KeyModifiers::CONTROL, 12, removed, vec![1]),
            (KeyCode::Char('d'), KeyModifiers::ALT, 2, removed, vec![1]),
            (KeyCode::Char('u'), KeyModifiers::CONTROL, 12, " x\nz [Image #1]\u{5c3e}", vec![1]),
            (
                KeyCode::Char('k'),
                KeyModifiers::CONTROL,
                2,
                "\u{e9} \nz [Image #1]\u{5c3e}",
                vec![1],
            ),
            (
                KeyCode::Backspace,
                KeyModifiers::NONE,
                2,
                "\u{e9}[Image #1] x\nz [Image #2]\u{5c3e}",
                vec![0, 1],
            ),
        ] {
            for restore in [false, true] {
                if restore && !matches!(key, KeyCode::Char('u' | 'k')) {
                    continue;
                }
                let (mut app, mut commands) = app_with_connection();
                app.status = AppStatus::Ready;
                app.input.set_text("\u{e9}  x\nz \u{5c3e}");
                for (row, read) in [(0, red as fn() -> ClipboardRead), (1, blue)] {
                    app.input.set_cursor(row, 2);
                    app.clipboard_paste =
                        ClipboardImagePaste { trigger: KeyEventKind::Press, read };
                    crate::app::handle_terminal_event(
                        &mut app,
                        Event::Key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL)),
                    );
                }
                assert_eq!(app.input.text(), "\u{e9} [Image #1] x\nz [Image #2]\u{5c3e}");
                assert!(commands.try_recv().is_err());
                app.input.set_cursor(0, col);
                crate::app::handle_terminal_event(
                    &mut app,
                    Event::Key(KeyEvent::new(key, modifiers)),
                );
                assert_eq!(app.input.text(), expected, "{key:?} {modifiers:?}");
                assert!(commands.try_recv().is_err());
                let (expected, surviving) = if restore {
                    crate::app::handle_terminal_event(
                        &mut app,
                        Event::Key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL)),
                    );
                    ("\u{e9} [Image #2] x\nz [Image #1]\u{5c3e}", vec![1, 0])
                } else {
                    (expected, surviving.clone())
                };
                assert_eq!(app.input.text(), expected);

                press(&mut app, KeyCode::Enter);
                assert_deleted_image_prompt(
                    commands.try_recv().expect("prompt after deletion").command,
                    expected,
                    &surviving,
                );
                assert!(app.input.is_empty());
                assert!(app.input.image_payloads.is_empty());
                while let Ok(envelope) = commands.try_recv() {
                    assert!(
                        !matches!(envelope.command, BridgeCommand::Prompt { .. }),
                        "no duplicate prompt"
                    );
                }
            }
        }
    }

    #[test]
    fn line_kill_yank_keeps_paste_payloads_when_their_old_numbers_are_reused() {
        use crate::agent::wire::BridgeCommand;
        use crate::app::test_support::{app_with_connection, press};
        use crossterm::event::Event;
        let (mut app, mut commands) = app_with_connection();
        app.status = AppStatus::Ready;
        let first = format!("first-{}-end", "a".repeat(1001));
        let second = format!("second-{}-end", "b".repeat(1002));
        let fresh = format!("fresh-{}-end", "c".repeat(1003));
        app.input.insert_paste_block(&first);
        app.input.insert_str(" + ");
        app.input.insert_paste_block(&second);
        crate::app::handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL)),
        );
        assert_eq!(app.input.text(), "");
        app.input.insert_paste_block(&fresh);
        app.input.insert_str(" / ");
        crate::app::handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL)),
        );
        let expected = format!("{fresh} / {first} + {second}");
        assert_eq!(app.input.text(), expected);
        assert!(commands.try_recv().is_err());
        press(&mut app, KeyCode::Enter);
        let BridgeCommand::Prompt { session_id, chunks, inline_pastes, .. } =
            commands.try_recv().expect("prompt after reuse").command
        else {
            panic!("expected Prompt")
        };
        assert_eq!(session_id, "session-1");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].kind, "text");
        assert_eq!(chunks[0].value, serde_json::json!(expected));
        assert_eq!(inline_pastes, [fresh, first, second]);
        while let Ok(envelope) = commands.try_recv() {
            assert!(!matches!(envelope.command, BridgeCommand::Prompt { .. }));
        }
    }
}
