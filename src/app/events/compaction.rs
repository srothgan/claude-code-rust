// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use super::super::{ActiveCompaction, App, SystemSeverity};
use crate::agent::model;

pub(super) fn handle_update(app: &mut App, update: model::CompactionUpdate) {
    match update {
        model::CompactionUpdate::Started => handle_started(app),
        model::CompactionUpdate::Boundary(boundary) => handle_boundary(app, boundary),
        model::CompactionUpdate::Finished { result, error_code, error } => {
            handle_finished(app, result, error_code, error.as_deref());
        }
    }
}

fn handle_started(app: &mut App) {
    let was_active = app.turn.compaction.is_active();
    app.turn.compaction.begin();
    tracing::debug!(
        target: crate::logging::targets::APP_SESSION,
        event_name = "compaction_started",
        message = "context compaction started",
        outcome = if was_active { "duplicate" } else { "success" },
    );
}

fn handle_boundary(app: &mut App, boundary: model::CompactionBoundary) {
    let attached_to_active = app.turn.compaction.apply_boundary(boundary);
    app.session_runtime.session_usage.last_compaction_trigger = Some(boundary.trigger);
    app.session_runtime.session_usage.last_compaction_pre_tokens = Some(boundary.pre_tokens);
    app.session_runtime.session_usage.last_compaction_post_tokens = boundary.post_tokens;
    app.session_runtime.session_usage.last_compaction_duration_ms = boundary.duration_ms;
    tracing::debug!(
        target: crate::logging::targets::APP_SESSION,
        event_name = "compaction_boundary_applied",
        message = "context compaction boundary recorded",
        outcome = if attached_to_active { "active" } else { "metadata_only" },
        attached_to_active,
        trigger = ?boundary.trigger,
        pre_tokens = boundary.pre_tokens,
        post_tokens = boundary.post_tokens.unwrap_or_default(),
        duration_ms = boundary.duration_ms.unwrap_or_default(),
    );
}

fn handle_finished(
    app: &mut App,
    result: model::CompactionResult,
    error_code: Option<model::CompactionFailureCode>,
    error: Option<&str>,
) {
    let Some(active) = app.turn.compaction.finish() else {
        tracing::debug!(
            target: crate::logging::targets::APP_SESSION,
            event_name = "compaction_finished",
            message = "context compaction completion ignored without active state",
            outcome = "ignored",
            result = ?result,
        );
        return;
    };

    match result {
        model::CompactionResult::Success => emit_manual_success_if_needed(app, &active),
        model::CompactionResult::Failed => {
            let message = format_failure(error_code, error);
            super::push_system_message_with_severity(app, Some(SystemSeverity::Error), &message);
        }
    }
    crate::app::session_runtime::request_context_usage_refresh(app);
    tracing::debug!(
        target: crate::logging::targets::APP_SESSION,
        event_name = "compaction_finished",
        message = "context compaction finished",
        outcome = match result {
            model::CompactionResult::Success => "success",
            model::CompactionResult::Failed => "failure",
        },
        result = ?result,
        trigger = ?active.trigger(),
        boundary_received = active.boundary().is_some(),
        error_code = ?error_code,
        error_present = error.is_some(),
    );
}

fn format_failure(error_code: Option<model::CompactionFailureCode>, error: Option<&str>) -> String {
    match error_code {
        Some(model::CompactionFailureCode::TooFewGroups) => "Context compaction cannot reduce a single oversized request because there are not enough earlier conversation turns to summarize. Start a new session and retry with a smaller request or a skill that loads less context.".to_owned(),
        Some(model::CompactionFailureCode::Unknown) | None => error.map_or_else(
            || "Context compaction failed.".to_owned(),
            |error| format!("Context compaction failed: {error}"),
        ),
    }
}

pub(super) fn finish_inferred(app: &mut App, emit_manual_success: bool) {
    let Some(active) = app.turn.compaction.finish() else {
        return;
    };
    if emit_manual_success {
        emit_manual_success_if_needed(app, &active);
    }
    tracing::debug!(
        target: crate::logging::targets::APP_SESSION,
        event_name = "compaction_finished",
        message = "context compaction completion inferred from turn lifecycle",
        outcome = "inferred",
        trigger = ?active.trigger(),
        boundary_received = active.boundary().is_some(),
        manual_success_emitted = emit_manual_success
            && matches!(active.trigger(), Some(model::CompactionTrigger::Manual)),
    );
}

pub(super) fn reset(app: &mut App) {
    app.turn.compaction.reset();
}

fn emit_manual_success_if_needed(app: &mut App, active: &ActiveCompaction) {
    if matches!(active.trigger(), Some(model::CompactionTrigger::Manual)) {
        super::push_system_message_with_severity(
            app,
            Some(SystemSeverity::Info),
            "Session successfully compacted.",
        );
    }
}
