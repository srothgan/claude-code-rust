// SPDX-License-Identifier: Apache-2.0
use super::{App, session, turn};
use crate::agent::{
    events::ClientEvent,
    model::{McpServerConnectionStatus, McpServerStatus, McpServerStatusConfig},
};

fn mcp_status_label(status: McpServerConnectionStatus) -> &'static str {
    match status {
        McpServerConnectionStatus::Connected => "connected",
        McpServerConnectionStatus::Failed => "failed",
        McpServerConnectionStatus::NeedsAuth => "needs-auth",
        McpServerConnectionStatus::Pending => "pending",
        McpServerConnectionStatus::Disabled => "disabled",
    }
}

fn mcp_config_diagnostics(
    config: Option<&McpServerStatusConfig>,
) -> (&'static str, Option<u64>, Option<u64>, Option<bool>, usize) {
    match config {
        Some(McpServerStatusConfig::Stdio { timeout, request_timeout_ms, always_load, .. }) => {
            ("stdio", *timeout, *request_timeout_ms, *always_load, 0)
        }
        Some(McpServerStatusConfig::Sse {
            tools,
            timeout,
            request_timeout_ms,
            always_load,
            ..
        }) => ("sse", *timeout, *request_timeout_ms, *always_load, tools.len()),
        Some(McpServerStatusConfig::Http {
            tools,
            timeout,
            request_timeout_ms,
            always_load,
            ..
        }) => ("http", *timeout, *request_timeout_ms, *always_load, tools.len()),
        Some(McpServerStatusConfig::Sdk { .. }) => ("sdk", None, None, None, 0),
        Some(McpServerStatusConfig::ClaudeaiProxy { timeout, .. }) => {
            ("claudeai-proxy", *timeout, None, None, 0)
        }
        Some(McpServerStatusConfig::Unknown { .. }) => ("unknown", None, None, None, 0),
        None => ("missing", None, None, None, 0),
    }
}

fn mcp_server_diagnostic_summaries(servers: &[McpServerStatus]) -> Vec<serde_json::Value> {
    servers
        .iter()
        .map(|server| {
            let (
                config_type,
                timeout_ms,
                request_timeout_ms,
                always_load,
                configured_tool_policy_count,
            ) = mcp_config_diagnostics(server.config.as_ref());
            serde_json::json!({
                "name": server.name,
                "status": mcp_status_label(server.status),
                "config_type": config_type,
                "scope": server.scope,
                "timeout_ms": timeout_ms,
                "request_timeout_ms": request_timeout_ms,
                "always_load": always_load,
                "tool_count": server.tools.len(),
                "configured_tool_policy_count": configured_tool_policy_count,
                "has_error": server.error.as_deref().is_some_and(|error| !error.is_empty()),
                "has_server_info": server.server_info.is_some(),
            })
        })
        .collect()
}

#[allow(clippy::too_many_lines)]
pub fn handle_client_event(app: &mut App, event: ClientEvent) {
    if let Some(event_session_id) = event.scoped_session_id()
        && app.session_runtime.session_id.as_ref().map(crate::agent::model::SessionId::as_str)
            != Some(event_session_id)
    {
        tracing::debug!(
            target: crate::logging::targets::APP_SESSION,
            event_name = "stale_client_event_dropped",
            message = "client event dropped for a stale session",
            outcome = "dropped",
            session_id = %event_session_id,
            active_session_id = app
                .session_runtime
                .session_id
                .as_ref()
                .map_or("<none>", crate::agent::model::SessionId::as_str),
        );
        return;
    }

    app.request_active_surface_repaint();
    match event {
        ClientEvent::SessionUpdate { session_id: _, update } => {
            super::handle_session_update_event(app, update);
        }
        ClientEvent::PermissionRequest { session_id: _, request, response_tx } => {
            turn::handle_permission_request_event(app, request, response_tx);
        }
        ClientEvent::QuestionRequest { session_id: _, request, response_tx } => {
            turn::handle_question_request_event(app, request, response_tx);
        }
        ClientEvent::UserDialogRequest { session_id: _, request, response_tx } => {
            turn::handle_user_dialog_request_event(app, request, response_tx);
        }
        ClientEvent::McpElicitationRequest { session_id: _, request } => {
            crate::app::config::present_mcp_elicitation_request(app, request);
        }
        ClientEvent::McpAuthRedirect { session_id: _, redirect } => {
            crate::app::config::present_mcp_auth_redirect(app, redirect);
        }
        ClientEvent::McpOperationError { session_id: _, error } => {
            crate::app::config::handle_mcp_operation_error(app, &error);
        }
        ClientEvent::McpSetServersResult { session_id: _, result } => {
            crate::app::config::handle_mcp_set_servers_result(app, &result);
        }
        ClientEvent::McpConfigRemoveSucceeded { cwd_raw, server_name, scope, claude_path } => {
            if app.cwd_raw != cwd_raw {
                return;
            }
            crate::app::config::apply_mcp_config_remove_success(
                app,
                &server_name,
                &scope,
                claude_path,
            );
        }
        ClientEvent::McpConfigRemoveFailed { cwd_raw, server_name, scope, message } => {
            if app.cwd_raw != cwd_raw {
                return;
            }
            crate::app::config::apply_mcp_config_remove_failure(
                app,
                &server_name,
                &scope,
                &message,
            );
        }
        ClientEvent::McpElicitationCompleted { session_id: _, elicitation_id, server_name } => {
            crate::app::config::handle_mcp_elicitation_completed(app, &elicitation_id, server_name);
        }
        ClientEvent::TurnCancelled { session_id: _ } => turn::handle_turn_cancelled_event(app),
        ClientEvent::TurnComplete { session_id: _, terminal_reason } => {
            turn::handle_turn_complete_event(app, terminal_reason);
        }
        ClientEvent::TurnError { session_id: _, message, api_error_status, terminal_reason } => {
            turn::handle_turn_error_event(app, &message, None, api_error_status, terminal_reason);
        }
        ClientEvent::TurnErrorClassified {
            session_id: _,
            message,
            class,
            api_error_status,
            terminal_reason,
        } => {
            turn::handle_turn_error_event(
                app,
                &message,
                Some(class),
                api_error_status,
                terminal_reason,
            );
        }
        ClientEvent::Connected {
            session_id,
            cwd,
            current_model,
            available_models,
            mode,
            history_updates,
        } => {
            session::handle_connected_client_event(
                app,
                session_id,
                cwd,
                current_model,
                available_models,
                mode,
                &history_updates,
            );
            crate::app::config::refresh_mcp_snapshot(app);
            crate::app::session_runtime::request_status_snapshot_refresh(app);
            crate::app::session_runtime::request_context_usage_refresh(app);
        }
        ClientEvent::SessionsListed { sessions } => {
            session::handle_sessions_listed_event(app, sessions);
        }
        ClientEvent::AuthRequired { method_name, method_description } => {
            session::handle_auth_required_event(app, method_name, method_description);
        }
        ClientEvent::ConnectionFailed(msg) => {
            session::handle_connection_failed_event(app, &msg);
        }
        ClientEvent::SlashCommandError { session_id: _, message } => {
            session::handle_slash_command_error_event(app, &message);
        }
        ClientEvent::TerminalReleasedToChild { reason } => {
            app.terminal_lifecycle = crate::app::TerminalLifecycleState::ReleasedToChild(reason);
            app.surface_dirty.clear_for_child_release();
        }
        ClientEvent::TerminalReturnedFromChild { reason: _ } => {
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
        ClientEvent::RuntimeReloadFailed { session_id: _, message } => {
            crate::app::plugins::apply_runtime_reload_failure(app, &message);
            if app.mcp.in_flight {
                app.mcp.in_flight = false;
                app.mcp.last_error =
                    Some(format!("Failed to reload MCP server snapshot: {message}"));
            }
        }
        ClientEvent::SessionReplaced {
            session_id,
            cwd,
            current_model,
            available_models,
            mode,
            history_updates,
            restored_input,
        } => {
            session::handle_session_replaced_event(
                app,
                session::SessionReplacedEventData {
                    session_id,
                    cwd,
                    current_model,
                    available_models,
                    mode,
                    history_updates,
                    restored_input,
                },
            );
            crate::app::config::refresh_mcp_snapshot(app);
            crate::app::session_runtime::request_status_snapshot_refresh(app);
            crate::app::session_runtime::request_context_usage_refresh(app);
        }
        ClientEvent::UpdateAvailable { latest_version, current_version } => {
            session::handle_update_available_event(app, &latest_version, &current_version);
        }
        ClientEvent::ServiceStatus { severity, message } => {
            session::handle_service_status_event(app, severity, &message);
        }
        ClientEvent::AuthCompleted { conn } => {
            session::handle_auth_completed_event(app, &conn);
        }
        ClientEvent::LogoutCompleted => {
            session::handle_logout_completed_event(app);
        }
        ClientEvent::StatusSnapshotReceived { session_id, account } => {
            let has_email = account.email.as_deref().is_some_and(|email| !email.trim().is_empty());
            let has_organization = account.organization.is_some();
            let subscription_type = account.subscription_type.clone();
            let token_source = account.token_source.clone();
            let api_key_source = account.api_key_source.clone();
            let api_provider = account.api_provider.clone();
            app.session_runtime.account_info = Some(account);
            app.sync_welcome_snapshot();
            app.request_active_surface_repaint();
            tracing::info!(
                target: crate::logging::targets::APP_AUTH,
                event_name = "status_snapshot_applied",
                message = "status snapshot applied",
                outcome = "success",
                session_id = %session_id,
                has_email,
                has_organization,
                subscription_type = ?subscription_type,
                token_source = ?token_source,
                api_key_source = ?api_key_source,
                api_provider = ?api_provider,
            );
        }
        ClientEvent::ContextUsageReceived { session_id: _, percentage } => {
            crate::app::session_runtime::apply_context_usage_snapshot(app, percentage);
        }
        ClientEvent::RewindTargetsReceived { session_id, targets } => {
            app.sdk_inventory.rewind_targets = targets;
            app.sdk_inventory.rewind_targets_session_id =
                Some(crate::agent::model::SessionId::new(session_id));
            app.sdk_inventory.rewind_targets_in_flight = false;
            crate::app::slash::sync_with_cursor(app);
        }
        ClientEvent::RewindResultReceived { result } => {
            session::handle_rewind_result_event(app, &result);
        }
        ClientEvent::McpSnapshotReceived { session_id, mut servers, source, error } => {
            let pending_dynamic_mcp_removal_confirmation =
                crate::app::config::pending_dynamic_mcp_removal_confirmation_from_snapshot(
                    app,
                    source,
                    error.as_deref(),
                    &servers,
                );
            let remove_confirmation_failures =
                crate::app::config::reconcile_removed_config_mcp_server_guards(
                    app,
                    source,
                    error.as_deref(),
                    &servers,
                );
            crate::app::config::filter_removed_config_mcp_servers(app, &mut servers);
            crate::app::config::filter_stale_plugin_mcp_servers(app, source, &mut servers);
            let server_count = servers.len();
            let error_present = error.is_some();
            let server_diagnostics = mcp_server_diagnostic_summaries(&servers);
            app.mcp.servers = servers;
            app.mcp.in_flight = false;
            app.mcp.last_error = error;
            app.config.mcp_selected_server_index =
                app.config.mcp_selected_server_index.min(app.mcp.servers.len().saturating_sub(1));
            if let Some(overlay) = app.config.mcp_auth_redirect_overlay() {
                let server_name = overlay.redirect.server_name.clone();
                if let Some(server) =
                    app.mcp.servers.iter().find(|server| server.name == server_name)
                    && !matches!(
                        server.status,
                        crate::agent::model::McpServerConnectionStatus::NeedsAuth
                            | crate::agent::model::McpServerConnectionStatus::Pending
                    )
                {
                    if matches!(
                        server.status,
                        crate::agent::model::McpServerConnectionStatus::Connected
                    ) {
                        app.config.status_message =
                            Some(format!("{} authenticated successfully.", server.name));
                        app.config.last_error = None;
                    }
                    app.config.clear_overlay();
                }
            }
            crate::app::config::apply_pending_dynamic_mcp_removal_confirmation(
                app,
                pending_dynamic_mcp_removal_confirmation,
            );
            crate::app::config::apply_removed_config_mcp_server_confirmation_failures(
                app,
                remove_confirmation_failures,
            );
            tracing::info!(
                target: crate::logging::targets::APP_CONFIG,
                event_name = "mcp_snapshot_applied",
                message = "MCP snapshot applied",
                outcome = "success",
                session_id = %session_id,
                source = ?source,
                server_count,
                error_present,
                servers = ?server_diagnostics,
            );
        }
        ClientEvent::UsageRefreshStarted { epoch } => {
            if app.session_runtime.session_scope_epoch != epoch {
                return;
            }
            crate::app::usage::apply_refresh_started(app);
        }
        ClientEvent::UsageSnapshotReceived { epoch, snapshot } => {
            if app.session_runtime.session_scope_epoch != epoch {
                return;
            }
            crate::app::usage::apply_refresh_success(app, snapshot);
            crate::app::usage::emit_pending_limits_success(app);
        }
        ClientEvent::UsageRefreshFailed { epoch, message, source } => {
            if app.session_runtime.session_scope_epoch != epoch {
                return;
            }
            crate::app::usage::apply_refresh_failure(app, message.clone(), source);
            crate::app::usage::emit_pending_limits_failure(app, &message);
        }
        ClientEvent::PluginsInventoryUpdated { cwd_raw, snapshot, claude_path } => {
            if app.cwd_raw != cwd_raw {
                return;
            }
            crate::app::plugins::apply_inventory_refresh_success(app, snapshot, claude_path);
        }
        ClientEvent::PluginsInventoryRefreshFailed { cwd_raw, message } => {
            if app.cwd_raw != cwd_raw {
                return;
            }
            crate::app::plugins::apply_inventory_refresh_failure(app, message);
        }
        ClientEvent::PluginsCliActionSucceeded { cwd_raw, result } => {
            if app.cwd_raw != cwd_raw {
                return;
            }
            crate::app::plugins::apply_cli_action_success(app, result);
        }
        ClientEvent::PluginsCliActionFailed { cwd_raw, message } => {
            if app.cwd_raw != cwd_raw {
                return;
            }
            crate::app::plugins::apply_cli_action_failure(app, message);
        }
        ClientEvent::FatalError(error) => session::handle_fatal_error_event(app, error),
    }
}
