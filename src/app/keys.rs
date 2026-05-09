// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::paste_burst::CharAction;
use super::{App, AppStatus, CancelOrigin, FocusOwner, InvalidationLevel, ModeInfo, ModeState};
#[cfg(not(test))]
use crate::app::SystemSeverity;
use crate::app::inline_interactions::{
    clear_inline_interaction_focus, focus_next_inline_interaction, handle_inline_interaction_key,
};
use crate::app::state::AutocompleteKind;
use crate::app::{mention, questions, slash, subagent};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::rc::Rc;
use std::time::Instant;

#[cfg(target_os = "macos")]
pub(crate) const CMD_MOD: KeyModifiers = KeyModifiers::SUPER;
#[cfg(not(target_os = "macos"))]
pub(crate) const CMD_MOD: KeyModifiers = KeyModifiers::CONTROL;

#[cfg(target_os = "macos")]
pub(crate) const WORD_NAV_MOD: KeyModifiers = KeyModifiers::ALT;
#[cfg(not(target_os = "macos"))]
pub(crate) const WORD_NAV_MOD: KeyModifiers = KeyModifiers::CONTROL;

#[cfg(target_os = "macos")]
pub(crate) const WORD_NAV_MOD_EXCLUDED: KeyModifiers = KeyModifiers::empty();
#[cfg(not(target_os = "macos"))]
pub(crate) const WORD_NAV_MOD_EXCLUDED: KeyModifiers = KeyModifiers::ALT;

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

fn is_permission_ctrl_shortcut(key: KeyEvent) -> bool {
    is_ctrl_char_shortcut(key, 'y')
        || is_ctrl_char_shortcut(key, 'a')
        || is_ctrl_char_shortcut(key, 'n')
}

fn handle_always_allowed_shortcuts(app: &mut App, key: KeyEvent) -> bool {
    if is_ctrl_char_shortcut(key, 'q') {
        app.should_quit = true;
        return true;
    }
    if is_ctrl_char_shortcut(key, 'c') {
        app.should_quit = true;
        return true;
    }
    false
}

pub(super) fn dispatch_key_by_focus(app: &mut App, key: KeyEvent) -> bool {
    if handle_always_allowed_shortcuts(app, key) {
        return true;
    }

    if matches!(app.status, AppStatus::Connecting | AppStatus::CommandPending | AppStatus::Error)
        || app.is_compacting
    {
        return handle_blocked_input_shortcuts(app, key);
    }

    if handle_global_shortcuts(app, key) {
        return true;
    }

    match app.focus_owner() {
        FocusOwner::Mention => handle_autocomplete_key(app, key),
        FocusOwner::Permission => {
            if should_reclaim_input_focus_before_inline_interaction(app, key) {
                reclaim_input_from_inline_prompt_if_needed(app);
                handle_normal_key(app, key)
            } else if handle_inline_interaction_key(app, key) {
                true
            } else {
                handle_normal_key(app, key)
            }
        }
        FocusOwner::Input => handle_normal_key(app, key),
    }
}

/// During blocked-input states (Connecting, `CommandPending`, Error), keep input disabled and only allow
/// navigation/help shortcuts.
fn handle_blocked_input_shortcuts(app: &mut App, key: KeyEvent) -> bool {
    if is_ctrl_char_shortcut(key, 'l') {
        app.request_chat_visible_rebuild();
        return true;
    }
    false
}

/// Handle shortcuts that should work regardless of current focus owner.
fn handle_global_shortcuts(app: &mut App, key: KeyEvent) -> bool {
    // Permission quick shortcuts are global when permissions are pending.
    if !app.pending_interaction_ids.is_empty() && is_permission_ctrl_shortcut(key) {
        return handle_inline_interaction_key(app, key);
    }

    match (key.code, key.modifiers) {
        (KeyCode::Char('l'), m) if m == KeyModifiers::CONTROL => {
            app.request_chat_visible_rebuild();
            true
        }
        _ => false,
    }
}

#[inline]
pub(super) fn is_printable_text_modifiers(modifiers: KeyModifiers) -> bool {
    let ctrl_alt =
        modifiers.contains(KeyModifiers::CONTROL) && modifiers.contains(KeyModifiers::ALT);
    !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) || ctrl_alt
}

pub(super) fn handle_normal_key(app: &mut App, key: KeyEvent) -> bool {
    let input_version_before = app.input.version;

    if should_ignore_key_during_paste(app, key) {
        return false;
    }

    let changed = handle_normal_key_actions(app, key);

    if app.input.version != input_version_before && should_sync_autocomplete_after_key(app, key) {
        mention::sync_with_cursor(app);
        slash::sync_with_cursor(app);
        subagent::sync_with_cursor(app);
    }

    changed
}

fn should_ignore_key_during_paste(app: &mut App, key: KeyEvent) -> bool {
    if app.pending_submit.is_some() && is_editing_like_key(key) {
        app.pending_submit = None;
    }
    !app.pending_paste_text.is_empty() && is_editing_like_key(key)
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

fn handle_normal_key_actions(app: &mut App, key: KeyEvent) -> bool {
    if handle_turn_control_key(app, key) {
        return true;
    }
    if handle_submit_key(app, key) {
        return true;
    }
    if handle_history_key(app, key) {
        return true;
    }
    if handle_navigation_key(app, key) {
        return true;
    }
    if handle_focus_toggle_key(app, key) {
        return true;
    }
    if handle_prompt_suggestion_key(app, key) {
        return true;
    }
    if handle_mode_cycle_key(app, key) {
        return true;
    }
    if handle_clipboard_paste_key(app, key) {
        return true;
    }
    if handle_editing_key(app, key) {
        return true;
    }
    handle_printable_key(app, key)
}

fn handle_turn_control_key(app: &mut App, key: KeyEvent) -> bool {
    if !matches!(key.code, KeyCode::Esc) {
        return false;
    }
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

fn handle_submit_key(app: &mut App, key: KeyEvent) -> bool {
    if !matches!(key.code, KeyCode::Enter) {
        return false;
    }

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

    if !key.modifiers.contains(KeyModifiers::SHIFT)
        && !key.modifiers.contains(KeyModifiers::CONTROL)
    {
        app.pending_submit = Some(app.input.snapshot());
        tracing::debug!(
            target: crate::logging::targets::APP_INPUT,
            event_name = "deferred_submit_armed",
            message = "deferred submit snapshot armed",
            outcome = "start",
        );
        return false;
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

fn handle_history_key(app: &mut App, key: KeyEvent) -> bool {
    if is_undo_shortcut(key.code, key.modifiers) {
        app.input.textarea_undo();
        return true;
    }
    if is_redo_shortcut(key.code, key.modifiers) {
        app.input.textarea_redo();
        return true;
    }
    false
}

fn is_undo_shortcut(code: KeyCode, modifiers: KeyModifiers) -> bool {
    matches!(code, KeyCode::Char('z')) && modifiers == CMD_MOD
}

#[cfg(target_os = "macos")]
fn is_redo_shortcut(code: KeyCode, modifiers: KeyModifiers) -> bool {
    let command_shift_z = matches!(code, KeyCode::Char('z' | 'Z'))
        && modifiers.contains(CMD_MOD)
        && modifiers.contains(KeyModifiers::SHIFT)
        && !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
    let command_upper_z = matches!(code, KeyCode::Char('Z')) && modifiers == CMD_MOD;
    let command_y = matches!(code, KeyCode::Char('y')) && modifiers == CMD_MOD;
    command_shift_z || command_upper_z || command_y
}

#[cfg(not(target_os = "macos"))]
fn is_redo_shortcut(code: KeyCode, modifiers: KeyModifiers) -> bool {
    matches!(code, KeyCode::Char('y')) && modifiers == CMD_MOD
}

fn handle_navigation_key(app: &mut App, key: KeyEvent) -> bool {
    match (key.code, key.modifiers) {
        (KeyCode::Left, m) if m.contains(WORD_NAV_MOD) && !m.intersects(WORD_NAV_MOD_EXCLUDED) => {
            app.input.textarea_move_word_left()
        }
        (KeyCode::Right, m) if m.contains(WORD_NAV_MOD) && !m.intersects(WORD_NAV_MOD_EXCLUDED) => {
            app.input.textarea_move_word_right()
        }
        (KeyCode::Left, _) => app.input.textarea_move_left(),
        (KeyCode::Right, _) => app.input.textarea_move_right(),
        (KeyCode::Up, _) => {
            let _ = try_move_input_cursor_up(app);
            true
        }
        (KeyCode::Down, _) => {
            let _ = try_move_input_cursor_down(app);
            true
        }
        (KeyCode::Home, _) => app.input.textarea_move_home(),
        (KeyCode::End, _) => app.input.textarea_move_end(),
        _ => false,
    }
}

fn handle_focus_toggle_key(app: &mut App, key: KeyEvent) -> bool {
    match (key.code, key.modifiers) {
        (KeyCode::Tab, m)
            if !m.contains(KeyModifiers::SHIFT)
                && !m.contains(KeyModifiers::CONTROL)
                && !m.contains(KeyModifiers::ALT) =>
        {
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
        _ => false,
    }
}

fn handle_prompt_suggestion_key(app: &mut App, key: KeyEvent) -> bool {
    if !matches!(key.code, KeyCode::Tab)
        || !key.modifiers.is_empty()
        || app.focus_owner() != FocusOwner::Input
        || !app.input.is_empty()
    {
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

fn handle_mode_cycle_key(app: &mut App, key: KeyEvent) -> bool {
    if !matches!(key.code, KeyCode::BackTab) {
        return false;
    }
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

fn handle_editing_key(app: &mut App, key: KeyEvent) -> bool {
    match (key.code, key.modifiers) {
        (KeyCode::Backspace, m)
            if m.contains(WORD_NAV_MOD) && !m.intersects(WORD_NAV_MOD_EXCLUDED) =>
        {
            reclaim_input_from_inline_prompt_if_needed(app);
            if try_delete_image_badge(app, "before") {
                return true;
            }
            app.input.textarea_delete_word_before()
        }
        (KeyCode::Delete, m)
            if m.contains(WORD_NAV_MOD) && !m.intersects(WORD_NAV_MOD_EXCLUDED) =>
        {
            reclaim_input_from_inline_prompt_if_needed(app);
            if try_delete_image_badge(app, "after") {
                return true;
            }
            app.input.textarea_delete_word_after()
        }
        (KeyCode::Backspace, _) => {
            reclaim_input_from_inline_prompt_if_needed(app);
            if try_delete_image_badge(app, "before") {
                return true;
            }
            app.input.textarea_delete_char_before()
        }
        (KeyCode::Delete, _) => {
            reclaim_input_from_inline_prompt_if_needed(app);
            if try_delete_image_badge(app, "after") {
                return true;
            }
            app.input.textarea_delete_char_after()
        }
        _ => false,
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

fn should_sync_autocomplete_after_key(_app: &App, key: KeyEvent) -> bool {
    match (key.code, key.modifiers) {
        (
            KeyCode::Up
            | KeyCode::Down
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Home
            | KeyCode::End
            | KeyCode::Backspace
            | KeyCode::Delete
            | KeyCode::Enter,
            _,
        ) => true,
        (code, modifiers)
            if is_undo_shortcut(code, modifiers) || is_redo_shortcut(code, modifiers) =>
        {
            true
        }
        (KeyCode::Char(_), m) if is_printable_text_modifiers(m) => true,
        _ => false,
    }
}

/// Handle keystrokes while mention/slash autocomplete dropdown is active.
pub(super) fn handle_autocomplete_key(app: &mut App, key: KeyEvent) -> bool {
    match app.active_autocomplete_kind() {
        Some(AutocompleteKind::Mention) => return handle_mention_key(app, key),
        Some(AutocompleteKind::Slash) => return handle_slash_key(app, key),
        Some(AutocompleteKind::Subagent) => return handle_subagent_key(app, key),
        None => {}
    }
    dispatch_key_by_focus(app, key)
}

/// Handle keystrokes while the `@` mention autocomplete dropdown is active.
pub(super) fn handle_mention_key(app: &mut App, key: KeyEvent) -> bool {
    match (key.code, key.modifiers) {
        (KeyCode::Up, _) => {
            mention::move_up(app);
            true
        }
        (KeyCode::Down, _) => {
            mention::move_down(app);
            true
        }
        (KeyCode::Enter | KeyCode::Tab, _) => {
            mention::confirm_selection(app);
            true
        }
        (KeyCode::Esc, _) => {
            mention::deactivate(app);
            true
        }
        (KeyCode::Backspace, _) => {
            let changed = app.input.textarea_delete_char_before();
            mention::update_query(app);
            changed
        }
        (KeyCode::Char(c), m) if is_printable_text_modifiers(m) => {
            let changed = app.input.textarea_insert_char(c);
            if c.is_whitespace() {
                mention::deactivate(app);
            } else {
                mention::update_query(app);
            }
            changed
        }
        // Any other key: deactivate mention and forward to normal handling
        _ => {
            mention::deactivate(app);
            dispatch_key_by_focus(app, key)
        }
    }
}

/// Handle keystrokes while slash autocomplete dropdown is active.
fn handle_slash_key(app: &mut App, key: KeyEvent) -> bool {
    match (key.code, key.modifiers) {
        (KeyCode::Up, _) => {
            if app.slash.as_ref().is_some_and(|slash| !slash.candidates.is_empty()) {
                slash::move_up(app);
            }
            true
        }
        (KeyCode::Down, _) => {
            if app.slash.as_ref().is_some_and(|slash| !slash.candidates.is_empty()) {
                slash::move_down(app);
            }
            true
        }
        (KeyCode::Enter | KeyCode::Tab, _) => {
            if app.slash.as_ref().is_some_and(|slash| !slash.candidates.is_empty()) {
                slash::confirm_selection(app);
            }
            true
        }
        (KeyCode::Esc, _) => {
            slash::deactivate(app);
            true
        }
        (KeyCode::Backspace, _) => {
            let changed = app.input.textarea_delete_char_before();
            slash::update_query(app);
            changed
        }
        (KeyCode::Char(c), m) if is_printable_text_modifiers(m) => {
            let changed = app.input.textarea_insert_char(c);
            slash::update_query(app);
            changed
        }
        _ => {
            slash::deactivate(app);
            dispatch_key_by_focus(app, key)
        }
    }
}

/// Handle keystrokes while `&` subagent autocomplete dropdown is active.
fn handle_subagent_key(app: &mut App, key: KeyEvent) -> bool {
    match (key.code, key.modifiers) {
        (KeyCode::Up, _) => {
            if app.subagent.as_ref().is_some_and(|subagent| !subagent.query.is_empty()) {
                subagent::move_up(app);
            }
            true
        }
        (KeyCode::Down, _) => {
            if app.subagent.as_ref().is_some_and(|subagent| !subagent.query.is_empty()) {
                subagent::move_down(app);
            }
            true
        }
        (KeyCode::Enter | KeyCode::Tab, _) => {
            if app.subagent.as_ref().is_some_and(|subagent| !subagent.query.is_empty()) {
                subagent::confirm_selection(app);
            }
            true
        }
        (KeyCode::Esc, _) => {
            subagent::deactivate(app);
            true
        }
        (KeyCode::Backspace, _) => {
            let changed = app.input.textarea_delete_char_before();
            subagent::update_query(app);
            changed
        }
        (KeyCode::Char(c), m) if is_printable_text_modifiers(m) => {
            let changed = app.input.textarea_insert_char(c);
            subagent::update_query(app);
            changed
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

        assert!(handled);
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

        assert!(handled);
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
