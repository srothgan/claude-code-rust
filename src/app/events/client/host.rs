// SPDX-License-Identifier: Apache-2.0

use super::super::{App, notices, session};
use crate::agent::events::ClientEvent;
use crate::agent::types::ReloadOutcome;
use crate::app::{PendingCommandAck, SystemSeverity};

fn reload_outcome_phrase(label: &str, outcome: &ReloadOutcome) -> String {
    match outcome {
        ReloadOutcome::Reloaded { names } => format!("Reloaded {label} ({})", names.len()),
        // The notice adds its own full stop, so one the SDK message ends with is not doubled.
        ReloadOutcome::Failed { message } => {
            format!("Could not reload {label}: {}", message.trim_end_matches('.'))
        }
        ReloadOutcome::TimedOut { after_ms } => {
            let mut phrase =
                format!("{label} did not answer within {} s; it may still apply", after_ms / 1000);
            if let Some(first) = phrase.get_mut(..1) {
                first.make_ascii_uppercase();
            }
            phrase
        }
    }
}

/// One line for the whole `/reload`: the plain success wording when both halves reloaded,
/// otherwise each half on its own, so a half that worked is never reported as failed.
fn reload_notice(
    skills: &ReloadOutcome,
    output_styles: &ReloadOutcome,
) -> (SystemSeverity, String) {
    if let (
        ReloadOutcome::Reloaded { names: skill_names },
        ReloadOutcome::Reloaded { names: styles },
    ) = (skills, output_styles)
    {
        return (
            SystemSeverity::Info,
            format!("Reloaded skills ({}) and output styles ({})", skill_names.len(), styles.len()),
        );
    }
    let any_reloaded = [skills, output_styles]
        .iter()
        .any(|outcome| matches!(outcome, ReloadOutcome::Reloaded { .. }));
    let any_failed = [skills, output_styles]
        .iter()
        .any(|outcome| matches!(outcome, ReloadOutcome::Failed { .. }));
    let severity =
        if any_failed && !any_reloaded { SystemSeverity::Error } else { SystemSeverity::Warning };
    let text = format!(
        "{}. {}.",
        reload_outcome_phrase("skills", skills),
        reload_outcome_phrase("output styles", output_styles)
    );
    (severity, text)
}

/// The SDK's own `commands_changed` message refreshes the command inventory;
/// this only acknowledges /reload and says how each half ended.
fn apply_skills_and_styles_reloaded(
    app: &mut App,
    skills: &ReloadOutcome,
    output_styles: &ReloadOutcome,
) {
    release_pending_reload(app);
    let (severity, text) = reload_notice(skills, output_styles);
    notices::emit_system_notice(app, severity, &text);
}

/// The bridge skips a `/reload` that waited `after_ms` behind earlier requests for its session,
/// so nothing will reload later and the input it held is released now.
fn apply_skills_and_styles_reload_expired(app: &mut App, after_ms: u64) {
    release_pending_reload(app);
    let text = format!(
        "Reload did not start within {} s because an earlier request for this session has not finished. Nothing was reloaded; run /reload again later.",
        after_ms / 1000
    );
    notices::emit_system_notice(app, SystemSeverity::Error, &text);
}

fn release_pending_reload(app: &mut App) {
    if matches!(app.turn.pending_command_ack, Some(PendingCommandAck::SkillsAndStylesReload)) {
        session::clear_pending_command(app);
    }
}

pub(super) fn handle(app: &mut App, event: ClientEvent) {
    match event {
        ClientEvent::FooterSaved { request_id, revision, error } => {
            crate::app::config::complete_footer_save(app, &request_id, revision, error);
        }
        ClientEvent::UpdateCheckCompleted { result } => {
            crate::app::update_check::apply_check_result(app, &result);
        }
        ClientEvent::TerminalReleasedToChild { reason, ready_tx, cancel_tx } => {
            app.terminal_lifecycle = crate::app::TerminalLifecycleState::ReleasedToChild(reason);
            app.surface_dirty.clear_for_child_release();
            if app.shutdown_requested() {
                let _ = cancel_tx.send(());
            } else {
                app.terminal_child_cancel = Some(cancel_tx);
            }
            let _ = ready_tx.send(());
        }
        ClientEvent::TerminalReturnedFromChild { reason: _ } => {
            app.terminal_child_cancel = None;
            app.terminal_lifecycle =
                crate::app::TerminalLifecycleState::Running(crate::app::SurfaceMode::Chat);
            app.surface_dirty.terminal_mode = true;
            app.chat_render.clear_measurements();
            app.chat_render.invalidate_live_anchor();
            app.request_chat_visible_rebuild();
        }
        ClientEvent::RuntimeReloadCompleted { session_id: _ } => {
            crate::app::plugins::apply_runtime_reload_success(app);
        }
        ClientEvent::RuntimeReloadHeld { session_id: _, cache_impact } => {
            crate::app::plugins::apply_runtime_reload_held(app, &cache_impact);
            if app.mcp.in_flight {
                app.mcp.in_flight = false;
            }
        }
        ClientEvent::RuntimeReloadFailed { session_id: _, message } => {
            crate::app::plugins::apply_runtime_reload_failure(app, &message);
            if app.mcp.in_flight {
                app.mcp.in_flight = false;
                app.mcp.last_error =
                    Some(format!("Failed to reload MCP server snapshot: {message}"));
            }
        }
        ClientEvent::SkillsAndStylesReloaded { session_id: _, skills, output_styles } => {
            apply_skills_and_styles_reloaded(app, &skills, &output_styles);
        }
        ClientEvent::SkillsAndStylesReloadExpired { session_id: _, after_ms } => {
            apply_skills_and_styles_reload_expired(app, after_ms);
        }
        ClientEvent::UsageRefreshStarted { epoch } => {
            if app.session_runtime.session_scope_epoch == epoch {
                crate::app::usage::apply_refresh_started(app);
            }
        }
        ClientEvent::StructuredUsageReceived { session_id: _, snapshot, error } => {
            crate::app::usage::apply_structured_sdk_result(app, snapshot, error);
        }
        ClientEvent::UsageSnapshotReceived { epoch, snapshot } => {
            if app.session_runtime.session_scope_epoch == epoch {
                crate::app::usage::apply_refresh_success(app, snapshot);
            }
        }
        ClientEvent::UsageRefreshFailed { epoch, message, source } => {
            if app.session_runtime.session_scope_epoch == epoch {
                crate::app::usage::apply_refresh_failure(app, message, source);
            }
        }
        ClientEvent::PluginsInventoryUpdated { cwd_raw, snapshot, claude_path } => {
            if app.cwd_raw == cwd_raw {
                crate::app::plugins::apply_inventory_refresh_success(app, snapshot, claude_path);
            }
        }
        ClientEvent::PluginsInventoryRefreshFailed { cwd_raw, message } => {
            if app.cwd_raw == cwd_raw {
                crate::app::plugins::apply_inventory_refresh_failure(app, message);
            }
        }
        ClientEvent::PluginsCliActionSucceeded { cwd_raw, result } => {
            if app.cwd_raw == cwd_raw {
                crate::app::plugins::apply_cli_action_success(app, result);
            }
        }
        ClientEvent::PluginsCliActionFailed { cwd_raw, message } => {
            if app.cwd_raw == cwd_raw {
                crate::app::plugins::apply_cli_action_failure(app, message);
            }
        }
        _ => unreachable!("client event family routed a non-host event to the host handler"),
    }
}
