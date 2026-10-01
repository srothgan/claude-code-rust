// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

//! Slash command autocomplete synchronization, dismissal, and completion.

use super::candidates::{
    build_slash_state, builtin_argument_confirmation_closes, detect_slash_at_cursor,
};
use super::{
    CompletionRequest, ResolvedSubmission, SlashContext, SlashDetection, SlashState,
    SubmissionClass,
};
use crate::app::{AUTOCOMPLETE_VISIBLE_ROWS, App, FocusTarget};

fn release_autocomplete_focus_if_idle(app: &mut App) {
    if app.slash.visible().is_none() && app.mention.is_none() && app.subagent.is_none() {
        app.release_focus_target(FocusTarget::Mention);
    }
}

fn replacement_range(slash: &SlashState, chars: &[char]) -> Option<(usize, usize)> {
    match &slash.context {
        SlashContext::CommandName => {
            if slash.trigger_col >= chars.len() {
                tracing::debug!(
                    trigger_col = slash.trigger_col,
                    line_len = chars.len(),
                    "Slash confirm aborted: trigger column out of bounds"
                );
                return None;
            }
            if chars[slash.trigger_col] != '/' {
                tracing::debug!(
                    trigger_col = slash.trigger_col,
                    found = ?chars[slash.trigger_col],
                    "Slash confirm aborted: trigger column is not slash"
                );
                return None;
            }

            let token_end = (slash.trigger_col + 1..chars.len())
                .find(|&i| chars[i].is_whitespace())
                .unwrap_or(chars.len());
            Some((slash.trigger_col, token_end))
        }
        SlashContext::Argument { token_range, .. } => {
            let (start, end) = *token_range;
            if start > end || end > chars.len() {
                tracing::debug!(
                    start,
                    end,
                    line_len = chars.len(),
                    "Slash confirm aborted: invalid argument token range"
                );
                return None;
            }
            Some((start, end))
        }
    }
}

fn request_rewind_targets_for_active_argument(app: &mut App, detection: &SlashDetection) {
    let SlashContext::Argument { command, arg_index, .. } = &detection.context else {
        return;
    };
    if command != "/rewind" || *arg_index != 0 || app.sdk_inventory.rewind_targets_in_flight {
        return;
    }
    let Some(session_id) = app.session_runtime.session_id.clone() else {
        return;
    };
    if app.sdk_inventory.rewind_targets_session_id.as_ref() == Some(&session_id) {
        return;
    }
    let Some(conn) = app.session_runtime.conn.clone() else {
        return;
    };
    let session_id_text = session_id.to_string();
    app.sdk_inventory.rewind_targets_in_flight = true;
    app.sdk_inventory.rewind_targets.clear();
    app.sdk_inventory.rewind_targets_session_id = None;
    app.sdk_inventory.rewind_targets_request_session_id = Some(session_id.clone());
    if let Err(err) = conn.get_rewind_targets(session_id_text.clone()) {
        app.sdk_inventory.rewind_targets_in_flight = false;
        app.sdk_inventory.rewind_targets_request_session_id = None;
        tracing::warn!(
            target: crate::logging::targets::APP_SESSION,
            event_name = "rewind_targets_request_failed",
            message = "failed to request rewind targets",
            outcome = "failure",
            session_id = %session_id_text,
            error_message = %err,
        );
    }
}

pub fn sync_with_cursor(app: &mut App) {
    let detection =
        detect_slash_at_cursor(app.input.lines(), app.input.cursor_row(), app.input.cursor_col());
    let request = app.slash.request_at(detection.as_ref());
    sync_completion(app, detection, request);
}

fn sync_completion(app: &mut App, detection: Option<SlashDetection>, request: CompletionRequest) {
    if app.slash.suppresses(detection.as_ref()) {
        release_autocomplete_focus_if_idle(app);
        return;
    }
    let Some(detection) = detection else {
        deactivate(app);
        return;
    };
    request_rewind_targets_for_active_argument(app, &detection);
    let Some(next_state) = build_slash_state(app, detection, request) else {
        deactivate(app);
        return;
    };

    if let Some(slash) = app.slash.visible_mut() {
        let keep_selection = slash.context == next_state.context;
        let dialog = if keep_selection { slash.dialog } else { super::DialogState::default() };
        *slash = SlashState { dialog, ..next_state };
        slash.dialog.clamp(slash.candidates.len(), AUTOCOMPLETE_VISIBLE_ROWS);
    } else {
        app.slash.show(next_state);
        app.mention = None;
        app.subagent = None;
        app.claim_focus_target(FocusTarget::Mention);
    }
}

pub(crate) fn dismiss(app: &mut App) {
    if let Some(detection) =
        detect_slash_at_cursor(app.input.lines(), app.input.cursor_row(), app.input.cursor_col())
    {
        app.slash.dismiss(detection);
    } else {
        app.slash.clear();
    }
    release_autocomplete_focus_if_idle(app);
}

/// Tab explicitly requests argument completion, including optional SDK hints.
pub(crate) fn request_completion(app: &mut App) -> bool {
    app.slash.clear();
    let detection =
        detect_slash_at_cursor(app.input.lines(), app.input.cursor_row(), app.input.cursor_col());
    sync_completion(app, detection, CompletionRequest::Arguments);
    if !app.slash.is_visible() {
        return false;
    }
    confirm_selection(app);
    true
}

fn deactivate(app: &mut App) {
    app.slash.clear();
    release_autocomplete_focus_if_idle(app);
}

pub fn move_up(app: &mut App) {
    if let Some(slash) = app.slash.visible_mut() {
        slash.dialog.move_up(slash.candidates.len(), AUTOCOMPLETE_VISIBLE_ROWS);
    }
}

pub fn move_down(app: &mut App) {
    if let Some(slash) = app.slash.visible_mut() {
        slash.dialog.move_down(slash.candidates.len(), AUTOCOMPLETE_VISIBLE_ROWS);
    }
}

/// Complete the selected token before Enter uses the normal deferred submit path.
/// App-owned commands use their authoritative submission classification; SDK
/// argument hints guide completion but leave validation to the active session.
pub fn prepare_submit(app: &mut App) -> bool {
    let Some(slash) = app.slash.visible() else {
        return true;
    };
    if slash.candidates.is_empty() {
        deactivate(app);
        return true;
    }

    // An empty argument token is a suggestion, not user input. In particular,
    // /resume must open its picker instead of silently selecting a recent ID.
    let submission = ResolvedSubmission::resolve(app.input.text());
    if matches!(slash.context, SlashContext::Argument { .. })
        && slash.query.is_empty()
        && submission.class() != SubmissionClass::Invalid
    {
        deactivate(app);
        return true;
    }

    confirm_selection(app);
    let lines = app.input.lines();
    let row = app.input.cursor_row();
    let col = app.input.cursor_col();
    let has_remaining_text = lines[row].chars().skip(col).any(|ch| !ch.is_whitespace())
        || lines[row + 1..].iter().any(|line| !line.trim().is_empty());
    if !has_remaining_text
        && ResolvedSubmission::resolve(app.input.text()).class() == SubmissionClass::Invalid
    {
        // Continue at the end of the draft even when completion reused existing
        // trailing whitespace. Invalid suffixes instead reach normal validation.
        let last_row = lines.len() - 1;
        let last_col = lines[last_row].chars().count();
        app.input.set_cursor(last_row, last_col);
        sync_with_cursor(app);
        return false;
    }
    deactivate(app);
    true
}

/// Confirm selected candidate in input without submitting it.
pub fn confirm_selection(app: &mut App) {
    // Opaque SDK hints are information, not selectable candidates. Tab keeps
    // them visible; Enter independently continues through prepare_submit.
    if app.slash.visible().is_some_and(|slash| slash.candidates.is_empty()) {
        return;
    }
    let Some(slash) = app.slash.take_visible() else {
        return;
    };

    let Some(candidate) = slash.candidates.get(slash.dialog.selected) else {
        release_autocomplete_focus_if_idle(app);
        return;
    };

    let mut lines = app.input.lines().to_vec();
    let Some(line) = lines.get(slash.trigger_row) else {
        tracing::debug!(
            trigger_row = slash.trigger_row,
            line_count = app.input.lines().len(),
            "Slash confirm aborted: trigger row out of bounds"
        );
        release_autocomplete_focus_if_idle(app);
        return;
    };

    let chars: Vec<char> = line.chars().collect();
    let closes_after_confirmation = match &slash.context {
        SlashContext::Argument { command, arg_index, .. } => {
            builtin_argument_confirmation_closes(command, *arg_index)
        }
        SlashContext::CommandName => false,
    };
    let Some((replace_start, replace_end)) = replacement_range(&slash, &chars) else {
        release_autocomplete_focus_if_idle(app);
        return;
    };

    let before: String = chars[..replace_start].iter().collect();
    let after: String = chars[replace_end..].iter().collect();
    let replacement = if after.is_empty() {
        format!("{} ", candidate.insert_value)
    } else {
        candidate.insert_value.clone()
    };
    let new_line = format!("{before}{replacement}{after}");
    // Reuse an existing separator and continue in argument context, rather
    // than reopening command-name completion at the end of the replaced token.
    let separator = usize::from(after.chars().next().is_some_and(char::is_whitespace));
    let new_cursor_col = replace_start + replacement.chars().count() + separator;
    let new_line_len = new_line.chars().count();
    if new_cursor_col > new_line_len {
        tracing::warn!(
            cursor_col = new_cursor_col,
            line_len = new_line_len,
            "Slash confirm produced cursor beyond line length; clamping"
        );
    }
    lines[slash.trigger_row] = new_line;
    app.input.replace_lines_and_cursor(lines, slash.trigger_row, new_cursor_col.min(new_line_len));

    if closes_after_confirmation {
        dismiss(app);
    } else {
        sync_with_cursor(app);
    }
    release_autocomplete_focus_if_idle(app);
}
