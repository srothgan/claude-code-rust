// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::paste_burst::CharAction;
use super::{App, AppStatus, CancelOrigin, FocusOwner, InvalidationLevel, ModeInfo, ModeState};
#[cfg(not(test))]
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
use crate::app::{mention, permissions, questions, slash, subagent};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::rc::Rc;
use std::time::Instant;

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
    if matches!(app.status, AppStatus::Connecting | AppStatus::CommandPending | AppStatus::Error)
        || app.is_compacting
    {
        return handle_keymap_context(app, KeyContext::ChatBlocked, key);
    }

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

    if should_ignore_key_during_paste(app, key) {
        return KeyOutcome::Ignored;
    }

    let outcome = handle_chat_input_key(app, key);

    if app.input.version != input_version_before {
        mention::sync_with_cursor(app);
        slash::sync_with_cursor(app);
        subagent::sync_with_cursor(app);
    }

    outcome
}

fn should_ignore_key_during_paste(app: &mut App, key: KeyEvent) -> bool {
    if paste_suppression_bypass_action(app, key).is_some() {
        return false;
    }
    if app.pending_submit.is_some() && is_editing_like_key(key) {
        app.pending_submit = None;
    }
    !app.pending_paste_text.is_empty() && is_editing_like_key(key)
}

fn paste_suppression_bypass_action(app: &App, key: KeyEvent) -> Option<KeyAction> {
    resolve_key_action_for_context(app, KeyContext::ChatInput, key).filter(|action| {
        matches!(
            action,
            KeyAction::App(
                AppAction::ClearInputOrQuit
                    | AppAction::Quit
                    | AppAction::Redraw
                    | AppAction::CancelTurn
            )
        )
    })
}

fn is_editing_like_key(key: KeyEvent) -> bool {
    matches!(
        key.code,
        KeyCode::Char(_) | KeyCode::Enter | KeyCode::Tab | KeyCode::Backspace | KeyCode::Delete
    )
}

fn should_reclaim_input_focus_before_inline_interaction(app: &App, key: KeyEvent) -> bool {
    let question_notes_editing = questions::focused_question_is_editing_notes(app);
    match key.code {
        KeyCode::Backspace | KeyCode::Delete => !question_notes_editing,
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
            app.should_quit = true;
            KeyOutcome::Handled(true)
        }
        AppAction::ClearInputOrQuit => clear_input_or_quit(app).into(),
        AppAction::Redraw => {
            app.request_chat_visible_rebuild();
            KeyOutcome::Handled(true)
        }
        AppAction::CancelTurn => handle_turn_control(app).into(),
        AppAction::SubmitInput => handle_submit(app).into(),
        AppAction::FocusPromptOrAcceptSuggestion => {
            (handle_focus_toggle(app) || handle_prompt_suggestion(app)).into()
        }
        AppAction::CycleMode => handle_mode_cycle(app).into(),
    }
}

fn clear_input_or_quit(app: &mut App) -> bool {
    let has_local_input = !app.input.is_empty()
        || !app.pending_images.is_empty()
        || !app.pending_paste_text.is_empty()
        || app.pending_submit.is_some();
    if !has_local_input {
        app.should_quit = true;
        return true;
    }

    app.input.clear();
    app.pending_images.clear();
    app.pending_paste_text.clear();
    app.pending_paste_session = None;
    app.active_paste_session = None;
    app.pending_submit = None;
    app.request_chat_repaint();
    true
}

fn execute_input_action(app: &mut App, action: InputAction) -> KeyOutcome {
    reclaim_input_from_inline_prompt_if_needed(app);
    let handled = match action {
        InputAction::MoveCharLeft => app.input.textarea_move_left(),
        InputAction::MoveCharRight => app.input.textarea_move_right(),
        InputAction::MoveWordLeft => app.input.textarea_move_word_left(),
        InputAction::MoveWordRight => app.input.textarea_move_word_right(),
        InputAction::MoveLineStart => app.input.textarea_move_home(),
        InputAction::MoveLineEnd => app.input.textarea_move_end(),
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
        InputAction::KillLineStart => app.input.textarea_delete_line_before(),
        InputAction::KillLineEnd => app.input.textarea_delete_line_after(),
        InputAction::Yank => app.input.textarea_yank(),
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
    handled.into()
}

fn delete_input_char_before(app: &mut App) -> bool {
    if try_delete_image_badge(app, "before") {
        return true;
    }
    app.input.textarea_delete_char_before()
}

fn delete_input_char_after(app: &mut App) -> bool {
    if try_delete_image_badge(app, "after") {
        return true;
    }
    app.input.textarea_delete_char_after()
}

fn delete_input_word_before(app: &mut App) -> bool {
    if try_delete_image_badge(app, "before") {
        return true;
    }
    app.input.textarea_delete_word_before()
}

fn delete_input_word_after(app: &mut App) -> bool {
    if try_delete_image_badge(app, "after") {
        return true;
    }
    app.input.textarea_delete_word_after()
}

fn insert_explicit_newline(app: &mut App) -> bool {
    if app.paste_burst.on_enter(Instant::now()) {
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
            if app.slash.as_ref().is_some_and(|slash| !slash.candidates.is_empty()) {
                slash::move_up(app);
            }
        }
        AutocompleteAction::MoveNext => {
            if app.slash.as_ref().is_some_and(|slash| !slash.candidates.is_empty()) {
                slash::move_down(app);
            }
        }
        AutocompleteAction::Confirm => {
            if app.slash.as_ref().is_some_and(|slash| !slash.candidates.is_empty()) {
                slash::confirm_selection(app);
            }
        }
        AutocompleteAction::Cancel => slash::deactivate(app),
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
    if questions::has_focused_question(app) {
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
    if !app.pending_images.is_empty() {
        app.pending_images.clear();
        app.request_chat_repaint();
    }
    if matches!(app.status, AppStatus::Thinking | AppStatus::Running)
        && let Err(message) = super::input_submit::request_cancel(app, CancelOrigin::Manual)
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
    let now = Instant::now();

    // During an active burst or the post-burst suppression window, Enter
    // becomes a newline to keep multi-line pastes grouped.
    if app.paste_burst.on_enter(now) {
        tracing::debug!(
            target: crate::logging::targets::APP_INPUT,
            event_name = "enter_routed_to_paste_buffer",
            message = "enter was routed through the paste buffer",
            outcome = "success",
        );
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

    let Some(suggestion) = app.prompt_suggestion.take() else {
        return false;
    };
    if suggestion.trim().is_empty() {
        return false;
    }
    app.input.set_text(&suggestion);
    true
}

fn handle_mode_cycle(app: &mut App) -> bool {
    let Some(ref mode) = app.mode else {
        return true;
    };
    if mode.available_modes.len() <= 1 {
        return true;
    }

    let current_idx =
        mode.available_modes.iter().position(|m| m.id == mode.current_mode_id).unwrap_or(0);
    let next_idx = (current_idx + 1) % mode.available_modes.len();
    let next = &mode.available_modes[next_idx];

    if let Some(ref conn) = app.conn
        && let Some(sid) = app.session_id.clone()
    {
        let mode_id = next.id.clone();
        let conn = Rc::clone(conn);
        tokio::task::spawn_local(async move {
            if let Err(e) = conn.set_mode(sid.to_string(), mode_id) {
                tracing::error!(
                    target: crate::logging::targets::APP_INPUT,
                    event_name = "mode_change_request_failed",
                    message = "failed to request mode change",
                    outcome = "failure",
                    error_message = %e,
                );
            }
        });
    }

    let next_id = next.id.clone();
    let next_name = next.name.clone();
    let modes = mode
        .available_modes
        .iter()
        .map(|m| ModeInfo { id: m.id.clone(), name: m.name.clone() })
        .collect();
    app.mode = Some(ModeState {
        current_mode_id: next_id,
        current_mode_name: next_name,
        available_modes: modes,
    });
    app.invalidate_layout(InvalidationLevel::Global);
    true
}

fn handle_clipboard_paste_key(app: &mut App, key: KeyEvent) -> bool {
    if !is_clipboard_paste_shortcut(key) {
        return false;
    }
    if key.kind != KeyEventKind::Release {
        return false;
    }

    // Skip system clipboard access in tests to avoid flaky failures / segfaults.
    #[cfg(test)]
    {
        let _ = app;
        false
    }
    #[cfg(not(test))]
    {
        let Ok(mut clipboard) = arboard::Clipboard::new() else {
            super::events::push_system_message_with_severity(
                app,
                Some(SystemSeverity::Warning),
                "Failed to access the system clipboard.",
            );
            app.request_chat_repaint();
            tracing::warn!("clipboard_paste: failed to access system clipboard");
            return true;
        };

        // Try reading an image from the clipboard first.
        if let Ok(img_data) = clipboard.get_image() {
            match super::clipboard_image::encode_clipboard_image(img_data) {
                Ok(attachment) => {
                    app.pending_images.push(attachment);
                    // Insert badge text at the cursor position so the user (and
                    // the model) can see where images are relative to text.
                    let idx = app.pending_images.len();
                    let badge = format!("[Image #{idx}]");
                    app.input.insert_str(&badge);
                    app.request_chat_repaint();
                    tracing::debug!(
                        count = app.pending_images.len(),
                        "clipboard_paste: attached image from clipboard"
                    );
                    return true;
                }
                Err(error) => {
                    super::events::push_system_message_with_severity(
                        app,
                        Some(SystemSeverity::Warning),
                        error.user_message(),
                    );
                    app.request_chat_repaint();
                    tracing::warn!("clipboard_paste: image attachment failed: {error:?}");
                    return true;
                }
            }
        }

        false
    }
}

pub(super) fn is_clipboard_paste_shortcut(key: KeyEvent) -> bool {
    is_ctrl_char_shortcut(key, 'v')
}

pub(super) fn reclaim_input_from_inline_prompt_if_needed(app: &mut App) {
    if app.focus_owner() == FocusOwner::Permission {
        clear_inline_interaction_focus(app);
    }
}

/// If the cursor is inside or adjacent to an `[Image #N]` badge, delete the
/// entire badge, remove the associated image from `pending_images`, and
/// renumber remaining badges. Returns `true` if a badge was deleted.
fn try_delete_image_badge(app: &mut App, direction: &str) -> bool {
    let Some(one_based_idx) = app.input.delete_image_badge(direction) else {
        return false;
    };
    let array_idx = one_based_idx.saturating_sub(1);
    if array_idx < app.pending_images.len() {
        app.pending_images.remove(array_idx);
    }
    app.input.renumber_image_badges();
    app.request_chat_repaint();
    true
}

fn handle_printable_key(app: &mut App, key: KeyEvent) -> bool {
    let (KeyCode::Char(c), m) = (key.code, key.modifiers) else {
        // Non-char key: reset burst state to prevent leakage.
        app.paste_burst.on_non_char_key(Instant::now());
        return false;
    };
    if !is_printable_text_modifiers(m) {
        return false;
    }
    reclaim_input_from_inline_prompt_if_needed(app);

    let now = Instant::now();
    match app.paste_burst.on_char(c, now) {
        CharAction::Consumed => {
            // Character absorbed into burst buffer. Don't insert.
            return false;
        }
        CharAction::RetroCapture(delete_count) => {
            // Burst confirmation retro-captured already-inserted leading chars.
            for _ in 0..delete_count {
                let _ = app.input.textarea_delete_char_before();
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
            // Normal typing or a previously-held char released.
            // If `ch == c`, single normal insert. Otherwise the detector
            // emitted a held char; insert it first, then the current char.
            if ch == c {
                let _ = app.input.textarea_insert_char(c);
            } else {
                let _ = app.input.textarea_insert_char(ch);
                let _ = app.input.textarea_insert_char(c);
            }
        }
    }

    true
}

fn try_move_input_cursor_up(app: &mut App) -> bool {
    let before = (app.input.cursor_row(), app.input.cursor_col());
    let _ = app.input.textarea_move_up();
    (app.input.cursor_row(), app.input.cursor_col()) != before
}

fn try_move_input_cursor_down(app: &mut App) -> bool {
    let before = (app.input.cursor_row(), app.input.cursor_col());
    let _ = app.input.textarea_move_down();
    (app.input.cursor_row(), app.input.cursor_col()) != before
}

/// Handle keystrokes while mention/slash autocomplete dropdown is active.
pub(super) fn handle_autocomplete_key(app: &mut App, key: KeyEvent) -> KeyOutcome {
    let context = match app.active_autocomplete_kind() {
        Some(AutocompleteKind::Mention) => KeyContext::AutocompleteMention,
        Some(AutocompleteKind::Slash) => KeyContext::AutocompleteSlash,
        Some(AutocompleteKind::Subagent) => KeyContext::AutocompleteSubagent,
        None => return handle_normal_key(app, key),
    };
    first_handled(handle_keymap_context(app, context, key), || {
        handle_autocomplete_fallback_key(app, key)
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
            let changed = app.input.textarea_delete_char_before();
            mention::update_query(app);
            changed.into()
        }
        (KeyCode::Char(c), m) if is_printable_text_modifiers(m) => {
            let changed = app.input.textarea_insert_char(c);
            if c.is_whitespace() {
                mention::deactivate(app);
            } else {
                mention::update_query(app);
            }
            changed.into()
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
            if app.slash.as_ref().is_some_and(|slash| !slash.candidates.is_empty()) {
                slash::move_up(app);
            }
            KeyOutcome::Handled(true)
        }
        (KeyCode::Down, _) => {
            if app.slash.as_ref().is_some_and(|slash| !slash.candidates.is_empty()) {
                slash::move_down(app);
            }
            KeyOutcome::Handled(true)
        }
        (KeyCode::Enter | KeyCode::Tab, _) => {
            if app.slash.as_ref().is_some_and(|slash| !slash.candidates.is_empty()) {
                slash::confirm_selection(app);
            }
            KeyOutcome::Handled(true)
        }
        (KeyCode::Esc, _) => {
            slash::deactivate(app);
            KeyOutcome::Handled(true)
        }
        (KeyCode::Backspace, _) => {
            let changed = app.input.textarea_delete_char_before();
            slash::update_query(app);
            changed.into()
        }
        (KeyCode::Char(c), m) if is_printable_text_modifiers(m) => {
            let changed = app.input.textarea_insert_char(c);
            slash::update_query(app);
            changed.into()
        }
        _ => {
            slash::deactivate(app);
            dispatch_key_by_focus(app, key)
        }
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
            let changed = app.input.textarea_delete_char_before();
            subagent::update_query(app);
            changed.into()
        }
        (KeyCode::Char(c), m) if is_printable_text_modifiers(m) => {
            let changed = app.input.textarea_insert_char(c);
            subagent::update_query(app);
            changed.into()
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
    use crate::app::FocusTarget;
    use crate::app::keymap::{KeyBinding, KeyBindingSource, KeyCodeSpec, KeySpec, ResolvedKeymap};
    use crossterm::event::{KeyCode, KeyModifiers};
    use std::time::{Duration, Instant};

    #[test]
    fn ctrl_shortcut_accepts_standard_ctrl_v_encoding() {
        let key = KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL);
        assert!(is_ctrl_char_shortcut(key, 'v'));
    }

    #[test]
    fn ctrl_shortcut_accepts_raw_control_character_encoding() {
        let key = KeyEvent::new(KeyCode::Char('\u{16}'), KeyModifiers::NONE);
        assert!(is_ctrl_char_shortcut(key, 'v'));
    }

    #[test]
    fn ctrl_shortcut_rejects_raw_control_character_with_alt() {
        let key = KeyEvent::new(KeyCode::Char('\u{16}'), KeyModifiers::ALT);
        assert!(!is_ctrl_char_shortcut(key, 'v'));
    }

    #[test]
    fn queued_paste_still_blocks_overlapping_key_text() {
        let mut app = App::test_default();
        app.pending_paste_text = "clipboard".to_owned();

        let blocked = should_ignore_key_during_paste(
            &mut app,
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
        );
        assert!(blocked);
    }

    #[test]
    fn queued_paste_allows_app_control_shortcuts() {
        let mut app = App::test_default();
        app.pending_paste_text = "clipboard".to_owned();

        let blocked = should_ignore_key_during_paste(
            &mut app,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );
        assert!(!blocked);
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
        app.slash = Some(slash::SlashState {
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
            dialog: crate::app::dialog::DialogState::default(),
        });
        app.claim_focus_target(FocusTarget::Mention);

        let handled =
            handle_autocomplete_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));

        assert!(handled.changed());
        let slash = app.slash.as_ref().expect("slash autocomplete should stay active");
        assert_eq!(slash.dialog.selected, 1);
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
        assert_eq!(app.input.text(), "/1m-context ");
        assert!(app.slash.is_some());
    }

    #[test]
    fn burst_active_does_not_block_followup_chars() {
        let mut app = App::test_default();
        let t0 = Instant::now();

        assert_eq!(app.paste_burst.on_char('a', t0), CharAction::Passthrough('a'));
        assert_eq!(
            app.paste_burst.on_char('b', t0 + Duration::from_millis(1)),
            CharAction::Consumed
        );
        assert!(app.paste_burst.is_buffering());

        let blocked = should_ignore_key_during_paste(
            &mut app,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE),
        );
        assert!(!blocked);
    }
}
